import { expect, test } from "bun:test";
import { mkdtempSync, readdirSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { mcpTools } from "./local.ts";

const PLUGIN = join(import.meta.dir, "../../../laptop-plugin/plugin");
const LAPTOP_NEVER = ["workspace_ask", "task_add", "task_update", "task_list", "workspace_tasks"];

const served = async () => {
  const deps = { live: new Map(), busy: new Set<string>(), open: async () => null as never, permit: async () => ({}), tasks: mkdtempSync(join(tmpdir(), "kl-plugin-")) };
  return new Set((await mcpTools(deps)).map((t) => t.name));
};
const skills = () => readdirSync(join(PLUGIN, "skills")).map((d) => readFileSync(join(PLUGIN, "skills", d, "SKILL.md"), "utf8"));
const agents = () => readdirSync(join(PLUGIN, "agents")).map((f) => readFileSync(join(PLUGIN, "agents", f), "utf8"));

test("every tool a laptop skill lists is one the bench serves, and none is a bench-session tool", async () => {
  const names = await served();
  // tool tables are headed `| Tool | ..` or `| Group | Tools |`; a row lists one or more tools in its first backticked cell
  // (Agent, SendMessage and `subagent_type: ..` are not bench tools; state tables are not tool tables)
  const rows = skills().flatMap((s) => {
    let tools = false;
    return s.split("\n").filter((l) => l.startsWith("|")).flatMap((l) => {
      if (l.includes("---")) return [];
      if (!l.includes("`")) tools = /^\| (Tool|Group) \|/.test(l);
      const cell = l.split("|").slice(1).find((c) => c.includes("`")) ?? "";
      return tools ? [...cell.matchAll(/`([a-z_]+)`/g)].map((m) => m[1]!) : [];
    });
  });
  expect(rows.length).toBeGreaterThan(40);
  for (const n of rows) expect(names.has(n) ? n : `missing: ${n}`).toBe(n);
  for (const n of rows) expect(LAPTOP_NEVER).not.toContain(n);
});

test("every mcp__plugin_kloudlite_kl__ name in the plugin is one the bench serves", async () => {
  const names = await served();
  for (const text of [...skills(), ...agents()])
    for (const m of text.matchAll(/mcp__plugin_kloudlite_kl__([a-z_]+)/g)) expect(names.has(m[1]!) ? m[1] : `missing: ${m[1]}`).toBe(m[1]!);
});

test("the two agents have the kl tools (and ToolSearch to load them) only, and no lifecycle tool", () => {
  const a = agents();
  expect(a.length).toBe(2);
  for (const text of a) {
    const tools = text.match(/^tools: (.*)$/m)![1]!.split(", ");
    // ToolSearch loads the kl tools when they were deferred; it runs nothing
    for (const t of tools) if (t !== "ToolSearch") expect(t).toStartWith("mcp__plugin_kloudlite_kl__");
    // kl-workspace may stop and push its own workspace; the subagent may not
    const own = text.includes("name: kl-workspace") ? "" : "|stop|push";
    for (const t of tools) expect(t).not.toMatch(new RegExp(`__(workspace_(list|create|clone|restore|start|delete${own})|env_(create|start|stop|delete|clone|push|restore.*))$`));
  }
});
