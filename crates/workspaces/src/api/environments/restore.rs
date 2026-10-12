//! Restore and clone of an environment.
//!
//! Two verbs, never mixed up: RESTORE puts THIS environment back to one of its own snapshots,
//! services and data both; CLONE makes a NEW environment, from the live disk or from a snapshot.
//! Agents confused the two while restore-to-new and restore-in-place were both called restore, and
//! a snapshot taken before redis existed, restored in place, still ran redis (2026-10-12). Now a
//! restore writes the snapshot's services beside the wish, and the question "what does this throw
//! away?" is answered by `plan`, a pure function, before anything is written.
//!
//! Map: `plan` (the persistent-data guard), `restore_env` (`POST /v1/environments/{id}/restore`),
//! `clone_env` (`POST /v1/environments/{id}/clone`, with or without `snapshot_id`).

use super::{check_services, create_environment, default_env_quota, env_doc, env_volume};
use crate::api::scope::{denial, find_env, may_allocate_for, resolve_new_owner, Named};
use crate::api::volumes::{find_snapshot, volume_region};
use crate::api::workspaces::{check_name, clamp_quota, interrupted, storage_quota};
use crate::api::{caller_for, check_region, environment_cost, guard_alloc, kube, kube_err, not_found, not_ready, rid, ApiState};
use crate::crd::{self, DesiredState, VolumeSource};
use crate::model::Service;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use kube::api::{Api, Patch, PatchParams};
use std::collections::HashSet;
use std::sync::Arc;

const PREDATES: &str = "snapshot predates saved services; clone it into a new environment instead";

/// A service that keeps data on the environment volume now, and that the snapshot does not have.
#[derive(Debug, PartialEq, serde::Serialize)]
pub(crate) struct Lost {
    pub service: String,
    pub folders: Vec<String>,
}

/// What a restore does to the service list, by name.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Plan {
    /// Persistent in both: their folders are rewound. Never blocks.
    pub rolled_back: Vec<String>,
    /// Lost services the caller agreed to discard.
    pub discarded: Vec<String>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub(crate) enum Refusal {
    /// 409: persistent services the snapshot does not have, and `discard` does not name.
    Lost(Vec<Lost>),
    /// 422: `discard` names something that is not lost.
    NotLost(Vec<String>),
}

/// A service is persistent iff it mounts a folder of the environment volume. Without a mount
/// everything it writes is gone at the next pod restart, so a restore cannot lose it.
fn persistent(s: &Service) -> bool {
    !s.mounts.is_empty()
}

/// Compare the live list with the snapshot's, by name. `discard` must name exactly the lost
/// services: a missing name is a 409 (the caller has not agreed), an extra name is a 422 (the
/// caller believes something that is false, and a stale belief must not pass as consent).
pub(crate) fn plan(live: &[Service], snap: &[Service], discard: &[String]) -> Result<Plan, Refusal> {
    let find = |l: &'_ [Service], n: &str| l.iter().find(|s| s.name == n).cloned();
    let mut out = Plan::default();
    let mut lost = Vec::new();
    for s in live {
        match find(snap, &s.name) {
            None => {
                out.removed.push(s.name.clone());
                if persistent(s) {
                    lost.push(Lost { service: s.name.clone(), folders: s.mounts.iter().map(|m| m.folder.clone()).collect() });
                }
            }
            Some(then) if persistent(s) && persistent(&then) => out.rolled_back.push(s.name.clone()),
            Some(_) => {}
        }
    }
    out.added = snap.iter().filter(|s| find(live, &s.name).is_none()).map(|s| s.name.clone()).collect();
    let extra: Vec<String> = discard.iter().filter(|d| !lost.iter().any(|l| &l.service == *d)).cloned().collect();
    if !extra.is_empty() {
        return Err(Refusal::NotLost(extra));
    }
    let unnamed: Vec<Lost> = lost.into_iter().filter(|l| !discard.contains(&l.service)).collect();
    if !unnamed.is_empty() {
        return Err(Refusal::Lost(unnamed));
    }
    out.discarded = discard.to_vec();
    Ok(out)
}

#[derive(serde::Deserialize)]
pub(crate) struct RestoreBody {
    snapshot_id: String,
    #[serde(default)]
    discard: Vec<String>,
}

/// Put THIS environment back to one of its own snapshots: services and data.
///
/// The API writes a wish and answers; the controllers do the work (scale the services down, swap
/// the subvolume, scale back up), which is why this is a 202 with no result to read. Everything
/// that could go wrong lives in the Environment's `Restoring` condition and the Volume's `Ready`.
///
/// The snapshot must belong to this environment's own volume, so a 404 still means "no such
/// snapshot" and "not yours" alike. The wish and `spec.services` go in ONE merge patch: the agent
/// drains whatever is running before it swaps the disk, and a service list written separately
/// could be read by a pass between the two writes and started against the old disk.
pub(crate) async fn restore_env(
    State(s): State<Arc<ApiState>>,
    headers: axum::http::HeaderMap,
    method: axum::http::Method,
    uri: axum::extract::OriginalUri,
    Named(id): Named,
    Json(body): Json<RestoreBody>,
) -> Result<Response, Response> {
    let caller_id = caller_for(&s, &headers, &method, uri.path()).await?;
    let e = find_env(&s, &caller_id, &id).await?;
    let id = kube::ResourceExt::name_any(&e);
    let volume = env_volume(&e).ok_or_else(not_ready)?.to_string();
    // Validated Ready and same-volume BEFORE the wish is written, so a bad id is a fast 4xx here
    // rather than a silent hang in `restore_gate` (which reads the wish uncritically).
    let snap = find_snapshot(&s, &caller_id, Some(&volume), &body.snapshot_id).await?;
    // No saved services (an old cut, or a workspace's state) leaves nothing to put back: restoring
    // the bytes alone is the half-restore this verb exists to end.
    let (services, quota_gb) = match &snap.spec.state {
        Some(crd::SnapshotState::Environment { services, quota_gb }) if !services.is_empty() => (services.clone(), *quota_gb),
        _ => return Err((StatusCode::UNPROCESSABLE_ENTITY, PREDATES).into_response()),
    };
    // A frozen list is DATA like any other: `check_services` is the trust boundary for mounts and
    // a hand-edited `state` is no more trusted than a request body.
    check_services(&services)?;
    let plan = plan(&e.spec.services, &services, &body.discard).map_err(|r| match r {
        Refusal::Lost(lost) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "these services keep data that the snapshot does not have; name them in `discard` to lose it",
                "lost": lost,
            })),
        )
            .into_response(),
        Refusal::NotLost(extra) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("`discard` names services that lose nothing: {}", extra.join(", ")),
        )
            .into_response(),
    })?;
    // Same rule as `patch_env_services`: a wish must not point at a service that is gone.
    if let Some(i) = e.spec.intercepts.iter().find(|i| plan.removed.contains(&i.service)) {
        let msg = format!("{} is intercepted by {}; release the intercept before restoring", i.service, i.workspace);
        return Err((StatusCode::CONFLICT, msg).into_response());
    }
    // Only added services are new allocation, and only their cpu/memory: the slot and the disk are
    // already spent (see `patch_env_services`).
    if !plan.added.is_empty() {
        use crate::quota::Dim;
        let want: Vec<_> = environment_cost(plan.added.len())
            .into_iter()
            .filter(|(d, _)| matches!(d, Dim::Cpu | Dim::MemoryGb))
            .collect();
        guard_alloc(&s, &e.spec.owner, e.spec.owner != caller_id.name, &want).await?;
    }
    let wish = crd::RestoreWish {
        snapshot_id: body.snapshot_id.clone(),
        volume: snap.spec.volume,
        owner: Some(snap.spec.owner),
        region: None,
        // What makes a repeat of the SAME snapshot a new wish: the controllers compare the id
        // against what is already live, so without this a second attempt after a failure would
        // look like a restore that had already happened.
        requested_at: chrono::Utc::now().to_rfc3339(),
    };
    let mut spec = serde_json::json!({"restore": wish, "services": services});
    let quota = clamp_quota(&s, quota_gb);
    if e.spec.storage.as_ref().is_some_and(|st| st.quota_gb != quota) {
        spec["storage"] = serde_json::json!({"quotaGb": quota});
    }
    // A merge patch: this touches fields of a spec the caller never sent the rest of.
    let api: Api<crd::Environment> = Api::all(kube(&s)?.clone());
    api.patch(&id, &PatchParams::default(), &Patch::Merge(&serde_json::json!({ "spec": spec })))
        .await
        .map_err(kube_err)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "snapshot_id": body.snapshot_id,
            "rolled_back": plan.rolled_back,
            "discarded": plan.discarded,
            "added": plan.added,
            "removed": plan.removed,
        })),
    )
        .into_response())
}

#[derive(serde::Deserialize)]
pub(crate) struct CloneBody {
    name: String,
    /// Absent: copy the live disk. Present: grow the new environment from this snapshot of the
    /// source's own volume. The fields below belong to this second case only.
    #[serde(default)]
    snapshot_id: Option<String>,
    /// A team slug, resolved exactly as `NewEnvironment.owner` is — cloning a team's snapshot
    /// must produce a TEAM environment, or the copy is invisible to everyone but the person who
    /// clicked.
    #[serde(default)]
    owner: Option<String>,
    /// Validated exactly as `create_env`'s are — `check_services` is the trust boundary for mounts
    /// and a clone is just as much a caller-authored service list as a create is. Absent means
    /// "the services the snapshot froze", and so does an explicit `[]`: an environment always has
    /// services, so an empty list is not a way to ask for the data with nothing running (a
    /// snapshot that froze none is a 400).
    #[serde(default)]
    services: Option<Vec<Service>>,
    /// The region to RUN in. Where the snapshot's bytes live is the record's business, not this
    /// field's — that goes on the volume source.
    #[serde(default)]
    region: Option<String>,
    /// Absent means the snapshot's frozen quota, then the standard default.
    #[serde(default)]
    quota_gb: Option<u64>,
}

/// A NEW environment from this one: its live state, or one of its snapshots (`snapshot_id`).
/// Live clone: names no node, for the same reason `clone_ws` does not, and the source's
/// already-validated services carry over untouched.
pub(crate) async fn clone_env(
    State(s): State<Arc<ApiState>>,
    headers: axum::http::HeaderMap,
    method: axum::http::Method,
    uri: axum::extract::OriginalUri,
    Named(id): Named,
    Json(body): Json<CloneBody>,
) -> Result<Response, Response> {
    let caller_id = caller_for(&s, &headers, &method, uri.path()).await?;
    let src = find_env(&s, &caller_id, &id).await?;
    if body.snapshot_id.is_some() {
        return clone_from_snapshot(&s, &caller_id, src, body).await;
    }
    // Refused rather than ignored: a caller who sent `services` expects them to run.
    if body.owner.is_some() || body.services.is_some() || body.region.is_some() || body.quota_gb.is_some() {
        return Err((StatusCode::BAD_REQUEST, "`owner`, `services`, `region` and `quota_gb` apply only with `snapshot_id`").into_response());
    }
    let c = kube(&s)?;
    let new_id = rid("env");
    let volume = env_volume(&src).ok_or_else(not_ready)?.to_string();
    let quota = storage_quota(c, &src.spec.storage, &volume).await;
    // An environment clone COPIES bytes from the source's own live subvolume on the node that
    // holds it (`clone_local_ids`). An interrupted source's node is down, so there is nothing to
    // copy from and the clone would sit `Creating` forever — refused here, before anything is
    // created, rather than left as a workspace-shaped promise this path cannot keep. A workspace
    // clone of an interrupted source IS allowed: it grafts onto a replicated sync point instead.
    if src.status.as_ref().is_some_and(|st| interrupted(&st.conditions)) {
        return Err((
            StatusCode::CONFLICT,
            "the source environment is interrupted: its node is down, and an environment is copied from that node; cloning it waits for the node to return",
        )
            .into_response());
    }
    // No cut here, unlike `clone_ws`: the copy is taken from the live subvolume, so a snapshot
    // would be a CR nothing reads that retention sweeps a minute later. That is also why the
    // response carries no `based_on` — there is no cut this clone is based ON.
    //
    // ponytail: the ceiling is that an environment clone is LOCAL-ONLY. The upgrade is the
    // workspace's shared-worktree path (a `clone-{env}-{hex}` cut, `commit: Some(_)`, and the
    // `SnapshotPending` guard in this controller that `resolve_volume` would then need).
    //
    // `find_env` above admits a superadmin claim to reach any owner's environment (get, allowed);
    // the clone is the allocation, and that claim must not spend a team's quota it is not a
    // member of.
    if !may_allocate_for(&s, &caller_id, &src.spec.owner).await {
        return Err(denial(&s, &caller_id, &src.spec.owner, not_found()).await);
    }
    guard_alloc(&s, &src.spec.owner, src.spec.owner != caller_id.name, &environment_cost(src.spec.services.len())).await?;
    let e = create_environment(
        &s,
        c,
        &new_id,
        crd::EnvironmentSpec {
            owner: src.spec.owner.clone(),
            name: body.name,
            region: src.spec.region.clone(),
            services: src.spec.services.clone(),
            storage: Some(crd::WorkspaceStorage {
                quota_gb: quota,
                // `None`: a fresh child Volume filled by a local copy, never a second worktree of
                // the source's volume. See the comment above `create_environment`.
                source: Some(VolumeSource::CloneOf { volume, commit: None }),
            }),
            desired_state: DesiredState::Running,
            restore: None,
            intercepts: Vec::new(),
            system: None,
        },
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(env_doc(&e, &HashSet::new()))).into_response())
}


/// New environment grafted onto one of the source's own past snapshots — `restore_ws`'s twin,
/// resolving the snapshot the same way (server-tier history, caller/team scoping) and differing only
/// in which kind of object it writes. The agent needs no new path: `resolve_volume` already
/// materializes a `cloneOf { commit }` source for an Environment.
///
/// The services default to what the snapshot froze beside the bytes (`SnapshotState`), because an
/// environment's data without its services is not the environment. A non-empty body list overrides
/// it; an empty one means the same as none. An environment always has services.
///
/// ponytail: the snapshot must belong to a LIVE environment of the caller (`{id}` goes through
/// `find_env`). The old restore-to-new took a bare snapshot id, so it also reached the snapshots of
/// a deleted environment's volume; that way in is gone until a route for it is wanted.
async fn clone_from_snapshot(
    s: &Arc<ApiState>,
    caller_id: &crate::api::Caller,
    src: crd::Environment,
    body: CloneBody,
) -> Result<Response, Response> {
    let snapshot_id = body.snapshot_id.clone().unwrap_or_default();
    // A caller-authored list is refused before anything is read or written, as it always was; the
    // resolved list is checked again below, because it may instead come from the snapshot.
    if let Some(svcs) = &body.services {
        check_services(svcs)?;
    }
    // Named before anything is written, like `create_env`'s: an environment with no name is a row
    // nobody can tell apart from another.
    check_name(&body.name)?;
    // The record's own region needs no check — it was checked when the environment was created,
    // and it is the one region guaranteed to hold these bytes. A caller's choice is checked like
    // a create's.
    if let Some(r) = &body.region {
        check_region(s, r).await?;
    }
    // A clone at a named snapshot under the snapshot model — see `restore_ws`'s matching comment.
    // `find_snapshot` is the ownership check: CR exists, Ready, same volume as the source, and the
    // caller may read `spec.owner`.
    let source_volume = env_volume(&src).ok_or_else(not_ready)?.to_string();
    let snap = find_snapshot(s, caller_id, Some(&source_volume), &snapshot_id).await?;
    let (volume, src_owner) = (snap.spec.volume.clone(), snap.spec.owner.clone());
    // Twin of restore_ws's guard: a workspace's frozen state under an environment clone would
    // mount nothing and silently ignore the image/packages it froze. `None` stays "absent means
    // old". Checked before any other lookup, right after the fetch, same reasoning as restore_ws.
    let frozen = match &snap.spec.state {
        Some(crd::SnapshotState::Environment { services, quota_gb }) => Some((services.clone(), *quota_gb)),
        Some(crd::SnapshotState::Workspace { .. }) => {
            return Err((
                StatusCode::BAD_REQUEST,
                "this snapshot was cut from a workspace; use POST /v1/workspaces/restore",
            )
                .into_response())
        }
        None => None,
    };
    // Defaults to the label the snapshot was FOUND under, not the caller: cloning a team's
    // environment produces a team environment without the client having to say so. Any OTHER
    // owner is refused even when the caller is a member of it: a snapshot found under team A is
    // A's data, and a clone into team B would carry it past A's membership boundary to everyone
    // in B. The caller's own account is the one legitimate elsewhere — their own copy.
    if body.owner.as_deref().is_some_and(|o| o != src_owner && o != caller_id.name) {
        return Err((StatusCode::FORBIDDEN, "a snapshot clones under its own owner, or under you").into_response());
    }
    let owner = resolve_new_owner(s, caller_id, body.owner.or(Some(src_owner.clone()))).await?;
    // The request, then what the snapshot froze, then nothing. A frozen list is DATA like any
    // other — `check_services` runs on whichever source won, because it is the trust boundary for
    // mounts and a hand-edited `state` is no more trusted than a request body.
    // An environment always has services: an empty body list is "use the snapshot's", never
    // "clone the data with nothing running" — the owner ruled the latter out on 2026-09-03.
    let mut services = body
        .services
        .clone()
        .filter(|l| !l.is_empty())
        .or_else(|| frozen.as_ref().map(|f| f.0.clone()))
        .unwrap_or_default();
    if services.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "an environment needs at least one service; this snapshot froze none, so pass `services`").into_response());
    }
    check_services(&services)?;
    let quota = match (body.quota_gb, &frozen) {
        (Some(q), _) => clamp_quota(s, q),
        (None, Some(f)) => clamp_quota(s, f.1),
        (None, None) => default_env_quota(),
    };
    let c = kube(s)?;
    guard_alloc(s, &owner, owner != caller_id.name, &environment_cost(services.len())).await?;
    crate::api::image_pin::pin_images(s, &owner, &mut services).await;
    // The volume holding the bytes still names its region.
    let region = match body.region {
        Some(r) => r,
        None => volume_region(c, &volume).await.unwrap_or_else(|| "default".to_string()),
    };
    let id = rid("env");
    let e = create_environment(
        s,
        c,
        &id,
        crd::EnvironmentSpec {
            owner,
            name: body.name,
            // No per-snapshot region under the snapshot model (see `restore_ws`'s matching comment).
            region,
            services,
            storage: Some(crd::WorkspaceStorage {
                quota_gb: quota,
                source: Some(VolumeSource::CloneOf { volume, commit: Some(snapshot_id) }),
            }),
            desired_state: DesiredState::Running,
            restore: None,
            intercepts: Vec::new(),
            system: None,
        },
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(env_doc(&e, &HashSet::new()))).into_response())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Mount;

    fn svc(name: &str, folders: &[&str]) -> Service {
        Service {
            name: name.into(),
            image: "img".into(),
            command: vec![],
            env: Default::default(),
            mounts: folders.iter().map(|f| Mount { folder: (*f).into(), path: format!("/{f}") }).collect(),
            ports: vec![],
            resources: None,
        }
    }

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn a_persistent_service_in_both_is_rolled_back_and_never_blocks() {
        let p = plan(&[svc("db", &["data"]), svc("web", &[])], &[svc("db", &["data"]), svc("web", &[])], &[]).unwrap();
        assert_eq!(p.rolled_back, names(&["db"]));
        assert!(p.discarded.is_empty() && p.added.is_empty() && p.removed.is_empty());
    }

    #[test]
    fn a_persistent_service_the_snapshot_lacks_is_lost_until_named() {
        let e = plan(&[svc("db", &["data"]), svc("redis", &["rd", "aof"])], &[svc("db", &["data"])], &[]).unwrap_err();
        assert_eq!(e, Refusal::Lost(vec![Lost { service: "redis".into(), folders: names(&["rd", "aof"]) }]));
        let p = plan(&[svc("db", &["data"]), svc("redis", &["rd"])], &[svc("db", &["data"])], &names(&["redis"])).unwrap();
        assert_eq!((p.discarded, p.removed, p.rolled_back), (names(&["redis"]), names(&["redis"]), names(&["db"])));
    }

    #[test]
    fn a_stateless_service_the_snapshot_lacks_is_removed_without_consent() {
        let p = plan(&[svc("web", &[]), svc("db", &["d"])], &[svc("db", &["d"]), svc("worker", &[])], &[]).unwrap();
        assert_eq!((p.removed, p.added), (names(&["web"]), names(&["worker"])));
    }

    #[test]
    fn discard_must_name_exactly_the_lost_services() {
        // A name that loses nothing is a stale belief: 422, even beside a real one.
        let e = plan(&[svc("redis", &["rd"])], &[svc("db", &["d"])], &names(&["redis", "web"])).unwrap_err();
        assert_eq!(e, Refusal::NotLost(names(&["web"])));
        let e = plan(&[svc("db", &["d"])], &[svc("db", &["d"])], &names(&["db"])).unwrap_err();
        assert_eq!(e, Refusal::NotLost(names(&["db"])));
        // One lost service named, the other not: still a 409 for the one left.
        let e = plan(&[svc("a", &["x"]), svc("b", &["y"])], &[svc("c", &[])], &names(&["a"])).unwrap_err();
        assert_eq!(e, Refusal::Lost(vec![Lost { service: "b".into(), folders: names(&["y"]) }]));
    }
}
