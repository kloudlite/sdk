// The bench-net container's open-tunnel count (`/run/kl-net/net.streams`, written every 5 s). Stale
// after 15 s: a dead `net` container holds nothing open. Owner ruling 2026-10-11: "I want the bench
// to be awake as long as there is connection."
export const STALE_MS = 15_000;
export function openStreams(text: string | null, mtimeMs: number, now: number): number {
  if (text === null || now - mtimeMs > STALE_MS) return 0;
  const n = Number.parseInt(text.trim(), 10);
  return Number.isFinite(n) && n > 0 ? n : 0;
}
