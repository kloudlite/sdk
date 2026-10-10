import { expect, test } from 'claude-code/testing'
import { environments, mark, workspaces } from './panel'

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
  expect(env.notes.map(n => n.text)).toEqual(['Degraded: NodeDead'])
  const [web, db] = env.services
  expect(web.ports).toBe(':3000 :3001')
  expect(web.intercept?.who).toBe('chat')
  expect(web.intercept?.ports).toBe(':3000 → :5173')
  expect(web.intercept?.state).toBe('proxy ready')
  expect(db.state).toBe('failing')
  expect(db.ports).toBe('—')
  expect(db.message).toBe('ImagePullBackOff')
  expect(db.intercept).toBeUndefined()

  // asked for, not yet in force
  const [pending] = environments(
    JSON.stringify([{ name: 'x', state: 'running', services: [{ name: 'api' }], intercepts: [{ service: 'api', workspace: 'w9' }] }]),
  )
  expect(pending.services[0].intercept?.state).toBe('pending')
  expect(pending.services[0].intercept?.who).toBe('w9')

  expect(() => workspaces('503: api unavailable')).toThrow('503: api unavailable')
  expect(mark('running').color).toBe('success')
  expect(mark('stopped').glyph).toBe('○')
  expect(mark('error').color).toBe('error')
  expect(mark('creating').color).toBe('warning')
})
