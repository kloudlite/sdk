---
name: kloudlite
description: You run on Kloudlite from the laptop through the kl tools. In a request, the words "workspace", "environment", "service" and "snapshot" mean Kloudlite workspaces, environments, services and snapshots. Read this skill before any task that creates, changes, stops, deletes or lists a workspace or environment, or that works in a workspace. Also read it before a task about intercept, push, restore, clone, package, service or build. It gives the concepts, the tools, the team rules and the roles. Your role (main session, kl-workspace or kl-subagent) has its own text. Read the two.
---

# Kloudlite

This skill is written in ASD-STE100 Simplified Technical English. Each word has one meaning. Each instruction is one sentence.

Kloudlite gives each person dev machines in the cloud (workspaces) and shared stacks that run (environments). All of them are on one cluster. You control Kloudlite with the kl tools (`mcp__plugin_kloudlite_kl__<name>`). In kl mode, no local tool works. Nothing runs on the laptop.

The request "delete all workspaces" means Kloudlite workspaces. Only the main session creates and deletes workspaces. To do this, list the workspaces with `workspace_list`. Then delete them, and obey the rule for destructive actions in "Team rules".

## Three roles

Each session has one role. The `orchestrate` skill gives the role of the main session. The agent file of `kl-workspace` and `kl-subagent` gives their roles. This skill gives the shared concepts.

| Role | Where it runs | What it does |
|---|---|---|
| **Main** | This Claude Code session. It has no workspace and does not read code. | Talks to the person. Creates, clones, starts and deletes workspaces and environments. Gives each task to the `kl-workspace` agent of the workspace. Moves facts between agents. |
| **kl-workspace** | An agent. It works in the pod of one workspace, in `~/workspace`. | Owns one component: its design, code, packages, service, intercept and working branch. Its last message is its report to main (`DONE`, `BLOCKED` or `NEED`). Can ask for subagents for independent parts of its task (`SUBAGENT`). |
| **kl-subagent** | An agent. It works in a temporary clone of one workspace, in `~/workspace`. | Does one task that its workspace gave. Commits, and resolves each conflict with the branch of the workspace. Talks only to its workspace, through main. Its last message is its report. |

Each session is isolated. It works only in its own workspace. A session does not read, run or change code in a different workspace.

Work moves between sessions only in these ways:

- Main gives work to a `kl-workspace` agent with `Agent` (first time) or `SendMessage` (after that).
- The last message of a `kl-workspace` agent is its report to main.
- A `kl-workspace` agent asks for subagents with `SUBAGENT`. Main makes the clones and starts the `kl-subagent` agents. Main gives the report of each subagent to its workspace, without a change.

Workspaces do not send messages to other workspaces. Main moves facts between them. A subagent does not send messages to main for itself. Main only carries its report.

## Ids and names

Each workspace has a name and an id. The id starts with `ws-`. Each environment has a name and an id. The id starts with `env-`.

1. Get the id from `workspace_list` or `env_list`.
2. Use the id in the `workspace` or `env` parameter of a tool call. Do not use the name.
3. Use the name when you write to the person.

**Every pod tool takes `workspace`.** The code, run, graph, package and build tools work in the pod of the workspace that you give in `workspace`. Always give it. Give only your own workspace. The workspace must be `ready`.

## Concepts

**Workspace.** One dev machine. It is a pod with these items:

- Its own home, `/home/kl`. This is a btrfs volume.
- Source code in `~/workspace`.
- Its own installed packages.

Use one workspace for each component, for example frontend, backend, worker or test suite. Components can come from different repos or from the same repo. Each type of component usually needs different packages.

**Worktree.** An extra working copy in a workspace. It gives a second branch without a second workspace. Use `worktree_add` and `worktree_drop`.

**Environment.** A shared stack of services for a team. Example: the backend, its database and a queue. Each service runs as its own StatefulSet. Use `service_add`, `service_update` and `service_remove` to add, change and remove services.

**Packages are development tools. Services go in the environment.** This rule is important. Obey it:

- Packages install the tools that you use to develop: compilers, runtimes, language servers, build tools, test tools and CLIs. Examples: `go`, `nodejs`, `python3`, `rustc`, `protobuf`, `postgresql` for the `psql` client, `mongosh`.
- Do not install a server as a package to run it. Examples of servers: MongoDB, PostgreSQL, MySQL, Redis, Kafka, RabbitMQ, Elasticsearch, MinIO.
- Do not download a server binary or a container image into a workspace, and do not run it with `exec`.
- A service that your code needs is a service of the environment. Add it with `service_add` and its public image, for example `mongo:7`. Give it a mount for its data (refer to "Service data").
- Your workspace gets to the service by its DNS name through the space (refer to "Space"). Use `env_get` to get the service name and its port.
- The only process that you run in a workspace is the code of your own component, for example its dev server, its tests or its service under an intercept.
- If the service is missing and you cannot add it, report `NEED` with the service, its image and its port. Do not run a copy of it in the workspace.

**Service data.** A service with no `mounts` loses everything it writes each time its pod restarts. It also loses it when the environment stops, restarts or moves. Before each `service_add` or `service_update`, check the service for data that must persist:

1. Ask what the service writes to disk and whether that data must still be there after a restart. Records, uploads, queues, indexes and sessions must persist. Temporary files and build output need not.
2. Find each directory that holds such data. For a public image, read its documentation or its Dockerfile `VOLUME` lines. For an image that you build, read the code: find where it writes files and the paths in its config and env.
3. Give each directory a mount in the same call that creates the service. Never add the mount later: the data that the service writes before the mount is lost.
4. If the service keeps all its data in another service, for example an API that stores its data in a database, it needs no mount. State this in your report.

- A mount has two parts. `folder` is one name, for example `mongo-data`. It names a folder in the environment's own volume, so it is saved in every snapshot. `path` is the directory in the container.
- Example: `{"name": "mongodb", "image": "mongo:7", "ports": [27017], "mounts": [{"folder": "mongo-data", "path": "/data/db"}]}`.
- If you find a service that persists data and has no mount, add the mount with `service_update` and report it.

**Service image.** When you add a service, you select its image. For standard software, use a public image. For code that you own, do not build an image while you develop. Run the code in your workspace. Then `intercept` the service (refer to "Intercept"). Build an image only when the person tells you to publish it to the registry. Then use the `container_build` tool:

- Give `tags: ["name:tag"]`.
- The build context is your working directory.
- The tool builds on your builder and pushes the image to the Kloudlite registry.

To give an image a new tag, use `container_push`. The `kl` command is not on the PATH in `exec`. These tools run `kl` for you.

**Intercept.** An intercept sends the traffic for one service of an environment to a workspace. The service pod does not get this traffic. The code that runs in your workspace gets it.

- `intercept` starts an intercept. `release` ends it.
- Only one workspace at a time can intercept a service.
- If the workspace stops, the platform releases the intercept after a short time. But the request for the intercept stays until `release`.
- `env_get` shows each intercept and the workspace that holds it.
- Use an intercept to test a change to a service in the real environment, without a build and without a deploy. This is the short dev loop.

**Space.** A space follows one environment. Then the workspaces of the team can get to the services of that environment by their DNS names. Use `space_env_current`, `space_env_switch` and `space_env_clear`.

**Snapshots.** A snapshot is a read-only copy of the disk of a workspace or environment.

- *Sync points*: The platform makes them automatically while a workspace runs. They are only for recovery after a crash. They do not show in the history.
- *Push* (`workspace_push`, `env_push`): Records a named snapshot in the history. The platform does not remove pushes. This push is not `git push`.
- *Restore* (`env_restore`): Puts THIS environment back in time, services and data. Data written since the snapshot is lost. If the reply is 409 with `lost`, ask the person, then repeat with `discard`. `workspace_restore` still makes a new workspace from a snapshot.
- *Clone* (`workspace_clone`, `env_clone`): Makes a NEW workspace or environment. `env_clone` takes `snapshot_id` to copy a snapshot instead of the live state. The `based_on` field in the reply tells you the age of the copy.
- `volume_list`, `volume_history` and `volume_refs` read the history. `snapshot_delete` and `volume_delete` remove history.

**Interrupted workspace.** If the node of a workspace stops, `workspace_start` answers 409. Then clone the workspace. The clone starts from the last sync point.

**Packages (the tools).** Nix packages with a version: `name@version`. The version is `latest`, `N`, `N.N` or `N.N.N`.

- Use `packages_list`, `packages_add` and `packages_remove`.
- `packages_update` finds newer versions.
- If a version is not in the cache, the answer is 422. The answer gives the versions that are in the cache.

**Builders.** Each owner has a hidden builder. It starts when `container_build` needs it. `builder_status` shows it.

**Quota and regions.**

- `quota` shows the resources that the owner can still use: workspaces, environments, snapshots, disk, cpu and memory.
- A call that goes above the quota answers 409.
- `regions` gives the regions where workspaces can run.
- If a limit or missing access stops the person, use `request_create` to ask a superadmin. The kinds are quota, access, region and other. Only one request of each kind can wait at a time.
- `requests_list` and `request_get` show the decision.
- Do not try the stopped call again. It will not pass.

**Lifecycle calls are asynchronous.** Create, start, stop, clone and restore return immediately. The work continues after the return. Poll `workspace_get` or `env_get` until you get the state that you want.

| Object | States | The state when it is up |
|---|---|---|
| Workspace | `creating`, `ready`, `stopped`, `error`, `deleted` | `ready`. A workspace is not `running` at any time. |
| Environment | `creating`, `running`, `stopped`, `error`, `deleted` | `running` |

Stop the poll when the state is `error` or `deleted`. That state will not change to the state that you want.

## Tools

Each role has a different set of tools. Its role text gives its tools. If a tool is not in your list, it is the work of a different role.

| Group | Tools |
|---|---|
| Code | `read`, `write`, `edit`, `patch`, `glob`, `grep` |
| Run | `exec` (with `detach: true` for a process that continues), `process_list`, `process_output`, `process_write`, `process_kill`, `watch`, `watch_poll`, `watch_stop` |
| Graph (use it first to find code) | `graft_find_code`, `graft_find_all`, `graft_trace_calls`, `graft_file_api`, `graft_repo_map`, `graft_build`, `graft_blast` |
| Build | `container_build`, `container_push`, `builder_status` |
| Workspaces | `workspace_list`, `workspace_get`, `workspace_create`, `workspace_clone`, `workspace_restore`, `workspace_start`, `workspace_stop`, `workspace_delete`, `workspace_push`, `worktree_add`, `worktree_drop` |
| Packages | `packages_list`, `packages_add`, `packages_remove`, `packages_update` |
| Environments | `env_list`, `env_get`, `env_create`, `env_clone`, `env_start`, `env_stop`, `env_delete`, `env_push`, `env_restore`, `service_add`, `service_update`, `service_remove`, `service_logs`, `intercept`, `release` |
| Spaces | `space_env_current`, `space_env_switch`, `space_env_clear` |
| Repos | `repo_list`, `repo_get`, `repo_create`, `repo_update`, `repo_delete`, `protection_get`, `protection_set`, `branch_list`, `branch_delete`, `compare` |
| Pull requests | `pr_list`, `pr_get`, `pr_create`, `pr_comment`, `pr_merge`, `pr_close` |
| Images | `image_list`, `image_tags`, `image_visibility`, `image_tag_delete`, `image_delete` |
| Storage | `volume_list`, `volume_history`, `volume_refs`, `volume_delete`, `snapshot_delete` |
| Account | `quota`, `regions`, `requests_list`, `request_get`, `request_create` |
| Web | `web_fetch`, `web_search` |

These kl tools belong to the bench sessions. Do not use them from the laptop: `workspace_ask`, `main_tell`, `subagent`, `task_add`, `task_update`, `task_list`, `workspace_tasks`. Main uses `scratch_read` and `scratch_write` only for its contract files (`contract-<feature>.md`). Main does not use `scratch_bash`.

## Repos and images

Every role has the repo, pull request and image tools. Each tool needs `owner`. Read `owner` from `workspace_get` or ask the person.

1. Before you push code, run `git remote -v`.
2. If it shows no `origin`, link a repo with `git remote add origin kl:<owner>/<repo>`.
3. If the repo does not exist, create it first with `repo_create`.
4. A repo can hold the code of more than one workspace. Each workspace uses its own branch.
5. To merge work, open a pull request with `pr_create`.
6. Merge it with `pr_merge` only when the person or the main session tells you to.
7. `pr_merge` returns 202 at once. Poll `pr_get` until the merge state is final.

The tools `repo_delete`, `branch_delete`, `image_tag_delete` and `image_delete` delete data that cannot be recovered. Before you call one, get a yes from the person. Main asks with `AskUserQuestion`. Name exactly what the call deletes. A `kl-workspace` or `kl-subagent` agent cannot ask the person. It stops and reports `NEED` with the exact target. Main asks the person and passes the answer back.

## Team rules

These are the rules of the team. Obey them.

**Destructive actions** (delete, stop, restore in place, and `repo_delete`, `branch_delete`, `image_tag_delete`, `image_delete`):

1. List the exact targets by name.
2. Ask the person in one line, unless the person asked for this action and gave the target in this turn.
3. Call the tool with their ids.

**No backdoors.** Do not go into a different workspace, pod or session to do work. If a delegation or a push fails, tell the person about the failure as it is. Do not go around it.

**Push a snapshot only when the person asks.** The sync points already give recovery after a crash.

**Environments.** Intercept the shared team environment from your workspace. Do not make a private copy of the environment. Release the intercept when you are done.

**Packages are tools, services are in the environment.** Refer to the concept above. Never run a database or a different server inside a workspace.

**Packages and setup.** At the start, each workspace reads the Setup section of `AGENTS.md` in the repo. Then it installs the missing packages that this section gives. When you install a new package, add it to `AGENTS.md`. The setup requirements of a repo are in that repo.

**Service images.** For standard software, use a public image. For code that you own, do not build an image for each change. Use an intercept while you develop. Use `container_build` and `container_push` only when the person tells you to publish an image.

**Refused calls.** If the person refuses a call, that is their answer. Do not try the call again. Do not get the same result in a different way.
