import { expect, test } from 'claude-code/testing'
import { mark, rows } from './panel'

test('lists parse and errors surface', () => {
  const ws = rows(JSON.stringify([{ id: 'ws-1', name: 'chat', state: 'ready' }]))
  expect(ws).toEqual([{ name: 'chat', state: 'ready' }])
  const env = rows(JSON.stringify([{ name: 'app', state: 'running', services: [{ name: 'web' }, { name: 'db' }] }]))
  expect(env[0]?.detail).toBe('web, db')
  expect(() => rows('503: api unavailable')).toThrow('503: api unavailable')
  expect(mark('ready').color).toBe('green')
  expect(mark('creating').glyph).toBe('◐')
})
