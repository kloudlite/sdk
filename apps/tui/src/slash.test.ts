import { expect, test } from "bun:test";
import { matchCommands, menuItems } from "./slash.ts";

test("no menu for ordinary prompts", () => {
  expect(matchCommands("what is this repo")).toEqual([]);
  expect(matchCommands("")).toEqual([]);
});

test("bare slash lists everything, prefix narrows", () => {
  expect(matchCommands("/").length).toBeGreaterThan(1);
  expect(matchCommands("/cl").map((c) => c.name)).toEqual(["/clear"]);
  expect(matchCommands("/nope")).toEqual([]);
});

const ctx = {
  models: [] as { provider: string; id: string; hint: string }[],
  themes: [],
  logins: [
    { provider: "anthropic", type: "oauth", label: "Anthropic (Claude Pro/Max)" },
    { provider: "deepseek", type: "api_key", label: "DeepSeek API key" },
  ],
  settings: [],
  sessions: [],
};

// The models list is filtered to connected providers, so a first run has
// nothing to pick — an empty menu would be the only clue that /login exists.
test("/model offers the logins when no provider is connected", () => {
  const items = menuItems("/model ", ctx);
  expect(items.length).toBe(2);
  expect(items.every((i) => i.insert.startsWith("/login "))).toBe(true);
  expect(items[0]!.hint).toContain("connect");
});

test("/model lists models once a provider is connected", () => {
  const items = menuItems("/model ", {
    ...ctx,
    models: [{ provider: "deepseek", id: "deepseek-flash", hint: "DeepSeek V4.1 Flash" }],
  });
  expect(items.map((i) => i.insert)).toEqual(["/model deepseek/deepseek-flash"]);
});

// the login fallback still has to answer the filter the user is typing
test("the login fallback filters", () => {
  expect(menuItems("/model deep", ctx).map((i) => i.insert)).toEqual([
    "/login deepseek api_key",
  ]);
});
