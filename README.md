# Kloudlite SDKs

One small client per language, no dependencies beyond each language's standard
library. All five share the same shape: a client taking the base URL and a
Bearer token (a session or CLI token; omit it for anonymous access to public
data), a `request` core that returns parsed JSON and raises an `ApiError`-style
error carrying the HTTP status and the server's message, and one module per
area of the platform: `code_repos`, `model_repos`, `data_repos`, `workspaces`,
`environments`, `sessions`, `sandboxes`, `pipelines` and `volumes` (`codeRepos`/`CodeRepos`
in the camel- and Pascal-cased languages).

## Every input is named

No call depends on argument order. Each language does it its own way:

| Language   | How                              | Example                                                                                   |
| ---------- | -------------------------------- | ----------------------------------------------------------------------------------------- |
| Python     | keyword-only arguments           | `kl.code_repos.open_pull(repo="acme/payments", title="Fix", head="fix", base="main")`     |
| Ruby       | keyword arguments                | `kl.code_repos.open_pull(repo: "acme/payments", title: "Fix", head: "fix", base: "main")` |
| TypeScript | one options object               | `kl.codeRepos.openPull({ repo: "acme/payments", title: "Fix", head: "fix", base: "main" })` |
| Go         | one request struct               | `kl.CodeRepos.OpenPull(kloudlite.OpenPull{Repo: "acme/payments", Title: "Fix", Head: "fix", Base: "main"})` |
| Java       | one fluent params object         | `kl.codeRepos().openPull(new Kloudlite.OpenPull().repo("acme/payments").title("Fix").head("fix").base("main"))` |

Go and Java have no named arguments, so a call whose ONLY input is the thing it
is about takes that one string (`kl.CodeRepos.Get("acme/payments")`) — a single
argument has no order to get wrong. Everything with two or more inputs goes
through the struct or params object.

The names are the same everywhere:

- A resource is passed under its own name: `repo`, `workspace`, `environment`,
  `session`, `volume`, `snapshot`, `run` — never `id` or `name`.
- **A repository is ONE `"owner/name"` string** (the owner being a person's
  handle or a team slug). **A workspace, environment or session is `"name"`
  (your own) or `"team/name"` (a team's)**, or its id.
- Something new made from something existing is `to`: `fork(repo=…, to=…)`,
  `clone(workspace=…, to=…)`, `restore(snapshot=…, to=…)`.
- A git URL to import from is `url`.

| Language   | Entry point           | Construct                                                  |
| ---------- | --------------------- | ---------------------------------------------------------- |
| Python     | `python/kloudlite.py` | `Kloudlite(base_url=…, token=…)`                           |
| TypeScript | `typescript/index.ts` | `new Kloudlite({ baseUrl, token })`                        |
| Ruby       | `ruby/kloudlite.rb`   | `Kloudlite::Client.new(base_url: …, token: …)`             |
| Go         | `go/kloudlite.go`     | `kloudlite.New(kloudlite.Options{BaseURL: …, Token: …})`   |
| Java       | `java/Kloudlite.java` | `new Kloudlite(new Kloudlite.Options().baseUrl(…).token(…))` |

The Java client returns the raw JSON string from every call — bring your own
mapper — except `sandboxes()` and `pipelines()`, which hand back live handles
that still expose their raw JSON; every other client returns decoded JSON. The examples below are Python;
the other languages use the same names.

## `code_repos`

Code repositories, over `/v1/repos` on the api tier:

```python
kl = Kloudlite(base_url="https://api.example.com", token="…")

kl.code_repos.create(repo="acme/payments", visibility="private", description="…")
kl.code_repos.fork(repo="acme/payments", to="me/payments")       # objects + refs, private by default
kl.code_repos.import_git(repo="acme/rails", url="https://github.com/rails/rails.git")
kl.code_repos.import_status(repo="acme/rails")   # 202 — a job: queued | running | done | failed
kl.code_repos.list(owner="acme")
kl.code_repos.get(repo="acme/payments")
kl.code_repos.update(repo="acme/payments", visibility="public")
kl.code_repos.delete(repo="acme/payments")

kl.code_repos.branches(repo="acme/payments")
kl.code_repos.delete_branch(repo="acme/payments", branch="old", oid="<head oid>")
kl.code_repos.protection(repo="acme/payments")
kl.code_repos.protect(repo="acme/payments", pattern="main")

pr = kl.code_repos.open_pull(repo="acme/payments", title="…", head="fix", base="main")
# from a fork: head names the fork's branch, proposed onto this repo's base
kl.code_repos.open_pull(repo="acme/payments", title="…", head="fix", base="main", head_repo="me/payments")
kl.code_repos.pulls(repo="acme/payments", state="open")
kl.code_repos.pull(repo="acme/payments", number=pr["number"])
kl.code_repos.comment(repo="acme/payments", number=pr["number"], body="LGTM")
kl.code_repos.merge(repo="acme/payments", number=pr["number"], strategy="squash")   # 202: a job
kl.code_repos.close(repo="acme/payments", number=pr["number"])

kl.code_repos.compare(repo="acme/payments", base="main", head="fix")
kl.code_repos.commit(repo="acme/payments", branch="main", message="…",
                     changes=[{"path": "a.txt", "contentBase64": "aGk="}])
kl.code_repos.signature(repo="acme/payments", sha="<commit sha>")
```

A cross-repo pull's diff is read with the mirror spelling:
`kl.code_repos.compare(repo="acme/payments", base="main", head="pull/7/head")`.

## `workspaces` and `volumes`

Btrfs-backed dev workspaces over `/v1/workspaces`; their snapshot history lives
on volumes (`/v1/volumes`). `push` is the one durable verb — it cuts a snapshot
that outlives the workspace and is a restore target forever:

```python
kl.quota()                                                # the ceiling, and what is used

kl.workspaces.create(workspace="acme/dev", quota_gb=40,
                     repo="acme/payments", branch="main",
                     packages=["nodejs@22", "ripgrep"])
kl.workspaces.list(team="acme")
kl.workspaces.set_ttl(workspace="acme/dev", ttl_secs=86400)   # delete after a day unused; None clears
kl.workspaces.stop(workspace="acme/dev")
kl.workspaces.start(workspace="acme/dev")

kl.workspaces.push(workspace="acme/dev", message="before the refactor")   # a kept snapshot
kl.workspaces.clone(workspace="acme/dev", to="dev-experiment", task="try the new parser")
vol = kl.workspaces.get(workspace="acme/dev")["volume"]   # set once it has pushed
snaps = kl.volumes.history(volume=vol)                    # restore targets, newest first
kl.workspaces.restore(snapshot=snaps[0]["id"], to="dev-again")

kl.workspaces.set_packages(workspace="acme/dev", packages=["nodejs@22", "ripgrep", "jq"])
kl.workspaces.update_packages(workspace="acme/dev")       # re-resolve pins

# The IDE tools: every workspace runs a tool server, and ide() binds a client
# to it — read/write/edit/patch/glob/grep, exec and detached processes, watches,
# and container builds. list() carries every schema.
ide = kl.workspaces.ide(workspace="acme/dev")   # inside the fleet (a bench); from a laptop:
                                                # `kl-connect ws ide dev`, then
                                                # Ide(url="http://127.0.0.1:7788", token=…)
ide.write(path="~/workspace/notes.md", content="# plan\n")
ide.edit(path="~/workspace/notes.md", edits=[{"old": "plan", "new": "steps"}])
ide.read(path="~/workspace/notes.md")
ide.grep(pattern="TODO", glob="**/*.rs")
job = ide.exec(cmd=["cargo", "test"], timeout_ms=600_000, tail=40)
proc = ide.exec(cmd="npm run dev", detach=True)          # → {"id"}; then:
ide.process_output(process=proc["id"])
ide.process_kill(process=proc["id"])

kl.workspaces.delete(workspace="acme/dev")                # snapshots survive on the volume
kl.volumes.delete_snapshot(volume=vol, snapshot=snaps[0]["id"])
```

## `environments`

Multi-service environments over `/v1/environments`: a set of containers sharing
one disk. A service needs only `name` and `image`; `ports`, `env`, `command`
and `mounts` (folders of the environment's disk) are optional:

```python
kl.environments.create(environment="acme/staging", quota_gb=20, services=[
    {"name": "db", "image": "postgres:16", "ports": [5432],
     "env": {"POSTGRES_PASSWORD": "dev"},
     "mounts": [{"folder": "pgdata", "path": "/var/lib/postgresql/data"}]},
    {"name": "api", "image": "ghcr.io/acme/api:1", "ports": [8080]},
])
kl.environments.list(owner="acme")
kl.environments.get(environment="acme/staging")
kl.environments.set_services(environment="acme/staging", services=[...])   # the whole list; data stays
kl.environments.stop(environment="acme/staging")
kl.environments.start(environment="acme/staging")

kl.environments.push(environment="acme/staging", message="seeded")         # a kept snapshot
kl.environments.clone(environment="acme/staging", to="acme/staging-2")     # a copy of the live disk
snaps = kl.volumes.history(volume=kl.environments.get(environment="acme/staging")["volume"])
kl.environments.restore(environment="acme/staging", snapshot=snaps[0]["id"])   # in place
kl.environments.create(environment="acme/staging-old", snapshot=snaps[0]["id"])  # a new one from it
kl.environments.set_ttl(environment="acme/staging", ttl_secs=86400)        # delete after a day unused; None clears
kl.environments.delete(environment="acme/staging")                         # snapshots survive
```

Java builds services the same way: `new Kloudlite.Service().name("db")
.image("postgres:16").port(5432).env("POSTGRES_PASSWORD", "dev")
.mount("pgdata", "/var/lib/postgresql/data")`.

## `sessions`

A working session is some of YOUR workspaces in one team, connected to one
environment. Many people's sessions connect to the same environment and
collaborate through it. An intercept a session takes is environment-wide —
the environment's own services and every other session reach your workspace —
and the session owns it: only it releases it, and removing the workspace or
deleting the session releases it. Workspaces are created with `workspaces` and
then added:

```python
kl.sessions.create(session="acme/fix-login", environment="acme/staging")  # environment is optional
kl.sessions.add(session="acme/fix-login", workspace="acme/api-dev")    # resolves staging by bare name
kl.sessions.add(session="acme/fix-login", workspace="acme/web-dev")
kl.sessions.intercept(session="acme/fix-login", service="api",
                      workspace="acme/api-dev", ports={8080: 3000})
kl.sessions.connect(session="acme/fix-login", environment="acme/qa")    # switch; None disconnects
kl.sessions.get(session="acme/fix-login")                               # its workspaces and intercepts
kl.sessions.release(session="acme/fix-login", service="api")
kl.sessions.remove(session="acme/fix-login", workspace="acme/web-dev")  # back to the team's choice
kl.sessions.set_ttl(session="acme/fix-login", ttl_secs=86400)           # delete after a day unused; None clears
kl.sessions.delete(session="acme/fix-login")                            # releases; deletes no workspace
```

`workspaces.create`, `environments.create` and `sessions.create` also take `ttl_secs` (600 to 2592000).
An awake session, and a workspace that wants a pod, is never deleted by its TTL.

A workspace is in at most one session; adding one that is in another session
is refused naming it. A session's environment is optional (`create` without
one, `connect` later); switching or disconnecting releases its intercepts in
the old one first, and deleting an environment disconnects its sessions.

## `sandboxes`

A sandbox is a short-lived, isolated container you send commands to: any image, its
own disk (empty, or a copy of one of your workspaces as it is right now, or of a
snapshot), no access to your keys or the cluster, and gone when you terminate it or
its timeout passes. Each command is a process with stdin, stdout and stderr:

```python
sb = kl.sandboxes.create(image="python:3.12", timeout_s=600)

p = sb.exec(cmd=["bash", "-c", "while read line; do echo $line; done"])
p.stdin.write(b"foo bar\n")
p.stdin.write_eof()
p.stdin.drain()
p.wait()                  # 0
p.stdout.read()           # "foo bar\n"
for line in p.stdout:     # or line by line, as it arrives
    ...
sb.terminate()

# Everything also has .aio:
sb = await kl.sandboxes.create.aio(image="python:3.12")
p = await sb.exec.aio(cmd=["bash", "-c", "while read line; do echo $line; done"])
await p.stdin.write.aio(b"foo bar\n")

# One command, start to finish (the sandbox is always terminated):
kl.sandboxes.run(cmd="cargo test", from_workspace="acme/api-dev", timeout_s=900)
# → {"exit_code": 0, "stdout": "...", "stderr": "...", "sandbox": "sbx-…"}
```

`create` takes `image`, `packages`, `from_workspace` or `from_snapshot`, `team`,
`session`, `timeout_s` (default 300), `idle_timeout_s`, `network` (`"internet"`, the default —
never the cluster — or `"none"`), `env`, `cpu`, `memory_gb` and `disk_gb`. A sandbox
also has `read(path=…)` and `write(path=…, content=…)` for files under `~/workspace`;
read anything you want to keep before terminating. A terminated sandbox's record
(`get`, `list`) says why it ended (`Terminated`, `Timeout`, `Idle`, `Exited`,
`Failed`, `Unplaceable`, `SourceGone`, `Lost`, `EnvironmentGone`) for 90 days.

A sandbox is a workspace, and like any workspace it reaches an environment only
through a session: `POST /v1/sandboxes` takes a `session` of yours, and a copy made
with `from_workspace` inherits none.

Per language: TypeScript is promise-based (`for await (const line of
p.stdout)`); Go has `Process.Stdin` as an `io.Writer` with `WriteEOF()`, and
`Stdout.ReadAll()` / `Stdout.Lines(fn)`; Ruby has `stdout.each_line`; Java has
`stdout().read()` and `stdout().lines()` (a `Stream<String>`). Python and Ruby
yield lines with their `\n`, like file iteration; TypeScript, Go and Java without.

## `pipelines`

CI pipelines are YAML files in an owner's `{owner}/pipelines` repository, run against
that owner's repositories with each job in its own sandbox. A run is recorded on the
repository it is about, so it is read back by that `repo`:

```python
kl.pipelines.list(owner="acme")    # every definition on acme/pipelines, with its validation errors

run = kl.pipelines.run(owner="acme", pipeline="ci", repo="acme/payments", ref="main",
                       inputs={"level": "full"}, clone="acme/staging")   # → a Run, "queued"
for chunk in run.follow(job="test (postgres16)"):   # a job by its key, or its index
    print(chunk, end="")
run.wait(timeout_s=1800)           # "success" | "failure" | "cancelled"
run.logs(job=0)                    # the whole log so far
run.cancel()

kl.pipelines.runs(repo="acme/payments", pipeline="ci", limit=10)   # newest first
kl.pipelines.get_run(repo="acme/payments", run="1760400000000-0a1b2c").refresh()

# Everything also has .aio; follow's twin is aio_follow:
run = await kl.pipelines.run.aio(owner="acme", pipeline="ci", repo="payments")
async for chunk in run.aio_follow(job=0):
    ...
```

`repo` on `run` is a name or `"owner/name"` (one of the pipeline owner's); `ref` defaults
to `main`. `environment="acme/staging"` runs every job that names no environment of its
own in that environment; `clone="acme/staging"` runs them in a copy of it made for the run
and deleted when it ends — pass one or the other. A pipeline that cannot run answers 422
with every reason in the error's message, and nothing is recorded. A `Run` carries the
record's fields (`id`, `number`, `state`, `jobs`, …; the wire's camelCase in the raw doc)
and `refresh`, `cancel`, `wait` (polls every 5 s by default), `logs` and `follow`, which
polls the job's log until the job has ended and every byte has arrived.

```ruby
run = kl.pipelines.run(owner: "acme", pipeline: "ci", repo: "acme/payments", clone: "acme/staging")
run.follow(job: "test (postgres16)") { |chunk| print chunk }   # no block: an Enumerator
run.wait(timeout_s: 1800)          # => "success"
```

```typescript
const run = await kl.pipelines.run({ owner: "acme", pipeline: "ci", repo: "acme/payments", clone: "acme/staging" });
for await (const chunk of run.follow({ job: "test (postgres16)" })) process.stdout.write(chunk);
await run.wait({ timeoutS: 1800 }); // "success"
```

```go
run, err := kl.Pipelines.Run(kloudlite.RunPipeline{Owner: "acme", Pipeline: "ci", Repo: "acme/payments", Clone: "acme/staging"})
err = run.Follow(kloudlite.FollowLogs{Job: "test (postgres16)"}, func(chunk string) bool {
	fmt.Print(chunk)
	return true // false stops following
})
state, err := run.Wait(kloudlite.WaitRun{Timeout: 30 * time.Minute})
```

```java
Kloudlite.Run run = kl.pipelines().run(new Kloudlite.RunPipeline()
    .owner("acme").pipeline("ci").repo("acme/payments").clone("acme/staging"));
try (Stream<String> log = run.follow("test (postgres16)")) { log.forEach(System.out::print); }
String state = run.waitFor(new Kloudlite.WaitRun().timeout(Duration.ofMinutes(30)));
```

Go names a job with `Job: 0` or `Job: "key"` (and `run.Logs(0)`); Java has `logs(int)` /
`logs(String)` and `follow(int)` / `follow(String)`, and `FollowLogs` / `WaitRun` to set
the poll interval.

## `model_repos` and `data_repos`

Model and dataset repositories live on the HUB listener (the Hugging Face Hub
API, `hub.<domain>`), so the client takes its URL separately. Management level
only — files move with git or `huggingface_hub` against the same hub URL:

```python
kl = Kloudlite(base_url="https://api.example.com", token="…", hub_url="https://hub.example.com")

kl.model_repos.create(repo="acme/bert-finetune")             # private by default
kl.model_repos.get(repo="acme/bert-finetune")
kl.model_repos.refs(repo="acme/bert-finetune")               # branches and tags
kl.model_repos.set_private(repo="acme/bert-finetune", private=False)
kl.model_repos.delete(repo="acme/bert-finetune")

kl.data_repos.create(repo="acme/support-tickets")            # same verbs, dataset kind

kl.model_repos.import_git(repo="acme/bert", url="https://huggingface.co/google-bert/bert-base-uncased")
kl.model_repos.import_status(repo="acme/bert")   # 202 — a job; large files included (git-lfs)
kl.data_repos.import_git(repo="acme/squad", url="https://huggingface.co/datasets/rajpurkar/squad")
```

Each SDK directory carries a runnable `example` file walking the whole fork
workflow: fork, commit onto a new branch of the fork, open the change back
upstream with `head_repo`, review the mirror diff, merge.
