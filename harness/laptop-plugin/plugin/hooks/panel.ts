// What the Kloudlite pane shows, as plain data: parsed from the `kl` server's `workspace_list` and
// `env_list` text (the api's JSON, pretty-printed by harness/packages/tools/src/platform.ts `show`;
// the environment shape is `model::Environment` from `api/environments.rs` `env_doc`). No engine
// calls here, so a test reads it without a session.
//
// No agents section: Claude Code already lists agents, and there are only the two kinds
// (one kl-workspace per workspace, one kl-subagent per part).

export type Mark = { glyph: string; color: string }
export type Service = { name: string; ports: string; state: string; mark: Mark; interceptedBy?: string }
export type Row = { id: string; name: string; state: string }
export type Env = Row & { services: Service[] }

// theme keys, so the pane follows the person's Claude Code theme
const GREEN = { glyph: '●', color: 'success' }
const GRAY = { glyph: '○', color: 'inactive' }
const RED = { glyph: '✕', color: 'error' }
const YELLOW = { glyph: '◐', color: 'warning' } // creating, pending, deleting, starting

/** The dot and color one state draws with. */
export function mark(state: string): Mark {
  switch (state) {
    case 'ready':
    case 'running':
      return GREEN
    case 'stopped':
      return GRAY
    case 'error':
    case 'failed':
      return RED
    default:
      return YELLOW
  }
}

/** A tool result's text as a JSON array; a text that is not one (an error, "unavailable") throws it. */
function list(text: string): any[] {
  let data: unknown
  try {
    data = JSON.parse(text)
  } catch {
    throw new Error(text.split('\n')[0] || 'no answer')
  }
  if (!Array.isArray(data)) throw new Error(text.split('\n')[0] || 'no answer')
  return data
}

const row = (d: any): Row => ({
  id: String(d?.id ?? ''),
  name: String(d?.name ?? d?.id ?? '?'),
  state: String(d?.state ?? '?'),
})

export function workspaces(text: string): Row[] {
  return list(text).map(row)
}

/** `ws` names the intercepting workspace, which the api gives by id. */
export function environments(text: string, ws: Row[] = []): Env[] {
  const wsName = (id: string) => ws.find(w => w.id === id)?.name ?? id
  return list(text).map(d => {
    const status: any[] = Array.isArray(d?.service_status) ? d.service_status : []
    const services = (Array.isArray(d?.services) ? d.services : []).map((s: any): Service => {
      const name = String(s?.name ?? '?')
      const st = status.find(x => x?.name === name)
      const ports: number[] = Array.isArray(s?.ports) ? s.ports : []
      return {
        name,
        ports: ports.map(p => `:${p}`).join(' '),
        state: !st ? 'pending' : st.failing ? 'failing' : st.ready ? 'ready' : 'starting',
        mark: !st ? YELLOW : st.failing ? RED : st.ready ? GREEN : YELLOW,
        ...(st?.interceptedBy && { interceptedBy: wsName(String(st.interceptedBy)) }),
      }
    })
    return { ...row(d), services }
  })
}
