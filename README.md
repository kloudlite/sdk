# Kloudlite SDKs

Small clients for the Kloudlite API in Go, Python, TypeScript, Ruby and Java.
Each has no dependencies beyond its language's standard library. Every client
takes the API base URL and an optional Bearer token (omit it for anonymous
access to public data), returns decoded JSON (the Java client returns the raw
JSON string) and raises an error carrying the HTTP status and the server's
message.

A capsule is a disposable container that runs one command or one shell and is deleted when it exits. Pipeline jobs run in capsules.

The full manual is at <https://dev.kloudlite.io/docs>.

## Install

| Language   | How                                                                 |
| ---------- | ------------------------------------------------------------------- |
| Go         | `go get github.com/kloudlite/sdk/go`                                |
| Python     | copy `python/kloudlite.py` into your project                        |
| TypeScript | copy `typescript/index.ts` into your project (no npm package yet)   |
| Ruby       | copy `ruby/kloudlite.rb` into your project                          |
| Java       | copy `java/Kloudlite.java` into your project                        |

## Every input is named

No call depends on argument order: keyword arguments in Python and Ruby, one
options object in TypeScript, one request struct in Go, one fluent params
object in Java. A workspace,
environment or desk is `"name"` (your own) or `"team/name"`.

## Example

List your workspaces.

Go

```go
kl := kloudlite.New(kloudlite.Options{BaseURL: "https://dev.kloudlite.io", Token: "…"})
workspaces, err := kl.Workspaces.List("")
```

Python

```python
from kloudlite import Kloudlite
kl = Kloudlite(base_url="https://dev.kloudlite.io", token="…")
workspaces = kl.workspaces.list()
```

TypeScript

```ts
import { Kloudlite } from "./index.ts";
const kl = new Kloudlite({ baseUrl: "https://dev.kloudlite.io", token: "…" });
const workspaces = await kl.workspaces.list();
```

Ruby

```ruby
require_relative "kloudlite"
kl = Kloudlite::Client.new(base_url: "https://dev.kloudlite.io", token: "…")
workspaces = kl.workspaces.list
```

Java

```java
Kloudlite kl = new Kloudlite(new Kloudlite.Options().baseUrl("https://dev.kloudlite.io").token("…"));
String workspaces = kl.workspaces().list(null);
```

## Snapshots and tags

A snapshot push is a storage commit of the workspace's or environment's volume;
its number is the snapshot's version (`v4`). A tag names one snapshot and moves
only when asked: a push with `tag` puts that tag on the new snapshot, moving it
from wherever it was, and a push without one is untagged (the answer carries
`tag`). A reference is `name` (its snapshot tagged `base`, else its newest),
`name@N`, `name@<tag>` or a snapshot id. Each client also describes a volume
(description and labels), lists volumes by label, and puts, moves or removes a
tag.

```go
kl.Workspaces.TakeSnapshot(kloudlite.TakeWorkspaceSnapshot{Workspace: "api", Tag: "wip-auth", Labels: []string{"auth"}})
kl.Volumes.SetTag(kloudlite.SetTag{Owner: "acme", Name: "api-home", Tag: "base", Version: 4})
kl.Volumes.DeleteTag(kloudlite.DeleteTag{Owner: "acme", Name: "api-home", Tag: "wip-auth"})
```

## License

Apache-2.0, see [LICENSE](LICENSE).
