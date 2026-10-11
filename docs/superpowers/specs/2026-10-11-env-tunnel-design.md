# Environment tunnel: the laptop reaches the connected environment's services

Status: design, approved in conversation 2026-10-11. Owner words: "kl-connect to be modified and run
a wireguard proxy. local machine should be able to access all environment services ... modify the
local /etc/hosts file with sudo access ... run this as a service in the background"; "it should
tunnel through the existing http"; "there will be only one connected env at a time ... just one env
that is connected"; "the service will ... keep polling and keep the service hosts and ips updated in
local /etc/hosts". Approach chosen: a tunnel pod inside the environment ("go with 1").

## Goal

On the laptop, `mongodb://mongodb:27017` (any service of the connected environment, by its bare
name, any port) works exactly as it does inside a workspace attached to that environment.

Success: after `kl-connect env connect <env>`, `nc -z <service> <port>` succeeds from the laptop
for every service of that environment, and the names follow the environment as services are added
or removed, within one poll (10 s).

## Constraints

- Exactly one connected environment per laptop at a time.
- No new port, ingress or UDP path. The region gateway (`bins/gateway`, `ws-<region>.khost.dev`,
  behind Cloudflare, node firewall admits Cloudflare ranges only) is reachable over HTTPS
  websocket only, so WireGuard packets ride inside the gateway's existing websocket pipe.
- The gateway stays a pipe: it holds no WireGuard key and reads no packet.
- Access is `may_act(caller, environment owner)`, decided by the API when it mints the session, as
  `ssh-session` and `bench/session` do today.
- macOS only (launchd). The laptop CLI is the only client.

## Components

### 1. Laptop CLI: `kl-connect env`

- `kl-connect env connect <env>`: resolves the environment by name or id through `/v1`, writes
  `~/.config/kl-connect/env.json` (`{"env": "<id>", "name": "<name>"}`, mode 0600). Fails if the
  daemon is not installed, naming `kl-connect env daemon install`.
- `kl-connect env disconnect`: deletes `env.json`.
- `kl-connect env status`: prints the connected environment, the tunnel state and the names in the
  hosts block, read from `~/.config/kl-connect/env-status.json` that the daemon writes.
- `kl-connect env daemon install`: run with `sudo`. Writes
  `/Library/LaunchDaemons/io.kloudlite.kl-connect.plist` (`RunAtLoad`, `KeepAlive`) running
  `<absolute path of this binary> env daemon --home <invoking user's home>` as root, then
  `launchctl bootstrap system`. `--home` comes from `SUDO_USER`; install refuses without it.
  `env daemon uninstall` boots it out, removes the plist and the hosts block.
- `kl-connect env daemon`: the daemon itself (hidden).

The CLI and the daemon share only these two files. No socket, no IPC.

### 2. Daemon loop (every 10 s)

1. Read `{home}/.config/kl-connect/env.json` and the existing login config (token, API base).
2. No environment: tear down (below) and sleep.
3. Environment changed since the last pass: tear down, then bring up for the new one.
4. Bring up (once per environment):
   - Generate a WireGuard key pair, in memory only.
   - `POST /v1/environments/{id}/tunnel` with the public key; the answer is
     `{gateway, token, server_key, address}` (section 3).
   - Create a `utun` device, address `address` (from `100.96.0.0/16`, assigned by the API), route
     `10.43.0.0/16` (service CIDR) into it.
   - Open the websocket `wss://{gateway}/tunnel/{env}` with the token; run `boringtun` between
     the `utun` device and the websocket (framing in section 3).
5. Every pass while up:
   - `GET /v1/environments/{id}` for the service list.
   - Resolve each `{svc}.env-{id}.svc.cluster.local` against the cluster DNS (`10.43.0.10`) through
     the tunnel.
   - Rewrite the hosts block if the set changed (section 4).
   - Write `env-status.json`.
6. Tear down: close the websocket, remove the route and the `utun` device, remove the hosts block.

A dropped websocket reconnects with backoff (1 s doubling to 30 s), reusing the session until the
API refuses it, then minting a new one. The session token is short-lived like the ssh-session one;
the daemon re-mints on expiry.

### 3. Transport

- **API** `POST /v1/environments/{id}/tunnel` (user router, `bins/api`): `may_act` on the
  environment's `spec.owner`; refuses a stopped environment (409). Adds the caller's public key to
  `Environment.spec.tunnel.peers` (list of `{key, address, user, at}`, one entry per key; a second
  connect from the same user replaces their older entry), assigns the peer's address from
  `100.96.0.0/16`, returns `{gateway, token, server_key, address}`. `server_key` comes from
  `status.tunnel.serverKey`; until the pod has published it the call is 503 with `Retry-After`.
  `DELETE /v1/environments/{id}/tunnel` drops the caller's peer.
- **Token**: a new `TunnelSessionClaims` (env id, owner, peer key) in `kloudlite_core::jwt`,
  beside `SshSessionClaims` and `BenchSessionClaims`.
- **Gateway** `GET /tunnel/{env}` (websocket): the same claims-then-resolve-then-dial shape as
  `/ssh/` and `/tui/` in `bins/gateway/src/tunnel.rs`; resolves the environment's tunnel pod IP
  (new `resolve_tunnel` in `resolve.rs`), dials `TUNNEL_PORT` (7790) over TCP, pumps bytes. Same
  per-owner and global limits.
- **Framing**: the websocket and the TCP leg are byte streams to the endpoints. Each WireGuard
  datagram is sent as a 2-byte big-endian length then the datagram. Both ends reassemble; a
  length over 65535 or a short read closes the stream.

### 4. `/etc/hosts`

One block, everything outside it left byte for byte:

```
# kl-connect begin env=<id> name=<name>
10.43.12.7	mongodb
10.43.40.2	api
# kl-connect end
```

Written to a temp file in `/etc`, then renamed over `/etc/hosts`, then
`dscacheutil -flushcache` and `killall -HUP mDNSResponder`. Rewritten only when the set changed.
A bare name already defined outside the block is skipped and reported in `env-status.json`; the
block never shadows a line the person wrote.

### 5. Tunnel pod (cluster side)

- **Wish**: `Environment.spec.tunnel.peers` non-empty, written only by `/v1`.
- **Agent** (owner node's environment controller, `bins/agent/src/controller/environment/`)
  renders in `env-{id}`:
  - Secret `tunnel-key`: the server private key, generated once, kept for the environment's life;
    the public half stamped to `status.tunnel.serverKey`.
  - Pod `tunnel` (one replica, plain Pod re-created by the controller): new binary
    `bins/env-tunnel`, image built beside the others. Security context: non-root is impossible with
    TUN setup, so root with `capabilities: drop ALL, add NET_ADMIN`, read-only root filesystem,
    no service account token. Peers read from a ConfigMap `tunnel-peers` the agent renders from
    `spec.tunnel.peers`; the pod reloads it on change.
  - The binary: creates a TUN device (`100.96.0.1/16`), enables forwarding, adds one
    `MASQUERADE` rule for `100.96.0.0/16` out of `eth0`, listens on TCP 7790, and runs one
    `boringtun` peer table shared across connections. After 30 min with no datagram it exits 0
    (`restartPolicy: Never`); it holds no API credential and writes no status.
  - NetworkPolicy `allow-gateway-tunnel`: ingress to the pod on 7790 from the gateway, the same
    shape as `allow_gateway_ingress`. Egress to the namespace's own services is already allowed
    (`default_policies`: the namespace talks to itself).
- **Idle**: the pod ends itself after 30 min idle. The agent re-creates it only when some
  `spec.tunnel.peers[].at` is newer than the ended pod's finish time, i.e. a `POST /tunnel` came
  after it went idle. The agent never writes spec. `ponytail:` stale peers stay in
  `spec.tunnel.peers` until the person disconnects or the environment is deleted; prune by age
  when the list matters.
- Stopped environment: the pod goes with the rest.

## Errors

| Condition | Daemon | Hosts block |
|---|---|---|
| API or gateway unreachable | retry with backoff | kept as last written |
| 503 (pod not ready) | retry after `Retry-After` | kept |
| 403 / 404 (access lost, environment gone) | tear down, `env-status.json` says why | removed |
| 409 (environment stopped) | tear down, keep the choice, retry every pass | removed |
| Pod restarted | websocket drops, reconnects; same server key from the Secret | kept |
| Daemon killed | launchd restarts it; first pass rewrites everything | rewritten |

## Testing

- Unit: hosts block rewrite (lines outside untouched; idempotent; a name defined outside is
  skipped; removal leaves the file as it was before the first write); framing (split and joined
  reads, oversize length closes); tunnel pod and policy rendering (`k8s/tests.rs` style); API
  `tunnel` handler (may_act refusal, 409 stopped, 503 without server key, peer replace).
- Gateway: `resolve_tunnel` beside the workspace and bench resolve tests.
- Probe first, before any other task: on k3s, a hand-written pod with `NET_ADMIN`, a TUN device and
  one `MASQUERADE` rule reaches a ClusterIP in its namespace from a peer address. If this fails the
  design stops there and returns to the owner.
- End to end: probe `env.tunnel.reach` (`deploy/slo.md` catalogue entry): connect, then
  `nc -z <service> <port>` by bare name from the laptop.

## Not in this design

- Linux (systemd). Add when the CLI is used on Linux.
- More than one connected environment.
- Reaching workspaces or pods by pod IP; only services.
- UDP services are carried but not probed.
