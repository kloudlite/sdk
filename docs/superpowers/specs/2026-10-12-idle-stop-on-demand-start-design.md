# Idle stop and on-demand start for workspaces and environments

Status: draft for owner review, 2026-10-12. Rulings marked **R** are defaults the owner has not
decided yet; each says what changes if overturned.

## Why

A running workspace holds cpu/memory quota and node capacity whether or not anyone uses it. An
environment is worse: it is shared by a team and nothing ever stops it. Most of that time no session
is connected, so the compute is wasted. Benches already park when idle
(`bins/agent/src/controller/workspace/bench.rs`) and builders already stop when idle
(`bins/builder-gate/src/idle.rs`). Ordinary workspaces and environments have no idle clock
(`crd/workspace.rs:449` says so outright).

## Owner rules (2026-10-12, verbatim intent)

1. No running session means the workspace is wasted: stop it after a cooldown, start it on demand.
2. No session connected to an environment means it is wasted too.
3. A session comes from the kl-tui / ttyd (pi-SDK harness on the bench) OR from Claude Code through
   the kloudlite plugin (`kl-connect mcp`). Both count.
4. A session doing work in the background (a turn running, a long job) keeps the workspace AND its
   environment alive, whether or not anyone is connected.
5. Processes running with no connected and no busy session do NOT keep it alive: stop anyway.
6. On the next connection, those processes come back in the state they were stopped in.

## What "activity" is

Both session sources reach a workspace the same way: the bench's `kl-host` daemon calls the pod's
tool server (`kl ide serve`, port 7788) over plain HTTP (`harness/packages/tools/src/pod.ts`).
Claude Code's MCP lines travel `kl-connect mcp` → gateway `/tui/{bench}` → the same daemon
(`backend/src/mcp.ts`) → the same `pod.ts`. So **the tool server sees every session's work**, and it
is where activity is measured.

A workspace is **active** at an instant when any of these holds:

| Signal | Covers | Where |
|---|---|---|
| A tool request is in flight (exec job, read, edit, grep, …) | rule 4: a long job waits in its request (up to 600 s, then the caller re-calls) | tool server, `in_flight > 0` |
| A tool request finished | rules 1, 3: ordinary use | tool server, request end |
| An open `/stream/process/{id}` or `/stream/watch/{id}` websocket not marked as a watcher | someone following output live | tool server |
| `POST /hold` arrived | rule 4: a busy turn that is not calling this workspace right now (see Hold beat) | tool server |
| An ESTABLISHED inbound TCP connection on any listening port of the pod except 7788 | `kl-connect ssh`/`ide`/`proxy` tunnels (port 22 and dev servers), intercept traffic | tool server reads `/proc/net/tcp` and `/proc/net/tcp6` every 30 s |

NOT activity:
- `/healthz`.
- Any request carrying `X-KL-Watch: 1`. The sidebar (`backend/src/space.ts`) polls every ready
  workspace's `/healthz` + `process_list` + changed files about every 5 s and sends this header.
  Without it, the sidebar alone would keep every workspace up forever.
- A detached process running (rule 5). Its own outbound connections are not inbound, so the
  `/proc/net` rule never sees them.

**R1 — cooldown 30 min** (`ws_idle_secs` = 1800). A connected but quiet session keeps the workspace
only through the cooldown after its last call. Overturned: change the default, nothing else.

### Hold beat (busy sessions)

A turn can run for a long time without touching a workspace it still needs: waiting on a subagent,
or on a job in another workspace while a server in this one stays up. The bench knows its turns
(`LocalBackend.#busy`, `local.ts:420`):

- `pod.ts` records `touched: Map<workspaceId, lastCallMs>` on every call.
- `LocalBackend` records `turnStartedAt` per busy key.
- The daemon's existing 5 s busy beat (`daemon.ts:83-90`), every 60 s while `busyCount > 0`, sends
  `POST /hold` to each READY workspace touched since the oldest running turn started. `/hold` never
  wakes a stopped workspace.

Claude Code has no turn signal visible to the bench. Its long jobs are in-flight requests and count
anyway; between calls the cooldown covers it.

### The activity file

The tool server writes activity to `~/.kl/activity`. Home is the workspace's own btrfs volume, so
the agent on the same node reads it at `{pool}/vol/{volume}/live/{ws}/.kl/activity` with no network
and no RBAC change. This is the bench `~/.kl/host.busy` pattern again.

- The tool server sets the file's **mtime** to now when a request ends, at least every 30 s while
  any signal above holds, and on `/hold`. Writes are throttled to one per 30 s.
- The tool server also writes the file once at boot, before serving.
- The agent reads only the mtime. A missing file means a tool server that predates this change (or
  one that never came up): verdict `Keep`, never idle-stop on a signal we cannot read.
- The person can touch the file themselves. That keeps only their own workspace up, inside their
  own quota, so it needs no guard.

## Stopping: the idle verdict

The agent that owns the workspace decides, on its reconcile beat. Same shape as `bench_verdict`.

- Eligible: an ordinary workspace (not `spec.bench`, not a builder), `desiredState: Running`, phase
  Running, `ws_idle_secs > 0`.
- Idle when `now - max(activity mtime, pod startTime, spec.wakeAt) > ws_idle_secs`.
- Verdict: write **status** `idleSince = now` (status write; the RBAC/admission rule that a controller
  never edits spec holds).
- `wants_pod` (`crd/workspace.rs:236`) generalises the bench rule to every workspace:
  `desiredState == Running && !(status.idleSince >= spec.wakeAt)`.
- When `wants_pod` is false because of `idleSince`, reconcile runs the existing `stop_workspace`
  path unchanged (`controller/workspace/lifecycle.rs:12-88`): `FlushBeforeStop` cuts
  `stop-{ws}-{gen}` (transient, `stop.rs:90`, so no history grows), pod delete, `wake_peers`,
  `Replicated`.
- Phase stays `Stopped`, condition `Ready=False/Idle`. No new phase, so every reader that knows
  Stopped (intercepts, quota `live()`, listings) stays correct. `/v1` adds `idle_since` to the
  workspace doc; `state` stays `"stopped"`.

Race, accepted: a call arriving while the agent writes the verdict fails with the pod. The client
sees NotReady, wakes, and retries once (below). The window needs 30 min of silence followed by a call
landing in that instant.

## Starting on demand: wake

`POST /v1/workspaces/{id}/start` today merge-patches `desiredState: Running`. It now also writes
`spec.wakeAt = max(now, status.idleSince + 1 s)`. The max absorbs clock skew between api and agent.
`guard_fill` stays first, so an over-quota wake is the usual 409.

**R2 — wake revives only idle-stopped workspaces.** A workspace a person stopped explicitly
(`desiredState: Stopped`) stays stopped. Entry points report "stopped; start it with
`workspace_start`" instead of starting it. Overturned: entry points call start for any stopped
workspace; one condition changes in `ensureRunning`.

Every entry point calls one client helper, `ensureRunning(ws)`:

1. `GET /v1/workspaces/{id}`. Ready: return.
2. `state == "stopped"` with `idle_since` set: `POST start`, then poll `GET` every 2 s until ready,
   up to 180 s (cold start includes `nix copy` of packages). Explicitly stopped: return the R2 error.
3. Timeout: an error naming the last condition reason (`KeysNotReady`, `PackagesReady=False`, …).

Entry points:

| Entry point | Where `ensureRunning` goes |
|---|---|
| Every harness tool call to a pod (TUI, ttyd, Claude Code MCP) | `pod.ts`, when `GET /v1/workspaces/{id}/tools` fails or the pod call fails with NotReady; then retry the call once |
| `kl-connect ssh` / `ide` / `proxy` | `bins/kl-connect`, before opening `/tunnel/{ws}` |
| `kl-connect ws ide <ws>` | same |
| Sidebar | never; it shows `stopped (idle)` and does not wake |

`/v1/workspaces/{id}/tools` stays a plain GET with no side effect.

## Processes come back

Rule 6. The tool server keeps processes in memory only (`crates/ide/src/procs.rs`), and they die with
the pod. Without help, the next connection finds them gone.

**R3 — "same state" means same command, cwd, env and id, started fresh.** Memory, open sockets and
the output ring are not restored. CRIU is rejected: it needs ptrace/`CAP_SYS_ADMIN` in a user pod,
cannot restore TCP peers, and the volume may wake on another node. Overturned: a separate
checkpoint/restore project; nothing here blocks it.

- `Proc` gains `cwd: PathBuf` and `env: Vec<(String, String)>`: the request's env overrides only, not
  the pod's environment. `exec.rs:49-102` already computes both at spawn; store them.
- The tool server keeps `~/.kl/procs.json` (0600) current: rewrite it (write temp, rename) on every
  detached spawn, exit and kill. One entry per process: `{id, cmd, cwd, env, tree, started_at,
  attempts}`. Only `running` processes are listed; an exit or `process_kill` removes the entry.
  - The file must be current BEFORE stop. `stop_workspace` snapshots before deleting the pod, so a
    write at SIGTERM would miss the snapshot, and the volume may wake on another node.
- On boot, before serving, the tool server relaunches every entry with the same id, cmd, cwd and env.
  The new output ring opens with `[restarted <RFC 3339>: workspace was stopped]`. This happens after
  ANY stop (idle, explicit, eviction, node death): the tool server cannot tell them apart, and a
  process the person wanted gone was killed and is no longer listed.
- Crash-loop guard: increment `attempts` and save before spawning. Reset to 0 once the process has
  run 60 s. Skip an entry with `attempts >= 3` and leave it in `process_list` as `exited` with the
  note `not restarted: failed 3 restarts`.
- `env` may hold secrets the caller passed. It sits in the person's own volume at 0600, as their
  shell history does.
- Jobs (exec without detach) are never recorded. They live in a request, and an in-flight request
  keeps the workspace up.

## Environments

An environment's users are workspaces and laptops, never sessions directly.

**Followers** of environment E, computed by the agent that owns E from the reflector caches
(`Ctx::workspaces()`; `None` means UNKNOWN, so the verdict is `Keep`):
- a Running, not-idle workspace whose space resolves to E (`controller/space.rs:60`
  `space_environment`, the same resolution the attach renderer uses);
- a Running, not-idle workspace named in `E.spec.intercepts`;
- a hold: annotation `kloudlite.io/held-at` newer than `env_idle_secs`.

**Verdict.** A Running environment with no follower gets status `unfollowedSince` (cleared when a
follower appears). When `now - unfollowedSince > env_idle_secs`, the agent writes status
`idleSince = now`. `wants_running = desiredState == Running && !(idleSince >= spec.wakeAt)`. When it
is false, the existing `stop_environment` path runs (`controller/environment/stop.rs`): drain, push
`stop-{env}-{gen}`, delete StatefulSets, `wake_peers`, Stopped + Replicated, `Ready=False/Idle`.

**R4 — `env_idle_secs` = 300.** Followers already waited out their own 30 min cooldown, so the
environment needs only a short grace. That also lets an intercept release settle before the env
stops.

**Wake.**
- `start_ws`: after patching the workspace, if its space environment or any environment intercepting
  through it is idle-stopped, patch that environment's `wakeAt` the same way. No separate quota
  check: `guard_fill` already covered the workspace, and the env's cpu/memory is the team's.
  - Over quota on the env side: the workspace still starts. The env stays stopped with its usual
    409 visible on `env_get`.
- `POST /v1/environments/{id}/start` gains the same `wakeAt` write.
- **Laptop** `kl-connect env`:
  - On open: `ensureRunning` for the environment.
  - While the tunnel is open: `POST /v1/environments/{id}/hold` every 60 s. The new handler (owner or
    team member, as `env_get`) patches the `kloudlite.io/held-at` annotation. An annotation bumps no
    generation, so the sync beat cuts nothing.
- Harness `env_*` tools call `ensureRunning` for the environment.

**R5 — no wake on inbound HTTP.** No inbound path to an environment exists today (ClusterIP only).
Overturned: needs an ingress design first; out of scope.

Builder environments (`EnvironmentSpec::system`) are excluded; builder-gate already stops them.

Intercepts, unchanged: a stopped workspace is `Off("WorkspaceStopped")` at once (`intercept.rs:196`),
and the service's StatefulSet comes back. CLAUDE.md line ~341 says such a workspace "releases after
`INTERCEPT_GRACE_SECS`", which contradicts the code. The plan fixes CLAUDE.md to match the code:
the grace applies only to unreachable/NotReady.

## Knobs

Both are agent tunables in `ClusterSettings/default`, `Mark::Live`, following `bench_idle_secs`
through `crd/settings.rs` defaults + meta, `settings.rs` `AgentSettings` + env, `admin/settings.rs`
`merge_cluster_spec`, and `admin/schema.rs`.

| Knob | Env | Default | Range | 0 means |
|---|---|---|---|---|
| `ws_idle_secs` | `WS_IDLE_SECS` | 1800 | 0 or 300..86400 | never idle-stop workspaces |
| `env_idle_secs` | `WS_ENV_IDLE_SECS` | 300 | 0 or 60..86400 | never idle-stop environments |

Client constants (no knob, `ponytail:`): wake poll 2 s, wake timeout 180 s, hold beat 60 s, activity
write throttle 30 s.

## Quota and cost

An idle-stopped workspace or environment is not `live()` (`quota.rs:136`), so its cpu/memory quota
frees at once; disk stays charged. Stop and wake each cut one transient snapshot (`retain` keeps one
per worktree) and a `wake_peers`. No new storage.

## Observability

Log lines, following the existing boundary set:
- `idle.verdict {kind, id, idle_secs, last_activity}` when the agent writes `idleSince`.
- `idle.wake {kind, id, by: tools|tunnel|env|start|follower}` in the api.
- `procs.relaunch {id, attempts, ok}` in the tool server.

History needs no new code: phase `stopped` with reason `Idle` already flows through
`{kind}.condition` rows.

## Testing

- Agent unit:
  - verdict boundaries: mtime fresh, stale, missing (Keep), wakeAt newer than activity;
  - bench and builder excluded;
  - `ws_idle_secs = 0`;
  - `wants_pod` table: explicit stop, idle, woken, skew.
- Env verdict unit:
  - follower via space, via intercept, via hold;
  - unknown cache means Keep;
  - `unfollowedSince` set and cleared;
  - system env excluded.
- Tool server:
  - watcher header and `/healthz` do not touch the file;
  - in-flight request and `/hold` do;
  - `/proc/net/tcp` parse (fixture text, port 7788 excluded, LISTEN excluded);
  - `procs.json` written on spawn, exit and kill;
  - relaunch keeps the id and adds the marker line;
  - attempts guard.
- API:
  - start writes `wakeAt = max(now, idleSince + 1 s)`;
  - start of a follower wakes its env;
  - `/hold` writes only the annotation and needs membership;
  - `/v1` doc carries `idle_since`.
- Harness (`bun test`):
  - `ensureRunning` covers ready, idle wake, the explicit-stop error and timeout;
  - `pod.ts` retries once after a wake;
  - the sidebar sends `X-KL-Watch` and never wakes;
  - the hold beat targets only workspaces touched since the oldest running turn.
- Probes (deploy/slo.md, hand-run during the dev phase):
  - `ws.idle.stop`: with `ws_idle_secs` = 300, a quiet workspace stops with `Ready=False/Idle`;
  - `ws.idle.wake`: a tool call wakes it and succeeds; a detached `sleep` process is back with the
    same id;
  - `env.idle.stop`: the env stops `env_idle_secs` after its last follower.

## Out of scope

- Benches and builders (they have their own idle).
- Inbound HTTP wake for environments (R5).
- Memory-exact process restore (R3).
- Migration: none (dev phase). A workspace whose pod predates the tool server change has no
  activity file and is never idle-stopped until its pod is recreated (next start, or by hand).
