//! Xet CAS tokens. huggingface_hub asks for one before every Xet upload or download and reads the
//! three `X-Xet-*` headers (newer clients read the JSON body). The token names the hub routing key
//! as `repo`, so a CAS request can be traced back to the repo that authorised it.

use crate::auth::{open, Access};
use crate::read::commit_of;
use crate::rev::revision_not_found;
use crate::{internal, HubState};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use kloudlite_core::httpx::Trusted;
use kloudlite_storage::{hub_routing_key, HubKind};

pub const TOKEN_TTL: u64 = 3600;

/// `(cas_url, token, exp)`. Task 12 (git-lfs batch) hands the same triple to its clients.
pub fn token_headers(st: &HubState, a: &Access, rev: &str, write: bool) -> Result<(String, String, u64), Response> {
    let exp = kloudlite_xet::sign::now() + TOKEN_TTL;
    let token = st
        .app
        .jwt
        .mint_cas(&a.owner, &hub_routing_key(a.kind, &a.owner, &a.name), rev, write, TOKEN_TTL)
        .map_err(internal)?;
    Ok((st.cas.base.clone(), token, exp))
}

fn reply(st: &HubState, a: &Access, rev: &str, write: bool) -> Result<Response, Response> {
    let (cas, token, exp) = token_headers(st, a, rev, write)?;
    let mut r = Json(serde_json::json!({ "accessToken": token, "exp": exp, "casUrl": cas })).into_response();
    let h = r.headers_mut();
    for (k, v) in [("x-xet-cas-url", cas), ("x-xet-access-token", token), ("x-xet-token-expiration", exp.to_string())] {
        if let Ok(v) = HeaderValue::from_str(&v) {
            h.insert(k, v);
        }
    }
    Ok(r)
}

#[allow(clippy::too_many_arguments)]
async fn token(st: HubState, kind: HubKind, trusted: Option<Extension<Trusted>>, headers: HeaderMap, owner: String, name: String, rev: String, write: bool) -> Response {
    let a = match open(&st, kind, trusted.as_deref(), &headers, &owner, &name, write).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    // A write token for a branch that does not exist yet is fine: the commit creates it.
    if !write {
        // `Ok(None)` is the empty repo's `main`, which `commit_of` allows.
        if let Err(r) = commit_of(&st, &a, &rev).await {
            return r;
        }
    } else if !crate::refs::ref_ok(&rev) {
        return revision_not_found(&rev);
    }
    reply(&st, &a, &rev, write).unwrap_or_else(|r| r)
}

pub async fn read_token(
    State(st): State<HubState>,
    Extension(kind): Extension<HubKind>,
    trusted: Option<Extension<Trusted>>,
    headers: HeaderMap,
    Path((owner, name, rev)): Path<(String, String, String)>,
) -> Response {
    token(st, kind, trusted, headers, owner, name, rev, false).await
}

pub async fn write_token(
    State(st): State<HubState>,
    Extension(kind): Extension<HubKind>,
    trusted: Option<Extension<Trusted>>,
    headers: HeaderMap,
    Path((owner, name, rev)): Path<(String, String, String)>,
) -> Response {
    token(st, kind, trusted, headers, owner, name, rev, true).await
}
