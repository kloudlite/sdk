---
description: Turn kl mode on. Nothing runs, reads or writes on this laptop; work goes through the kl tools.
---
kl mode is on. From now on, do all work through the `mcp__plugin_kloudlite_kl__*` tools. The tools that run, read or write on this laptop are refused. Other tools, such as Agent, Artifact and the web tools, work. Load the `kloudlite:kloudlite`, `kloudlite:orchestrate` and `kloudlite:develop` skills now. Then:

1. Call `workspace_list`.
2. For each workspace whose state is `ready`, spawn one `kl-workspace` agent in the background. Give the agent the name of the workspace. The prompt is: `Workspace <name> (<id>). No goal yet.` Spawn them all in one message.
3. For each workspace whose state is `ready`, call `process_list`. For each process whose state is `running`, start a follow (refer to "Show running processes" in the `kloudlite:orchestrate` skill).
4. Do not start a stopped workspace. Do not wait for the agents.
5. Tell the person in one line that kl mode is on, and name the workspaces that have an agent.
