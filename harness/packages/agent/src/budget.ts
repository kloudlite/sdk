//! A session never sends more than its budget. Incident 2026-10-11: one `exec` result of 4.6M
//! characters stayed in history after compaction (pi keeps the newest messages) and the next request
//! was refused at 1.9M tokens. Three guards: cap each tool result when it is stored (`tool_result`),
//! cap any message in the outgoing copy as a backstop (`context`), and shrink the model's window and
//! output so pi compacts at ~184k tokens instead of near 1M (`budgeted`).
//! Before the cut, `tool_result` compresses by content type (compress.ts) and keeps the original under
//! `<session dir>/results/`; the `retrieve` tool reads it back. Only the `context` backstop has no dir,
//! so its note still says to run the command again.
import type { AgentMessage } from "@earendil-works/pi-agent-core";
import type { Api, Model } from "@earendil-works/pi-ai";
import type { ExtensionFactory } from "@earendil-works/pi-coding-agent";
import { compressWithModel, grepLines, remember, retrieve } from "./compress.ts";

export const TOOL_CAP = 32_000;
export const MESSAGE_CAP = 64_000;
const HALF = 12_000;

export const CONTEXT_BUDGET = 200_000;
export const OUTPUT_CAP = 32_000;
// ponytail: constants, not live settings; make them settings if one model needs a different budget.
export const budgeted = <M extends Model<Api>>(m: M): M => ({
  ...m,
  contextWindow: Math.min(m.contextWindow || CONTEXT_BUDGET, CONTEXT_BUDGET),
  maxTokens: Math.min(m.maxTokens || OUTPUT_CAP, OUTPUT_CAP),
});

// These need exact bytes (or are tiny); compressing them would make the model edit from a summary.
const VERBATIM = new Set(["read", "retrieve", "edit", "write", "patch", "display"]);

/** Head + note + tail when over `cap`; the same string otherwise. With `dir`, the original is kept for `retrieve`. */
export function cut(text: string, cap: number, dir?: string): string {
  if (text.length <= cap) return text;
  const note = dir
    ? `[cut: ${text.length} characters, middle dropped. Call retrieve with id ${remember(dir, text)} for the rest.]`
    : `[cut: ${text.length} characters, middle dropped. Run it again with output to a file under ~/.cache/kl/ and use grep or read on that file.]`;
  return `${text.slice(0, HALF)}\n${note}\n${text.slice(-HALF)}`;
}

type Part = { type: string; text?: string };

/** Cut each text part over `cap`; returns the same array when nothing changed. */
function cutParts<P extends Part>(parts: P[], cap: number): P[] {
  let changed = false;
  const out = parts.map((p) => {
    if (p.type !== "text" || typeof p.text !== "string" || p.text.length <= cap) return p;
    changed = true;
    return { ...p, text: cut(p.text, cap) };
  });
  return changed ? out : parts;
}

/** The outgoing copy: stored messages are never mutated, a changed message is a new object. */
export function capMessages(messages: AgentMessage[], cap = MESSAGE_CAP): AgentMessage[] {
  let changed = false;
  const out = messages.map((m) => {
    if (m.role === "user" && typeof m.content === "string") {
      if (m.content.length <= cap) return m;
      changed = true;
      return { ...m, content: cut(m.content, cap) };
    }
    if (m.role !== "user" && m.role !== "toolResult" && m.role !== "assistant") return m;
    const content = cutParts(m.content as Part[], cap);
    if (content === m.content) return m;
    changed = true;
    return { ...m, content } as AgentMessage;
  });
  return changed ? out : messages;
}

const RETRIEVE_LIMIT = 20_000;

export const budgetExtension = (dir: string): ExtensionFactory => (pi) => {
  pi.registerTool({
    name: "retrieve",
    label: "retrieve",
    description:
      "Get the full text of a compressed or cut tool result. Use the id from its note. Use grep to find lines, or offset and limit to read a part.",
    parameters: {
      type: "object",
      properties: {
        id: { type: "string" },
        grep: { type: "string", description: "Regular expression, not case sensitive." },
        offset: { type: "number", description: "First character to return. Default 0." },
        limit: { type: "number", description: "Characters to return. Default 20000, most 32000." },
      },
      required: ["id"],
    } as never,
    async execute(_id, params) {
      const p = params as { id: string; grep?: string; offset?: number; limit?: number };
      const reply = (text: string) => ({ content: [{ type: "text" as const, text }], details: {} });
      const original = retrieve(dir, p.id);
      if (original === undefined) return reply(`no stored result ${p.id}`);
      if (p.grep) {
        try {
          return reply(grepLines(original, p.grep) || "no matching lines");
        } catch {
          return reply(`bad regular expression ${p.grep}`);
        }
      }
      const offset = Math.max(0, p.offset ?? 0);
      const limit = Math.min(Math.max(1, p.limit ?? RETRIEVE_LIMIT), TOOL_CAP);
      return reply(original.slice(offset, offset + limit));
    },
  });
  pi.on("tool_result", async (e) => {
    if (VERBATIM.has(e.toolName)) return undefined;
    let changed = false;
    const content = await Promise.all(
      e.content.map(async (p) => {
        if (p.type !== "text") return p;
        let text = (await compressWithModel(dir, p.text)).text;
        if (text.length > TOOL_CAP) text = cut(text, TOOL_CAP, dir);
        if (text === p.text) return p;
        changed = true;
        return { ...p, text };
      }),
    );
    // pi drops structuredContent when content is replaced without it; codemode reads it, so hand it back.
    return changed ? { content, structuredContent: e.structuredContent } : undefined;
  });
  pi.on("context", (e) => {
    const m = capMessages(e.messages);
    return m === e.messages ? undefined : { messages: m };
  });
};
