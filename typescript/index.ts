/**
 * Kloudlite SDK for TypeScript — zero dependencies, built on `fetch`.
 *
 * Auth is a Bearer token (a session or CLI token). Every module hangs off the
 * one client, and every repository is named by ONE `"owner/name"` string — the
 * owner half being a person's handle or a team slug:
 *
 *     import { Kloudlite } from "@kloudlite/sdk";
 *     const kl = new Kloudlite({ baseUrl: "https://api.example.com", token: "…" });
 *     const repo = await kl.repos.create("acme/payments");
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

export class Kloudlite {
  readonly repos: Repos;
  private readonly baseUrl: string;
  private readonly token?: string;
  private readonly timeoutMs: number;

  constructor(opts: KloudliteOptions) {
    this.baseUrl = opts.baseUrl.replace(/\/+$/, "");
    this.token = opts.token;
    this.timeoutMs = opts.timeoutMs ?? 30_000;
    this.repos = new Repos(this);
  }

  /**
   * One HTTP round trip. Resolves to parsed JSON (or `null` on an empty body);
   * rejects with {@link ApiError} on any non-2xx status.
   */
  async request<T = unknown>(method: string, path: string, body?: unknown, query?: Query): Promise<T> {
    let url = this.baseUrl + path;
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
export class Repos {
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
