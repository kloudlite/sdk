import { expect, test } from "bun:test";
import { formatContext } from "./HintBar.tsx";

test("formatContext", () => {
  expect(formatContext({ tokens: 84_312 })).toBe("84k ctx");
  expect(formatContext({ tokens: null })).toBe("? ctx");
  expect(formatContext({ tokens: 0 })).toBe("0 ctx");
  expect(formatContext({ tokens: 500_000 })).toBe("0.5M ctx");
});
