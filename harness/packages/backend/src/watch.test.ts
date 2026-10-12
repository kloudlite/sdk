import { expect, test } from "bun:test";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { addTask, readTasks, tasksFile, updateTask } from "./tasks.ts";
import { boardWatcher } from "./watch.ts";

const end = (toolName = "exec") => ({ type: "tool_execution_end", toolName }) as never;
const tick = () => new Promise((r) => setTimeout(r, 5));

function rig(reply: () => Promise<string | null> = async () => "T1") {
  const file = tasksFile("ws-a", mkdtempSync(join(tmpdir(), "kl-watch-")));
  addTask(file, { title: "a" });
  addTask(file, { title: "b" });
  const asked: string[] = [];
  let t = 1_000_000;
  const w = boardWatcher({ key: "ws-a", file, ask: async (b) => (asked.push(b), reply()), now: () => t });
  return { file, asked, w, advance: (ms: number) => void (t += ms) };
}

test("two tool ends do nothing; the third asks once and marks the task running by the watcher", async () => {
  const r = rig();
  r.w(end());
  r.w(end());
  await tick();
  expect(r.asked).toEqual([]);
  r.w(end());
  await tick();
  expect(r.asked.length).toBe(1);
  expect(r.asked[0]).toContain("T1 [queued]");
  expect(readTasks(r.file)[0]).toMatchObject({ state: "running", by: "watcher" });
});

test("a task_update end resets the count; a running task means no call", async () => {
  const r = rig();
  r.w(end());
  r.w(end());
  r.w(end("task_update"));
  r.w(end());
  r.w(end());
  await tick();
  expect(r.asked).toEqual([]);
  updateTask(r.file, "T1", { state: "running" });
  r.w(end());
  r.w(end());
  r.w(end());
  await tick();
  expect(r.asked).toEqual([]);
});

test("agent_start resets the count", async () => {
  const r = rig();
  r.w(end());
  r.w(end());
  r.w({ type: "agent_start" } as never);
  r.w(end());
  await tick();
  expect(r.asked).toEqual([]);
});

test("calls are at least 30 s apart", async () => {
  const r = rig(async () => null);
  for (let i = 0; i < 3; i++) r.w(end());
  await tick();
  for (let i = 0; i < 3; i++) r.w(end());
  await tick();
  expect(r.asked.length).toBe(1);
  r.advance(30_000);
  for (let i = 0; i < 3; i++) r.w(end());
  await tick();
  expect(r.asked.length).toBe(2);
});

test("an ask that throws writes nothing and does not crash", async () => {
  const r = rig(async () => {
    throw new Error("offline");
  });
  for (let i = 0; i < 3; i++) r.w(end());
  await tick();
  expect(r.asked.length).toBe(1);
  expect(readTasks(r.file).every((t) => t.state === "queued")).toBe(true);
});
