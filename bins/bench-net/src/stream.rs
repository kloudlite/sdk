//! One laptop stream: handshake, WireGuard (boringtun) in, userspace TCP/IP (`ipstack`) out to the
//! pod's own sockets. Everything the stream spawns lives in a `JoinSet` owned by `serve`, so
//! dropping it on any exit closes every socket the laptop opened.
use crate::link::{Link, admit};
use boringtun::noise::{Tunn, TunnResult};
use boringtun::x25519::{PublicKey, StaticSecret};
use ipstack::{IpStack, IpStackConfig, IpStackStream, TcpConfig};
use kloudlite_nettun::{Hello, Pool, Welcome, read_frame, write_frame};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, atomic::{AtomicUsize, Ordering}};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpStream, UdpSocket, tcp::OwnedWriteHalf};
use tokio::task::JoinSet;

const MTU: u16 = 1420; // WireGuard's usual inner MTU; the laptop utun uses the same (Task 7)

pub async fn run(
    mut s: TcpStream,
    secret: StaticSecret,
    public: PublicKey,
    pool: Arc<Mutex<Pool>>,
    open: Arc<AtomicUsize>,
) {
    let mut buf = Box::new([0u8; 65535]);
    let hello = match read_frame(&mut s, &mut buf).await {
        Ok(Some(n)) => Hello::decode(&buf[..n]),
        _ => None,
    };
    let Some(hello) = hello else { return };
    let Some(addr) = pool.lock().unwrap().take() else {
        tracing::warn!("net.full");
        return;
    };
    open.fetch_add(1, Ordering::Relaxed);
    // A guard, not code after `serve`: a panic in this task must still free the slot.
    let _slot = Slot { pool, open, addr };
    let r = serve(s, hello, secret, public, addr, buf).await;
    tracing::info!(%addr, result = ?r, "net.stream.end");
}

struct Slot {
    pool: Arc<Mutex<Pool>>,
    open: Arc<AtomicUsize>,
    addr: Ipv4Addr,
}

impl Drop for Slot {
    fn drop(&mut self) {
        // ponytail: a poisoned pool lock leaks the address; the pod restarts on that anyway.
        if let Ok(mut p) = self.pool.lock() {
            p.free(self.addr);
        }
        self.open.fetch_sub(1, Ordering::Relaxed);
    }
}

async fn serve(
    s: TcpStream,
    hello: Hello,
    secret: StaticSecret,
    public: PublicKey,
    addr: Ipv4Addr,
    mut buf: Box<[u8; 65535]>,
) -> std::io::Result<()> {
    let (mut rd, mut wr) = s.into_split();
    write_frame(&mut wr, &Welcome { key: public.to_bytes(), addr }.encode()).await?;
    let mut tunn = Tunn::new(secret, PublicKey::from(hello.key), None, Some(25), u32::from(addr) & 0xffff, None);

    let (link, inbound, mut outbound) = Link::new(256);
    let mut cfg = IpStackConfig::default();
    cfg.mtu(MTU).map_err(std::io::Error::other)?;
    // The 60 s idle default would cut idle database pools.
    let mut tcp = TcpConfig::default();
    tcp.timeout = Duration::from_secs(2 * 3600);
    cfg.with_tcp_config(tcp).udp_timeout(Duration::from_secs(60));
    let mut stack = IpStack::new(cfg, link);
    let mut jobs: JoinSet<()> = JoinSet::new();

    // read_frame is not cancel-safe (read_exact), so it gets its own task and select! only sees
    // whole frames.
    let (ftx, mut frx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    jobs.spawn(async move {
        while let Ok(Some(n)) = read_frame(&mut rd, &mut buf).await {
            if ftx.send(buf[..n].to_vec()).await.is_err() {
                break;
            }
        }
    });

    let mut out = Box::new([0u8; 65535]);
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    loop {
        tokio::select! {
            f = frx.recv() => {
                let Some(f) = f else { return Ok(()) };
                feed(&mut tunn, &f, &mut out, &mut wr, &inbound, addr).await?;
            }
            Some(p) = outbound.recv() => {
                if let TunnResult::WriteToNetwork(d) = tunn.encapsulate(&p, &mut out[..]) {
                    write_frame(&mut wr, d).await?;
                }
            }
            a = stack.accept() => match a {
                Ok(IpStackStream::Tcp(t)) => { jobs.spawn(tcp_job(t)); }
                Ok(IpStackStream::Udp(u)) => { jobs.spawn(udp_job(u)); }
                Ok(_) => {} // ICMP etc.: no ping (spec)
                Err(_) => return Ok(()),
            },
            _ = tick.tick() => match tunn.update_timers(&mut out[..]) {
                TunnResult::WriteToNetwork(d) => write_frame(&mut wr, d).await?,
                TunnResult::Err(boringtun::noise::errors::WireGuardError::ConnectionExpired) => return Ok(()),
                TunnResult::Err(e) => tracing::debug!(error = ?e, "net.timer"),
                _ => {}
            },
            _ = jobs.join_next(), if !jobs.is_empty() => {}
        }
    }
}

/// One encrypted datagram in. After a `WriteToNetwork` boringtun may hold queued packets, which
/// only come out by calling again with an empty datagram until `Done`.
async fn feed(
    tunn: &mut Tunn,
    frame: &[u8],
    out: &mut [u8; 65535],
    wr: &mut OwnedWriteHalf,
    inbound: &tokio::sync::mpsc::Sender<Vec<u8>>,
    addr: Ipv4Addr,
) -> std::io::Result<()> {
    let mut src: &[u8] = frame;
    loop {
        match tunn.decapsulate(None, src, &mut out[..]) {
            TunnResult::WriteToNetwork(d) => write_frame(wr, d).await?,
            TunnResult::WriteToTunnelV4(p, from) => {
                if admit(from, addr) {
                    let _ = inbound.try_send(p.to_vec());
                } else {
                    tracing::warn!(%from, %addr, "net.inject");
                }
                return Ok(());
            }
            TunnResult::Err(e) => {
                tracing::debug!(error = ?e, "net.decap");
                return Ok(());
            }
            TunnResult::Done | TunnResult::WriteToTunnelV6(..) => return Ok(()),
        }
        src = &[];
    }
}

/// The cluster's service range, the only thing a laptop may reach through the bench: the pod's
/// own loopback, node and metadata addresses are not the laptop's to dial.
const SERVICE_CIDR: (Ipv4Addr, u8) = (Ipv4Addr::new(10, 43, 0, 0), 16);

fn allowed(dst: SocketAddr) -> bool {
    let IpAddr::V4(ip) = dst.ip() else { return false };
    let (net, bits) = SERVICE_CIDR;
    u32::from(ip) >> (32 - bits) == u32::from(net) >> (32 - bits)
}

async fn tcp_job(mut t: ipstack::IpStackTcpStream) {
    let dst = t.peer_addr();
    if !allowed(dst) {
        tracing::info!(%dst, ok = false, reason = "outside service range", "net.tcp");
        return; // dropping `t` resets the laptop's connection
    }
    let conn = tokio::time::timeout(Duration::from_secs(10), TcpStream::connect(dst)).await;
    let Ok(Ok(mut s)) = conn else {
        tracing::info!(%dst, ok = false, "net.tcp");
        return; // dropping `t` resets the laptop's connection
    };
    let r = tokio::io::copy_bidirectional(&mut t, &mut s).await;
    let (up, down) = r.as_ref().map(|&(a, b)| (a, b)).unwrap_or((0, 0));
    tracing::info!(%dst, ok = true, up, down, "net.tcp");
}

/// One read of `u` is one datagram (udp.rs); this carries DNS to the cluster resolver.
async fn udp_job(mut u: ipstack::IpStackUdpStream) {
    use tokio::io::AsyncReadExt;
    if !allowed(u.peer_addr()) {
        tracing::info!(dst = %u.peer_addr(), ok = false, reason = "outside service range", "net.udp");
        return;
    }
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0").await else { return };
    if sock.connect(u.peer_addr()).await.is_err() {
        return;
    }
    let (mut a, mut b) = (vec![0u8; 65535], vec![0u8; 65535]);
    loop {
        tokio::select! {
            n = u.read(&mut a) => match n {
                Ok(n) if n > 0 => { if sock.send(&a[..n]).await.is_err() { return } }
                _ => return,
            },
            n = sock.recv(&mut b) => match n {
                Ok(n) => { if u.write_all(&b[..n]).await.is_err() { return } }
                Err(_) => return,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_service_range_is_dialled() {
        let a = |s: &str| s.parse::<SocketAddr>().unwrap();
        assert!(allowed(a("10.43.0.10:53")) && allowed(a("10.43.255.1:80")));
        for s in ["127.0.0.1:8917", "10.42.0.5:80", "169.254.169.254:80", "8.8.8.8:53", "[::1]:80"] {
            assert!(!allowed(a(s)), "{s}");
        }
    }
}
