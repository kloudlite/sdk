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
use slatedb::object_store::{ObjectStore, ObjectStoreExt};
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
                match XorbObject::deserialize(&mut r) {
                    Ok(x) => x.get_bytes_by_chunk_range(&mut r, t.start, t.end).map_err(|e| io::Error::other(e.to_string())),
                    // hf_xet uploads bare chunks with no footer (see `xorbs::footerless_idx`).
                    Err(_) => footerless_range(&body, t.start, t.end),
                }
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

fn footerless_range(body: &[u8], start: u32, end: u32) -> Result<Vec<u8>, io::Error> {
    let mut cur = io::Cursor::new(body);
    let (mut out, mut i) = (Vec::new(), 0u32);
    while i < end && (cur.position() as usize) < body.len() {
        let (data, _, _) = xet_core_structures::xorb_object::deserialize_chunk(&mut cur).map_err(|e| io::Error::other(e.to_string()))?;
        if i >= start {
            out.extend_from_slice(&data);
        }
        i += 1;
    }
    if i < end {
        return Err(io::Error::other("xorb has fewer chunks than its record"));
    }
    Ok(out)
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
    use slatedb::object_store::{ObjectStore, ObjectStoreExt};
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

    #[tokio::test]
    async fn streams_a_footerless_xorb_as_hf_xet_uploads_it() {
        let os: Arc<dyn ObjectStore> = Arc::new(InMemory::new());
        let b = build(&[b"hello ".as_slice(), b"xet ".as_slice(), b"world".as_slice()]);
        put_xorb(&*os, "alice", &b.xorb_hash, Bytes::from(b.xorb.clone())).await.unwrap();
        let idx = crate::xorbs::read_idx(&*os, "alice", &b.xorb_hash).await.unwrap().unwrap();
        let bare = b.xorb[..*idx.boundaries.last().unwrap() as usize].to_vec();
        os.delete(&crate::keys::xorb("alice", &b.xorb_hash)).await.unwrap();
        os.put(&crate::keys::xorb("alice", &b.xorb_hash), bare.into()).await.unwrap();
        put_shard(&*os, "alice", &b.shard).await.unwrap();
        let rec = read_file(&*os, "alice", &b.file_hash).await.unwrap().unwrap();
        assert_eq!(collect(&os, &rec, 0, rec.size).await, b.bytes);
        assert_eq!(collect(&os, &rec, 3, 9).await, b.bytes[3..9]);
    }
}
