//! Read API: repo info, tree listing, paths-info, refs. Everything reads one commit's tree on a
//! blocking thread (gix is sync), then decorates LFS pointers with their sha256 and, when the
//! bytes live in Xet, the Xet file hash huggingface_hub needs to download through the CAS.
use crate::auth::{open, Access};
use crate::pointer::{self, Pointer, POINTER_MAX};
use crate::rev::{resolve, revision_not_found, safe_path};
use crate::{hub_err, internal, HubState};
use axum::extract::{Path, Query, RawForm, State};
use axum::http::{header, HeaderMap, StatusCode};
use object_store::ObjectStoreExt;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use gix_hash::ObjectId;
use kloudlite_core::httpx::Trusted;
use kloudlite_storage::HubKind;
use serde::Deserialize;
use serde_json::{json, Value};

pub const PAGE: usize = 1000;

/// path, oid, size, blob bytes when small enough to be an LFS pointer.
type Raw = (String, String, u64, Option<Vec<u8>>);

pub struct FileInfo {
    pub path: String,
    pub oid: String,
    pub size: u64,
    pub lfs: Option<(Pointer, u64)>,
    pub xet_hash: Option<String>,
}

impl FileInfo {
    fn entry(&self) -> Value {
        let mut v = json!({ "type": "file", "oid": self.oid, "size": self.size, "path": self.path });
        if let Some((p, psize)) = &self.lfs {
            v["lfs"] = json!({ "oid": p.sha256, "size": p.size, "pointerSize": psize });
        }
        if let Some(x) = &self.xet_hash {
            v["xetHash"] = json!(x);
        }
        v
    }
}

pub async fn xet_hash_of(st: &HubState, owner: &str, sha256: &str) -> Option<String> {
    let got = st.cas.os.get(&kloudlite_xet::keys::sha(owner, sha256)).await.ok()?;
    String::from_utf8(got.bytes().await.ok()?.to_vec()).ok()
}

/// Every file under `prefix` at `commit` (one level when not `recursive`), sorted by path, with
/// LFS pointers resolved. Pointer blobs are read only when small enough to be one.
pub async fn file_infos(st: &HubState, a: &Access, commit: ObjectId, prefix: &str, recursive: bool) -> Result<Vec<FileInfo>, Response> {
    let repo = a.repo.clone();
    let prefix = prefix.to_string();
    let raw = tokio::task::spawn_blocking(move || -> kloudlite_core::Result<Vec<Raw>> {
        let odb = repo.odb().map_err(|e| kloudlite_core::Error::from(e.to_string()))?;
        let entries = if recursive {
            kloudlite_vcs::browse::files_at(&odb, commit, &prefix, usize::MAX, true)?
        } else {
            kloudlite_vcs::browse::tree_at(&odb, commit, &prefix)?.into_iter().filter(|e| e.kind == "blob").collect()
        };
        let mut out = Vec::with_capacity(entries.len());
        for e in entries {
            let path = if recursive || prefix.is_empty() { e.name.clone() } else { format!("{prefix}/{}", e.name) };
            let size = e.size.unwrap_or(0);
            let body = if size as usize <= POINTER_MAX {
                Some(kloudlite_vcs::browse::blob_at(&odb, commit, &path, POINTER_MAX)?.bytes)
            } else {
                None
            };
            out.push((path, e.oid, size, body));
        }
        Ok(out)
    })
    .await
    .map_err(internal)?;
    let raw = match raw {
        Ok(r) => r,
        Err(e) if kloudlite_vcs::browse::is_not_found(&e) => return Err(entry_not_found()),
        Err(e) => return Err(internal(e)),
    };
    let mut out = Vec::with_capacity(raw.len());
    for (path, oid, size, body) in raw {
        let lfs = body.as_deref().and_then(pointer::parse).map(|p| (p, size));
        let xet_hash = match &lfs {
            Some((p, _)) => xet_hash_of(st, &a.owner, &p.sha256).await,
            None => None,
        };
        let size = lfs.as_ref().map(|(p, _)| p.size).unwrap_or(size);
        out.push(FileInfo { path, oid, size, lfs, xet_hash });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

pub fn entry_not_found() -> Response {
    hub_err(StatusCode::NOT_FOUND, Some("EntryNotFound"), "Entry not found")
}

/// The commit a request names, or the response that refuses it. `None` for an empty repo's
/// default branch so `info` can answer it.
async fn commit_of(st: &HubState, a: &Access, rev: &str) -> Result<Option<ObjectId>, Response> {
    match resolve(&st.app.store, &a.repo, rev).await {
        Ok(Some(o)) => Ok(Some(o)),
        Ok(None) if rev == "main" && st.app.store.list_refs(&a.repo).await.map_err(internal)?.is_empty() => Ok(None),
        Ok(None) => Err(revision_not_found(rev)),
        Err(e) => Err(internal(e)),
    }
}

pub async fn info(
    State(st): State<HubState>,
    Extension(kind): Extension<HubKind>,
    trusted: Option<Extension<Trusted>>,
    headers: HeaderMap,
    Path((owner, name)): Path<(String, String)>,
) -> Response {
    info_at(st, kind, trusted, headers, owner, name, "main".into()).await
}

pub async fn info_rev(
    State(st): State<HubState>,
    Extension(kind): Extension<HubKind>,
    trusted: Option<Extension<Trusted>>,
    headers: HeaderMap,
    Path((owner, name, rev)): Path<(String, String, String)>,
) -> Response {
    info_at(st, kind, trusted, headers, owner, name, rev).await
}

async fn info_at(st: HubState, kind: HubKind, trusted: Option<Extension<Trusted>>, headers: HeaderMap, owner: String, name: String, rev: String) -> Response {
    let a = match open(&st, kind, trusted.as_deref(), &headers, &owner, &name, false).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let private = match st.app.store.hub_public(kind, &owner, &name).await {
        Ok(p) => !p,
        Err(e) => return internal(e),
    };
    let id = format!("{owner}/{name}");
    let mut v = json!({ "_id": format!("{}/{id}", kind.ns()), "id": id, "private": private, "sha": null, "siblings": [] });
    if kind == HubKind::Model {
        v["modelId"] = json!(id);
    }
    let commit = match commit_of(&st, &a, &rev).await {
        Ok(Some(c)) => c,
        Ok(None) => return Json(v).into_response(),
        Err(r) => return r,
    };
    let repo = a.repo.clone();
    let when = tokio::task::spawn_blocking(move || {
        let odb = repo.odb().ok()?;
        kloudlite_vcs::browse::commit(&odb, commit).ok().map(|(c, _)| c.time)
    })
    .await
    .ok()
    .flatten();
    let files = match file_infos(&st, &a, commit, "", true).await {
        Ok(f) => f,
        Err(r) => return r,
    };
    v["sha"] = json!(commit.to_string());
    if let Some(t) = when {
        v["lastModified"] = json!(rfc3339(t));
    }
    v["siblings"] = files
        .iter()
        .map(|f| {
            let mut s = json!({ "rfilename": f.path, "size": f.size, "blobId": f.oid });
            if let Some((p, psize)) = &f.lfs {
                s["lfs"] = json!({ "size": p.size, "sha256": p.sha256, "pointerSize": psize });
            }
            s
        })
        .collect();
    Json(v).into_response()
}

/// Seconds since the epoch as `YYYY-MM-DDTHH:MM:SS.000Z`.
fn rfc3339(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Civil-from-days (Howard Hinnant), valid for every date git can carry.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[derive(Deserialize)]
pub struct TreeQuery {
    #[serde(default)]
    pub recursive: bool,
    pub cursor: Option<String>,
}

pub async fn tree_root(
    st: State<HubState>,
    kind: Extension<HubKind>,
    trusted: Option<Extension<Trusted>>,
    headers: HeaderMap,
    Path((owner, name, rev)): Path<(String, String, String)>,
    q: Query<TreeQuery>,
) -> Response {
    tree_at(st, kind, trusted, headers, owner, name, rev, String::new(), q).await
}

pub async fn tree_path(
    st: State<HubState>,
    kind: Extension<HubKind>,
    trusted: Option<Extension<Trusted>>,
    headers: HeaderMap,
    Path((owner, name, rev, path)): Path<(String, String, String, String)>,
    q: Query<TreeQuery>,
) -> Response {
    tree_at(st, kind, trusted, headers, owner, name, rev, path, q).await
}

#[allow(clippy::too_many_arguments)]
async fn tree_at(
    State(st): State<HubState>,
    Extension(kind): Extension<HubKind>,
    trusted: Option<Extension<Trusted>>,
    headers: HeaderMap,
    owner: String,
    name: String,
    rev: String,
    path: String,
    Query(q): Query<TreeQuery>,
) -> Response {
    let path = path.trim_end_matches('/').to_string();
    if !path.is_empty() && !safe_path(&path) {
        return entry_not_found();
    }
    let a = match open(&st, kind, trusted.as_deref(), &headers, &owner, &name, false).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let commit = match commit_of(&st, &a, &rev).await {
        Ok(Some(c)) => c,
        Ok(None) => return Json(json!([])).into_response(),
        Err(r) => return r,
    };
    let mut entries: Vec<(String, Value)> = match file_infos(&st, &a, commit, &path, q.recursive).await {
        Ok(f) => f.iter().map(|f| (f.path.clone(), f.entry())).collect(),
        Err(r) => return r,
    };
    if !q.recursive {
        let repo = a.repo.clone();
        let p = path.clone();
        let dirs = tokio::task::spawn_blocking(move || {
            let odb = repo.odb().map_err(|e| e.to_string())?;
            kloudlite_vcs::browse::tree_at(&odb, commit, &p).map_err(|e| e.to_string())
        })
        .await;
        match dirs {
            Ok(Ok(list)) => {
                for e in list.into_iter().filter(|e| e.kind == "tree") {
                    let full = if path.is_empty() { e.name } else { format!("{path}/{}", e.name) };
                    entries.push((full.clone(), json!({ "type": "directory", "oid": e.oid, "path": full })));
                }
            }
            Ok(Err(e)) => return internal(e),
            Err(e) => return internal(e),
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
    }
    if let Some(c) = &q.cursor {
        entries.retain(|(p, _)| p > c);
    }
    let more = entries.len() > PAGE;
    entries.truncate(PAGE);
    let mut r = Json(Value::Array(entries.iter().map(|(_, v)| v.clone()).collect())).into_response();
    if more {
        if let Some((last, _)) = entries.last() {
            let dir = if path.is_empty() { String::new() } else { format!("/{path}") };
            let link = format!(
                "<{}/api/{}/{owner}/{name}/tree/{rev}{dir}?recursive={}&cursor={}>; rel=\"next\"",
                st.base,
                kind.api_plural(),
                q.recursive,
                urlencode(last)
            );
            if let Ok(v) = link.parse() {
                r.headers_mut().insert(header::LINK, v);
            }
        }
    }
    r
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

pub async fn paths_info(
    State(st): State<HubState>,
    Extension(kind): Extension<HubKind>,
    trusted: Option<Extension<Trusted>>,
    headers: HeaderMap,
    Path((owner, name, rev)): Path<(String, String, String)>,
    RawForm(body): RawForm,
) -> Response {
    let wanted: Vec<String> = form_values(&body, "paths");
    let a = match open(&st, kind, trusted.as_deref(), &headers, &owner, &name, false).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let commit = match commit_of(&st, &a, &rev).await {
        Ok(Some(c)) => c,
        Ok(None) => return Json(json!([])).into_response(),
        Err(r) => return r,
    };
    let files = match file_infos(&st, &a, commit, "", true).await {
        Ok(f) => f,
        Err(r) => return r,
    };
    let mut out = Vec::new();
    for w in wanted.iter().map(|w| w.trim_end_matches('/')) {
        if !safe_path(w) {
            continue;
        }
        if let Some(f) = files.iter().find(|f| f.path == w) {
            out.push(f.entry());
        } else if files.iter().any(|f| f.path.starts_with(&format!("{w}/"))) {
            // ponytail: directory oid is not looked up for paths-info; huggingface_hub reads only
            // `type` and `path` of a folder here. Look it up with `tree_at` on the parent if a
            // client needs it.
            out.push(json!({ "type": "directory", "oid": "", "path": w }));
        }
    }
    Json(Value::Array(out)).into_response()
}

/// Every value of `key` in an `application/x-www-form-urlencoded` body, percent-decoded.
pub fn form_values(body: &[u8], key: &str) -> Vec<String> {
    let s = String::from_utf8_lossy(body);
    s.split('&')
        .filter_map(|kv| kv.split_once('='))
        .filter(|(k, _)| *k == key)
        .map(|(_, v)| percent_decode(&v.replace('+', " ")))
        .collect()
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub async fn refs(
    State(st): State<HubState>,
    Extension(kind): Extension<HubKind>,
    trusted: Option<Extension<Trusted>>,
    headers: HeaderMap,
    Path((owner, name)): Path<(String, String)>,
) -> Response {
    let a = match open(&st, kind, trusted.as_deref(), &headers, &owner, &name, false).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let all = match st.app.store.list_refs(&a.repo).await {
        Ok(r) => r,
        Err(e) => return internal(e),
    };
    let pick = |prefix: &str| -> Vec<Value> {
        all.iter()
            .filter_map(|(r, o)| r.strip_prefix(prefix).map(|n| json!({ "name": n, "ref": r, "targetCommit": o.to_string() })))
            .collect()
    };
    Json(json!({ "branches": pick("refs/heads/"), "converts": [], "tags": pick("refs/tags/") })).into_response()
}
