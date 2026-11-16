# Kloudlite SDK for Ruby

```sh
gem install kloudlite-sdk
```

```ruby
require "kloudlite"

kl = Kloudlite::Client.new # uses the credentials of `kloudlite login`
```

A snapshot push is a storage commit; a branch names one and moves like a git branch:

```ruby
kl.workspaces.take_snapshot(workspace: "api", branch: "wip-auth", labels: ["auth"])
kl.volumes.list(label: "auth")
kl.volumes.describe(owner: "acme", name: "api-home", description: "API dev home")
kl.volumes.set_branch(owner: "acme", name: "api-home", branch: "base", version: 4)
kl.volumes.delete_branch(owner: "acme", name: "api-home", branch: "wip-auth")
```
