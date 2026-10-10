import { expect, test } from "bun:test";
import { mcpServer } from "./mcp.ts";

const echo = { name: "echo", description: "Say it back", inputSchema: { type: "object" }, run: async (i: any) => `said ${i.text}` };
const boom = { name: "boom", description: "Fails", inputSchema: { type: "object" }, run: async () => { throw new Error("no such workspace"); } };

test("initialize, list, call, failures and notifications", async () => {
  const mcp = mcpServer(async () => [echo, boom]);
  expect(await mcp({ jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2025-03-26" } })).toEqual({ jsonrpc: "2.0", id: 1, result: { protocolVersion: "2025-03-26", capabilities: { tools: {} }, serverInfo: { name: "kl", version: "1" } } });
  expect(await mcp({ jsonrpc: "2.0", method: "notifications/initialized" })).toBeNull();
  expect(((await mcp({ jsonrpc: "2.0", id: 2, method: "tools/list" })) as any).result.tools.map((t: any) => t.name)).toEqual(["echo", "boom"]);
  expect(await mcp({ jsonrpc: "2.0", id: 3, method: "tools/call", params: { name: "echo", arguments: { text: "hi" } } })).toEqual({ jsonrpc: "2.0", id: 3, result: { content: [{ type: "text", text: "said hi" }] } });
  expect(await mcp({ jsonrpc: "2.0", id: 4, method: "tools/call", params: { name: "boom" } })).toEqual({ jsonrpc: "2.0", id: 4, result: { content: [{ type: "text", text: "no such workspace" }], isError: true } });
  expect(((await mcp({ jsonrpc: "2.0", id: 5, method: "tools/call", params: { name: "nope" } })) as any).error.code).toBe(-32602);
  expect(((await mcp({ jsonrpc: "2.0", id: 6, method: "resources/list" })) as any).error.code).toBe(-32601);
});
