import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { mkdtempSync, rmSync, writeFileSync, mkdirSync, unlinkSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { ExternalAgentStatusMonitor, ExternalAgentStatusReader, INTERRUPTED_AGENT_DETAIL, STALE_AGENT_DETAIL } from './external-agent-status'
let root: string
let now: number
let reader: ExternalAgentStatusReader
let alive: ReturnType<typeof vi.fn<(pid: number) => boolean>>
const write = (data = {}, name = 'task.json') => writeFileSync(join(root, name), JSON.stringify({ version: 1, label: 'task', state: 'running', startedAt: now, updatedAt: now, ...data }))
beforeEach(() => { root = mkdtempSync(join(tmpdir(), 'matou-external-')); now = Date.parse('2026-10-15T03:00:00Z'); alive = vi.fn(() => true); reader = new ExternalAgentStatusReader(root, () => now, alive) })
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
  it.each(['running', 'waiting', 'warning'])('marks dead %s processes interrupted and expires five minutes after detection', state => {
    alive.mockReturnValue(false)
    write({ state, pid: 42, updatedAt: now - 14_400_000, detail: 'old detail' })
    const interrupted = { state: 'error', detail: INTERRUPTED_AGENT_DETAIL }
    expect(reader.read()[0]).toMatchObject(interrupted)
    expect(alive).toHaveBeenCalledWith(42)
    now += 299_999
    expect(reader.read()[0]).toMatchObject(interrupted)
    now++
    expect(reader.read()).toEqual([])
    now += 300_000
    expect(reader.read()).toEqual([])
  })
  it.each(['running', 'waiting', 'warning'])('keeps live %s processes on the existing heartbeat rules', state => {
    write({ state, pid: 42 })
    expect(reader.read()[0]?.state).toBe(state)
    now += 60_000
    expect(reader.read()[0]?.state).toBe(state)
    now++
    expect(reader.read()[0]).toMatchObject({ state: 'error', detail: STALE_AGENT_DETAIL })
    now += 300_000
    expect(reader.read()[0]).toMatchObject({ state: 'error', detail: STALE_AGENT_DETAIL })
    write({ state, pid: 42 })
    expect(reader.read()[0]?.state).toBe(state)
  })
  it.each([undefined, '42', 0, -1, 1.5, null, true])('ignores invalid or absent pid %s and preserves stale legacy entries', pid => {
    alive.mockReturnValue(false)
    write({ pid })
    now += 60_001
    expect(reader.read()[0]).toMatchObject({ state: 'error', detail: STALE_AGENT_DETAIL })
    now += 300_000
    expect(reader.read()[0]).toMatchObject({ state: 'error', detail: STALE_AGENT_DETAIL })
    expect(alive).not.toHaveBeenCalled()
  })
  it('restarts the interruption timer when a dead pid is replaced by another dead pid', () => {
    alive.mockReturnValue(false)
    write({ pid: 42 }); reader.read()
    now += 300_000
    expect(reader.read()).toEqual([])
    write({ pid: 43, updatedAt: now - 14_400_000 })
    expect(reader.read()[0]?.detail).toBe(INTERRUPTED_AGENT_DETAIL)
    expect(alive).toHaveBeenLastCalledWith(43)
    now += 299_999
    expect(reader.read()).toHaveLength(1)
    now++
    expect(reader.read()).toEqual([])
  })
  it('clears the interruption timer when the same pid is alive again', () => {
    alive.mockReturnValue(false)
    write({ pid: 42 }); reader.read()
    now += 300_000
    expect(reader.read()).toEqual([])
    alive.mockReturnValue(true)
    expect(reader.read()[0]?.detail).toBe(STALE_AGENT_DETAIL)
    alive.mockReturnValue(false)
    expect(reader.read()[0]?.detail).toBe(INTERRUPTED_AGENT_DETAIL)
    now += 299_999
    expect(reader.read()).toHaveLength(1)
    now++
    expect(reader.read()).toEqual([])
  })
  it('clears the interruption timer when a file is deleted and recreated with the same pid', () => {
    alive.mockReturnValue(false)
    write({ pid: 42 }); reader.read()
    now += 300_000
    expect(reader.read()).toEqual([])
    unlinkSync(join(root, 'task.json'))
    expect(reader.read()).toEqual([])
    write({ pid: 42 })
    expect(reader.read()[0]?.detail).toBe(INTERRUPTED_AGENT_DETAIL)
    now += 300_000
    expect(reader.read()).toEqual([])
  })
  it('clears the interruption timer when the pid field is removed', () => {
    alive.mockReturnValue(false)
    write({ pid: 42 }); reader.read()
    now += 300_000
    expect(reader.read()).toEqual([])
    write()
    expect(reader.read()[0]?.state).toBe('running')
    write({ pid: 42 })
    expect(reader.read()[0]?.detail).toBe(INTERRUPTED_AGENT_DETAIL)
  })
  it.each(['error', 'completed', 'stopped'])('does not apply process checks to file state %s', state => {
    alive.mockReturnValue(false)
    write({ state, pid: 42, detail: 'file detail' })
    expect(reader.read()).toEqual(state === 'error' ? [expect.objectContaining({ state: 'error', detail: 'file detail' })] : [])
    now += 300_000
    expect(reader.read()).toEqual([])
    expect(alive).not.toHaveBeenCalled()
  })
  it('tracks interruption times independently for each file and reader', () => {
    alive.mockReturnValue(false)
    write({ pid: 42 }, 'first.json'); reader.read()
    now += 300_000
    write({ pid: 43 }, 'second.json')
    expect(reader.read()).toEqual([expect.objectContaining({ id: 'external:second', detail: INTERRUPTED_AGENT_DETAIL })])
    const anotherReader = new ExternalAgentStatusReader(root, () => now, alive)
    expect(anotherReader.read()).toHaveLength(2)
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
