import { expect, test } from "bun:test";
import type { AgentMessage } from "@earendil-works/pi-agent-core";
import type { Api, Model } from "@earendil-works/pi-ai";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { MESSAGE_CAP, TOOL_CAP, budgetExtension, budgeted } from "./budget.ts";

// Capture the handlers and tools the extension registers, as pi would call them.
const dir = mkdtempSync(join(tmpdir(), "kl-budget-"));
const hooks: Record<string, (e: any) => any> = {};
let tool: any;
budgetExtension(dir)({
  on: (n: string, h: (e: any) => any) => void (hooks[n] = h),
  registerTool: (t: unknown) => void (tool = t),
} as never);
const result = (toolName: string, text: string, extra = {}) =>
  hooks.tool_result!({ toolName, content: [{ type: "text", text }], ...extra });
const ask = async (p: object) => (await tool.execute("1", p)).content[0].text as string;

test("giant tool result becomes head + note + tail under the cap", async () => {
  const text = "a".repeat(12_000) + "x".repeat(4_600_000) + "z".repeat(12_000);
  const r = await result("exec", text, { structuredContent: { k: 1 }, isError: true });
  const out = r.content[0].text as string;
  expect(out.length).toBeLessThan(TOOL_CAP);
  expect(out.startsWith("a".repeat(12_000))).toBe(true);
  expect(out.endsWith("z".repeat(12_000))).toBe(true);
  expect(out).toContain("[cut: 4624000 characters, middle dropped. Call retrieve with id ");
  const id = /id ([0-9a-f]{12})/.exec(out)![1]!;
  expect((await ask({ id, offset: 12_000, limit: 5 }))).toBe("xxxxx");
  // structuredContent is handed back (pi drops it otherwise); isError is left to pass through.
  expect(r.structuredContent).toEqual({ k: 1 });
  expect(r.isError).toBeUndefined();
});

test("small tool result is untouched", async () => {
  expect(await result("exec", "ok")).toBeUndefined();
});

test("read results are verbatim", async () => {
  expect(await result("read", "r".repeat(100_000))).toBeUndefined();
});

test("context cuts the outgoing copy, never the stored message", () => {
  const big = { role: "user", content: [{ type: "text", text: "q".repeat(100_000) }], timestamp: 1 } as AgentMessage;
  const small = { role: "user", content: "hi", timestamp: 2 } as AgentMessage;
  const r = hooks.context!({ messages: [big, small] });
  expect(r.messages[0].content[0].text.length).toBeLessThan(MESSAGE_CAP);
  expect(r.messages[0].content[0].text).toContain("[cut: 100000 characters");
  expect((big as unknown as { content: { text: string }[] }).content[0]!.text.length).toBe(100_000);
  expect(r.messages[1]).toBe(small);
});

test("context leaves small messages alone", () => {
  expect(hooks.context!({ messages: [{ role: "user", content: "hi", timestamp: 1 }] })).toBeUndefined();
});

const model = (contextWindow: number, maxTokens: number) => ({ id: "m", contextWindow, maxTokens }) as Model<Api>;

test("budgeted clamps big models, keeps small ones, fills zero", () => {
  expect(budgeted(model(1_048_576, 384_000))).toMatchObject({ contextWindow: 200_000, maxTokens: 32_000 });
  expect(budgeted(model(128_000, 8_192))).toMatchObject({ contextWindow: 128_000, maxTokens: 8_192 });
  expect(budgeted(model(0, 0))).toMatchObject({ contextWindow: 200_000, maxTokens: 32_000 });
});
