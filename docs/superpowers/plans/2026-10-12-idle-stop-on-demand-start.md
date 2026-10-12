# Idle Stop and On-Demand Start Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop an ordinary workspace after `ws_idle_secs` with no session activity, and an environment `env_idle_secs` after its last follower. Start either one again on the next connection. Detached processes come back with the same id.

**Architecture:**
- The tool server (`kl ide serve`) is where every session meets the pod, so it measures activity and writes the mtime of `~/.kl/activity`. It also keeps `~/.kl/procs.json` and relaunches from it at boot.
- The owning agent reads that mtime from the volume. It writes status `idleSince` and runs the existing stop path.
- `POST start` writes `spec.wakeAt`. Clients call `ensureRunning` before they use a workspace or environment.

**Tech Stack:** Rust (axum, kube-rs, tokio), TypeScript/Bun (harness), reqwest (kl-connect).

**Spec:** `docs/superpowers/specs/2026-10-12-idle-stop-on-demand-start-design.md`. Read it before any task. Where this plan and the spec disagree, the spec wins, except for the Rulings below.

## Global Constraints

**Where the work happens**
- Work in a worktree under `/Volumes/kdisk/rustic-git-wt/`. Never in the dev pod. Never with `cargo` outside `CARGO_TARGET_DIR=/Volumes/kdisk/cargo-target`.
- The shell is fish. Wrap multi-statement commands in `bash -c '...'`.
- Pipe test output through `tail -40` or `grep -E "^test |FAILED|panicked|error"`.

**Commits**
- Subject is an imperative sentence in sentence case.
- No trailers and no tool or AI attribution.
- `git add` named files only. Never `-A`. Never `--no-verify`.

**House style**
- Comments explain WHY.
- A deliberate shortcut carries `// ponytail: <ceiling and upgrade path>`.
- Files stay under ~800 lines. A new module gets a `//!` header.

**Knobs (the spec's table, verbatim)**
- `ws_idle_secs`: env `WS_IDLE_SECS`, default 1800, range 0 or 300..86400. 0 means never idle-stop workspaces.
- `env_idle_secs`: env `WS_ENV_IDLE_SECS`, default 300, range 0 or 60..86400. 0 means never idle-stop environments.

**Client constants**
- Wake poll 2 s, wake timeout 180 s, hold beat 60 s, activity write throttle 30 s.
- No knob for these. Each carries a `ponytail:` comment.

**Names**
- Condition reason `Idle` is `crd::IDLE`.
- Activity file `~/.kl/activity`. Process file `~/.kl/procs.json` (0600, written as a temp file then renamed).
- Header `X-KL-Watch: 1`. Annotation `kloudlite.io/held-at`.
- Relaunch marker line: `[restarted <RFC 3339>: workspace was stopped]`.
- Skip note: `not restarted: failed 3 restarts`.

**Log events**
- `idle.verdict {kind, id, idle_secs, last_activity}`
- `idle.wake {kind, id, by}`, where `by` is `tools|tunnel|env|start|follower`
- `procs.relaunch {id, attempts, ok}`

**Dev phase:** no migration, no backfill, no live-fleet compat.

## Rulings (made while planning; each overrides spec wording)

**P1. `procs.json` stores the request, not the derived values.**
- Each entry stores the exec request's own `args` object: `cmd`/`argv`, `cwd`, `env`.
- Relaunch rebuilds the command through `exec::command(tree, &args)`.
- This captures the spec's `{cmd, cwd, env}` without duplicating the sandbox wrapping in a second place.
- Entry shape: `{id, tree, args, started_at, attempts}`.

**P2. Every `podFetch` request is a watcher.**
- `podFetch` (behind `podGet`, `podText` and `podPost`) always sends `X-KL-Watch: 1`.
- Its callers are the sidebar (`space.ts`) and the file explorer (`podfs.ts`). Both are views, not work.
- Tool calls go through `podTools.call`, which never sends it.

**P3. Probes are a hand-run drill.**
- Probes `ws.idle.stop`, `ws.idle.wake` and `env.idle.stop` ship as a hand-run k3s drill (Task 11).
- Adding them to the SLO catalogue and `deploy/slo.md` is a recorded follow-up. The SLO CronJobs are suspended for the dev phase.

**P4. The workspace verdict writes `idleSince` and stops in the same reconcile pass.**

**P5. The `/v1` doc shows `idle_since` only for idle-stopped resources.**
- It appears only when `desiredState == Running && !wants_pod` (environments: `!wants_running`).
- A workspace a person stopped never shows it. This is how clients tell R2's two cases apart.

## Review Focus

1. **Explicit stop, then start, with a stale `idleSince`.** A workspace idle-stopped yesterday, explicitly stopped, then started must run. `wakeAt = max(now, idleSince+1s)` must exceed `idleSince`. Pinned in Task 2 (`wants_pod` table) and Task 7 (`wake_at_after_idle`).
2. **A freshly woken workspace is not re-idled before its first request.** Its activity mtime is old, but `wakeAt` and pod `startTime` are new. Pinned in Task 5 (`woken_recently_keeps`).
3. **A sidebar left open forever must not keep a workspace up.** `X-KL-Watch` requests and `/healthz` never touch the file. Pinned in Task 3 (`watcher_and_healthz_do_not_touch`).
4. **A process that crashes at boot must not loop forever.** It is skipped after 3 attempts and listed as exited with the note. Pinned in Task 4 (`relaunch_skips_after_three_attempts`).
5. **An env whose workspace reflector has not listed yet must not stop.** Unknown cache means Keep. Pinned in Task 6 (`unknown_cache_keeps`).

---

## File map

| File | Change |
|---|---|
| `crates/workspaces/src/crd/settings.rs`, `src/settings.rs`, `src/api/admin/schema.rs`, `src/api/admin/settings.rs` | two knobs |
| `crates/workspaces/src/crd/workspace.rs` | `spec.wakeAt`, `IDLE`, generalised `wants_pod`, `awake` |
| `crates/workspaces/src/crd/environment.rs` | `spec.wakeAt`, `status.idleSince`/`unfollowedSince`, `wants_running` |
| `deploy/k3s/crds.yaml` | regenerated |
| `crates/ide/src/activity.rs` (new) | activity tracker, `/proc/net/tcp` scan, `/hold`, beat |
| `crates/ide/src/server.rs`, `src/stream.rs`, `src/lib.rs` | wire activity |
| `crates/ide/src/procfile.rs` (new) | `procs.json` save/load/relaunch |
| `crates/ide/src/procs.rs`, `src/tools/exec.rs` | id-preserving spawn, args recorded, save hooks |
| `bins/agent/src/controller/workspace/idle.rs` (new) | pure verdict + activity read |
| `bins/agent/src/controller/workspace/{mod.rs,lifecycle.rs}` | verdict wiring, Ready condition param |
| `crates/workspaces/src/k8s/workspace.rs` | `worktree_path` pub |
| `bins/agent/src/controller/environment/idle.rs` (new) | follower verdict |
| `bins/agent/src/controller/environment/{mod.rs,stop.rs,services.rs}` | wiring, condition param, status equality |
| `crates/workspaces/src/api/workspaces/mod.rs`, `api/environments/mod.rs`, `api/mod.rs` | wakeAt, follower wake, `/hold`, doc field |
| `harness/packages/tools/src/{pod.ts,platform.ts,ensure.ts(new)}` | `ensureRunning`, retry, watch header, `podHold` |
| `harness/packages/backend/src/{local.ts,daemon.ts}` | turn starts, hold beat |
| `bins/kl-connect/src/{api.rs,ws.rs,proxy.rs,env/mod.rs,env/tunnel.rs}` | `ensure_running`, env hold |
| `CLAUDE.md` | idle section, intercept release fix |

---

### Task 1: Knobs `ws_idle_secs` and `env_idle_secs`

**Files:**
- Modify: `crates/workspaces/src/crd/settings.rs` (defaults fn near :15, field near :88, meta near :191)
- Modify: `crates/workspaces/src/settings.rs` (field near :14, env read near :59, `over!` near :104)
- Modify: `crates/workspaces/src/api/admin/schema.rs` (env table near :171, `cluster_range` near :218)
- Modify: `crates/workspaces/src/api/admin/settings.rs` (`validate_cluster_patch` near :294, `merge_cluster_spec`)

**Interfaces:**
- Produces:
  - `AgentSettings.ws_idle_secs: u64` and `AgentSettings.env_idle_secs: u64`;
  - `crd::defaults::ws_idle_secs() -> u64` (1800) and `crd::defaults::env_idle_secs() -> u64` (300);
  - CR fields `wsIdleSecs` and `envIdleSecs`.

Copy every place `bench_idle_secs` / `benchIdleSecs` / `WS_BENCH_IDLE_SECS` appears in these four files. One field missing from `merge_cluster_spec` is silently dropped on save. To be sure nothing is missed: `grep -rn "bench_idle_secs\|benchIdleSecs\|WS_BENCH_IDLE_SECS" crates/workspaces/src`.

- [ ] **Step 1: Write the failing tests** in `crates/workspaces/src/api/admin/settings.rs` `#[cfg(test)]`. Follow the existing `validate_cluster_patch` tests and build the patch the same way they do:

```rust
#[test]
fn idle_knobs_take_zero_or_range() {
    for (f, ok, bad) in [("wsIdleSecs", [0u64, 300, 86_400], [1u64, 299, 86_401]), ("envIdleSecs", [0, 60, 86_400], [1, 59, 86_401])] {
        for v in ok {
            assert!(validate_cluster_patch(&serde_json::json!({ f: v })).is_ok(), "{f}={v}");
        }
        for v in bad {
            let e = validate_cluster_patch(&serde_json::json!({ f: v })).unwrap_err();
            assert!(format!("{e:?}").contains("must be 0 (off) or"), "{f}={v}: {e:?}");
        }
    }
}
```

If `validate_cluster_patch` takes a typed spec rather than JSON, build the spec the way the neighbouring tests do. The assertions stay the same.

- [ ] **Step 2: Run, expect FAIL.**
  Run: `CARGO_TARGET_DIR=/Volumes/kdisk/cargo-target cargo test -p kloudlite-workspaces idle_knobs 2>&1 | tail -20`

- [ ] **Step 3: Implement.**
  - Add the defaults fns, fields (serde `default = "defaults::ws_idle_secs"` style, as `bench_idle_secs` does), and meta rows `("wsIdleSecs", Mark::Live, &[])` and `("envIdleSecs", Mark::Live, &[])`.
  - Add the env reads `env_u64("WS_IDLE_SECS", crd::defaults::ws_idle_secs())` and `env_u64("WS_ENV_IDLE_SECS", crd::defaults::env_idle_secs())`, and the `over!` lines.
  - Schema: add the env table rows, and `cluster_range` arms `"wsIdleSecs" | "envIdleSecs" => Some((0.0, 86_400.0))`.
  - Add this macro next to `range!` in `validate_cluster_patch`, and apply it to both fields:

```rust
macro_rules! off_or_range {
    ($f:literal, $v:expr, $lo:expr, $hi:expr) => {
        if let Some(v) = $v {
            if v != 0 && !($lo..=$hi).contains(&v) {
                return Err(format!("{} must be 0 (off) or {}..={}, got {}", $f, $lo, $hi, v));
            }
        }
    };
}
off_or_range!("wsIdleSecs", patch.ws_idle_secs, 300, 86_400);
off_or_range!("envIdleSecs", patch.env_idle_secs, 60, 86_400);
```

  Match the shape of the error the existing `range!` returns. If it builds a `Response`, wrap the message the same way.

- [ ] **Step 4: Run the new test and the schema test, expect PASS.**
  Run: `CARGO_TARGET_DIR=/Volumes/kdisk/cargo-target cargo test -p kloudlite-workspaces idle_knobs every_row_has_a_range 2>&1 | tail -20`

- [ ] **Step 5: Commit.** Add the four files. Message: `Add the ws and env idle knobs`.

---

### Task 2: CRD fields, `wants_pod`, `wants_running`

**Files:**
- Modify: `crates/workspaces/src/crd/workspace.rs` (`WorkspaceSpec` :41, `wants_pod` :236, consts near :261)
- Modify: `crates/workspaces/src/crd/environment.rs` (`EnvironmentSpec` :67, `EnvironmentStatus` :165)
- Regenerate: `deploy/k3s/crds.yaml`

**Interfaces:**
- Produces:
  - `WorkspaceSpec.wake_at: Option<String>` (serde `wakeAt`, `skip_serializing_if = "Option::is_none"`, `default`);
  - `pub const IDLE: &str = "Idle"` in `crd/workspace.rs`, re-exported as `crd::IDLE` the way `BENCH_IDLE` is. Keep `BENCH_IDLE` too.
  - `pub fn awake(idle_since: Option<&str>, wake_at: Option<&str>) -> bool`;
  - `pub fn wants_pod(w: &Workspace) -> bool`;
  - `EnvironmentSpec.wake_at: Option<String>`;
  - `EnvironmentStatus.idle_since: Option<String>` and `EnvironmentStatus.unfollowed_since: Option<String>` (camelCase, optional);
  - `pub fn wants_running(e: &Environment) -> bool`, exported from `crd`.

- [ ] **Step 1: Write the failing tests** in `crd/workspace.rs` tests (create a `#[cfg(test)] mod idle_tests` if no test module fits):

```rust
#[test]
fn awake_table() {
    let t0 = "2026-10-12T10:00:00Z";
    let t1 = "2026-10-12T10:00:01Z";
    assert!(awake(None, None), "never idle");
    assert!(!awake(Some(t0), None), "idle, never woken");
    assert!(!awake(Some(t1), Some(t0)), "woken before the idle it must end");
    assert!(!awake(Some(t0), Some(t0)), "same instant is not after");
    assert!(awake(Some(t0), Some(t1)), "woken after idle");
    assert!(awake(Some("garbage"), None), "unparsable idle_since is ignored");
}

#[test]
fn wants_pod_ordinary_workspace() {
    let mut w = Workspace::new("w", WorkspaceSpec::default());
    w.spec.desired_state = DesiredState::Running;
    assert!(wants_pod(&w));
    w.status = Some(WorkspaceStatus { idle_since: Some("2026-10-12T10:00:00Z".into()), ..Default::default() });
    assert!(!wants_pod(&w), "idle-stopped");
    w.spec.wake_at = Some("2026-10-12T10:00:01Z".into());
    assert!(wants_pod(&w), "woken");
    w.spec.desired_state = DesiredState::Stopped;
    assert!(!wants_pod(&w), "explicit stop wins");
}
```

  If `WorkspaceSpec`/`WorkspaceStatus` have no `Default`, build them the way the existing tests in this file do. Add an env equivalent in `crd/environment.rs`:

```rust
#[test]
fn wants_running_table() {
    let mut e = Environment::new("e", EnvironmentSpec::default());
    e.spec.desired_state = DesiredState::Running;
    assert!(wants_running(&e));
    e.status = Some(EnvironmentStatus { idle_since: Some("2026-10-12T10:00:00Z".into()), ..Default::default() });
    assert!(!wants_running(&e));
    e.spec.wake_at = Some("2026-10-12T10:00:01Z".into());
    assert!(wants_running(&e));
}
```

- [ ] **Step 2: Run, expect FAIL (compile errors).**
  Run: `CARGO_TARGET_DIR=/Volumes/kdisk/cargo-target cargo test -p kloudlite-workspaces awake_table wants_pod_ordinary wants_running_table 2>&1 | tail -20`

- [ ] **Step 3: Implement.** Replace `wants_pod` with:

```rust
/// Whether a pod should exist now: asked to run, and not idle-stopped since the last wake. A wake
/// only counts if it came AFTER the idle it is meant to end, so a stale wake never undoes a newer
/// idle verdict. A bench reads its own `bench.wakeAt`; every other workspace reads `spec.wakeAt`.
pub fn wants_pod(w: &Workspace) -> bool {
    if w.spec.desired_state != DesiredState::Running {
        return false;
    }
    let wake_at = match w.spec.bench.as_ref() {
        Some(_) if w.spec.access == Access::Paused => return false,
        Some(b) => b.wake_at.as_deref(),
        None => w.spec.wake_at.as_deref(),
    };
    awake(w.status.as_ref().and_then(|s| s.idle_since.as_deref()), wake_at)
}

/// `idle_since` unset or unparsable: awake. Otherwise only a `wake_at` strictly after it wakes.
pub fn awake(idle_since: Option<&str>, wake_at: Option<&str>) -> bool {
    let parse = |s: &str| chrono::DateTime::parse_from_rfc3339(s).ok();
    let Some(idle) = idle_since.and_then(parse) else { return true };
    wake_at.and_then(parse).is_some_and(|w| w > idle)
}
```

  Before replacing, read the old body. Confirm the bench branch had the same semantics (`None` wake means asleep, and an unparsable `idle_since` means awake). If it differed, keep the bench's old behaviour and record the difference in the report.

  In `environment.rs`, add the fields and this function:

```rust
/// Same rule as `wants_pod`: asked to run and not idle-stopped since the last wake.
pub fn wants_running(e: &Environment) -> bool {
    e.spec.desired_state == DesiredState::Running
        && super::workspace::awake(e.status.as_ref().and_then(|s| s.idle_since.as_deref()), e.spec.wake_at.as_deref())
}
```

  Fix the comment at `crd/workspace.rs:449` ("no idle clock") to say ordinary workspaces now idle-stop on `ws_idle_secs`.

- [ ] **Step 4: Regenerate CRDs and run the tests.**
  Run: `bash -c 'cd /Volumes/kdisk/rustic-git-wt/<wt> && CRD_REGEN=1 CARGO_TARGET_DIR=/Volumes/kdisk/cargo-target cargo test -p kloudlite-workspaces --test crd_yaml 2>&1 | tail -5 && CARGO_TARGET_DIR=/Volumes/kdisk/cargo-target cargo test -p kloudlite-workspaces 2>&1 | grep -E "test result|FAILED|panicked" | head'`
  Expected: all pass. `git diff --stat deploy/k3s/crds.yaml` shows `wakeAt`, `idleSince` and `unfollowedSince` added.

- [ ] **Step 5: Commit.** Add `crd/workspace.rs`, `crd/environment.rs`, `crd/mod.rs` (only if re-exports changed), and `deploy/k3s/crds.yaml`. Message: `Add wake and idle fields to workspaces and environments`.

---

### Task 3: Tool server activity, `/hold`, inbound TCP scan

**Files:**
- Create: `crates/ide/src/activity.rs`
- Modify: `crates/ide/src/lib.rs` (`mod activity;`)
- Modify: `crates/ide/src/server.rs` (App field, route, middleware, boot touch, beat)
- Modify: `crates/ide/src/stream.rs` (stream guard)

**Interfaces:**
- Produces:
  - `pub struct Activity`, built by `Activity::new(path: PathBuf)`;
  - `fn touch(&self)`, which sets the mtime, writes at most once per 30 s, and creates the parent dir and file;
  - `fn touch_now(&self)`, the unthrottled boot write;
  - `fn stream(&self) -> StreamGuard` (RAII; counts while held);
  - `fn busy(&self) -> bool`;
  - `pub fn inbound(tcp: &str, exclude: u16) -> bool`;
  - `App.activity: Arc<Activity>`, with path `cfg.home.join(".kl/activity")`.
- Requests are tracked by an axum middleware `activity::track`.

- [ ] **Step 1: Write the failing tests** in `activity.rs` `#[cfg(test)]`:

```rust
const TCP: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000:1E6C 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 1 1 0000000000000000 100 0 0 10 0
   1: 00000000:0016 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 2 1 0000000000000000 100 0 0 10 0
   2: 0100007F:1E6C 0100007F:9C40 01 00000000:00000000 00:00000000 00000000  1000        0 3 1 0000000000000000 20 4 30 10 -1
";

#[test]
fn inbound_ignores_tool_port_and_listeners() {
    // 0x1E6C = 7788: listening and ESTABLISHED, excluded. 0x0016 = 22: listening only.
    assert!(!inbound(TCP, 7788));
}

#[test]
fn inbound_sees_established_on_another_listening_port() {
    let t = format!("{TCP}   3: 0A2A0005:0016 0A2A0009:D431 01 00000000:00000000 00:00000000 00000000     0        0 4 1 0000000000000000 20 4 30 10 -1\n");
    assert!(inbound(&t, 7788));
}

#[test]
fn outbound_from_a_process_is_not_inbound() {
    // Local port 0xD431 is an ephemeral port, not a listening one: a process dialing out.
    let t = format!("{TCP}   3: 0A2A0005:D431 0A2A0009:01BB 01 00000000:00000000 00:00000000 00000000  1000        0 4 1 0000000000000000 20 4 30 10 -1\n");
    assert!(!inbound(&t, 7788));
}

#[test]
fn touch_is_throttled() {
    let d = tempfile::tempdir().unwrap();
    let a = Activity::new(d.path().join(".kl/activity"));
    a.touch_now();
    let m1 = std::fs::metadata(d.path().join(".kl/activity")).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    a.touch();
    let m2 = std::fs::metadata(d.path().join(".kl/activity")).unwrap().modified().unwrap();
    assert_eq!(m1, m2, "a second write inside 30 s is skipped");
}
```

  In `server.rs` tests, use the existing `traced_app()`/token fixtures. The router must be built from an `App` whose `cfg.home` is the fixture tempdir:

```rust
#[tokio::test]
async fn watcher_and_healthz_do_not_touch() {
    let (dir, app) = traced_app();
    let f = app.cfg.home.join(".kl/activity");
    let _ = std::fs::remove_file(&f);
    let r = router(app.clone());
    r.clone().oneshot(Request::get("/healthz").body(Body::empty()).unwrap()).await.unwrap();
    r.clone().oneshot(authed(Request::get("/fs/changes").header("x-kl-watch", "1")).body(Body::empty()).unwrap()).await.unwrap();
    assert!(!f.exists(), "neither touched the activity file");
    drop(dir);
}

#[tokio::test]
async fn tool_request_and_hold_touch() {
    let (dir, app) = traced_app();
    let f = app.cfg.home.join(".kl/activity");
    let r = router(app.clone());
    let _ = std::fs::remove_file(&f);
    r.clone().oneshot(authed(Request::get("/fs/changes")).body(Body::empty()).unwrap()).await.unwrap();
    assert!(f.exists(), "an ordinary request touches");
    let (dir2, app2) = traced_app();
    let f2 = app2.cfg.home.join(".kl/activity");
    let _ = std::fs::remove_file(&f2);
    router(app2).oneshot(authed(Request::post("/hold")).body(Body::empty()).unwrap()).await.unwrap();
    assert!(f2.exists(), "/hold touches");
    drop((dir, dir2));
}
```

  `authed(builder)` stands for however the existing tests attach the token header. Reuse their helper, or inline `.header("authorization", format!("Bearer {TOKEN}"))`, matching what `auth::require` checks.

- [ ] **Step 2: Run, expect FAIL.**
  Run: `CARGO_TARGET_DIR=/Volumes/kdisk/cargo-target cargo test -p kloudlite-ide inbound touch_ watcher_and tool_request 2>&1 | tail -20`
  If the package name differs, check `crates/ide/Cargo.toml`.

- [ ] **Step 3: Implement `activity.rs`:**

```rust
//! Session activity, measured where every session meets the pod (spec 2026-10-12 idle stop).
//! The agent on the same node reads ONLY the mtime of `~/.kl/activity` from the volume; this
//! process keeps that mtime fresh while anything counts as activity: a request in flight or just
//! ended, an open non-watcher stream, `POST /hold`, or an ESTABLISHED inbound connection on a
//! listening port other than ours (ssh/ide/proxy tunnels, ttyd, intercept traffic).
//! NOT activity: `/healthz`, any request with `X-KL-Watch: 1` (the sidebar and explorer poll every
//! ready workspace), and a detached process (its connections are outbound).
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ponytail: fixed 30 s throttle and beat, no knob; the agent's verdict works in minutes.
pub const THROTTLE: Duration = Duration::from_secs(30);

pub struct Activity {
    path: PathBuf,
    in_flight: AtomicUsize,
    streams: AtomicUsize,
    last: Mutex<Option<Instant>>,
}

pub struct StreamGuard(Arc<Activity>);
impl Drop for StreamGuard {
    fn drop(&mut self) {
        self.0.streams.fetch_sub(1, Ordering::Relaxed);
        self.0.touch();
    }
}

impl Activity {
    pub fn new(path: PathBuf) -> Self {
        Activity { path, in_flight: AtomicUsize::new(0), streams: AtomicUsize::new(0), last: Mutex::new(None) }
    }
    pub fn touch(&self) {
        let mut last = self.last.lock().unwrap_or_else(|p| p.into_inner());
        if last.is_some_and(|t| t.elapsed() < THROTTLE) {
            return;
        }
        *last = Some(Instant::now());
        drop(last);
        self.write();
    }
    pub fn touch_now(&self) {
        *self.last.lock().unwrap_or_else(|p| p.into_inner()) = Some(Instant::now());
        self.write();
    }
    fn write(&self) {
        // Rewriting the file is the portable way to bump an mtime; the content is a debugging aid.
        if let Some(d) = self.path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Err(e) = std::fs::write(&self.path, chrono::Utc::now().to_rfc3339()) {
            tracing::warn!(error = %e, "activity.write_failed");
        }
    }
    pub fn stream(self: &Arc<Self>) -> StreamGuard {
        self.streams.fetch_add(1, Ordering::Relaxed);
        self.touch();
        StreamGuard(self.clone())
    }
    pub fn busy(&self) -> bool {
        self.in_flight.load(Ordering::Relaxed) > 0 || self.streams.load(Ordering::Relaxed) > 0
    }
    /// The 30 s beat: refresh while a request or stream is open, or a tunnel is connected.
    pub async fn beat(self: Arc<Self>, tool_port: u16) {
        let mut t = tokio::time::interval(THROTTLE);
        loop {
            t.tick().await;
            let net = ["/proc/net/tcp", "/proc/net/tcp6"]
                .iter()
                .filter_map(|p| std::fs::read_to_string(p).ok())
                .any(|s| inbound(&s, tool_port));
            if self.busy() || net {
                self.touch();
            }
        }
    }
}

/// True when some ESTABLISHED (st 01) socket's local port is a LISTENING (st 0A) port other than
/// `exclude`. Inbound = accepted on a listener; a process dialing out uses an ephemeral port no
/// one listens on, so it never counts.
pub fn inbound(tcp: &str, exclude: u16) -> bool {
    let rows: Vec<(u16, &str)> = tcp
        .lines()
        .skip(1)
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let local = f.nth(1)?;
            let st = f.nth(1)?;
            let port = u16::from_str_radix(local.rsplit(':').next()?, 16).ok()?;
            Some((port, st))
        })
        .collect();
    let listening: std::collections::HashSet<u16> = rows.iter().filter(|(p, st)| *st == "0A" && *p != exclude).map(|(p, _)| *p).collect();
    rows.iter().any(|(p, st)| *st == "01" && listening.contains(p))
}

/// Counts a request while it runs and touches when it ends. `/healthz` and watchers pass through.
pub async fn track(
    axum::extract::State(a): axum::extract::State<Arc<Activity>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let watcher = req.uri().path() == "/healthz" || req.headers().get("x-kl-watch").is_some_and(|v| v == "1");
    if watcher {
        return next.run(req).await;
    }
    a.in_flight.fetch_add(1, Ordering::Relaxed);
    let res = next.run(req).await;
    a.in_flight.fetch_sub(1, Ordering::Relaxed);
    a.touch();
    res
}
```

  If `chrono` is not a dependency of the ide crate, write `"active"` instead of the timestamp. Do not add a dependency.

  Wiring in `server.rs`:
  - Add `pub activity: Arc<Activity>` to `App`. In `App::new`, build it from `cfg.home.join(".kl/activity")`.
  - Add `.route("/hold", post(hold))` with `async fn hold(State(app): State<Arc<App>>) -> axum::http::StatusCode { app.activity.touch(); StatusCode::NO_CONTENT }`.
  - Add `.layer(axum::middleware::from_fn_with_state(app.activity.clone(), crate::activity::track))` BELOW the auth layer. Auth is outermost, so an unauthenticated request never counts. In axum the last `.layer` call is outermost; put `track` before the auth `.layer` line.
  - The `track` middleware sees the request counted until the response is returned. For a websocket upgrade that is the 101 only. Streams are counted by the guard below.
  - In `serve`: after building `app` and before binding, call `app.activity.touch_now();`. Then `tokio::spawn(app.activity.clone().beat(bind.port()));`.

  Wiring in `stream.rs`:
  - In both upgrade handlers, read `let watcher = headers.get("x-kl-watch").is_some_and(|v| v == "1");` (add a `HeaderMap` extractor).
  - Inside the `on_upgrade` closure, first line: `let _g = (!watcher).then(|| app.activity.stream());`.
  - The guard lives as long as the socket task.

- [ ] **Step 4: Run all ide tests, expect PASS.**
  Run: `CARGO_TARGET_DIR=/Volumes/kdisk/cargo-target cargo test -p kloudlite-ide 2>&1 | grep -E "test result|FAILED|panicked"`

- [ ] **Step 5: Commit.** Add `activity.rs`, `lib.rs`, `server.rs` and `stream.rs`. Message: `Record session activity in the tool server`.

---

### Task 4: `procs.json` and relaunch at boot

**Files:**
- Create: `crates/ide/src/procfile.rs`
- Modify: `crates/ide/src/procs.rs` (`Proc` fields; `spawn` takes an optional id; exit hook)
- Modify: `crates/ide/src/tools/exec.rs` (record args on detached spawn; `command` becomes `pub(crate)`)
- Modify: `crates/ide/src/server.rs` (`serve`: relaunch before binding)
- Modify: `crates/ide/src/lib.rs` (`mod procfile;`)

**Interfaces:**
- Consumes: `exec::command(t: &TreeCtx, args: &Value) -> Result<(Command, String), ToolError>` (exec.rs:49) and `App::tree(Some(name))`.
- Produces:
  - `Procs::spawn(&self, tree: &str, command: Command, cmdline: String) -> Result<String, String>` is unchanged for callers. It delegates to `spawn_as(id: Option<String>, tree, command, cmdline, args: Option<Value>)`.
  - `Proc.args: Option<serde_json::Value>`: the exec request's `{cmd|argv, cwd, env}`. `None` for graft and watch spawns, which are never recorded.
  - `Procs.file: OnceLock<PathBuf>`, set by `serve`; saves are skipped while it is unset, so test Apps write nothing.
  - `procfile::Entry {id, tree, args, started_at, attempts}`;
  - `procfile::save(path, &[Entry])` (temp file, then rename, mode 0600) and `procfile::load(path) -> Vec<Entry>`;
  - `procfile::relaunch(app: &App) -> usize`.

The rules, verbatim from the spec:
- Only `running` processes are listed. An exit or `process_kill` removes the entry.
- The file is current BEFORE stop.
- Relaunch keeps the id. The ring opens with `[restarted <RFC 3339>: workspace was stopped]`.
- Before spawning, increment `attempts` and save. Reset `attempts` to 0 once the process has run 60 s.
- Skip an entry with `attempts >= 3`, and list it as `exited` with failed note `not restarted: failed 3 restarts`.
- Jobs are never recorded.

- [ ] **Step 1: Write the failing tests** in `procfile.rs`:

```rust
#[test]
fn save_load_round_trip_is_0600() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join(".kl/procs.json");
    let e = Entry { id: "p-1".into(), tree: "main".into(), args: serde_json::json!({"cmd": "sleep 600"}), started_at: "1".into(), attempts: 0 };
    save(&p, &[e.clone()]).unwrap();
    assert_eq!(load(&p), vec![e]);
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
}

#[test]
fn load_of_missing_or_garbage_is_empty() {
    let d = tempfile::tempdir().unwrap();
    assert!(load(&d.path().join("nope")).is_empty());
    std::fs::write(d.path().join("bad"), "{").unwrap();
    assert!(load(&d.path().join("bad")).is_empty());
}
```

  In `server.rs` tests (real spawns; tokio multi-thread runtime as the existing exec tests use):

```rust
#[tokio::test(flavor = "multi_thread")]
async fn detached_spawn_exit_and_kill_rewrite_procs_json() {
    let (_d, app) = traced_app();
    let f = app.cfg.home.join(".kl/procs.json");
    app.procs.file.set(f.clone()).unwrap();
    let id = spawn_detached(&app, "sleep 600").await; // via the exec tool with detach: true
    assert!(procfile::load(&f).iter().any(|e| e.id == id), "listed on spawn");
    app.procs.kill("main", &id, "TERM").await.unwrap();
    assert!(!procfile::load(&f).iter().any(|e| e.id == id), "gone on kill");
    let id2 = spawn_detached(&app, "true").await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(!procfile::load(&f).iter().any(|e| e.id == id2), "gone on exit");
}

#[tokio::test(flavor = "multi_thread")]
async fn relaunch_keeps_id_and_marks_ring() {
    let (_d, app) = traced_app();
    let f = app.cfg.home.join(".kl/procs.json");
    app.procs.file.set(f.clone()).unwrap();
    procfile::save(&f, &[procfile::Entry { id: "p-abc".into(), tree: "main".into(), args: serde_json::json!({"cmd": "sleep 600"}), started_at: "1".into(), attempts: 0 }]).unwrap();
    assert_eq!(procfile::relaunch(&app), 1);
    let p = app.procs.get("p-abc").expect("same id");
    let g = p.lock().unwrap();
    let (bytes, _, _) = g.out.read_since(0);
    assert!(String::from_utf8_lossy(&bytes).starts_with("[restarted "), "marker first");
    assert_eq!(procfile::load(&f)[0].attempts, 1, "attempt counted before spawn");
}

#[tokio::test(flavor = "multi_thread")]
async fn relaunch_skips_after_three_attempts() {
    let (_d, app) = traced_app();
    let f = app.cfg.home.join(".kl/procs.json");
    app.procs.file.set(f.clone()).unwrap();
    procfile::save(&f, &[procfile::Entry { id: "p-bad".into(), tree: "main".into(), args: serde_json::json!({"cmd": "false"}), started_at: "1".into(), attempts: 3 }]).unwrap();
    assert_eq!(procfile::relaunch(&app), 0);
    let p = app.procs.get("p-bad").expect("still listed");
    let g = p.lock().unwrap();
    assert_eq!(g.state, State::Exited);
    assert_eq!(g.failed.as_deref(), Some("not restarted: failed 3 restarts"));
}
```

  `spawn_detached(&app, cmd)` is a small test helper. It calls the registry's `exec` tool with `{"cmd": cmd, "detach": true}` the way existing exec tests do, and returns the id from the result JSON.

- [ ] **Step 2: Run, expect FAIL.**
  Run: `CARGO_TARGET_DIR=/Volumes/kdisk/cargo-target cargo test -p kloudlite-ide procs_json relaunch save_load load_of 2>&1 | tail -20`

- [ ] **Step 3: Implement.**
  - **`procs.rs`:**
    - Add `pub args: Option<serde_json::Value>` to `Proc`, and `pub file: std::sync::OnceLock<PathBuf>` to `Procs`.
    - Split `spawn` into `spawn_as(id: Option<String>, …, args)`, which uses `id.unwrap_or_else(|| format!("p-{:x}", rand_u64()))`. `spawn` calls `spawn_as(None, …, None)`.
    - Add `pub fn save(&self)`. It collects every `Proc` with `state == Running && args.is_some()` into `Entry`s and calls `procfile::save`. `attempts` comes from a new `Proc.attempts: u32` field.
    - Log a write error as `tracing::warn!("procs.save_failed")` and never return it to the caller.
    - Call `save()` after insert in `spawn_as`, where the waiter sets `State::Exited`, and at the end of `kill`.
    - Also in the waiter: after 60 s of running, set `attempts = 0` and save. Spawn a `tokio::time::sleep(60s)` task that checks the process is still Running.
    - Add `pub fn exited_note(&self, id, tree, cmd, note)`. It inserts a `Proc` in `State::Exited` with `failed: Some(note)` and empty rings. The existing `KEEP_EXITED_SECS` reaping applies to it.
  - **`exec.rs`:**
    - Where `detach: true` calls `procs.spawn`, call `spawn_as(None, …, Some(json!({ "cmd": args.get("cmd"), "argv": args.get("argv"), "cwd": args.get("cwd"), "env": args.get("env") })))`.
    - Keep only the keys `words_of` reads. Check `words_of` for the exact key names.
    - Make `command` `pub(crate)`.
  - **`procfile.rs`:** add a `//!` header citing spec rule 6 and R3. `Entry` derives `Serialize, Deserialize, Clone, PartialEq, Debug`. Then:

```rust
pub fn save(path: &Path, entries: &[Entry]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(d) = path.parent() { std::fs::create_dir_all(d)?; }
    let tmp = path.with_extension("json.tmp");
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(&serde_json::to_vec(entries)?)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)
}

pub fn load(path: &Path) -> Vec<Entry> {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Boot, before serving: bring back every process that was running when the pod went away.
pub fn relaunch(app: &App) -> usize {
    let Some(path) = app.procs.file.get().cloned() else { return 0 };
    let mut entries = load(&path);
    let mut ok = 0;
    for e in entries.iter_mut() {
        if e.attempts >= 3 {
            app.procs.exited_note(&e.id, &e.tree, &cmd_line(&e.args), "not restarted: failed 3 restarts");
            tracing::info!(id = %e.id, attempts = e.attempts, ok = false, "procs.relaunch");
            continue;
        }
        e.attempts += 1;
        let _ = save(&path, &entries_snapshot(&entries)); // counted BEFORE the spawn, so a crash at spawn still counts
        let started = app.tree(Some(&e.tree)).ok().and_then(|t| crate::tools::exec::command(&t, &e.args).ok()).and_then(|(c, line)| {
            app.procs.spawn_restored(&e.id, &e.tree, c, line, e.args.clone(), e.attempts).ok()
        });
        tracing::info!(id = %e.id, attempts = e.attempts, ok = started.is_some(), "procs.relaunch");
        ok += started.is_some() as usize;
    }
    app.procs.save();
    ok
}
```

  `entries_snapshot` and `cmd_line` are local helpers: a clone, and the `cmd` string or the argv joined with spaces. Don't save from inside the iterator borrow: clone `entries` before calling `save`.

  `Procs::spawn_restored(id, tree, cmd, line, args, attempts)` = `spawn_as(Some(id), …)`, plus:
  - set `attempts`;
  - push `format!("[restarted {}: workspace was stopped]\n", chrono::Utc::now().to_rfc3339())` into `out` before the pumps start;
  - if chrono is absent, use `jiff` or `httpdate` if present, or `std::time` formatted by hand as RFC 3339 UTC.

  A tree that no longer resolves leaves `started` None. The entry stays in the file with its attempts counted, and is skipped after 3.

  In `server.rs` `serve`, after `touch_now()` and before binding:

```rust
let _ = app.procs.file.set(app.cfg.home.join(".kl/procs.json"));
crate::procfile::relaunch(&app);
```

- [ ] **Step 4: Run all ide tests, expect PASS.**
  Run: `CARGO_TARGET_DIR=/Volumes/kdisk/cargo-target cargo test -p kloudlite-ide 2>&1 | grep -E "test result|FAILED|panicked"`

- [ ] **Step 5: Commit.** Add `procfile.rs`, `procs.rs`, `tools/exec.rs`, `server.rs` and `lib.rs`. Message: `Bring detached processes back after a workspace stop`.

---

### Task 5: Agent workspace idle verdict

**Files:**
- Create: `bins/agent/src/controller/workspace/idle.rs`
- Modify: `bins/agent/src/controller/workspace/mod.rs` (`mod idle;`; `:157` branch; verdict on the ready path near `:501`)
- Modify: `bins/agent/src/controller/workspace/lifecycle.rs:12` (`stop_workspace` gains `ready: Condition`)
- Modify: `crates/workspaces/src/k8s/workspace.rs:374` (`worktree_path` becomes `pub`, re-exported from `k8s` if needed)

**Interfaces:**
- Consumes:
  - `crd::IDLE`, `crd::wants_pod` and `WorkspaceSpec.wake_at` (Task 2);
  - `AgentSettings.ws_idle_secs` (Task 1);
  - activity file `~/.kl/activity` (Task 3).
- Produces:
  - `pub(crate) enum Verdict { Keep, Idle }`;
  - `pub(crate) fn idle_verdict(idle_secs: u64, activity: Option<SystemTime>, started: Option<SystemTime>, wake_at: Option<SystemTime>, now: SystemTime) -> Verdict`;
  - `pub(crate) fn activity_mtime(pool: &str, volume: &str, ws: &str) -> Option<SystemTime>`;
  - `pub(crate) fn idle_condition(gen: i64) -> Condition`.

- [ ] **Step 1: Write the failing tests** in `idle.rs`:

```rust
use std::time::{Duration, SystemTime};
const T: u64 = 1800;
fn at(s: u64) -> SystemTime { SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 + s) }

#[test]
fn fresh_activity_keeps() {
    assert_eq!(idle_verdict(T, Some(at(100)), Some(at(0)), None, at(100 + T)), Verdict::Keep);
}
#[test]
fn stale_activity_idles() {
    assert_eq!(idle_verdict(T, Some(at(100)), Some(at(0)), None, at(101 + T)), Verdict::Idle);
}
#[test]
fn missing_file_keeps() {
    assert_eq!(idle_verdict(T, None, Some(at(0)), None, at(10 * T)), Verdict::Keep);
}
#[test]
fn woken_recently_keeps() {
    // Activity is a day old, but a wake 10 s ago and a pod started 5 s ago both count.
    assert_eq!(idle_verdict(T, Some(at(0)), Some(at(86_395)), Some(at(86_390)), at(86_400)), Verdict::Keep);
    assert_eq!(idle_verdict(T, Some(at(0)), None, Some(at(86_390)), at(86_400)), Verdict::Keep);
}
#[test]
fn zero_disables() {
    assert_eq!(idle_verdict(0, Some(at(0)), Some(at(0)), None, at(10 * T)), Verdict::Keep);
}
#[test]
fn reads_mtime_from_volume_path() {
    let d = tempfile::tempdir().unwrap();
    let pool = d.path().to_str().unwrap();
    assert_eq!(activity_mtime(pool, "v1", "w1"), None);
    let p = d.path().join("vol/v1/live/w1/.kl");
    std::fs::create_dir_all(&p).unwrap();
    std::fs::write(p.join("activity"), "x").unwrap();
    assert!(activity_mtime(pool, "v1", "w1").is_some());
}
```

  Bench and builder exclusion is a property of the call site (Step 3). Add this test in the same file:

```rust
#[test]
fn only_ordinary_running_workspaces_are_eligible() {
    let mut w = crd::Workspace::new("w", Default::default());
    w.spec.desired_state = crd::DesiredState::Running;
    assert!(eligible(&w, 1800));
    assert!(!eligible(&w, 0));
    w.spec.bench = Some(Default::default());
    assert!(!eligible(&w, 1800), "bench has its own idle");
}
```

  with `pub(crate) fn eligible(w: &crd::Workspace, idle_secs: u64) -> bool { idle_secs > 0 && !crd::is_bench(w) && w.spec.desired_state == crd::DesiredState::Running }`.
  - Builders are Environments, not Workspaces, so nothing in this file can see one.
  - If `BenchSpec` has no `Default`, build it as `bench.rs` tests do.

- [ ] **Step 2: Run, expect FAIL.**
  Run: `CARGO_TARGET_DIR=/Volumes/kdisk/cargo-target cargo test -p kloudlite-agent idle 2>&1 | tail -20`

- [ ] **Step 3: Implement `idle.rs`:**

```rust
//! Idle stop for ordinary workspaces (spec 2026-10-12): the tool server keeps the mtime of
//! `~/.kl/activity` fresh while any session uses the workspace; this agent reads it from the volume
//! on the same node and, after `ws_idle_secs` of silence, writes `status.idleSince` and stops the
//! pod through the ordinary stop path. A missing file is KEEP: a tool server that predates the
//! change, or one that never came up, is a signal we cannot read, never a reason to stop.
//! Bench has its own clock (`bench.rs`); builders are Environments.
use std::time::{Duration, SystemTime};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Verdict { Keep, Idle }

pub(crate) fn eligible(w: &crd::Workspace, idle_secs: u64) -> bool {
    idle_secs > 0 && !crd::is_bench(w) && w.spec.desired_state == crd::DesiredState::Running
}

pub(crate) fn idle_verdict(idle_secs: u64, activity: Option<SystemTime>, started: Option<SystemTime>, wake_at: Option<SystemTime>, now: SystemTime) -> Verdict {
    let Some(activity) = activity else { return Verdict::Keep };
    if idle_secs == 0 {
        return Verdict::Keep;
    }
    let last = [Some(activity), started, wake_at].into_iter().flatten().max().unwrap_or(activity);
    match now.duration_since(last) {
        Ok(d) if d > Duration::from_secs(idle_secs) => Verdict::Idle,
        _ => Verdict::Keep,
    }
}

pub(crate) fn activity_mtime(pool: &str, volume: &str, ws: &str) -> Option<SystemTime> {
    let p = format!("{}/.kl/activity", kloudlite_workspaces::k8s::worktree_path(pool, volume, ws));
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

pub(crate) fn idle_condition(gen: i64) -> crd::Condition {
    crd::condition("Ready", false, crd::IDLE, "no session for wsIdleSecs; the next connection starts it", gen)
}
```

  Adjust the `crd` and `worktree_path` import paths to the agent's existing imports. Check `bench.rs` for how it names the crate.

  **`lifecycle.rs`:**
  - `stop_workspace(w, prev, gen, ctx)` becomes `stop_workspace(w, prev, gen, ctx, ready: Condition)`.
  - At :77 replace `stopped_condition(gen)` with `ready`.
  - Every existing caller passes `stopped_condition(gen)`. Find them with `grep -rn "stop_workspace(" bins/agent/src`.

  **`mod.rs`:**
  - At :157, replace `if !crd::wants_pod(w) { return park_bench(...) }` with:
    ```rust
    if !crd::wants_pod(w) {
        if crd::is_bench(w) { return park_bench(/* unchanged args */).await; }
        return stop_workspace(w, prev, gen, ctx, idle::idle_condition(gen)).await;
    }
    ```
  - On the ready path, after the pod is observed (~:501, after the bench verdict and before `own_ready_pod`), add:
    ```rust
    if idle::eligible(w, ctx.settings.get().ws_idle_secs) && pod_is_running {
        let started = pod.status.as_ref().and_then(|s| s.start_time.as_ref()).map(|t| SystemTime::from(t.0));
        let wake = w.spec.wake_at.as_deref().and_then(|s| s.parse::<jiff::Timestamp>().ok()).map(SystemTime::from);
        let act = idle::activity_mtime(&ctx.pool, &volume, &name);
        if idle::idle_verdict(idle_secs, act, started, wake, SystemTime::now()) == idle::Verdict::Idle {
            tracing::info!(kind = "workspace", id = %name, idle_secs, last_activity = ?act, "idle.verdict");
            let mut prev = prev;
            prev.idle_since = Some(now_rfc3339());
            return stop_workspace(w, prev, gen, ctx, idle::idle_condition(gen)).await;
        }
    }
    ```
  - Use the names that exist at that point in the function:
    - the observed pod variable;
    - the volume name, as the code uses it for the worktree path;
    - the ws name;
    - the timestamp helper `bench.rs` uses for `idle_since`;
    - the `Time` to `SystemTime` conversion the crate already uses (`k8s_openapi::jiff::Timestamp` per `bench.rs`).
  - `pod_is_running` means the observed pod's phase is Running.
  - Read `ctx.settings` the way `bench.rs` reads `bench_idle_secs`.
  - Confirm `stop_workspace` builds its status with `..prev`, so `idle_since` is written. Then confirm `write_ws_status`'s change detection compares `idle_since`. If it has an equality closure like `write_env_status`, add `idle_since` to it.

- [ ] **Step 4: Run the agent tests and clippy, expect PASS.**
  Run: `bash -c 'CARGO_TARGET_DIR=/Volumes/kdisk/cargo-target cargo test -p kloudlite-agent 2>&1 | grep -E "test result|FAILED|panicked"; CARGO_TARGET_DIR=/Volumes/kdisk/cargo-target cargo clippy -p kloudlite-agent --all-targets -- -D warnings 2>&1 | tail -5'`

- [ ] **Step 5: Commit.** Add `idle.rs`, `mod.rs`, `lifecycle.rs` and `k8s/workspace.rs` (plus `k8s/mod.rs` if a re-export was needed). Message: `Stop ordinary workspaces after ws_idle_secs without activity`.

---

### Task 6: Agent environment follower verdict

**Files:**
- Create: `bins/agent/src/controller/environment/idle.rs`
- Modify: `bins/agent/src/controller/environment/mod.rs:171` (after the Stopped check)
- Modify: `bins/agent/src/controller/environment/stop.rs:9,142` (`stop_environment` gains `ready: Condition`)
- Modify: `bins/agent/src/controller/environment/services.rs:117` (`write_env_status` equality closure)

**Interfaces:**
- Consumes:
  - `crd::wants_running`, `EnvironmentStatus.{idle_since, unfollowed_since}` and `crd::IDLE` (Task 2);
  - `crd::wants_pod` (Task 2);
  - `AgentSettings.env_idle_secs` (Task 1);
  - `controller::space::space_environment(ctx, owner, team, field) -> SpaceEnv{Unknown, Known(Option<Chosen>)}` and `crd::retired_attach(&w.metadata, w.spec.attached_environment.as_deref())`.
- Produces:
  - `pub(crate) enum Followed { Unknown, Yes, No }`;
  - `pub(crate) fn followed(env: &crd::Environment, wss: Option<&[Arc<crd::Workspace>]>, resolve: impl Fn(&crd::Workspace) -> Option<Option<String>>, now: SystemTime, env_idle_secs: u64) -> Followed`. `resolve` returns `None` when unknown, `Some(None)` when the workspace has no space env, and `Some(Some(id))` otherwise.
  - `pub(crate) fn step(prev_unfollowed: Option<&str>, f: Followed, now: SystemTime, env_idle_secs: u64) -> (Option<String>, bool)`, which returns the new `unfollowed_since` and whether the env is idle.

- [ ] **Step 1: Write the failing tests** in `idle.rs`. Build the fixtures with the existing helpers in `environment` tests, or `Workspace::new`/`Environment::new`:

```rust
fn running_ws(name: &str) -> Arc<crd::Workspace> { /* desired Running, phase Running, no idle_since */ }
fn env_with_intercept(ws: &str) -> crd::Environment { /* spec.intercepts = [Intercept{service:"api", workspace: ws.into(), ports: vec![]}] */ }
const S: u64 = 300;
fn now() -> SystemTime { SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000) }

#[test]
fn unknown_cache_keeps() {
    let e = env_with_intercept("w1");
    assert_eq!(followed(&e, None, |_| Some(None), now(), S), Followed::Unknown);
}
#[test]
fn follower_via_space() {
    let e = crd::Environment::new("e1", Default::default());
    let w = [running_ws("w1")];
    assert_eq!(followed(&e, Some(&w), |_| Some(Some("e1".into())), now(), S), Followed::Yes);
    assert_eq!(followed(&e, Some(&w), |_| Some(Some("e2".into())), now(), S), Followed::No);
}
#[test]
fn space_unknown_is_unknown() {
    let e = crd::Environment::new("e1", Default::default());
    let w = [running_ws("w1")];
    assert_eq!(followed(&e, Some(&w), |_| None, now(), S), Followed::Unknown);
}
#[test]
fn follower_via_intercept() {
    let e = env_with_intercept("w1");
    let w = [running_ws("w1")];
    assert_eq!(followed(&e, Some(&w), |_| Some(None), now(), S), Followed::Yes);
}
#[test]
fn idle_stopped_workspace_is_not_a_follower() {
    let e = env_with_intercept("w1");
    let mut w = (*running_ws("w1")).clone();
    w.status.get_or_insert_with(Default::default).idle_since = Some("2026-10-12T10:00:00Z".into());
    assert_eq!(followed(&e, Some(&[Arc::new(w)]), |_| Some(None), now(), S), Followed::No);
}
#[test]
fn follower_via_hold() {
    let mut e = crd::Environment::new("e1", Default::default());
    let held = jiff::Timestamp::try_from(now() - Duration::from_secs(10)).unwrap().to_string();
    e.metadata.annotations = Some([("kloudlite.io/held-at".to_string(), held)].into());
    assert_eq!(followed(&e, Some(&[]), |_| Some(None), now(), S), Followed::Yes);
    let old = jiff::Timestamp::try_from(now() - Duration::from_secs(S + 1)).unwrap().to_string();
    e.metadata.annotations = Some([("kloudlite.io/held-at".to_string(), old)].into());
    assert_eq!(followed(&e, Some(&[]), |_| Some(None), now(), S), Followed::No);
}
#[test]
fn step_sets_clears_and_idles() {
    let (u, idle) = step(None, Followed::No, now(), S);
    assert!(u.is_some() && !idle, "first unfollowed beat starts the clock");
    let since = jiff::Timestamp::try_from(now() - Duration::from_secs(S + 1)).unwrap().to_string();
    assert_eq!(step(Some(&since), Followed::No, now(), S), (Some(since.clone()), true));
    assert_eq!(step(Some(&since), Followed::Yes, now(), S), (None, false), "a follower clears it");
    assert_eq!(step(Some(&since), Followed::Unknown, now(), S), (Some(since.clone()), false), "unknown keeps the clock, never idles");
}
```

  The system env exclusion is pinned by `eligible`:

```rust
#[test]
fn system_env_is_not_eligible() {
    let mut e = crd::Environment::new("bld-x", Default::default());
    e.spec.desired_state = crd::DesiredState::Running;
    assert!(eligible(&e, 300));
    e.spec.system = Some(crd::BUILDER_SYSTEM.into());
    assert!(!eligible(&e, 300));
    e.spec.system = None;
    assert!(!eligible(&e, 0));
}
```

- [ ] **Step 2: Run, expect FAIL.**
  Run: `CARGO_TARGET_DIR=/Volumes/kdisk/cargo-target cargo test -p kloudlite-agent environment::idle 2>&1 | tail -20`

- [ ] **Step 3: Implement `idle.rs`:**

```rust
//! Idle stop for environments (spec 2026-10-12). An environment's users are workspaces and
//! laptops: a FOLLOWER is a running, not-idle workspace whose space resolves here, a running,
//! not-idle workspace named in `spec.intercepts`, or a `kloudlite.io/held-at` annotation newer
//! than `env_idle_secs` (`kl-connect env` holds every 60 s). No follower starts
//! `status.unfollowedSince`; `env_idle_secs` later the env is idle and stops through the ordinary
//! path. An UNKNOWN cache is never "no follower" (CLAUDE.md: a reflector before its first list is
//! unknown, never empty). System envs (builders) have their own idle in builder-gate.
pub(crate) const HELD_AT: &str = "kloudlite.io/held-at";

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Followed { Unknown, Yes, No }

pub(crate) fn eligible(e: &crd::Environment, idle_secs: u64) -> bool {
    idle_secs > 0 && e.spec.system.is_none() && e.spec.desired_state == crd::DesiredState::Running
}

fn ts(s: &str) -> Option<SystemTime> { s.parse::<jiff::Timestamp>().ok().map(SystemTime::from) }

fn live(w: &crd::Workspace) -> bool {
    crd::wants_pod(w) && w.status.as_ref().is_some_and(|s| s.phase == crd::Phase::Running)
}

pub(crate) fn followed(e: &crd::Environment, wss: Option<&[Arc<crd::Workspace>]>, resolve: impl Fn(&crd::Workspace) -> Option<Option<String>>, now: SystemTime, idle_secs: u64) -> Followed {
    let held = e.metadata.annotations.as_ref().and_then(|a| a.get(HELD_AT)).and_then(|s| ts(s));
    if held.is_some_and(|h| now.duration_since(h).map_or(true, |d| d.as_secs() <= idle_secs)) {
        return Followed::Yes;
    }
    let Some(wss) = wss else { return Followed::Unknown };
    let name = e.metadata.name.as_deref().unwrap_or_default();
    let mut unknown = false;
    for w in wss.iter().filter(|w| live(w)) {
        let wn = w.metadata.name.as_deref().unwrap_or_default();
        if e.spec.intercepts.iter().any(|i| i.workspace == wn) {
            return Followed::Yes;
        }
        match resolve(w) {
            None => unknown = true,
            Some(Some(id)) if id == name => return Followed::Yes,
            Some(_) => {}
        }
    }
    if unknown { Followed::Unknown } else { Followed::No }
}

pub(crate) fn step(prev: Option<&str>, f: Followed, now: SystemTime, idle_secs: u64) -> (Option<String>, bool) {
    match f {
        Followed::Yes => (None, false),
        Followed::Unknown => (prev.map(str::to_string), false),
        Followed::No => {
            let since = prev.map(str::to_string).unwrap_or_else(|| jiff::Timestamp::try_from(now).map(|t| t.to_string()).unwrap_or_default());
            let idle = ts(&since).is_some_and(|s| now.duration_since(s).is_ok_and(|d| d.as_secs() > idle_secs));
            (Some(since), idle)
        }
    }
}
```

  Use the crate's actual paths:
  - `jiff` may be `k8s_openapi::jiff`;
  - `Phase` and `BUILDER_SYSTEM` live under `crd`.

  In `live`, check against the real enum for the workspace phase name.

  **`stop.rs`:**
  - `stop_environment(…, ready: Condition)`. At :142 use `ready` instead of `stopped_condition(gen)`.
  - The existing caller passes `stopped_condition(gen)`.
  - Confirm the final status is built with `..prev` (or copies `idle_since`). If it builds a fresh status, carry `idle_since: prev.idle_since.clone()` over explicitly.

  **`services.rs:117`:** add `idle_since` and `unfollowed_since` to the equality closure, so a status write that only changes them is not skipped as a no-op.

  **`mod.rs` (right after the `desired == Stopped` return at :171):**

```rust
let idle_secs = ctx.settings.get().env_idle_secs;
if !crd::wants_running(e) {
    return stop_environment(e, &vol, &ns, &deployments, prev, gen, ctx, idle::idle_condition(gen)).await;
}
if idle::eligible(e, idle_secs) && prev.phase == crd::Phase::Running {
    let wss = ctx.workspaces();
    let f = idle::followed(e, wss.as_deref(), |w| match space_environment(ctx, owner_of(w), team_of(w), crd::retired_attach(&w.metadata, w.spec.attached_environment.as_deref())) {
        SpaceEnv::Unknown => None,
        SpaceEnv::Known(c) => Some(c.map(|c| c.environment)),
    }, SystemTime::now(), idle_secs);
    let (unfollowed, idle) = idle::step(prev.unfollowed_since.as_deref(), f, SystemTime::now(), idle_secs);
    if idle {
        tracing::info!(kind = "environment", id = %name, idle_secs, last_activity = ?prev.unfollowed_since, "idle.verdict");
        let prev = crd::EnvironmentStatus { idle_since: Some(now_rfc3339()), unfollowed_since: unfollowed, ..prev };
        return stop_environment(e, &vol, &ns, &deployments, prev, gen, ctx, idle::idle_condition(gen)).await;
    }
    if unfollowed != prev.unfollowed_since {
        prev.unfollowed_since = unfollowed; // carried by the `..prev` status writes below
    }
}
```

  - `idle::idle_condition(gen)` is `crd::condition("Ready", false, crd::IDLE, "no follower for envIdleSecs; starting a follower starts it", gen)`.
  - `owner_of`/`team_of` stand for how the attach renderer passes owner/team to `space_environment`. Find that caller with `grep -rn "space_environment(" bins/agent/src`, and copy its argument expressions exactly.
  - If `wss` is a guard type rather than an `Option<Vec<Arc<_>>>`, adapt the signature of `followed` to what `Ctx::workspaces()` returns. Keep `None` meaning Unknown.
  - Confirm `prev` is mutable at this point, and that the Running path's status write uses `..prev` so `unfollowed_since` is persisted. Per the summary, `run.rs` writes use `..prev.clone()`.

- [ ] **Step 4: Run the agent tests and clippy, expect PASS.**
  Run: `bash -c 'CARGO_TARGET_DIR=/Volumes/kdisk/cargo-target cargo test -p kloudlite-agent 2>&1 | grep -E "test result|FAILED|panicked"; CARGO_TARGET_DIR=/Volumes/kdisk/cargo-target cargo clippy -p kloudlite-agent --all-targets -- -D warnings 2>&1 | tail -5'`

- [ ] **Step 5: Commit.** Add `idle.rs`, `mod.rs`, `stop.rs` and `services.rs` under `environment/`. Message: `Stop environments env_idle_secs after their last follower`.

---

### Task 7: API wake, follower env wake, env hold, `idle_since` in docs

**Files:**
- Modify: `crates/workspaces/src/api/workspaces/mod.rs` (`ws_doc` :76, `start_ws` :678)
- Modify: `crates/workspaces/src/api/environments/mod.rs` (`env_doc` :32, `start_env` :386, new `hold_env`)
- Modify: `crates/workspaces/src/api/mod.rs` (route tables near :110/:141/:153, router near :432)
- Modify: the `Workspace`/`Environment` doc structs (wherever `ws_doc`/`env_doc` return types live) to add `idle_since: Option<String>` (`skip_serializing_if = "Option::is_none"`).

**Interfaces:**
- Consumes: `crd::wants_pod`, `crd::wants_running`, `wake_at`, `idle_since` (Task 2).
- Produces:
  - `pub(crate) fn wake_at(now: jiff::Timestamp, idle_since: Option<&str>) -> String`;
  - `pub(crate) fn wake_patch(at: &str) -> serde_json::Value`, which returns `{"spec":{"desiredState":"Running","wakeAt":at}}`;
  - route `POST /v1/environments/{id}/hold`, answering 204;
  - doc field `idle_since`.

- [ ] **Step 1: Write the failing tests** next to the existing `start_ws`/`ws_doc` tests:

```rust
#[test]
fn wake_at_after_idle() {
    let now: jiff::Timestamp = "2026-10-12T10:00:00Z".parse().unwrap();
    assert_eq!(wake_at(now, None), "2026-10-12T10:00:00Z");
    assert_eq!(wake_at(now, Some("2026-10-12T09:00:00Z")), "2026-10-12T10:00:00Z");
    // agent clock ahead of ours: still strictly after the idle
    assert_eq!(wake_at(now, Some("2026-10-12T10:00:05Z")), "2026-10-12T10:00:06Z");
    assert_eq!(wake_at(now, Some("garbage")), "2026-10-12T10:00:00Z");
}

#[test]
fn ws_doc_shows_idle_since_only_when_idle_stopped() {
    let mut w = /* fixture used by existing ws_doc tests, desired Running */;
    w.status.get_or_insert_with(Default::default).idle_since = Some("2026-10-12T10:00:00Z".into());
    assert_eq!(ws_doc(&w, &Default::default()).idle_since.as_deref(), Some("2026-10-12T10:00:00Z"));
    w.spec.desired_state = crd::DesiredState::Stopped;
    assert_eq!(ws_doc(&w, &Default::default()).idle_since, None, "explicit stop hides it (R2)");
    w.spec.desired_state = crd::DesiredState::Running;
    w.spec.wake_at = Some("2026-10-12T10:00:01Z".into());
    assert_eq!(ws_doc(&w, &Default::default()).idle_since, None, "woken");
}
```

  Add the same doc test for `env_doc`.

  Then a router test for hold, using whatever fake-kube harness the existing environment handler tests use. Find it with `grep -rn "async fn .*env_get\|mock\|tower_test" crates/workspaces/src/api/environments`:
  - A member's `POST /v1/environments/e1/hold` answers 204. The single PATCH it sends carries ONLY `{"metadata":{"annotations":{"kloudlite.io/held-at": <rfc3339>}}}`.
  - A non-member gets the same refusal `env_get` gives (404 or 403, whichever `env_get` returns).
  - If no fake-kube harness exists for these handlers, unit-test a pure `fn hold_patch(at: &str) -> Value` for the exact body. Write the access check as a call to the same function `env_get` uses, so the call itself shows the access rule. Note that in the report.

  Start of a follower wakes its env: split the decision from the IO as a pure function and test it:

```rust
#[test]
fn follower_envs_to_wake() {
    // ws w1's space env is e1 (idle-stopped); e2 intercepts through w1 (idle-stopped); e3 intercepts w1 but runs.
    let envs = vec![idle_env("e1", None), idle_env("e2", Some("w1")), running_env("e3", Some("w1"))];
    assert_eq!(envs_to_wake("w1", Some("e1"), &envs), vec!["e1".to_string(), "e2".to_string()]);
    assert!(envs_to_wake("w1", None, &[running_env("e3", Some("w1"))]).is_empty());
}
```

  `envs_to_wake(ws: &str, space_env: Option<&str>, envs: &[crd::Environment]) -> Vec<String>` returns the envs that are `desiredState == Running && !wants_running`, and either equal `space_env` or have an intercept naming `ws`. `idle_env`/`running_env` are small local fixtures.

- [ ] **Step 2: Run, expect FAIL.**
  Run: `CARGO_TARGET_DIR=/Volumes/kdisk/cargo-target cargo test -p kloudlite-workspaces wake_at_after ws_doc_shows env_doc_shows follower_envs hold 2>&1 | tail -20`

- [ ] **Step 3: Implement.**

```rust
/// `max(now, idleSince + 1 s)`: a wake must land strictly after the idle verdict it ends, even
/// when the agent's clock runs ahead of ours.
pub(crate) fn wake_at(now: jiff::Timestamp, idle_since: Option<&str>) -> String {
    let after = idle_since.and_then(|s| s.parse::<jiff::Timestamp>().ok()).and_then(|t| t.checked_add(jiff::SignedDuration::from_secs(1)).ok());
    after.filter(|a| *a > now).unwrap_or(now).to_string()
}
pub(crate) fn wake_patch(at: &str) -> serde_json::Value {
    serde_json::json!({ "spec": { "desiredState": "Running", "wakeAt": at } })
}
```

  `jiff`'s `to_string()` prints `2026-10-12T10:00:00Z` for whole seconds. If the crate uses chrono instead, keep the same output shape: `to_rfc3339_opts(SecondsFormat::AutoSi, true)`.

  **`start_ws`:**
  - Keep `caller_for` → `my_ws` → interrupted 409 → `guard_fill` exactly as they are.
  - Replace `set_desired(...)` with a merge patch of `wake_patch(&wake_at(now, w.status idle_since))`. Copy the patch call shape from `set_desired` at :132.
  - Log `tracing::info!(kind = "workspace", id, by = "start", "idle.wake")` only when the ws doc would have shown `idle_since`.
  - Then wake followers:
    - list environments with the same client and access the handler already holds (`Api::<crd::Environment>::all`);
    - resolve the workspace's space env with the api's existing space lookup (`grep -rn "fn space\|space_env\|current_space" crates/workspaces/src/api`; use the same function `me.rs`/`space_env_current` uses);
    - call `envs_to_wake`;
    - merge-patch `wake_patch(&wake_at(now, env idle_since))` on each, logging `idle.wake {kind:"environment", id, by:"follower"}`.
  - Errors on the env side are logged and never fail the workspace start (spec: "the workspace still starts").

  **`start_env`:** patch `wake_patch(&wake_at(now, idle_since))` instead of the bare desired state.

  **`hold_env` (new):**
  - Same caller/access path as `env_get`. Copy its first lines.
  - Then merge-patch `{"metadata":{"annotations":{"kloudlite.io/held-at": now}}}` and return `StatusCode::NO_CONTENT`.
  - Add the route `.route("/v1/environments/{id}/hold", post(hold_env))`.
  - Add the path to every route table in `api/mod.rs` that lists `/v1/environments/{id}/start`. Those tables are held by tests: the ingress allow-list, the method table and `workspace_tool_route`. Mirror exactly what `start` has in each.

  **Docs:** in `ws_doc`, set `idle_since: (w.spec.desired_state == DesiredState::Running && !crd::wants_pod(w)).then(|| w.status idle_since).flatten()`. Do the same in `env_doc` with `wants_running`.

  Benches keep their own doc (`bench_doc`). Do not touch it.

- [ ] **Step 4: Run the package tests and clippy, expect PASS.**
  Run: `bash -c 'CARGO_TARGET_DIR=/Volumes/kdisk/cargo-target cargo test -p kloudlite-workspaces 2>&1 | grep -E "test result|FAILED|panicked"; CARGO_TARGET_DIR=/Volumes/kdisk/cargo-target cargo clippy -p kloudlite-workspaces --all-targets -- -D warnings 2>&1 | tail -5'`
  Also run the web types if the API doc types are mirrored in `web/` (`grep -rn "idle_since\|interface Workspace " web/apps/web/lib | head`). The field is optional, so no web change is required.

- [ ] **Step 5: Commit.** Add the touched API files. Message: `Wake idle-stopped workspaces and environments on start`.

---

### Task 8: Harness `ensureRunning`, pod retry, watch header, hold beat

**Files:**
- Create: `harness/packages/tools/src/ensure.ts`
- Create: `harness/packages/tools/src/ensure.test.ts`
- Modify: `harness/packages/tools/src/pod.ts` (`podFetch` header, `call` retry after wake, `touched`, `podHold`)
- Modify: `harness/packages/tools/src/platform.ts` (env tools near :250-261 and :288-292 call `ensureRunning`)
- Modify: `harness/packages/tools/src/index.ts` (exports)
- Modify: `harness/packages/backend/src/local.ts` (`#busy` toggle near :470: turn start times; `oldestTurnStart()`)
- Modify: `harness/packages/backend/src/daemon.ts` (beat near :83-92)

**Interfaces:**
- Consumes: `apiJson<T>(method, path, body?)` and `seg` (`platform.ts`); `NotReady` and `lookup` (`pod.ts`).
- Produces:
  - `export async function ensureRunning(kind: "workspaces" | "environments", id: string, o?: { api?: typeof apiJson; sleep?: (ms: number) => Promise<void>; timeoutMs?: number }): Promise<void>`;
  - `export const touched: Map<string, number>`;
  - `export async function podHold(ws: string): Promise<void>`;
  - `LocalBackend.oldestTurnStart(): number | undefined`.

- [ ] **Step 1: Write the failing tests** (`ensure.test.ts`, bun):

```ts
import { describe, expect, test } from "bun:test";
import { ensureRunning } from "./ensure.ts";

const doc = (o: any) => o;
function fakeApi(seq: any[]) {
  const calls: string[] = [];
  const api = async (method: string, path: string) => {
    calls.push(`${method} ${path}`);
    if (method === "POST") return {} as any;
    return (seq.length > 1 ? seq.shift() : seq[0]) as any;
  };
  return { api, calls };
}
const sleep = async () => {};

describe("ensureRunning", () => {
  test("ready returns without starting", async () => {
    const f = fakeApi([doc({ state: "running", ready: true })]);
    await ensureRunning("workspaces", "w1", { api: f.api as any, sleep });
    expect(f.calls).toEqual(["GET /v1/workspaces/w1"]);
  });
  test("idle-stopped wakes and waits", async () => {
    const f = fakeApi([doc({ state: "stopped", idle_since: "2026-10-12T10:00:00Z" }), doc({ state: "starting" }), doc({ state: "running", ready: true })]);
    await ensureRunning("workspaces", "w1", { api: f.api as any, sleep });
    expect(f.calls).toEqual(["GET /v1/workspaces/w1", "POST /v1/workspaces/w1/start", "GET /v1/workspaces/w1", "GET /v1/workspaces/w1"]);
  });
  test("explicitly stopped is an error naming workspace_start", async () => {
    const f = fakeApi([doc({ state: "stopped" })]);
    await expect(ensureRunning("workspaces", "w1", { api: f.api as any, sleep })).rejects.toThrow("stopped; start it with `workspace_start`");
    expect(f.calls).toEqual(["GET /v1/workspaces/w1"]);
  });
  test("timeout names the last condition reason", async () => {
    const f = fakeApi([doc({ state: "stopped", idle_since: "x" }), doc({ state: "starting", conditions: [{ type: "Ready", status: "False", reason: "KeysNotReady" }] })]);
    await expect(ensureRunning("workspaces", "w1", { api: f.api as any, sleep, timeoutMs: 0 })).rejects.toThrow("KeysNotReady");
  });
});
```

  Before writing the predicate, read the real `/v1` workspace and environment doc shapes. Look at `ws_doc`'s return struct, or `withReady` in `platform.ts:96`, which already reads readiness. Use the same fields `withReady` uses for "ready" and for the condition reason. If they differ from `state`/`ready`/`conditions`, change the fixture fields to match, not the logic.

  In the `pod.ts` tests (existing `pod.test.ts` if present, else new):

```ts
test("podFetch marks itself a watcher", async () => { /* stub global fetch; podGet("w1","/healthz"); expect header x-kl-watch === "1" */ });
test("a tool call that hits NotReady wakes and retries once", async () => { /* lookup fails NotReady first, ensureRunning stub resolves, second lookup ok; expect exactly one ensureRunning and the call result */ });
test("podHold never wakes", async () => { /* lookup throws NotReady; podHold resolves; ensureRunning stub never called */ });
```

  Give `podTools` an injectable `ensure` option (default `ensureRunning`), the same way `podTools` already takes test seams. Check its signature first. If it takes none, add a trailing `o: { ensure?: typeof ensureRunning } = {}`.

  Hold-beat target selection, in a backend test:

```ts
test("hold targets only workspaces touched since the oldest running turn", () => {
  expect(holdTargets(new Map([["w1", 100], ["w2", 50]]), 80)).toEqual(["w1"]);
  expect(holdTargets(new Map([["w1", 100]]), undefined)).toEqual([]);
});
```

  `holdTargets(touched, oldest)` is a pure export from `daemon.ts`, or a small new `hold.ts` if `daemon.ts` has side effects at import.

- [ ] **Step 2: Run, expect FAIL.**
  Run: `bash -c 'cd /Volumes/kdisk/rustic-git-wt/<wt>/harness && bun test packages/tools/src/ensure.test.ts packages/tools/src/pod 2>&1 | tail -20'`

- [ ] **Step 3: Implement.**

```ts
// ensure.ts
/** One door for every entry point (spec 2026-10-12): ready returns; idle-stopped is started and
 * waited for; a workspace a person stopped stays stopped (R2) and says how to start it. */
import { apiJson, seg } from "./platform.ts";

// ponytail: fixed 2 s poll and 180 s wake budget (cold start includes `nix copy`); a knob if anyone waits longer.
const POLL_MS = 2_000;
const TIMEOUT_MS = 180_000;

export async function ensureRunning(kind: "workspaces" | "environments", id: string, o: { api?: typeof apiJson; sleep?: (ms: number) => Promise<void>; timeoutMs?: number } = {}): Promise<void> {
  const api = o.api ?? apiJson;
  const sleep = o.sleep ?? ((ms) => new Promise((r) => setTimeout(r, ms)));
  const path = `/v1/${kind}/${seg(id)}`;
  let d: any = await api("GET", path);
  if (ready(d)) return;
  if (d.state === "stopped") {
    if (!d.idle_since) throw new Error(`${id} is stopped; start it with \`${kind === "workspaces" ? "workspace_start" : "env_start"}\``);
    await api("POST", `${path}/start`);
  }
  const until = Date.now() + (o.timeoutMs ?? TIMEOUT_MS);
  for (;;) {
    d = await api("GET", path);
    if (ready(d)) return;
    if (Date.now() >= until) throw new Error(`${id} did not become ready: ${reason(d)}`);
    await sleep(POLL_MS);
  }
}
const ready = (d: any) => /* same predicate withReady uses */ d?.ready === true || d?.state === "running";
const reason = (d: any) => (d?.conditions ?? []).find((c: any) => c.status === "False")?.reason ?? d?.state ?? "unknown";
```

  For environments, the explicit-stop hint names `env_start`, the tool that exists in `platform.ts` (check the actual name). The spec's R2 text names `workspace_start` for workspaces. The R2 test asserts the workspace string.

  **`pod.ts`:**
  - `podFetch` adds `"x-kl-watch": "1"` to its headers (Ruling P2).
  - `export const touched = new Map<string, number>()`. `call` does `touched.set(ws, Date.now())` before posting.
  - In `call`, on `NotReady` from the lookup/post, wake once:
    - `await (o.ensure ?? ensureRunning)("workspaces", ws)`;
    - `cache = undefined`;
    - retry the post once;
    - log `idle.wake` client-side is unnecessary; the api logs it with `by = "start"`.
  - Keep the existing 401 retry intact. The wake retry is separate and happens at most once per call.

```ts
/** Keeps a READY workspace from idling while a turn that needs it runs elsewhere. Never wakes:
 * a stopped workspace stays stopped until a real call needs it. Errors are dropped. */
export async function podHold(ws: string): Promise<void> {
  try {
    const a = await lookup(ws);
    await fetch(`http://${a.address}/hold`, { method: "POST", headers: headers(a), signal: AbortSignal.timeout(5_000) });
  } catch {}
}
```

  **`platform.ts` env tools:**
  - Before the call they make, the env tools that act on a running env (`env_get`, service logs and the like) `await ensureRunning("environments", id)`.
  - Exclude `env_start`/`env_stop` themselves.

  **`local.ts`:**
  - Next to `#busy`, add `#turnStart = new Map<string, number>()`.
  - Where the busy toggle runs (~:470): `on ? this.#turnStart.set(key, Date.now()) : this.#turnStart.delete(key)`.
  - Add `oldestTurnStart(): number | undefined { return this.#turnStart.size ? Math.min(...this.#turnStart.values()) : undefined; }`.

  **`daemon.ts` beat:**
  - Count ticks. Every 12th 5 s tick (60 s), while `b.busyCount > 0`, run `for (const ws of holdTargets(touched, b.oldestTurnStart())) void podHold(ws);`.
  - `holdTargets(t, oldest) = oldest === undefined ? [] : [...t].filter(([, ms]) => ms >= oldest).map(([ws]) => ws)`.
  - Add a `// ponytail: fixed 60 s hold beat` comment.
  - The beat already knows the backend `b`. If `oldestTurnStart` is not on the `Backend` interface, add it as optional and call it with `?.`.

- [ ] **Step 4: Run the harness tests and typecheck, expect PASS.**
  Run: `bash -c 'cd /Volumes/kdisk/rustic-git-wt/<wt>/harness && bun test 2>&1 | tail -8 && bunx tsc --noEmit -p . 2>&1 | tail -5'`
  If `harness/` has no root tsconfig, use the command its `package.json` `typecheck` script runs.

- [ ] **Step 5: Commit.** Add the named harness files. Message: `Wake idle workspaces from the harness and hold busy ones`.

---

### Task 9: kl-connect `ensure_running` and env hold

**Files:**
- Modify: `bins/kl-connect/src/api.rs` (new `ensure_running`, `hold_env`)
- Modify: `bins/kl-connect/src/ws.rs:35`, `proxy.rs:14` and the `ssh` entry (the caller of `ssh_session` at `api.rs:93`) to call `ensure_running` before opening `/tunnel/{ws}`
- Modify: `bins/kl-connect/src/env/mod.rs:47` (`connect`) and `env/tunnel.rs:49` (hold beat while the tunnel is open)

**Interfaces:**
- Consumes: `api::json<T>`, `api::client()`, `Config`, the `/v1` doc `idle_since` (Task 7), `POST /v1/environments/{id}/hold` (Task 7).
- Produces:
  - `pub async fn ensure_running(cfg: &Config, kind: Kind, id: &str) -> Result<(), Error>`, with `pub enum Kind { Workspace, Environment }`;
  - `pub fn decide(doc: &serde_json::Value) -> Step`, where `Step = Ready | Wake | Stopped | Wait(String)`;
  - `pub async fn hold_env(cfg, id) -> Result<(), Error>`.

- [ ] **Step 1: Write the failing tests** in `api.rs` `#[cfg(test)]`. Decide with the same predicate as the harness. Read the doc fields from the real `ws_doc` struct:

```rust
#[test]
fn decide_table() {
    use serde_json::json;
    assert_eq!(decide(&json!({"state": "running", "ready": true})), Step::Ready);
    assert_eq!(decide(&json!({"state": "stopped", "idle_since": "2026-10-12T10:00:00Z"})), Step::Wake);
    assert_eq!(decide(&json!({"state": "stopped"})), Step::Stopped);
    assert_eq!(decide(&json!({"state": "starting", "conditions": [{"type": "Ready", "status": "False", "reason": "KeysNotReady"}]})), Step::Wait("KeysNotReady".into()));
}
```

- [ ] **Step 2: Run, expect FAIL.**
  Run: `CARGO_TARGET_DIR=/Volumes/kdisk/cargo-target cargo test -p kl-connect decide_table 2>&1 | tail -10`
  Check the package name in `bins/kl-connect/Cargo.toml`.

- [ ] **Step 3: Implement.**
  - `decide` is pure.
  - `ensure_running`:
    - GET `/v1/{workspaces|environments}/{id}`.
    - `Ready` returns.
    - `Stopped` returns `Error` with message `"{id} is stopped; start it with `kl-connect ws start {id}`"`. If kl-connect has no start command, say `"start it from the console"`.
    - `Wake` POSTs `/start` (empty body), then loops: GET every 2 s until `Ready`, for up to 180 s.
    - On timeout, return `Error` naming the last `Wait` reason.
    - Print one line to stderr when waking: `waking {id}…`.
  - Add the `ponytail:` comment on the constants.
  - Call it in `ws.rs`, `proxy.rs` and the ssh path before the tunnel request, with `Kind::Workspace`. In `env/mod.rs` `connect`, call it with `Kind::Environment`.
  - `env/tunnel.rs`: while the tunnel task runs, spawn a 60 s interval that calls `hold_env`, ignoring errors (log at debug). Abort that task when the tunnel ends, by holding its `JoinHandle` and calling `abort()` in the same scope that ends the tunnel.

- [ ] **Step 4: Run kl-connect tests and clippy, expect PASS.**
  Run: `bash -c 'CARGO_TARGET_DIR=/Volumes/kdisk/cargo-target cargo test -p kl-connect 2>&1 | grep -E "test result|FAILED|panicked"; CARGO_TARGET_DIR=/Volumes/kdisk/cargo-target cargo clippy -p kl-connect --all-targets -- -D warnings 2>&1 | tail -5'`

- [ ] **Step 5: Commit.** Add the named kl-connect files. Message: `Wake idle workspaces and hold environments from kl-connect`.

---

### Task 10: Docs

**Files:**
- Modify: `CLAUDE.md`
- Modify: `crates/ide/src/lib.rs` `//!` header (activity, `/hold`, `procs.json`)

- [ ] **Step 1: Write the CLAUDE.md changes.**
  - In the **Intercept** paragraph, replace "A stopped/deleted/unreachable workspace releases after `INTERCEPT_GRACE_SECS` (…)" with: "A stopped or deleted workspace releases at once (`Off("WorkspaceStopped")`); an unreachable or NotReady one releases after `INTERCEPT_GRACE_SECS` (…)". Keep the parenthetical about where the grace is measured from.
  - Add a paragraph after **Attach**:

```markdown
**Idle stop** (spec 2026-10-12): the tool server keeps the mtime of `~/.kl/activity` fresh while a
request is in flight or just ended, a non-watcher stream is open, `POST /hold` arrived, or an inbound
ESTABLISHED connection exists on a listening port other than 7788. `/healthz` and `X-KL-Watch: 1`
(sidebar, explorer) never count; neither does a detached process. The owning agent reads the mtime
from the volume (missing = Keep) and after `ws_idle_secs` writes `status.idleSince` and stops through
`stop_workspace` with `Ready=False/Idle`. An environment stops `env_idle_secs` after its last
follower (running not-idle workspace in its space or intercepting it, or `kloudlite.io/held-at`);
unknown cache = Keep; system envs excluded. Wake is `POST start` writing `spec.wakeAt = max(now,
idleSince + 1 s)`; `wants_pod`/`wants_running` = desired Running and not idle since the last wake.
Entry points call `ensureRunning` (harness `ensure.ts`, kl-connect `api::ensure_running`), which wakes
only idle-stopped resources: an explicit stop stays stopped. Detached processes are listed in
`~/.kl/procs.json` and relaunched at boot with the same id (3 attempts).
```

- [ ] **Step 2: Verify.**
  Run: `grep -n "Idle stop\|releases at once" CLAUDE.md`
  Expected: two hits.

- [ ] **Step 3: Commit.** Add `CLAUDE.md` and `crates/ide/src/lib.rs`. Message: `Document idle stop and fix the intercept release rule`.

---

### Task 11: Hand-run k3s drill (after the images ship)

This task needs images built from this branch and deployed to k3s. Run it only after the branch is merged and shipped by the usual flow: Dagger ship from a clean detached worktree, pin, roll, then apply `deploy/k3s/crds.yaml` and the agent DaemonSet on k3s.

**Rules for the drill**
- `KUBECONFIG=/Users/karthik/rustic-git/.local/k3s.yaml` always explicit.
- Never unsuspend an SLO CronJob.
- Times in IST.

- [ ] **Step 1: Set short knobs.** `PUT /admin/settings/clusters/{region}` with `{"wsIdleSecs": 300, "envIdleSecs": 60}`. Confirm `kubectl get clustersettings default -o jsonpath='{.spec.wsIdleSecs}'` prints 300.

- [ ] **Step 2: `ws.idle.stop`.**
  - Create a workspace. Start a detached `sleep 3600` through the tool API, and note its id.
  - Make no calls for 6 min. Expected:
    - `kubectl get workspace <id> -o jsonpath='{.status.idleSince}'` is set;
    - phase is Stopped;
    - the Ready condition reason is `Idle`;
    - the agent logs one `idle.verdict`.

- [ ] **Step 3: `ws.idle.wake`.**
  - Call a tool (`exec echo hi`) through the harness or `kl-connect`. Expected:
    - the call succeeds within 180 s;
    - the api logs `idle.wake` with `by=start`;
    - `process_list` shows the same id running;
    - its output starts with `[restarted `.

- [ ] **Step 4: `env.idle.stop`.**
  - Make the workspace follow an env through `space_env_switch`.
  - Stop the workspace explicitly. Expected: the env shows `unfollowedSince`, and about 60 s later `idleSince` is set and the env is Stopped with `Ready=False/Idle`.
  - Start the workspace. Expected: the env comes back (`idle.wake by=follower`).

- [ ] **Step 5: Restore knobs, then record.**
  - Reset the knobs to defaults: `PUT` `{"wsIdleSecs": null, "envIdleSecs": null}`, or revert through the settings history.
  - Record the results in the ledger.
