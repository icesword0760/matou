import { describe, expect, it } from 'vitest'
import { DshProviderEvents, parseDshObservation, type DshObservation } from './dsh-provider-events'

function event(type: string, sequence: number, data: Record<string, unknown> = {}, extra: Partial<DshObservation> = {}): DshObservation {
  return { type, sequence, generation: 1, sessionId: 'session-a', data, ready: type === 'ready', persisted: true, pending: 0, error: false, ...extra }
}
function feed(state: DshProviderEvents, observation: DshObservation) {
  return state.accepts(observation) ? state.ingest(observation) : undefined
}

describe('DSH live event mapping', () => {
  it('requires a foreground ready handshake and rejects replay, old generations and background identities', () => {
    const state = new DshProviderEvents()
    expect(feed(state, event('turn/start', 1, { turn: 1 }))).toBeUndefined()
    expect(feed(state, event('ready', 2))).toEqual({ workStatus: 'idle' })
    expect(feed(state, event('turn/start', 3, { turn: 1 }))).toEqual({ workStatus: 'running' })
    expect(feed(state, event('turn/end', 3, { turn: 1, reason: { kind: 'completed' } }))).toBeUndefined()
    expect(feed(state, event('ready', 4, {}, { generation: 2, sessionId: 'session-b' }))).toEqual({ workStatus: 'idle' })
    expect(feed(state, event('turn/end', 5, { turn: 1, reason: { kind: 'completed' } }))).toBeUndefined()
  })
  it('pairs concurrent user requests, clears pending state and emits one DSH completion', () => {
    const state = new DshProviderEvents()
    feed(state, event('ready', 1))
    feed(state, event('turn/start', 2, { turn: 1 }))
    expect(feed(state, event('approval/asked', 3, { id: 'audit' }))?.workStatus).toBe('running')
    expect(feed(state, event('approval/pending', 4, { id: 'a', toolName: 'shell' }))?.notification).toMatchObject({ title: 'DSH', eventType: 'permission' })
    feed(state, event('question/pending', 5, { id: 'b' }))
    expect(feed(state, event('approval/resolved', 6, { id: 'a' }))?.workStatus).toBe('needs-input')
    expect(feed(state, event('question/resolved', 7, { id: 'b' }, { status: 'running' }))?.workStatus).toBe('running')
    expect(feed(state, event('turn/end', 8, { turn: 1, reason: { kind: 'completed' } }))).toMatchObject({ workStatus: 'idle', notification: { title: 'DSH', eventType: 'completed' } })
    expect(feed(state, event('turn/end', 9, { turn: 1, reason: { kind: 'completed' } }))?.notification).toBeUndefined()
  })
  it('uses structured whole-turn failures, keeps errors until a real retry and never calls cancellation success', () => {
    const state = new DshProviderEvents()
    feed(state, event('ready', 1))
    feed(state, event('turn/start', 2, { turn: 1 }))
    expect(feed(state, event('tool/result', 3, { error: 'failed tool' }))?.workStatus).toBe('running')
    expect(feed(state, event('turn/end', 4, { turn: 1, reason: { kind: 'error', error: { code: 'NETWORK', message: 'Connection failed' } } }))).toMatchObject({ workStatus: 'error', notification: { eventType: 'error', body: 'Connection failed' } })
    expect(feed(state, event('agent/status', 5, {}, { status: 'idle' }))?.workStatus).toBe('error')
    expect(feed(state, event('ready', 6, {}, { error: true }))?.notification).toBeUndefined()
    expect(feed(state, event('turn/start', 7, { turn: 2 }))?.workStatus).toBe('running')
    expect(feed(state, event('turn/end', 8, { turn: 2, reason: { kind: 'aborted' } }))).toEqual({ workStatus: 'idle' })
  })
  it('does not synthesize successful completion from missing or future fields', () => {
    const state = new DshProviderEvents()
    feed(state, event('ready', 1))
    expect(feed(state, event('turn/end', 2, { reason: { kind: 'completed' } }))?.notification).toBeUndefined()
    feed(state, event('turn/start', 3, { turn: 1 }))
    expect(feed(state, event('turn/end', 4, { turn: 1, reason: { kind: 'future' } }))?.notification).toBeUndefined()
    expect(parseDshObservation({ dsh_observation: 1, run_id: 'stale', foreground: true }, 'current')).toBeUndefined()
    expect(parseDshObservation({ dsh_observation: 1, run_id: 'run', foreground: false, sequence: 1, generation: 1, session_id: 's', event_type: 'ready' }, 'run')).toBeUndefined()
  })
})
