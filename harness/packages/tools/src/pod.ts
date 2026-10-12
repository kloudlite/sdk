//! The workspace pod's code tools (crates/ide `kl ide serve`, port 7788), reached through the
//! platform: GET /v1/workspaces/{id}/tools gives {address, token?}, then plain HTTP to the pod.
//! The names are a FIXED list so the tools exist even while the pod is not Ready; the pod's own
//! description/schema replace the permissive fallback when it answers at construction.
import type { ToolDef } from "./index.ts";
import { api, apiJson } from "./platform.ts";
import { ensureRunning } from "./ensure.ts";

export const POD_TOOLS = [
  "read", "write", "edit", "patch", "glob", "grep", "exec",
  "process_list", "process_output", "process_write", "process_kill",
  "watch", "watch_poll", "watch_stop",
  "graft_find_code", "graft_find_all", "graft_trace_calls", "graft_file_api", "graft_repo_map", "graft_build", "graft_blast",
  "container_build", "container_push",
];

type Addr = { address: string; token?: string };
type Listed = { name: string; description?: string; schema?: Record<string, unknown> };

/** What the tool server says stands between exec and the world (`fence` in `GET /tools`). */
export type PodFence = { sandbox?: string; network?: string };
const fences = new Map<string, PodFence>();
/** The fence `podTools(ws)` last read; undefined when the server is older or was unreachable. */
export const podFence = (ws: string): PodFence | undefined => fences.get(ws);

/** Last call per workspace: the hold beat (daemon.ts) pings only workspaces a running turn has used. */
export const touched = new Map<string, number>();
/** Calls in flight per workspace: a Claude Code session has no turn in the backend, but its call is running. */
export const inflight = new Map<string, number>();

class NotReady extends Error {}

async function lookup(ws: string): Promise<Addr> {
  // api throws on a non-2xx answer; for these callers that is "not ready", not a crash
  const out = await api("GET", `/v1/workspaces/${encodeURIComponent(ws)}/tools`).catch((e) => {
    throw new NotReady(String(e?.message ?? e).replace(/^\d+: /, ""));
  });
  const a = JSON.parse(out);
  if (!a?.address) throw new NotReady("no address");
  return a;
}

/** A ready workspace's tool server, for reads that are not model tool calls (sidebar). One
 * lookup per call: the address and token rotate, and a sidebar beat is 5 s apart. */
async function podFetch<T>(ws: string, path: string, init: RequestInit, ms: number, text = false): Promise<T> {
  const a = await apiJson<Addr>("GET", `/v1/workspaces/${encodeURIComponent(ws)}/tools`);
  if (!a?.address) throw new Error("no address");
  const res = await fetch(`http://${a.address}${path}`, { ...init, headers: { ...(init.headers as object), ...headers(a), "x-kl-watch": "1" }, signal: AbortSignal.timeout(ms) });
  if (!res.ok) throw new Error(`error ${res.status}`); // body withheld: it may echo the token
  return (text ? await res.text() : await res.json()) as T;
}
export const podGet = <T>(ws: string, path: string, ms = 5000) => podFetch<T>(ws, path, {}, ms);
/** A pod file's bytes as text (`/fs/file`). */
export const podText = (ws: string, path: string, ms = 10_000) => podFetch<string>(ws, path, {}, ms, true);
export const podPost = <T>(ws: string, tool: string, args: unknown, ms = 5000) =>
  podFetch<T>(ws, `/tools/${tool}`, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(args ?? {}) }, ms);

const headers = (a: Addr): Record<string, string> => (a.token ? { authorization: `Bearer ${a.token}` } : {});

export async function podTools(ws: string, o: { ensure?: typeof ensureRunning } = {}): Promise<ToolDef[]> {
  let cache: Addr | undefined;
  const addr = async () => (cache ??= await lookup(ws));

  let listed = new Map<string, Listed>();
  fences.delete(ws);
  try {
    const a = await addr();
    const res = await fetch(`http://${a.address}/tools`, { headers: headers(a), signal: AbortSignal.timeout(5000) });
    if (res.ok) {
      const body = (await res.json()) as { tools: Listed[]; fence?: PodFence };
      for (const t of body.tools) listed.set(t.name, t);
      if (body.fence) fences.set(ws, body.fence);
    }
  } catch {
    cache = undefined;
  }

  async function post(name: string, args: any, timeout: number): Promise<Response> {
    const a = await addr();
    return fetch(`http://${a.address}/tools/${name}`, {
      method: "POST",
      headers: { "content-type": "application/json", ...headers(a) },
      body: JSON.stringify(args ?? {}),
      signal: AbortSignal.timeout(timeout),
    });
  }

  async function call(name: string, args: any): Promise<string> {
    // A build waits on the builder and the registry: same long leash as a job, its own default.
    const timeout = name === "exec" ? (args?.timeout_ms ?? 120_000) + 15_000
      : name === "container_build" ? (args?.timeout_ms ?? 600_000) + 15_000
      : name === "container_push" ? 180_000 : 60_000;
    try {
      let res: Response | undefined;
      touched.set(ws, Date.now());
      inflight.set(ws, (inflight.get(ws) ?? 0) + 1);
      // a stale address (pod restarted, token rotated) shows as a network error or 401: re-resolve once.
      // Never after our own timeout: the pod got the call and may still be running it, so a resend
      // runs a second `go test` beside the first (2026-10-10: one exec became nine minutes).
      let woke = false;
      for (let attempt = 0; attempt < 2; attempt++) {
        try {
          res = await post(name, args, timeout);
          if (res.status !== 401 || attempt) break;
        } catch (e: any) {
          // an idle-stopped workspace has no address: wake it once (a person's stop throws from ensure) and go again
          if (e instanceof NotReady && !woke) {
            woke = true;
            await (o.ensure ?? ensureRunning)("workspaces", ws);
            attempt--;
          } else if (e instanceof NotReady || attempt || e?.name === "TimeoutError") throw e;
        }
        cache = undefined;
      }
      const text = await res!.text();
      if (res!.ok) {
        try {
          return JSON.stringify(JSON.parse(text), null, 2);
        } catch {
          return text;
        }
      }
      let msg = text;
      try {
        msg = JSON.parse(text).error ?? text;
      } catch {}
      throw new Error(`${name} ${res!.status}: ${msg}`);
    } catch (e: any) {
      // a failed call throws (a codemode script stops, a direct call shows as failed); a command that ran and exited non-zero is a 200 and stays a result
      if (e instanceof NotReady) throw new Error(`${name}: workspace not ready: ${e.message}`);
      throw e?.message?.startsWith(`${name} `) ? e : new Error(`${name}: ${e?.message ?? e}`);
    } finally {
      const n = (inflight.get(ws) ?? 1) - 1;
      n > 0 ? inflight.set(ws, n) : inflight.delete(ws);
    }
  }

  return POD_TOOLS.filter((n) => listed.size === 0 || listed.has(n)).map((name) => {
    const t = listed.get(name);
    return {
      name,
      description: t?.description ? `${t.description} (This tool runs in workspace ${ws}.)` : `This tool runs in workspace ${ws}.`,
      inputSchema: t?.schema ?? { type: "object", additionalProperties: true },
      run: (args: any) => call(name, args),
    };
  });
}

/** Keeps a READY workspace from idling while a turn that needs it runs elsewhere. Never wakes:
 * a stopped workspace stays stopped until a real call needs it. Errors are dropped. */
export async function podHold(ws: string): Promise<void> {
  try {
    const a = await lookup(ws);
    await fetch(`http://${a.address}/hold`, { method: "POST", headers: headers(a), signal: AbortSignal.timeout(5_000) });
  } catch {}
}

export type ExecResult = { code: number; stdout: string; stderr: string };

/** One shell job in workspace `ws` through its tool server (same lookup as `podTools`, fresh each
 * call: callers are the delegate's few git steps, not a hot path). A transport or non-2xx failure
 * comes back as code -1 with the reason in stderr, never a throw. The token stays in the header. */
export async function podExec(ws: string, cmd: string, timeoutMs = 120_000): Promise<ExecResult> {
  try {
    const a = await lookup(ws);
    const res = await fetch(`http://${a.address}/tools/exec`, {
      method: "POST",
      headers: { "content-type": "application/json", ...headers(a) },
      body: JSON.stringify({ cmd, timeout_ms: timeoutMs }),
      signal: AbortSignal.timeout(timeoutMs + 15_000),
    });
    const text = await res.text();
    if (!res.ok) return { code: -1, stdout: "", stderr: `exec ${res.status}: ${text}` };
    const j = JSON.parse(text);
    return { code: j.exit_code ?? -1, stdout: j.stdout ?? "", stderr: j.stderr ?? "" };
  } catch (e: any) {
    return { code: -1, stdout: "", stderr: e instanceof NotReady ? `workspace not ready: ${e.message}` : `${e?.message ?? e}` };
  }
}

/**
 * The pod tools for ANY workspace (the laptop MCP door, backend/src/mcp.ts): each takes a required
 * `workspace` and runs through that workspace's own `podTools`. Schemas come from `sample` (a running
 * workspace) when it answers; a workspace's tools are built once and kept, so its address is
 * resolved on first use and re-resolved by `podTools` itself.
 */
export async function anyPodTools(sample?: string): Promise<ToolDef[]> {
  const shape = new Map((sample ? await podTools(sample) : []).map((t) => [t.name, t]));
  const per = new Map<string, Promise<ToolDef[]>>();
  return POD_TOOLS.map((name) => {
    const s: any = shape.get(name)?.inputSchema ?? { type: "object", additionalProperties: true };
    const description = (shape.get(name)?.description ?? "").replace(/ ?\(This tool runs in workspace [^)]*\.\)$/, "");
    return {
      name,
      description: `${description} Runs in the workspace named by \`workspace\`.`.trim(),
      inputSchema: { ...s, type: "object", properties: { workspace: { type: "string", description: "Workspace id" }, ...s.properties }, required: ["workspace", ...(s.required ?? []).filter((n: string) => n !== "workspace")] },
      run: async (input: any) => {
        const { workspace, ...args } = input ?? {};
        if (typeof workspace !== "string" || !workspace) throw new Error(`${name}: workspace is required`);
        let tools = per.get(workspace);
        if (!tools) per.set(workspace, (tools = podTools(workspace)));
        const t = (await tools).find((t) => t.name === name);
        if (!t) throw new Error(`${name}: workspace ${workspace} does not serve this tool`);
        return t.run(args);
      },
    };
  });
}
