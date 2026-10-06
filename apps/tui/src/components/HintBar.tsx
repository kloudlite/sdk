import { TextAttributes } from "@opentui/core";
import { theme } from "../theme.ts";
import { Spinner } from "./Spinner.tsx";

/** Bottom bar: context path (+ spinner while busy) left; tokens, hints right. */
export function HintBar({
  busy,
  tokens,
  queued,
  active,
  inWorkspace,
  normal,
  vim = true,
  compact = false,
  onHint,
}: {
  busy: boolean;
  tokens: number;
  queued: number;
  /** Context path: "main" or "main › <workspace>" */
  active: string;
  inWorkspace: boolean;
  normal: boolean;
  /** vim keys on: esc leads to NORMAL mode. Off: ctrl+<letter> commands. */
  vim?: boolean;
  /** Narrow terminal: show only the essential hints so the row can't wrap. */
  compact?: boolean;
  /** Hints are clickable: the id matches the key they stand for. */
  onHint?: (id: "type" | "jump" | "files" | "commands" | "help" | "queue") => void;
}) {
  const click = (id: Parameters<NonNullable<typeof onHint>>[0]) =>
    onHint ? () => onHint(id) : undefined;
  return (
    <box flexDirection="row" flexShrink={0} justifyContent="space-between" overflow="hidden">
      <box flexDirection="row" gap={2} marginLeft={1}>
        <text fg={theme.muted}>{active}</text>
        {queued > 0 && (
          <box onMouseDown={click("queue")}>
            <text fg={theme.warning}>
              {queued} queued <span attributes={TextAttributes.DIM}>q edit</span>
            </text>
          </box>
        )}
        {busy && (
          <text fg={theme.fg}>
            <Spinner fg={theme.accent} />{" "}
            <span fg={theme.muted}>
              <b>esc</b> <span attributes={TextAttributes.DIM}>interrupt</span>
            </span>
          </text>
        )}
      </box>
      <box flexDirection="row" gap={2} flexShrink={1} overflow="hidden">
        <text fg={theme.muted}>{tokens.toLocaleString()} tok</text>
        {normal ? (
          <>
            <box onMouseDown={click("type")}><text fg={theme.fg}>i <span fg={theme.muted}>type</span></text></box>
            {!compact && <text fg={theme.fg}>j k <span fg={theme.muted}>workspaces</span></text>}
            {!compact && <box onMouseDown={click("jump")}><text fg={theme.fg}>p <span fg={theme.muted}>jump</span></text></box>}
            {inWorkspace && <box onMouseDown={click("files")}><text fg={theme.fg}>f <span fg={theme.muted}>view</span></text></box>}
            <box onMouseDown={click("commands")}><text fg={theme.fg}>/ <span fg={theme.muted}>commands</span></text></box>
            <box onMouseDown={click("help")}><text fg={theme.fg}>? <span fg={theme.muted}>help</span></text></box>
          </>
        ) : vim ? (
          <>
            <text fg={theme.fg}>enter <span fg={theme.muted}>send</span></text>
            <text fg={theme.fg}>esc <span fg={theme.muted}>normal mode</span></text>
          </>
        ) : (
          <>
            <text fg={theme.fg}>enter <span fg={theme.muted}>send</span></text>
            {!compact && <box onMouseDown={click("jump")}><text fg={theme.fg}>^p <span fg={theme.muted}>jump</span></text></box>}
            {inWorkspace && <box onMouseDown={click("files")}><text fg={theme.fg}>^f <span fg={theme.muted}>view</span></text></box>}
            <box onMouseDown={click("commands")}><text fg={theme.fg}>/ <span fg={theme.muted}>commands</span></text></box>
          </>
        )}
      </box>
    </box>
  );
}
