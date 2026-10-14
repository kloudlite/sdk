# Kloudlite SDKs

One small client per language, no dependencies beyond each language's standard
library. All five share the same shape: a client taking the base URL and a
Bearer token (a session or CLI token; omit it for anonymous access to public
data), a `request` core that returns parsed JSON and raises an `ApiError`-style
error carrying the HTTP status and the server's message, and one module per
area of the platform: `code_repos`, `model_repos` and `data_repos`
(`codeRepos`/`CodeRepos` in the camel- and Pascal-cased languages).

**A repository is always named by ONE `"owner/name"` string** — the owner half
being a person's handle or a team slug — wherever it appears: the repo a call
is about, a fork's destination, a pull's `head_repo`. The only bare handle in
the API is the owner-scoped listing (`list("acme")`).

| Language   | Entry point           | Construct                                     |
| ---------- | --------------------- | --------------------------------------------- |
| Python     | `python/kloudlite.py` | `Kloudlite(base_url, token=None)`             |
| TypeScript | `typescript/index.ts` | `new Kloudlite({ baseUrl, token? })`          |
| Ruby       | `ruby/kloudlite.rb`   | `Kloudlite::Client.new(base_url, token: nil)` |
| Go         | `go/kloudlite.go`     | `kloudlite.New(baseURL, token)`               |
| Java       | `java/Kloudlite.java` | `new Kloudlite(baseUrl, token)`               |

The Java client returns the raw JSON string from every call — bring your own
mapper; every other client returns decoded JSON.

## The `code_repos` module

Code repositories, over `/v1/repos` on the api tier. Python spelling; the other
languages differ only in casing (`openPull` / `OpenPull`):

```python
kl = Kloudlite("https://api.example.com", token="…")

kl.code_repos.create("acme/payments", visibility="private", description="…")
kl.code_repos.fork("acme/payments", "me/payments")   # objects + refs, private by default
kl.code_repos.import_git("acme/rails", "https://github.com/rails/rails.git")
kl.code_repos.import_status("acme/rails")   # 202 — a job: queued | running | done | failed
kl.code_repos.list("acme")
kl.code_repos.get("acme/payments")
kl.code_repos.update("acme/payments", visibility="public")
kl.code_repos.delete("acme/payments")

kl.code_repos.branches("acme/payments")
kl.code_repos.delete_branch("acme/payments", "old", oid="<head oid>")
kl.code_repos.protection("acme/payments")
kl.code_repos.protect("acme/payments", pattern="main")

pr = kl.code_repos.open_pull("acme/payments", title="…", head="fix", base="main")
# from a fork: head names the fork's branch, proposed onto this repo's base
kl.code_repos.open_pull("acme/payments", title="…", head="fix", base="main", head_repo="me/payments")
kl.code_repos.pulls("acme/payments", state="open")
kl.code_repos.pull("acme/payments", pr["number"])
kl.code_repos.comment("acme/payments", pr["number"], "LGTM")
kl.code_repos.merge("acme/payments", pr["number"], strategy="squash")   # 202: a job
kl.code_repos.close("acme/payments", pr["number"])

kl.code_repos.compare("acme/payments", base="main", head="fix")
kl.code_repos.commit("acme/payments", branch="main", message="…",
                changes=[{"path": "a.txt", "contentBase64": "aGk="}])
kl.code_repos.signature("acme/payments", "<commit sha>")
```

A cross-repo pull's diff is read with the mirror spelling:
`kl.code_repos.compare("acme/payments", base="main", head="pull/7/head")`.

## The `workspaces` and `volumes` modules

Btrfs-backed dev workspaces over `/v1/workspaces`; their snapshot history lives
on volumes (`/v1/volumes`). `push` is the one durable verb — it cuts a snapshot
that outlives the workspace and is a restore target forever:

```python
kl.quota()                                           # the ceiling, and what is used

ws = kl.workspaces.create(name="dev", quota_gb=40,
                          repo="acme/payments", branch="main",
                          packages=["nodejs@22", "ripgrep"])
wid = ws["id"]                                       # "ws-…": every call takes the id, not the name
kl.workspaces.list(team="acme")
kl.workspaces.stop(wid); kl.workspaces.start(wid)

kl.workspaces.push(wid, message="before the refactor")    # a kept snapshot
kl.workspaces.clone(wid, "dev-experiment", task="try the new parser")
vol = kl.workspaces.get(wid)["volume"]                    # set once the workspace has a volume
snaps = kl.volumes.history(vol)                           # restore targets, newest first
kl.workspaces.restore(name="dev-again", snapshot_id=snaps[0]["id"])

kl.workspaces.set_packages(wid, ["nodejs@22", "ripgrep", "jq"])

# The IDE tools: every workspace runs a tool server, and ide() binds a client
# to it — read/write/edit/patch/glob/grep, exec and detached processes, watches,
# and container builds. list() carries every schema.
ide = kl.workspaces.ide(wid)        # inside the fleet (a bench); from a laptop:
                                    # `kl-connect ws ide dev` then Ide("http://127.0.0.1:7788", token)
ide.write("~/workspace/notes.md", "# plan\n")
ide.edit(path="~/workspace/notes.md", edits=[{"old": "plan", "new": "steps"}])
ide.read(path="~/workspace/notes.md")
ide.grep("TODO", glob="**/*.rs")
job = ide.exec(["cargo", "test"], timeout_ms=600_000, tail=40)
proc = ide.exec("npm run dev", detach=True)          # → {"id"}; then:
ide.process_output(proc["id"]); ide.process_kill(proc["id"])
kl.workspaces.update_packages(wid)                         # re-resolve pins
kl.workspaces.delete(wid)                                  # snapshots survive on the volume
kl.volumes.delete_snapshot(vol, snaps[0]["id"])
```

## The `environments` module

Multi-service environments over `/v1/environments`: a set of containers sharing
one disk. An environment is named by ONE string — `"name"` for your own,
`"team/name"` for a team's — or by its id (`env-…`). A service needs only
`name` and `image`; `ports`, `env`, `command` and `mounts` (folders of the
environment's disk) are optional:

```python
kl.environments.create("acme/staging", quota_gb=20, services=[
    {"name": "db", "image": "postgres:16", "ports": [5432],
     "env": {"POSTGRES_PASSWORD": "dev"},
     "mounts": [{"folder": "pgdata", "path": "/var/lib/postgresql/data"}]},
    {"name": "api", "image": "ghcr.io/acme/api:1", "ports": [8080]},
])
kl.environments.list(owner="acme")
kl.environments.get("acme/staging")
kl.environments.set_services("acme/staging", [...])     # the whole list; data folders stay
kl.environments.stop("acme/staging"); kl.environments.start("acme/staging")
kl.environments.hold("acme/staging")                    # reset the idle clock

kl.environments.push("acme/staging", message="seeded")  # a kept snapshot
kl.environments.clone("acme/staging", "acme/staging-2") # a copy of the live disk
snaps = kl.volumes.history(kl.environments.get("acme/staging")["volume"])
kl.environments.restore("acme/staging", snaps[0]["id"]) # in place: services and data
kl.environments.create("acme/staging-old", snapshot_id=snaps[0]["id"])  # a new one from it
kl.environments.delete("acme/staging")                  # snapshots survive on the volume
```

Java builds services with `new Kloudlite.Service("db", "postgres:16").port(5432)
.env("POSTGRES_PASSWORD", "dev").mount("pgdata", "/var/lib/postgresql/data")`.

## The `sessions` module

A working session is some of YOUR workspaces in one team, connected to one
environment. Many people's sessions connect to the same environment and
collaborate through it. An intercept a session takes is environment-wide —
the environment's own services and every other session reach your workspace —
and the session owns it: only it releases it, and removing the workspace or
deleting the session releases it. A session is named like an environment
(`"name"` or `"team/name"`); workspaces are created with `workspaces` and then
added:

```python
kl.sessions.create("acme/fix-login", environment="acme/staging")
kl.sessions.add("acme/fix-login", "acme/api-dev")      # resolves staging's services by bare name
kl.sessions.add("acme/fix-login", "acme/web-dev")
kl.sessions.intercept("acme/fix-login", "api", "acme/api-dev", {8080: 3000})
kl.sessions.get("acme/fix-login")                      # its workspaces and intercepts
kl.sessions.release("acme/fix-login", "api")
kl.sessions.remove("acme/fix-login", "acme/web-dev")   # back to the team's choice
kl.sessions.delete("acme/fix-login")                   # releases, lets go; deletes no workspace
```

A workspace is in at most one session; adding one that is in another session
is refused naming it.

## The `model_repos` and `data_repos` modules

Model and dataset repositories live on the HUB listener (the Hugging Face Hub
API, `hub.<domain>`), so the client takes its URL separately. Management level
only — files move with git or `huggingface_hub` against the same hub URL:

```python
kl = Kloudlite("https://api.example.com", token="…", hub_url="https://hub.example.com")

kl.model_repos.create("acme/bert-finetune")             # private by default
kl.model_repos.get("acme/bert-finetune")
kl.model_repos.refs("acme/bert-finetune")               # branches and tags
kl.model_repos.set_private("acme/bert-finetune", False)
kl.model_repos.delete("acme/bert-finetune")

kl.data_repos.create("acme/support-tickets")         # same five verbs, dataset kind

kl.model_repos.import_git("acme/bert", "https://huggingface.co/google-bert/bert-base-uncased")
kl.model_repos.import_status("acme/bert")   # 202 — a job; large files included (git-lfs)
kl.data_repos.import_git("acme/squad", "https://huggingface.co/datasets/rajpurkar/squad")
```

Each SDK directory carries a runnable `example` file walking the whole fork
workflow: fork, commit onto a new branch of the fork, open the change back
upstream with `head_repo`, review the mirror diff, merge.
