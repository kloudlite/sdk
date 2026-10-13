/**
 * Kloudlite SDK for TypeScript — zero dependencies, built on `fetch`.
 *
 * Auth is a Bearer token (a session or CLI token). Every module hangs off the
 * one client, and every repository is named by ONE `"owner/name"` string — the
 * owner half being a person's handle or a team slug:
 *
 *     import { Kloudlite } from "@kloudlite/sdk";
 *     const kl = new Kloudlite({ baseUrl: "https://api.example.com", token: "…" });
 *     const repo = await kl.codeRepos.create("acme/payments");
 */

export const VERSION = "0.1.0";
const USER_AGENT = `kloudlite-sdk-typescript/${VERSION}`;

export interface KloudliteOptions {
  baseUrl: string;
  /** The hub listener (models and datasets), e.g. `https://hub.example.com`. */
  hubUrl?: string;
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

export class Kloudlite {
  readonly codeRepos: CodeRepos;
  readonly modelRepos: HubRepos;
  readonly dataRepos: HubRepos;
  readonly workspaces: Workspaces;
  readonly volumes: Volumes;
  private readonly baseUrl: string;
  private readonly hubUrl?: string;
  private readonly token?: string;
  private readonly timeoutMs: number;

  constructor(opts: KloudliteOptions) {
    this.baseUrl = opts.baseUrl.replace(/\/+$/, "");
    this.hubUrl = opts.hubUrl?.replace(/\/+$/, "");
    this.token = opts.token;
    this.timeoutMs = opts.timeoutMs ?? 30_000;
    this.codeRepos = new CodeRepos(this);
    this.modelRepos = new HubRepos(this, "model");
    this.dataRepos = new HubRepos(this, "dataset");
    this.workspaces = new Workspaces(this);
    this.volumes = new Volumes(this);
  }

  /** The ceiling and what is against it; `owner` defaults to the caller's own. */
  quota(owner?: string) {
    return this.request("GET", "/v1/quota", undefined, { owner });
  }

  /**
   * One HTTP round trip. Resolves to parsed JSON (or `null` on an empty body);
   * rejects with {@link ApiError} on any non-2xx status.
   */
  request<T = unknown>(method: string, path: string, body?: unknown, query?: Query): Promise<T> {
    return this.send(this.baseUrl, method, path, body, query);
  }

  /** The same round trip against the hub listener (models and datasets). */
  hubRequest<T = unknown>(method: string, path: string, body?: unknown, query?: Query): Promise<T> {
    if (!this.hubUrl) throw new Error("this client has no hubUrl; pass one to reach models and datasets");
    return this.send(this.hubUrl, method, path, body, query);
  }

  private async send<T>(base: string, method: string, path: string, body?: unknown, query?: Query): Promise<T> {
    let url = base + path;
    if (query) {
      const q = new URLSearchParams();
      for (const [k, v] of Object.entries(query)) {
        if (v !== undefined) q.set(k, String(v));
      }
      const qs = q.toString();
      if (qs) url += `?${qs}`;
    }
    const headers: Record<string, string> = { "User-Agent": USER_AGENT, Accept: "application/json" };
    if (this.token) headers.Authorization = `Bearer ${this.token}`;
    if (body !== undefined) headers["Content-Type"] = "application/json";
    const resp = await fetch(url, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(this.timeoutMs),
    });
    const raw = await resp.text();
    if (!resp.ok) throw new ApiError(resp.status, errorText(raw));
    return (raw ? JSON.parse(raw) : null) as T;
  }
}

export interface Repo {
  _id: string;
  owner: string;
  name: string;
  public: boolean;
  description: string;
  created_by: string;
  created_at: number;
}

export interface Branch {
  name: string;
  oid: string;
  default: boolean;
}

export interface FileChange {
  path: string;
  /** Base64-encoded bytes; absent when `delete` is set. */
  contentBase64?: string;
  /** Absent keeps the mode the file already has. */
  executable?: boolean;
  delete?: boolean;
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

/**
 * Code repositories: `/v1/repos`. Every `repo` argument is one `"owner/name"`
 * string; only owner-scoped calls (`list`) take a bare handle.
 */
export class CodeRepos {
  constructor(private readonly c: Kloudlite) {}

  private path(repo: string, tail = ""): string {
    const [owner, name] = split(repo);
    return `/v1/repos/${seg(owner)}/${seg(name)}${tail}`;
  }

  // repos
  create(repo: string, opts?: { visibility?: "public" | "private"; description?: string }) {
    const [owner, name] = split(repo);
    return this.c.request<Repo>("POST", "/v1/repos", {
      owner,
      name,
      visibility: "private",
      description: "",
      ...opts,
    });
  }

  list(owner: string) {
    return this.c.request<Repo[]>("GET", "/v1/repos", undefined, { owner });
  }

  get(repo: string) {
    return this.c.request<Repo>("GET", this.path(repo));
  }

  update(repo: string, change: { description?: string; visibility?: "public" | "private" }) {
    return this.c.request("PATCH", this.path(repo), change);
  }

  delete(repo: string) {
    return this.c.request("DELETE", this.path(repo));
  }

  /**
   * A new repo `dst` copied from `src`: objects and refs, never its pull requests
   * or settings. Private by default, whatever the source was.
   */
  fork(src: string, dst: string, opts?: { visibility?: "public" | "private"; description?: string }) {
    const [owner, name] = split(dst);
    return this.c.request<Repo>("POST", this.path(src, "/fork"), {
      owner,
      name,
      visibility: "private",
      description: "",
      ...opts,
    });
  }

  /**
   * A new repo whose contents are fetched from a PUBLIC git remote — `git` is a strict
   * `https://` URL (GitHub is one host among any). Answers 202: the repo exists but the fetch
   * runs in the background; poll {@link importStatus}.
   */
  importGit(repo: string, git: string, opts?: { visibility?: "public" | "private"; description?: string }) {
    const [owner, name] = split(repo);
    return this.c.request<Repo>("POST", "/v1/repos/import", {
      owner,
      name,
      git,
      visibility: "private",
      description: "",
      ...opts,
    });
  }

  /** `{from, state: queued|running|done|failed, detail?}` for an imported repo. */
  importStatus(repo: string) {
    return this.c.request("GET", this.path(repo, "/import"));
  }

  // branches
  branches(repo: string) {
    return this.c.request<Branch[]>("GET", this.path(repo, "/branches"));
  }

  /** `oid` is the head the caller saw; the delete is refused if the branch moved. */
  deleteBranch(repo: string, branch: string, oid: string) {
    return this.c.request("DELETE", this.path(repo, `/branches/${seg(branch)}`), undefined, { oid });
  }

  // protection
  protection(repo: string) {
    return this.c.request("GET", this.path(repo, "/protection"));
  }

  protect(repo: string, rule: { pattern: string; remove?: boolean; no_force?: boolean; no_delete?: boolean }) {
    return this.c.request("POST", this.path(repo, "/protection"), rule);
  }

  // pull requests
  pulls(repo: string, query?: { state?: string; limit?: number }) {
    return this.c.request<unknown[]>("GET", this.path(repo, "/pulls"), undefined, query);
  }

  pull(repo: string, number: number) {
    return this.c.request("GET", this.path(repo, `/pulls/${number}`));
  }

  /**
   * `head_repo` (another `"owner/name"`) makes it a cross-repo pull: `head` then
   * names a branch of that fork, proposed onto this repo's `base`.
   */
  openPull(
    repo: string,
    opts: { title: string; head: string; base: string; description?: string; head_repo?: string },
  ) {
    if (opts.head_repo !== undefined) split(opts.head_repo);
    return this.c.request("POST", this.path(repo, "/pulls"), { description: "", ...opts });
  }

  comment(repo: string, number: number, body: string) {
    return this.c.request("POST", this.path(repo, `/pulls/${number}/comments`), { body });
  }

  /** The merge is a job: the server answers 202 and the pull reports where it got to. */
  merge(repo: string, number: number, strategy: "fast-forward" | "squash" | "merge" | "rebase" = "fast-forward") {
    return this.c.request("POST", this.path(repo, `/pulls/${number}/merge`), undefined, { strategy });
  }

  close(repo: string, number: number) {
    return this.c.request("POST", this.path(repo, `/pulls/${number}/close`));
  }

  // commits
  compare(repo: string, base: string, head: string) {
    return this.c.request("GET", this.path(repo, "/compare"), undefined, { base, head });
  }

  /** One commit of file changes; `expect` refuses the commit if the branch moved. */
  commit(
    repo: string,
    opts: { branch: string; message: string; changes: FileChange[]; expect?: string; newBranch?: string },
  ) {
    return this.c.request<{ commit: string; branch: string }>("POST", this.path(repo, "/commits"), opts);
  }

  signature(repo: string, sha: string) {
    return this.c.request("GET", this.path(repo, `/commits/${seg(sha)}/signature`));
  }
}

/**
 * Btrfs-backed dev workspaces, over `/v1/workspaces`. A workspace is named by its id (the `name`
 * given at create); pushes cut snapshots, kept on the workspace's VOLUME — see {@link Volumes}
 * for history, restore targets and deletion.
 */
export class Workspaces {
  constructor(private readonly c: Kloudlite) {}

  /**
   * `repo` seeds from a PLATFORM repo ("owner/name", with `branch`); `packages` are nixpkgs
   * attributes, optionally pinned (`attr@version`). `region` matters only on a multi-region
   * fleet — absent, the deployment's one region is used.
   */
  create(opts: {
    name: string;
    quota_gb: number;
    region?: string;
    team?: string;
    image?: string;
    repo?: string;
    branch?: string;
    packages?: string[];
  }) {
    return this.c.request("POST", "/v1/workspaces", { packages: [], ...opts });
  }

  /** The caller's workspaces; `team` scopes to their workspaces in that team. */
  list(team?: string) {
    return this.c.request<unknown[]>("GET", "/v1/workspaces", undefined, { team });
  }

  get(id: string) {
    return this.c.request("GET", `/v1/workspaces/${seg(id)}`);
  }

  /** Runs the worktree finalizer: the volume survives while a pushed snapshot references it. */
  delete(id: string) {
    return this.c.request("DELETE", `/v1/workspaces/${seg(id)}`);
  }

  start(id: string) {
    return this.c.request("POST", `/v1/workspaces/${seg(id)}/start`);
  }

  /** A stop cuts a sync point and tears the pod down — seconds, never waiting on a replica. */
  stop(id: string) {
    return this.c.request("POST", `/v1/workspaces/${seg(id)}/stop`);
  }

  /** Cut a SNAPSHOT of the working copy — the one durable, restorable state. */
  push(id: string, message?: string) {
    return this.c.request("POST", `/v1/workspaces/${seg(id)}/push`, message === undefined ? undefined : { message });
  }

  /**
   * A new workspace from this one's state, cut at the moment of the request; the answer's
   * `based_on` names exactly what it grafted onto.
   */
  clone(id: string, name: string, task?: string) {
    return this.c.request("POST", `/v1/workspaces/${seg(id)}/clone`, { name, task });
  }

  /**
   * A new workspace grafted onto a PUSHED snapshot — also one whose workspace is long gone.
   * Absent overrides mean "whatever the snapshot froze".
   */
  restore(opts: { name: string; snapshot_id: string; image?: string; packages?: string[]; quota_gb?: number }) {
    return this.c.request("POST", "/v1/workspaces/restore", opts);
  }

  /** Replace the package list; pinned entries are locked before anything is written. */
  setPackages(id: string, packages: string[]) {
    return this.c.request("PATCH", `/v1/workspaces/${seg(id)}`, { packages });
  }

  /** Re-resolve every pinned package — the only call that moves existing locks. */
  updatePackages(id: string) {
    return this.c.request("POST", `/v1/workspaces/${seg(id)}/packages/update`);
  }

  /** The workspace's tool-server address and token — answered to the owner alone. */
  tools(id: string) {
    return this.c.request<{ address: string; token?: string }>("GET", `/v1/workspaces/${seg(id)}/tools`);
  }

  /**
   * An {@link Ide} bound to this workspace's tool server. The address is a pod IP: reachable
   * from inside the fleet (a bench); from a laptop, tunnel with `kl-connect ws ide` and use
   * `new Ide("http://127.0.0.1:7788", token)` instead.
   */
  async ide(id: string): Promise<Ide> {
    const doc = await this.tools(id);
    return new Ide(`http://${doc.address}`, doc.token);
  }
}

type ToolArgs = Record<string, unknown>;

/**
 * A workspace's tool server (`kl ide serve`): the full tool set over plain HTTP — `GET /tools`
 * lists every tool with its schema, `POST /tools/{name}` runs one. The typed methods cover them
 * all; {@link call} takes anything the listing names. `undefined` arguments are dropped, so
 * every optional means "the tool's own default".
 */
export class Ide {
  private readonly url: string;

  constructor(
    url: string,
    private readonly token?: string,
    private readonly timeoutMs = 630_000,
  ) {
    this.url = url.replace(/\/+$/, "");
  }

  /** Every tool, with its JSON schema — the authority on arguments. */
  list() {
    return this.http("GET", "/tools");
  }

  call<T = unknown>(name: string, args: ToolArgs = {}): Promise<T> {
    return this.http<T>("POST", `/tools/${seg(name)}`, args);
  }

  // files
  read(args: { path?: string; paths?: unknown[]; offset?: number; limit?: number }) {
    return this.call("read", args);
  }
  write(path: string, content: string) {
    return this.call("write", { path, content });
  }
  /**
   * One file as `{path, edits}`, several as `{files: [{path, edits}]}`; each edit is
   * `{old, new, replace_all?}` and the whole call is atomic.
   */
  edit(args: { path?: string; edits?: unknown[]; files?: unknown[] }) {
    return this.call("edit", args);
  }
  patch(diff: string, cwd?: string) {
    return this.call("patch", { diff, cwd });
  }
  glob(pattern: string, cwd?: string) {
    return this.call("glob", { pattern, cwd });
  }
  grep(
    pattern: string,
    opts?: { cwd?: string; glob?: string; mode?: "content" | "files" | "count"; context?: number; ignore_case?: boolean; max?: number },
  ) {
    return this.call("grep", { pattern, ...opts });
  }

  // commands and processes — `detach: true` answers `{id}` for the process_* calls
  exec(
    cmd: string | string[],
    opts?: { cwd?: string; env?: Record<string, string>; timeout_ms?: number; head?: number; tail?: number; quiet?: boolean; detach?: boolean },
  ) {
    return this.call("exec", { cmd, ...opts });
  }
  processList() {
    return this.call("process_list");
  }
  processOutput(id: string, opts?: { since?: number; since_err?: number }) {
    return this.call("process_output", { id, ...opts });
  }
  processWrite(id: string, data: string) {
    return this.call("process_write", { id, data });
  }
  processKill(id: string, signal?: "TERM" | "KILL") {
    return this.call("process_kill", { id, signal });
  }

  // watches
  watch(args: { paths?: string[]; cmd?: string | string[]; pattern?: string; once?: boolean }) {
    return this.call("watch", args);
  }
  watchPoll(id: string, since?: number) {
    return this.call("watch_poll", { id, since });
  }
  watchStop(id: string) {
    return this.call("watch_stop", { id });
  }

  // containers (the hidden per-owner builder)
  containerBuild(
    tags: string[],
    opts?: { context?: string; file?: string; build_args?: string[]; platform?: string; no_cache?: boolean; timeout_ms?: number; head?: number; tail?: number },
  ) {
    return this.call("container_build", { tags, ...opts });
  }
  containerPush(src: string, dst: string[]) {
    return this.call("container_push", { src, dst });
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
 * The btrfs volumes behind workspaces and environments: snapshot history, restore targets, and
 * the two deletes — over `/v1/volumes`.
 */
export class Volumes {
  constructor(private readonly c: Kloudlite) {}

  /** `kind` is `workspace` or `environment`; `owner` a handle or team slug. */
  list(query?: { kind?: "workspace" | "environment"; owner?: string }) {
    return this.c.request<unknown[]>("GET", "/v1/volumes", undefined, query);
  }

  /** The chain of PUSHED snapshots, newest first — each row a restore target. */
  history(name: string) {
    return this.c.request("GET", `/v1/volumes/${seg(name)}/history`);
  }

  refs(name: string) {
    return this.c.request("GET", `/v1/volumes/${seg(name)}/refs`);
  }

  /**
   * Refused (409) for a sync point or a running worktree's base; deleting a detached volume's
   * last snapshot deletes the volume.
   */
  deleteSnapshot(name: string, snapshotId: string) {
    return this.c.request("DELETE", `/v1/volumes/${seg(name)}/snapshots/${seg(snapshotId)}`);
  }

  /** Takes a DETACHED volume with all its snapshots; refused while a working copy remains. */
  delete(name: string) {
    return this.c.request("DELETE", `/v1/volumes/${seg(name)}`);
  }
}

/**
 * Model and dataset repositories, on the hub listener (Hugging Face Hub API).
 * Management level: create, read, refs, visibility, delete — files move with
 * git/`huggingface_hub` against the same hub URL. Every `repo` is one
 * `"owner/name"` string, exactly as in {@link CodeRepos}.
 */
export class HubRepos {
  private readonly plural: string;

  constructor(
    private readonly c: Kloudlite,
    private readonly kind: "model" | "dataset",
  ) {
    this.plural = kind === "model" ? "models" : "datasets";
  }

  private body(repo: string, extra?: Record<string, unknown>) {
    const [owner, name] = split(repo);
    return { name, organization: owner, type: this.kind, ...extra };
  }

  private path(repo: string, tail = ""): string {
    const [owner, name] = split(repo);
    return `/api/${this.plural}/${seg(owner)}/${seg(name)}${tail}`;
  }

  /** `private` is sent explicitly rather than trusting a server default. */
  create(repo: string, opts?: { private?: boolean }) {
    return this.c.hubRequest("POST", "/api/repos/create", this.body(repo, { private: opts?.private ?? true }));
  }

  get(repo: string) {
    return this.c.hubRequest("GET", this.path(repo));
  }

  /** Branches and tags, as the hub spells them. */
  refs(repo: string) {
    return this.c.hubRequest("GET", this.path(repo, "/refs"));
  }

  setPrivate(repo: string, isPrivate: boolean) {
    return this.c.hubRequest("PUT", this.path(repo, "/settings"), { private: isPrivate });
  }

  delete(repo: string) {
    return this.c.hubRequest("DELETE", "/api/repos/delete", this.body(repo));
  }

  /**
   * A new repo whose contents — large files included, through git-lfs — are fetched from a
   * PUBLIC git remote; `git` is a strict `https://` URL (the Hugging Face Hub is one host among
   * any). Answers 202; poll {@link importStatus}.
   */
  importGit(repo: string, git: string, opts?: { private?: boolean }) {
    return this.c.hubRequest("POST", this.path(repo, "/import"), { from: git, private: opts?.private ?? true });
  }

  /** `{from, state: queued|running|done|failed, detail?}` for an imported repo. */
  importStatus(repo: string) {
    return this.c.hubRequest("GET", this.path(repo, "/import"));
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
