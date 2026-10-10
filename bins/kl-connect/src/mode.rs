//! `kl-connect mode gate|prompt` — kl mode for the laptop `kloudlite` Claude Code plugin
//! (`harness/laptop-plugin/plugin/hooks/hooks.json`). While the mode is on, every tool call
//! except the kl tools and Claude Code's own coordination tools is denied, in the main session
//! and in its subagents: nothing reads, writes or runs on this laptop. The mode is a flag file
//! `~/.claude/kl-mode/<key>`; `/kloudlite:on` and `/kloudlite:off` set and clear it from the
//! `UserPromptSubmit` hook, before the command's own body runs.
//!
//! An allow list, not a deny list: a tool Claude Code adds later is denied until named here.
//! Spec: docs/superpowers/specs/2026-10-11-laptop-kl-plugin-design.md.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

const KL_PREFIX: &str = "mcp__plugin_kloudlite_kl__";
/// Claude Code's coordination tools: they move work between agents, they touch no file.
const ALLOWED: &[&str] = &[
    "Agent", "Task", "Skill", "ToolSearch", "AskUserQuestion", "SendMessage", "TaskCreate", "TaskUpdate", "TaskList",
    "TaskGet", "TodoWrite",
];
pub const DENY_REASON: &str = "kl mode: use the kl tools; nothing runs on this laptop";
const ON_CONTEXT: &str = "kl mode is on: work only through the kl tools; nothing runs on this laptop.";

pub fn allowed(tool: &str) -> bool {
    tool.starts_with(KL_PREFIX) || ALLOWED.contains(&tool)
}

// KEY = session_id (Task 1 found a subagent's hook input carries its parent's session_id).
fn key(input: &Value) -> Option<String> {
    let s = input.get("session_id")?.as_str()?;
    // the key becomes a file name: nothing that can climb out of the flag folder
    (!s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')).then(|| s.to_string())
}

/// Whether kl mode is on for the session this hook or status-line input names.
pub fn is_on(input: &Value) -> bool {
    dirs::home_dir().and_then(|h| flag(&h.join(".claude/kl-mode"), input)).is_some_and(|f| f.exists())
}

fn flag(dir: &Path, input: &Value) -> Option<PathBuf> {
    key(input).map(|k| dir.join(k))
}

/// The `PreToolUse` answer: a deny line while the mode is on and the tool is not allowed.
pub fn gate(dir: &Path, input: &Value) -> Option<String> {
    let on = flag(dir, input).is_some_and(|f| f.exists());
    let tool = input.get("tool_name").and_then(Value::as_str).unwrap_or("");
    (on && !allowed(tool)).then(|| {
        json!({ "hookSpecificOutput": {
            "hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": DENY_REASON,
        } })
        .to_string()
    })
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
    fn on_allows_kl_and_coordination_only() {
        let d = tmp("on");
        assert!(prompt(&d, &input("", "/kloudlite:on")).unwrap().unwrap().contains(ON_CONTEXT));
        for t in ["mcp__plugin_kloudlite_kl__read", "mcp__plugin_kloudlite_kl__exec", "Agent", "Skill", "SendMessage", "TodoWrite"] {
            assert_eq!(gate(&d, &input(t, "")), None, "{t}");
        }
        for t in ["Bash", "Read", "Write", "Edit", "Glob", "Grep", "WebFetch", "WebSearch", "NotebookEdit", "mcp__serena__find_symbol", "mcp__kl__read"] {
            assert!(gate(&d, &input(t, "")).unwrap().contains(DENY_REASON), "{t}");
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
