import { useEffect, useRef, useState } from "react";
import { useKeyboard, usePaste } from "@opentui/react";
import { TextAttributes } from "@opentui/core";
import { theme } from "../theme.ts";

export const SPECIAL = new Set([
  "return", "enter", "linefeed", "tab", "backspace", "delete", "escape",
  "up", "down", "left", "right", "pageup", "pagedown", "home", "end", "insert",
]);

/**
 * Owned multiline input. Consumes ONLY plain printable keys and edit keys;
 * anything ctrl/meta is left for the app-level handler. There is no focus
 * state — this is always the text consumer.
 *
 * Newlines: Shift+Enter or Alt/Option+Enter inserts one; a trailing "\" +
 * Enter continues on the next line; plain Enter submits.
 */
export function Input({
  value,
  onChange,
  onSubmit,
  placeholder,
  showCursor,
  active = true,
  mask = false,
  onPasteImage,
}: {
  value: string;
  onChange: (v: string) => void;
  onSubmit: (v: string) => void;
  placeholder: string;
  showCursor: boolean;
  active?: boolean;
  /** Render bullets instead of the value (secrets). */
  mask?: boolean;
  /**
   * ctrl+v: hand back a placeholder to insert at the caret, or null when the
   * clipboard holds no image. Like opencode, the attachment is a token in the
   * value, so the cursor lands after it and later text goes where it is typed.
   */
  onPasteImage?: () => string | null;
}) {
  const [cursor, setCursor] = useState(value.length);
  // Distinguish our own edits from external value changes (menu insert,
  // history recall): external changes move the cursor to the end.
  const expected = useRef(value);
  useEffect(() => {
    if (value !== expected.current) {
      expected.current = value;
      cursorRef.current = value.length;
      setCursor(value.length);
    }
  }, [value]);
  const cursorRef = useRef(cursor);
  const change = (v: string) => {
    expected.current = v;
    onChange(v);
  };
  const moveCursor = (c: number) => {
    cursorRef.current = c;
    setCursor(c);
  };
  const pos = Math.min(cursor, value.length);

  const insert = (text: string) => {
    const v = expected.current;
    const p = Math.min(cursorRef.current, v.length);
    change(v.slice(0, p) + text + v.slice(p));
    moveCursor(p + text.length);
  };

  usePaste((event) => {
    if (active)
      insert(new TextDecoder().decode(event.bytes).replace(/\r\n?/g, "\n"));
  });

  useKeyboard((key) => {
    if (!active) return;
    if (key.ctrl && key.name === "v") {
      const token = onPasteImage?.();
      if (token) insert(token);
      return;
    }
    // Live refs, not render props: a burst of keys in one stdin chunk runs
    // every callback against the same stale render.
    const v = expected.current;
    const p = Math.min(cursorRef.current, v.length);

    if (key.name === "return") {
      if (key.meta || key.shift || key.option) return insert("\n");
      if (v.endsWith("\\")) {
        // continuation: swap the trailing backslash for a newline
        change(v.slice(0, -1) + "\n");
        moveCursor(v.length);
        return;
      }
      onSubmit(v);
      // reset the live refs without onChange: the submit handler owns the
      // next value (may immediately set e.g. "/login "), and the external-
      // change effect re-syncs when that lands.
      expected.current = "";
      moveCursor(0);
      return;
    }
    if (key.ctrl || key.meta || key.option) return;
    if (key.name === "left") return moveCursor(Math.max(0, p - 1));
    if (key.name === "right") return moveCursor(Math.min(v.length, p + 1));
    if (key.name === "backspace" || key.name === "delete") {
      if (p > 0) {
        change(v.slice(0, p - 1) + v.slice(p));
        moveCursor(p - 1);
      }
      return;
    }
    if (SPECIAL.has(key.name)) return;
    const text = key.sequence;
    if (text && !text.startsWith("\x1b") && text >= " ") insert(text);
  });

  if (value === "") {
    return (
      <text>
        {showCursor && <span attributes={TextAttributes.INVERSE}> </span>}
        <span fg={theme.placeholder}>{placeholder || " "}</span>
      </text>
    );
  }

  // An attachment token in the value renders as a highlighted badge, so the
  // text keeps the attachment's place and the cursor moves across it normally.
  const badged = (text: string, key: string) =>
    text.split(/(\[Image \d+\])/).map((part, i) =>
      /^\[Image \d+\]$/.test(part) ? (
        <span key={`${key}-${i}`} bg={theme.warning} fg={theme.bg} attributes={TextAttributes.BOLD}>
          {part}
        </span>
      ) : (
        <span key={`${key}-${i}`}>{part}</span>
      ),
    );

  // Render lines with the cursor on the right one.
  const display = mask ? "•".repeat(value.length) : value;
  const lines = display.split("\n");
  let offset = 0;
  return (
    <box flexDirection="column">
      {lines.map((line, i) => {
        const start = offset;
        const end = start + line.length;
        offset = end + 1; // account for the newline
        const cursorHere = showCursor && pos >= start && pos <= end;
        const col = pos - start;
        return (
          <text key={i} fg={theme.fg}>
            {cursorHere ? (
              <>
                {badged(line.slice(0, col), `${i}a`)}
                <span attributes={TextAttributes.INVERSE}>{line[col] ?? " "}</span>
                {badged(line.slice(col + 1), `${i}b`)}
              </>
            ) : (
              badged(line || " ", `${i}`)
            )}
          </text>
        );
      })}
    </box>
  );
}
