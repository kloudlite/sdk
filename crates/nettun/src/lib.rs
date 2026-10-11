//! The tunnel's wire format, shared by the bench end (`bench-net`) and the laptop daemon
//! (`kl-connect`): length framing, the Hello/Welcome handshake, the peer address pool and a
//! hand-rolled DNS A lookup. Kept dependency-light so both ends compile it cheaply.
pub mod dns;
pub mod frame;
pub mod handshake;
pub use frame::*;
pub use handshake::*;
