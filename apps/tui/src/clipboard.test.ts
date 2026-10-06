import { afterAll, expect, test } from "bun:test";
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { readClipboardImage } from "./clipboard.ts";

// A fake xclip on PATH, holding whatever $FAKE_CLIP points at.
const dir = mkdtempSync(join(tmpdir(), "clip-"));
writeFileSync(
  join(dir, "xclip"),
  `#!/bin/sh
[ -f "$FAKE_CLIP" ] || exit 1
case "$*" in
  *TARGETS*) echo image/png ;;
  *image/png*) cat "$FAKE_CLIP" ;;
  *) exit 1 ;;
esac
`,
);
chmodSync(join(dir, "xclip"), 0o755);
const path = process.env.PATH;
process.env.PATH = `${dir}:${path}`;
afterAll(() => {
  process.env.PATH = path;
  rmSync(dir, { recursive: true });
});

test("reads a PNG off the clipboard as base64", () => {
  writeFileSync(join(dir, "png"), "PNGBYTES");
  process.env.FAKE_CLIP = join(dir, "png");
  expect(readClipboardImage()).toEqual({
    type: "image",
    data: Buffer.from("PNGBYTES").toString("base64"),
    mimeType: "image/png",
  });
});

test("no image on the clipboard is null", () => {
  process.env.FAKE_CLIP = join(dir, "missing");
  expect(readClipboardImage()).toBeNull();
});
