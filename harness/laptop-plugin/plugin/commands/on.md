---
description: Turn kl mode on. Only the kl tools work; nothing runs on this laptop.
---
kl mode is on. From now on, do all work through the `mcp__plugin_kloudlite_kl__*` tools. Local tools are refused. Load the `kloudlite:kloudlite`, `kloudlite:orchestrate` and `kloudlite:develop` skills now. Then:

1. Call `workspace_list`.
2. For each workspace whose state is `ready`, spawn one `kl-workspace` agent in the background. Give the agent the name of the workspace. The prompt is: `Workspace <name> (<id>). No goal yet.` Spawn them all in one message.
3. Do not start a stopped workspace. Do not wait for the agents.
4. Tell the person in one line that kl mode is on, and name the workspaces that have an agent.
