import { afterEach, describe, expect, it, vi } from 'vitest'
import { ClaudeAccountsManager, type PreferenceStore, type AccountRuntime } from './manager'
import { AccountFault, type AccountEngine, type EngineState } from './engine'
import { isAccountCommand } from '../../shared/claude-accounts'
const a = { id: 'a', name: 'Work', email: 'wo•••@example.com', plan: 'Claude', needsLogin: false }
const b = { ...a, id: 'b', name: 'Personal' }
function setup() {
  let engineState: EngineState = { accounts: [a, b], currentId: 'a' }
  const engine: AccountEngine = { run: vi.fn(async input => {
    if (input.action === 'switch') engineState = { ...engineState, currentId: String(input.id) }
    if (input.action === 'remove') engineState = { ...engineState, accounts: engineState.accounts.filter(a => a.id !== input.id) }
    if (input.action === 'login') return { accounts: [], draft: b }
    return engineState
  }) }
  const runtime: AccountRuntime = { request: vi.fn(async () => [{ sessionId: 's', runId: 'r', busy: false, resumable: true, defaultLogin: true }]) }
  const prefs: PreferenceStore = { load: vi.fn(async () => ({ names: {}, rememberScope: false })), save: vi.fn(async () => {}) }
  const publish = vi.fn()
  return { manager: new ClaudeAccountsManager(engine, prefs, runtime, publish), engine, runtime, prefs, publish }
}
afterEach(() => vi.useRealTimers())
describe('Claude account orchestration delegates to upstream engine', () => {
  it('does not touch login state on app startup', async () => {
    const { manager, engine } = setup(); await manager.snapshot(); expect(engine.run).not.toHaveBeenCalled()
  })
  it('holds runtime before switch, publishes default only after success, and resumes separately', async () => {
    const { manager, engine, runtime, publish } = setup()
    await manager.command({ type: 'inspect' })
    const pending = manager.command({ type: 'switch', id: 'b', rememberScope: true })
    await pending
    expect(engine.run).toHaveBeenCalledWith({ action: 'switch', id: 'b' })
    expect(runtime.request).toHaveBeenCalledWith('hold')
    expect(runtime.request).toHaveBeenCalledWith('release')
    expect(manager.state()).toMatchObject({ currentId: 'b', pendingSessionIds: ['s'], rememberScope: true })
    expect(publish.mock.calls.some(([s]) => s.operation === 'switching' && s.currentId === 'a')).toBe(true)
    expect(runtime.request).not.toHaveBeenCalledWith('resume', expect.anything(), expect.anything())
    await manager.command({ type: 'resume', sessionId: 's', runId: 'r' })
    expect(runtime.request).toHaveBeenCalledWith('resume', 's', 'r')
    expect(manager.state().pendingSessionIds).toEqual([])
  })
  it('keeps displayed default on engine failure and always releases runtime', async () => {
    const { manager, engine, runtime } = setup()
    await manager.command({ type: 'inspect' })
    vi.mocked(engine.run).mockRejectedValueOnce(new AccountFault('switch-failed'))
    await manager.command({ type: 'switch', id: 'b', rememberScope: false })
    expect(manager.state()).toMatchObject({ currentId: 'a', error: 'switch-failed' })
    expect(runtime.request).toHaveBeenLastCalledWith('release')
  })
  it('switches immediately with visible and hidden busy sessions without restarting or scheduling', async () => {
    vi.useFakeTimers()
    const { manager, runtime, engine } = setup()
    vi.mocked(runtime.request).mockResolvedValue([
      { sessionId: 'visible', runId: 'v', busy: true, resumable: true, defaultLogin: true },
      { sessionId: 'hidden', runId: 'h', busy: true, resumable: true, defaultLogin: true },
      { sessionId: 'custom', runId: 'c', busy: true, resumable: true, defaultLogin: false }
    ])
    await manager.command({ type: 'inspect' })
    vi.mocked(engine.run).mockClear(); vi.mocked(runtime.request).mockClear()
    await manager.command({ type: 'switch', id: 'b', rememberScope: true })
    expect(engine.run).toHaveBeenCalledExactlyOnceWith({ action: 'switch', id: 'b' })
    expect(manager.state()).toMatchObject({ currentId: 'b', pendingSessionIds: ['visible', 'hidden'], rememberScope: true, operation: 'idle' })
    expect(manager.state().error).toBeUndefined()
    expect(vi.mocked(runtime.request).mock.calls.map(([action]) => action)).toEqual(['hold', 'release'])
    await vi.advanceTimersByTimeAsync(5000)
    expect(engine.run).toHaveBeenCalledTimes(1)
    await manager.dispose()
  })
  it('keeps the account unchanged if runtime coordination is unavailable', async () => {
    const { manager, runtime, engine } = setup()
    await manager.command({ type: 'inspect' })
    vi.mocked(engine.run).mockClear()
    vi.mocked(runtime.request).mockRejectedValueOnce(new AccountFault('runtime'))
    await manager.command({ type: 'switch', id: 'b', rememberScope: false })
    expect(engine.run).not.toHaveBeenCalled()
    expect(manager.state()).toMatchObject({ currentId: 'a', operation: 'idle', error: 'runtime' })
    await manager.dispose()
  })
  it('coalesces repeated clicks while a switch is in progress', async () => {
    const { manager, engine } = setup()
    await manager.command({ type: 'inspect' })
    let finish!: (state: EngineState) => void
    vi.mocked(engine.run).mockClear().mockImplementationOnce(() => new Promise(resolve => { finish = resolve }))
    const first = manager.command({ type: 'switch', id: 'b', rememberScope: false })
    await vi.waitFor(() => expect(finish).toBeTypeOf('function'))
    await manager.command({ type: 'switch', id: 'b', rememberScope: false })
    finish({ accounts: [a, b], currentId: 'b' }); await first
    expect(engine.run).toHaveBeenCalledTimes(1)
    expect(manager.state().currentId).toBe('b')
    await manager.dispose()
  })
  it('adds via isolated engine login and never implicitly switches', async () => {
    const { manager, engine } = setup()
    await manager.command({ type: 'inspect' })
    await manager.command({ type: 'login' })
    expect(manager.state()).toMatchObject({ currentId: 'a', draft: b })
    await manager.command({ type: 'save-draft', name: '个人账户' })
    expect(manager.state().accounts.find(a => a.id === 'b')?.name).toBe('个人账户')
    expect(manager.state().currentId).toBe('a')
    expect(engine.run).not.toHaveBeenCalledWith(expect.objectContaining({ action: 'switch' }))
  })
  it('clears the previous draft when retrying the same identity', async () => {
    const { manager, publish } = setup()
    await manager.command({ type: 'login' })
    publish.mockClear()
    await manager.command({ type: 'login' })
    expect(publish.mock.calls.some(([s]) => s.operation === 'login' && !s.draft)).toBe(true)
    expect(manager.state().draft?.id).toBe('b')
  })
  it('renames only Matou presentation metadata', async () => {
    const { manager, engine, prefs } = setup()
    await manager.command({ type: 'inspect' }); vi.mocked(engine.run).mockClear()
    await manager.command({ type: 'rename', id: 'a', name: '工作账户' })
    expect(engine.run).not.toHaveBeenCalled()
    expect(prefs.save).toHaveBeenCalledWith({ names: { a: '工作账户' }, rememberScope: false })
  })
  it('rejects stale resume target and observes a changed external default', async () => {
    const { manager, runtime } = setup()
    await manager.command({ type: 'resume', sessionId: 's', runId: 'stale' })
    expect(manager.state().error).toBe('conflict')
    expect(runtime.request).not.toHaveBeenCalled()
  })
  it('removes through engine without issuing logout', async () => {
    const { manager, engine } = setup()
    await manager.command({ type: 'inspect' }); await manager.command({ type: 'remove', id: 'a' })
    expect(engine.run).toHaveBeenLastCalledWith({ action: 'remove', id: 'a' })
  })
  it('validates command types and labels before entering main service', () => {
    expect(isAccountCommand({ type: 'switch', id: 'b', rememberScope: true })).toBe(true)
    expect(isAccountCommand({ type: 'cancel-switch' })).toBe(false)
    expect(isAccountCommand({ type: 'rename', id: 'a', name: '工作账户' })).toBe(true)
    for (const value of [{ type: 'exec', command: 'anything' }, { type: 'rename', id: 'a', name: '' }, { type: 'rename', id: 'a', name: 'a'.repeat(13) }, { type: 'switch', id: 'a' }]) expect(isAccountCommand(value)).toBe(false)
  })
})
