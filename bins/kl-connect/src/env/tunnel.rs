//! The laptop end of the environment tunnel (the cluster end is `bins/bench-net/src/stream.rs`).
//! WireGuard datagrams ride the gateway websocket `/net/{bench}` because that is the only door
//! the laptop can open through Cloudflare (spec "Constraints": no UDP, no inbound). boringtun runs
//! in userspace between a `utun` device and a length-framed stream; a route sends the cluster's
//! service CIDR into the device so cluster DNS and service IPs are reachable by plain sockets.
//!
//! The task owns the device: aborting it (`Up::down`) closes the utun, and the websocket pump it
//! guards is aborted with it, so nothing outlives a tear-down.
use crate::config::Config;
use boringtun::noise::{errors::WireGuardError, Tunn, TunnResult};
use boringtun::x25519::{PublicKey, StaticSecret};
use kloudlite_nettun::{dns, read_frame, write_frame, Hello, Welcome, GATEWAY};
use std::net::Ipv4Addr;
use std::time::Duration;
use tun::AbstractDevice;

/// WireGuard's usual inner MTU; `bins/bench-net/src/stream.rs` uses the same.
const MTU: u16 = 1420;
/// The cluster's service CIDR and its DNS service (k3s defaults, as in the spec).
const SERVICE_CIDR: &str = "10.43.0.0/16";
const CLUSTER_DNS: &str = "10.43.0.10:53";

pub struct Up {
    pub addr: Ipv4Addr,
    task: tokio::task::JoinHandle<String>,
    pub iface: String,
    /// Holds the environment awake while the tunnel is open; aborted in `down`.
    hold: tokio::task::JoinHandle<()>,
}

impl Up {
    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }

    pub async fn down(self) {
        self.hold.abort();
        self.task.abort();
        let _ = self.task.await; // the device is dropped with the task
        delete_route();
    }
}

/// Aborts the websocket pump when the tunnel task ends by any path.
struct AbortOnDrop(tokio::task::JoinHandle<Result<(), String>>);
impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub async fn bring_up(cfg: &Config, team: &str, env: &str, secret: &StaticSecret) -> Result<Up, String> {
    let session = crate::bench::session_ready(cfg, Some(team), |_| {}).await?;
    let url = crate::proxy::gateway_url(&session.gateway).replacen("/tunnel/", "/net/", 1);
    let ws = crate::proxy::connect(&url, &session.token).await?;
    let (mut mine, theirs) = tokio::io::duplex(1 << 16);
    let (tr, tw) = tokio::io::split(theirs);
    let pump = AbortOnDrop(tokio::spawn(crate::proxy::pump_io(ws, tr, tw, Some(Duration::from_secs(15)))));

    let mut buf = Box::new([0u8; 65535]);
    let public = PublicKey::from(secret);
    write_frame(&mut mine, &Hello { key: public.to_bytes() }.encode()).await.map_err(|e| e.to_string())?;
    let n = tokio::time::timeout(Duration::from_secs(10), read_frame(&mut mine, &mut buf))
        .await
        .map_err(|_| "tunnel handshake timed out".to_string())?
        .map_err(|e| e.to_string())?
        .ok_or("tunnel closed during handshake (bench has no free address?)")?;
    let welcome = Welcome::decode(&buf[..n]).ok_or("bad tunnel welcome")?;

    let mut c = tun::Configuration::default();
    c.address(welcome.addr).destination(GATEWAY).netmask(Ipv4Addr::new(255, 255, 255, 255)).mtu(MTU).up();
    // We add the one route we want ourselves; the crate's own would route the /32 peer. macOS-only
    // knob: the workspace build compiles this binary on Linux too.
    #[cfg(target_os = "macos")]
    c.platform_config(|p| {
        p.enable_routing(false);
    });
    let dev = tun::create_as_async(&c).map_err(|e| format!("utun: {e}"))?;
    let iface = dev.tun_name().map_err(|e| format!("utun name: {e}"))?;
    delete_route();
    route(&["add", "-net", SERVICE_CIDR, "-interface", &iface])?;

    let tunn = Tunn::new(secret.clone(), PublicKey::from(welcome.key), None, Some(25), u32::from(welcome.addr) & 0xffff, None);
    let task = tokio::spawn(run(dev, mine, tunn, buf, pump));
    let hold = {
        let (cfg, env) = (cfg.clone(), env.to_string());
        tokio::spawn(async move {
            let mut beat = tokio::time::interval(Duration::from_secs(60));
            loop {
                beat.tick().await;
                // best effort: the next beat retries, and the idle window is far longer than one miss
                let _ = crate::api::hold_env(&cfg, &env).await;
            }
        })
    };
    Ok(Up { addr: welcome.addr, task, iface, hold })
}

/// Device <-> boringtun <-> frames until either side ends; the return value is why.
async fn run(
    dev: tun::AsyncDevice,
    mine: tokio::io::DuplexStream,
    mut tunn: Tunn,
    mut buf: Box<[u8; 65535]>,
    pump: AbortOnDrop,
) -> String {
    let (mut rd, mut wr) = tokio::io::split(mine);
    // read_frame is not cancel-safe (read_exact), so it gets its own task and select! only sees
    // whole frames -- same shape as the cluster end.
    let (ftx, mut frx) = tokio::sync::mpsc::channel::<Vec<u8>>(64);
    let reader = tokio::spawn(async move {
        while let Ok(Some(n)) = read_frame(&mut rd, &mut buf).await {
            if ftx.send(buf[..n].to_vec()).await.is_err() {
                break;
            }
        }
    });
    let mut out = Box::new([0u8; 65535]);
    let mut pkt = vec![0u8; 4096];
    let mut tick = tokio::time::interval(Duration::from_millis(250));
    let reason = loop {
        tokio::select! {
            f = frx.recv() => {
                let Some(f) = f else { break "stream closed".to_string() };
                // After a WriteToNetwork boringtun may hold queued packets, which only come out by
                // calling again with an empty datagram until Done.
                let mut src: &[u8] = &f;
                loop {
                    match tunn.decapsulate(None, src, &mut out[..]) {
                        TunnResult::WriteToNetwork(d) => {
                            if let Err(e) = write_frame(&mut wr, d).await { return format!("stream write: {e}") }
                        }
                        TunnResult::WriteToTunnelV4(p, src) => {
                            // Only the cluster's services may talk back; the bench side is not trusted
                            // to hand the laptop packets from anywhere else.
                            if in_service_range(src) {
                                // `send` adds the 4-byte address-family header utun wants.
                                let _ = dev.send(p).await;
                            } // else dropped (kl-connect has no tracing dep; the drop is the whole effect)
                            break;
                        }
                        _ => break,
                    }
                    src = &[];
                }
            }
            n = dev.recv(&mut pkt) => {
                // `recv` strips the utun header, so boringtun only ever sees raw IPv4.
                let Ok(n) = n else { break "utun read failed".to_string() };
                if let TunnResult::WriteToNetwork(d) = tunn.encapsulate(&pkt[..n], &mut out[..]) {
                    if let Err(e) = write_frame(&mut wr, d).await { break format!("stream write: {e}") }
                }
            }
            _ = tick.tick() => match tunn.update_timers(&mut out[..]) {
                TunnResult::WriteToNetwork(d) => {
                    if let Err(e) = write_frame(&mut wr, d).await { break format!("stream write: {e}") }
                }
                TunnResult::Err(WireGuardError::ConnectionExpired) => break "tunnel expired".to_string(),
                _ => {}
            },
        }
    };
    reader.abort();
    // The pump's own error (lost connection, close) is the useful reason when it has one.
    let mut pump = pump;
    if pump.0.is_finished() {
        if let Ok(Err(e)) = (&mut pump.0).await {
            return e;
        }
    }
    reason
}

fn route(args: &[&str]) -> Result<(), String> {
    let o = std::process::Command::new("/sbin/route")
        .arg("-n")
        .args(args)
        .output()
        .map_err(|e| format!("route: {e}"))?;
    if o.status.success() {
        Ok(())
    } else {
        Err(format!("route {}: {}", args.join(" "), String::from_utf8_lossy(&o.stderr).trim()))
    }
}

/// Failure ignored: there is nothing to delete on a first bring-up or after the device vanished.
fn delete_route() {
    let _ = route(&["delete", "-net", SERVICE_CIDR]);
}

/// The environment's namespace, `crd::names::env_namespace` in `crates/workspaces` (one line,
/// copied rather than depended on so the laptop binary does not link the workspaces crate).
fn env_namespace(id: &str) -> String {
    let id = id.to_lowercase();
    format!("env-{}", id.strip_prefix("env-").unwrap_or(&id))
}

/// Each service's A record through cluster DNS (over the tunnel). A name that does not resolve
/// (a service with no ports has no ClusterIP Service, or the lookup was lost) is left out rather
/// than failing the pass; the caller keeps its last address. Each name gets two tries of 2 s, and
/// a late reply to an earlier query is read and dropped (id mismatch) instead of eating the next.
fn in_service_range(a: Ipv4Addr) -> bool {
    a.octets()[..2] == [10, 43]
}

pub fn resolve(names: &[String], env_id: &str) -> Vec<(String, Ipv4Addr)> {
    let Ok(sock) = std::net::UdpSocket::bind("0.0.0.0:0") else { return vec![] };
    let ns = env_namespace(env_id);
    let mut out = vec![];
    let mut buf = [0u8; 1500];
    for (i, n) in names.iter().enumerate() {
        let id = i as u16;
        let q = dns::a_query(id, &format!("{n}.{ns}.svc.cluster.local"));
        'tries: for _ in 0..2 {
            if sock.send_to(&q, CLUSTER_DNS).is_err() {
                continue;
            }
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            loop {
                let left = deadline.saturating_duration_since(std::time::Instant::now());
                if left.is_zero() || sock.set_read_timeout(Some(left)).is_err() {
                    break;
                }
                let Ok(len) = sock.recv(&mut buf) else { break };
                if let Some(ip) = dns::a_answer(id, &buf[..len]) {
                    out.push((n.clone(), ip));
                    break 'tries;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_namespace_is_env_dash_id_whether_or_not_the_id_has_the_prefix() {
        assert_eq!(env_namespace("AB12"), "env-ab12");
        assert_eq!(env_namespace("env-ab12"), "env-ab12");
    }
}
