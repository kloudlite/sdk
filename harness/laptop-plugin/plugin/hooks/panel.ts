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
export type Env = Row & { services: Service[]; volume: boolean }

// theme keys, so the pane follows the person's Claude Code theme
const GREEN = { glyph: '●', color: 'success' }
const GRAY = { glyph: '○', color: 'inactive' }
const RED = { glyph: '✕', color: 'error' }
const YELLOW = { glyph: '◐', color: 'warning' } // creating, pending, deleting, starting
const BLUE = { glyph: '◉', color: 'claude' } // a kl tool is running against it

/** The dot and color one state draws with. */
export function mark(state: string): Mark {
  switch (state) {
    case 'working':
      return BLUE
    case 'ready':
    case 'idle':
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

// ponytail: an agent's tool calls arrive with thinking gaps between them, so "working" holds this long
// after the last call ends; a real activity signal from the pod replaces it if one ever lands.
export const GRACE_MS = 15_000

/** What a workspace row says: a ready workspace is `working` while a kl tool call names it (or did
 * within `GRACE_MS`), `idle` otherwise; any other state is shown as the api gives it. */
export function activity(r: Row, inFlight: number, lastEnd: number, now: number): Row {
  if (r.state !== 'ready') return r
  return { ...r, state: inFlight > 0 || now - lastEnd < GRACE_MS ? 'working' : 'idle' }
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
    return { ...row(d), services, volume: Boolean(d?.volume) }
  })
}

/** A row of `volume_history` (`/v1/volumes/{name}/history`), newest first. */
export type Snap = { id: string; parent?: string | null; message?: string; createdAt: string | null }

/** Where an environment sits: the web's rule (`web/apps/web/src/lib/env-current.ts`, also
 * `harness/packages/backend/src/space.ts`), kept equal by hand. Never restored: the newest record.
 * Restored: the newest record pushed AFTER the restore that descends from the restored one, else the
 * restored record. A `restoredTo` naming no record here (a foreign graft) is no record. */
export function envCurrent(history: Snap[], restoredTo: string | null, restoredAt: string | null): Snap | undefined {
  if (restoredTo === null) return history[0]
  const byId = new Map(history.map(h => [h.id, h]))
  const restored = byId.get(restoredTo)
  if (!restored) return undefined
  const since = restoredAt ? Date.parse(restoredAt) : 0
  const time = (h: Snap) => (h.createdAt ? Date.parse(h.createdAt) : NaN)
  const descends = (n: Snap): boolean => {
    for (let p: Snap | undefined = n; p; p = p.parent ? byId.get(p.parent) : undefined) {
      if (p.id === restored.id) return true
      if (!(time(p) > since)) return false
    }
    return false
  }
  return history.find(h => time(h) > since && descends(h)) ?? restored
}

/** The snapshot line's name: the push message, else the record id. */
export const snapName = (s: Snap) => (s.message?.trim() || s.id).split('\n')[0]
