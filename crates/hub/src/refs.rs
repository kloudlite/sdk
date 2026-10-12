//! Branch and tag create/delete. Every change is one `update_refs` call with an expected old
//! value, so a concurrent writer loses with 409 instead of being overwritten.
//!
//! Tags are lightweight: huggingface_hub's `create_tag` sends a message, which we accept and drop
//! (`// ponytail:` below).

use crate::auth::{open, Access};
use crate::read::commit_of;
use crate::rev::{resolve, revision_not_found, safe_path};
use crate::{hub_err, internal, HubState};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use kloudlite_core::httpx::Trusted;
use kloudlite_gitbase::refs::UpdateRefsExt;
use kloudlite_storage::refmeta::RefUpdate;
use kloudlite_storage::HubKind;
use serde::Deserialize;

/// git's check-ref-format rules that matter for a single path segment we put under `refs/`.
pub fn ref_ok(name: &str) -> bool {
    safe_path(name)
        && !name.contains("..")
        && !name.contains("@{")
        && !name.ends_with(".lock")
        && !name.ends_with('.')
        && !name.bytes().any(|b| b < 0x20 || b == 0x7f || b" ~^:?*[\\".contains(&b))
}

fn bad_name() -> Response {
    hub_err(StatusCode::BAD_REQUEST, None, "invalid reference name")
}

#[derive(Deserialize, Default)]
pub struct BranchBody {
    #[serde(rename = "startingPoint")]
    starting_point: Option<String>,
}

#[derive(Deserialize)]
pub struct TagBody {
    tag: String,
    // ponytail: annotated tags are not written; the message is dropped. Write a tag object when a
    // client is seen to read it back.
    #[allow(dead_code)]
    message: Option<String>,
}

async fn set(st: &HubState, a: &Access, name: String, old: Option<gix_hash::ObjectId>, new: Option<gix_hash::ObjectId>) -> Response {
    match st.app.store.update_refs(&a.repo, &[RefUpdate { name, old, new }]).await {
        Ok(v) if v.iter().all(|x| x.is_none()) => {
            crate::live::after_update(st, a).await;
            Json(serde_json::json!({})).into_response()
        }
        Ok(_) => hub_err(StatusCode::CONFLICT, None, "reference changed concurrently"),
        Err(e) => internal(e),
    }
}

pub async fn branch_create(
    State(st): State<HubState>,
    Extension(kind): Extension<HubKind>,
    trusted: Option<Extension<Trusted>>,
    headers: HeaderMap,
    Path((owner, name, branch)): Path<(String, String, String)>,
    body: Option<Json<BranchBody>>,
) -> Response {
    let a = match open(&st, kind, trusted.as_deref(), &headers, &owner, &name, true).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    if !ref_ok(&branch) {
        return bad_name();
    }
    let start = body.and_then(|b| b.0.starting_point).unwrap_or_else(|| "main".into());
    let at = match resolve(&st.app.store, &a.repo, &start).await {
        Ok(Some(o)) => o,
        Ok(None) => return revision_not_found(&start),
        Err(e) => return internal(e),
    };
    let full = format!("refs/heads/{branch}");
    match st.app.store.get_ref(&a.repo, &full).await {
        Ok(Some(_)) => return hub_err(StatusCode::CONFLICT, None, "branch already exists"),
        Ok(None) => {}
        Err(e) => return internal(e),
    }
    set(&st, &a, full, None, Some(at)).await
}

pub async fn branch_delete(
    State(st): State<HubState>,
    Extension(kind): Extension<HubKind>,
    trusted: Option<Extension<Trusted>>,
    headers: HeaderMap,
    Path((owner, name, branch)): Path<(String, String, String)>,
) -> Response {
    let a = match open(&st, kind, trusted.as_deref(), &headers, &owner, &name, true).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    if branch == "main" {
        return hub_err(StatusCode::FORBIDDEN, None, "the default branch cannot be deleted");
    }
    if !ref_ok(&branch) {
        return bad_name();
    }
    let full = format!("refs/heads/{branch}");
    match st.app.store.get_ref(&a.repo, &full).await {
        Ok(Some(old)) => set(&st, &a, full, Some(old), None).await,
        Ok(None) => revision_not_found(&branch),
        Err(e) => internal(e),
    }
}

pub async fn tag_create(
    State(st): State<HubState>,
    Extension(kind): Extension<HubKind>,
    trusted: Option<Extension<Trusted>>,
    headers: HeaderMap,
    Path((owner, name, rev)): Path<(String, String, String)>,
    Json(body): Json<TagBody>,
) -> Response {
    let a = match open(&st, kind, trusted.as_deref(), &headers, &owner, &name, true).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    if !ref_ok(&body.tag) {
        return bad_name();
    }
    let at = match commit_of(&st, &a, &rev).await {
        Ok(Some(o)) => o,
        Ok(None) => return revision_not_found(&rev),
        Err(r) => return r,
    };
    let full = format!("refs/tags/{}", body.tag);
    match st.app.store.get_ref(&a.repo, &full).await {
        Ok(Some(_)) => hub_err(StatusCode::CONFLICT, None, "tag already exists"),
        Ok(None) => set(&st, &a, full, None, Some(at)).await,
        Err(e) => internal(e),
    }
}

pub async fn tag_delete(
    State(st): State<HubState>,
    Extension(kind): Extension<HubKind>,
    trusted: Option<Extension<Trusted>>,
    headers: HeaderMap,
    Path((owner, name, tag)): Path<(String, String, String)>,
) -> Response {
    let a = match open(&st, kind, trusted.as_deref(), &headers, &owner, &name, true).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    if !ref_ok(&tag) {
        return bad_name();
    }
    let full = format!("refs/tags/{tag}");
    match st.app.store.get_ref(&a.repo, &full).await {
        Ok(Some(old)) => set(&st, &a, full, Some(old), None).await,
        Ok(None) => revision_not_found(&tag),
        Err(e) => internal(e),
    }
}
