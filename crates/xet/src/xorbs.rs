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
use xet_core_structures::xorb_object::XorbObject;

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
    let x = match XorbObject::validate_xorb_object(&mut cur, hash) {
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
