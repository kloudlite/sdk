import { expect, test } from 'claude-code/testing'
import { activity, envCurrent, environments, GRACE_MS, mark, snapName, tree, workspaces } from './panel'

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

test('the snapshot an environment is based on follows restores', () => {
  const h = [
    { id: 'c', parent: 'a', message: 'after restore', createdAt: '2026-10-11T12:00:00Z' },
    { id: 'b', parent: 'a', message: 'left behind', createdAt: '2026-10-11T10:00:00Z' },
    { id: 'a', parent: null, message: 'seed data\nmore', createdAt: '2026-10-11T09:00:00Z' },
  ]
  expect(envCurrent(h, null, null)?.id).toBe('c')
  expect(envCurrent(h, 'a', '2026-10-11T11:00:00Z')?.id).toBe('c')
  expect(envCurrent(h, 'a', '2026-10-11T13:00:00Z')?.id).toBe('a')
  expect(envCurrent(h, 'zz', null)).toBeUndefined()
  expect(snapName(h[2])).toBe('seed data')
  expect(snapName({ id: 'x', createdAt: null })).toBe('x')
})

test('a clone sits under the workspace it was cloned from', () => {
  const ws = workspaces(
    JSON.stringify([
      { id: 'w1', name: 'sync', state: 'ready' },
      { id: 'w2', name: 'web', state: 'ready' },
      { id: 'c1', name: 'sync-review-1', state: 'ready', clone_of: 'w1' },
      { id: 'c2', name: 'sync-review-1-a', state: 'ready', clone_of: 'c1' },
      { id: 'c3', name: 'orphan', state: 'ready', clone_of: 'gone' },
    ]),
  )
  expect(ws[2].cloneOf).toBe('w1')
  expect(tree(ws).map(t => `${t.depth}${t.last ? 'L' : '-'}${t.row.name}`)).toEqual([
    '0-sync',
    '1Lsync-review-1',
    '2Lsync-review-1-a',
    '0-web',
    '0Lorphan',
  ])
})
