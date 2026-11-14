import { afterEach, describe, expect, it, vi } from 'vitest'
import { chmod, mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { setTimeout as delay } from 'node:timers/promises'
import { PtySession } from '../session/pty-session'
import type { SubscriptionAccounts } from './subscription-accounts'
import type { ClaudeLaunchInput } from './claude-launch'

const root = resolve(import.meta.dirname, '../../../../output/model-router-stage4')
const sessions: PtySession[] = []
afterEach(async () => { await Promise.all(sessions.splice(0).map(session => session.shutdownForRuntime({ gracePeriodMs: 100, hardKillWaitMs: 1000 }))); vi.unstubAllEnvs() })
async function until(check: () => boolean) { const deadline = Date.now() + 5000; while (!check()) { if (Date.now() > deadline) throw new Error('Fixture output timed out'); await delay(10) } }
async function fixture() {
  await mkdir(root, { recursive: true }); const dataRoot = await mkdtemp(join(root, 'spawn-'))
  const executable = join(dataRoot, 'capture.cjs'), capture = join(dataRoot, 'capture.json'), settingsPath = join(dataRoot, 'settings.json')
  await writeFile(settingsPath, JSON.stringify({ hooks: { Stop: [{ hooks: [{ type: 'http', url: 'http://127.0.0.1/fixture' }] }] }, env: { ANTHROPIC_API_KEY: 'old-settings-key', CLAUDE_CODE_OAUTH_TOKEN: 'old-settings-oauth', KEEP_ME: 'keep' }, customSetting: true }))
  await writeFile(executable, `#!/usr/bin/env node
const fs = require('fs'); const keys = ['ANTHROPIC_API_KEY','CLAUDE_CODE_OAUTH_TOKEN','ANTHROPIC_AUTH_TOKEN','ANTHROPIC_BASE_URL','ANTHROPIC_DEFAULT_OPUS_MODEL','ANTHROPIC_DEFAULT_OPUS_MODEL_NAME','ANTHROPIC_DEFAULT_FUTURE_MODEL','CLAUDE_CODE_FUTURE_SUBAGENT_MODEL','CLAUDE_CODE_MAX_CONTEXT_TOKENS','CLAUDE_CODE_MAX_OUTPUT_TOKENS','DISABLE_AUTO_COMPACT','TZ'];
fs.writeFileSync(${JSON.stringify(capture)},JSON.stringify({argv:process.argv.slice(2),env:Object.fromEntries(keys.filter(key=>process.env[key]!==undefined).map(key=>[key,process.env[key]]))})); console.log('CAPTURE_READY'); setInterval(()=>{},1000);
`); await chmod(executable, 0o755)
  vi.stubEnv('MATOU_CLAUDE_COMMAND', executable)
  let leases = 0
  const input: Omit<ClaudeLaunchInput, 'inheritedEnvNames'> = { subscription: 'chatgpt', accountPrefix: 'matou-chatgpt', routerUrl: 'http://127.0.0.1:12345', clientKey: 'local-router-client-key', providerName: 'Fixture subscription', mainRole: 'opus', models: { opus: { id: 'gpt-6.1-sol', displayName: 'Fixture Opus', contextWindow: 400000, maxOutputTokens: 128000 }, sonnet: { id: 'gpt-6-sol', contextWindow: 200000 }, haiku: { id: 'gpt-6-luna', contextWindow: 100000 } } }
  const accounts = { prepareLaunch: vi.fn(async () => { leases++; let released = false; return { input, lease: { connection: { baseUrl: input.routerUrl, clientKey: input.clientKey, managementKey: 'private', pid: process.pid }, release: () => { if (!released) { released = true; leases-- } } } } }) }
  let output = ''
  const options = { sessionId: 'spawn', executionContextId: 'fixture', cols: 100, rows: 24, cwd: dataRoot, dataRoot, profile: 'claude-code' as const, settingsPath, onOutput: (data: string) => { output += data }, env: { ANTHROPIC_API_KEY: 'old-override-key', CLAUDE_CODE_OAUTH_TOKEN: 'old-override-oauth', ANTHROPIC_AUTH_TOKEN: 'old-override-token', TZ: 'Asia/Singapore' } }
  return { dataRoot, capture, settingsPath, accounts, options, leases: () => leases, ready: async () => { await until(() => output.includes('CAPTURE_READY')); return JSON.parse(await readFile(capture, 'utf8')) } }
}
describe('subscription projection at the actual final PTY spawn boundary', () => {
  it.each(['new', 'resume', 'fork'] as const)('clears inherited/override credentials, merges temporary settings, and owns one live lease for %s', async mode => {
    const f = await fixture()
    vi.stubEnv('ANTHROPIC_API_KEY', 'inherited-old-key'); vi.stubEnv('ANTHROPIC_DEFAULT_FUTURE_MODEL', 'inherited-model'); vi.stubEnv('CLAUDE_CODE_FUTURE_SUBAGENT_MODEL', 'inherited-child'); vi.stubEnv('DISABLE_AUTO_COMPACT', '1')
    const session = await PtySession.create({ ...f.options, subscription: { accounts: f.accounts as unknown as SubscriptionAccounts, providerConfigId: 'subscription-id' }, ...(mode === 'new' ? {} : { providerSessionId: 'resume-identity' }), ...(mode === 'fork' ? { forkSession: true } : {}) })
    sessions.push(session)
    const capture = await f.ready()
    expect(capture.env).toEqual({ ANTHROPIC_AUTH_TOKEN: 'local-router-client-key', ANTHROPIC_BASE_URL: 'http://127.0.0.1:12345', ANTHROPIC_DEFAULT_OPUS_MODEL: 'matou-chatgpt/matou-opus', ANTHROPIC_DEFAULT_OPUS_MODEL_NAME: 'Fixture Opus', CLAUDE_CODE_MAX_CONTEXT_TOKENS: '512000', CLAUDE_CODE_MAX_OUTPUT_TOKENS: '128000', TZ: 'Asia/Singapore' })
    expect(capture.argv).toContain('matou-chatgpt/matou-opus')
    if (mode !== 'new') expect(capture.argv).toContain('--resume')
    if (mode === 'fork') expect(capture.argv).toContain('--fork-session')
    const settings = JSON.parse(await readFile(f.settingsPath, 'utf8'))
    expect(settings.env).toEqual({ ANTHROPIC_API_KEY: '', CLAUDE_CODE_OAUTH_TOKEN: '', CLAUDE_CODE_DISABLE_1M_CONTEXT: '1', CLAUDE_CODE_MAX_CONTEXT_TOKENS: '512000', CLAUDE_AUTOCOMPACT_PCT_OVERRIDE: '90', KEEP_ME: 'keep' })
    expect(settings.autoCompactWindow).toBe(512000)
    expect(settings.customSetting).toBe(true)
    expect(settings.hooks.Stop[0].hooks[0].type).toBe('http')
    expect(f.leases()).toBe(1)
    expect(process.env.ANTHROPIC_API_KEY).toBe('inherited-old-key')
    await session.shutdownForRuntime({ gracePeriodMs: 100, hardKillWaitMs: 1000 })
    expect(f.leases()).toBe(0)
  })
  it('keeps the existing official/API key environment and settings construction when no subscription is selected', async () => {
    const f = await fixture(), before = await readFile(f.settingsPath, 'utf8')
    const session = await PtySession.create({ ...f.options, model: 'official-model', contextWindow: null }); sessions.push(session)
    const capture = await f.ready()
    expect(capture.env.ANTHROPIC_API_KEY).toBe('old-override-key')
    expect(capture.env.CLAUDE_CODE_OAUTH_TOKEN).toBe('old-override-oauth')
    expect(capture.env.ANTHROPIC_AUTH_TOKEN).toBe('old-override-token')
    expect(capture.argv).toContain('official-model')
    expect(await readFile(f.settingsPath, 'utf8')).toBe(before)
    expect(f.accounts.prepareLaunch).not.toHaveBeenCalled()
    expect(f.leases()).toBe(0)
  })
  it('applies 512K to an API GPT route without replacing credentials or hooks', async () => {
    const f = await fixture(); const session = await PtySession.create({ ...f.options, model: 'gpt-6-astra' }); sessions.push(session)
    const capture = await f.ready(); const settings = JSON.parse(await readFile(f.settingsPath, 'utf8'))
    expect(capture.env).toMatchObject({ ANTHROPIC_API_KEY: 'old-override-key', CLAUDE_CODE_MAX_CONTEXT_TOKENS: '512000' })
    expect(settings.autoCompactWindow).toBe(512000)
    expect(settings.env).toMatchObject({ ANTHROPIC_API_KEY: 'old-settings-key', CLAUDE_AUTOCOMPACT_PCT_OVERRIDE: '90' })
    expect(settings.hooks.Stop[0].hooks[0].type).toBe('http')
    expect(f.accounts.prepareLaunch).not.toHaveBeenCalled()
  })
  it('releases a prepared subscription lease if settings cannot be merged', async () => {
    const f = await fixture(); await writeFile(f.settingsPath, '{invalid')
    await expect(PtySession.create({ ...f.options, subscription: { accounts: f.accounts as unknown as SubscriptionAccounts, providerConfigId: 'subscription-id' } })).rejects.toThrow()
    expect(f.leases()).toBe(0)
  })
  it('releases its lease when the PTY helper reports an executable failure as an immediate process exit', async () => {
    const f = await fixture(); vi.stubEnv('MATOU_CLAUDE_COMMAND', join(f.dataRoot, 'missing-executable'))
    const started = Date.now(), onProcessExit = vi.fn()
    const session = await PtySession.create({ ...f.options, onProcessExit, subscription: { accounts: f.accounts as unknown as SubscriptionAccounts, providerConfigId: 'subscription-id' } }); sessions.push(session)
    await vi.waitFor(() => expect(onProcessExit).toHaveBeenCalledOnce(), { timeout: 10000 })
    // Process exit releases the lease before slower journal finalization.
    expect(f.leases()).toBe(0)
    await session.whenClosed()
    expect(f.leases()).toBe(0)
    console.info(`PTY executable failure: exit and lease release completed in ${Date.now() - started}ms`)
  }, 15000)
})

it.each([null, 512000, 872000])('uses the saved context policy for a non-GPT provider: %s', async contextWindow => {
  const f = await fixture(); const before = JSON.parse(await readFile(f.settingsPath, 'utf8'))
  const session = await PtySession.create({ ...f.options, model: 'claude-sonnet-5-5', contextWindow }); sessions.push(session)
  const capture = await f.ready(), settings = JSON.parse(await readFile(f.settingsPath, 'utf8'))
  expect(capture.argv).toContain('claude-sonnet-5-5')
  expect(settings.hooks).toEqual(before.hooks)
  if (contextWindow) { expect(settings.autoCompactWindow).toBe(contextWindow); expect(capture.env.CLAUDE_CODE_MAX_CONTEXT_TOKENS).toBe(String(contextWindow)) }
  else expect(settings).toEqual(before)
})
