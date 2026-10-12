//! The live list: every sha256 an LFS pointer reachable from any ref of a hub repo names, in all
//! history. The sweep (`kloudlite_xet::gc`) keeps exactly these and treats a missing list as
//! "keep everything", so a failed rebuild DELETES the list rather than leaving a stale one.
//!
//! ponytail: a full walk of every ref on every ref change, serialised process-wide. Fine for
//! model repos (few commits, few files); make it incremental (walk only new commits, full walk on
//! delete) when `hub.live.failed` or slow pushes show walks over a second.

use crate::auth::Access;
use crate::HubState;
use gix_hash::ObjectId;
use gix_object::FindExt;
use gix_odb::HeaderExt;
use kloudlite_storage::store::{Repo, Store};
use kloudlite_storage::HubKind;
use kloudlite_xet::gc::LiveList;
use kloudlite_xet::keys;
use object_store::ObjectStoreExt;
use std::collections::{BTreeSet, HashSet};

static WALK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn walk(odb: &gix_odb::Handle, tips: Vec<ObjectId>) -> Result<BTreeSet<String>, String> {
    let mut shas = BTreeSet::new();
    let (mut seen_c, mut seen_t, mut seen_b) = (HashSet::new(), HashSet::new(), HashSet::new());
    let mut commits = tips;
    let mut trees = Vec::new();
    let mut buf = Vec::new();
    while let Some(c) = commits.pop() {
        if !seen_c.insert(c) {
            continue;
        }
        let data = odb.find(&c, &mut buf).map_err(|e| e.to_string())?;
        let commit = gix_object::CommitRef::from_bytes(data.data, gix_hash::Kind::Sha1).map_err(|e| format!("{c}: {e}"))?;
        trees.push(commit.tree());
        commits.extend(commit.parents());
    }
    while let Some(t) = trees.pop() {
        if !seen_t.insert(t) {
            continue;
        }
        let tree = odb.find_tree(&t, &mut buf).map_err(|e| e.to_string())?;
        for e in tree.entries.iter() {
            let oid = e.oid.to_owned();
            if e.mode.is_tree() {
                trees.push(oid);
            } else if e.mode.is_blob() && seen_b.insert(oid) {
                if odb.header(oid).map_err(|e| e.to_string())?.size() > crate::pointer::POINTER_MAX as u64 {
                    continue;
                }
                let mut b = Vec::new();
                let blob = odb.find_blob(&oid, &mut b).map_err(|e| e.to_string())?;
                if let Some(p) = crate::pointer::parse(blob.data) {
                    shas.insert(p.sha256);
                }
            }
        }
    }
    Ok(shas)
}

pub async fn rebuild(store: &Store, repo: &Repo, owner: &str, kind: HubKind, name: &str) -> Result<(), String> {
    let _g = WALK.lock().await;
    let key = keys::live(owner, kind.ns(), name);
    let result = async {
        let tips: Vec<ObjectId> = store.list_refs(repo).await.map_err(|e| e.to_string())?.into_iter().map(|(_, id)| id).collect();
        let odb = repo.odb().map_err(|e| e.to_string())?;
        let shas = tokio::task::spawn_blocking(move || walk(&odb, tips)).await.map_err(|e| e.to_string())??;
        let body = serde_json::to_vec(&LiveList { shas }).map_err(|e| e.to_string())?;
        store.os.put(&key, body.into()).await.map_err(|e| e.to_string())?;
        Ok::<(), String>(())
    }
    .await;
    if result.is_err() {
        let _ = store.os.delete(&key).await;
    }
    result
}

pub async fn after_update(st: &HubState, a: &Access) {
    if let Err(e) = rebuild(&st.app.store, &a.repo, &a.owner, a.kind, &a.name).await {
        tracing::warn!(owner = %a.owner, name = %a.name, error = %e, "hub.live.failed");
    }
}
