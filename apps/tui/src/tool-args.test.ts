import { test, expect } from "bun:test";
import { Registry, defineTool } from "@kloudlite-tui/tools";
import { createSession } from "@kloudlite-tui/agent";

// pi's execute() takes params as its SECOND argument and our adapter passes it
// through `as never`, so nothing at compile time catches a shift back to the
// first. This is the only guard: every custom tool silently loses its
// parameters if it regresses.
test("a custom tool receives its parameters", async () => {
  let seen: unknown = "NEVER CALLED";
  const echo = defineTool<{ word: string }>({
    name: "echo_probe",
    description: "Echo the word back. Call it exactly once with word='banana'.",
    inputSchema: { type: "object", properties: { word: { type: "string" } }, required: ["word"] },
    async run(args) { seen = args; return `echoed ${JSON.stringify(args)}`; },
  });
  const s = await createSession({ key: "tool-args-regression", fresh: true, registry: new Registry().add(echo) });
  await s.prompt("Call echo_probe with word='banana'. Nothing else.");
  console.log("SEEN:", JSON.stringify(seen));
  expect(seen).toEqual({ word: "banana" });
}, 120000);
