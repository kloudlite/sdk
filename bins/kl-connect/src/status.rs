//! `kl-connect mode status` — the Claude Code `statusLine` for kl mode: the workspaces and the
//! environments, each with its state, under the prompt. Silent while kl mode is off, so the same
//! status line costs nothing in an ordinary session.
//!
//! Claude Code runs the command on every update, so the answer is cached for `TTL` in the config
//! dir and each api call is bounded by `CALL`: a slow or dead api shows the last answer marked
//! stale, never a frozen prompt.

use std::io::Read;
use std::time::{Duration, SystemTime};

use serde_json::Value;

use crate::{api, config, mode};

const TTL: Duration = Duration::from_secs(10);
const CALL: Duration = Duration::from_secs(3);

#[derive(serde::Deserialize)]
pub struct Env {
    pub name: String,
    pub state: String,
    #[serde(default)]
    pub services: Vec<Value>,
}

fn dot(state: &str) -> &'static str {
    match state {
        "ready" | "running" => "\x1b[32m●\x1b[0m",
        "stopped" => "\x1b[2m○\x1b[0m",
        "error" => "\x1b[31m✕\x1b[0m",
        _ => "\x1b[33m◐\x1b[0m", // creating, starting, deleting
    }
}

pub fn render(ws: &[(String, String)], envs: &[(String, String, usize)]) -> String {
    let w = ws.iter().map(|(n, s)| format!("{n} {} {s}", dot(s))).collect::<Vec<_>>();
    let e = envs.iter().map(|(n, s, k)| format!("{n} {} {s} ({k} svc)", dot(s))).collect::<Vec<_>>();
    let or_none = |v: Vec<String>| if v.is_empty() { "none".to_string() } else { v.join("  ") };
    format!("kl ON │ ws:  {}\n      │ env: {}", or_none(w), or_none(e))
}

async fn fetch() -> Result<String, String> {
    let cfg = config::load()?;
    let team = std::env::var("KL_TEAM").ok().filter(|t| !t.is_empty());
    let (ws, envs) = tokio::join!(
        tokio::time::timeout(CALL, api::list(&cfg, team.as_deref())),
        tokio::time::timeout(CALL, api::list_envs(&cfg, team.as_deref())),
    );
    let ws = ws.map_err(|_| "api timed out")?.map_err(|e| e.to_string())?;
    let envs = envs.map_err(|_| "api timed out")?.map_err(|e| e.to_string())?;
    Ok(render(
        &ws.into_iter().map(|w| (w.name, w.state)).collect::<Vec<_>>(),
        &envs.into_iter().map(|e| (e.name, e.state, e.services.len())).collect::<Vec<_>>(),
    ))
}

pub async fn run() -> Result<(), String> {
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw).map_err(|e| e.to_string())?;
    let input: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
    if !mode::is_on(&input) {
        return Ok(());
    }
    let cache = config::dir().join("status.txt");
    let age = std::fs::metadata(&cache).and_then(|m| m.modified()).ok().and_then(|t| SystemTime::now().duration_since(t).ok());
    let cached = std::fs::read_to_string(&cache).ok();
    if let (Some(a), Some(c)) = (age, &cached) {
        if a < TTL {
            print!("{c}");
            return Ok(());
        }
    }
    match fetch().await {
        Ok(s) => {
            let _ = config::write_atomic(&cache, &s);
            print!("{s}");
        }
        Err(e) => match cached {
            Some(c) => print!("{c}  \x1b[2m(stale: {e})\x1b[0m"),
            None => print!("kl ON │ {e}"),
        },
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_both_rows() {
        let s = render(
            &[("chat".into(), "ready".into()), ("api".into(), "stopped".into())],
            &[("dev".into(), "running".into(), 3)],
        );
        assert!(s.contains("chat \x1b[32m●\x1b[0m ready"), "{s}");
        assert!(s.contains("api \x1b[2m○\x1b[0m stopped"), "{s}");
        assert!(s.contains("dev \x1b[32m●\x1b[0m running (3 svc)"), "{s}");
        assert_eq!(render(&[], &[]), "kl ON │ ws:  none\n      │ env: none");
    }
}
