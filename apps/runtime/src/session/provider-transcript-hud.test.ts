import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { mkdtemp, rm, writeFile, appendFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { ProviderTranscriptHudReader } from './provider-transcript-hud'

let root: string
let path: string
let now: number
let reader: ProviderTranscriptHudReader
const stamp = '2026-10-15T03:00:00.000Z'
const use = (id = 'a', name = 'Agent', input = { description: 'review' } as Record<string, string>) =>
  ({ type: 'assistant', timestamp: stamp, message: { content: [{ type: 'tool_use', id, name, input }] } })
const result = (is_error = false, toolUseResult = {}) => ({ type: 'user', timestamp: stamp, toolUseResult,
  message: { content: [{ type: 'tool_result', tool_use_id: 'a', is_error, content: 'failure\nsecond line' }] } })
const notification = (status = 'completed', id = 'a') => `<task-notification><task-id>worker</task-id><tool-use-id>${id}</tool-use-id><status>${status}</status><summary>failed task</summary></task-notification>`
const write = async (...rows: unknown[]) => { await writeFile(path, rows.map(row => JSON.stringify(row)).join('\n') + '\n'); return reader.read(path) }
beforeEach(async () => { root = await mkdtemp(join(tmpdir(), 'matou-agent-history-')); path = join(root, 'history.jsonl'); now = Date.parse(stamp); reader = new ProviderTranscriptHudReader(() => now) })
afterEach(async () => { await rm(root, { recursive: true, force: true }) })
describe('live transcript agents', () => {
  it('removes synchronous completion', async () => { expect((await write(use(), result()))?.agents).toEqual([]) })
  it('expires synchronous errors after five minutes using snapshot time', async () => {
    expect((await write(use(), result(true)))?.agents[0]).toMatchObject({ state: 'error', detail: 'failure' })
    now += 299_999
    expect((await reader.read(path))?.agents).toHaveLength(1)
    now++
    expect((await reader.read(path))?.agents).toEqual([])
  })
  it('keeps async launches running', async () => {
    expect((await write(use(), result(false, { isAsync: true, status: 'async_launched', agentId: 'worker' })))?.agents[0]).toMatchObject({ id: 'a', state: 'running', startedAt: now })
  })
  it.each(['string', 'blocks'])('handles task notifications as %s including multiple notifications', async format => {
    const text = notification() + notification('completed', 'b')
    const content = format === 'string' ? text : [{ type: 'text', text }]
    expect((await write(use(), use('b'), { type: 'user', message: { content } }))?.agents).toEqual([])
  })
  it('ignores queue operations and non Agent notifications', async () => {
    const snapshot = await write(use(), { type: 'queue-operation', message: { content: notification() }, content: notification() },
      { type: 'user', message: { content: notification('completed', 'bash-task') } })
    expect(snapshot?.agents).toHaveLength(1)
  })
  it.each(['killed', 'stopped', 'cancelled'])('removes %s notifications', async status => {
    expect((await write(use(), { type: 'user', message: { content: notification(status) } }))?.agents).toEqual([])
  })
  it.each(['failed', 'error'])('retains %s notification errors for five minutes', async status => {
    expect((await write(use(), { type: 'user', timestamp: stamp, message: { content: notification(status) } }))?.agents[0]?.state).toBe('error')
    now += 300_000
    expect((await reader.read(path))?.agents).toEqual([])
  })
  it('wakes a completed agent with SendMessage and accepts its next completion', async () => {
    await write(use(), result(false, { isAsync: true, agentId: 'worker' }), { type: 'user', message: { content: notification() } })
    now += 10_000
    await appendFile(path, JSON.stringify({ ...use('send', 'SendMessage', { to: 'worker' }), timestamp: new Date(now).toISOString() }) + '\n')
    expect((await reader.read(path))?.agents[0]).toMatchObject({ id: 'a', label: 'review', state: 'running', startedAt: now })
    await appendFile(path, JSON.stringify({ type: 'user', message: { content: notification() } }) + '\n')
    expect((await reader.read(path))?.agents).toEqual([])
  })
  it('keeps concurrent agents independent', async () => {
    const snapshot = await write(use(), use('b'), result())
    expect(snapshot?.agents.map(agent => agent.id)).toEqual(['b'])
    expect(snapshot?.subagentCount).toBe(1)
    expect(snapshot?.subagents).toEqual(['review'])
  })
})

it.each(['unknown', 'paused', ''])('removes task notifications with non-error status %s', async status => {
  expect((await write(use(), { type: 'user', message: { content: notification(status) } }))?.agents).toEqual([])
})
it('removes task notifications with missing status', async () => {
  expect((await write(use(), { type: 'user', message: { content: notification().replace('<status>completed</status>', '') } }))?.agents).toEqual([])
})
