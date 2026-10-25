/**
 * Kloudlite SDK for TypeScript — zero dependencies, built on `fetch`.
 *
 * Auth is a Bearer token (a session or CLI token). Every module hangs off the
 * one client. Every method takes ONE
 * options object, so no caller ever has to remember argument order:
 *
 *     import { Kloudlite } from "@kloudlite/sdk";
 *     const kl = new Kloudlite({ baseUrl: "https://dev.kloudlite.io", token: "…" });
 *     const workspaces = await kl.workspaces.list();
 *
 * Option names are camelCase everywhere (`quotaGb`, `snapshot`, `timeoutMs`); the
 * SDK maps them onto whatever the wire spells (`quota_gb`, `snapshot_id`,
 * `timeout_ms`). {@link Kloudlite.request} is the low-level escape hatch and takes the wire as-is.
 */

export const VERSION = "0.1.0";
const USER_AGENT = `kloudlite-sdk-typescript/${VERSION}`;

export interface KloudliteOptions {
  baseUrl: string;
  /** Omit for anonymous access to public data. */
  token?: string;
  /** Per-request timeout in milliseconds. Default 30 000. */
  timeoutMs?: number;
}

/** A non-2xx answer. `status` is the HTTP code, `message` the server's text. */
export class ApiError extends Error {
  constructor(
    public readonly status: number,
    message: string,
  ) {
    super(`${status}: ${message}`);
    this.name = "ApiError";
  }
}

type Query = Record<string, string | number | boolean | undefined>;

/** Module-private: the NDJSON stream {@link Process} output follows. Not exported, so not API. */
const STREAM = Symbol("stream");

export class Kloudlite {
  readonly workspaces: Workspaces;
  readonly environments: Environments;
  readonly sessions: Sessions;
  readonly volumes: Volumes;
  readonly secrets: Secrets;
  readonly sandboxes: Sandboxes;
  readonly pipelines: Pipelines;
  private readonly baseUrl: string;
  private readonly token?: string;
  private readonly timeoutMs: number;

  constructor(opts: KloudliteOptions) {
    this.baseUrl = opts.baseUrl.replace(/\/+$/, "");
    this.token = opts.token;
    this.timeoutMs = opts.timeoutMs ?? 30_000;
    this.workspaces = new Workspaces(this);
    this.environments = new Environments(this);
    this.sessions = new Sessions(this);
    this.volumes = new Volumes(this);
    this.secrets = new Secrets(this);
    this.sandboxes = new Sandboxes(this);
    this.pipelines = new Pipelines(this);
  }

  /** The ceiling and what is against it; `owner` defaults to the caller's own. */
  quota({ owner }: { owner?: string } = {}) {
    return this.request("GET", "/v1/quota", undefined, { owner });
  }

  /**
   * One HTTP round trip. Resolves to parsed JSON (or `null` on an empty body);
   * rejects with {@link ApiError} on any non-2xx status.
   */
  request<T = unknown>(method: string, path: string, body?: unknown, query?: Query): Promise<T> {
    return this.send(this.baseUrl, method, path, body, query);
  }

  private async send<T>(base: string, method: string, path: string, body?: unknown, query?: Query): Promise<T> {
    const resp = await fetch(url(base, path, query), {
      method,
      headers: this.headers("application/json", body !== undefined),
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(this.timeoutMs),
    });
    const raw = await resp.text();
    if (!resp.ok) throw new ApiError(resp.status, errorText(raw));
    return (raw ? JSON.parse(raw) : null) as T;
  }

  /**
   * A GET answering `application/x-ndjson`, yielded one parsed object per line as the bytes
   * arrive. No timeout: a stream lasts as long as the process behind it, and the server sends its
   * headers at once, so a dead request still fails fast. Leaving the loop early cancels the body.
   */
  async *[STREAM](path: string, query?: Query): AsyncGenerator<unknown> {
    const ctl = new AbortController();
    const resp = await fetch(url(this.baseUrl, path, query), {
      headers: this.headers("application/x-ndjson", false),
      signal: ctl.signal,
    });
    if (!resp.ok) throw new ApiError(resp.status, errorText(await resp.text()));
    if (!resp.body) return;
    const reader = resp.body.getReader();
    const text = new TextDecoder();
    let buf = "";
    let done = false;
    try {
      for (;;) {
        const chunk = await reader.read();
        // `stream: true` holds back a UTF-8 sequence split across chunks; the final call flushes it.
        buf += chunk.done ? text.decode() : text.decode(chunk.value, { stream: true });
        let nl: number;
        while ((nl = buf.indexOf("\n")) >= 0) {
          const line = buf.slice(0, nl).trim();
          buf = buf.slice(nl + 1);
          if (line) yield JSON.parse(line);
        }
        if (chunk.done) break;
      }
      if (buf.trim()) yield JSON.parse(buf);
      done = true;
    } finally {
      if (!done) ctl.abort();
    }
  }

  private headers(accept: string, json: boolean): Record<string, string> {
    const headers: Record<string, string> = { "User-Agent": USER_AGENT, Accept: accept };
    if (this.token) headers.Authorization = `Bearer ${this.token}`;
    if (json) headers["Content-Type"] = "application/json";
    return headers;
  }
}

function url(base: string, path: string, query?: Query): string {
  let out = base + path;
  if (query) {
    const q = new URLSearchParams();
    for (const [k, v] of Object.entries(query)) {
      if (v !== undefined) q.set(k, String(v));
    }
    const qs = q.toString();
    if (qs) out += `?${qs}`;
  }
  return out;
}

/** A repository, by its one `"owner/name"` string. */
export interface RepoRef {
  repo: string;
}

const seg = encodeURIComponent;

/** `"owner/name"` → its two halves, refused client-side when malformed. */
function split(repo: string): [string, string] {
  const at = repo.indexOf("/");
  const owner = repo.slice(0, at < 0 ? 0 : at);
  const name = at < 0 ? "" : repo.slice(at + 1);
  if (!owner || !name || name.includes("/")) {
    throw new Error(`a repository is named owner/name, got ${JSON.stringify(repo)}`);
  }
  return [owner, name];
}

/** A workspace: `"name"` (the caller's own), `"team/name"` (theirs in a team), or its id. */
export interface WorkspaceRef {
  workspace: string;
}

export interface CreateWorkspace extends WorkspaceRef {
  quotaGb: number;
  image?: string;
  /** A PLATFORM repo, `"owner/name"`, to seed from (with `branch`). */
  repo?: string;
  branch?: string;
  /** nixpkgs attributes, optionally pinned (`attr@version`). */
  packages?: string[];
  /** Seconds unused (600..=2592000) before the workspace is deleted; absent, never. */
  ttlSecs?: number;
  /** Variable name to a literal value or a `"${{ secrets.NAME }}"` reference to the owner's secrets. */
  env?: Record<string, string>;
}

export interface CloneWorkspace extends WorkspaceRef {
  /** The new workspace's name. */
  to: string;
  task?: string;
}

export interface RestoreWorkspace {
  /** The PUSHED snapshot to graft onto. */
  snapshot: string;
  /** The new workspace's name. */
  to: string;
  image?: string;
  packages?: string[];
  quotaGb?: number;
}

/**
 * Btrfs-backed dev workspaces, over `/v1/workspaces`. A workspace is named by ONE string —
 * `"name"` for the caller's own, `"team/name"` for theirs in a team — or by its id; pushes cut
 * snapshots, kept on the workspace's VOLUME — see {@link Volumes} for history, restore targets
 * and deletion.
 */
export class Workspaces {
  constructor(private readonly c: Kloudlite) {}

  /**
   * `workspace` is split into the body's `name` and `team` (no team for a personal one). `repo`
   * seeds from a PLATFORM repo ("owner/name", with `branch`); `packages` are nixpkgs attributes,
   * optionally pinned (`attr@version`).
   */
  create({ workspace, quotaGb, image, repo, branch, packages = [], ttlSecs, env }: CreateWorkspace) {
    const [team, name] = owned(workspace);
    return this.c.request("POST", "/v1/workspaces", {
      packages, name, team, quota_gb: quotaGb, image, repo, branch, ttl_secs: ttlSecs, env,
    });
  }

  /** Seconds unused before the workspace is deleted (600..=2592000); `null` clears it. Not for a sandbox or a bench. */
  setTtl({ workspace, ttlSecs }: WorkspaceRef & { ttlSecs: number | null }) {
    return this.at("PUT", workspace, "/ttl", { ttl_secs: ttlSecs });
  }

  /** Replace the whole env (literals or `"${{ secrets.NAME }}"`); takes effect on the next start. */
  setEnv({ workspace, env }: WorkspaceRef & { env: Record<string, string> }) {
    return this.at("PUT", workspace, "/env", { env });
  }

  /** The caller's workspaces; `team` scopes to their workspaces in that team. */
  list({ team }: { team?: string } = {}) {
    return this.c.request<unknown[]>("GET", "/v1/workspaces", undefined, { team });
  }

  get({ workspace }: WorkspaceRef) {
    return this.at("GET", workspace);
  }

  /** Runs the worktree finalizer: the volume survives while a pushed snapshot references it. */
  delete({ workspace }: WorkspaceRef) {
    return this.at("DELETE", workspace);
  }

  start({ workspace }: WorkspaceRef) {
    return this.at("POST", workspace, "/start");
  }

  /** A stop cuts a sync point and tears the workspace down — seconds, never waiting on a replica. */
  stop({ workspace }: WorkspaceRef) {
    return this.at("POST", workspace, "/stop");
  }

  /** Cut a SNAPSHOT of the working copy — the one durable, restorable state. */
  push({ workspace, message }: WorkspaceRef & { message?: string }) {
    return this.at("POST", workspace, "/push", message === undefined ? undefined : { message });
  }

  /**
   * A new workspace `to` from this one's state, cut at the moment of the request; the answer's
   * `based_on` names exactly what it grafted onto.
   */
  clone({ workspace, to, task }: CloneWorkspace) {
    return this.at("POST", workspace, "/clone", { name: to, task });
  }

  /**
   * A new workspace `to` grafted onto a PUSHED `snapshot` — also one whose workspace is long
   * gone. Absent overrides mean "whatever the snapshot froze".
   */
  restore({ snapshot, to, image, packages, quotaGb }: RestoreWorkspace) {
    return this.c.request("POST", "/v1/workspaces/restore", {
      name: to, snapshot_id: snapshot, image, packages, quota_gb: quotaGb,
    });
  }

  /** Replace the package list; pinned entries are locked before anything is written. */
  setPackages({ workspace, packages }: WorkspaceRef & { packages: string[] }) {
    return this.at("PATCH", workspace, "", { packages });
  }

  /** Re-resolve every pinned package — the only call that moves existing locks. */
  updatePackages({ workspace }: WorkspaceRef) {
    return this.at("POST", workspace, "/packages/update");
  }

  /** The workspace's tool-server address and token — answered to the owner alone. */
  tools({ workspace }: WorkspaceRef) {
    return this.at<{ address: string; token?: string }>("GET", workspace, "/tools");
  }

  /**
   * An {@link Ide} bound to this workspace's tool server. The address is a private IP: reachable
   * from inside Kloudlite (a bench); from a laptop, tunnel with `kl-connect ws ide` and use
   * `new Ide({ url: "http://127.0.0.1:7788", token })` instead.
   */
  async ide({ workspace }: WorkspaceRef): Promise<Ide> {
    const doc = await this.tools({ workspace });
    return new Ide({ url: `http://${doc.address}`, token: doc.token });
  }

  /** `"team/name"` travels as segment `name` plus `?owner=team`, which the server folds back. */
  private at<T = unknown>(method: string, workspace: string, tail = "", body?: unknown) {
    const [owner, name] = owned(workspace);
    return this.c.request<T>(method, `/v1/workspaces/${seg(name)}${tail}`, body, { owner });
  }
}

type ToolArgs = Record<string, unknown>;

export interface IdeOptions {
  /** The tool server's base URL, e.g. `http://127.0.0.1:7788` through a tunnel. */
  url: string;
  token?: string;
  /** Per-call timeout in milliseconds. Default 630 000 — above the longest tool timeout. */
  timeoutMs?: number;
}

export interface Grep {
  pattern: string;
  cwd?: string;
  glob?: string;
  mode?: "content" | "files" | "count";
  context?: number;
  ignoreCase?: boolean;
  max?: number;
}

export interface Exec {
  cmd: string | string[];
  cwd?: string;
  env?: Record<string, string>;
  timeoutMs?: number;
  head?: number;
  tail?: number;
  quiet?: boolean;
  /** Answer `{id}` at once — the `process` the process_* calls take. */
  detach?: boolean;
}

export interface ContainerBuild {
  tags: string[];
  context?: string;
  file?: string;
  buildArgs?: string[];
  platform?: string;
  noCache?: boolean;
  timeoutMs?: number;
  head?: number;
  tail?: number;
}

/**
 * A workspace's tool server (`kl ide serve`): the full tool set over plain HTTP — `GET /tools`
 * lists every tool with its schema, `POST /tools/{name}` runs one. The typed methods cover them
 * all; {@link call} takes anything the listing names. `undefined` options are dropped, so every
 * optional means "the tool's own default".
 */
export class Ide {
  private readonly url: string;
  private readonly token?: string;
  private readonly timeoutMs: number;

  constructor({ url, token, timeoutMs = 630_000 }: IdeOptions) {
    this.url = url.replace(/\/+$/, "");
    this.token = token;
    this.timeoutMs = timeoutMs;
  }

  /** Every tool, with its JSON schema — the authority on arguments. */
  list() {
    return this.http("GET", "/tools");
  }

  /** Any tool by name; `args` are its own arguments, spelled exactly as its schema spells them. */
  call<T = unknown>({ tool, args = {} }: { tool: string; args?: ToolArgs }): Promise<T> {
    return this.http<T>("POST", `/tools/${seg(tool)}`, args);
  }

  private run(tool: string, args: ToolArgs = {}) {
    return this.call({ tool, args });
  }

  // files
  read({ path, paths, offset, limit }: { path?: string; paths?: unknown[]; offset?: number; limit?: number } = {}) {
    return this.run("read", { path, paths, offset, limit });
  }
  write({ path, content }: { path: string; content: string }) {
    return this.run("write", { path, content });
  }
  /**
   * One file as `{path, edits}`, several as `{files: [{path, edits}]}`; each edit is
   * `{old, new, replace_all?}` and the whole call is atomic.
   */
  edit({ path, edits, files }: { path?: string; edits?: unknown[]; files?: unknown[] }) {
    return this.run("edit", { path, edits, files });
  }
  patch({ diff, cwd }: { diff: string; cwd?: string }) {
    return this.run("patch", { diff, cwd });
  }
  glob({ pattern, cwd }: { pattern: string; cwd?: string }) {
    return this.run("glob", { pattern, cwd });
  }
  grep({ pattern, cwd, glob, mode, context, ignoreCase, max }: Grep) {
    return this.run("grep", { pattern, cwd, glob, mode, context, ignore_case: ignoreCase, max });
  }

  // commands and processes — `detach: true` answers `{id}`, the `process` of the process_* calls
  exec({ cmd, cwd, env, timeoutMs, head, tail, quiet, detach }: Exec) {
    return this.run("exec", { cmd, cwd, env, timeout_ms: timeoutMs, head, tail, quiet, detach });
  }
  processList() {
    return this.run("process_list");
  }
  processOutput({ process, since, sinceErr }: { process: string; since?: number; sinceErr?: number }) {
    return this.run("process_output", { id: process, since, since_err: sinceErr });
  }
  processWrite({ process, data }: { process: string; data: string }) {
    return this.run("process_write", { id: process, data });
  }
  processKill({ process, signal }: { process: string; signal?: "TERM" | "KILL" }) {
    return this.run("process_kill", { id: process, signal });
  }

  // watches — `watch` answers `{id}`, the `watch` of watchPoll/watchStop
  watch({ paths, cmd, pattern, once }: { paths?: string[]; cmd?: string | string[]; pattern?: string; once?: boolean } = {}) {
    return this.run("watch", { paths, cmd, pattern, once });
  }
  watchPoll({ watch, since }: { watch: string; since?: number }) {
    return this.run("watch_poll", { id: watch, since });
  }
  watchStop({ watch }: { watch: string }) {
    return this.run("watch_stop", { id: watch });
  }

  // containers (the hidden per-owner builder)
  containerBuild({ tags, context, file, buildArgs, platform, noCache, timeoutMs, head, tail }: ContainerBuild) {
    return this.run("container_build", {
      tags, context, file, build_args: buildArgs, platform, no_cache: noCache, timeout_ms: timeoutMs, head, tail,
    });
  }
  containerPush({ src, dst }: { src: string; dst: string[] }) {
    return this.run("container_push", { src, dst });
  }

  private async http<T = unknown>(method: string, path: string, body?: ToolArgs): Promise<T> {
    const headers: Record<string, string> = { "User-Agent": USER_AGENT, Accept: "application/json" };
    if (this.token) headers.Authorization = `Bearer ${this.token}`;
    if (body !== undefined) headers["Content-Type"] = "application/json";
    const resp = await fetch(this.url + path, {
      method,
      headers,
      // JSON.stringify drops undefined values, which is the "tool's own default" rule above.
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(this.timeoutMs),
    });
    const raw = await resp.text();
    if (!resp.ok) throw new ApiError(resp.status, errorText(raw));
    return (raw ? JSON.parse(raw) : null) as T;
  }
}

/**
 * A service of an environment: `name` and `image`, plus optional `ports`, `env`, `command` and
 * `mounts` (a folder of the environment's disk, mounted at `path`).
 */
export interface Service {
  name: string;
  image: string;
  ports?: number[];
  env?: Record<string, string>;
  command?: string[];
  mounts?: { folder: string; path: string }[];
}

/** An environment: `"name"` (the caller's own), `"team/name"` (a team's), or its id (`env-…`). */
export interface EnvironmentRef {
  environment: string;
}

export interface CreateEnvironment extends EnvironmentRef {
  services?: Service[];
  quotaGb?: number;
  /** A pushed snapshot to grow it from. */
  snapshot?: string;
  /** Seconds unused (600..=2592000) before the environment is deleted; absent, never. */
  ttlSecs?: number;
}

export interface CloneEnvironment extends EnvironmentRef {
  /** The new environment, `"name"` or `"team/name"`. */
  to: string;
  snapshot?: string;
  services?: Service[];
  quotaGb?: number;
}

export interface RestoreEnvironment extends EnvironmentRef {
  snapshot: string;
  /** Persistent services whose data the caller accepts losing. Default `[]`. */
  discard?: string[];
}

/**
 * Multi-service environments, over `/v1/environments`: a set of containers sharing one disk. An
 * environment is named by ONE string — `"name"` for the caller's own, `"team/name"` for a
 * team's — or by its id (`env-…`).
 */
export class Environments {
  constructor(private readonly c: Kloudlite) {}

  /**
   * `snapshot` grows it from a pushed snapshot — also one of a deleted environment — and then
   * `services` default to the ones that snapshot froze.
   */
  create({ environment, services: list, quotaGb, snapshot, ttlSecs }: CreateEnvironment) {
    const [owner, name] = owned(environment);
    return this.c.request("POST", "/v1/environments", {
      name, owner, quota_gb: quotaGb, snapshot_id: snapshot, services: services(list), ttl_secs: ttlSecs,
    });
  }

  /** The caller's environments and their teams'; `owner` narrows to one handle or team. */
  list({ owner }: { owner?: string } = {}) {
    return this.c.request<unknown[]>("GET", "/v1/environments", undefined, { owner });
  }

  get({ environment }: EnvironmentRef) {
    return this.at("GET", environment);
  }

  /** Replace the WHOLE service list. A removed service keeps its folders, so re-adding finds its data. */
  setServices({ environment, services: list }: EnvironmentRef & { services: Service[] }) {
    return this.at("PATCH", environment, "", { services: services(list) });
  }

  start({ environment }: EnvironmentRef) {
    return this.at("POST", environment, "/start");
  }

  stop({ environment }: EnvironmentRef) {
    return this.at("POST", environment, "/stop");
  }

  /** Seconds the environment may sit unused (600..=2592000) before it is deleted; null clears. */
  setTtl({ environment, ttlSecs }: EnvironmentRef & { ttlSecs: number | null }) {
    return this.at("PUT", environment, "/ttl", { ttl_secs: ttlSecs });
  }

  /** The volume survives while a pushed snapshot references it. */
  delete({ environment }: EnvironmentRef) {
    return this.at("DELETE", environment);
  }

  /** Cut a SNAPSHOT of every service's data — the one durable, restorable state. */
  push({ environment, message }: EnvironmentRef & { message?: string }) {
    return this.at("POST", environment, "/push", message === undefined ? undefined : { message });
  }

  /** A new environment `to` ("name" or "team/name"): the live disk, or with `snapshot` one of this one's snapshots. */
  clone({ environment, to, snapshot, services: list, quotaGb }: CloneEnvironment) {
    const [owner, name] = owned(to);
    return this.at("POST", environment, "/clone", {
      name, owner, snapshot_id: snapshot, quota_gb: quotaGb,
      services: list ? services(list) : undefined,
    });
  }

  /**
   * Put THIS environment back to one of its own snapshots, services and data (202). A persistent
   * service the snapshot lacks is a 409 naming it; list it in `discard` to accept losing its data.
   */
  restore({ environment, snapshot, discard = [] }: RestoreEnvironment) {
    return this.at("POST", environment, "/restore", { snapshot_id: snapshot, discard });
  }

  private at(method: string, environment: string, tail = "", body?: unknown) {
    const [owner, name] = owned(environment);
    return this.c.request(method, `/v1/environments/${seg(name)}${tail}`, body, { owner });
  }
}

/** `"name"` → [undefined, name]; `"team/name"` → [team, name]. */
function owned(s: string): [string | undefined, string] {
  const i = s.lastIndexOf("/");
  if (i < 0) return [undefined, s];
  const owner = s.slice(0, i);
  const name = s.slice(i + 1);
  if (!owner || !name || owner.includes("/")) throw new Error(`expected name or team/name, got ${JSON.stringify(s)}`);
  return [owner, name];
}

/** The server requires every field; fill the ones a caller leaves out. */
function services(list: Service[] = []) {
  return list.map((sv) => ({ command: [], env: {}, mounts: [], ports: [], ...sv }));
}

/** A session: `"name"` (personal), `"team/name"`, or its id (`ses-…`). */
export interface SessionRef {
  session: string;
}

export interface Intercept extends SessionRef {
  service: string;
  /** A member workspace: its id, name or `"team/name"`, sent verbatim. */
  workspace: string;
  /** Each service port → the workspace port that answers it (`{ 8080: 3000 }`). */
  ports?: Record<number, number>;
}

/**
 * Working sessions, over `/v1/sessions`: some of YOUR workspaces in one team, connected to one
 * environment. Many people's sessions connect to the same environment and collaborate through it;
 * an intercept a session takes is environment-wide, and the session owns it — only it can release
 * it, and removing the workspace or deleting the session releases it.
 *
 * A session is named like an environment — `"name"` (personal) or `"team/name"` — or by its id
 * (`ses-…`); a workspace by its id, name or `"team/name"`, sent verbatim.
 */
export class Sessions {
  constructor(private readonly c: Kloudlite) {}

  /** `environment`, when given, must be the session's team's: an id, a name or `"team/name"`;
   * without one the session connects to nothing until `connect`. `ttlSecs` (600..=2592000) deletes
   * the session after that long unused. */
  create({ session, environment, ttlSecs }: SessionRef & { environment?: string; ttlSecs?: number }) {
    const [team, name] = owned(session);
    return this.c.request("POST", "/v1/sessions", { name, team, environment, ttl_secs: ttlSecs });
  }

  /** Seconds unused before the session is deleted (600..=2592000); `null` clears it. */
  setTtl({ session, ttlSecs }: SessionRef & { ttlSecs: number | null }) {
    return this.at("PUT", session, "/ttl", { ttl_secs: ttlSecs });
  }

  list({ team }: { team?: string } = {}) {
    return this.c.request<unknown[]>("GET", "/v1/sessions", undefined, { team });
  }

  /** The session with its `workspaces` and the `intercepts` it holds. */
  get({ session }: SessionRef) {
    return this.at("GET", session);
  }

  /** Releases its intercepts and lets its workspaces go — never deletes a workspace. */
  delete({ session }: SessionRef) {
    return this.at("DELETE", session);
  }

  /**
   * Sets, switches or (`null`) clears the session's environment. Its intercepts in the old
   * environment are released first; deleting an environment disconnects its sessions the same way.
   */
  connect({ session, environment }: SessionRef & { environment: string | null }) {
    return this.at("PUT", session, "/environment", { environment });
  }

  /** A workspace is in at most one session; while in one it resolves that session's environment. */
  add({ session, workspace }: SessionRef & WorkspaceRef) {
    return this.at("POST", session, "/workspaces", { workspace });
  }

  /** Releases whatever the workspace intercepts in this session first. */
  remove({ session, workspace }: SessionRef & WorkspaceRef) {
    return this.at("DELETE", session, `/workspaces/${seg(workspace)}`);
  }

  /**
   * Send everything that calls `service` in the environment to `workspace` (a member), `ports`
   * mapping each service port to the workspace port that answers it (`{ 8080: 3000 }`).
   */
  intercept({ session, service, workspace, ports = {} }: Intercept) {
    return this.at("POST", session, "/intercepts", {
      service,
      workspace,
      ports: Object.entries(ports).map(([s, w]) => ({ service: Number(s), workspace: Number(w) })),
    });
  }

  release({ session, service }: SessionRef & { service: string }) {
    return this.at("DELETE", session, `/intercepts/${seg(service)}`);
  }

  private at(method: string, session: string, tail = "", body?: unknown) {
    const [owner, name] = owned(session);
    return this.c.request(method, `/v1/sessions/${seg(name)}${tail}`, body, { owner });
  }
}

/** A btrfs volume, by its name. */
export interface VolumeRef {
  volume: string;
}

/**
 * An owner's named secrets, over `/v1/secrets`. A value goes in and never comes back: `list`
 * returns names with who and when. Reference one from a workspace env as `"${{ secrets.NAME }}"`.
 */
export class Secrets {
  constructor(private readonly c: Kloudlite) {}

  async set({ owner, name, value }: { owner: string; name: string; value: string }): Promise<void> {
    await this.c.request("PUT", `/v1/secrets/${seg(owner)}/${seg(name)}`, { value });
  }

  list({ owner }: { owner: string }) {
    return this.c.request<{ name: string; updated_at: string; updated_by: string }[]>(
      "GET", `/v1/secrets/${seg(owner)}`);
  }

  /** 409 while a workspace or environment still references it; the body names them. */
  async delete({ owner, name }: { owner: string; name: string }): Promise<void> {
    await this.c.request("DELETE", `/v1/secrets/${seg(owner)}/${seg(name)}`);
  }
}

/**
 * The btrfs volumes behind workspaces and environments: snapshot history, restore targets, and
 * the two deletes — over `/v1/volumes`.
 */
export class Volumes {
  constructor(private readonly c: Kloudlite) {}

  /** `kind` is `workspace` or `environment`; `owner` a handle or team slug. */
  list({ kind, owner }: { kind?: "workspace" | "environment"; owner?: string } = {}) {
    return this.c.request<unknown[]>("GET", "/v1/volumes", undefined, { kind, owner });
  }

  /** The chain of PUSHED snapshots, newest first — each row a restore target. */
  history({ volume }: VolumeRef) {
    return this.c.request("GET", `/v1/volumes/${seg(volume)}/history`);
  }

  refs({ volume }: VolumeRef) {
    return this.c.request("GET", `/v1/volumes/${seg(volume)}/refs`);
  }

  /**
   * Refused (409) for a sync point or a running worktree's base; deleting a detached volume's
   * last snapshot deletes the volume.
   */
  deleteSnapshot({ volume, snapshot }: VolumeRef & { snapshot: string }) {
    return this.c.request("DELETE", `/v1/volumes/${seg(volume)}/snapshots/${seg(snapshot)}`);
  }

  /** Takes a DETACHED volume with all its snapshots; refused while a working copy remains. */
  delete({ volume }: VolumeRef) {
    return this.c.request("DELETE", `/v1/volumes/${seg(volume)}`);
  }
}

export type SandboxPhase = "pending" | "starting" | "running" | "terminated";

/** Why a sandbox ended; set once its phase is `terminated`. */
export type SandboxReason =
  | "Terminated" | "Timeout" | "Idle" | "Exited" | "Failed" | "Unplaceable" | "SourceGone" | "Lost"
  | "EnvironmentGone";

/** A sandbox as `/v1/sandboxes` answers it — wire spelling, as every other doc here. */
export interface SandboxDoc {
  id: string;
  owner: string;
  team: string | null;
  region: string;
  image: string;
  packages: string[];
  phase: SandboxPhase;
  reason: SandboxReason | null;
  message: string | null;
  created_at: string | null;
  started_at: string | null;
  finished_at: string | null;
  timeout_secs: number;
  idle_timeout_secs: number | null;
  network: "internet" | "none";
  cpu: number;
  memory_gb: number;
  disk_gb: number;
  from: { workspace: string } | { snapshot: string } | null;
  /** The session it is a member of, or null. */
  session: string | null;
}

/** Every field optional: absent means the server's default. */
export interface CreateSandbox {
  image?: string;
  /** nixpkgs attributes, optionally pinned (`attr@version`). */
  packages?: string[];
  /** Start from a copy of one of YOUR workspaces: its id, `"name"` or `"team/name"`. */
  fromWorkspace?: string;
  /** Start from a snapshot id the caller may read. */
  fromSnapshot?: string;
  team?: string;
  /** Lifetime in seconds from the moment it runs (server default 300, 10..=86 400). */
  timeoutS?: number;
  /** Terminate after this many seconds with no tool call (10..=86 400). */
  idleTimeoutS?: number;
  network?: "internet" | "none";
  env?: Record<string, string>;
  /** Cores (1..=8). */
  cpu?: number;
  memoryGb?: number;
  diskGb?: number;
  /**
   * Make it a member of one of your sessions in `team` from birth — an id, `"name"` or
   * `"team/name"`; the session's environment is the only way a sandbox reaches one.
   */
  session?: string;
}

export interface ExecSandbox {
  /** argv (`["ls", "-la"]`), or one string run by the shell. */
  cmd: string | string[];
  /** Relative to `~/workspace`. */
  cwd?: string;
  env?: Record<string, string>;
}

/**
 * {@link Sandboxes.run}: the command plus every {@link CreateSandbox} field except the two whose
 * names the command takes — `env` is the COMMAND's environment and `timeoutS` how long to wait
 * for it (absent: until it exits), never the sandbox's.
 */
export interface RunSandbox extends Omit<CreateSandbox, "env" | "timeoutS">, ExecSandbox {
  timeoutS?: number;
}

export interface RunResult {
  /** `null` when the process ended on a signal. */
  exitCode: number | null;
  stdout: string;
  stderr: string;
  /** The sandbox's id — already terminated. */
  sandbox: string;
}

/** One NDJSON frame of a process's output stream; the `exit` frame is always the last. */
export type ProcessFrame =
  | { dropped: number; dropped_err: number }
  | { stream: "stdout" | "stderr"; data: string }
  | { dropped_frames: number }
  | { exit: number | null };

/** A wait that ran out its `timeoutS` — the thing waited on carries on. */
export class TimeoutError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "TimeoutError";
  }
}

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

/** Bytes as base64 without Node: `Buffer` when the runtime has one, else `btoa` over a binary string. */
function base64(data: string | Uint8Array): string {
  const bytes = typeof data === "string" ? new TextEncoder().encode(data) : data;
  const B = (globalThis as { Buffer?: { from(b: Uint8Array): { toString(enc: "base64"): string } } }).Buffer;
  if (B) return B.from(bytes).toString("base64");
  let bin = "";
  // Chunked: spreading a large array into fromCharCode overflows the argument limit.
  for (let i = 0; i < bytes.length; i += 0x8000) bin += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(bin);
}

/**
 * Short-lived, isolated containers for running code, over `/v1/sandboxes` — Modal-shaped:
 *
 *     const sb = await kl.sandboxes.create({ image: "python:3.12" });
 *     const p = await sb.exec({ cmd: ["python", "-c", "print(input())"] });
 *     await p.stdin.write("hi\n");
 *     await p.stdin.writeEof();
 *     console.log(await p.wait(), await p.stdout.read());
 *     await sb.terminate();
 *
 * {@link create} answers at once; the first {@link Sandbox.exec} waits for it to run. A sandbox
 * ends on {@link Sandbox.terminate}, its `timeoutS`, or its `idleTimeoutS` — never on its own
 * otherwise, so terminate what you create (or use {@link run}, which always does).
 */
export class Sandboxes {
  constructor(private readonly c: Kloudlite) {}

  /** Returns immediately, phase `pending`/`starting`; nothing waits for it to run. */
  async create(opts: CreateSandbox = {}): Promise<Sandbox> {
    const doc = await this.c.request<SandboxDoc>("POST", "/v1/sandboxes", createBody(opts));
    return new Sandbox(this.c, doc);
  }

  async get({ sandbox }: { sandbox: string }): Promise<Sandbox> {
    return new Sandbox(this.c, await this.c.request<SandboxDoc>("GET", `/v1/sandboxes/${seg(sandbox)}`));
  }

  /** The caller's sandboxes, newest first; `team` scopes to theirs in that team. */
  async list({ team }: { team?: string } = {}): Promise<Sandbox[]> {
    const docs = await this.c.request<SandboxDoc[]>("GET", "/v1/sandboxes", undefined, { team });
    return (docs ?? []).map((d) => new Sandbox(this.c, d));
  }

  /**
   * Create, wait until running, run `cmd`, wait for it, read all its output — and terminate the
   * sandbox whatever happened. A terminate that fails after an earlier error never hides it.
   */
  async run({ cmd, cwd, env, timeoutS, ...create }: RunSandbox): Promise<RunResult> {
    // A command allowed longer than the sandbox's default lifetime must not be cut by it: the
    // sandbox outlives the wait by a minute, within the server's 10 s..24 h.
    const lifetime = timeoutS == null ? undefined : Math.min(86_400, Math.max(10, Math.ceil(timeoutS) + 60));
    const sb = await this.create({ ...create, timeoutS: lifetime });
    let failed = false;
    try {
      await sb.waitRunning();
      const p = await sb.exec({ cmd, cwd, env });
      const exitCode = await p.wait({ timeoutS });
      const [stdout, stderr] = await Promise.all([p.stdout.read(), p.stderr.read()]);
      return { exitCode, stdout, stderr, sandbox: sb.id };
    } catch (e) {
      failed = true;
      throw e;
    } finally {
      try {
        await sb.terminate();
      } catch (e) {
        if (!failed) throw e;
      }
    }
  }
}

function createBody(o: CreateSandbox) {
  // JSON.stringify drops undefined, so an unset option is absent from the body.
  return {
    team: o.team, image: o.image, packages: o.packages,
    from_workspace: o.fromWorkspace, from_snapshot: o.fromSnapshot,
    timeout_secs: o.timeoutS, idle_timeout_secs: o.idleTimeoutS,
    network: o.network, env: o.env, cpu: o.cpu, memory_gb: o.memoryGb, disk_gb: o.diskGb,
    session: o.session,
  };
}

/** One sandbox: the last doc seen ({@link doc}) plus the calls on it. */
export class Sandbox {
  constructor(
    private readonly c: Kloudlite,
    /** The last doc the server answered; {@link refresh} re-reads it. */
    public doc: SandboxDoc,
  ) {}

  get id(): string {
    return this.doc.id;
  }

  get phase(): SandboxPhase {
    return this.doc.phase;
  }

  /** Why it ended — `null` until it is `terminated`. */
  get reason(): SandboxReason | null {
    return this.doc.reason ?? null;
  }

  async refresh(): Promise<this> {
    this.doc = await this.c.request<SandboxDoc>("GET", this.path());
    return this;
  }

  /**
   * Poll until `running` (0.5 s, backing off to 2 s). Rejects with an {@link ApiError} (409)
   * naming the reason if it is `terminated` instead, or a {@link TimeoutError} after `timeoutS`.
   */
  async waitRunning({ timeoutS = 120 }: { timeoutS?: number } = {}): Promise<this> {
    const deadline = Date.now() + timeoutS * 1000;
    for (let delay = 500; ; delay = Math.min(delay * 2, 2000)) {
      if (this.phase === "running") return this;
      if (this.phase === "terminated") throw this.ended();
      const left = deadline - Date.now();
      if (left <= 0) throw new TimeoutError(`sandbox ${this.id} not running after ${timeoutS}s (phase ${this.phase})`);
      await sleep(Math.min(delay, left));
      await this.refresh();
    }
  }

  /** Start `cmd` in the background (waiting for the sandbox to run first) and hand back its {@link Process}. */
  async exec({ cmd, cwd, env }: ExecSandbox): Promise<Process> {
    if (this.phase !== "running") await this.waitRunning();
    // Always detached: a job the api would hold open is clamped to 25 s; wait() polls instead.
    const out = await this.tool<{ id: string }>("exec", { cmd, cwd, env, detach: true });
    return new Process(this, String(out.id));
  }

  /** The tool's JSON, as is; `path` is relative to `~/workspace`. */
  read({ path }: { path: string }) {
    return this.tool("read", { path });
  }

  write({ path, content }: { path: string; content: string }) {
    return this.tool("write", { path, content });
  }

  /** Ask it to end (202); the doc it answers is kept. {@link wait} sees it reach `terminated`. */
  async terminate(): Promise<this> {
    this.doc = (await this.c.request<SandboxDoc>("POST", this.path("/terminate"))) ?? this.doc;
    return this;
  }

  /** Poll until `terminated` and answer the reason; a {@link TimeoutError} after `timeoutS` (absent: no limit). */
  async wait({ timeoutS }: { timeoutS?: number } = {}): Promise<SandboxReason | null> {
    const deadline = timeoutS === undefined ? Infinity : Date.now() + timeoutS * 1000;
    for (let delay = 500; ; delay = Math.min(delay * 2, 2000)) {
      if (this.phase === "terminated") return this.reason;
      const left = deadline - Date.now();
      if (left <= 0) throw new TimeoutError(`sandbox ${this.id} not terminated after ${timeoutS}s (phase ${this.phase})`);
      await sleep(Math.min(delay, left));
      await this.refresh();
    }
  }

  /** Delete the record (204). */
  async delete(): Promise<void> {
    await this.c.request("DELETE", this.path());
  }

  /**
   * Any tool of the sandbox's tool server by name, `args` spelled as its schema spells them —
   * the same tools as {@link Ide}. A sandbox that is not `running` answers 409.
   */
  tool<T = unknown>(name: string, args: Record<string, unknown>): Promise<T> {
    return this.c.request<T>("POST", this.path(`/tools/${seg(name)}`), args);
  }

  /** @internal The process's NDJSON output frames, from the start; {@link ProcessOutput} reads them. */
  frames(pid: string): AsyncGenerator<ProcessFrame> {
    return this.c[STREAM](this.path(`/processes/${seg(pid)}/stream`), { since: 0, since_err: 0 }) as AsyncGenerator<ProcessFrame>;
  }

  private path(tail = ""): string {
    return `/v1/sandboxes/${seg(this.id)}${tail}`;
  }

  private ended(): ApiError {
    const why = this.reason ?? "unknown reason";
    return new ApiError(409, `sandbox ${this.id} is terminated (${why})${this.doc.message ? `: ${this.doc.message}` : ""}`);
  }
}

/** A process started by {@link Sandbox.exec}, by its `id` (the tool server's process id). */
export class Process {
  readonly stdin: ProcessStdin;
  readonly stdout: ProcessOutput;
  readonly stderr: ProcessOutput;
  private code: number | null | undefined;

  constructor(
    readonly sandbox: Sandbox,
    readonly id: string,
  ) {
    this.stdin = new ProcessStdin(sandbox, id);
    this.stdout = new ProcessOutput(sandbox, id, "stdout");
    this.stderr = new ProcessOutput(sandbox, id, "stderr");
  }

  /** The exit code once a {@link wait} or {@link poll} has seen it exit; `undefined` until then. */
  get returnCode(): number | null | undefined {
    return this.code;
  }

  /**
   * Block until it exits and answer the exit code (`null` on a signal), in polls of at most 20 s
   * — the api's own ceiling. A {@link TimeoutError} after `timeoutS` (absent: no limit).
   */
  async wait({ timeoutS }: { timeoutS?: number } = {}): Promise<number | null> {
    const deadline = timeoutS === undefined ? Infinity : Date.now() + timeoutS * 1000;
    for (;;) {
      const left = Math.max(0, deadline - Date.now());
      const st = await this.waitOnce(Math.min(left, 20_000));
      if (st.state === "exited") return st.exit_code;
      if (deadline - Date.now() <= 0) throw new TimeoutError(`process ${this.id} still running after ${timeoutS}s`);
    }
  }

  /** The exit code if it has exited, `null` while it runs — one look, no wait. */
  async poll(): Promise<number | null> {
    const st = await this.waitOnce(0);
    return st.state === "exited" ? st.exit_code : null;
  }

  kill({ signal }: { signal?: "TERM" | "KILL" } = {}) {
    return this.sandbox.tool("process_kill", { id: this.id, signal });
  }

  private async waitOnce(ms: number) {
    const st = await this.sandbox.tool<{ state: "running" | "exited"; exit_code: number | null; timed_out: boolean }>(
      "process_wait", { id: this.id, timeout_ms: Math.floor(ms) },
    );
    if (st.state === "exited") this.code = st.exit_code;
    return st;
  }
}

/** A process's stdin. Each write is delivered when its promise resolves. */
export class ProcessStdin {
  constructor(
    private readonly sb: Sandbox,
    private readonly id: string,
  ) {}

  /** A string is sent as UTF-8; bytes as they are. */
  async write(data: string | Uint8Array): Promise<void> {
    await this.sb.tool("process_write", { id: this.id, data_base64: base64(data) });
  }

  /** Close stdin: the process reads end-of-file. */
  async writeEof(): Promise<void> {
    await this.sb.tool("process_close_stdin", { id: this.id });
  }

  /** Nothing to flush — kept for Modal's shape, since every write has landed when it resolves. */
  async drain(): Promise<void> {}
}

/**
 * One of a process's output streams, followed from its first byte until the process exits:
 * {@link read} for all of it, `for await (const line of p.stdout)` for lines as they arrive
 * (split on `\n`, without it; a final unterminated line is yielded last). Each read or loop
 * follows the stream afresh, so both can be used, and more than once.
 */
export class ProcessOutput implements AsyncIterable<string> {
  constructor(
    private readonly sb: Sandbox,
    private readonly id: string,
    private readonly which: "stdout" | "stderr",
  ) {}

  /** The whole stream's text, once the process has exited. */
  async read(): Promise<string> {
    let out = "";
    for await (const chunk of this.chunks()) out += chunk;
    return out;
  }

  async *[Symbol.asyncIterator](): AsyncGenerator<string> {
    let buf = "";
    for await (const chunk of this.chunks()) {
      buf += chunk;
      let nl: number;
      while ((nl = buf.indexOf("\n")) >= 0) {
        yield buf.slice(0, nl);
        buf = buf.slice(nl + 1);
      }
    }
    if (buf) yield buf;
  }

  private async *chunks(): AsyncGenerator<string> {
    for await (const f of this.sb.frames(this.id)) {
      if ("stream" in f && f.stream === this.which) yield f.data;
      else if ("exit" in f) return;
    }
  }
}

/** One definition of `GET /v1/pipelines`; a broken one has `valid: false` and its `errors`. */
export interface PipelineDef {
  name: string;
  /** The path it was parsed from, e.g. `pipelines/ci.yaml`. */
  file: string;
  valid: boolean;
  errors: string[];
  /** The repositories it runs on. */
  repos: string[];
  /** Its triggers, as the definition spells them; `null` when it has none. */
  on: Record<string, unknown> | null;
  /** Its job names, in the order written. */
  jobs: string[];
}

/** `GET /v1/pipelines`: the definitions on `{owner}/pipelines`'s main branch at `sha`. */
export interface PipelineCatalog {
  repo: string;
  sha: string;
  pipelines: PipelineDef[];
  /** Errors in the shared actions, which belong to no one pipeline. */
  errors: string[];
}

export type RunState = "queued" | "running" | "success" | "failure" | "cancelled";
export type JobState = "pending" | "running" | "success" | "failure" | "skipped" | "cancelled";

/** One job of a run, matrix instances expanded: `key` is unique in the run (`test (postgres16)`). */
export interface RunJob {
  key: string;
  /** The job's name in the definition — what `needs` names. */
  job: string;
  matrix: Record<string, unknown>;
  needs: string[];
  state: JobState;
  sandbox: string | null;
  startedAtMs: number | null;
  finishedAtMs: number | null;
  exitCode: number | null;
  failedStep: string | null;
  detail: string | null;
}

/** A run as `/v1` answers it — the wire's camelCase, as stored. */
export interface RunDoc {
  id: string;
  /** Per repo, from 1: the name people say out loud. */
  number: number;
  pipeline: string;
  pipelinesRepo: string;
  pipelinesSha: string;
  /** `"owner/name"` of the repo the run is about. */
  repo: string;
  ref: string;
  sha: string;
  event: "manual" | "push" | "pull_request" | "schedule" | "pipeline";
  inputs: Record<string, unknown>;
  /** As requested: an environment, `{ clone: name }`, or `null` for the pipeline's own. */
  environment: string | { clone: string } | null;
  requestedBy: string;
  requestedAtMs: number;
  state: RunState;
  cancelRequested: boolean;
  startedAtMs: number | null;
  finishedAtMs: number | null;
  detail: string | null;
  jobs: RunJob[];
}

/** One page of a job's log from a byte offset; poll with `next` until `complete`. */
export interface LogPage {
  data: string;
  next: number;
  /** The job has ended and every byte was returned. */
  complete: boolean;
  state: JobState;
}

/**
 * {@link Pipelines.run}: `owner`'s `pipeline` on `repo` (a name, or `"owner/name"` — one of
 * `owner`'s). `environment` runs every job that names none in that environment; `clone` instead
 * in a per-run copy of it, deleted when the run ends — one or the other.
 */
export interface RunPipeline {
  owner: string;
  pipeline: string;
  repo: string;
  /** A branch, tag, full ref or sha; default `main`. */
  ref?: string;
  /** The inputs the pipeline's `on.manual.inputs` declares. */
  inputs?: Record<string, string>;
  environment?: string;
  clone?: string;
}

/** A job of a run: its index in `jobs`, or its `key`. */
export type JobRef = number | string;

const RUN_TERMINAL: readonly RunState[] = ["success", "failure", "cancelled"];

function runsPath(repo: string, tail = ""): string {
  const [owner, name] = split(repo);
  return `/v1/repos/${seg(owner)}/${seg(name)}/runs${tail}`;
}

/**
 * CI pipelines, over `/v1/pipelines`: definitions live as YAML in an owner's `{owner}/pipelines`
 * repository and run against that owner's repositories, each job in its own sandbox. A run is
 * recorded on the repository it is about, so it is read back by that `repo` (`"owner/name"`):
 *
 *     const run = await kl.pipelines.run({ owner: "acme", pipeline: "ci", repo: "acme/payments" });
 *     for await (const chunk of run.follow({ job: "test" })) process.stdout.write(chunk);
 *     console.log(await run.wait()); // "success"
 */
export class Pipelines {
  constructor(private readonly c: Kloudlite) {}

  /** `owner` (a handle or team slug) defaults to the caller. */
  list({ owner }: { owner?: string } = {}) {
    return this.c.request<PipelineCatalog>("GET", "/v1/pipelines", undefined, { owner });
  }

  /**
   * Start a run by hand and answer it at once (202), `queued`. A definition that cannot run
   * rejects with an {@link ApiError} 422 whose message lists every reason, and nothing is recorded.
   */
  async run({ owner, pipeline, repo, ref, inputs, environment, clone }: RunPipeline): Promise<Run> {
    if (environment !== undefined && clone !== undefined) throw new Error("pass environment or clone, not both");
    const body = { repo, ref, inputs, environment: clone !== undefined ? { clone } : environment };
    const doc = await this.c.request<RunDoc>("POST", `/v1/pipelines/${seg(owner)}/${seg(pipeline)}/runs`, body);
    return new Run(this.c, doc);
  }

  /** `repo`'s runs, newest first; `pipeline` keeps one pipeline's. */
  async runs({ repo, pipeline, limit }: RepoRef & { pipeline?: string; limit?: number }): Promise<Run[]> {
    const docs = await this.c.request<RunDoc[]>("GET", runsPath(repo), undefined, { pipeline, limit });
    return (docs ?? []).map((d) => new Run(this.c, d));
  }

  /** One run of `repo` by its id (`1760400000000-0a1b2c`). */
  async getRun({ repo, run }: RepoRef & { run: string }): Promise<Run> {
    return new Run(this.c, await this.c.request<RunDoc>("GET", runsPath(repo, `/${seg(run)}`)));
  }
}

/** One pipeline run: the last doc seen ({@link doc}) plus the calls on it. */
export class Run {
  constructor(
    private readonly c: Kloudlite,
    /** The last doc the server answered; {@link refresh} re-reads it. */
    public doc: RunDoc,
  ) {}

  get id(): string {
    return this.doc.id;
  }

  get number(): number {
    return this.doc.number;
  }

  get state(): RunState {
    return this.doc.state;
  }

  get jobs(): RunJob[] {
    return this.doc.jobs ?? [];
  }

  async refresh(): Promise<this> {
    this.doc = await this.c.request<RunDoc>("GET", this.path());
    return this;
  }

  /**
   * Ask it to stop: a queued run is cancelled at once, a running one once its worker has ended
   * its sandboxes — {@link wait} sees it reach `cancelled`.
   */
  async cancel(): Promise<this> {
    this.doc = (await this.c.request<RunDoc>("POST", this.path("/cancel"))) ?? this.doc;
    return this;
  }

  /**
   * Poll every `pollS` (default 5) until the run has ended and answer its final state; a
   * {@link TimeoutError} after `timeoutS` (absent: no limit).
   */
  async wait({ timeoutS, pollS = 5 }: { timeoutS?: number; pollS?: number } = {}): Promise<RunState> {
    const deadline = timeoutS === undefined ? Infinity : Date.now() + timeoutS * 1000;
    while (!RUN_TERMINAL.includes(this.state)) {
      await this.refresh();
      if (RUN_TERMINAL.includes(this.state)) break;
      const left = deadline - Date.now();
      if (left <= 0) throw new TimeoutError(`run ${this.id} still ${this.state} after ${timeoutS}s`);
      await sleep(Math.min(pollS * 1000, left));
    }
    return this.state;
  }

  /** The job's whole log so far (combined stdout and stderr), every page of it. */
  async logs({ job }: { job: JobRef }): Promise<string> {
    const index = this.jobIndex(job);
    let out = "";
    for (let offset = 0; ; ) {
      const page = await this.page(index, offset);
      out += page.data ?? "";
      const next = page.next ?? offset;
      if (page.complete || next <= offset) return out;
      offset = next;
    }
  }

  /**
   * The job's log in chunks as it is written — from the first byte, polling every `pollS`
   * (default 5) while nothing new has landed — ending once the job has ended and every byte was
   * yielded: `for await (const chunk of run.follow({ job: "test" }))`.
   */
  async *follow({ job, pollS = 5 }: { job: JobRef; pollS?: number }): AsyncGenerator<string> {
    const index = this.jobIndex(job);
    for (let offset = 0; ; ) {
      const page = await this.page(index, offset);
      if (page.data) yield page.data;
      if (page.complete) return;
      const next = page.next ?? offset;
      if (next <= offset) await sleep(pollS * 1000);
      offset = Math.max(offset, next);
    }
  }

  private path(tail = ""): string {
    return runsPath(this.doc.repo, `/${seg(this.id)}${tail}`);
  }

  private page(index: number, offset: number): Promise<LogPage> {
    return this.c.request<LogPage>("GET", this.path(`/jobs/${index}/logs`), undefined, { offset });
  }

  private jobIndex(job: JobRef): number {
    const keys = this.jobs.map((j) => j.key);
    if (typeof job === "number") {
      if (!Number.isInteger(job) || job < 0 || job >= keys.length) {
        throw new Error(`run ${this.id} has ${keys.length} jobs, no job ${job}`);
      }
      return job;
    }
    const i = keys.indexOf(job);
    if (i < 0) throw new Error(`run ${this.id} has no job ${JSON.stringify(job)}; its jobs are ${JSON.stringify(keys)}`);
    return i;
  }
}

/** The server answers errors as `{"error": …}` or plain text; take either. */
function errorText(raw: string): string {
  const text = raw.trim();
  try {
    const parsed = JSON.parse(text);
    if (parsed && typeof parsed === "object" && "error" in parsed) return String(parsed.error);
  } catch {
    // plain text
  }
  return text || "request failed";
}
