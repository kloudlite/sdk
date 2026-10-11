//! `/v1/me/environments`: which environment each of the CALLER's spaces follows — one per team they
//! are in, plus their personal space (`team` = their handle). The only writer of
//! `SpaceEnvironment`; the agent converges every pod of the space from it (`controller::space`).
//! `GET /v1/me/environments/watch` is the same list as a long-poll, for the laptop daemon.
//!
//! The person is always the caller: nothing here names another person, and a body carrying
//! `owner` is refused outright (400) rather than ignored, the same rule as `/v1/keys`. Region is
//! deliberately not checked — a space spans regions, an environment does not, and a pod elsewhere
//! reports `Attached=False/RegionMismatch` itself. See
//! `docs/superpowers/specs/2026-09-14-person-environment-design.md`.

use super::*;
use super::scope::{in_scope, scope_refusal, teams_for};


#[derive(serde::Serialize)]
pub(crate) struct SpaceDoc {
    team: String,
    environment: String,
    region: Option<String>,
}


/// The team segment as the caller's space: their own handle is the personal space, any other
/// value must be a team they are a MEMBER of — a superadmin claim chooses nothing for anyone.
/// Everything else is a 404: the caller learns nothing about teams they are not in.
async fn space_team(s: &ApiState, caller: &Caller, team: &str) -> Result<String, Response> {
    let team = team.to_lowercase();
    if !in_scope(caller, &team) {
        return Err(scope_refusal(caller));
    }
    if team == caller.name.to_lowercase() || teams_for(s, &caller.name).await.iter().any(|t| t.eq_ignore_ascii_case(&team)) {
        return Ok(team);
    }
    Err(not_found())
}


fn spaces(s: &ApiState) -> Result<Api<crd::SpaceEnvironment>, Response> {
    Ok(Api::all(kube(s)?.clone()))
}


/// The caller's spaces and the list's `resourceVersion` (the point a watch resumes from).
async fn my_spaces(s: &ApiState, caller: &Caller) -> Result<(Vec<SpaceDoc>, String), Response> {
    let c = kube(s)?;
    let lp = ListParams::default().labels(&owner_selector(caller));
    // The label is a view; `spec.owner` is the answer.
    let list = spaces(s)?.list(&lp).await.map_err(kube_err)?;
    let rv = list.metadata.resource_version.clone().unwrap_or_default();
    let mine: Vec<crd::SpaceEnvironment> = list.items.into_iter().filter(|x| wakes(x, caller)).collect();
    let envs: Api<crd::Environment> = Api::all(c.clone());
    let mut out = Vec::with_capacity(mine.len());
    for x in mine {
        let region = envs.get_opt(&x.spec.environment).await.map_err(kube_err)?.map(|e| e.spec.region);
        out.push(SpaceDoc { team: x.spec.team, environment: x.spec.environment, region });
    }
    Ok((out, rv))
}

fn owner_selector(caller: &Caller) -> String {
    format!("{}={}", crate::k8s::OWNER_LABEL, caller.name.to_lowercase())
}

/// Strong ETag over the selection: sorted `team=environment` pairs. Region is left out — an
/// environment's region does not change. Ids are label-safe, so nothing needs escaping.
fn etag(docs: &[SpaceDoc]) -> String {
    let mut v: Vec<String> = docs.iter().map(|d| format!("{}={}", d.team, d.environment)).collect();
    v.sort();
    format!("\"{}\"", v.join(","))
}

/// The same filter as the list: `spec.owner` is truth, the label only narrows the query.
fn wakes(x: &crd::SpaceEnvironment, caller: &Caller) -> bool {
    x.spec.owner.eq_ignore_ascii_case(&caller.name) && in_scope(caller, &x.spec.team)
}

/// A client with no tag, or a different one, is answered without waiting.
pub(super) fn answers_now(if_none_match: Option<&str>, tag: &str) -> bool {
    if_none_match != Some(tag)
}

fn spaces_response(docs: Vec<SpaceDoc>, tag: &str) -> Response {
    let mut r = Json(docs).into_response();
    if let Ok(v) = tag.parse() {
        r.headers_mut().insert(axum::http::header::ETAG, v);
    }
    r
}

pub(crate) async fn list_my_environments(
    State(s): State<Arc<ApiState>>,
    headers: axum::http::HeaderMap,
    method: axum::http::Method,
    uri: axum::extract::OriginalUri,
) -> Result<Response, Response> {
    let caller = caller_for(&s, &headers, &method, uri.path()).await?;
    let (docs, _) = my_spaces(&s, &caller).await?;
    let tag = etag(&docs);
    Ok(spaces_response(docs, &tag))
}


/// Long-poll form of the list, so the laptop daemon follows a switch at once instead of on a
/// beat. The client sends the `ETag` it last saw as `If-None-Match`: a different selection is
/// answered now (200 + new tag); the same one is held until a `SpaceEnvironment` of the caller's
/// changes, then re-read (200 if the selection moved, 304 if not). The server gives up after
/// 50 s with 304, under the 60 s read timeout most proxies default to (our ingress allows 600 s); the client just asks again.
pub(crate) async fn watch_my_environments(
    State(s): State<Arc<ApiState>>,
    headers: axum::http::HeaderMap,
    method: axum::http::Method,
    uri: axum::extract::OriginalUri,
) -> Result<Response, Response> {
    use futures::TryStreamExt;
    use kube::api::{WatchEvent, WatchParams};
    use std::time::Duration;
    let caller = caller_for(&s, &headers, &method, uri.path()).await?;
    let seen = headers.get(axum::http::header::IF_NONE_MATCH).and_then(|v| v.to_str().ok());
    let (docs, rv) = my_spaces(&s, &caller).await?;
    let tag = etag(&docs);
    if answers_now(seen, &tag) {
        return Ok(spaces_response(docs, &tag));
    }
    // ponytail: one Kubernetes watch per held request; a shared reflector keyed by owner is the upgrade if many laptops connect.
    let wp = WatchParams::default().labels(&owner_selector(&caller)).timeout(45);
    // Only a change of the caller's own ends the wait early. A watch that cannot open, errors or
    // ends early holds until the deadline instead: answering at once would turn the client's
    // immediate re-ask into a hot loop against the api. The re-list below answers every case.
    let _ = tokio::time::timeout(Duration::from_secs(50), async {
        if let Ok(st) = spaces(&s)?.watch(&wp, &rv).await {
            let mut st = Box::pin(st);
            while let Ok(Some(ev)) = st.try_next().await {
                if let WatchEvent::Added(x) | WatchEvent::Modified(x) | WatchEvent::Deleted(x) = ev {
                    if wakes(&x, &caller) {
                        return Ok::<(), Response>(());
                    }
                }
            }
        }
        std::future::pending().await
    })
    .await;
    let (docs, _) = my_spaces(&s, &caller).await?;
    let tag = etag(&docs);
    if answers_now(seen, &tag) {
        return Ok(spaces_response(docs, &tag));
    }
    let mut r = StatusCode::NOT_MODIFIED.into_response();
    if let Ok(v) = tag.parse() {
        r.headers_mut().insert(axum::http::header::ETAG, v);
    }
    Ok(r)
}


pub(crate) async fn set_my_environment(
    State(s): State<Arc<ApiState>>,
    headers: axum::http::HeaderMap,
    method: axum::http::Method,
    uri: axum::extract::OriginalUri,
    Path(team): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Response, Response> {
    let caller = caller_for(&s, &headers, &method, uri.path()).await?;
    if body.get("owner").is_some() {
        return Err((StatusCode::BAD_REQUEST, "a space is always the caller's own; the body carries no owner").into_response());
    }
    let Some(environment) = body.get("environment").and_then(|v| v.as_str()).map(str::to_string) else {
        return Err((StatusCode::BAD_REQUEST, "environment is required").into_response());
    };
    // Patched verbatim into a label value.
    if !crate::model::valid_segment_label(&environment) {
        return Err((StatusCode::UNPROCESSABLE_ENTITY, "invalid environment id").into_response());
    }
    let team = space_team(&s, &caller, &team).await?;
    let envs: Api<crd::Environment> = Api::all(kube(&s)?.clone());
    // `visible_env`: the hidden builder is indistinguishable from an id that does not exist.
    let e = envs.get_opt(&environment).await.map_err(kube_err)?.filter(environments::visible_env).ok_or_else(not_found)?;
    if !e.spec.owner.eq_ignore_ascii_case(&team) {
        return Err((StatusCode::CONFLICT, format!("that environment is not {team}'s; a space may use only its own team's environments")).into_response());
    }
    let obj = crd::space_environment(&caller.name, &team, &environment);
    let name = obj.metadata.name.clone().unwrap_or_default();
    spaces(&s)?
        .patch(&name, &PatchParams::apply(crd::API_FIELD_MANAGER).force(), &Patch::Apply(&obj))
        .await
        .map_err(kube_err)?;
    Ok(Json(SpaceDoc { team, environment, region: Some(e.spec.region) }).into_response())
}


/// Idempotent: a space with no environment is already the state asked for. Named from the caller,
/// so it can only ever clear the caller's own choice — including one left in a team they have
/// since left, which is why membership is not checked here.
pub(crate) async fn clear_my_environment(
    State(s): State<Arc<ApiState>>,
    headers: axum::http::HeaderMap,
    method: axum::http::Method,
    uri: axum::extract::OriginalUri,
    Path(team): Path<String>,
) -> Result<Response, Response> {
    let caller = caller_for(&s, &headers, &method, uri.path()).await?;
    if !in_scope(&caller, &team) {
        return Err(scope_refusal(&caller));
    }
    match spaces(&s)?.delete(&crd::space_name(&caller.name, &team), &Default::default()).await {
        Ok(_) => {}
        Err(kube::Error::Api(ae)) if ae.code == 404 => {}
        Err(e) => return Err(kube_err(e)),
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}


/// The retired per-workspace and per-bench attach/detach routes, for one release: a stale client
/// fails loudly instead of silently attaching nothing.
// ponytail: delete with the routes next release.
pub(crate) async fn attach_gone() -> Response {
    (StatusCode::GONE, "an environment is chosen per team now: PUT /v1/me/environments/{team}").into_response()
}


#[cfg(test)]
mod tests {
    use super::*;

    fn doc(team: &str, env: &str) -> SpaceDoc {
        SpaceDoc { team: team.into(), environment: env.into(), region: None }
    }

    fn caller() -> Caller {
        Caller { name: "meera".into(), superadmin: false, parent: None, scope: None, jti8: None }
    }

    #[test]
    fn etag_is_order_independent_and_moves_with_an_environment() {
        let a = etag(&[doc("t1", "e1"), doc("meera", "e2")]);
        assert_eq!(a, etag(&[doc("meera", "e2"), doc("t1", "e1")]));
        assert_ne!(a, etag(&[doc("t1", "e9"), doc("meera", "e2")]));
        assert!(a.starts_with('"') && a.ends_with('"'));
    }

    #[test]
    fn wakes_only_for_the_callers_own_space() {
        let c = caller();
        assert!(wakes(&crd::space_environment("meera", "t1", "e1"), &c));
        // The label says meera, spec.owner says bob: spec wins.
        let mut other = crd::space_environment("bob", "t1", "e1");
        other.metadata.labels.get_or_insert_default().insert(crate::k8s::OWNER_LABEL.into(), "meera".into());
        assert!(!wakes(&other, &c));
    }

    #[test]
    fn answers_now_unless_the_tag_matches() {
        assert!(answers_now(None, "\"a\""));
        assert!(answers_now(Some("\"b\""), "\"a\""));
        assert!(!answers_now(Some("\"a\""), "\"a\""));
    }
}
