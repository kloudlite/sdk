import { expect, test } from "bun:test";
import { createSession } from "@kloudlite-tui/agent";
import { catalog } from "./models.ts";

// Codemode needs two separate things to reach pi: the extension factory on a
// resource loader, and `codemode` named in `tools`. Naming `tools` at all
// replaces the default allowlist, so the four built-ins have to be listed back
// or they vanish — this asserts both halves, and that the flag off leaves the
// defaults alone. pi resolves the list when the session is built and never
// calls the model, so no credentials or network are involved.
const activeTools = async (codemode: boolean) => {
  const s = (await createSession({
    key: `codemode-test-${codemode}`,
    model: catalog[0] as never,
    codemode,
  })) as unknown as { _initialActiveToolNames: string[] };
  return s._initialActiveToolNames;
};

test("codemode off leaves pi's default tools alone", async () => {
  expect(await activeTools(false)).toEqual(["read", "bash", "edit", "write"]);
});

test("codemode on adds it without dropping the built-ins", async () => {
  expect(await activeTools(true)).toEqual(["read", "bash", "edit", "write", "codemode"]);
});
