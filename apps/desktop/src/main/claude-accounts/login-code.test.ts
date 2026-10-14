import { describe, it, expect, vi } from 'vitest'
import { mkdtemp, writeFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { ClaudeSwapEngine, type AccountEngine, type EngineState } from './engine'
import { ClaudeAccountsManager } from './manager'
import { validLoginCode, isAccountCommand } from '../../shared/claude-accounts'
const code = 'fixture_code_123456#fixture_state_123456'

describe('manual authorization stays bound to its live login', () => {
  it('accepts code#state only; rejects URLs, commands, controls and oversized input', () => {
    expect(validLoginCode(` ${code}\n`)).toBe(true)
    for (const value of ['', 'https://example.com/?code=secret', 'echo anything', code + '\ncommand', 'a'.repeat(4097), 'missing_state']) expect(validLoginCode(value)).toBe(false)
    expect(isAccountCommand({ type: 'submit-login-code', attemptId: 'attempt', code })).toBe(true)
    expect(isAccountCommand({ type: 'submit-login-code', attemptId: '', code })).toBe(false)
  })
  it('keeps old default, forwards once, hides secrets, and rejects codes after completion', async () => {
    let finish!: (s: EngineState) => void
    const engine: AccountEngine = {
      run: vi.fn(async input => input.action === 'login' ? new Promise<EngineState>(r => { finish = r }) : { accounts: [], currentId: 'old' }),
      submitLoginCode: vi.fn(async () => {})
    }
    const publish = vi.fn()
    const manager = new ClaudeAccountsManager(engine, { load: async () => ({ names: {}, rememberScope: false }), save: async () => {} }, { request: async () => [] }, publish)
    await manager.command({ type: 'inspect' })
    const login = manager.command({ type: 'login' })
    await vi.waitFor(() => expect(manager.state().loginAttemptId).toBeTruthy())
    const attemptId = manager.state().loginAttemptId!
    await manager.command({ type: 'submit-login-code', attemptId: 'old-attempt', code })
    expect(engine.submitLoginCode).not.toHaveBeenCalled()
    await manager.command({ type: 'submit-login-code', attemptId, code: 'invalid' })
    expect(manager.state().error).toBe('login-code-invalid')
    await Promise.all([manager.command({ type: 'submit-login-code', attemptId, code }), manager.command({ type: 'submit-login-code', attemptId, code })])
    expect(engine.submitLoginCode).toHaveBeenCalledTimes(1)
    expect(manager.state()).toMatchObject({ operation: 'login', loginCodeSubmitted: true, currentId: 'old' })
    expect(JSON.stringify(publish.mock.calls)).not.toContain(code)
    finish({ accounts: [], draft: { id: 'new', name: 'New', email: 'fixture@example.com', plan: 'Claude', needsLogin: false } })
    await login
    expect(manager.state()).toMatchObject({ operation: 'idle', currentId: 'old', draft: { id: 'new' } })
    expect(manager.state().loginAttemptId).toBeUndefined()
    await manager.command({ type: 'submit-login-code', attemptId, code })
    expect(manager.state().error).toBe('login-ended')
    expect(engine.submitLoginCode).toHaveBeenCalledTimes(1)
    await manager.dispose()
  })
  it('sends through a private persistent pipe and invalidates it on exit', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-code-pipe-'))
    try {
      const script = join(root, 'fixture.cjs')
      await writeFile(script, `const rl=require('node:readline').createInterface({input:process.stdin}); let initial; rl.on('line',line=>{const m=JSON.parse(line); if(!initial){initial=m;return;} if(m.nonce!==initial.nonce||m.code!==${JSON.stringify(code)})process.exit(2); console.log(JSON.stringify({ok:true,value:{accounts:[],draft:{id:'fixture'}}})); process.exit(0);});`)
      const engine = new ClaudeSwapEngine({ executable: process.execPath, args: [script], stateRoot: root }, { ...process.env, SHELL: '/fixture/no-shell' })
      const login = engine.run({ action: 'login', nonce: 'attempt' })
      await vi.waitFor(async () => { await expect(engine.submitLoginCode('attempt', code)).resolves.toBeUndefined() })
      expect((await login).draft?.id).toBe('fixture')
      await expect(engine.submitLoginCode('attempt', code)).rejects.toMatchObject({ code: 'login-ended' })
    } finally { await rm(root, { recursive: true, force: true }) }
  })
  it('cancellation immediately revokes the input pipe', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-code-abort-'))
    try {
      const script = join(root, 'fixture.cjs')
      await writeFile(script, 'process.stdin.resume(); setTimeout(()=>{},30000)')
      const engine = new ClaudeSwapEngine({ executable: process.execPath, args: [script], stateRoot: root }, { ...process.env, SHELL: '/fixture/no-shell' })
      const abort = new AbortController()
      const login = engine.run({ action: 'login', nonce: 'attempt' }, abort.signal)
      const result = expect(login).rejects.toMatchObject({ code: 'login-failed' })
      await vi.waitFor(async () => { await expect(engine.submitLoginCode('attempt', code)).resolves.toBeUndefined() })
      abort.abort()
      await expect(engine.submitLoginCode('attempt', code)).rejects.toMatchObject({ code: 'login-ended' })
      await result
    } finally { await rm(root, { recursive: true, force: true }) }
  })
})
