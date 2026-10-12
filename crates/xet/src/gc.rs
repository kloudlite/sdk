//! The hub sweep: the only code that deletes hub bytes. Shaped like the registry sweep
//! (`crates/registry/src/gc.rs`): read every reference first, honour a grace window, and abort the
//! owner on any doubt. Keep-biased: a wrong delete loses a model; a wrong keep costs storage.
//!
//! Order per owner:
//! 1. Every repo under `repo/{mdl,ds}/{owner}/` must have a live list, or the owner is skipped.
//! 2. `sha/` and `lfs/` entries no live list names and older than the grace are candidates.
//! 3. Re-read the lists; any change since step 1 aborts (a push landed mid-sweep).
//! 4. Delete `sha/` (then its `files/` record) and `lfs/` candidates; stale `lfs/tmp/` uploads.
//! 5. Xorbs no remaining `files/` record names and older than the grace go, xorb before `.idx`.

use crate::keys;
use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use slatedb::object_store::path::Path;
use slatedb::object_store::{ObjectMeta, ObjectStore, ObjectStoreExt};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

#[derive(Serialize, Deserialize, Default, Debug, Clone, PartialEq)]
pub struct LiveList {
    pub shas: BTreeSet<String>,
}

/// A staging upload younger than this may still be streaming, whatever grace the caller passes.
const TMP_GRACE: Duration = Duration::from_secs(24 * 3600);

async fn list(os: &dyn ObjectStore, p: &Path) -> Result<Vec<ObjectMeta>, String> {
    os.list(Some(p)).try_collect().await.map_err(|e| e.to_string())
}

fn leaf(m: &ObjectMeta) -> String {
    m.location.filename().unwrap_or_default().to_string()
}

/// Union of every live list plus a fingerprint of the set (location + e_tag) for step 3.
async fn read_live(os: &dyn ObjectStore, owner: &str) -> Result<(BTreeSet<String>, BTreeMap<String, Option<String>>), String> {
    for tag in ["mdl", "ds"] {
        let repos = os.list_with_delimiter(Some(&Path::from(format!("repo/{tag}/{owner}")))).await.map_err(|e| e.to_string())?;
        for p in repos.common_prefixes {
            let name = p.filename().unwrap_or_default().to_string();
            match os.head(&keys::live(owner, tag, &name)).await {
                Ok(_) => {}
                Err(slatedb::object_store::Error::NotFound { .. }) => return Err(format!("{tag}/{owner}/{name} has no live list")),
                Err(e) => return Err(e.to_string()),
            }
        }
    }
    let mut shas = BTreeSet::new();
    let mut print = BTreeMap::new();
    for m in list(os, &keys::live_prefix(owner)).await? {
        let body = os.get(&m.location).await.map_err(|e| e.to_string())?.bytes().await.map_err(|e| e.to_string())?;
        let l: LiveList = serde_json::from_slice(&body).map_err(|e| format!("{}: {e}", m.location))?;
        shas.extend(l.shas);
        print.insert(m.location.to_string(), m.e_tag.clone());
    }
    Ok((shas, print))
}

fn older(m: &ObjectMeta, cutoff: chrono::DateTime<chrono::Utc>) -> bool {
    m.last_modified < cutoff
}

pub async fn sweep_owner(os: &dyn ObjectStore, owner: &str, grace: Duration) -> Result<usize, String> {
    let start = chrono::Utc::now();
    let cutoff = start - chrono::Duration::from_std(grace).map_err(|e| e.to_string())?;
    let tmp_cutoff = start - chrono::Duration::from_std(TMP_GRACE).map_err(|e| e.to_string())?;
    let (live, print) = read_live(os, owner).await?;

    let dead_sha: Vec<ObjectMeta> = list(os, &keys::sha_prefix(owner))
        .await?
        .into_iter()
        .filter(|m| !live.contains(&leaf(m)) && older(m, cutoff))
        .collect();
    let dead_lfs: Vec<ObjectMeta> = list(os, &keys::lfs_prefix(owner))
        .await?
        .into_iter()
        .filter(|m| !live.contains(&leaf(m)) && older(m, cutoff))
        .collect();
    let dead_tmp: Vec<ObjectMeta> =
        list(os, &keys::lfs_tmp_prefix(owner)).await?.into_iter().filter(|m| older(m, tmp_cutoff)).collect();

    if read_live(os, owner).await?.1 != print {
        return Err("live lists changed during the sweep".into());
    }
    // ponytail: a commit (or LFS batch "already have") that re-references an old, unreferenced
    // object after the re-read above can still lose it: the grace window only protects fresh
    // uploads. Close it with a per-owner lease shared by commit and sweep, or a live-list re-read
    // before each delete, if re-referencing orphans turns out to be common.

    let mut n = 0;
    for m in &dead_sha {
        // Read the file hash before deleting the pointer to it, or the record is unreachable.
        let fh = os.get(&m.location).await.map_err(|e| e.to_string())?.bytes().await.map_err(|e| e.to_string())?;
        os.delete(&m.location).await.map_err(|e| e.to_string())?;
        n += 1;
        if let Some(h) = keys::parse_hash(String::from_utf8_lossy(&fh).trim()) {
            match os.delete(&keys::file(owner, &h)).await {
                Ok(()) | Err(slatedb::object_store::Error::NotFound { .. }) => n += 1,
                Err(e) => return Err(e.to_string()),
            }
        }
    }
    for m in dead_lfs.iter().chain(&dead_tmp) {
        os.delete(&m.location).await.map_err(|e| e.to_string())?;
        n += 1;
    }

    // Xorb phase. A file record written after `start` may name a xorb the listing below would
    // call dead; skip the phase rather than race it.
    // ponytail: a shard landing between the files listing and a xorb delete is covered only by
    // the grace window (24 h in production). Close it with a per-owner lease if uploads ever take
    // longer than the grace.
    let files = list(os, &keys::file_prefix(owner)).await?;
    if files.iter().any(|m| m.last_modified >= start) {
        return Ok(n);
    }
    let mut named = BTreeSet::new();
    for m in &files {
        let Some(h) = keys::parse_hash(&leaf(m)) else { continue };
        if let Some(rec) = crate::shards::read_file(os, owner, &h).await.map_err(|e| e.to_string())? {
            named.extend(rec.terms.into_iter().map(|t| t.xorb));
        }
    }
    let objs = list(os, &keys::xorb_prefix(owner)).await?;
    let present: BTreeSet<String> = objs.iter().map(leaf).filter(|l| !l.ends_with(".idx")).collect();
    for m in objs.iter().filter(|m| !leaf(m).ends_with(".idx")) {
        let hex = leaf(m);
        if named.contains(&hex) || !older(m, cutoff) {
            continue;
        }
        let Some(h) = keys::parse_hash(&hex) else { continue };
        os.delete(&keys::xorb(owner, &h)).await.map_err(|e| e.to_string())?;
        let _ = os.delete(&keys::xorb_idx(owner, &h)).await;
        n += 1;
    }
    for m in objs.iter().filter(|m| leaf(m).ends_with(".idx") && older(m, cutoff)) {
        let hex = leaf(m).trim_end_matches(".idx").to_string();
        if !present.contains(&hex) && !named.contains(&hex) {
            os.delete(&m.location).await.map_err(|e| e.to_string())?;
            n += 1;
        }
    }
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;
    use slatedb::object_store::memory::InMemory;
    use slatedb::object_store::PutPayload;

    #[tokio::test]
    async fn missing_list_aborts_before_any_delete() {
        let os = InMemory::new();
        os.put(&"repo/ds/bob/d/manifest".into(), PutPayload::from_static(b"x")).await.unwrap();
        os.put(&keys::lfs("bob", &"d".repeat(64)), PutPayload::from_static(b"x")).await.unwrap();
        let e = sweep_owner(&os, "bob", Duration::ZERO).await.unwrap_err();
        assert!(e.contains("no live list"), "{e}");
        assert!(os.head(&keys::lfs("bob", &"d".repeat(64))).await.is_ok());
    }
}
