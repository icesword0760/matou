/** Adapted from CC Switch src-tauri/src/services/agy_account/oauth.rs (MIT):
 * loopback callback ownership, state validation, timeout and cancellation.
 * Commit: 3c76d3bda670297dd8c27d7d9be9b18a4c9143f5
 * License: ../../../../packages/contracts/third-party/subscription-accounts-reference-LICENSE
 * Token exchange stays in the pinned router; Matou never reads raw tokens.
 */
import { createServer, type Server, type ServerResponse, type IncomingMessage } from 'node:http'
import { setTimeout as delay } from 'node:timers/promises'
import type { Locale, SubscriptionProvider } from '@matou/contracts'
import { RouterManagementClient, type OAuthStart } from './management-client'
import { isSubscriptionAuthorizationUrl } from './authorization-url'
import { sanitizeProviderReason } from './errors'

export type OAuthOutcome = { status: 'ok'; authId: string } | { status: 'error' | 'cancelled' | 'expired'; reason: string }
export interface OAuthFlowOptions {
  provider: SubscriptionProvider
  login: OAuthStart
  client: RouterManagementClient
  locale?: Locale
  timeoutMs?: number
  /** Isolated tests only; browser redirect URI itself stays the fixed official URI. */
  callbackPort?: number
}

export class OAuthLoopbackFlow {
  readonly #options: OAuthFlowOptions
  readonly #servers: Server[] = []
  readonly #abort = new AbortController()
  readonly completion: Promise<OAuthOutcome>
  #resolve!: (outcome: OAuthOutcome) => void
  #outcome: OAuthOutcome | undefined
  #accepted = false
  #timer: ReturnType<typeof setTimeout> | undefined
  #closing: Promise<void> | undefined
  #port = 0
  get port(): number { return this.#port }
  get callbackAccepted(): boolean { return this.#accepted }

  private constructor(options: OAuthFlowOptions) {
    this.#options = options
    this.completion = new Promise(done => { this.#resolve = done })
  }
  static async start(options: OAuthFlowOptions): Promise<OAuthLoopbackFlow> {
    if (!isSubscriptionAuthorizationUrl(options.provider, options.login.url)) throw new Error('Sign-in authorization URL is not allowed')
    const flow = new OAuthLoopbackFlow(options)
    try {
      const port = options.callbackPort ?? (options.provider === 'codex' ? 1455 : 51121)
      flow.#port = await flow.#listen('127.0.0.1', port)
      try { await flow.#listen('::1', flow.#port) }
      catch (error) {
        if (!['EAFNOSUPPORT', 'EADDRNOTAVAIL', 'EPROTONOSUPPORT'].includes((error as NodeJS.ErrnoException).code ?? '')) throw error
      }
      flow.#timer = setTimeout(() => { void flow.cancel('expired') }, options.timeoutMs ?? 5 * 60_000)
      flow.#timer.unref()
      void flow.#poll()
      return flow
    } catch (error) {
      await flow.close()
      if ((error as NodeJS.ErrnoException).code === 'EADDRINUSE') throw new Error(options.locale === 'en'
        ? 'Another program is using the sign-in port. Finish or close it first.'
        : '另一个程序正在占用登录端口，请先完成或关闭它')
      throw error
    }
  }
  async #listen(host: string, port: number): Promise<number> {
    const server = createServer((request, response) => { void this.#callback(request, response) })
    try {
      await new Promise<void>((done, reject) => {
        server.once('error', reject)
        server.listen({ host, port, ipv6Only: host === '::1' }, done)
      })
      server.on('error', () => this.#finish({ status: 'error', reason: 'Sign-in callback listener failed' }))
      this.#servers.push(server)
      return (server.address() as { port: number }).port
    } catch (error) { server.close(); throw error }
  }
  async #callback(request: IncomingMessage, response: ServerResponse): Promise<void> {
    const locale = this.#options.locale ?? 'zh-CN'
    const fail = (code: number, reason: string) => callbackPage(response, code, false, locale, reason)
    if (request.method !== 'GET' || (request.url?.length ?? 0) > 8192) { fail(400, 'Invalid callback request'); return }
    const url = new URL(request.url ?? '/', 'http://127.0.0.1')
    if (url.pathname !== (this.#options.provider === 'codex' ? '/auth/callback' : '/oauth-callback')) { fail(404, 'Callback not found'); return }
    if (url.searchParams.getAll('state').length !== 1 || url.searchParams.get('state') !== this.#options.login.state) { fail(400, 'Invalid sign-in state'); return }
    if (this.#outcome || this.#accepted) { fail(409, 'Sign-in callback was already handled or expired'); return }
    const code = url.searchParams.get('code'), error = url.searchParams.get('error')
    if ((!code && !error) || (code && error) || url.searchParams.getAll('code').length > 1 || url.searchParams.getAll('error').length > 1) { fail(400, 'Invalid callback parameters'); return }
    this.#accepted = true
    try {
      await this.#options.client.callback(this.#options.provider, this.#options.login.state, code ? { code } : { error: error! }, this.#abort.signal)
      const outcome = await this.completion
      if (outcome.status === 'ok') callbackPage(response, 200, true, locale)
      else fail(outcome.status === 'cancelled' || outcome.status === 'expired' ? 410 : 400, outcome.reason)
    } catch {
      if (!this.#outcome) this.#finish({ status: 'error', reason: locale === 'en' ? 'Sign-in callback could not be completed' : '登录回调未能完成' })
      const outcome = await this.completion
      fail(outcome.status === 'cancelled' || outcome.status === 'expired' ? 410 : 400, outcome.status !== 'ok' ? outcome.reason : 'Sign-in callback could not be completed')
    }
  }
  async #poll(): Promise<void> {
    try {
      while (!this.#outcome) {
        const state = await this.#options.client.oauthStatus(this.#options.login.state, this.#abort.signal)
        if (state.status === 'ok' && state.auth_id) { this.#finish({ status: 'ok', authId: state.auth_id }); return }
        if (state.status === 'error') { this.#finish({ status: 'error', reason: sanitizeProviderReason(state.error) ?? 'Sign-in was not completed' }); return }
        await delay(100, undefined, { signal: this.#abort.signal })
      }
    } catch {
      if (!this.#outcome) this.#finish({ status: 'error', reason: 'Model router sign-in status is unavailable' })
    }
  }
  #finish(outcome: OAuthOutcome): void {
    if (this.#outcome) return
    this.#outcome = outcome
    if (this.#timer) clearTimeout(this.#timer)
    this.#resolve(outcome)
    this.#abort.abort()
    void this.close()
  }
  async cancel(status: 'cancelled' | 'expired' = 'cancelled'): Promise<void> {
    // Finish locally first so an in-flight browser callback cannot hang close().
    if (!this.#outcome) this.#finish({ status, reason: status === 'expired' ? (this.#options.locale === 'en' ? 'Sign-in timed out' : '登录已超时') : (this.#options.locale === 'en' ? 'Sign-in cancelled' : '登录已取消') })
    await this.#options.client.cancelOAuth(this.#options.login.state).catch(() => undefined)
    await this.close()
  }
  close(): Promise<void> {
    this.#closing ??= Promise.all(this.#servers.map(server => new Promise<void>(done => {
      server.close(() => done())
      server.closeIdleConnections()
    }))).then(() => undefined)
    return this.#closing
  }
}

export function callbackPage(response: ServerResponse, status: number, success: boolean, locale: Locale, reason?: string): void {
  const title = success ? (locale === 'en' ? 'Sign-in complete. You can return to Matou.' : '登录完成，可以回到 Matou 了') : (locale === 'en' ? 'Sign-in was not completed' : '登录未完成')
  const escape = (value: string) => value.replace(/[&<>"']/g, character => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[character]!))
  const safeReason = sanitizeProviderReason(reason)
  response.writeHead(status, { 'Content-Type': 'text/html; charset=utf-8', 'Cache-Control': 'no-store', 'Referrer-Policy': 'no-referrer', 'Content-Security-Policy': "default-src 'none'; style-src 'unsafe-inline'", Connection: 'close' })
  response.end(`<!doctype html><html lang="${locale === 'en' ? 'en' : 'zh-CN'}"><meta charset="utf-8"><title>${escape(title)}</title><body><h1>${escape(title)}</h1>${!success && safeReason ? `<p>${escape(safeReason)}</p>` : ''}</body></html>`)
}
