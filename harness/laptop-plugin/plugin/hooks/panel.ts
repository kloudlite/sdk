// What the Kloudlite pane shows, as plain data: parsed from the `kl` server's `workspace_list` and
// `env_list` text (the api's JSON, pretty-printed by harness/packages/tools/src/platform.ts `show`;
// the environment shape is `model::Environment` from `api/environments.rs` `env_doc`). No engine
// calls here, so a test reads it without a session.
//
// No agents section: Claude Code already lists agents, and there are only the two kinds
// (one kl-workspace per workspace, one kl-subagent per part).

export type Mark = { glyph: string; color: string }
/** A condition that changes what an environment can do: restoring, not placed, degraded. */
export type Note = { mark: Mark; text: string }
export type Intercept = { mark: Mark; who: string; ports: string; state: string; notes: string[] }
export type Service = { name: string; ports: string; state: string; mark: Mark; message?: string; intercept?: Intercept }
export type Row = { id: string; name: string; state: string }
export type Env = Row & { notes: Note[]; services: Service[] }

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

/** `ws` names the intercepting workspaces, which the api gives by id. */
export function environments(text: string, ws: Row[] = []): Env[] {
  const wsName = (id: string) => ws.find(w => w.id === id)?.name ?? id
  return list(text).map(d => {
    const notes: Note[] = []
    if (d?.restoring) notes.push({ mark: YELLOW, text: `Restoring: ${d.restoring}` })
    if (d?.placed) notes.push({ mark: RED, text: `Not placed: ${cond(d.placed)}` })
    if (d?.degraded?.ready) notes.push({ mark: RED, text: `Degraded: ${cond(d.degraded)}` })
    if (d?.decommissioning?.ready) notes.push({ mark: YELLOW, text: `Node leaving: ${cond(d.decommissioning)}` })

    const status: any[] = Array.isArray(d?.service_status) ? d.service_status : []
    const wishes: any[] = Array.isArray(d?.intercepts) ? d.intercepts : []
    const services = (Array.isArray(d?.services) ? d.services : []).map((s: any): Service => {
      const name = String(s?.name ?? '?')
      const st = status.find(x => x?.name === name)
      const ports: number[] = Array.isArray(s?.ports) ? s.ports : []
      const svc: Service = {
        name,
        ports: ports.length ? ports.map(p => `:${p}`).join(' ') : '—',
        state: !st ? 'pending' : st.failing ? 'failing' : st.ready ? 'ready' : 'starting',
        mark: !st ? YELLOW : st.failing ? RED : st.ready ? GREEN : YELLOW,
        ...(st?.message && { message: String(st.message) }),
      }
      const wish = wishes.find(w => w?.service === name)
      const by = st?.interceptedBy
      if (wish || by) {
        const proxy = st?.proxy
        const notes: string[] = []
        if (by && wish && wish.workspace !== by) notes.push(`Requested: ${wsName(wish.workspace)}`)
        if (!wish && by) notes.push('Releasing: intercept removed')
        if (st?.unreachableSince) notes.push(`Workspace unreachable since ${clock(st.unreachableSince)}`)
        svc.intercept = {
          mark: !by ? YELLOW : proxy === 'failed' || st?.unreachableSince ? RED : proxy === 'starting' ? YELLOW : GREEN,
          who: wsName(String(by ?? wish.workspace)),
          ports: (Array.isArray(wish?.ports) ? wish.ports : []).map((p: any) => `:${p.service} → :${p.workspace}`).join('  '),
          state: !by ? 'pending' : proxy ? `proxy ${proxy}` : 'active',
          notes,
        }
      }
      return svc
    })
    return { ...row(d), notes, services }
  })
}

function cond(c: any): string {
  return [c?.reason, c?.message].filter(Boolean).join(' — ')
}

/** Unix seconds as local HH:MM. */
function clock(secs: number): string {
  const t = new Date(secs * 1000)
  return `${String(t.getHours()).padStart(2, '0')}:${String(t.getMinutes()).padStart(2, '0')}`
}
