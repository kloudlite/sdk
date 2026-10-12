//! Revision names as huggingface_hub sends them: a branch, a tag, or a full commit sha. Branch
//! wins over tag, as on the Hub. Short shas are not accepted (the Hub refuses them too).
use crate::hub_err;
use axum::http::StatusCode;
use axum::response::Response;
use gix_hash::ObjectId;
use kloudlite_storage::store::{Repo, Store};

pub async fn resolve(store: &Store, repo: &Repo, rev: &str) -> kloudlite_core::Result<Option<ObjectId>> {
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
