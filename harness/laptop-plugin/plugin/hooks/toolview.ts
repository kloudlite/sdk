// What a kl tool call looks like in the transcript: the same header and result lines the local
// Bash / Read / Write / Update / Search tools draw, instead of the MCP default
// (`plugin:kloudlite:kl - exec (MCP)(workspace: …)` plus the raw JSON reply). Pure, so
// `toolview.test.ts` holds it; `tools.tsx` only draws what these return.
//
// Reply shapes are the ide crate's (`crates/ide/src/tools/{exec,files,patch}.rs`); a tool whose
// shape is not listed here falls back to its scalar fields, one per line.

export const PREFIX = 'mcp__plugin_kloudlite_kl__'
export const MAX_LINES = 10

export type Head = { name: string; arg: string; workspace: string }
export type Body = { lines: string[]; more: number; error: string }

const LOCAL: Record<string, [string, string]> = {
  exec: ['Bash', 'cmd'],
  read: ['Read', 'path'],
  write: ['Write', 'path'],
  edit: ['Update', 'path'],
  patch: ['Update', 'cwd'],
  glob: ['Search', 'pattern'],
  grep: ['Search', 'pattern'],
}

const short = (v: unknown) => {
  const s = typeof v === 'string' ? v : JSON.stringify(v)
  return s.length > 60 ? s.slice(0, 59) + '…' : s
}

export function head(tool: string, input: any): Head {
  const name = tool.slice(PREFIX.length)
  const i = input && typeof input === 'object' ? input : {}
  const workspace = typeof i.workspace === 'string' ? i.workspace : ''
  const local = LOCAL[name]
  if (local) {
    let arg = typeof i[local[1]] === 'string' ? i[local[1]] : ''
    if (name === 'read' && Array.isArray(i.files)) arg = `${i.files.length} files`
    if (name === 'exec' && i.detach) arg += ' &'
    return { name: local[0], arg, workspace }
  }
  const args = Object.entries(i)
    .filter(([k]) => k !== 'workspace')
    .map(([k, v]) => `${k}: ${short(v)}`)
    .join(', ')
  return { name, arg: args, workspace }
}

// MCP results arrive as a string, a list of content blocks, or `{ content: [...] }`.
export function text(output: unknown): string {
  if (typeof output === 'string') return output
  if (Array.isArray(output)) return output.map(b => (typeof b?.text === 'string' ? b.text : '')).join('\n')
  if (output && typeof output === 'object' && Array.isArray((output as any).content)) return text((output as any).content)
  return output == null ? '' : JSON.stringify(output)
}

const clip = (lines: string[], error = ''): Body => ({
  lines: lines.slice(0, MAX_LINES),
  more: Math.max(0, lines.length - MAX_LINES),
  error,
})

const split = (s: unknown) => (typeof s === 'string' && s.trim() ? s.replace(/\s+$/, '').split('\n') : [])
const plural = (n: number, one: string) => `${n} ${one}${n === 1 ? '' : 's'}`

export function body(tool: string, output: unknown): Body {
  const raw = text(output)
  let r: any
  try {
    r = JSON.parse(raw)
  } catch {
    return clip(split(raw)) // an MCP error or plain text; the caller colours it by `isErrored`
  }
  if (!r || typeof r !== 'object' || Array.isArray(r)) return clip(split(raw))
  if (typeof r.error === 'string') return clip([], r.error)
  switch (tool.slice(PREFIX.length)) {
    case 'exec':
      if (typeof r.id === 'string' || typeof r.id === 'number') return clip([`Running in the background (id ${r.id})`])
    // falls through: a job's reply has the same streams as process_output
    case 'process_output': {
      const lines = [...split(r.stdout), ...split(r.stderr)]
      const bad = r.timed_out ? 'timed out' : typeof r.exit_code === 'number' && r.exit_code !== 0 ? `exit ${r.exit_code}` : ''
      return clip(lines.length || bad ? lines : ['(No output)'], bad)
    }
    case 'read':
      if (Array.isArray(r.files)) return clip([`Read ${plural(r.files.length, 'file')}`])
      if (r.binary) return clip([`Binary file, ${r.size} bytes`])
      return clip([`Read ${plural(typeof r.content === 'string' ? r.content.split('\n').length : 0, 'line')}`])
    case 'write':
      return clip([`${r.created ? 'Created' : 'Wrote'} ${r.path} (${r.bytes} bytes)`])
    case 'edit':
    case 'patch':
      return clip([`Applied ${plural(Number(r.applied ?? (r.files?.length || 0)), 'change')}`])
    case 'glob':
      return clip([`Found ${plural(Array.isArray(r.paths) ? r.paths.length : 0, 'file')}`])
  }
  return clip(
    Object.entries(r).map(([k, v]) =>
      Array.isArray(v) ? `${k}: ${plural(v.length, 'item')}` : v && typeof v === 'object' ? `${k}: …` : `${k}: ${short(v)}`,
    ),
  )
}
