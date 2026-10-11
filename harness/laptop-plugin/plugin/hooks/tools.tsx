// Draws each kl tool call the way Claude Code draws a local one: `● Bash(cargo test)` and the result
// under `⎿`, not the MCP default with its argument list and raw JSON reply. Every other tool passes
// through untouched. The text comes from `toolview.ts`. `register.tsx` calls this (one module per
// plugin).
import type { Register } from 'claude-code'
import { body, head, PREFIX } from './toolview'

const kl = (e: any) => typeof e?.props?.tool === 'string' && e.props.tool.startsWith(PREFIX)

export const register: Register = on => {
  on('ui.render', { component: 'ToolUse' }, async ($, e: any, next) => {
    if (!kl(e)) return next(e)
    const { Box, Text } = $.ui.resolve(e)
    const h = head(e.props.tool, e.props.input)
    const dot = e.props.isRunning ? 'inactive' : e.props.isErrored ? 'error' : 'success'
    return (
      <Box marginTop={1} gap={1}>
        <Text color={dot}>●</Text>
        <Box flexGrow={1}>
          <Text wrap="truncate-end">
            <Text bold>{h.name}</Text>
            {h.arg ? `(${h.arg})` : ''}
          </Text>
        </Box>
        {h.workspace ? <Text dimColor>{h.workspace}</Text> : null}
      </Box>
    )
  })

  on('ui.render', { component: 'ToolResult' }, async ($, e: any, next) => {
    if (!kl(e)) return next(e)
    const { Box, Text } = $.ui.resolve(e)
    const b = body(e.props.tool, e.props.output)
    const color = e.props.isErrored ? 'error' : undefined
    return (
      <Box gap={1} paddingLeft={2}>
        <Text dimColor>⎿</Text>
        <Box flexDirection="column" flexGrow={1}>
          {b.lines.map((l, i) => (
            <Text key={i} color={color}>{l || ' '}</Text>
          ))}
          {b.more ? <Text dimColor>{`… +${b.more} lines`}</Text> : null}
          {b.error ? <Text color="error">{b.error}</Text> : null}
        </Box>
      </Box>
    )
  })
}
