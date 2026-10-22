import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { mkdtempSync, rmSync, writeFileSync, mkdirSync, unlinkSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { ExternalAgentStatusMonitor, ExternalAgentStatusReader, STALE_AGENT_DETAIL } from './external-agent-status'
let root: string
let now: number
let reader: ExternalAgentStatusReader
const write = (data = {}, name = 'task.json') => writeFileSync(join(root, name), JSON.stringify({ version: 1, label: 'task', state: 'running', startedAt: now, updatedAt: now, ...data }))
beforeEach(() => { root = mkdtempSync(join(tmpdir(), 'matou-external-')); now = Date.parse('2026-10-15T03:00:00Z'); reader = new ExternalAgentStatusReader(root, () => now) })
afterEach(() => { vi.useRealTimers(); rmSync(root, { recursive: true, force: true }) })
describe('external agent status', () => {
  it('reads timestamps and truncates first line labels and details', () => {
    write({ startedAt: new Date(now).toISOString(), label: 'x'.repeat(70) + '\nnext', detail: 'd'.repeat(130) + '\nnext' })
    expect(reader.read()).toEqual([{ id: 'external:task', source: 'external', state: 'running', startedAt: now, label: 'x'.repeat(60), detail: 'd'.repeat(120) }])
  })
  it.each(['completed', 'stopped'])('hides %s', state => { write({ state }); expect(reader.read()).toEqual([]) })
  it('expires errors after five minutes', () => { write({ state: 'error' }); now += 299_999; expect(reader.read()).toHaveLength(1); now++; expect(reader.read()).toEqual([]) })
  it.each(['running', 'waiting', 'warning'])('marks stale %s heartbeats and recovers on refresh', state => {
    write({ state }); now += 60_000; expect(reader.read()[0]?.state).toBe(state); now++
    expect(reader.read()[0]).toMatchObject({ state: 'error', detail: STALE_AGENT_DETAIL })
    write({ state }); expect(reader.read()[0]?.state).toBe(state)
  })
  it('retains previous valid content on bad JSON or invalid fields and removes deleted files', () => {
    write(); const before = reader.read(); writeFileSync(join(root, 'task.json'), '{'); expect(reader.read()).toEqual(before)
    write({ label: '' }); expect(reader.read()).toEqual(before)
    unlinkSync(join(root, 'task.json')); expect(reader.read()).toEqual([])
  })
  it('ignores invalid IDs, temporary files, missing fields and empty labels', () => {
    write({}, 'invalid name.json'); write({}, 'a'.repeat(81) + '.json'); write({}, 'task.tmp'); write({ updatedAt: null }); write({ label: '\nignored' }, 'empty.json')
    expect(reader.read()).toEqual([])
  })
  it('tolerates missing directories and polls, publishing changes only', () => {
    vi.useFakeTimers(); rmSync(root, { recursive: true }); const changed = vi.fn()
    const monitor = new ExternalAgentStatusMonitor(root, changed, () => now)
    try {
      expect(() => monitor.start()).not.toThrow(); expect(changed).toHaveBeenCalledTimes(1)
      vi.advanceTimersByTime(2_000); expect(changed).toHaveBeenCalledTimes(1)
      mkdirSync(root); write(); vi.advanceTimersByTime(2_000); expect(changed).toHaveBeenCalledTimes(2)
      write(); vi.advanceTimersByTime(2_000); expect(changed).toHaveBeenCalledTimes(2)
      now += 60_001; vi.advanceTimersByTime(2_000); expect(changed).toHaveBeenCalledTimes(3)
      monitor.stop(); now += 2_000; vi.advanceTimersByTime(2_000); expect(changed).toHaveBeenCalledTimes(3)
    } finally { monitor.stop() }
  })
})
