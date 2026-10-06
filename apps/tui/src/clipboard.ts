import { execFileSync } from "node:child_process";

export type ClipImage = { type: "image"; data: string; mimeType: string };

/** The clipboard's PNG image, read through `xclip` (a real one, or the bench
 *  browser terminal's stand-in that serves the last pasted image), or null
 *  when there is none or no xclip. */
export function readClipboardImage(): ClipImage | null {
  const xclip = (target: string) =>
    execFileSync("xclip", ["-selection", "clipboard", "-t", target, "-o"], {
      stdio: ["ignore", "pipe", "ignore"],
      maxBuffer: 32 << 20,
      env: process.env, // Bun snapshots env at boot; pass the live one
    });
  try {
    if (!xclip("TARGETS").toString().split("\n").includes("image/png")) return null;
    const bytes = xclip("image/png");
    return bytes.length ? { type: "image", data: bytes.toString("base64"), mimeType: "image/png" } : null;
  } catch {
    return null;
  }
}
