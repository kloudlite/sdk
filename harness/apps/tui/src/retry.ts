import type { Entry } from "./components/Transcript.tsx";

type Tool = Entry & { kind: "tool" };
const isTool = (e: Entry): e is Tool => e.kind === "tool";

/**
 * A retry replaces the failed row instead of stacking under it: when `next` is a tool entry and the
 * latest tool entry before it is the same tool, failed, with no user message since, drop that row
 * and carry a retry count forward. Anything else appends.
 */
export function foldRetry(entries: Entry[], next: Entry): Entry[] {
  if (!isTool(next)) return [...entries, next];
  for (let i = entries.length - 1; i >= 0; i--) {
    const e = entries[i]!;
    if (e.kind === "user") break;
    if (!isTool(e)) continue;
    if (e.name !== next.name || e.status !== "error") break;
    return [...entries.slice(0, i), ...entries.slice(i + 1), { ...next, retries: (e.retries ?? 0) + 1 }];
  }
  return [...entries, next];
}

/** Reload: tool status arrives with the later toolResult, so fold retries over the finished list. */
export const foldRetries = (entries: Entry[]): Entry[] => entries.reduce<Entry[]>(foldRetry, []);

/**
 * Codemode inner calls arrive as their own tool entries (ids `<parent>/<n>`). An inner call with the
 * same name and summary as an earlier one of the same script folds into that row, counted in
 * `repeats`, so a poll loop (`env_get`, `sleep`, `env_get`, ...) is one row per call shape, not
 * forty (2026-10-11).
 */
export function foldRepeats(entries: Entry[]): Entry[] {
  const out: Entry[] = [];
  // `<parent>|<name>|<summary>` -> index in `out`
  const seen = new Map<string, number>();
  for (const e of entries) {
    const slash = isTool(e) ? (e.id?.lastIndexOf("/") ?? -1) : -1;
    if (!isTool(e) || slash < 0) {
      out.push(e);
      continue;
    }
    const key = `${e.id!.slice(0, slash)}|${e.name}|${e.summary}`;
    const i = seen.get(key);
    if (i === undefined) {
      seen.set(key, out.length);
      out.push(e);
    } else {
      const p = out[i] as Tool;
      out[i] = { ...e, id: p.id, repeats: (p.repeats ?? 1) + 1 };
    }
  }
  return out;
}
