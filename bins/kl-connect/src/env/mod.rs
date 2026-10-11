//! `kl-connect env`: reach the connected environment from the laptop by service name.
//!
//! `connect`/`disconnect` only record the wish (the space choice on the api, plus `env.json`); the
//! root daemon (`daemon.rs`) does everything that needs privilege -- utun, route, `/etc/hosts` --
//! and reports through `env-status.json`. Design: docs/superpowers/specs/2026-10-11-env-tunnel-design.md.
mod daemon;
mod hosts;
mod launchd;
mod tunnel;

use crate::{api, config};
use clap::Subcommand;
use std::path::{Path, PathBuf};

#[derive(Subcommand)]
pub enum EnvCmd {
    /// Connect the laptop to an environment (the space's environment for that team).
    Connect {
        env: String,
        #[arg(long)]
        team: Option<String>,
    },
    /// Disconnect: clear the space's environment and remove the hosts block.
    Disconnect,
    /// What the daemon is doing: environment, tunnel state, names in the hosts block.
    Status,
    /// Install or remove the background daemon (needs sudo).
    Daemon {
        #[command(subcommand)]
        cmd: Option<DaemonCmd>,
        /// Whose login to serve; set by the LaunchDaemon plist.
        #[arg(long, hide = true)]
        home: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
pub enum DaemonCmd {
    /// Install the root LaunchDaemon: `sudo kl-connect env daemon install`
    Install,
    /// Stop and remove the LaunchDaemon and the hosts block
    Uninstall,
}

pub async fn run(cmd: &EnvCmd, top_team: Option<&str>) -> Result<(), String> {
    match cmd {
        EnvCmd::Connect { env, team } => connect(env, team.as_deref().or(top_team)).await,
        EnvCmd::Disconnect => disconnect().await,
        EnvCmd::Status => status(),
        EnvCmd::Daemon { cmd, home } => {
            if unsafe { libc::geteuid() } != 0 {
                return Err("the daemon runs as root: sudo kl-connect env daemon install".into());
            }
            match cmd {
                Some(DaemonCmd::Install) => launchd::install(),
                Some(DaemonCmd::Uninstall) => launchd::uninstall(),
                None => daemon::run(home.clone().ok_or("--home is required")?).await,
            }
        }
    }
}

/// Written by the root daemon, outside the person's home: root never writes where the user can
/// plant a symlink. World-readable; it holds environment and service names only.
pub const STATUS: &str = "/var/run/kl-connect/env-status.json";

fn env_file() -> PathBuf {
    config::dir().join("env.json")
}

async fn connect(env: &str, team: Option<&str>) -> Result<(), String> {
    if !Path::new(launchd::PLIST).exists() {
        return Err("the daemon is not installed: run `sudo kl-connect env daemon install` first".into());
    }
    // The daemon reads env.json from the plain home; a redirected dir would be a wish nobody sees.
    let home = std::env::var("HOME").map_err(|e| format!("HOME: {e}"))?;
    let (dir_in, dir) = (config::dir_in(Path::new(&home)), config::dir());
    if dir_in != dir {
        return Err(format!(
            "the daemon reads {}; KL_CONFIG_DIR/XDG_CONFIG_HOME point kl-connect at {} — unset them for env connect",
            dir_in.display(),
            dir.display()
        ));
    }
    let cfg = config::load()?;
    let team = team.map(str::to_string).unwrap_or_else(|| cfg.username.clone());
    let envs = api::environments(&cfg, &team).await.map_err(|e| e.to_string())?;
    let id = pick(&envs, env, &team)?;
    api::set_space(&cfg, &team, &id).await.map_err(|e| e.to_string())?;
    let p = env_file();
    config::write_atomic(&p, &serde_json::json!({ "team": team }).to_string())?; // 0600 at create
    println!("connected {env} (team {team}); names appear in a few seconds");
    Ok(())
}

/// People type the name they see (`blog`); the space stores the id. An id always wins, then a
/// unique name; names are not unique within a team, so two matches is a refusal naming both ids.
fn pick(envs: &[api::EnvDoc], arg: &str, team: &str) -> Result<String, String> {
    if let Some(e) = envs.iter().find(|e| e.id == arg) {
        return Ok(e.id.clone());
    }
    match envs.iter().filter(|e| e.name == arg).collect::<Vec<_>>()[..] {
        [e] => Ok(e.id.clone()),
        [] => Err(format!(
            "no environment {arg} in team {team}; have: {}",
            envs.iter().map(|e| format!("{} ({})", e.name, e.id)).collect::<Vec<_>>().join(", ")
        )),
        ref many => Err(format!(
            "{} environments are named {arg} in team {team}; pass the id: {}",
            many.len(),
            many.iter().map(|e| e.id.as_str()).collect::<Vec<_>>().join(", ")
        )),
    }
}

async fn disconnect() -> Result<(), String> {
    let p = env_file();
    let s = std::fs::read_to_string(&p).map_err(|_| "not connected".to_string())?;
    let team = serde_json::from_str::<serde_json::Value>(&s)
        .ok()
        .and_then(|v| v.get("team").and_then(|t| t.as_str().map(str::to_string)))
        .ok_or_else(|| format!("{}: no team", p.display()))?;
    // Logged out or token expired: the wish file still goes, so the daemon tears down on its next beat.
    let cleared = match config::load() {
        Ok(cfg) => match api::clear_space(&cfg, &team).await {
            Ok(()) => true,
            Err(api::Error::Unauthorized) => false,
            Err(e) => return Err(e.to_string()),
        },
        Err(_) => false,
    };
    std::fs::remove_file(&p).map_err(|e| e.to_string())?;
    if cleared {
        println!("disconnected (team {team})");
    } else {
        println!("disconnected locally (team {team}); the space still names the environment — run kl-connect login and disconnect again to clear it");
    }
    Ok(())
}

fn status() -> Result<(), String> {
    let p = PathBuf::from(STATUS);
    let s = std::fs::read_to_string(&p).map_err(|_| "no status yet: is the daemon installed and a connection made?".to_string())?;
    let v: serde_json::Value = serde_json::from_str(&s).map_err(|e| format!("{}: {e}", p.display()))?;
    let text = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let list = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", "))
            .unwrap_or_default()
    };
    println!("environment: {} {}", text("environment"), text("name"));
    println!("state: {}", text("state"));
    println!("names: {}", list("names"));
    println!("skipped: {}", list("skipped"));
    println!("error: {}", text("error"));
    println!("at: {}", text("at"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::pick;

    fn envs(v: &[(&str, &str)]) -> Vec<crate::api::EnvDoc> {
        v.iter().map(|(id, name)| serde_json::from_value(serde_json::json!({ "id": id, "name": name })).unwrap()).collect()
    }

    #[test]
    fn picks_by_id_then_unique_name_and_refuses_the_rest() {
        let e = envs(&[("env-1", "blog"), ("env-2", "shop"), ("env-3", "shop"), ("blog", "x")]);
        assert_eq!(pick(&e, "env-1", "t").unwrap(), "env-1");
        assert_eq!(pick(&e, "blog", "t").unwrap(), "blog", "an id beats a name");
        assert_eq!(pick(&envs(&[("env-1", "blog")]), "blog", "t").unwrap(), "env-1");
        assert!(pick(&e, "shop", "t").unwrap_err().contains("env-2, env-3"));
        assert!(pick(&e, "nope", "t").unwrap_err().contains("blog (env-1)"));
    }
}
