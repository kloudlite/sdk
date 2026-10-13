# Kloudlite SDKs

One small client per language, no dependencies beyond each language's standard
library. All five share the same shape: a client taking the base URL and a
Bearer token (a session or CLI token; omit it for anonymous access to public
data), a `request` core that returns parsed JSON and raises an `ApiError`-style
error carrying the HTTP status and the server's message, and one module per
area of the platform — `repos` is the first.

| Language   | Entry point           | Construct                                              |
| ---------- | --------------------- | ------------------------------------------------------ |
| Python     | `python/kloudlite.py` | `Kloudlite(base_url, token=None)`                      |
| TypeScript | `typescript/index.ts` | `new Kloudlite({ baseUrl, token? })`                   |
| Ruby       | `ruby/kloudlite.rb`   | `Kloudlite::Client.new(base_url, token: nil)`          |
| Go         | `go/kloudlite.go`     | `kloudlite.New(baseURL, token)`                        |
| Java       | `java/Kloudlite.java` | `new Kloudlite(baseUrl, token)`                        |

The Java client returns the raw JSON string from every call — bring your own
mapper; every other client returns decoded JSON.

## The `repos` module

Code repositories, over `/v1/repos` on the api tier. Python spelling; the other
languages differ only in casing (`openPull` / `OpenPull`):

```python
kl = Kloudlite("https://api.example.com", token="…")

kl.repos.create(owner="acme", name="payments", visibility="private", description="…")
kl.repos.list("acme")
kl.repos.get("acme", "payments")
kl.repos.update("acme", "payments", visibility="public")
kl.repos.delete("acme", "payments")

kl.repos.branches("acme", "payments")
kl.repos.delete_branch("acme", "payments", "old", oid="<head oid>")
kl.repos.protection("acme", "payments")
kl.repos.protect("acme", "payments", pattern="main")

pr = kl.repos.open_pull("acme", "payments", title="…", head="fix", base="main")
kl.repos.pulls("acme", "payments", state="open")
kl.repos.pull("acme", "payments", pr["number"])
kl.repos.comment("acme", "payments", pr["number"], "LGTM")
kl.repos.merge("acme", "payments", pr["number"], strategy="squash")   # 202: a job
kl.repos.close("acme", "payments", pr["number"])

kl.repos.compare("acme", "payments", base="main", head="fix")
kl.repos.commit("acme", "payments", branch="main", message="…",
                changes=[{"path": "a.txt", "contentBase64": "aGk="}])
kl.repos.signature("acme", "payments", "<commit sha>")
```

Not there yet: **fork** — the server has no user-facing fork endpoint; the
module grows a `fork` method the day one exists.
