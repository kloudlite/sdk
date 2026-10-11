//! Compress a big tool result by its content type, keep the original on disk, hand the model an id.
//! Ports headroom's ContentRouter + CCR (Compress / Cache / Retrieve): five rule detectors (json
//! array, search, diff, log, text), a size gate, and an optional Kompress model for plain text.
//! Incident 2026-10-11: one `exec` result of 4.6M characters sat in history and the next request was
//! refused at 1.9M tokens. Cutting the middle (budget.ts) loses it; compressing and keeping the
//! original lets the model ask for any part with the `retrieve` tool.
//! Map: `compress` (rules) -> `compressWithModel` (plain text only) -> `remember`/`retrieve` (disk
//! store `<dir>/results/<hash>`, dropped with the session) -> `grepLines` (the tool's grep mode).
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

export type Compressed = { text: string; hash?: string; strategy: string; before: number; after: number; plain?: boolean };

const MIN_CHARS = 800;
const MAX_ITEMS = 15;
// A refusal is already short and exact; compressing it hides why the tool said no.
const REFUSAL = /^(blocked|failed|denied|error)/i;
const HASH = /^[0-9a-f]{12}$/;
// Kompress is a small model behind one POST; past this a reply cannot land inside the timeout.
const MODEL_MAX = 200_000;

/** Write the original once under its content hash; the hash is the only name the model can use. */
export function remember(dir: string, original: string): string {
  const hash = createHash("sha256").update(original).digest("hex").slice(0, 12);
  const file = join(dir, "results", hash);
  if (!existsSync(file)) {
    mkdirSync(join(dir, "results"), { recursive: true });
    writeFileSync(file, original);
  }
  return hash;
}

/** The stored original, or undefined. The pattern check keeps `../x` out of the path. */
export function retrieve(dir: string, hash: string): string | undefined {
  if (!HASH.test(hash)) return undefined;
  try {
    return readFileSync(join(dir, "results", hash), "utf8");
  } catch {
    return undefined;
  }
}

/** Matching lines as `<lineNo>: <line>`, at most `max`, plus a count line when more match. */
export function grepLines(text: string, pattern: string, max = 200): string {
  const re = new RegExp(pattern, "i");
  const out: string[] = [];
  let total = 0;
  text.split("\n").forEach((l, i) => {
    if (!re.test(l)) return;
    if (++total <= max) out.push(`${i + 1}: ${l}`);
  });
  if (total > max) out.push(`[${total - max} more matching lines not shown]`);
  return out.join("\n");
}

function marker(before: number, after: number, hash: string): string {
  return `\n\n[compressed ${before} chars to ${after}, id ${hash}. Call retrieve with this id (and grep, or offset/limit) for the full text.]`;
}

function pass(text: string): Compressed {
  return { text, strategy: "pass", before: text.length, after: text.length };
}

// ---- json-array ------------------------------------------------------------------------------

function asArray(parsed: unknown): unknown[] | undefined {
  if (Array.isArray(parsed)) return parsed;
  if (parsed && typeof parsed === "object") {
    const arrayKeys = Object.entries(parsed as Record<string, unknown>).filter(([, v]) => Array.isArray(v));
    if (arrayKeys.length === 1) return arrayKeys[0][1] as unknown[];
  }
  return undefined;
}

function sameKeySet(items: unknown[]): string[] | undefined {
  if (items.length === 0 || !items.every((it) => it && typeof it === "object" && !Array.isArray(it))) return undefined;
  const first = Object.keys(items[0] as object).sort();
  const key = first.join("\0");
  for (const it of items) if (Object.keys(it as object).sort().join("\0") !== key) return undefined;
  return first;
}

function cell(v: unknown): string {
  return typeof v === "string" ? v : JSON.stringify(v);
}

function asTable(items: unknown[], original: string): Compressed | undefined {
  const keys = sameKeySet(items);
  if (!keys) return undefined;
  const lines = [`keys: ${keys.join(", ")}`, ...items.map((it) => keys.map((k) => cell((it as Record<string, unknown>)[k])).join(" | "))];
  const text = lines.join("\n");
  return { text, strategy: "table", before: original.length, after: text.length };
}

function words(s: string): Set<string> {
  return new Set((s.toLowerCase().match(/[a-z0-9]{4,}/g) ?? []));
}

function smartSample(items: unknown[], query: string): { kept: unknown[]; strategy: string } {
  const n = items.length;
  const keep = new Set<number>();
  const headN = Math.min(5, Math.ceil(0.3 * n));
  const tailN = Math.min(3, Math.ceil(0.15 * n));
  for (let i = 0; i < headN; i++) keep.add(i);
  for (let i = n - tailN; i < n; i++) if (i >= 0) keep.add(i);
  for (let i = 0; i < n; i++) if (/error|fail|exception/i.test(JSON.stringify(items[i]))) keep.add(i);

  // numeric-anomaly pass: for each field that is numeric across items, flag >2 stddev from the mean
  const fields = new Map<string, number[]>();
  for (const it of items) {
    if (!it || typeof it !== "object") continue;
    for (const [k, v] of Object.entries(it as Record<string, unknown>)) if (typeof v === "number") (fields.get(k) ?? fields.set(k, []).get(k)!).push(v);
  }
  for (const [k, vals] of fields) {
    if (vals.length !== items.length) continue;
    const mean = vals.reduce((a, b) => a + b, 0) / vals.length;
    const sd = Math.sqrt(vals.reduce((a, b) => a + (b - mean) ** 2, 0) / vals.length);
    if (sd === 0) continue;
    items.forEach((it, i) => {
      const v = (it as Record<string, unknown>)[k];
      if (typeof v === "number" && Math.abs(v - mean) > 2 * sd) keep.add(i);
    });
  }

  const qWords = words(query);
  if (qWords.size > 0) items.forEach((it, i) => { for (const w of words(JSON.stringify(it))) if (qWords.has(w)) { keep.add(i); break; } });

  const orderedKept = [...keep].sort((a, b) => a - b).slice(0, MAX_ITEMS);
  return { kept: orderedKept.map((i) => items[i]), strategy: `smart_sample(${n}->${orderedKept.length})` };
}

function tryJsonArray(text: string, query: string): Compressed | undefined {
  let parsed: unknown;
  try { parsed = JSON.parse(text.trim()); } catch { return undefined; }
  const items = asArray(parsed);
  if (!items) return undefined;
  const table = asTable(items, text);
  if (table) return table;
  const { kept, strategy } = smartSample(items, query);
  const body = JSON.stringify(kept);
  return { text: body, strategy, before: text.length, after: body.length };
}

// ---- search (grep -n shape) -------------------------------------------------------------------

const SEARCH_LINE = /^[^\s:]+:\d+[:-]/;

function trySearch(text: string): Compressed | undefined {
  const lines = text.split("\n");
  const nonEmpty = lines.filter((l) => l.length > 0);
  if (nonEmpty.length === 0) return undefined;
  const hitCount = nonEmpty.filter((l) => SEARCH_LINE.test(l)).length;
  if (hitCount / nonEmpty.length < 0.6) return undefined;

  const byFile = new Map<string, string[]>();
  for (const l of nonEmpty) {
    const m = SEARCH_LINE.exec(l);
    const file = m ? l.slice(0, l.indexOf(":")) : "?";
    (byFile.get(file) ?? byFile.set(file, []).get(file)!).push(l);
  }
  const files = [...byFile.entries()].sort((a, b) => b[1].length - a[1].length);
  const out: string[] = [];
  for (const [file, hits] of files) {
    out.push(...hits.slice(0, 3));
    if (hits.length > 3) out.push(`… +${hits.length - 3} more in ${file}`);
  }
  const body = out.join("\n");
  return { text: body, strategy: "search", before: text.length, after: body.length };
}

// ---- diff ---------------------------------------------------------------------------------

function tryDiff(text: string): Compressed | undefined {
  const lines = text.split("\n");
  const looksDiff = lines[0]?.startsWith("diff --git") || (text.includes("---") && text.includes("+++"));
  if (!looksDiff || !text.includes("@@")) return undefined;
  const kept = lines.filter((l) => l.startsWith("diff --git") || l.startsWith("+++") || l.startsWith("---") || l.startsWith("@@") || l.startsWith("+") || l.startsWith("-"));
  let body = kept.join("\n");
  if (body.length > 4000) body = `${body.slice(0, 2000)}…${body.slice(-1500)}`;
  return { text: body, strategy: "diff", before: text.length, after: body.length };
}

// ---- log ------------------------------------------------------------------------------------

const LOG_START = /^\d{4}-\d\d-\d\d|^\[?\d\d:\d\d:\d\d/;
const LOG_LEVEL = /\b(INFO|WARN|ERROR|DEBUG|TRACE)\b/;
const LOG_IMPORTANT = /error|warn|fail|panic|exception/i;

function tryLog(text: string): Compressed | undefined {
  const lines = text.split("\n");
  const matching = lines.filter((l) => LOG_START.test(l) || LOG_LEVEL.test(l)).length;
  if (lines.length === 0 || matching / lines.length < 0.4) return undefined;

  const collapsed: string[] = [];
  let runKey = "";
  let runLine = "";
  let runCount = 0;
  const flush = () => {
    if (runCount === 0) return;
    collapsed.push(runCount > 1 ? `${runLine} (×${runCount})` : runLine);
  };
  for (const l of lines) {
    const key = l.replace(/\d/g, "");
    if (key === runKey) { runCount++; continue; }
    flush();
    runKey = key; runLine = l; runCount = 1;
  }
  flush();

  const important = new Set(lines.filter((l) => LOG_IMPORTANT.test(l)));
  const head = collapsed.slice(0, 10);
  const tail = collapsed.slice(-20);
  const middleImportant = collapsed.slice(10, -20).filter((l) => [...important].some((i) => l.startsWith(i.replace(/\d/g, "")) || l === i));
  const kept = [...new Set([...head, ...middleImportant, ...tail])];
  const body = kept.join("\n");
  return { text: body, strategy: "log", before: text.length, after: body.length };
}

// ---- text (fallback) -----------------------------------------------------------------------

function tryText(text: string): Compressed {
  const lines = text.split("\n");
  const deduped = [...new Set(lines)];
  let out = deduped;
  if (deduped.length > 80) {
    const head = deduped.slice(0, 40);
    const tail = deduped.slice(-20);
    out = [...head, `… [${deduped.length - 60} lines omitted] …`, ...tail];
  }
  const body = out.join("\n");
  return { text: body, strategy: "text", before: text.length, after: body.length };
}

// ---- entry point ----------------------------------------------------------------------------

export function compress(dir: string, text: string, query = ""): Compressed {
  if (text.length < MIN_CHARS || REFUSAL.test(text)) return pass(text);

  let result: Compressed;
  let plain = false;
  try {
    const rule = tryJsonArray(text, query) ?? trySearch(text) ?? tryDiff(text) ?? tryLog(text);
    if (rule) {
      result = rule;
    } else {
      result = tryText(text);
      plain = true;
    }
  } catch {
    return pass(text);
  }

  if (result.after >= result.before * 0.8) return { ...pass(text), plain }; // overhead would exceed savings
  // A table is lossless only while it is small; budget.ts may cut it later, so keep the original too.
  const hash = remember(dir, text);
  return { ...result, hash, plain, text: result.text + marker(result.before, result.after, hash) };
}

// ---- model-backed compression (Kompress) ---------------------------------------------------

// Only the plain-text fallback is sent to the model: a rule result already used the shape's own
// redundancy, and a failure here can only cost the rules result, never a structural one.
// ponytail: one blocking POST per result, 5 s cap; batch or run in the background if latency shows.
export async function compressWithModel(dir: string, text: string, query = ""): Promise<Compressed> {
  const c = compress(dir, text, query);
  const url = process.env.KL_KOMPRESS_URL;
  if (!url || !c.plain || text.length > MODEL_MAX) return c;

  try {
    const res = await fetch(`${url}/compress`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ content: text, target_ratio: null }),
      signal: AbortSignal.timeout(5000),
    });
    if (!res.ok) return c;
    const body = (await res.json()) as { compressed: string };
    if (typeof body.compressed !== "string") return c;
    // Any shrink, as headroom (kompress_remote.py): the whole marked result against the whole original
    // in one unit we count ourselves. Kompress's token counts are its tokenizer, not the model's, so
    // characters it is (headroom's own fallback unit); retrieve undoes the loss.
    const out = (hash: string) => body.compressed + marker(text.length, body.compressed.length, hash);
    if (out("0".repeat(12)).length >= text.length) return c;
    const hash = remember(dir, text);
    return {
      text: out(hash),
      hash,
      strategy: "kompress",
      before: text.length,
      after: body.compressed.length,
    };
  } catch {
    return c; // network error, timeout, bad json: the rules result already stands
  }
}
