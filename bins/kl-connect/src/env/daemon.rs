//! The root daemon's loop (spec section 2): holds `GET /v1/me/environments/watch` and, while
//! connected, `GET /v1/environments/{id}/watch` open, and watches `/etc/hosts` itself, and runs a
//! pass as soon as any of them moves. Every change is followed at once: a switch, services added
//! to or readied in the environment, a person editing `/etc/hosts`. The 60 s beat only covers login
//! expiry and a dropped tunnel (plus a 2 s retry while a new service does not resolve yet). An
//! environment switch keeps the tunnel and rewrites only the hosts block. A pass reads what the
//! person chose, makes the tunnel and `/etc/hosts` match it, and say what happened in
//! `env-status.json`. No socket: the CLI writes
//! `env.json` (the wish) under the person's config dir, which the daemon only READS (it runs as root
//! with root's `HOME` and is told whose home to serve); the report goes to `super::STATUS` under
//! /var/run, because root never writes inside a directory the user controls.
//!
//! Which failures keep the hosts block and which remove it is the spec's error table: transient
//! trouble (api, gateway, bench waking or restarted, laptop offline) keeps the last good names so a
//! running app keeps resolving; access lost, environment or space gone, or `disconnect` removes
//! them, because the names would now point at nothing or at someone else's addresses.
use super::{hosts, tunnel};
use crate::api::{self, Error, Space};
use boringtun::x25519::StaticSecret;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::time::Duration;

const HOSTS: &str = "/etc/hosts";
// Login expiry and a dropped tunnel; everything else arrives through a watch.
const BEAT: Duration = Duration::from_secs(60);
// While a service name did not resolve (its Service object is not there yet) look again soon.
// ponytail: a name that never resolves costs one pass per 2 s; back off if that ever shows up.
const RETRY: Duration = Duration::from_secs(2);
const BACKOFF_CAP: Duration = Duration::from_secs(30);

#[derive(serde::Serialize, Default, Clone)]
pub struct Status {
    pub environment: String,
    pub name: String,
    pub state: String,
    pub names: Vec<String>,
    pub skipped: Vec<String>,
    pub error: String,
    pub at: String,
}

struct Daemon {
    home: PathBuf,
    /// Lives for the daemon's life; the bench session, not this key, is the authentication.
    secret: StaticSecret,
    current: Option<(Space, tunnel::Up)>,
    /// The names last written to /etc/hosts; a lookup that misses keeps its old address.
    last: Vec<(String, Ipv4Addr)>,
    backoff: Duration,
    /// The ETag of the selection the watch last returned.
    seen: Option<String>,
    /// The ETag of the connected environment the watch last returned; cleared with the environment
    /// so the first watch on a new one returns at once.
    env_seen: Option<String>,
    /// The last pass had a service name with no address yet.
    retry: bool,
    /// `/etc/hosts` events; the watcher must live as long as the daemon.
    hosts_rx: Option<tokio::sync::mpsc::UnboundedReceiver<()>>,
    _hosts_watcher: Option<notify::RecommendedWatcher>,
}

/// Does this event touch the file `hosts` itself? We replace it by rename, so the directory is
/// watched and everything else in it (our `.kl-connect.tmp`, resolv.conf) is noise.
fn is_hosts_event(ev: &notify::Event) -> bool {
    ev.paths.iter().any(|p| p.file_name().is_some_and(|n| n == "hosts"))
}

/// Watches the DIRECTORY of /etc/hosts (`/etc` is a symlink to `/private/etc`). Our own rewrite
/// fires an event too: the pass it causes finds `text == cur` and writes nothing, so no loop.
/// `None` (logged once) if it cannot be made; the beat still runs.
fn watch_hosts() -> (Option<tokio::sync::mpsc::UnboundedReceiver<()>>, Option<notify::RecommendedWatcher>) {
    use notify::{RecursiveMode, Watcher};
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let made = (|| -> Result<_, String> {
        let dir = Path::new(HOSTS).parent().unwrap().canonicalize().map_err(|e| e.to_string())?;
        let mut w = notify::recommended_watcher(move |r: notify::Result<notify::Event>| {
            if r.is_ok_and(|ev| is_hosts_event(&ev)) {
                let _ = tx.send(());
            }
        })
        .map_err(|e| e.to_string())?;
        w.watch(&dir, RecursiveMode::NonRecursive).map_err(|e| e.to_string())?;
        Ok(w)
    })();
    match made {
        Ok(w) => (Some(rx), Some(w)),
        Err(e) => {
            eprintln!("kl-connect env: not watching {HOSTS}: {e}");
            (None, None)
        }
    }
}

pub async fn run(home: PathBuf) -> Result<(), String> {
    let (hosts_rx, _hosts_watcher) = watch_hosts();
    let mut d = Daemon {
        home,
        secret: StaticSecret::random_from_rng(rand_core::OsRng),
        current: None,
        last: vec![],
        backoff: Duration::from_secs(1),
        seen: None,
        env_seen: None,
        retry: false,
        hosts_rx,
        _hosts_watcher,
    };
    loop {
        match d.pass().await {
            Ok(()) => {
                d.backoff = Duration::from_secs(1);
                d.until_change(BEAT).await;
            }
            Err(e) => {
                eprintln!("kl-connect env: {e}");
                let b = d.backoff;
                d.backoff = (b * 2).min(BACKOFF_CAP);
                tokio::time::sleep(b).await;
            }
        }
    }
}

#[derive(Debug, PartialEq)]
enum Change {
    None,
    /// Same team, other environment: the tunnel is per team, only the names move.
    Hosts,
    Tunnel,
}

fn change(current: Option<&Space>, wanted: &Space) -> Change {
    match current {
        Some(s) if s.team != wanted.team => Change::Tunnel,
        Some(s) if s.environment != wanted.environment => Change::Hosts,
        _ => Change::None,
    }
}

impl Daemon {
    /// Returns at the first of: the selection moved, the connected environment moved, `/etc/hosts`
    /// was touched, or the timer (`RETRY` while a name is unresolved, else `beat`).
    /// The first watch has no tag and returns at once; that one extra pass at start is the price
    /// of never missing a change made between a pass and its watch.
    async fn until_change(&mut self, beat: Duration) {
        let home = self.home.clone();
        let env = self.current.as_ref().map(|(s, _)| s.environment.clone());
        let wait = if self.retry { RETRY } else { beat };
        let Self { seen, env_seen, hosts_rx, .. } = self;
        // Events from before this wait were answered by the pass that just ran.
        if let Some(rx) = hosts_rx.as_mut() {
            while rx.try_recv().is_ok() {}
        }
        // A watch that cannot be held (old api without the route, offline) waits forever: the
        // timer still runs the pass.
        let spaces = async {
            loop {
                let Ok(cfg) = crate::config::load_from(&home) else { return std::future::pending::<()>().await };
                match api::watch_spaces(&cfg, seen.as_deref()).await {
                    Ok(Some(t)) => {
                        *seen = Some(t);
                        return;
                    }
                    Ok(None) => {}
                    Err(_) => return std::future::pending::<()>().await,
                }
            }
        };
        let environment = async {
            let Some(id) = env else { return std::future::pending::<()>().await };
            loop {
                let Ok(cfg) = crate::config::load_from(&home) else { return std::future::pending::<()>().await };
                match api::watch_environment(&cfg, &id, env_seen.as_deref()).await {
                    Ok(Some(t)) => {
                        *env_seen = Some(t);
                        return;
                    }
                    Ok(None) => {}
                    Err(_) => return std::future::pending::<()>().await,
                }
            }
        };
        let edited = async {
            match hosts_rx.as_mut() {
                Some(rx) => {
                    rx.recv().await;
                }
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = spaces => {}
            _ = environment => {}
            _ = edited => {}
        }
    }

    fn dir(&self) -> PathBuf {
        crate::config::dir_in(&self.home)
    }

    fn status(&self, st: Status) {
        let st = Status { at: now_rfc3339(), ..st };
        let dir = Path::new(super::STATUS).parent().unwrap();
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::set_permissions(dir, std::os::unix::fs::PermissionsExt::from_mode(0o755));
        // 0644 via install's default: names only, no secrets. Temp + rename like /etc/hosts.
        if let Err(e) = hosts::install(Path::new(super::STATUS), &serde_json::to_string_pretty(&st).unwrap()) {
            eprintln!("kl-connect env: status: {e}");
        }
    }

    /// `Err` means "retry with backoff"; every other outcome (including a deliberate tear-down)
    /// is `Ok` and has already written the status.
    async fn pass(&mut self) -> Result<(), String> {
        let fail = |d: &Self, e: String, env: &str| {
            d.status(Status { environment: env.into(), state: "error".into(), error: e.clone(), ..Default::default() });
            Err(e)
        };
        let cfg = match crate::config::load_from(&self.home) {
            Ok(c) => c,
            Err(e) => return self.logged_out(e).await,
        };
        let Some(team) = read_team(&self.dir()) else {
            self.tear_down(true).await;
            self.status(Status { state: "disconnected".into(), ..Default::default() });
            return Ok(());
        };
        let space = match api::spaces(&cfg).await {
            Ok(v) => v.into_iter().find(|s| s.team == team),
            Err(Error::Gone(c, m)) => {
                self.tear_down(true).await;
                self.status(Status { state: "gone".into(), error: format!("{c}: {m}"), ..Default::default() });
                return Ok(());
            }
            Err(Error::Unauthorized) => return self.logged_out(Error::Unauthorized.to_string()).await,
            Err(e) => return fail(self, e.to_string(), ""),
        };
        let Some(space) = space else {
            self.tear_down(true).await;
            self.status(Status { state: "no environment".into(), error: format!("team {team} has no environment"), ..Default::default() });
            return Ok(());
        };

        match change(self.current.as_ref().map(|(s, _)| s), &space) {
            Change::Tunnel => self.tear_down(true).await,
            // Old addresses belong to the old environment's services.
            Change::Hosts => {
                self.last.clear();
                self.env_seen = None;
                if let Some((s, _)) = &mut self.current {
                    *s = space.clone();
                }
            }
            Change::None => {}
        }
        match &self.current {
            // The stream ended (bench restarted, laptop slept): reconnect, last names stay meanwhile.
            Some((_, up)) if up.is_finished() => self.tear_down(false).await,
            _ => {}
        }
        if self.current.is_none() {
            match tunnel::bring_up(&cfg, &team, &space.environment, &self.secret).await {
                Ok(up) => {
                    eprintln!("kl-connect env: up on {} as {}", up.iface, up.addr);
                    self.current = Some((space.clone(), up));
                }
                Err(e) => return fail(self, e, &space.environment),
            }
        }

        let doc = match api::environment(&cfg, &space.environment).await {
            Ok(d) => d,
            Err(Error::Gone(c, m)) => {
                self.tear_down(true).await;
                self.status(Status { environment: space.environment.clone(), state: "gone".into(), error: format!("{c}: {m}"), ..Default::default() });
                return Ok(());
            }
            Err(Error::Unauthorized) => return self.logged_out(Error::Unauthorized.to_string()).await,
            Err(e) => return fail(self, e.to_string(), &space.environment),
        };
        // Laptop slept past the idle window: nothing else starts an idle-stopped environment, and
        // the hold beat alone never does. Wake it before resolving names against it.
        if let Err(e) = wake_if_idle(&cfg, &doc).await {
            return fail(self, e.to_string(), &space.environment);
        }
        let names: Vec<String> = doc.services.iter().map(|s| s.name.clone()).collect();
        let id = doc.id.clone();
        let asked = names.clone();
        let fresh = tokio::task::spawn_blocking(move || tunnel::resolve(&asked, &id)).await.map_err(|e| e.to_string())?;
        if fresh.is_empty() && !names.is_empty() {
            return fail(self, "cluster DNS did not answer over the tunnel".into(), &doc.id);
        }
        self.retry = names.iter().any(|n| !fresh.iter().any(|(m, _)| m == n));
        let entries = merge(&names, &fresh, &self.last);
        self.last = entries.clone();
        let block = hosts::Block { env: doc.id.clone(), name: doc.name.clone(), entries };
        let cur = std::fs::read_to_string(HOSTS).map_err(|e| format!("read {HOSTS}: {e}"))?;
        let (text, skipped) = match hosts::render(&cur, Some(&block)) {
            Ok(r) => r,
            Err(e) => {
                self.status(Status { environment: doc.id, state: "error".into(), error: e.clone(), ..Default::default() });
                return Err(e);
            }
        };
        if text != cur {
            if let Err(e) = hosts::install(Path::new(HOSTS), &text) {
                return fail(self, e, &doc.id);
            }
            hosts::flush();
        }
        self.status(Status {
            environment: doc.id,
            name: doc.name,
            state: "up".into(),
            names: block.entries.iter().map(|(n, _)| n.clone()).filter(|n| !skipped.contains(n)).collect(),
            skipped,
            ..Default::default()
        });
        Ok(())
    }

    /// No login to act for: names would point at an environment the person can no longer reach.
    /// Not an `Err`: the next beat checks again, without backoff.
    async fn logged_out(&mut self, e: String) -> Result<(), String> {
        self.tear_down(true).await;
        self.status(Status { state: "logged out".into(), error: e, ..Default::default() });
        Ok(())
    }

    async fn tear_down(&mut self, remove_hosts: bool) {
        self.env_seen = None;
        if let Some((_, up)) = self.current.take() {
            up.down().await;
        }
        if remove_hosts {
            self.last.clear();
            if let Ok(cur) = std::fs::read_to_string(HOSTS) {
                if let Ok((t, _)) = hosts::render(&cur, None) {
                    if t != cur && hosts::install(Path::new(HOSTS), &t).is_ok() {
                        hosts::flush();
                    }
                }
            }
        }
    }
}

/// One address per service in the doc: the fresh answer, else the one written last time. A service
/// gone from the doc is dropped (a real change, not a miss).
fn merge(names: &[String], fresh: &[(String, Ipv4Addr)], last: &[(String, Ipv4Addr)]) -> Vec<(String, Ipv4Addr)> {
    let find = |v: &[(String, Ipv4Addr)], n: &String| v.iter().find(|(m, _)| m == n).map(|(_, ip)| *ip);
    names.iter().filter_map(|n| find(fresh, n).or_else(|| find(last, n)).map(|ip| (n.clone(), ip))).collect()
}

fn read_team(dir: &Path) -> Option<String> {
    let s = std::fs::read_to_string(dir.join("env.json")).ok()?;
    serde_json::from_str::<serde_json::Value>(&s).ok()?.get("team")?.as_str().map(str::to_string)
}

/// UTC, to the second. Hand-rolled (days-from-civil) because this is the only timestamp the
/// binary writes and a date crate is not worth linking for it.
fn now_rfc3339() -> String {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    rfc3339(t)
}

fn rfc3339(t: u64) -> String {
    let (days, rem) = ((t / 86400) as i64, t % 86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

async fn wake_if_idle(cfg: &crate::config::Config, doc: &api::EnvDoc) -> Result<(), Error> {
    if doc.idle_since.is_some() {
        api::ensure_running(cfg, api::Kind::Environment, &doc.id).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sp(t: &str, e: &str) -> Space {
        Space { team: t.into(), environment: e.into(), region: None }
    }

    #[test]
    fn change_keeps_the_tunnel_for_an_environment_switch() {
        assert_eq!(change(Some(&sp("a", "e1")), &sp("a", "e2")), Change::Hosts);
        assert_eq!(change(Some(&sp("a", "e1")), &sp("b", "e1")), Change::Tunnel);
        assert_eq!(change(Some(&sp("a", "e1")), &sp("a", "e1")), Change::None);
        assert_eq!(change(None, &sp("a", "e1")), Change::None);
    }

    #[test]
    fn a_miss_keeps_the_last_address_and_a_removed_service_goes() {
        let ip = |d| Ipv4Addr::new(10, 43, 0, d);
        let n = |s: &str| s.to_string();
        let last = vec![(n("a"), ip(1)), (n("b"), ip(2)), (n("gone"), ip(9))];
        let fresh = vec![(n("a"), ip(5))];
        let got = merge(&[n("a"), n("b"), n("new")], &fresh, &last);
        assert_eq!(got, vec![(n("a"), ip(5)), (n("b"), ip(2))]);
    }

    #[test]
    fn only_the_hosts_file_wakes_the_daemon() {
        let ev = |p: &str| notify::Event::new(notify::EventKind::Any).add_path(PathBuf::from(p));
        assert!(is_hosts_event(&ev("/private/etc/hosts")));
        assert!(!is_hosts_event(&ev("/private/etc/hosts.kl-tmp")));
        assert!(!is_hosts_event(&ev("/private/etc/hosts.kl-connect.tmp")));
        assert!(!is_hosts_event(&ev("/private/etc/resolv.conf")));
    }

    #[test]
    fn timestamps_are_rfc3339_utc() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(1_760_000_000), "2025-10-09T08:53:20Z");
    }

    #[test]
    fn the_wish_file_names_the_team() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(read_team(d.path()), None);
        std::fs::write(d.path().join("env.json"), r#"{"team":"acme"}"#).unwrap();
        assert_eq!(read_team(d.path()).as_deref(), Some("acme"));
    }

    #[tokio::test]
    async fn an_idle_environment_is_started_and_awaited() {
        use axum::{extract::State, routing::{get, post}, Json, Router};
        use std::sync::{atomic::{AtomicBool, Ordering}, Arc};
        let started = Arc::new(AtomicBool::new(false));
        let app = Router::new()
            .route(
                "/v1/environments/e1",
                get(|State(s): State<Arc<AtomicBool>>| async move {
                    Json(if s.load(Ordering::SeqCst) {
                        serde_json::json!({"state": "running", "ready": true})
                    } else {
                        serde_json::json!({"state": "stopped", "idle_since": "2026-10-12T10:00:00Z"})
                    })
                }),
            )
            .route(
                "/v1/environments/e1/start",
                post(|State(s): State<Arc<AtomicBool>>| async move {
                    s.store(true, Ordering::SeqCst);
                    axum::http::StatusCode::ACCEPTED
                }),
            )
            .with_state(started.clone());
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let cfg = crate::config::Config { api: format!("http://{}", l.local_addr().unwrap()), token: "t".into(), expires_at: String::new(), username: "u".into() };
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        let doc = |idle: Option<&str>| api::EnvDoc { id: "e1".into(), name: "e".into(), services: vec![], idle_since: idle.map(Into::into) };
        wake_if_idle(&cfg, &doc(None)).await.is_ok().then_some(()).expect("wake");
        assert!(!started.load(Ordering::SeqCst), "a running environment is left alone");
        wake_if_idle(&cfg, &doc(Some("2026-10-12T10:00:00Z"))).await.is_ok().then_some(()).expect("wake");
        assert!(started.load(Ordering::SeqCst), "idle-stopped is started");
    }
}
