# Laptop kloudlite plugin Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Claude Code plugin `kloudlite` for the laptop: the bench's tools over `kl-connect mcp`, two agents (`kl-workspace`, `kl-subagent`), two skills (`kloudlite`, `orchestrate`), and a kl mode that denies every local tool while it is on.

**Architecture:** The plugin is static files under `harness/laptop-plugin/`. The kl mode is two Claude Code hooks (`PreToolUse`, `UserPromptSubmit`) that call a new hidden subcommand `kl-connect mode gate|prompt`; the mode flag is one file under `~/.claude/kl-mode/`. A bun test holds the plugin's tool names equal to `mcpTools`.

**Tech Stack:** Rust (kl-connect, clap, serde_json, dirs), Claude Code plugin format (plugin.json, marketplace.json, .mcp.json, hooks.json, agents/*.md, skills/*/SKILL.md, commands/*.md), bun test.

**Spec:** `docs/superpowers/specs/2026-10-11-laptop-kl-plugin-design.md`

## Global Constraints

- Worktree `/Volumes/kdisk/rustic-git-wt/names`, branch `names`. Every command below runs from there unless it says otherwise.
- cargo always with `CARGO_TARGET_DIR=/Volumes/kdisk/target-names`. Never run cargo in the dev pod.
- Commit subjects imperative sentence case, no tool attribution, no Claude reference.
- Plugin name `kloudlite`, MCP server name `kl`, so tool names are `mcp__plugin_kloudlite_kl__<tool>`.
- Deny reason, verbatim: `kl mode: use the kl tools; nothing runs on this laptop`
- Context line while on, verbatim: `kl mode is on: work only through the kl tools; nothing runs on this laptop.`
- Allowed while on: `mcp__plugin_kloudlite_kl__*`, `Agent`, `Task`, `Skill`, `ToolSearch`, `AskUserQuestion`, `SendMessage`, `TaskCreate`, `TaskUpdate`, `TaskList`, `TaskGet`, `TodoWrite`. Everything else denied.
- Mode off: the gate prints nothing and exits 0.
- Skills in ASD-STE100 Simplified Technical English, like `harness/packages/agent/claude-plugin/skills/*`.
- No permission cards, no `because` argument, no board tools for laptop roles.
- Do not change `harness/packages/agent/claude-plugin/` or any bench tool.

## Review Focus

1. **`kl-connect` missing from PATH while the mode is off** must not block local tools. The hook fallback denies only while a flag file exists (Task 3, step 3 test).
2. **A subagent's tool call** while main's mode is on must be denied for local tools. Covered by the Task 1 probe and the key choice; Task 5 checks it live.
3. **A prompt that only mentions `/kloudlite:on` mid-sentence** must not switch the mode. Task 2 test `prompt_mentions_do_not_toggle`.
4. **A hostile or odd `session_id`** (`../x`, empty) must not write outside `~/.claude/kl-mode/`. Task 2 test `key_is_safe`.
5. **`KL_TEAM` unset** must start `kl-connect mcp` with no team, not team `""`. Task 2 test `empty_team_is_none`.

---

### Task 1: Probe what the hooks see

Throwaway. Answers two questions the spec leaves open: does a subagent's `PreToolUse` input carry the parent's `session_id`, and what `prompt` text does `UserPromptSubmit` get for a plugin command.

**Files:**
- Create (scratch, deleted at the end): `$S/probe-plugin/` where `S=/private/tmp/claude-501/-Users-karthik-rustic-git/ca5119e6-5c89-409b-a058-b668d4457515/scratchpad`
- Modify: `docs/superpowers/plans/2026-10-11-laptop-kl-plugin.md` (record the result under "Probe result" below)

**Interfaces:**
- Produces: the ruling `KEY = session_id` or `KEY = cwd`, and the exact prompt text for `/kloudlite:on`, used by Task 2.

- [ ] **Step 1: Write the probe plugin**

```bash
P=$S/probe-plugin
mkdir -p $P/.claude-plugin $P/hooks $P/commands
echo '{"name":"kloudlite"}' > $P/.claude-plugin/plugin.json
echo 'Say "on".' > $P/commands/on.md
cat > $P/hooks/hooks.json <<'EOF'
{"hooks":{
 "PreToolUse":[{"matcher":".*","hooks":[{"type":"command","command":"cat >> \"$HOME/.claude/kl-probe.jsonl\"; echo >> \"$HOME/.claude/kl-probe.jsonl\""}]}],
 "UserPromptSubmit":[{"hooks":[{"type":"command","command":"cat >> \"$HOME/.claude/kl-probe.jsonl\"; echo >> \"$HOME/.claude/kl-probe.jsonl\""}]}]
}}
EOF
rm -f ~/.claude/kl-probe.jsonl
```

- [ ] **Step 2: Run a session that spawns a subagent**

```bash
cd $S && claude -p --plugin-dir $S/probe-plugin --permission-mode bypassPermissions \
  "Use the Agent tool with subagent_type general-purpose to run: list the files in the current folder with Glob, then say done." >/dev/null
cd $S && claude -p --plugin-dir $S/probe-plugin "/kloudlite:on" >/dev/null
```

- [ ] **Step 3: Read the result**

```bash
jq -c '{e:.hook_event_name, s:.session_id, a:.agent_id, t:.tool_name, p:.prompt, cwd:.cwd}' ~/.claude/kl-probe.jsonl
```

Expected: one `Agent` row from main, at least one `Glob` row with a non-null `agent_id`, and one `UserPromptSubmit` row with the prompt.

- [ ] **Step 4: Rule and record**

If the `Glob` row's `s` equals the main `Agent` row's `s`: `KEY = session_id`. Otherwise: `KEY = cwd`. Record the `p` value for the `/kloudlite:on` prompt. Write both under "Probe result" below.

- [ ] **Step 5: Clean up and commit**

```bash
rm -rf $S/probe-plugin ~/.claude/kl-probe.jsonl
git add docs/superpowers/plans/2026-10-11-laptop-kl-plugin.md
git commit -m "Record what the hooks see in subagents"
```

**Probe result:** KEY = session_id. `UserPromptSubmit` prompt for `/kloudlite:on` is the raw typed text `/kloudlite:on` (not the expanded body). A subagent's `PreToolUse` carries the parent's `session_id` with a non-null `agent_id` (main-thread rows have `agent_id` null); the subagent used Bash rather than Glob, same result.

---

### Task 2: `kl-connect mode gate|prompt`

**Files:**
- Create: `bins/kl-connect/src/mode.rs`
- Modify: `bins/kl-connect/src/main.rs` (add `mod mode;`, `Cmd::Mode`, `ModeCmd`, the dispatch arm, and the empty-team filter on `Cmd::Mcp`)

**Interfaces:**
- Consumes: Task 1's KEY ruling and prompt text.
- Produces: `kl-connect mode gate` and `kl-connect mode prompt`, each reading one hook JSON object on stdin and printing zero or one JSON line on stdout, exit 0. Task 3's hooks call them.

- [ ] **Step 1: Write `mode.rs` with its tests**

Use this file. `key()` has two bodies; keep the one Task 1 ruled and delete the other with its comment.

```rust
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

// KEY = cwd (Task 1 found a subagent's session_id differs from its parent's): subagents share the
// parent's working folder. ponytail: two laptop sessions in one folder share one mode; key by
// session once Claude Code passes the parent's id to subagent hooks.
fn key(input: &Value) -> Option<String> {
    use std::hash::{Hash, Hasher};
    let cwd = input.get("cwd")?.as_str().filter(|c| !c.is_empty())?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    cwd.hash(&mut h);
    Some(format!("{:016x}", h.finish()))
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
```

If Task 1 ruled `KEY = cwd`, `key_is_safe` still holds: both bad inputs have an empty `cwd`, and the first gives no `..` path because the key is a hash.

- [ ] **Step 2: Wire it into `main.rs`**

Add `mod mode;` beside the other `mod` lines. Add to `enum Cmd` after `BenchProxy`:

```rust
    /// kl mode hooks for the laptop `kloudlite` Claude Code plugin: one hook JSON on stdin
    #[command(hide = true)]
    Mode {
        #[command(subcommand)]
        cmd: ModeCmd,
    },
```

Add the enum beside `BuilderCmd`:

```rust
#[derive(Subcommand)]
enum ModeCmd {
    /// PreToolUse: deny a local tool while kl mode is on
    Gate,
    /// UserPromptSubmit: switch kl mode on `/kloudlite:on|off`
    Prompt,
}
```

Add the dispatch arm beside `Cmd::Mcp`:

```rust
        Cmd::Mode { cmd } => mode::run(matches!(cmd, ModeCmd::Gate)),
```

Change the `Cmd::Mcp` arm so an unset `${KL_TEAM:-}` (an empty argument) means no team:

```rust
        Cmd::Mcp { team } => mcp::mcp(team.as_deref().filter(|t| !t.is_empty())).await,
```

And add to `main.rs`'s tests (create `#[cfg(test)] mod tests { use super::*; use clap::Parser; ... }` if `main.rs` has none; `Cli` is the clap root struct's name in `main.rs`, check it):

```rust
    #[test]
    fn empty_team_is_none() {
        let cli = Cli::try_parse_from(["kl-connect", "mcp", ""]).unwrap();
        let Cmd::Mcp { team } = cli.cmd else { panic!() };
        assert_eq!(team.as_deref().filter(|t| !t.is_empty()), None);
    }
```

Match the field name (`cmd` or `command`) to the root struct.

- [ ] **Step 3: Run the tests**

Run: `CARGO_TARGET_DIR=/Volumes/kdisk/target-names cargo test -p kl-connect 2>&1 | tail -20`
Expected: all pass, including the five `mode::tests` and `empty_team_is_none`.

- [ ] **Step 4: Clippy**

Run: `CARGO_TARGET_DIR=/Volumes/kdisk/target-names cargo clippy -p kl-connect --all-targets -- -D warnings 2>&1 | tail -20`
Expected: no warnings.

- [ ] **Step 5: Smoke the binary**

```bash
B=/Volumes/kdisk/target-names/debug/kl-connect
echo '{"session_id":"t1","cwd":"/tmp/p","tool_name":"Bash"}' | HOME=$S/h $B mode gate; echo "exit $?"
echo '{"session_id":"t1","cwd":"/tmp/p","prompt":"/kloudlite:on"}' | HOME=$S/h $B mode prompt
echo '{"session_id":"t1","cwd":"/tmp/p","tool_name":"Bash"}' | HOME=$S/h $B mode gate
rm -rf $S/h
```

Expected: first prints nothing and `exit 0`; second prints the context line JSON; third prints the deny JSON.

- [ ] **Step 6: Commit**

```bash
git add bins/kl-connect/src/mode.rs bins/kl-connect/src/main.rs
git commit -m "Gate local tools behind kl mode in kl-connect"
```

---

### Task 3: Plugin skeleton: marketplace, MCP server, hooks, commands

**Files:**
- Create: `harness/laptop-plugin/.claude-plugin/marketplace.json`
- Create: `harness/laptop-plugin/plugin/.claude-plugin/plugin.json`
- Create: `harness/laptop-plugin/plugin/.mcp.json`
- Create: `harness/laptop-plugin/plugin/hooks/hooks.json`
- Create: `harness/laptop-plugin/plugin/commands/on.md`
- Create: `harness/laptop-plugin/plugin/commands/off.md`

**Interfaces:**
- Consumes: `kl-connect mode gate|prompt` (Task 2), `kl-connect mcp [team]`.
- Produces: plugin `kloudlite`, server `kl`, commands `/kloudlite:on`, `/kloudlite:off`. Tasks 4 and 5 add to this tree.

- [ ] **Step 1: Write the files**

`harness/laptop-plugin/.claude-plugin/marketplace.json`:

```json
{
  "name": "kloudlite",
  "owner": { "name": "Kloudlite" },
  "plugins": [
    { "name": "kloudlite", "source": "./plugin", "description": "Work on Kloudlite workspaces from Claude Code, through the bench's tools only" }
  ]
}
```

`harness/laptop-plugin/plugin/.claude-plugin/plugin.json`:

```json
{ "name": "kloudlite", "description": "Kloudlite workspaces through kl-connect mcp, with kl mode: no local tools" }
```

`harness/laptop-plugin/plugin/.mcp.json`:

```json
{ "mcpServers": { "kl": { "command": "kl-connect", "args": ["mcp", "${KL_TEAM:-}"] } } }
```

`harness/laptop-plugin/plugin/hooks/hooks.json` (the gate's fallback denies only while some flag exists, so a missing `kl-connect` never blocks a laptop whose mode is off):

```json
{
  "hooks": {
    "PreToolUse": [
      { "matcher": ".*", "hooks": [ { "type": "command", "command": "kl-connect mode gate || { ls \"$HOME\"/.claude/kl-mode/* >/dev/null 2>&1 && echo '{\"hookSpecificOutput\":{\"hookEventName\":\"PreToolUse\",\"permissionDecision\":\"deny\",\"permissionDecisionReason\":\"kl mode: use the kl tools; nothing runs on this laptop\"}}'; true; }" } ] }
    ],
    "UserPromptSubmit": [
      { "hooks": [ { "type": "command", "command": "kl-connect mode prompt" } ] }
    ]
  }
}
```

`harness/laptop-plugin/plugin/commands/on.md`:

```markdown
---
description: Turn kl mode on. Only the kl tools work; nothing runs on this laptop.
---
kl mode is on. From now on, do all work through the `mcp__plugin_kloudlite_kl__*` tools. Local tools are refused. Load the `kloudlite:kloudlite` and `kloudlite:orchestrate` skills now, then tell the person in one line that kl mode is on.
```

`harness/laptop-plugin/plugin/commands/off.md`:

```markdown
---
description: Turn kl mode off. Local tools work again.
---
kl mode is off. Local tools work again. Tell the person in one line.
```

- [ ] **Step 2: Validate the JSON**

Run: `for f in harness/laptop-plugin/.claude-plugin/marketplace.json harness/laptop-plugin/plugin/.claude-plugin/plugin.json harness/laptop-plugin/plugin/.mcp.json harness/laptop-plugin/plugin/hooks/hooks.json; do jq empty $f && echo ok $f; done`
Expected: four `ok` lines.

- [ ] **Step 3: Check the fallback shell**

```bash
CMD=$(jq -r '.hooks.PreToolUse[0].hooks[0].command' harness/laptop-plugin/plugin/hooks/hooks.json)
mkdir -p $S/h1/.claude; echo '{}' | HOME=$S/h1 PATH=/usr/bin:/bin sh -c "$CMD"; echo "off exit $?"
mkdir -p $S/h1/.claude/kl-mode; touch $S/h1/.claude/kl-mode/k; echo '{}' | HOME=$S/h1 PATH=/usr/bin:/bin sh -c "$CMD"
rm -rf $S/h1
```

Expected: `kl-connect` is not on that PATH. First prints nothing then `off exit 0`; second prints the deny JSON.

- [ ] **Step 4: Validate the plugin**

Run: `claude plugin validate harness/laptop-plugin 2>&1 | tail -10`
Expected: no errors. If `validate` is not a subcommand in this Claude Code version, skip and note it in the report.

- [ ] **Step 5: Commit**

```bash
git add harness/laptop-plugin
git commit -m "Add the laptop kloudlite plugin with its MCP server, kl mode hooks and commands"
```

---

### Task 4: Agents and skills

**Files:**
- Create: `harness/laptop-plugin/plugin/agents/kl-workspace.md`
- Create: `harness/laptop-plugin/plugin/agents/kl-subagent.md`
- Create: `harness/laptop-plugin/plugin/skills/kloudlite/SKILL.md`
- Create: `harness/laptop-plugin/plugin/skills/orchestrate/SKILL.md`
- Read (sources to adapt): `harness/packages/agent/claude-plugin/skills/{kloudlite,main-session,workspace-session,subagent-session}/SKILL.md`

**Interfaces:**
- Consumes: tool names from `mcpTools` (list below).
- Produces: every tool table row in the skills uses the form ``| `tool_name` | what it does |``; Task 5's test parses rows of that form. Agent frontmatter `tools:` line is exactly `tools: mcp__plugin_kloudlite_kl__*`.

Tool names `mcpTools` serves (bare names; inside Claude Code each is `mcp__plugin_kloudlite_kl__<name>`): `web_fetch web_search read write edit patch glob grep exec process_list process_output process_write process_kill watch watch_poll watch_stop graft_find_code graft_find_all graft_trace_calls graft_file_api graft_repo_map graft_build graft_blast container_build container_push workspace_list workspace_get workspace_create workspace_clone workspace_restore workspace_start workspace_stop workspace_delete workspace_push worktree_add worktree_drop packages_list packages_add packages_remove packages_update env_list env_get service_logs env_delete env_start env_stop env_create env_clone env_push env_restore env_restore_in_place service_add service_update service_remove space_env_current space_env_switch space_env_clear volume_list volume_history volume_refs volume_delete snapshot_delete builder_status quota requests_list request_get request_create regions intercept release`. The laptop roles never use `workspace_ask`, `task_*`, `workspace_tasks` or `scratch_*`; the skills say so.

- [ ] **Step 1: Write `agents/kl-workspace.md`**

```markdown
---
name: kl-workspace
description: Owns the code of ONE Kloudlite workspace, named in the prompt. Plans the goal, does its own parts, returns a split for subagents, reviews their branches, merges, tests and pushes. Works only through the kl tools.
tools: mcp__plugin_kloudlite_kl__*
---

You are the workspace agent for ONE Kloudlite workspace. The prompt names it. Read the `kloudlite:kloudlite` skill first.

You own these items of your workspace: its design, its code, its working branch, its packages, its service and its intercept.

Rules:

1. Give `workspace` = your workspace on every pod tool. Work in `~/workspace`.
2. Do not create, start, stop, clone or delete workspaces. Main does that.
3. You cannot start agents. To split work, stop and return a split to main.
4. Use only the kl tools. Nothing runs on the laptop.

How to work:

1. Read the code. Make a plan for the goal.
2. Do the parts that you keep.
3. If parts can run in parallel, return this to main and stop:
   `SPLIT` then one line per part: `<branch> — <task in one paragraph>`.
4. Main resumes you with the subagents' reports. For each report, read its branch (`exec` with `git`). Compare it with your plan. Run its tests.
5. If a branch fails review, return `REWORK <branch> — <what to change>` to main and stop.
6. Merge the accepted branches into your working branch. Run all the tests. Push.
7. Return `DONE` with the branch, the commits, the test result and what you did not do.
```

- [ ] **Step 2: Write `agents/kl-subagent.md`**

```markdown
---
name: kl-subagent
description: Does ONE part of a workspace task in a clone that main made. Works on its own branch, commits, pushes and reports. Never reviews or merges. Works only through the kl tools.
tools: mcp__plugin_kloudlite_kl__*
---

You are a subagent. The prompt names your clone, your branch and your part. Read the `kloudlite:kloudlite` skill first.

Rules:

1. Give `workspace` = your clone on every pod tool. Work in `~/workspace`.
2. Do only your part. Do not touch the parent workspace or any other workspace.
3. Do not review or merge other branches.
4. Use only the kl tools. Nothing runs on the laptop.

How to work:

1. Make your branch from the clone's current branch.
2. Do the part. Write tests for it. Run them.
3. Commit and push your branch.
4. Return `REPORT` with: `branch`, a summary in three lines or fewer, the test command and its result, and anything you did not do.
```

- [ ] **Step 3: Write `skills/kloudlite/SKILL.md`**

Frontmatter:

```markdown
---
name: kloudlite
description: Concepts and tools for Kloudlite work from the laptop through the kl tools. Read it before any workspace, environment or code task in kl mode.
---
```

Body in ASD-STE100, adapted from the bench `kloudlite` skill. Sections, in this order:

1. `# Kloudlite from the laptop` — one paragraph: the laptop session and its agents do the work; the bench serves the tools; in kl mode no local tool works.
2. `## Concepts` — workspace, clone, snapshot (push), environment, service, intercept, space (team) and its environment. Take the definitions from the bench skill; remove permission cards and board text.
3. `## Every pod tool takes \`workspace\`` — the pod tools work in the pod of the named workspace; the workspace must be running; start it with `workspace_start` (main only).
4. `## Tools` — one table per group: Code (`read write edit patch glob grep`), Run (`exec process_list process_output process_write process_kill watch watch_poll watch_stop`), Graph (`graft_*` all eight), Build (`container_build container_push builder_status`), Workspaces (`workspace_* worktree_add worktree_drop`), Packages (`packages_*`), Environments (`env_* service_* intercept release service_logs`), Spaces (`space_env_*`), Storage (`volume_* snapshot_delete`), Account (`quota requests_list request_get request_create regions`), Web (`web_fetch web_search`). Every row: ``| `name` | one sentence |``. Take each sentence from the tool's own description in `harness/packages/tools/src/{pod,platform}.ts`.
5. `## Tools that the laptop does not use` — `workspace_ask`, `task_add`, `task_update`, `task_list`, `workspace_tasks`, `scratch_bash`, `scratch_read`, `scratch_write`: they are the bench sessions' tools. Do not use them. (Plain text list, not table rows.)
6. `## Roles` — main, `kl-workspace`, `kl-subagent`, one paragraph each, as in the spec's Roles section.

- [ ] **Step 4: Write `skills/orchestrate/SKILL.md`**

Frontmatter:

```markdown
---
name: orchestrate
description: The laptop main session's role in kl mode. Lifecycle and environments only; code goes to kl-workspace agents; relay splits and reports between agents.
---
```

Body in ASD-STE100, adapted from the bench `main-session` skill. Sections, in this order:

1. `# You are main` — you own lifecycle and environments. You do not read code. You do not judge code.
2. `## Your tools` — table rows for: `workspace_list workspace_get workspace_create workspace_clone workspace_restore workspace_start workspace_stop workspace_delete workspace_push env_list env_get env_create env_start env_stop env_delete env_clone env_push env_restore env_restore_in_place service_add service_update service_remove service_logs space_env_current space_env_switch space_env_clear quota builder_status regions`.
3. `## Give code work to kl-workspace` — one `kl-workspace` agent per workspace; the prompt names the workspace and the goal; start the workspace first.
4. `## The flow` — the spec's flow as numbered steps: on `SPLIT`, `workspace_clone` once per part (name `<ws>-<part>`), start each clone, spawn one `kl-subagent` per part in parallel with clone, branch and part; collect each `REPORT`; resume the SAME `kl-workspace` with `SendMessage` and all reports; on `REWORK`, resume that part's `kl-subagent` with `SendMessage` and the text, then send the new report; on `DONE`, `workspace_delete` each clone and tell the person.
5. `## If an agent is gone` — spawn a new `kl-workspace` with the goal, the split and the reports.
6. `## Rules` — no local tools in kl mode; do not do a workspace agent's work; ask the person before `workspace_delete` of a workspace that main did not create in this task.

- [ ] **Step 5: Check names by hand**

Run: `grep -ho '| `[a-z_]*`' harness/laptop-plugin/plugin/skills/*/SKILL.md | sort -u | wc -l`
Expected: a count; Task 5's test does the real check.

- [ ] **Step 6: Commit**

```bash
git add harness/laptop-plugin/plugin/agents harness/laptop-plugin/plugin/skills
git commit -m "Add the kl-workspace and kl-subagent agents and the laptop skills"
```

---

### Task 5: Plugin consistency test

**Files:**
- Create: `harness/packages/backend/src/laptop-plugin.test.ts`

**Interfaces:**
- Consumes: `mcpTools(deps)` from `./local.ts`; skill table rows ``| `name` |``; agent `tools:` lines (Task 4).

- [ ] **Step 1: Write the test**

```ts
import { expect, test } from "bun:test";
import { mkdtempSync, readdirSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { mcpTools } from "./local.ts";

const PLUGIN = join(import.meta.dir, "../../../laptop-plugin/plugin");
const LAPTOP_NEVER = ["workspace_ask", "task_add", "task_update", "task_list", "workspace_tasks", "scratch_bash", "scratch_read", "scratch_write"];

const served = async () => {
  const deps = { live: new Map(), busy: new Set<string>(), open: async () => null as never, permit: async () => ({}), tasks: mkdtempSync(join(tmpdir(), "kl-plugin-")) };
  return new Set((await mcpTools(deps)).map((t) => t.name));
};
const skills = () => readdirSync(join(PLUGIN, "skills")).map((d) => readFileSync(join(PLUGIN, "skills", d, "SKILL.md"), "utf8"));
const agents = () => readdirSync(join(PLUGIN, "agents")).map((f) => readFileSync(join(PLUGIN, "agents", f), "utf8"));

test("every tool a laptop skill lists is one the bench serves, and none is a bench-session tool", async () => {
  const names = await served();
  const rows = skills().flatMap((s) => [...s.matchAll(/^\| `([a-z_]+)` \|/gm)].map((m) => m[1]!));
  expect(rows.length).toBeGreaterThan(40);
  for (const n of rows) expect(names.has(n) ? n : `missing: ${n}`).toBe(n);
  for (const n of rows) expect(LAPTOP_NEVER).not.toContain(n);
});

test("every mcp__plugin_kloudlite_kl__ name in the plugin is one the bench serves", async () => {
  const names = await served();
  for (const text of [...skills(), ...agents()])
    for (const m of text.matchAll(/mcp__plugin_kloudlite_kl__([a-z_]+)/g)) expect(names.has(m[1]!) ? m[1] : `missing: ${m[1]}`).toBe(m[1]!);
});

test("the two agents have the kl tools only", () => {
  const a = agents();
  expect(a.length).toBe(2);
  for (const text of a) expect(text.match(/^tools: (.*)$/m)?.[1]).toBe("mcp__plugin_kloudlite_kl__*");
});
```

- [ ] **Step 2: Run it**

Run: `cd harness/packages/backend && bun test src/laptop-plugin.test.ts 2>&1 | tail -15`
Expected: 3 pass. A `missing: <name>` failure means Task 4's skill names a tool the bench does not serve: fix the skill, not the test.

- [ ] **Step 3: Typecheck**

Run: `cd harness/packages/backend && bunx tsc --noEmit 2>&1 | tail -10`
Expected: no errors.

- [ ] **Step 4: Commit**

```bash
git add harness/packages/backend/src/laptop-plugin.test.ts
git commit -m "Hold the laptop plugin's tool names equal to the bench's"
```

---

### Task 6: Install and live check

Controller does this, not an implementer subagent: it installs into the person's Claude Code and builds the laptop `kl-connect`.

**Files:** none in the repo.

- [ ] **Step 1: Build `kl-connect` from a clean worktree at the branch head and install it**

```bash
SHA=$(git rev-parse HEAD)
git worktree add --detach /Volumes/kdisk/rustic-git-wt/kl-plugin-build $SHA
cd /Volumes/kdisk/rustic-git-wt/kl-plugin-build && CARGO_TARGET_DIR=/Volumes/kdisk/target-names cargo build --release -p kl-connect 2>&1 | tail -3
cp /Volumes/kdisk/target-names/release/kl-connect "$(command -v kl-connect)"
cd /Volumes/kdisk/rustic-git-wt/names && git worktree remove /Volumes/kdisk/rustic-git-wt/kl-plugin-build
kl-connect mode --help >/dev/null && echo ok
```

- [ ] **Step 2: Install the plugin**

```bash
claude plugin marketplace add /Volumes/kdisk/rustic-git-wt/names/harness/laptop-plugin
claude plugin install kloudlite@kloudlite
```

- [ ] **Step 3: Live check in a fresh session (`claude -p` runs, one per line)**

```bash
cd $S && claude -p --permission-mode bypassPermissions "/kloudlite:on"
cd $S && claude -p --resume <that session> --permission-mode bypassPermissions "Read the file /etc/hosts with the Read tool and quote the first line."
```

Expected: the Read call is denied with the kl mode reason. Then in the same session: spawn `kl-workspace` on `bench-505f6b8c9d7b`'s workspace to `read` a file (works, through kl); have a `general-purpose` subagent run `Bash ls` (denied); `/kloudlite:off`; Read `/etc/hosts` works. Record each result in the ledger.

- [ ] **Step 4: Clean up**

`rm -rf ~/.claude/kl-mode` if any flag is left from the check. Ledger line with the commits and the live results.

---

## Self-review notes

- Spec coverage: roles and flow (Task 4 agents and orchestrate skill), layout and install (Tasks 3, 6), kl mode state, toggle, gate, fail-closed (Tasks 2, 3), subagent session id probe and `cwd` fallback (Tasks 1, 2), tests (Tasks 2, 5, 6).
- Ruling recorded: the spec's fallback `kl-connect mode gate || echo '<deny json>'` would deny every local tool with the mode off whenever `kl-connect` is missing. The plan's fallback denies only while a flag file exists. Cost if wrong: a missing `kl-connect` with the mode on for one session also denies in other sessions until the flag is removed.
- `Task` is in the allow list beside `Agent`: older Claude Code builds name the subagent tool `Task`.
