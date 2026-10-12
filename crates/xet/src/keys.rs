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

/// `hub/{owner}/live/{mdl|ds}/{name}`: every sha256 any ref of that repo reaches. Written only by
/// the owning node; read only by the sweep. `tag` is `HubKind::ns()`.
pub fn live(owner: &str, tag: &str, name: &str) -> Path {
    Path::from(format!("hub/{owner}/live/{tag}/{name}"))
}
pub fn live_prefix(owner: &str) -> Path {
    Path::from(format!("hub/{owner}/live"))
}
pub fn sha_prefix(owner: &str) -> Path {
    Path::from(format!("xet/{owner}/sha"))
}
pub fn file_prefix(owner: &str) -> Path {
    Path::from(format!("xet/{owner}/files"))
}
/// Xorbs and their `.idx` siblings share this directory.
pub fn xorb_prefix(owner: &str) -> Path {
    Path::from(format!("xet/{owner}/xorbs"))
}
pub fn lfs_prefix(owner: &str) -> Path {
    Path::from(format!("lfs/{owner}/sha256"))
}
/// Staging uploads (`lfs.rs` writes `{oid}-{rand}` here before the rename into place).
pub fn lfs_tmp_prefix(owner: &str) -> Path {
    Path::from(format!("lfs/{owner}/tmp"))
}
