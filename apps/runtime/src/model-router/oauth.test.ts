import { afterEach, describe, expect, it } from 'vitest'
import { createServer } from 'node:http'
import { setTimeout as delay } from 'node:timers/promises'
import { OAuthLoopbackFlow } from './oauth'
import { RouterManagementClient, type OAuthStatus } from './management-client'
import { isSubscriptionAuthorizationUrl } from './authorization-url'

const flows: OAuthLoopbackFlow[] = []
afterEach(async () => { await Promise.all(flows.splice(0).map(flow => flow.cancel())) })
const authorization = (provider = 'codex') => provider === 'codex'
  ? 'https://auth.openai.com/oauth/authorize?redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback&state=test-state'
  : 'https://accounts.google.com/o/oauth2/v2/auth?redirect_uri=http%3A%2F%2F127.0.0.1%3A51121%2Foauth-callback&state=test-state'
async function fixture(options: { locale?: 'en' | 'zh-CN'; timeoutMs?: number; provider?: 'codex' | 'antigravity'; manual?: boolean; error?: string; omitAuthId?: boolean } = {}) {
  let status: OAuthStatus = { status: 'wait' }, callbacks = 0, cancels = 0
  const client = {
    oauthStatus: async () => status,
    callback: async () => { callbacks++; if (!options.manual) status = options.error ? { status: 'error', error: options.error } : options.omitAuthId ? { status: 'ok' } : { status: 'ok', auth_id: 'fixture.json' } },
    cancelOAuth: async () => { cancels++ }
  } as unknown as RouterManagementClient
  const flow = await OAuthLoopbackFlow.start({ provider: options.provider ?? 'codex', login: { url: authorization(options.provider), state: 'test-state' }, client, callbackPort: 0, timeoutMs: options.timeoutMs ?? 3000, ...(options.locale ? { locale: options.locale } : {}) })
  flows.push(flow)
  const path = options.provider === 'antigravity' ? '/oauth-callback' : '/auth/callback'
  return { flow, url: `http://127.0.0.1:${flow.port}${path}?state=test-state&code=fake-code`, callbacks: () => callbacks, cancels: () => cancels, finish: () => { status = { status: 'ok', auth_id: 'fixture.json' } } }
}
describe('short-lived dual loopback OAuth callback', () => {
  it.each([['en', 'Sign-in complete. You can return to Matou.'], ['zh-CN', '登录完成，可以回到 Matou 了']] as const)('serves the %s completion page after router status ok', async (locale, text) => {
    const { flow, url, callbacks } = await fixture({ locale })
    const response = await fetch(url)
    expect(response.status).toBe(200)
    expect(await response.text()).toContain(text)
    expect(response.headers.get('cache-control')).toBe('no-store')
    await expect(flow.completion).resolves.toEqual({ status: 'ok', authId: 'fixture.json' })
    expect(callbacks()).toBe(1)
    await flow.close()
    await expect(fetch(url)).rejects.toThrow()
  })
  it('completes Antigravity sign-in when router oauth/status returns ok without auth_id', async () => {
    const { flow, url, callbacks } = await fixture({ provider: 'antigravity', omitAuthId: true })
    const response = await fetch(url)
    expect(response.status).toBe(200)
    expect(await response.text()).toContain('登录完成，可以回到 Matou 了')
    await expect(flow.completion).resolves.toEqual({ status: 'ok' })
    expect(callbacks()).toBe(1)
  })
  it('rejects bad state/path/method without consuming the valid attempt, including IPv6', async () => {
    const { url, callbacks } = await fixture()
    expect((await fetch(url.replace('test-state', 'foreign-state'))).status).toBe(400)
    expect((await fetch(url.replace('/auth/callback', '/wrong'))).status).toBe(404)
    expect((await fetch(url, { method: 'POST' })).status).toBe(400)
    expect(callbacks()).toBe(0)
    expect((await fetch(url.replace('127.0.0.1', '[::1]'))).status).toBe(200)
  })
  it('rejects duplicate callbacks while the first exchange is pending', async () => {
    const { flow, url, callbacks, finish } = await fixture({ manual: true })
    const first = fetch(url)
    while (!flow.callbackAccepted) await delay(5)
    expect((await fetch(url)).status).toBe(409)
    expect(callbacks()).toBe(1)
    finish()
    expect((await first).status).toBe(200)
  })
  it.each(['codex', 'antigravity'] as const)('cleans up cancelled and expired %s callbacks', async provider => {
    const cancelled = await fixture({ provider, manual: true })
    const browser = fetch(cancelled.url)
    while (!cancelled.flow.callbackAccepted) await delay(5)
    await cancelled.flow.cancel()
    expect((await browser).status).toBe(410)
    expect((await cancelled.flow.completion).status).toBe('cancelled')
    expect(cancelled.cancels()).toBe(1)
    await expect(fetch(cancelled.url)).rejects.toThrow()
    const expired = await fixture({ provider, timeoutMs: 40 })
    expect((await expired.flow.completion).status).toBe('expired')
    await expired.flow.close()
    await expect(fetch(expired.url)).rejects.toThrow()
  })
  it('escapes failure reasons and never puts callback secrets into HTML', async () => {
    const { url } = await fixture({ error: '<script>alert(1)</script> access_token=private-token' })
    const response = await fetch(url), html = await response.text()
    expect(response.status).toBe(400)
    expect(html).toContain('&lt;script&gt;')
    expect(html).not.toContain('private-token')
    expect(html).not.toContain('fake-code')
  })
  it.each(['127.0.0.1', '::1'])('reports a busy %s callback port and closes any partial listener', async host => {
    const server = createServer()
    await new Promise<void>(done => server.listen({ host, port: 0, ipv6Only: host === '::1' }, done))
    const port = (server.address() as { port: number }).port
    try {
      await expect(OAuthLoopbackFlow.start({ provider: 'codex', login: { url: authorization(), state: 'test-state' }, client: {} as RouterManagementClient, callbackPort: port })).rejects.toThrow('登录端口')
    } finally { await new Promise<void>(done => server.close(() => done())) }
    const replacement = createServer()
    await new Promise<void>(done => replacement.listen(port, '127.0.0.1', done))
    await new Promise<void>(done => replacement.close(() => done()))
  })
  it('allows only fixed authorization hosts and loopback redirect URIs', () => {
    expect(isSubscriptionAuthorizationUrl('codex', authorization())).toBe(true)
    expect(isSubscriptionAuthorizationUrl('antigravity', authorization('antigravity'))).toBe(true)
    for (const value of [authorization().replace('auth.openai.com', 'auth.openai.com.evil.test'), authorization().replace('https:', 'http:'), authorization().replace('localhost', 'evil.test'), authorization().replace('1455', '1456'), authorization() + '#secret']) expect(isSubscriptionAuthorizationUrl('codex', value)).toBe(false)
  })
})
