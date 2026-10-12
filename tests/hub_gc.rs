//! The hub sweep against an in-memory object store: live bytes stay, dead bytes past the grace go,
//! and any doubt (a repo without a live list, a list that changed mid-sweep) keeps everything.

use kloudlite_xet::gc::{sweep_owner, LiveList};
use kloudlite_xet::keys;
use sha2::{Digest, Sha256};
use slatedb::object_store::memory::InMemory;
use slatedb::object_store::{ObjectStoreExt, PutPayload};
use std::time::Duration;

async fn upload(os: &InMemory, owner: &str, pieces: &[&[u8]]) -> (kloudlite_xet::testkit::Built, String) {
    let b = kloudlite_xet::testkit::build(pieces);
    kloudlite_xet::xorbs::put_xorb(os, owner, &b.xorb_hash, b.xorb.clone().into()).await.unwrap();
    assert!(kloudlite_xet::shards::put_shard(os, owner, &b.shard).await.unwrap());
    let sha = hex::encode(Sha256::digest(&b.bytes));
    (b, sha)
}

async fn put_live(os: &InMemory, owner: &str, tag: &str, name: &str, shas: &[&str]) {
    let l = LiveList { shas: shas.iter().map(|s| s.to_string()).collect() };
    os.put(&keys::live(owner, tag, name), PutPayload::from(serde_json::to_vec(&l).unwrap())).await.unwrap();
    // The repo database exists: GC reads the repo set from the `repo/` prefix.
    os.put(&format!("repo/{tag}/{owner}/{name}/manifest").into(), PutPayload::from_static(b"x")).await.unwrap();
}

async fn exists(os: &InMemory, p: &slatedb::object_store::path::Path) -> bool {
    os.head(p).await.is_ok()
}

#[tokio::test]
async fn sweep_keeps_live_and_deletes_dead() {
    let os = InMemory::new();
    let (live, live_sha) = upload(&os, "alice", &[b"keep me"]).await;
    let (dead, dead_sha) = upload(&os, "alice", &[b"drop me"]).await;
    put_live(&os, "alice", "mdl", "m", &[&live_sha]).await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    let n = sweep_owner(&os, "alice", Duration::ZERO).await.unwrap();
    assert!(n >= 3, "sha, file record and xorb of the dead file");
    assert!(exists(&os, &keys::sha("alice", &live_sha)).await);
    assert!(exists(&os, &keys::xorb("alice", &live.xorb_hash)).await);
    assert!(!exists(&os, &keys::sha("alice", &dead_sha)).await);
    assert!(!exists(&os, &keys::file("alice", &dead.file_hash)).await);
    assert!(!exists(&os, &keys::xorb("alice", &dead.xorb_hash)).await);
}

#[tokio::test]
async fn sweep_respects_the_grace() {
    let os = InMemory::new();
    let (dead, dead_sha) = upload(&os, "alice", &[b"young"]).await;
    put_live(&os, "alice", "ds", "d", &[]).await;
    sweep_owner(&os, "alice", Duration::from_secs(3600)).await.unwrap();
    assert!(exists(&os, &keys::sha("alice", &dead_sha)).await);
    assert!(exists(&os, &keys::xorb("alice", &dead.xorb_hash)).await);
}

#[tokio::test]
async fn a_repo_without_a_live_list_aborts_the_owner() {
    let os = InMemory::new();
    let (_, sha) = upload(&os, "alice", &[b"orphan?"]).await;
    os.put(&"repo/mdl/alice/m/manifest".into(), PutPayload::from_static(b"x")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(5)).await;
    assert!(sweep_owner(&os, "alice", Duration::ZERO).await.is_err());
    assert!(exists(&os, &keys::sha("alice", &sha)).await);
}

#[tokio::test]
async fn plain_lfs_objects_follow_the_same_rule() {
    let os = InMemory::new();
    let keep = "a".repeat(64);
    let drop = "b".repeat(64);
    for oid in [&keep, &drop] {
        os.put(&keys::lfs("alice", oid), PutPayload::from_static(b"blob")).await.unwrap();
    }
    put_live(&os, "alice", "mdl", "m", &[&keep]).await;
    tokio::time::sleep(Duration::from_millis(5)).await;
    sweep_owner(&os, "alice", Duration::ZERO).await.unwrap();
    assert!(exists(&os, &keys::lfs("alice", &keep)).await);
    assert!(!exists(&os, &keys::lfs("alice", &drop)).await);
}

#[tokio::test]
async fn fresh_staging_uploads_stay_whatever_the_grace() {
    let os = InMemory::new();
    put_live(&os, "alice", "mdl", "m", &[]).await;
    let p: slatedb::object_store::path::Path = "lfs/alice/tmp/x-0000000000000001".into();
    os.put(&p, PutPayload::from_static(b"half")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(5)).await;
    // The grace argument does not shorten the staging window: a fresh upload may still be streaming.
    sweep_owner(&os, "alice", Duration::ZERO).await.unwrap();
    assert!(exists(&os, &p).await);
}
