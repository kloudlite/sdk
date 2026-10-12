//! Git LFS object bytes, served by the CAS listener behind presigned URLs.
//!
//! The hub's LFS batch endpoint (`crates/hub`) authenticates the caller and hands out these
//! URLs; this side checks only the signature. An upload is staged under a random key and
//! hashed as it streams, so a body that does not hash to its oid never becomes visible at the
//! final key, and two racing uploads of the same oid both end at the same bytes
//! (`copy_if_not_exists`, the loser's copy is refused and it deletes its staging key).

use crate::routes::CasState;
use crate::{keys, sign, MAX_LFS_OBJECT};
use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use futures::StreamExt;
use sha2::{Digest, Sha256};
use slatedb::object_store::{path::Path as OsPath, ObjectStoreExt, WriteMultipart};
use std::collections::HashMap;

pub fn lfs_url(st: &CasState, owner: &str, oid: &str, ttl: u64) -> String {
    sign::signed_url(&st.base, st.jwt.secret(), &format!("/v1/lfs/{owner}/{oid}"), ttl)
}

/// Both segments become part of an object key: the oid must be a lowercase sha256 and the owner
/// must not be able to step out of its prefix (the signature covers the path, this is the backstop).
fn valid(owner: &str, oid: &str) -> bool {
    keys::is_sha_hex(oid) && !owner.is_empty() && !owner.contains(['/', '\\']) && !owner.contains("..")
}

fn signed(st: &CasState, owner: &str, oid: &str, q: &HashMap<String, String>) -> bool {
    let (Some(exp), Some(sig)) = (q.get("exp").and_then(|e| e.parse().ok()), q.get("sig")) else { return false };
    sign::verify(st.jwt.secret(), &format!("/v1/lfs/{owner}/{oid}"), exp, sig, sign::now())
}

fn refuse(code: StatusCode, msg: &str) -> Response {
    (code, msg.to_string()).into_response()
}

fn internal(e: impl std::fmt::Display) -> Response {
    tracing::error!(error = %e, "lfs.failed");
    refuse(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
}

pub(crate) async fn get_lfs(
    State(st): State<CasState>,
    Path((owner, oid)): Path<(String, String)>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    if !signed(&st, &owner, &oid, &q) {
        return refuse(StatusCode::FORBIDDEN, "bad or expired signature");
    }
    if !valid(&owner, &oid) {
        return refuse(StatusCode::BAD_REQUEST, "bad owner or oid is not a sha256");
    }
    match st.os.get(&keys::lfs(&owner, &oid)).await {
        Ok(g) => (StatusCode::OK, Body::from_stream(g.into_stream())).into_response(),
        Err(slatedb::object_store::Error::NotFound { .. }) => match xet_bytes(&st, &owner, &oid).await {
            Ok(Some(body)) => (StatusCode::OK, body).into_response(),
            Ok(None) => refuse(StatusCode::NOT_FOUND, "no such object"),
            Err(e) => internal(e),
        },
        Err(e) => internal(e),
    }
}

/// A Xet-uploaded file has no whole object: `sha/{sha256}` names its file hash, whose record
/// lists the xorb ranges to stream. git-lfs clients see the same bytes either way.
async fn xet_bytes(st: &CasState, owner: &str, oid: &str) -> Result<Option<Body>, String> {
    let fh = match st.os.get(&keys::sha(owner, oid)).await {
        Ok(g) => g.bytes().await.map_err(|e| e.to_string())?,
        Err(slatedb::object_store::Error::NotFound { .. }) => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let Some(h) = keys::parse_hash(String::from_utf8_lossy(&fh).trim()) else { return Err("corrupt sha record".into()) };
    let Some(rec) = crate::shards::read_file(&*st.os, owner, &h).await.map_err(|e| e.to_string())? else { return Ok(None) };
    let size = rec.size;
    Ok(Some(Body::from_stream(crate::stream::file_bytes(st.os.clone(), owner.to_string(), rec, 0, size))))
}

pub(crate) async fn put_lfs(
    State(st): State<CasState>,
    Path((owner, oid)): Path<(String, String)>,
    Query(q): Query<HashMap<String, String>>,
    body: Body,
) -> Response {
    put_lfs_capped(st, owner, oid, q, body, MAX_LFS_OBJECT).await
}

async fn put_lfs_capped(
    st: CasState,
    owner: String,
    oid: String,
    q: HashMap<String, String>,
    body: Body,
    max: u64,
) -> Response {
    if !signed(&st, &owner, &oid, &q) {
        return refuse(StatusCode::FORBIDDEN, "bad or expired signature");
    }
    if !valid(&owner, &oid) {
        return refuse(StatusCode::BAD_REQUEST, "bad owner or oid is not a sha256");
    }
    let dest = keys::lfs(&owner, &oid);
    match st.os.head(&dest).await {
        Ok(_) => return StatusCode::OK.into_response(),
        Err(slatedb::object_store::Error::NotFound { .. }) => {}
        Err(e) => return internal(e),
    }
    let tmp = OsPath::from(format!("lfs/{owner}/tmp/{oid}-{:016x}", rand::random::<u64>()));
    let upload = match st.os.put_multipart(&tmp).await {
        Ok(u) => u,
        Err(e) => return internal(e),
    };
    let mut w = WriteMultipart::new(upload);
    let (mut hasher, mut n) = (Sha256::new(), 0u64);
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                let _ = w.abort().await;
                return refuse(StatusCode::BAD_REQUEST, &format!("reading body: {e}"));
            }
        };
        n += chunk.len() as u64;
        if n > max {
            let _ = w.abort().await;
            return refuse(StatusCode::PAYLOAD_TOO_LARGE, "object over 5 GiB");
        }
        hasher.update(&chunk);
        if let Err(e) = w.wait_for_capacity(8).await {
            let _ = w.abort().await;
            return internal(e);
        }
        w.write(&chunk);
    }
    if let Err(e) = w.finish().await {
        return internal(e);
    }
    let got = hex::encode(hasher.finalize());
    let result = if got != oid {
        refuse(StatusCode::BAD_REQUEST, &format!("body hashes to {got}, not {oid}"))
    } else {
        // Plain copy: S3 has no conditional copy without extra config, and the key is content-addressed
        // with the sha256 just verified, so a racing overwrite writes identical bytes.
        match st.os.copy(&tmp, &dest).await {
            Ok(()) => StatusCode::OK.into_response(),
            Err(e) => internal(e),
        }
    };
    // ponytail: a crash between finish and this delete strands a tmp object; the GC lane
    // (Task 13) sweeps `lfs/*/tmp/` older than a day.
    let _ = st.os.delete(&tmp).await;
    result
}

#[cfg(test)]
mod tests {
    use crate::routes::{router, CasState};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use kloudlite_core::jwt::Jwt;
    use sha2::{Digest, Sha256};
    use slatedb::object_store::memory::InMemory;
    use slatedb::object_store::ObjectStore;
    use std::sync::Arc;
    use tower::ServiceExt;
    use futures::StreamExt;

    fn setup() -> (axum::Router, CasState) {
        let st = CasState {
            os: Arc::new(InMemory::new()),
            jwt: Arc::new(Jwt::new(&"k".repeat(32)).unwrap()),
            base: "http://cas.test".into(),
        };
        (router(st.clone()), st)
    }

    async fn call(app: &axum::Router, method: &str, uri: &str, body: Vec<u8>) -> (StatusCode, Vec<u8>) {
        let r = Request::builder().method(method).uri(uri).body(Body::from(body)).unwrap();
        let resp = app.clone().oneshot(r).await.unwrap();
        let s = resp.status();
        (s, axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap().to_vec())
    }

    fn local(url: &str) -> String {
        url.strip_prefix("http://cas.test").unwrap().to_string()
    }

    #[tokio::test]
    async fn put_then_get_round_trips() {
        let (app, st) = setup();
        let body = b"weights".to_vec();
        let oid = hex::encode(Sha256::digest(&body));
        let url = local(&super::lfs_url(&st, "alice", &oid, 600));
        assert_eq!(call(&app, "PUT", &url, body.clone()).await.0, StatusCode::OK);
        assert_eq!(call(&app, "PUT", &url, body.clone()).await.0, StatusCode::OK, "second put is a no-op");
        let (s, got) = call(&app, "GET", &url, vec![]).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(got, body);
        let staged = st.os.list(Some(&"lfs/alice/tmp".into())).next().await;
        assert!(staged.is_none(), "staging key left behind");
    }

    #[tokio::test]
    async fn get_falls_back_to_a_xet_file_by_sha256() {
        let (app, st) = setup();
        let b = crate::testkit::build(&[b"hello ", b"xet"]);
        crate::xorbs::put_xorb(&*st.os, "alice", &b.xorb_hash, bytes::Bytes::from(b.xorb.clone())).await.unwrap();
        crate::shards::put_shard(&*st.os, "alice", &b.shard).await.unwrap();
        let oid = hex::encode(b.sha256);
        let (s, got) = call(&app, "GET", &local(&super::lfs_url(&st, "alice", &oid, 600)), vec![]).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(got, b.bytes);
    }

    #[tokio::test]
    async fn wrong_bytes_are_refused_and_leave_nothing() {
        let (app, st) = setup();
        let oid = hex::encode(Sha256::digest(b"right"));
        let url = local(&super::lfs_url(&st, "alice", &oid, 600));
        assert_eq!(call(&app, "PUT", &url, b"wrong".to_vec()).await.0, StatusCode::BAD_REQUEST);
        assert_eq!(call(&app, "GET", &url, vec![]).await.0, StatusCode::NOT_FOUND);
        assert!(st.os.list(Some(&"lfs/alice".into())).next().await.is_none());
    }

    #[tokio::test]
    async fn bad_signature_and_bad_oid_are_refused() {
        let (app, st) = setup();
        let oid = hex::encode(Sha256::digest(b"x"));
        let (s, _) = call(&app, "PUT", &format!("/v1/lfs/alice/{oid}?exp=99999999999&sig=00"), b"x".to_vec()).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        let url = local(&super::lfs_url(&st, "alice", "NOTHEX", 600));
        assert_eq!(call(&app, "PUT", &url, b"x".to_vec()).await.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn over_cap_is_413_and_leaves_nothing() {
        let (_, st) = setup();
        let oid = hex::encode(Sha256::digest(b"0123456789"));
        let url = local(&super::lfs_url(&st, "alice", &oid, 600));
        let q = url.split_once('?').unwrap().1.split('&').map(|kv| {
            let (k, v) = kv.split_once('=').unwrap();
            (k.to_string(), v.to_string())
        }).collect();
        let r = super::put_lfs_capped(st.clone(), "alice".into(), oid, q, Body::from(b"0123456789".to_vec()), 4).await;
        assert_eq!(r.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert!(st.os.list(Some(&"lfs/alice".into())).next().await.is_none());
    }

    #[tokio::test]
    async fn owner_cannot_escape_and_big_body_is_not_capped_at_64_mib() {
        let (app, st) = setup();
        let oid = hex::encode(Sha256::digest(b"x"));
        let url = local(&super::lfs_url(&st, "..", &oid, 600));
        assert_eq!(call(&app, "PUT", &url, b"x".to_vec()).await.0, StatusCode::BAD_REQUEST);
        let big = vec![7u8; 65 * 1024 * 1024];
        let oid = hex::encode(Sha256::digest(&big));
        let url = local(&super::lfs_url(&st, "alice", &oid, 600));
        assert_eq!(call(&app, "PUT", &url, big).await.0, StatusCode::OK);
    }
}
