import { expect, test } from "bun:test";
import { testRender } from "@opentui/react/test-utils";
import { Transcript, type Entry } from "./components/Transcript.tsx";

const entries: Entry[] = Array.from({ length: 60 }, (_, i) => ({
  kind: "user" as const,
  text: `line ${i}`,
}));

test("scrolling up offers a jump back to the bottom, end takes it", async () => {
  const t = await testRender(<Transcript entries={entries} />, {
    width: 80,
    height: 16,
    kittyKeyboard: true,
  });
  const frame = async () => {
    await new Promise((r) => setTimeout(r, 30));
    await t.renderOnce();
    return t.captureCharFrame();
  };
  await frame();
  expect(await frame()).not.toContain("jump to bottom"); // sticky at the bottom

  t.mockInput.pressKey("\x1b[5~"); // PageUp — mockInput has no constant for it
  expect(await frame()).toContain("jump to bottom");

  t.mockInput.pressKey("\x1b[F"); // End
  await new Promise((r) => setTimeout(r, 250)); // the at-bottom poll
  expect(await frame()).not.toContain("jump to bottom");
  t.renderer.destroy();
});
