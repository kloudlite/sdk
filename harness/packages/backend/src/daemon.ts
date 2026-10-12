//! The bench's one agent backend, run under runit (bench/sv/kl-host). Every client — the browser TUI
//! (ttyd -> cli.tsx -> relay.ts) on the unix socket, the laptop kl-tui through the gateway on TCP
//! 7791 — is a connection to it, so a turn outlives the client that started it (a quit, a dropped
//! connection) and there is exactly one agent, and one writer of each pi session file, per key. A
//! connection speaks ./wire through one `serve`; closing it disposes that client's views only
//! (local.ts keeps a running agent alive and settles an idle one). Console output stays on
//! stdout/stderr, which runit sends to the container log: frames only travel on sockets.
//! The socket is 0600 in the user's home; the TCP port is fenced by the pod's NetworkPolicy to the
//! gateway, whose token is the lock.
import { mkdirSync, rmSync, chmodSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import { createServer, type Server, type Socket } from "node:net";
import type { Backend } from "./index.ts";
import { serve } from "./serve.ts";
import { Peer } from "./wire.ts";
import { envHold, followedEnvs, inflight, podHold, touched } from "@kloudlite-tui/tools";

export const sockPath = () => process.env.KL_HOST_SOCK ?? join(homedir(), ".kl", "host.sock");

/** One client connection, whatever it arrived on: one `serve`, its views disposed when it closes. */
export function attach(backend: Backend) {
  return (conn: Socket) => {
    const peer = new Peer((line) => void conn.write(line));
    const s = serve(backend, peer);
    conn.setNoDelay?.(true);
    conn.on("data", (c: Buffer) => peer.feed(c));
    conn.on("close", () => {
      peer.close();
      void s.dispose();
    });
    conn.on("error", () => {});
  };
}

/** Workspaces used since the oldest running turn began: those turns may still need them. */
export const holdTargets = (t: Map<string, number>, oldest: number | undefined): string[] =>
  oldest === undefined ? [] : [...t].filter(([, ms]) => ms >= oldest).map(([ws]) => ws);

export type HoldDeps = {
  busy: () => number;
  oldest: () => number | undefined;
  touched: Map<string, number>;
  inflight: Map<string, number>;
  podHold: (ws: string) => Promise<void>;
  envHold: (env: string) => Promise<void>;
  listEnvs: () => Promise<string[]>;
};

/** One hold pass: workspaces with a call in flight (Claude Code sessions run no backend turn) plus,
 * while a backend turn runs, those touched since it began; then every followed environment if any
 * workspace is held. Best effort, never throws. */
export async function holdBeat(d: HoldDeps): Promise<void> {
  try {
    const held = new Set(d.inflight.keys());
    if (d.busy() > 0) for (const ws of holdTargets(d.touched, d.oldest())) held.add(ws);
    if (!held.size) return;
    await Promise.all([...held].map((ws) => d.podHold(ws).catch(() => {})));
    for (const e of await d.listEnvs()) await d.envHold(e).catch(() => {});
  } catch {}
}

export function host(backend: Backend, sock: string): Promise<Server> {
  rmSync(sock, { force: true }); // a crashed daemon leaves its socket file behind
  const server = createServer(attach(backend));
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(sock, () => {
      chmodSync(sock, 0o600);
      resolve(server);
    });
  });
}

/** The laptop kl-tui's door: the gateway's `/tui/{bench}` pumps here. No auth of its own — the
 * gateway token is the lock and the pod's NetworkPolicy admits only the gateway to this port
 * (`crates/workspaces/src/k8s/bench.rs` `BENCH_TUI_PORT`). */
export function listenTcp(backend: Backend, port: number, hostname = "0.0.0.0"): Promise<Server> {
  const server = createServer(attach(backend));
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(port, hostname, () => resolve(server));
  });
}

/** Live count of a server's open connections. A laptop `kl-connect mcp` or kl-tui holds one 7791
 * connection for its whole life but runs no turn between calls; without this the idle clock reads
 * it as nobody and the agent deletes the pod under it (2026-10-10: "Connection closed" 12 s after
 * the plugin connected). */
export function openConnections(server: Server): () => number {
  let open = 0;
  server.on("connection", (s) => {
    open++;
    s.once("close", () => open--);
  });
  return () => open;
}

if (import.meta.main) {
  // one stray rejection must not take every session on the bench down with it
  process.on("unhandledRejection", (e) => console.error("unhandled rejection", e));
  process.chdir(homedir());
  mkdirSync(join(homedir(), ".kl"), { recursive: true });
  const { LocalBackend } = await import("./local.ts");
  const b = new LocalBackend();
  await host(b, sockPath());
  // 7791 = BENCH_TUI_PORT (crates/workspaces/src/k8s/bench.rs); env only so a laptop run can move it
  const tcp = openConnections(await listenTcp(b, Number(process.env.KL_HOST_TUI_PORT ?? 7791)));
  // The idle clock (bench/sessions/main.ts) cannot see a turn whose client left, nor a laptop
  // client on 7791 (its process scan finds only local bun clients); this file is how it does. Rewritten on a beat so a dead daemon's last count goes stale instead of pinning the pod.
  const busyFile = join(homedir(), ".kl", "host.busy");
  const beat = () => {
    try {
      writeFileSync(busyFile, String(b.busyCount + tcp()));
    } catch (e) {
      console.error("host.busy", e);
    }
  };
  beat();
  // ponytail: fixed 60 s hold beat (every 12th 5 s tick); the tool server idles out far slower
  let ticks = 0;
  setInterval(() => {
    beat();
    if (++ticks % 12 === 0) void holdBeat({ busy: () => b.busyCount, oldest: () => b.oldestTurnStart(), touched, inflight, podHold, envHold, listEnvs: followedEnvs });
  }, 5000);
  await b.resumeAsks();
}
