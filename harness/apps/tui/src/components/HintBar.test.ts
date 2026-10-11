import { expect, test } from "bun:test";
import { formatContext } from "./HintBar.tsx";

test("formatContext", () => {
  expect(formatContext({ tokens: 84_312, window: 184_000 })).toBe("84k / 184k ctx");
  expect(formatContext({ tokens: null, window: 184_000 })).toBe("? / 184k ctx");
  expect(formatContext({ tokens: 0, window: 184_000 })).toBe("0 / 184k ctx");
  expect(formatContext({ tokens: 84_312, window: 1_000_000 })).toBe("84k / 1M ctx");
  expect(formatContext({ tokens: 500_000, window: 1_000_000 })).toBe("0.5M / 1M ctx");
});
