# Kloudlite SDK for TypeScript

```sh
npm install @kloudlite/sdk
```

```ts
import { Kloudlite } from "@kloudlite/sdk";

const kl = new Kloudlite(); // uses the credentials of `kloudlite login`
```

A snapshot push is a storage commit; a branch names one and moves like a git branch:

```ts
await kl.workspaces.takeSnapshot({ workspace: "api", branch: "wip-auth", labels: ["auth"] });
await kl.volumes.list({ label: "auth" });
await kl.volumes.describe({ owner: "acme", name: "api-home", description: "API dev home" });
await kl.volumes.setBranch({ owner: "acme", name: "api-home", branch: "base", version: 4 });
await kl.volumes.deleteBranch({ owner: "acme", name: "api-home", branch: "wip-auth" });
```

Requires Node 18 or newer.
