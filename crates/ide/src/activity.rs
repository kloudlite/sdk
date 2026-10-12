//! Session activity, measured where every session meets the pod (spec 2026-10-12 idle stop).
//! The agent on the same node reads ONLY the mtime of `~/.kl/activity` from the volume; this
//! process keeps that mtime fresh while anything counts as activity: a request in flight or just
//! ended, an open non-watcher stream, `POST /hold`, or an ESTABLISHED inbound connection on a
//! listening port other than ours (ssh/ide/proxy tunnels, ttyd, intercept traffic).
//! NOT activity: `/healthz`, any request with `X-KL-Watch: 1` (the sidebar and explorer poll every
//! ready workspace), and a detached process (its connections are outbound).
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

// ponytail: fixed 30 s throttle and beat, no knob; the agent's verdict works in minutes.
pub const THROTTLE: Duration = Duration::from_secs(30);

pub struct Activity {
    path: PathBuf,
    in_flight: AtomicUsize,
    streams: AtomicUsize,
    last: Mutex<Option<Instant>>,
}

pub struct StreamGuard(Arc<Activity>);
impl Drop for StreamGuard {
    fn drop(&mut self) {
        self.0.streams.fetch_sub(1, Ordering::Relaxed);
        self.0.touch();
    }
}

impl Activity {
    pub fn new(path: PathBuf) -> Self {
        Activity { path, in_flight: AtomicUsize::new(0), streams: AtomicUsize::new(0), last: Mutex::new(None) }
    }
    pub fn touch(&self) {
        let mut last = self.last.lock().unwrap_or_else(|p| p.into_inner());
        if last.is_some_and(|t| t.elapsed() < THROTTLE) {
            return;
        }
        *last = Some(Instant::now());
        drop(last);
        self.write();
    }
    pub fn touch_now(&self) {
        *self.last.lock().unwrap_or_else(|p| p.into_inner()) = Some(Instant::now());
        self.write();
    }
    fn write(&self) {
        // Rewriting the file is the portable way to bump an mtime; the content is a marker only.
        if let Some(d) = self.path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Err(e) = std::fs::write(&self.path, "active") {
            tracing::warn!(error = %e, "activity.write_failed");
        }
    }
    pub fn stream(self: &Arc<Self>) -> StreamGuard {
        self.streams.fetch_add(1, Ordering::Relaxed);
        self.touch();
        StreamGuard(self.clone())
    }
    pub fn busy(&self) -> bool {
        self.in_flight.load(Ordering::Relaxed) > 0 || self.streams.load(Ordering::Relaxed) > 0
    }
    /// The 30 s beat: refresh while a request or stream is open, or a tunnel is connected.
    pub async fn beat(self: Arc<Self>, tool_port: u16) {
        let mut t = tokio::time::interval(THROTTLE);
        loop {
            t.tick().await;
            let net = ["/proc/net/tcp", "/proc/net/tcp6"]
                .iter()
                .filter_map(|p| std::fs::read_to_string(p).ok())
                .any(|s| inbound(&s, tool_port));
            if self.busy() || net {
                self.touch();
            }
        }
    }
}

/// True when some ESTABLISHED (st 01) socket's local port is a LISTENING (st 0A) port other than
/// `exclude`. Inbound = accepted on a listener; a process dialing out uses an ephemeral port no
/// one listens on, so it never counts.
pub fn inbound(tcp: &str, exclude: u16) -> bool {
    let rows: Vec<(u16, &str)> = tcp
        .lines()
        .skip(1)
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let local = f.nth(1)?;
            let st = f.nth(1)?;
            let port = u16::from_str_radix(local.rsplit(':').next()?, 16).ok()?;
            Some((port, st))
        })
        .collect();
    let listening: std::collections::HashSet<u16> = rows.iter().filter(|(p, st)| *st == "0A" && *p != exclude).map(|(p, _)| *p).collect();
    rows.iter().any(|(p, st)| *st == "01" && listening.contains(p))
}

/// Counts a request while it runs and touches when it ends. `/healthz` and watchers pass through.
pub async fn track(
    axum::extract::State(a): axum::extract::State<Arc<Activity>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let watcher = req.uri().path() == "/healthz" || req.headers().get("x-kl-watch").is_some_and(|v| v == "1");
    if watcher {
        return next.run(req).await;
    }
    a.in_flight.fetch_add(1, Ordering::Relaxed);
    let res = next.run(req).await;
    a.in_flight.fetch_sub(1, Ordering::Relaxed);
    a.touch();
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    const TCP: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
       0: 00000000:1E6C 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 1 1 0000000000000000 100 0 0 10 0
       1: 00000000:0016 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 2 1 0000000000000000 100 0 0 10 0
       2: 0100007F:1E6C 0100007F:9C40 01 00000000:00000000 00:00000000 00000000  1000        0 3 1 0000000000000000 20 4 30 10 -1
    ";

    #[test]
    fn inbound_ignores_tool_port_and_listeners() {
        // 0x1E6C = 7788: listening and ESTABLISHED, excluded. 0x0016 = 22: listening only.
        assert!(!inbound(TCP, 7788));
    }

    #[test]
    fn inbound_sees_established_on_another_listening_port() {
        let t = format!("{TCP}   3: 0A2A0005:0016 0A2A0009:D431 01 00000000:00000000 00:00000000 00000000     0        0 4 1 0000000000000000 20 4 30 10 -1\n");
        assert!(inbound(&t, 7788));
    }

    #[test]
    fn outbound_from_a_process_is_not_inbound() {
        // Local port 0xD431 is an ephemeral port, not a listening one: a process dialing out.
        let t = format!("{TCP}   3: 0A2A0005:D431 0A2A0009:01BB 01 00000000:00000000 00:00000000 00000000  1000        0 4 1 0000000000000000 20 4 30 10 -1\n");
        assert!(!inbound(&t, 7788));
    }

    #[test]
    fn touch_is_throttled() {
        let d = tempfile::tempdir().unwrap();
        let a = Activity::new(d.path().join(".kl/activity"));
        a.touch_now();
        let m1 = std::fs::metadata(d.path().join(".kl/activity")).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        a.touch();
        let m2 = std::fs::metadata(d.path().join(".kl/activity")).unwrap().modified().unwrap();
        assert_eq!(m1, m2, "a second write inside 30 s is skipped");
    }
}
