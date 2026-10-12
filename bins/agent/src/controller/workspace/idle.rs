//! Idle stop for ordinary workspaces (spec 2026-10-12): the tool server keeps the mtime of
//! `~/.kl/activity` fresh while any session uses the workspace; this agent reads it from the volume
//! on the same node and, after `ws_idle_secs` of silence, writes `status.idleSince` and stops the
//! pod through the ordinary stop path. A missing file is KEEP: a tool server that predates the
//! change, or one that never came up, is a signal we cannot read, never a reason to stop.
//! Bench has its own clock (`bench.rs`); builders are Environments.
use kloudlite_workspaces::{crd, k8s};
use std::time::{Duration, SystemTime};

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Verdict {
    Keep,
    Idle,
}

pub(crate) fn eligible(w: &crd::Workspace, idle_secs: u64) -> bool {
    idle_secs > 0 && !crd::is_bench(w) && w.spec.desired_state == crd::DesiredState::Running
}

/// The newest of activity, pod start and wake: a freshly started or woken workspace has had no
/// time to touch the file yet, and the file on a volume that sat stopped for a day is a day old.
pub(crate) fn idle_verdict(
    idle_secs: u64,
    activity: Option<SystemTime>,
    started: Option<SystemTime>,
    wake_at: Option<SystemTime>,
    now: SystemTime,
) -> Verdict {
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
    let p = format!("{}/.kl/activity", k8s::worktree_path(pool, volume, ws));
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

pub(crate) fn idle_condition(gen: i64) -> crd::Condition {
    crd::condition("Ready", false, crd::IDLE, "no session for wsIdleSecs; the next connection starts it", gen)
}

#[cfg(test)]
mod tests {
    use super::*;
    const T: u64 = 1800;
    fn at(s: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 + s)
    }

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
    #[test]
    fn only_ordinary_running_workspaces_are_eligible() {
        let spec: crd::WorkspaceSpec = serde_json::from_value(serde_json::json!({
            "owner": "alice", "name": "w", "region": "r", "image": "i", "desiredState": "running"
        }))
        .unwrap();
        let mut w = crd::Workspace::new("w", spec);
        assert!(eligible(&w, 1800));
        assert!(!eligible(&w, 0));
        w.spec.bench = Some(crd::BenchOptions::default());
        assert!(!eligible(&w, 1800), "bench has its own idle");
    }
}
