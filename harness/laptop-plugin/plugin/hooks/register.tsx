// The Kloudlite side pane: workspaces, environments (with their services) and this session's
// agents, each with its state, beside the transcript while kl mode is on. kl mode is the flag file
// `~/.claude/kl-mode/<session id>` that `kl-connect mode prompt` writes on /kloudlite:on, so the
// pane follows the same switch as the tool gate: it opens with it and closes with it.
//
// Every second (owner ruling 2026-10-11: "every sec") one tick reads the flag and, while on, asks
// the plugin's own `kl` MCP server for `workspace_list` and `env_list`. A tick still waiting on the
// last answer skips, so a slow api never stacks calls. A failed call keeps the last lists and shows
// the error under them. A pane the person closes stays closed until kl mode goes off and on.
import type { Register } from 'claude-code'
import { mark, rows, type Agent, type Row } from './panel'

const PANE = 'kloudlite'

const state = {
  ws: [] as Row[],
  envs: [] as Row[],
  agents: [] as Agent[],
  error: '',
  busy: false,
  open: false,
}

export const register: Register = on => {
  on('session.start', async ($, e, next) => {
    if (!e.isInteractive) return next(e)
    const flag = `${await $.env.get('HOME')}/.claude/kl-mode/${await $.session.id()}`
    const team = (await $.env.get('KL_TEAM')) || undefined
    const args = team ? { team } : {}
    const text = (r: { content?: readonly unknown[] }) =>
      (r.content ?? []).map((c: any) => (c?.type === 'text' ? c.text : '')).join('')

    $.clock.every(1000, async () => {
      if (state.busy) return
      state.busy = true
      try {
        const lit = await $.fs.exists(flag)
        if (lit !== state.open) {
          state.open = lit
          if (lit) await $.ui.open({ id: PANE, title: 'Kloudlite' })
          else await $.ui.close({ id: PANE })
        }
        if (!lit) return
        state.agents = (await $.agent.list()).map(a => ({ name: a.name ?? a.description, status: a.status }))
        const kl = await $.mcp.connect('kl')
        if (!kl.isConnected) throw new Error(`kl server: ${kl.message ?? kl.reason ?? 'not connected'}`)
        const [ws, envs] = await Promise.all([
          $.mcp.call(kl.server, 'workspace_list', args),
          $.mcp.call(kl.server, 'env_list', args),
        ])
        state.ws = rows(text(ws))
        state.envs = rows(text(envs))
        state.error = ''
      } catch (err: any) {
        state.error = String(err?.message ?? err)
      } finally {
        state.busy = false
        $.ui.invalidate('ui.render')
      }
    })
    return next(e)
  })

  on('ui.render', { component: 'Pane', requestId: PANE }, async ($, e) => {
    const { Box, Text } = $.ui.resolve(e)
    const line = (r: { name: string; state: string; detail?: string }) => {
      const m = mark(r.state)
      return (
        <Box flexDirection="column">
          <Box gap={1}>
            <Text color={m.color}>{m.glyph}</Text>
            <Text wrap="truncate-end">{r.name}</Text>
            <Text dimColor>{r.state}</Text>
          </Box>
          {r.detail ? <Text dimColor wrap="wrap">{`  ${r.detail}`}</Text> : null}
        </Box>
      )
    }
    const section = (title: string, items: { name: string; state: string; detail?: string }[]) => (
      <Box flexDirection="column" marginBottom={1}>
        <Text bold>{title}</Text>
        {items.length ? items.map(line) : <Text dimColor>none</Text>}
      </Box>
    )
    return (
      <Box flexDirection="column">
        {section('Workspaces', state.ws)}
        {section('Environments', state.envs)}
        {section('Agents', state.agents.map(a => ({ name: a.name, state: a.status })))}
        {state.error ? <Text color="red" wrap="wrap">{state.error}</Text> : null}
      </Box>
    )
  })
}
