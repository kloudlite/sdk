//! Repo lifecycle and identity: `POST /api/repos/create`, `DELETE /api/repos/delete`,
//! `GET /api/whoami-v2`. The server routes create/delete by the key in the JSON body (Task 14);
//! these handlers assume they already run on the owning node.
use crate::auth::{caller, may_write};
use crate::{hub_err, internal, HubState};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use kloudlite_core::httpx::Trusted;
use kloudlite_storage::HubKind;
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
pub struct RepoBody {
    pub name: String,
    pub organization: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub private: Option<bool>,
}

/// `(kind, owner, name)` from a create/delete body, or the 400 that refuses it. `caller` fills in
/// the owner when the body names none.
pub fn target(b: &RepoBody, caller: Option<&str>) -> Result<(HubKind, String, String), Response> {
    let kind = match b.kind.as_deref().unwrap_or("model") {
        "space" => return Err(hub_err(StatusCode::BAD_REQUEST, None, "Spaces are not served")),
        t => HubKind::from_type(t).ok_or_else(|| hub_err(StatusCode::BAD_REQUEST, None, "type must be model or dataset"))?,
    };
    let (owner, name) = match (&b.organization, b.name.split_once('/')) {
        (Some(o), _) => (o.clone(), b.name.clone()),
        (None, Some((o, n))) => (o.to_string(), n.to_string()),
        (None, None) => match caller {
            Some(c) => (c.to_string(), b.name.clone()),
            None => return Err(hub_err(StatusCode::UNAUTHORIZED, None, "sign in to name an owner")),
        },
    };
    Ok((kind, owner, name))
}

pub fn repo_url(base: &str, kind: HubKind, owner: &str, name: &str) -> String {
    format!("{base}/{}{owner}/{name}", kind.url_prefix())
}

pub async fn create(
    State(st): State<HubState>,
    trusted: Option<Extension<Trusted>>,
    headers: HeaderMap,
    Json(b): Json<RepoBody>,
) -> Response {
    let who = match caller(&st.app, trusted.as_deref(), &headers).await {
        Ok(Some(c)) => c,
        Ok(None) => return hub_err(StatusCode::UNAUTHORIZED, None, "sign in to create a repository"),
        Err(r) => return r,
    };
    let (kind, owner, name) = match target(&b, Some(&who)) {
        Ok(t) => t,
        Err(r) => return r,
    };
    if !may_write(&st.app, &who, &owner).await {
        return hub_err(StatusCode::FORBIDDEN, None, "you cannot create repositories for this owner");
    }
    let url = repo_url(&st.base, kind, &owner, &name);
    let store = &st.app.store;
    match store.hub_exists(kind, &owner, &name).await {
        Ok(true) => {
            let mut r = (StatusCode::CONFLICT, Json(json!({ "error": "You already created this repo", "url": url }))).into_response();
            if let Ok(v) = "You already created this repo".parse() {
                r.headers_mut().insert("x-error-message", v);
            }
            return r;
        }
        Ok(false) => {}
        Err(e) => return internal(e),
    }
    // ponytail: exists-then-create can race two creators; the loser's create fails inside the
    // store and surfaces as a 500. A conditional create in `hub_create` closes it if it matters.
    match store.hub_create(kind, &owner, &name, b.private.unwrap_or(true)).await {
        Ok(()) => Json(json!({ "url": url })).into_response(),
        Err(e) if e.to_string().contains("invalid") => hub_err(StatusCode::BAD_REQUEST, None, "invalid repository name"),
        Err(e) => internal(e),
    }
}

pub async fn delete(
    State(st): State<HubState>,
    trusted: Option<Extension<Trusted>>,
    headers: HeaderMap,
    Json(b): Json<RepoBody>,
) -> Response {
    let who = match caller(&st.app, trusted.as_deref(), &headers).await {
        Ok(Some(c)) => c,
        Ok(None) => return hub_err(StatusCode::UNAUTHORIZED, Some("RepoNotFound"), "Repository not found"),
        Err(r) => return r,
    };
    let (kind, owner, name) = match target(&b, Some(&who)) {
        Ok(t) => t,
        Err(r) => return r,
    };
    let gone = || hub_err(StatusCode::NOT_FOUND, Some("RepoNotFound"), "Repository not found");
    if !may_write(&st.app, &who, &owner).await {
        return gone();
    }
    match st.app.store.hub_exists(kind, &owner, &name).await {
        Ok(true) => {}
        Ok(false) => return gone(),
        Err(e) => return internal(e),
    }
    match st.app.store.hub_delete(kind, &owner, &name).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => internal(e),
    }
}

pub async fn whoami(State(st): State<HubState>, trusted: Option<Extension<Trusted>>, headers: HeaderMap) -> Response {
    match caller(&st.app, trusted.as_deref(), &headers).await {
        Ok(Some(c)) => Json(json!({
            "type": "user",
            "name": c,
            "fullname": c,
            "email": null,
            "orgs": [],
            "auth": { "accessToken": { "displayName": "token", "role": "write" } },
        }))
        .into_response(),
        Ok(None) => hub_err(StatusCode::UNAUTHORIZED, None, "sign in first"),
        Err(r) => r,
    }
}
