import { afterEach, beforeEach, expect, test } from "bun:test";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { anyPodTools, podGet, podHold, inflight, podTools, touched } from "./pod.ts";

const realFetch = globalThis.fetch;
let log: { url: string; auth: string | null }[] = [];
let handler: (url: string, init: any) => Response | Promise<Response>;

beforeEach(() => {
  log = [];
  const f = join(mkdtempSync(join(tmpdir(), "kl-tok-")), "t");
  writeFileSync(f, "bench");
  process.env.KL_API_URL = "http://api.test";
  process.env.KL_TOOL_TOKEN_FILE = f;
  globalThis.fetch = (async (url: string, init: any = {}) => {
    log.push({ url: String(url), auth: init.headers?.authorization ?? null });
    return handler(String(url), init);
  }) as any;
});
afterEach(() => {
  globalThis.fetch = realFetch;
});

const j = (v: unknown, status = 200) => new Response(JSON.stringify(v), { status });

test("schema comes from the pod, only for fixed names, token is sent", async () => {
  handler = (u) =>
    u.endsWith("/v1/workspaces/w1/tools") ? j({ address: "10.0.0.5:7788", token: "pt" })
    : u.endsWith("/tools") ? j({ tools: [{ name: "read", description: "Read a file", schema: { type: "object", properties: { path: {} } } }, { name: "bogus", schema: {} }] })
    : j({ ok: 1 });
  const t = await podTools("w1");
  expect(t.map((x) => x.name)).toEqual(["read"]);
  expect(t[0]!.description).toContain("Read a file");
  expect((t[0]!.inputSchema as any).properties.path).toBeDefined();
  expect(log.find((l) => l.url === "http://10.0.0.5:7788/tools")!.auth).toBe("Bearer pt");
  expect(await t[0]!.run({ path: "a" })).toBe('{\n  "ok": 1\n}');
  expect(log.at(-1)!.auth).toBe("Bearer pt");
});

test("pod not ready falls back to the permissive schema and call says not ready", async () => {
  handler = () => j({ error: "workspace not ready" }, 409);
  const t = await podTools("w1");
  expect(t.length).toBe(23);
  expect(t[0]!.inputSchema).toEqual({ type: "object", additionalProperties: true });
  await expect(t[0]!.run({})).rejects.toThrow("workspace not ready");
});

test("a network error re-fetches the address once and retries once", async () => {
  let addr = 0;
  handler = (u) => {
    if (u.endsWith("/v1/workspaces/w1/tools")) return j({ address: `10.0.0.${++addr}:7788` });
    if (u.endsWith("/tools")) return j({ tools: [] });
    if (u.startsWith("http://10.0.0.1")) throw new TypeError("connection refused");
    return j({ done: true });
  };
  const t = await podTools("w1");
  expect(await t.find((x) => x.name === "exec")!.run({ cmd: "ls" })).toContain("done");
  expect(addr).toBe(2);
  // a second failure is not retried again
  handler = () => {
    throw new TypeError("down");
  };
  await expect(t[0]!.run({})).rejects.toThrow("down");
});

test("a timeout is never resent: the pod may still be running the first call", async () => {
  let posts = 0;
  handler = (u) => {
    if (u.endsWith("/v1/workspaces/w1/tools")) return j({ address: "10.0.0.1:7788" });
    if (u.endsWith("/tools")) return j({ tools: [] });
    posts++;
    throw new DOMException("The operation timed out.", "TimeoutError");
  };
  const t = await podTools("w1");
  await expect(t.find((x) => x.name === "exec")!.run({ cmd: "go test" })).rejects.toThrow("exec: The operation timed out.");
  expect(posts).toBe(1);
});

test("a 4xx from a pod tool throws with tool name and status; a non-zero exit stays a result", async () => {
  handler = (u) => {
    if (u.endsWith("/v1/workspaces/w1/tools")) return j({ address: "10.0.0.1:7788" });
    if (u.endsWith("/tools")) return j({ tools: [] });
    if (u.endsWith("/tools/read")) return j({ error: "no such file" }, 404);
    return j({ exit_code: 3, stdout: "", stderr: "boom" });
  };
  const t = await podTools("w1");
  await expect(t.find((x) => x.name === "read")!.run({ path: "x" })).rejects.toThrow('read 404: no such file');
  expect(await t.find((x) => x.name === "exec")!.run({ cmd: "false" })).toContain('"exit_code": 3');
});

test("anyPodTools: schemas from the sample, each call reaches the named workspace, a stopped one says not ready", async () => {
  handler = (u) =>
    u.endsWith("/v1/workspaces/w1/tools") ? j({ address: "10.0.0.1:7788" })
    : u.endsWith("/v1/workspaces/w2/tools") ? j({ address: "10.0.0.2:7788" })
    : u.endsWith("/v1/workspaces/off/tools") ? j({ error: "workspace not ready" }, 409)
    : u.endsWith("/v1/workspaces/off") ? j({ state: "stopped" })
    : u.endsWith(":7788/tools") ? j({ tools: [{ name: "read", description: "Read a file", schema: { type: "object", properties: { path: {} }, required: ["path"] } }] })
    : j({ at: u });
  const t = await anyPodTools("w1");
  expect(t.length).toBe(23);
  const read = t.find((x) => x.name === "read")!;
  expect(read.inputSchema).toEqual({ type: "object", properties: { workspace: { type: "string", description: "Workspace id" }, path: {} }, required: ["workspace", "path"] });
  expect(read.description).toBe("Read a file Runs in the workspace named by `workspace`.");
  expect(await read.run({ workspace: "w2", path: "a" })).toContain("http://10.0.0.2:7788/tools/read");
  const posted = log.filter((l) => l.url.endsWith("/tools/read")).map((l) => l.url);
  expect(posted).toEqual(["http://10.0.0.2:7788/tools/read"]);
  await expect(read.run({ workspace: "off", path: "a" })).rejects.toThrow("stopped; start it with `workspace_start`");
  await expect(read.run({ path: "a" })).rejects.toThrow("workspace is required");
});

test("podFetch marks itself a watcher", async () => {
  let seen: any;
  handler = (u, init) => {
    if (u.endsWith("/tools")) return j({ address: "10.0.0.5:7788" });
    seen = init.headers;
    return j({ ok: 1 });
  };
  await podGet("w1", "/healthz");
  expect(seen["x-kl-watch"]).toBe("1");
});

test("a tool call that hits NotReady wakes and retries once", async () => {
  let lookups = 0;
  handler = (u) => {
    if (u.endsWith("/v1/workspaces/w1/tools")) return ++lookups <= 2 ? j({ error: "idle" }, 409) : j({ address: "10.0.0.5:7788" });
    return j({ ok: 1 });
  };
  let woke = 0;
  const t = await podTools("w1", { ensure: (async () => void woke++) as any });
  expect(await t[0]!.run({})).toBe('{\n  "ok": 1\n}');
  expect(woke).toBe(1);
  expect(touched.has("w1")).toBe(true);
});

test("podHold never wakes", async () => {
  handler = () => j({ error: "idle" }, 409);
  await podHold("w1");
  expect(log.some((l) => l.url.includes("/hold"))).toBe(false);
});

test("podHold posts /hold to a ready workspace", async () => {
  handler = (u) => (u.endsWith("/tools") && u.includes("/v1/") ? j({ address: "10.0.0.5:7788" }) : new Response(null, { status: 204 }));
  await podHold("w1");
  expect(log.at(-1)!.url).toBe("http://10.0.0.5:7788/hold");
});

test("a second NotReady after the wake throws: one retry only", async () => {
  handler = () => j({ error: "idle" }, 409);
  let woke = 0;
  const t = await podTools("w1", { ensure: (async () => void woke++) as any });
  await expect(t[0]!.run({})).rejects.toThrow("workspace not ready");
  expect(woke).toBe(1);
});

test("an ensure failure surfaces instead of the NotReady", async () => {
  handler = () => j({ error: "idle" }, 409);
  const t = await podTools("w1", { ensure: (async () => { throw new Error("w1 is stopped; start it with `workspace_start`"); }) as any });
  await expect(t[0]!.run({})).rejects.toThrow("workspace_start");
});

test("call() is activity: no x-kl-watch header, and inflight returns to empty", async () => {
  let seen: any;
  handler = (u, init) => {
    if (u.endsWith("/v1/workspaces/w1/tools")) return j({ address: "10.0.0.5:7788" });
    if (u.includes("/tools/")) seen = init.headers;
    return j({ ok: 1 });
  };
  const t = await podTools("w1");
  await t[0]!.run({});
  expect(seen["x-kl-watch"]).toBeUndefined();
  expect(inflight.has("w1")).toBe(false);
});
