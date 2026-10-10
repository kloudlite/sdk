//! The laptop MCP door (`kl-connect mcp`): any MCP client on the laptop gets every tool the bench
//! can reach, for every workspace and environment, with no workspace chosen up front. The client
//! sends one JSON-RPC message per `mcp` wire op (serve.ts) and gets the reply back, null for a
//! notification. Hand-rolled, not the SDK's `Server`: three methods, and the SDK wants a transport
//! this door does not have.
//!
//! No bench permission cards and no `because` here: the client's own approvals stand in for them.
import type { ToolDef } from "@kloudlite-tui/tools";

type Msg = { jsonrpc?: string; id?: string | number | null; method?: string; params?: any };

/** `tools()` is read on every `tools/list`; `tools/call` runs against the last list. */
export function mcpServer(tools: () => Promise<ToolDef[]>): (msg: Msg) => Promise<unknown> {
  let last: Promise<ToolDef[]> | undefined;
  const list = () => (last = tools());
  const reply = (id: Msg["id"], result: unknown) => ({ jsonrpc: "2.0", id, result });
  const fail = (id: Msg["id"], code: number, message: string) => ({ jsonrpc: "2.0", id, error: { code, message } });

  return async (msg) => {
    const { id, method, params } = msg ?? {};
    if (id === undefined || id === null) return null; // a notification (`notifications/initialized`, a cancel)
    switch (method) {
      case "initialize":
        return reply(id, { protocolVersion: params?.protocolVersion ?? "2025-06-18", capabilities: { tools: {} }, serverInfo: { name: "kl", version: "1" } });
      case "ping":
        return reply(id, {});
      case "tools/list":
        return reply(id, { tools: (await list()).map((t) => ({ name: t.name, description: t.description, inputSchema: t.inputSchema })) });
      case "tools/call": {
        const t = (await (last ?? list())).find((t) => t.name === params?.name);
        if (!t) return fail(id, -32602, `unknown tool: ${params?.name}`);
        try {
          return reply(id, { content: [{ type: "text", text: await t.run(params?.arguments ?? {}) }] });
        } catch (e: any) {
          return reply(id, { content: [{ type: "text", text: String(e?.message ?? e) }], isError: true });
        }
      }
      default:
        return fail(id, -32601, `method not found: ${method}`);
    }
  };
}
