import { expect, test } from "bun:test";
import { guardExtension } from "./guard.ts";

const hooks: Record<string, (e: any) => any> = {};
guardExtension({ on: (n: string, h: (e: any) => any) => void (hooks[n] = h) } as never);
const call = (toolName: string, input: object) => hooks.tool_result!({ toolName, input, content: [{ type: "text", text: "ok" }] });

test("third identical call gets the note, the second does not", () => {
  hooks.agent_start!({});
  expect(call("exec", { cmd: "sh test.sh" })).toBeUndefined();
  expect(call("exec", { cmd: "sh test.sh" })).toBeUndefined();
  const r = call("exec", { cmd: "sh test.sh" });
  expect(r.content).toHaveLength(2);
  expect(r.content[1].text).toContain("has run 3 times this turn");
  expect(call("exec", { cmd: "sh test.sh" })).toBeUndefined();
});

test("different arguments or tools count separately", () => {
  hooks.agent_start!({});
  for (let i = 0; i < 2; i++) {
    expect(call("exec", { cmd: "a" })).toBeUndefined();
    expect(call("exec", { cmd: "b" })).toBeUndefined();
    expect(call("read", { cmd: "a" })).toBeUndefined();
  }
});

test("agent_start resets the count", () => {
  hooks.agent_start!({});
  call("exec", { cmd: "x" });
  call("exec", { cmd: "x" });
  hooks.agent_start!({});
  expect(call("exec", { cmd: "x" })).toBeUndefined();
});
