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
        metadata_ext: Some(FileMetadataExt::new(DataHash::from_be_bytes(sha256))),
    };
    let mut shard = MDBInMemoryShard::default();
    shard.add_xorb_block(xorb_info).unwrap();
    shard.add_file_reconstruction_info(info).unwrap();
    Built { xorb: xorb.serialized_data, shard: shard.to_bytes().unwrap(), xorb_hash, file_hash: fh, sha256, bytes }
}
