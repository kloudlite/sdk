import { TextAttributes } from "@opentui/core";
import { theme } from "../theme.ts";
import { clip } from "../ui/text.ts";
import type { QueuedMessage } from "../sessions.ts";


/**
 * Messages waiting to be delivered to the running agent, newest last. Shown
 * above the prompt while a turn streams; ↑ on an empty prompt selects one to edit, `d` drops
 * it. A steering message interrupts the turn, a follow-up waits for it. A message whose steer-or-queue
 * fork has not answered is a `deciding` row; once queued, the fork's reason shows dim under its row.
 */
export function Queue({
  messages,
  pending = [],
  reasons,
  selected,
  width,
  onSelect,
}: {
  messages: QueuedMessage[];
  /** texts still waiting for the routing fork */
  pending?: string[];
  /** fork reason per queued text (TUI memory only) */
  reasons?: Map<string, string>;
  /** index being targeted in NORMAL mode, or null when the list is just shown */
  selected: number | null;
  width: number;
  onSelect?: (index: number) => void;
}) {
  if (messages.length === 0 && pending.length === 0) return null;
  // ponytail: reasons for texts no longer queued are never pruned; the map dies with the TUI
  const total = messages.length + pending.length;

  return (
    <box flexDirection="column" flexShrink={0} marginBottom={1}>
      <box flexDirection="row" justifyContent="space-between" height={1} paddingLeft={1} paddingRight={1}>
        <text selectable={false}>
          <span fg={theme.warning} attributes={TextAttributes.BOLD}>
            QUEUED
          </span>
          <span fg={theme.border}>
            {"  "}
            {String(total)}
          </span>
        </text>
        {selected !== null && (
          <text selectable={false} fg={theme.muted}>
            enter edit · d drop · esc done
          </text>
        )}
      </box>
      {messages.map((m, i) => {
        const on = selected === i;
        const why = reasons?.get(m.text);
        return (
          <box key={`${i}-${m.text}`} flexDirection="column" flexShrink={0}>
          <box
            flexDirection="row"
            height={1}
            overflow="hidden"
            paddingLeft={1}
            paddingRight={1}
            backgroundColor={on ? theme.surfaceRaised : undefined}
            onMouseDown={onSelect ? () => onSelect(i) : undefined}
          >
            <text selectable={false}>
              <span fg={on ? theme.accent : theme.border}>{on ? "› " : "  "}</span>
              <span fg={theme.muted}>
                {m.kind === "steer" ? "steer " : "after "}
              </span>
              <span fg={on ? theme.fg : theme.muted}>{clip(m.text, width - 12)}</span>
            </text>
          </box>
          {why && (
            <box height={1} overflow="hidden" paddingLeft={5} paddingRight={1}>
              <text selectable={false} fg={theme.border}>{clip(why, width - 8)}</text>
            </box>
          )}
          </box>
        );
      })}
      {pending.map((text, i) => (
        <box key={`p${i}-${text}`} flexDirection="row" height={1} overflow="hidden" paddingLeft={1} paddingRight={1}>
          <text selectable={false}>
            <span fg={theme.border}>{"  "}</span>
            <span fg={theme.muted}>deciding </span>
            <span fg={theme.muted}>{clip(text, width - 14)}</span>
          </text>
        </box>
      ))}
    </box>
  );
}
