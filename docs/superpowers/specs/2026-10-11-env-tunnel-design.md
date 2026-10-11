# Environment tunnel: the laptop reaches the connected environment through the bench

Status: design, approved in conversation 2026-10-11. Owner words: "kl-connect to be modified and run
a wireguard proxy. local machine should be able to access all environment services ... modify the
local /etc/hosts file with sudo access ... run this as a service in the background"; "it should
tunnel through the existing http"; "there will be only one connected env at a time ... just one env
that is connected"; "the service will ... keep polling and keep the service hosts and ips updated in
local /etc/hosts"; "use bench pod for this. it will serve as gateway for tools and sessions and even
this tunnel. that way we can make it as single gateway for user".

## Goal

On the laptop, `mongodb://mongodb:27017` (any service of the connected environment, by its bare
name, any port) works exactly as it does inside the bench.

Success: after `kl-connect env connect <env>`, `nc -z <service> <port>` succeeds from the laptop
for every service of that environment, and the names follow the environment as services are added
or removed, within one poll (10 s).

## Shape

The bench is the person's single gateway into the cluster: sessions, tools, and now the network.
The connected environment is the SPACE's environment (`SpaceEnvironment`, the same choice the bench
sidebar and every pod of the space follow), so the laptop sees exactly what the bench sees. The
bench already reaches it: `space-env` / `space-{ns}` (`k8s/policies.rs`) admit every pod of the
space into the environment's namespace.

```
laptop utun ── boringtun ── wss /net/{bench} ── gateway pipe ── bench pod :7792 ── boringtun ── TUN ── NAT ── env services
```

## Constraints

- Exactly one connected environment, the space's.
- No new port, ingress or UDP path. The region gateway (`bins/gateway`, behind Cloudflare, node
  firewall admits Cloudflare ranges only) is reachable over HTTPS websocket only, so WireGuard
  datagrams ride inside the gateway's existing pipe.
- The gateway stays a pipe: it holds no WireGuard key and reads no datagram.
- Access is the bench session's: `POST /v1/bench/session` already decides who may reach the
  bench and wakes it. Nothing new is authorized anywhere.
- No API or CRD change.
- macOS only (launchd). The laptop CLI is the only client.

## Components

### 1. Laptop CLI: `kl-connect env`

- `kl-connect env connect <env> [--team T]`: `PUT /v1/me/environments/{team}` (the existing space
  choice; the bench and the sidebar follow it too), then writes `~/.config/kl-connect/env.json`
  (`{"team": "<team>"}`, mode 0600). Fails if the daemon is not installed, naming
  `sudo kl-connect env daemon install`.
- `kl-connect env disconnect`: `DELETE /v1/me/environments/{team}` and deletes `env.json`.
- `kl-connect env status`: prints the connected environment, the tunnel state and the names in the
  hosts block, from `~/.config/kl-connect/env-status.json` that the daemon writes.
- `sudo kl-connect env daemon install`: writes
  `/Library/LaunchDaemons/io.kloudlite.kl-connect.plist` (`RunAtLoad`, `KeepAlive`) running
  `<absolute path of this binary> env daemon --home <home of SUDO_USER>` as root, then
  `launchctl bootstrap system`. Refuses without `SUDO_USER`. `env daemon uninstall` boots it out,
  removes the plist and removes the hosts block.
- `kl-connect env daemon`: the daemon (hidden).

CLI and daemon share only these two files. No socket.

### 2. Daemon loop (every 10 s)

1. Read `{home}/.config/kl-connect/env.json` and the login config (token, API base).
2. No `env.json`: tear down (step 6) and sleep.
3. `GET /v1/me/environments` for the space's environment in that team. None: tear down.
   Changed since the last pass: tear down, then bring up.
4. Bring up:
   - `POST /v1/bench/session` (`api::bench_session`, the TUI's own call), waiting while it
     answers `Waking`.
   - Create a `utun` device, route `10.43.0.0/16` (service CIDR) into it.
   - Open `wss://{gateway}/net/{bench}` with the session token (the session's `/tunnel/` URL with
     the route swapped, as `bench.rs` already does for `/tui/`).
   - Handshake (section 3), then run `boringtun` between the `utun` device and the stream.
5. Every pass while up:
   - `GET /v1/environments/{id}` for the service list.
   - Resolve each `{svc}.env-{id}.svc.cluster.local` against cluster DNS (`10.43.0.10`) through
     the tunnel.
   - Rewrite the hosts block if the set changed (section 4). Write `env-status.json`.
6. Tear down: close the stream, remove the route and the `utun` device, remove the hosts block.

A dropped stream reconnects with backoff (1 s doubling to 30 s), minting a new bench session each
time. The WireGuard key pair lives in memory for the daemon's life.

### 3. Transport

- **Gateway** `GET /net/{bench}` (websocket): a third route beside `/tunnel/{ws}` and
  `/tui/{bench}` in `bins/gateway/src/tunnel.rs`, the same `BenchSessionClaims` and
  `resolve_bench` as `/tui/`, dialing `BENCH_NET_PORT` (7792) instead of `BENCH_TUI_PORT`. Same
  limits. Policy `allow-gateway-bench` gains port 7792.
- **Framing**: both legs are byte streams to the endpoints. Every message is a 2-byte big-endian
  length then the payload. A length of 0 or a short read closes the stream.
- **Handshake**: the first message from the laptop is its 32-byte WireGuard public key; the first
  message back is the bench's 32-byte public key then its 4-byte peer address (from
  `100.96.0.0/16`, one per open stream). Every later message is one WireGuard datagram. The keys
  are not the authentication (the bench session is); they exist so the bytes are WireGuard as
  asked, and so a stream cannot inject packets for another stream's address.

### 4. `/etc/hosts`

One block, everything outside it left byte for byte:

```
# kl-connect begin env=<id> name=<name>
10.43.12.7	mongodb
10.43.40.2	api
# kl-connect end
```

Written to a temp file in `/etc`, renamed over `/etc/hosts`, then `dscacheutil -flushcache` and
`killall -HUP mDNSResponder`. Rewritten only when the set changed. A bare name already defined
outside the block is skipped and reported in `env-status.json`; the block never shadows a line the
person wrote.

### 5. Bench pod (cluster side)

- **Container `net`**, a second container in the bench pod (`crates/workspaces/src/k8s/bench.rs`),
  running the new binary `bins/bench-net` from the bench image. It is a separate container so
  `NET_ADMIN` never reaches the `sessions` container, where agent sessions and user code run.
  Security context: `hardened()` plus `NET_ADMIN`, read-only root filesystem; mounts `/tmp` and
  hostPath `/dev/net/tun` (`type: CharDevice`). The pod sets sysctl `net.ipv4.ip_forward=1`, which
  k3s admits through the kubelet's `allowed-unsafe-sysctls` (node config, `deploy/k3s/README.md`).
  The cluster is ours (owner, 2026-10-11: "we have permission"): if the probe shows the runtime
  refusing either, `net` runs `privileged: true` instead; still only `net`, never `sessions`.
  This is a second container beside the one the 2026-09-25 ruling allows ("no shell sidecar, no
  workspace container on a bench pod"); it carries no shell and no workspace, and exists only to
  keep the capability out of `sessions`.
- **The binary**: at start creates a TUN device (`100.96.0.1/16`), enables forwarding, adds one
  `MASQUERADE` rule for `100.96.0.0/16` out of `eth0`, generates its key pair (memory only),
  listens on TCP 7792, and runs one `boringtun` peer per stream, routing by peer address. A stream
  that closes drops its peer.
- Pod restart: streams drop, the daemon reconnects, the handshake exchanges new keys.
- **An open tunnel keeps the bench awake** (owner, 2026-10-11: "I want the bench to be awake as
  long as there is connection"). `bench-net` writes its open-stream count to `net.streams` in an
  `emptyDir` mounted at `/run/kl-net` in both `net` and `sessions`, every 5 s, the same shape as
  the daemon's `~/.kl/host.busy`. `bench/sessions/main.ts` reads it beside `busy()` (older than
  15 s counts 0, a dead `net` holds nothing open): idle = no clients, no running turn, no tunnel.
  The laptop daemon keeps its stream open while connected, so a connected laptop never lets the
  bench idle; `env disconnect` (or the laptop going away) closes the stream and the bench idles
  `bench_idle_secs` later as today.

## Errors

| Condition | Daemon | Hosts block |
|---|---|---|
| API or gateway unreachable | retry with backoff | kept as last written |
| Bench waking | wait on `Waking`, as the TUI does | kept |
| 403 / 404 (access lost, environment or space gone) | tear down, `env-status.json` says why | removed |
| Space has no environment | tear down | removed |
| Bench restarted | stream drops, reconnects | kept |
| Laptop asleep / offline | stream drops; bench idles `bench_idle_secs` later | kept |
| Daemon killed | launchd restarts it; first pass rewrites everything | rewritten |

## Testing

- Probe first, before any other task: on k3s, a hand-run container with `NET_ADMIN`, a TUN device
  and one `MASQUERADE` rule, in a space namespace, reaches a ClusterIP of the space's environment
  from a peer address. The probe tries the narrow form (NET_ADMIN, `/dev/net/tun` hostPath, the
  sysctl) first and records which form worked; that form is what `bench.rs` renders.
- Unit: hosts block rewrite (lines outside untouched; idempotent; a name defined outside is
  skipped; removal leaves the file as before the first write); framing (split and joined reads,
  zero length closes); handshake (addresses unique per stream, freed on close); bench pod
  rendering keeps `NET_ADMIN` on `net` only (`k8s/tests.rs` style); policy carries 7792; the idle
  probe stays awake while `net.streams` is fresh and non-zero, and ignores a stale one.
- Gateway: `/net/{bench}` beside the `/tui/` tests (same refusals).
- End to end: probe `env.tunnel.reach` (`deploy/slo.md` catalogue entry): connect, then
  `nc -z <service> <port>` by bare name from the laptop.

## Not in this design

- Linux (systemd). Add when the CLI is used on Linux.
- More than one connected environment.
- Reaching workspaces or pods by pod IP; only services.
- UDP services are carried but not probed.
