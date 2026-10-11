import { afterEach, expect, test } from "bun:test";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { budgetExtension } from "./budget.ts";
import { compress, compressWithModel, grepLines, retrieve } from "./compress.ts";

const dir = () => mkdtempSync(join(tmpdir(), "kl-compress-"));
let tool: any;
let hook: (e: any) => any;
const ext = (d: string) =>
  budgetExtension(d)({ on: (n: string, h: any) => void (n === "tool_result" && (hook = h)), registerTool: (t: unknown) => void (tool = t) } as never);
// Plain text with no structure the rule detectors know, so it takes the model-eligible path.
const PLAIN = "The quick brown fox jumps over the lazy dog. ".repeat(40);
afterEach(() => delete process.env.KL_KOMPRESS_URL);

test("4.6M-character npm-like JSON array: tiny result, exact slices and grep via retrieve", async () => {
  const d = dir();
  ext(d);
  const items = Array.from({ length: 12_000 }, (_, i) => ({
    name: `pkg-${i}`,
    version: "1.0.0",
    description: "x".repeat(300),
    ...(i % 2 ? { keywords: ["a"] } : {}),
    ...(i === 6000 ? { note: "needle-in-the-middle" } : {}),
  }));
  const original = JSON.stringify(items);
  expect(original.length).toBeGreaterThan(4_000_000);
  const r = await hook({ toolName: "exec", content: [{ type: "text", text: original }] });
  const out = r.content[0].text as string;
  expect(out.length).toBeLessThan(8_000);
  const id = /id ([0-9a-f]{12})\./.exec(out)![1]!;
  expect(out).toContain("Call retrieve with this id");
  const slice = await tool.execute("1", { id, offset: 1000, limit: 500 });
  expect(slice.content[0].text).toBe(original.slice(1000, 1500));
  const hit = await tool.execute("1", { id, grep: "NEEDLE-in" });
  expect(hit.content[0].text).toContain("needle-in-the-middle");
});

test("log with 500 repeated lines keeps the ERROR line", () => {
  const lines = Array.from({ length: 500 }, () => "2026-10-11 10:00:00 INFO heartbeat ok");
  lines.splice(250, 0, "2026-10-11 10:00:01 ERROR disk on fire");
  const text = lines.join("\n");
  const c = compress(dir(), text);
  expect(c.strategy).toBe("log");
  expect(c.text.length).toBeLessThan(text.length / 4);
  expect(c.text).toContain("ERROR disk on fire");
});

test("structuredContent passes through when content changes", async () => {
  ext(dir());
  const r = await hook({ toolName: "exec", content: [{ type: "text", text: "line\n".repeat(5000) }], structuredContent: { k: 1 } });
  expect(r.structuredContent).toEqual({ k: 1 });
});

test("kompress: a good reply is used for plain text and the original is retrievable", async () => {
  const d = dir();
  const srv = Bun.serve({ port: 0, fetch: () => Response.json({ compressed: "short", original_tokens: 400, compressed_tokens: 10 }) });
  process.env.KL_KOMPRESS_URL = `http://localhost:${srv.port}`;
  try {
    const c = await compressWithModel(d, PLAIN);
    expect(c.strategy).toBe("kompress");
    expect(c.text).toContain("Call retrieve with this id");
    expect(retrieve(d, c.hash!)).toBe(PLAIN);
  } finally {
    srv.stop(true);
  }
});

test("kompress: any shrink is used, no shrink falls back to rules", async () => {
  const d = dir();
  let reply = { compressed: "shorter", original_tokens: 1029, compressed_tokens: 874 };
  const srv = Bun.serve({ port: 0, fetch: () => Response.json(reply) });
  process.env.KL_KOMPRESS_URL = `http://localhost:${srv.port}`;
  try {
    expect((await compressWithModel(d, PLAIN)).strategy).toBe("kompress");
    reply = { compressed: "same", original_tokens: 1029, compressed_tokens: 1029 };
    expect((await compressWithModel(d, PLAIN)).strategy).not.toBe("kompress");
  } finally {
    srv.stop(true);
    delete process.env.KL_KOMPRESS_URL;
  }
});

test("kompress: 500 and unset URL fall back to rules", async () => {
  const d = dir();
  const rules = compress(d, PLAIN);
  expect((await compressWithModel(d, PLAIN)).text).toBe(rules.text);
  const srv = Bun.serve({ port: 0, fetch: () => new Response("boom", { status: 500 }) });
  process.env.KL_KOMPRESS_URL = `http://localhost:${srv.port}`;
  try {
    expect((await compressWithModel(d, PLAIN)).text).toBe(rules.text);
  } finally {
    srv.stop(true);
  }
});

test("refusals and small text pass; bad ids are refused", async () => {
  const d = dir();
  ext(d);
  expect(compress(d, "Error: nope\n".repeat(200)).strategy).toBe("pass");
  expect(retrieve(d, "../x")).toBeUndefined();
  expect((await tool.execute("1", { id: "../x" })).content[0].text).toBe("no stored result ../x");
  expect(grepLines("a\nB\nc", "b")).toBe("2: B");
});
