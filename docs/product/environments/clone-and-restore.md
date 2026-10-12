# Clone and restore

Restore puts this environment back in time. Clone makes a new environment.

## Clone

`POST /v1/environments/{id}/clone` copies the environment into a new one. It copies the live disk, or one of the environment's snapshots when you send `snapshot_id`.

A live clone copies bytes from the source's subvolume on the node that holds it. It works on a running or stopped environment. It does not work on an interrupted one (`409`). No sync point is cut and there is no `based_on`.

```bash [API]
curl -sS -X POST https://dev.kloudlite.io/v1/environments/$ENV/clone \
  -H "Authorization: Bearer $KL_TOKEN" -H 'content-type: application/json' \
  -d '{"name": "acme-dev-2"}'
```

To clone a snapshot, add `snapshot_id`. The clone runs the services the snapshot froze, unless you send `services`.

```bash [API]
curl -sS -X POST https://dev.kloudlite.io/v1/environments/$ENV/clone \
  -H "Authorization: Bearer $KL_TOKEN" -H 'content-type: application/json' \
  -d '{"name": "acme-dev-yesterday", "snapshot_id": "push-env-2b8c-77aa", "owner": "acme"}'
```

| Field | Default |
|---|---|
| `name` | required |
| `snapshot_id` | none: clone the live disk |
| `owner` | you |
| `services` | the services the snapshot froze; an empty list means the same. A snapshot that froze none needs them here |
| `region` | the source's |
| `quota_gb` | the snapshot's |

Only `name` is allowed without `snapshot_id`. The snapshot must belong to this environment. To grow a new environment from a snapshot of a deleted environment, call `env_create` (`POST /v1/environments`) with `snapshot_id`.

## Restore

`POST /v1/environments/{id}/restore` puts this environment back to one of its snapshots: its services and its data. The environment keeps its id and its DNS names. See [restore](lifecycle.md#restore).
