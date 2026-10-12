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
// The brief fixes this signature; the only failure is "unsupported range".
#[allow(clippy::result_unit_err)]
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
    url_for: &(dyn Fn(&str) -> String + Sync),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shards::Term;
    use crate::xorbs::XorbIdx;
    use serde_json::json;
    use slatedb::object_store::memory::InMemory;
    use slatedb::object_store::ObjectStoreExt;

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
