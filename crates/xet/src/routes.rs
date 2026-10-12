//! The CAS listener: what hf-xet calls with the token the hub minted (`/api/.../xet-*-token`).
//!
//! Every bearer route takes the OWNER from the token, never the URL: objects are deduplicated
//! per owner, so a token is a key to that owner's whole pool and to nothing else. Downloads go
//! through `/v1/fetch`, presigned (HMAC over path and expiry, `sign.rs`) because hf-xet sends no
//! bearer on fetch URLs. Absent on purpose: `/v2/*` (the client falls back to v1 on 404) and
//! `/v1/chunks/*` (global dedup is out of scope; 404 means "no dedup hit").

use crate::{keys, recon, shards, sign, xorbs, MAX_XET_OBJECT};
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, RawQuery, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use kloudlite_core::jwt::{CasClaims, Jwt};
use serde_json::{json, Map, Value};
use slatedb::object_store::{GetOptions, GetRange, ObjectStore, ObjectStoreExt};
use std::sync::Arc;

#[derive(Clone)]
pub struct CasState {
    pub os: Arc<dyn ObjectStore>,
    pub jwt: Arc<Jwt>,
    /// External base URL of this listener (`cas_url()`), used in presigned fetch URLs.
    pub base: String,
}

/// A presigned fetch URL lives this long: longer than the slowest single range of a large
/// download, short enough that a leaked link stops working the same day.
const FETCH_TTL: u64 = 3600;

pub fn router(st: CasState) -> Router {
    Router::new()
        .route("/v1/xorbs/default/{hash}", post(post_xorb))
        .route("/v1/shards", post(post_shard))
        .route("/v1/reconstructions", get(get_batch))
        .route("/v1/reconstructions/{hash}", get(get_recon))
        .route("/v1/fetch/{owner}/{xorb}", get(get_fetch))
        .route("/v1/lfs/{owner}/{oid}", get(crate::lfs::get_lfs).put(crate::lfs::put_lfs))
        .layer(DefaultBodyLimit::max(MAX_XET_OBJECT))
        .with_state(st)
}

fn status(code: StatusCode, msg: &str) -> Response {
    (code, msg.to_string()).into_response()
}

fn claims(st: &CasState, h: &HeaderMap, write: bool) -> Result<CasClaims, Response> {
    let token = h
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| kloudlite_core::httpx::scheme(v, "Bearer"));
    let Some(c) = token.and_then(|t| st.jwt.verify_cas(t)) else {
        return Err(status(StatusCode::UNAUTHORIZED, "missing or invalid CAS token"));
    };
    if write && !c.write {
        return Err(status(StatusCode::FORBIDDEN, "token is read-only"));
    }
    Ok(c)
}

/// `put_xorb`/`put_shard` refuse with a message; one starting `storing` is our fault (500),
/// anything else is the client's (400).
fn refused(msg: String) -> Response {
    if msg.starts_with("storing") {
        tracing::error!(error = %msg, "cas.write.failed");
        return status(StatusCode::INTERNAL_SERVER_ERROR, "internal error");
    }
    tracing::warn!(error = %msg, "cas.refused");
    status(StatusCode::BAD_REQUEST, &msg)
}

fn internal(e: impl std::fmt::Display) -> Response {
    tracing::error!(error = %e, "cas.failed");
    status(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
}

async fn post_xorb(State(st): State<CasState>, Path(hash): Path<String>, h: HeaderMap, body: Bytes) -> Response {
    let c = match claims(&st, &h, true) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let Some(hash) = keys::parse_hash(&hash) else { return status(StatusCode::BAD_REQUEST, "bad xorb hash") };
    match xorbs::put_xorb(&*st.os, &c.owner, &hash, body).await {
        Ok(i) => Json(json!({"was_inserted": matches!(i, xorbs::Ingest::Inserted)})).into_response(),
        Err(m) => refused(m),
    }
}

async fn post_shard(State(st): State<CasState>, h: HeaderMap, body: Bytes) -> Response {
    let c = match claims(&st, &h, true) {
        Ok(c) => c,
        Err(r) => return r,
    };
    match shards::put_shard(&*st.os, &c.owner, &body).await {
        Ok(new) => Json(json!({"result": u8::from(new)})).into_response(),
        Err(m) => refused(m),
    }
}

fn fetch_url(st: &CasState, owner: &str) -> impl Fn(&str) -> String {
    let (base, secret, owner) = (st.base.clone(), st.jwt.secret().to_vec(), owner.to_string());
    move |xorb| sign::signed_url(&base, &secret, &format!("/v1/fetch/{owner}/{xorb}"), FETCH_TTL)
}

async fn get_recon(State(st): State<CasState>, Path(hash): Path<String>, h: HeaderMap) -> Response {
    let c = match claims(&st, &h, false) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let Some(hash) = keys::parse_hash(&hash) else { return status(StatusCode::NOT_FOUND, "no such file") };
    let Ok(range) = recon::parse_range(h.get(header::RANGE).and_then(|v| v.to_str().ok())) else {
        return status(StatusCode::RANGE_NOT_SATISFIABLE, "unsupported range");
    };
    let file = match shards::read_file(&*st.os, &c.owner, &hash).await {
        Ok(Some(f)) => f,
        Ok(None) => return status(StatusCode::NOT_FOUND, "no such file"),
        Err(e) => return internal(e),
    };
    match recon::reconstruct(&*st.os, &c.owner, &file, range, &fetch_url(&st, &c.owner)).await {
        Ok(Some(v)) => Json(v).into_response(),
        Ok(None) => status(StatusCode::RANGE_NOT_SATISFIABLE, "range starts past the end"),
        Err(e) => internal(e),
    }
}

/// hf-xet 1.7.0 sends the batch as a GET with `file_id` repeated, so the query is parsed raw
/// (axum's `Query` keeps only the last of a repeated key). Unknown ids are left out.
async fn get_batch(State(st): State<CasState>, RawQuery(q): RawQuery, h: HeaderMap) -> Response {
    let c = match claims(&st, &h, false) {
        Ok(c) => c,
        Err(r) => return r,
    };
    let url = fetch_url(&st, &c.owner);
    let (mut files, mut fetch) = (Map::new(), Map::new());
    for id in q.as_deref().unwrap_or("").split('&').filter_map(|kv| kv.strip_prefix("file_id=")) {
        let Some(hash) = keys::parse_hash(id) else { continue };
        let file = match shards::read_file(&*st.os, &c.owner, &hash).await {
            Ok(Some(f)) => f,
            Ok(None) => continue,
            Err(e) => return internal(e),
        };
        let v = match recon::reconstruct(&*st.os, &c.owner, &file, None, &url).await {
            Ok(Some(v)) => v,
            Ok(None) => continue,
            Err(e) => return internal(e),
        };
        files.insert(id.to_string(), v["terms"].clone());
        if let Value::Object(m) = &v["fetch_info"] {
            for (k, list) in m {
                fetch.entry(k.clone()).or_insert_with(|| list.clone());
            }
        }
    }
    Json(json!({"files": files, "fetch_info": fetch})).into_response()
}

async fn get_fetch(
    State(st): State<CasState>,
    Path((owner, xorb)): Path<(String, String)>,
    Query(q): Query<std::collections::HashMap<String, String>>,
    h: HeaderMap,
) -> Response {
    let path = format!("/v1/fetch/{owner}/{xorb}");
    let ok = match (q.get("exp").and_then(|e| e.parse::<u64>().ok()), q.get("sig")) {
        (Some(exp), Some(sig)) => sign::verify(st.jwt.secret(), &path, exp, sig, sign::now()),
        _ => false,
    };
    if !ok {
        return status(StatusCode::FORBIDDEN, "bad or expired signature");
    }
    let Some(hash) = keys::parse_hash(&xorb) else { return status(StatusCode::NOT_FOUND, "no such xorb") };
    let Ok(range) = recon::parse_range(h.get(header::RANGE).and_then(|v| v.to_str().ok())) else {
        return status(StatusCode::RANGE_NOT_SATISFIABLE, "unsupported range");
    };
    let key = keys::xorb(&owner, &hash);
    // Size first: an unsatisfiable start must be 416, and an end past the object is clamped.
    let size = match st.os.head(&key).await {
        Ok(m) => m.size,
        Err(slatedb::object_store::Error::NotFound { .. }) => return status(StatusCode::NOT_FOUND, "no such xorb"),
        Err(e) => return internal(e),
    };
    let range = match range {
        Some((s, _)) if s >= size => return status(StatusCode::RANGE_NOT_SATISFIABLE, "range starts past the end"),
        Some((s, e)) => Some((s, e.unwrap_or(u64::MAX).min(size - 1))),
        None => None,
    };
    let opts = GetOptions { range: range.map(|(s, e)| GetRange::Bounded(s..e + 1)), ..Default::default() };
    let got = match st.os.get_opts(&key, opts).await {
        Ok(g) => g,
        Err(slatedb::object_store::Error::NotFound { .. }) => return status(StatusCode::NOT_FOUND, "no such xorb"),
        Err(e) => return internal(e),
    };
    let served = got.range.clone();
    match got.bytes().await {
        Ok(b) if range.is_some() => (
            StatusCode::PARTIAL_CONTENT,
            [(header::CONTENT_RANGE, format!("bytes {}-{}/{size}", served.start, served.end - 1))],
            b,
        )
            .into_response(),
        Ok(b) => (StatusCode::OK, b).into_response(),
        Err(e) => internal(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::build;
    use axum::body::Body;
    use axum::http::Request;
    use slatedb::object_store::memory::InMemory;
    use tower::ServiceExt;

    fn setup() -> (Router, Arc<Jwt>) {
        let jwt = Arc::new(Jwt::new(&"k".repeat(32)).unwrap());
        let st = CasState { os: Arc::new(InMemory::new()), jwt: jwt.clone(), base: "http://cas.test".into() };
        (router(st), jwt)
    }

    fn req(method: &str, uri: &str, tok: &str, range: Option<&str>, body: Vec<u8>) -> Request<Body> {
        let mut r = Request::builder().method(method).uri(uri);
        if !tok.is_empty() {
            r = r.header("authorization", format!("Bearer {tok}"));
        }
        if let Some(v) = range {
            r = r.header("range", v);
        }
        r.body(Body::from(body)).unwrap()
    }

    async fn call(app: &Router, r: Request<Body>) -> (StatusCode, Vec<u8>) {
        let resp = app.clone().oneshot(r).await.unwrap();
        let s = resp.status();
        (s, axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap().to_vec())
    }

    fn json(b: &[u8]) -> serde_json::Value {
        serde_json::from_slice(b).unwrap()
    }

    async fn upload(app: &Router, tok: &str, b: &crate::testkit::Built) {
        let uri = format!("/v1/xorbs/default/{}", b.xorb_hash.hex());
        let (s, body) = call(app, req("POST", &uri, tok, None, b.xorb.clone())).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(json(&body)["was_inserted"], true);
        let (s, body) = call(app, req("POST", "/v1/shards", tok, None, b.shard.clone())).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(json(&body)["result"], 1);
    }

    #[tokio::test]
    async fn upload_reconstruct_fetch_round_trip() {
        let (app, jwt) = setup();
        let w = jwt.mint_cas("alice", "mdl/alice/bert", "main", true, 600).unwrap();
        let b = build(&[b"hello ", b"world"]);
        upload(&app, &w, &b).await;

        let uri = format!("/v1/reconstructions/{}", b.file_hash.hex());
        let (s, body) = call(&app, req("GET", &uri, &w, None, vec![])).await;
        assert_eq!(s, StatusCode::OK);
        let v = json(&body);
        assert_eq!(v["offset_into_first_range"], 0);
        let fi = &v["fetch_info"][b.xorb_hash.hex()][0];
        let url = fi["url"].as_str().unwrap().strip_prefix("http://cas.test").unwrap().to_string();
        let (a, z) = (fi["url_range"]["start"].as_u64().unwrap(), fi["url_range"]["end"].as_u64().unwrap());
        // The fetch URL is presigned: no bearer.
        let (s, bytes) = call(&app, req("GET", &url, "", Some(&format!("bytes={a}-{z}")), vec![])).await;
        assert_eq!(s, StatusCode::PARTIAL_CONTENT);
        assert_eq!(bytes, b.xorb[a as usize..=z as usize]);

        let (s, body) = call(&app, req("GET", &format!("/v1/reconstructions?file_id={}", b.file_hash.hex()), &w, None, vec![])).await;
        assert_eq!(s, StatusCode::OK);
        assert!(json(&body)["files"].get(b.file_hash.hex()).is_some());
    }

    #[tokio::test]
    async fn reconstruction_single_byte_and_past_end() {
        let (app, jwt) = setup();
        let w = jwt.mint_cas("alice", "mdl/alice/bert", "main", true, 600).unwrap();
        let b = build(&[b"hello ", b"world"]);
        upload(&app, &w, &b).await;
        let uri = format!("/v1/reconstructions/{}", b.file_hash.hex());
        let (s, body) = call(&app, req("GET", &uri, &w, Some("bytes=0-0"), vec![])).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(json(&body)["offset_into_first_range"], 0);
        let (s, body) = call(&app, req("GET", &uri, &w, Some("bytes=10-99"), vec![])).await;
        assert_eq!(s, StatusCode::OK);
        assert_eq!(json(&body)["offset_into_first_range"], 10);
        let (s, _) = call(&app, req("GET", &uri, &w, Some("bytes=11-"), vec![])).await;
        assert_eq!(s, StatusCode::RANGE_NOT_SATISFIABLE);
    }

    #[tokio::test]
    async fn tokens_gate_reads_and_writes() {
        let (app, jwt) = setup();
        let b = build(&[b"abc"]);
        let uri = format!("/v1/xorbs/default/{}", b.xorb_hash.hex());
        let (s, _) = call(&app, req("POST", &uri, "", None, b.xorb.clone())).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        let (s, _) = call(&app, req("POST", &uri, "not-a-token", None, b.xorb.clone())).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
        let r = jwt.mint_cas("alice", "mdl/alice/bert", "main", false, 600).unwrap();
        let (s, _) = call(&app, req("POST", &uri, &r, None, b.xorb.clone())).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        // A session JWT is not a CAS token.
        let session = jwt.mint("alice@x", "Alice", Some("alice")).unwrap();
        let (s, _) = call(&app, req("GET", &format!("/v1/reconstructions/{}", b.file_hash.hex()), &session, None, vec![])).await;
        assert_eq!(s, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn owners_do_not_share_files() {
        let (app, jwt) = setup();
        let a = jwt.mint_cas("alice", "mdl/alice/bert", "main", true, 600).unwrap();
        let m = jwt.mint_cas("mallory", "mdl/mallory/x", "main", false, 600).unwrap();
        let b = build(&[b"secret weights"]);
        upload(&app, &a, &b).await;
        let (s, _) = call(&app, req("GET", &format!("/v1/reconstructions/{}", b.file_hash.hex()), &m, None, vec![])).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn ranged_fetch_has_content_range_and_past_end_is_416() {
        let (app, jwt) = setup();
        let w = jwt.mint_cas("alice", "mdl/alice/bert", "main", true, 600).unwrap();
        let b = build(&[b"hello ", b"world"]);
        upload(&app, &w, &b).await;
        let n = b.xorb.len();
        let path = format!("/v1/fetch/alice/{}", b.xorb_hash.hex());
        let url = sign::signed_url("", jwt.secret(), &path, 600);
        let r = app.clone().oneshot(req("GET", &url, "", Some("bytes=2-999999"), vec![])).await.unwrap();
        assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(r.headers()["content-range"], format!("bytes 2-{}/{n}", n - 1).as_str());
        let (s, _) = call(&app, req("GET", &url, "", Some(&format!("bytes={n}-")), vec![])).await;
        assert_eq!(s, StatusCode::RANGE_NOT_SATISFIABLE);
    }

    #[tokio::test]
    async fn tampered_fetch_signature_is_403() {
        let (app, jwt) = setup();
        let w = jwt.mint_cas("alice", "mdl/alice/bert", "main", true, 600).unwrap();
        let b = build(&[b"abc"]);
        upload(&app, &w, &b).await;
        let good = format!("/v1/fetch/alice/{}", b.xorb_hash.hex());
        let (s, _) = call(&app, req("GET", &format!("{good}?exp=99999999999&sig=00"), "", None, vec![])).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
        // A valid signature for alice's path does not open bob's.
        let url = sign::signed_url("", jwt.secret(), &good, 600);
        let bob = url.replacen("/alice/", "/bob/", 1);
        let (s, _) = call(&app, req("GET", &bob, "", None, vec![])).await;
        assert_eq!(s, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn v2_and_chunk_routes_are_404() {
        let (app, jwt) = setup();
        let w = jwt.mint_cas("alice", "mdl/alice/bert", "main", true, 600).unwrap();
        let h = "0".repeat(64);
        for uri in [format!("/v2/reconstructions/{h}"), format!("/v1/chunks/default-merkledb/{h}")] {
            let (s, _) = call(&app, req("GET", &uri, &w, None, vec![])).await;
            assert_eq!(s, StatusCode::NOT_FOUND, "{uri}");
        }
        let (s, _) = call(&app, req("POST", "/v2/shards", &w, None, vec![1])).await;
        assert_eq!(s, StatusCode::NOT_FOUND);
    }
}
