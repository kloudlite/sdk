# Kloudlite SDK for TypeScript

```sh
npm install @kloudlite/sdk
```

```ts
import { Kloudlite } from "@kloudlite/sdk";

const kl = new Kloudlite(); // uses the credentials of `kloudlite login`
```

A snapshot push is a storage commit; a tag names one and moves only when asked:

```ts
await kl.workspaces.takeSnapshot({ workspace: "api", tag: "wip-auth", labels: ["auth"] });
await kl.volumes.list({ label: "auth" });
await kl.volumes.describe({ owner: "acme", name: "api-home", description: "API dev home" });
await kl.volumes.setTag({ owner: "acme", name: "api-home", tag: "base", version: 4 });
await kl.volumes.deleteTag({ owner: "acme", name: "api-home", tag: "wip-auth" });
```

Requires Node 18 or newer.
