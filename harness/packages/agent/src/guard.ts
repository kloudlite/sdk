//! A session does not run the same call again and again hoping for a different answer.
//! Incident ws-73, 2026-10-11/12: `sh test.sh` ran 7 times and the same detached start 3 times in one
//! turn, each time with an unchanged result. The rule lived only in prompt text and was ignored.
//! Here the harness counts identical calls (tool name + arguments) per turn and, on every third
//! repeat, appends a note to the result. It never blocks or alters the call: a repeat is sometimes
//! right (a poll after a change), so the model gets the evidence and decides.
//! The count resets on `agent_start`; a new turn is a new situation.
import type { ExtensionFactory } from "@earendil-works/pi-coding-agent";

const EVERY = 3;

export const guardExtension: ExtensionFactory = (pi) => {
  const seen = new Map<string, number>();
  pi.on("agent_start", () => void seen.clear());
  pi.on("tool_result", (e) => {
    // ponytail: key is the raw JSON of the arguments, so reordered keys count as different calls.
    const key = `${e.toolName}\0${JSON.stringify(e.input)}`;
    const n = (seen.get(key) ?? 0) + 1;
    seen.set(key, n);
    if (n % EVERY !== 0) return undefined;
    const note = `[repeat: this exact call has run ${n} times this turn. Running it again will not change the result unless something else changed. Change the approach; to wait for an environment use env_wait.]`;
    // pi drops structuredContent when content is replaced without it; codemode reads it, so hand it back.
    return { content: [...e.content, { type: "text" as const, text: note }], structuredContent: e.structuredContent };
  });
};
