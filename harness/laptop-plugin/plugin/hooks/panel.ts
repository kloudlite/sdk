// What the Kloudlite pane shows, as plain data: parsed from the `kl` server's `workspace_list` and
// `env_list` text (the api's JSON, pretty-printed by harness/packages/tools/src/platform.ts `show`)
// and from `$.agent.list()`. No engine calls here, so a test reads it without a session.

export type Row = { name: string; state: string; detail?: string }
export type Agent = { name: string; status: string }

/** A tool result's text as rows; a text that is not a JSON array (an error, "unavailable") throws it. */
export function rows(text: string): Row[] {
  let data: unknown
  try {
    data = JSON.parse(text)
  } catch {
    throw new Error(text.split('\n')[0] || 'no answer')
  }
  if (!Array.isArray(data)) throw new Error(text.split('\n')[0] || 'no answer')
  return data.map((d: any) => ({
    name: String(d?.name ?? d?.id ?? '?'),
    state: String(d?.state ?? '?'),
    ...(Array.isArray(d?.services) && {
      detail: d.services.map((s: any) => String(s?.name ?? s)).join(', '),
    }),
  }))
}

/** The dot and color one state draws with. */
export function mark(state: string): { glyph: string; color: string } {
  switch (state) {
    case 'ready':
    case 'running':
    case 'idle':
      return { glyph: '●', color: 'green' }
    case 'stopped':
    case 'completed':
      return { glyph: '○', color: 'gray' }
    case 'error':
    case 'failed':
    case 'killed':
      return { glyph: '✕', color: 'red' }
    default:
      return { glyph: '◐', color: 'yellow' } // creating, pending, deleting, waiting
  }
}
