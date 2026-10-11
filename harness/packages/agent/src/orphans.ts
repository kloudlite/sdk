//! A tool result is only valid straight after the assistant turn that called it.
//! pi's transform-messages skips an assistant turn whose stopReason is "error"/"aborted" but still
//! sends the toolResult messages that answered it, so an OpenAI-compatible provider answers 400
//! ("role 'tool' must be a response to a preceding message with 'tool_calls'") on every later turn.
//! Compaction cutting a call off, or a result before any assistant, does the same.
//! Here and not a pi patch because pi is a dependency; delete when pi drops a toolResult together
//! with its skipped assistant.
import type { AgentMessage } from "@earendil-works/pi-agent-core";
import type { ExtensionFactory } from "@earendil-works/pi-coding-agent";

export function dropOrphanToolResults(messages: AgentMessage[]): AgentMessage[] {
  let open = new Set<string>();
  const kept = messages.filter((m) => {
    if (m.role === "user") open = new Set();
    else if (m.role === "assistant") {
      const survives = m.stopReason !== "error" && m.stopReason !== "aborted";
      open = new Set(survives ? m.content.flatMap((c) => (c.type === "toolCall" ? [c.id] : [])) : []);
    } else if (m.role === "toolResult") return open.has(m.toolCallId);
    return true;
  });
  return kept.length === messages.length ? messages : kept;
}

export const orphansExtension: ExtensionFactory = (pi) => {
  pi.on("context", (e) => {
    const m = dropOrphanToolResults(e.messages);
    return m === e.messages ? undefined : { messages: m };
  });
};
