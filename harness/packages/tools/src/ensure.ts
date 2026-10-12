//! One door for every entry point (spec 2026-10-12): ready returns; idle-stopped is started and
//! waited for; a workspace a person stopped stays stopped (R2) and says how to start it.
import { apiJson, seg } from "./platform.ts";

// ponytail: fixed 2 s poll and 180 s wake budget (cold start includes `nix copy`); a knob if anyone waits longer.
const POLL_MS = 2_000;
const TIMEOUT_MS = 180_000;

export async function ensureRunning(
  kind: "workspaces" | "environments",
  id: string,
  o: { api?: typeof apiJson; sleep?: (ms: number) => Promise<void>; timeoutMs?: number } = {},
): Promise<void> {
  const api = o.api ?? apiJson;
  const sleep = o.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));
  const path = `/v1/${kind}/${seg(id)}`;
  let d: any = await api("GET", path);
  if (ready(d)) return;
  // idle_since is present only while desired Running but idle-stopped; a person's stop never has it
  if (d?.idle_since) await api("POST", `${path}/start`);
  else if (d?.state === "stopped") throw new Error(`${id} is stopped; start it with \`${kind === "workspaces" ? "workspace_start" : "env_start"}\``);
  const until = Date.now() + (o.timeoutMs ?? TIMEOUT_MS);
  for (;;) {
    d = await api("GET", path);
    if (ready(d)) return;
    if (Date.now() >= until) throw new Error(`${id} did not become ready: ${reason(d)}`);
    await sleep(POLL_MS);
  }
}

// workspaces report `ready`, environments `running` (see workspace_get / env_get)
const ready = (d: any) => d?.ready === true || d?.state === "ready" || d?.state === "running";
// the workspace doc names its conditions (packages_status, degraded, placed); `conditions` covers a list shape
const reason = (d: any) =>
  [...(d?.conditions ?? []), d?.packages_status, d?.degraded, d?.placed].find((c: any) => c?.status === "False" && c.reason)?.reason ?? d?.state ?? "unknown";
