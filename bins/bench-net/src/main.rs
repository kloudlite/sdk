//! `bench-net`: the bench end of the environment tunnel. Runs as container `net` in the bench pod
//! in the plain hardened context: no TUN, no NAT, no capability (gVisor probe, spec §5). Each
//! laptop stream terminates WireGuard (boringtun) and feeds its own userspace TCP/IP stack
//! (`ipstack`), so every connection the laptop opens becomes an ordinary socket from this pod.
//! Spec: docs/superpowers/specs/2026-10-11-env-tunnel-design.md (§3 handshake, §5 userspace stack).
//!
//! Modules: `link` is the packet device, anti-inject rule and stream count; `stream` is one
//! laptop connection. The keypair is generated at boot and lives in memory only; the keys are not
//! authentication (the bench session is), they only make the bytes WireGuard.
mod link;
mod stream;

use kloudlite_nettun::Pool;
use std::sync::{Arc, Mutex, atomic::{AtomicUsize, Ordering}};
use std::time::Duration;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().json().init();
    let port: u16 = std::env::var("KL_NET_PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(7792);
    let count = std::env::var("KL_NET_STREAMS").unwrap_or_else(|_| "/run/kl-net/net.streams".into());
    let secret = boringtun::x25519::StaticSecret::random_from_rng(rand_core::OsRng);
    let public = boringtun::x25519::PublicKey::from(&secret);
    let pool = Arc::new(Mutex::new(Pool::new()));
    let open = Arc::new(AtomicUsize::new(0));

    // The sessions daemon reads this to know the bench is in use; a failed write is logged, not fatal.
    let o = open.clone();
    tokio::spawn(async move {
        loop {
            if let Err(e) = link::write_count(std::path::Path::new(&count), o.load(Ordering::Relaxed)) {
                tracing::warn!(error = %e, "net.count");
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    });

    let l = TcpListener::bind(("0.0.0.0", port)).await.expect("bind");
    tracing::info!(port, "net.listen");
    loop {
        match l.accept().await {
            Ok((s, _)) => {
                tokio::spawn(stream::run(s, secret.clone(), public, pool.clone(), open.clone()));
            }
            Err(e) => {
                tracing::warn!(error = %e, "net.accept");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}
