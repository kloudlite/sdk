//! Writes: `POST /api/{plural}/{owner}/{name}/preupload/{rev}` and
//! `POST /api/{plural}/{owner}/{name}/commit/{rev}` (huggingface_hub's `create_commit`).
//!
//! Preupload tells the client which files go inline and which go through LFS/Xet first. The
//! commit is NDJSON: one header, then file, lfsFile, deletedFile and deletedFolder lines. Inline
//! files become blobs; an lfsFile becomes a pointer blob, refused 422 unless its bytes already
//! sit in this owner's CAS (`xet/{owner}/sha/`) or LFS store (`lfs/{owner}/sha256/`), and its
//! path gains a `.gitattributes` line. The ref moves through `update_refs` with the head we read
//! as `old`, so two racing commits cannot both land: the loser gets 412, like a stale
//! `parentCommit`.
//!
//! ponytail: the whole NDJSON body is buffered (cap `COMMIT_MAX`); inline files are under 10 MiB
//! by the preupload rule, so a streaming parser waits for a client that inlines big files.

use crate::auth::open;
use crate::pointer;
use crate::resolve::exists;
use crate::rev::{revision_not_found, safe_path};
use crate::{hub_err, internal, HubState};
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use base64::Engine;
use kloudlite_core::httpx::Trusted;
use kloudlite_gitbase::objects::{apply_changes, write_commit, Change, NewCommit, Staging};
use kloudlite_gitbase::refs::UpdateRefsExt;
use kloudlite_storage::refmeta::RefUpdate;
use kloudlite_storage::HubKind;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};

pub const LFS_THRESHOLD: u64 = 10_485_760;
pub const COMMIT_MAX: usize = 512 * 1024 * 1024;
const B64: base64::engine::GeneralPurpose = base64::engine::general_purpose::STANDARD;

#[derive(Deserialize)]
pub struct PreBody {
    files: Vec<PreFile>,
}

#[derive(Deserialize)]
struct PreFile {
    path: String,
    size: u64,
    #[serde(default)]
    sample: String,
}

pub async fn preupload(
    State(st): State<HubState>,
    Extension(kind): Extension<HubKind>,
    trusted: Option<Extension<Trusted>>,
    headers: HeaderMap,
    Path((owner, name, rev)): Path<(String, String, String)>,
    Json(body): Json<PreBody>,
) -> Response {
    let a = match open(&st, kind, trusted.as_deref(), &headers, &owner, &name, true).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let head = match st.app.store.get_ref(&a.repo, &format!("refs/heads/{rev}")).await {
        Ok(h) => h.map(|o| o.to_string()),
        Err(e) => return internal(e),
    };
    let mut files = Vec::with_capacity(body.files.len());
    for f in body.files {
        if !safe_path(&f.path) {
            return hub_err(StatusCode::BAD_REQUEST, None, &format!("invalid path: {}", f.path));
        }
        let binary = B64.decode(&f.sample).map(|b| b.contains(&0)).unwrap_or(true);
        let mode = if f.size >= LFS_THRESHOLD || binary { "lfs" } else { "regular" };
        files.push(json!({ "path": f.path, "uploadMode": mode, "shouldIgnore": false }));
    }
    Json(json!({ "files": files, "commitOid": head })).into_response()
}

/// One parsed commit body.
#[derive(Default)]
pub struct Ops {
    pub summary: Option<String>,
    pub description: String,
    pub parent: Option<String>,
    pub files: Vec<(String, Vec<u8>)>,
    /// (path, sha256 hex, size)
    pub lfs: Vec<(String, String, u64)>,
    pub deleted: Vec<String>,
    pub folders: Vec<String>,
}

impl Ops {
    pub fn message(&self) -> String {
        let s = self.summary.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| "Commit via API".into());
        if self.description.is_empty() { s } else { format!("{s}\n\n{}", self.description) }
    }
}

#[derive(Deserialize)]
struct Line {
    key: String,
    value: Value,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Header {
    summary: Option<String>,
    #[serde(default)]
    description: String,
    parent_commit: Option<String>,
}

#[derive(Deserialize)]
struct FileOp {
    path: String,
    content: String,
    #[serde(default)]
    encoding: String,
}

#[derive(Deserialize)]
struct LfsOp {
    path: String,
    #[serde(default)]
    algo: String,
    oid: String,
    size: u64,
}

#[derive(Deserialize)]
struct PathOp {
    path: String,
}

fn checked(p: String) -> Result<String, String> {
    if safe_path(&p) { Ok(p) } else { Err(format!("invalid path: {p}")) }
}

pub fn parse_ops(body: &[u8]) -> Result<Ops, String> {
    let text = std::str::from_utf8(body).map_err(|_| "commit body is not UTF-8".to_string())?;
    let mut o = Ops::default();
    for (i, raw) in text.lines().enumerate() {
        if raw.trim().is_empty() {
            continue;
        }
        let bad = |e: serde_json::Error| format!("line {}: {e}", i + 1);
        let l: Line = serde_json::from_str(raw).map_err(bad)?;
        match l.key.as_str() {
            "header" => {
                let h: Header = serde_json::from_value(l.value).map_err(bad)?;
                o.summary = h.summary;
                o.description = h.description;
                o.parent = h.parent_commit;
            }
            "file" => {
                let f: FileOp = serde_json::from_value(l.value).map_err(bad)?;
                if f.encoding != "base64" {
                    return Err(format!("line {}: encoding must be base64", i + 1));
                }
                let bytes = B64.decode(&f.content).map_err(|e| format!("line {}: {e}", i + 1))?;
                o.files.push((checked(f.path)?, bytes));
            }
            "lfsFile" => {
                let f: LfsOp = serde_json::from_value(l.value).map_err(bad)?;
                let hex_ok = f.oid.len() == 64 && f.oid.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
                if f.algo != "sha256" || !hex_ok {
                    return Err(format!("line {}: lfsFile needs algo sha256 and a 64-hex oid", i + 1));
                }
                o.lfs.push((checked(f.path)?, f.oid, f.size));
            }
            "deletedFile" => {
                let f: PathOp = serde_json::from_value(l.value).map_err(bad)?;
                o.deleted.push(checked(f.path)?);
            }
            "deletedFolder" => {
                let f: PathOp = serde_json::from_value(l.value).map_err(bad)?;
                o.folders.push(checked(f.path.trim_end_matches('/').to_string())?);
            }
            other => return Err(format!("line {}: unknown key {other}", i + 1)),
        }
    }
    Ok(o)
}

/// The `.gitattributes` line git-lfs itself writes for one path.
pub fn lfs_line(path: &str) -> String {
    let mut s = String::with_capacity(path.len());
    for c in path.chars() {
        match c {
            ' ' => s.push_str("[[:space:]]"),
            '*' | '?' | '[' | '\\' | '#' | '!' => {
                s.push('\\');
                s.push(c);
            }
            c => s.push(c),
        }
    }
    format!("{s} filter=lfs diff=lfs merge=lfs -text")
}

/// `base` with a line for every path in `paths` it does not already hold.
pub fn with_lfs_lines(base: &str, paths: &[String]) -> String {
    let mut out = base.to_string();
    for p in paths {
        let l = lfs_line(p);
        if out.lines().any(|x| x.trim() == l) {
            continue;
        }
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out.push_str(&l);
        out.push('\n');
    }
    out
}

/// Whether this owner holds the LFS object: Xet CAS key first, then the plain LFS store.
pub(crate) async fn uploaded(st: &HubState, owner: &str, oid: &str) -> Result<bool, Response> {
    for key in [kloudlite_xet::keys::sha(owner, oid), kloudlite_xet::keys::lfs(owner, oid)] {
        if exists(&*st.cas.os, &key).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn stale() -> Response {
    hub_err(StatusCode::PRECONDITION_FAILED, None, "A commit has happened since. Please refresh and try again.")
}

fn tree_of(odb: &gix_odb::Handle, commit: gix_hash::ObjectId) -> Option<gix_hash::ObjectId> {
    use gix_object::Find;
    let mut buf = Vec::new();
    let c = odb.try_find(&commit, &mut buf).ok()??;
    gix_object::CommitRef::from_bytes(c.data, gix_hash::Kind::Sha1).ok().map(|c| c.tree())
}

#[allow(clippy::too_many_arguments)]
pub async fn commit(
    State(st): State<HubState>,
    Extension(kind): Extension<HubKind>,
    trusted: Option<Extension<Trusted>>,
    headers: HeaderMap,
    Path((owner, name, rev)): Path<(String, String, String)>,
    Query(q): Query<HashMap<String, String>>,
    body: Bytes,
) -> Response {
    if q.get("create_pr").is_some_and(|v| v == "1" || v == "true") {
        return hub_err(StatusCode::NOT_FOUND, None, "pull requests are not served");
    }
    let a = match open(&st, kind, trusted.as_deref(), &headers, &owner, &name, true).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    let ops = match parse_ops(&body) {
        Ok(o) => o,
        Err(m) => return hub_err(StatusCode::BAD_REQUEST, None, &m),
    };
    let store = &st.app.store;
    let branch = format!("refs/heads/{rev}");
    let head = match store.get_ref(&a.repo, &branch).await {
        Ok(h) => h,
        Err(e) => return internal(e),
    };
    if head.is_none() {
        match store.list_refs(&a.repo).await {
            Ok(refs) if refs.is_empty() && rev == "main" => {}
            Ok(_) => return revision_not_found(&rev),
            Err(e) => return internal(e),
        }
    }
    if let Some(p) = &ops.parent {
        if head.map(|h| h.to_string()).as_deref() != Some(p.as_str()) {
            return stale();
        }
    }

    let mut changes: BTreeMap<String, Change> = BTreeMap::new();
    for (path, content) in &ops.files {
        changes.insert(path.clone(), Change::Upsert { content: content.clone(), executable: None });
    }
    let mut lfs_paths = Vec::new();
    for (path, oid, size) in &ops.lfs {
        match uploaded(&st, &a.owner, oid).await {
            Ok(true) => {}
            Ok(false) => {
                return hub_err(StatusCode::UNPROCESSABLE_ENTITY, None, &format!("lfsFile {path}: no uploaded object with sha256 {oid}"));
            }
            Err(r) => return r,
        }
        changes.insert(path.clone(), Change::Upsert { content: pointer::text(oid, *size).into_bytes(), executable: None });
        lfs_paths.push(path.clone());
    }
    for path in &ops.deleted {
        changes.insert(path.clone(), Change::Delete);
    }

    // Reads of the parent tree: folder expansion and the existing `.gitattributes`.
    let repo = a.repo.clone();
    let folders = ops.folders.clone();
    let need_attrs = !lfs_paths.is_empty() && !changes.contains_key(".gitattributes");
    let read = tokio::task::spawn_blocking(move || -> kloudlite_core::Result<(Vec<String>, String)> {
        let Some(h) = head else { return Ok((Vec::new(), String::new())) };
        let odb = repo.odb().map_err(|e| kloudlite_core::Error::from(e.to_string()))?;
        let mut under = Vec::new();
        for f in &folders {
            match kloudlite_vcs::browse::files_at(&odb, h, f, usize::MAX, false) {
                Ok(es) => under.extend(es.into_iter().map(|e| e.name)),
                Err(e) if kloudlite_vcs::browse::is_not_found(&e) => {}
                Err(e) => return Err(e),
            }
        }
        let attrs = if need_attrs {
            match kloudlite_vcs::browse::blob_at(&odb, h, ".gitattributes", usize::MAX) {
                Ok(b) => String::from_utf8_lossy(&b.bytes).into_owned(),
                Err(e) if kloudlite_vcs::browse::is_not_found(&e) => String::new(),
                Err(e) => return Err(e),
            }
        } else {
            String::new()
        };
        Ok((under, attrs))
    })
    .await;
    let (under, parent_attrs) = match read {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => return internal(e),
        Err(e) => return internal(e),
    };
    for path in under {
        changes.entry(path).or_insert(Change::Delete);
    }
    if !lfs_paths.is_empty() {
        let base = match changes.get(".gitattributes") {
            Some(Change::Upsert { content, .. }) => String::from_utf8_lossy(content).into_owned(),
            _ => parent_attrs,
        };
        let next = with_lfs_lines(&base, &lfs_paths);
        changes.insert(".gitattributes".into(), Change::Upsert { content: next.into_bytes(), executable: None });
    }
    if changes.is_empty() {
        return hub_err(StatusCode::BAD_REQUEST, None, "nothing to commit");
    }

    let repo = a.repo.clone();
    let built = tokio::task::spawn_blocking(move || -> Result<(gix_hash::ObjectId, Staging), String> {
        let odb = repo.odb().map_err(|e| e.to_string())?;
        let mut staging = Staging::default();
        let tree = apply_changes(&odb, head.and_then(|h| tree_of(&odb, h)), changes, &mut staging).map_err(|e| e.to_string())?;
        Ok((tree, staging))
    })
    .await;
    let (tree, staging) = match built {
        Ok(Ok(t)) => t,
        // ponytail: every apply failure is the caller's (deleting a missing path, a file where a
        // folder is); an odb read failure would also land here as 400. Split if one is seen.
        Ok(Err(m)) => return hub_err(StatusCode::BAD_REQUEST, None, &m),
        Err(e) => return internal(e),
    };
    if let Err(e) = staging.write(store, &a.repo).await {
        return internal(e);
    }
    let caller = a.caller.clone().unwrap_or_default();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let new = match write_commit(store, &a.repo, NewCommit {
        tree,
        parents: head.into_iter().collect(),
        message: ops.message(),
        author_name: caller.clone(),
        author_email: format!("{caller}@users.noreply.kloudlite"),
        time: now,
    })
    .await
    {
        Ok(c) => c,
        Err(e) => return internal(e),
    };
    match store.update_refs(&a.repo, &[RefUpdate { name: branch, old: head, new: Some(new) }]).await {
        Ok(v) if v.iter().all(|x| x.is_none()) => {}
        Ok(_) => return stale(),
        Err(e) => return internal(e),
    }
    crate::live::after_update(&st, &a).await;
    let url = format!("{}/{}{owner}/{name}/commit/{new}", st.base, kind.url_prefix());
    Json(json!({ "commitUrl": url, "commitOid": new.to_string(), "pullRequestUrl": null })).into_response()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn lfs_lines_escape_and_are_idempotent() {
        assert_eq!(lfs_line("model.bin"), "model.bin filter=lfs diff=lfs merge=lfs -text");
        assert_eq!(lfs_line("my model/*.bin"), "my[[:space:]]model/\\*.bin filter=lfs diff=lfs merge=lfs -text");
        let once = with_lfs_lines("*.h5 filter=lfs diff=lfs merge=lfs -text", &["a.bin".into()]);
        assert_eq!(once, "*.h5 filter=lfs diff=lfs merge=lfs -text\na.bin filter=lfs diff=lfs merge=lfs -text\n");
        assert_eq!(with_lfs_lines(&once, &["a.bin".into()]), once);
    }

    #[test]
    fn parse_ops_reads_every_key() {
        let body = concat!(
            r#"{"key":"header","value":{"summary":"s","description":"d","parentCommit":"abc"}}"#, "\n",
            r#"{"key":"file","value":{"path":"a.txt","content":"aGk=","encoding":"base64"}}"#, "\n",
            "\n",
            r#"{"key":"lfsFile","value":{"path":"m.bin","algo":"sha256","oid":"2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824","size":5}}"#, "\n",
            r#"{"key":"deletedFile","value":{"path":"old.txt"}}"#, "\n",
            r#"{"key":"deletedFolder","value":{"path":"logs/"}}"#,
        );
        let o = parse_ops(body.as_bytes()).unwrap();
        assert_eq!(o.message(), "s\n\nd");
        assert_eq!(o.parent.as_deref(), Some("abc"));
        assert_eq!(o.files, vec![("a.txt".to_string(), b"hi".to_vec())]);
        assert_eq!(o.lfs[0].0, "m.bin");
        assert_eq!(o.lfs[0].2, 5);
        assert_eq!(o.deleted, vec!["old.txt".to_string()]);
        assert_eq!(o.folders, vec!["logs".to_string()]);
    }

    #[test]
    fn parse_ops_refuses_bad_lines() {
        for bad in [
            r#"{"key":"file","value":{"path":"../x","content":"aGk=","encoding":"base64"}}"#,
            r#"{"key":"file","value":{"path":".git/config","content":"aGk=","encoding":"base64"}}"#,
            r#"{"key":"file","value":{"path":"a","content":"hi","encoding":"utf-8"}}"#,
            r#"{"key":"lfsFile","value":{"path":"a","algo":"sha256","oid":"XYZ","size":1}}"#,
            r#"{"key":"rename","value":{}}"#,
            "not json",
        ] {
            assert!(parse_ops(bad.as_bytes()).is_err(), "{bad}");
        }
        assert_eq!(parse_ops(b"").unwrap().message(), "Commit via API");
    }
}
