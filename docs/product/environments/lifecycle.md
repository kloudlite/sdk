# Lifecycle

Start, stop, and delete for an environment mirror a workspace's, with one difference: a stop drains every service before it cuts.

## Stop

Cuts a sync point of the disk, then removes every StatefulSet. State becomes `stopped`. Data is kept.

```bash [API]
curl -sS -X POST https://dev.kloudlite.io/v1/environments/$ENV/stop -H "Authorization: Bearer $KL_TOKEN"
```

## Start

Applies every service again on the node that holds the newest sync point.

```bash [API]
curl -sS -X POST https://dev.kloudlite.io/v1/environments/$ENV/start -H "Authorization: Bearer $KL_TOKEN"
```

An environment running on a node that is down cannot be started elsewhere and cannot be cloned; it resumes when the node returns. A stopped one starts on any up-to-date node.

## Delete

Removes the services and the disk. Pushed snapshots survive on a detached [volume](../snapshots/volumes.md).

```bash [API]
curl -sS -X DELETE https://dev.kloudlite.io/v1/environments/$ENV -H "Authorization: Bearer $KL_TOKEN"
```

Any intercept wishes on the environment go with it.

## Restore

Put this environment back to one of its snapshots: its services and its data. Services drain, the disk is replaced, the snapshot's services come back. Data written since the snapshot is lost. `restoring` on the document reads `Draining` then `Restoring` while it runs, and `restored_to` names the snapshot afterwards.

```bash [API]
curl -sS -X POST https://dev.kloudlite.io/v1/environments/$ENV/restore \
  -H "Authorization: Bearer $KL_TOKEN" -H 'content-type: application/json' \
  -d '{"snapshot_id": "push-env-2b8c-77aa"}'
```

A service is persistent when it has mounts. If a persistent service exists now and the snapshot does not have it, the restore would delete its data. The call answers `409` with `lost`, a list of `{service, folders}`, and writes nothing. Ask the person. Then repeat the call with `discard` naming those services. A name in `discard` that loses nothing is `422`.

The `202` body lists `snapshot_id`, `rolled_back`, `discarded`, `added` and `removed`.

A snapshot that froze no services answers `422`. Clone it into a new environment instead.
