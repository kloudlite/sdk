//! The git-lfs batch API, so `git clone` / `git push` with git-lfs (or git-xet) work against hub
//! repos. Only the batch call lives here, on the owning node, because it checks visibility; bytes
//! move through signed URLs on the CAS listener (`kloudlite_xet::lfs`), which any node serves.
//!
//! Per-object problems (unknown oid, malformed oid) are per-object `error`s with 200 overall, as
//! the LFS spec requires; only auth failures fail the whole call.

use crate::auth::open;
use crate::commit::uploaded;
use crate::{hub_err, HubState};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Extension;
use kloudlite_core::httpx::Trusted;
use kloudlite_storage::HubKind;
use serde::Deserialize;
use serde_json::{json, Value};

const LFS_JSON: &str = "application/vnd.git-lfs+json";
const HREF_TTL: u64 = 3600;

#[derive(Deserialize)]
pub struct Req {
    operation: String,
    #[serde(default)]
    transfers: Vec<String>,
    objects: Vec<Obj>,
    #[serde(rename = "ref")]
    git_ref: Option<Value>,
}

#[derive(Deserialize)]
struct Obj {
    oid: String,
    size: u64,
}

fn lfs_reply(code: StatusCode, v: Value) -> Response {
    (code, [("content-type", LFS_JSON)], v.to_string()).into_response()
}

/// The branch the push targets, for the Xet write token's `rev`. `ref.name` is
/// `refs/heads/<branch>`; anything else falls back to `main`.
fn branch_of(r: &Option<Value>) -> String {
    r.as_ref()
        .and_then(|v| v["name"].as_str())
        .and_then(|n| n.strip_prefix("refs/heads/"))
        .filter(|b| crate::refs::ref_ok(b))
        .unwrap_or("main")
        .to_string()
}

pub async fn batch(
    State(st): State<HubState>,
    Extension(kind): Extension<HubKind>,
    trusted: Option<Extension<Trusted>>,
    headers: HeaderMap,
    Path((owner, repo)): Path<(String, String)>,
    body: axum::body::Bytes,
) -> Response {
    let name = repo.strip_suffix(".git").unwrap_or(&repo).to_string();
    let Ok(req) = serde_json::from_slice::<Req>(&body) else {
        return lfs_reply(StatusCode::UNPROCESSABLE_ENTITY, json!({ "message": "malformed batch request" }));
    };
    let write = match req.operation.as_str() {
        "download" => false,
        "upload" => true,
        _ => return lfs_reply(StatusCode::UNPROCESSABLE_ENTITY, json!({ "message": "unknown operation" })),
    };
    let a = match open(&st, kind, trusted.as_deref(), &headers, &owner, &name, write).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let use_xet = write && req.transfers.iter().any(|t| t == "xet");
    let branch = branch_of(&req.git_ref);
    let mut out = Vec::with_capacity(req.objects.len());
    for o in &req.objects {
        if !kloudlite_xet::keys::is_sha_hex(&o.oid) {
            out.push(json!({ "oid": o.oid, "size": o.size, "error": { "code": 422, "message": "oid is not a sha256" } }));
            continue;
        }
        let have = match uploaded(&st, &owner, &o.oid).await {
            Ok(b) => b,
            Err(r) => return r,
        };
        let url = || kloudlite_xet::lfs::lfs_url(&st.cas, &owner, &o.oid, HREF_TTL);
        let entry = match (write, have) {
            (false, false) => json!({ "oid": o.oid, "size": o.size, "error": { "code": 404, "message": "object not found" } }),
            (false, true) => json!({ "oid": o.oid, "size": o.size, "authenticated": true, "actions": { "download": {
                "href": url(), "expires_in": HREF_TTL } } }),
            (true, true) => json!({ "oid": o.oid, "size": o.size, "authenticated": true }),
            (true, false) if use_xet => {
                let (cas, token, exp) = match crate::xet::token_headers(&st, &a, &branch, true) {
                    Ok(t) => t,
                    Err(r) => return r,
                };
                let href = format!("{}/api/{}/{owner}/{name}/xet-write-token/{branch}", st.base, kind.api_plural());
                json!({ "oid": o.oid, "size": o.size, "authenticated": true, "actions": { "upload": {
                    "href": href, "expires_in": HREF_TTL,
                    "header": { "X-Xet-Cas-Url": cas, "X-Xet-Access-Token": token, "X-Xet-Token-Expiration": exp.to_string() } } } })
            }
            (true, false) => json!({ "oid": o.oid, "size": o.size, "authenticated": true, "actions": { "upload": {
                "href": url(), "expires_in": HREF_TTL } } }),
        };
        out.push(entry);
    }
    lfs_reply(StatusCode::OK, json!({ "transfer": if use_xet { "xet" } else { "basic" }, "objects": out, "hash_algo": "sha256" }))
}

pub async fn locks() -> Response {
    hub_err(StatusCode::NOT_FOUND, None, "locking is not supported")
}
