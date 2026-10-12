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
        // Skip on the LAST key written: a crash between the two puts leaves files/ without sha/,
        // and testing files/ would skip every retry and never write the mapping.
        if os.head(&keys::sha(owner, &rec.sha256)).await.is_ok() {
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::testkit::build;
    use crate::xorbs::put_xorb;
    use bytes::Bytes;
    use xet_core_structures::metadata_shard::chunk_verification::range_hash_from_chunks;
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
        assert_eq!(f.sha256, hex::encode(b.sha256));
        assert_eq!(f.terms.len(), 1);
        let sha = os.get(&crate::keys::sha("alice", &f.sha256)).await.unwrap().bytes().await.unwrap();
        assert_eq!(&sha[..], b.file_hash.hex().as_bytes());
    }

    #[tokio::test]
    async fn retry_after_partial_write_restores_sha_mapping() {
        let os = InMemory::new();
        let b = build(&[b"hello ", b"world"]);
        put_xorb(&os, "alice", &b.xorb_hash, Bytes::from(b.xorb.clone())).await.unwrap();
        put_shard(&os, "alice", &b.shard).await.unwrap();
        let f = read_file(&os, "alice", &b.file_hash).await.unwrap().unwrap();
        let sha = crate::keys::sha("alice", &f.sha256);
        os.delete(&sha).await.unwrap();
        assert!(put_shard(&os, "alice", &b.shard).await.unwrap());
        assert!(os.head(&sha).await.is_ok());
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

    #[tokio::test]
    async fn shard_without_sha256_extension_is_refused() {
        use xet_core_structures::merklehash::file_hash;
        use xet_core_structures::metadata_shard::file_structs::{
            FileDataSequenceEntry, FileDataSequenceHeader, FileVerificationEntry, MDBFileInfo,
        };
        use xet_core_structures::metadata_shard::shard_in_memory::MDBInMemoryShard;
        use xet_core_structures::xorb_object::{Chunk, RawXorbData, SerializedXorbObject};
        let os = InMemory::new();
        let chunks = vec![Chunk::new(Bytes::from_static(b"abc"))];
        let raw = RawXorbData::from_chunks(&chunks, vec![0]);
        let xh = raw.hash();
        let info = raw.xorb_info.clone();
        let xorb = SerializedXorbObject::from_xorb(raw, true, "none", 0).unwrap();
        put_xorb(&os, "alice", &xh, Bytes::from(xorb.serialized_data)).await.unwrap();
        let fh = file_hash(&[(chunks[0].hash, 3)]);
        let f = MDBFileInfo {
            metadata: FileDataSequenceHeader::new(fh, 1u32, true, false),
            segments: vec![FileDataSequenceEntry::new(xh, 3u32, 0u32, 1u32)],
            verification: vec![FileVerificationEntry::new(range_hash_from_chunks(&[chunks[0].hash]))],
            metadata_ext: None,
        };
        let mut shard = MDBInMemoryShard::default();
        shard.add_xorb_block(info).unwrap();
        shard.add_file_reconstruction_info(f).unwrap();
        let e = put_shard(&os, "alice", &shard.to_bytes().unwrap()).await.unwrap_err();
        assert!(e.contains("sha256"), "{e}");
    }
}
