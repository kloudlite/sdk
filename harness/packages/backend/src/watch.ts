//! Marks the task a session is working on as running, because the session never does.
//! Incident ws-73, 2026-10-12: T20-T23 sat queued through a whole build and push with zero
//! task_update, so the board the person follows showed nothing happening. The prompt asks for
//! `task_update running` before each task; sessions skip it.
//! Trigger: three tool calls since the session last touched its board (`task_add`/`task_update`, or
//! the turn start), the board has a queued task and none running, no watcher call is in flight, and
//! the last one started at least 30 s ago. Then one side call (`ask`, ~3k tokens of recent steps plus
//! the board) names the queued task the steps are working on, and `markRunning` writes it.
//! Cost: at most one such call per 30 s per session, and only while a board is stale.
//! It NEVER sets done/failed/blocked, adds or deletes tasks, or messages anyone. The mark carries
//! `by: "watcher"` and the session's own `task_update` clears it, so the session always wins.
//! Every error is swallowed: a missed mark is the status quo, not a failure.
import type { SessionEvent } from "./index.ts";
import { boardText, markRunning, readTasks } from "./tasks.ts";

const AFTER_CALLS = 3;
const GAP_MS = 30_000;

export function boardWatcher(o: { key: string; file: string; ask: (board: string) => Promise<string | null>; now?: () => number }): (e: SessionEvent) => void {
  const now = o.now ?? Date.now;
  let calls = 0;
  let inFlight = false;
  let last = -Infinity;
  return (e) => {
    const ev = e as { type: string; toolName?: string };
    if (ev.type === "agent_start") calls = 0;
    if (ev.type !== "tool_execution_end") return;
    if (ev.toolName === "task_add" || ev.toolName === "task_update") {
      calls = 0;
      return;
    }
    if (++calls < AFTER_CALLS || inFlight || now() - last < GAP_MS) return;
    const ts = readTasks(o.file);
    if (!ts.some((t) => t.state === "queued") || ts.some((t) => t.state === "running")) return;
    last = now();
    inFlight = true;
    calls = 0;
    void o
      .ask(boardText(ts))
      .then((id) => id && markRunning(o.file, id))
      .catch(() => {}) // ponytail: errors are dropped silently; log them if the watcher ever needs tuning
      .finally(() => void (inFlight = false));
  };
}
