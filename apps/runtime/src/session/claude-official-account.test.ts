import { beforeEach, expect, it, vi } from 'vitest'
const fixture = vi.hoisted(() => ({ account: 'A', token: 'first', reads: 0 }))
vi.mock('node:child_process', () => ({ execFile: vi.fn((_f, _a, _o, cb) => cb(new Error('fixture keychain unavailable'), '', '')) }))
vi.mock('node:fs/promises', () => ({ readFile: vi.fn(async (path: string) => {
 fixture.reads++
 if (path.endsWith('.claude.json')) return JSON.stringify(fixture.account ? { oauthAccount: { accountUuid: fixture.account, organizationUuid: 'org', emailAddress: 'fixture@example.invalid' } } : {})
 return JSON.stringify({ claudeAiOauth: { accessToken: fixture.token, expiresAt: Date.now() + 100000 } })
}) }))
import { captureOfficialClaudeAccount } from './claude-official-account'
beforeEach(() => { fixture.account = 'A'; fixture.token = 'first'; fixture.reads = 0 })
it('pins accountUuid/org from local official configuration while allowing credential refresh', async () => {
 const connection = await captureOfficialClaudeAccount({ CLAUDE_CONFIG_DIR: '/fixture-only' })
 fixture.token = 'refreshed'
 expect(await connection.readHeaders()).toMatchObject({ identity: connection.identity, headers: { authorization: 'Bearer refreshed' } })
 fixture.account = 'B'
 await expect(connection.readHeaders()).rejects.toThrow(/identity/i)
})
it('does not invent an identity for a token-only account', async () => {
 fixture.account = ''
 await expect(captureOfficialClaudeAccount({ CLAUDE_CONFIG_DIR: '/fixture-only', CLAUDE_CODE_OAUTH_TOKEN: 'fixture' })).rejects.toThrow(/identity/i)
})
