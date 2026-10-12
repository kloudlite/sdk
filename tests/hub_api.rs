//! Hub API over the hub listener: repo lifecycle, auth, visibility, reads, commits.
mod common;

use common::*;
use kloudlite_storage::HubKind;
use serde_json::{json, Value};
use kloudlite_gitbase::objects::{apply_changes, write_commit, Change, NewCommit, Staging};
use kloudlite_gitbase::refs::UpdateRefsExt;
use kloudlite_storage::refmeta::RefUpdate;
use std::collections::BTreeMap;

struct Hub {
    base: String,
    e: TestEnv,
    alice: String,
    bob: String,
}

async fn hub() -> Hub {
    let e = env().await;
    let app = app(e.store.clone()).await;
    let alice = e.store.create_token("alice").await.unwrap();
    let bob = e.store.create_token("bob").await.unwrap();
    let port = serve_hub(app).await;
    Hub { base: format!("http://127.0.0.1:{port}"), e, alice, bob }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap()
}

async fn create(h: &Hub, token: Option<&str>, body: Value) -> reqwest::Response {
    let mut r = client().post(format!("{}/api/repos/create", h.base)).json(&body);
    if let Some(t) = token {
        r = r.bearer_auth(t);
    }
    r.send().await.unwrap()
}

#[tokio::test]
async fn create_defaults_to_private_model() {
    let h = hub().await;
    let r = create(&h, Some(&h.alice), json!({"name": "bert"})).await;
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["url"], format!("{}/alice/bert", h.base));
    assert!(h.e.store.hub_exists(HubKind::Model, "alice", "bert").await.unwrap());
    assert!(!h.e.store.hub_public(HubKind::Model, "alice", "bert").await.unwrap());
}

#[tokio::test]
async fn create_dataset_public_with_organization() {
    let h = hub().await;
    let r = create(&h, Some(&h.alice), json!({"name": "squad", "organization": "alice", "type": "dataset", "private": false})).await;
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["url"], format!("{}/datasets/alice/squad", h.base));
    assert!(h.e.store.hub_public(HubKind::Dataset, "alice", "squad").await.unwrap());
}

#[tokio::test]
async fn create_accepts_owner_slash_name() {
    let h = hub().await;
    assert_eq!(create(&h, Some(&h.alice), json!({"name": "alice/gpt"})).await.status(), 200);
    assert!(h.e.store.hub_exists(HubKind::Model, "alice", "gpt").await.unwrap());
}

#[tokio::test]
async fn create_twice_is_409_with_url() {
    let h = hub().await;
    create(&h, Some(&h.alice), json!({"name": "bert"})).await;
    let r = create(&h, Some(&h.alice), json!({"name": "bert"})).await;
    assert_eq!(r.status(), 409);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["url"], format!("{}/alice/bert", h.base));
}

#[tokio::test]
async fn create_refuses_space_anonymous_and_strangers() {
    let h = hub().await;
    assert_eq!(create(&h, Some(&h.alice), json!({"name": "s", "type": "space"})).await.status(), 400);
    assert_eq!(create(&h, None, json!({"name": "bert"})).await.status(), 401);
    assert_eq!(create(&h, Some("not-a-token"), json!({"name": "bert"})).await.status(), 401);
    let r = create(&h, Some(&h.bob), json!({"name": "bert", "organization": "alice"})).await;
    assert_eq!(r.status(), 403);
    assert!(!h.e.store.hub_exists(HubKind::Model, "alice", "bert").await.unwrap());
}

#[tokio::test]
async fn basic_user_must_name_the_token_owner() {
    let h = hub().await;
    let ok = client().get(format!("{}/api/whoami-v2", h.base)).basic_auth("x", Some(&h.alice)).send().await.unwrap();
    assert_eq!(ok.status(), 200);
    let bad = client().get(format!("{}/api/whoami-v2", h.base)).basic_auth("bob", Some(&h.alice)).send().await.unwrap();
    assert_eq!(bad.status(), 401);
}

#[tokio::test]
async fn whoami_names_the_caller() {
    let h = hub().await;
    let r = client().get(format!("{}/api/whoami-v2", h.base)).bearer_auth(&h.alice).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["name"], "alice");
    assert_eq!(v["type"], "user");
    let anon = client().get(format!("{}/api/whoami-v2", h.base)).send().await.unwrap();
    assert_eq!(anon.status(), 401);
}

#[tokio::test]
async fn delete_by_owner_only() {
    let h = hub().await;
    create(&h, Some(&h.alice), json!({"name": "bert"})).await;
    let del = |t: String| {
        let url = format!("{}/api/repos/delete", h.base);
        async move { client().delete(url).bearer_auth(t).json(&json!({"name": "bert", "organization": "alice"})).send().await.unwrap() }
    };
    let r = del(h.bob.clone()).await;
    assert_eq!(r.status(), 404);
    assert_eq!(r.headers()["x-error-code"], "RepoNotFound");
    assert_eq!(del(h.alice.clone()).await.status(), 200);
    assert!(!h.e.store.hub_exists(HubKind::Model, "alice", "bert").await.unwrap());
    assert_eq!(del(h.alice.clone()).await.status(), 404);
}

/// Commit `files` onto `main` of a hub repo straight through the store — the read tests must not
/// depend on the commit endpoint.
async fn seed_hub(h: &Hub, kind: HubKind, owner: &str, name: &str, public: bool, files: &[(&str, &[u8])]) -> String {
    let s = &h.e.store;
    if !s.hub_exists(kind, owner, name).await.unwrap() {
        s.hub_create(kind, owner, name, !public).await.unwrap();
    }
    let repo = s.hub_open(kind, owner, name).await.unwrap().unwrap();
    let parent = s.get_ref(&repo, "refs/heads/main").await.unwrap();
    let mut changes = BTreeMap::new();
    for (p, b) in files {
        changes.insert(p.to_string(), Change::Upsert { content: b.to_vec(), executable: None });
    }
    let mut staging = Staging::default();
    let odb = repo.odb().unwrap();
    let tree = apply_changes(&odb, parent.and_then(|c| tree_of(&odb, c)), changes, &mut staging).unwrap();
    staging.write(s, &repo).await.unwrap();
    let commit = write_commit(s, &repo, NewCommit {
        tree,
        parents: parent.into_iter().collect(),
        message: "seed".into(),
        author_name: owner.into(),
        author_email: format!("{owner}@test"),
        time: 1_700_000_000,
    })
    .await
    .unwrap();
    let r = s.update_refs(&repo, &[RefUpdate { name: "refs/heads/main".into(), old: parent, new: Some(commit) }]).await.unwrap();
    assert_eq!(r, vec![None]);
    commit.to_string()
}

fn tree_of(odb: &gix_odb::Handle, commit: gix_hash::ObjectId) -> Option<gix_hash::ObjectId> {
    use gix_object::Find;
    let mut buf = Vec::new();
    let c = odb.try_find(&commit, &mut buf).ok()??;
    gix_object::CommitRef::from_bytes(c.data, gix_hash::Kind::Sha1).ok().map(|c| c.tree())
}

const SHA_A: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

#[tokio::test]
async fn info_lists_siblings_with_lfs() {
    let h = hub().await;
    let ptr = format!("version https://git-lfs.github.com/spec/v1\noid sha256:{SHA_A}\nsize 5\n");
    let sha = seed_hub(&h, HubKind::Model, "alice", "bert", true, &[("config.json", b"{}"), ("model.bin", ptr.as_bytes())]).await;
    let v: Value = client().get(format!("{}/api/models/alice/bert", h.base)).send().await.unwrap().json().await.unwrap();
    assert_eq!(v["id"], "alice/bert");
    assert_eq!(v["sha"], sha);
    assert_eq!(v["private"], false);
    let sib = v["siblings"].as_array().unwrap();
    assert_eq!(sib.len(), 2);
    assert_eq!(sib[1]["rfilename"], "model.bin");
    assert_eq!(sib[1]["size"], 5);
    assert_eq!(sib[1]["lfs"]["sha256"], SHA_A);
    assert_eq!(sib[1]["lfs"]["pointerSize"], ptr.len());
    assert_eq!(sib[0]["size"], 2);
    assert!(sib[0].get("lfs").is_none());
}

#[tokio::test]
async fn info_of_empty_repo_has_no_sha() {
    let h = hub().await;
    create(&h, Some(&h.alice), json!({"name": "bert"})).await;
    let r = client().get(format!("{}/api/models/alice/bert", h.base)).bearer_auth(&h.alice).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    assert!(v["sha"].is_null());
    assert_eq!(v["siblings"], json!([]));
}

#[tokio::test]
async fn private_repo_is_401_anonymous_and_404_stranger() {
    let h = hub().await;
    seed_hub(&h, HubKind::Model, "alice", "secret", false, &[("a", b"1")]).await;
    let url = format!("{}/api/models/alice/secret", h.base);
    let anon = client().get(&url).send().await.unwrap();
    assert_eq!(anon.status(), 401);
    assert_eq!(anon.headers()["x-error-code"], "RepoNotFound");
    let bob = client().get(&url).bearer_auth(&h.bob).send().await.unwrap();
    assert_eq!(bob.status(), 404);
    assert_eq!(bob.headers()["x-error-code"], "RepoNotFound");
    assert_eq!(client().get(&url).bearer_auth(&h.alice).send().await.unwrap().status(), 200);
    let missing = client().get(format!("{}/api/models/alice/nope", h.base)).bearer_auth(&h.bob).send().await.unwrap();
    assert_eq!(missing.status(), 404);
}

#[tokio::test]
async fn revision_by_branch_tag_and_sha() {
    let h = hub().await;
    let sha = seed_hub(&h, HubKind::Dataset, "alice", "squad", true, &[("train.csv", b"a,b\n")]).await;
    for rev in ["main", sha.as_str()] {
        let v: Value = client().get(format!("{}/api/datasets/alice/squad/revision/{rev}", h.base)).send().await.unwrap().json().await.unwrap();
        assert_eq!(v["sha"], sha);
    }
    let r = client().get(format!("{}/api/datasets/alice/squad/revision/nope", h.base)).send().await.unwrap();
    assert_eq!(r.status(), 404);
    assert_eq!(r.headers()["x-error-code"], "RevisionNotFound");
}

#[tokio::test]
async fn tree_lists_and_pages() {
    let h = hub().await;
    let names: Vec<String> = (0..1001).map(|i| format!("d/f{i:04}")).collect();
    let files: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), b"x" as &[u8])).chain([("top.txt", b"t" as &[u8])]).collect();
    seed_hub(&h, HubKind::Model, "alice", "many", true, &files).await;
    let top: Value = client().get(format!("{}/api/models/alice/many/tree/main", h.base)).send().await.unwrap().json().await.unwrap();
    let top = top.as_array().unwrap();
    assert_eq!(top.len(), 2);
    assert_eq!(top[0]["type"], "directory");
    assert_eq!(top[0]["path"], "d");
    assert_eq!(top[1]["path"], "top.txt");
    let r = client().get(format!("{}/api/models/alice/many/tree/main/d", h.base)).send().await.unwrap();
    let next = r.headers()["link"].to_str().unwrap().to_string();
    assert!(next.contains("rel=\"next\""), "{next}");
    let page: Value = r.json().await.unwrap();
    assert_eq!(page.as_array().unwrap().len(), 1000);
    let url = next.trim_start_matches('<').split('>').next().unwrap().to_string();
    let rest: Value = client().get(url).send().await.unwrap().json().await.unwrap();
    assert_eq!(rest.as_array().unwrap().len(), 1);
    assert_eq!(rest[0]["path"], "d/f1000");
}

#[tokio::test]
async fn tree_with_dotdot_is_entry_not_found() {
    let h = hub().await;
    seed_hub(&h, HubKind::Model, "alice", "bert", true, &[("a", b"1")]).await;
    let r = client().get(format!("{}/api/models/alice/bert/tree/main/a%2F..%2F..", h.base)).send().await.unwrap();
    assert_eq!(r.status(), 404);
    assert_eq!(r.headers()["x-error-code"], "EntryNotFound");
}

#[tokio::test]
async fn paths_info_returns_known_paths_in_order() {
    let h = hub().await;
    seed_hub(&h, HubKind::Model, "alice", "bert", true, &[("a.txt", b"1"), ("dir/b.txt", b"22")]).await;
    let r = client()
        .post(format!("{}/api/models/alice/bert/paths-info/main", h.base))
        .header("content-type", "application/x-www-form-urlencoded")
        .body("paths=dir%2Fb.txt&paths=missing&paths=dir")
        .send()
        .await
        .unwrap();
    let v: Value = r.json().await.unwrap();
    let v = v.as_array().unwrap();
    assert_eq!(v.len(), 2);
    assert_eq!(v[0]["path"], "dir/b.txt");
    assert_eq!(v[0]["size"], 2);
    assert_eq!(v[1]["type"], "directory");
}

#[tokio::test]
async fn refs_lists_branches() {
    let h = hub().await;
    let sha = seed_hub(&h, HubKind::Model, "alice", "bert", true, &[("a", b"1")]).await;
    let v: Value = client().get(format!("{}/api/models/alice/bert/refs", h.base)).send().await.unwrap().json().await.unwrap();
    assert_eq!(v["branches"], json!([{"name": "main", "ref": "refs/heads/main", "targetCommit": sha}]));
    assert_eq!(v["tags"], json!([]));
    assert_eq!(v["converts"], json!([]));
}

#[tokio::test]
async fn tree_root_next_link_has_no_trailing_slash() {
    let h = hub().await;
    let names: Vec<String> = (0..1001).map(|i| format!("f{i:04}")).collect();
    let files: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), b"x" as &[u8])).collect();
    seed_hub(&h, HubKind::Model, "alice", "flat", true, &files).await;
    let r = client().get(format!("{}/api/models/alice/flat/tree/main", h.base)).send().await.unwrap();
    let next = r.headers()["link"].to_str().unwrap().to_string();
    assert!(next.contains("/tree/main?recursive=false&cursor=f0999>"), "{next}");
}

use slatedb::object_store::ObjectStoreExt;

/// A request line sent byte for byte: reqwest normalises `..` out of a URL before it leaves.
async fn raw_get(base: &str, path: &str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut s = tokio::net::TcpStream::connect(base.trim_start_matches("http://")).await.unwrap();
    s.write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    out.to_ascii_lowercase()
}

fn header<'a>(r: &'a reqwest::Response, k: &str) -> &'a str {
    r.headers().get(k).map(|v| v.to_str().unwrap()).unwrap_or("")
}

#[tokio::test]
async fn resolve_git_blob_whole_ranged_and_head() {
    let h = hub().await;
    let sha = seed_hub(&h, HubKind::Model, "alice", "bert", true, &[("config.json", b"{\"a\":1}")]).await;
    let url = format!("{}/alice/bert/resolve/main/config.json", h.base);
    let r = client().get(&url).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(header(&r, "x-repo-commit"), sha);
    assert_eq!(header(&r, "accept-ranges"), "bytes");
    assert!(header(&r, "etag").starts_with('"'));
    assert_eq!(header(&r, "x-linked-etag"), header(&r, "etag"));
    assert_eq!(header(&r, "x-linked-size"), "7");
    assert_eq!(r.bytes().await.unwrap().as_ref(), b"{\"a\":1}");

    let r = client().get(&url).header("range", "bytes=1-2").send().await.unwrap();
    assert_eq!(r.status(), 206);
    assert_eq!(header(&r, "content-range"), "bytes 1-2/7");
    assert_eq!(r.bytes().await.unwrap().as_ref(), b"\"a");

    let r = client().head(&url).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(header(&r, "content-length"), "7");
    assert_eq!(header(&r, "x-repo-commit"), sha);

    let r = client().get(format!("{}/alice/bert/resolve/{sha}/config.json", h.base)).send().await.unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn resolve_range_past_end_is_416() {
    let h = hub().await;
    seed_hub(&h, HubKind::Model, "alice", "bert", true, &[("config.json", b"{\"a\":1}")]).await;
    let url = format!("{}/alice/bert/resolve/main/config.json", h.base);
    let get = |range: &'static str| client().get(&url).header("range", range).send();
    let r = get("bytes=0-0").await.unwrap();
    assert_eq!(r.status(), 206);
    assert_eq!(r.bytes().await.unwrap().as_ref(), b"{");
    let r = get("bytes=6-6").await.unwrap();
    assert_eq!(r.bytes().await.unwrap().as_ref(), b"}");
    let r = get("bytes=5-99").await.unwrap();
    assert_eq!(header(&r, "content-range"), "bytes 5-6/7");
    let r = get("bytes=7-").await.unwrap();
    assert_eq!(r.status(), 416);
    assert_eq!(header(&r, "content-range"), "bytes */7");
    assert_eq!(get("bytes=a-b").await.unwrap().status(), 416);
}

#[tokio::test]
async fn resolve_dotdot_is_entry_not_found() {
    let h = hub().await;
    let sha = seed_hub(&h, HubKind::Model, "alice", "bert", true, &[("config.json", b"{}"), ("a/x", b"x")]).await;
    for p in ["a/../config.json", "a/%2e%2e/config.json", ".git/config", "a//x"] {
        let out = raw_get(&h.base, &format!("/alice/bert/resolve/main/{p}")).await;
        assert!(out.starts_with("http/1.1 404"), "{p}: {out}");
        assert!(out.contains("x-error-code: entrynotfound"), "{p}");
        assert!(out.contains(&format!("x-repo-commit: {sha}")), "{p}");
    }
}

#[tokio::test]
async fn resolve_missing_file_and_rev() {
    let h = hub().await;
    let sha = seed_hub(&h, HubKind::Model, "alice", "bert", true, &[("config.json", b"{}")]).await;
    let r = client().get(format!("{}/alice/bert/resolve/main/nope.txt", h.base)).send().await.unwrap();
    assert_eq!(r.status(), 404);
    assert_eq!(header(&r, "x-error-code"), "EntryNotFound");
    assert_eq!(header(&r, "x-repo-commit"), sha);
    let r = client().get(format!("{}/alice/bert/resolve/nope/config.json", h.base)).send().await.unwrap();
    assert_eq!(r.status(), 404);
    assert_eq!(header(&r, "x-error-code"), "RevisionNotFound");
}

#[tokio::test]
async fn resolve_lfs_file_from_lfs_store() {
    let h = hub().await;
    let ptr = format!("version https://git-lfs.github.com/spec/v1\noid sha256:{SHA_A}\nsize 5\n");
    seed_hub(&h, HubKind::Dataset, "alice", "squad", true, &[("data.bin", ptr.as_bytes())]).await;
    let url = format!("{}/datasets/alice/squad/resolve/main/data.bin", h.base);
    let r = client().get(&url).send().await.unwrap();
    assert_eq!(r.status(), 404);
    assert_eq!(header(&r, "x-error-code"), "EntryNotFound");

    h.e.store.os.put(&kloudlite_xet::keys::lfs("alice", SHA_A), b"hello".to_vec().into()).await.unwrap();
    let r = client().get(&url).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(header(&r, "x-linked-etag").trim_matches('"'), SHA_A);
    assert_eq!(header(&r, "x-linked-size"), "5");
    assert_eq!(header(&r, "etag").trim_matches('"'), SHA_A);
    assert_eq!(header(&r, "x-xet-hash"), "");
    assert_eq!(r.bytes().await.unwrap().as_ref(), b"hello");
    let r = client().get(&url).header("range", "bytes=1-3").send().await.unwrap();
    assert_eq!(r.status(), 206);
    assert_eq!(r.bytes().await.unwrap().as_ref(), b"ell");
}

#[tokio::test]
async fn resolve_xet_file_streams_from_xorbs() {
    let h = hub().await;
    let b = kloudlite_xet::testkit::build(&[b"hello ".as_slice(), b"xet ".as_slice(), b"world".as_slice()]);
    let os = &*h.e.store.os;
    kloudlite_xet::xorbs::put_xorb(os, "alice", &b.xorb_hash, b.xorb.clone().into()).await.unwrap();
    kloudlite_xet::shards::put_shard(os, "alice", &b.shard).await.unwrap();
    let sha = hex::encode(b.sha256);
    let ptr = format!("version https://git-lfs.github.com/spec/v1\noid sha256:{sha}\nsize {}\n", b.bytes.len());
    let commit = seed_hub(&h, HubKind::Model, "alice", "bert", false, &[("model.safetensors", ptr.as_bytes())]).await;
    let url = format!("{}/alice/bert/resolve/main/model.safetensors", h.base);

    let r = client().get(&url).send().await.unwrap();
    assert_eq!(r.status(), 401, "private repo, anonymous");

    let r = client().get(&url).bearer_auth(&h.alice).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(header(&r, "x-xet-hash"), b.file_hash.hex());
    assert_eq!(
        header(&r, "link"),
        format!("<{}/api/models/alice/bert/xet-read-token/{commit}>; rel=\"xet-auth\"", h.base)
    );
    assert_eq!(header(&r, "x-linked-size"), b.bytes.len().to_string());
    assert_eq!(r.bytes().await.unwrap().as_ref(), &b.bytes[..]);

    let r = client().get(&url).bearer_auth(&h.alice).header("range", "bytes=3-8").send().await.unwrap();
    assert_eq!(r.status(), 206);
    assert_eq!(r.bytes().await.unwrap().as_ref(), &b.bytes[3..9]);

    let r = client().head(&url).bearer_auth(&h.alice).send().await.unwrap();
    assert_eq!(header(&r, "content-length"), b.bytes.len().to_string());
}

use base64::Engine;

fn b64(b: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(b)
}

async fn hub_commit(h: &Hub, token: Option<&str>, path: &str, lines: &[Value]) -> reqwest::Response {
    let body = lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
    let mut r = client().post(format!("{}/api/{path}", h.base)).header("content-type", "application/x-ndjson").body(body);
    if let Some(t) = token {
        r = r.bearer_auth(t);
    }
    r.send().await.unwrap()
}

fn file_line(path: &str, content: &[u8]) -> Value {
    json!({"key": "file", "value": {"path": path, "content": b64(content), "encoding": "base64"}})
}

async fn main_head(h: &Hub, kind: HubKind, name: &str) -> Option<String> {
    let s = &h.e.store;
    let repo = s.hub_open(kind, "alice", name).await.unwrap().unwrap();
    s.get_ref(&repo, "refs/heads/main").await.unwrap().map(|o| o.to_string())
}

async fn fetch(h: &Hub, path: &str) -> (u16, Vec<u8>) {
    let r = client().get(format!("{}/{path}", h.base)).bearer_auth(&h.alice).send().await.unwrap();
    (r.status().as_u16(), r.bytes().await.unwrap().to_vec())
}

#[tokio::test]
async fn preupload_picks_modes() {
    let h = hub().await;
    create(&h, Some(&h.alice), json!({"name": "bert"})).await;
    let body = json!({"files": [
        {"path": "config.json", "size": 2, "sample": b64(b"{}")},
        {"path": "big.txt", "size": 10485760, "sample": b64(b"text")},
        {"path": "w.bin", "size": 3, "sample": b64(b"a\0b")},
    ]});
    let r = client().post(format!("{}/api/models/alice/bert/preupload/main", h.base)).bearer_auth(&h.alice).json(&body).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    let modes: Vec<&str> = v["files"].as_array().unwrap().iter().map(|f| f["uploadMode"].as_str().unwrap()).collect();
    assert_eq!(modes, ["regular", "lfs", "lfs"]);
    assert_eq!(v["files"][0]["shouldIgnore"], false);
    let r = client().post(format!("{}/api/models/alice/bert/preupload/main", h.base)).json(&body).send().await.unwrap();
    assert_eq!(r.status(), 401);
}

#[tokio::test]
async fn commit_inline_files_then_read_back() {
    let h = hub().await;
    create(&h, Some(&h.alice), json!({"name": "bert"})).await;
    let r = hub_commit(&h, Some(&h.alice), "models/alice/bert/commit/main", &[
        json!({"key": "header", "value": {"summary": "first", "description": ""}}),
        file_line("config.json", b"{\"a\":1}"),
        file_line("sub/readme.md", b"# hi"),
    ]).await;
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    let oid = v["commitOid"].as_str().unwrap().to_string();
    assert_eq!(v["commitUrl"], format!("{}/alice/bert/commit/{oid}", h.base));
    assert_eq!(main_head(&h, HubKind::Model, "bert").await.as_deref(), Some(oid.as_str()));
    assert_eq!(fetch(&h, "alice/bert/resolve/main/sub/readme.md").await, (200, b"# hi".to_vec()));
}

#[tokio::test]
async fn stale_parent_is_412() {
    let h = hub().await;
    create(&h, Some(&h.alice), json!({"name": "bert"})).await;
    let first: Value = hub_commit(&h, Some(&h.alice), "models/alice/bert/commit/main", &[file_line("a", b"1")]).await.json().await.unwrap();
    let a = first["commitOid"].as_str().unwrap().to_string();
    let hdr = |p: &str| json!({"key": "header", "value": {"summary": "s", "parentCommit": p}});
    let r = hub_commit(&h, Some(&h.alice), "models/alice/bert/commit/main", &[hdr(&a), file_line("a", b"2")]).await;
    assert_eq!(r.status(), 200);
    let b: Value = r.json().await.unwrap();
    let r = hub_commit(&h, Some(&h.alice), "models/alice/bert/commit/main", &[hdr(&a), file_line("a", b"3")]).await;
    assert_eq!(r.status(), 412);
    assert_eq!(main_head(&h, HubKind::Model, "bert").await.as_deref(), b["commitOid"].as_str());
    assert_eq!(fetch(&h, "alice/bert/resolve/main/a").await.1, b"2");
}

#[tokio::test]
async fn commit_refuses_dotdot_path() {
    let h = hub().await;
    create(&h, Some(&h.alice), json!({"name": "bert"})).await;
    for p in ["../x", "a/../../x", "/etc/passwd", ".git/hooks/pre-receive"] {
        let r = hub_commit(&h, Some(&h.alice), "models/alice/bert/commit/main", &[file_line(p, b"x")]).await;
        assert_eq!(r.status(), 400, "{p}");
    }
    assert_eq!(main_head(&h, HubKind::Model, "bert").await, None);
}

#[tokio::test]
async fn lfs_file_needs_uploaded_bytes_and_writes_gitattributes() {
    let h = hub().await;
    create(&h, Some(&h.alice), json!({"name": "bert"})).await;
    let lfs = json!({"key": "lfsFile", "value": {"path": "model.bin", "algo": "sha256", "oid": SHA_A, "size": 5}});
    let r = hub_commit(&h, Some(&h.alice), "models/alice/bert/commit/main", std::slice::from_ref(&lfs)).await;
    assert_eq!(r.status(), 422);
    assert!(r.text().await.unwrap().contains("model.bin"));

    h.e.store.os.put(&kloudlite_xet::keys::lfs("alice", SHA_A), b"hello".to_vec().into()).await.unwrap();
    let r = hub_commit(&h, Some(&h.alice), "models/alice/bert/commit/main", &[lfs]).await;
    assert_eq!(r.status(), 200);
    assert_eq!(fetch(&h, "alice/bert/resolve/main/model.bin").await, (200, b"hello".to_vec()));
    let (_, attrs) = fetch(&h, "alice/bert/resolve/main/.gitattributes").await;
    assert_eq!(String::from_utf8(attrs).unwrap(), "model.bin filter=lfs diff=lfs merge=lfs -text\n");
}

#[tokio::test]
async fn deletes_file_and_folder() {
    let h = hub().await;
    create(&h, Some(&h.alice), json!({"name": "bert"})).await;
    hub_commit(&h, Some(&h.alice), "models/alice/bert/commit/main", &[file_line("logs/1", b"1"), file_line("logs/2/x", b"2"), file_line("keep", b"k"), file_line("old", b"o")]).await;
    let r = hub_commit(&h, Some(&h.alice), "models/alice/bert/commit/main", &[
        json!({"key": "deletedFolder", "value": {"path": "logs/"}}),
        json!({"key": "deletedFile", "value": {"path": "old"}}),
    ]).await;
    assert_eq!(r.status(), 200);
    assert_eq!(fetch(&h, "alice/bert/resolve/main/keep").await.0, 200);
    for gone in ["logs/1", "logs/2/x", "old"] {
        assert_eq!(fetch(&h, &format!("alice/bert/resolve/main/{gone}")).await.0, 404, "{gone}");
    }
}

#[tokio::test]
async fn commit_refusals() {
    let h = hub().await;
    create(&h, Some(&h.alice), json!({"name": "bert", "private": false})).await;
    let line = [file_line("a", b"1")];
    let path = "models/alice/bert/commit/main";
    assert_eq!(hub_commit(&h, None, path, &line).await.status(), 401);
    assert_eq!(hub_commit(&h, Some(&h.bob), path, &line).await.status(), 403);
    assert_eq!(hub_commit(&h, Some(&h.alice), "models/alice/bert/commit/main?create_pr=1", &line).await.status(), 404);
    assert_eq!(hub_commit(&h, Some(&h.alice), "models/alice/bert/commit/dev", &line).await.status(), 404, "no such branch on an empty repo");
    assert_eq!(hub_commit(&h, Some(&h.alice), path, &[]).await.status(), 400, "nothing to commit");
    assert_eq!(hub_commit(&h, Some(&h.alice), "datasets/alice/bert/commit/main", &line).await.status(), 404, "a model is not a dataset");
}

async fn post_json(h: &Hub, path: &str, token: &str, body: Value) -> reqwest::Response {
    client().post(format!("{}{path}", h.base)).bearer_auth(token).json(&body).send().await.unwrap()
}

async fn del(h: &Hub, path: &str, token: &str) -> reqwest::Response {
    client().delete(format!("{}{path}", h.base)).bearer_auth(token).send().await.unwrap()
}

#[tokio::test]
async fn branch_create_list_delete() {
    let h = hub().await;
    let main = seed_hub(&h, HubKind::Model, "alice", "m", false, &[("a.txt", b"a")]).await;
    let r = post_json(&h, "/api/models/alice/m/branch/dev", &h.alice, json!({})).await;
    assert_eq!(r.status(), 200);
    let refs: Value = client().get(format!("{}/api/models/alice/m/refs", h.base)).bearer_auth(&h.alice).send().await.unwrap().json().await.unwrap();
    let dev = refs["branches"].as_array().unwrap().iter().find(|b| b["name"] == "dev").unwrap();
    assert_eq!(dev["targetCommit"], main);
    assert_eq!(post_json(&h, "/api/models/alice/m/branch/dev", &h.alice, json!({})).await.status(), 409);
    assert_eq!(del(&h, "/api/models/alice/m/branch/dev", &h.alice).await.status(), 200);
    assert_eq!(del(&h, "/api/models/alice/m/branch/dev", &h.alice).await.status(), 404);
}

#[tokio::test]
async fn branch_from_unknown_start_is_revision_not_found() {
    let h = hub().await;
    seed_hub(&h, HubKind::Model, "alice", "m", false, &[("a.txt", b"a")]).await;
    let r = post_json(&h, "/api/models/alice/m/branch/dev", &h.alice, json!({"startingPoint": "nope"})).await;
    assert_eq!(r.status(), 404);
    assert_eq!(r.headers()["x-error-code"], "RevisionNotFound");
}

#[tokio::test]
async fn main_cannot_be_deleted_and_strangers_cannot_branch() {
    let h = hub().await;
    seed_hub(&h, HubKind::Model, "alice", "m", true, &[("a.txt", b"a")]).await;
    assert_eq!(del(&h, "/api/models/alice/m/branch/main", &h.alice).await.status(), 403);
    assert_eq!(post_json(&h, "/api/models/alice/m/branch/dev", &h.bob, json!({})).await.status(), 403);
}

#[tokio::test]
async fn bad_ref_names_are_400() {
    let h = hub().await;
    seed_hub(&h, HubKind::Model, "alice", "m", false, &[("a.txt", b"a")]).await;
    for bad in ["a..b", "a.lock", "a%20b", "a~1", "x.", "%40%7B"] {
        let r = post_json(&h, &format!("/api/models/alice/m/branch/{bad}"), &h.alice, json!({})).await;
        assert_eq!(r.status(), 400, "{bad}");
    }
}

#[tokio::test]
async fn tag_create_and_delete() {
    let h = hub().await;
    let main = seed_hub(&h, HubKind::Dataset, "alice", "d", false, &[("a.csv", b"1")]).await;
    let r = post_json(&h, "/api/datasets/alice/d/tag/main", &h.alice, json!({"tag": "v1"})).await;
    assert_eq!(r.status(), 200);
    assert_eq!(post_json(&h, "/api/datasets/alice/d/tag/main", &h.alice, json!({"tag": "v1"})).await.status(), 409);
    let info: Value = client().get(format!("{}/api/datasets/alice/d/revision/v1", h.base)).bearer_auth(&h.alice).send().await.unwrap().json().await.unwrap();
    assert_eq!(info["sha"], main);
    assert_eq!(del(&h, "/api/datasets/alice/d/tag/v1", &h.alice).await.status(), 200);
}

#[tokio::test]
async fn xet_tokens_carry_scope_and_cas_url() {
    let h = hub().await;
    seed_hub(&h, HubKind::Model, "alice", "m", false, &[("a.txt", b"a")]).await;
    let r = client().get(format!("{}/api/models/alice/m/xet-read-token/main", h.base)).bearer_auth(&h.alice).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(r.headers().contains_key("x-xet-access-token"));
    let b: Value = r.json().await.unwrap();
    assert!(b["casUrl"].as_str().unwrap().starts_with("http"));
    assert!(b["exp"].as_u64().unwrap() > 0);
    let r = client().get(format!("{}/api/models/alice/m/xet-write-token/main", h.base)).bearer_auth(&h.bob).send().await.unwrap();
    assert_eq!(r.status(), 404, "private repo hides from a stranger");
    let r = client().get(format!("{}/api/models/alice/m/xet-read-token/nope", h.base)).bearer_auth(&h.alice).send().await.unwrap();
    assert_eq!(r.status(), 404);
}

fn batch_body(op: &str, oid: &str, size: u64, transfers: &[&str]) -> Value {
    json!({ "operation": op, "transfers": transfers, "objects": [{ "oid": oid, "size": size }], "hash_algo": "sha256" })
}

async fn batch(h: &Hub, path: &str, auth: Option<(&str, &str)>, body: Value) -> reqwest::Response {
    let mut r = client().post(format!("{}{path}/info/lfs/objects/batch", h.base))
        .header("accept", "application/vnd.git-lfs+json")
        .header("content-type", "application/vnd.git-lfs+json")
        .body(body.to_string());
    if let Some((u, p)) = auth {
        r = r.basic_auth(u, Some(p));
    }
    r.send().await.unwrap()
}

fn sha256_hex(b: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(b))
}

#[tokio::test]
async fn lfs_basic_upload_then_download() {
    let h = hub().await;
    seed_hub(&h, HubKind::Model, "alice", "m", false, &[("a.txt", b"a")]).await;
    let data = b"weights weights weights".to_vec();
    let oid = sha256_hex(&data);
    let r = batch(&h, "/alice/m.git", Some(("alice", &h.alice)), batch_body("upload", &oid, data.len() as u64, &["basic"])).await;
    assert_eq!(r.status(), 200);
    assert_eq!(r.headers()["content-type"], "application/vnd.git-lfs+json");
    let b: Value = r.json().await.unwrap();
    assert_eq!(b["transfer"], "basic");
    let href = b["objects"][0]["actions"]["upload"]["href"].as_str().unwrap().to_string();
    assert_eq!(client().put(&href).body(data.clone()).send().await.unwrap().status(), 200);

    let b: Value = batch(&h, "/alice/m", Some(("alice", &h.alice)), batch_body("upload", &oid, data.len() as u64, &["basic"])).await.json().await.unwrap();
    assert!(b["objects"][0].get("actions").is_none(), "stored object needs no upload");

    let b: Value = batch(&h, "/alice/m.git", Some(("alice", &h.alice)), batch_body("download", &oid, data.len() as u64, &["basic"])).await.json().await.unwrap();
    let href = b["objects"][0]["actions"]["download"]["href"].as_str().unwrap();
    assert_eq!(b["objects"][0]["actions"]["download"]["expires_in"], 3600);
    assert_eq!(client().get(href).send().await.unwrap().bytes().await.unwrap().to_vec(), data);
}

#[tokio::test]
async fn lfs_upload_offers_xet_when_asked() {
    let h = hub().await;
    seed_hub(&h, HubKind::Dataset, "alice", "d", false, &[("a.csv", b"1")]).await;
    let oid = sha256_hex(b"new");
    let b: Value = batch(&h, "/datasets/alice/d.git", Some(("alice", &h.alice)), batch_body("upload", &oid, 3, &["xet", "basic"])).await.json().await.unwrap();
    assert_eq!(b["transfer"], "xet");
    let up = &b["objects"][0]["actions"]["upload"];
    assert!(up["href"].as_str().unwrap().ends_with("/api/datasets/alice/d/xet-write-token/main"));
    assert!(up["header"]["X-Xet-Access-Token"].as_str().is_some());
}

#[tokio::test]
async fn lfs_unknown_and_invalid_oids_are_per_object_errors() {
    let h = hub().await;
    seed_hub(&h, HubKind::Model, "alice", "m", true, &[("a.txt", b"a")]).await;
    let b: Value = batch(&h, "/alice/m.git", None, batch_body("download", &"0".repeat(64), 1, &["basic"])).await.json().await.unwrap();
    assert_eq!(b["objects"][0]["error"]["code"], 404);
    let b: Value = batch(&h, "/alice/m.git", None, batch_body("download", "../x", 1, &["basic"])).await.json().await.unwrap();
    assert_eq!(b["objects"][0]["error"]["code"], 422);
}

#[tokio::test]
async fn lfs_upload_needs_write_and_basic_user_must_match() {
    let h = hub().await;
    seed_hub(&h, HubKind::Model, "alice", "m", true, &[("a.txt", b"a")]).await;
    let oid = sha256_hex(b"x");
    assert_eq!(batch(&h, "/alice/m.git", None, batch_body("upload", &oid, 1, &["basic"])).await.status(), 401);
    assert_eq!(batch(&h, "/alice/m.git", Some(("bob", &h.bob)), batch_body("upload", &oid, 1, &["basic"])).await.status(), 403);
    assert_eq!(batch(&h, "/alice/m.git", Some(("mallory", &h.alice)), batch_body("upload", &oid, 1, &["basic"])).await.status(), 401);
}

#[tokio::test]
async fn lfs_locks_are_404() {
    let h = hub().await;
    seed_hub(&h, HubKind::Model, "alice", "m", true, &[("a.txt", b"a")]).await;
    let r = client().post(format!("{}/alice/m.git/info/lfs/locks/verify", h.base)).basic_auth("alice", Some(&h.alice)).body("{}").send().await.unwrap();
    assert_eq!(r.status(), 404);
}
