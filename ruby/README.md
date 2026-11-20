# Kloudlite SDK for Ruby

```sh
gem install kloudlite-sdk
```

```ruby
require "kloudlite"

kl = Kloudlite::Client.new # uses the credentials of `kloudlite login`
```

A snapshot push is a storage commit; a tag names one and moves only when asked:

```ruby
kl.workspaces.take_snapshot(workspace: "api", tag: "wip-auth", labels: ["auth"])
kl.volumes.list(label: "auth")
kl.volumes.describe(owner: "acme", name: "api-home", description: "API dev home")
kl.volumes.set_tag(owner: "acme", name: "api-home", tag: "base", version: 4)
kl.volumes.delete_tag(owner: "acme", name: "api-home", tag: "wip-auth")
```
