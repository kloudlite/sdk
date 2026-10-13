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
