/**
 * Kloudlite SDK for TypeScript — zero dependencies, built on `fetch`.
 *
 * Auth is a Bearer token (a session or CLI token). Every module hangs off the
 * one client:
 *
 *     import { Kloudlite } from "@kloudlite/sdk";
 *     const kl = new Kloudlite({ baseUrl: "https://api.example.com", token: "…" });
 *     const repo = await kl.repos.create({ owner: "acme", name: "payments" });
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

/** Code repositories: `/v1/repos`. */
export class Repos {
  constructor(private readonly c: Kloudlite) {}

  // repos
  create(opts: { owner: string; name: string; visibility?: "public" | "private"; description?: string }) {
    return this.c.request<Repo>("POST", "/v1/repos", { visibility: "private", description: "", ...opts });
  }

  list(owner: string) {
    return this.c.request<Repo[]>("GET", "/v1/repos", undefined, { owner });
  }

  get(owner: string, name: string) {
    return this.c.request<Repo>("GET", `/v1/repos/${seg(owner)}/${seg(name)}`);
  }

  update(owner: string, name: string, change: { description?: string; visibility?: "public" | "private" }) {
    return this.c.request("PATCH", `/v1/repos/${seg(owner)}/${seg(name)}`, change);
  }

  delete(owner: string, name: string) {
    return this.c.request("DELETE", `/v1/repos/${seg(owner)}/${seg(name)}`);
  }

  // branches
  branches(owner: string, name: string) {
    return this.c.request<Branch[]>("GET", `/v1/repos/${seg(owner)}/${seg(name)}/branches`);
  }

  /** `oid` is the head the caller saw; the delete is refused if the branch moved. */
  deleteBranch(owner: string, name: string, branch: string, oid: string) {
    return this.c.request("DELETE", `/v1/repos/${seg(owner)}/${seg(name)}/branches/${seg(branch)}`, undefined, { oid });
  }

  // protection
  protection(owner: string, name: string) {
    return this.c.request("GET", `/v1/repos/${seg(owner)}/${seg(name)}/protection`);
  }

  protect(owner: string, name: string, rule: { pattern: string; remove?: boolean; no_force?: boolean; no_delete?: boolean }) {
    return this.c.request("POST", `/v1/repos/${seg(owner)}/${seg(name)}/protection`, rule);
  }

  // pull requests
  pulls(owner: string, name: string, query?: { state?: string; limit?: number }) {
    return this.c.request<unknown[]>("GET", `/v1/repos/${seg(owner)}/${seg(name)}/pulls`, undefined, query);
  }

  pull(owner: string, name: string, number: number) {
    return this.c.request("GET", `/v1/repos/${seg(owner)}/${seg(name)}/pulls/${number}`);
  }

  openPull(owner: string, name: string, opts: { title: string; head: string; base: string; description?: string }) {
    return this.c.request("POST", `/v1/repos/${seg(owner)}/${seg(name)}/pulls`, { description: "", ...opts });
  }

  comment(owner: string, name: string, number: number, body: string) {
    return this.c.request("POST", `/v1/repos/${seg(owner)}/${seg(name)}/pulls/${number}/comments`, { body });
  }

  /** The merge is a job: the server answers 202 and the pull reports where it got to. */
  merge(owner: string, name: string, number: number, strategy: "fast-forward" | "squash" | "merge" | "rebase" = "fast-forward") {
    return this.c.request("POST", `/v1/repos/${seg(owner)}/${seg(name)}/pulls/${number}/merge`, undefined, { strategy });
  }

  close(owner: string, name: string, number: number) {
    return this.c.request("POST", `/v1/repos/${seg(owner)}/${seg(name)}/pulls/${number}/close`);
  }

  // commits
  compare(owner: string, name: string, base: string, head: string) {
    return this.c.request("GET", `/v1/repos/${seg(owner)}/${seg(name)}/compare`, undefined, { base, head });
  }

  /** One commit of file changes; `expect` refuses the commit if the branch moved. */
  commit(
    owner: string,
    name: string,
    opts: { branch: string; message: string; changes: FileChange[]; expect?: string; newBranch?: string },
  ) {
    return this.c.request<{ commit: string; branch: string }>("POST", `/v1/repos/${seg(owner)}/${seg(name)}/commits`, opts);
  }

  signature(owner: string, name: string, sha: string) {
    return this.c.request("GET", `/v1/repos/${seg(owner)}/${seg(name)}/commits/${seg(sha)}/signature`);
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
