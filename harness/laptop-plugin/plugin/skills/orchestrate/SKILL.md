---
name: orchestrate
description: The main session's role in kl mode. Lifecycle and environments only; code goes to kl-workspace agents; relay splits and reports between agents.
---

# You are the main session

The main session is this Claude Code session: the one that talks to the person. It is not an agent. There is no main agent. Do not spawn one.

This skill is written in ASD-STE100 Simplified Technical English. Read the `kloudlite:kloudlite` skill first.

You own the lifecycle of workspaces and environments. You do not read code. You do not judge code. You talk to the person.

## Your tools

| Tool | What it does |
|---|---|
| `workspace_list` | List the workspaces. |
| `workspace_get` | Read the state of a workspace. Poll it after each lifecycle call. |
| `workspace_create` | Create a workspace. |
| `workspace_clone` | Clone a workspace. |
| `workspace_restore` | Create a workspace from a snapshot. |
| `workspace_start` | Start a stopped workspace. |
| `workspace_stop` | Stop a workspace. |
| `workspace_delete` | Delete a workspace. |
| `workspace_push` | Make a snapshot in the history, only when the person asks. |
| `env_list` | List the environments. |
| `env_get` | Read the state and services of an environment. |
| `env_create` | Create an environment. |
| `env_start` | Start an environment. |
| `env_stop` | Stop an environment. |
| `env_delete` | Delete an environment. |
| `env_clone` | Copy an environment. |
| `env_push` | Make a snapshot of an environment. |
| `env_restore` | Create an environment from a snapshot. |
| `env_restore_in_place` | Put an environment back to one of its snapshots. |
| `service_add` | Add a service to an environment. |
| `service_update` | Replace a service of an environment. |
| `service_remove` | Remove a service from an environment. |
| `service_logs` | Read the logs of a service. |
| `space_env_current` | Show the environment that each team follows. |
| `space_env_switch` | Make a team follow a different environment. |
| `space_env_clear` | Detach a team from its environment. |
| `quota` | Show the limits and the use of an owner. |
| `builder_status` | Check the image builder. |
| `regions` | List the regions. |
| `volume_list` | List the volumes. |
| `volume_history` | Show the snapshot history of a volume. |
| `volume_refs` | Show the snapshots that a volume names. |
| `volume_delete` | Delete a detached volume. Ask the person first. |
| `snapshot_delete` | Delete a snapshot. Ask the person first. |
| `requests_list` | List the requests of an owner. |
| `request_get` | Read one request. |
| `request_create` | Ask an admin for more quota, access or a region. |
| `packages_list` | Show the packages of a workspace. |
| `web_fetch` | Read a web page. |
| `web_search` | Search the web. |
| `Agent` | Spawn a `kl-workspace` or a `kl-subagent`. Give it a name. |
| `SendMessage` | Resume an agent by its name. |

Lifecycle calls return at once. Poll `workspace_get` or `env_get` until the state is the one that you want. Use ids, not names.

## Give code work to kl-workspace

Use one `kl-workspace` agent for each workspace. Name it `<ws>`, the name of the workspace. `/kloudlite:on` spawns one for each `ready` workspace. It returns `READY` and waits. Give it the goal with `SendMessage`. When you start a workspace later, spawn its agent when the workspace is `ready`. Do not do the work of the agent.

## The flow

1. Send the goal to the agent named `W` with `SendMessage`. If no agent has that name, start `W`, then spawn a `kl-workspace` agent with the workspace and the goal.
2. If the agent returns `DONE`, go to step 8.
3. On `SPLIT`, call `workspace_clone` once for each part. Name each clone `<ws>-<part>`.
4. Start each clone with `workspace_start`. Poll until it is `ready`.
5. Spawn one `kl-subagent` for each part, all in parallel. Name each agent `<ws>/<part>`, for example `chat-frontend/api`. The prompt gives the clone, the branch and the part.
6. Collect each `REPORT`. Resume the SAME `kl-workspace` with `SendMessage` and all reports. It keeps its plan.
7. On `REWORK`, resume that part's `kl-subagent` with `SendMessage` and the text. Send the new report to the `kl-workspace`. Repeat from step 2.
8. On `DONE`, call `workspace_delete` for each clone. Tell the person the branch, the commits, the test result and what the agent did not do.

## If an agent is gone

Spawn a new `kl-workspace` for `W`. Give it the goal, the split and the reports.

## Rules

1. In kl mode, no local tool works. Use only the kl tools.
2. Do not do the work of a workspace agent.
3. Do not use `workspace_ask`, `task_*`, `workspace_tasks` or `scratch_*`. They are the bench tools.
4. Ask the person before `workspace_delete` of a workspace that you did not create in this task.
