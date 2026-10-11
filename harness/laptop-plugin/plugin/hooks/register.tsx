// The Kloudlite side pane: workspaces (working / idle / their api state), then environments with their
// services (ports, state, the intercepting workspace) at the bottom, beside the transcript while kl
// mode is on. kl mode is the flag file
// `~/.claude/kl-mode/<session id>` that `kl-connect mode prompt` writes on /kloudlite:on, so the
// pane follows the same switch as the tool gate: it opens with it and closes with it.
//
// Every second (owner ruling 2026-10-11: "every sec") one tick reads the flag and, while on, asks
// the plugin's own `kl` MCP server for `workspace_list` and `env_list`. A tick still waiting on the
// last answer skips, so a slow api never stacks calls. A failed call keeps the last lists and shows
// the error under them. A pane the person closes stays closed until kl mode goes off and on.
//
// Working / idle: the api has no such field, so it comes from this session. `tool.call` fires for
// every kl tool call, the subagents' included; one that names a `workspace` marks it working until
// the call ends plus `GRACE_MS`.
import type { Register } from 'claude-code'
import { activity, environments, mark, workspaces, type Env, type Row, type Service } from './panel'
import { register as tools } from './tools'

const PANE = 'kloudlite'

const state = {
  ws: [] as Row[],
  envs: [] as Env[],
  error: '',
  at: '', // HH:MM:SS of the last good answer
  busy: false,
  open: false,
  calls: new Map<string, { inFlight: number; lastEnd: number }>(), // keyed by the `workspace` argument
}

export const register: Register = on => {
  tools(on) // hooks.json takes one module per plugin; the kl tool rows live in tools.tsx

  on('tool.call', { tool: /^mcp__plugin_kloudlite_kl__/ }, async ($, e: any, next) => {
    const key = typeof e.workspace === 'string' ? e.workspace : ''
    if (!key) return next(e)
    const c = state.calls.get(key) ?? { inFlight: 0, lastEnd: 0 }
    state.calls.set(key, c)
    c.inFlight++
    try {
      return await next(e)
    } finally {
      c.inFlight--
      c.lastEnd = Date.now()
    }
  })

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
        const kl = await $.mcp.connect('kl')
        if (!kl.isConnected) throw new Error(`kl server: ${kl.message ?? kl.reason ?? 'not connected'}`)
        const [ws, envs] = await Promise.all([
          $.mcp.call(kl.server, 'workspace_list', args),
          $.mcp.call(kl.server, 'env_list', args),
        ])
        state.ws = workspaces(text(ws))
        state.envs = environments(text(envs), state.ws)
        state.error = ''
        state.at = new Date().toTimeString().slice(0, 8)
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
    const now = Date.now()
    const live = (w: Row) => {
      const c = state.calls.get(w.id) ?? state.calls.get(w.name)
      return activity(w, c?.inFlight ?? 0, c?.lastEnd ?? 0, now)
    }
    // Colour sits on the dot; the state word stays quiet unless it needs a look.
    const loud = (s: string) => (s === 'working' ? 'claude' : s === 'error' || s === 'failed' || s === 'failing' ? 'error' : undefined)
    const word = (s: string) => {
      const c = loud(s)
      return c ? <Text color={c}>{s}</Text> : <Text dimColor>{s}</Text>
    }
    const row = (r: Row, bold: boolean) => {
      const m = mark(r.state)
      return (
        <Box gap={1}>
          <Text color={m.color}>{m.glyph}</Text>
          <Box flexGrow={1}>
            <Text bold={bold} wrap="truncate-end">{r.name}</Text>
          </Box>
          {word(r.state)}
        </Box>
      )
    }
    // services as a tree under their environment, names in one column so the ports line up
    const env = (x: Env) => {
      const w = Math.max(...x.services.map(s => s.name.length), 0) + 2
      return (
        <Box flexDirection="column">
          {row(x, true)}
          {x.services.map((s: Service, i) => (
            <Box gap={1}>
              <Text dimColor>{i === x.services.length - 1 ? '└' : '├'}</Text>
              <Text color={s.mark.color}>{s.mark.glyph}</Text>
              <Box width={w}>
                <Text wrap="truncate-end">{s.name}</Text>
              </Box>
              <Box flexGrow={1}>
                <Text dimColor>{s.ports}</Text>
              </Box>
              {s.interceptedBy ? <Text color="suggestion" wrap="truncate-start">{`→ ${s.interceptedBy}`}</Text> : word(s.state)}
            </Box>
          ))}
        </Box>
      )
    }
    const section = (title: string, count: number, body: unknown) => (
      <Box flexDirection="column" gap={1}>
        <Box gap={1}>
          <Text bold dimColor>{title}</Text>
          <Text dimColor>{`· ${count}`}</Text>
        </Box>
        {body}
      </Box>
    )
    const health = state.error ? { color: 'error', label: 'error' } : state.at ? { color: 'success', label: state.at } : { color: 'inactive', label: 'connecting' }
    return (
      <Box flexDirection="column" flexGrow={1} paddingX={2} paddingY={1} gap={1}>
        <Box justifyContent="space-between">
          <Text bold>Kloudlite</Text>
          <Box gap={1}>
            <Text color={health.color}>●</Text>
            <Text dimColor>{health.label}</Text>
          </Box>
        </Box>
        {state.error ? <Text color="error" wrap="wrap">{state.error}</Text> : null}
        {section(
          'WORKSPACES',
          state.ws.length,
          state.ws.length ? (
            <Box flexDirection="column">{state.ws.map(w => row(live(w), false))}</Box>
          ) : (
            <Text dimColor>No workspaces</Text>
          ),
        )}
        <Box flexGrow={1} />
        {section(
          'ENVIRONMENTS',
          state.envs.length,
          state.envs.length ? (
            <Box flexDirection="column" gap={1}>{state.envs.map(env)}</Box>
          ) : (
            <Text dimColor>No environments</Text>
          ),
        )}
      </Box>
    )
  })
}
