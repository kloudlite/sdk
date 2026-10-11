# Environment Tunnel Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** After `kl-connect env connect <env>`, every service of the space's environment answers on
the laptop by its bare name, carried as WireGuard inside the gateway's existing websocket pipe to
the bench pod.

**Architecture:** A root launchd daemon in `kl-connect` runs boringtun on a `utun` device and sends
each datagram, length-framed, through `wss://{gateway}/net/{bench}`. The gateway pipes the bytes to
the bench pod's new `net` container (`bins/bench-net`, TCP 7792), which runs one boringtun peer and one
userspace TCP/IP stack (`ipstack`) per stream and opens an ordinary pod socket for every connection. The daemon polls every 10 s and keeps
a marked block in `/etc/hosts` equal to the environment's services.

**Tech Stack:** Rust (tokio, boringtun, ipstack, tun on the laptop, x25519), axum gateway, k8s-openapi pod render,
Node 24 (`bench/sessions/main.ts`), launchd.

**Spec:** `docs/superpowers/specs/2026-10-11-env-tunnel-design.md`

## Global Constraints

- Exactly one connected environment, the space's (`PUT/DELETE/GET /v1/me/environments`).
- No API or CRD change. No new ingress, port on the node, or UDP path.
- The gateway holds no WireGuard key and reads no datagram.
- Access is the bench session's (`POST /v1/bench/session`); nothing new is authorized.
- macOS only on the laptop (launchd). Plist `/Library/LaunchDaemons/io.kloudlite.kl-connect.plist`.
- Framing: 2-byte big-endian length, then payload; length 0 or a short read closes the stream.
- Handshake: laptop sends its 32-byte public key; bench answers 32-byte public key + 4-byte peer
  address from `100.96.0.0/16`, unique per open stream. `GATEWAY` `100.96.0.1` is the laptop utun's point-to-point peer.
- Ports: `BENCH_NET_PORT` = 7792. Service CIDR `10.43.0.0/16`, cluster DNS `10.43.0.10`.
- Poll 10 s. Reconnect backoff 1 s doubling to 30 s, new bench session each time.
- `net.streams` in `/run/kl-net`, written every 5 s, stale after 15 s.
- No capability, hostPath or sysctl anywhere: `net` runs the plain `hardened()` context under gVisor.
- TCP and UDP only; ICMP is dropped. Inner MTU 1420 on both ends.
- Hosts block markers: `# kl-connect begin env=<id> name=<name>` … `# kl-connect end`.
- Commits: imperative sentence case, no tool attribution, no trailers.
- Cargo on the laptop with `CARGO_TARGET_DIR=/Volumes/kdisk/target-env-tunnel`; never on the dev pod.
- Every kubectl call carries an explicit `--kubeconfig`; never the default context.
- `cargo clippy --workspace --all-targets -- -D warnings` stays clean after every task.
- House style: `//!` header with design context per file, comments say WHY, files under ~800 lines.

## Review Focus

1. **A service name already in `/etc/hosts` outside the block** (e.g. `api`): left untouched,
   skipped, listed in `env-status.json`. Test in Task 7 (`a_name_defined_outside_is_skipped`).
2. **A stream whose decrypted packet claims another stream's source address**: dropped, never
   fed to the stack. Test in Task 3 (`a_packet_from_the_wrong_address_is_dropped`).
3. **`/etc/hosts` without a trailing newline, or with CRLF**: block appended on its own line, the
   rest byte for byte. Test in Task 7 (`a_file_without_a_trailing_newline_keeps_its_last_line`).
4. **Daemon killed mid-write**: temp file in `/etc` + rename, never a half-written `/etc/hosts`; a
   leftover `/etc/hosts.kl-connect.tmp` is overwritten next pass. Test in Task 7
   (`a_stale_temp_file_is_replaced`).
5. **Address pool exhausted / address reuse**: the 65 534th concurrent stream is refused (closed
   after handshake read), a closed stream's address is reusable. Test in Task 2
   (`addresses_are_unique_and_freed`).

---

## File map

| File | Responsibility |
|---|---|
| `crates/nettun/src/lib.rs` | `//!` header, re-exports |
| `crates/nettun/src/frame.rs` | `read_frame` / `write_frame` |
| `crates/nettun/src/handshake.rs` | `Hello`, `Welcome`, `Pool` (address allocator) |
| `crates/nettun/src/dns.rs` | one A query encode/decode (stdlib only) |
| `bins/bench-net/src/main.rs` | listener, per-stream peer + stack, sockets out, `net.streams` |
| `bins/bench-net/src/link.rs` | packet device for `ipstack`, `admit`, `write_count` |
| `crates/workspaces/src/k8s/bench.rs` | `net` container, port, policy, volume |
| `bench/sessions/main.ts` | `streams()` in `isIdle` |
| `bins/gateway/src/tunnel.rs` | `/net/{bench}` route |
| `bins/kl-connect/src/env/{mod,cmd,hosts,daemon,launchd,tunnel}.rs` | CLI + daemon |
| `deploy/bench/Dockerfile`, `.github/workflows/image.yml`, `deploy/dagger/src/index.ts` | ship `bench-net` |
| `crates/workspaces/src/slo/catalogue.rs`, `deploy/slo.md` | `env.tunnel.reach` |

---

### Task 1: k3s probe — which `net` form the runtime admits

Done (c964b4c6). Under gVisor no form gives kernel NAT or forwarding (spec §5 records the errors);
the owner chose the userspace stack (2026-10-11). Nothing for Task 4 to read from it.

---

### Task 2: `crates/nettun` — framing, handshake, address pool, DNS query

**Files:**
- Create: `crates/nettun/Cargo.toml`, `crates/nettun/src/{lib,frame,handshake,dns}.rs`
- Modify: root `Cargo.toml` (`members` AND `default-members` gain `"crates/nettun"`; workspace dep
  `kloudlite-nettun = { path = "crates/nettun" }`)

**Interfaces:**
- Produces:
  - `pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R, buf: &mut [u8; 65535]) -> io::Result<Option<usize>>` — `Ok(None)` on length 0 or EOF at a frame boundary; `Err(UnexpectedEof)` on a short read.
  - `pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, payload: &[u8]) -> io::Result<()>` — refuses empty or >65535 payload with `InvalidInput`.
  - `pub struct Hello { pub key: [u8; 32] }`, `Hello::encode(&self) -> [u8; 32]`, `Hello::decode(&[u8]) -> Option<Hello>`.
  - `pub struct Welcome { pub key: [u8; 32], pub addr: Ipv4Addr }`, `encode(&self) -> [u8; 36]`, `decode(&[u8]) -> Option<Welcome>`.
  - `pub struct Pool` (`Pool::new()`, `fn take(&mut self) -> Option<Ipv4Addr>`, `fn free(&mut self, a: Ipv4Addr)`); hands out 100.96.0.2 ..= 100.96.255.254, never .0.1, never .255.255.
  - `pub const NET: Ipv4Addr = 100.96.0.0`, `pub const GATEWAY: Ipv4Addr = 100.96.0.1`, `pub const PREFIX: u8 = 16`.
  - `pub fn a_query(id: u16, name: &str) -> Vec<u8>` and `pub fn a_answer(id: u16, reply: &[u8]) -> Option<Ipv4Addr>` (first A record; `None` on NXDOMAIN, id mismatch or malformed).

- [ ] **Step 1: Write `Cargo.toml`**

```toml
[package]
name = "kloudlite-nettun"
version = "0.1.0"
edition = "2024"

[dependencies]
tokio = { workspace = true, features = ["io-util"] }

[dev-dependencies]
tokio = { workspace = true, features = ["io-util", "macros", "rt"] }
```

(Match `edition` and the `tokio` workspace-dep spelling to `crates/core/Cargo.toml`.)

- [ ] **Step 2: Write the failing tests** in each module's `#[cfg(test)] mod tests`

```rust
// frame.rs
#[tokio::test]
async fn split_and_joined_reads_give_whole_frames() {
    let (mut a, mut b) = tokio::io::duplex(8);              // 8-byte pipe forces split reads
    let w = tokio::spawn(async move {
        write_frame(&mut a, b"hello").await.unwrap();
        write_frame(&mut a, &[7u8; 300]).await.unwrap();
    });
    let mut buf = [0u8; 65535];
    assert_eq!(read_frame(&mut b, &mut buf).await.unwrap(), Some(5));
    assert_eq!(&buf[..5], b"hello");
    assert_eq!(read_frame(&mut b, &mut buf).await.unwrap(), Some(300));
    assert!(buf[..300].iter().all(|&x| x == 7));
    w.await.unwrap();
    assert_eq!(read_frame(&mut b, &mut buf).await.unwrap(), None); // EOF at a boundary
}

#[tokio::test]
async fn zero_length_closes_and_short_read_is_an_error() {
    let mut buf = [0u8; 65535];
    assert_eq!(read_frame(&mut &[0u8, 0][..], &mut buf).await.unwrap(), None);
    let e = read_frame(&mut &[0u8, 9, 1, 2][..], &mut buf).await.unwrap_err();
    assert_eq!(e.kind(), std::io::ErrorKind::UnexpectedEof);
    assert!(write_frame(&mut Vec::new(), b"").await.is_err());
}

// handshake.rs
#[test]
fn hello_and_welcome_round_trip() {
    let h = Hello { key: [3; 32] };
    assert_eq!(Hello::decode(&h.encode()).unwrap().key, [3; 32]);
    let w = Welcome { key: [4; 32], addr: "100.96.0.2".parse().unwrap() };
    let d = Welcome::decode(&w.encode()).unwrap();
    assert_eq!((d.key, d.addr), ([4; 32], w.addr));
    assert!(Hello::decode(&[0; 31]).is_none());
    assert!(Welcome::decode(&[0; 35]).is_none());
}

#[test]
fn addresses_are_unique_and_freed() {
    let mut p = Pool::new();
    let a = p.take().unwrap();
    let b = p.take().unwrap();
    assert_ne!(a, b);
    assert_ne!(a, GATEWAY);
    p.free(a);
    let mut seen = std::collections::HashSet::new();
    while let Some(x) = p.take() { assert!(seen.insert(x)); }
    assert_eq!(seen.len() + 1, 65_533);     // b still held
    assert!(!seen.contains(&b) && seen.contains(&a) && !seen.contains(&GATEWAY));
    assert!(!seen.contains(&"100.96.255.255".parse().unwrap()));
}

// dns.rs
#[test]
fn a_query_and_answer() {
    let q = a_query(0x1234, "mongodb.env-ab12.svc.cluster.local");
    assert_eq!(&q[..2], &[0x12, 0x34]);
    // reply = query with QR set, ANCOUNT 1, then a compressed-name A answer 10.43.12.7
    let mut r = q.clone();
    r[2] |= 0x80;
    r[7] = 1;
    r.extend_from_slice(&[0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 30, 0, 4, 10, 43, 12, 7]);
    assert_eq!(a_answer(0x1234, &r), Some("10.43.12.7".parse().unwrap()));
    assert_eq!(a_answer(0x9999, &r), None);          // id mismatch
    let mut nx = q.clone(); nx[2] |= 0x80; nx[3] |= 3; // RCODE NXDOMAIN
    assert_eq!(a_answer(0x1234, &nx), None);
    assert_eq!(a_answer(0x1234, &r[..20]), None);    // truncated
}
```

- [ ] **Step 3: Run, expect compile failure**

Run: `CARGO_TARGET_DIR=/Volumes/kdisk/target-env-tunnel cargo test -p kloudlite-nettun 2>&1 | tail -5`
Expected: FAIL (`cannot find function read_frame`).

- [ ] **Step 4: Implement**

```rust
// frame.rs
//! Length framing for the tunnel stream: both legs (laptop ⇄ gateway ⇄ bench) are byte streams,
//! so a WireGuard datagram needs its boundary back. 2-byte big-endian length, then payload.
//! Length 0 is the polite close; a short read is a broken stream.
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use std::io;

pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R, buf: &mut [u8; 65535]) -> io::Result<Option<usize>> {
    let mut len = [0u8; 2];
    match r.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let n = u16::from_be_bytes(len) as usize;
    if n == 0 { return Ok(None) }
    r.read_exact(&mut buf[..n]).await?;
    Ok(Some(n))
}

pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, payload: &[u8]) -> io::Result<()> {
    if payload.is_empty() || payload.len() > 65535 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "frame payload must be 1..=65535 bytes"));
    }
    w.write_all(&(payload.len() as u16).to_be_bytes()).await?;
    w.write_all(payload).await?;
    w.flush().await
}
```

Note: `read_exact` on a 1-byte partial length also returns `UnexpectedEof`; treat EOF at the
length as `None` (the far side closed), EOF inside a payload as the error. That is what the tests pin.

```rust
// handshake.rs
//! The first frame each way. The keys are not authentication (the bench session is); they make
//! the bytes WireGuard and bind each stream to one peer address, so a stream cannot inject
//! packets for another stream's address.
use std::net::Ipv4Addr;

pub const NET: Ipv4Addr = Ipv4Addr::new(100, 96, 0, 0);
pub const GATEWAY: Ipv4Addr = Ipv4Addr::new(100, 96, 0, 1);
pub const PREFIX: u8 = 16;

pub struct Hello { pub key: [u8; 32] }
impl Hello {
    pub fn encode(&self) -> [u8; 32] { self.key }
    pub fn decode(b: &[u8]) -> Option<Hello> { Some(Hello { key: b.try_into().ok()? }) }
}

pub struct Welcome { pub key: [u8; 32], pub addr: Ipv4Addr }
impl Welcome {
    pub fn encode(&self) -> [u8; 36] {
        let mut o = [0u8; 36];
        o[..32].copy_from_slice(&self.key);
        o[32..].copy_from_slice(&self.addr.octets());
        o
    }
    pub fn decode(b: &[u8]) -> Option<Welcome> {
        if b.len() != 36 { return None }
        Some(Welcome { key: b[..32].try_into().ok()?, addr: Ipv4Addr::from(<[u8; 4]>::try_from(&b[32..]).ok()?) })
    }
}

/// Host part 2..=65534 of 100.96.0.0/16: .0.0 is the network, .0.1 the bench, .255.255 broadcast.
pub struct Pool { used: std::collections::BTreeSet<u16>, next: u16 }
impl Pool {
    pub fn new() -> Pool { Pool { used: Default::default(), next: 2 } }
    pub fn take(&mut self) -> Option<Ipv4Addr> {
        for _ in 0..65533u32 {
            let h = self.next;
            self.next = if h >= 65534 { 2 } else { h + 1 };
            if self.used.insert(h) {
                return Some(Ipv4Addr::from(u32::from(NET) | h as u32));
            }
        }
        None
    }
    pub fn free(&mut self, a: Ipv4Addr) { self.used.remove(&((u32::from(a) & 0xffff) as u16)); }
}
impl Default for Pool { fn default() -> Self { Pool::new() } }
```

```rust
// dns.rs
//! One A query against cluster DNS, by hand: the daemon needs exactly "name → first IPv4" for a
//! few dozen names every 10 s, which is not worth a resolver dependency.
// ponytail: A only, no retries, no EDNS; a resolver crate if AAAA or SRV are ever needed.
use std::net::Ipv4Addr;

pub fn a_query(id: u16, name: &str) -> Vec<u8> {
    let mut q = Vec::with_capacity(name.len() + 18);
    q.extend_from_slice(&id.to_be_bytes());
    q.extend_from_slice(&[0x01, 0x00, 0, 1, 0, 0, 0, 0, 0, 0]); // RD, QDCOUNT 1
    for label in name.trim_end_matches('.').split('.') {
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.extend_from_slice(&[0, 0, 1, 0, 1]); // root, QTYPE A, QCLASS IN
    q
}

pub fn a_answer(id: u16, r: &[u8]) -> Option<Ipv4Addr> {
    if r.len() < 12 || r[..2] != id.to_be_bytes() || r[2] & 0x80 == 0 || r[3] & 0x0f != 0 { return None }
    let qd = u16::from_be_bytes([r[4], r[5]]);
    let an = u16::from_be_bytes([r[6], r[7]]);
    let mut i = 12;
    for _ in 0..qd { i = skip_name(r, i)? + 4; }
    for _ in 0..an {
        i = skip_name(r, i)?;
        let h = r.get(i..i + 10)?;
        let (ty, len) = (u16::from_be_bytes([h[0], h[1]]), u16::from_be_bytes([h[8], h[9]]) as usize);
        let data = r.get(i + 10..i + 10 + len)?;
        if ty == 1 && len == 4 { return Some(Ipv4Addr::new(data[0], data[1], data[2], data[3])) }
        i += 10 + len;
    }
    None
}

fn skip_name(r: &[u8], mut i: usize) -> Option<usize> {
    loop {
        let b = *r.get(i)?;
        if b & 0xc0 == 0xc0 { return Some(i + 2) }
        if b == 0 { return Some(i + 1) }
        i += 1 + b as usize;
    }
}
```

`lib.rs`: `//!` header (purpose: the tunnel's wire format, shared by `kl-connect` and `bench-net`,
see the spec), `pub mod dns; pub mod frame; pub mod handshake; pub use frame::*; pub use handshake::*;`.

- [ ] **Step 5: Run tests and clippy**

Run: `CARGO_TARGET_DIR=/Volumes/kdisk/target-env-tunnel cargo test -p kloudlite-nettun 2>&1 | tail -3 && cargo clippy -p kloudlite-nettun --all-targets -- -D warnings 2>&1 | tail -3`
Expected: all PASS, clippy clean.

- [ ] **Step 6: Commit**

```bash
git add crates/nettun Cargo.toml Cargo.lock
git commit -m "Add the tunnel wire format crate"
```

---

### Task 3: `bins/bench-net` — the bench end of the tunnel

Userspace design (owner ruling 2026-10-11, "Userspace stack"; spec §5): no TUN, no NAT, no
capability. Each stream terminates WireGuard and feeds its own `ipstack` userspace TCP/IP stack;
every TCP connection / UDP flow the laptop opens becomes an ordinary socket from the bench pod.

**Files:**
- Create: `bins/bench-net/Cargo.toml`, `bins/bench-net/src/{main,link}.rs`
- Modify: root `Cargo.toml` (`members` and `default-members` gain `"bins/bench-net"`; workspace deps
  `boringtun`, `ipstack`; x25519 comes from boringtun's re-export `boringtun::x25519`, no direct dep)

**Interfaces:**
- Consumes: `kloudlite_nettun::{read_frame, write_frame, Hello, Welcome, Pool, GATEWAY, PREFIX}`.
- Produces: binary `bench-net`; env `KL_NET_PORT` (default 7792), `KL_NET_STREAMS`
  (default `/run/kl-net/net.streams`); writes the open-stream count as decimal text every 5 s.
  `link::{admit, write_count, Link}`: `fn admit(src: Ipv4Addr, stream: Ipv4Addr) -> bool` (the
  anti-inject rule, `src == stream`); `Link` is the packet device `ipstack` reads and writes.

- [ ] **Step 1: Add deps and read their vendored source**

```toml
[package]
name = "bench-net"
version = "0.1.0"
edition = "2024"

[dependencies]
kloudlite-nettun = { workspace = true }
boringtun = { version = "0.6", default-features = false }
ipstack = "1.0.1"
tokio = { workspace = true, features = ["full"] }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }
rand_core = { version = "0.6", features = ["getrandom"] }

[dev-dependencies]
tempfile = { workspace = true }
```

Run `cargo fetch`, then read `~/.cargo/registry/src/*/boringtun-0.6*/src/noise/mod.rs` (`Tunn::new`,
`encapsulate`, `decapsulate`, `update_timers`, `TunnResult`) and
`~/.cargo/registry/src/*/ipstack-1.0.1/src/{lib.rs,stream/tcp.rs,stream/udp.rs}`. Facts already
read there: `IpStack::new(IpStackConfig, device)` takes any `AsyncRead + AsyncWrite + Unpin + Send +
'static`; its loop does ONE `device.read` per IP packet into an `mtu`-sized buffer and ONE
`write_all` per outgoing packet; `accept()` yields `IpStackStream::{Tcp, Udp, UnknownTransport,
UnknownNetwork}`; `peer_addr()` is the destination the laptop dialled; `TcpConfig::timeout` defaults
to 60 s idle. Every call below must match that source; where it differs, follow the source and note
it in the report.

- [ ] **Step 2: Write the failing tests** (`link.rs`)

```rust
#[test]
fn a_packet_from_the_wrong_address_is_dropped() {
    let a: Ipv4Addr = "100.96.0.2".parse().unwrap();
    let b: Ipv4Addr = "100.96.0.3".parse().unwrap();
    assert!(admit(a, a));
    assert!(!admit(b, a));      // stream a claiming b's source
}

#[test]
fn the_count_file_holds_the_number() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("net.streams");
    write_count(&p, 3).unwrap();
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "3");
    write_count(&p, 0).unwrap();
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "0");
}

#[tokio::test]
async fn the_link_moves_whole_packets_both_ways() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (mut link, inbound, mut outbound) = Link::new(4);
    inbound.send(vec![1, 2, 3]).await.unwrap();
    let mut buf = [0u8; 1500];
    assert_eq!(link.read(&mut buf).await.unwrap(), 3);      // one read = one packet
    assert_eq!(&buf[..3], &[1, 2, 3]);
    link.write_all(&[9, 9]).await.unwrap();
    assert_eq!(outbound.recv().await.unwrap(), vec![9, 9]); // one write = one packet
    drop(inbound);
    assert_eq!(link.read(&mut buf).await.unwrap(), 0);      // stream gone = EOF
}
```

- [ ] **Step 3: Run, expect failure**

Run: `CARGO_TARGET_DIR=/Volumes/kdisk/target-env-tunnel cargo test -p bench-net 2>&1 | tail -5`
Expected: FAIL (`cannot find function admit`).

- [ ] **Step 4: Implement `link.rs`**

```rust
//! Per-stream plumbing on the bench: the packet device a stream's userspace stack (`ipstack`) runs
//! on, the anti-inject rule, and the open-stream count the sessions daemon reads to stay awake
//! (`bench/sessions/main.ts` `streams()`).
use std::{io, net::Ipv4Addr, path::Path, pin::Pin, task::{Context, Poll}};
use tokio::{io::{AsyncRead, AsyncWrite, ReadBuf}, sync::mpsc};

/// A decrypted packet enters the stack only when its source is the address this stream was given.
/// Without this one stream could speak as another (spec §3: "a stream cannot inject packets for
/// another stream's address").
pub fn admit(src: Ipv4Addr, stream: Ipv4Addr) -> bool { src == stream }

/// Rename over, so the reader never sees a half-written number.
pub fn write_count(p: &Path, n: usize) -> io::Result<()> {
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, n.to_string())?;
    std::fs::rename(tmp, p)
}

/// `ipstack`'s device: reads hand it one decrypted packet each, writes take one packet each for the
/// stream to encrypt. A full outbound queue drops the packet, as a congested link would; TCP
/// retransmits, and a stalled laptop cannot grow the bench's memory.
pub struct Link { rx: mpsc::Receiver<Vec<u8>>, tx: mpsc::Sender<Vec<u8>> }

impl Link {
    /// (device, sender of inbound packets, receiver of outbound packets)
    pub fn new(depth: usize) -> (Link, mpsc::Sender<Vec<u8>>, mpsc::Receiver<Vec<u8>>) {
        let (in_tx, in_rx) = mpsc::channel(depth);
        let (out_tx, out_rx) = mpsc::channel(depth);
        (Link { rx: in_rx, tx: out_tx }, in_tx, out_rx)
    }
}

impl AsyncRead for Link {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        match self.rx.poll_recv(cx) {
            // ponytail: an over-MTU packet is truncated; the laptop utun MTU matches ours (Task 7)
            Poll::Ready(Some(p)) => { let n = p.len().min(buf.remaining()); buf.put_slice(&p[..n]); Poll::Ready(Ok(())) }
            Poll::Ready(None) => Poll::Ready(Ok(())),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl AsyncWrite for Link {
    fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, b: &[u8]) -> Poll<io::Result<usize>> {
        let _ = self.tx.try_send(b.to_vec());
        Poll::Ready(Ok(b.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> { Poll::Ready(Ok(())) }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> { Poll::Ready(Ok(())) }
}
```

- [ ] **Step 5: Implement `main.rs`** — `//!` header (container `net` in the bench pod, plain
hardened context, why userspace: the gVisor probe in spec §5, the spec path). Flow:

```rust
const MTU: u16 = 1420; // WireGuard's usual inner MTU; the laptop utun uses the same (Task 7)

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt().json().init();
    let port: u16 = std::env::var("KL_NET_PORT").ok().and_then(|v| v.parse().ok()).unwrap_or(7792);
    let count = std::env::var("KL_NET_STREAMS").unwrap_or_else(|_| "/run/kl-net/net.streams".into());
    let secret = boringtun::x25519::StaticSecret::random_from_rng(rand_core::OsRng);   // memory only
    let public = boringtun::x25519::PublicKey::from(&secret);
    let pool = Arc::new(Mutex::new(Pool::new()));
    let open = Arc::new(AtomicUsize::new(0));
    spawn count task: every 5 s write_count(&count, open.load()) (log and continue on error);
    let l = TcpListener::bind(("0.0.0.0", port)).await.unwrap();
    loop { let (s, _) = l.accept().await?; spawn stream(s, secret.clone(), public, pool.clone(), open.clone()) }
}
```

`stream()`:
1. `read_frame` → `Hello::decode` (else close). `pool.take()` (None → close, log `net.full`).
   `open += 1`.
2. `write_frame(Welcome { key: public.to_bytes(), addr })`.
3. `Tunn::new(secret, PublicKey::from(hello.key), None, Some(25), index, None)` (index = host part).
4. `let (link, inbound, mut outbound) = Link::new(256);` and
   `let mut cfg = IpStackConfig::default(); cfg.mtu(MTU)?;` with `TcpConfig { timeout: 2 h, ..Default }`
   via `with_tcp_config` (the 60 s default would cut idle database pools); `udp_timeout(60 s)`.
   `let mut stack = IpStack::new(cfg, link);`
5. `select!` loop:
   - frame in → `decapsulate(None, frame, out)`; `WriteToNetwork` → `write_frame` and keep calling
     `decapsulate(None, &[], out)` until `Done` (boringtun's queued-packet rule); `WriteToTunnelV4(p, src)`
     → `if admit(src, addr) { let _ = inbound.try_send(p.to_vec()) } else { tracing::warn!(%src, %addr, "net.inject") }`.
     `WriteToTunnelV6` → drop.
   - `outbound.recv()` → `encapsulate` → `write_frame`.
   - `stack.accept()` →
     - `Tcp(t)`: spawn `TcpStream::connect(t.peer_addr())` with a 10 s timeout, then
       `tokio::io::copy_bidirectional(&mut t, &mut s)`; connect failure drops `t` (the laptop sees a
       reset). Log `net.tcp` with `dst`, `ok`, bytes each way.
     - `Udp(u)`: spawn `UdpSocket::bind("0.0.0.0:0")`, `connect(u.peer_addr())`, then pump datagrams
       both ways (`u.read` → `sock.send`, `sock.recv` → `u.write_all`) until either side ends; one
       read of `u` is one datagram (udp.rs). This carries DNS to `10.43.0.10:53`.
     - `UnknownTransport`/`UnknownNetwork` (ICMP etc.): drop (spec: no ping).
   - 250 ms tick → `update_timers` → `WriteToNetwork` → `write_frame`; `Err(ConnectionExpired)` → close.
6. On any exit: drop the stack (its task aborts on `Drop`, closing every socket it spawned via the
   copies ending), `pool.free(addr)`, `open -= 1`. Hold each spawned copy's `JoinHandle` in a
   `tokio::task::JoinSet` owned by the stream so they abort with it.

- [ ] **Step 6: Tests + clippy pass**

Run: `CARGO_TARGET_DIR=/Volumes/kdisk/target-env-tunnel cargo test -p bench-net 2>&1 | tail -3; cargo clippy -p bench-net --all-targets -- -D warnings 2>&1 | tail -3`
Expected: PASS, clean. (The musl build runs in CI and Dagger; Task 8 wires it.)

- [ ] **Step 7: Commit**

```bash
git add bins/bench-net Cargo.toml Cargo.lock
git commit -m "Add the bench end of the environment tunnel"
```

---

### Task 4: Bench pod renders the `net` container

**Files:**
- Modify: `crates/workspaces/src/k8s/bench.rs` (`BENCH_NET_PORT`, new `net_container`, policy port)
- Modify: `crates/workspaces/src/k8s/workspace.rs` (`workspace_pod` bench branch: second container,
  volume `kl-net` emptyDir; `sessions` mounts `/run/kl-net` read-only)
- Modify: `crates/workspaces/src/k8s/mod.rs` (re-export `BENCH_NET_PORT` beside `BENCH_TUI_PORT`)
- Test: `crates/workspaces/src/k8s/tests/bench.rs`

**Interfaces:**
- Produces: `pub const BENCH_NET_PORT: u16 = 7792;` (gateway uses it); container name `net`; env
  `KL_NET_PORT=7792`, `KL_NET_STREAMS=/run/kl-net/net.streams` on `net`; mount `/run/kl-net` in both.

- [ ] **Step 1: Write the failing tests** (beside the existing ones, same helpers)

```rust
#[test]
fn the_net_container_is_plainly_hardened() {
    let pod = bench_pod_for_test();             // new helper, see below
    let cs = &pod.spec.as_ref().unwrap().containers;
    let net = cs.iter().find(|c| c.name == "net").unwrap();
    // no capability, no privilege: the workspace pod fence admits it unchanged (spec §5)
    assert_eq!(net.security_context, Some(crate::k8s::workspace::hardened()));
    assert_eq!(net.ports.as_ref().unwrap()[0].container_port, BENCH_NET_PORT as i32);
}

#[test]
fn both_containers_share_the_streams_dir() {
    let pod = bench_pod_for_test();
    let spec = pod.spec.as_ref().unwrap();
    let mounts = |name: &str| spec.containers.iter().find(|c| c.name == name).unwrap()
        .volume_mounts.clone().unwrap_or_default().into_iter().map(|m| (m.mount_path, m.read_only.unwrap_or(false))).collect::<Vec<_>>();
    assert!(mounts("net").contains(&("/run/kl-net".into(), false)));
    assert!(mounts(BENCH_CONTAINER).contains(&("/run/kl-net".into(), true)));
    assert!(spec.volumes.as_ref().unwrap().iter().all(|v| v.host_path.is_none() || v.name != "kl-net"));
}

#[test]
fn a_plain_workspace_has_no_net_container() {
    let pod = workspace_pod(&ws_spec(), "ws-1", "ws-1", &ctx(), None, None).unwrap();
    assert!(pod.spec.unwrap().containers.iter().all(|c| c.name != "net"));
}
```

Add `bench_pod_for_test()` to the test file: the whole `Pod` from the same `workspace_pod(...)` call
the existing `bench_container_for_test` helper makes (return the pod instead of picking the container).
If `hardened()` is private, make it `pub(crate)`. If `ws_spec()` is not the name of the
plain-workspace helper in `k8s/tests/`, use the one that file defines.

Update the existing assertions: l.68 `sessions` ports stay `[BENCH_TERM_PORT, BENCH_TUI_PORT]`
(the net port is on `net`); the policy test (l.160/172) port list becomes
`[BENCH_TERM_PORT as i32, BENCH_TUI_PORT as i32, BENCH_NET_PORT as i32]`.

- [ ] **Step 2: Run, expect failure**

Run: `CARGO_TARGET_DIR=/Volumes/kdisk/target-env-tunnel cargo test -p kloudlite-workspaces k8s::tests::bench 2>&1 | tail -8`
Expected: FAIL (`BENCH_NET_PORT` not found).

- [ ] **Step 3: Implement**

`bench.rs`:

```rust
/// `bench-net`'s listener: the environment tunnel (`docs/superpowers/specs/2026-10-11-env-tunnel-design.md`).
/// A gateway hole beside `BENCH_TUI_PORT`, on its own container so a tunnel crash never restarts `sessions`.
pub const BENCH_NET_PORT: u16 = 7792;
pub const NET_CONTAINER: &str = "net";

pub(super) fn net_container(image: &str) -> Container {
    Container {
        name: NET_CONTAINER.into(),
        image: Some(image.into()),
        command: Some(vec!["/usr/local/bin/bench-net".into()]),
        env: Some(vec![
            EnvVar { name: "KL_NET_PORT".into(), value: Some(BENCH_NET_PORT.to_string()), ..Default::default() },
            EnvVar { name: "KL_NET_STREAMS".into(), value: Some("/run/kl-net/net.streams".into()), ..Default::default() },
        ]),
        ports: Some(vec![ContainerPort { container_port: BENCH_NET_PORT as i32, name: Some("net".into()), ..Default::default() }]),
        volume_mounts: Some(vec![
            VolumeMount { name: "kl-net".into(), mount_path: "/run/kl-net".into(), ..Default::default() },
        ]),
        resources: Some(ResourceRequirements {
            requests: Some([("cpu".into(), Quantity("10m".into())), ("memory".into(), Quantity("32Mi".into()))].into()),
            limits: Some([("memory".into(), Quantity("256Mi".into()))].into()),
            ..Default::default()
        }),
        security_context: Some(super::workspace::hardened()),
        ..Default::default()
    }
}
```

`bench-net` writes only to `/run/kl-net` (the count file and its `.tmp`), so it needs no `/tmp`.
The `net` image is the bench image (same `bench` tuple's image string). The memory limit covers
per-stream stacks and socket buffers; ponytail: 256 Mi, raise if `net` is OOM-killed under load.

`allow_gateway_bench`: add `{ "protocol": "TCP", "port": BENCH_NET_PORT as i32 }`.

`bench_container`: add `VolumeMount { name: "kl-net", mount_path: "/run/kl-net", read_only: Some(true) }`.

`workspace_pod` bench branch: push `net_container(image)`; volumes gain
`Volume { name: "kl-net", empty_dir: Some(EmptyDirVolumeSource { medium: Some("Memory".into()), size_limit: Some(Quantity("1Mi".into())) }) }`.
Update `bench.rs`'s `//!` header: the 2026-09-25 "no second container" ruling now has one
exception, `net`, and why (own lifecycle; no shell, no workspace; plain hardened context).

Check the bench pod's egress: whatever NetworkPolicy lets the `sessions` container reach the space's
environment must also cover `net` (same pod, so the same policy; confirm by reading `policies.rs`
`space-env` and say so in the report).

- [ ] **Step 4: Run tests + clippy**

Run: `CARGO_TARGET_DIR=/Volumes/kdisk/target-env-tunnel cargo test -p kloudlite-workspaces k8s 2>&1 | tail -3; cargo clippy -p kloudlite-workspaces --all-targets -- -D warnings 2>&1 | tail -3`
Expected: PASS, clean.

- [ ] **Step 5: Commit**

```bash
git add crates/workspaces/src/k8s
git commit -m "Render the bench net container"
```

---

### Task 5: An open tunnel keeps the bench awake

**Files:**
- Modify: `bench/sessions/main.ts` (`streams()` beside `busy()` l.21; `isIdle` l.56; `idleBody()` l.64)
- Create: `bench/sessions/streams.ts` (pure function) and `bench/sessions/streams.test.ts`

**Interfaces:**
- Consumes: `/run/kl-net/net.streams` (decimal text, Task 3), env override `KL_NET_STREAMS`.
- Produces: `export function openStreams(text: string | null, mtimeMs: number, now: number): number`
  — `0` when `text` is null, unparsable, or `now - mtimeMs > 15_000`.

- [ ] **Step 1: Write the failing test** (`node --test`, type stripping; node 24)

```ts
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { openStreams } from './streams.ts'

test('a fresh non-zero count keeps the bench awake', () => {
  assert.equal(openStreams('2', 1_000, 10_000), 2)
})
test('a stale count holds nothing open', () => {
  assert.equal(openStreams('2', 1_000, 16_001), 0)
})
test('a missing or garbled file counts zero', () => {
  assert.equal(openStreams(null, 0, 0), 0)
  assert.equal(openStreams('x', 1_000, 2_000), 0)
})
```

- [ ] **Step 2: Run, expect failure**

Run: `cd bench/sessions && node --test streams.test.ts 2>&1 | tail -5`
Expected: FAIL (cannot find module `./streams.ts`).

- [ ] **Step 3: Implement**

```ts
// streams.ts — the bench-net container's open-tunnel count (`/run/kl-net/net.streams`, written every
// 5 s). Stale after 15 s: a dead `net` container holds nothing open. Owner ruling 2026-10-11: "I want
// the bench to be awake as long as there is connection."
export const STALE_MS = 15_000
export function openStreams(text: string | null, mtimeMs: number, now: number): number {
  if (text === null || now - mtimeMs > STALE_MS) return 0
  const n = Number.parseInt(text.trim(), 10)
  return Number.isFinite(n) && n > 0 ? n : 0
}
```

In `main.ts`, beside `busy()`, read the file the same way `busy()` reads `~/.kl/host.busy`
(sync `statSync` + `readFileSync` in a try, `null` on error), path
`process.env.KL_NET_STREAMS ?? '/run/kl-net/net.streams'`; `isIdle` becomes
`clients() === 0 && !busy() && streams() === 0`; `idleBody()` gains `streams: streams()`.

- [ ] **Step 4: Run test; typecheck by running the daemon's own entry once**

Run: `cd bench/sessions && node --test streams.test.ts 2>&1 | tail -3 && node --check main.ts`
Expected: 3 pass; no syntax error. (If `node --check` refuses `.ts`, use
`node --experimental-strip-types --check main.ts`.)

- [ ] **Step 5: Commit**

```bash
git add bench/sessions
git commit -m "Keep the bench awake while a tunnel stream is open"
```

---

### Task 6: Gateway `/net/{bench}` route

**Files:**
- Modify: `bins/gateway/src/tunnel.rs` (l.43 field, l.63 `new`, l.200 routes, l.219-252 `connect`)
- Modify: `bins/gateway/src/main.rs:57-63` (pass `BENCH_NET_PORT`)
- Modify: `bins/gateway/tests/tui.rs` (`Gateway::new` gains the new argument)
- Create: `bins/gateway/tests/net.rs`

**Interfaces:**
- Consumes: `kloudlite_workspaces::k8s::BENCH_NET_PORT` (Task 4).
- Produces: `Gateway::new(jwt, region, kube, ssh_port, term_port, tui_port, net_port)`; route
  `GET /net/{bench}` (websocket), bench session token, dials `net_port`.

- [ ] **Step 1: Write the failing tests** — copy `tests/tui.rs` whole to `tests/net.rs`, then: `serve(net)`
passes `Gateway::new(jwt(), REGION.into(), client, 1, 1, 1, net)` (TUI port closed, net port the echo),
`req` builds `ws://{addr}/net/bench-1`, and rename the tests:
`net_route_pumps_to_the_net_port`, `a_workspace_token_is_refused_on_the_net_route`,
`a_spent_token_is_refused_on_the_net_route`. Bodies unchanged apart from the names. In `tests/tui.rs`
change its `Gateway::new(..., 1, 1, tui)` to `Gateway::new(..., 1, 1, tui, 1)`.

- [ ] **Step 2: Run, expect failure**

Run: `CARGO_TARGET_DIR=/Volumes/kdisk/target-env-tunnel cargo test -p kloudlite-gateway --test net 2>&1 | tail -5`
Expected: FAIL (`Gateway::new` takes 6 arguments).

- [ ] **Step 3: Implement**

- Field `pub net_port: u16` beside `tui_port` (doc: `BENCH_NET_PORT` everywhere real).
- `new(..., tui_port: u16, net_port: u16)`.
- `.route("/net/{bench}", get(net_tunnel))`; `net_tunnel` = `tui_tunnel` with `Some(gw.net_port)`.
- `connect(..., bench: Option<u16>)` replaces `tui: bool`: `None` = workspace door,
  `Some(port)` = bench door. The verify branch is `if bench.is_some()`, the resolve arm is
  `Ticket::Bench(_) => resolve_bench(&gw.kube, &ws, bench.unwrap_or(gw.tui_port))`. Update the comment
  above it: "/tui/ and /net/ take bench tokens". `tunnel` passes `None`, `tui_tunnel` passes
  `Some(gw.tui_port)`.
- `main.rs`: append `kloudlite_workspaces::k8s::BENCH_NET_PORT`.

- [ ] **Step 4: Run all gateway tests + clippy**

Run: `CARGO_TARGET_DIR=/Volumes/kdisk/target-env-tunnel cargo test -p kloudlite-gateway 2>&1 | grep -E "test result|FAILED|panicked" ; cargo clippy -p kloudlite-gateway --all-targets -- -D warnings 2>&1 | tail -3`
Expected: all `ok`, clean.

- [ ] **Step 5: Commit**

```bash
git add bins/gateway
git commit -m "Pipe the environment tunnel through the gateway to the bench"
```

---

### Task 7: `kl-connect env` — CLI, hosts block, launchd, daemon

**Files:**
- Create: `bins/kl-connect/src/env/mod.rs` (`//!` header, `EnvCmd` clap enum, dispatch)
- Create: `bins/kl-connect/src/env/hosts.rs` (pure block rewrite + atomic install)
- Create: `bins/kl-connect/src/env/launchd.rs` (plist render, install, uninstall)
- Create: `bins/kl-connect/src/env/tunnel.rs` (utun + route + boringtun + stream)
- Create: `bins/kl-connect/src/env/daemon.rs` (10 s loop, status file)
- Modify: `bins/kl-connect/src/api.rs` (three space calls + environment GET)
- Modify: `bins/kl-connect/src/main.rs` (`Cmd::Env { #[command(subcommand)] cmd: env::EnvCmd }`, `mod env;`)
- Modify: `bins/kl-connect/Cargo.toml` (`kloudlite-nettun`, `boringtun`, `tun`, `rand_core` — boringtun/rand_core as in Task 3, `tun = { version = "0.7", features = ["async"] }`; x25519 via `boringtun::x25519`)

**Interfaces:**
- Consumes: `api::{Config, bench_session, SessionAnswer, client}`, `proxy::{gateway_url, connect, pump_io}`,
  `config::{dir, write_atomic, load}`, `kloudlite_nettun::*`.
- Produces:
  - `api::Space { team: String, environment: String, region: Option<String> }` (Deserialize).
  - `api::spaces(cfg) -> Result<Vec<Space>, Error>`, `api::set_space(cfg, team, env) -> Result<(), Error>`,
    `api::clear_space(cfg, team) -> Result<(), Error>`, `api::environment(cfg, id) -> Result<EnvDoc, Error>`
    with `EnvDoc { id: String, name: String, services: Vec<EnvService> }`, `EnvService { name: String, ports: Vec<u16> }`.
  - `hosts::render(current: &str, block: Option<&Block>) -> (String, Vec<String>)` (new file text, skipped names);
    `Block { env: String, name: String, entries: Vec<(String, Ipv4Addr)> }`;
    `hosts::install(path: &Path, text: &str) -> Result<(), String>`.
  - `launchd::plist(bin: &Path, home: &Path) -> String`.
  - files `{home}/.config/kl-connect/env.json` `{"team": "<team>"}` 0600, and
    `{home}/.config/kl-connect/env-status.json`
    `{"environment","name","state","names":[..],"skipped":[..],"error","at"}`.

- [ ] **Step 1: Write the failing hosts tests** (`hosts.rs`)

```rust
fn block() -> Block {
    Block { env: "ab12".into(), name: "dev".into(),
            entries: vec![("mongodb".into(), "10.43.12.7".parse().unwrap()), ("api".into(), "10.43.40.2".parse().unwrap())] }
}
const BASE: &str = "127.0.0.1\tlocalhost\n::1\tlocalhost\n";

#[test]
fn writes_one_block_and_leaves_the_rest() {
    let (t, skipped) = render(BASE, Some(&block()));
    assert!(t.starts_with(BASE));
    assert!(t.ends_with("# kl-connect begin env=ab12 name=dev\n10.43.12.7\tmongodb\n10.43.40.2\tapi\n# kl-connect end\n"));
    assert!(skipped.is_empty());
}

#[test]
fn rewriting_is_idempotent_and_removal_restores_the_file() {
    let (once, _) = render(BASE, Some(&block()));
    assert_eq!(render(&once, Some(&block())).0, once);
    assert_eq!(render(&once, None).0, BASE);
}

#[test]
fn a_name_defined_outside_is_skipped() {
    let base = format!("{BASE}10.0.0.9 api # mine\n");
    let (t, skipped) = render(&base, Some(&block()));
    assert_eq!(skipped, vec!["api".to_string()]);
    assert!(t.contains("10.43.12.7\tmongodb\n") && !t.contains("10.43.40.2"));
    assert!(t.starts_with(&base));
}

#[test]
fn a_file_without_a_trailing_newline_keeps_its_last_line() {
    let (t, _) = render("127.0.0.1 localhost", Some(&block()));
    assert!(t.starts_with("127.0.0.1 localhost\n# kl-connect begin"));
    assert_eq!(render(&t, None).0, "127.0.0.1 localhost\n");
}

#[test]
fn a_stale_temp_file_is_replaced() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("hosts");
    std::fs::write(&p, BASE).unwrap();
    std::fs::write(d.path().join("hosts.kl-connect.tmp"), "garbage").unwrap();
    install(&p, "new\n").unwrap();
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "new\n");
    assert!(!d.path().join("hosts.kl-connect.tmp").exists());
}
```

And in `launchd.rs`:

```rust
#[test]
fn the_plist_runs_the_daemon_for_that_home() {
    let p = plist(Path::new("/usr/local/bin/kl-connect"), Path::new("/Users/a"));
    for s in ["<string>io.kloudlite.kl-connect</string>", "<string>/usr/local/bin/kl-connect</string>",
              "<string>env</string>", "<string>daemon</string>", "<string>--home</string>",
              "<string>/Users/a</string>", "<key>RunAtLoad</key>", "<key>KeepAlive</key>"] {
        assert!(p.contains(s), "{s}");
    }
}
```

- [ ] **Step 2: Run, expect failure**

Run: `CARGO_TARGET_DIR=/Volumes/kdisk/target-env-tunnel cargo test -p kl-connect env:: 2>&1 | tail -5`
Expected: FAIL (`cannot find function render`).

- [ ] **Step 3: Implement `hosts.rs`**

```rust
//! The `/etc/hosts` block: one marked block, everything outside it left byte for byte. A bare name
//! the person defined outside the block is never shadowed — skipped and reported instead.
//! Written by temp file in the same directory and rename, so a killed daemon never leaves half a file.
use std::{net::Ipv4Addr, path::Path};

pub const BEGIN: &str = "# kl-connect begin";
pub const END: &str = "# kl-connect end";

pub struct Block { pub env: String, pub name: String, pub entries: Vec<(String, Ipv4Addr)> }

pub fn render(current: &str, block: Option<&Block>) -> (String, Vec<String>) {
    let mut out = String::new();
    let mut inside = false;
    for line in current.split_inclusive('\n') {
        let l = line.trim_end_matches(['\n', '\r']);
        if l.starts_with(BEGIN) { inside = true; continue }
        if inside { if l == END { inside = false } continue }
        out.push_str(line);
    }
    if !out.is_empty() && !out.ends_with('\n') { out.push('\n') }
    let Some(b) = block else { return (out, vec![]) };
    let taken: std::collections::HashSet<&str> = out.lines()
        .map(|l| l.split('#').next().unwrap_or(""))
        .flat_map(|l| l.split_whitespace().skip(1))
        .collect();
    let mut skipped = vec![];
    let mut body = format!("{BEGIN} env={} name={}\n", b.env, b.name);
    for (n, ip) in &b.entries {
        if taken.contains(n.as_str()) { skipped.push(n.clone()); continue }
        body.push_str(&format!("{ip}\t{n}\n"));
    }
    body.push_str(END);
    body.push('\n');
    out.push_str(&body);
    (out, skipped)
}

pub fn install(path: &Path, text: &str) -> Result<(), String> {
    let tmp = path.with_file_name(format!("{}.kl-connect.tmp", path.file_name().unwrap().to_string_lossy()));
    std::fs::write(&tmp, text).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("rename over {}: {e}", path.display()))
}

/// macOS caches /etc/hosts in the directory service and mDNSResponder.
pub fn flush() {
    let _ = std::process::Command::new("dscacheutil").arg("-flushcache").status();
    let _ = std::process::Command::new("killall").args(["-HUP", "mDNSResponder"]).status();
}
```

`taken` borrows from `out` while `out` is then mutated — collect to `HashSet<String>` instead if the
borrow checker refuses. `/etc/hosts` keeps mode 0644: after `install` on `/etc/hosts`, `chmod 644`
via `std::fs::set_permissions`.

- [ ] **Step 4: Implement `launchd.rs`**

```rust
//! The root LaunchDaemon. Installed by `sudo kl-connect env daemon install`: root because the daemon
//! rewrites /etc/hosts and creates a utun device; `--home` because root's own home holds no login.
pub const LABEL: &str = "io.kloudlite.kl-connect";
pub const PLIST: &str = "/Library/LaunchDaemons/io.kloudlite.kl-connect.plist";

pub fn plist(bin: &Path, home: &Path) -> String {
    format!(r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key><array>
    <string>{}</string><string>env</string><string>daemon</string><string>--home</string><string>{}</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardErrorPath</key><string>/var/log/kl-connect.log</string>
</dict></plist>
"#, xml(bin), xml(home))
}
fn xml(p: &Path) -> String { p.display().to_string().replace('&', "&amp;").replace('<', "&lt;") }

pub fn install() -> Result<(), String> {
    let user = std::env::var("SUDO_USER").map_err(|_| "run with sudo: sudo kl-connect env daemon install".to_string())?;
    let out = std::process::Command::new("dscl").args([".", "-read", &format!("/Users/{user}"), "NFSHomeDirectory"])
        .output().map_err(|e| e.to_string())?;
    let home = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim().strip_prefix("NFSHomeDirectory: ")
        .ok_or(format!("no home for {user}"))?);
    let bin = std::env::current_exe().and_then(|p| p.canonicalize()).map_err(|e| e.to_string())?;
    let _ = launchctl(&["bootout", &format!("system/{LABEL}")]);   // reinstall replaces
    std::fs::write(PLIST, plist(&bin, &home)).map_err(|e| format!("write {PLIST}: {e}"))?;
    launchctl(&["bootstrap", "system", PLIST])
}

pub fn uninstall() -> Result<(), String> {
    let _ = launchctl(&["bootout", &format!("system/{LABEL}")]);
    let _ = std::fs::remove_file(PLIST);
    let cur = std::fs::read_to_string("/etc/hosts").map_err(|e| e.to_string())?;
    let (t, _) = super::hosts::render(&cur, None);
    if t != cur { super::hosts::install(Path::new("/etc/hosts"), &t)?; super::hosts::flush(); }
    Ok(())
}
```

`launchctl(args)`: runs `/bin/launchctl`, `Err` with stderr on non-zero exit.

- [ ] **Step 5: Implement `api.rs` additions** (same `client()`/`json<T>`/`path_segment` helpers the file already uses)

```rust
#[derive(serde::Deserialize, Clone, PartialEq)]
pub struct Space { pub team: String, pub environment: String, pub region: Option<String> }
#[derive(serde::Deserialize)]
pub struct EnvDoc { pub id: String, pub name: String, #[serde(default)] pub services: Vec<EnvService> }
#[derive(serde::Deserialize)]
pub struct EnvService { pub name: String, #[serde(default)] pub ports: Vec<u16> }

pub async fn spaces(cfg: &Config) -> Result<Vec<Space>, Error>               // GET  {api}/v1/me/environments
pub async fn set_space(cfg: &Config, team: &str, env: &str) -> Result<(), Error>   // PUT  {api}/v1/me/environments/{team} {"environment": env}
pub async fn clear_space(cfg: &Config, team: &str) -> Result<(), Error>     // DELETE ... 204 (404 is Ok: already clear)
pub async fn environment(cfg: &Config, id: &str) -> Result<EnvDoc, Error>   // GET  {api}/v1/environments/{id}
```

Read `model::Environment` (`crates/workspaces/src/model.rs:406`) and confirm the field names `id`,
`name`, `services[].name`, `services[].ports`; follow the model where it differs. A 403/404 must be
distinguishable by the daemon: add `Error::Gone(u16)` for 403/404 if `json<T>` folds them into
`Other` today.

- [ ] **Step 6: Implement `tunnel.rs`** — `//!` header (laptop end; why datagrams ride the websocket:
spec §Constraints). Exposes:

```rust
pub struct Up { pub addr: Ipv4Addr, task: tokio::task::JoinHandle<String>, iface: String }
pub async fn bring_up(cfg: &Config, team: &str) -> Result<Up, String>;
impl Up { pub fn is_finished(&self) -> bool; pub async fn down(self); }
```

`bring_up`:
1. Loop `api::bench_session(cfg, team)` while `Waking` (1 s sleep, 90 s deadline) — the exact loop
   `bench.rs::serve` runs; move that loop into a shared `bench::session_ready(cfg, team)` and call it
   from both places rather than copying it.
2. `url = proxy::gateway_url(&session.gateway).replacen("/tunnel/", "/net/", 1)`; `ws = proxy::connect(&url, &session.token)`.
3. `let (mine, theirs) = tokio::io::duplex(1 << 16)`; spawn `proxy::pump_io(ws, theirs_r, theirs_w, Some(15s))`
   (split `theirs` with `tokio::io::split`). The tunnel code speaks frames on `mine`.
4. Key pair (memory, generated once per daemon life and passed in — make `bring_up` take
   `&StaticSecret`). `write_frame(Hello)`; `read_frame` → `Welcome::decode` (else `Err`).
5. `tun::Configuration`: `address(welcome.addr)`, `destination(GATEWAY)`, `netmask(255,255,255,255)`, `up()`,
   macOS utun (name chosen by the kernel; read it back from the device). Then
   `route -n add -net 10.43.0.0/16 -interface {iface}` (`route -n delete` first, ignore its failure).
6. Spawn the pump: `Tunn::new(secret, PublicKey::from(welcome.key), None, Some(25), 0, None)`;
   `select!` over utun packet → `encapsulate` → `write_frame`; frame → `decapsulate` (drain queued with
   `&[]` until `Done`) → `WriteToTunnelV4` → write to utun; 250 ms `update_timers`. Returns the reason
   string when the stream ends.

`down`: abort the task, `route -n delete -net 10.43.0.0/16`, drop the device.

`resolve(names: &[String], env_hex: &str) -> Vec<(String, Ipv4Addr)>`: one `std::net::UdpSocket`
(bound `0.0.0.0:0`, read timeout 2 s), send `dns::a_query(i, "{svc}.env-{hex}.svc.cluster.local")`
to `10.43.0.10:53` for each name, collect `dns::a_answer`. Runs in `spawn_blocking`. The env
namespace is `env-{id}`: confirm against `crd/names.rs::env_namespace` and build it the same way
(do not depend on the workspaces crate; copy the one-line rule with a comment naming the source).

- [ ] **Step 7: Implement `daemon.rs`** — the loop from spec §2, as written there. State:
`current: Option<(Space, Up)>`, `backoff: Duration` (1 s → ×2 → 30 s cap, reset on a pass that ends
up). Each 10 s pass:

```
cfg = config::load_from(home)?                 // login config under {home}/.config/kl-connect
team = read env.json; None -> tear_down("disconnected"), continue
space = api::spaces(cfg).find(team)            // Err(Gone) or None -> tear_down(reason); other Err -> keep, backoff
if current.space != space or current.up.is_finished() -> tear_down(keep_hosts = up.is_finished()), bring_up (on Err: status error, backoff, keep hosts)
doc = api::environment(cfg, space.environment) // Gone -> tear_down(remove hosts)
entries = resolve(doc.services names)
(text, skipped) = hosts::render(read /etc/hosts, Some(block)); if text != current file -> install + chmod 644 + flush
write env-status.json (0600, chown to the home's owner uid so `env status` can read it)
```

`tear_down(remove_hosts: bool)`: `up.down()`, and when `remove_hosts` render `None` and install. The
error table in the spec decides `remove_hosts`: unreachable/waking/bench restarted/offline keep it;
403/404/no environment/disconnected remove it. `config::load` needs a variant reading from an
explicit home: add `config::dir_in(home: &Path) -> PathBuf` = `home/.config/kl-connect` and use it
when `--home` is given (root's `KL_CONFIG_DIR`/`XDG_CONFIG_HOME` are not the user's).

- [ ] **Step 8: Implement `mod.rs` and wire `main.rs`**

```rust
#[derive(clap::Subcommand)]
pub enum EnvCmd {
    /// Connect the laptop to an environment (the space's environment for that team).
    Connect { env: String, #[arg(long)] team: Option<String> },
    Disconnect,
    Status,
    /// Run or manage the background daemon.
    Daemon { #[command(subcommand)] cmd: Option<DaemonCmd>, #[arg(long, hide = true)] home: Option<PathBuf> },
}
#[derive(clap::Subcommand)]
pub enum DaemonCmd { Install, Uninstall }
```

- `connect`: refuse unless `launchd::PLIST` exists ("run `sudo kl-connect env daemon install` first");
  team = `--team` else the top-level `--team` else `cfg.username`; `api::set_space`; write `env.json`
  `{"team": team}` with `config::write_atomic` then `chmod 600`. Print `connected <env> (team <team>); names appear within 10 s`.
- `disconnect`: read `env.json` (none → "not connected"), `api::clear_space`, delete `env.json`.
- `status`: print `env-status.json` fields as lines (`environment`, `state`, `names`, `skipped`, `error`, `at`).
- `daemon` with no subcommand: refuse unless euid 0 (`libc::geteuid`), require `--home`, run `daemon::run(home)`.
- `daemon install|uninstall`: refuse unless euid 0.

The `Daemon` with no subcommand is hidden from help (`#[command(hide = true)]` is on the subcommand
docs only through `about`; keep `install`/`uninstall` visible).

- [ ] **Step 9: Run tests + clippy**

Run: `CARGO_TARGET_DIR=/Volumes/kdisk/target-env-tunnel cargo test -p kl-connect 2>&1 | grep -E "test result|FAILED|panicked"; cargo clippy -p kl-connect --all-targets -- -D warnings 2>&1 | tail -3`
Expected: all `ok` (old bench tests still pass after the `session_ready` move), clean.

- [ ] **Step 10: Commit**

```bash
git add bins/kl-connect Cargo.lock
git commit -m "Connect the laptop to the space's environment through the bench"
```

---

### Task 8: Ship `bench-net` in the bench image

**Files:**
- Modify: `.github/workflows/image.yml:111` (`-p bench-net` in the musl build) and the `kl-musl`
  artifact path list (l.140-143) gains `target/x86_64-unknown-linux-musl/release/bench-net`
- Modify: `deploy/bench/Dockerfile` (beside the `kl` COPY at l.38:
  `COPY target/x86_64-unknown-linux-musl/release/bench-net /usr/local/bin/bench-net`; no apt package:
  the userspace stack needs no `iptables`/`iproute2`)
- Modify: `deploy/dagger/src/index.ts` (~l.287-323, `imageBench` l.399: the same `-p bench-net` and copy)

**Interfaces:**
- Consumes: binary `bench-net` (Task 3), command path `/usr/local/bin/bench-net` (Task 4).

- [ ] **Step 1: Make the three edits.** Read each file's existing `kl` line and mirror it exactly for
`bench-net`.

- [ ] **Step 2: Verify the musl build compiles on the laptop** (cross target already used by CI;
if the target is not installed locally, `rustup target add x86_64-unknown-linux-musl`). If the laptop
lacks a musl linker, skip this run and say so in the report; CI is the gate.

Run: `CARGO_TARGET_DIR=/Volumes/kdisk/target-env-tunnel cargo build --release --locked -p bench-net --target x86_64-unknown-linux-musl 2>&1 | tail -3`
Expected: `Finished`.

- [ ] **Step 3: Check the Dockerfile and workflow agree**

Run: `grep -n bench-net .github/workflows/image.yml deploy/bench/Dockerfile deploy/dagger/src/index.ts`
Expected: one build line + one artifact line in the workflow, one COPY in the Dockerfile, the build
and copy in Dagger.

- [ ] **Step 4: Commit**

```bash
git add .github/workflows/image.yml deploy/bench/Dockerfile deploy/dagger/src/index.ts
git commit -m "Ship bench-net in the bench image"
```

---

### Task 9: SLO probe `env.tunnel.reach` and the end-to-end check

**Files:**
- Modify: `crates/workspaces/src/slo/catalogue.rs:267` (`CATALOGUE` gains the entry)
- Modify: `deploy/slo.md` (matching table row)

**Interfaces:**
- Consumes: everything above, deployed.

- [ ] **Step 1: Add the entry** (stage and suite copied from the nearest bench entry already in the
catalogue; feature `Environments`)

```rust
Slo { id: "env.tunnel.reach", feature: "Environments",
      sli: "kl-connect env connect, then nc -z <service> <port> by bare name from the laptop",
      target: avail(99.0), suite: Suite::Hourly, stage: "<the bench entries' stage>" },
```

and the row in `deploy/slo.md`:

```
| `env.tunnel.reach` | Environments | kl-connect env connect, then nc -z <service> <port> by bare name from the laptop | 99.0 % | hourly | <same stage> |
```

If `Suite::Hourly` is not the variant name, use the hourly one the file defines.

- [ ] **Step 2: Run the catalogue test**

Run: `CARGO_TARGET_DIR=/Volumes/kdisk/target-env-tunnel cargo test -p kloudlite-workspaces the_catalogue_matches_deploy_slo_md 2>&1 | tail -3`
Expected: PASS.

- [ ] **Step 3: Full workspace gate**

Run: `CARGO_TARGET_DIR=/Volumes/kdisk/target-env-tunnel cargo test 2>&1 | grep -E "test result: FAILED|panicked|error\[" | head; cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -3`
Expected: no lines from the first, clean clippy.

- [ ] **Step 4: Commit**

```bash
git add crates/workspaces/src/slo/catalogue.rs deploy/slo.md
git commit -m "Add the environment tunnel reach probe to the catalogue"
```

- [ ] **Step 5: End to end (after the owner rolls the bench image and gateway; never rolled unasked)**

```bash
sudo kl-connect env daemon install
kl-connect env connect <env-id>
sleep 12; kl-connect env status
grep -A20 '# kl-connect begin' /etc/hosts
nc -z <service> <port> && echo REACHED
kl-connect env disconnect; sleep 12; grep -c 'kl-connect begin' /etc/hosts   # expect 0
```

Expected: status `up`, the block lists the environment's services, `REACHED`, block gone after
disconnect, and the bench idles `bench_idle_secs` after disconnect (not before).
