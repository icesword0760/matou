import { describe, expect, it } from 'vitest'
import { parseDshHud } from './dsh-hud-payload'
import { SessionHudRegistry } from './session-hud-registry'

const nativeHud = () => import(new URL('../../control-assets/providers/dsh/hud.mjs', import.meta.url).href)

describe('DSH authoritative HUD facts', () => {
  it('uses current model and native projection occupancy instead of billed or cumulative tokens', async () => {
    const { observationHud } = await nativeHud()
    const snapshot = { values: { contextPressure: { projectedTokens: 250, pressureTokens: 100, contextWindow: 1000 },
      permissions: { currentValue: 'custom-auto' }, plan: { active: true }, title: 'Native title', workingDirectory: '/native/cwd' } }
    const services: Record<string, unknown> = { sessionProjections: { snapshot: () => snapshot },
      permissionPresets: { optionOf: () => ({ name: 'Native auto' }) } }
    const hud = observationHud({ session: { id: 'native', header: { cwd: '/old' } },
      model: { current: () => ({ model: 'native-model', reasoningEffort: 'high' }) } }, { get: (name: string) => services[name] })
    expect(hud).toMatchObject({ model: 'native-model', effort: 'high', contextPercent: 25, contextWindowSize: 1000,
      providerPermissionLabel: 'plan · Native auto', title: 'Native title', cwd: '/native/cwd' })
    expect(hud).not.toHaveProperty('toolCounts')
    expect(observationHud({}, { get: () => undefined })).toEqual({})
  })

  it('folds tool starts and paired results, without sending arguments or bodies', async () => {
    const { createHudState, foldHudEvent, observationHud } = await nativeHud()
    const state = createHudState()
    const event = (type: string, data: unknown) => foldHudEvent(state, { type, data })
    event('tool/call', { callId: 'one', name: 'read_file', arguments: { secret: 'do not send' } })
    expect(observationHud({}, {}, state)).toMatchObject({ runningTools: [{ name: 'read_file' }] })
    event('tool/result', { message: { toolCallId: 'one', isError: false, content: 'private result' } })
    event('session/title', { title: 'native rename' })
    const hud = observationHud({}, {}, state)
    expect(hud).toMatchObject({ runningTools: [], toolCounts: [{ name: 'read_file', count: 1 }],
      lastTool: { name: 'read_file', status: 'completed' }, title: 'native rename' })
    expect(JSON.stringify(hud)).not.toMatch(/secret|private result|arguments/)
  })

  it('validates optional fields, preserves measured zero and hides invalid or unrelated values', () => {
    expect(parseDshHud({ model: ' native ', effort: 'high', contextPercent: 0, contextWindowSize: 1000,
      providerPermissionLabel: 'Native mode', costUsd: 99, usageWindows: [], instructionFileLabel: 'guessed',
      toolCounts: [{ name: 'read', count: 2 }, { name: 'bad', count: -1 }, { name: 'zero', count: 0 }] })).toEqual({
      model: 'native', effort: 'high', contextPercent: 0, contextWindowSize: 1000,
      providerPermissionLabel: 'Native mode', toolCounts: [{ name: 'read', count: 2 }] })
    expect(parseDshHud({ effort: 'high', contextPercent: NaN, contextWindowSize: 0, model: [], providerPermissionLabel: {} })).toEqual({})
    expect(parseDshHud({ contextPercent: 12.3456789, contextWindowSize: 1000 })).toEqual({ contextPercent: 12, contextWindowSize: 1000 })
    expect(parseDshHud(null)).toBeUndefined()
  })

  it('clears absent facts and switched-session data while preserving the shared Git environment', () => {
    const registry = new SessionHudRegistry()
    registry.spawn({ sessionId: 'card', profile: 'dsh', cwd: '/project' })
    registry.updateEnvironment('card', { cwd: '/project', gitBranch: 'branch', gitDirty: true })
    expect(registry.snapshot('card')).not.toHaveProperty('toolCounts')
    registry.ingestProvider('card', { dsh_session_id: 'first', dsh_work_status: 'running', dsh_hud: {
      model: 'native', effort: 'high', contextPercent: 25, contextWindowSize: 1000, providerPermissionLabel: 'default',
      runningTools: [{ name: 'read' }], toolCounts: [{ name: 'read', count: 1 }] } })
    expect(registry.snapshot('card')).toMatchObject({ provider: 'dsh', model: 'native', taskStatus: 'running', gitBranch: 'branch' })
    registry.ingestProvider('card', { dsh_session_id: 'first', dsh_hud: {} })
    expect(registry.snapshot('card')).not.toHaveProperty('model')
    expect(registry.snapshot('card')).not.toHaveProperty('contextPercent')
    registry.ingestProvider('card', { dsh_session_id: 'first', dsh_hud: { model: 'old' } })
    registry.ingestProvider('card', { dsh_session_id: 'second' })
    const current = registry.snapshot('card')!
    for (const key of ['model', 'effort', 'contextPercent', 'providerPermissionLabel', 'toolCounts', 'permissionMode', 'instructionFileLabel', 'costUsd', 'teamRole']) expect(current).not.toHaveProperty(key)
    expect(current.gitBranch).toBe('branch')
  })
})
