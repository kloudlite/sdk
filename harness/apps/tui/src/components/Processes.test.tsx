import { expect, test } from "bun:test";
import { testRender } from "@opentui/react/test-utils";
import { Processes } from "./Processes.tsx";

test("a long command is clipped and the status stays on one line", async () => {
  const proc = {
    id: "p1",
    name: "node .next/standalone/server.js",
    command: "cd /home/kl/workspace && HOSTNAME=0.0.0.0 PORT=3000 API_URL=http://backend:3000 node .next/standalone/server.js --and-much-more",
    status: "running" as const,
    startedAt: new Date(Date.now() - 37 * 60_000).toISOString(),
    logs: [{ text: "ready" }],
  };
  const setup = await testRender(<Processes workspace="w" processes={[proc]} onClose={() => {}} />, { width: 120, height: 12 });
  await setup.renderOnce();
  const frame = setup.captureCharFrame();
  setup.renderer.destroy();
  const header = frame.split("\n").find((l) => l.includes("cd /home/kl"))!;
  expect(header).toContain("running · ");
  expect(header).toContain("1 lines");
  expect(frame).not.toContain("runnin\n");
});
