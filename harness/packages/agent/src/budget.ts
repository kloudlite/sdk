//! A session never sends more than its budget. Incident 2026-10-11: one `exec` result of 4.6M
//! characters stayed in history after compaction (pi keeps the newest messages) and the next request
//! was refused at 1.9M tokens. Three guards: cap each tool result when it is stored (`tool_result`),
//! cap any message in the outgoing copy as a backstop (`context`), and shrink the model's window and
//! output so pi compacts at ~184k tokens instead of near 1M (`budgeted`).
//! The full output is not saved. ponytail: no spill file; add one if a non-repeatable command loses data.
import type { AgentMessage } from "@earendil-works/pi-agent-core";
import type { Api, Model } from "@earendil-works/pi-ai";
import type { ExtensionFactory } from "@earendil-works/pi-coding-agent";

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

/** Head + note + tail when over `cap`; the same string otherwise. */
export function cut(text: string, cap: number): string {
  if (text.length <= cap) return text;
  const note = `[cut: ${text.length} characters, middle dropped. Run it again with output to a file under ~/.cache/kl/ and use grep or read on that file.]`;
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

export const budgetExtension: ExtensionFactory = (pi) => {
  pi.on("tool_result", (e) => {
    const content = cutParts(e.content, TOOL_CAP);
    return content === e.content ? undefined : { content };
  });
  pi.on("context", (e) => {
    const m = capMessages(e.messages);
    return m === e.messages ? undefined : { messages: m };
  });
};
