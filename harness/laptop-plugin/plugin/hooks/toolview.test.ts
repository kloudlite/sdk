import { expect, test } from 'claude-code/testing'
import { body, head, MAX_LINES, PREFIX } from './toolview'

const reply = (r: unknown) => [{ type: 'text', text: JSON.stringify(r) }]

test('kl calls get the local tool header', () => {
  expect(head(PREFIX + 'exec', { workspace: 'ws-1', cmd: 'cargo test' })).toEqual({ name: 'Bash', arg: 'cargo test', workspace: 'ws-1' })
  expect(head(PREFIX + 'exec', { workspace: 'ws-1', cmd: 'npm run dev', detach: true }).arg).toBe('npm run dev &')
  expect(head(PREFIX + 'read', { workspace: 'ws-1', path: 'src/a.rs' }).name).toBe('Read')
  expect(head(PREFIX + 'env_get', { env: 'env-1' })).toEqual({ name: 'env_get', arg: 'env: env-1', workspace: '' })
})

test('exec shows its streams, and the exit code only when it failed', () => {
  expect(body(PREFIX + 'exec', reply({ exit_code: 0, stdout: '\nHTTP 000\n', stderr: '' }))).toEqual({ lines: ['', 'HTTP 000'], more: 0, error: '' })
  expect(body(PREFIX + 'exec', reply({ exit_code: 2, stdout: '', stderr: 'boom' })).error).toBe('exit 2')
  expect(body(PREFIX + 'exec', reply({ exit_code: 0, stdout: '', stderr: '' })).lines).toEqual(['(No output)'])
  const long = body(PREFIX + 'exec', reply({ exit_code: 0, stdout: Array.from({ length: 25 }, (_, i) => `l${i}`).join('\n'), stderr: '' }))
  expect(long.lines.length).toBe(MAX_LINES)
  expect(long.more).toBe(15)
})

test('file tools summarise, errors and plain text pass through', () => {
  expect(body(PREFIX + 'read', reply({ path: 'a', content: 'x\ny', total_lines: 2 })).lines).toEqual(['Read 2 lines'])
  expect(body(PREFIX + 'glob', reply({ paths: ['a'] })).lines).toEqual(['Found 1 file'])
  expect(body(PREFIX + 'edit', reply({ path: 'a', applied: 3 })).lines).toEqual(['Applied 3 changes'])
  expect(body(PREFIX + 'read', reply({ error: 'no such file' })).error).toBe('no such file')
  expect(body(PREFIX + 'exec', 'workspace not ready').lines).toEqual(['workspace not ready'])
  expect(body(PREFIX + 'quota', reply({ cpu: 4, items: [1, 2] })).lines).toEqual(['cpu: 4', 'items: 2 items'])
})
