import { expect, test } from "bun:test";
import { mkdtempSync, readdirSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { mcpTools } from "./local.ts";

const PLUGIN = join(import.meta.dir, "../../../laptop-plugin/plugin");
const LAPTOP_NEVER = ["workspace_ask", "task_add", "task_update", "task_list", "workspace_tasks", "scratch_bash", "scratch_read", "scratch_write"];

const served = async () => {
  const deps = { live: new Map(), busy: new Set<string>(), open: async () => null as never, permit: async () => ({}), tasks: mkdtempSync(join(tmpdir(), "kl-plugin-")) };
  return new Set((await mcpTools(deps)).map((t) => t.name));
};
const skills = () => readdirSync(join(PLUGIN, "skills")).map((d) => readFileSync(join(PLUGIN, "skills", d, "SKILL.md"), "utf8"));
const agents = () => readdirSync(join(PLUGIN, "agents")).map((f) => readFileSync(join(PLUGIN, "agents", f), "utf8"));

test("every tool a laptop skill lists is one the bench serves, and none is a bench-session tool", async () => {
  const names = await served();
  const rows = skills().flatMap((s) => [...s.matchAll(/^\| `([a-z_]+)` \|/gm)].map((m) => m[1]!));
  expect(rows.length).toBeGreaterThan(40);
  for (const n of rows) expect(names.has(n) ? n : `missing: ${n}`).toBe(n);
  for (const n of rows) expect(LAPTOP_NEVER).not.toContain(n);
});

test("every mcp__plugin_kloudlite_kl__ name in the plugin is one the bench serves", async () => {
  const names = await served();
  for (const text of [...skills(), ...agents()])
    for (const m of text.matchAll(/mcp__plugin_kloudlite_kl__([a-z_]+)/g)) expect(names.has(m[1]!) ? m[1] : `missing: ${m[1]}`).toBe(m[1]!);
});

test("the two agents have the kl tools only", () => {
  const a = agents();
  expect(a.length).toBe(2);
  for (const text of a) expect(text.match(/^tools: (.*)$/m)?.[1]).toBe("mcp__plugin_kloudlite_kl__*");
});
