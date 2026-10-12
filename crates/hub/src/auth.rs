//! Who is calling, and may they see or change this repo.
//!
//! Credentials: Bearer PAT (huggingface_hub, transformers) or Basic `user:PAT` (git, git-lfs). A
//! credential that does not verify is a 401, never a silent downgrade to anonymous. Visibility
//! follows the spec: anonymous on a private or missing repo is 401 (so huggingface_hub prompts for
//! login), a signed-in stranger gets 404 `RepoNotFound` (existence is not leaked), and a write by
//! a stranger to a public repo is 403.
use crate::{hub_err, internal, HubState};
use axum::http::header::{AUTHORIZATION, WWW_AUTHENTICATE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use kloudlite_app::App;
use kloudlite_core::httpx::{basic_token, basic_user_names, scheme, Trusted};
use kloudlite_storage::store::Repo;
use kloudlite_storage::HubKind;

fn bad_credential() -> Response {
    let mut r = hub_err(StatusCode::UNAUTHORIZED, None, "invalid credentials");
    r.headers_mut().insert(WWW_AUTHENTICATE, HeaderValue::from_static("Basic realm=\"kloudlite\""));
    r
}

pub async fn caller(app: &App, trusted: Option<&Trusted>, headers: &HeaderMap) -> Result<Option<String>, Response> {
    // A peer forwarded this request after authenticating it; `trust_peer` checked the secret.
    if let Some(Trusted(Some(o))) = trusted {
        return Ok(Some(o.clone()));
    }
    let Some(v) = headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok()) else {
        return Ok(None);
    };
    let token = match (scheme(v, "Bearer"), basic_token(headers)) {
        (Some(t), _) => t.to_string(),
        (None, Some(t)) => t,
        (None, None) => return Err(bad_credential()),
    };
    match app.store.owner_for_token(&token).await {
        // git-lfs replays the git remote's credentials, so git's `x` placeholder is a valid user.
        Ok(Some(o)) if basic_user_names(headers, &o, true) => Ok(Some(o)),
        Ok(_) => Err(bad_credential()),
        Err(e) => Err(internal(e)),
    }
}

/// Owner, or a member the directory vouches for. A directory that cannot answer denies.
pub async fn may_write(app: &App, caller: &str, owner: &str) -> bool {
    if caller == owner {
        return true;
    }
    match app.may_act(caller, owner).await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(target: "hub", error = %e, caller, owner, "hub.may_act.unavailable");
            false
        }
    }
}

pub struct Access {
    pub kind: HubKind,
    pub owner: String,
    pub name: String,
    pub repo: Repo,
    pub caller: Option<String>,
}

fn not_found(caller: &Option<String>) -> Response {
    match caller {
        None => hub_err(StatusCode::UNAUTHORIZED, Some("RepoNotFound"), "Repository not found"),
        Some(_) => hub_err(StatusCode::NOT_FOUND, Some("RepoNotFound"), "Repository not found"),
    }
}

pub async fn open(
    st: &HubState,
    kind: HubKind,
    trusted: Option<&Trusted>,
    headers: &HeaderMap,
    owner: &str,
    name: &str,
    write: bool,
) -> Result<Access, Response> {
    let caller = caller(&st.app, trusted, headers).await?;
    let member = match &caller {
        Some(c) => may_write(&st.app, c, owner).await,
        None => false,
    };
    let store = &st.app.store;
    let repo = match store.hub_open(kind, owner, name).await {
        Ok(Some(r)) => r,
        Ok(None) => return Err(not_found(&caller)),
        Err(e) => return Err(internal(e)),
    };
    if !member {
        let public = store.hub_public(kind, owner, name).await.map_err(internal)?;
        if !public {
            return Err(not_found(&caller));
        }
        if write {
            return Err(match caller {
                None => bad_credential(),
                Some(_) => hub_err(StatusCode::FORBIDDEN, None, "no write access to this repository"),
            });
        }
    }
    Ok(Access { kind, owner: owner.to_string(), name: name.to_string(), repo, caller })
}
