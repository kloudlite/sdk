# Session token budget

Date: 2026-10-11. Status: draft, waiting for owner review.

## Goal

A harness session (bench main, workspace and subagent sessions, as seen in kl-tui) never sends a
request larger than its budget, sends far fewer tokens per turn, writes shorter replies, and starts
fresh from a saved summary when a task is done and verified.

## Owner rulings (quoted)

- "we need auto compaction" / "can we reduce the amount of tokens given out by these models" /
  "when the task is done and stable can we actually get the task summary persisted and start fresh".
  "yes I want all these".
- Who decides done and stable: "Agent, after verify". The agent marks a task done with a summary only
  after its check passes; the harness then archives the session and starts fresh. No step for the person.

## Incident (checked 2026-10-11)

Session `ws-879b4a9e8779e8b3` (blog-app) in `bench-505f6b8c9d7b`, model `deepseek/deepseek-flash`:
the file is 6.0 MB, 226 lines. Line 222 is one `exec` tool result of 4 608 403 characters (npm
registry JSON on stdout). pi compacted at `tokensBefore: 1244468` but keeps the most recent messages,
so the giant result stayed. The next request was refused: "This model's maximum context length is
1048576 tokens. However, you requested 1923634 tokens".

Causes, from the source:

- `adaptTools` (`harness/packages/agent/src/index.ts:81`) puts the raw tool string into history. The
  pod caps a job at 4 MiB per stream (`crates/ide/src/tools/exec.rs:17`), about 1M tokens.
- pi compacts at `contextTokens > contextWindow - reserveTokens` (`compaction.js:173-176`, reserve
  16384). With 1M-token windows a session resends close to 1M tokens on every turn before it compacts.
- No max output is set: pi sends `model.maxTokens` (`simple-options.js:23`), 384 000 for
  deepseek-flash. The owner's `settings.json` sets thinking `high`.
- `subagent` and `workspace_ask` replies enter the parent without a length cap (`delegate.ts:100-117`,
  `:288-316`).

## Design

### 1. Cap every tool result (root cause)

New pi extension `harness/packages/agent/src/budget.ts`, registered beside `orphans` in
`createSession`, with two hooks:

- `tool_result`: when the text content is over `TOOL_CAP` = 32 000 characters, replace it with the
  first 12 000 characters, a line `[cut: N characters, middle dropped. Run it again with output to a
  file under ~/.cache/kl/ and use grep or read on that file.]`, and the last 12 000 characters.
  `structuredContent` and `isError` pass through unchanged. The session file then never stores a giant
  result.
- `context` (the backstop, every send): any single message whose text is over `MESSAGE_CAP` =
  64 000 characters is cut the same way in the outgoing copy. This covers `subagent` replies,
  `[from <key>]` replies, user pastes, and the giant message already stored in the blog-app session,
  so that session works again on its next prompt without a hand edit.

The full output is not saved by the harness. `exec` output is cheap to recreate, and the cut note
tells the model how. ponytail: no spill file; add one if a non-repeatable command loses data.

### 2. Compact on a budget, not on the model limit

New function `budgeted(model)` in `budget.ts`, applied inside `resolveModel`
(`harness/packages/agent/src/index.ts:93`). Both callers go through it: the first build
(`local.ts:590`) and every model switch (`setModel`, `local.ts:332`).

```ts
export const CONTEXT_BUDGET = 200_000;
export const OUTPUT_CAP = 32_000;
export const budgeted = <M extends Model<Api>>(m: M): M => ({
  ...m,
  contextWindow: Math.min(m.contextWindow || CONTEXT_BUDGET, CONTEXT_BUDGET),
  maxTokens: Math.min(m.maxTokens || OUTPUT_CAP, OUTPUT_CAP),
});
```

pi then compacts at about 184k tokens (200 000 - 16 384), clamps output to the window, and treats a
request over 200k as an overflow it recovers from by compacting. The compaction summary is capped at
`min(0.8 * 16384, maxTokens)` (`compaction.js:512`), so summaries stay short. The model's real window
is never used past 200k; that is the point. ponytail: constants, not live settings; make them settings
if one model needs a different budget.

### 3. Less output

- `OUTPUT_CAP` (above) caps every reply at 32 000 tokens, thinking included.
- Default thinking stays `medium` (`local.ts:571`). The owner's own
  `~/.config/kloudlite/settings.json` has `high`; it is changed to `medium` in the bench's settings
  file as part of the ship, not in code.
- A "Short replies" rule in the `kloudlite` skill (`harness/packages/agent/claude-plugin/skills/
  kloudlite/SKILL.md`), in ASD-STE100: "Write short replies. Do not repeat tool output. Do not repeat
  the plan. Give the result first. To see a large output, write it to a file and use grep or read
  with a limit."

### 4. Task done, then a fresh session

- `BoardTask` (`harness/packages/backend/src/index.ts:60`) gains `summary?: string` and
  `verified?: string`. `task_update` (`tasks.ts:126`) accepts both. A change to `state: "done"` is
  refused unless both are set and non-empty: `verified` names the check that passed (command and
  result), `summary` says what changed and where (at most 2 000 characters, longer is refused).
- Skill rule (`main-session`, `workspace-session`, `subagent-session` SKILL.md): "Mark a task done only
  after its check passes. Give the summary and the check in the same `task_update`."
- Trigger: on `agent_end`, when the session's board has a task that went to `done` during that
  turn and no task is `running`, the backend restarts the session fresh:
  1. archive the current files as `/clear` does (`clearSessionHistory`), BEFORE the fresh session is
     built (today `clear` builds first, then archives; the order is fixed so the new file survives);
  2. build the session with `fresh: true`;
  3. store a seed: the open board (`boardText`), the summaries of the last 10 done tasks (each with its
     `verified`), capped at 8 000 characters. The next prompt into that session is sent as
     `[fresh session after task <id>]\n<seed>\n\n<prompt>`, once.
- The TUI shows one line: `fresh session after task <id>: <title>`.
- Subagent sessions do not restart (they end when their task ends).

## Tests

- `budget.test.ts`: a 4.6M-character tool result is stored as head + note + tail under `TOOL_CAP`;
  `structuredContent` and `isError` pass through; a 100k-character message is cut in the `context`
  copy and the stored message is unchanged; a small message is untouched.
- `budgeted`: a 1 048 576 / 384 000 model becomes 200 000 / 32 000; a 128 000 / 8 192 model is
  unchanged; a model with `contextWindow: 0` gets the budget.
- `tasks.test.ts`: `done` without `summary` or `verified` is refused with a message naming the
  missing field; a summary over 2 000 characters is refused; `done` with both is stored.
- `local.test.ts`: after a turn that marks a task done with nothing running, the old file is in
  `archive/`, a new session file exists, and the next prompt carries the seed once; a turn that marks
  done while another task is `running` does not restart.
- Live check after ship: in the blog-app session, send one prompt; it answers instead of the 400.
  Run `npm view <pkg>` with full JSON; the result is cut with the note. Finish a small task; the TUI
  shows the fresh-session line and the next reply knows the board.

## Known gaps (not fixed here)

- The cut is by characters, not tokens; dense text (minified JSON) can be up to about 1 token per
  3 characters, so 32 000 characters is at most about 11k tokens.
- A fresh restart drops the conversation about tasks that are `queued` or `blocked`; only the board
  and the done summaries carry over.
- `workspace_ask` replies are cut only by the 64 000-character backstop, not at 32 000.
