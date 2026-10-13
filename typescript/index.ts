/**
 * Kloudlite SDK for TypeScript — zero dependencies, built on `fetch`.
 *
 * The client speaks to the browse API with HTTP Basic auth: the username names
 * the owner the token belongs to (git's `x` placeholder also works), the token
 * is the secret. No credentials is still a valid client — public repositories
 * admit anonymous reads.
 *
 *     import { Kloudlite } from "@kloudlite/sdk";
 *     const kl = new Kloudlite({ baseUrl: "https://git.example.com", username: "acme", token: "…" });
 */

export const VERSION = "0.1.0";
const USER_AGENT = `kloudlite-sdk-typescript/${VERSION}`;

export interface KloudliteOptions {
  baseUrl: string;
  /** Owner handle the token belongs to; `x` (the default) is git's placeholder. */
  username?: string;
  /** Omit for anonymous access to public repositories. */
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
  private readonly baseUrl: string;
  private readonly auth?: string;
  private readonly timeoutMs: number;

  constructor(opts: KloudliteOptions) {
    this.baseUrl = opts.baseUrl.replace(/\/+$/, "");
    this.timeoutMs = opts.timeoutMs ?? 30_000;
    if (opts.token) {
      const raw = `${opts.username ?? "x"}:${opts.token}`;
      this.auth = "Basic " + btoa(raw);
    }
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
    if (this.auth) headers.Authorization = this.auth;
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

  protected segment(s: string): string {
    return encodeURIComponent(s);
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
