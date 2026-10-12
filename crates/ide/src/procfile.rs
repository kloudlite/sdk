//! `procs.json`: the detached processes that were running, so an idle stop does not lose them
//! (idle-stop spec rule 6, R3). Only `running` processes are listed; an exit or `process_kill`
//! removes the entry, and the file is rewritten on every spawn and exit, so it is current BEFORE
//! a stop. Jobs and graft/watch spawns are never recorded.
//!
//! Relaunch keeps the id. `attempts` is incremented and saved BEFORE the spawn, and resets to 0
//! once the process has run `procs::RESET_ATTEMPTS_SECS`; an entry at 3 is listed as exited with
//! a note instead of being retried, so a crash-looping command cannot hold the workspace awake.
use crate::server::App;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct Entry {
    pub id: String,
    pub tree: String,
    pub args: serde_json::Value,
    pub started_at: String,
    pub attempts: u32,
}

pub fn save(path: &Path, entries: &[Entry]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("json.tmp");
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(&serde_json::to_vec(entries)?)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)
}

pub fn load(path: &Path) -> Vec<Entry> {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn cmd_line(args: &serde_json::Value) -> String {
    match args.get("cmd") {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(a)) => a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(" "),
        _ => String::new(),
    }
}

/// Boot, before serving: bring back every process that was running when the pod went away.
pub fn relaunch(app: &App) -> usize {
    let Some(path) = app.procs.file.get().cloned() else { return 0 };
    let mut entries = load(&path);
    let mut ok = 0;
    for i in 0..entries.len() {
        let e = entries[i].clone();
        if e.attempts >= 3 {
            app.procs.exited_note(&e.id, &e.tree, &cmd_line(&e.args), "not restarted: failed 3 restarts");
            tracing::info!(id = %e.id, attempts = e.attempts, ok = false, "procs.relaunch");
            continue;
        }
        entries[i].attempts += 1;
        // Counted BEFORE the spawn, so a crash at spawn still counts.
        if let Err(err) = save(&path, &entries) {
            tracing::warn!(error = %err, "procs.save_failed");
        }
        let attempts = entries[i].attempts;
        let started = app.tree(Some(&e.tree)).ok().and_then(|t| crate::tools::exec::command(&t, &e.args).ok()).and_then(|(c, line)| app.procs.spawn_restored(&e.id, &e.tree, c, line, e.args.clone(), attempts).ok());
        tracing::info!(id = %e.id, attempts, ok = started.is_some(), "procs.relaunch");
        ok += started.is_some() as usize;
    }
    app.procs.save();
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_load_round_trip_is_0600() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join(".kl/procs.json");
        let e = Entry { id: "p-1".into(), tree: "main".into(), args: serde_json::json!({"cmd": "sleep 1"}), started_at: "1".into(), attempts: 0 };
        save(&p, std::slice::from_ref(&e)).unwrap();
        assert_eq!(load(&p), vec![e]);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn load_of_missing_or_garbage_is_empty() {
        let d = tempfile::tempdir().unwrap();
        assert!(load(&d.path().join("nope")).is_empty());
        std::fs::write(d.path().join("bad"), "{").unwrap();
        assert!(load(&d.path().join("bad")).is_empty());
    }
}
