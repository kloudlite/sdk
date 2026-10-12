//! Model and dataset repos: git repos in namespaces of their own.
//!
//! A hub repo is an ordinary store-level repo whose owner is the namespace (`mdl` or `ds`) and
//! whose name is `{owner}/{name}`. That one choice gives it a database `repo/mdl/{owner}/{name}`,
//! packs under `objects/mdl/{owner}/{name}/pack` and refs under `ref/mdl/{owner}/{name}/`, so the
//! whole git layer works unchanged and nothing it writes can collide with a code repo or an image.
//! The namespaces are reserved owners, so no person can register `mdl` and reach these keys
//! through the git routes. Index markers are not written: hub listing is out of scope.
use crate::store::{valid_owner, valid_segment, Repo, Store};
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

/// `valid_owner` already refuses the reserved names, `mdl` and `ds` among them.
fn valid(owner: &str, name: &str) -> bool {
    valid_owner(owner) && valid_segment(name)
}

impl Store {
    pub async fn hub_create(&self, kind: HubKind, owner: &str, name: &str, private: bool) -> Result<()> {
        if !valid(owner, name) {
            return Err(err("invalid repo path"));
        }
        let (ns, n) = hub_pool_coords(kind, owner, name);
        self.create_repo_unchecked(ns, &n).await?;
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
        self.delete_repo(ns, &n).await
    }

    pub async fn hub_public(&self, kind: HubKind, owner: &str, name: &str) -> Result<bool> {
        if !valid(owner, name) {
            return Ok(false);
        }
        let (ns, n) = hub_pool_coords(kind, owner, name);
        self.repo_public(ns, &n).await
    }

    /// Flip visibility. Refuses a repo that does not exist: `set_public` opens the database, and
    /// opening one conjures it.
    pub async fn hub_set_private(&self, repo: &Repo, private: bool) -> Result<()> {
        if !self.repo_exists(&repo.owner, &repo.name).await? {
            return Err(err("repository not found"));
        }
        self.set_public(&repo.owner, &repo.name, !private).await
    }
}

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

    #[tokio::test]
    async fn set_private_round_trips() {
        let (s, _t) = store().await;
        s.hub_create(HubKind::Dataset, "alice", "d", true).await.unwrap();
        let r = s.hub_open(HubKind::Dataset, "alice", "d").await.unwrap().unwrap();
        s.hub_set_private(&r, false).await.unwrap();
        assert!(s.hub_public(HubKind::Dataset, "alice", "d").await.unwrap());
        s.hub_set_private(&r, true).await.unwrap();
        assert!(!s.hub_public(HubKind::Dataset, "alice", "d").await.unwrap());
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
