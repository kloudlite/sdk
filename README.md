# Kloudlite SDKs

Small clients for the Kloudlite API in Go, Python, TypeScript, Ruby and Java.
Each has no dependencies beyond its language's standard library. Every client
takes the API base URL and an optional Bearer token (omit it for anonymous
access to public data), returns decoded JSON (the Java client returns the raw
JSON string) and raises an error carrying the HTTP status and the server's
message.

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
environment or session is `"name"` (your own) or `"team/name"`.

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

## License

Apache-2.0, see [LICENSE](LICENSE).
