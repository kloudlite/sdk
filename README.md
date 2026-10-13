# Kloudlite SDKs

One small client per language, no dependencies beyond each language's standard
library. All five share the same shape: a constructor taking the base URL plus
optional credentials, a `request` core that returns parsed JSON and raises an
`ApiError`-style error carrying the HTTP status and the server's message, and —
coming next — named methods over the code-repository (browse) API.

Auth is HTTP Basic: the username names the owner the token belongs to (git's
`x` placeholder also works), the token is the secret. No credentials is a valid
client — public repositories admit anonymous reads.

| Language   | Entry point              | Construct                                                  |
| ---------- | ------------------------ | ---------------------------------------------------------- |
| Python     | `python/kloudlite.py`    | `Kloudlite(base_url, username="x", token=None)`            |
| TypeScript | `typescript/index.ts`    | `new Kloudlite({ baseUrl, username?, token? })`            |
| Ruby       | `ruby/kloudlite.rb`      | `Kloudlite::Client.new(base_url, username: "x", token: nil)` |
| Go         | `go/kloudlite.go`        | `kloudlite.New(baseURL, username, token)`                  |
| Java       | `java/Kloudlite.java`    | `new Kloudlite(baseUrl, username, token)`                  |

The Java client returns the raw JSON string from `request` — bring your own
mapper; every other client returns decoded JSON.
