import { expect, test } from 'claude-code/testing'
import { activity, environments, GRACE_MS, mark, workspaces } from './panel'

test('lists parse and errors surface', () => {
  const ws = workspaces(JSON.stringify([{ id: 'w1', name: 'chat', state: 'ready' }]))
  expect(ws).toEqual([{ id: 'w1', name: 'chat', state: 'ready' }])

  const [env] = environments(
    JSON.stringify([
      {
        id: 'e1',
        name: 'dev',
        state: 'running',
        degraded: { ready: true, reason: 'NodeDead', message: '' },
        services: [
          { name: 'web', ports: [3000, 3001] },
          { name: 'db', ports: [] },
        ],
        intercepts: [{ service: 'web', workspace: 'w1', ports: [{ service: 3000, workspace: 5173 }] }],
        service_status: [
          { name: 'web', ready: true, interceptedBy: 'w1', proxy: 'ready' },
          { name: 'db', ready: false, failing: true, message: 'ImagePullBackOff' },
        ],
      },
    ]),
    ws,
  )
  const [web, db] = env.services
  expect(web).toEqual({ name: 'web', ports: ':3000 :3001', state: 'ready', mark: mark('ready'), interceptedBy: 'chat' })
  expect(db.state).toBe('failing')
  expect(db.ports).toBe('')
  expect(db.interceptedBy).toBeUndefined()

  expect(() => workspaces('503: api unavailable')).toThrow('503: api unavailable')
  expect(mark('running').color).toBe('success')
  expect(mark('stopped').glyph).toBe('○')
  expect(mark('error').color).toBe('error')
  expect(mark('creating').color).toBe('warning')
})

test('a ready workspace is working while a kl call names it, then idle', () => {
  const w = { id: 'w1', name: 'chat', state: 'ready' }
  expect(activity(w, 1, 0, 1e9).state).toBe('working')
  expect(activity(w, 0, 1e9 - GRACE_MS + 1, 1e9).state).toBe('working')
  expect(activity(w, 0, 1e9 - GRACE_MS, 1e9).state).toBe('idle')
  expect(activity({ ...w, state: 'stopped' }, 1, 0, 1e9).state).toBe('stopped')
  expect(mark('working').color).toBe('claude')
  expect(mark('idle').color).toBe('success')
})
