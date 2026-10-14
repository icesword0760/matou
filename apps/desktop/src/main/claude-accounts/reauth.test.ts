import { describe, it, expect, vi } from 'vitest'
import { ClaudeAccountsManager, type AccountPreferences, type AccountRuntime, type PreferenceStore } from './manager'
import { AccountFault, type AccountEngine, type EngineState } from './engine'
const account = { id: 'a', name: 'Claude', email: 'wo•••@example.com', plan: 'Claude', needsLogin: true }
function setup() {
  let preferences: AccountPreferences = { names: { a: '工作账户' }, rememberScope: false }
  let live: EngineState = { accounts: [account], currentId: 'a' }
  const engine: AccountEngine = { run: vi.fn(async input => {
    if (input.action === 'login') return { accounts: [], draft: { ...account, needsLogin: false } }
    if (input.action === 'save-draft') live = { ...live, accounts: [{ ...account, needsLogin: false }] }
    return live
  }) }
  const store: PreferenceStore = { load: async () => preferences, save: async next => { preferences = next } }
  const runtime: AccountRuntime = { request: vi.fn(async () => [{ sessionId: 's', runId: 'r', defaultLogin: true, busy: false, resumable: true }]) }
  const create = () => new ClaudeAccountsManager(engine, store, runtime, vi.fn())
  return { engine, runtime, create, setLive: (next: EngineState) => { live = next } }
}
async function renew(manager: ClaudeAccountsManager) {
  await manager.command({ type: 'inspect' })
  await manager.command({ type: 'login', replaceId: 'a' })
  return manager.command({ type: 'save-draft', name: '忽略改名' })
}
describe('reauthentication is explicit and never restarts sessions automatically', () => {
  it('keeps the original name, saves renewal across app restarts, applies only on request, then resumes separately', async () => {
    const f = setup(); let m = f.create()
    await renew(m)
    expect(m.state()).toMatchObject({ currentId: 'a', renewedIds: ['a'], accounts: [expect.objectContaining({ name: '工作账户' })] })
    expect(f.engine.run).not.toHaveBeenCalledWith(expect.objectContaining({ action: 'reauth' }))
    expect(f.runtime.request).not.toHaveBeenCalledWith('hold')
    await m.dispose(); m = f.create(); await m.command({ type: 'inspect' })
    expect(m.state().renewedIds).toEqual(['a'])
    await m.command({ type: 'apply-reauth', id: 'a' })
    expect(f.engine.run).toHaveBeenCalledWith({ action: 'reauth', id: 'a' })
    expect(m.state()).toMatchObject({ appliedReauthId: 'a', renewedIds: [], pendingSessionIds: ['s'] })
    expect(f.runtime.request).not.toHaveBeenCalledWith('resume', 's', 'r')
    await m.command({ type: 'resume', sessionId: 's', runId: 'r' })
    expect(f.runtime.request).toHaveBeenCalledWith('resume', 's', 'r')
    await m.dispose()
  })
  it('allows explicit renewal activation while busy without interrupting sessions', async () => {
    const f = setup(), m = f.create()
    vi.mocked(f.runtime.request).mockResolvedValue([{ sessionId: 's', runId: 'r', defaultLogin: true, busy: true, resumable: true }])
    await renew(m); await m.command({ type: 'apply-reauth', id: 'a' })
    expect(m.state()).toMatchObject({ renewedIds: [], appliedReauthId: 'a', pendingSessionIds: ['s'] })
    expect(m.state().error).toBeUndefined()
    expect(f.engine.run).toHaveBeenCalledWith({ action: 'reauth', id: 'a' })
    expect(f.runtime.request).not.toHaveBeenCalledWith('resume', expect.anything(), expect.anything())
    expect(f.runtime.request).not.toHaveBeenCalledWith('restart', expect.anything(), expect.anything())
    expect(f.runtime.request).toHaveBeenLastCalledWith('release')
    await m.dispose()
  })
  it('rejects a different identity and retains repair context for retry', async () => {
    const f = setup(), m = f.create(); await m.command({ type: 'inspect' })
    vi.mocked(f.engine.run).mockResolvedValueOnce({ accounts: [], draft: { ...account, id: 'wrong' } })
    await m.command({ type: 'login', replaceId: 'a' })
    expect(m.state()).toMatchObject({ error: 'identity', reauthId: 'a', currentId: 'a' })
    expect(m.state().draft).toBeUndefined()
    await m.command({ type: 'login', replaceId: 'a' })
    expect(m.state().draft?.id).toBe('a')
    await m.command({ type: 'cancel-login' })
    expect(m.state().reauthId).toBeUndefined()
    expect(m.state().renewedIds).toEqual([])
    await m.dispose()
  })
  it('rejects unsaved activation and retains pending renewal after an external-default conflict or engine error', async () => {
    const f = setup(), m = f.create()
    await m.command({ type: 'apply-reauth', id: 'a' })
    expect(m.state().error).toBe('conflict')
    await renew(m)
    vi.mocked(f.engine.run).mockRejectedValueOnce(new AccountFault('conflict'))
    await m.command({ type: 'apply-reauth', id: 'a' })
    expect(m.state()).toMatchObject({ error: 'conflict', renewedIds: ['a'], currentId: 'a' })
    expect(f.runtime.request).toHaveBeenLastCalledWith('release')
    await m.dispose()
  })
  it('never changes default when renewing a different saved account', async () => {
    const f = setup(), m = f.create()
    f.setLive({ accounts: [account, { ...account, id: 'b' }], currentId: 'b' })
    await renew(m)
    expect(m.state()).toMatchObject({ currentId: 'b', renewedIds: ['a'] })
    expect(f.engine.run).not.toHaveBeenCalledWith(expect.objectContaining({ action: 'switch' }))
    await m.dispose()
  })
})
