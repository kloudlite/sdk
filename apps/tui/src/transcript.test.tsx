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

test("a long block collapses to its head and ctrl+o toggles it both ways", async () => {
  const long: Entry[] = [
    { id: "a", kind: "agent", text: Array.from({ length: 25 }, (_, i) => `line ${i + 1}`).join("\n") },
  ];
  const t = await testRender(<Transcript entries={long} keys="page" />, {
    width: 70,
    height: 24,
    kittyKeyboard: true,
  });
  const frame = async () => {
    await new Promise((r) => setTimeout(r, 30));
    await t.renderOnce();
    return t.captureCharFrame();
  };
  // the head is kept, not the tail — Claude Code reads from the beginning
  const first = await frame();
  expect(first).toContain("line 1");
  expect(first).not.toContain("line 25");
  expect(first).toContain("+15 lines");
  expect(first).toContain("to expand");

  t.mockInput.pressKey("o", { ctrl: true });
  const open = await frame();
  expect(open).toContain("line 25");
  expect(open).toContain("to collapse"); // the row stays, offering the way back

  t.mockInput.pressKey("o", { ctrl: true }); // and it really collapses again
  expect(await frame()).not.toContain("line 25");
  t.renderer.destroy();
});

// reasoning is a ticker while it streams and a readable block once it lands —
// the old renderer clipped every thinking entry to one line, so a large
// thinking budget was invisible
test("thinking tickers while streaming and opens up when done", async () => {
  const long = Array.from({ length: 18 }, (_, i) => `reasoning line ${i + 1}`).join("\n");
  const t = await testRender(
    <Transcript
      entries={[
        { kind: "thinking", id: "a", text: "step one\nstep two\nstep three" },
        { kind: "thinking", id: "b", text: long, done: true },
      ]}
    />,
    { width: 90, height: 40 },
  );
  await new Promise((r) => setTimeout(r, 200));
  await t.renderOnce();
  const frame = t.captureCharFrame();
  // streaming: newest line only, earlier ones not kept
  expect(frame).toContain("step three");
  expect(frame).not.toContain("step one");
  // done: labelled, collapsed to its head, expandable
  expect(frame).toContain("Thinking");
  expect(frame).toContain("reasoning line 10");
  expect(frame).toContain("+8 lines");
  expect(frame).not.toContain("reasoning line 11");
}, 20000);
