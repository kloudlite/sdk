//! Two side calls over the conversation so far, neither ever recorded: `/btw` (a side question) and
//! `piRoute` (a throwaway fork that decides whether a message arriving mid-turn steers or queues).
//!
//! Same pattern as pi's own `summarizeForBugReport`: the conversation is serialized into ONE plain
//! user message, so there are no tool_use blocks to replay, no tool definitions to send and no
//! dangling-call problem, whichever engine owns the session and whether or not a turn is running.
//! Nothing here touches the session: it reads `messages`, makes a separate one-shot request and
//! returns the text. Callers must not record it (transcript, session file, consent words).
import { convertToLlm, estimateTokens, serializeConversation } from "@earendil-works/pi-coding-agent";
import { streamSimple } from "@earendil-works/pi-ai/compat";

export const BTW_SYSTEM = "You answer a side question about a coding conversation. Be brief and exact. Use markdown.";

const INSTRUCTION =
  "This is a side question from the person. It is not part of the task. Answer it briefly from the conversation above. You have no tools. Do not offer to do work.";

/** Newest messages that fit the budget (copied from pi's bug-report.js; it is not exported). */
function selectMessages(messages: any[], budget: number): any[] {
  const selected: any[] = [];
  let tokens = 0;
  for (let i = messages.length - 1; i >= 0; i--) {
    const next = estimateTokens(messages[i]);
    if (selected.length > 0 && tokens + next > budget) break;
    selected.push(messages[i]);
    tokens += next;
  }
  return selected.reverse();
}

export function btwPrompt(messages: any[], question: string, contextWindow: number): string {
  const selected = selectMessages(messages, Math.floor((contextWindow > 0 ? contextWindow : 128_000) * 0.6));
  return [
    selected.length < messages.length ? `Note: only the last ${selected.length} of ${messages.length} messages are shown.` : undefined,
    `<conversation>\n${serializeConversation(convertToLlm(selected))}\n</conversation>`,
    `<question>\n${question}\n</question>`,
    INSTRUCTION,
  ]
    .filter((p) => p !== undefined)
    .join("\n\n");
}

export const ROUTE_SYSTEM = "You route a new message that arrived while a coding agent is working. Answer with one line only.";

const ROUTE_INSTRUCTION =
  "A new message from the person arrived while the agent is working. Reply STEER if it changes, corrects, stops or adds to the work in progress, so the agent must see it now. Reply QUEUE if it is separate work that can wait until the current work is finished. Reply with exactly one line: `STEER: <short reason>` or `QUEUE: <short reason>`.";

/** Same `<conversation>` block as btwPrompt, then the new message and the routing instruction. */
function routePrompt(messages: any[], text: string, contextWindow: number): string {
  const selected = selectMessages(messages, Math.floor((contextWindow > 0 ? contextWindow : 128_000) * 0.6));
  return [
    selected.length < messages.length ? `Note: only the last ${selected.length} of ${messages.length} messages are shown.` : undefined,
    `<conversation>\n${serializeConversation(convertToLlm(selected))}\n</conversation>`,
    `<new_message>\n${text}\n</new_message>`,
    ROUTE_INSTRUCTION,
  ]
    .filter((p) => p !== undefined)
    .join("\n\n");
}

/** One stateless request to the session's own model. `build` gets the model's context window. */
async function sideCall(session: any, system: string, build: (contextWindow: number) => string, maxTokens: number): Promise<string> {
  if (!session.model) throw new Error("No model selected");
  const auth = await session._getSummarizationRequestAuth(session.model, undefined);
  const { model } = auth;
  const context = {
    systemPrompt: system,
    messages: [{ role: "user", content: [{ type: "text", text: build(model.contextWindow) }], timestamp: Date.now() }],
  };
  const options = {
    apiKey: auth.apiKey,
    headers: auth.headers,
    env: auth.env,
    maxTokens: Math.min(maxTokens, model.maxTokens > 0 ? model.maxTokens : Infinity),
    cacheRetention: "none",
  };
  const stream = session.agent.streamFunction ?? streamSimple;
  const res = await (await stream(model, context, options)).result();
  if (res.stopReason === "error") throw new Error(res.errorMessage ?? "request failed");
  return res.content.filter((c: any) => c.type === "text").map((c: any) => c.text).join("").trim();
}

/** `session` is a pi AgentSession; `_getSummarizationRequestAuth` is private in the types only. */
export async function piBtw(session: any, question: string): Promise<string> {
  const text = await sideCall(session, BTW_SYSTEM, (cw) => btwPrompt(session.messages, question, cw), 4096);
  return text || "(no answer)";
}

/** Throwaway fork: steer or queue a message that arrived while the session is busy. */
export async function piRoute(session: any, text: string): Promise<{ steer: boolean; reason: string }> {
  const reply = await sideCall(session, ROUTE_SYSTEM, (cw) => routePrompt(session.messages, text, cw), 200);
  const line = reply.split("\n").find((l) => l.trim()) ?? "";
  const m = /^(STEER|QUEUE)\s*:?\s*(.*)$/i.exec(line.trim());
  if (!m) throw new Error(`unparsable route: ${line.slice(0, 80)}`);
  return { steer: m[1]!.toUpperCase() === "STEER", reason: m[2]!.trim() };
}

export const WATCH_SYSTEM =
  "You read a coding agent's most recent steps and its task board. Reply with exactly one line: RUNNING: <task id> when the steps are clearly working on that queued task, otherwise NONE.";

/** The task id of a `RUNNING: <id>` first line, else null. Pure, so the format is tested without a model. */
export function parseWatch(reply: string): string | null {
  const line = reply.split("\n").find((l) => l.trim()) ?? "";
  return /^RUNNING:\s*(\S+)/i.exec(line.trim())?.[1] ?? null;
}

/** Throwaway read of the newest steps (3000 tokens) against the board: which queued task is being worked on. Throws on a request error. */
export async function piWatch(session: any, board: string): Promise<string | null> {
  const build = () => {
    const selected = selectMessages(session.messages, 3000);
    return `<board>\n${board}\n</board>\n\n<conversation>\n${serializeConversation(convertToLlm(selected))}\n</conversation>`;
  };
  return parseWatch(await sideCall(session, WATCH_SYSTEM, build, 50));
}
