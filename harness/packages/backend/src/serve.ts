//! The bench side of a TUI: one `serve` per client connection to the bench daemon (./daemon),
//! speaking ./wire over that connection's socket. The agents live in the daemon's one backend, so
//! the connection closing disposes this client's VIEWS (`dispose` below), never the agents: a
//! running turn survives a quit or a dropped connection and a reconnecting client finds it still going
//! (`session.open` reports `busy`).
import type { Backend, SessionHandle, ToolSpec } from "./index.ts";
import { Peer } from "./wire.ts";

const METHODS = new Set([
  "prompt", "steer", "followUp", "clearQueue", "btw", "abort",
  "setModel", "setThinkingLevel", "setAutoCompactionEnabled", "setCodemode",
]);

/** pi's `message_update` carries the WHOLE partial message per token, twice (`message` and
 * `assistantMessageEvent.partial`, pi-agent-core agent-loop.js): forwarded as is, a long answer or
 * tool call is quadratic on the wire (2026-10-11: 322 MB to one laptop in three minutes, queued in
 * the daemon's socket buffer until the bench sandbox was OOM-killed mid-turn, twice). So the wire
 * carries only the token: text and thinking deltas (a block's start is an empty one), and a tool
 * call's header so content indexes stay true. Tool-call arguments are not streamed: nothing renders
 * them before `tool_execution_start`, and `message_end` carries the finished message. A connection
 * that joins mid-message gets that message whole once, then deltas. remote.ts rebuilds it. */
function delta(e: any): unknown {
  const a = e.assistantMessageEvent ?? {};
  const i = a.contentIndex;
  const [kind, step] = String(a.type).split("_");
  if ((kind === "text" || kind === "thinking") && step !== "end") return { type: "message_update", i, kind, delta: a.delta ?? "" };
  if (a.type === "toolcall_start") {
    const { id, name } = e.message.content[i] ?? {};
    return { type: "message_update", i, kind: "toolCall", id, name };
  }
  return undefined; // a block's end adds nothing its deltas did not
}

export function serve(backend: Backend, peer: Peer) {
  const open = new Map<string, SessionHandle>();

  peer.handle("hello", () => backend.hello());

  peer.handle("session.open", async ({ key, tools, ...o }: { key: string; tools: ToolSpec[] } & any) => {
    // a tool without an object schema would be declared to the model with no usable parameters
    // (a drill client sent `parameters` and lost all 53)
    const bad = tools.find((t: ToolSpec) => (t?.inputSchema as any)?.type !== "object");
    if (bad) throw new Error(`tool ${bad?.name} needs an inputSchema of type "object"`);
    await open.get(key)?.dispose();
    open.delete(key);
    const h = await backend.session(key, {
      ...o,
      tools: tools.map((t: ToolSpec) => ({ ...t, run: (input: unknown) => peer.request<string>("tool", { key, name: t.name, input }) })),
      client: true,
    });
    open.set(key, h);
    let streaming = false; // this connection has the in-progress message: deltas suffice
    h.subscribe((event: any) => {
      if (event.type === "message_start") streaming = event.message?.role === "assistant";
      else if (event.type === "message_end") streaming = false;
      if (event.type !== "message_update") return peer.emit("session", key, event);
      if (!streaming) return (streaming = true), peer.emit("session", key, { type: "message_update", message: event.message });
      const d = delta(event);
      if (d) peer.emit("session", key, d);
    });
    return { messages: h.messages, busy: h.busy, state: h.state };
  });

  peer.handle("session.call", async ({ key, method, args }: { key: string; method: string; args: unknown[] }) => {
    const h = open.get(key);
    if (!h) throw new Error(`no session ${key}`);
    if (method === "dispose") {
      open.delete(key);
      return h.dispose();
    }
    if (!METHODS.has(method)) throw new Error(`method not allowed: ${method}`);
    return (h as any)[method](...args);
  });

  for (const [group, ops] of [
    ["sessions", backend.sessions],
    ["fs", backend.fs],
    ["podfs", backend.podfs],
  ] as const)
    for (const [name, fn] of Object.entries(ops)) {
      if (group === "sessions" && name === "watch") continue; // a stream, not a call: below
      peer.handle(`${group}.${name}`, (args: unknown[]) => (fn as any)(...args));
    }

  // One watch per connection, however often the client asks; the list goes out as an event.
  let unwatch: (() => void) | undefined;
  peer.handle("sessions.watch", async () => {
    unwatch?.();
    unwatch = await backend.sessions.watch((list) => peer.emit("sessions", "*", list));
    return true;
  });

  // The bench stream: cards and permission mode, one per connection like sessions.watch.
  let unbench: (() => void) | undefined;
  peer.handle("watch", async () => {
    unbench?.();
    unbench = await backend.watch((e) => peer.emit("bench", "*", e));
    return true;
  });
  peer.handle("ask.answer", ({ id, choice }) => backend.asks.answer(id, choice));
  peer.handle("mode.set", ({ mode }) => backend.mode.set(mode));

  peer.handle("settings.write", (patch) => backend.settings.write(patch));
  peer.handle("space", () => backend.space());
  peer.handle("mcp", (msg) => {
    if (!backend.mcp) throw new Error("this bench serves no MCP");
    return backend.mcp(msg);
  });
  peer.handle("models.refresh", () => backend.models.refresh());
  peer.handle("auth.providers", () => backend.auth.providers());
  peer.handle("auth.login", ({ lid, provider, type }, signal) =>
    backend.auth.login(provider, type, {
      signal,
      prompt: (prompt) => peer.request<string>("auth.prompt", { lid, prompt }, signal),
      notify: (event) => peer.emit("auth", String(lid), event),
    }),
  );

  return {
    async dispose() {
      unwatch?.();
      unbench?.();
      await Promise.allSettled([...open.values()].map((h) => h.dispose()));
      open.clear();
    },
  };
}
