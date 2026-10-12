# Model and dataset hub

Date: 2026-10-12. Status: design approved in conversation, spec awaiting review.

## Goal

Host machine-learning models and datasets next to code, in the style of the Hugging Face Hub, so
that unmodified clients work against us by setting `HF_ENDPOINT`:

- `huggingface_hub` / the `hf` CLI with `hf_xet` (Xet storage) from day one;
- `transformers` / `datasets` `from_pretrained` / `load_dataset`;
- `git clone` / `git push` with git-lfs, and git-xet for uploads.

Success: `hf upload` of a real small model, `from_pretrained` of it from a workspace, and a
`git clone` of the same repo all work on the fleet.

## Decisions (owner, 2026-10-12)

1. Hugging Face Hub style. HF clients and Xet from day one, plus git-lfs and git-xet.
2. Same server (`kloudlite-srv`); the CAS gets its own hostname.
3. Separate namespaces, like images.
4. Public and private repos. Gated access is later.
5. Dedup is per owner.
6. Model and dataset repos are git-backed.
7. Code, model and dataset repos are all supported, but **code repos stay plain git**: no LFS on
   the code namespace.
8. Approach A: our own CAS, immutable per-owner objects, built on `xet-core-structures`.

Out of scope: global dedup, gated repos, Spaces, a web UI, pull requests on hub repos, search and
listing.

## 1. Architecture

Two new listeners in `kloudlite-srv`, each with its own hostname, Service port and Ingress:

| Listener | Host | Role |
|---|---|---|
| `hub` | `hub.<domain>` | The `HF_ENDPOINT`. Hub API, resolve, LFS batch, git over HTTP for hub repos. |
| `cas` | `cas.<domain>` | The `casUrl` handed out in Xet tokens. Xet CAS and signed LFS byte URLs. |

The hub gets its own host because the HF URL space (`/{owner}/{name}/resolve/...`,
`/api/models/...`) collides with our git and browse paths. The CAS gets its own host so it can move
to its own pods later without clients noticing.

### Namespaces

| Kind | Repo DB | Ownership key | URL on the hub host |
|---|---|---|---|
| model | `repo/mdl/{owner}/{name}` | `mdl/{owner}/{name}` | `/{owner}/{name}` |
| dataset | `repo/ds/{owner}/{name}` | `ds/{owner}/{name}` | `/datasets/{owner}/{name}` |

`mdl`, `ds`, `models` and `datasets` join `RESERVED_OWNERS` in `crates/storage/src/store.rs`
(today `["api","v2","img","vol","superadmin"]`).

The hub listener runs the same route-before-auth middleware as the main listener, with its own
`hub_repo_of` deriving the ownership key from the path. Anything that touches a hub repo DB is
routed to the owning node; an unroutable path is refused. The CAS listener never routes: it only
touches immutable per-owner objects, so any node may serve it.

### Object store layout (per owner)

| Key | Contents |
|---|---|
| `xet/{owner}/xorbs/{hash}` | Xorb bytes exactly as uploaded |
| `xet/{owner}/xorbs/{hash}.idx` | Chunk hashes and byte offsets of that xorb |
| `xet/{owner}/files/{hash}` | File reconstruction: terms, size, sha256 |
| `xet/{owner}/sha/{sha256}` | The Xet file hash for that sha256 |
| `lfs/{owner}/sha256/{hex}` | Whole file uploaded through plain git-lfs |
| `hub/{owner}/live/{mdl\|ds}/{name}` | Every sha256 reachable from that repo (GC live list) |

Shards are not stored; their content lands in `files/` and `sha/`.

### Crates

- `crates/xet`: the CAS (token check, xorb and shard ingest, reconstruction, signed fetch, the
  LFS byte endpoints, GC sweep). Depends on `xet-core-structures` 1.7.0 (Apache-2.0, crates.io).
- `crates/hub`: the Hub API over `crates/git` (resolve, info, tree, refs, preupload, commit,
  tokens, LFS batch, repo create/delete, whoami).
- `bins/server` mounts both on their listeners.
- `hf-xet` 1.7.0 is a dev-dependency only, to drive tests with the real client code where useful.

## 2. Hub API

`{t}` is `models` or `datasets`; `{p}` is empty for models and `datasets/` for datasets. `{repo}`
is `{owner}/{name}`.

### Resolve

`HEAD` and `GET /{p}{repo}/resolve/{rev}/{path}`. `rev` is a branch, tag or 40-hex commit.

- Every response that resolved a commit, including a 404 for a missing file, carries `X-Repo-Commit` (full commit sha).
- `X-Linked-Etag`: sha256 hex for LFS/Xet files, the git blob id otherwise. `X-Linked-Size`: file
  size.
- Files stored in Xet also carry `X-Xet-Hash` and `Link: <…/xet-read-token/{rev}>; rel="xet-auth"`.
- `GET` streams the git blob, or reassembles the Xet file, or streams the whole LFS object.
  `Range: bytes=…` is honoured.
- Missing file: 404, `X-Error-Code: EntryNotFound`. Missing repo: `RepoNotFound`. Missing
  revision: `RevisionNotFound`.

### Read API

- `GET /api/{t}/{repo}` and `/api/{t}/{repo}/revision/{rev}`: `id`, `sha`, `private`,
  `lastModified`, `siblings[]` with `rfilename`, `size`, `blobId` and, for big files,
  `lfs{size, sha256, pointerSize}`.
- `GET /api/{t}/{repo}/tree/{rev}[/{path}]` (`recursive`, `expand`): entries
  `{type, path, oid, size, lfs?, xetHash?}`, paged with `Link: <…>; rel="next"`.
- `POST /api/{t}/{repo}/paths-info/{rev}` (form body `paths`): the same entry shape.
- `GET /api/{t}/{repo}/refs`: `{branches, converts: [], tags}`, each `[{name, ref, targetCommit}]`.
- `GET /api/whoami-v2`.

### Write API

- `POST/DELETE /api/{t}/{repo}/branch/{b}` and `POST /api/{t}/{repo}/tag/{rev}`,
  `DELETE …/tag/{tag}`: through `update_refs`. Creating an existing branch is 409.
- `POST /api/{t}/{repo}/preupload/{rev}`: per file, `uploadMode: "lfs"` if `size ≥ 10 MiB` or the
  sample contains a NUL byte, else `"regular"`. `shouldIgnore: false`.
- `POST /api/{t}/{repo}/commit/{rev}` (`application/x-ndjson`): builds one git commit on the
  owning node and lands it through `update_refs`.
  - `header`: summary, description, optional `parentCommit`. A `parentCommit` that is not the
    current head is 412.
  - `file`: base64 content becomes a blob.
  - `lfsFile`: becomes a standard LFS pointer
    (`version https://git-lfs.github.com/spec/v1`, `oid sha256:<hex>`, `size <n>`). The sha256 must
    exist in `xet/{owner}/sha/` or `lfs/{owner}/sha256/`, else 422 naming the path.
  - `deletedFile`, `deletedFolder`: remove paths.
  - `.gitattributes` gains `<path> filter=lfs diff=lfs merge=lfs -text` for each new LFS path not
    already covered.
  - Reply `{commitUrl, commitOid}`. `create_pr=1` is 404.
- `GET /api/{t}/{repo}/xet-read-token/{rev}` and `…/xet-write-token/{rev}`: JSON
  `{accessToken, exp, casUrl}` and the headers `X-Xet-Cas-Url`, `X-Xet-Access-Token`,
  `X-Xet-Token-Expiration`.
- `POST /api/repos/create` (`{name, organization?, type, private?}`; 409 if it exists, reply
  `{url}`) and `DELETE /api/repos/delete`. `type: "space"` is 400.

### Git

`/{p}{repo}.git/info/refs`, `git-upload-pack`, `git-receive-pack` reuse the existing handlers.
`/{p}{repo}.git/info/lfs/objects/batch` is section 4.

### Not served

404 with `X-Error-Message`: pull requests (`create_pr`, discussions), move, settings,
super-squash, commit listing, search and listing, Spaces.

## 3. CAS

### Token

A 1-hour JWT signed with the `kloudlite-jwt` key, claims `{owner, repo, rev, scope, exp}` with
`scope` `read` or `write`. Minted only after the hub's access check for that repo. Expired or bad
signature: 401. A `read` token on an upload route: 403. The token's `owner` is the only namespace
the request may touch.

### Routes

The CAS speaks protocol v1 only. Every `/v2/*` path is 404, which makes clients fall back to v1.

- `GET /v1/reconstructions/{file_hash}`: v1 shape, `terms[]` plus
  `fetch_info{<xorb>: [{range, url, url_range}]}`. `Range` trims the terms and sets
  `offset_into_first_range`. Unknown file 404, range outside the file 416. Each `url` is a signed
  fetch URL.
- Batch: `GET /v1/reconstructions?file_id=…` (repeated). The POST form is 404. The e2e test
  settles which form the clients use; if it is POST, POST is served with the same reply and the
  GET form is removed.
- `GET /v1/fetch/{owner}/{xorb}?exp=…&sig=…`: HMAC-signed (`kloudlite-jwt` key), serves one byte
  range of a stored xorb. Expired or bad signature 403.
- `POST /v1/xorbs/default/{hash}`: parsed and decompressed with `xet-core-structures`. The Merkle
  root must equal `{hash}`, the body must be at most 64 MiB, the format must be valid, else 400.
  Written put-if-absent with its `.idx`. Reply `{was_inserted}`.
- `POST /v1/shards`: for each file in the shard, every term's xorb must exist under this owner,
  its chunk range must fit that xorb's `.idx`, and the term's verification hash must match; the
  sha256 metadata extension is required. Then `files/{hash}` and `sha/{sha256}` are written. Reply
  `{result: 1}` (new) or `{result: 0}` (all known), else 400 naming the first failure.
- `GET /v1/chunks/…` (global dedup): always 404.

### Limits

A fourth body limit, `max_xet_object` = 64 MiB, for xorbs and shards. No limit on file size; a
file is as many xorbs as it needs.

`ponytail:` the sha256 a shard declares is trusted, not recomputed. It can only point at bytes in
the same owner's namespace, so a wrong one hurts only that owner. Upgrade path: a background pass
that reassembles new files and checks the sha256.

## 4. git-lfs bridge

The batch call (`POST /{p}{repo}.git/info/lfs/objects/batch`) runs on the hub listener, routed to
the owning node, which checks visibility. Bytes always come from the CAS listener on any node,
through HMAC-signed URLs valid for one hour.

Download, per object:

- sha256 known in `xet/{owner}/sha/` or `lfs/{owner}/sha256/`: `actions.download.href` =
  `https://cas.<domain>/v1/lfs/{owner}/{oid}?exp=…&sig=…`, which streams the reassembled Xet file
  or the whole LFS object.
- Otherwise the object gets `error: {code: 404}`.

Upload, per object:

- Already stored: no actions.
- Request `transfers` includes `xet`: reply `transfer: "xet"`, `actions.upload.href` = the
  `xet-write-token` route, `actions.upload.header` = `X-Xet-Cas-Url`, `X-Xet-Access-Token`,
  `X-Xet-Token-Expiration`. git-xet then uploads through the CAS.
- Otherwise `transfer: "basic"` with a signed `PUT https://cas.<domain>/v1/lfs/{owner}/{oid}?…`.
  The body streams into an object-store multipart upload while sha256 is computed; a mismatch is
  400 and the upload is aborted. Cap 5 GiB, as for registry blobs.

`/locks/verify` and the rest of the locks API are 404.

Known gaps:

- `ponytail:` `git push` of a pointer whose object is missing is accepted. Upgrade path: a
  receive-pack check that every new pointer's sha256 exists.
- Plain-LFS uploads are whole objects, not deduplicated against Xet.

## 5. Auth, visibility, GC

### Auth on the hub listener

- `Authorization: Bearer <PAT>` resolved through `owner_for_token`. git and git-lfs send Basic
  `user:PAT`; the user name must match, as in `crates/registry/src/auth.rs` `caller`.
- A bad credential is 401. No credential is anonymous.
- Read: owner, `may_act(caller, owner)`, or a public repo. Write: owner or `may_act`.
- Private repo: 401 for anonymous, 404 `RepoNotFound` for a signed-in caller without access.
- `may_act` answering `Source::Unavailable` denies.

### Visibility

A `private` flag in the repo DB. `create` without `private` makes a **private** repo. Changed
through the existing visibility-route pattern, routed by the `mdl/` or `ds/` key.

CAS tokens and signed URLs are minted only after the hub check and last one hour.

### GC

Only the sweep deletes hub bytes.

- Live lists: after every ref change the owning node rewrites
  `hub/{owner}/live/{kind}/{name}` with every sha256 reachable from any ref in all history:
  rebuilt in full on every ref change, serialised per process (`ponytail:` incremental walk
  when rebuilds take over a second). A failed rebuild deletes the list. Repo delete drops it.
- Sweep: a worker lane beside `gc_lane` (`bins/worker/src/main.rs`) every 6 h, one owner at a
  time.
  1. Read the owner's repo list, then every live list. A missing or unreadable list aborts that
     owner.
  2. List `sha/` and `lfs/`. Delete entries no live list names and older than the 24 h grace:
     `sha/` then its `files/` record, and `lfs/` objects.
  3. Delete xorbs (and `.idx`) that no remaining `files/` record names and that are older than
     the grace.
- The shape follows the registry sweep (`crates/registry/src/gc.rs`, `sweep_owner`): references
  read first, grace window, abort on doubt. Its code is not shared.

## 6. Testing and rollout

### Tests

- Unit (`crates/xet`, `crates/hub`): xorb accept and reject (bad hash, over 64 MiB, malformed);
  shard checks (missing xorb, chunk range out of bounds, bad verification hash, no sha256);
  reconstruction `Range` trimming and `offset_into_first_range`; pointer text; the preupload rule;
  the NDJSON commit including 412 and 422; signed URL expiry and tampering; GC abort on a missing
  live list.
- Routing: every hub route that touches a repo DB is routable, held by a test like
  `every_browse_route_is_routable`.
- Integration (`tests/hub_api.rs`, `tests/hub_gc.rs`, `mem://` store, real router): xorbs and
  shards built with `xet-core-structures`; commit then resolve, tree, refs and reassembled
  download; public, private and stranger callers; LFS batch download and upload; a sweep that
  keeps live data and deletes data past the grace.
- `tests/hub_e2e.sh` (exit 77 = skipped when a client is missing), real clients against a local
  server:
  - `huggingface_hub` + `hf_xet` in a venv: `upload_folder` with a 200 MiB random file and a small
    config, `snapshot_download`, byte compare;
  - the same file re-uploaded under a new name stores no new xorb;
  - `git clone` with git-lfs, byte compare;
  - `git push` with git-xet installed, then download through the HF client;
  - `AutoConfig.from_pretrained` via `HF_ENDPOINT`.

  `tests/hub_e2e.sh` settled the open protocol questions:
  - Batch reconstruction: hf_xet 1.x downloading a three-file folder issued only
    `GET /v1/reconstructions/{hash}`, one per file, and never touched the batch route. The
    batch GET stays served but is unexercised by this client.
  - Tree entries: `huggingface_hub` reads `lfs["oid"]` from `/tree` entries
    (`BlobLfsInfo(sha256=lfs["oid"])`) and `lfs["sha256"]` from `siblings` in model info. The
    hub emits exactly that split (`read.rs`), so neither key is wrong.
  - `transfers` git-xet sends: not observed: git-xet not installed in the test run.
  - Also seen: hf_xet uploads xorbs as bare serialized chunks with no info footer, and
    its shard sha256 is a `DataHash` read with `from_be_bytes`; xorb ingest, `sha_hex` and the
    resolve stream were fixed to match, with tests.

### Rollout

- `kloudlite-srv` gets the hub and cas ports; the Service gains both; two Ingresses
  `hub.<domain>` and `cas.<domain>`, each with its own TLS, checked with `dig` first.
- Settings `KLOUDLITE_HUB_URL` and `KLOUDLITE_CAS_URL` (the latter is the token's `casUrl`).
- SLO probes `hub.upload` and `hub.download` in `deploy/slo.md` and the catalogue.
- CLAUDE.md: the third namespace, "only the sweep deletes hub bytes", `max_xet_object`, CAS v1
  only.

### Done

`tests/hub_e2e.sh` passes locally, the fleet runs the new pin, and the live drill passes:
`hf upload` of a real small model (for example `gpt2` config and weights), `from_pretrained` from
a workspace, then `git clone` of the same repo.
