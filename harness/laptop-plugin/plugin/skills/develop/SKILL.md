---
name: develop
description: You are the MAIN session in kl mode and the person asks for a change to code. Read this skill before you give any code work to a workspace. It is the superpowers flow for kl mode - brainstorm with the person, write the spec, write the plan, then run the plan through the kl-workspace agents with a review of each task.
---

# Develop: brainstorm, spec, plan, run

This skill is written in ASD-STE100 Simplified Technical English. Each word has one meaning. Each instruction is one sentence.

You are the main session. The `orchestrate` skill gives your role and the `kloudlite` skill gives the concepts. This skill gives the steps for each change to code. You do not read code and you do not write files on the laptop. The workspaces read the code, write the documents and do the work. You decide, you write the text, and you carry the messages.

## Select the path

| The request | Path |
|---|---|
| One workspace, one small change, no change to a contract. Example: a fix, a new flag, one endpoint. | **Small.** Do "Brainstorm", then write a short design in chat (three to ten lines). Get a yes from the person. Then send one ask and do "Review". Do not write a spec or a plan. |
| A new component, a feature, work in more than one workspace, or a change to a contract | **Full.** Do all the steps below, in sequence. |

If you are not sure, use the full path. If a small change becomes larger, stop and change to the full path. Do not change from the full path to the small path.

Each step ends with a yes from the person. Do not start the next step without that yes. A yes for a step is not a yes for the next step.

## 1. Brainstorm

1. **Get the facts.** You do not read code. Ask the agent of each workspace that the change touches for the facts that you need. Write the ask like this: "Facts only, do not change code: <your questions>." Examples: the current interface, the data model, the test command, the services that the code uses. Use `env_get` and `workspace_list` for the platform facts.
2. **Ask the person.** Ask one question at a time with `AskUserQuestion`. Ask about the goal, the users, the limits and how to know that the work is done. If the request already gives a fact, do not ask about it again.
3. **Write back.** Give the person a short note: the goal, the limits, and the success criteria. Mark which items the person said and which items you assume. Correct the note with their answer.
4. **Give the approaches.** Give two or three approaches with their trade-offs. Put your recommendation first and give the reason in one sentence.
5. **Give the design.** Give the design in sections: the components and the workspace that owns each one, the contract between them, the data flow, the errors, and the tests. Ask the person after each section if it is correct.

Remove each feature that the person did not ask for.

## 2. Spec

1. **Write the spec text.** Use the approved design. Include these sections: Goal, Decisions of the person, Components (one workspace each), Contract, Errors, Tests, Out of scope.
2. **Check the spec.** Remove each "TBD" and each open item. Each requirement has one meaning. The sections do not contradict each other.
3. **Select the docs workspace.** This is the workspace that owns the main component. If there is no main component, ask the person.
4. **Commit the spec.** Send the docs workspace this ask: "Write this text, without a change, to `docs/superpowers/specs/<YYYY-MM-DD>-<feature>-design.md`. Commit it on your working branch and push. Report the branch and the commit." Give the full text in the ask.
5. **Get a yes.** Tell the person the path, the branch and the commit. Ask the person to read the spec. If the person asks for changes, send the full new text again, with the same steps.

## 3. Plan

1. **Write the plan text.** Start with these lines: Goal, Spec (its path), Workspaces, Contract (the file `contract-<feature>.md` and its version).
2. **Write the tasks.** Each task has these items:
   - `Task <N>`: a short name.
   - `Workspace`: the one workspace that does the task.
   - `Goal`: the result, in the words of the spec.
   - `Done when`: the tests that prove the result. Name each test case and its expected result.
   - `Needs`: the tasks that must be complete first, or `none`.
3. **Size the tasks.** One task is one result that a reviewer can accept or refuse alone. Put setup and docs into the task that needs them. Give a workspace one task at a time.
4. **Check the plan.** Each requirement of the spec has a task. Each name and each interface is the same in all tasks. The contract in the tasks agrees with the contract file.
5. **Write the contract.** If the work crosses components, write `contract-<feature>.md` v1 with `scratch_write`, as in the `orchestrate` skill.
6. **Commit the plan.** Send the docs workspace the same ask as for the spec, to `docs/superpowers/plans/<YYYY-MM-DD>-<feature>.md`.
7. **Get a yes.** Tell the person the path and ask for a yes before you run the plan.

## 4. Run

**The ledger.** Keep the progress in `progress-<feature>.md` with `scratch_write`. Its first line is `# plan: <plan path>`. Add one line at each event:

- `Task <N>: sent`
- `Task <N>: done <branch> <base>..<head>`
- `Task <N>: review <R> findings` or `Task <N>: review <R> approved`
- `Task <N>: complete`

Your memory does not stay after a compaction. Before you send a task, read the ledger with `scratch_read`. Do not send a task again that has `complete` or `sent` without a later `done`.

**Send the tasks.**

1. Send each task whose `Needs` are complete. Send all of them in one message, to different workspaces. Use the rules of "How to write an ask" in the `orchestrate` skill.
2. Each ask gives: the plan path, the task text without a change, the full contract text with its version, and this line: "Report the base commit (the head before your first change) and the head commit."
3. When a workspace reports `NEED`, `BLOCKED` or `SUBAGENT`, do the steps of the `orchestrate` skill.

## Review

Do this for each `DONE` report of a task.

1. Write `Task <N>: done ...` in the ledger.
2. Use `workspace_clone` on the workspace. Give the clone the name `<ws>-review-<N>`. Poll `workspace_get` until it is `ready`.
3. Start a `kl-subagent` agent with `name: "<ws>/review-<N>"`, `run_in_background: true` and this prompt: `Clone <clone name> (<clone id>) of workspace <ws>. Review only. Range <base>..<head>. Spec <spec path>. Task: <the task text, without a change>`.
4. The reviewer report starts with `APPROVED` or `FINDINGS`. Delete the review clone with `workspace_delete`. A review clone has no commits, so this delete loses no work.
5. If the report is `APPROVED`, write `Task <N>: complete` in the ledger. Then send the tasks that this task let start.
6. If the report is `FINDINGS`, send the findings to the workspace without a change: "Review findings for Task <N>: <findings>. Fix each one, or give the reason that it is not a problem. Run the tests, push, and report `DONE` with the base and head commits." Then do this review again with a new clone.
7. Do a maximum of three review rounds for one task. If findings stay after round three, stop that task. Tell the person the findings and ask for a decision.

## Finish

1. When every task is complete, do the integration of the `orchestrate` skill, if the work crosses components.
2. Tell the person the result: each workspace, its branch, its commits, the spec and plan paths, and the decisions that the person must make.
3. Do not merge. Open or merge a pull request only when the person tells you to.
