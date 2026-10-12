# Environments API

## The document

```json
{
  "id": "env-2b8c…",
  "owner": "acme",
  "name": "acme-dev",
  "region": "centralindia-k3s",
  "state": "running",
  "placement": "node-1",
  "volume": "vol-…",
  "services": [
    {
      "name": "api",
      "image": "cr.khost.dev/acme/api:1",
      "command": ["node", "server.js"],
      "env": { "DATABASE_URL": "postgres://postgres:dev@db:5432/postgres" },
      "ports": [8080],
      "mounts": [],
      "resources": { "cpu_request": "500m", "cpu_limit": "1", "memory_request": "1Gi", "memory_limit": "2Gi" }
    }
  ],
  "restored_to": null,
  "restore_requested_at": null,
  "restoring": null
}
```

`state` is one of `creating`, `running`, `stopped`, `error`, `deleted`. Per-service status (`ready`, `intercepted_by`, `unreachable_since`) is reported beside the spec.

## `POST /environments`

| Field | Type | Required |
|---|---|---|
| `name` | string | yes |
| `region` | string | yes |
| `services` | Service[] | yes, at least one |
| `quota_gb` | integer | yes |
| `owner` | string | no |

### Service

| Field | Type | Rule |
|---|---|---|
| `name` | string | DNS label, ≤ 63 |
| `image` | string | |
| `command` | string[] | `[]` keeps the image's |
| `env` | object | valid container env keys |
| `ports` | integer[] | 1–65535 |
| `mounts` | `{folder, path}[]` | `folder` one path segment; `path` absolute, no `:` |
| `resources` | `{cpu_request, cpu_limit, memory_request, memory_limit}` | optional |

## `GET /environments` · `GET /environments/{id}`

The hidden per-owner builder never appears in either.

## `POST /environments/{id}/start` · `/stop` · `DELETE /environments/{id}`

No body.

## `POST /environments/{id}/clone`

Makes a NEW environment. `{"name": "…"}` copies the live disk; `409` when the source is interrupted. Add `snapshot_id` to copy one of this environment's snapshots:

| Field | Required |
|---|---|
| `name` | yes |
| `snapshot_id` | no; without it, only `name` is allowed |
| `owner`, `region` | no |
| `services` | no; empty or absent means the snapshot's |
| `quota_gb` | no |

## `POST /environments/{id}/restore`

Puts THIS environment back to a snapshot: services and data. `{"snapshot_id": "…", "discard": ["…"]}`; `discard` is optional.

`409` with `{"error", "lost": [{"service", "folders"}]}` when a persistent service would lose data the snapshot lacks; nothing is written. `422` when `discard` names a service that loses nothing, or when the snapshot froze no services. `202` with `snapshot_id`, `rolled_back`, `discarded`, `added` and `removed`.

## `POST /environments/{id}/push`

Optional `{"message": "…"}`.

## `POST /environments/{id}/intercepts`

```json
{ "service": "api", "workspace": "ws-…", "ports": [{ "service": 8080, "workspace": 3000 }] }
```

`409` when another workspace holds the service, naming it.

## `DELETE /environments/{id}/intercepts/{service}`

Removes the wish. No body.
