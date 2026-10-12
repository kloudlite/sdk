import { describe, expect, test } from "bun:test";
import { ensureRunning } from "./ensure.ts";

const doc = (o: any) => o;
function fakeApi(seq: any[]) {
  const calls: string[] = [];
  const api = async (method: string, path: string) => {
    calls.push(`${method} ${path}`);
    if (method === "POST") return {} as any;
    return (seq.length > 1 ? seq.shift() : seq[0]) as any;
  };
  return { api, calls };
}
const sleep = async () => {};

describe("ensureRunning", () => {
  test("ready returns without starting", async () => {
    const f = fakeApi([doc({ state: "running", ready: true })]);
    await ensureRunning("workspaces", "w1", { api: f.api as any, sleep });
    expect(f.calls).toEqual(["GET /v1/workspaces/w1"]);
  });
  test("a workspace in state ready returns too", async () => {
    const f = fakeApi([doc({ state: "ready" })]);
    await ensureRunning("workspaces", "w1", { api: f.api as any, sleep });
    expect(f.calls).toEqual(["GET /v1/workspaces/w1"]);
  });
  test("idle-stopped wakes and waits", async () => {
    const f = fakeApi([doc({ state: "stopped", idle_since: "2026-10-12T10:00:00Z" }), doc({ state: "creating" }), doc({ state: "ready" })]);
    await ensureRunning("workspaces", "w1", { api: f.api as any, sleep });
    expect(f.calls).toEqual(["GET /v1/workspaces/w1", "POST /v1/workspaces/w1/start", "GET /v1/workspaces/w1", "GET /v1/workspaces/w1"]);
  });
  test("explicitly stopped is an error naming workspace_start", async () => {
    const f = fakeApi([doc({ state: "stopped" })]);
    await expect(ensureRunning("workspaces", "w1", { api: f.api as any, sleep })).rejects.toThrow("stopped; start it with `workspace_start`");
    expect(f.calls).toEqual(["GET /v1/workspaces/w1"]);
  });
  test("timeout names the last condition reason", async () => {
    const f = fakeApi([
      doc({ state: "stopped", idle_since: "x" }),
      doc({ state: "creating", packages_status: { type: "PackagesReady", status: "False", reason: "KeysNotReady" } }),
    ]);
    await expect(ensureRunning("workspaces", "w1", { api: f.api as any, sleep, timeoutMs: 0 })).rejects.toThrow("KeysNotReady");
  });
});
