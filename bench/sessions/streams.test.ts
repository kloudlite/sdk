import { test } from "node:test";
import assert from "node:assert/strict";
import { openStreams } from "./streams.ts";

test("a fresh non-zero count keeps the bench awake", () => {
  assert.equal(openStreams("2", 1_000, 10_000), 2);
});
test("a stale count holds nothing open", () => {
  assert.equal(openStreams("2", 1_000, 16_001), 0);
});
test("a missing or garbled file counts zero", () => {
  assert.equal(openStreams(null, 0, 0), 0);
  assert.equal(openStreams("x", 1_000, 2_000), 0);
});
