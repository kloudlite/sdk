/**
 * Kloudlite SDK for TypeScript — zero dependencies, built on `fetch`.
 *
 * Runtime: ESM; Node 20.16+ or Bun (global `fetch` and `AbortSignal.timeout`; the login file is read with
 * `process.getBuiltinModule`, so there is no top-level await and no import of `node:fs`).
 * A runtime without `fs` (a browser) must pass `baseUrl` and `token`.
 *
 * Every module hangs off the one client, and every method takes ONE options object, so no caller
 * ever has to remember argument order:
 *
 *     import { Kloudlite } from "./index.ts";
 *     const kl = new Kloudlite();   // the login of `kloudlite`; see "Discovery" below
 *     const workspaces = await kl.workspaces.list();
 *
 * Discovery: the API origin and the token each come from the first source that has one:
 * 1. the options `baseUrl` / `token`; 2. the environment `KLOUDLITE_URL` / `KLOUDLITE_TOKEN`;
 * 3. `api` / `token` in `config.json` in `KLOUDLITE_CONFIG_DIR`, else `~/.config/kloudlite`.
 * `/v1` is added to the origin. With no token the constructor throws an Error carrying
 * `no Kloudlite login: pass token=, set KLOUDLITE_TOKEN, or run kloudlite login`.
 *
 * Option names are camelCase everywhere (`quotaGb`, `snapshot`, `timeoutS`); the
 * SDK maps them onto whatever the wire spells (`quota_gb`, `snapshot_id`,
 * `timeout_secs`). {@link Kloudlite.request} is the low-level escape hatch and takes the wire as-is.
 */

if (typeof fetch !== "function" || typeof AbortSignal?.timeout !== "function") {
  throw new Error("kloudlite SDK needs fetch and AbortSignal.timeout: Node 20.16+ or Bun");
}

export const VERSION = "0.2.0";
const USER_AGENT = `kloudlite-sdk-typescript/${VERSION}`;
const NO_LOGIN = "no Kloudlite login: pass token=, set KLOUDLITE_TOKEN, or run kloudlite login";

export interface KloudliteOptions {
  /** The API origin. Absent: `KLOUDLITE_URL`, else `api` in the `kloudlite` login's config.json. */
  baseUrl?: string;
  /** A CLI token. Absent: `KLOUDLITE_TOKEN`, else `token` in config.json. */
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

/** Module-private: a request that also answers the response headers, or the body as text. Not exported, so not API. */
const RAW = Symbol("raw");

interface Raw {
  text?: boolean;
  /** Milliseconds; `null` for none. Default the client's. */
  timeoutMs?: number | null;
}

type Mod = Record<string, (...a: unknown[]) => unknown>;

/** `config.json` of the `kloudlite` login; `{}` when it is absent, unreadable or not an object. */
function login(env: Record<string, string | undefined>): { api?: string; token?: string } {
  try {
    const proc = (globalThis as { process?: { getBuiltinModule?: (n: string) => Mod } }).process;
    const fs = proc?.getBuiltinModule?.("node:fs");
    const os = proc?.getBuiltinModule?.("node:os");
    const path = proc?.getBuiltinModule?.("node:path");
    if (!fs || !os || !path) return {};
    const dir = env.KLOUDLITE_CONFIG_DIR || (path.join as (...p: string[]) => string)((os.homedir as () => string)(), ".config", "kloudlite");
    const doc = JSON.parse((fs.readFileSync as (p: string, e: string) => string)((path.join as (...p: string[]) => string)(dir, "config.json"), "utf8"));
    return doc && typeof doc === "object" && !Array.isArray(doc) ? doc : {};
  } catch {
    return {};
  }
}

export class Kloudlite {
  readonly workspaces: Workspaces;
  readonly environments: Environments;
  readonly sessions: Sessions;
  readonly volumes: Volumes;
  readonly secrets: Secrets;
  readonly connectors: Connectors;
  readonly sandboxes: Sandboxes;
  readonly pipelines: Pipelines;
  private readonly baseUrl: string;
  private readonly token: string;
  private readonly timeoutMs: number;

  constructor(opts: KloudliteOptions = {}) {
    const env = (globalThis as { process?: { env?: Record<string, string | undefined> } }).process?.env ?? {};
    let base = opts.baseUrl || env.KLOUDLITE_URL;
    let token = opts.token || env.KLOUDLITE_TOKEN;
    if (!base || !token) {
      const cfg = login(env);
      base = base || (typeof cfg.api === "string" ? cfg.api : undefined);
      token = token || (typeof cfg.token === "string" ? cfg.token : undefined);
    }
    if (!token) throw new Error(NO_LOGIN);
    if (!base) throw new Error("no Kloudlite API URL: pass baseUrl, set KLOUDLITE_URL, or run kloudlite login");
    base = base.replace(/\/+$/, "");
    this.baseUrl = base.endsWith("/v1") ? base.slice(0, -3) : base; // paths carry /v1
    this.token = token;
    this.timeoutMs = opts.timeoutMs ?? 30_000;
    this.workspaces = new Workspaces(this);
    this.environments = new Environments(this);
    this.sessions = new Sessions(this);
    this.volumes = new Volumes(this);
    this.secrets = new Secrets(this);
    this.connectors = new Connectors(this);
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
    return this[RAW](method, path, body, query).then((r) => r.body as T);
  }

  /** @internal Body (JSON, or text with `text`) plus the response headers. */
  async [RAW](method: string, path: string, body?: unknown, query?: Query, o: Raw = {}): Promise<{ body: unknown; headers: Headers }> {
    const ms = o.timeoutMs === undefined ? this.timeoutMs : o.timeoutMs;
    const resp = await fetch(url(this.baseUrl, path, query), {
      method,
      headers: this.headers(o.text ? "text/plain" : "application/json", body !== undefined),
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: ms === null ? undefined : AbortSignal.timeout(ms),
    });
    const raw = await resp.text();
    if (!resp.ok) throw new ApiError(resp.status, errorText(raw));
    return { body: o.text ? raw : raw ? JSON.parse(raw) : null, headers: resp.headers };
  }

  private headers(accept: string, json: boolean): Record<string, string> {
    const headers: Record<string, string> = { "User-Agent": USER_AGENT, Accept: accept };
    headers.Authorization = `Bearer ${this.token}`;
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

const seg = encodeURIComponent;

/** A workspace: `"name"` (the caller's own), `"team/name"` (theirs in a team), or its id. */
export interface WorkspaceRef {
  workspace: string;
}

export interface CreateWorkspace extends WorkspaceRef {
  quotaGb: number;
  image?: string;
  /** nixpkgs attributes, optionally pinned (`attr@version`). */
  packages?: string[];
  /** Seconds unused (600..=2592000) before the workspace is deleted; absent, never. */
  ttlSecs?: number;
  /** Variable name to a literal value or a `"${{ secrets.NAME }}"` reference to the owner's secrets. */
  env?: Record<string, string>;
}

/** What a push of a workspace or an environment takes. */
export interface TakeSnapshot {
  message?: string;
  volume?: string;
  ttlSecs?: number;
  never?: boolean;
}

export interface CloneWorkspace extends WorkspaceRef {
  /** The new workspace's name. */
  to: string;
  task?: string;
}

export interface RestoreWorkspace {
  /** The PUSHED snapshot to graft onto: a volume name, `name@N` or a snapshot id. */
  snapshot: string;
  /** The new workspace's name; `"team/name"` also names the team. */
  to: string;
  /** Required for a team's volume (or write `to` as `"team/name"`). */
  team?: string;
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
   * `workspace` is split into the body's `name` and `team` (no team for a personal one);
   * `packages` are nixpkgs attributes, optionally pinned (`attr@version`). 202.
   */
  create({ workspace, quotaGb, image, packages, ttlSecs, env }: CreateWorkspace) {
    const [team, name] = owned(workspace);
    return this.c.request("POST", "/v1/workspaces", {
      name, quota_gb: quotaGb, team, image, packages, ttl_secs: ttlSecs, env,
    });
  }

  /** Seconds unused before the workspace is deleted (600..=2592000); `null` clears it. Not for a sandbox or a bench. */
  setTtl({ workspace, ttlSecs }: WorkspaceRef & { ttlSecs: number | null }) {
    return this.at("PUT", workspace, "/ttl", { ttl_secs: ttlSecs });
  }

  /**
   * Replace the whole env (literals or `"${{ secrets.NAME }}"`); takes effect on the next start. A
   * running workspace is only marked `restart_required` unless `restartOnEnvChange` is true.
   */
  setEnv({ workspace, env, restartOnEnvChange }: WorkspaceRef & { env: Record<string, string>; restartOnEnvChange?: boolean }) {
    return this.at("PUT", workspace, "/env", { env, restartOnEnvChange });
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

  /** Restart a running workspace so it gets the current env (202); 409 when not running. */
  restart({ workspace }: WorkspaceRef) {
    return this.at("POST", workspace, "/restart");
  }

  /**
   * Push a SNAPSHOT of the working copy — the one durable, restorable state (202). `volume` names
   * the volume and is required on its first push; `ttlSecs` (3600..=31536000) or `never: true`
   * set its expiry.
   */
  takeSnapshot({ workspace, message, volume, ttlSecs, never }: TakeSnapshot & WorkspaceRef) {
    return this.at("POST", workspace, "/snapshots", { message, volume, ttl_secs: ttlSecs, never });
  }

  /** The old name of {@link takeSnapshot}. */
  push(o: TakeSnapshot & WorkspaceRef) {
    return this.takeSnapshot(o);
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
  restore({ snapshot, to, team, image, packages, quotaGb }: RestoreWorkspace) {
    const [owner, name] = owned(to);
    return this.c.request("POST", "/v1/workspaces/restore", {
      name, snapshot_id: snapshot, team: team || owner, image, packages, quota_gb: quotaGb,
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
  /** The new environment: `"name"`; `"team/name"` only with `snapshot`. */
  to: string;
  snapshot?: string;
  /** Applies only with `snapshot`. */
  services?: Service[];
  /** Applies only with `snapshot`. */
  quotaGb?: number;
  /** Seconds unused (600..=2592000) before the clone is deleted; absent, never. */
  ttlSecs?: number;
}

export interface RestoreEnvironment extends EnvironmentRef {
  snapshot: string;
  /** Persistent services whose data the caller accepts losing. */
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
      name, owner, services: list?.length ? services(list) : undefined,
      quota_gb: quotaGb, snapshot_id: snapshot, ttl_secs: ttlSecs,
    });
  }

  /** The caller's environments and their teams'; `owner` narrows to one handle or team. */
  list({ owner }: { owner?: string } = {}) {
    return this.c.request<unknown[]>("GET", "/v1/environments", undefined, { owner });
  }

  get({ environment }: EnvironmentRef) {
    return this.at("GET", environment);
  }

  /**
   * Replace the WHOLE service list. A removed service keeps its folders, so re-adding finds its
   * data. A changed `env` only marks a running service `restartRequired` unless
   * `restartOnEnvChange` is true.
   */
  setServices({ environment, services: list, restartOnEnvChange }: EnvironmentRef & { services: Service[]; restartOnEnvChange?: boolean }) {
    return this.at("PATCH", environment, "", { services: services(list), restartOnEnvChange });
  }

  start({ environment }: EnvironmentRef) {
    return this.at("POST", environment, "/start");
  }

  stop({ environment }: EnvironmentRef) {
    return this.at("POST", environment, "/stop");
  }

  /** Restart one service so it gets the current env (202); 409 when the environment is not running. */
  restart({ environment, service }: EnvironmentRef & { service: string }) {
    return this.at("POST", environment, `/services/${seg(service)}/restart`);
  }

  /** Seconds the environment may sit unused (600..=2592000) before it is deleted; null clears. */
  setTtl({ environment, ttlSecs }: EnvironmentRef & { ttlSecs: number | null }) {
    return this.at("PUT", environment, "/ttl", { ttl_secs: ttlSecs });
  }

  /** The volume survives while a pushed snapshot references it. */
  delete({ environment }: EnvironmentRef) {
    return this.at("DELETE", environment);
  }

  /**
   * Push a SNAPSHOT of every service's data and the service list (202). `volume` names the
   * volume and is required on its first push.
   */
  takeSnapshot({ environment, message, volume, ttlSecs, never }: TakeSnapshot & EnvironmentRef) {
    return this.at("POST", environment, "/snapshots", { message, volume, ttl_secs: ttlSecs, never });
  }

  /** The old name of {@link takeSnapshot}. */
  push(o: TakeSnapshot & EnvironmentRef) {
    return this.takeSnapshot(o);
  }

  /** A new environment `to`: a copy of the live disk, or with `snapshot` of one of this one's snapshots. */
  clone({ environment, to, snapshot, services: list, quotaGb, ttlSecs }: CloneEnvironment) {
    const [owner, name] = owned(to);
    return this.at("POST", environment, "/clone", {
      name, owner, snapshot_id: snapshot, services: list?.length ? services(list) : undefined,
      quota_gb: quotaGb, ttl_secs: ttlSecs,
    });
  }

  /**
   * Put THIS environment back to one of its own snapshots, services and data (202). A persistent
   * service the snapshot lacks is a 409 naming it; list it in `discard` to accept losing its data.
   */
  restore({ environment, snapshot, discard }: RestoreEnvironment) {
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
 * it, and deleting the workspace or the session releases it.
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

  /** Stop the session and every workspace in it (202, `{stopped: [ids]}`); sandboxes and the environment are untouched. */
  stop({ session }: SessionRef) {
    return this.at("POST", session, "/stop");
  }

  /**
   * Create a workspace `workspace` (a bare name) managed by the session, in its team (201). The
   * session controls it from then on; it takes the inputs of {@link Workspaces.create}.
   */
  createWorkspace({ session, workspace, quotaGb, image, packages, ttlSecs, env }: SessionRef & CreateWorkspace) {
    return this.at("POST", session, "/workspaces", {
      name: workspace, quota_gb: quotaGb, image, packages, ttl_secs: ttlSecs, env,
    });
  }

  /** Delete a workspace the session manages, releasing its intercepts first (204); snapshots stay. */
  deleteWorkspace({ session, workspace }: SessionRef & WorkspaceRef) {
    return this.at("DELETE", session, `/workspaces/${seg(workspace)}`);
  }

  /**
   * Create a sandbox managed by the session (202), in its team and reaching its environment.
   * Same inputs as {@link Sandboxes.create} (`team` must be the session's).
   */
  async createSandbox({ session, ...opts }: SessionRef & CreateSandbox): Promise<Sandbox> {
    const doc = await this.at<SandboxDoc>("POST", session, "/sandboxes", sandboxBody(opts));
    return new Sandbox(this.c, doc);
  }

  /**
   * Send everything that calls `service` in the environment to `workspace` (a member), `ports`
   * mapping each service port to the workspace port that answers it (`{ 8080: 3000 }`).
   */
  intercept({ session, service, workspace, ports = {} }: Intercept) {
    return this.at("POST", session, "/intercepts", {
      service,
      workspace,
      ports: Object.entries(ports ?? {}).map(([s, w]) => ({ service: Number(s), workspace: Number(w) })),
    });
  }

  release({ session, service }: SessionRef & { service: string }) {
    return this.at("DELETE", session, `/intercepts/${seg(service)}`);
  }

  private at<T = unknown>(method: string, session: string, tail = "", body?: unknown) {
    const [owner, name] = owned(session);
    return this.c.request<T>(method, `/v1/sessions/${seg(name)}${tail}`, body, { owner });
  }
}

/** A btrfs volume, by its name. */
export interface VolumeRef {
  volume: string;
}

/** `"mine"` (default) is your own secret in the team `owner`; `"team"` is the team's shared one and needs an admin. */
export type SecretScope = "mine" | "team";

/**
 * An owner's named secrets, over `/v1/secrets`. A value goes in and never comes back: `list`
 * returns names with who and when. Reference one from a workspace env as `"${{ secrets.NAME }}"`.
 * `scope` `"mine"` (default) is `PUT /v1/secrets/{owner}/mine/{name}`; `"team"` the team's shared
 * `PUT /v1/secrets/{owner}/{name}`.
 */
export class Secrets {
  constructor(private readonly c: Kloudlite) {}

  async set({ owner, name, value, scope }: { owner: string; name: string; value: string; scope?: SecretScope }): Promise<void> {
    await this.c.request("PUT", path(owner, scope, name), { value });
  }

  list({ owner, scope }: { owner: string; scope?: SecretScope }) {
    return this.c.request<{ name: string; updated_at: string; updated_by: string }[]>("GET", path(owner, scope));
  }

  /** 409 while a workspace or environment still references it; the body names them. */
  async delete({ owner, name, scope }: { owner: string; name: string; scope?: SecretScope }): Promise<void> {
    await this.c.request("DELETE", path(owner, scope, name));
  }
}

function path(owner: string, scope: SecretScope = "mine", name?: string): string {
  if (scope !== "mine" && scope !== "team") throw new Error(`scope is 'mine' or 'team', got ${JSON.stringify(scope)}`);
  return `/v1/secrets/${seg(owner)}${scope === "mine" ? "/mine" : ""}${name === undefined ? "" : `/${seg(name)}`}`;
}

/** The provider accounts linked to an owner, over `/v1/connectors`. */
export class Connectors {
  constructor(private readonly c: Kloudlite) {}

  /** Connectors by name; `owner` is a handle or team slug, default your own. */
  list({ owner }: { owner?: string } = {}) {
    return this.c.request<unknown[]>("GET", "/v1/connectors", undefined, { owner });
  }

  /** Remove a connector (204); a team's needs an admin. */
  async delete({ name }: { name: string }): Promise<void> {
    await this.c.request("DELETE", `/v1/connectors/${seg(name)}`);
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

  /** Takes a DETACHED volume (by its id) with all its snapshots; refused while a working copy remains. */
  deleteById({ volume }: VolumeRef) {
    return this.c.request("DELETE", `/v1/volumes/${seg(volume)}`);
  }

  /** One named volume; `owner` is your handle or a team slug. */
  get({ owner, name }: { owner: string; name: string }) {
    return this.c.request("GET", named(owner, name));
  }

  /** The versions of a named volume, newest first. */
  versions({ owner, name }: { owner: string; name: string }) {
    return this.c.request("GET", named(owner, name, "/versions"));
  }

  /** Delete a named volume and every snapshot on it (204); the name is free again. */
  delete({ owner, name }: { owner: string; name: string }) {
    return this.c.request("DELETE", named(owner, name));
  }

  /** Delete version `n` (204); deleting the last one deletes the volume. */
  deleteVersion({ owner, name, n }: { owner: string; name: string; n: number }) {
    return this.c.request("DELETE", named(owner, name, `/versions/${Math.trunc(n)}`));
  }
}

function named(owner: string, name: string, tail = ""): string {
  return `/v1/volumes/${seg(owner)}/${seg(name)}${tail}`;
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
  /** The command's exit code once it exited; `null` when it ended another way. */
  exit_code: number | null;
  message: string | null;
  created_at: string | null;
  started_at: string | null;
  finished_at: string | null;
  timeout_secs: number;
  network: "internet" | "none";
  cpu: number;
  memory_gb: number;
  disk_gb: number;
  from: { workspace: string } | { snapshot: string } | null;
  /** The session it is a member of, or null. */
  session: string | null;
}

/** `cmd` is required; every other field is optional and absent means the server's default. */
export interface CreateSandbox {
  /** The program and its arguments, a non-empty array of strings. */
  cmd: string[];
  image?: string;
  /** nixpkgs attributes, optionally pinned (`attr@version`). */
  packages?: string[];
  /** Start from a copy of one of YOUR workspaces: its id, `"name"` or `"team/name"`. */
  fromWorkspace?: string;
  /** Start from a volume name, `name@N` or snapshot id the caller may read (exclusive with `fromWorkspace`). */
  fromSnapshot?: string;
  team?: string;
  /** Run `cmd` on a pty. */
  tty?: boolean;
  /** Keep input open for `attach`. */
  stdin?: boolean;
  /** Lifetime in seconds from the moment it runs (server default 300, 10..=86 400). */
  timeoutS?: number;
  network?: "internet" | "none";
  env?: Record<string, string>;
  /** Cores (1..=8). */
  cpu?: number;
  memoryGb?: number;
  diskGb?: number;
}

/**
 * {@link Sandboxes.run}: every {@link CreateSandbox} field; `timeoutS` is how long to wait for the
 * command (absent: until it exits) and, when given, sizes the sandbox's own lifetime to cover it
 * plus a minute.
 */
export type RunSandbox = CreateSandbox;

export interface RunResult {
  /** `null` when the sandbox ended some other way than the command exiting. */
  exitCode: number | null;
  /** The sandbox keeps one stream, so this holds the output and error output together. */
  stdout: string;
  /** Always `""`. */
  stderr: string;
  /** The sandbox's id — already terminated. */
  sandbox: string;
}

/** A wait that ran out its `timeoutS` — the thing waited on carries on. */
export class TimeoutError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "TimeoutError";
  }
}

const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));

/** The `POST /v1/sandboxes` body; refuses a missing `cmd` before anything is sent. */
function sandboxBody(o: CreateSandbox) {
  if (!Array.isArray(o.cmd) || o.cmd.length === 0 || !o.cmd.every((a) => typeof a === "string")) {
    throw new Error("cmd is required: the program and its arguments as a non-empty array of strings");
  }
  // JSON.stringify drops undefined, so an unset option is absent from the body.
  return {
    cmd: o.cmd, image: o.image, packages: o.packages,
    from_workspace: o.fromWorkspace, from_snapshot: o.fromSnapshot, team: o.team,
    tty: o.tty, stdin: o.stdin, timeout_secs: o.timeoutS, network: o.network,
    env: o.env, cpu: o.cpu, memory_gb: o.memoryGb, disk_gb: o.diskGb,
  };
}

/**
 * Throwaway, isolated machines that run ONE command, over `/v1/sandboxes`. A sandbox starts from
 * an image, from one of your workspaces (`fromWorkspace`) or from a pushed snapshot
 * (`fromSnapshot`), runs `cmd`, and ends when it exits or after `timeoutS` (server default 300):
 *
 *     const sb = await kl.sandboxes.create({ cmd: ["python3", "-c", "print(6 * 7)"], packages: ["python3"] });
 *     const code = await sb.wait();
 *     console.log(code, await sb.logs());
 *
 * A session's sandbox is made with {@link Sessions.createSandbox}.
 */
export class Sandboxes {
  constructor(private readonly c: Kloudlite) {}

  /** Returns immediately (202), usually still `pending`; nothing waits for it to run. */
  async create(opts: CreateSandbox): Promise<Sandbox> {
    const doc = await this.c.request<SandboxDoc>("POST", "/v1/sandboxes", sandboxBody(opts));
    return new Sandbox(this.c, doc);
  }

  /** One sandbox by id (a name does not resolve). */
  async get({ sandbox }: { sandbox: string }): Promise<Sandbox> {
    return new Sandbox(this.c, await this.c.request<SandboxDoc>("GET", `/v1/sandboxes/${seg(sandbox)}`));
  }

  /**
   * ONE page of the caller's sandboxes, newest first (`limit` 1..=200, default 50); `team` scopes
   * to that team. {@link listPage} also answers the next cursor.
   */
  async list(o: { team?: string; limit?: number; cursor?: string } = {}): Promise<Sandbox[]> {
    return (await this.listPage(o)).sandboxes;
  }

  /** A page, and the `X-Next-Cursor` to pass as `cursor` for the next one (`null` on the last page). */
  async listPage({ team, limit, cursor }: { team?: string; limit?: number; cursor?: string } = {}): Promise<{ sandboxes: Sandbox[]; nextCursor: string | null }> {
    const r = await this.c[RAW]("GET", "/v1/sandboxes", undefined, { team, limit, cursor });
    return {
      sandboxes: ((r.body as SandboxDoc[] | null) ?? []).map((d) => new Sandbox(this.c, d)),
      nextCursor: r.headers.get("X-Next-Cursor"),
    };
  }

  /**
   * One command in a fresh sandbox: create, wait for it to exit, read the output, and terminate —
   * ALWAYS, also when anything before it threw (a terminate that fails after an earlier error never
   * hides it). `timeoutS` bounds the wait ({@link TimeoutError}; absent waits as long as it takes).
   */
  async run(opts: RunSandbox): Promise<RunResult> {
    const { timeoutS } = opts;
    // A command allowed longer than the sandbox's default lifetime must not be cut by it: the
    // sandbox outlives the wait by a minute, within the server's 10 s..24 h.
    const lifetime = timeoutS == null ? opts.timeoutS : Math.min(86_400, Math.max(10, Math.ceil(timeoutS) + 60));
    const sb = await this.create({ ...opts, timeoutS: lifetime });
    let exitCode: number | null, stdout: string;
    try {
      exitCode = await sb.wait({ timeoutS });
      stdout = await sb.logs();
    } catch (e) {
      await sb.terminate().catch(() => {}); // never mask the failure that got us here
      throw e;
    }
    await sb.terminate();
    return { exitCode, stdout, stderr: "", sandbox: sb.id };
  }
}

/**
 * A handle on one sandbox: `id`, `phase` (`pending`, `starting`, `running`, `terminated`),
 * `reason` once terminated, `exitCode`, and the whole server {@link doc} as of the last answer —
 * {@link refresh} re-reads it.
 */
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

  /** The command's exit code once it exited; `null` before, or when it ended another way. */
  get exitCode(): number | null {
    return this.doc.exit_code ?? null;
  }

  async refresh(): Promise<this> {
    this.doc = await this.c.request<SandboxDoc>("GET", this.path());
    return this;
  }

  /**
   * Poll until `running` (0.5 s, backing off to 2 s; each poll is a GET). Rejects with an {@link ApiError} (409)
   * naming the reason if it is `terminated` instead, or a {@link TimeoutError} after `timeoutS`.
   */
  async waitRunning({ timeoutS = 120 }: { timeoutS?: number } = {}): Promise<this> {
    const deadline = Date.now() + timeoutS * 1000;
    for (let delay = 500; ; delay = Math.min(delay * 1.5, 2000)) {
      await this.refresh();
      if (this.phase === "running") return this;
      if (this.phase === "terminated") throw this.ended();
      const left = deadline - Date.now();
      if (left <= delay) throw new TimeoutError(`sandbox ${this.id} not running after ${timeoutS}s (phase ${this.phase})`);
      await sleep(delay);
    }
  }

  /**
   * The main process's output (stdout and stderr together) as text: the live output while it
   * runs, the last 256 KiB after. `follow: true` holds until the process ends. 409 while the
   * sandbox has no output yet (still starting).
   */
  async logs({ follow = false }: { follow?: boolean } = {}): Promise<string> {
    const r = await this.c[RAW]("GET", this.path("/logs"), undefined, { follow: follow ? "true" : undefined },
      { text: true, timeoutMs: follow ? null : undefined });
    return r.body as string;
  }

  /** Not in this SDK: `GET /v1/sandboxes/{id}/attach` is a WebSocket and this client uses only `fetch`. */
  attach(_o: { cols?: number; rows?: number } = {}): never {
    throw new Error("sandbox attach is a WebSocket and is not in the TypeScript SDK");
  }

  /** Stop it for good (202, or 200 if already ended); the record stays until {@link delete}. */
  async terminate(): Promise<this> {
    this.doc = (await this.c.request<SandboxDoc>("POST", this.path("/terminate"))) ?? this.doc;
    return this;
  }

  /**
   * Block until it has ended and answer its exit code (`null` when it ended some other way than
   * the command exiting; `reason` says how). Waits in steps of at most 20 s, the most the api
   * holds one open; a {@link TimeoutError} past `timeoutS` (absent: no limit).
   */
  async wait({ timeoutS }: { timeoutS?: number } = {}): Promise<number | null> {
    const deadline = timeoutS === undefined ? undefined : Date.now() + timeoutS * 1000;
    for (;;) {
      const leftMs = deadline === undefined ? 20_000 : Math.trunc(deadline - Date.now());
      if (leftMs <= 0) throw new TimeoutError(`sandbox ${this.id} still ${this.phase} after ${timeoutS} s`);
      const out = await this.c.request<Partial<SandboxDoc> | null>("POST", this.path("/wait"), { timeout_ms: Math.min(leftMs, 20_000) });
      const { phase, reason, exit_code, message } = out ?? {};
      this.doc = {
        ...this.doc,
        ...(phase !== undefined && { phase }), ...(reason !== undefined && { reason }),
        ...(exit_code !== undefined && { exit_code }), ...(message !== undefined && { message }),
      };
      if (this.phase === "terminated") return this.exitCode;
    }
  }

  /** Delete the record (204); a sandbox still running dies with it. */
  async delete(): Promise<void> {
    await this.c.request("DELETE", this.path());
  }

  private path(tail = ""): string {
    return `/v1/sandboxes/${seg(this.id)}${tail}`;
  }

  private ended(): ApiError {
    const why = this.reason ?? "unknown reason";
    return new ApiError(409, `sandbox ${this.id} is terminated (${why})${this.doc.message ? `: ${this.doc.message}` : ""}`);
  }
}

/** A pipeline definition: the JSON of the pipeline definition reference. `name` is the pipeline's. */
export type PipelineDefinition = { name: string } & Record<string, unknown>;

/** A pipeline document, as the definition routes answer it. */
export interface PipelineDoc {
  owner: string;
  name: string;
  /** The version shown, and the newest one. */
  version: number;
  latestVersion: number;
  createdBy: string;
  createdAtMs: number;
  definitionSha256: string;
  definition: PipelineDefinition;
  triggers: { schedule?: unknown[]; webhook?: boolean; after?: unknown };
  webhook: { set: boolean };
  /** The webhook URL with its secret: only in the answer of the save that made it. */
  webhookUrl?: string;
}

/** One row of `GET /v1/pipelines/{owner}`. */
export interface PipelineSummary {
  name: string;
  latestVersion: number;
  updatedAtMs: number;
  triggers: { schedule?: unknown[]; webhook?: boolean; after?: unknown };
}

/** One row of `GET /v1/pipelines/{owner}/{name}/versions`. */
export interface PipelineVersion {
  version: number;
  createdBy: string;
  createdAtMs: number;
  definitionSha256: string;
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
  /** What the job started from; absent for an empty home. */
  home?: { volume: string; name?: string; version?: number; snapshot: string };
  /** What the job saved; absent until the save is ready. */
  saved?: { volume: string; name?: string; version?: number; snapshot: string };
}

/** A run as `/v1` answers it — the wire's camelCase, as stored. */
export interface RunDoc {
  id: string;
  /** Per owner and pipeline, from 1: the name people say out loud. */
  number: number;
  pipeline: string;
  owner: string;
  /** The version the run was planned from; it cannot change. */
  pipelineVersion: number;
  definitionSha256: string;
  event: "manual" | "schedule" | "webhook" | "pipeline";
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
  /** The version's definition: only when reading one run, and only while the version exists. */
  definition: PipelineDefinition | null;
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
 * {@link Pipelines.run}: start `pipeline` of `owner`. `version` defaults to the newest.
 * `environment` runs every job that names none in that environment; `clone` instead in a per-run
 * copy of it — one or the other.
 */
export interface RunPipeline {
  owner: string;
  pipeline: string;
  version?: number;
  /** String values of the inputs the pipeline declares. */
  inputs?: Record<string, string>;
  environment?: string;
  clone?: string;
}

/** A job of a run: its index in `jobs`, or its `key`. */
export type JobRef = number | string;

const RUN_TERMINAL: readonly RunState[] = ["success", "failure", "cancelled"];

/**
 * Pipelines, over `/v1/pipelines`: a JSON definition stored as numbered versions, run job by job
 * in sandboxes.
 *
 *     await kl.pipelines.create({ owner: "acme", definition });
 *     const run = await kl.pipelines.run({ owner: "acme", pipeline: "test", inputs: { target: "staging" } });
 *     for await (const chunk of run.follow({ job: "test" })) process.stdout.write(chunk);
 *     console.log(await run.wait()); // "success"
 */
export class Pipelines {
  constructor(private readonly c: Kloudlite) {}

  private base(owner: string, name?: string): string {
    return `/v1/pipelines/${seg(owner)}` + (name === undefined ? "" : `/${seg(name)}`);
  }

  /** Save `definition` (its `name` is the pipeline's) as version 1. A webhook trigger's URL is in `webhookUrl`, this once. 409 if the name exists. */
  create({ owner, definition }: { owner: string; definition: PipelineDefinition }) {
    return this.c.request<PipelineDoc>("POST", this.base(owner), definition);
  }

  /**
   * Replace the definition: a changed one is the next version, an equal one saves nothing. To roll
   * back, send an old definition. A concurrent save rejects with an {@link ApiError} 409 (`changed; retry`).
   */
  update({ owner, name, definition }: { owner: string; name: string; definition: PipelineDefinition }) {
    return this.c.request<PipelineDoc>("PUT", this.base(owner, name), definition);
  }

  list({ owner }: { owner: string }) {
    return this.c.request<PipelineSummary[]>("GET", this.base(owner));
  }

  /** `version` defaults to the latest. */
  get({ owner, name, version }: { owner: string; name: string; version?: number }) {
    return this.c.request<PipelineDoc>("GET", this.base(owner, name), undefined, { version });
  }

  /** Oldest first. */
  versions({ owner, name }: { owner: string; name: string }) {
    return this.c.request<PipelineVersion[]>("GET", `${this.base(owner, name)}/versions`);
  }

  /** Delete the pipeline and its versions; finished runs stay. 409 while a run is queued or running. */
  async delete({ owner, name }: { owner: string; name: string }): Promise<void> {
    await this.c.request("DELETE", this.base(owner, name));
  }

  /** 409 for the latest version or one a queued or running run uses. */
  async deleteVersion({ owner, name, version }: { owner: string; name: string; version: number }): Promise<void> {
    await this.c.request("DELETE", `${this.base(owner, name)}/versions/${version}`);
  }

  /** A new webhook secret; the old URL stops at once. The URL is shown only here. */
  rotateWebhook({ owner, name }: { owner: string; name: string }) {
    return this.c.request<{ webhookUrl: string }>("POST", `${this.base(owner, name)}/webhook/rotate`);
  }

  /**
   * Start a run by hand and answer it at once (202), `queued`. A version that cannot run rejects
   * with an {@link ApiError} 422 whose message lists every reason, and nothing is recorded.
   */
  async run({ owner, pipeline, version, inputs, environment, clone }: RunPipeline): Promise<Run> {
    if (environment !== undefined && clone !== undefined) throw new Error("pass environment or clone, not both");
    const body = { version, inputs, environment: clone !== undefined ? { clone } : environment };
    return new Run(this.c, await this.c.request<RunDoc>("POST", `${this.base(owner, pipeline)}/runs`, body));
  }

  /** The pipeline's runs, newest first (`limit` default 50, at most 200). */
  async runs({ owner, pipeline, limit }: { owner: string; pipeline: string; limit?: number }): Promise<Run[]> {
    const docs = await this.c.request<RunDoc[]>("GET", `${this.base(owner, pipeline)}/runs`, undefined, { limit });
    return (docs ?? []).map((d) => new Run(this.c, d));
  }

  /** One run by its id (`1760400000000-0a1b2c`). */
  async getRun({ owner, pipeline, run }: { owner: string; pipeline: string; run: string }): Promise<Run> {
    return new Run(this.c, await this.c.request<RunDoc>("GET", `${this.base(owner, pipeline)}/runs/${seg(run)}`));
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
    return `/v1/pipelines/${seg(this.doc.owner)}/${seg(this.doc.pipeline)}/runs/${seg(this.id)}${tail}`;
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
