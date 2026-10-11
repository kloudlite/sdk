//! The root daemon's loop (spec section 2): every 10 s read what the person chose, make the tunnel
//! and `/etc/hosts` match it, and say what happened in `env-status.json`. No socket: the CLI and the
//! daemon share `env.json` (the wish) and `env-status.json` (the report) under the person's config
//! dir, because the daemon runs as root with root's `HOME` and must be told whose home to serve.
//!
//! Which failures keep the hosts block and which remove it is the spec's error table: transient
//! trouble (api, gateway, bench waking or restarted, laptop offline) keeps the last good names so a
//! running app keeps resolving; access lost, environment or space gone, or `disconnect` removes
//! them, because the names would now point at nothing or at someone else's addresses.
use super::{hosts, tunnel};
use crate::api::{self, Error, Space};
use boringtun::x25519::StaticSecret;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

const HOSTS: &str = "/etc/hosts";
const BEAT: Duration = Duration::from_secs(10);
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
    backoff: Duration,
}

pub async fn run(home: PathBuf) -> Result<(), String> {
    let mut d = Daemon {
        home,
        secret: StaticSecret::random_from_rng(rand_core::OsRng),
        current: None,
        backoff: Duration::from_secs(1),
    };
    loop {
        let delay = match d.pass().await {
            Ok(()) => {
                d.backoff = Duration::from_secs(1);
                BEAT
            }
            Err(e) => {
                eprintln!("kl-connect env: {e}");
                let b = d.backoff;
                d.backoff = (b * 2).min(BACKOFF_CAP);
                b
            }
        };
        tokio::time::sleep(delay).await;
    }
}

impl Daemon {
    fn dir(&self) -> PathBuf {
        crate::config::dir_in(&self.home)
    }

    fn status(&self, st: Status) {
        let st = Status { at: now_rfc3339(), ..st };
        let p = self.dir().join("env-status.json");
        // 0600 from write_atomic; chowned so the person's `env status` (not root) can read it.
        if crate::config::write_atomic(&p, &serde_json::to_string_pretty(&st).unwrap()).is_ok() {
            if let Ok(m) = std::fs::metadata(&self.home) {
                let _ = std::os::unix::fs::chown(&p, Some(m.uid()), None);
            }
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
            Err(e) => return fail(self, e, ""),
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
            Err(e) => return fail(self, e.to_string(), ""),
        };
        let Some(space) = space else {
            self.tear_down(true).await;
            self.status(Status { state: "no environment".into(), error: format!("team {team} has no environment"), ..Default::default() });
            return Ok(());
        };

        match &self.current {
            Some((s, _)) if *s != space => self.tear_down(true).await,
            // The stream ended (bench restarted, laptop slept): reconnect, last names stay meanwhile.
            Some((_, up)) if up.is_finished() => self.tear_down(false).await,
            _ => {}
        }
        if self.current.is_none() {
            match tunnel::bring_up(&cfg, &team, &self.secret).await {
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
            Err(e) => return fail(self, e.to_string(), &space.environment),
        };
        let names: Vec<String> = doc.services.iter().map(|s| s.name.clone()).collect();
        let id = doc.id.clone();
        let entries = tokio::task::spawn_blocking(move || tunnel::resolve(&names, &id)).await.map_err(|e| e.to_string())?;
        let block = hosts::Block { env: doc.id.clone(), name: doc.name.clone(), entries };
        let cur = std::fs::read_to_string(HOSTS).map_err(|e| format!("read {HOSTS}: {e}"))?;
        let (text, skipped) = hosts::render(&cur, Some(&block));
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

    async fn tear_down(&mut self, remove_hosts: bool) {
        if let Some((_, up)) = self.current.take() {
            up.down().await;
        }
        if remove_hosts {
            if let Ok(cur) = std::fs::read_to_string(HOSTS) {
                let (t, _) = hosts::render(&cur, None);
                if t != cur && hosts::install(Path::new(HOSTS), &t).is_ok() {
                    hosts::flush();
                }
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
