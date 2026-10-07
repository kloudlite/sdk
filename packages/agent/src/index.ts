import { mkdirSync, readdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import {
  ModelRuntime,
  SessionManager,
  createAgentSession,
  type AgentSession,
} from "@earendil-works/pi-coding-agent";
import type { Api, AuthInteraction, AuthType, Credential, Model } from "@earendil-works/pi-ai";
import type { Registry } from "@kloudlite-tui/tools";

export type {
  AgentSession,
  AgentSessionEvent,
} from "@earendil-works/pi-coding-agent";
export type { AuthInteraction, AuthPrompt, AuthEvent, Message } from "@earendil-works/pi-ai";

const CONFIG_DIR =
  process.env.KLOUDLITE_CONFIG_DIR ?? join(homedir(), ".config", "kloudlite");

/**
 * Shared model/auth runtime (pi): every provider pi supports, models.json
 * custom providers, and credential storage (env keys, API keys, OAuth with
 * refresh) persisted under the kloudlite config dir.
 */
const runtime = await ModelRuntime.create({
  authPath: join(CONFIG_DIR, "auth.json"),
  modelsPath: join(CONFIG_DIR, "models.json"),
});

// ModelRuntime proxies the Models surface publicly (getModels/checkAuth/login/…)
export const models = runtime;

export type ModelRef = { provider: string; id: string };

type Settings = { defaultModel?: ModelRef; theme?: string; sidebar?: "show" | "hide"; sidebarWidth?: number; thinking?: "show" | "hide"; vim?: "on" | "off" };

const SETTINGS_PATH = join(CONFIG_DIR, "settings.json");

export function readSettings(): Settings {
  try {
    return JSON.parse(readFileSync(SETTINGS_PATH, "utf8"));
  } catch {
    return {};
  }
}

export function writeSettings(patch: Partial<Settings>): void {
  mkdirSync(CONFIG_DIR, { recursive: true });
  writeFileSync(SETTINGS_PATH, JSON.stringify({ ...readSettings(), ...patch }, null, 2));
}

export function listModels(): { provider: string; id: string; name: string }[] {
  return models
    .getModels()
    .map((m) => ({ provider: m.provider, id: m.id, name: m.name ?? m.id }));
}

export function resolveModel(ref: ModelRef): Model<Api> | undefined {
  return models.getModel(ref.provider, ref.id);
}

export type ProviderAuth = {
  provider: string;
  /** Credentials resolve (env key, OAuth token, ambient) - a request can run. */
  ok: boolean;
  /** Env var(s) that would configure this provider, for the "how to add" hint. */
  envKeys: string[];
};

/**
 * The env vars a provider actually reads, asked of pi rather than guessed.
 * pi exports no mapping, but every provider resolves its credentials through
 * `ctx.env(name)` — so a no-op context records the names on the way past.
 * Guessing `${ID}_API_KEY` was wrong for 13 of 40 providers, and wrong in
 * ways that strand a user: Bedrock wants AWS credentials, Hugging Face wants
 * HF_TOKEN, github-copilot wants COPILOT_GITHUB_TOKEN.
 */
async function envKeysFor(provider: {
  auth?: { apiKey?: { resolve?: Function; check?: Function } };
}): Promise<string[]> {
  const asked: string[] = [];
  const ctx = {
    env: async (name: string) => {
      asked.push(name);
      return undefined;
    },
    fileExists: async () => false,
  };
  const input = { ctx, credential: undefined, signal: new AbortController().signal };
  for (const probe of [provider.auth?.apiKey?.resolve, provider.auth?.apiKey?.check]) {
    // a provider may resolve ambient credentials any way it likes; a throw
    // here just means it had nothing to ask for
    try {
      await probe?.(input);
    } catch {}
  }
  return [...new Set(asked)];
}

/** Auth status for every provider (env keys, stored credentials, ambient). */
export async function providerAuth(): Promise<ProviderAuth[]> {
  return Promise.all(
    models.getProviders().map(async (p) => ({
      provider: p.id,
      ok: await models
        .checkAuth(p.id)
        .then((c) => c !== undefined)
        .catch(() => false),
      envKeys: await envKeysFor(p as never),
    })),
  );
}

export type LoginOption = {
  provider: string;
  type: AuthType;
  label: string;
};

/** Interactive login flows the providers offer (OAuth and prompted API keys). */
export function loginOptions(): LoginOption[] {
  const out: LoginOption[] = [];
  for (const p of models.getProviders()) {
    if (p.auth.oauth)
      out.push({
        provider: p.id,
        type: "oauth",
        label: p.auth.oauth.loginLabel ?? p.auth.oauth.name,
      });
    if (p.auth.apiKey?.login)
      out.push({ provider: p.id, type: "api_key", label: p.auth.apiKey.name });
  }
  return out;
}

export function loginProvider(
  provider: string,
  type: AuthType,
  interaction: AuthInteraction,
): Promise<Credential> {
  return models.login(provider, type, interaction);
}


/**
 * Create an agent session: pi's full harness - streaming, thinking, coding
 * tools (read/bash/edit/write), steering/follow-up queues, compaction,
 * auto-retry. Subscribe to `AgentSessionEvent`s for the UI.
 *
 * `key` scopes persistence: each session's history lives under
 * `~/.config/kloudlite/sessions/<key>` and is restored on the next start.
 */
/**
 * Archive a key's persisted transcripts (/clear): moves the session files into
 * an archive/ subdir so `continueRecent` starts from scratch, without deleting
 * anything.
 */
type SessionMeta = { key: string; name?: string; description?: string; updated: number };

function sessionDir(key: string): string {
  return join(CONFIG_DIR, "sessions", key.replace(/[^\w.-]/g, "_"));
}

function metaPath(key: string): string {
  return join(sessionDir(key), "meta.json");
}

function readMeta(key: string): SessionMeta | undefined {
  try {
    return JSON.parse(readFileSync(metaPath(key), "utf8"));
  } catch {
    return undefined;
  }
}

function writeMeta(meta: SessionMeta): void {
  mkdirSync(sessionDir(meta.key), { recursive: true });
  writeFileSync(metaPath(meta.key), JSON.stringify(meta, null, 2));
}

/** Give a session a human name, so it can be found and continued later. */
export function nameSession(key: string, name: string): void {
  writeMeta({ ...(readMeta(key) ?? { key, updated: Date.now() }), key, name });
}

/** One line on what this session is for, shown under its title. */
export function describeSession(key: string, description: string): void {
  writeMeta({ ...(readMeta(key) ?? { key, updated: Date.now() }), key, description });
}

/**
 * Persisted sessions, newest first. `key` is what `createSession` takes, so a
 * listed session can be continued as-is; `prefix` narrows to one context
 * (e.g. an environment's main sessions).
 */
export function listSessions(prefix = ""): SessionMeta[] {
  let dirs: string[];
  try {
    dirs = readdirSync(join(CONFIG_DIR, "sessions"));
  } catch {
    return [];
  }
  return dirs
    .map((d) => {
      try {
        return JSON.parse(readFileSync(join(CONFIG_DIR, "sessions", d, "meta.json"), "utf8")) as SessionMeta;
      } catch {
        return undefined;
      }
    })
    .filter((m): m is SessionMeta => !!m && m.key.startsWith(prefix))
    .sort((a, b) => b.updated - a.updated);
}

export function clearSessionHistory(key: string): void {
  const dir = sessionDir(key);
  let entries: string[];
  try {
    entries = readdirSync(dir);
  } catch {
    return;
  }
  const archive = join(dir, "archive");
  mkdirSync(archive, { recursive: true });
  for (const name of entries) {
    if (name === "archive" || name === "meta.json") continue;
    try {
      renameSync(join(dir, name), join(archive, `${Date.now()}-${name}`));
    } catch {}
  }
}

export async function createSession({
  key,
  cwd = process.cwd(),
  model,
  registry,
  fresh = false,
}: {
  key: string;
  cwd?: string;
  model: Model<Api>;
  registry?: Registry;
  /** Start a brand-new persisted session instead of continuing the last one (/clear). */
  fresh?: boolean;
}): Promise<AgentSession> {
  const dir = sessionDir(key);
  // meta.json makes a session findable later: its key, its name, last use
  writeMeta({ ...(readMeta(key) ?? { key }), key, updated: Date.now() });
  const { session } = await createAgentSession({
    cwd,
    modelRuntime: runtime,
    model,
    // continue the most recent session in this key's dir (new file if none)
    sessionManager: fresh
      ? SessionManager.create(cwd, dir)
      : SessionManager.continueRecent(cwd, dir),
    customTools: registry?.all().map((def) => ({
      name: def.name,
      label: def.name,
      description: def.description,
      parameters: def.inputSchema as never,
      execute: async (args: unknown) => ({
        content: [{ type: "text" as const, text: await def.run(args) }],
      }),
    })) as never,
  });
  return session;
}
