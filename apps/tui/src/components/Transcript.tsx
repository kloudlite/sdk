import { useEffect, useRef, useState } from "react";
import { useKeyboard } from "@opentui/react";
import { TextAttributes, type ScrollBoxRenderable } from "@opentui/core";
import { theme } from "../theme.ts";
import { SplitBorder } from "../ui/border.ts";
import { DiffView } from "./Diff.tsx";
import type { FileDiff } from "../diff.ts";

export type Entry =
  | { kind: "user"; text: string; images?: number }
  | { kind: "agent"; id?: string; text: string }
  | {
      kind: "thinking";
      id?: string;
      text: string;
      /** the message finished streaming — render the whole block, not a ticker */
      done?: boolean;
    }
  | {
      kind: "tool";
      id?: string;
      name: string;
      summary: string;
      status?: "running" | "ok" | "error";
      /** Streaming / final output (rendered for bash blocks). */
      output?: string;
      error?: string;
      /** Unified diff hunk (edit/write tools). */
      diff?: FileDiff;
    }
  | { kind: "info"; text: string }
  | { kind: "error"; text: string };

/** Rows kept when a block is collapsed (Claude Code shows a short head). */
const COLLAPSE_MAX = 10;

/**
 * Soft-wrap one source line to `width`, the way the terminal will draw it, so
 * a row count means rows on screen. Reasoning and bash output arrive as long
 * unbroken paragraphs: counting "\n" said 3 lines where the terminal drew 9,
 * and the block was then sized for 3 — the expander landed on top of the text.
 */
function wrap(line: string, width: number): string[] {
  if (line.length <= width) return [line];
  const rows: string[] = [];
  let rest = line;
  while (rest.length > width) {
    // break on the last space that fits; a word longer than the width is cut
    const cut = rest.lastIndexOf(" ", width);
    const at = cut > 0 ? cut : width;
    rows.push(rest.slice(0, at));
    rest = rest.slice(cut > 0 ? at + 1 : at);
  }
  if (rest) rows.push(rest);
  return rows;
}

/**
 * Collapse a block to its first `COLLAPSE_MAX` rows — Claude Code keeps the
 * head, not the tail, so a long block still reads from its beginning. Returns
 * the kept text and how many rows it hid, so the caller can offer the expander.
 * `width` is the column count the block is drawn into; rows are counted after
 * wrapping to it, since that is what the user actually sees.
 */
function collapse(
  text: string | undefined,
  open: boolean,
  width = Infinity,
): { text: string; hidden: number } {
  if (!text) return { text: "", hidden: 0 };
  const rows = text
    .trim()
    .split("\n")
    .flatMap((line) => wrap(line, width));
  if (open || rows.length <= COLLAPSE_MAX) return { text: rows.join("\n"), hidden: 0 };
  return { text: rows.slice(0, COLLAPSE_MAX).join("\n"), hidden: rows.length - COLLAPSE_MAX };
}

/**
 * Whether this entry has a block long enough to collapse — the wrapper needs
 * to know before rendering the Row, so the whole cell can be the toggle and
 * only entries that actually collapse react to a click.
 */
function collapsible(entry: Entry, width: number): boolean {
  if (entry.kind === "agent") return rowCount(entry.text, width - 3) > COLLAPSE_MAX;
  if (entry.kind === "thinking")
    return !!entry.done && rowCount(entry.text, width - 3) > COLLAPSE_MAX;
  if (entry.kind === "tool" && entry.name === "bash")
    return rowCount(entry.output, width - 2) > COLLAPSE_MAX;
  return false;
}

/** Rows a block will occupy once wrapped — decides if it needs an expander. */
function rowCount(text: string | undefined, width: number): number {
  if (!text) return 0;
  return text.trim().split("\n").flatMap((line) => wrap(line, width)).length;
}

/**
 * Claude Code's expander, under the block it belongs to: "… +N lines (ctrl+o
 * to expand)", and once open the same row offers to collapse it again. Click
 * it or press ctrl+o.
 */
function More({
  hidden,
  open,
  onToggle,
  hover,
}: {
  hidden: number;
  open: boolean;
  onToggle?: () => void;
  /** the pointer is over this entry — the row brightens to say it is clickable */
  hover?: boolean;
}) {
  return (
    <box height={1} onMouseDown={onToggle}>
      <text fg={hover ? theme.fg : theme.muted} selectable={false}>
        {open ? "… " : `… +${hidden} lines `}
        <span fg={theme.accent}>{hover ? "click" : "ctrl+o"}</span>
        {open ? " to collapse" : " to expand"}
      </text>
    </box>
  );
}

/** Minimal inline markdown: **bold** and `code`. */
function Md({ text, fg }: { text: string; fg?: string }) {
  const parts = text.split(/(\*\*[^*]+\*\*|`[^`]+`)/g);
  return (
    <text fg={fg ?? theme.fg}>
      {parts.map((part, i) => {
        if (part.startsWith("**") && part.endsWith("**"))
          return <b key={i}>{part.slice(2, -2)}</b>;
        if (part.startsWith("`") && part.endsWith("`"))
          return <span key={i} fg={theme.accent}>{part.slice(1, -1)}</span>;
        return <span key={i}>{part}</span>;
      })}
    </text>
  );
}

const inlineIcon: Record<string, string> = {
  read: "→",
  glob: "→",
  grep: "→",
  write: "←",
  edit: "←",
};

const inlineVerb: Record<string, string> = {
  read: "Read",
  glob: "Glob",
  grep: "Grep",
  write: "Write",
  edit: "Edit",
};

/** Is this entry a one-line inline tool row (stacks tight, opencode-style)? */
function isInlineTool(entry: Entry): boolean {
  return entry.kind === "tool" && entry.name !== "bash";
}

/** Cap on rendered entries; older ones fall out of the scrollback. */
const SCROLLBACK = 200;

function Row({
  entry,
  open,
  onOpen,
  width,
  hover,
}: {
  entry: Entry;
  /** this entry is expanded — render every line */
  open: boolean;
  onOpen?: () => void;
  /** columns the row is drawn into — collapse counts wrapped rows at it */
  width: number;
  /** the pointer is over this entry and it can collapse */
  hover?: boolean;
}) {
  switch (entry.kind) {
    case "user":
      // opencode UserMessage: native left border ┃ on the panel background
      return (
        <box
          border={["left"]}
          customBorderChars={SplitBorder.customBorderChars}
          borderColor={theme.accent}
        >
          <box flexDirection="column" paddingLeft={2} paddingTop={1} paddingBottom={1} backgroundColor={theme.surface}>
            <text fg={theme.fg}>{entry.text}</text>
            {entry.images ? (
              // where the attachments came from, as a label/value badge pair
              <>
                <text> </text>
                <box flexDirection="row" height={1}>
                  <box backgroundColor={theme.accent} paddingLeft={1} paddingRight={1}>
                    <text fg={theme.bg}>{entry.images === 1 ? "File" : `${entry.images} Files`}</text>
                  </box>
                  <box backgroundColor={theme.surfaceRaised} paddingLeft={1} paddingRight={1}>
                    <text fg={theme.muted}>clipboard</text>
                  </box>
                </box>
              </>
            ) : null}
          </box>
        </box>
      );
    case "agent": {
      // opencode TextPart: markdown, paddingLeft 3
      const body = collapse(entry.text, open, width - 3);
      // the expander stays visible once open, so the block can be re-collapsed
      const long = rowCount(entry.text, width - 3) > COLLAPSE_MAX;
      return (
        // hovering a collapsible block tints it, the way a desktop list row
        // lights up under the pointer — subtle, one step off the background
        <box flexDirection="column" paddingLeft={3} backgroundColor={hover ? theme.surface : undefined}>
          <Md text={body.text} />
          {long && <More hidden={body.hidden} open={open} onToggle={onOpen} hover={hover} />}
        </box>
      );
    }
    case "thinking": {
      // while streaming, reasoning is a progress ticker: its newest line only
      if (!entry.done) {
        const lines = entry.text.trim().split("\n");
        return (
          <box paddingLeft={3} height={1} overflow="hidden">
            <text fg={theme.muted} attributes={TextAttributes.ITALIC}>
              {lines[lines.length - 1] ?? ""}
            </text>
          </box>
        );
      }
      // finished: the same collapsing block an agent message gets, so a long
      // reasoning budget is actually readable instead of clipped to one line
      const body = collapse(entry.text, open, width - 3);
      const long = rowCount(entry.text, width - 3) > COLLAPSE_MAX;
      return (
        <box flexDirection="column" paddingLeft={3} backgroundColor={hover ? theme.surface : undefined}>
          <text fg={theme.muted} attributes={TextAttributes.ITALIC}>
            Thinking
          </text>
          <Md text={body.text} fg={theme.muted} />
          {long && <More hidden={body.hidden} open={open} onToggle={onOpen} hover={hover} />}
        </box>
      );
    }
    case "tool": {
      const running = entry.status === "running";
      const failed = entry.status === "error";

      if (entry.name === "bash") {
        // opencode Shell via BlockTool: panel bg block, $ command, output tail
        const out = collapse(entry.output, open, width - 2);
        return (
          <box
            flexDirection="column"
            paddingLeft={2}
            paddingTop={1}
            paddingBottom={1}
            backgroundColor={hover ? theme.surfaceRaised : theme.surface}
          >
            <text fg={running ? theme.fg : theme.muted}>
              {running ? "⚙ " : "$ "}
              {entry.summary}
            </text>
            {out.text !== "" && <text fg={theme.muted}>{out.text}</text>}
            {rowCount(entry.output, width - 2) > COLLAPSE_MAX && (
              <More hidden={out.hidden} open={open} onToggle={onOpen} hover={hover} />
            )}
            {entry.error && <text fg={theme.error}>{entry.error}</text>}
          </box>
        );
      }

      if (entry.diff) {
        // Claude Code's Update block: ● Update(path), stats line, diff hunk
        const verb = entry.name === "write" ? "Write" : "Update";
        const stats = [
          entry.diff.added && `Added ${entry.diff.added} line${entry.diff.added === 1 ? "" : "s"}`,
          entry.diff.removed && `removed ${entry.diff.removed} line${entry.diff.removed === 1 ? "" : "s"}`,
        ]
          .filter(Boolean)
          .join(", ");
        return (
          <box flexDirection="column" paddingLeft={1}>
            <text>
              <span fg={failed ? theme.error : theme.success}>●</span>
              <span fg={theme.fg}> {verb}</span>
              <span fg={theme.muted}>({entry.diff.path})</span>
            </text>
            <box paddingLeft={2}>
              <text fg={theme.muted}>⎿ {stats}</text>
            </box>
            <box paddingLeft={2} flexDirection="column">
              <DiffView diff={entry.diff} />
            </box>
            {entry.error && (
              <box paddingLeft={2}>
                <text fg={theme.error}>{entry.error}</text>
              </box>
            )}
          </box>
        );
      }

      // opencode InlineToolRow: icon column (width 2) + content, tight rows
      const icon = failed ? "✗" : (inlineIcon[entry.name] ?? "·");
      const verb = inlineVerb[entry.name] ?? entry.name;
      const fg = failed ? theme.error : running ? theme.fg : theme.muted;
      return (
        <box flexDirection="column" paddingLeft={3}>
          <box flexDirection="row">
            <box width={2} flexShrink={0}>
              <text fg={fg}>{running ? "⚙" : icon}</text>
            </box>
            <text fg={fg}>
              {verb}{" "}
              <span attributes={failed ? undefined : TextAttributes.DIM}>
                {entry.summary}
              </span>
            </text>
          </box>
          {entry.error && (
            <box paddingLeft={2}>
              <text fg={theme.error}>{entry.error}</text>
            </box>
          )}
        </box>
      );
    }
    case "info":
      return (
        <box paddingLeft={3}>
          <text fg={theme.muted}>
            {entry.text}
          </text>
        </box>
      );
    case "error":
      return (
        <box
          border={["left"]}
          customBorderChars={SplitBorder.customBorderChars}
          borderColor={theme.error}
        >
          <box paddingLeft={2} paddingTop={1} paddingBottom={1} backgroundColor={theme.surface}>
            <text fg={theme.error}>{entry.text}</text>
          </box>
        </box>
      );
  }
}

/**
 * Message scrollback on opentui's scrollbox: cell-accurate compositing,
 * native mouse-wheel scrolling, sticky bottom while streaming (opencode's
 * own setup). PageUp/PageDown page manually via the scrollbox ref.
 */
export function Transcript({
  entries,
  keys = "page",
  width = 80,
}: {
  entries: Entry[];
  /** content columns available — long blocks wrap, so row counts need it */
  width?: number;
  /** "off" while a modal owns keys; "page" = pgup/pgdn; "normal" adds u/d. */
  keys?: "off" | "page" | "normal";
}) {
  const scrollRef = useRef<ScrollBoxRenderable>(null);
  // scrolled up far enough that new output lands off screen — shows the
  // jump-to-bottom affordance, which `stickyScroll` otherwise hides
  const [away, setAway] = useState(false);
  // keys of entries the user expanded; ctrl+o (o in vim NORMAL) expands all
  const [open, setOpen] = useState<Set<string>>(new Set());
  const [openAll, setOpenAll] = useState(false);
  // the entry the pointer is over, so its expander can light up
  const [hover, setHover] = useState<string | null>(null);
  // where the button went down, so an up in the same cell is a click and an
  // up somewhere else is the end of a text selection. opentui marks every up
  // isDragging once a mousedown over selectable text has started a selection,
  // so that flag alone cannot tell the two apart.
  const downAt = useRef<{ x: number; y: number } | null>(null);

  // ponytail: opentui's scrollbox has no onScroll, so "am I at the bottom?"
  // is sampled on a timer — cheap, and the only hook the wheel also trips
  useEffect(() => {
    const id = setInterval(() => {
      const sb = scrollRef.current;
      if (sb) setAway(!atBottom(sb));
    }, 200);
    return () => clearInterval(id);
  }, []);

  const atBottom = (sb: ScrollBoxRenderable) =>
    sb.scrollTop >= sb.scrollHeight - sb.viewport.height - 1;
  const toBottom = () => {
    const sb = scrollRef.current;
    if (!sb) return;
    sb.scrollTop = sb.scrollHeight;
    setAway(false);
  };

  useKeyboard((key) => {
    if (keys === "off") return;
    const sb = scrollRef.current;
    if (!sb) return;
    const page = Math.max(1, sb.viewport.height - 2);
    if (key.name === "pageup") sb.scrollBy(-page);
    if (key.name === "pagedown") sb.scrollBy(page);
    if (key.name === "end" || (keys === "normal" && key.sequence === "G")) toBottom();
    if (key.name === "o" && (key.ctrl || keys === "normal")) {
      // ctrl+o is the master switch: flipping it drops the per-block overrides,
      // so every block really does open (or close) together
      setOpenAll((v) => !v);
      setOpen(new Set());
    }
    if (keys === "normal" && !key.ctrl && !key.meta) {
      if (key.name === "u") sb.scrollBy(-Math.ceil(page / 2));
      if (key.name === "d") sb.scrollBy(Math.ceil(page / 2));
    }
    setAway(!atBottom(sb));
  });

  const visible = entries.slice(-SCROLLBACK);

  if (visible.length === 0) {
    return (
      <box flexGrow={1} flexDirection="column" paddingLeft={1} gap={1}>
        {/* opencode-style home: block wordmark, tagline, quiet shortcut table */}
        <box flexDirection="row">
          <ascii-font font="tiny" text="kloud" color={theme.muted} />
          <ascii-font font="tiny" text="lite" color={theme.accent} />
        </box>
        <text fg={theme.muted}>Orchestrate agents across your kloudlite workspaces.</text>
        <box flexDirection="column" marginTop={1}>
          {(
            [
              ["/", "commands"],
              ["^p", "jump to a workspace"],
              ["^1-9", "jump to workspace · ^0 main"],
              ["^j ^k", "cycle workspaces"],
              ["^f", "files, then processes & their logs"],
              ["^b", "back to the main context"],
              ["^o", "expand or collapse long output"],
            ] as const
          ).map(([key, label]) => (
            <box key={key} flexDirection="row" height={1} flexShrink={0} overflow="hidden">
              <box width={7} flexShrink={0}>
                <text fg={theme.fg}>{key}</text>
              </box>
              <text fg={theme.muted}>{label}</text>
            </box>
          ))}
        </box>
      </box>
    );
  }

  return (
    <box flexGrow={1} flexBasis={0} flexShrink={1} flexDirection="column">
    <scrollbox
      ref={scrollRef}
      // basis 0 + shrink: yoga's flex-basis auto would size the scrollbox to its
      // content and shove the prompt/hint bar off screen
      flexGrow={1}
      flexBasis={0}
      flexShrink={1}
      stickyScroll
      stickyStart="bottom"
      scrollbarOptions={{ visible: false }}
    >
      {/* opencode: one blank row above the first message */}
      <box height={1} />
      {visible.map((entry, i) => {
        const key = "id" in entry && entry.id ? entry.id : `e${i}`;
        // the whole cell is the expander, so a long block does not have to be
        // scrolled past to reach its "… +N lines" row
        const canCollapse = collapsible(entry, width);
        const toggle = () =>
          setOpen((prev) => {
            const next = new Set(prev);
            if (!next.delete(key)) next.add(key);
            return next;
          });
        return (
        <box
          key={key}
          flexDirection="column"
          // opencode sibling margins: consecutive inline tool rows stack
          // tight; everything else separates by one blank line
          marginTop={
            i === 0 ? 0 : isInlineTool(entry) && isInlineTool(visible[i - 1]!) ? 0 : 1
          }
          // mouseup, not mousedown: a mousedown on the body starts a text
          // selection, so toggling there would make a block impossible to
          // select from. A click is an up within a cell of where it went down.
          onMouseDown={canCollapse ? (e: { x: number; y: number }) => {
            downAt.current = { x: e.x, y: e.y };
          } : undefined}
          onMouseUp={canCollapse ? (e: { x: number; y: number }) => {
            const from = downAt.current;
            downAt.current = null;
            if (from && Math.abs(from.x - e.x) < 2 && from.y === e.y) toggle();
          } : undefined}
          onMouseOver={canCollapse ? () => setHover(key) : undefined}
          onMouseOut={canCollapse ? () => setHover((h) => (h === key ? null : h)) : undefined}
        >
          <Row
            width={width}
            entry={entry}
            hover={canCollapse && hover === key}
            open={openAll !== open.has(key)}
            onOpen={toggle}
          />
        </box>
        );
      })}
    </scrollbox>
      {away && (
        <box height={1} justifyContent="center" onMouseDown={toBottom}>
          <text selectable={false} fg={theme.accent}>
            ↓ jump to bottom <span fg={theme.muted}>end</span>
          </text>
        </box>
      )}
    </box>
  );
}
