//! `HEAD|GET /{prefix}{owner}/{name}/resolve/{rev}/{*path}`: the download route every client
//! uses (huggingface_hub, transformers, git-xet-less curl).
//!
//! A git blob is served as stored. A blob that parses as an LFS pointer is served from the CAS
//! when `xet/{owner}/sha/{sha256}` maps it to a Xet file — with `X-Xet-Hash` and the `xet-auth`
//! link so hf_xet clients skip this route and reconstruct from the CAS themselves — else from the
//! LFS store `lfs/{owner}/sha256/{sha256}`. Every answer past the revision lookup carries
//! `X-Repo-Commit`, 404s included: huggingface_hub caches "absent at this commit" from it.
//!
//! An unsafe path is `EntryNotFound`, never 400: a reader cannot tell "climbs out" from "absent".
//!
//! ponytail: git blobs are inflated whole into memory; the preupload rule keeps git files under
//! 10 MiB, so a streaming blob reader waits until a repo shows otherwise.

use crate::auth::open;
use crate::pointer::{self, POINTER_MAX};
use crate::read::{entry_not_found, xet_hash_of};
use crate::rev::{resolve as resolve_rev, revision_not_found, safe_path};
use crate::{hub_err, internal, HubState};
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::Response;
use axum::Extension;
use futures::TryStreamExt;
use kloudlite_core::httpx::Trusted;
use kloudlite_storage::HubKind;
use kloudlite_xet::recon::parse_range;
use object_store::path::Path as ObjPath;
use object_store::{GetOptions, GetRange, ObjectStore, ObjectStoreExt};

/// Whether `p` exists; a store failure is a 500, never "absent".
pub async fn exists(os: &dyn ObjectStore, p: &ObjPath) -> Result<bool, Response> {
    match os.head(p).await {
        Ok(_) => Ok(true),
        Err(object_store::Error::NotFound { .. }) => Ok(false),
        Err(e) => Err(internal(e)),
    }
}

/// The byte span `[s, e)` a range asks for over `size` bytes; `Err` is unsatisfiable.
fn span(range: Option<(u64, Option<u64>)>, size: u64) -> Result<(u64, u64), ()> {
    match range {
        None => Ok((0, size)),
        Some((s, _)) if s >= size => Err(()),
        Some((s, e)) => Ok((s, e.map(|e| e.saturating_add(1)).unwrap_or(size).min(size))),
    }
}

fn set(r: &mut Response, k: &'static str, v: &str) {
    if let Ok(v) = HeaderValue::from_str(v) {
        r.headers_mut().insert(k, v);
    }
}

pub async fn resolve(
    State(st): State<HubState>,
    Extension(kind): Extension<HubKind>,
    trusted: Option<Extension<Trusted>>,
    method: Method,
    headers: HeaderMap,
    Path((owner, name, rev, path)): Path<(String, String, String, String)>,
) -> Response {
    let a = match open(&st, kind, trusted.as_deref(), &headers, &owner, &name, false).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let commit = match resolve_rev(&st.app.store, &a.repo, &rev).await {
        Ok(Some(c)) => c,
        Ok(None) => return revision_not_found(&rev),
        Err(e) => return internal(e),
    };
    let tag = commit.to_string();
    let mut r = serve(&st, kind, &a.owner, &a.name, a.repo.clone(), commit, &tag, &path, &method, &headers).await;
    set(&mut r, "x-repo-commit", &tag);
    r
}

#[allow(clippy::too_many_arguments)]
async fn serve(
    st: &HubState,
    kind: HubKind,
    owner: &str,
    name: &str,
    repo: kloudlite_storage::store::Repo,
    commit: gix_hash::ObjectId,
    tag: &str,
    path: &str,
    method: &Method,
    headers: &HeaderMap,
) -> Response {
    if !safe_path(path) {
        return entry_not_found();
    }
    let p = path.to_string();
    let blob = tokio::task::spawn_blocking(move || -> kloudlite_core::Result<kloudlite_vcs::browse::Blob> {
        let odb = repo.odb().map_err(|e| kloudlite_core::Error::from(e.to_string()))?;
        kloudlite_vcs::browse::blob_at(&odb, commit, &p, usize::MAX)
    })
    .await;
    let blob = match blob {
        Ok(Ok(b)) => b,
        Ok(Err(e)) if kloudlite_vcs::browse::is_not_found(&e) => return entry_not_found(),
        Ok(Err(e)) => return internal(e),
        Err(e) => return internal(e),
    };
    let Ok(range) = parse_range(headers.get(header::RANGE).and_then(|v| v.to_str().ok())) else {
        return unsatisfiable(blob_size(&blob));
    };
    let head = method == Method::HEAD;
    let ptr = if blob.bytes.len() <= POINTER_MAX { pointer::parse(&blob.bytes) } else { None };
    let Some(ptr) = ptr else {
        let size = blob.bytes.len() as u64;
        let Ok((s, e)) = span(range, size) else { return unsatisfiable(size) };
        let body = if head { Body::empty() } else { Body::from(blob.bytes[s as usize..e as usize].to_vec()) };
        let mut r = framed(body, range.is_some(), s, e, size);
        let q = format!("\"{}\"", blob.oid);
        set(&mut r, "etag", &q);
        set(&mut r, "x-linked-etag", &q);
        set(&mut r, "x-linked-size", &size.to_string());
        return r;
    };
    let size = ptr.size;
    let Ok((s, e)) = span(range, size) else { return unsatisfiable(size) };
    let os = st.cas.os.clone();
    let quoted = format!("\"{}\"", ptr.sha256);
    let mut r = if let Some(xet) = xet_hash_of(st, owner, &ptr.sha256).await {
        let Some(hash) = kloudlite_xet::keys::parse_hash(&xet) else { return internal(format!("bad xet hash for {}", ptr.sha256)) };
        let body = if head {
            Body::empty()
        } else {
            let rec = match kloudlite_xet::shards::read_file(&*os, owner, &hash).await {
                Ok(Some(rec)) => rec,
                Ok(None) => return hub_err(StatusCode::NOT_FOUND, Some("EntryNotFound"), "large file content missing"),
                Err(e) => return internal(e),
            };
            Body::from_stream(kloudlite_xet::stream::file_bytes(os, owner.to_string(), rec, s, e))
        };
        let mut r = framed(body, range.is_some(), s, e, size);
        set(&mut r, "x-xet-hash", &xet);
        let link = format!("<{}/api/{}/{owner}/{name}/xet-read-token/{tag}>; rel=\"xet-auth\"", st.base, kind.api_plural());
        set(&mut r, "link", &link);
        r
    } else {
        let key = kloudlite_xet::keys::lfs(owner, &ptr.sha256);
        match exists(&*os, &key).await {
            Ok(true) => {}
            Ok(false) => return hub_err(StatusCode::NOT_FOUND, Some("EntryNotFound"), "large file content missing"),
            Err(r) => return r,
        }
        let body = if head || s == e {
            Body::empty()
        } else {
            let opts = GetOptions { range: Some(GetRange::Bounded(s..e)), ..Default::default() };
            match os.get_opts(&key, opts).await {
                Ok(got) => Body::from_stream(got.into_stream().map_err(std::io::Error::other)),
                Err(e) => return internal(e),
            }
        };
        framed(body, range.is_some(), s, e, size)
    };
    set(&mut r, "etag", &quoted);
    set(&mut r, "x-linked-etag", &quoted);
    set(&mut r, "x-linked-size", &size.to_string());
    r
}

fn blob_size(b: &kloudlite_vcs::browse::Blob) -> u64 {
    pointer::parse(&b.bytes).map(|p| p.size).unwrap_or(b.bytes.len() as u64)
}

fn unsatisfiable(size: u64) -> Response {
    let mut r = hub_err(StatusCode::RANGE_NOT_SATISFIABLE, None, "range not satisfiable");
    set(&mut r, "content-range", &format!("bytes */{size}"));
    r
}

/// Status, length and range headers for `[s, e)` of `size` bytes.
fn framed(body: Body, ranged: bool, s: u64, e: u64, size: u64) -> Response {
    let mut r = Response::new(body);
    *r.status_mut() = if ranged { StatusCode::PARTIAL_CONTENT } else { StatusCode::OK };
    set(&mut r, "content-length", &(e - s).to_string());
    set(&mut r, "accept-ranges", "bytes");
    set(&mut r, "content-type", "application/octet-stream");
    if ranged {
        set(&mut r, "content-range", &format!("bytes {s}-{}/{size}", e.saturating_sub(1)));
    }
    r
}
