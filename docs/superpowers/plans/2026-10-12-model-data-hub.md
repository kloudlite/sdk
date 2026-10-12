# Model and Dataset Hub Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Serve Hugging Face style model and dataset repos (huggingface_hub, git-lfs, git-xet, transformers) from kloudlite-srv, with a per-owner deduplicating Xet CAS on its own hostname.

**Architecture:** Two new library crates mounted by `bins/server` on two new listeners. `crates/hub` speaks the Hub API over the existing git storage (`crates/gitbase`, `crates/git`) in two new namespaces (`mdl`, `ds`) routed before auth like every other per-repo database. `crates/xet` is the CAS: it validates xorbs and shards with `xet-core-structures` 1.7.0, stores bytes per owner in the object store, and answers reconstruction queries with HMAC-signed fetch URLs. A worker lane sweeps unreferenced bytes from per-repo live lists.

**Tech Stack:** Rust, axum, slatedb object store, gix, `xet-core-structures = "1.7.0"`, `hmac 0.12`, `sha2`, `jsonwebtoken` (existing), `hf-xet 1.7.0` as a dev-dependency only, `huggingface_hub`/`git-lfs`/`git-xet` in the e2e script.

**Spec:** `docs/superpowers/specs/2026-10-12-model-data-hub-design.md`

**Paths:** all paths are relative to the worktree `/Volumes/kdisk/rustic-git-wt/master`. Every command is prefixed `cd /Volumes/kdisk/rustic-git-wt/master &&` and every cargo call sets `CARGO_TARGET_DIR=/Volumes/kdisk/target`. Pipe test output through `2>&1 | tail -30`.

## Global Constraints

- Listeners: `hub` on `KLOUDLITE_HUB_ADDR` (default `0.0.0.0:8090`), the HF_ENDPOINT at `hub.<domain>`; `cas` on `KLOUDLITE_CAS_ADDR` (default `0.0.0.0:8091`) at `cas.<domain>`. External URLs `KLOUDLITE_HUB_URL`, `KLOUDLITE_CAS_URL`.
- Namespaces: model DB `repo/mdl/{owner}/{name}`, routing key `mdl/{owner}/{name}`, URL `/{owner}/{name}`; dataset DB `repo/ds/{owner}/{name}`, key `ds/{owner}/{name}`, URL `/datasets/{owner}/{name}`.
- Reserved owners gain `mdl`, `ds`, `models`, `datasets`.
- Object keys: `xet/{owner}/xorbs/{hash}` and `xet/{owner}/xorbs/{hash}.idx`; `xet/{owner}/files/{hash}`; `xet/{owner}/sha/{sha256}`; `lfs/{owner}/sha256/{hex}`; `hub/{owner}/live/{mdl|ds}/{name}`. Shards are not stored.
- Hashes on the wire and in keys use `MerkleHash::hex()` form; sha256 keys use the true digest hex (from `as_bytes()`).
- CAS token: JWT HS256, `typ:"cas"`, `sub`=owner, claims `repo`, `rev`, `scope` (`read`|`write`), TTL 3600 s. Bad token 401; read token on an upload route 403; a token touches only its own owner's namespace.
- CAS is v1 only: `/v2/*` is 404 (the client falls back to v1 on 404). `/v1/chunks/*` is 404. `POST /v1/reconstructions` is 404.
- Xorb limits enforced by us: body ≤ 64 MiB (`max_xet_object`), ≤ 8192 chunks.
- Signed fetch URLs: HMAC-SHA256 with the jwt secret, `exp` + `sig` query, 1 h; bad or expired 403.
- Preupload: `uploadMode` `lfs` when size ≥ 10 MiB (10485760) or the sample contains a NUL byte, else `regular`; `shouldIgnore:false`.
- LFS pointer text exactly `version https://git-lfs.github.com/spec/v1\noid sha256:<hex>\nsize <n>\n`; `.gitattributes` line `<path> filter=lfs diff=lfs merge=lfs -text`.
- LFS basic upload cap 5 GiB; sha256 checked while streaming; mismatch 400 and the object is not stored.
- Create defaults to private. Private repo: anonymous 401, stranger 404 `RepoNotFound`. Directory `Unavailable` denies.
- Every resolve response carries `X-Repo-Commit`, including 404s; 404s carry `X-Error-Code` `EntryNotFound` | `RepoNotFound` | `RevisionNotFound`.
- Not served (404 with `X-Error-Message`): PRs (`create_pr=1`), discussions, move, settings, super-squash, commit listing, search, Spaces.
- GC: 24 h grace, one owner at a time, missing/unreadable live list aborts that owner; delete order sha/, files/, lfs/, then orphan xorbs and `.idx`.
- House rules: `#![deny(clippy::unwrap_used, clippy::expect_used)]` in new crates; files under ~800 lines with a `//!` header; deliberate shortcuts carry `// ponytail:`; commits imperative sentence case, no tool attribution, never `--no-verify`; `cargo clippy --workspace --all-targets -- -D warnings` must stay clean.

**Deviation from spec §5, ruled here:** the GC sweep runs as a worker lane modelled on registry `gc_lane` (`bins/worker/src/main.rs:587-649`), not in `bins/server/src/lanes.rs`. Reason: the registry sweep already lives there, owner listing and gap pacing come for free, and the server tier stays free of bulk deletes.

## Review Focus

1. **Path traversal in a file path** (`resolve`, `tree`, commit `file.path` with `..`, leading `/`, or `.git/`): the request is refused 400 (commit) or 404 `EntryNotFound` (read), never stored. Test added to Task 10 (`commit_refuses_dotdot_path`) and Task 9 (`resolve_dotdot_is_entry_not_found`).
2. **A shard whose term points at another owner's xorb**: refused 400 even though the xorb exists under the other owner. Test added to Task 4 (`shard_cannot_reference_another_owners_xorb`).
3. **Re-upload of an existing xorb under a stale or tampered body with the same hash**: `was_inserted:false`, stored bytes unchanged, tampered body 400. Test added to Task 3 (`tampered_xorb_is_refused_and_original_kept`).
4. **Range edges on resolve and reconstruction** (`bytes=0-0`, last byte, start beyond size): one byte, last byte, 416. Test added to Task 5 (`reconstruction_single_byte_and_past_end`) and Task 9 (`resolve_range_past_end_is_416`).
5. **Concurrent commit with a stale `parentCommit`**: second commit 412, branch unchanged. Test added to Task 10 (`stale_parent_is_412`).

---

### Task 1: Reserved owners and hub repo storage

**Files:**
- Modify: `crates/storage/src/store.rs:340` (`RESERVED_OWNERS`), `crates/storage/src/store.rs:493-560` (`open_repo`)
- Modify: `crates/storage/src/refmeta.rs:129-330` (`create_repo`, `delete_repo`, `repo_exists`, `set_public`, `repo_public`)
- Create: `crates/storage/src/hubrepo.rs`
- Modify: `crates/storage/src/lib.rs` (add `pub mod hubrepo;`)
- Test: `crates/storage/src/hubrepo.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: existing `Store`, `Repo{owner,name,objects_dir,pack_dir}`, `valid_owner`, `valid_segment`.
- Produces:
  - `pub enum HubKind { Model, Dataset }` with `fn ns(self) -> &'static str` (`"mdl"`/`"ds"`), `fn url_prefix(self) -> &'static str` (`""`/`"datasets/"`), `fn api_plural(self) -> &'static str` (`"models"`/`"datasets"`), `fn from_type(&str) -> Option<HubKind>` (`"model"`→Model, `"dataset"`→Dataset).
  - `pub fn hub_routing_key(kind: HubKind, owner: &str, name: &str) -> String` → `"{ns}/{owner}/{name}"`.
  - `pub fn hub_pool_coords(kind, owner, name) -> (&'static str, String)` → `(ns, "{owner}/{name}")`.
  - `impl Store { pub async fn hub_create(&self, kind, owner, name, private: bool) -> Result<()>; pub async fn hub_open(&self, kind, owner, name) -> Result<Option<Repo>>; pub async fn hub_exists(&self, kind, owner, name) -> Result<bool>; pub async fn hub_delete(&self, kind, owner, name) -> Result<()>; pub async fn hub_public(&self, kind, owner, name) -> Result<bool>; }`
  - A hub repo is a store-level repo with `owner = ns` and `name = "{owner}/{name}"`, so its DB is `repo/mdl/{owner}/{name}`, its packs `objects/mdl/{owner}/{name}/pack`, its refs `ref/mdl/{owner}/{name}/…`. Index markers are never written for hub repos.

- [ ] **Step 1: Write the failing test**

Add to `crates/storage/src/hubrepo.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use slatedb::object_store::memory::InMemory;
    use std::sync::Arc;

    async fn store() -> (crate::store::Store, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let os = Arc::new(InMemory::new());
        let s = crate::store::Store::open(os, tmp.path().join("cache"), false).await.unwrap();
        (s, tmp)
    }

    #[tokio::test]
    async fn hub_repo_lives_under_its_own_namespace() {
        let (s, _t) = store().await;
        s.hub_create(HubKind::Model, "alice", "bert", true).await.unwrap();
        assert!(s.hub_exists(HubKind::Model, "alice", "bert").await.unwrap());
        assert!(!s.hub_exists(HubKind::Dataset, "alice", "bert").await.unwrap());
        assert!(!s.repo_exists("alice", "bert").await.unwrap(), "a model is not a git repo");
        let r = s.hub_open(HubKind::Model, "alice", "bert").await.unwrap().unwrap();
        assert_eq!((r.owner.as_str(), r.name.as_str()), ("mdl", "alice/bert"));
        assert!(!s.hub_public(HubKind::Model, "alice", "bert").await.unwrap());
        assert!(s.hub_create(HubKind::Model, "alice", "bert", true).await.is_err(), "exists");
        s.hub_delete(HubKind::Model, "alice", "bert").await.unwrap();
        assert!(!s.hub_exists(HubKind::Model, "alice", "bert").await.unwrap());
    }

    #[tokio::test]
    async fn hub_names_are_validated_per_segment() {
        let (s, _t) = store().await;
        assert!(s.hub_create(HubKind::Model, "alice", "a/b", false).await.is_err());
        assert!(s.hub_create(HubKind::Model, "..", "x", false).await.is_err());
        assert!(s.hub_create(HubKind::Model, "mdl", "x", false).await.is_err(), "reserved owner");
        assert!(s.hub_open(HubKind::Model, "alice", "..").await.unwrap().is_none());
    }

    #[test]
    fn keys() {
        assert_eq!(hub_routing_key(HubKind::Dataset, "a", "b"), "ds/a/b");
        assert_eq!(hub_pool_coords(HubKind::Model, "a", "b"), ("mdl", "a/b".to_string()));
        for o in ["mdl", "ds", "models", "datasets"] {
            assert!(crate::store::RESERVED_OWNERS.contains(&o), "{o}");
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test -p kloudlite-storage hubrepo 2>&1 | tail -20`
Expected: FAIL, `cannot find type HubKind` / unresolved `hub_create`.

- [ ] **Step 3: Implement**

1. `RESERVED_OWNERS` (store.rs:340): append `"mdl", "ds", "models", "datasets"` and widen the array length.
2. Split the existing bodies so validation is the only thing the public git entry points add. In `store.rs` rename the body of `open_repo` after its `valid_segment` check into `pub(crate) async fn open_repo_unchecked(&self, owner: &str, name: &str, markers: bool) -> Result<Option<Repo>>`; the marker reconcile block runs only `if markers`. `open_repo` becomes:

```rust
pub async fn open_repo(&self, owner: &str, name: &str) -> Result<Option<Repo>> {
    if !valid_segment(owner) || !valid_segment(name) {
        return Ok(None);
    }
    self.open_repo_unchecked(owner, name, true).await
}
```

   Do the same in `refmeta.rs` for `create_repo` (→ `create_repo_unchecked(owner, name, markers)`, the `valid_owner`/`valid_segment` check stays in the wrapper; any index marker write runs only `if markers`) and `delete_repo` (→ `delete_repo_unchecked(owner, name, markers)`). `repo_exists`, `set_public`, `repo_public` already take the strings as given; verify they apply no segment check and reuse them directly.
3. Create `crates/storage/src/hubrepo.rs`:

```rust
//! Model and dataset repos: git repos in namespaces of their own.
//!
//! A hub repo is an ordinary store-level repo whose owner is the namespace (`mdl` or `ds`) and
//! whose name is `{owner}/{name}`. That one choice gives it a database `repo/mdl/{owner}/{name}`,
//! packs under `objects/mdl/{owner}/{name}/pack` and refs under `ref/mdl/{owner}/{name}/`, so the
//! whole git layer works unchanged and nothing it writes can collide with a code repo or an image.
//! The namespaces are reserved owners, so no person can register `mdl` and reach these keys
//! through the git routes. Index markers are not written: hub listing is out of scope.
use crate::store::{valid_owner, valid_segment, Repo, Store, RESERVED_OWNERS};
use crate::{err, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HubKind {
    Model,
    Dataset,
}

impl HubKind {
    pub fn ns(self) -> &'static str {
        match self {
            HubKind::Model => "mdl",
            HubKind::Dataset => "ds",
        }
    }
    /// What precedes `{owner}/{name}` in a Hub URL.
    pub fn url_prefix(self) -> &'static str {
        match self {
            HubKind::Model => "",
            HubKind::Dataset => "datasets/",
        }
    }
    /// The `/api/{plural}/...` segment.
    pub fn api_plural(self) -> &'static str {
        match self {
            HubKind::Model => "models",
            HubKind::Dataset => "datasets",
        }
    }
    /// The `type` field of `/api/repos/create`; `space` and anything else is `None`.
    pub fn from_type(t: &str) -> Option<HubKind> {
        match t {
            "model" => Some(HubKind::Model),
            "dataset" => Some(HubKind::Dataset),
            _ => None,
        }
    }
}

pub fn hub_routing_key(kind: HubKind, owner: &str, name: &str) -> String {
    format!("{}/{owner}/{name}", kind.ns())
}

pub fn hub_pool_coords(kind: HubKind, owner: &str, name: &str) -> (&'static str, String) {
    (kind.ns(), format!("{owner}/{name}"))
}

fn valid(owner: &str, name: &str) -> bool {
    valid_owner(owner) && !RESERVED_OWNERS.contains(&owner) && valid_segment(name)
}

impl Store {
    pub async fn hub_create(&self, kind: HubKind, owner: &str, name: &str, private: bool) -> Result<()> {
        if !valid(owner, name) {
            return Err(err("invalid repo path"));
        }
        let (ns, n) = hub_pool_coords(kind, owner, name);
        self.create_repo_unchecked(ns, &n, false).await?;
        self.set_public(ns, &n, !private).await
    }

    pub async fn hub_open(&self, kind: HubKind, owner: &str, name: &str) -> Result<Option<Repo>> {
        if !valid(owner, name) {
            return Ok(None);
        }
        let (ns, n) = hub_pool_coords(kind, owner, name);
        self.open_repo_unchecked(ns, &n, false).await
    }

    pub async fn hub_exists(&self, kind: HubKind, owner: &str, name: &str) -> Result<bool> {
        if !valid(owner, name) {
            return Ok(false);
        }
        let (ns, n) = hub_pool_coords(kind, owner, name);
        self.repo_exists(ns, &n).await
    }

    pub async fn hub_delete(&self, kind: HubKind, owner: &str, name: &str) -> Result<()> {
        if !valid(owner, name) {
            return Err(err("invalid repo path"));
        }
        let (ns, n) = hub_pool_coords(kind, owner, name);
        self.delete_repo_unchecked(ns, &n, false).await
    }

    pub async fn hub_public(&self, kind: HubKind, owner: &str, name: &str) -> Result<bool> {
        if !valid(owner, name) {
            return Ok(false);
        }
        let (ns, n) = hub_pool_coords(kind, owner, name);
        self.repo_public(ns, &n).await
    }
}
```

   If `create_repo_unchecked` already errors on an existing repo, keep that; otherwise add `if self.repo_exists(ns, &n).await? { return Err(err("repository exists")); }` at the top of `hub_create`.
4. `crates/storage/src/lib.rs`: `pub mod hubrepo;` and `pub use hubrepo::{HubKind, hub_routing_key, hub_pool_coords};`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test -p kloudlite-storage 2>&1 | tail -20`
Expected: PASS, including every existing storage test (the split must not change git behaviour).

- [ ] **Step 5: Commit**

```bash
git add crates/storage/src
git commit -m "Store model and dataset repos in namespaces of their own"
```

---

### Task 2: crates/xet scaffold, CAS tokens and URL signing

**Files:**
- Modify: `Cargo.toml` (workspace members, default-members, `[workspace.dependencies]`: `xet-core-structures = "1.7.0"`, `hmac = "0.12"`, `sha2 = "0.10"` if absent)
- Create: `crates/xet/Cargo.toml`, `crates/xet/src/lib.rs`, `crates/xet/src/sign.rs`
- Modify: `crates/core/src/jwt.rs` (after `verify_registry`)
- Test: `crates/xet/src/sign.rs`, `crates/core/src/jwt.rs` tests module

**Interfaces:**
- Consumes: `kloudlite_core::jwt::Jwt`, `App { store, jwt, .. }`.
- Produces:
  - `pub struct CasClaims { pub owner: String, pub repo: String, pub rev: String, pub write: bool }`
  - `impl Jwt { pub fn mint_cas(&self, owner: &str, repo: &str, rev: &str, write: bool, ttl_secs: u64) -> Result<String>; pub fn verify_cas(&self, token: &str) -> Option<CasClaims> }` (`repo` is the routing key, e.g. `mdl/alice/bert`)
  - `crates/xet/src/sign.rs`: `pub fn sign(secret: &[u8], path: &str, exp: u64) -> String` (lowercase hex HMAC-SHA256 of `"{path}\n{exp}"`), `pub fn signed_url(base: &str, secret: &[u8], path: &str, ttl: u64) -> String` (`{base}{path}?exp={exp}&sig={sig}`), `pub fn verify(secret: &[u8], path: &str, exp: u64, sig: &str, now: u64) -> bool` (constant-time via `Mac::verify_slice`).
  - `pub fn cas_url() -> String` (`KLOUDLITE_CAS_URL`, default `http://localhost:8091`), `pub const MAX_XET_OBJECT: usize = 64 * 1024 * 1024`, `pub const MAX_XORB_CHUNKS: u32 = 8192`.
  - The signing secret: `App` exposes the jwt secret bytes. Add `pub fn secret(&self) -> &[u8]` to `Jwt` (store the secret bytes in a new `secret: Vec<u8>` field set in `Jwt::new`).

- [ ] **Step 1: Write the failing tests**

In `crates/core/src/jwt.rs` tests module (create `#[cfg(test)] mod cas_tests` if none):

```rust
#[cfg(test)]
mod cas_tests {
    use super::*;
    const S: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn cas_token_round_trips_and_is_typed() {
        let j = Jwt::new(S).unwrap();
        let t = j.mint_cas("alice", "mdl/alice/bert", "abc", true, 3600).unwrap();
        let c = j.verify_cas(&t).unwrap();
        assert_eq!((c.owner.as_str(), c.repo.as_str(), c.rev.as_str(), c.write), ("alice", "mdl/alice/bert", "abc", true));
        let reg = j.mint_registry("alice", "pull", 3600).unwrap();
        assert!(j.verify_cas(&reg).is_none(), "a registry token is not a CAS token");
        let other = Jwt::new("ffffffffffffffffffffffffffffffff").unwrap();
        assert!(other.verify_cas(&t).is_none());
    }

    #[test]
    fn expired_cas_token_is_refused() {
        let j = Jwt::new(S).unwrap();
        let mut v = jsonwebtoken::Validation::new(Algorithm::HS256);
        v.leeway = 0;
        let now = now().unwrap();
        let claims = serde_json::json!({"sub":"a","repo":"mdl/a/b","rev":"x","scope":"read","iat":now-7200,"exp":now-3600,"typ":"cas"});
        let t = encode(&Header::new(Algorithm::HS256), &claims, &j.encoding).unwrap();
        assert!(j.verify_cas(&t).is_none());
    }
}
```

In `crates/xet/src/sign.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    const K: &[u8] = b"0123456789abcdef0123456789abcdef";

    #[test]
    fn signature_binds_path_and_expiry() {
        let s = sign(K, "/v1/fetch/alice/abc", 100);
        assert!(verify(K, "/v1/fetch/alice/abc", 100, &s, 50));
        assert!(!verify(K, "/v1/fetch/alice/abc", 100, &s, 101), "expired");
        assert!(!verify(K, "/v1/fetch/bob/abc", 100, &s, 50), "other path");
        assert!(!verify(K, "/v1/fetch/alice/abc", 200, &s, 50), "moved expiry");
        assert!(!verify(K, "/v1/fetch/alice/abc", 100, "zz", 50), "not hex");
    }

    #[test]
    fn signed_url_shape() {
        let u = signed_url("https://cas.x", K, "/v1/fetch/a/h", 3600);
        assert!(u.starts_with("https://cas.x/v1/fetch/a/h?exp="));
        assert!(u.contains("&sig="));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test -p kloudlite-core cas_tests 2>&1 | tail -15`
Expected: FAIL, `no method named mint_cas`. (The xet crate does not exist yet, so its test cannot run until Step 3.)

- [ ] **Step 3: Implement**

`crates/core/src/jwt.rs`, after `verify_registry`:

```rust
    /// A CAS access token, handed to Xet clients by the hub's xet-read/write-token routes. It
    /// names the owner whose bytes it may touch (`sub`), the repo and revision it was minted for,
    /// and whether it may upload. The CAS checks `sub` against every key it reads or writes; the
    /// repo and revision are recorded for the logs, not enforced (dedup is per owner, not per repo).
    pub fn mint_cas(&self, owner: &str, repo: &str, rev: &str, write: bool, ttl_secs: u64) -> Result<String> {
        let now = now()?;
        let claims = serde_json::json!({
            "sub": owner,
            "repo": repo,
            "rev": rev,
            "scope": if write { "write" } else { "read" },
            "iat": now,
            "exp": now + ttl_secs,
            "typ": "cas",
        });
        encode(&Header::new(Algorithm::HS256), &claims, &self.encoding)
            .map_err(|e| err(format!("minting cas token: {e}")))
    }

    pub fn verify_cas(&self, token: &str) -> Option<CasClaims> {
        let mut v = Validation::new(Algorithm::HS256);
        v.set_required_spec_claims(&["exp", "sub"]);
        let data = decode::<serde_json::Value>(token, &self.decoding, &v).ok()?;
        let c = &data.claims;
        if c["typ"] != "cas" {
            return None;
        }
        Some(CasClaims {
            owner: c["sub"].as_str()?.to_string(),
            repo: c["repo"].as_str()?.to_string(),
            rev: c["rev"].as_str()?.to_string(),
            write: c["scope"] == "write",
        })
    }

    /// The raw secret, for HMAC-signed URLs that must verify on every node.
    pub fn secret(&self) -> &[u8] {
        &self.secret
    }
```

Add `pub struct CasClaims { pub owner: String, pub repo: String, pub rev: String, pub write: bool }` near `Claims`, add `secret: Vec<u8>` to `Jwt`, and set `secret: secret.as_bytes().to_vec()` in `Jwt::new`.

`crates/xet/Cargo.toml` (copy the `[package]` shape, edition and lints from `crates/registry/Cargo.toml`):

```toml
[package]
name = "kloudlite-xet"
version = "0.1.0"
edition = "2021"

[dependencies]
kloudlite-core = { path = "../core" }
kloudlite-storage = { path = "../storage" }
axum = { workspace = true }
bytes = { workspace = true }
hex = { workspace = true }
hmac = { workspace = true }
sha2 = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
slatedb = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
futures = { workspace = true }
xet-core-structures = { workspace = true }

[dev-dependencies]
tempfile = { workspace = true }
```

Match each `workspace = true` against what the root `Cargo.toml` actually declares; for any dependency the workspace does not declare, copy the version `crates/registry/Cargo.toml` uses.

`crates/xet/src/lib.rs`:

```rust
//! The Xet content-addressed store behind the model and dataset hub.
//!
//! Clients (huggingface_hub through hf-xet, git-xet) chunk files themselves and upload two kinds
//! of object: xorbs (up to 8192 compressed chunks, ≤ 64 MiB) and shards (which files are made of
//! which chunk ranges). We keep xorbs, a small `.idx` per xorb holding its chunk boundaries, and
//! one reconstruction record per file. Shards are parsed and dropped. Everything is keyed per
//! owner (`xet/{owner}/...`): dedup is per owner, and the GC sweep reads one owner at a time.
//!
//! Authorization is a CAS token (`Jwt::mint_cas`), minted by the hub for one owner. Byte fetches
//! carry no token: the reconstruction answer hands out HMAC-signed URLs (`sign`), which is what
//! lets a client fetch terms in parallel without re-authenticating.
//!
//! Module map: `sign` (URL signing), `keys` (object keys), `xorbs` (upload and `.idx`), `shards`
//! (shard ingest), `recon` (reconstruction and fetch), `lfs` (git-lfs byte routes), `routes`.
#![deny(clippy::unwrap_used, clippy::expect_used)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
#![allow(clippy::result_large_err)]

pub mod sign;

pub const MAX_XET_OBJECT: usize = 64 * 1024 * 1024;
pub const MAX_XORB_CHUNKS: u32 = 8192;

/// The CAS base URL clients are told to use. Configuration, not derived from the request: the
/// client must reach it from outside.
pub fn cas_url() -> String {
    std::env::var("KLOUDLITE_CAS_URL").unwrap_or_else(|_| "http://localhost:8091".into())
}
```

`crates/xet/src/sign.rs`:

```rust
//! HMAC-signed byte URLs. The reconstruction answer and the LFS batch both hand clients URLs they
//! fetch without a token; the signature binds the path and the expiry, so a URL is good for one
//! object, for one hour, and verifies on any node (the key is the cluster's jwt secret).
use hmac::{Hmac, Mac};
use sha2::Sha256;

type H = Hmac<Sha256>;

fn mac(secret: &[u8], path: &str, exp: u64) -> H {
    #[allow(clippy::expect_used)] // HMAC accepts any key length
    let mut m = H::new_from_slice(secret).expect("hmac takes any key");
    m.update(path.as_bytes());
    m.update(b"\n");
    m.update(exp.to_string().as_bytes());
    m
}

pub fn sign(secret: &[u8], path: &str, exp: u64) -> String {
    hex::encode(&mac(secret, path, exp).finalize().into_bytes())
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn signed_url(base: &str, secret: &[u8], path: &str, ttl: u64) -> String {
    let exp = now() + ttl;
    format!("{base}{path}?exp={exp}&sig={}", sign(secret, path, exp))
}

pub fn verify(secret: &[u8], path: &str, exp: u64, sig: &str, now: u64) -> bool {
    if now > exp {
        return false;
    }
    let Ok(raw) = hex::decode(sig) else { return false };
    mac(secret, path, exp).verify_slice(&raw).is_ok()
}
```

Hex goes through the workspace `hex` crate directly (`hex::encode`, `hex::decode`); `kloudlite_core::hex` is only an encode function, so there is nothing to add in core.

Root `Cargo.toml`: add `"crates/xet"` to `members` and `default-members`; add `xet-core-structures = "1.7.0"`, `hmac = "0.12"`, and `sha2 = "0.10"` (if absent) to `[workspace.dependencies]`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test -p kloudlite-core cas_tests 2>&1 | tail -10 && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test -p kloudlite-xet 2>&1 | tail -10`
Expected: PASS both.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/core/src/jwt.rs crates/xet
git commit -m "Add the Xet CAS crate with CAS tokens and signed URLs"
```

---

### Task 3: Xorb upload and chunk index

**Files:**
- Create: `crates/xet/src/keys.rs`, `crates/xet/src/xorbs.rs`, `crates/xet/src/testkit.rs`
- Modify: `crates/xet/src/lib.rs` (modules)
- Test: `crates/xet/src/xorbs.rs`

**Interfaces:**
- Consumes: `MAX_XET_OBJECT`, `MAX_XORB_CHUNKS`; `slatedb::object_store::{ObjectStore, PutMode, PutPayload, path::Path}`.
- Produces:
  - `keys.rs`: `pub fn parse_hash(s: &str) -> Option<MerkleHash>` (exactly 64 lowercase hex, then `MerkleHash::from_hex`); `pub fn sha_hex(h: &DataHash) -> String` (true digest hex from `as_bytes()`); `pub fn xorb(owner, &MerkleHash) -> Path` → `xet/{owner}/xorbs/{hex}`; `pub fn xorb_idx(owner, &MerkleHash) -> Path` → `…/{hex}.idx`; `pub fn file(owner, &MerkleHash) -> Path` → `xet/{owner}/files/{hex}`; `pub fn sha(owner, sha_hex: &str) -> Path` → `xet/{owner}/sha/{sha_hex}`; `pub fn lfs(owner, sha_hex) -> Path` → `lfs/{owner}/sha256/{sha_hex}`.
  - `xorbs.rs`: `#[derive(Serialize, Deserialize)] pub struct XorbIdx { pub chunk_hashes: Vec<String>, pub boundaries: Vec<u32>, pub unpacked: Vec<u32> }` (hex hashes; `boundaries[i]`/`unpacked[i]` are cumulative END offsets of chunk i); `pub enum Ingest { Inserted, Existed }`; `pub async fn put_xorb(os: &dyn ObjectStore, owner: &str, hash: &MerkleHash, body: Bytes) -> Result<Ingest, String>` (Err = 400 message); `pub async fn read_idx(os, owner, &MerkleHash) -> crate::Result<Option<XorbIdx>>`.
  - `testkit.rs` (`#[cfg(test)]` and `pub` behind a `testkit` feature for `tests/`): `pub struct Built { pub xorb: Vec<u8>, pub shard: Vec<u8>, pub xorb_hash: MerkleHash, pub file_hash: MerkleHash, pub sha256: [u8; 32], pub bytes: Vec<u8> }`, `pub fn build(pieces: &[&[u8]]) -> Built`.

- [ ] **Step 1: Write the failing test**

`crates/xet/src/testkit.rs` (the fixture builder; this is test support, written first):

```rust
//! Builds a real xorb and shard the way hf-xet does, for tests. Kept beside the code because the
//! integration tests in `tests/` use it too (feature `testkit`).
use bytes::Bytes;
use sha2::{Digest, Sha256};
use xet_core_structures::merklehash::{file_hash, DataHash, MerkleHash};
use xet_core_structures::metadata_shard::chunk_verification::range_hash_from_chunks;
use xet_core_structures::metadata_shard::file_structs::{
    FileDataSequenceEntry, FileDataSequenceHeader, FileMetadataExt, FileVerificationEntry, MDBFileInfo,
};
use xet_core_structures::metadata_shard::shard_in_memory::MDBInMemoryShard;
use xet_core_structures::xorb_object::{Chunk, RawXorbData, SerializedXorbObject};

pub struct Built {
    pub xorb: Vec<u8>,
    pub shard: Vec<u8>,
    pub xorb_hash: MerkleHash,
    pub file_hash: MerkleHash,
    pub sha256: [u8; 32],
    pub bytes: Vec<u8>,
}

#[allow(clippy::unwrap_used)]
pub fn build(pieces: &[&[u8]]) -> Built {
    let chunks: Vec<Chunk> = pieces.iter().map(|p| Chunk::new(Bytes::copy_from_slice(p))).collect();
    let raw = RawXorbData::from_chunks(&chunks, vec![0]);
    let xorb_hash = raw.hash();
    let xorb_info = raw.xorb_info.clone();
    let xorb = SerializedXorbObject::from_xorb(raw, true, "none", 0).unwrap();
    let hl: Vec<(MerkleHash, u64)> = chunks.iter().map(|c| (c.hash, c.data.len() as u64)).collect();
    let fh = file_hash(&hl);
    let total: u32 = hl.iter().map(|(_, l)| *l as u32).sum();
    let n: u32 = chunks.len() as u32;
    let bytes: Vec<u8> = pieces.concat();
    let sha256: [u8; 32] = Sha256::digest(&bytes).into();
    let ranged = range_hash_from_chunks(&chunks.iter().map(|c| c.hash).collect::<Vec<_>>());
    let info = MDBFileInfo {
        metadata: FileDataSequenceHeader::new(fh, 1u32, true, true),
        segments: vec![FileDataSequenceEntry::new(xorb_hash, total, 0u32, n)],
        verification: vec![FileVerificationEntry::new(ranged)],
        metadata_ext: Some(FileMetadataExt::new(DataHash::from(sha256))),
    };
    let mut shard = MDBInMemoryShard::default();
    shard.add_xorb_block(xorb_info).unwrap();
    shard.add_file_reconstruction_info(info).unwrap();
    Built { xorb: xorb.serialized_data, shard: shard.to_bytes().unwrap(), xorb_hash, file_hash: fh, sha256, bytes }
}
```

If `MDBXorbInfo` is not `Clone`, call `RawXorbData::from_chunks` a second time for the info.

`crates/xet/src/xorbs.rs` tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::build;
    use slatedb::object_store::memory::InMemory;

    #[tokio::test]
    async fn valid_xorb_is_stored_once_with_its_index() {
        let os = InMemory::new();
        let b = build(&[b"hello ", b"world"]);
        let r = put_xorb(&os, "alice", &b.xorb_hash, Bytes::from(b.xorb.clone())).await.unwrap();
        assert!(matches!(r, Ingest::Inserted));
        let r = put_xorb(&os, "alice", &b.xorb_hash, Bytes::from(b.xorb.clone())).await.unwrap();
        assert!(matches!(r, Ingest::Existed));
        let idx = read_idx(&os, "alice", &b.xorb_hash).await.unwrap().unwrap();
        assert_eq!(idx.chunk_hashes.len(), 2);
        assert_eq!(idx.unpacked, vec![6, 11]);
        assert!(read_idx(&os, "bob", &b.xorb_hash).await.unwrap().is_none(), "per owner");
    }

    #[tokio::test]
    async fn tampered_xorb_is_refused_and_original_kept() {
        let os = InMemory::new();
        let b = build(&[b"hello ", b"world"]);
        let mut bad = b.xorb.clone();
        bad[10] ^= 0xff;
        assert!(put_xorb(&os, "alice", &b.xorb_hash, Bytes::from(bad.clone())).await.is_err());
        put_xorb(&os, "alice", &b.xorb_hash, Bytes::from(b.xorb.clone())).await.unwrap();
        assert!(put_xorb(&os, "alice", &b.xorb_hash, Bytes::from(bad)).await.is_err());
        let got = os.get(&crate::keys::xorb("alice", &b.xorb_hash)).await.unwrap().bytes().await.unwrap();
        assert_eq!(&got[..], &b.xorb[..]);
    }

    #[tokio::test]
    async fn wrong_hash_and_oversize_are_refused() {
        let os = InMemory::new();
        let b = build(&[b"abc"]);
        let other = build(&[b"xyz"]).xorb_hash;
        assert!(put_xorb(&os, "alice", &other, Bytes::from(b.xorb.clone())).await.is_err());
        let big = Bytes::from(vec![0u8; crate::MAX_XET_OBJECT + 1]);
        assert!(put_xorb(&os, "alice", &b.xorb_hash, big).await.is_err());
    }

    #[test]
    fn hash_parsing_is_strict() {
        let b = build(&[b"abc"]);
        let hex = b.xorb_hash.hex();
        assert!(crate::keys::parse_hash(&hex).is_some());
        assert!(crate::keys::parse_hash(&hex.to_uppercase()).is_none());
        assert!(crate::keys::parse_hash("../x").is_none());
        assert!(crate::keys::parse_hash(&hex[..63]).is_none());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test -p kloudlite-xet xorbs 2>&1 | tail -15`
Expected: FAIL, unresolved `put_xorb`.

- [ ] **Step 3: Implement**

`crates/xet/src/keys.rs`:

```rust
//! Object keys. `parse_hash` is the only way a path segment becomes part of a key, like
//! `Digest::parse` for the registry: 64 lowercase hex, nothing else.
use slatedb::object_store::path::Path;
use xet_core_structures::merklehash::{DataHash, MerkleHash};

pub fn parse_hash(s: &str) -> Option<MerkleHash> {
    if s.len() != 64 || !s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return None;
    }
    MerkleHash::from_hex(s).ok()
}

/// The ordinary sha256 hex of a digest stored raw in a `DataHash` (its `hex()` reverses bytes).
pub fn sha_hex(h: &DataHash) -> String {
    hex::encode(h.as_bytes())
}

pub fn is_sha_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn xorb(owner: &str, h: &MerkleHash) -> Path {
    Path::from(format!("xet/{owner}/xorbs/{}", h.hex()))
}
pub fn xorb_idx(owner: &str, h: &MerkleHash) -> Path {
    Path::from(format!("xet/{owner}/xorbs/{}.idx", h.hex()))
}
pub fn file(owner: &str, h: &MerkleHash) -> Path {
    Path::from(format!("xet/{owner}/files/{}", h.hex()))
}
pub fn sha(owner: &str, sha_hex: &str) -> Path {
    Path::from(format!("xet/{owner}/sha/{sha_hex}"))
}
pub fn lfs(owner: &str, sha_hex: &str) -> Path {
    Path::from(format!("lfs/{owner}/sha256/{sha_hex}"))
}
```

`crates/xet/src/xorbs.rs`:

```rust
//! Xorb upload. A xorb is validated end to end before it is stored: `validate_xorb_object`
//! recomputes every chunk hash and the xorb hash from the bytes, so the key always names its
//! content. Limits are ours to enforce (the library only debug-asserts them). Beside each xorb we
//! keep a small JSON `.idx` of chunk hashes and boundaries: shard ingest checks term ranges and
//! verification hashes against it, and reconstruction turns chunk ranges into byte ranges with
//! it, so neither has to read a 64 MiB object.
use crate::keys;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use slatedb::object_store::{ObjectStore, ObjectStoreExt, PutMode, PutOptions, PutPayload};
use xet_core_structures::merklehash::MerkleHash;
use xet_core_structures::xorb_object::validate_xorb_object;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct XorbIdx {
    pub chunk_hashes: Vec<String>,
    pub boundaries: Vec<u32>,
    pub unpacked: Vec<u32>,
}

pub enum Ingest {
    Inserted,
    Existed,
}

pub async fn put_xorb(os: &dyn ObjectStore, owner: &str, hash: &MerkleHash, body: Bytes) -> Result<Ingest, String> {
    if body.len() > crate::MAX_XET_OBJECT {
        return Err("xorb larger than 64 MiB".into());
    }
    let mut cur = std::io::Cursor::new(&body[..]);
    let x = match validate_xorb_object(&mut cur, hash) {
        Ok(Some(x)) => x,
        Ok(None) => return Err("xorb does not validate against its hash".into()),
        Err(e) => return Err(format!("xorb unreadable: {e}")),
    };
    if x.info.num_chunks > crate::MAX_XORB_CHUNKS {
        return Err("xorb has more than 8192 chunks".into());
    }
    let idx = XorbIdx {
        chunk_hashes: x.info.chunk_hashes.iter().map(|h| h.hex()).collect(),
        boundaries: x.info.chunk_boundary_offsets.clone(),
        unpacked: x.info.unpacked_chunk_offsets.clone(),
    };
    let idx_bytes = serde_json::to_vec(&idx).map_err(|e| e.to_string())?;
    // Index first: a xorb without its index is invisible to shard ingest, so a crash between the
    // two writes leaves a harmless orphan for GC rather than a xorb nothing can use.
    let create = PutOptions { mode: PutMode::Create, ..Default::default() };
    match os.put_opts(&keys::xorb_idx(owner, hash), PutPayload::from(idx_bytes), create.clone()).await {
        Ok(_) | Err(slatedb::object_store::Error::AlreadyExists { .. }) => {}
        Err(e) => return Err(format!("storing index: {e}")),
    }
    match os.put_opts(&keys::xorb(owner, hash), PutPayload::from(body), create).await {
        Ok(_) => Ok(Ingest::Inserted),
        Err(slatedb::object_store::Error::AlreadyExists { .. }) => Ok(Ingest::Existed),
        Err(e) => Err(format!("storing xorb: {e}")),
    }
}

pub async fn read_idx(os: &dyn ObjectStore, owner: &str, hash: &MerkleHash) -> crate::Result<Option<XorbIdx>> {
    match os.get(&keys::xorb_idx(owner, hash)).await {
        Ok(r) => {
            let b = r.bytes().await?;
            Ok(Some(serde_json::from_slice(&b).map_err(|e| crate::err(format!("xorb index: {e}")))?))
        }
        Err(slatedb::object_store::Error::NotFound { .. }) => Ok(None),
        Err(e) => Err(e.into()),
    }
}
```

Storage errors in `put_xorb` are returned as `Err(String)` too; the route (Task 5) maps a message starting `storing` to 500 and everything else to 400. Add to `lib.rs`: `pub mod keys; pub mod xorbs; #[cfg(any(test, feature = "testkit"))] pub mod testkit;`, `pub(crate) use kloudlite_core::{err, Error, Result};`, and in `Cargo.toml` `[features] testkit = []`. Check `InMemory` supports `PutMode::Create` (it does in object_store ≥ 0.9; if the vendored version differs, read `~/.cargo/registry/src/*/object_store-*/src/memory.rs`).

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test -p kloudlite-xet 2>&1 | tail -15`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/xet
git commit -m "Validate and store xorbs with a chunk index"
```

---

### Task 4: Shard ingest

**Files:**
- Create: `crates/xet/src/shards.rs`
- Modify: `crates/xet/src/lib.rs` (`pub mod shards;`)
- Test: `crates/xet/src/shards.rs`

**Interfaces:**
- Consumes: `keys::*`, `xorbs::{read_idx, XorbIdx}`, `testkit::build`.
- Produces:
  - `#[derive(Serialize, Deserialize, Clone)] pub struct FileRecord { pub size: u64, pub sha256: String, pub terms: Vec<Term> }` and `pub struct Term { pub xorb: String, pub start: u32, pub end: u32, pub unpacked: u32 }` (`end` exclusive), stored as JSON at `xet/{owner}/files/{file_hash}`.
  - `pub async fn put_shard(os: &dyn ObjectStore, owner: &str, body: &[u8]) -> Result<bool, String>`: `Ok(true)` when at least one file record was new (`{result:1}`), `Ok(false)` when all existed (`{result:0}`), `Err(msg)` = 400 naming the first failure.
  - `pub async fn read_file(os, owner, &MerkleHash) -> crate::Result<Option<FileRecord>>`.
  - `xet/{owner}/sha/{sha256}` holds the file hash hex as its body.

- [ ] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::build;
    use crate::xorbs::put_xorb;
    use bytes::Bytes;
    use slatedb::object_store::memory::InMemory;

    #[tokio::test]
    async fn shard_records_file_and_sha() {
        let os = InMemory::new();
        let b = build(&[b"hello ", b"world"]);
        put_xorb(&os, "alice", &b.xorb_hash, Bytes::from(b.xorb.clone())).await.unwrap();
        assert!(put_shard(&os, "alice", &b.shard).await.unwrap());
        assert!(!put_shard(&os, "alice", &b.shard).await.unwrap(), "second time exists");
        let f = read_file(&os, "alice", &b.file_hash).await.unwrap().unwrap();
        assert_eq!(f.size, 11);
        assert_eq!(f.sha256, hex::encode(&b.sha256));
        assert_eq!(f.terms.len(), 1);
        let sha = os.get(&crate::keys::sha("alice", &f.sha256)).await.unwrap().bytes().await.unwrap();
        assert_eq!(&sha[..], b.file_hash.hex().as_bytes());
    }

    #[tokio::test]
    async fn shard_with_missing_xorb_is_refused() {
        let os = InMemory::new();
        let b = build(&[b"abc"]);
        let e = put_shard(&os, "alice", &b.shard).await.unwrap_err();
        assert!(e.contains(&b.xorb_hash.hex()), "{e}");
    }

    #[tokio::test]
    async fn shard_cannot_reference_another_owners_xorb() {
        let os = InMemory::new();
        let b = build(&[b"abc"]);
        put_xorb(&os, "bob", &b.xorb_hash, Bytes::from(b.xorb.clone())).await.unwrap();
        assert!(put_shard(&os, "alice", &b.shard).await.is_err());
        assert!(read_file(&os, "alice", &b.file_hash).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn garbage_shard_is_refused() {
        let os = InMemory::new();
        assert!(put_shard(&os, "alice", b"not a shard").await.is_err());
    }
}
```

Also add a test that a shard without the sha256 extension is refused: build it by copying `testkit::build` inline with `FileDataSequenceHeader::new(fh, 1u32, true, false)` and `metadata_ext: None`, upload the xorb, and assert `put_shard(...).unwrap_err().contains("sha256")`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test -p kloudlite-xet shards 2>&1 | tail -15`
Expected: FAIL, unresolved `put_shard`.

- [ ] **Step 3: Implement**

```rust
//! Shard ingest. A shard says "file F is these chunk ranges of these xorbs". We check every claim
//! against what we hold FOR THIS OWNER before recording anything: each xorb exists, each range is
//! inside it, and each verification hash equals the hash of the chunk hashes in that range (which
//! is what stops a client claiming a file made of someone else's chunks it never had). The shard
//! itself is not stored; one JSON record per file is, plus a sha256 → file hash pointer the hub
//! uses to accept `lfsFile` commits and LFS batch answers.
//!
//! ponytail: the declared sha256 is trusted, not recomputed (that would mean decompressing every
//! byte of every file on upload). A wrong sha256 only misleads its own uploader's pointer files;
//! recompute in the GC sweep if a client is ever caught lying.
use crate::keys;
use crate::xorbs::read_idx;
use serde::{Deserialize, Serialize};
use slatedb::object_store::{ObjectStore, ObjectStoreExt, PutPayload};
use xet_core_structures::merklehash::MerkleHash;
use xet_core_structures::metadata_shard::chunk_verification::range_hash_from_chunks;
use xet_core_structures::metadata_shard::streaming_shard::MDBMinimalShard;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Term {
    pub xorb: String,
    pub start: u32,
    pub end: u32,
    pub unpacked: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FileRecord {
    pub size: u64,
    pub sha256: String,
    pub terms: Vec<Term>,
}

pub async fn put_shard(os: &dyn ObjectStore, owner: &str, body: &[u8]) -> Result<bool, String> {
    let shard = MDBMinimalShard::from_reader(&mut std::io::Cursor::new(body), true, false)
        .map_err(|e| format!("shard unreadable: {e}"))?;
    if shard.num_files() > 0 && !shard.has_file_verification() {
        return Err("shard carries no verification entries".into());
    }
    let mut records = Vec::new();
    for i in 0..shard.num_files() {
        let Some(f) = shard.file(i) else { return Err(format!("file {i} unreadable")) };
        let fh = f.file_hash();
        let Some(ext) = f.metadata_ext() else {
            return Err(format!("file {} has no sha256 extension", fh.hex()));
        };
        let mut terms = Vec::new();
        let mut size = 0u64;
        for j in 0..f.num_entries() {
            let e = f.entry(j);
            let Some(idx) = read_idx(os, owner, &e.xorb_hash).await.map_err(|e| format!("storing: {e}"))? else {
                return Err(format!("xorb {} not uploaded", e.xorb_hash.hex()));
            };
            let (s, t) = (e.chunk_index_start as usize, e.chunk_index_end as usize);
            if s >= t || t > idx.chunk_hashes.len() {
                return Err(format!("range {s}..{t} outside xorb {}", e.xorb_hash.hex()));
            }
            let hashes: Option<Vec<MerkleHash>> =
                idx.chunk_hashes[s..t].iter().map(|h| MerkleHash::from_hex(h).ok()).collect();
            let hashes = hashes.ok_or("xorb index corrupt")?;
            if range_hash_from_chunks(&hashes) != f.verification(j).range_hash {
                return Err(format!("verification hash mismatch for file {}", fh.hex()));
            }
            let unpacked = idx.unpacked[t - 1] - if s == 0 { 0 } else { idx.unpacked[s - 1] };
            if unpacked != e.unpacked_segment_bytes {
                return Err(format!("segment length mismatch for file {}", fh.hex()));
            }
            size += unpacked as u64;
            terms.push(Term { xorb: e.xorb_hash.hex(), start: s as u32, end: t as u32, unpacked });
        }
        records.push((fh, FileRecord { size, sha256: keys::sha_hex(&ext.sha256), terms }));
    }
    // Every claim checked; now write. A file record is written before its sha pointer, so a
    // pointer never names a file we cannot reconstruct.
    let mut new = false;
    for (fh, rec) in records {
        let key = keys::file(owner, &fh);
        if os.head(&key).await.is_ok() {
            continue;
        }
        let body = serde_json::to_vec(&rec).map_err(|e| e.to_string())?;
        os.put(&key, PutPayload::from(body)).await.map_err(|e| format!("storing: {e}"))?;
        os.put(&keys::sha(owner, &rec.sha256), PutPayload::from(fh.hex().into_bytes()))
            .await
            .map_err(|e| format!("storing: {e}"))?;
        new = true;
    }
    Ok(new)
}

pub async fn read_file(os: &dyn ObjectStore, owner: &str, fh: &MerkleHash) -> crate::Result<Option<FileRecord>> {
    match os.get(&keys::file(owner, fh)).await {
        Ok(r) => {
            let b = r.bytes().await?;
            Ok(Some(serde_json::from_slice(&b).map_err(|e| crate::err(format!("file record: {e}")))?))
        }
        Err(slatedb::object_store::Error::NotFound { .. }) => Ok(None),
        Err(e) => Err(e.into()),
    }
}
```

Verify the exact paths (`metadata_shard::streaming_shard::MDBMinimalShard`, `f.entry(j)` returning a value vs `Option`, `f.verification(j)` shape) in `~/.cargo/registry/src/*/xet-core-structures-1.7.0/src/metadata_shard/` and adjust; the facts sheet says `entry(idx)` and `verification(idx)` return values.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test -p kloudlite-xet 2>&1 | tail -15`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/xet
git commit -m "Ingest shards after checking every term against the owner's xorbs"
```

---

### Task 5: CAS reconstruction and routes

**Files:**
- Create: `crates/xet/src/recon.rs`, `crates/xet/src/routes.rs`
- Modify: `crates/xet/src/lib.rs` (`pub mod recon; pub mod routes; pub use routes::{router, CasState};`), `crates/xet/Cargo.toml` (`[dev-dependencies]` add `tower = { workspace = true }`)
- Test: `crates/xet/src/recon.rs`, `crates/xet/src/routes.rs`

**Interfaces:**
- Consumes: `keys::{parse_hash, xorb, xorb_idx}`, `xorbs::{put_xorb, read_idx, Ingest, XorbIdx}`, `shards::{put_shard, read_file, FileRecord, Term}`, `sign::{signed_url, verify, now}`, `Jwt::{verify_cas, secret}`, `CasClaims`, `MAX_XET_OBJECT`, `kloudlite_core::httpx::scheme`.
- Produces:
  - `recon.rs`: `pub fn parse_range(h: Option<&str>) -> Result<Option<(u64, Option<u64>)>, ()>` (`bytes=S-E` inclusive, `bytes=S-` open; anything else `Err`); `pub async fn reconstruct(os: &dyn ObjectStore, owner: &str, file: &FileRecord, range: Option<(u64, Option<u64>)>, url_for: &dyn Fn(&str) -> String) -> crate::Result<Option<serde_json::Value>>` (`Ok(None)` = range starts at or past the end, a 416).
  - `routes.rs`: `#[derive(Clone)] pub struct CasState { pub os: Arc<dyn ObjectStore>, pub jwt: Arc<Jwt>, pub base: String }` (`base` = `cas_url()`, no trailing slash); `pub fn router(st: CasState) -> Router` (state already applied).
  - Routes: `POST /v1/xorbs/default/{hash}` → `{"was_inserted":bool}`; `POST /v1/shards` → `{"result":0|1}`; `GET /v1/reconstructions/{hash}` (optional `Range`) → v1 `QueryReconstructionResponse` JSON; `GET /v1/reconstructions?file_id=..&file_id=..` → `{"files":{hex:[terms]},"fetch_info":{..}}`; `GET /v1/fetch/{owner}/{xorb}?exp=&sig=` (presigned, no bearer, honours `Range`, 206 when ranged). Everything else (`/v2/*`, `/v1/chunks/*`) is 404 — hf-xet falls back from v2 to v1 on exactly 404, and a chunk-query 404 means "no global dedup", which is the ruling.
  - Owner for every bearer route comes from the token's `owner`, never the URL.

- [ ] **Step 1: Write the failing tests**

`crates/xet/src/recon.rs`, tests module at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::shards::Term;
    use crate::xorbs::XorbIdx;
    use serde_json::json;
    use slatedb::object_store::memory::InMemory;

    #[test]
    fn parse_range_shapes() {
        assert_eq!(parse_range(None), Ok(None));
        assert_eq!(parse_range(Some("bytes=0-0")), Ok(Some((0, Some(0)))));
        assert_eq!(parse_range(Some("bytes=7-")), Ok(Some((7, None))));
        assert!(parse_range(Some("bytes=5-4")).is_err());
        assert!(parse_range(Some("bytes=-5")).is_err());
        assert!(parse_range(Some("items=0-1")).is_err());
        assert!(parse_range(Some("bytes=0-1,4-5")).is_err());
    }

    #[tokio::test]
    async fn range_skips_terms_and_sets_offset() {
        let os = InMemory::new();
        let (h1, h2) = ("1".repeat(64), "2".repeat(64));
        for (h, b) in [(&h1, vec![4u32, 8]), (&h2, vec![5, 9, 14])] {
            let idx = XorbIdx { chunk_hashes: vec![], boundaries: b.clone(), unpacked: b };
            let key = crate::keys::xorb_idx("alice", &crate::keys::parse_hash(h).unwrap());
            os.put(&key, serde_json::to_vec(&idx).unwrap().into()).await.unwrap();
        }
        let f = FileRecord {
            size: 30,
            sha256: String::new(),
            terms: vec![
                Term { xorb: h1.clone(), start: 0, end: 2, unpacked: 10 },
                Term { xorb: h2.clone(), start: 1, end: 3, unpacked: 20 },
            ],
        };
        let url = |x: &str| format!("u/{x}");
        let v = reconstruct(&os, "alice", &f, Some((12, Some(15))), &url).await.unwrap().unwrap();
        assert_eq!(v["offset_into_first_range"], 2);
        assert_eq!(v["terms"].as_array().unwrap().len(), 1);
        assert_eq!(v["terms"][0]["hash"], h2);
        assert_eq!(v["terms"][0]["range"], json!({"start": 1, "end": 3}));
        assert_eq!(v["fetch_info"][&h2][0]["url_range"], json!({"start": 5, "end": 13}));
        assert_eq!(v["fetch_info"][&h2][0]["url"], format!("u/{h2}"));
        assert!(v["fetch_info"].get(&h1).is_none());

        let v = reconstruct(&os, "alice", &f, None, &url).await.unwrap().unwrap();
        assert_eq!(v["terms"].as_array().unwrap().len(), 2);
        assert_eq!(v["fetch_info"][&h1][0]["url_range"], json!({"start": 0, "end": 7}));

        // Last byte exactly, and one past it.
        let v = reconstruct(&os, "alice", &f, Some((29, None)), &url).await.unwrap().unwrap();
        assert_eq!(v["offset_into_first_range"], 19);
        assert!(reconstruct(&os, "alice", &f, Some((30, None)), &url).await.unwrap().is_none());
    }
}
```

`crates/xet/src/routes.rs`, tests module at the bottom:

```rust
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test -p kloudlite-xet recon routes 2>&1 | tail -20`
Expected: compile errors, `cannot find function reconstruct` / `cannot find function router`.

- [ ] **Step 3: Implement**

`crates/xet/src/recon.rs` (above the tests module):

```rust
//! Reconstruction: how a client turns a file hash into byte ranges of xorbs.
//!
//! The answer is hf-xet's v1 `QueryReconstructionResponse` (field names literal snake_case,
//! hashes in `MerkleHash::hex()` form). hf-xet asks v2 first and falls back to v1 on a 404,
//! which is why there is no `/v2/reconstructions` route. Units: the client's `Range` header and
//! every `url_range` are INCLUSIVE at the end; chunk ranges are exclusive. Getting one of those
//! wrong is an off-by-one that only shows on the last byte of a file.

use crate::keys;
use crate::shards::FileRecord;
use crate::xorbs::read_idx;
use serde_json::{json, Map, Value};
use slatedb::object_store::ObjectStore;

/// `bytes=S-E` or `bytes=S-` → `(S, Some(E) | None)`; no header → whole file. Suffix ranges
/// (`bytes=-N`) and multi-range are refused: hf-xet's v1 path never sends them.
pub fn parse_range(h: Option<&str>) -> Result<Option<(u64, Option<u64>)>, ()> {
    let Some(h) = h else { return Ok(None) };
    let (s, e) = h.strip_prefix("bytes=").and_then(|r| r.split_once('-')).ok_or(())?;
    let s: u64 = s.parse().map_err(|_| ())?;
    let e = if e.is_empty() { None } else { Some(e.parse::<u64>().map_err(|_| ())?) };
    match e {
        Some(e) if e < s => Err(()),
        _ => Ok(Some((s, e))),
    }
}

/// The terms of `file` that overlap `range`, each with where to fetch its bytes. Whole terms
/// only: the client trims the first by `offset_into_first_range` and the last by the length it
/// asked for. `url_for(xorb_hex)` makes the presigned fetch URL.
pub async fn reconstruct(
    os: &dyn ObjectStore,
    owner: &str,
    file: &FileRecord,
    range: Option<(u64, Option<u64>)>,
    url_for: &dyn Fn(&str) -> String,
) -> crate::Result<Option<Value>> {
    let (start, end) = match range {
        None => (0, file.size.saturating_sub(1)),
        Some((s, _)) if s >= file.size => return Ok(None),
        Some((s, e)) => (s, e.unwrap_or(u64::MAX).min(file.size - 1)),
    };
    let mut terms = Vec::new();
    let mut fetch = Map::new();
    let (mut offset, mut pos) = (0u64, 0u64);
    for t in &file.terms {
        let (lo, hi) = (pos, pos + u64::from(t.unpacked));
        pos = hi;
        if hi <= start || lo > end {
            continue;
        }
        if terms.is_empty() {
            offset = start - lo;
        }
        terms.push(json!({"hash": t.xorb, "unpacked_length": t.unpacked, "range": {"start": t.start, "end": t.end}}));
        let hash = keys::parse_hash(&t.xorb).ok_or_else(|| crate::err(format!("file record names bad xorb {}", t.xorb)))?;
        let idx = read_idx(os, owner, &hash)
            .await?
            .ok_or_else(|| crate::err(format!("xorb {} has a file record but no index", t.xorb)))?;
        // `boundaries[i]` is the END of chunk i in the serialized xorb; shard ingest checked the
        // range against it, so a miss here is a corrupt index, reported rather than panicked on.
        let at = |i: usize| {
            idx.boundaries.get(i).copied().ok_or_else(|| crate::err(format!("xorb {} index too short", t.xorb)))
        };
        let first = if t.start == 0 { 0 } else { at(t.start as usize - 1)? };
        let last = at(t.end as usize - 1)? - 1;
        let entry = json!({"range": {"start": t.start, "end": t.end}, "url": url_for(&t.xorb), "url_range": {"start": first, "end": last}});
        if let Value::Array(v) = fetch.entry(t.xorb.clone()).or_insert_with(|| Value::Array(vec![])) {
            if !v.contains(&entry) {
                v.push(entry);
            }
        }
    }
    Ok(Some(json!({"offset_into_first_range": offset, "terms": terms, "fetch_info": fetch})))
}
```

`crates/xet/src/routes.rs` (above the tests module):

```rust
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
use slatedb::object_store::{GetOptions, GetRange, ObjectStore};
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

#[derive(serde::Deserialize)]
struct Signed {
    exp: u64,
    sig: String,
}

async fn get_fetch(
    State(st): State<CasState>,
    Path((owner, xorb)): Path<(String, String)>,
    q: Option<Query<Signed>>,
    h: HeaderMap,
) -> Response {
    let path = format!("/v1/fetch/{owner}/{xorb}");
    let ok = q.is_some_and(|Query(q)| sign::verify(st.jwt.secret(), &path, q.exp, &q.sig, sign::now()));
    if !ok {
        return status(StatusCode::FORBIDDEN, "bad or expired signature");
    }
    let Some(hash) = keys::parse_hash(&xorb) else { return status(StatusCode::NOT_FOUND, "no such xorb") };
    let Ok(range) = recon::parse_range(h.get(header::RANGE).and_then(|v| v.to_str().ok())) else {
        return status(StatusCode::RANGE_NOT_SATISFIABLE, "unsupported range");
    };
    let opts = GetOptions {
        range: range.map(|(s, e)| match e {
            Some(e) => GetRange::Bounded(s..e + 1),
            None => GetRange::Offset(s),
        }),
        ..Default::default()
    };
    let got = match st.os.get_opts(&keys::xorb(&owner, &hash), opts).await {
        Ok(g) => g,
        Err(slatedb::object_store::Error::NotFound { .. }) => return status(StatusCode::NOT_FOUND, "no such xorb"),
        Err(e) => return internal(e),
    };
    match got.bytes().await {
        Ok(b) if range.is_some() => (StatusCode::PARTIAL_CONTENT, b).into_response(),
        Ok(b) => (StatusCode::OK, b).into_response(),
        Err(e) => internal(e),
    }
}
```

`lib.rs`: add `pub mod recon; pub mod routes; pub use routes::{router, CasState};`. `Cargo.toml` `[dev-dependencies]`: `tower = { workspace = true }`. If `Option<Query<Signed>>` does not compile on axum 0.8 (it requires `OptionalFromRequestParts`), take `Query<std::collections::HashMap<String, String>>` instead and read `exp`/`sig` from it, with a missing or unparsable value treated as a bad signature.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test -p kloudlite-xet 2>&1 | tail -20`
Expected: every `kloudlite-xet` test passes, including `range_skips_terms_and_sets_offset`, `reconstruction_single_byte_and_past_end` and `tampered_fetch_signature_is_403`.

- [ ] **Step 5: Commit**

```bash
cd /Volumes/kdisk/rustic-git-wt/master
git add crates/xet Cargo.lock
git commit -m "Serve xet uploads, reconstructions and signed fetches"
```

### Task 6: LFS bytes on the CAS listener

**Files:**
- Create: `crates/xet/src/lfs.rs`
- Modify: `crates/xet/src/routes.rs` (two routes in `router`), `crates/xet/src/lib.rs` (`pub mod lfs;`, `pub const MAX_LFS_OBJECT: u64 = 5 * 1024 * 1024 * 1024;`), `crates/xet/Cargo.toml` (`rand = { workspace = true }`, `sha2` already present)
- Test: `crates/xet/src/lfs.rs`

**Interfaces:**
- Consumes: `CasState`, `sign::{verify, now, signed_url}`, `keys::lfs`.
- Produces:
  - `GET /v1/lfs/{owner}/{oid}?exp=&sig=` (presigned; streams the object; 404 when absent) and `PUT /v1/lfs/{owner}/{oid}?exp=&sig=` (presigned; streams the body to `lfs/{owner}/tmp/{oid}-{rand}` while hashing; on a sha256 match `copy_if_not_exists` to `lfs/{owner}/sha256/{oid}` and delete the staging key; mismatch → 400 and the staging key deleted; over `MAX_LFS_OBJECT` → 413). Both 200 on success; an already-present object is a 200 with nothing written.
  - `pub fn lfs_url(st: &CasState, owner: &str, oid: &str, ttl: u64) -> String` — the presigned href the LFS batch endpoint (Task 12) hands out.
  - The signed path is `/v1/lfs/{owner}/{oid}`, the same for GET and PUT; the hub decides upload vs download by which action it hands out, and an upload URL that is used to download only reads what the same caller may already read.

- [ ] **Step 1: Write the failing tests**

`crates/xet/src/lfs.rs` tests module:

```rust
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
}
```

Add `use futures::StreamExt;` inside the tests module for `.next()` on the listing.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test -p kloudlite-xet lfs 2>&1 | tail -20`
Expected: compile error, `cannot find function lfs_url`.

- [ ] **Step 3: Implement**

`crates/xet/src/lfs.rs`:

```rust
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
use slatedb::object_store::{path::Path as OsPath, ObjectStore, WriteMultipart};
use std::collections::HashMap;

pub fn lfs_url(st: &CasState, owner: &str, oid: &str, ttl: u64) -> String {
    sign::signed_url(&st.base, st.jwt.secret(), &format!("/v1/lfs/{owner}/{oid}"), ttl)
}

/// An LFS oid is a lowercase sha256; it becomes part of an object key, so nothing else passes.
fn is_oid(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
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
    if !is_oid(&oid) {
        return refuse(StatusCode::BAD_REQUEST, "oid is not a sha256");
    }
    match st.os.get(&keys::lfs(&owner, &oid)).await {
        Ok(g) => (StatusCode::OK, Body::from_stream(g.into_stream())).into_response(),
        Err(slatedb::object_store::Error::NotFound { .. }) => refuse(StatusCode::NOT_FOUND, "no such object"),
        Err(e) => internal(e),
    }
}

pub(crate) async fn put_lfs(
    State(st): State<CasState>,
    Path((owner, oid)): Path<(String, String)>,
    Query(q): Query<HashMap<String, String>>,
    body: Body,
) -> Response {
    if !signed(&st, &owner, &oid, &q) {
        return refuse(StatusCode::FORBIDDEN, "bad or expired signature");
    }
    if !is_oid(&oid) {
        return refuse(StatusCode::BAD_REQUEST, "oid is not a sha256");
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
        if n > MAX_LFS_OBJECT {
            let _ = w.abort().await;
            return refuse(StatusCode::PAYLOAD_TOO_LARGE, "object over 5 GiB");
        }
        hasher.update(&chunk);
        w.write(&chunk);
    }
    if let Err(e) = w.finish().await {
        return internal(e);
    }
    let got = hex::encode(hasher.finalize());
    let result = if got != oid {
        refuse(StatusCode::BAD_REQUEST, &format!("body hashes to {got}, not {oid}"))
    } else {
        match st.os.copy_if_not_exists(&tmp, &dest).await {
            Ok(()) | Err(slatedb::object_store::Error::AlreadyExists { .. }) => StatusCode::OK.into_response(),
            Err(e) => internal(e),
        }
    };
    // ponytail: a crash between finish and this delete strands a tmp object; the GC lane
    // (Task 13) sweeps `lfs/*/tmp/` older than a day.
    let _ = st.os.delete(&tmp).await;
    result
}
```

In `routes.rs` `router`, before `.layer(DefaultBodyLimit…)`, add:

```rust
        .route("/v1/lfs/{owner}/{oid}", get(crate::lfs::get_lfs).put(crate::lfs::put_lfs))
```

`put_lfs` takes a raw `Body`, which `DefaultBodyLimit` does not cap (it applies to the `Bytes` extractor); the 5 GiB check in the loop is the limit. If `LocalFileSystem` (`file://`) lacks `copy_if_not_exists`, it implements it via hard link; if the vendored version returns `NotImplemented`, fall back to `put_opts(PutMode::Create)` of the bytes read back from `tmp`, with a `ponytail:` note that this path is for `file://` dev only. Check `~/.cargo/registry/src/*/object_store-0.14*/src/local.rs` before writing either.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test -p kloudlite-xet 2>&1 | tail -20`
Expected: all pass, including `wrong_bytes_are_refused_and_leave_nothing`.

- [ ] **Step 5: Commit**

```bash
cd /Volumes/kdisk/rustic-git-wt/master
git add crates/xet Cargo.lock
git commit -m "Store git LFS objects behind presigned CAS URLs"
```

---

### Task 7: Hub crate, auth, repo create and delete, whoami

**Files:**
- Create: `crates/hub/Cargo.toml`, `crates/hub/src/lib.rs`, `crates/hub/src/auth.rs`, `crates/hub/src/repos.rs`
- Modify: root `Cargo.toml` (`members` gains `crates/hub`; `[dev-dependencies]` of the root package gains `kloudlite-hub = { path = "crates/hub" }` and `kloudlite-xet = { path = "crates/xet", features = ["testkit"] }`), `tests/common/mod.rs` (add `serve_hub`)
- Test: `tests/hub_api.rs`

**Interfaces:**
- Consumes: `kloudlite_storage::{HubKind, hub_routing_key}`, `Store::{hub_create, hub_open, hub_exists, hub_delete, hub_public}` (Task 1); `kloudlite_xet::routes::CasState`, `kloudlite_xet::cas_url` (Tasks 2, 5); `kloudlite_core::httpx::{Trusted, scheme, basic_token, basic_user_names}`; `Store::owner_for_token`; `App::may_act`.
- Produces:
  - `lib.rs`: `#[derive(Clone)] pub struct HubState { pub app: Arc<App>, pub cas: CasState, pub base: String }`; `impl HubState { pub fn new(app: Arc<App>, base: String) -> HubState }` (cas = `CasState { os: app.store.os.clone(), jwt: app.jwt.clone(), base: kloudlite_xet::cas_url() }`); `pub fn hub_url() -> String` (`KLOUDLITE_HUB_URL`, default `http://localhost:8090`, trailing `/` trimmed); `pub fn hub_err(status: StatusCode, code: Option<&str>, msg: &str) -> Response` (headers `X-Error-Code` when `code` is `Some`, `X-Error-Message` always, body `{"error": msg}`); `pub fn internal(e: impl std::fmt::Display) -> Response` (500 via `hub_err`, logs `tracing::error!`); `pub fn router(st: HubState) -> Router` (state applied); `fn per_kind(kind: HubKind) -> Router<HubState>` (each later task adds its routes here, once, for both kinds).
  - `auth.rs`: `pub async fn caller(app: &App, trusted: Option<&Trusted>, headers: &HeaderMap) -> Result<Option<String>, Response>`; `pub struct Access { pub kind: HubKind, pub owner: String, pub name: String, pub repo: Repo, pub caller: Option<String> }`; `pub async fn open(st: &HubState, kind: HubKind, trusted: Option<&Trusted>, headers: &HeaderMap, owner: &str, name: &str, write: bool) -> Result<Access, Response>`; `pub async fn may_write(app: &App, caller: &str, owner: &str) -> bool`.
  - `repos.rs`: `POST /api/repos/create`, `DELETE /api/repos/delete`, `GET /api/whoami-v2`.
  - `tests/common/mod.rs`: `pub async fn serve_hub(app: Arc<kloudlite_app::App>) -> u16` (hub router on 127.0.0.1:0, `base` = `http://127.0.0.1:{port}`).

Rulings this task applies:
- A credential that does not verify is 401 (`kloudlite_core::httpx::unauthorized()` plus `X-Error-Code: Unauthorized`... no: plain `hub_err(401, None, "invalid credentials")` with `WWW-Authenticate: Basic realm="kloudlite"`). No credential is anonymous.
- Bearer carries a PAT (`owner_for_token`). Basic is `user:PAT`; the user must name the token's owner or be git's `x` placeholder (`basic_user_names(headers, &o, true)`), because git-lfs replays the git remote's credentials.
- Visibility: anonymous on a private or missing repo is 401 `RepoNotFound`; a signed-in caller without access is 404 `RepoNotFound`; a write by a signed-in caller without access to a public repo is 403. `may_act` returning `Err` (directory unavailable) counts as no access, logged.
- Create: body `{name, organization?, type?, private?}`; `type` defaults to `model`, `space` is 400, anything else unknown is 400; `private` defaults to `true`; owner is `organization` or the caller; anonymous is 401; a caller who is not the owner and not `may_act` is 403; an existing repo is 409 with `{"error", "url"}`; success is 200 `{"url": "{base}/{prefix}{owner}/{name}"}`. `name` may arrive as `owner/name` (older huggingface_hub): split on the first `/` when `organization` is absent.
- Delete: same body; missing repo or no access is 404 `RepoNotFound`; success 200 with an empty body.
- whoami-v2: anonymous 401; else `{"type":"user","name":<caller>,"fullname":<caller>,"email":null,"orgs":[],"auth":{"accessToken":{"displayName":"token","role":"write"}}}`.

- [ ] **Step 1: Write the failing test**

Add to `tests/common/mod.rs`:

```rust
/// The hub listener alone, no routing middleware — single-node tests. Task 14 swaps the router
/// for the server's routed one.
pub async fn serve_hub(app: Arc<kloudlite_app::App>) -> u16 {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let st = kloudlite_hub::HubState::new(app, format!("http://127.0.0.1:{port}"));
    tokio::spawn(async move {
        axum::serve(l, kloudlite_hub::router(st)).await.unwrap();
    });
    port
}
```

Create `tests/hub_api.rs`:

```rust
//! Hub API over the hub listener: repo lifecycle, auth, visibility, reads, commits.
mod common;

use common::*;
use kloudlite_storage::HubKind;
use serde_json::{json, Value};

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
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test --test hub_api 2>&1 | tail -20`
Expected: FAIL to compile, "unresolved import `kloudlite_hub`".

- [ ] **Step 3: Write the implementation**

`crates/hub/Cargo.toml` (copy the exact dependency spellings — `workspace = true` or pinned — from `crates/registry/Cargo.toml` and `crates/gitbase/Cargo.toml`; add any of these the workspace lacks at the versions those crates use):

```toml
[package]
name = "kloudlite-hub"
version = "0.1.0"
edition = "2021"

[dependencies]
kloudlite-core = { path = "../core" }
kloudlite-storage = { path = "../storage" }
kloudlite-app = { path = "../app" }
kloudlite-gitbase = { path = "../gitbase" }
kloudlite-vcs = { path = "../git" }
kloudlite-xet = { path = "../xet" }
axum = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
tokio = { workspace = true }
tracing = { workspace = true }
base64 = { workspace = true }
sha2 = { workspace = true }
hex = { workspace = true }
gix-hash = { workspace = true }
gix-object = { workspace = true }
gix-odb = { workspace = true }
futures = { workspace = true }
bytes = { workspace = true }
slatedb = { workspace = true }

[dev-dependencies]
tower = { workspace = true }
```

`crates/hub/src/lib.rs`:

```rust
//! The Hugging Face Hub API, served on its own listener (`KLOUDLITE_HUB_ADDR`, `hub.<domain>`).
//!
//! Model repos live at `/{owner}/{name}`, datasets at `/datasets/{owner}/{name}`; each is an
//! ordinary git repo in its own namespace (`mdl`, `ds`; see `kloudlite_storage::hubrepo`), so
//! history, packs and refs reuse the git tier unchanged. Large files are LFS pointers in git and
//! bytes in the per-owner Xet CAS (`kloudlite_xet`), which this crate only mints tokens and URLs
//! for.
//!
//! Every route here touches a per-repo DB except `/api/repos/*` and `/api/whoami-v2`; the
//! server's `route_hub` middleware routes them to the owning node BEFORE this router runs, so
//! nothing in this crate forwards.
//!
//! Module map: `auth` (caller, visibility), `repos` (create, delete, whoami), `rev` (revision
//! names), `pointer` (LFS pointer text), `read` (info, tree, paths-info, refs), `resolve` (file
//! bytes), `commit` (preupload, NDJSON commit), `refs_api` (branches, tags), `tokens` (Xet
//! tokens), `lfs_batch` (git-lfs batch).
#![deny(clippy::unwrap_used, clippy::expect_used)]

pub mod auth;
pub mod repos;

use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use kloudlite_app::App;
use kloudlite_storage::HubKind;
use kloudlite_xet::routes::CasState;
use std::sync::Arc;

#[derive(Clone)]
pub struct HubState {
    pub app: Arc<App>,
    pub cas: CasState,
    /// External URL of the hub listener, no trailing slash. Repo URLs and `Link` headers use it.
    pub base: String,
}

impl HubState {
    pub fn new(app: Arc<App>, base: String) -> HubState {
        let cas = CasState { os: app.store.os.clone(), jwt: app.jwt.clone(), base: kloudlite_xet::cas_url() };
        HubState { app, cas, base: base.trim_end_matches('/').to_string() }
    }
}

pub fn hub_url() -> String {
    std::env::var("KLOUDLITE_HUB_URL")
        .unwrap_or_else(|_| "http://localhost:8090".into())
        .trim_end_matches('/')
        .to_string()
}

/// huggingface_hub turns `X-Error-Code` into a typed exception (`RepositoryNotFoundError`,
/// `EntryNotFoundError`, `RevisionNotFoundError`) and shows `X-Error-Message`; the JSON body is for
/// everyone else.
pub fn hub_err(status: StatusCode, code: Option<&str>, msg: &str) -> Response {
    let mut r = (status, Json(serde_json::json!({ "error": msg }))).into_response();
    let h = r.headers_mut();
    if let Some(c) = code.and_then(|c| HeaderValue::from_str(c).ok()) {
        h.insert("x-error-code", c);
    }
    if let Ok(m) = HeaderValue::from_str(msg) {
        h.insert("x-error-message", m);
    }
    r
}

pub fn internal(e: impl std::fmt::Display) -> Response {
    tracing::error!(target: "hub", error = %e, "hub.internal");
    hub_err(StatusCode::INTERNAL_SERVER_ERROR, None, "internal error")
}

pub fn router(st: HubState) -> Router {
    Router::new()
        .route("/api/repos/create", post(repos::create))
        .route("/api/repos/delete", delete(repos::delete))
        .route("/api/whoami-v2", get(repos::whoami))
        .merge(per_kind(HubKind::Model))
        .merge(per_kind(HubKind::Dataset))
        .with_state(st)
}

/// Routes that exist once per kind. Paths are built from the kind so the two never drift; the
/// kind reaches handlers as an `Extension`, set by this router's own layer.
fn per_kind(kind: HubKind) -> Router<HubState> {
    let _api = format!("/api/{}/{{owner}}/{{name}}", kind.api_plural());
    let _repo = format!("/{}{{owner}}/{{name}}", kind.url_prefix());
    Router::new().layer(axum::Extension(kind))
}
```

(`_api` and `_repo` lose their underscore in Task 8, when the first per-kind route lands.)

`crates/hub/src/auth.rs`:

```rust
//! Who is calling, and may they see or change this repo.
//!
//! Credentials: Bearer PAT (huggingface_hub, transformers) or Basic `user:PAT` (git, git-lfs). A
//! credential that does not verify is a 401, never a silent downgrade to anonymous. Visibility
//! follows the spec: anonymous on a private or missing repo is 401 (so huggingface_hub prompts for
//! login), a signed-in stranger gets 404 `RepoNotFound` (existence is not leaked), and a write by
//! a stranger to a public repo is 403.
use crate::{hub_err, internal, HubState};
use axum::http::header::{AUTHORIZATION, WWW_AUTHENTICATE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use kloudlite_app::App;
use kloudlite_core::httpx::{basic_token, basic_user_names, scheme, Trusted};
use kloudlite_storage::store::Repo;
use kloudlite_storage::HubKind;

fn bad_credential() -> Response {
    let mut r = hub_err(StatusCode::UNAUTHORIZED, None, "invalid credentials");
    r.headers_mut().insert(WWW_AUTHENTICATE, HeaderValue::from_static("Basic realm=\"kloudlite\""));
    r
}

pub async fn caller(app: &App, trusted: Option<&Trusted>, headers: &HeaderMap) -> Result<Option<String>, Response> {
    // A peer forwarded this request after authenticating it; `trust_peer` checked the secret.
    if let Some(Trusted(Some(o))) = trusted {
        return Ok(Some(o.clone()));
    }
    let Some(v) = headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok()) else {
        return Ok(None);
    };
    let token = match (scheme(v, "Bearer"), basic_token(headers)) {
        (Some(t), _) => t.to_string(),
        (None, Some(t)) => t,
        (None, None) => return Err(bad_credential()),
    };
    match app.store.owner_for_token(&token).await {
        // git-lfs replays the git remote's credentials, so git's `x` placeholder is a valid user.
        Ok(Some(o)) if basic_user_names(headers, &o, true) => Ok(Some(o)),
        Ok(_) => Err(bad_credential()),
        Err(e) => Err(internal(e)),
    }
}

/// Owner, or a member the directory vouches for. A directory that cannot answer denies.
pub async fn may_write(app: &App, caller: &str, owner: &str) -> bool {
    if caller == owner {
        return true;
    }
    match app.may_act(caller, owner).await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(target: "hub", error = %e, caller, owner, "hub.may_act.unavailable");
            false
        }
    }
}

pub struct Access {
    pub kind: HubKind,
    pub owner: String,
    pub name: String,
    pub repo: Repo,
    pub caller: Option<String>,
}

fn not_found(caller: &Option<String>) -> Response {
    match caller {
        None => hub_err(StatusCode::UNAUTHORIZED, Some("RepoNotFound"), "Repository not found"),
        Some(_) => hub_err(StatusCode::NOT_FOUND, Some("RepoNotFound"), "Repository not found"),
    }
}

pub async fn open(
    st: &HubState,
    kind: HubKind,
    trusted: Option<&Trusted>,
    headers: &HeaderMap,
    owner: &str,
    name: &str,
    write: bool,
) -> Result<Access, Response> {
    let caller = caller(&st.app, trusted, headers).await?;
    let member = match &caller {
        Some(c) => may_write(&st.app, c, owner).await,
        None => false,
    };
    let store = &st.app.store;
    let repo = match store.hub_open(kind, owner, name).await {
        Ok(Some(r)) => r,
        Ok(None) => return Err(not_found(&caller)),
        Err(e) => return Err(internal(e)),
    };
    if !member {
        let public = store.hub_public(kind, owner, name).await.map_err(internal)?;
        if !public {
            return Err(not_found(&caller));
        }
        if write {
            return Err(match caller {
                None => bad_credential(),
                Some(_) => hub_err(StatusCode::FORBIDDEN, None, "no write access to this repository"),
            });
        }
    }
    Ok(Access { kind, owner: owner.to_string(), name: name.to_string(), repo, caller })
}
```

`crates/hub/src/repos.rs`:

```rust
//! Repo lifecycle and identity: `POST /api/repos/create`, `DELETE /api/repos/delete`,
//! `GET /api/whoami-v2`. The server routes create/delete by the key in the JSON body (Task 14);
//! these handlers assume they already run on the owning node.
use crate::auth::{caller, may_write};
use crate::{hub_err, internal, HubState};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use kloudlite_core::httpx::Trusted;
use kloudlite_storage::HubKind;
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
pub struct RepoBody {
    pub name: String,
    pub organization: Option<String>,
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub private: Option<bool>,
}

/// `(kind, owner, name)` from a create/delete body, or the 400 that refuses it. `caller` fills in
/// the owner when the body names none.
pub fn target(b: &RepoBody, caller: Option<&str>) -> Result<(HubKind, String, String), Response> {
    let kind = match b.kind.as_deref().unwrap_or("model") {
        "space" => return Err(hub_err(StatusCode::BAD_REQUEST, None, "Spaces are not served")),
        t => HubKind::from_type(t).ok_or_else(|| hub_err(StatusCode::BAD_REQUEST, None, "type must be model or dataset"))?,
    };
    let (owner, name) = match (&b.organization, b.name.split_once('/')) {
        (Some(o), _) => (o.clone(), b.name.clone()),
        (None, Some((o, n))) => (o.to_string(), n.to_string()),
        (None, None) => match caller {
            Some(c) => (c.to_string(), b.name.clone()),
            None => return Err(hub_err(StatusCode::UNAUTHORIZED, None, "sign in to name an owner")),
        },
    };
    Ok((kind, owner, name))
}

pub fn repo_url(base: &str, kind: HubKind, owner: &str, name: &str) -> String {
    format!("{base}/{}{owner}/{name}", kind.url_prefix())
}

pub async fn create(
    State(st): State<HubState>,
    trusted: Option<Extension<Trusted>>,
    headers: HeaderMap,
    Json(b): Json<RepoBody>,
) -> Response {
    let who = match caller(&st.app, trusted.as_deref(), &headers).await {
        Ok(Some(c)) => c,
        Ok(None) => return hub_err(StatusCode::UNAUTHORIZED, None, "sign in to create a repository"),
        Err(r) => return r,
    };
    let (kind, owner, name) = match target(&b, Some(&who)) {
        Ok(t) => t,
        Err(r) => return r,
    };
    if !may_write(&st.app, &who, &owner).await {
        return hub_err(StatusCode::FORBIDDEN, None, "you cannot create repositories for this owner");
    }
    let url = repo_url(&st.base, kind, &owner, &name);
    let store = &st.app.store;
    match store.hub_exists(kind, &owner, &name).await {
        Ok(true) => {
            let mut r = (StatusCode::CONFLICT, Json(json!({ "error": "You already created this repo", "url": url }))).into_response();
            if let Ok(v) = "You already created this repo".parse() {
                r.headers_mut().insert("x-error-message", v);
            }
            return r;
        }
        Ok(false) => {}
        Err(e) => return internal(e),
    }
    // ponytail: exists-then-create can race two creators; the loser's create fails inside the
    // store and surfaces as a 500. A conditional create in `hub_create` closes it if it matters.
    match store.hub_create(kind, &owner, &name, b.private.unwrap_or(true)).await {
        Ok(()) => Json(json!({ "url": url })).into_response(),
        Err(e) if e.to_string().contains("invalid") => hub_err(StatusCode::BAD_REQUEST, None, "invalid repository name"),
        Err(e) => internal(e),
    }
}

pub async fn delete(
    State(st): State<HubState>,
    trusted: Option<Extension<Trusted>>,
    headers: HeaderMap,
    Json(b): Json<RepoBody>,
) -> Response {
    let who = match caller(&st.app, trusted.as_deref(), &headers).await {
        Ok(Some(c)) => c,
        Ok(None) => return hub_err(StatusCode::UNAUTHORIZED, Some("RepoNotFound"), "Repository not found"),
        Err(r) => return r,
    };
    let (kind, owner, name) = match target(&b, Some(&who)) {
        Ok(t) => t,
        Err(r) => return r,
    };
    let gone = || hub_err(StatusCode::NOT_FOUND, Some("RepoNotFound"), "Repository not found");
    if !may_write(&st.app, &who, &owner).await {
        return gone();
    }
    match st.app.store.hub_exists(kind, &owner, &name).await {
        Ok(true) => {}
        Ok(false) => return gone(),
        Err(e) => return internal(e),
    }
    match st.app.store.hub_delete(kind, &owner, &name).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(e) => internal(e),
    }
}

pub async fn whoami(State(st): State<HubState>, trusted: Option<Extension<Trusted>>, headers: HeaderMap) -> Response {
    match caller(&st.app, trusted.as_deref(), &headers).await {
        Ok(Some(c)) => Json(json!({
            "type": "user",
            "name": c,
            "fullname": c,
            "email": null,
            "orgs": [],
            "auth": { "accessToken": { "displayName": "token", "role": "write" } },
        }))
        .into_response(),
        Ok(None) => hub_err(StatusCode::UNAUTHORIZED, None, "sign in first"),
        Err(r) => r,
    }
}
```

Before writing `create`, read the error `hub_create` returns for an invalid owner or name (Task 1, `crates/storage/src/hubrepo.rs`) and match on that exact text instead of `"invalid"` if it differs; if Task 1 returned a typed error, match the type.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test --test hub_api 2>&1 | tail -20`
Expected: 8 passed.
Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo clippy -p kloudlite-hub --all-targets -- -D warnings 2>&1 | tail -20`
Expected: clean.

- [ ] **Step 5: Commit**

```bash
cd /Volumes/kdisk/rustic-git-wt/master
git add crates/hub Cargo.toml Cargo.lock tests/common/mod.rs tests/hub_api.rs
git commit -m "Create and delete hub repos with Hugging Face auth"
```

---

### Task 8: Read API — revisions, info, tree, paths-info, refs

**Files:**
- Create: `crates/hub/src/rev.rs`, `crates/hub/src/pointer.rs`, `crates/hub/src/read.rs`
- Modify: `crates/hub/src/lib.rs` (`pub mod rev; pub mod pointer; pub mod read;`, routes in `per_kind`)
- Test: `crates/hub/src/pointer.rs` (unit), `tests/hub_api.rs` (append)

**Interfaces:**
- Consumes: `auth::{open, Access}`, `hub_err`, `internal`, `HubState`; `Store::{get_ref, list_refs}`; `kloudlite_vcs::browse::{files_at, tree_at, blob_at, commit, is_not_found, Entry}`; `kloudlite_xet::keys::sha`.
- Produces:
  - `rev.rs`: `pub async fn resolve(store: &Store, repo: &Repo, rev: &str) -> kloudlite_storage::Result<Option<ObjectId>>` (tries `refs/heads/{rev}`, then `refs/tags/{rev}`, then a full 40-hex sha that exists as a commit; `None` otherwise); `pub fn revision_not_found(rev: &str) -> Response` (404 `RevisionNotFound`); `pub fn safe_path(p: &str) -> bool` (no empty segment, no `.`/`..` segment, no leading `/`, no `\0`, first segment not `.git`).
  - `pointer.rs`: `pub const POINTER_MAX: usize = 1024`; `pub struct Pointer { pub sha256: String, pub size: u64 }`; `pub fn parse(b: &[u8]) -> Option<Pointer>` (exactly the three-line spec form, `sha256` 64 lowercase hex); `pub fn text(sha256: &str, size: u64) -> String`.
  - `read.rs`: `pub struct FileInfo { pub path: String, pub oid: String, pub size: u64, pub lfs: Option<(Pointer, u64)> /* pointer, pointer size */, pub xet_hash: Option<String> }`; `pub async fn file_infos(st: &HubState, a: &Access, commit: ObjectId, prefix: &str, recursive: bool) -> Result<Vec<FileInfo>, Response>` (files only, sorted by path); `pub async fn xet_hash_of(st: &HubState, owner: &str, sha256: &str) -> Option<String>` (body of `xet/{owner}/sha/{sha256}`); routes `GET /api/{t}/{owner}/{name}`, `GET …/revision/{rev}`, `GET …/tree/{rev}` and `…/tree/{rev}/{*path}`, `POST …/paths-info/{rev}`, `GET …/refs`.

Rulings this task applies:
- `info`: `{"_id": "{ns}/{owner}/{name}", "id": "{owner}/{name}", "modelId"(models only): same, "sha": commit hex, "private": bool, "lastModified": RFC 3339 of the commit time, "siblings": [{"rfilename", "size", "blobId", "lfs"?: {"size", "sha256", "pointerSize"}}]}`. Siblings are always listed with sizes (huggingface_hub's `files_metadata` asks for them; listing them unasked costs one tree walk). A repo with no commit (fresh create, no `main`) answers `info` with `sha: null` and `siblings: []`, because `create_repo` then `upload_file` reads info before the first commit.
- `tree`: one page of at most 1000 entries in path order; a further page is `Link: <{base}/api/{t}/{owner}/{name}/tree/{rev}/{path}?recursive=..&cursor={last path}>; rel="next"` where `cursor` is the last path returned; directories appear as `{"type":"directory","oid","path"}` only when not recursive; files `{"type":"file","oid","size","path","lfs"?: {"oid": sha256, "size", "pointerSize"},"xetHash"?}`. `expand` is accepted and ignored (`lastCommit`/`securityFileStatus` are not computed).
- `paths-info`: form field `paths` (repeated) and optional `expand`; returns the entries for paths that exist, in request order, files and directories; unknown paths are left out, not 404.
- `refs`: `{"branches":[{"name","ref":"refs/heads/x","targetCommit"}],"converts":[],"tags":[...]}`; `include_pull_requests` ignored.
- A path that is not `safe_path` reads as missing.

- [ ] **Step 1: Write the failing tests**

`crates/hub/src/pointer.rs` (whole file; tests at the bottom):

```rust
//! git-lfs pointer text. The exact three-line form is what git-lfs, git-xet and huggingface_hub
//! all write, so it is the only form this server writes or recognises.

pub const POINTER_MAX: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pointer {
    pub sha256: String,
    pub size: u64,
}

pub fn text(sha256: &str, size: u64) -> String {
    format!("version https://git-lfs.github.com/spec/v1\noid sha256:{sha256}\nsize {size}\n")
}

pub fn parse(b: &[u8]) -> Option<Pointer> {
    if b.len() > POINTER_MAX {
        return None;
    }
    let s = std::str::from_utf8(b).ok()?;
    let mut lines = s.strip_suffix('\n')?.split('\n');
    if lines.next()? != "version https://git-lfs.github.com/spec/v1" {
        return None;
    }
    let sha = lines.next()?.strip_prefix("oid sha256:")?;
    let size = lines.next()?.strip_prefix("size ")?.parse().ok()?;
    if lines.next().is_some() || sha.len() != 64 || !sha.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f')) {
        return None;
    }
    Some(Pointer { sha256: sha.to_string(), size })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        let sha = "a".repeat(64);
        let t = text(&sha, 42);
        assert_eq!(parse(t.as_bytes()), Some(Pointer { sha256: sha, size: 42 }));
    }

    #[test]
    fn refuses_near_misses() {
        let sha = "a".repeat(64);
        assert!(parse(text(&sha, 1).trim_end().as_bytes()).is_none());
        assert!(parse(text(&"A".repeat(64), 1).as_bytes()).is_none());
        assert!(parse(text(&"a".repeat(63), 1).as_bytes()).is_none());
        assert!(parse(format!("{}extra\n", text(&sha, 1)).as_bytes()).is_none());
        assert!(parse(b"hello world\n").is_none());
    }
}
```

Append to `tests/hub_api.rs` (these drive the commit endpoint built in Task 10 only through the store, so they stand alone: they seed git history with `seed_hub` below):

```rust
use kloudlite_gitbase::objects::{apply_changes, write_commit, Change, NewCommit, Staging};
use kloudlite_gitbase::refs::UpdateRefsExt;
use kloudlite_storage::refmeta::RefUpdate;
use std::collections::BTreeMap;

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
    gix_object::CommitRef::from_bytes(c.data).ok().map(|c| c.tree())
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
        .form(&[("paths", "dir/b.txt"), ("paths", "missing"), ("paths", "dir")])
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
```

`reqwest`'s `form` needs the `"form"`-free default features; if the root package pins `reqwest` without default features, add `features = [..., "json"]` as already present and build the body by hand: `.header("content-type", "application/x-www-form-urlencoded").body("paths=dir%2Fb.txt&paths=missing&paths=dir")`. The root package's dev-dependencies also gain `gix-hash`, `gix-object`, `gix-odb` (workspace spellings) for `tree_of`.

Before writing `seed_hub`, read `crates/gitbase/src/objects.rs` for the exact `apply_changes` signature (its `base` argument may be a tree or a commit oid; pass a commit directly and drop `tree_of` if it takes a commit) and the `Change` variants; adjust `seed_hub` to them, not the other way round.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test -p kloudlite-hub 2>&1 | tail -10; CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test --test hub_api 2>&1 | tail -20`
Expected: unit tests fail to compile (`pointer` not a module); `hub_api` read tests fail with 404 (routes missing).

- [ ] **Step 3: Write the implementation**

`crates/hub/src/rev.rs`:

```rust
//! Revision names as huggingface_hub sends them: a branch, a tag, or a full commit sha. Branch
//! wins over tag, as on the Hub. Short shas are not accepted (the Hub refuses them too).
use crate::hub_err;
use axum::http::StatusCode;
use axum::response::Response;
use gix_hash::ObjectId;
use kloudlite_storage::store::{Repo, Store};

pub async fn resolve(store: &Store, repo: &Repo, rev: &str) -> kloudlite_storage::Result<Option<ObjectId>> {
    for prefix in ["refs/heads/", "refs/tags/"] {
        if let Some(o) = store.get_ref(repo, &format!("{prefix}{rev}")).await? {
            return Ok(Some(o));
        }
    }
    let Ok(oid) = rev.parse::<ObjectId>() else { return Ok(None) };
    if rev.len() != 40 {
        return Ok(None);
    }
    let repo = repo.clone();
    let found = tokio::task::spawn_blocking(move || {
        let odb = repo.odb().ok()?;
        kloudlite_vcs::browse::commit(&odb, oid).ok()
    })
    .await
    .ok()
    .flatten();
    Ok(found.map(|_| oid))
}

pub fn revision_not_found(rev: &str) -> Response {
    hub_err(StatusCode::NOT_FOUND, Some("RevisionNotFound"), &format!("Invalid rev id: {rev}"))
}

/// A repo-relative path that cannot climb out or into git's own directory.
pub fn safe_path(p: &str) -> bool {
    !p.is_empty()
        && !p.contains('\0')
        && !p.starts_with('/')
        && p.split('/').all(|s| !s.is_empty() && s != "." && s != "..")
        && p.split('/').next() != Some(".git")
}

#[cfg(test)]
mod tests {
    use super::safe_path;

    #[test]
    fn safe_paths() {
        assert!(safe_path("a/b.txt"));
        assert!(safe_path(".gitattributes"));
        for bad in ["", "/a", "a//b", "a/../b", "..", "./a", ".git/config", "a\0b"] {
            assert!(!safe_path(bad), "{bad:?}");
        }
    }
}
```

If `Store` methods return a different error type than `kloudlite_storage::Result`, use the type `get_ref` returns. If `ObjectId` parses short hex, the `len() != 40` guard keeps it out; leave the guard.

`crates/hub/src/read.rs`:

```rust
//! Read API: repo info, tree listing, paths-info, refs. Everything reads one commit's tree on a
//! blocking thread (gix is sync), then decorates LFS pointers with their sha256 and, when the
//! bytes live in Xet, the Xet file hash huggingface_hub needs to download through the CAS.
use crate::auth::{open, Access};
use crate::pointer::{self, Pointer, POINTER_MAX};
use crate::rev::{resolve, revision_not_found, safe_path};
use crate::{hub_err, internal, HubState};
use axum::extract::{Path, Query, RawForm, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use gix_hash::ObjectId;
use kloudlite_core::httpx::Trusted;
use kloudlite_storage::HubKind;
use serde::Deserialize;
use serde_json::{json, Value};

pub const PAGE: usize = 1000;

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
    let raw = tokio::task::spawn_blocking(move || -> kloudlite_core::Result<Vec<(String, String, u64, Option<Vec<u8>>)>> {
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
            let link = format!(
                "<{}/api/{}/{owner}/{name}/tree/{rev}/{path}?recursive={}&cursor={}>; rel=\"next\"",
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
        if b[i] == b'%' && i + 2 < b.len() + 0 && i + 2 <= b.len() - 1 {
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
```

Replace `percent_decode`'s bounds check with the plain `i + 2 < b.len()` (the expression above is equivalent; write the plain one). Before writing `file_infos`, read `crates/git/src/browse.rs` around `files_at` (`:222`) and `Entry` for: whether `files_at` returns full paths or names (the code assumes full paths, matching the browse API's file finder), what `kind` strings are used (`"blob"`/`"tree"` assumed), what `cap` means, and the crate's `Error`/`Result` names; adapt the closure to them. If `kloudlite_vcs::Error` has no `From<String>`, map `odb()` errors with whatever constructor `browse.rs` itself uses. `Commit.time` is assumed to be epoch seconds; read the `Commit` struct and adapt.

In `lib.rs`: add `pub mod pointer; pub mod read; pub mod rev;`, drop the underscores from `_api`/`_repo`, and make `per_kind`:

```rust
fn per_kind(kind: HubKind) -> Router<HubState> {
    let api = format!("/api/{}/{{owner}}/{{name}}", kind.api_plural());
    let _repo = format!("/{}{{owner}}/{{name}}", kind.url_prefix());
    Router::new()
        .route(&api, get(read::info))
        .route(&format!("{api}/revision/{{rev}}"), get(read::info_rev))
        .route(&format!("{api}/tree/{{rev}}"), get(read::tree_root))
        .route(&format!("{api}/tree/{{rev}}/{{*path}}"), get(read::tree_path))
        .route(&format!("{api}/paths-info/{{rev}}"), post(read::paths_info))
        .route(&format!("{api}/refs"), get(read::refs))
        .layer(axum::Extension(kind))
}
```

`HubKind` needs `PartialEq` and `Copy` (`kind == HubKind::Model`, passed by value); add the derives in `crates/storage/src/hubrepo.rs` if Task 1 left them out.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test -p kloudlite-hub 2>&1 | tail -10; CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test --test hub_api 2>&1 | tail -20`
Expected: all pass (3 unit, 16 `hub_api`).

- [ ] **Step 5: Commit**

```bash
cd /Volumes/kdisk/rustic-git-wt/master
git add crates/hub crates/storage Cargo.toml Cargo.lock tests/hub_api.rs
git commit -m "Serve hub repo info, trees, paths-info and refs"
```

---

### Task 9: Resolve downloads (git blob, LFS store, Xet)

**Files:**
- Create: `crates/xet/src/stream.rs`, `crates/hub/src/resolve.rs`
- Modify: `crates/xet/src/lib.rs` (`pub mod stream;`), `crates/hub/src/lib.rs` (`pub mod resolve;`, route in `per_kind`)
- Test: `crates/xet/src/stream.rs` (unit), `tests/hub_api.rs`

**Interfaces:**
- Consumes: `keys::{parse_hash, xorb, sha, lfs}`, `shards::{read_file, FileRecord, Term}`, `recon::parse_range`, `testkit::build`, `xorbs::put_xorb`, `shards::put_shard` (Tasks 3–5); `auth::open`, `rev::{resolve, revision_not_found, safe_path}`, `pointer::{parse, POINTER_MAX}`, `read::{entry_not_found, xet_hash_of}`, `hub_err`, `internal`, `HubState` (Tasks 7–8); `kloudlite_vcs::browse::{blob_at, is_not_found}` (`Blob { oid: String, bytes: Vec<u8>, truncated: bool }`).
- Produces:
  - `kloudlite_xet::stream::file_bytes(os: Arc<dyn ObjectStore>, owner: String, rec: FileRecord, start: u64, end: u64) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send + 'static` (`end` exclusive).
  - `kloudlite_hub::resolve::{resolve (handler), exists(os: &dyn ObjectStore, p: &Path) -> Result<bool, Response>}`. Task 10 uses `exists`.

Rulings this task applies:
- Every response after the commit is known carries `X-Repo-Commit` (40-hex), 404s included. Repo-level refusals (401/404 `RepoNotFound`) and `RevisionNotFound` have no commit to name and carry none.
- An unsafe path (`safe_path` false) is 404 `EntryNotFound`, never a 400: a reader cannot tell "climbs out" from "absent".
- `ETag` is the quoted git blob oid for a plain file, the quoted sha256 for an LFS file; LFS files also carry `X-Linked-Etag` (same quoted sha256) and `X-Linked-Size` (the pointer's size). `Accept-Ranges: bytes` always.
- Range: `parse_range` error or start ≥ size is 416 with `Content-Range: bytes */{size}`; an end past the size is clamped; a satisfied range is 206 with `Content-Range: bytes {s}-{e-1}/{size}`.
- Xet-backed LFS file (`xet/{owner}/sha/{sha256}` exists): `X-Xet-Hash: {file hash hex}` and `Link: <{hub base}/api/{plural}/{owner}/{name}/xet-read-token/{commit}>; rel="xet-auth"`; body streamed from xorbs. Else `lfs/{owner}/sha256/{sha256}` streamed with a ranged `get_opts`. Else 404 `EntryNotFound` "large file content missing".
- HEAD: same status and headers including `Content-Length`, empty body; the xorbs are never fetched.

- [ ] **Step 1: Write the failing unit test for the xorb stream**

`crates/xet/src/stream.rs`:

```rust
//! Streams a Xet file's bytes out of its xorbs, term by term, for the hub's resolve route.
//!
//! A file record is an ordered list of terms; each term is a chunk range `[start, end)` of one
//! xorb whose bytes are `unpacked` long. A byte range of the file maps onto the terms it overlaps;
//! each overlapping term costs one xorb GET, one decode on the blocking pool, and a slice. Terms
//! are fetched one after another so memory stays one xorb (≤ 64 MiB) per download.
//!
//! ponytail: one whole-xorb GET per term even when the term covers a few chunks; a ranged GET
//! driven by the `.idx` boundaries is the upgrade if downloads of small slices of big xorbs show
//! up in egress.

use crate::keys;
use crate::shards::FileRecord;
use bytes::Bytes;
use futures::stream::{self, Stream, StreamExt};
use slatedb::object_store::ObjectStore;
use std::io;
use std::sync::Arc;
use xet_core_structures::xorb_object::XorbObject;

/// Bytes `[start, end)` of the file `rec` describes. `end` past the file is clamped by the terms
/// themselves: nothing is produced past the last term.
pub fn file_bytes(
    os: Arc<dyn ObjectStore>,
    owner: String,
    rec: FileRecord,
    start: u64,
    end: u64,
) -> impl Stream<Item = Result<Bytes, io::Error>> + Send + 'static {
    let mut plan = Vec::new();
    let mut at = 0u64;
    for t in rec.terms {
        let (a, b) = (at, at + t.unpacked as u64);
        at = b;
        if b <= start || a >= end {
            continue;
        }
        let skip = (start.max(a) - a) as usize;
        let take = (end.min(b) - start.max(a)) as usize;
        plan.push((t, skip, take));
    }
    stream::iter(plan).then(move |(t, skip, take)| {
        let os = os.clone();
        let owner = owner.clone();
        async move {
            let Some(hash) = keys::parse_hash(&t.xorb) else {
                return Err(io::Error::other(format!("bad xorb hash in file record: {}", t.xorb)));
            };
            let body = os.get(&keys::xorb(&owner, &hash)).await.map_err(io::Error::other)?.bytes().await.map_err(io::Error::other)?;
            let bytes = tokio::task::spawn_blocking(move || -> Result<Vec<u8>, io::Error> {
                let mut r = io::Cursor::new(&body[..]);
                let x = XorbObject::deserialize(&mut r).map_err(|e| io::Error::other(e.to_string()))?;
                x.get_bytes_by_chunk_range(&mut r, t.start, t.end).map_err(|e| io::Error::other(e.to_string()))
            })
            .await
            .map_err(io::Error::other)??;
            if skip + take > bytes.len() {
                return Err(io::Error::other(format!("xorb {} term shorter than its record", t.xorb)));
            }
            Ok(Bytes::from(bytes).slice(skip..skip + take))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::file_bytes;
    use crate::shards::{put_shard, read_file};
    use crate::testkit::build;
    use crate::xorbs::put_xorb;
    use bytes::Bytes;
    use futures::TryStreamExt;
    use slatedb::object_store::memory::InMemory;
    use slatedb::object_store::ObjectStore;
    use std::sync::Arc;

    async fn collect(os: &Arc<dyn ObjectStore>, rec: &crate::shards::FileRecord, s: u64, e: u64) -> Vec<u8> {
        let parts: Vec<Bytes> = file_bytes(os.clone(), "alice".into(), rec.clone(), s, e).try_collect().await.unwrap();
        parts.concat()
    }

    #[tokio::test]
    async fn streams_whole_and_ranged() {
        let os: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let b = build(&[b"hello ".as_slice(), b"xet ".as_slice(), b"world".as_slice()]);
        put_xorb(&*os, "alice", &b.xorb_hash, Bytes::from(b.xorb.clone())).await.unwrap();
        put_shard(&*os, "alice", &b.shard).await.unwrap();
        let rec = read_file(&*os, "alice", &b.file_hash).await.unwrap().unwrap();
        assert_eq!(collect(&os, &rec, 0, rec.size).await, b.bytes);
        assert_eq!(collect(&os, &rec, 3, 9).await, b.bytes[3..9]);
        assert_eq!(collect(&os, &rec, 0, 1).await, b.bytes[..1]);
        let n = b.bytes.len() as u64;
        assert_eq!(collect(&os, &rec, n - 1, n + 50).await, b.bytes[b.bytes.len() - 1..]);
        assert!(collect(&os, &rec, n, n + 1).await.is_empty());
    }
}
```

Add `pub mod stream;` to `crates/xet/src/lib.rs`. If `FileRecord` or `Term` lacks `#[derive(Clone)]`, add it in `shards.rs`.

- [ ] **Step 2: Run it to verify it passes or fails for the right reason**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test -p kloudlite-xet stream 2>&1 | tail -15`
Expected: the test compiles against Tasks 3–5 and PASSES (this step writes the code with the test; the failing test that drives the hub side is Step 3). A compile error naming `Clone` means the derive above is missing.

- [ ] **Step 3: Write the failing resolve tests**

Append to `tests/hub_api.rs`:

```rust
use slatedb::object_store::ObjectStore;

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
```

`hex` and `bytes` must be root dev-dependencies (`hex = { workspace = true }`); add `hex` if `Cargo.toml`'s root `[dev-dependencies]` lacks it. `b.xorb.clone().into()` converts `Vec<u8>` to `Bytes`.

- [ ] **Step 4: Run them to verify they fail**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test --test hub_api resolve 2>&1 | tail -20`
Expected: FAIL, every `resolve_*` test gets 404 with no `x-repo-commit` (no route yet).

- [ ] **Step 5: Write the handler**

`crates/hub/src/resolve.rs`:

```rust
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
use slatedb::object_store::path::Path as ObjPath;
use slatedb::object_store::{GetOptions, GetRange, ObjectStore};

/// Whether `p` exists; a store failure is a 500, never "absent".
pub async fn exists(os: &dyn ObjectStore, p: &ObjPath) -> Result<bool, Response> {
    match os.head(p).await {
        Ok(_) => Ok(true),
        Err(slatedb::object_store::Error::NotFound { .. }) => Ok(false),
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
    let mut r = serve(&st, kind, &a.owner, &a.name, commit, &tag, &path, &method, &headers).await;
    set(&mut r, "x-repo-commit", &tag);
    r
}

#[allow(clippy::too_many_arguments)]
async fn serve(
    st: &HubState,
    kind: HubKind,
    owner: &str,
    name: &str,
    commit: gix_hash::ObjectId,
    tag: &str,
    path: &str,
    method: &Method,
    headers: &HeaderMap,
) -> Response {
    if !safe_path(path) {
        return entry_not_found();
    }
    let repo = match st.app.store.hub_open(kind, owner, name).await {
        Ok(Some(r)) => r,
        Ok(None) => return entry_not_found(),
        Err(e) => return internal(e),
    };
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
        set(&mut r, "etag", &format!("\"{}\"", blob.oid));
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
```

The handler reopens the repo inside `serve` only because `Access` is not `Clone`; if Task 7's `Access` derives `Clone` or `Repo` is reachable by reference, pass `a.repo.clone()` in instead and drop the `hub_open` call — behaviour is identical. `set` takes `&'static str` keys, which `HeaderMap::insert` accepts as `HeaderName`; if the compiler refuses, use `header::HeaderName::from_static(k)`.

In `lib.rs`: add `pub mod resolve;`, rename `_repo` to `repo` in `per_kind`, and add before `.layer(axum::Extension(kind))`:

```rust
        .route(&format!("{repo}/resolve/{{rev}}/{{*path}}"), get(resolve::resolve))
```

`get` already answers HEAD in axum; the handler skips the body itself so a HEAD never fetches a xorb.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test --test hub_api 2>&1 | tail -15; CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test -p kloudlite-xet -p kloudlite-hub 2>&1 | grep -E "test result|FAILED|panicked" | head`
Expected: PASS for every `resolve_*` test and all earlier tests.

If `resolve_git_blob_whole_ranged_and_head` fails only on the HEAD `content-length` (hyper rewriting it to 0 for an empty body), set the body for HEAD to `Body::empty()` and keep the header — hyper keeps an explicit `content-length` on HEAD responses; if it does not, report the observed header in the task report instead of weakening the assertion.

- [ ] **Step 7: Clippy and commit**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -5`
Expected: no warnings.

```bash
cd /Volumes/kdisk/rustic-git-wt/master && git add crates/xet/src/stream.rs crates/xet/src/lib.rs crates/xet/src/shards.rs crates/hub/src/resolve.rs crates/hub/src/lib.rs tests/hub_api.rs Cargo.toml
cd /Volumes/kdisk/rustic-git-wt/master && git commit -m "Serve hub downloads from git, the LFS store and Xet"
```

---

### Task 10: Preupload and NDJSON commits

**Files:**
- Create: `crates/hub/src/commit.rs`
- Modify: `crates/hub/src/lib.rs` (`pub mod commit;`, two routes)
- Test: `crates/hub/src/commit.rs` (unit), `tests/hub_api.rs`

**Interfaces:**
- Consumes: `auth::open` (write), `rev::{revision_not_found, safe_path}`, `pointer::text`, `resolve::exists`, `hub_err`, `internal`, `HubState`; `kloudlite_gitbase::objects::{apply_changes, write_commit, Change, NewCommit, Staging}`; `kloudlite_gitbase::refs::UpdateRefsExt`; `kloudlite_storage::refmeta::RefUpdate { name, old, new }`; `Store::{get_ref, list_refs}`; `kloudlite_vcs::browse::{blob_at, files_at, is_not_found}`; `kloudlite_xet::keys::{sha, lfs}`.
- Produces:
  - `commit::{LFS_THRESHOLD: u64 = 10_485_760, COMMIT_MAX: usize = 512 MiB, preupload (handler), commit (handler), lfs_line(path) -> String, with_lfs_lines(base: &str, paths: &[String]) -> String, parse_ops(body: &[u8]) -> Result<Ops, String>}`.
  - `POST /api/{plural}/{owner}/{name}/preupload/{rev}` and `POST /api/{plural}/{owner}/{name}/commit/{rev}`. Task 12's LFS batch and Task 15's e2e drive them.

Rulings this task applies:
- Both routes need write access (`open(.., true)`): anonymous 401, signed-in stranger 404 on private / 403 on public.
- Preupload body `{"files":[{"path","size","sample"}]}` (`sample` base64 of the first 512 bytes). Reply `{"files":[{"path","uploadMode":"lfs"|"regular","shouldIgnore":false}],"commitOid":<head or null>}`. `lfs` when `size >= 10485760` or the decoded sample has a NUL byte. Unsafe path 400. Undecodable sample counts as binary (`lfs`).
- Commit body is NDJSON, one `{"key","value"}` per line; blank lines skipped. Keys: `header` (`summary`, `description`, optional `parentCommit`), `file` (`path`, `content`, `encoding` must be `base64`), `lfsFile` (`path`, `algo` must be `sha256`, `oid` 64 lowercase hex, `size`), `deletedFile` (`path`), `deletedFolder` (`path`, trailing `/` trimmed). Anything else, a malformed line, or an unsafe path is 400 naming the line or path. No header line is fine; the summary defaults to `Commit via API`.
- `?create_pr=1` (or `true`) is 404 "pull requests are not served", checked before auth.
- The branch is `refs/heads/{rev}`. Absent branch: allowed only when the repo has no refs at all and `rev == "main"` (first commit); else 404 `RevisionNotFound`.
- `parentCommit` present and not the branch head: 412 `"A commit has happened since. Please refresh and try again."`. The ref CAS (`update_refs` old = head) refusing is the same 412: that is the concurrent case.
- `lfsFile` whose sha has neither `xet/{owner}/sha/{oid}` nor `lfs/{owner}/sha256/{oid}`: 422 `lfsFile {path}: no uploaded object with sha256 {oid}`. The stored blob is `pointer::text(oid, size)`.
- `.gitattributes`: when the commit has any `lfsFile`, start from the commit's own `.gitattributes` `file` op if present, else the parent tree's `.gitattributes`, else empty, and append `lfs_line(path)` for each LFS path not already present as a whole line. Spaces escape as `[[:space:]]` (git-lfs's own form); `* ? [ \ # !` get a backslash.
- `deletedFolder` expands to every file under that folder in the parent tree; a folder with nothing under it is a no-op. A commit whose change set ends up empty is 400 "nothing to commit".
- Author and committer: the caller, email `{caller}@users.noreply.kloudlite`, time now (epoch seconds). Message: summary, plus `\n\n{description}` when non-empty.
- Reply 200 `{"commitUrl":"{hub base}/{prefix}{owner}/{name}/commit/{oid}","commitOid":"{oid}","pullRequestUrl":null}`.
- ponytail: the whole NDJSON body is buffered (cap `COMMIT_MAX` = 512 MiB); inline files are small by the preupload rule, so a streaming parser waits for a client that inlines big files.

- [ ] **Step 1: Write the failing unit tests**

`crates/hub/src/commit.rs` starts with its tests (the module below them is Step 3):

```rust
#[cfg(test)]
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
```

- [ ] **Step 2: Write the failing integration tests**

Append to `tests/hub_api.rs`:

```rust
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
    let r = hub_commit(&h, Some(&h.alice), "models/alice/bert/commit/main", &[lfs.clone()]).await;
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
```

- [ ] **Step 3: Run them to verify they fail**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test --test hub_api 2>&1 | grep -E "^test |test result" | tail -25`
Expected: FAIL (compile error in `crates/hub` for the unit tests until Step 4; the integration tests get 404/405 from the missing routes).

- [ ] **Step 4: Write the module**

Put this above the `#[cfg(test)]` block in `crates/hub/src/commit.rs`:

```rust
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

fn stale() -> Response {
    hub_err(StatusCode::PRECONDITION_FAILED, None, "A commit has happened since. Please refresh and try again.")
}

fn tree_of(odb: &gix_odb::Handle, commit: gix_hash::ObjectId) -> Option<gix_hash::ObjectId> {
    use gix_object::Find;
    let mut buf = Vec::new();
    let c = odb.try_find(&commit, &mut buf).ok()??;
    gix_object::CommitRef::from_bytes(c.data).ok().map(|c| c.tree())
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
        let mut found = false;
        for key in [kloudlite_xet::keys::sha(&a.owner, oid), kloudlite_xet::keys::lfs(&a.owner, oid)] {
            match exists(&*st.cas.os, &key).await {
                Ok(true) => {
                    found = true;
                    break;
                }
                Ok(false) => {}
                Err(r) => return r,
            }
        }
        if !found {
            return hub_err(StatusCode::UNPROCESSABLE_ENTITY, None, &format!("lfsFile {path}: no uploaded object with sha256 {oid}"));
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
    let url = format!("{}/{}{owner}/{name}/commit/{new}", st.base, kind.url_prefix());
    Json(json!({ "commitUrl": url, "commitOid": new.to_string(), "pullRequestUrl": null })).into_response()
}
```

`files_at(.., prefix, cap, sizes)` is assumed to return entries whose `name` is the full repo path (Task 8 relies on the same); if it returns names relative to `prefix`, join them as `format!("{f}/{}", e.name)`. `Staging::write` consumes `self`; the binding above is moved into it. If `Staging` is not `Send`, build and write it inside one `spawn_blocking` plus `Handle::current().block_on` is NOT the fix: report it instead.

In `lib.rs` add `pub mod commit;` and, in `per_kind` before `.layer(axum::Extension(kind))`:

```rust
        .route(&format!("{api}/preupload/{{rev}}"), post(commit::preupload))
        .route(
            &format!("{api}/commit/{{rev}}"),
            post(commit::commit).layer(axum::extract::DefaultBodyLimit::max(commit::COMMIT_MAX)),
        )
```

Add `gix-hash`, `gix-object`, `gix-odb` and `base64` to `crates/hub/Cargo.toml` if Task 7 left any out (Task 7 lists all four).

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test -p kloudlite-hub 2>&1 | grep -E "test result|FAILED|panicked" | head; CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test --test hub_api 2>&1 | grep -E "test result|FAILED|panicked" | head`
Expected: PASS, both suites.

- [ ] **Step 6: Clippy and commit**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -5`
Expected: no warnings.

```bash
cd /Volumes/kdisk/rustic-git-wt/master && git add crates/hub/src/commit.rs crates/hub/src/lib.rs crates/hub/Cargo.toml tests/hub_api.rs
cd /Volumes/kdisk/rustic-git-wt/master && git commit -m "Accept hub preupload and NDJSON commits"
```

### Task 11: Branches, tags and Xet tokens

**Files:**
- Create: `crates/hub/src/refs.rs`, `crates/hub/src/xet.rs`
- Modify: `crates/hub/src/lib.rs` (modules, routes), `crates/hub/src/read.rs` (`commit_of` becomes `pub(crate)`)
- Test: `tests/hub_api.rs`

**Interfaces:**
- Consumes: `auth::open(&HubState, HubKind, Option<&Trusted>, &HeaderMap, &str, &str, bool) -> Result<Access, Response>`; `rev::{resolve, revision_not_found, safe_path}`; `read::commit_of(&HubState, &Access, &str) -> Result<Option<ObjectId>, Response>`; `hub_err`, `internal`; `Jwt::mint_cas(owner, repo, rev, write, ttl)`; `kloudlite_storage::hub_routing_key(kind, owner, name)`; `UpdateRefsExt::update_refs`; `RefUpdate { name, old, new }`.
- Produces: `refs::{ref_ok(&str) -> bool, branch_create, branch_delete, tag_create, tag_delete}`; `xet::{read_token, write_token, token_reply(&HubState, &Access, &str, bool) -> Response}` (Task 12 calls `token_reply`'s headers through `xet::token_headers(&HubState, &Access, &str, bool) -> (String, String, u64)` = `(cas_url, token, exp)`).

- [ ] **Step 1: Write the failing tests**

Append to `tests/hub_api.rs`:

```rust
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
    let refs: Value = client().get(format!("{}/api/models/alice/m/refs", h.base)).send().await.unwrap().json().await.unwrap();
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
    let info: Value = client().get(format!("{}/api/datasets/alice/d/revision/v1", h.base)).send().await.unwrap().json().await.unwrap();
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
```

The `refs` response shape (`branches[].name`, `targetCommit`) is Task 8's; if Task 8 named them differently, use Task 8's names.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test --test hub_api 2>&1 | grep -E "test result|FAILED" | head`
Expected: the six new tests FAIL (404 from the router).

- [ ] **Step 3: Implement**

`crates/hub/src/refs.rs`:

```rust
//! Branch and tag create/delete. Every change is one `update_refs` call with an expected old
//! value, so a concurrent writer loses with 409/412 instead of being overwritten.
//!
//! Tags are lightweight: huggingface_hub's `create_tag` sends a message, which we accept and drop
//! (`// ponytail:` below).

use crate::auth::open;
use crate::read::commit_of;
use crate::rev::{resolve, revision_not_found, safe_path};
use crate::{hub_err, internal, HubState};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use kloudlite_gitbase::refs::UpdateRefsExt;
use kloudlite_server_auth::Trusted;
use kloudlite_storage::refmeta::RefUpdate;
use kloudlite_storage::HubKind;
use serde::Deserialize;

/// git's check-ref-format rules that matter for a single path segment we put under `refs/`.
pub fn ref_ok(name: &str) -> bool {
    safe_path(name)
        && !name.contains("..")
        && !name.contains("@{")
        && !name.ends_with(".lock")
        && !name.ends_with('.')
        && !name.bytes().any(|b| b < 0x20 || b == 0x7f || b" ~^:?*[\\".contains(&b))
}

fn bad_name() -> Response {
    hub_err(StatusCode::BAD_REQUEST, None, "invalid reference name")
}

#[derive(Deserialize, Default)]
pub struct BranchBody {
    #[serde(rename = "startingPoint")]
    starting_point: Option<String>,
}

#[derive(Deserialize)]
pub struct TagBody {
    tag: String,
    // ponytail: annotated tags are not written; the message is dropped. Write a tag object when a
    // client is seen to read it back.
    #[allow(dead_code)]
    message: Option<String>,
}

async fn set(st: &HubState, a: &crate::auth::Access, name: String, old: Option<gix_hash::ObjectId>, new: Option<gix_hash::ObjectId>) -> Response {
    match st.app.store.update_refs(&a.repo, &[RefUpdate { name, old, new }]).await {
        Ok(v) if v.iter().all(|x| x.is_none()) => {
            crate::live::after_update(st, a).await;
            Json(serde_json::json!({})).into_response()
        }
        Ok(_) => hub_err(StatusCode::CONFLICT, None, "reference changed concurrently"),
        Err(e) => internal(e),
    }
}

pub async fn branch_create(
    State(st): State<HubState>,
    Extension(kind): Extension<HubKind>,
    trusted: Option<Extension<Trusted>>,
    headers: HeaderMap,
    Path((owner, name, branch)): Path<(String, String, String)>,
    body: Option<Json<BranchBody>>,
) -> Response {
    let a = match open(&st, kind, trusted.as_deref(), &headers, &owner, &name, true).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    if !ref_ok(&branch) {
        return bad_name();
    }
    let start = body.and_then(|b| b.0.starting_point).unwrap_or_else(|| "main".into());
    let at = match resolve(&st.app.store, &a.repo, &start).await {
        Ok(Some(o)) => o,
        Ok(None) => return revision_not_found(&start),
        Err(e) => return internal(e),
    };
    let full = format!("refs/heads/{branch}");
    match st.app.store.get_ref(&a.repo, &full).await {
        Ok(Some(_)) => return hub_err(StatusCode::CONFLICT, None, "branch already exists"),
        Ok(None) => {}
        Err(e) => return internal(e),
    }
    set(&st, &a, full, None, Some(at)).await
}

pub async fn branch_delete(
    State(st): State<HubState>,
    Extension(kind): Extension<HubKind>,
    trusted: Option<Extension<Trusted>>,
    headers: HeaderMap,
    Path((owner, name, branch)): Path<(String, String, String)>,
) -> Response {
    let a = match open(&st, kind, trusted.as_deref(), &headers, &owner, &name, true).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    if branch == "main" {
        return hub_err(StatusCode::FORBIDDEN, None, "the default branch cannot be deleted");
    }
    if !ref_ok(&branch) {
        return bad_name();
    }
    let full = format!("refs/heads/{branch}");
    match st.app.store.get_ref(&a.repo, &full).await {
        Ok(Some(old)) => set(&st, &a, full, Some(old), None).await,
        Ok(None) => revision_not_found(&branch),
        Err(e) => internal(e),
    }
}

pub async fn tag_create(
    State(st): State<HubState>,
    Extension(kind): Extension<HubKind>,
    trusted: Option<Extension<Trusted>>,
    headers: HeaderMap,
    Path((owner, name, rev)): Path<(String, String, String)>,
    Json(body): Json<TagBody>,
) -> Response {
    let a = match open(&st, kind, trusted.as_deref(), &headers, &owner, &name, true).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    if !ref_ok(&body.tag) {
        return bad_name();
    }
    let at = match commit_of(&st, &a, &rev).await {
        Ok(Some(o)) => o,
        Ok(None) => return revision_not_found(&rev),
        Err(r) => return r,
    };
    let full = format!("refs/tags/{}", body.tag);
    match st.app.store.get_ref(&a.repo, &full).await {
        Ok(Some(_)) => hub_err(StatusCode::CONFLICT, None, "tag already exists"),
        Ok(None) => set(&st, &a, full, None, Some(at)).await,
        Err(e) => internal(e),
    }
}

pub async fn tag_delete(
    State(st): State<HubState>,
    Extension(kind): Extension<HubKind>,
    trusted: Option<Extension<Trusted>>,
    headers: HeaderMap,
    Path((owner, name, tag)): Path<(String, String, String)>,
) -> Response {
    let a = match open(&st, kind, trusted.as_deref(), &headers, &owner, &name, true).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    if !ref_ok(&tag) {
        return bad_name();
    }
    let full = format!("refs/tags/{tag}");
    match st.app.store.get_ref(&a.repo, &full).await {
        Ok(Some(old)) => set(&st, &a, full, Some(old), None).await,
        Ok(None) => revision_not_found(&tag),
        Err(e) => internal(e),
    }
}
```

Use the same `Trusted` import path `read.rs` uses (the line above names it `kloudlite_server_auth::Trusted` only as a stand-in for whatever Task 7 imported); likewise `get_ref` is the accessor `seed_hub` uses. `crate::live::after_update` lands in Task 13: until then, add a one-line stub `pub async fn after_update(_st: &crate::HubState, _a: &crate::auth::Access) {}` in `crates/hub/src/live.rs` with `pub mod live;` in `lib.rs`; Task 13 replaces the body.

`crates/hub/src/xet.rs`:

```rust
//! Xet CAS tokens. huggingface_hub asks for one before every Xet upload or download and reads the
//! three `X-Xet-*` headers (newer clients read the JSON body). The token names the hub routing key
//! as `repo`, so a CAS request can be traced back to the repo that authorised it.

use crate::auth::{open, Access};
use crate::read::commit_of;
use crate::rev::revision_not_found;
use crate::HubState;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use kloudlite_storage::{hub_routing_key, HubKind};

pub const TOKEN_TTL: u64 = 3600;

pub fn token_headers(st: &HubState, a: &Access, rev: &str, write: bool) -> (String, String, u64) {
    let exp = kloudlite_xet::sign::now() + TOKEN_TTL;
    let token = st.app.jwt.mint_cas(&a.owner, &hub_routing_key(a.kind, &a.owner, &a.name), rev, write, TOKEN_TTL);
    (st.cas.base.clone(), token, exp)
}

fn reply(st: &HubState, a: &Access, rev: &str, write: bool) -> Response {
    let (cas, token, exp) = token_headers(st, a, rev, write);
    let mut r = Json(serde_json::json!({ "accessToken": token, "exp": exp, "casUrl": cas })).into_response();
    let h = r.headers_mut();
    for (k, v) in [("x-xet-cas-url", cas.clone()), ("x-xet-access-token", token.clone()), ("x-xet-token-expiration", exp.to_string())] {
        if let Ok(v) = HeaderValue::from_str(&v) {
            h.insert(k, v);
        }
    }
    r
}

async fn token(st: HubState, kind: HubKind, trusted: Option<Extension<kloudlite_server_auth::Trusted>>, headers: HeaderMap, owner: String, name: String, rev: String, write: bool) -> Response {
    let a = match open(&st, kind, trusted.as_deref(), &headers, &owner, &name, write).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    // A write token for a branch that does not exist yet is fine: the commit creates it.
    if !write {
        match commit_of(&st, &a, &rev).await {
            Ok(Some(_)) | Ok(None) => {}
            Err(r) => return r,
        }
    } else if !crate::refs::ref_ok(&rev) {
        return revision_not_found(&rev);
    }
    reply(&st, &a, &rev, write)
}

pub async fn read_token(
    State(st): State<HubState>,
    Extension(kind): Extension<HubKind>,
    trusted: Option<Extension<kloudlite_server_auth::Trusted>>,
    headers: HeaderMap,
    Path((owner, name, rev)): Path<(String, String, String)>,
) -> Response {
    token(st, kind, trusted, headers, owner, name, rev, false).await
}

pub async fn write_token(
    State(st): State<HubState>,
    Extension(kind): Extension<HubKind>,
    trusted: Option<Extension<kloudlite_server_auth::Trusted>>,
    headers: HeaderMap,
    Path((owner, name, rev)): Path<(String, String, String)>,
) -> Response {
    token(st, kind, trusted, headers, owner, name, rev, true).await
}
```

`token` has eight arguments: add `#[allow(clippy::too_many_arguments)]` above it (as `read::info_at` does if Task 8 needed it). `mint_cas`'s exact argument types are Task 2's; match them. The `Ok(None)` read-token arm is the empty-repo `main` case `commit_of` already allows.

In `lib.rs`: `pub mod live; pub mod refs; pub mod xet;`, make `read::commit_of` `pub(crate)`, and in `per_kind` before the layer:

```rust
        .route(&format!("{api}/branch/{{branch}}"), post(refs::branch_create).delete(refs::branch_delete))
        .route(&format!("{api}/tag/{{rev}}"), post(refs::tag_create).delete(refs::tag_delete))
        .route(&format!("{api}/xet-read-token/{{rev}}"), get(xet::read_token))
        .route(&format!("{api}/xet-write-token/{{rev}}"), get(xet::write_token))
```

`DELETE /tag/{tag}` and `POST /tag/{rev}` share one path; the segment means the tag on delete and the revision on create, which is what huggingface_hub sends.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test --test hub_api 2>&1 | grep -E "test result|FAILED|panicked" | head`
Expected: PASS.

- [ ] **Step 5: Clippy and commit**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -5`
Expected: clean.

```bash
cd /Volumes/kdisk/rustic-git-wt/master && git add crates/hub tests/hub_api.rs
cd /Volumes/kdisk/rustic-git-wt/master && git commit -m "Manage hub branches and tags and mint Xet tokens"
```

### Task 12: git-lfs batch bridge

**Files:**
- Create: `crates/hub/src/lfs.rs`
- Modify: `crates/hub/src/lib.rs` (route), `crates/xet/src/lfs.rs` (Task 6 GET falls back to Xet)
- Test: `tests/hub_api.rs`, `tests/xet_cas.rs` (the Task 5/6 CAS test file; use its actual name)

**Interfaces:**
- Consumes: `auth::open`; `xet::token_headers`; `kloudlite_xet::lfs::lfs_url(&CasState, owner, oid, ttl) -> String`; `kloudlite_xet::keys::{sha, lfs, parse_hash}`; `kloudlite_xet::shard::read_file(os, owner, &MerkleHash) -> Result<Option<FileRecord>>`; `kloudlite_xet::stream::file_bytes(os, owner, &FileRecord, start, end)`.
- Produces: `lfs::batch` on `POST /{prefix}{owner}/{repo}/info/lfs/objects/batch` (`{repo}` may end `.git`).

- [ ] **Step 1: Write the failing tests**

Append to `tests/hub_api.rs`:

```rust
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
```

Add `sha2` and `hex` to the root package's `[dev-dependencies]` if missing. Append to the CAS test file (Task 6's LFS tests live there; reuse its helpers for building a file with `kloudlite_xet::testutil::build` and posting xorb + shard):

```rust
#[tokio::test]
async fn lfs_get_serves_a_xet_file_by_sha256() {
    let c = cas().await;
    let built = kloudlite_xet::testutil::build(&[b"hello ", b"xet"]);
    post_xorb_and_shard(&c, "alice", &built).await;
    let oid = sha256_hex(&built.bytes);
    let url = kloudlite_xet::lfs::lfs_url(&c.st, "alice", &oid, 60);
    let got = reqwest::get(&url).await.unwrap();
    assert_eq!(got.status(), 200);
    assert_eq!(got.bytes().await.unwrap().to_vec(), built.bytes);
}
```

Name `cas()`, `c.st`, `post_xorb_and_shard` and the `build` path after what Tasks 3–6 actually defined.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test --test hub_api lfs_ 2>&1 | grep -E "test result|FAILED" | head`
Expected: FAIL (404 route; the CAS Xet fallback test 404 "no such object").

- [ ] **Step 3: Implement the CAS fallback**

In `crates/xet/src/lfs.rs` `get_lfs`, replace the `NotFound` arm:

```rust
        Err(slatedb::object_store::Error::NotFound { .. }) => match xet_bytes(&st, &owner, &oid).await {
            Ok(Some(body)) => (StatusCode::OK, body).into_response(),
            Ok(None) => refuse(StatusCode::NOT_FOUND, "no such object"),
            Err(e) => internal(e),
        },
```

and add:

```rust
/// A Xet-uploaded file has no whole object: `sha/{sha256}` names its file hash, whose record
/// lists the xorb ranges to stream. git-lfs clients see the same bytes either way.
async fn xet_bytes(st: &CasState, owner: &str, oid: &str) -> Result<Option<Body>, String> {
    let fh = match st.os.get(&keys::sha(owner, oid)).await {
        Ok(g) => g.bytes().await.map_err(|e| e.to_string())?,
        Err(slatedb::object_store::Error::NotFound { .. }) => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let Some(h) = keys::parse_hash(String::from_utf8_lossy(&fh).trim()) else { return Err("corrupt sha record".into()) };
    let Some(rec) = crate::shard::read_file(&*st.os, owner, &h).await.map_err(|e| e.to_string())? else { return Ok(None) };
    let size = rec.size;
    Ok(Some(crate::stream::file_bytes(st.os.clone(), owner, &rec, 0, size).await?))
}
```

`file_bytes`'s exact signature and return type are Task 9's: if it returns a stream, wrap it in `Body::from_stream`; if it takes `&dyn ObjectStore` rather than `Arc`, pass `&*st.os`. `read_file`'s module path is Task 4's.

- [ ] **Step 4: Implement the batch handler**

`crates/hub/src/lfs.rs`:

```rust
//! The git-lfs batch API, so `git clone` / `git push` with git-lfs (or git-xet) work against hub
//! repos. Only the batch call lives here, on the owning node, because it checks visibility; bytes
//! move through signed URLs on the CAS listener (`kloudlite_xet::lfs`), which any node serves.
//!
//! Per-object problems (unknown oid, malformed oid) are per-object `error`s with 200 overall, as
//! the LFS spec requires; only auth failures fail the whole call.

use crate::auth::open;
use crate::{hub_err, internal, HubState};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Extension;
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

fn is_oid(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn lfs_reply(code: StatusCode, v: Value) -> Response {
    (code, [("content-type", LFS_JSON)], v.to_string()).into_response()
}

async fn stored(st: &HubState, owner: &str, oid: &str) -> Result<bool, String> {
    use slatedb::object_store::Error::NotFound;
    for key in [kloudlite_xet::keys::sha(owner, oid), kloudlite_xet::keys::lfs(owner, oid)] {
        match st.cas.os.head(&key).await {
            Ok(_) => return Ok(true),
            Err(NotFound { .. }) => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(false)
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
    trusted: Option<Extension<kloudlite_server_auth::Trusted>>,
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
        if !is_oid(&o.oid) {
            out.push(json!({ "oid": o.oid, "size": o.size, "error": { "code": 422, "message": "oid is not a sha256" } }));
            continue;
        }
        let have = match stored(&st, &owner, &o.oid).await {
            Ok(b) => b,
            Err(e) => return internal(e),
        };
        let entry = match (write, have) {
            (false, false) => json!({ "oid": o.oid, "size": o.size, "error": { "code": 404, "message": "object not found" } }),
            (false, true) => json!({ "oid": o.oid, "size": o.size, "authenticated": true, "actions": { "download": {
                "href": kloudlite_xet::lfs::lfs_url(&st.cas, &owner, &o.oid, HREF_TTL), "expires_in": HREF_TTL } } }),
            (true, true) => json!({ "oid": o.oid, "size": o.size, "authenticated": true }),
            (true, false) if use_xet => {
                let (cas, token, exp) = crate::xet::token_headers(&st, &a, &branch, true);
                let href = format!("{}/api/{}/{owner}/{name}/xet-write-token/{branch}", st.base, kind.api_plural());
                json!({ "oid": o.oid, "size": o.size, "authenticated": true, "actions": { "upload": {
                    "href": href, "expires_in": HREF_TTL,
                    "header": { "X-Xet-Cas-Url": cas, "X-Xet-Access-Token": token, "X-Xet-Token-Expiration": exp.to_string() } } } })
            }
            (true, false) => json!({ "oid": o.oid, "size": o.size, "authenticated": true, "actions": { "upload": {
                "href": kloudlite_xet::lfs::lfs_url(&st.cas, &owner, &o.oid, HREF_TTL), "expires_in": HREF_TTL } } }),
        };
        out.push(entry);
    }
    lfs_reply(StatusCode::OK, json!({ "transfer": if use_xet { "xet" } else { "basic" }, "objects": out, "hash_algo": "sha256" }))
}

pub async fn locks() -> Response {
    hub_err(StatusCode::NOT_FOUND, None, "locking is not supported")
}
```

`open` already maps Basic `user:PAT` (user must match) and returns 401 anonymous / 403 or 404 per visibility; the tests pin those codes. If `open` returns 404 for a signed-in stranger writing to a PUBLIC repo rather than 403, Task 7's rule wins and the test changes to that code. `Trusted`'s path: as in Task 11.

In `lib.rs`: `pub mod lfs;` and in `per_kind`:

```rust
        .route(&format!("{repo}/info/lfs/objects/batch"), post(lfs::batch))
        .route(&format!("{repo}/info/lfs/locks"), get(lfs::locks).post(lfs::locks))
        .route(&format!("{repo}/info/lfs/locks/{{*rest}}"), get(lfs::locks).post(lfs::locks))
```

renaming `_repo` to `repo`. The model prefix is empty, so `/{owner}/{name}/info/...` sits beside `/{owner}/{name}/resolve/...` from Task 9; axum matches the longer literal segments.

- [ ] **Step 5: Run tests to verify they pass**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test -p kloudlite-xet 2>&1 | grep -E "test result|FAILED" | head; CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test --test hub_api 2>&1 | grep -E "test result|FAILED|panicked" | head`
Then the CAS test file the same way. Expected: PASS.

- [ ] **Step 6: Clippy and commit**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -5`

```bash
cd /Volumes/kdisk/rustic-git-wt/master && git add crates/hub crates/xet tests Cargo.toml Cargo.lock
cd /Volumes/kdisk/rustic-git-wt/master && git commit -m "Bridge git-lfs batch requests to the CAS"
```

### Task 13: Live lists and the hub GC sweep

**Files:**
- Create: `crates/xet/src/gc.rs`, `tests/hub_gc.rs`
- Modify: `crates/xet/src/keys.rs` (live keys, prefixes), `crates/xet/src/lib.rs` (`pub mod gc;`), `crates/hub/src/live.rs` (replace the Task 11 stub), `crates/hub/src/commit.rs` (call after a ref update), `crates/hub/src/repos.rs` (create writes an empty list first; delete drops it; use the file Task 7 put `create`/`delete` in), `bins/worker/src/main.rs` (lane)
- Test: `tests/hub_gc.rs`, `tests/hub_api.rs`

**Interfaces:**
- Consumes: `pointer::{parse, POINTER_MAX}`; `Repo::odb() -> gix_odb::Handle`; `Store::list_refs(&Repo)`; `shard::read_file`; `keys::{xorb, xorb_idx, file, sha, lfs, parse_hash}`; worker `owners_under(&Store, &str) -> Vec<String>` and `GC_OWNER_GAP`.
- Produces:
  - `kloudlite_xet::keys::{live(owner, tag, name) -> Path, live_prefix(owner) -> Path, kind_tag(HubKind) -> &'static str}` with tags `"mdl"` / `"ds"`.
  - `kloudlite_xet::gc::{LiveList { shas: BTreeSet<String> }, sweep_owner(&dyn ObjectStore, owner: &str, grace: Duration) -> Result<usize, String>}`.
  - `kloudlite_hub::live::{rebuild(store: &Store, repo: &Repo, owner: &str, kind: HubKind, name: &str) -> Result<(), String>, after_update(&HubState, &Access)}`. Task 14 calls `rebuild` after a hub `git push`.

- [ ] **Step 1: Write the failing tests**

`tests/hub_gc.rs`:

```rust
//! The hub sweep against an in-memory object store: live bytes stay, dead bytes past the grace go,
//! and any doubt (a repo without a live list, a list that changed mid-sweep) keeps everything.

use kloudlite_xet::gc::{sweep_owner, LiveList};
use kloudlite_xet::keys;
use object_store::memory::InMemory;
use object_store::{ObjectStore, PutPayload};
use sha2::{Digest, Sha256};
use std::time::Duration;

async fn upload(os: &InMemory, owner: &str, pieces: &[&[u8]]) -> (kloudlite_xet::testutil::Built, String) {
    let b = kloudlite_xet::testutil::build(pieces);
    kloudlite_xet::xorb::put_xorb(os, owner, &b.xorb_hash, b.xorb.clone().into()).await.unwrap();
    assert!(kloudlite_xet::shard::put_shard(os, owner, &b.shard).await.unwrap());
    let sha = hex::encode(Sha256::digest(&b.bytes));
    (b, sha)
}

async fn put_live(os: &InMemory, owner: &str, tag: &str, name: &str, shas: &[&str]) {
    let l = LiveList { shas: shas.iter().map(|s| s.to_string()).collect() };
    os.put(&keys::live(owner, tag, name), PutPayload::from(serde_json::to_vec(&l).unwrap())).await.unwrap();
    // The repo database exists: GC reads the repo set from the `repo/` prefix.
    os.put(&format!("repo/{tag}/{owner}/{name}/manifest").into(), PutPayload::from_static(b"x")).await.unwrap();
}

async fn exists(os: &InMemory, p: &object_store::path::Path) -> bool {
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
```

Use the crate paths Tasks 3–5 gave `put_xorb`, `put_shard` and `testutil::build`; the root package already has `object_store` through the workspace (add it to `[dev-dependencies]` with `workspace = true` if not). If `put_xorb` takes `&dyn ObjectStore`, pass `&os`.

Append to `tests/hub_api.rs`:

```rust
async fn live_of(h: &Hub, tag: &str, owner: &str, name: &str) -> Option<kloudlite_xet::gc::LiveList> {
    let os = h.e.app.store.os.clone();
    match os.get(&kloudlite_xet::keys::live(owner, tag, name)).await {
        Ok(g) => Some(serde_json::from_slice(&g.bytes().await.unwrap()).unwrap()),
        Err(_) => None,
    }
}

#[tokio::test]
async fn create_writes_an_empty_live_list_and_delete_drops_it() {
    let h = hub().await;
    assert_eq!(create(&h, Some(&h.alice), json!({"name": "m"})).await.status(), 200);
    assert!(live_of(&h, "mdl", "alice", "m").await.unwrap().shas.is_empty());
    let r = client().delete(format!("{}/api/repos/delete", h.base)).bearer_auth(&h.alice).json(&json!({"name": "m"})).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(live_of(&h, "mdl", "alice", "m").await.is_none());
}

#[tokio::test]
async fn ref_changes_rebuild_the_live_list() {
    let h = hub().await;
    let sha = "c".repeat(64);
    let ptr = kloudlite_hub::pointer::text(&sha, 10);
    seed_hub(&h, HubKind::Model, "alice", "m", false, &[("w.bin", ptr.as_bytes()), ("a.txt", b"plain")]).await;
    assert!(live_of(&h, "mdl", "alice", "m").await.is_none(), "seed_hub bypasses the API");
    assert_eq!(post_json(&h, "/api/models/alice/m/branch/dev", &h.alice, json!({})).await.status(), 200);
    let l = live_of(&h, "mdl", "alice", "m").await.unwrap();
    assert_eq!(l.shas.into_iter().collect::<Vec<_>>(), vec![sha]);
}
```

`h.e.app` is the `App` Task 7's `Hub` holds; use its field name.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test --test hub_gc 2>&1 | grep -E "^error|test result" | head -5`
Expected: compile error, `gc` and `keys::live` do not exist.

- [ ] **Step 3: Keys**

In `crates/xet/src/keys.rs`, beside `sha`/`lfs`:

```rust
/// `hub/{owner}/live/{mdl|ds}/{name}`: every sha256 any ref of that repo reaches. Written only by
/// the owning node; read only by the sweep.
pub fn live(owner: &str, tag: &str, name: &str) -> Path {
    Path::from(format!("hub/{owner}/live/{tag}/{name}"))
}

pub fn live_prefix(owner: &str) -> Path {
    Path::from(format!("hub/{owner}/live"))
}

pub fn kind_tag(kind: kloudlite_storage::HubKind) -> &'static str {
    match kind {
        kloudlite_storage::HubKind::Model => "mdl",
        kloudlite_storage::HubKind::Dataset => "ds",
    }
}
```

Add one `*_prefix(owner) -> Path` per existing key function the sweep lists: `sha_prefix`, `lfs_prefix`, `lfs_tmp_prefix`, `file_prefix`, `xorb_prefix`. Each returns the directory its sibling function writes under (read the sibling: if `sha` builds `xet/{owner}/sha/{hex}`, `sha_prefix` is `xet/{owner}/sha`). If `xorb_idx` writes beside the xorb with a `.idx` suffix, the sweep below already tells them apart by suffix; if it writes under its own directory, add `xorb_idx_prefix` and list it separately in Step 4. If `crates/xet` cannot depend on `kloudlite-storage`, drop `kind_tag` here and put it in `crates/hub/src/live.rs`.

- [ ] **Step 4: The sweep**

`crates/xet/src/gc.rs`:

```rust
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
use object_store::path::Path;
use object_store::{ObjectMeta, ObjectStore};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

#[derive(Serialize, Deserialize, Default, Debug, Clone, PartialEq)]
pub struct LiveList {
    pub shas: BTreeSet<String>,
}

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
                Err(object_store::Error::NotFound { .. }) => return Err(format!("{tag}/{owner}/{name} has no live list")),
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

    let dead_sha: Vec<ObjectMeta> = list(os, &keys::sha_prefix(owner)).await?
        .into_iter().filter(|m| !live.contains(&leaf(m)) && older(m, cutoff)).collect();
    let dead_lfs: Vec<ObjectMeta> = list(os, &keys::lfs_prefix(owner)).await?
        .into_iter().filter(|m| !live.contains(&leaf(m)) && older(m, cutoff)).collect();
    let dead_tmp: Vec<ObjectMeta> = list(os, &keys::lfs_tmp_prefix(owner)).await?
        .into_iter().filter(|m| older(m, tmp_cutoff)).collect();

    if read_live(os, owner).await?.1 != print {
        return Err("live lists changed during the sweep".into());
    }

    let mut n = 0;
    for m in &dead_sha {
        // Read the file hash before deleting the pointer to it, or the record is unreachable.
        let fh = os.get(&m.location).await.map_err(|e| e.to_string())?.bytes().await.map_err(|e| e.to_string())?;
        os.delete(&m.location).await.map_err(|e| e.to_string())?;
        n += 1;
        if let Some(h) = keys::parse_hash(String::from_utf8_lossy(&fh).trim()) {
            match os.delete(&keys::file(owner, &h)).await {
                Ok(()) | Err(object_store::Error::NotFound { .. }) => n += 1,
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
        if let Some(rec) = crate::shard::read_file(os, owner, &h).await.map_err(|e| e.to_string())? {
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
    use object_store::memory::InMemory;
    use object_store::PutPayload;

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
```

`Term.xorb` is the hex string (Task 4); `FileRecord.terms` is `pub`, make it so if not. If xorb leaves carry no hex-only name (for example a `.xorb` suffix), strip that suffix where `leaf` is compared. Add `chrono`, `futures` and `serde_json` to `crates/xet/Cargo.toml` with `workspace = true` if missing (`object_store`'s `last_modified` is `chrono::DateTime<Utc>`).

- [ ] **Step 5: Live list rebuild**

Replace `crates/hub/src/live.rs`:

```rust
//! The live list: every sha256 an LFS pointer reachable from any ref of a hub repo names, in all
//! history. The sweep (`kloudlite_xet::gc`) keeps exactly these and treats a missing list as
//! "keep everything", so a failed rebuild DELETES the list rather than leaving a stale one.
//!
//! ponytail: a full walk of every ref on every ref change, serialised process-wide. Fine for
//! model repos (few commits, few files); make it incremental (walk only new commits, full walk on
//! delete) when `hub.live.rebuilt` shows walks over a second.

use crate::auth::Access;
use crate::HubState;
use gix_hash::ObjectId;
use kloudlite_storage::store::{Repo, Store};
use kloudlite_storage::HubKind;
use kloudlite_xet::gc::LiveList;
use kloudlite_xet::keys;
use std::collections::{BTreeSet, HashSet};

static WALK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn walk(odb: &gix_odb::Handle, tips: Vec<ObjectId>) -> Result<BTreeSet<String>, String> {
    use gix_object::{Find, FindExt};
    use gix_odb::HeaderExt;
    let mut shas = BTreeSet::new();
    let (mut seen_c, mut seen_t, mut seen_b) = (HashSet::new(), HashSet::new(), HashSet::new());
    let mut commits = tips;
    let mut trees = Vec::new();
    let mut buf = Vec::new();
    while let Some(c) = commits.pop() {
        if !seen_c.insert(c) {
            continue;
        }
        let commit = odb.find_commit(&c, &mut buf).map_err(|e| e.to_string())?;
        trees.push(commit.tree());
        commits.extend(commit.parents());
    }
    while let Some(t) = trees.pop() {
        if !seen_t.insert(t) {
            continue;
        }
        let tree = odb.find_tree(&t, &mut buf).map_err(|e| e.to_string())?.to_owned();
        for e in tree.entries {
            if e.mode.is_tree() {
                trees.push(e.oid);
            } else if e.mode.is_blob() && seen_b.insert(e.oid) {
                let size = odb.header(e.oid).map_err(|e| e.to_string())?.size();
                if size as usize > crate::pointer::POINTER_MAX {
                    continue;
                }
                let mut b = Vec::new();
                let blob = odb.find_blob(&e.oid, &mut b).map_err(|e| e.to_string())?;
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
    let key = keys::live(owner, keys::kind_tag(kind), name);
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
```

Read gix's vendored source before trusting three names above (`~/.cargo/registry/src/*/gix-odb-0.*/src/`, `gix-object-0.*/src/`): the header lookup (`HeaderExt::header` or `Header::try_header` returning `Option<Header>` with `.size()`), `find_commit`/`find_tree`/`find_blob` on `FindExt`, and `EntryMode::is_tree`/`is_blob`. `list_refs` returns whatever Task 8's `refs` handler iterates; map it to the target ids. If `Pointer` names its field other than `sha256`, use that name. If a ref points at a tag object rather than a commit, peel it first (Task 11 writes only lightweight tags, so tips are commits).

Hooks:
- `crates/hub/src/commit.rs`, after the `update_refs` success arm (`Ok(v) if v.iter().all(|x| x.is_none()) => {}`), before building `url`: `crate::live::after_update(&st, &a).await;`.
- The create handler: before `hub_create`, `st.app.store.os.put(&keys::live(&owner, keys::kind_tag(kind), &name), serde_json::to_vec(&LiveList::default()).unwrap().into())`; on error, `internal(e)`. A crash between this put and `hub_create` leaves an empty list with no repo: it only keeps nothing extra.
- The delete handler: after `hub_delete` succeeds, delete the same key (`NotFound` is fine). A list left by a failed delete over-keeps, never under-keeps.

- [ ] **Step 6: Worker lane**

In `bins/worker/src/main.rs`, beside `gc_lane`:

```rust
const HUB_GC_GAP: std::time::Duration = std::time::Duration::from_secs(6 * 3600);
const HUB_GC_GRACE: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

/// Hub bytes (Xet and plain LFS). Owners come from both byte prefixes: an owner who only ever
/// used plain git-lfs has nothing under `xet/`.
async fn hub_gc_lane(store: &kloudlite_storage::store::Store) {
    loop {
        let mut owners = owners_under(store, "xet/").await;
        owners.extend(owners_under(store, "lfs/").await);
        owners.sort();
        owners.dedup();
        for owner in &owners {
            match kloudlite_xet::gc::sweep_owner(&*store.os, owner, HUB_GC_GRACE).await {
                Ok(n) if n > 0 => tracing::info!(%owner, count = n, "hub.gc.swept"),
                Ok(_) => {}
                Err(e) => tracing::warn!(%owner, error = %e, "hub.gc.failed"),
            }
            tokio::time::sleep(GC_OWNER_GAP).await;
        }
        tokio::time::sleep(HUB_GC_GAP).await;
    }
}
```

Spawn it where `gc_lane` is spawned, under the same condition and with the same heartbeat wiring that lane uses (worker liveness counts lane heartbeat files: if `gc_lane` touches a heartbeat, this lane touches its own the same way, and the liveness count in the worker's manifest goes up by one in Task 16). Add `kloudlite-xet` to `bins/worker/Cargo.toml`. `owners_under` returns `Vec<String>` (if it returns a set, drop the sort/dedup).

- [ ] **Step 7: Run tests to verify they pass**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test -p kloudlite-xet gc 2>&1 | grep -E "test result|FAILED" | head -3; CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test --test hub_gc --test hub_api 2>&1 | grep -E "test result|FAILED|panicked" | head`
Expected: PASS.

- [ ] **Step 8: Clippy and commit**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -5`

```bash
cd /Volumes/kdisk/rustic-git-wt/master && git add crates/xet crates/hub bins/worker tests/hub_gc.rs tests/hub_api.rs Cargo.toml Cargo.lock
cd /Volumes/kdisk/rustic-git-wt/master && git commit -m "Track live hub objects and sweep the rest"
```

### Task 14: Serve the hub and CAS from the server

**Files:**
- Modify: `bins/server/src/listeners.rs` (two optional binds), `bins/server/src/main.rs` (serve them; nest the hub under `/hub` on the peer listener), `bins/server/src/router/route.rs` (`hub_repo_of`, `route_hub`, `route_key` split out of `route_inner`, routability test), `bins/server/src/router/git.rs` (`serve_*` seam, hub git routes), `bins/server/Cargo.toml`, `crates/hub/src/lib.rs` + new `crates/hub/src/settings.rs` (visibility)
- Test: `bins/server/src/router/route.rs` unit tests, `tests/hub_api.rs` (settings), `tests/hub_routing.rs`

**Interfaces:**
- Consumes: `kloudlite_hub::{router, HubState::new, hub_url, auth::{open, caller, target, RepoBody}, live::rebuild}`; `kloudlite_xet::{router, CasState, cas_url}`; `hub_routing_key`; `App::route_for`, `app.forwarder.forward`; `hops_of`.
- Produces: `route::{hub_repo_of(&str) -> Option<String>, HUB_TAILS}`; `git::{serve_info_refs, serve_upload_pack, serve_receive_pack}`; `PUT {api}/settings` `{private: bool}`.

- [ ] **Step 1: Write the failing tests**

In `route.rs`'s test module:

```rust
    /// Every hub route that opens a repo database must yield its `mdl/`/`ds/` key, as
    /// `every_browse_route_is_routable` holds for git. A new hub route goes in `HUB_TAILS`.
    #[test]
    fn every_hub_route_is_routable() {
        for (prefix_api, prefix_repo, tag) in [("/api/models", "", "mdl"), ("/api/datasets", "/datasets", "ds")] {
            let want = Some(format!("{tag}/alice/m"));
            assert_eq!(hub_repo_of(&format!("{prefix_api}/alice/m")), want);
            for tail in HUB_TAILS {
                assert_eq!(hub_repo_of(&format!("{prefix_api}/alice/m/{tail}")), want, "{tail}");
            }
            for tail in ["resolve/main/a.bin", "info/refs", "git-upload-pack", "git-receive-pack", "info/lfs/objects/batch"] {
                assert_eq!(hub_repo_of(&format!("{prefix_repo}/alice/m.git/{tail}")), want, "{tail}");
                assert_eq!(hub_repo_of(&format!("{prefix_repo}/alice/m/{tail}")), want, "{tail}");
            }
        }
        assert_eq!(hub_repo_of("/api/whoami-v2"), None);
        assert_eq!(hub_repo_of("/api/repos/create"), None);
        assert_eq!(hub_repo_of("/api/models/../x/tree/main"), None);
    }
```

`HUB_TAILS` is `["revision/main", "tree/main", "tree/main/a/b", "paths-info/main", "refs", "preupload/main", "commit/main", "branch/dev", "tag/main", "xet-read-token/main", "xet-write-token/main", "settings"]`.

`tests/hub_routing.rs` (two nodes over a shared `mem://` store, the way the existing multi-node routing tests build them; reuse their helper by name, for example `common::two_nodes()`):

```rust
#[tokio::test]
async fn hub_requests_reach_the_owning_node_from_either_listener() {
    let (a, b) = common::two_nodes_with_hub().await;
    let alice = a.token_for("alice").await;
    let r = reqwest::Client::new().post(format!("{}/api/repos/create", a.hub)).bearer_auth(&alice)
        .json(&serde_json::json!({"name": "m"})).send().await.unwrap();
    assert_eq!(r.status(), 200);
    for node in [&a, &b] {
        let r = reqwest::Client::new().get(format!("{}/api/models/alice/m", node.hub)).bearer_auth(&alice).send().await.unwrap();
        assert_eq!(r.status(), 200, "via {}", node.hub);
    }
}
```

`two_nodes_with_hub` extends the existing two-node helper so each node also binds `KLOUDLITE_HUB_ADDR=127.0.0.1:0` and reports `hub: String` (base URL). If no multi-node helper exists in `tests/common`, say so in the report and keep only the unit test; do not build one here.

Append to `tests/hub_api.rs`:

```rust
#[tokio::test]
async fn settings_flip_visibility() {
    let h = hub().await;
    seed_hub(&h, HubKind::Model, "alice", "m", false, &[("a.txt", b"a")]).await;
    let anon = || client().get(format!("{}/api/models/alice/m", h.base));
    assert_eq!(anon().send().await.unwrap().status(), 401);
    let r = client().put(format!("{}/api/models/alice/m/settings", h.base)).bearer_auth(&h.alice).json(&json!({"private": false})).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(anon().send().await.unwrap().status(), 200);
    let r = client().put(format!("{}/api/models/alice/m/settings", h.base)).bearer_auth(&h.bob).json(&json!({"private": true})).send().await.unwrap();
    assert_eq!(r.status(), 403);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test -p kloudlite-server every_hub_route 2>&1 | grep -E "^error|test result" | head -3`
Expected: compile error, `hub_repo_of` not found. (Use the server package's real name from `bins/server/Cargo.toml`.)

- [ ] **Step 3: Visibility handler**

`crates/hub/src/settings.rs`:

```rust
//! `PUT {api}/settings` `{"private": bool}`, huggingface_hub's `update_repo_settings`. Routed by
//! the `mdl/`/`ds/` key like every other `{api}` route, so the flag is written on the owning node.

use crate::auth::open;
use crate::{internal, HubState};
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use kloudlite_storage::HubKind;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct Body {
    private: Option<bool>,
}

pub async fn put(
    State(st): State<HubState>,
    Extension(kind): Extension<HubKind>,
    trusted: Option<Extension<kloudlite_server_auth::Trusted>>,
    headers: HeaderMap,
    Path((owner, name)): Path<(String, String)>,
    Json(b): Json<Body>,
) -> Response {
    let a = match open(&st, kind, trusted.as_deref(), &headers, &owner, &name, true).await {
        Ok(a) => a,
        Err(r) => return r,
    };
    if let Some(private) = b.private {
        if let Err(e) = st.app.store.hub_set_private(&a.repo, private).await {
            return internal(e);
        }
    }
    Json(serde_json::json!({})).into_response()
}
```

`hub_set_private` writes the same flag `hub_create` writes and `hub_public` reads (Task 1's `crates/storage/src/hubrepo.rs`); add it there if Task 1 did not, as the one-line counterpart of the write `hub_create` does. Route in `per_kind`: `.route(&format!("{api}/settings"), put(settings::put))`. `Trusted`'s path: as in Task 11.

- [ ] **Step 4: Routing**

In `route.rs`:

```rust
/// Hub routes that open the repo database, after `/api/{models|datasets}/{owner}/{name}/`. Held by
/// `every_hub_route_is_routable`; a new `{api}` route is added here or it is served unrouted.
pub(crate) const HUB_TAILS: &[&str] = &[
    "revision/main", "tree/main", "tree/main/a/b", "paths-info/main", "refs", "preupload/main",
    "commit/main", "branch/dev", "tag/main", "xet-read-token/main", "xet-write-token/main", "settings",
];

/// The ownership key of a hub path (hub listener paths, or peer paths after the `/hub` nest strips
/// its prefix). `None` = serve locally: whoami, create/delete (routed by body in `route_hub`).
pub(crate) fn hub_repo_of(path: &str) -> Option<String> {
    use kloudlite_storage::{hub_routing_key, HubKind};
    let p = path.trim_start_matches('/');
    let (kind, rest) = if let Some(r) = p.strip_prefix("api/models/") {
        (HubKind::Model, r)
    } else if let Some(r) = p.strip_prefix("api/datasets/") {
        (HubKind::Dataset, r)
    } else if p.starts_with("api/") {
        return None;
    } else if let Some(r) = p.strip_prefix("datasets/") {
        (HubKind::Dataset, r)
    } else {
        (HubKind::Model, p)
    };
    let mut it = rest.split('/');
    let (owner, name) = (it.next()?, it.next()?);
    let name = name.strip_suffix(".git").unwrap_or(name);
    let (owner, name) = crate::protocol::parse_repo_pair(owner, name)?;
    Some(hub_routing_key(kind, &owner, &name))
}
```

`parse_repo_pair` is what `repo_of` already uses to refuse `..` and bad names; if it returns `&str`s rather than `String`s, drop the `&`. If `hub_routing_key` yields a different shape than `mdl/alice/m`, the test's `want` follows `hub_routing_key`.

Split the tail of `route_inner` (from `let hops = hops_of(...)` to the end) into:

```rust
async fn route_key(
    app: Arc<App>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
    repo: String,
    forward_prefix: &'static str,
) -> Response
```

and have `route_inner` call `route_key(app, req, next, repo, "")` after `classify`. Inside `route_key`, where the request is forwarded (`app.forwarder.forward(&peer.addr, …)`), rewrite the URI first when `forward_prefix` is non-empty:

```rust
    let req = if forward_prefix.is_empty() {
        req
    } else {
        let (mut parts, body) = req.into_parts();
        let pq = parts.uri.path_and_query().map(|p| p.as_str()).unwrap_or("/");
        parts.uri = format!("{forward_prefix}{pq}").parse().expect("prefix + valid path is a valid URI");
        axum::extract::Request::from_parts(parts, body)
    };
```

Apply the rewrite to the request handed to the forwarder only, including any replay copy `recover_after_forward` keeps, never to the request served locally. `may_create(req.method(), &path)` stays as is: hub creates are routed by `route_hub` below, which passes `true` itself.

Then:

```rust
const HUB_CREATE_MAX: usize = 64 * 1024;

/// Hub listener (public) and the peer listener's `/hub` nest: both see un-prefixed paths and both
/// forward with `/hub` in front, because the peer listener's root already serves git.
pub(crate) async fn route_hub(
    axum::extract::State(app): axum::extract::State<Arc<App>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let path = req.uri().path().to_string();
    if let Some(key) = hub_repo_of(&path) {
        return route_key(app, req, next, key, "/hub").await;
    }
    if !matches!(path.as_str(), "/api/repos/create" | "/api/repos/delete") {
        return next.run(req).await;
    }
    // create/delete name the repo in the body; the caller decides the owner when it is omitted.
    let (parts, body) = req.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, HUB_CREATE_MAX).await else {
        return (StatusCode::PAYLOAD_TOO_LARGE, "body too large").into_response();
    };
    let trusted = parts.extensions.get::<kloudlite_server_auth::Trusted>().cloned();
    let key = match serde_json::from_slice::<kloudlite_hub::auth::RepoBody>(&bytes) {
        Ok(b) => match kloudlite_hub::auth::caller(&app, trusted.as_ref(), &parts.headers).await {
            Ok(c) => match kloudlite_hub::auth::target(&b, c.as_deref()) {
                Ok((kind, owner, name)) => Some(kloudlite_storage::hub_routing_key(kind, &owner, &name)),
                Err(r) => return r,
            },
            Err(r) => return r,
        },
        // Malformed: the handler answers 400 locally; nothing is opened.
        Err(_) => None,
    };
    let req = axum::extract::Request::from_parts(parts, axum::body::Body::from(bytes));
    match key {
        Some(k) => route_key(app, req, next, k, "/hub").await,
        None => next.run(req).await,
    }
}
```

`route_key` calls `app.route_for(&repo, may_create(..))`; for the create path it must claim, so give `route_key` a `create: bool` parameter (`route_inner` passes `may_create(req.method(), &path)`, `route_hub` passes `path == "/api/repos/create"`) instead of having it recompute. Same `Trusted` import as `bins/server` uses today; same `StatusCode`/`IntoResponse` imports `route.rs` already has.

- [ ] **Step 5: git seam**

In `git.rs`, each of `info_refs`, `upload_pack`, `receive_pack` today opens the repo with `open(&app, &trusted, &headers, &owner, &name, read)` and then serves it. Move everything after the open into `pub(crate) async fn serve_info_refs(app: &Arc<App>, repo: Repo, service: &str, headers: &HeaderMap) -> Response`, `serve_upload_pack(app, repo, headers, body)` and `serve_receive_pack(app, repo, headers, body)` (whatever arguments each tail reads, by value), so the existing handlers become `open` + `serve_*`, and existing git tests stay green unchanged.

Add hub git routes in the same file:

```rust
/// git over the hub listener for model and dataset repos. Auth is the hub's (`auth::open`), the
/// bytes are git's; a push rebuilds the live list so the sweep never sees a ref it does not know.
pub(crate) fn hub_git(st: kloudlite_hub::HubState) -> axum::Router {
    use kloudlite_storage::HubKind;
    let mut r = axum::Router::new();
    for kind in [HubKind::Model, HubKind::Dataset] {
        let p = format!("/{}{{owner}}/{{name}}", kind.url_prefix());
        r = r.merge(
            axum::Router::new()
                .route(&format!("{p}/info/refs"), get(hub_info_refs))
                .route(&format!("{p}/git-upload-pack"), post(hub_upload_pack))
                .route(&format!("{p}/git-receive-pack"), post(hub_receive_pack))
                .layer(axum::Extension(kind)),
        );
    }
    r.with_state(st)
}
```

Each `hub_*` handler extracts `State<HubState>`, `Extension<HubKind>`, `Option<Extension<Trusted>>`, headers, `Path((owner, name))` (strip `.git` from `name`), the query/body the matching git handler takes; calls `kloudlite_hub::auth::open(&st, kind, trusted.as_deref(), &headers, &owner, &name, write)` with `write` true only for receive-pack (and for `info/refs?service=git-receive-pack`); then calls the `serve_*` with `&st.app` and `a.repo`. `hub_receive_pack` after a 2xx response calls `kloudlite_hub::live::rebuild(&st.app.store, &a.repo, &a.owner, a.kind, &a.name).await`, logging `hub.live.failed` on error. git's own limits apply: wrap the receive-pack route in the same `DefaultBodyLimit` `git.rs` uses for `max_body`.

- [ ] **Step 6: Listeners and mounting**

`listeners.rs`: add `pub hub: Option<TcpListener>, pub cas: Option<TcpListener>` to `Listeners`, bound in `bind` from `KLOUDLITE_HUB_ADDR` and `KLOUDLITE_CAS_ADDR` when set (same error style as the other binds; unset = not served, so existing deployments are unchanged).

`main.rs`, beside the `peer_router(a3)` serve:

```rust
    let hub_st = kloudlite_hub::HubState::new(app.clone(), kloudlite_hub::hub_url());
    let hub_app = kloudlite_hub::router(hub_st.clone())
        .merge(router::git::hub_git(hub_st.clone()))
        .layer(axum::middleware::from_fn_with_state(app.clone(), router::route::route_hub));
    if let Some(l) = l.hub {
        let svc = hub_app.clone();
        tokio::spawn(async move { axum::serve(l, svc).await });
    }
    if let Some(l) = l.cas {
        // No routing: CAS bytes live in the shared object store, any node serves them.
        let cas = kloudlite_xet::router(hub_st.cas.clone());
        tokio::spawn(async move { axum::serve(l, cas).await });
    }
```

and in `peer_router` (or where `main.rs` builds it) `.nest("/hub", hub_app)`. Match the serve/spawn/shutdown idiom the neighbouring listeners use (if they use `with_graceful_shutdown` or `into_make_service_with_connect_info`, do the same), and use the trusted-peer layer the peer listener already applies so forwarded requests keep their `Trusted` extension. Add `kloudlite-hub` and `kloudlite-xet` to `bins/server/Cargo.toml`.

- [ ] **Step 7: Run tests to verify they pass**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test --workspace 2>&1 | grep -E "test result: FAILED|panicked|^error" | head; echo exit=$?`
Expected: no lines (every suite green, including the existing git and routing tests).

- [ ] **Step 8: Clippy and commit**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -5`

```bash
cd /Volumes/kdisk/rustic-git-wt/master && git add bins/server crates/hub crates/storage tests Cargo.lock
cd /Volumes/kdisk/rustic-git-wt/master && git commit -m "Serve the hub and CAS listeners with routing"
```

### Task 15: Real-client round trip

**Files:**
- Create: `tests/hub_e2e.sh`

**Interfaces:**
- Consumes: the server binary with `KLOUDLITE_HUB_ADDR`, `KLOUDLITE_CAS_ADDR`, `KLOUDLITE_HUB_URL`, `KLOUDLITE_CAS_URL`, `KLOUDLITE_S3_URL=file://…`; a PAT for a local user, minted the way `tests/registry_e2e.sh` mints its credential.
- Produces: exit 0 pass, 77 skipped (client missing), anything else fail. Answers recorded back into the spec §6.

- [ ] **Step 1: Write the script**

```bash
#!/usr/bin/env bash
# Real clients against a local server: huggingface_hub + hf_xet, git + git-lfs, git-xet,
# transformers. Exit 77 = a client is missing (skipped, not passed).
set -euo pipefail
cd "$(dirname "$0")/.."

need() { command -v "$1" >/dev/null || { echo "skip: $1 not installed"; exit 77; }; }
need python3; need git; need git-lfs

ROOT=$(pwd)/.local/hub-e2e
rm -rf "$ROOT" && mkdir -p "$ROOT"
python3 -m venv "$ROOT/venv"
"$ROOT/venv/bin/pip" -q install 'huggingface_hub[hf_xet]' transformers >/dev/null 2>&1 \
  || { echo "skip: pip install failed (offline?)"; exit 77; }
PY="$ROOT/venv/bin/python"

HUB_PORT=18081 CAS_PORT=18082
export KLOUDLITE_S3_URL="file://$ROOT/store"
export KLOUDLITE_HUB_ADDR=127.0.0.1:$HUB_PORT KLOUDLITE_CAS_ADDR=127.0.0.1:$CAS_PORT
export KLOUDLITE_HUB_URL=http://127.0.0.1:$HUB_PORT KLOUDLITE_CAS_URL=http://127.0.0.1:$CAS_PORT
cargo build -q -p kloudlite-server
./target/debug/kloudlite-server >"$ROOT/server.log" 2>&1 &
SERVER=$!
trap 'kill $SERVER 2>/dev/null || true' EXIT
for _ in $(seq 50); do curl -sf "$KLOUDLITE_HUB_URL/api/whoami-v2" -o /dev/null -w '%{http_code}' 2>/dev/null | grep -q 401 && break; sleep 0.2; done

# Same user + PAT bootstrap as tests/registry_e2e.sh; it leaves the token in $TOKEN.
source tests/lib/e2e_user.sh
TOKEN=$(e2e_user alice)
export HF_ENDPOINT=$KLOUDLITE_HUB_URL HF_TOKEN=$TOKEN HF_HUB_DISABLE_TELEMETRY=1

head -c $((200 * 1024 * 1024)) /dev/urandom >"$ROOT/weights.bin"
mkdir -p "$ROOT/up" && cp "$ROOT/weights.bin" "$ROOT/up/model.safetensors"
cat >"$ROOT/up/config.json" <<'EOF'
{"model_type": "gpt2", "n_layer": 1, "n_head": 1, "n_embd": 8, "vocab_size": 16, "n_positions": 8}
EOF

echo "== upload_folder + snapshot_download"
"$PY" - "$ROOT" <<'EOF'
import filecmp, sys
from huggingface_hub import HfApi, snapshot_download
root = sys.argv[1]
api = HfApi()
api.create_repo("alice/m")
api.upload_folder(repo_id="alice/m", folder_path=f"{root}/up")
d = snapshot_download("alice/m", cache_dir=f"{root}/cache")
assert filecmp.cmp(f"{d}/model.safetensors", f"{root}/weights.bin", shallow=False)
EOF

echo "== dedup: same bytes under a new name store no new xorb"
before=$(find "$ROOT/store" -path '*xet/alice/*xorb*' -type f | wc -l)
"$PY" - "$ROOT" <<'EOF'
import sys
from huggingface_hub import HfApi
HfApi().upload_file(path_or_fileobj=f"{sys.argv[1]}/weights.bin", path_in_repo="copy.bin", repo_id="alice/m")
EOF
after=$(find "$ROOT/store" -path '*xet/alice/*xorb*' -type f | wc -l)
[ "$before" = "$after" ] || { echo "FAIL: dedup stored $((after - before)) new xorb files"; exit 1; }

echo "== git clone with git-lfs"
git -c credential.helper= clone -q "http://alice:$TOKEN@127.0.0.1:$HUB_PORT/alice/m" "$ROOT/clone"
cmp "$ROOT/clone/model.safetensors" "$ROOT/weights.bin"

echo "== AutoConfig.from_pretrained"
"$PY" -c 'from transformers import AutoConfig; assert AutoConfig.from_pretrained("alice/m").n_embd == 8'

if command -v git-xet >/dev/null; then
  echo "== git-xet push, HF download"
  ( cd "$ROOT/clone" && git xet install --local >/dev/null \
    && head -c $((8 * 1024 * 1024)) /dev/urandom >pushed.bin && git lfs track pushed.bin >/dev/null \
    && git add .gitattributes pushed.bin && git -c user.name=a -c user.email=a@x commit -qm push && git push -q )
  "$PY" - "$ROOT" <<'EOF'
import filecmp, sys
from huggingface_hub import hf_hub_download
p = hf_hub_download("alice/m", "pushed.bin", cache_dir=f"{sys.argv[1]}/cache2")
assert filecmp.cmp(p, f"{sys.argv[1]}/clone/pushed.bin", shallow=False)
EOF
else
  echo "note: git-xet not installed; git-xet push case not run"
fi

echo "PASS"
```

Before writing it, read `tests/registry_e2e.sh` for three things and follow them: how it builds and starts the server (binary name, any required env such as the JWT secret), how it creates a user and a token (if it does so inline, move those lines into `tests/lib/e2e_user.sh` as `e2e_user <name>` printing the token, and make `registry_e2e.sh` source it too), and where it puts scratch (`.local/`). The `find` pattern for xorbs follows `keys::xorb`'s layout under the `file://` root.

- [ ] **Step 2: Run it**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && chmod +x tests/hub_e2e.sh && CARGO_TARGET_DIR=/Volumes/kdisk/target ./tests/hub_e2e.sh 2>&1 | tail -15; echo exit=${PIPESTATUS[0]}`
Expected: `PASS`, exit 0. `CARGO_TARGET_DIR` moves the binary: use `"${CARGO_TARGET_DIR:-target}/debug/…"` in the script.

A failure here is a protocol mismatch, not a flaky test: read `$ROOT/server.log` (`grep -E "http.failed|WARN|ERROR" | tail -20`) for the refused request, fix the handler in the task that owns it, and add a unit test for the shape the real client sent.

- [ ] **Step 3: Record what the clients actually sent**

From `server.log` (the `http.write` lines carry method and path), answer the three open questions in spec §6 and replace that paragraph with the answers: batch reconstruction GET or POST; `lfs.oid` or `lfs.sha256` read from tree entries (whichever key the client parsed without error; if both are emitted, say so); the `transfers` list git-xet sent. If an answer contradicts what Tasks 6, 8 or 12 built, fix that task's code in this commit with a test for the observed shape.

- [ ] **Step 4: Commit**

```bash
cd /Volumes/kdisk/rustic-git-wt/master && git add tests/hub_e2e.sh tests/lib docs/superpowers/specs/2026-10-12-model-data-hub-design.md
cd /Volumes/kdisk/rustic-git-wt/master && git commit -m "Round-trip real Hugging Face and git clients through the hub"
```

Add `tests/registry_e2e.sh` and any handler fixes to the `git add` if Step 1 or 3 touched them.

### Task 16: Rollout

**Files:**
- Modify: `deploy/kloudlite.yaml` (Service ports, Deployment/StatefulSet env and ports, two Ingresses), `deploy/slo.md`, `crates/workspaces/src/slo/catalogue.rs`, `bins/slo/src/` (two stages), `CLAUDE.md`
- Test: the catalogue/doc equality test (existing), `cargo test -p kloudlite-slo` (use its real name)

**Interfaces:**
- Consumes: everything above; `deploy/pin.sh`, `deploy/roll.sh`.
- Produces: `hub.<domain>`, `cas.<domain>` live; probes `hub.upload`, `hub.download`.

- [ ] **Step 1: Manifests**

In `deploy/kloudlite.yaml`, for `kloudlite-srv`:
- container ports `hub` 8081 and `cas` 8082; env `KLOUDLITE_HUB_ADDR=0.0.0.0:8081`, `KLOUDLITE_CAS_ADDR=0.0.0.0:8082`, `KLOUDLITE_HUB_URL=https://hub.<domain>`, `KLOUDLITE_CAS_URL=https://cas.<domain>` (the same `<domain>` the registry ingress uses);
- the Service gains `hub` and `cas` ports;
- two Ingresses `kloudlite-hub` and `kloudlite-cas`, copied from the registry Ingress (its body-size and timeout annotations matter: CAS takes 64 MiB xorbs and streams multi-GiB downloads; LFS basic PUTs are up to 5 GiB), each with its own TLS secret and a comment saying which hostname it serves.

If 8081/8082 are taken in that pod spec, pick the next free pair and use it everywhere above.

Before applying, `dig +short hub.<domain> cas.<domain>` must answer with the ingress address; if not, the DNS records are the user's step: stop and say so with the two records needed.

In the worker manifest, if Task 13 added a heartbeat for `hub_gc_lane`, raise the liveness lane count by one in the same commit.

- [ ] **Step 2: Probes**

Add to `crates/workspaces/src/slo/catalogue.rs` and `deploy/slo.md` (the equality test holds them together), in the fast suite:
- `hub.upload`: create `run-{id}-hub` model repo, NDJSON commit of one 1 KiB regular file (no Xet), expect 200 and a `commitOid`.
- `hub.download`: `GET /{owner}/run-{id}-hub/resolve/main/probe.txt`, bytes equal; then `DELETE /api/repos/delete`.

In `bins/slo`, implement both as one stage function in the shape of an existing HTTP stage (same report call, same timing, same `run-{id}` teardown prefix so a crashed run is cleaned). The NDJSON body is two lines: `{"key":"header","value":{"summary":"probe"}}` and `{"key":"file","value":{"path":"probe.txt","content":"<base64>","encoding":"base64"}}`. The hub base URL comes from the same config source the probe uses for its other endpoints (add `hub_url` beside them).

- [ ] **Step 3: CLAUDE.md**

Under "Two namespaces, one server", rename the heading to "Three namespaces, one server" and add:

```markdown
- Models and datasets (Hugging Face Hub API, `crates/hub`): DB `repo/mdl/{owner}/{name}` and
  `repo/ds/{owner}/{name}`, keys `mdl/…`/`ds/…`. Own listener (`KLOUDLITE_HUB_ADDR`), routed by
  `route_hub`; on the peer listener under `/hub`. `HUB_TAILS` is the routing contract.
- Xet CAS (`crates/xet`, `KLOUDLITE_CAS_ADDR`): unrouted, any node; v1 routes only. Bytes
  `xet/{owner}/…` and `lfs/{owner}/…`, shared across an owner's repos. Objects capped at
  `MAX_XET_OBJECT` (64 MiB).
```

Under "Load-bearing rules":

```markdown
- **Only the hub sweep deletes hub bytes** (`crates/xet/src/gc.rs`, worker lane). It trusts the
  live lists `hub/{owner}/live/…`; a missing list keeps everything, so a failed rebuild deletes
  the list instead of leaving a stale one.
```

- [ ] **Step 4: Verify and commit**

Run: `cd /Volumes/kdisk/rustic-git-wt/master && CARGO_TARGET_DIR=/Volumes/kdisk/target cargo test --workspace 2>&1 | grep -E "test result: FAILED|panicked|^error" | head; CARGO_TARGET_DIR=/Volumes/kdisk/target cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3`
Expected: no failures, no warnings.

```bash
cd /Volumes/kdisk/rustic-git-wt/master && git add deploy crates/workspaces bins/slo CLAUDE.md
cd /Volumes/kdisk/rustic-git-wt/master && git commit -m "Deploy the hub and probe uploads and downloads"
```

- [ ] **Step 5: Ship (asks first)**

Pushing, pinning and rolling are outward-facing: ask the user before `git push`. After a yes: push, wait for the image job, `deploy/pin.sh <sha>`, commit the pin, `deploy/roll.sh`, then the live drill from spec §6 "Done" (`hf upload` of `gpt2` config + weights, `from_pretrained` from a workspace, `git clone` of the same repo). Report each step's result.
