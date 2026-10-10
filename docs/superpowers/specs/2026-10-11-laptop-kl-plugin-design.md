# Laptop Claude Code plugin around `kl-connect mcp`

Date: 2026-10-11. Status: design, awaiting owner review.

## Goal

A laptop Claude Code session works on Kloudlite workspaces the way the bench does, using the
same tools. The laptop session and its subagents do the work themselves (owner choice "A"); the
bench only serves tools over `kl-connect mcp`. While **kl mode** is on, nothing runs or writes on
the laptop: every agent has the kl tools only.

Owner rulings this design carries:

- "I want an entire skill. I want all the subagents and workspaces etc to be designed with these
  tools." Not only an MCP entry.
- "when the mod is active it should not write locally and all the agents should be provided with
  this tools only not local tools."
- "we have workspace agent, and subagents only." No other agent kinds.
- "workspace agent is responsible for the code in the workspace and it will be responsible for the
  review etc."

## Non-goals

- No new bench tools. `mcpTools` (`harness/packages/backend/src/local.ts`) is the tool set as
  shipped in e62a9fdf.
- No change to the bench's own plugin (`harness/packages/agent/claude-plugin/`) or its sessions.
- No permission cards. On the laptop, Claude Code's own permission prompts are the only prompts.

## Roles

Three roles, matching the platform's main / workspace / subagent sessions.

**main** is the laptop session itself, not a plugin agent. It owns lifecycle and environments:
create, start, stop, clone and delete workspaces; set up environment services and space bindings.
It never reads or judges code. It also relays messages between agents, because Claude Code agents
cannot call each other.

**`kl-workspace`** (plugin agent) owns the code of ONE workspace, named in its prompt, from start
to finish:

1. Plans the goal main gave it.
2. Does the parts it keeps, through the pod tools (`read`, `write`, `edit`, `exec`, ...) with
   `workspace` set to its own workspace.
3. Returns any split to main as a list of parts. Each part names a branch and a one-paragraph
   task.
4. When main resumes it with the subagents' reports, reviews each branch against its plan. A
   branch that fails review goes back to main as rework for that part.
5. Merges the accepted branches, runs the tests, pushes, and reports done to main.

It also owns that workspace's packages, its service and its intercept.

**`kl-subagent`** (plugin agent) does ONE part in a clone that main created. It works on its own
branch, commits, pushes and reports `{branch, summary, tests}`. It does not review or merge, and it
does not touch the parent workspace.

### Flow

```
person ──goal──▶ main
main ──goal, workspace W──▶ kl-workspace(W)
kl-workspace(W) ──split: parts A, B──▶ main
main: workspace_clone W → W-a, W-b
main ──part A, clone W-a──▶ kl-subagent      (parallel)
main ──part B, clone W-b──▶ kl-subagent
kl-subagent ──report(branch)──▶ main
main ──SendMessage: reports──▶ kl-workspace(W)   (same agent, context kept)
kl-workspace(W): review → rework via main, or merge + test + push
kl-workspace(W) ──done──▶ main
main: workspace_delete W-a, W-b
```

Main resumes the same `kl-workspace` with `SendMessage`, so the workspace agent keeps its plan
between the split and the review. If that agent is gone, main spawns a new `kl-workspace(W)` with
the original goal, the split and the reports.

## Plugin layout

New directory `harness/laptop-plugin/`, its own marketplace:

```
harness/laptop-plugin/
  .claude-plugin/marketplace.json   # marketplace "kloudlite", one plugin "kloudlite" at ./plugin
  plugin/
    .claude-plugin/plugin.json      # {"name":"kloudlite"}
    .mcp.json                       # server "kl": kl-connect mcp ${KL_TEAM}
    agents/kl-workspace.md
    agents/kl-subagent.md
    skills/kloudlite/SKILL.md       # concepts: workspaces, clones, environments, intercepts, tools
    skills/orchestrate/SKILL.md     # main's role and the flow above
    commands/on.md                  # /kloudlite:on
    commands/off.md                 # /kloudlite:off
    hooks/hooks.json
```

Install: `claude plugin marketplace add ~/rustic-git/harness/laptop-plugin` then
`claude plugin install kloudlite@kloudlite`.

Tool names inside Claude Code are `mcp__plugin_kloudlite_kl__<tool>`. Both agents list
`tools: mcp__plugin_kloudlite_kl__*` plus nothing local. The skills are adapted from the bench
skills (`kloudlite`, `main-session`, `workspace-session`, `subagent-session`): same concepts, but
every pod tool takes `workspace`, there are no permission cards and no board tools for the laptop
roles. They stay in ASD-STE100 Simplified Technical English, like the bench skills.

## kl mode

**State**: a flag file `~/.claude/kl-mode/<session_id>`. On means the file exists.

**Toggle**: `/kloudlite:on` and `/kloudlite:off`. A `UserPromptSubmit` hook sees the prompt text
and creates or removes the flag before the command body runs. While on, the same hook adds one
context line: "kl mode is on: work only through the kl tools; nothing runs on this laptop."

**Gate**: a `PreToolUse` hook with matcher `.*` (all tools, main session and subagents). While the
flag exists, it allows:

- `mcp__plugin_kloudlite_kl__*`
- `Agent`, `Skill`, `ToolSearch`, `AskUserQuestion`, `SendMessage`
- the task and todo tools (`TaskCreate`, `TaskUpdate`, `TaskList`, `TaskGet`, `TodoWrite`)

It denies everything else (`Read`, `Write`, `Edit`, `Bash`, `Glob`, `Grep`, `WebFetch`,
`WebSearch`, `NotebookEdit`, other MCP servers) with
`{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"kl mode: use the kl tools; nothing runs on this laptop"}}`.
While the flag is absent it outputs nothing and the call proceeds as usual. The bench's own
`web_fetch` and `web_search` tools stay available through kl.

**Code**: both hooks call one Rust subcommand, `kl-connect mode gate` and
`kl-connect mode prompt`, reading the hook JSON on stdin. The allow list lives in
`bins/kl-connect/src/mode.rs`. A broken or missing `kl-connect` fails closed for the gate: the
hook command is `kl-connect mode gate || echo '<deny json>'`.

**Subagents**: the gate keys on `session_id`. Whether a subagent's hook input carries the parent's
`session_id` is not documented. The plan's first step probes it. If it differs, the key becomes the
working folder (`cwd` from the hook input, hashed), which subagents share with main.

## Tests

- `kl-connect mode`: unit tests for gate decisions (allowed name, denied name, other MCP server,
  flag absent) and for the toggle (`/kloudlite:on`, `/kloudlite:off`, unrelated prompt).
- Plugin consistency test (bun, in `harness/packages/backend`): every tool name that an agent file
  or skill names exists in `mcpTools`; the agent `tools:` lines name no local tool.
- Live check, by hand, once: install the plugin, `/kloudlite:on`, ask main to read a local file
  (denied), spawn `kl-workspace` on the bench workspace and have it read a file there (works),
  `/kloudlite:off`, local read works again.

## Risks

- **Subagent session id**: covered by the probe and the `cwd` fallback.
- **Matcher drift**: a new local Claude Code tool is denied by default, because the gate is an
  allow list.
- **Long merges** in `kl-workspace` go through `exec` in the pod, with the same limits as on the
  bench.
