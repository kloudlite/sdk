//! The Hugging Face Hub API, served on its own listener (`KLOUDLITE_HUB_ADDR`, `hub.<domain>`).
//!
//! Model repos live at `/{owner}/{name}`, datasets at `/datasets/{owner}/{name}`; each is an
//! ordinary git repo in its own namespace (`mdl`, `ds`; see `kloudlite_storage::hubrepo`), so
//! history, packs and refs reuse the git tier unchanged. Large files are LFS pointers in git and
//! bytes in the per-owner Xet CAS (`kloudlite_xet`), which this crate only mints tokens and URLs
//! for.
//!
//! Every route here touches a per-repo DB except `/api/repos/*` and `/api/whoami-v2`; the
//! server's `route_hub` middleware routes them to the owning node BEFORE this router runs, so
//! nothing in this crate forwards.
//!
//! Module map: `auth` (caller, visibility), `repos` (create, delete, whoami), `rev` (revision
//! names), `pointer` (LFS pointer text), `read` (info, tree, paths-info, refs), `resolve` (file
//! bytes), `commit` (preupload, NDJSON commit), `refs_api` (branches, tags), `tokens` (Xet
//! tokens), `lfs_batch` (git-lfs batch).
#![deny(clippy::unwrap_used, clippy::expect_used)]
// The error side of every guard is the finished HTTP response; boxing it buys nothing.
#![allow(clippy::result_large_err)]

pub mod auth;
pub mod commit;
pub mod live;
pub mod pointer;
pub mod read;
pub mod refs;
pub mod repos;
pub mod resolve;
pub mod rev;
pub mod xet;

use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use kloudlite_app::App;
use kloudlite_storage::HubKind;
use kloudlite_xet::routes::CasState;
use std::sync::Arc;

#[derive(Clone)]
pub struct HubState {
    pub app: Arc<App>,
    pub cas: CasState,
    /// External URL of the hub listener, no trailing slash. Repo URLs and `Link` headers use it.
    pub base: String,
}

impl HubState {
    pub fn new(app: Arc<App>, base: String) -> HubState {
        let cas = CasState { os: app.store.os.clone(), jwt: app.jwt.clone(), base: kloudlite_xet::cas_url() };
        HubState { app, cas, base: base.trim_end_matches('/').to_string() }
    }
}

pub fn hub_url() -> String {
    std::env::var("KLOUDLITE_HUB_URL")
        .unwrap_or_else(|_| "http://localhost:8090".into())
        .trim_end_matches('/')
        .to_string()
}

/// huggingface_hub turns `X-Error-Code` into a typed exception (`RepositoryNotFoundError`,
/// `EntryNotFoundError`, `RevisionNotFoundError`) and shows `X-Error-Message`; the JSON body is for
/// everyone else.
pub fn hub_err(status: StatusCode, code: Option<&str>, msg: &str) -> Response {
    let mut r = (status, Json(serde_json::json!({ "error": msg }))).into_response();
    let h = r.headers_mut();
    if let Some(c) = code.and_then(|c| HeaderValue::from_str(c).ok()) {
        h.insert("x-error-code", c);
    }
    if let Ok(m) = HeaderValue::from_str(msg) {
        h.insert("x-error-message", m);
    }
    r
}

pub fn internal(e: impl std::fmt::Display) -> Response {
    tracing::error!(target: "hub", error = %e, "hub.internal");
    hub_err(StatusCode::INTERNAL_SERVER_ERROR, None, "internal error")
}

pub fn router(st: HubState) -> Router {
    Router::new()
        .route("/api/repos/create", post(repos::create))
        .route("/api/repos/delete", delete(repos::delete))
        .route("/api/whoami-v2", get(repos::whoami))
        .merge(per_kind(HubKind::Model))
        .merge(per_kind(HubKind::Dataset))
        .with_state(st)
}

/// Routes that exist once per kind. Paths are built from the kind so the two never drift; the
/// kind reaches handlers as an `Extension`, set by this router's own layer.
fn per_kind(kind: HubKind) -> Router<HubState> {
    let api = format!("/api/{}/{{owner}}/{{name}}", kind.api_plural());
    let repo = format!("/{}{{owner}}/{{name}}", kind.url_prefix());
    Router::new()
        .route(&api, get(read::info))
        .route(&format!("{api}/revision/{{rev}}"), get(read::info_rev))
        .route(&format!("{api}/tree/{{rev}}"), get(read::tree_root))
        .route(&format!("{api}/tree/{{rev}}/{{*path}}"), get(read::tree_path))
        .route(&format!("{api}/paths-info/{{rev}}"), post(read::paths_info))
        .route(&format!("{api}/refs"), get(read::refs))
        .route(&format!("{repo}/resolve/{{rev}}/{{*path}}"), get(resolve::resolve))
        .route(&format!("{api}/preupload/{{rev}}"), post(commit::preupload))
        .route(
            &format!("{api}/commit/{{rev}}"),
            post(commit::commit).layer(axum::extract::DefaultBodyLimit::max(commit::COMMIT_MAX)),
        )
        .route(&format!("{api}/branch/{{branch}}"), post(refs::branch_create).delete(refs::branch_delete))
        .route(&format!("{api}/tag/{{rev}}"), post(refs::tag_create).delete(refs::tag_delete))
        .route(&format!("{api}/xet-read-token/{{rev}}"), get(xet::read_token))
        .route(&format!("{api}/xet-write-token/{{rev}}"), get(xet::write_token))
        .layer(axum::Extension(kind))
}
