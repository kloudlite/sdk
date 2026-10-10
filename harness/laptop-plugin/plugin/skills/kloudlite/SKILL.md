---
name: kloudlite
description: Concepts and tools for Kloudlite work from the laptop through the kl tools. Read it before any workspace, environment or code task in kl mode.
---

# Kloudlite from the laptop

This skill is written in ASD-STE100 Simplified Technical English. The laptop session and its agents do the work. The bench serves the tools. In kl mode, no local tool works: use only the kl tools (`mcp__plugin_kloudlite_kl__<name>`). In a request, the words "workspace", "environment", "service" and "snapshot" mean Kloudlite items.

## Concepts

**Workspace.** One dev machine. It is a pod with its own home, `/home/kl` (a btrfs volume), source code in `~/workspace` and its own packages. Use one workspace for each component, for example frontend, backend or tests.

**Worktree.** An extra working copy in a workspace. It gives a second branch without a second workspace.

**Clone.** A copy of a workspace in its current state. The clone is a new workspace. The `based_on` field in the reply tells you the age of the copy. The main session makes clones for subagents.

**Snapshot.** A read-only copy of the disk of a workspace or environment. A push records a named snapshot in the history. This push is not `git push`. Sync points are automatic copies for recovery after a crash. They do not show in the history. A restore makes a new copy from a snapshot.

**Environment.** A shared stack of services for a team, for example a backend, its database and a queue. Each service runs as its own StatefulSet.

**Service data.** A service with no `mounts` loses its data when its pod restarts. Give each directory that must persist a mount in the same call that creates the service. A mount has a `folder` (a name in the volume of the environment) and a `path` (the directory in the container).

**Service image.** Use a public image or build one with `container_build`. Give `tags: ["name:tag"]`. The build context is the working directory of the workspace. The `kl` command is not on the PATH in `exec`. The build tools run it for you.

**Intercept.** An intercept sends the traffic for one service of an environment to a workspace. One workspace at a time can intercept a service. A stopped workspace releases the intercept after a short time. The request stays until `release`.

**Space.** A space is a team. It follows one environment. The workspaces of the team reach the services of that environment by DNS name.

**Packages.** Nix packages with a version: `name@version`. The version is `latest`, `N`, `N.N` or `N.N.N`. A version that is not in the cache gives 422 and the list of cached versions.

**Lifecycle calls are asynchronous.** Create, start, stop, clone and restore return at once. Poll `workspace_get` or `env_get` until the state is the one that you want. A workspace is up when its state is `ready`. An environment is up when its state is `running`. Stop the poll on `error` or `deleted`.

**Ids.** Use the id (`ws-…`, `env-…`) in tool calls. Do not use the name.

## Every pod tool takes `workspace`

The pod tools work in the pod of the workspace that you name in the `workspace` argument. The workspace must be up. Only the main session starts a workspace, with `workspace_start`. Always give `workspace`.

## Tools

### Code

| Tool | What it does |
|---|---|
| `read` | Read a file in the workspace. |
| `write` | Write a whole file. |
| `edit` | Replace exact text in a file. |
| `patch` | Apply a unified diff. Use it for many changes at the same time. |
| `glob` | Find files by name pattern. |
| `grep` | Search file contents with a regex. |

### Run

| Tool | What it does |
|---|---|
| `exec` | Run a command. It waits for a job that ends, for example a build, a test or `git`. With `detach: true` it starts a process that continues to run and returns a process id. |
| `process_list` | List the processes that run. |
| `process_output` | Read the output of a process (the last 4 MiB). |
| `process_write` | Write to the stdin of a process. |
| `process_kill` | Stop a process and its group. |
| `watch` | Wait for a change to files or events. |
| `watch_poll` | Read the events that a watch collected. |
| `watch_stop` | Stop a watch. |

### Graph

Use the graph first to find code and to see what a change can break.

| Tool | What it does |
|---|---|
| `graft_find_code` | Ask in plain words where an item is or how it works. |
| `graft_find_all` | Search the graph with a regex to find each use of a name. |
| `graft_trace_calls` | Find the callers and callees of a symbol before you change it. |
| `graft_file_api` | See the signatures of a file without its function bodies. |
| `graft_repo_map` | Get a map of a repo that you do not know. |
| `graft_build` | Build the graph again if its answers are old, for example after `git pull`. |
| `graft_blast` | See what a diff can break, before you commit or push it. |

### Build

| Tool | What it does |
|---|---|
| `container_build` | Build an image on the builder of the owner and push it to the Kloudlite registry. |
| `container_push` | Give an image a new tag in the registry. |
| `builder_status` | Check the state of the image builder for a team. |

### Workspaces

| Tool | What it does |
|---|---|
| `workspace_list` | List the workspaces of the person. You can give one team. |
| `workspace_get` | Read the state of one workspace. Poll it after any lifecycle call. |
| `workspace_create` | Create a workspace in a region. |
| `workspace_clone` | Clone the current state of a workspace into a new workspace. |
| `workspace_restore` | Create a workspace from a snapshot. |
| `workspace_start` | Start a stopped workspace. |
| `workspace_stop` | Stop a workspace that is up. The tool makes a snapshot first. |
| `workspace_delete` | Delete a workspace permanently. |
| `workspace_push` | Make a snapshot of the workspace in its history. |
| `worktree_add` | Make a new worktree in a workspace. |
| `worktree_drop` | Remove a worktree from a workspace. |

### Packages

| Tool | What it does |
|---|---|
| `packages_list` | List the packages that the workspace declares. Do this before you change them. |
| `packages_add` | Declare packages (`attr` or `attr@version`). An entry with the same `attr` replaces the old entry. |
| `packages_remove` | Remove declared packages. Give the `attr` of each package. |
| `packages_update` | Resolve the package pins again, to the newest matching versions. |

### Environments

| Tool | What it does |
|---|---|
| `env_list` | List the environments. You can give one team. |
| `env_get` | Read the services, state and intercepts of one environment. Poll `ready` of each service, not the spec. |
| `env_create` | Create an environment of services in a region. |
| `env_clone` | Copy an environment that is up into a new environment. |
| `env_start` | Start a stopped environment. |
| `env_stop` | Stop an environment that is up. |
| `env_delete` | Delete an environment permanently. |
| `env_push` | Make a snapshot of an environment in its history. |
| `env_restore` | Create an environment from a snapshot. |
| `env_restore_in_place` | Put an environment back to one of its own snapshots. |
| `service_add` | Add one service to an environment. The call fails if the name exists. |
| `service_update` | Replace one existing service. The tool matches it by name. |
| `service_remove` | Remove a service from an environment. |
| `service_logs` | Read the recent logs of one service. `previous` reads the last container that crashed. |
| `intercept` | Send the traffic of an environment service to a workspace. |
| `release` | Release an intercepted service back to its own pod. |

### Spaces

| Tool | What it does |
|---|---|
| `space_env_current` | Show the environment that each space (team) of the person follows. |
| `space_env_switch` | Make every workspace of a team follow a different environment. |
| `space_env_clear` | Detach the space of a team from its environment. |

### Storage

| Tool | What it does |
|---|---|
| `volume_list` | List the volumes. You can filter by kind and by owner. |
| `volume_history` | List the snapshots of a volume. These are the ids for restore. |
| `volume_refs` | Read the branch tips of a volume. |
| `volume_delete` | Delete a detached volume permanently. |
| `snapshot_delete` | Delete one snapshot of a volume. |

### Account

| Tool | What it does |
|---|---|
| `quota` | Show the limits and the current use of an owner. A call above a limit answers 409. |
| `requests_list` | List the requests of the person and of their teams, with state and decision note. |
| `request_get` | Show one request and its decision. |
| `request_create` | Ask a superadmin for quota, access, a region or anything else. One request of each kind can wait. |
| `regions` | List the regions where workspaces and environments can be created. |

### Web

| Tool | What it does |
|---|---|
| `web_fetch` | Read one URL. |
| `web_search` | Search the web. |

## Tools that the laptop does not use

These tools belong to the bench sessions. Do not use them: `workspace_ask`, `task_add`, `task_update`, `task_list`, `workspace_tasks`, `scratch_bash`, `scratch_read`, `scratch_write`.

## Team rules

1. If a limit or missing access stops the person, use `request_create`. Do not try the stopped call again.
2. Push a snapshot only when the person asks. Sync points already give recovery.
3. Intercept the shared team environment from your workspace. Do not make a private copy. Release the intercept when you are done.
4. Do not go into a different workspace to do work. If a call fails, report the failure as it is.

## Roles

**The main session** is this Claude Code session, not an agent. It talks to the person and owns the lifecycle of workspaces and environments. It does not read or judge code. It gives code work to `kl-workspace` agents and moves splits and reports between agents.

**`kl-workspace`** owns the code of one workspace: its design, working branch, packages, service and intercept. It plans, does its own parts and reviews the branches of subagents. It merges, tests and pushes. It cannot start agents, so it returns a split to main.

**`kl-subagent`** does one part in a clone that the main session made. It works on its own branch, commits, pushes and reports. It does not review or merge.
