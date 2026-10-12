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

pub mod gc;
pub mod keys;
pub mod lfs;
pub mod recon;
pub mod routes;
pub mod shards;
pub mod sign;
pub mod stream;
pub mod xorbs;
#[cfg(any(test, feature = "testkit"))]
pub mod testkit;

pub use routes::{router, CasState};
pub(crate) use kloudlite_core::{err, Result};

pub const MAX_XET_OBJECT: usize = 64 * 1024 * 1024;
pub const MAX_XORB_CHUNKS: u32 = 8192;
pub const MAX_LFS_OBJECT: u64 = 5 * 1024 * 1024 * 1024;

/// The CAS base URL clients are told to use. Configuration, not derived from the request: the
/// client must reach it from outside.
pub fn cas_url() -> String {
    std::env::var("KLOUDLITE_CAS_URL").unwrap_or_else(|_| "http://localhost:8091".into())
}
