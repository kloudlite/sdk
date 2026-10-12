//! Idle stop for environments (spec 2026-10-12). An environment's users are workspaces and
//! laptops: a FOLLOWER is a running, not-idle workspace whose space resolves here, a running,
//! not-idle workspace named in `spec.intercepts`, or a `kloudlite.io/held-at` annotation newer
//! than `env_idle_secs` (`kl-connect env` holds every 60 s). No follower starts
//! `status.unfollowedSince`; `env_idle_secs` later the env is idle and stops through the ordinary
//! path. An UNKNOWN cache is never "no follower" (CLAUDE.md: a reflector before its first list is
//! unknown, never empty). System envs (builders) have their own idle in builder-gate.
use kloudlite_workspaces::crd;
use std::sync::Arc;
use std::time::SystemTime;

pub(crate) const HELD_AT: &str = "kloudlite.io/held-at";

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Followed {
    Unknown,
    Yes,
    No,
}

pub(crate) fn eligible(e: &crd::Environment, idle_secs: u64) -> bool {
    idle_secs > 0 && e.spec.system.is_none() && e.spec.desired_state == crd::DesiredState::Running
}

fn ts(s: &str) -> Option<SystemTime> {
    s.parse::<k8s_openapi::jiff::Timestamp>().ok().map(SystemTime::from)
}

/// A workspace that is up and not idle-stopped; one that is stopping or stopped follows nothing.
fn live(w: &crd::Workspace) -> bool {
    crd::wants_pod(w) && w.status.as_ref().is_some_and(|s| s.phase == crd::Phase::Running)
}

/// `resolve` returns `None` when the space cache is unknown, `Some(None)` when the workspace has
/// no space environment, `Some(Some(id))` otherwise.
pub(crate) fn followed(
    e: &crd::Environment,
    wss: Option<&[Arc<crd::Workspace>]>,
    resolve: impl Fn(&crd::Workspace) -> Option<Option<String>>,
    now: SystemTime,
    idle_secs: u64,
) -> Followed {
    // A hold needs no cache, so it answers before the unknown check: a laptop keeps its
    // environment up through a relist.
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

/// The new `unfollowed_since` and whether the environment is now idle. Unknown keeps the clock
/// and never idles: a blind pass must not decide for or against a stop.
pub(crate) fn step(prev: Option<&str>, f: Followed, now: SystemTime, idle_secs: u64) -> (Option<String>, bool) {
    match f {
        Followed::Yes => (None, false),
        Followed::Unknown => (prev.map(str::to_string), false),
        Followed::No => {
            let since = prev
                .map(str::to_string)
                .unwrap_or_else(|| k8s_openapi::jiff::Timestamp::try_from(now).map(|t| t.to_string()).unwrap_or_default());
            let idle = ts(&since).is_some_and(|s| now.duration_since(s).is_ok_and(|d| d.as_secs() > idle_secs));
            (Some(since), idle)
        }
    }
}

pub(crate) fn idle_condition(gen: i64) -> crd::Condition {
    crd::condition("Ready", false, crd::IDLE, "no follower for envIdleSecs; starting a follower starts it", gen)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const S: u64 = 300;
    fn now() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(2_000_000)
    }
    fn running_ws(name: &str) -> Arc<crd::Workspace> {
        let mut w: crd::Workspace = serde_json::from_value(serde_json::json!({
            "apiVersion": "kloudlite.io/v1alpha1", "kind": "Workspace",
            "metadata": {"name": name},
            "spec": {"owner": "alice", "team": "", "name": name, "region": "r1",
                     "image": "nginx:alpine", "desiredState": "running", "packages": []},
        }))
        .unwrap();
        w.status = Some(crd::WorkspaceStatus { phase: crd::Phase::Running, ..Default::default() });
        Arc::new(w)
    }
    fn env(name: &str) -> crd::Environment {
        serde_json::from_value(serde_json::json!({
            "apiVersion": "kloudlite.io/v1alpha1", "kind": "Environment",
            "metadata": {"name": name},
            "spec": {"owner": "acme", "name": "e", "region": "r", "services": [], "desiredState": "running"},
        }))
        .unwrap()
    }
    fn env_with_intercept(ws: &str) -> crd::Environment {
        let mut e = env("e1");
        e.spec.intercepts = vec![crd::Intercept { service: "api".into(), workspace: ws.into(), ports: vec![] }];
        e
    }

    #[test]
    fn unknown_cache_keeps() {
        let e = env_with_intercept("w1");
        assert_eq!(followed(&e, None, |_| Some(None), now(), S), Followed::Unknown);
    }
    #[test]
    fn follower_via_space() {
        let e = env("e1");
        let w = [running_ws("w1")];
        assert_eq!(followed(&e, Some(&w), |_| Some(Some("e1".into())), now(), S), Followed::Yes);
        assert_eq!(followed(&e, Some(&w), |_| Some(Some("e2".into())), now(), S), Followed::No);
    }
    #[test]
    fn space_unknown_is_unknown() {
        let e = env("e1");
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
        let mut e = env("e1");
        let held = k8s_openapi::jiff::Timestamp::try_from(now() - Duration::from_secs(10)).unwrap().to_string();
        e.metadata.annotations = Some([(HELD_AT.to_string(), held)].into());
        assert_eq!(followed(&e, Some(&[]), |_| Some(None), now(), S), Followed::Yes);
        let old = k8s_openapi::jiff::Timestamp::try_from(now() - Duration::from_secs(S + 1)).unwrap().to_string();
        e.metadata.annotations = Some([(HELD_AT.to_string(), old)].into());
        assert_eq!(followed(&e, Some(&[]), |_| Some(None), now(), S), Followed::No);
    }
    #[test]
    fn step_sets_clears_and_idles() {
        let (u, idle) = step(None, Followed::No, now(), S);
        assert!(u.is_some() && !idle, "first unfollowed beat starts the clock");
        let since = k8s_openapi::jiff::Timestamp::try_from(now() - Duration::from_secs(S + 1)).unwrap().to_string();
        assert_eq!(step(Some(&since), Followed::No, now(), S), (Some(since.clone()), true));
        assert_eq!(step(Some(&since), Followed::Yes, now(), S), (None, false), "a follower clears it");
        assert_eq!(step(Some(&since), Followed::Unknown, now(), S), (Some(since.clone()), false), "unknown keeps the clock, never idles");
    }
    #[test]
    fn system_env_is_not_eligible() {
        let mut e = env("bld-x");
        e.spec.desired_state = crd::DesiredState::Running;
        assert!(eligible(&e, 300));
        e.spec.system = Some(crd::BUILDER_SYSTEM.into());
        assert!(!eligible(&e, 300));
        e.spec.system = None;
        assert!(!eligible(&e, 0));
    }
}
