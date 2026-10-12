import { expect, jest, test } from "bun:test";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { LocalBackend, DESTRUCTIVE, asking, baseHandle, ROUTE_TIMEOUT_MS, mustAsk, roleCard, stripCard, shareable, installGate, mcpTools, registryFor, sessionCwd, sessionKind, seen } from "./local.ts";
import { toolDiff } from "./diff.ts";
import { Cards } from "./cards.ts";
import { PROTOCOL } from "./wire.ts";
import { addTask, readBoards, readTasks, tasksFile, updateTask } from "./tasks.ts";
import { POD_TOOLS, platformTools } from "@kloudlite-tui/tools";

test("hello carries what the TUI reads at boot", async () => {
  const h = await new LocalBackend().hello();
  expect(h.protocol).toBe(PROTOCOL);
  expect(h.cwd).toBe(process.cwd());
  expect(h.tools).toContain("workspace_create");
  expect(h.tools).toContain("workspace_ask");
  expect(h.tools).toContain("bash");
  expect(h.tools.slice(0, 2)).toEqual(["web_fetch", "web_search"]);
  expect(h.catalog.length).toBeGreaterThan(0);
  expect(h.catalog[0]).toHaveProperty("input");
  expect(h.defaultModel).toHaveProperty("provider");
});

test("settings write lands in the next hello", async () => {
  const b = new LocalBackend();
  await b.settings.write({ theme: "kloudlite-light" });
  expect((await b.hello()).settings.theme).toBe("kloudlite-light");
});

test("fs wraps git.ts", async () => {
  const b = new LocalBackend();
  expect(typeof (await b.fs.isGitRepo(process.cwd()))).toBe("boolean");
  expect(Array.isArray(await b.fs.listDir(process.cwd(), ""))).toBe(true);
});

test("only destructive house actions ask", () => {
  const destructive = ["workspace_delete", "worktree_drop", "env_delete", "env_restore", "service_remove", "volume_delete", "snapshot_delete", "repo_delete", "branch_delete", "image_tag_delete", "image_delete"];
  expect(DESTRUCTIVE.size).toBe(11);
  for (const n of destructive) expect(mustAsk(n)).toBe(true);
  for (const n of ["write", "edit", "patch", "read", "exec", "bash", "web_fetch", "workspace_stop", "env_stop", "container_build", "container_push", "packages_remove", "service_update", "intercept"]) expect(mustAsk(n)).toBe(false);
});

test("the repo and image deletes carry a required because", () => {
  for (const n of ["repo_delete", "branch_delete", "image_tag_delete", "image_delete"]) {
    const t = platformTools("main").find((x) => x.name === n)!;
    const asked: any = asking(t).inputSchema;
    expect(asked.required).toContain("because");
    expect(asked.properties.because).toBeDefined();
  }
});

test("pod edit ({old,new}) has no local diff; pi edit still does", () => {
  expect(toolDiff("edit", { path: "a", edits: [{ old: "x", new: "y" }] })).toBeNull();
});

test("sessionKind", () => {
  expect(sessionKind("main")).toEqual({ kind: "main" });
  expect(sessionKind("main:x")).toEqual({ kind: "main" });
  expect(sessionKind("ws1")).toEqual({ kind: "workspace", ws: "ws1" });
  expect(sessionKind("ws1:x")).toEqual({ kind: "workspace", ws: "ws1" });
  expect(sessionKind("ws1:agent-ab12")).toEqual({ kind: "subagent", ws: "ws1" });
});

test("workspace sessions run in the pod's folder, main does not", () => {
  expect(sessionCwd(sessionKind("ws1"))).toBe("/home/kl/workspace");
  expect(sessionCwd(sessionKind("ws1:agent-ab12"))).toBe("/home/kl/workspace");
  expect(sessionCwd(sessionKind("main"))).toBeUndefined();
});

test("registry per session kind", async () => {
  delete process.env.KL_API_URL; // pod tools fall back to the fixed names
  const deps = { live: new Map(), busy: new Set<string>(), open: async () => null as never, permit: async () => ({}), cards: new Cards(() => {}) };
  const opts: any = { tools: [] };
  const main = (await registryFor({ kind: "main" }, deps, opts)).names();
  expect(main).toContain("workspace_create");
  expect(main).toContain("workspace_ask");
  expect(main).toContain("question");
  for (const n of ["bash", "read", "write"]) expect(main).toContain(n);
  expect(main).not.toContain("exec");
  const wsReg = await registryFor({ kind: "workspace", ws: "w1" }, deps, opts);
  const ws = wsReg.names();
  expect(ws).toContain("exec");
  expect(ws).toContain("packages_add");
  expect(ws).not.toContain("workspace_create");
  expect(ws).not.toContain("workspace_ask");
  expect(wsReg.get("read").description).not.toContain("scratch");
  expect(ws).toContain("main_tell");
  expect((wsReg.get("packages_add").inputSchema as any).properties.workspace).toBeUndefined();
  // a stop never asks, so it carries no card fields; main's still names the workspace
  const stop: any = wsReg.get("workspace_stop").inputSchema;
  expect(stop.properties.because).toBeUndefined();
  expect(stop.properties.workspace).toBeUndefined();
  const mstop: any = (await registryFor({ kind: "main" }, deps, opts)).get("workspace_stop").inputSchema;
  expect(mstop.required).toEqual(["workspace"]);
  expect(ws).toContain("intercept");
  expect(ws).toContain("subagent");
  expect(main).not.toContain("subagent");
  // a subagent talks only to its workspace session: none of these, everything else the workspace has
  const sub = (await registryFor({ kind: "subagent", ws: "w1" }, deps, opts)).names();
  for (const n of ["subagent", "workspace_ask", "main_tell", "workspace_stop", "intercept", "release", "question", "task_add", "task_update", "task_list", "workspace_tasks"]) expect(sub).not.toContain(n);
  for (const n of ws.filter((n) => !["subagent", "main_tell", "workspace_stop", "intercept", "release", "question", "task_add", "task_update", "task_list", "workspace_tasks"].includes(n))) expect(sub).toContain(n);
  expect(sub).toContain("exec");
  expect(sub).toContain("packages_add");
  expect(sub).toContain("web_search");
  expect(sub).not.toContain("workspace_create");
});

test("a subagent role card names its workspace and the commit-do-not-push rule", () => {
  expect(roleCard("ws-a:agent-1f")).toContain("You are a subagent of workspace ws-a. Do the task alone. Commit your work and do not push; the platform pushes.");
});

test("main keeps only the orchestrator's tools", async () => {
  delete process.env.KL_API_URL;
  const deps = { live: new Map(), busy: new Set<string>(), open: async () => null as never, permit: async () => ({}) };
  const main = (await registryFor({ kind: "main" }, deps, { tools: [] } as any)).names();
  for (const n of ["packages_add", "packages_remove", "packages_update", "intercept", "release", "subagent"]) expect(main).not.toContain(n);
  for (const n of ["packages_list", "workspace_create", "workspace_delete", "workspace_ask"]) expect(main).toContain(n);
});

/** An agent that records what reached `prompt`. */
function promptAgent(messages: unknown[]) {
  const seen: string[] = [];
  return { seen, messages, subscribe: () => () => {}, prompt: async (t: string) => void (seen.push(t), messages.push({ role: "user" })), dispose() {} };
}
const blank = (): any => ({ type: "session_state", model: { provider: "p", id: "m" }, thinkingLevel: "off", autoCompact: true, codemode: false, queued: { steering: [], followUp: [] } });
const handleOf = (agent: any, key: string) => baseHandle(agent, key, { busy: new Set(), onEnd() {}, onDispose() {}, state: blank() });

test("compact passes instructions to pi, empty becomes undefined", async () => {
  const seen: unknown[] = [];
  const h = handleOf({ subscribe: () => () => {}, messages: [], compact: async (i?: string) => void seen.push(i), dispose() {} }, "main");
  await h.compact("");
  await h.compact("x");
  expect(seen).toEqual([undefined, "x"]);
});

test("the role card rides the first prompt only, and never a resumed session's", async () => {
  const fresh = promptAgent([]);
  const h = handleOf(fresh, "main");
  await h.prompt("hi");
  await h.prompt("again");
  expect(fresh.seen[0]!.startsWith("[role: main session]")).toBe(true);
  expect(fresh.seen[0]!.endsWith("hi")).toBe(true);
  expect(fresh.seen[1]).toBe("again");
  const resumed = promptAgent([{ role: "user" }]);
  await handleOf(resumed, "main").prompt("hi");
  expect(resumed.seen).toEqual(["hi"]);
  const ws = promptAgent([]);
  await handleOf(ws, "ws-a").prompt("hi");
  expect(ws.seen[0]!.startsWith("[role: workspace session for ws-a]")).toBe(true);
  expect(roleCard("ws-a:x")).toContain("workspace session for ws-a");
});

test("sessions.list hides :agent- keys, a clone's own prefix lists them", async () => {
  process.env.KLOUDLITE_CONFIG_DIR = mkdtempSync(join(tmpdir(), "kl-cfg-"));
  delete process.env.KL_API_URL;
  const { models } = await import("@kloudlite-tui/agent");
  const m = models.getModels().find((x: any) => x.provider !== "anthropic") as any;
  const b = new LocalBackend();
  const o: any = { initial: { model: { provider: m.provider, id: m.id } }, fresh: true, tools: [], permission: async () => ({}) };
  const h = await b.session("w9:agent-ab12", o);
  const h2 = await b.session("w9", o);
  const keys = (await b.sessions.list()).map((s) => s.key);
  expect(keys).toContain("w9");
  expect(keys).not.toContain("w9:agent-ab12");
  expect((await b.sessions.list("w9")).map((s) => s.key)).toContain("w9:agent-ab12");
  await h.dispose();
  await h2.dispose();
}, 20000);

test("session state outlives the agent: reopen without fresh keeps what was set", async () => {
  process.env.KLOUDLITE_CONFIG_DIR = mkdtempSync(join(tmpdir(), "kl-cfg-"));
  delete process.env.KL_API_URL;
  const { models } = await import("@kloudlite-tui/agent");
  const m = models.getModels().find((x: any) => x.provider !== "anthropic") as any;
  const agent = () => ({ agent: {}, messages: [], subscribe: () => () => {}, setThinkingLevel() {}, dispose() {} });
  const b = new LocalBackend({ create: (async () => agent()) as any });
  const o: any = { initial: { model: { provider: m.provider, id: m.id }, thinkingLevel: "low" }, tools: [] };
  const h = await b.session("w9", { ...o, fresh: true });
  await h.setThinkingLevel("high");
  await h.dispose();
  const h2 = await b.session("w9", o);
  expect(h2.state.thinkingLevel).toBe("high");
  await h2.dispose();
}, 20000);

test("an internal open (no TUI tools) reuses a live session without disposing it", async () => {
  process.env.KLOUDLITE_CONFIG_DIR = mkdtempSync(join(tmpdir(), "kl-cfg-"));
  delete process.env.KL_API_URL;
  const { models } = await import("@kloudlite-tui/agent");
  const ms = models.getModels().filter((x: any) => x.provider !== "anthropic") as any[];
  const b = new LocalBackend();
  const o: any = { initial: { model: { provider: ms[0].provider, id: ms[0].id } }, fresh: true, tools: [], permission: async () => ({}) };
  const h = await b.session("w8", o);
  const events: string[] = [];
  h.subscribe((e) => void events.push(e.type));
  const other = ms.find((x) => x.id !== ms[0].id) ?? ms[0];
  const h2 = await b.session("w8", { initial: { model: { provider: other.provider, id: other.id } }, tools: [] } as any);
  expect(events).not.toContain("session_closed");
  expect(h2).toBeDefined();
  await h2.dispose();
  await h.dispose();
}, 20000);

test("the gate asks for a codemode script's nested gated call, once for a top-level one", async () => {
  const seen: string[] = [];
  const pi: any = {
    agent: {} as any,
    async _beforeToolCall(_ctx: any, _parent?: string) {
      seen.push("pi");
      return undefined;
    },
  };
  pi.agent.beforeToolCall = (ctx: any) => pi._beforeToolCall(ctx);
  const asked: string[] = [];
  installGate(pi, async (req) => {
    asked.push(req.name);
    return req.name === "volume_delete" ? { block: true, reason: "no" } : {};
  });
  const call = (name: string) => ({ toolCall: { name }, args: {} });
  // nested: pi's runner calls _beforeToolCall with the parent id, never agent.beforeToolCall
  expect(await pi._beforeToolCall(call("volume_delete"), "cm1")).toEqual({ block: true, reason: "no" });
  expect(await pi._beforeToolCall(call("read"), "cm1")).toBeUndefined();
  // top level: asked once, then falls through to pi's own hook
  await pi.agent.beforeToolCall(call("env_delete"));
  expect(asked).toEqual(["volume_delete", "env_delete"]);
  expect(seen).toEqual(["pi", "pi"]);
});

test("shareable: a view going away leaves the others; onZero runs each time the count drops to 0", async () => {
  let subs = 0, unsubs = 0, zero = 0;
  const base = { subscribe: () => (subs++, () => void unsubs++) } as any;
  const { view, count } = shareable(base, () => void zero++);
  const a = view(), b = view();
  a.subscribe(() => {});
  b.subscribe(() => {});
  expect(count()).toBe(2);
  await a.dispose();
  expect([subs, unsubs, zero, count()]).toEqual([2, 1, 0, 1]);
  await a.dispose();
  expect([unsubs, zero]).toEqual([1, 0]);
  await b.dispose();
  expect([unsubs, zero]).toEqual([2, 1]);
  view();
  await view().dispose();
  expect(zero).toBe(1);
});

function fakeAgent() {
  const subs = new Set<(e: any) => void>();
  return {
    messages: [],
    disposed: 0,
    subscribe: (cb: any) => (subs.add(cb), () => void subs.delete(cb)),
    dispose() { this.disposed++; },
    emit: (e: any) => subs.forEach((s) => s(e)),
  };
}

test("baseHandle: events reach every subscriber and track busy; dispose says session_closed once", () => {
  const agent = fakeAgent();
  const busy = new Set<string>();
  let ends = 0, disposes = 0;
  const h = baseHandle(agent, "k", { busy, onEnd: () => void ends++, onDispose: () => void disposes++, state: blank() });
  const a: string[] = [], b: string[] = [];
  h.subscribe((e) => a.push(e.type));
  h.subscribe((e) => b.push(e.type));
  agent.emit({ type: "agent_start" });
  expect(h.busy).toBe(true);
  agent.emit({ type: "agent_end" });
  expect([h.busy, ends]).toEqual([false, 1]);
  void h.dispose();
  void h.dispose();
  expect(a).toEqual(["agent_start", "agent_end", "session_closed"]);
  expect(b).toEqual(a);
  expect([agent.disposed, disposes]).toEqual([1, 1]);
});

test("asking adds a required because and strips it before the tool runs", async () => {
  let got: any;
  const t: any = { name: "workspace_delete", description: "d", inputSchema: { type: "object", properties: { workspace: { type: "string" } }, required: ["workspace"] }, run: async (i: any) => ((got = i), "ok") };
  const a = asking(t);
  expect((a.inputSchema as any).properties.because).toBeDefined();
  expect((a.inputSchema as any).required).toEqual(["workspace", "because"]);
  await a.run({ workspace: "x", because: { reason: "r" } });
  expect(got).toEqual({ workspace: "x" });
  const read: any = { name: "read", inputSchema: { type: "object" }, run: async () => "" };
  expect(asking(read)).toBe(read);
});

test("the gate skips the card for words the person typed, else shows the reason", async () => {
  const gate = (typed: string[]) => {
    const pi: any = { agent: {} };
    const reqs: any[] = [];
    installGate(pi, async (req) => (reqs.push(req), {}), () => ({ typed }));
    return { pi, reqs };
  };
  const call = (because: any) => ({ toolCall: { name: "workspace_delete" }, args: { workspace: "foo", because } });
  const yes = gate(["please delete workspace foo"]);
  expect(await yes.pi.agent.beforeToolCall(call({ asked: "delete workspace foo" }))).toBeUndefined();
  expect(yes.reqs).toEqual([]);
  const no = gate([]);
  await no.pi.agent.beforeToolCall(call({ asked: "delete workspace foo" }));
  expect(no.reqs.length).toBe(1);
  expect(no.reqs[0].claimed).toBe("delete workspace foo");
  expect(no.reqs[0].args).toEqual({ workspace: "foo" });
  await no.pi.agent.beforeToolCall(call({ reason: "cleanup after test" }));
  expect(no.reqs[1].reason).toBe("cleanup after test");
});

test("sessions.watch answers at once and on every change, and stops after off", async () => {
  process.env.KLOUDLITE_CONFIG_DIR = mkdtempSync(join(tmpdir(), "kl-cfg-"));
  delete process.env.KL_API_URL;
  const { models } = await import("@kloudlite-tui/agent");
  const pick = models.getModels().find((x: any) => x.provider !== "anthropic") as any;
  const o: any = { initial: { model: { provider: pick.provider, id: pick.id } }, fresh: true, tools: [], permission: async () => ({}) };
  const b = new LocalBackend();
  const lists: any[][] = [];
  const off = await b.sessions.watch((l) => lists.push(l));
  expect(lists.length).toBe(1);
  const h = await b.session("main", o);
  expect(lists.length).toBeGreaterThan(1); // opened
  const n = lists.length;
  await b.sessions.name("main", "renamed");
  expect(lists.length).toBe(n + 1);
  expect(lists.at(-1)!.find((m) => m.key === "main")?.name).toBe("renamed");
  await b.sessions.clear("main");
  expect(lists.length).toBeGreaterThan(n + 1); // the rebuild closes the agent, so the list changes
  await h.dispose();
  await new Promise((r) => setTimeout(r, 10));
  expect(lists.length).toBeGreaterThan(n + 2); // closed
  const m = lists.length;
  off();
  await b.sessions.name("main", "again");
  expect(lists.length).toBe(m);
}, 20000);

test("baseHandle reports turn start and end to its change hook", () => {
  const agent = fakeAgent();
  let changes = 0;
  baseHandle(agent, "main", { busy: new Set(), onEnd() {}, onDispose() {}, onChange: () => void changes++, state: blank() });
  agent.emit({ type: "agent_start" });
  agent.emit({ type: "agent_end", messages: [] });
  expect(changes).toBe(2);
});

test("a user message_start carries shown without the role card", () => {
  const agent = fakeAgent();
  const h = baseHandle(agent, "ws-a", { busy: new Set(), onEnd() {}, onDispose() {}, state: undefined as any });
  const got: any[] = [];
  h.subscribe((e) => got.push(e));
  const card = roleCard("ws-a");
  agent.emit({ type: "message_start", message: { role: "user", timestamp: 7, content: [{ type: "text", text: `${card}\n\nhello` }] } });
  expect(got[0].shown).toBe("hello");
  agent.emit({ type: "message_start", message: { role: "user", timestamp: 8, content: [{ type: "text", text: "again" }] } });
  expect(got[1].shown).toBe("again");
});

test("stripCard leaves text without a card alone", () => {
  expect(stripCard("main", "plain")).toBe("plain");
  expect(stripCard("main", `${roleCard("main")}\n\nx`)).toBe("x");
});

test("a typed prompt names an unnamed session", async () => {
  process.env.KLOUDLITE_CONFIG_DIR = mkdtempSync(join(tmpdir(), "kl-cfg-"));
  delete process.env.KL_API_URL;
  const { models } = await import("@kloudlite-tui/agent");
  const m = models.getModels().find((x: any) => x.provider !== "anthropic") as any;
  const b = new LocalBackend();
  const h = await b.session("w7", { initial: { model: { provider: m.provider, id: m.id } }, fresh: true, tools: [], client: true } as any);
  await h.prompt("  fix   the build please, it fails on the linker step every time  ").catch(() => {});
  const meta = (await b.sessions.list()).find((s) => s.key === "w7");
  expect(meta?.name).toBe("fix the build please, it fails on the li");
  await h.dispose();
}, 20000);

test("every session has task tools writing its own board", async () => {
  delete process.env.KL_API_URL;
  const tasks = mkdtempSync(join(tmpdir(), "kl-boards-"));
  const deps = { live: new Map(), busy: new Set<string>(), open: async () => null as never, permit: async () => ({}), cards: new Cards(() => {}), tasks };
  const opts: any = { tools: [] };
  const ws = await registryFor({ kind: "workspace", ws: "w1" }, deps, opts, "w1:agent-1");
  expect(ws.names()).toEqual(expect.arrayContaining(["task_add", "task_update", "task_list"]));
  expect((await registryFor({ kind: "main" }, deps, opts)).names()).toContain("task_add");
  await (ws.get("task_add") as any).run({ title: "mine" });
  expect(readTasks(join(tasks, "w1.json")).map((t) => t.title)).toEqual(["mine"]);
  expect(readTasks(join(tasks, "main.json"))).toEqual([]);
  // boards: main first even when empty, then each non-empty board by name
  await (ws.get("task_add") as any).run({ title: "again" });
  writeFileSync(join(tasks, "ws-a.json"), "[]");
  expect(readBoards(tasks).map((b) => [b.session, b.tasks.length])).toEqual([["main", 0], ["w1", 2]]);
});

test("space() carries the boards (main first) and the message log", async () => {
  const v = await new LocalBackend().space();
  expect(v.boards![0]!.session).toBe("main");
  expect(Array.isArray(v.messages)).toBe(true);
});

test("the MCP door serves every platform and pod tool with no `because`, plus main's delegation", async () => {
  const deps = { live: new Map(), busy: new Set<string>(), open: async () => null as never, permit: async () => ({}), tasks: mkdtempSync(join(tmpdir(), "kl-mcp-")) };
  const tools = await mcpTools(deps);
  const names = tools.map((t) => t.name);
  for (const n of [...platformTools("all").map((t) => t.name), ...POD_TOOLS, "workspace_ask", "web_fetch", "web_search", "scratch_bash", "scratch_read"]) expect(names).toContain(n);
  for (const t of tools) expect((t.inputSchema as any).properties?.because).toBeUndefined();
  expect((tools.find((t) => t.name === "exec")!.inputSchema as any).required).toContain("workspace");
  expect(names).not.toContain("question");
});

// A fake agent whose prompt() records what it was sent; a turn is agent_start, a board change, agent_end.
async function freshRig(key: string) {
  const tasks = mkdtempSync(join(tmpdir(), "kl-fresh-"));
  const { models } = await import("@kloudlite-tui/agent");
  const m = models.getModels().find((x: any) => x.provider !== "anthropic") as any;
  const log: string[] = [];
  const subs = new Set<(e: any) => void>();
  const create = (async () => {
    log.push("create");
    const sessionManager = { appendCompaction: (summary: string) => void log.push(`compact:${summary}`), getBranch: () => [] };
    return { agent: {}, sessionManager, messages: [], subscribe: (cb: any) => (subs.add(cb), () => void subs.delete(cb)), setThinkingLevel() {}, dispose() {}, prompt: async (t: string) => void log.push(`prompt:${t}`) };
  }) as any;
  const b = new LocalBackend({ create, archive: ((k: string) => void log.push(`archive:${k}`)) as any, tasks });
  const o: any = { initial: { model: { provider: m.provider, id: m.id } }, tools: [], permission: async () => ({}) };
  const file = tasksFile(key, tasks);
  const turn = async (change: () => void) => {
    const h = await b.session(key, { ...o, fresh: true });
    subs.forEach((s) => s({ type: "agent_start" }));
    change();
    subs.forEach((s) => s({ type: "agent_end", messages: [] }));
    await new Promise((r) => setTimeout(r, 30));
    return h;
  };
  return { b, o, key, file, log, turn };
}

test("a turn that marks a task done with nothing running compacts to the board and rebuilds, never archives", async () => {
  const r = await freshRig("main");
  addTask(r.file, { title: "first" });
  await r.turn(() => void updateTask(r.file, "T1", { state: "done", summary: "added x in y.ts", verified: "bun test: pass" }));
  const next = await r.b.session(r.key, r.o);
  const notices: string[] = [];
  next.subscribe((e: any) => e.type === "session_notice" && notices.push(e.text));
  await next.prompt("go on");
  await next.prompt("again");
  // the session files stay (the transcript is read back from them); only the context is cut
  expect(r.log.some((l) => l.startsWith("archive"))).toBe(false);
  expect(r.log.filter((l) => l === "create").length).toBe(2);
  const compact = r.log.filter((l) => l.startsWith("compact:"));
  expect(compact.length).toBe(1);
  expect(compact[0]).toContain("[fresh context after task T1]");
  expect(compact[0]).toContain("T1 first: added x in y.ts (verified: bun test: pass)");
  expect(r.log.indexOf(compact[0]!)).toBeLessThan(r.log.lastIndexOf("create"));
  // the board rides in the compaction summary, never on a prompt
  const sent = r.log.filter((l) => l.startsWith("prompt:"));
  expect(sent.length).toBe(2);
  expect(sent.some((l) => l.includes("fresh context"))).toBe(false);
  await new Promise((r) => setTimeout(r, 20));
  expect(notices).toEqual(["fresh context after task T1: first"]);
  await next.dispose();
}, 20000);

test("a done task while another is running does not restart", async () => {
  const r = await freshRig("main");
  addTask(r.file, { title: "a" });
  addTask(r.file, { title: "b" });
  await r.turn(() => {
    updateTask(r.file, "T2", { state: "running" });
    updateTask(r.file, "T1", { state: "done", summary: "s", verified: "v" });
  });
  expect(r.log.filter((l) => l === "create").length).toBe(1);
  expect(r.log.some((l) => l.startsWith("compact"))).toBe(false);
}, 20000);

test("a subagent session never restarts", async () => {
  const r = await freshRig("w7:agent-ab12");
  addTask(r.file, { title: "a" });
  await r.turn(() => void updateTask(r.file, "T1", { state: "done", summary: "s", verified: "v" }));
  expect(r.log.some((l) => l.startsWith("compact"))).toBe(false);
}, 20000);

test("a task the backend closes without a summary (an answered ask) does not restart", async () => {
  const r = await freshRig("main");
  addTask(r.file, { title: "ask w1" });
  await r.turn(() => void updateTask(r.file, "T1", { state: "done" }));
  expect(r.log.some((l) => l.startsWith("compact"))).toBe(false);
}, 20000);

test("compact drops pi's rejection, which compaction_end already reported", async () => {
  const h = handleOf({ subscribe: () => () => {}, messages: [], compact: async () => { throw new Error("Nothing to compact (session too small)"); }, dispose() {} }, "main");
  await h.compact();
});

test("an assistant message_end fills state.context from pi's context usage", () => {
  for (const [usage, want] of [[{ tokens: 5000, contextWindow: 184000, percent: 2.7 }, { tokens: 5000, window: 184000 }], [undefined, undefined]] as const) {
    let fire: (e: any) => void = () => {};
    const state = blank();
    baseHandle({ subscribe: (cb: any) => ((fire = cb), () => {}), messages: [], getContextUsage: () => usage, dispose() {} } as any, "main", { busy: new Set(), onEnd() {}, onDispose() {}, state });
    fire({ type: "message_end", message: { role: "assistant" } });
    expect(state.context).toEqual(want);
  }
});

/** An agent whose routing fork answers `reply` (or throws / hangs); records every call that reaches pi. */
function routeAgent(fork: "hang" | "throw" | string, busy: { v: boolean }, onFork?: () => void) {
  const calls: string[] = [];
  const agent: any = {
    calls,
    messages: [],
    model: { id: "m" },
    subscribe: () => () => {},
    // pi's prompt: queues on the named queue while a run is active, starts one otherwise
    prompt: async (t: string, o?: { streamingBehavior?: string }) =>
      void calls.push(`${(busy.v && o?.streamingBehavior) || "prompt"}:${t.split("\n").pop()}`),
    _getSummarizationRequestAuth: async () => ({ model: { id: "m", contextWindow: 100_000, maxTokens: 8000 }, apiKey: "k" }),
    agent: {
      streamFunction: async () => {
        onFork?.();
        if (fork === "throw") throw new Error("boom");
        if (fork === "hang") await new Promise(() => {});
        return { result: async () => ({ stopReason: "stop", content: [{ type: "text", text: fork }] }) };
      },
    },
  };
  const hooks = { busy: { has: () => busy.v, add() {}, delete() {} } as any, onEnd() {}, onDispose() {}, state: blank() };
  return { agent, h: baseHandle(agent, "main", hooks) };
}

test("route: idle prompts without a fork", async () => {
  let forked = 0;
  const r = routeAgent("STEER: x", { v: false }, () => forked++);
  expect(await r.h.route("hi")).toEqual({ kind: "prompt", reason: "" });
  expect(r.agent.calls).toEqual(["prompt:hi"]);
  expect(forked).toBe(0);
});

test("route: busy + STEER steers, QUEUE follows up", async () => {
  const a = routeAgent("STEER: it changes the work", { v: true });
  expect(await a.h.route("stop")).toEqual({ kind: "steer", reason: "it changes the work" });
  expect(a.agent.calls).toEqual(["steer:stop"]);
  const b = routeAgent("QUEUE: later", { v: true });
  expect(await b.h.route("more")).toEqual({ kind: "followUp", reason: "later" });
  expect(b.agent.calls).toEqual(["followUp:more"]);
});

test("route: a fork that throws or answers garbage queues", async () => {
  for (const fork of ["throw", "perhaps"]) {
    const r = routeAgent(fork, { v: true });
    expect(await r.h.route("x")).toEqual({ kind: "followUp", reason: "fork failed, queued" });
    expect(r.agent.calls).toEqual(["followUp:x"]);
  }
});

test("route: a fork slower than 20 s queues", async () => {
  jest.useFakeTimers();
  try {
    const r = routeAgent("hang", { v: true });
    const p = r.h.route("x");
    await Promise.resolve(); await Promise.resolve(); jest.advanceTimersByTime(ROUTE_TIMEOUT_MS + 1);
    expect(await p).toEqual({ kind: "followUp", reason: "fork failed, queued" });
    expect(r.agent.calls).toEqual(["followUp:x"]);
  } finally {
    jest.useRealTimers();
  }
});

test("route: idle by the time the fork answers prompts", async () => {
  const busy = { v: true };
  const r = routeAgent("STEER: x", busy, () => void (busy.v = false));
  expect(await r.h.route("x")).toEqual({ kind: "prompt", reason: "x" });
  expect(r.agent.calls).toEqual(["prompt:x"]);
});

test("a steer or follow-up sent after the turn ended starts a run instead of waiting in pi's queue", async () => {
  const r = routeAgent("", { v: false });
  await r.h.steer("late");
  await r.h.followUp("later");
  expect(r.agent.calls).toEqual(["prompt:late", "prompt:later"]);
});

test("seen keeps what a compaction summarised away, and a kept message once", () => {
  const msg = (role: string, timestamp: number) => ({ role, timestamp });
  const a = msg("user", 1), b = msg("assistant", 2), c = msg("user", 3);
  const sum = msg("compactionSummary", 4);
  const agent = {
    messages: [sum, c],
    sessionManager: { getBranch: () => [{ type: "message", message: a }, { type: "message", message: b }, { type: "message", message: c }, { type: "compaction" }] },
  };
  expect(seen(agent)).toEqual([a, b, sum, c]);
  expect(seen({ messages: [a] })).toEqual([a]);
});
