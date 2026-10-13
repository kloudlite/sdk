//! `kl-connect mode gate|prompt` — kl mode for the laptop `kloudlite` Claude Code plugin
//! (`harness/laptop-plugin/plugin/hooks/hooks.json`). While the mode is on, every tool that runs,
//! reads or writes on this laptop is denied, in the main session and in its subagents; everything
//! else (the kl tools, coordination, Artifact, the web) passes. The mode is a flag file
//! `~/.claude/kl-mode/<key>`; `/kloudlite:on` and `/kloudlite:off` set and clear it from the
//! `UserPromptSubmit` hook, before the command's own body runs.
//!
//! A deny list since 2026-10-13: the allow list it replaced also refused Artifact and every other
//! tool that touches nothing local, and the person wanted only the laptop fenced. A local tool
//! Claude Code adds later passes until named here.
//!
//! One Bash command passes anyway: `kl-connect ws follow <ws> <id>` in the background
//! (`is_follow`), which only prints a workspace process's output, so a server running in the
//! workspace shows up as a Claude Code background shell.
//!
//! The read-only kl tools are also allowed outright, mode on or off: without an answer from this
//! hook, auto mode's classifier judges every kl call and refused `workspace_list` (2026-10-11).
//! While the mode is on, EVERY kl tool is allowed too: turning it on is the person's choice to work
//! through kl, so the classifier never stands between them and a kl call. With the mode off,
//! anything that changes a workspace, an environment or a volume still goes to the classifier.
//! Spec: docs/superpowers/specs/2026-10-11-laptop-kl-plugin-design.md.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

const KL_PREFIX: &str = "mcp__plugin_kloudlite_kl__";
/// Claude Code's tools that run, read or write on this laptop.
const LOCAL: &[&str] = &[
    "Bash", "BashOutput", "KillShell", "Monitor", "PowerShell", "Read", "Write", "Edit", "MultiEdit", "NotebookEdit",
    "NotebookRead", "Glob", "Grep", "LS", "LSP",
];
/// MCP servers whose tools act on this laptop's files.
const LOCAL_MCP: &[&str] = &["mcp__serena__"];
/// kl tools that only read: lists, docs, history, files and the code graph. `graft_build` writes
/// the graph cache and `web_*` reach outside, so they stay with the classifier.
const READ_ONLY: &[&str] = &[
    "workspace_list", "workspace_get", "env_list", "env_get", "volume_list", "volume_history", "volume_refs",
    "requests_list", "request_get", "builder_status", "quota", "regions", "space_env_current", "packages_list",
    "read", "glob", "grep", "process_list", "process_output", "watch_poll", "graft_find_code", "graft_find_all",
    "graft_trace_calls", "graft_file_api", "graft_repo_map", "graft_blast",
];
const ALLOW_REASON: &str = "kl: read-only tool";
const ON_ALLOW_REASON: &str = "kl mode: kl tool";
pub const DENY_REASON: &str = "kl mode: use the kl tools; nothing runs on this laptop";
const FOLLOW_REASON: &str = "kl: follow a workspace process";
const ON_CONTEXT: &str = "kl mode is on: work only through the kl tools; nothing runs on this laptop.";

fn local(tool: &str) -> bool {
    LOCAL.contains(&tool) || LOCAL_MCP.iter().any(|p| tool.starts_with(p))
}

/// `kl-connect ws follow <ws> <id> [--team <t>]`, backgrounded. Every word is held to a plain
/// charset, so nothing rides along (`;`, `&&`, `$(…)`, a redirect, a newline).
fn is_follow(input: &Value) -> bool {
    let i = input.get("tool_input");
    let bg = i.and_then(|i| i.get("run_in_background")).and_then(Value::as_bool) == Some(true);
    let cmd = i.and_then(|i| i.get("command")).and_then(Value::as_str).unwrap_or("");
    let plain = |w: &&str| !w.is_empty() && w.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b));
    let words: Vec<&str> = cmd.split(' ').collect();
    bg && matches!(words.as_slice(), ["kl-connect", "ws", "follow", rest @ ..]
        if (rest.len() == 2 || (rest.len() == 4 && rest[2] == "--team")) && rest.iter().all(plain))
}

// KEY = session_id (Task 1 found a subagent's hook input carries its parent's session_id).
fn key(input: &Value) -> Option<String> {
    let s = input.get("session_id")?.as_str()?;
    // the key becomes a file name: nothing that can climb out of the flag folder
    (!s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')).then(|| s.to_string())
}

fn flag(dir: &Path, input: &Value) -> Option<PathBuf> {
    key(input).map(|k| dir.join(k))
}

/// The `PreToolUse` answer: allow a read-only kl tool and a follow, and every kl tool while the
/// mode is on; deny a local tool while the mode is on; otherwise nothing, and Claude Code's own
/// permission mode decides.
pub fn gate(dir: &Path, input: &Value) -> Option<String> {
    let on = flag(dir, input).is_some_and(|f| f.exists());
    let tool = input.get("tool_name").and_then(Value::as_str).unwrap_or("");
    let answer = |decision: &str, reason: &str| {
        json!({ "hookSpecificOutput": {
            "hookEventName": "PreToolUse", "permissionDecision": decision, "permissionDecisionReason": reason,
        } })
        .to_string()
    };
    if tool.strip_prefix(KL_PREFIX).is_some_and(|t| READ_ONLY.contains(&t)) {
        return Some(answer("allow", ALLOW_REASON));
    }
    if tool == "Bash" && is_follow(input) {
        return Some(answer("allow", FOLLOW_REASON));
    }
    if on && tool.starts_with(KL_PREFIX) {
        return Some(answer("allow", ON_ALLOW_REASON));
    }
    (on && local(tool)).then(|| answer("deny", DENY_REASON))
}

/// The `UserPromptSubmit` answer: switch the mode on `/kloudlite:on|off`, then add the context
/// line while it is on.
pub fn prompt(dir: &Path, input: &Value) -> Result<Option<String>, String> {
    let Some(f) = flag(dir, input) else { return Ok(None) };
    let typed = input.get("prompt").and_then(Value::as_str).unwrap_or("").trim();
    // the command itself, never a prompt that only mentions it
    let is = |cmd: &str| typed == cmd || typed.starts_with(&format!("{cmd} "));
    if is("/kloudlite:on") {
        std::fs::create_dir_all(dir).map_err(|e| format!("kl mode: {e}"))?;
        std::fs::write(&f, b"").map_err(|e| format!("kl mode: {e}"))?;
    } else if is("/kloudlite:off") {
        let _ = std::fs::remove_file(&f);
    }
    Ok(f.exists().then(|| {
        json!({ "hookSpecificOutput": { "hookEventName": "UserPromptSubmit", "additionalContext": ON_CONTEXT } })
            .to_string()
    }))
}

/// `kl-connect mode gate|prompt`: one hook JSON on stdin, zero or one line on stdout.
pub fn run(gate_not_prompt: bool) -> Result<(), String> {
    let mut raw = String::new();
    std::io::stdin().read_to_string(&mut raw).map_err(|e| format!("kl mode: {e}"))?;
    let input: Value = serde_json::from_str(&raw).map_err(|e| format!("kl mode: bad hook input: {e}"))?;
    let dir = dirs::home_dir().ok_or("kl mode: no home folder")?.join(".claude/kl-mode");
    let out = if gate_not_prompt { gate(&dir, &input) } else { prompt(&dir, &input)? };
    if let Some(line) = out {
        println!("{line}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("kl-mode-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }
    fn input(tool: &str, prompt: &str) -> Value {
        json!({ "session_id": "abc-123", "cwd": "/Users/x/proj", "tool_name": tool, "prompt": prompt })
    }

    #[test]
    fn off_allows_everything() {
        let d = tmp("off");
        assert_eq!(gate(&d, &input("Bash", "")), None);
        assert_eq!(gate(&d, &input("Write", "")), None);
    }

    #[test]
    fn on_denies_local_tools_only() {
        let d = tmp("on");
        assert!(prompt(&d, &input("", "/kloudlite:on")).unwrap().unwrap().contains(ON_CONTEXT));
        for t in ["exec", "env_delete", "workspace_stop"] {
            assert!(gate(&d, &input(&format!("{KL_PREFIX}{t}"), "")).unwrap().contains(ON_ALLOW_REASON), "{t}");
        }
        for t in ["Agent", "Skill", "SendMessage", "TodoWrite", "Artifact", "WebFetch", "WebSearch", "mcp__kl__read"] {
            assert_eq!(gate(&d, &input(t, "")), None, "{t}");
        }
        for t in ["Bash", "Read", "Write", "Edit", "Glob", "Grep", "NotebookEdit", "Monitor", "LSP", "mcp__serena__find_symbol"] {
            assert!(gate(&d, &input(t, "")).unwrap().contains(DENY_REASON), "{t}");
        }
    }

    #[test]
    fn read_only_kl_tools_are_allowed_mode_on_or_off() {
        let d = tmp("ro");
        for on in [false, true] {
            if on {
                prompt(&d, &input("", "/kloudlite:on")).unwrap();
            }
            for t in ["workspace_list", "env_get", "read", "graft_repo_map"] {
                assert!(gate(&d, &input(&format!("{KL_PREFIX}{t}"), "")).unwrap().contains(ALLOW_REASON), "{t} on={on}");
            }
            for t in ["exec", "write", "env_delete", "workspace_stop", "graft_build", "web_fetch"] {
                let a = gate(&d, &input(&format!("{KL_PREFIX}{t}"), ""));
                // off: the classifier decides; on: the mode allows it
                assert_eq!(a.is_some_and(|a| a.contains(ON_ALLOW_REASON)), on, "{t} on={on}");
            }
            assert_eq!(gate(&d, &input("mcp__kl__workspace_list", "")), None); // not our plugin's server, and not local
        }
    }

    fn bash(cmd: &str, bg: bool) -> Value {
        json!({ "session_id": "abc-123", "tool_name": "Bash", "tool_input": { "command": cmd, "run_in_background": bg } })
    }

    #[test]
    fn only_a_backgrounded_follow_passes_as_bash() {
        let d = tmp("follow");
        prompt(&d, &input("", "/kloudlite:on")).unwrap();
        for ok in ["kl-connect ws follow syncserver p-18de02ad09685f92", "kl-connect ws follow api p-1 --team acme-co"] {
            assert!(gate(&d, &bash(ok, true)).unwrap().contains(FOLLOW_REASON), "{ok}");
        }
        assert!(gate(&d, &bash("kl-connect ws follow api p-1", false)).unwrap().contains(DENY_REASON));
        for bad in [
            "kl-connect ws follow api p-1; rm -rf ~", "kl-connect ws follow api p-1 && ls", "kl-connect ws follow api $(id)",
            "kl-connect ws follow api p-1 > /tmp/x", "kl-connect ws follow api p-1\nls", "kl-connect ws follow  api p-1",
            "kl-connect ws follow api", "kl-connect ws follow api p-1 --tea x", "kl-connect ws ssh api", "/tmp/kl-connect ws follow a b",
        ] {
            assert!(gate(&d, &bash(bad, true)).unwrap().contains(DENY_REASON), "{bad}");
        }
    }

    #[test]
    fn off_command_clears() {
        let d = tmp("clear");
        prompt(&d, &input("", "/kloudlite:on")).unwrap();
        assert_eq!(prompt(&d, &input("", "/kloudlite:off")).unwrap(), None);
        assert_eq!(gate(&d, &input("Bash", "")), None);
    }

    #[test]
    fn prompt_mentions_do_not_toggle() {
        let d = tmp("mention");
        assert_eq!(prompt(&d, &input("", "what does /kloudlite:on do?")).unwrap(), None);
        assert_eq!(prompt(&d, &input("", "/kloudlite:onward")).unwrap(), None);
        assert_eq!(gate(&d, &input("Bash", "")), None);
    }

    #[test]
    fn key_is_safe() {
        let d = tmp("key");
        for bad in [json!({ "session_id": "../x", "cwd": "", "prompt": "/kloudlite:on" }), json!({ "session_id": "", "cwd": "", "prompt": "/kloudlite:on" })] {
            assert_eq!(prompt(&d, &bad).unwrap(), None);
        }
        assert!(!d.parent().unwrap().join("x").exists());
    }
}
