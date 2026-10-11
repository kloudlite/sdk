import { afterEach, beforeEach, expect, test } from "bun:test";
import { mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { platformTools } from "./platform.ts";
import { repoTools } from "./repos.ts";

type Call = { method: string; url: string; type: string | null; body: string | undefined };
let calls: Call[] = [];
let routes: Record<string, () => Response> = {};
const realFetch = globalThis.fetch;

beforeEach(() => {
  calls = [];
  routes = {};
  const f = join(mkdtempSync(join(tmpdir(), "kl-tok-")), "token");
  writeFileSync(f, "tok\n");
  process.env.KL_API_URL = "http://api.test";
  process.env.KL_TOOL_TOKEN_FILE = f;
  globalThis.fetch = (async (url: string, init: any = {}) => {
    const u = String(url).replace("http://api.test", "");
    calls.push({ method: init.method ?? "GET", url: u, type: init.headers?.["content-type"] ?? null, body: init.body });
    const r = routes[`${init.method ?? "GET"} ${u}`];
    return r ? r() : new Response("not found", { status: 404 });
  }) as any;
});
afterEach(() => {
  globalThis.fetch = realFetch;
  delete process.env.KL_API_URL;
  delete process.env.KL_TOOL_TOKEN_FILE;
});

const run = (n: string, a: any) => repoTools().find((t) => t.name === n)!.run(a);
const ok = () => new Response("{}", { status: 200 });
const sent = () => calls.map((c) => [c.method, c.url, c.body === undefined ? undefined : c.body]);

test("every kind carries the repo tools", () => {
  const names = repoTools().map((t) => t.name);
  expect(names.length).toBe(21);
  for (const k of ["main", "workspace", "all"] as const) {
    const have = new Set(platformTools(k, "ws-1").map((t) => t.name));
    for (const n of names) expect(have.has(n)).toBe(true);
  }
});

test("repo tools send their method, path, query and body", async () => {
  const cases: [string, any, string, string, any?][] = [
    ["repo_list", { owner: "a" }, "GET", "/v1/repos?owner=a"],
    ["repo_get", { owner: "a", name: "r" }, "GET", "/v1/repos/a/r"],
    ["repo_create", { owner: "a", name: "r", visibility: "public" }, "POST", "/v1/repos", { owner: "a", name: "r", visibility: "public" }],
    ["repo_update", { owner: "a", name: "r", description: "d" }, "PATCH", "/v1/repos/a/r", { description: "d" }],
    ["repo_delete", { owner: "a", name: "r" }, "DELETE", "/v1/repos/a/r"],
    ["protection_get", { owner: "a", name: "r" }, "GET", "/v1/repos/a/r/protection"],
    ["protection_set", { owner: "a", name: "r", pattern: "main", no_force: true }, "POST", "/v1/repos/a/r/protection", { pattern: "main", no_force: true }],
    ["branch_list", { owner: "a", name: "r" }, "GET", "/v1/repos/a/r/branches"],
    ["compare", { owner: "a", name: "r", base: "main", head: "f/x" }, "GET", "/v1/repos/a/r/compare?base=main&head=f%2Fx"],
    ["pr_list", { owner: "a", name: "r", state: "open", limit: 5 }, "GET", "/v1/repos/a/r/pulls?state=open&limit=5"],
    ["pr_get", { owner: "a", name: "r", number: 7 }, "GET", "/v1/repos/a/r/pulls/7"],
    ["pr_create", { owner: "a", name: "r", title: "t", base: "main", head: "x" }, "POST", "/v1/repos/a/r/pulls", { title: "t", base: "main", head: "x" }],
    ["pr_comment", { owner: "a", name: "r", number: 7, body: "hi" }, "POST", "/v1/repos/a/r/pulls/7/comments", { body: "hi" }],
    ["pr_merge", { owner: "a", name: "r", number: 7, strategy: "squash" }, "POST", "/v1/repos/a/r/pulls/7/merge?strategy=squash"],
    ["pr_merge", { owner: "a", name: "r", number: 7 }, "POST", "/v1/repos/a/r/pulls/7/merge"],
    ["pr_close", { owner: "a", name: "r", number: 7 }, "POST", "/v1/repos/a/r/pulls/7/close"],
    ["image_list", { owner: "a" }, "GET", "/api/a/images"],
    ["image_tags", { owner: "a", image: "app" }, "GET", "/api/a/app/imagetags"],
    ["image_visibility", { owner: "a", image: "app", visibility: "public" }, "POST", "/api/a/app/imagevisibility?visibility=public"],
    ["image_delete", { owner: "a", image: "app" }, "POST", "/api/a/app/imagedelete"],
  ];
  for (const [n, a, m, u, b] of cases) {
    calls = [];
    routes[`${m} ${u}`] = ok;
    await run(n, a);
    expect(calls.length).toBe(1);
    expect([n, calls[0]!.method, calls[0]!.url, calls[0]!.body && JSON.parse(calls[0]!.body)]).toEqual([n, m, u, b]);
  }
});

test("image_tag_delete sends the tag as raw text", async () => {
  routes["POST /api/a/app/imagetagdelete"] = ok;
  await run("image_tag_delete", { owner: "a", image: "app", tag: "v1" });
  expect(calls[0]!.body).toBe("v1");
  expect(calls[0]!.type).toBe("text/plain");
});

test("branch_delete reads the oid, then deletes with it", async () => {
  routes["GET /v1/repos/a/r/branches"] = () => new Response(JSON.stringify([{ name: "main", oid: "1".repeat(40) }, { name: "f/x", oid: "2".repeat(40) }]));
  routes[`DELETE /v1/repos/a/r/branches/f%2Fx?oid=${"2".repeat(40)}`] = () => new Response(null, { status: 204 });
  expect(await run("branch_delete", { owner: "a", name: "r", branch: "f/x" })).toBe("ok");
  expect(sent().map((c) => c[0])).toEqual(["GET", "DELETE"]);
});

test("branch_delete on an unknown branch throws and sends no DELETE", async () => {
  routes["GET /v1/repos/a/r/branches"] = () => new Response("[]");
  await expect(run("branch_delete", { owner: "a", name: "r", branch: "gone" })).rejects.toThrow("no such branch: gone");
  expect(calls.map((c) => c.method)).toEqual(["GET"]);
});
