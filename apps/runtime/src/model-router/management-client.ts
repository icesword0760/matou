import type { SubscriptionProvider } from '@matou/contracts'
import type { RouterConnection } from './manager'
import type { ModelAlias } from './claude-launch'
import { MAX_MODEL_BODY_BYTES } from './account-readers'

export interface RouterCredential extends Record<string, unknown> {
  id: string
  name: string
  provider: SubscriptionProvider
  auth_index: string
  disabled?: boolean
  status?: string
}
export interface OAuthStatus { status: 'wait' | 'ok' | 'error'; auth_id?: string; error?: string }
export interface OAuthStart { url: string; state: string }
export type AccountProbe = 'chatgpt-usage' | 'chatgpt-models' | 'antigravity-account' | 'antigravity-quota' | 'antigravity-models'
export class ManagementError extends Error {
  constructor(readonly status: number) { super(`Model router management request failed (${status})`) }
}

/** Private fixed operations, not a renderer-controlled management proxy.
 * No credential-download endpoint, tokens, URLs or headers are exposed via RPC.
 */
export class RouterManagementClient {
  constructor(readonly connection: RouterConnection) {
    const url = new URL(connection.baseUrl)
    if (url.protocol !== 'http:' || url.hostname !== '127.0.0.1' || url.username || url.password || url.search || url.hash || url.pathname !== '/') throw new Error('Invalid model router address')
  }
  async #request(path: string, method = 'GET', body?: unknown, signal?: AbortSignal, maximum = 1024 * 1024): Promise<unknown> {
    const response = await fetch(`${this.connection.baseUrl}/v8/management${path}`, {
      method, headers: { Authorization: `Bearer ${this.connection.managementKey}`, 'Content-Type': 'application/json' },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }),
      signal: AbortSignal.any([AbortSignal.timeout(15_000), ...(signal ? [signal] : [])])
    })
    if (!response.ok) { await response.body?.cancel(); throw new ManagementError(response.status) }
    if (!response.body) throw new Error('Empty model router response')
    const reader = response.body.getReader()
    const chunks: Uint8Array[] = []
    let size = 0
    try {
      while (true) {
        const result = await reader.read()
        if (result.done) break
        size += result.value.byteLength
        if (size > maximum) throw new Error('Model router response is too large')
        chunks.push(result.value)
      }
    } finally { await reader.cancel().catch(() => undefined) }
    try { return JSON.parse(Buffer.concat(chunks).toString('utf8')) as unknown }
    catch { throw new Error('Invalid model router response') }
  }
  async credentials(): Promise<RouterCredential[]> {
    const body = await this.#request('/credentials') as { files?: RouterCredential[] }
    if (!Array.isArray(body.files)) throw new Error('Invalid credential list')
    return body.files.filter(file => file && typeof file.name === 'string' && typeof file.id === 'string' && typeof file.auth_index === 'string' && ['codex', 'antigravity'].includes(file.provider))
  }
  async models(name: string): Promise<Array<Record<string, unknown>>> {
    const body = await this.#request(`/credentials/models?${new URLSearchParams({ name })}`, 'GET', undefined, undefined, MAX_MODEL_BODY_BYTES) as { models?: Array<Record<string, unknown>> }
    return Array.isArray(body.models) ? body.models : []
  }
  async modelDefinitions(provider: SubscriptionProvider): Promise<Array<Record<string, unknown>>> {
    const body = await this.#request(`/routing/model-definitions/${provider}`, 'GET', undefined, undefined, MAX_MODEL_BODY_BYTES) as { models?: Array<Record<string, unknown>> }
    return Array.isArray(body.models) ? body.models : []
  }
  async setDisabled(name: string, disabled: boolean): Promise<void> { await this.#request('/credentials/status', 'PATCH', { name, disabled }) }
  async setPrefix(name: string, prefix: string): Promise<void> { await this.#request('/credentials/fields', 'PATCH', { name, prefix }) }
  async deleteCredential(name: string): Promise<void> { await this.#request(`/credentials?${new URLSearchParams({ name })}`, 'DELETE') }
  async aliases(provider: SubscriptionProvider): Promise<ModelAlias[]> {
    try {
      const body = await this.#request(`/config/oauth/model-alias/${provider}`)
      return Array.isArray(body) ? body as ModelAlias[] : []
    } catch (error) { if (error instanceof ManagementError && error.status === 404) return []; throw error }
  }
  async setAliases(provider: SubscriptionProvider, aliases: ModelAlias[]): Promise<void> { await this.#request(`/config/oauth/model-alias/${provider}`, 'PUT', aliases) }
  async startOAuth(provider: SubscriptionProvider): Promise<OAuthStart> {
    const body = await this.#request(`/oauth/auth-url?${new URLSearchParams({ provider })}`) as OAuthStart
    if (typeof body.url !== 'string' || typeof body.state !== 'string' || !body.state) throw new Error('Invalid sign-in response')
    return { url: body.url, state: body.state }
  }
  async oauthStatus(state: string, signal?: AbortSignal): Promise<OAuthStatus> { return await this.#request(`/oauth/status?${new URLSearchParams({ state })}`, 'GET', undefined, signal) as OAuthStatus }
  async cancelOAuth(state: string): Promise<boolean> {
    const body = await this.#request(`/oauth/session?${new URLSearchParams({ state })}`, 'DELETE') as { cancelled?: boolean }
    return body.cancelled === true
  }
  async callback(provider: SubscriptionProvider, state: string, result: { code?: string; error?: string }, signal?: AbortSignal): Promise<void> {
    await this.#request('/oauth/callback', 'POST', { provider, state, ...result }, signal)
  }
  /** Fixed service URLs. The caller selects a probe; it cannot supply an URL/header. */
  async apiCall(probe: AccountProbe, credential: RouterCredential, project?: string): Promise<unknown> {
    const googleBase = 'https://daily-cloudcode-pa.googleapis.com/v1internal:'
    const specs = {
      'chatgpt-usage': { method: 'GET', url: 'https://chatgpt.com/backend-api/wham/usage' },
      'chatgpt-models': { method: 'GET', url: 'https://chatgpt.com/backend-api/codex/models?client_version=0.159.0' },
      'antigravity-account': { method: 'POST', url: `${googleBase}loadCodeAssist` },
      'antigravity-quota': { method: 'POST', url: `${googleBase}retrieveUserQuotaSummary` },
      'antigravity-models': { method: 'POST', url: `${googleBase}fetchAvailableModels` }
    } as const
    if ((probe.startsWith('chatgpt') ? 'codex' : 'antigravity') !== credential.provider) throw new Error('Probe does not match account')
    const accountId = typeof credential.account_id === 'string' ? credential.account_id : typeof (credential.id_token as Record<string, unknown> | undefined)?.chatgpt_account_id === 'string' ? (credential.id_token as Record<string, string>).chatgpt_account_id : undefined
    const header: Record<string, string> = { Authorization: 'Bearer $TOKEN$', 'Content-Type': 'application/json' }
    if (probe === 'chatgpt-models') { header.originator = 'codex_cli_rs'; header.version = '0.159.0' }
    if (credential.provider === 'codex' && accountId) header['ChatGPT-Account-Id'] = accountId
    if (credential.provider === 'antigravity') header['User-Agent'] = 'Matou (antigravity/cli/1.4.6)'
    const data = probe === 'antigravity-account' ? { metadata: { ideType: 'ANTIGRAVITY' } } : { ...(project ? { project } : {}) }
    // api-call wraps the provider's JSON text in another JSON string. Quotes
    // and backslashes may double its bytes; the decoded model body remains 4MiB.
    const maximum = probe === 'chatgpt-models' || probe === 'antigravity-models' ? MAX_MODEL_BODY_BYTES * 2 + 64 * 1024 : 1024 * 1024
    return this.#request('/requests/api-call', 'POST', { ...specs[probe], auth_index: credential.auth_index, header, ...(specs[probe].method === 'POST' ? { data: JSON.stringify(data) } : {}) }, undefined, maximum)
  }
}
