import { expect, test } from "bun:test";
import type { AgentMessage } from "@earendil-works/pi-agent-core";
import { dropOrphanToolResults } from "./orphans.ts";

const user = { role: "user", content: [{ type: "text", text: "hi" }], timestamp: 1 } as AgentMessage;
const assistant = (stopReason: string, ...ids: string[]) =>
  ({
    role: "assistant", content: ids.map((id) => ({ type: "toolCall", id, name: "t", arguments: {} })),
    api: "x", provider: "p", model: "m",
    usage: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 0, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } },
    stopReason, timestamp: 2,
  }) as AgentMessage;
const result = (toolCallId: string) =>
  ({ role: "toolResult", toolCallId, toolName: "t", content: [{ type: "text", text: "ok" }], isError: false, timestamp: 3 }) as AgentMessage;

test("aborted assistant's tool result is dropped", () => {
  const a = assistant("aborted", "a");
  expect(dropOrphanToolResults([a, result("a"), user])).toEqual([a, user]);
});

test("errored assistant's tool result is dropped", () => {
  const a = assistant("error", "a");
  expect(dropOrphanToolResults([a, result("a"), user])).toEqual([a, user]);
});

test("a normal call and its result stay, same array", () => {
  const msgs = [user, assistant("toolUse", "a"), result("a")];
  expect(dropOrphanToolResults(msgs)).toBe(msgs);
});

test("a result before any assistant is dropped", () => {
  expect(dropOrphanToolResults([result("a"), user])).toEqual([user]);
});

test("a result for a call the assistant never made is dropped", () => {
  const a = assistant("toolUse", "a");
  expect(dropOrphanToolResults([a, result("a"), result("b")])).toEqual([a, result("a")]);
});
