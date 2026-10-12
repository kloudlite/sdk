//! `/v1/environments` — create, list, read, delete, start/stop and intercepts. Restore (this
//! environment, back in time) and clone (a new environment) live in `restore.rs`.

mod restore;
pub(crate) use restore::{clone_env, restore_env};

use super::scope::{Named, NamedPair};
use super::scope::{caller_owners, denial, find_env, in_scope, may_act_on, mine, my_ws, owned_by, resolve_new_owner, scope_refusal};
use super::workspaces::{
    check_name, clamp_quota, interrupted, interrupted_409, node_dead_warning, pushed_volumes,
    set_desired,
};
use super::{caller_for, check_region, environment_cost, guard_alloc, guard_fill, kube, kube_err, not_found, phase, rid, ApiState, Caller};
use crate::crd::{self, DesiredState};
use crate::k8s::labels;
use crate::model::*;
use kube::api::{Api, DeleteParams, ListParams, Patch, PatchParams, PostParams};
use kube::ResourceExt;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use std::collections::HashSet;
use std::sync::Arc;

pub(crate) fn env_volume(e: &crd::Environment) -> Option<&str> {
    e.status.as_ref().and_then(|st| st.volume_ref.as_deref()).filter(|v| !v.is_empty())
}

pub(super) fn env_doc(e: &crd::Environment, pushed: &HashSet<String>) -> Environment {
    let id = e.name_any();
    let st = e.status.as_ref();
    Environment {
        owner: e.spec.owner.clone(),
        name: e.spec.name.clone(),
        region: e.spec.region.clone(),
        state: phase(st.map(|s| s.phase.as_str()), EnvState::Creating),
        placement: st.map(|s| s.node_name.clone()).filter(|n| !n.is_empty()),
        volume: env_volume(e)
            .filter(|v| pushed.contains(*v))
            .map(|_| id.clone()),
        services: e.spec.services.clone(),
        // Only `get_env` fills this in: it is a read of the CHILD volume's status, and a listing
        // that did it per row would be an N+1 against the API server for a field one page shows.
        restored_to: None,
        restore_requested_at: None,
        // Straight off the condition the reconciler writes, so the page shows the restore while it
        // is happening rather than a state that looks like an ordinary restart.
        restoring: st
            .and_then(|s| s.conditions.iter().find(|c| c.type_ == "Restoring" && c.status == "True"))
            .map(|c| c.reason.clone()),
        replicated: st.and_then(|s| s.conditions.iter().find(|c| c.type_ == "Replicated").map(ConditionDoc::from)),
        degraded: st.and_then(|s| s.conditions.iter().find(|c| c.type_ == "Degraded").map(ConditionDoc::from)),
        decommissioning: st.and_then(|s| s.conditions.iter().find(|c| c.type_ == "Decommissioning").map(ConditionDoc::from)),
        placed: st
            .and_then(|s| s.conditions.iter().find(|c| c.type_ == "Placed"))
            .filter(|c| c.status != "True" && c.reason == "NoCapacity")
            .map(ConditionDoc::from),
        intercepts: e.spec.intercepts.clone(),
        // The other half of the pair: the wish above, what the agent observed here. Empty until
        // a controller has written a status at all. `proxy` is always present here, `none` for a
        // service nobody intercepts: a session that polled for `proxy == "ready"` after releasing
        // an intercept waited on a field that was gone and spun out its whole loop (2026-10-11).
        service_status: st
            .map(|s| {
                let mut v = s.service_status.clone();
                for x in &mut v {
                    x.proxy.get_or_insert_with(|| "none".into());
                }
                v
            })
            .unwrap_or_default(),
        id,
    }
}

#[derive(serde::Deserialize)]
pub(crate) struct NewEnvironment {
    name: String,
    /// Required unless `snapshot_id` is set (then the snapshot's volume names it).
    #[serde(default)]
    region: Option<String>,
    /// Grow the environment from this snapshot of the caller's, also one of a deleted
    /// environment. `services` then default to the ones the snapshot froze.
    #[serde(default)]
    snapshot_id: Option<String>,
    #[serde(default)]
    services: Vec<Service>,
    /// A team slug — makes this a team-owned environment, run on the team's bound node.
    /// `None`/equal to the caller means an ordinary personal environment.
    #[serde(default)]
    owner: Option<String>,
    /// An environment's subvolume holds every service's data. Absent: the 20 GB default, or with
    /// `snapshot_id` the snapshot's frozen quota first.
    #[serde(default)]
    quota_gb: Option<u64>,
}

pub(super) fn default_env_quota() -> u64 {
    crd::DEFAULT_ENV_QUOTA_GB
}

/// The trust boundary for services: create, restore and `patch_env_services` are the routes that
/// accept caller-authored ones (`clone_env` copies an already-validated doc), so a mount that gets
/// past here is treated as trusted by a root agent from then on — and a name that gets past here
/// is what the controller applies, every requeue, forever.
pub(super) fn check_services(services: &[Service]) -> Result<(), Response> {
    if let Some(s) = services.iter().find(|s| !crate::model::name_shape(&s.name)) {
        return Err((StatusCode::BAD_REQUEST, format!("service name {:?} {}", s.name, crate::model::SERVICE_NAME_RULE)).into_response());
    }
    crate::model::validate_services(services).map_err(|e| (StatusCode::BAD_REQUEST, e).into_response())
}

#[derive(serde::Deserialize)]
pub(crate) struct ServicesBody {
    services: Vec<Service>,
}

/// Change an environment's service list in place. A merge patch on `spec.services` alone, for the
/// same reason `patch_ws_packages` is one: this handler was sent one field and must not claim
/// ownership of a spec the caller never wrote. A merge patch REPLACES the array, which is the
/// intent — the body is the whole list the caller wants, added and removed both.
///
/// The controller prunes what left the list (`run.rs`'s `prune_services`); the mount folders on the
/// volume are never touched, so re-adding a service finds its data where it left it.
pub(crate) async fn patch_env_services(
    State(s): State<Arc<ApiState>>,
    headers: axum::http::HeaderMap,
    method: axum::http::Method,
    uri: axum::extract::OriginalUri,
    Named(id): Named,
    Json(mut body): Json<ServicesBody>,
) -> Result<Response, Response> {
    let caller_id = caller_for(&s, &headers, &method, uri.path()).await?;
    let e = find_env(&s, &caller_id, &id).await?;
    let id = kube::ResourceExt::name_any(&e);
    check_services(&body.services)?;
    let names: HashSet<&str> = body.services.iter().map(|svc| svc.name.as_str()).collect();
    // An intercept names a service by name; removing the service under one would leave a wish
    // pointing at nothing and a workspace serving traffic for a service that no longer exists.
    // The person releases it first, deliberately — this route never discards what they asked for.
    if let Some(i) = e.spec.intercepts.iter().find(|i| !names.contains(i.service.as_str())) {
        let msg = format!(
            "{} is intercepted by {}; release the intercept before removing the service",
            i.service, i.workspace
        );
        return Err((StatusCode::CONFLICT, msg).into_response());
    }
    // Only what is ADDED is new allocation; removals free. And only the cpu/memory of those
    // services: the environment slot is already spent and its subvolume already allocated, so
    // charging `environment_cost`'s `Environments`/`DiskGb` would refuse a service to an owner
    // who is merely at their environment limit — which every owner of their last environment is.
    let added = body.services.len().saturating_sub(e.spec.services.len());
    if added > 0 {
        use crate::quota::Dim;
        let want: Vec<_> = environment_cost(added)
            .into_iter()
            .filter(|(d, _)| matches!(d, Dim::Cpu | Dim::MemoryGb))
            .collect();
        guard_alloc(&s, &e.spec.owner, e.spec.owner != caller_id.name, &want).await?;
    }
    super::image_pin::pin_images(&s, &e.spec.owner, &mut body.services).await;
    let api: Api<crd::Environment> = Api::all(kube(&s)?.clone());
    let patch = serde_json::json!({"spec": {"services": body.services}});
    let e = api.patch(&id, &PatchParams::default(), &Patch::Merge(&patch)).await.map_err(kube_err)?;
    let pushed = pushed_volumes(&s, kube(&s)?, &e.spec.owner).await?;
    Ok(Json(env_doc(&e, &pushed)).into_response())
}

/// The one place an `Environment` is written; `create_workspace`'s twin.
pub(super) async fn create_environment(
    s: &Arc<ApiState>,
    c: &kube::Client,
    id: &str,
    spec: crd::EnvironmentSpec,
) -> Result<crd::Environment, Response> {
    if spec.system.is_none() {
        super::scope::refuse_taken_env_name(c, &spec.owner, &spec.name).await?;
    }
    let l = labels(&spec.owner, "environment");
    let mut e = crd::Environment::new(id, spec);
    e.metadata.labels = Some(l);
    let api: Api<crd::Environment> = Api::all(c.clone());
    let e = api.create(&PostParams::default(), &e).await.map_err(kube_err)?;
    // The pull credential needs the namespace, which a node makes only once it claims the env.
    // Every create path (new, restore, clone) goes through here, so one spawn covers them all.
    tokio::spawn(super::env_pull::pull_after_placed(s.clone(), c.clone(), id.to_string(), e.spec.owner.clone()));
    Ok(e)
}

pub(crate) async fn create_env(
    State(s): State<Arc<ApiState>>,
    headers: axum::http::HeaderMap,
    method: axum::http::Method,
    uri: axum::extract::OriginalUri,
    Json(mut body): Json<NewEnvironment>,
) -> Result<Response, Response> {
    let caller_id = caller_for(&s, &headers, &method, uri.path()).await?;
    if let Some(snapshot_id) = body.snapshot_id {
        let from = restore::FromSnapshot {
            name: body.name,
            snapshot_id,
            owner: body.owner,
            services: Some(body.services),
            region: body.region,
            quota_gb: body.quota_gb,
        };
        return restore::clone_from_snapshot(&s, &caller_id, None, from).await;
    }
    let Some(region) = body.region else {
        return Err((StatusCode::BAD_REQUEST, "region is required").into_response());
    };
    // Mounts name volumes (folders inside the env's own subvolume), not workspaces. The name is
    // joined onto the env's subvolume by a root agent, so it is a security boundary, not a
    // formality — see `validate_mount`. Checked before anything is written, deliberately.
    check_services(&body.services)?;
    check_name(&body.name)?;
    check_region(&s, &region).await?;
    let owner = resolve_new_owner(&s, &caller_id, body.owner).await?;
    let c = kube(&s)?;
    let quota_gb = clamp_quota(&s, body.quota_gb.unwrap_or_else(default_env_quota));
    guard_alloc(&s, &owner, owner != caller_id.name, &environment_cost(body.services.len())).await?;
    super::image_pin::pin_images(&s, &owner, &mut body.services).await;
    let id = rid("env");
    let e = create_environment(
        &s,
        c,
        &id,
        crd::EnvironmentSpec {
            owner,
            name: body.name,
            region,
            services: body.services,
            storage: Some(crd::WorkspaceStorage { quota_gb, source: None }),
            desired_state: DesiredState::Running,
            wake_at: None,
            restore: None,
            intercepts: Vec::new(),
            system: None,
        },
    )
    .await?;
    Ok((StatusCode::ACCEPTED, Json(env_doc(&e, &HashSet::new()))).into_response())
}

#[derive(serde::Deserialize)]
pub(crate) struct ListEnvQuery {
    /// Filter to one owner (a username or a team slug) — what the web app's `/{owner}/environments`
    /// page passes so a team page shows only that team's environments, not the caller's personal
    /// ones mixed in. Validated the same way `create_env`'s team owner is: caller must be that
    /// owner, or a member of it.
    #[serde(default)]
    owner: Option<String>,
}

pub(crate) async fn list_env(
    State(s): State<Arc<ApiState>>,
    headers: axum::http::HeaderMap,
    method: axum::http::Method,
    uri: axum::extract::OriginalUri,
    Query(q): Query<ListEnvQuery>,
) -> Result<Response, Response> {
    let caller_id = caller_for(&s, &headers, &method, uri.path()).await?;
    let owners: Vec<String> = match q.owner {
        Some(o) if !in_scope(&caller_id, &o) => return Err(scope_refusal(&caller_id)),
        Some(o) if may_act_on(&s, &caller_id, &o).await => vec![o],
        Some(o) => return Err(denial(&s, &caller_id, &o, not_found()).await),
        None => caller_owners(&s, &caller_id).await,
    };
    Ok(Json(envs_for(&s, &owners).await?).into_response())
}

/// The query behind `/v1/environments`, owner-set already resolved by the caller — the admin
/// Owner detail page reuses this with a single-owner set instead of re-deriving the same read.
pub(crate) async fn envs_for(s: &ApiState, owners: &[String]) -> Result<Vec<Environment>, Response> {
    let c = kube(s)?;
    let api: Api<crd::Environment> = Api::all(c.clone());
    let mut list = vec![];
    for owner in owners {
        let pushed = pushed_volumes(s, c, owner).await?;
        for e in mine(api.list(&owned_by(owner)).await.map_err(kube_err)?.items, std::slice::from_ref(owner)) {
            // The same rule `find_env` applies, on the listing side: a builder is nobody's
            // environment, so it is in no owner's list and in no admin owner-detail page either.
            if !visible_env(&e) {
                continue;
            }
            list.push(env_doc(&e, &pushed));
        }
    }
    Ok(list)
}

pub(crate) async fn get_env(
    State(s): State<Arc<ApiState>>,
    headers: axum::http::HeaderMap,
    method: axum::http::Method,
    uri: axum::extract::OriginalUri,
    Named(id): Named,
) -> Result<Response, Response> {
    let caller_id = caller_for(&s, &headers, &method, uri.path()).await?;
    Ok(read_env(&s, &caller_id, &id).await?.1)
}

/// Strong ETag of an Environment: its `resourceVersion`, quoted. Any write to the object moves it.
fn env_tag(e: &crd::Environment) -> String {
    format!("\"{}\"", e.resource_version().unwrap_or_default())
}

/// What `get_env` answers, plus the tag it carries. One read for both routes, so the watch's 200
/// is byte-for-byte the plain GET.
async fn read_env(s: &ApiState, caller_id: &Caller, id: &str) -> Result<(String, Response), Response> {
    let e = find_env(s, caller_id, id).await?;
    let c = kube(s)?;
    let pushed = pushed_volumes(s, c, &e.spec.owner).await?;
    let mut doc = env_doc(&e, &pushed);
    // Which snapshot is CURRENT is the Volume's answer, not the history's: an in-place restore
    // makes an OLDER record the live one, and a page that assumed "newest = current" would then
    // offer to restore the snapshot the disk is already on.
    if let Some(v) = env_volume(&e) {
        let vols: Api<crd::Volume> = Api::all(c.clone());
        if let Some(st) = vols.get_opt(v).await.map_err(kube_err)?.and_then(|v| v.status) {
            doc.restored_to = st.restored_to;
            doc.restore_requested_at = st.restore_requested_at;
        }
    }
    let tag = env_tag(&e);
    let mut r = Json(doc).into_response();
    if let Ok(v) = tag.parse() {
        r.headers_mut().insert(axum::http::header::ETAG, v);
    }
    Ok((tag, r))
}

/// Long-poll form of `get_env`, for the laptop daemon: same contract as
/// `GET /v1/me/environments/watch`. A client with no tag, or a stale one, is answered now; the
/// same one is held until the Environment changes, then re-read (200 if it moved, 304 if not).
/// The server gives up after 50 s with 304; the client just asks again.
pub(crate) async fn watch_env(
    State(s): State<Arc<ApiState>>,
    headers: axum::http::HeaderMap,
    method: axum::http::Method,
    uri: axum::extract::OriginalUri,
    Named(id): Named,
) -> Result<Response, Response> {
    use futures::TryStreamExt;
    use kube::api::{WatchEvent, WatchParams};
    use std::time::Duration;
    let caller_id = caller_for(&s, &headers, &method, uri.path()).await?;
    let seen = headers.get(axum::http::header::IF_NONE_MATCH).and_then(|v| v.to_str().ok());
    let e = find_env(&s, &caller_id, &id).await?;
    let name = e.name_any();
    let rv = e.resource_version().unwrap_or_default();
    if super::me::answers_now(seen, &env_tag(&e)) {
        return Ok(read_env(&s, &caller_id, &id).await?.1);
    }
    // ponytail: one Kubernetes watch per held request; a shared reflector keyed by environment is the upgrade if many laptops connect.
    let wp = WatchParams::default().fields(&format!("metadata.name={name}")).timeout(50);
    // A watch that cannot open, errors or ends early holds until the deadline instead: answering
    // at once would turn the client's immediate re-ask into a hot loop. The re-read answers every case.
    let envs: Api<crd::Environment> = Api::all(kube(&s)?.clone());
    let _ = tokio::time::timeout(Duration::from_secs(55), async {
        if let Ok(st) = envs.watch(&wp, &rv).await {
            let mut st = Box::pin(st);
            while let Ok(Some(ev)) = st.try_next().await {
                if let WatchEvent::Added(_) | WatchEvent::Modified(_) | WatchEvent::Deleted(_) = ev {
                    return;
                }
            }
        }
        std::future::pending::<()>().await
    })
    .await;
    // A deleted object answers whatever `get_env` does (404).
    let (tag, r) = read_env(&s, &caller_id, &id).await?;
    if super::me::answers_now(seen, &tag) {
        return Ok(r);
    }
    let mut r = StatusCode::NOT_MODIFIED.into_response();
    if let Ok(v) = tag.parse() {
        r.headers_mut().insert(axum::http::header::ETAG, v);
    }
    Ok(r)
}

pub(crate) async fn start_env(
    State(s): State<Arc<ApiState>>,
    headers: axum::http::HeaderMap,
    method: axum::http::Method,
    uri: axum::extract::OriginalUri,
    Named(id): Named,
) -> Result<Response, Response> {
    let caller_id = caller_for(&s, &headers, &method, uri.path()).await?;
    let e = find_env(&s, &caller_id, &id).await?;
    let id = kube::ResourceExt::name_any(&e);
    if e.status.as_ref().is_some_and(|st| interrupted(&st.conditions)) {
        return Err(interrupted_409("environment"));
    }
    // Same as `start_ws`: a start resumes filling and is checked; a stop never is.
    guard_fill(&s, &e.spec.owner, e.spec.owner != caller_id.name).await?;
    set_desired::<crd::Environment>(kube(&s)?, &id, DesiredState::Running).await?;
    let pushed = pushed_volumes(&s, kube(&s)?, &e.spec.owner).await?;
    Ok((StatusCode::ACCEPTED, Json(env_doc(&e, &pushed))).into_response())
}

pub(crate) async fn stop_env(
    State(s): State<Arc<ApiState>>,
    headers: axum::http::HeaderMap,
    method: axum::http::Method,
    uri: axum::extract::OriginalUri,
    Named(id): Named,
) -> Result<Response, Response> {
    let caller_id = caller_for(&s, &headers, &method, uri.path()).await?;
    let e = find_env(&s, &caller_id, &id).await?;
    let id = kube::ResourceExt::name_any(&e);
    set_desired::<crd::Environment>(kube(&s)?, &id, DesiredState::Stopped).await?;
    let pushed = pushed_volumes(&s, kube(&s)?, &e.spec.owner).await?;
    let mut doc = env_doc(&e, &pushed);
    doc.state = EnvState::Stopped;
    let warning = e.status.as_ref().and_then(|st| node_dead_warning(&st.node_name, &st.conditions));
    #[allow(clippy::expect_used)] // serde of a derive(Serialize) value of ours cannot fail
        let mut body = serde_json::to_value(&doc).expect("Environment doc always serializes");
    if let Some(w) = warning {
        body["warning"] = serde_json::Value::String(w);
    }
    Ok((StatusCode::ACCEPTED, Json(body)).into_response())
}

pub(crate) async fn delete_env(
    State(s): State<Arc<ApiState>>,
    headers: axum::http::HeaderMap,
    method: axum::http::Method,
    uri: axum::extract::OriginalUri,
    Named(id): Named,
) -> Result<Response, Response> {
    let caller_id = caller_for(&s, &headers, &method, uri.path()).await?;
    let e = find_env(&s, &caller_id, &id).await?;
    let id = kube::ResourceExt::name_any(&e);
    let c = kube(&s)?;
    let envs: Api<crd::Environment> = Api::all(c.clone());
    envs.delete(&id, &DeleteParams::default()).await.map_err(kube_err)?;
    // Every space that chose this environment lets go of it. Best-effort: the agent treats a
    // missing environment as none regardless, so a failure here leaves a choice naming nothing,
    // never a grant. Selected by the label and re-checked against spec, the label being a view.
    let spaces: Api<crd::SpaceEnvironment> = Api::all(c.clone());
    let mut warning = None;
    match spaces.list(&ListParams::default().labels(&format!("{}={id}", crd::ENVIRONMENT_LABEL))).await {
        Ok(list) => {
            let mut failed = 0;
            for x in list.items.iter().filter(|x| x.spec.environment == id) {
                match spaces.delete(&x.name_any(), &DeleteParams::default()).await {
                    Ok(_) => {}
                    Err(kube::Error::Api(ae)) if ae.code == 404 => {}
                    Err(e) => {
                        tracing::warn!(space = %x.name_any(), error = %e, "space.clear.failed");
                        failed += 1;
                    }
                }
            }
            if failed > 0 {
                warning = Some(format!("{failed} space(s) may still name this deleted environment"));
            }
        }
        Err(err) => {
            tracing::warn!(kind = "SpaceEnvironment", environment = %id, error = %err, "listing.failed");
            warning = Some("could not list the spaces using this environment; some may still name it".to_string());
        }
    }
    let pushed = pushed_volumes(&s, c, &e.spec.owner).await?;
    let mut doc = env_doc(&e, &pushed);
    doc.state = EnvState::Deleted;
    #[allow(clippy::expect_used)] // serde of a derive(Serialize) value of ours cannot fail
        let mut body = serde_json::to_value(&doc).expect("Environment doc always serializes");
    if let Some(w) = warning {
        body["warning"] = serde_json::Value::String(w);
    }
    Ok((StatusCode::ACCEPTED, Json(body)).into_response())
}


// ── intercepts ────────────────────────────────────────────────────────────────

/// Every refusal `set_intercept` can give, in the order a person meets them. Returning the
/// `Response` rather than an error type is what keeps each arm one sentence at the point it is
/// decided.
///
/// The caller must be able to act on the ENVIRONMENT (`find_env`, already done) and on the
/// WORKSPACE (`my_ws`, here): an intercept redirects other people's traffic INTO someone's
/// workspace, so holding one half is not enough.
async fn validate_intercept(
    s: &ApiState,
    caller: &Caller,
    e: &crd::Environment,
    want: &crd::Intercept,
) -> Result<(), Response> {
    let Some(svc) = e.spec.services.iter().find(|sv| sv.name == want.service) else {
        return Err((StatusCode::NOT_FOUND, format!("this environment has no service named {:?}", want.service)).into_response());
    };
    // 404, not 403, exactly as everywhere else: a caller learns nothing about workspaces that are
    // not theirs, not even that the id exists.
    let w = my_ws(s, caller, &want.workspace).await?;
    // The workspace's SPACE must use this environment — the same answer `decide_intercept` reads
    // from the agent's cache, including its one-release fallback to the retired field while the
    // space has no choice yet.
    let space = Api::<crd::SpaceEnvironment>::all(kube(s)?.clone())
        .get_opt(&crd::space_name(&w.spec.owner, &w.spec.team))
        .await
        .map_err(kube_err)?;
    let chosen = match &space {
        Some(x) => Some(x.spec.environment.as_str()),
        None => w.spec.attached_environment.as_deref(),
    };
    if chosen != Some(e.name_any().as_str()) {
        return Err((StatusCode::CONFLICT, "that workspace's space does not use this environment; choose it first").into_response());
    }
    // `spec`, not `status`: desired state is what this tier owns, and a workspace whose pod is
    // merely between restarts is still one the person means to serve. Whether an intercept is
    // actually IN FORCE is the controller's answer, reported in status.
    if w.spec.desired_state != DesiredState::Running {
        return Err((StatusCode::CONFLICT, "that workspace is stopped; start it before it can answer for a service").into_response());
    }
    if let Some(held) = e.spec.intercepts.iter().find(|i| i.service == want.service && i.workspace != want.workspace) {
        return Err((
            StatusCode::CONFLICT,
            format!("{} already intercepts this service; release that one first", held.workspace),
        )
            .into_response());
    }
    // A service declaring no ports has nothing to forward: the port loop below is vacuous, so the
    // wish would be accepted here and only refused by the controller (`NoPorts`) minutes later.
    if svc.ports.is_empty() {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("service {} declares no ports; nothing to intercept", svc.name),
        )
            .into_response());
    }
    let mut seen = HashSet::new();
    // ponytail: no UDP refusal — `model::Service` carries no protocol and `service_clusterip`
    // hard-codes TCP, so nothing can declare one. When it grows a protocol field, refuse a
    // non-TCP port here with 422 ("port {n} is UDP, and an intercept forwards TCP only"): the
    // forwarder is TCP, and rendering a UDP port would scale the real service to 0 and drop half
    // a protocol on the floor.
    for p in &want.ports {
        if !svc.ports.contains(&p.service) {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("this service does not declare port {}, so nothing dials it", p.service),
            )
                .into_response());
        }
        if p.workspace == 0 {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("port {} must answer on a real port of the workspace, and 0 is not one", p.service),
            )
                .into_response());
        }
        if !seen.insert(p.service) {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("port {} is mapped twice; one service port answers on one workspace port", p.service),
            )
                .into_response());
        }
    }
    if let Some(p) = want.ide_port_collision(&svc.ports) {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("port {p} would land on workspace port {}, the tool server's; map it to another port", crate::k8s::IDE_PORT),
        )
            .into_response());
    }
    Ok(())
}

/// How many times a write re-reads and re-decides after losing the CAS below. Three: a lost race
/// means somebody else's write landed in the microseconds between our read and our patch, and a
/// caller that lost three of those in a row is contending with something a retry will not fix.
const INTERCEPT_ATTEMPTS: usize = 3;

/// Compare-and-set `spec.intercepts`, guarded by the `resourceVersion` the decision was made
/// against — the same `test`-op construction `take_volume` uses, for the same reason.
///
/// A merge patch REPLACES a JSON array wholesale, so two calls naming two different services
/// would each write the list they read and the second would silently discard the first — an
/// accepted 202 with nothing on disk and no signal anywhere. The `test` turns that into a lost
/// race the caller re-decides, which is the only reading that cannot lose a write.
///
/// `add`, not `replace`: `spec.intercepts` is `#[serde(default)]`, so an environment written
/// before the field existed has no such key and a `replace` on a missing path is a 422 this would
/// then retry forever. `add` creates or overwrites.
///
/// `Ok(None)` is a lost race. An `Err` is an outage — never "lost", same distinction `cas` draws.
// Two static JSON pointers and the serialisation of our own `Intercept` list: none can fail,
// and an attribute on an expression inside the vec is not stable Rust, so the allow is here.
#[allow(clippy::expect_used)]
async fn cas_intercepts(
    c: &kube::Client,
    e: &crd::Environment,
    want: Vec<crd::Intercept>,
) -> Result<Option<crd::Environment>, Response> {
    let api: Api<crd::Environment> = Api::all(c.clone());
    let ops = json_patch::Patch(vec![
        json_patch::PatchOperation::Test(json_patch::TestOperation {
            path: "/metadata/resourceVersion".parse().expect("a static pointer"),
            value: serde_json::json!(e.resource_version().unwrap_or_default()),
        }),
        json_patch::PatchOperation::Add(json_patch::AddOperation {
            path: "/spec/intercepts".parse().expect("a static pointer"),
            value: serde_json::to_value(want).expect("intercepts serialize"),
        }),
    ]);
    match api.patch(&e.name_any(), &PatchParams::default(), &Patch::Json::<crd::Environment>(ops)).await {
        Ok(v) => Ok(Some(v)),
        Err(kube::Error::Api(st)) if st.code == 409 || st.code == 422 => Ok(None),
        Err(err) => Err(kube_err(err)),
    }
}

/// The one answer a write gets when it keeps losing the CAS: honest, and never a 202 over a list
/// that was not written.
fn contended() -> Response {
    (StatusCode::CONFLICT, "the environment was changed while this was being written; try again").into_response()
}

/// Write one workspace's wish to receive a service's traffic. Spec only — every visible effect
/// is the environment controller's reconcile, which is why this is a 202.
///
/// Read, validate and write are one attempt, retried: the validation reads the CURRENT holders,
/// so a lost race must re-read them rather than re-send a decision made against a list somebody
/// else has since changed.
pub(crate) async fn set_intercept(
    State(s): State<Arc<ApiState>>,
    headers: axum::http::HeaderMap,
    method: axum::http::Method,
    uri: axum::extract::OriginalUri,
    Named(id): Named,
    Json(body): Json<crd::Intercept>,
) -> Result<Response, Response> {
    let caller_id = caller_for(&s, &headers, &method, uri.path()).await?;
    // The agent reads ids: store the resolved one whatever the caller typed (name, `team/name`, id).
    let mut body = body;
    body.workspace = my_ws(&s, &caller_id, &body.workspace).await?.name_any();
    for _ in 0..INTERCEPT_ATTEMPTS {
        let e = find_env(&s, &caller_id, &id).await?;
        validate_intercept(&s, &caller_id, &e, &body).await?;
        // At most one entry per service, held here rather than by the controller (which takes the
        // first, so a hand-edited object cannot flap): the same workspace asking again with new
        // ports REPLACES, and a different one was already refused above.
        let mut want: Vec<crd::Intercept> =
            e.spec.intercepts.iter().filter(|i| i.service != body.service).cloned().collect();
        want.push(body.clone());
        if let Some(e) = cas_intercepts(kube(&s)?, &e, want).await? {
            let pushed = pushed_volumes(&s, kube(&s)?, &e.spec.owner).await?;
            return Ok((StatusCode::ACCEPTED, Json(env_doc(&e, &pushed))).into_response());
        }
    }
    Err(contended())
}

/// The ONLY thing that removes an intercept. Deliberately not called by `stop_ws`, `detach_ws` or
/// `delete_ws`: the wish is the person's, and stopping a workspace for the night must not throw
/// away what they asked for — the controller brings the real service back in the meantime, and
/// the intercept takes hold again when the workspace returns, with nobody re-asking.
///
/// Idempotent: releasing one that is not there is the state being asked for.
pub(crate) async fn clear_intercept(
    State(s): State<Arc<ApiState>>,
    headers: axum::http::HeaderMap,
    method: axum::http::Method,
    uri: axum::extract::OriginalUri,
    NamedPair(id, service): NamedPair,
) -> Result<Response, Response> {
    let caller_id = caller_for(&s, &headers, &method, uri.path()).await?;
    for _ in 0..INTERCEPT_ATTEMPTS {
        let e = find_env(&s, &caller_id, &id).await?;
        if !e.spec.intercepts.iter().any(|i| i.service == service) {
            return Ok(StatusCode::NO_CONTENT.into_response());
        }
        let want: Vec<crd::Intercept> = e.spec.intercepts.iter().filter(|i| i.service != service).cloned().collect();
        // Guarded like the write above: a release that merge-patched the list it read would
        // discard an intercept somebody added for another service in the meantime.
        if cas_intercepts(kube(&s)?, &e, want).await?.is_some() {
            return Ok(StatusCode::NO_CONTENT.into_response());
        }
    }
    Err(contended())
}

/// Take a deleted workspace's intercepts out of every environment's spec.
///
/// The wish otherwise outlives the workspace: the agent renders it `Off/WorkspaceGone` but keeps
/// it in spec, so the service shows as wished-for by an id that no longer exists, and nothing but a
/// person's `DELETE .../intercepts/{service}` ever removes it (2026-10-10: t-env kept two after its
/// workspaces were gone). Best-effort, like `drop_attach_policy`: the Workspace is already deleted,
/// and the agent already serves the real service for a missing workspace.
pub(crate) async fn drop_intercepts_of(c: &kube::Client, ws: &str) {
    let api: Api<crd::Environment> = Api::all(c.clone());
    let envs = match api.list(&ListParams::default()).await {
        Ok(l) => l.items,
        Err(e) => return tracing::warn!(workspace = %ws, error = %e, "intercepts.drop.failed"),
    };
    for e in envs.into_iter().filter(|e| e.spec.intercepts.iter().any(|i| i.workspace == ws)) {
        let mut e = e;
        for _ in 0..INTERCEPT_ATTEMPTS {
            let want: Vec<crd::Intercept> = e.spec.intercepts.iter().filter(|i| i.workspace != ws).cloned().collect();
            match cas_intercepts(c, &e, want).await {
                Ok(Some(_)) => break,
                Ok(None) => match api.get_opt(&e.name_any()).await {
                    Ok(Some(fresh)) if fresh.spec.intercepts.iter().any(|i| i.workspace == ws) => e = fresh,
                    _ => break,
                },
                Err(_) => {
                    tracing::warn!(workspace = %ws, environment = %e.name_any(), "intercepts.drop.failed");
                    break;
                }
            }
        }
    }
}

// ── the hidden per-owner builder ─────────────────────────────────────────

/// Is this an environment a person may see and act on?
///
/// A `system` environment — the per-owner buildkitd builder — is not: nobody asked for it, nothing
/// about it is theirs to start, stop, clone or push, and it must not even be distinguishable from
/// an id that does not exist. Used by `scope::find_env` and by the listing above, which between
/// them are every user-facing environment route.
pub(crate) fn visible_env(e: &crd::Environment) -> bool {
    e.spec.system.is_none()
}

/// buildkitd, exactly as the gvisor spike settled it: the non-rootless image, one tcp listener the
/// build gate dials, and its cache on the environment's own subvolume. `builder_hardened()` in
/// `k8s` is what renders this service's security context, keyed off `spec.system`.
fn builder_service() -> Service {
    Service {
        name: "buildkit".into(),
        image: "moby/buildkit:v0.18.2".into(),
        command: ["buildkitd", "--addr", "tcp://0.0.0.0:1234", "--oci-worker-snapshotter=native", "--root", "/cache"]
            .iter()
            .map(|a| (*a).to_string())
            .collect(),
        env: Default::default(),
        mounts: vec![Mount { folder: "cache".into(), path: "/cache".into() }],
        ports: vec![1234],
        // Not the idle-database unit every other service gets: a build is the one thing in an
        // environment that is genuinely cpu- and memory-bound. The quota table's cpu/memory
        // defaults are derived assuming exactly one of these per owner.
        resources: Some(crd::PodResources::default()),
    }
}

/// Give an owner — or their team — the builder their workspaces build through.
///
/// Server-side apply, so a repeat is a no-op and a change to the image or the argv above reaches
/// every existing builder on the next workspace create. `desiredState` is the one field taken from
/// whatever is already there: the gate starts and stops the builder, and an apply that re-asserted
/// `Stopped` would tear down a running build the moment somebody created a workspace.
///
/// No `guard_alloc`: the builder is not the owner's allocation to approve, and the derived default
/// ceilings already reserve exactly one per owner. An owner on a hand-set `Quota` below that gets
/// a builder that is written but cannot start — the agent's `ResourceQuota` refuses its pod — and
/// `kl builder status` is where they see why.
pub(crate) async fn ensure_builder(s: &ApiState, owner: &str, team: &str, region: &str) -> Result<(), Response> {
    // A team's builder belongs to the team, like everything else a team workspace allocates —
    // through `owner_slug`, the fold `keys_owner` and therefore `prune_builders` use, because two
    // spellings of the slug means the beat prunes what the create just wrote.
    let slug = crate::k8s::owner_slug(owner, team);
    let id = crd::builder_id(slug);
    let api: Api<crd::Environment> = Api::all(kube(s)?.clone());
    let desired_state = match api.get_opt(&id).await.map_err(kube_err)? {
        Some(e) => e.spec.desired_state,
        None => DesiredState::Stopped,
    };
    let mut e = crd::Environment::new(
        &id,
        crd::EnvironmentSpec {
            owner: slug.to_string(),
            name: "builder".into(),
            region: region.to_string(),
            services: vec![builder_service()],
            storage: Some(crd::WorkspaceStorage { quota_gb: crd::BUILDER_CACHE_GB, source: None }),
            desired_state,
            wake_at: None,
            restore: None,
            intercepts: Vec::new(),
            system: Some(crd::BUILDER_SYSTEM.to_string()),
        },
    );
    e.metadata.labels = Some(labels(slug, "environment"));
    api.patch(&id, &PatchParams::apply("kloudlite-api"), &Patch::Apply(&e)).await.map_err(kube_err)?;
    Ok(())
}

/// The slug is spliced into a URL to the API server, so it is validated exactly as every other
/// path segment is. A bad one is 404, not 400: the gate has no business learning the difference.
fn builder_of(slug: &str) -> Result<String, Response> {
    if !kloudlite_storage::store::valid_owner(slug) {
        return Err(not_found());
    }
    Ok(crd::builder_id(slug))
}

/// `POST /v1/internal/builders/{slug}/start|stop` — the build gate's two writes, through the same
/// merge-patch `start_env`/`stop_env` use and deliberately past `visible_env`: `/v1` stays the
/// only writer of spec, and the gate holds a secret rather than a person's identity.
pub(crate) async fn start_builder(
    State(s): State<Arc<ApiState>>,
    Path(slug): Path<String>,
) -> Result<Response, Response> {
    set_builder_state(&s, &slug, DesiredState::Running).await
}

pub(crate) async fn stop_builder(
    State(s): State<Arc<ApiState>>,
    Path(slug): Path<String>,
) -> Result<Response, Response> {
    set_builder_state(&s, &slug, DesiredState::Stopped).await
}

async fn set_builder_state(s: &ApiState, slug: &str, want: DesiredState) -> Result<Response, Response> {
    let id = builder_of(slug)?;
    let api: Api<crd::Environment> = Api::all(kube(s)?.clone());
    // Read before writing, for the reason `get_builder` reads: these routes reach builders and
    // ONLY builders, so a person's environment that happens to be named `bld-…` must not be
    // startable or stoppable with the gate's secret.
    api.get_opt(&id).await.map_err(kube_err)?.filter(|e| !visible_env(e)).ok_or_else(not_found)?;
    set_desired::<crd::Environment>(kube(s)?, &id, want).await?;
    // 202 like every other desired-state write: the controller is what makes it true.
    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({"id": id, "desiredState": want}))).into_response())
}

/// The body both `GET /v1/internal/builders/{slug}` and `GET /v1/builders/me` answer — one
/// shape, so the CLI's own status view can never drift from what the gate itself polls.
fn builder_body(id: &str, e: &crd::Environment) -> serde_json::Value {
    let st = e.status.as_ref();
    // A builder the controller has not reconciled yet has no status, and reading `creating` for
    // one that was created Stopped made the caller wait for a stop that had already happened.
    // With nothing observed, what is desired is the whole truth about it.
    let unseen = match e.spec.desired_state {
        crd::DesiredState::Running => EnvState::Running,
        crd::DesiredState::Stopped => EnvState::Stopped,
    };
    // A builder that is meant to be Stopped and is not observed Running IS stopped, whatever
    // interim phase the claim wrote (`creating` between the claim and the controller's first
    // stop pass): nothing can be building, and a caller that waits on the word — the probe does,
    // before it measures a cold start — would otherwise read a state that is seconds from true.
    let observed = st.map(|s| s.phase.as_str());
    let state = match (e.spec.desired_state, observed) {
        (crd::DesiredState::Stopped, Some(p)) if p != "running" => EnvState::Stopped,
        _ => phase(observed, unseen),
    };
    let running = matches!(state, EnvState::Running);
    serde_json::json!({
        "id": id,
        "state": state,
        // The gate dials on this and nothing else, so it must mean "buildkit answers". NOT the
        // environment's `Ready` condition: a stopped environment carries `Ready=True/Stopped`
        // ("pushed and stopped"), which read as ready one second after `start` and sent the gate
        // at a Service that did not exist yet. The buildkit service's own readiness is written
        // from the StatefulSet's `readyReplicas`, which is the endpoint the dial needs.
        "ready": running
            && st.is_some_and(|s| s.service_status.iter().any(|x| x.name == "buildkit" && x.ready)),
        "conditions": st.map(|s| s.conditions.clone()).unwrap_or_default(),
    })
}

/// `GET /v1/internal/builders/{slug}` — what the gate polls between `start` and dialling buildkit.
pub(crate) async fn get_builder(
    State(s): State<Arc<ApiState>>,
    Path(slug): Path<String>,
) -> Result<Response, Response> {
    let id = builder_of(&slug)?;
    let api: Api<crd::Environment> = Api::all(kube(&s)?.clone());
    // `!visible_env`: this route reaches builders and ONLY builders, so a person's environment
    // that happened to be named `bld-…` is not readable through the gate's secret.
    let e = api.get_opt(&id).await.map_err(kube_err)?.filter(|e| !visible_env(e)).ok_or_else(not_found)?;
    Ok(Json(builder_body(&id, &e)).into_response())
}

#[derive(serde::Deserialize)]
pub(crate) struct BuilderQuery {
    /// Absent means the caller's own builder. A team slug they belong to is allowed; anything
    /// else is a 404, same as every other owner-scoped read — the builder must stay
    /// indistinguishable from nothing to a stranger.
    #[serde(default)]
    team: Option<String>,
}

/// `GET /v1/builders/me` — the ONE user-facing window onto the hidden builder: never listed,
/// never started or stopped by a person, but visible so `kl builder status` can say why a build
/// is waiting. Same body as the internal GET the gate polls.
pub(crate) async fn get_my_builder(
    State(s): State<Arc<ApiState>>,
    headers: axum::http::HeaderMap,
    method: axum::http::Method,
    uri: axum::extract::OriginalUri,
    Query(q): Query<BuilderQuery>,
) -> Result<Response, Response> {
    let c = caller_for(&s, &headers, &method, uri.path()).await?;
    let team = q.team.unwrap_or_default();
    if !team.is_empty() && !in_scope(&c, &team) {
        return Err(scope_refusal(&c));
    }
    if !team.is_empty() && !may_act_on(&s, &c, &team).await {
        return Err(denial(&s, &c, &team, not_found()).await);
    }
    let slug = crate::k8s::owner_slug(&c.name, &team);
    let id = crd::builder_id(slug);
    let api: Api<crd::Environment> = Api::all(kube(&s)?.clone());
    let e = api.get_opt(&id).await.map_err(kube_err)?.filter(|e| !visible_env(e)).ok_or_else(not_found)?;
    Ok(Json(builder_body(&id, &e)).into_response())
}

/// `GET /v1/internal/builders` — every builder that is meant to be running, so a restarted gate
/// re-seeds its idle timers instead of leaving one running until somebody builds again.
pub(crate) async fn list_builders(State(s): State<Arc<ApiState>>) -> Result<Response, Response> {
    let api: Api<crd::Environment> = Api::all(kube(&s)?.clone());
    let mut running: Vec<String> = api
        .list(&ListParams::default())
        .await
        .map_err(kube_err)?
        .items
        .into_iter()
        .filter(|e| !visible_env(e) && e.spec.desired_state == DesiredState::Running)
        .map(|e| e.spec.owner)
        .collect();
    running.sort();
    Ok(Json(serde_json::json!({"running": running})).into_response())
}

// ── volumes ──────────────────────────────────────────────────────────────
//
// A snapshot is a point in time and outlives the workspace it was taken of, so none of these reads
// may hang off a live Workspace/Environment. The index and the records both live on the SERVER
// tier (`vol/{owner}/{name}`); the cluster is consulted only to answer "is the parent still
// around?", which is a display detail, never an authorization one.

#[cfg(test)]
mod tests {
    use super::check_services;
    use crate::crd;
    use crate::model::{Mount, Service};

    #[test]
    fn an_environment_doc_carries_degraded_and_decommissioning() {
        let mut e = crd::Environment::new(
            "env-1",
            crd::EnvironmentSpec {
                owner: "karthik".into(),
                name: "app".into(),
                region: "centralindia".into(),
                services: vec![],
                storage: None,
                desired_state: crd::DesiredState::Running,
                wake_at: None,
                restore: None,
                intercepts: Vec::new(),
                system: None,
            },
        );
        e.status = Some(crd::EnvironmentStatus {
            conditions: vec![
                crd::condition("Degraded", true, "NodeDead", "node n1 is down", 4),
                crd::condition("Decommissioning", true, "NodeLeaving", "this node is being retired", 4),
            ],
            ..Default::default()
        });
        let d = super::env_doc(&e, &Default::default());
        assert_eq!(d.degraded.expect("degraded must be shown").reason, "NodeDead");
        let dec = d.decommissioning.expect("decommissioning must be shown");
        assert_eq!(dec.reason, "NodeLeaving");
        assert!(dec.message.contains("retired"));
    }

    /// `ServiceStatus` is the CRD's own type, re-serialized verbatim into a doc whose every other
    /// key is snake_case — so these entries alone are camelCase, and the web reads them by those
    /// names. Renaming the CRD side would rewrite keys in every object already stored on the fleet,
    /// so the wire name is the contract and this pins it.
    #[test]
    fn the_service_status_keys_are_camel_case() {
        let mut e = crd::Environment::new(
            "env-1",
            crd::EnvironmentSpec {
                owner: "karthik".into(),
                name: "app".into(),
                region: "centralindia".into(),
                services: vec![],
                storage: None,
                desired_state: crd::DesiredState::Running,
                wake_at: None,
                restore: None,
                intercepts: Vec::new(),
                system: None,
            },
        );
        e.status = Some(crd::EnvironmentStatus {
            service_status: vec![crd::ServiceStatus {
                name: "api".into(),
                ready: true,
                message: None,
                intercepted_by: Some("ws-1".into()),
                proxy: Some("starting".into()),
                unreachable_since: Some(1_700_000_000),
                failing: false,
            }, crd::ServiceStatus {
                name: "db".into(),
                ready: true,
                message: None,
                intercepted_by: None,
                proxy: None,
                unreachable_since: None,
                failing: false,
            }],
            ..Default::default()
        });
        let j = serde_json::to_value(super::env_doc(&e, &Default::default())).expect("doc serializes");
        assert_eq!(j["service_status"][1]["proxy"], "none", "a poll on proxy must always have a field to read");
        let st = &j["service_status"][0];
        assert_eq!(st["interceptedBy"], "ws-1");
        assert_eq!(st["unreachableSince"], 1_700_000_000);
        assert_eq!(st["proxy"], "starting");
        assert!(st.get("intercepted_by").is_none(), "snake_case keys are not what /v1 emits: {st}");
    }

    fn svc(folder: &str, path: &str) -> Service {
        Service {
            name: "web".into(),
            image: "nginx".into(),
            command: vec![],
            env: Default::default(),
            mounts: vec![Mount { folder: folder.into(), path: path.into() }],
            ports: vec![],
            resources: None,
        }
    }

    #[test]
    fn create_env_refuses_a_traversing_mount() {
        assert!(check_services(&[svc("data", "/data")]).is_ok());
        // The C1 payload: `{"folder": "/", "path": "/host"}` bind-mounts the host root RW into a
        // container whose image the same caller chose.
        for bad in ["/", "..", "a/b", "", "../../root/.ssh", "a:b"] {
            assert!(check_services(&[svc(bad, "/host")]).is_err(), "folder {bad:?} must be refused");
        }
        assert!(check_services(&[svc("data", "/data:/etc")]).is_err(), "a ':' in path splices a mapping");
        assert!(check_services(&[svc("data", "relative")]).is_err());
    }
}
