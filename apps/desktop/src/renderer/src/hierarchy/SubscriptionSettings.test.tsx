// @vitest-environment jsdom
import { act, cleanup, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { ProviderActivationFailure, ProviderConfigSnapshot, ProviderConfigView, SubscriptionAccountDetails, SubscriptionLoginView } from '@matou/contracts'
import { ModelSwitchSettings, type ProviderConfigClient } from './ModelSwitchSettings'
import { LocaleProvider } from '../i18n/LocaleProvider'
import { setCurrentLocale } from '../i18n/current'
import { subscriptionDefaults, subscriptionQuotaText, subscriptionQuotaTitle } from './subscription-settings'

afterEach(() => { cleanup(); vi.restoreAllMocks(); setCurrentLocale('zh-CN') })
type LocalizedText = { zh: string; en: string }
type LocalizedLoginView = SubscriptionLoginView & { reasonLocalized?: LocalizedText }
const details: SubscriptionAccountDetails = { account: { email: 'a@b.com', plan: 'Plus', status: 'ready' }, models: [
  { id: 'gpt-image-2' }, { id: 'gpt-video-1' }, { id: 'codex-auto-review' }, { id: 'gpt-6-luna', displayName: 'GPT-6 Luna' }, { id: 'gpt-6-astra', displayName: 'GPT-6 Astra' }, { id: 'gpt-6.1-sol', displayName: 'GPT-6.1 Sol' }
], quota: [{ name: 'five_hour', usedPercent: 12 }, { name: 'weekly', usedPercent: 34 }] }
const emptyDetails: SubscriptionAccountDetails = { ...details, models: [] }
function subscription(status: SubscriptionAccountDetails['account']['status'] = 'ready'): ProviderConfigView {
  return { id: 'chatgpt', cli: 'claude-code', kind: 'subscription', name: 'ChatGPT · a@b.com', endpoint: '', model: 'gpt-6.1-sol', hasApiKey: false, modelMapping: { opus: 'gpt-6-astra', sonnet: 'gpt-6.1-sol', haiku: 'gpt-6-luna' }, subscription: { provider: 'codex', account: { ...details.account, status, reason: '服务方拒绝此账号' }, modelMetadata: details.models } }
}
function fakeClient(rows: ProviderConfigView[] = [], accepted = false, activeId = 'official', readAccounts?: Record<string, SubscriptionAccountDetails>, subscriptionSupported?: boolean) {
  let state: ProviderConfigSnapshot = { revision: 1, activeProviderIds: { 'claude-code': activeId, codex: 'openai' }, providers: { 'claude-code': [{ id: 'official', cli: 'claude-code', builtIn: true, name: 'Anthropic 官方', endpoint: 'https://api.anthropic.com', model: 'opus', hasApiKey: false }, ...rows], codex: [] } }
  let listener: (() => void) | undefined
  const request = vi.fn<ProviderConfigClient['request']>(async (method, payload) => {
    if (method === 'provider-config.snapshot') return state
    if (method === 'provider-config.subscription.settings-read') return { snapshot: subscriptionSupported === undefined ? state : { ...state, subscriptionSupported }, accounts: readAccounts ?? Object.fromEntries(rows.map(row => [row.id, details])), antigravityRiskAccepted: accepted }
    if (method === 'provider-config.subscription.login-start' || method === 'provider-config.subscription.login-status') return { attemptId: 'attempt', provider: 'codex', status: 'ready', details } satisfies SubscriptionLoginView
    if (method === 'provider-config.subscription.login-cancel') return { attemptId: 'attempt', provider: 'codex', status: 'cancelled', reason: '登录已取消（runtime）' }
    if (method === 'provider-config.subscription.risk-confirm') return { accepted: true }
    if (method === 'provider-config.upsert') {
      const input = (payload as { provider: ProviderConfigView }).provider
      const row = { ...subscription(), ...input, subscription: subscription().subscription! }
      state = { ...state, revision: state.revision + 1, providers: { ...state.providers, 'claude-code': [...state.providers['claude-code'].filter(p => p.id !== row.id), row] } }
      return { provider: row }
    }
    if (method === 'provider-config.activate') { state = { ...state, revision: state.revision + 1, activeProviderIds: { ...state.activeProviderIds, 'claude-code': (payload as { providerId: string }).providerId } }; return { ...state, sessionTransitions: [] } }
    return {}
  })
  return { request, subscribeProviderConfig: (callback: () => void) => { listener = callback; return () => { listener = undefined } }, changed: () => listener?.(), setRows: (next: ProviderConfigView[]) => { state = { ...state, revision: state.revision + 1, providers: { ...state.providers, 'claude-code': [state.providers['claude-code'][0]!, ...next] } } } }
}
async function openAccount(user: ReturnType<typeof userEvent.setup>, kind = 'ChatGPT', locale: 'zh-CN' | 'en' = 'zh-CN') {
  await screen.findByText('Anthropic 官方')
  await user.click(screen.getByRole('button', { name: locale === 'zh-CN' ? '新增供应商' : 'New provider' }))
  await user.click(screen.getByRole('button', { name: new RegExp(`${kind} ${locale === 'zh-CN' ? '账号' : 'account'}`) }))
  await user.click(screen.getByRole('button', { name: locale === 'zh-CN' ? '下一步' : 'Next' }))
}
async function openProviderPicker(user: ReturnType<typeof userEvent.setup>, locale: 'zh-CN' | 'en' = 'zh-CN') {
  await screen.findByText('Anthropic 官方')
  await user.click(screen.getByRole('button', { name: locale === 'zh-CN' ? '新增供应商' : 'New provider' }))
}

describe('subscription settings', () => {
  it('adds subscription groups only to Claude Code, then signs in and saves with real model choices', async () => {
    const client = fakeClient(), user = userEvent.setup()
    render(<ModelSwitchSettings client={client} onClose={vi.fn()} />)
    await openAccount(user)
    await user.click(screen.getByRole('button', { name: '在浏览器中登录' }))
    const dialog = await screen.findByRole('dialog')
    expect(within(dialog).queryByLabelText('API Key')).toBeNull()
    expect(within(dialog).queryByLabelText('API 地址')).toBeNull()
    expect(within(dialog).getByLabelText('供应商名称')).toHaveProperty('value', 'ChatGPT')
    await waitFor(() => expect(document.activeElement).toBe(within(dialog).getByLabelText('供应商名称')))
    expect(screen.getAllByRole('combobox', { name: /模型|model/ }).map(select => (select as HTMLSelectElement).value)).toEqual(['gpt-6-astra', 'gpt-6.1-sol', 'gpt-6-luna'])
    expect(screen.queryByRole('option', { name: 'gpt-image-2' })).toBeNull()
    expect(screen.queryByRole('option', { name: 'gpt-video-1' })).toBeNull()
    expect(screen.queryByRole('option', { name: 'codex-auto-review' })).toBeNull()
    expect(screen.getAllByRole('option', { name: 'GPT-6 Astra' })).toHaveLength(3)
    expect(screen.getAllByRole('option', { name: 'GPT-6.1 Sol' })).toHaveLength(3)
    expect(screen.getAllByRole('option', { name: 'GPT-6 Luna' })).toHaveLength(3)
    await user.click(screen.getByRole('button', { name: '保存并切换' }))
    await screen.findByText('a@b.com · Plus')
    expect(screen.getByText('5 小时 12% · 每周 34%')).toBeTruthy()
    const savedCard = within(screen.getByText('a@b.com · Plus').closest('article')!)
    expect(savedCard.getByText('ChatGPT')).toBeTruthy()
    expect(savedCard.getByText('GPT-6 Astra')).toBeTruthy()
    expect(savedCard.getByText('默认模型')).toBeTruthy()
    expect(savedCard.queryByText('GPT-6.1 Sol')).toBeNull()
    expect(client.request).toHaveBeenCalledWith('provider-config.upsert', { provider: expect.objectContaining({ kind: 'subscription', name: 'ChatGPT', subscription: { provider: 'codex', attemptId: 'attempt' } }) })
    expect(client.request).toHaveBeenCalledWith('provider-config.activate', { cli: 'claude-code', providerId: 'chatgpt' })
    expect(screen.queryByRole('tab', { name: 'Codex' })).toBeNull()
  })
  it.each(['zh-CN', 'en'] as const)('disables both subscription options and keeps API providers selectable below macOS 15 in %s', async locale => {
    const zh = locale === 'zh-CN'
    const copy = zh
      ? { codex: /ChatGPT 账号/, antigravity: /Antigravity 账号/, note: '需要 macOS 15 或更高版本', next: '下一步', custom: /自定义服务/, signIn: '在浏览器中登录' }
      : { codex: /ChatGPT account/, antigravity: /Antigravity account/, note: 'Requires macOS 15 or later', next: 'Next', custom: /Custom service/, signIn: 'Sign in with browser' }
    const client = fakeClient([], false, 'official', undefined, false), user = userEvent.setup()
    render(<LocaleProvider initialLocale={locale}><ModelSwitchSettings client={client} onClose={vi.fn()} /></LocaleProvider>)
    await openProviderPicker(user, locale)
    expect(await screen.findByText(copy.note)).toBeTruthy()
    const codex = screen.getByRole('button', { name: copy.codex })
    const antigravity = screen.getByRole('button', { name: copy.antigravity })
    expect(codex).toHaveProperty('disabled', true)
    expect(antigravity).toHaveProperty('disabled', true)
    await user.click(codex); await user.click(antigravity)
    expect(codex.getAttribute('aria-pressed')).toBe('false')
    expect(antigravity.getAttribute('aria-pressed')).toBe('false')
    const next = screen.getByRole('button', { name: copy.next })
    expect(next).toHaveProperty('disabled', true)
    await user.click(next)
    expect(screen.queryByText(copy.signIn)).toBeNull()
    const custom = screen.getByRole('button', { name: copy.custom })
    await user.click(custom)
    expect(custom.getAttribute('aria-pressed')).toBe('true')
    expect(next).toHaveProperty('disabled', false)
    await user.click(next)
    expect(screen.getByLabelText('API Key')).toBeTruthy()
    expect(client.request.mock.calls.some(([method]) => method === 'provider-config.subscription.login-start')).toBe(false)
  })
  it.each(['zh-CN', 'en'] as const)('keeps existing subscription cards but disables switching and re-sign-in below macOS 15 in %s', async locale => {
    const zh = locale === 'zh-CN'
    const unavailable: ProviderConfigView = { ...subscription('account-unavailable'), id: 'chatgpt', name: 'ChatGPT', subscription: { provider: 'codex', account: { email: 'used@example.com', plan: 'Pro', status: 'account-unavailable', reason: '服务方拒绝此账号' }, modelMetadata: details.models } }
    const needsSignIn: ProviderConfigView = { ...subscription('needs-sign-in'), id: 'antigravity', name: 'Antigravity', subscription: { provider: 'antigravity', account: { email: 'old@example.com', plan: 'Plus', status: 'needs-sign-in' }, modelMetadata: details.models } }
    const client = fakeClient([unavailable, needsSignIn], false, 'chatgpt', undefined, false), user = userEvent.setup()
    render(<LocaleProvider initialLocale={locale}><ModelSwitchSettings client={client} onClose={vi.fn()} /></LocaleProvider>)
    const activeCard = (await screen.findByText('used@example.com · Pro')).closest('article')!
    const signInCard = (await screen.findByText('old@example.com · Plus')).closest('article')!
    expect(within(activeCard).getByText('GPT-6 Astra')).toBeTruthy()
    expect(within(signInCard).getByText('GPT-6 Astra')).toBeTruthy()
    expect(signInCard.querySelector('.model-provider__status')?.textContent).toBe(zh ? '需要重新登录' : 'Sign in again')
    const relogin = within(signInCard).getByRole('button', { name: zh ? '重新登录 Antigravity' : 'Sign in again for Antigravity' })
    await waitFor(() => expect(relogin).toHaveProperty('disabled', true))
    expect(within(signInCard).getByRole('button', { name: zh ? '编辑' : 'Edit' })).toHaveProperty('disabled', false)
    const officialLink = within(activeCard).getByRole('button', { name: zh ? '切回 Anthropic 官方' : 'Switch back to Anthropic official' })
    expect(officialLink).toHaveProperty('disabled', false)
    await user.click(officialLink)
    expect(client.request).toHaveBeenCalledWith('provider-config.activate', { cli: 'claude-code', providerId: 'official' })
    await waitFor(() => expect(screen.getByText('Anthropic 官方').closest('article')?.classList.contains('is-current')).toBe(true))
    // The activation result is a storage-only snapshot without the capability;
    // the remembered false must survive it.
    await user.click(screen.getByRole('button', { name: zh ? '新增供应商' : 'New provider' }))
    expect(await screen.findByText(zh ? '需要 macOS 15 或更高版本' : 'Requires macOS 15 or later')).toBeTruthy()
    expect(screen.getByRole('button', { name: zh ? /ChatGPT 账号/ : /ChatGPT account/ })).toHaveProperty('disabled', true)
  })
  it.each(['zh-CN', 'en'] as const)('disables an inactive subscription switch but keeps its card, models and edit below macOS 15 in %s', async locale => {
    const zh = locale === 'zh-CN'
    const client = fakeClient([subscription()], false, 'official', undefined, false), user = userEvent.setup()
    render(<LocaleProvider initialLocale={locale}><ModelSwitchSettings client={client} onClose={vi.fn()} /></LocaleProvider>)
    const card = (await screen.findByText('a@b.com · Plus')).closest('article')!
    const switchButton = within(card).getByRole('button', { name: zh ? '切换到 ChatGPT · a@b.com' : 'Switch to ChatGPT · a@b.com' })
    await waitFor(() => expect(switchButton).toHaveProperty('disabled', true))
    expect(within(card).getByText('GPT-6 Astra')).toBeTruthy()
    const edit = within(card).getByRole('button', { name: zh ? '编辑' : 'Edit' })
    expect(edit).toHaveProperty('disabled', false)
    await user.click(switchButton)
    expect(client.request.mock.calls.some(([method]) => method === 'provider-config.activate')).toBe(false)
    await user.click(edit)
    expect(screen.getAllByRole('combobox', { name: /模型|model/ })).toHaveLength(3)
    expect(screen.queryByLabelText('API Key')).toBeNull()
  })
  it('keeps subscription options enabled and hides the requirement when the runtime supports them', async () => {
    const client = fakeClient([], false, 'official', undefined, true), user = userEvent.setup()
    render(<ModelSwitchSettings client={client} onClose={vi.fn()} />)
    await openProviderPicker(user)
    expect(screen.queryByText('需要 macOS 15 或更高版本')).toBeNull()
    const option = screen.getByRole('button', { name: /ChatGPT 账号/ })
    expect(option).toHaveProperty('disabled', false)
    await user.click(option)
    const next = screen.getByRole('button', { name: '下一步' })
    expect(next).toHaveProperty('disabled', false)
    await user.click(next)
    expect(await screen.findByText('在浏览器中登录')).toBeTruthy()
  })
  it.each(['zh-CN', 'en'] as const)('shows a retryable empty-model state, disables both saves and rereads models only in %s', async locale => {
    const copy = locale === 'zh-CN'
      ? { empty: '没有读到可用的模型', retry: '重试', save: '保存', saveSwitch: '保存并切换', signIn: '在浏览器中登录' }
      : { empty: 'No available models were found', retry: 'Retry', save: 'Save', saveSwitch: 'Save and switch', signIn: 'Sign in with browser' }
    const client = fakeClient(), user = userEvent.setup(), base = client.request.getMockImplementation()!
    client.request.mockImplementation((method, payload) => {
      if (method === 'provider-config.subscription.login-start') return Promise.resolve({ attemptId: 'attempt', provider: 'codex', status: 'ready', details: emptyDetails } satisfies SubscriptionLoginView)
      if (method === 'provider-config.subscription.models-read') return Promise.resolve(details)
      return base(method, payload)
    })
    render(<LocaleProvider initialLocale={locale}><ModelSwitchSettings client={client} onClose={vi.fn()} /></LocaleProvider>)
    await openAccount(user, 'ChatGPT', locale)
    await user.click(screen.getByRole('button', { name: copy.signIn }))
    expect(await screen.findByText(copy.empty)).toBeTruthy()
    expect(screen.queryAllByRole('combobox')).toHaveLength(0)
    expect(screen.getByRole('button', { name: copy.retry })).toHaveProperty('disabled', false)
    expect(screen.getByRole('button', { name: copy.save })).toHaveProperty('disabled', true)
    expect(screen.getByRole('button', { name: copy.saveSwitch })).toHaveProperty('disabled', true)
    await user.click(screen.getByRole('button', { name: copy.retry }))
    await waitFor(() => expect(screen.getAllByRole('combobox', { name: /模型|model/ })).toHaveLength(3))
    expect(screen.getAllByRole('combobox', { name: /模型|model/ }).map(select => (select as HTMLSelectElement).value)).toEqual(['gpt-6-astra', 'gpt-6.1-sol', 'gpt-6-luna'])
    expect(screen.getByRole('button', { name: copy.save })).toHaveProperty('disabled', false)
    expect(screen.getByRole('button', { name: copy.saveSwitch })).toHaveProperty('disabled', false)
    expect(client.request.mock.calls.filter(([method]) => method === 'provider-config.subscription.models-read')).toEqual([['provider-config.subscription.models-read', { attemptId: 'attempt' }]])
    expect(client.request.mock.calls.filter(([method]) => method === 'provider-config.subscription.login-start')).toHaveLength(1)
    expect(client.request.mock.calls.some(([method]) => method === 'provider-config.subscription.login-cancel')).toBe(false)
    expect(client.request.mock.calls.some(([method]) => method === 'provider-config.upsert')).toBe(false)
    expect(client.request.mock.calls.some(([method]) => method === 'provider-config.activate')).toBe(false)
  })
  it.each(['zh-CN', 'en'] as const)('keeps retry available when a reread stays empty or is rejected in %s', async locale => {
    const copy = locale === 'zh-CN'
      ? { empty: '没有读到可用的模型', retry: '重试', save: '保存', saveSwitch: '保存并切换', signIn: '在浏览器中登录', failure: '模型列表未能读取，请重试。' }
      : { empty: 'No available models were found', retry: 'Retry', save: 'Save', saveSwitch: 'Save and switch', signIn: 'Sign in with browser', failure: 'The model list could not be read. Try again.' }
    const client = fakeClient(), user = userEvent.setup(), base = client.request.getMockImplementation()!
    let reads = 0
    client.request.mockImplementation((method, payload) => {
      if (method === 'provider-config.subscription.login-start') return Promise.resolve({ attemptId: 'attempt', provider: 'codex', status: 'ready', details: emptyDetails } satisfies SubscriptionLoginView)
      if (method === 'provider-config.subscription.models-read') {
        reads += 1
        if (reads === 1) return Promise.resolve(emptyDetails)
        if (reads === 2) return Promise.reject(Object.assign(new Error('raw model read failure'), { messageLocalized: { zh: '模型列表未能读取，请重试。', en: 'The model list could not be read. Try again.' } }))
        return Promise.resolve(details)
      }
      return base(method, payload)
    })
    render(<LocaleProvider initialLocale={locale}><ModelSwitchSettings client={client} onClose={vi.fn()} /></LocaleProvider>)
    await openAccount(user, 'ChatGPT', locale)
    await user.click(screen.getByRole('button', { name: copy.signIn }))
    expect(await screen.findByText(copy.empty)).toBeTruthy()
    await user.click(screen.getByRole('button', { name: copy.retry }))
    await waitFor(() => expect(reads).toBe(1))
    expect(screen.getByText(copy.empty)).toBeTruthy()
    expect(screen.getByRole('button', { name: copy.retry })).toHaveProperty('disabled', false)
    expect(screen.queryAllByRole('combobox')).toHaveLength(0)
    await user.click(screen.getByRole('button', { name: copy.retry }))
    expect(await screen.findByText(copy.failure)).toBeTruthy()
    expect(screen.getByText(copy.empty)).toBeTruthy()
    expect(screen.queryAllByRole('combobox')).toHaveLength(0)
    expect(screen.getByRole('button', { name: copy.retry })).toHaveProperty('disabled', false)
    expect(screen.getByRole('button', { name: copy.save })).toHaveProperty('disabled', true)
    expect(screen.getByRole('button', { name: copy.saveSwitch })).toHaveProperty('disabled', true)
    await user.click(screen.getByRole('button', { name: copy.retry }))
    await waitFor(() => expect(screen.getAllByRole('combobox', { name: /模型|model/ })).toHaveLength(3))
    expect(reads).toBe(3)
    expect(client.request.mock.calls.filter(([method]) => method === 'provider-config.subscription.models-read')).toEqual([
      ['provider-config.subscription.models-read', { attemptId: 'attempt' }],
      ['provider-config.subscription.models-read', { attemptId: 'attempt' }],
      ['provider-config.subscription.models-read', { attemptId: 'attempt' }]
    ])
    expect(client.request.mock.calls.filter(([method]) => method === 'provider-config.subscription.login-start')).toHaveLength(1)
    expect(client.request.mock.calls.some(([method]) => method === 'provider-config.subscription.login-cancel')).toBe(false)
    expect(client.request.mock.calls.some(([method]) => method === 'provider-config.upsert')).toBe(false)
  })
  it('rereads a saved account by providerId when its metadata is empty, preserving its name', async () => {
    const row = { ...subscription(), subscription: { ...subscription().subscription!, modelMetadata: [] } }
    const client = fakeClient([row], false, 'official', { chatgpt: emptyDetails }), user = userEvent.setup(), base = client.request.getMockImplementation()!
    client.request.mockImplementation((method, payload) => method === 'provider-config.subscription.models-read' ? Promise.resolve(details) : base(method, payload))
    render(<ModelSwitchSettings client={client} onClose={vi.fn()} />)
    await screen.findByText('a@b.com · Plus')
    await user.click(within(screen.getByText('a@b.com · Plus').closest('article')!).getByRole('button', { name: '编辑' }))
    expect(screen.getByText('没有读到可用的模型')).toBeTruthy()
    expect(screen.queryAllByRole('combobox')).toHaveLength(0)
    expect(screen.getByRole('button', { name: '保存' })).toHaveProperty('disabled', true)
    expect(screen.getByRole('button', { name: '保存并切换' })).toHaveProperty('disabled', true)
    await user.click(screen.getByRole('button', { name: '重试' }))
    await waitFor(() => expect(screen.getAllByRole('combobox', { name: /模型|model/ })).toHaveLength(3))
    expect(screen.getAllByRole('combobox', { name: /模型|model/ }).map(select => (select as HTMLSelectElement).value)).toEqual(['gpt-6-astra', 'gpt-6.1-sol', 'gpt-6-luna'])
    expect(screen.getByLabelText('供应商名称')).toHaveProperty('value', 'ChatGPT · a@b.com')
    expect(client.request.mock.calls.filter(([method]) => method === 'provider-config.subscription.models-read')).toEqual([['provider-config.subscription.models-read', { providerId: 'chatgpt' }]])
    expect(client.request.mock.calls.some(([method]) => method === 'provider-config.subscription.login-start')).toBe(false)
    expect(client.request.mock.calls.some(([method]) => method === 'provider-config.upsert')).toBe(false)
  })
  it('requires the exact Antigravity risk confirmation and remembers it in the same open settings', async () => {
    const client = fakeClient(), user = userEvent.setup()
    render(<ModelSwitchSettings client={client} onClose={vi.fn()} />)
    await openAccount(user, 'Antigravity')
    expect(screen.getByText('使用 Antigravity 账号有封号风险')).toBeTruthy()
    expect(screen.getByText('Google 的 Antigravity 条款不允许第三方工具使用该账号。被判定违规时，Antigravity 与 Gemini CLI 账号可能被停用，申诉机会有限。')).toBeTruthy()
    expect(screen.getByRole('button', { name: '继续' })).toHaveProperty('disabled', true)
    await user.click(screen.getByRole('checkbox', { name: '我了解风险，仍要继续' }))
    await user.click(screen.getByRole('button', { name: '继续' }))
    expect(client.request).toHaveBeenCalledWith('provider-config.subscription.risk-confirm', { accepted: true })
    await user.keyboard('{Escape}')
    await openAccount(user, 'Antigravity')
    expect(screen.getByRole('button', { name: '在浏览器中登录' })).toBeTruthy()
    expect(screen.queryByRole('checkbox')).toBeNull()
  })
  it('cancels a pending start on Escape, discards its late response and never creates a card', async () => {
    const client = fakeClient(), user = userEvent.setup()
    const base = client.request.getMockImplementation()!
    let resolve!: (view: SubscriptionLoginView) => void
    client.request.mockImplementation((method, payload) => method === 'provider-config.subscription.login-start' ? new Promise(yes => { resolve = yes }) : base(method, payload))
    render(<ModelSwitchSettings client={client} onClose={vi.fn()} />)
    await openAccount(user)
    await user.dblClick(screen.getByRole('button', { name: '在浏览器中登录' }))
    expect(screen.getByText('等待浏览器完成登录…')).toBeTruthy()
    await user.keyboard('{Escape}')
    expect(screen.queryByRole('dialog')).toBeNull()
    await act(async () => resolve({ attemptId: 'late', provider: 'codex', status: 'waiting' }))
    expect(client.request).toHaveBeenCalledWith('provider-config.subscription.login-cancel', { attemptId: 'late' })
    expect(client.request.mock.calls.filter(([m]) => m.endsWith('login-start'))).toHaveLength(1)
    expect(client.request.mock.calls.some(([m]) => m === 'provider-config.upsert')).toBe(false)
  })
  it.each(['登录超时（runtime）', '端口 1455 已被占用（runtime）', '用户拒绝授权（runtime）'])('shows runtime login failure %s and retry without a card', async reason => {
    const client = fakeClient(), user = userEvent.setup(), base = client.request.getMockImplementation()!
    client.request.mockImplementation((method, payload) => method === 'provider-config.subscription.login-start' ? Promise.resolve({ attemptId: 'failed', provider: 'codex', status: 'failed', reason }) : base(method, payload))
    render(<ModelSwitchSettings client={client} onClose={vi.fn()} />)
    await openAccount(user); await user.click(screen.getByRole('button', { name: '在浏览器中登录' }))
    expect(await screen.findByText(reason)).toBeTruthy()
    expect(screen.getByRole('button', { name: '重试' })).toBeTruthy()
    expect(screen.queryByText('a@b.com · Plus')).toBeNull()
  })
  it.each(['zh-CN', 'en'] as const)('shows a localized immediate login-start rejection and retries once into model setup in %s', async locale => {
    const client = fakeClient(), user = userEvent.setup(), base = client.request.getMockImplementation()!
    const localized: LocalizedText = { zh: '模型路由未能启动：本地模型路由暂时无法使用，请重试。', en: 'Model router failed to start: The local model router is temporarily unavailable. Try again.' }
    let starts = 0
    client.request.mockImplementation((method, payload) => {
      if (method !== 'provider-config.subscription.login-start') return base(method, payload)
      starts += 1
      if (starts === 1) return Promise.reject(Object.assign(new Error('Model router failed to start'), { messageLocalized: localized }))
      return Promise.resolve({ attemptId: 'retry-attempt', provider: 'codex', status: 'ready', details } satisfies SubscriptionLoginView)
    })
    render(<LocaleProvider initialLocale={locale}><ModelSwitchSettings client={client} onClose={vi.fn()} /></LocaleProvider>)
    await openAccount(user, 'ChatGPT', locale)
    await user.click(screen.getByRole('button', { name: locale === 'zh-CN' ? '在浏览器中登录' : 'Sign in with browser' }))
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toBe(localized[locale === 'zh-CN' ? 'zh' : 'en'])
    expect(alert.textContent).not.toBe('Model router failed to start')
    const retry = screen.getByRole('button', { name: locale === 'zh-CN' ? '重试' : 'Retry' })
    expect(retry).toHaveProperty('disabled', false)
    await user.click(retry)
    expect(await screen.findByLabelText(locale === 'zh-CN' ? '供应商名称' : 'Provider name')).toHaveProperty('value', 'ChatGPT')
    expect(client.request.mock.calls.filter(([method]) => method === 'provider-config.subscription.login-start')).toHaveLength(2)
    expect(client.request.mock.calls.some(([method]) => method === 'provider-config.upsert')).toBe(false)
    expect(screen.queryByText('a@b.com · Plus')).toBeNull()
  })
  it.each(['zh-CN', 'en'] as const)('prefers a failed login view reasonLocalized over its raw English reason in %s', async locale => {
    const client = fakeClient(), user = userEvent.setup(), base = client.request.getMockImplementation()!
    const localized: LocalizedText = { zh: '登录未能完成：模型路由暂时不可用，请重试。', en: 'Sign-in did not finish: The model router is temporarily unavailable. Try again.' }
    const failed: LocalizedLoginView = { attemptId: 'failed-attempt', provider: 'codex', status: 'failed', reason: 'Raw English engine failure', reasonLocalized: localized }
    client.request.mockImplementation((method, payload) => {
      if (method === 'provider-config.subscription.login-start') return Promise.resolve({ attemptId: 'failed-attempt', provider: 'codex', status: 'waiting' } satisfies SubscriptionLoginView)
      if (method === 'provider-config.subscription.login-status') return Promise.resolve(failed)
      return base(method, payload)
    })
    render(<LocaleProvider initialLocale={locale}><ModelSwitchSettings client={client} onClose={vi.fn()} /></LocaleProvider>)
    await openAccount(user, 'ChatGPT', locale)
    await user.click(screen.getByRole('button', { name: locale === 'zh-CN' ? '在浏览器中登录' : 'Sign in with browser' }))
    expect(await screen.findByText(localized[locale === 'zh-CN' ? 'zh' : 'en'], undefined, { timeout: 3000 })).toBeTruthy()
    expect(screen.queryByText('Raw English engine failure')).toBeNull()
    expect(screen.getByRole('button', { name: locale === 'zh-CN' ? '重试' : 'Retry' })).toHaveProperty('disabled', false)
    expect(screen.queryByText('a@b.com · Plus')).toBeNull()
  })
  it('cancels waiting explicitly and resumes from browser focus with a single status chain', async () => {
    const client = fakeClient(), user = userEvent.setup(), base = client.request.getMockImplementation()!
    client.request.mockImplementation((method, payload) => method === 'provider-config.subscription.login-start' ? Promise.resolve({ attemptId: 'attempt', provider: 'codex', status: 'waiting' }) : base(method, payload))
    render(<ModelSwitchSettings client={client} onClose={vi.fn()} />)
    await openAccount(user); await user.click(screen.getByRole('button', { name: '在浏览器中登录' }))
    await user.click(screen.getByRole('button', { name: '取消' }))
    expect(await screen.findByText('登录已取消（runtime）')).toBeTruthy()
    await user.click(screen.getByRole('button', { name: '重试' }))
    window.dispatchEvent(new Event('blur')); window.dispatchEvent(new Event('focus'))
    await waitFor(() => expect(screen.getAllByRole('combobox', { name: /模型|model/ })).toHaveLength(3), { timeout: 2000 })
  })
  it('reopens the same login attempt, blocks repeated clicks, and keeps one login', async () => {
    const client = fakeClient(), user = userEvent.setup(), base = client.request.getMockImplementation()!
    let resolve!: (view: SubscriptionLoginView) => void
    client.request.mockImplementation((method, payload) => method === 'provider-config.subscription.login-start' || method === 'provider-config.subscription.login-status' ? Promise.resolve({ attemptId: 'attempt', provider: 'codex', status: 'waiting' }) : method === 'provider-config.subscription.login-reopen' ? new Promise(yes => { resolve = yes }) : base(method, payload))
    render(<ModelSwitchSettings client={client} onClose={vi.fn()} />)
    await openAccount(user); await user.click(screen.getByRole('button', { name: '在浏览器中登录' }))
    const reopen = screen.getByRole('button', { name: '重新打开浏览器' })
    await user.dblClick(reopen)
    expect(reopen).toHaveProperty('disabled', true)
    expect(client.request.mock.calls.filter(([method]) => method === 'provider-config.subscription.login-reopen')).toEqual([['provider-config.subscription.login-reopen', { attemptId: 'attempt' }]])
    await act(async () => resolve({ attemptId: 'attempt', provider: 'codex', status: 'waiting' }))
    expect(reopen).toHaveProperty('disabled', false)
    expect(screen.getByText('等待浏览器完成登录…')).toBeTruthy()
    expect(client.request.mock.calls.filter(([method]) => method === 'provider-config.subscription.login-start')).toHaveLength(1)
    expect(client.request.mock.calls.some(([method]) => method === 'provider-config.subscription.login-cancel')).toBe(false)
  })
  it('shows reopen errors in the waiting page and preserves cancellation', async () => {
    const client = fakeClient(), user = userEvent.setup(), base = client.request.getMockImplementation()!
    client.request.mockImplementation((method, payload) => method === 'provider-config.subscription.login-start' ? Promise.resolve({ attemptId: 'attempt', provider: 'codex', status: 'waiting' }) : method === 'provider-config.subscription.login-reopen' ? Promise.reject(new Error('系统浏览器未能打开')) : base(method, payload))
    render(<ModelSwitchSettings client={client} onClose={vi.fn()} />)
    await openAccount(user); await user.click(screen.getByRole('button', { name: '在浏览器中登录' }))
    await user.click(screen.getByRole('button', { name: '重新打开浏览器' }))
    expect(await screen.findByRole('alert')).toHaveProperty('textContent', '系统浏览器未能打开')
    expect(screen.getByText('等待浏览器完成登录…')).toBeTruthy()
    expect(screen.getByRole('button', { name: '重新打开浏览器' })).toHaveProperty('disabled', false)
    await user.keyboard('{Escape}')
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(client.request).toHaveBeenCalledWith('provider-config.subscription.login-cancel', { attemptId: 'attempt' })
  })
  it('ignores a late reopen failure after browser sign-in succeeds', async () => {
    const client = fakeClient(), user = userEvent.setup(), base = client.request.getMockImplementation()!
    let reject!: (error: Error) => void
    client.request.mockImplementation((method, payload) => method === 'provider-config.subscription.login-start' ? Promise.resolve({ attemptId: 'attempt', provider: 'codex', status: 'waiting' }) : method === 'provider-config.subscription.login-reopen' ? new Promise((_resolve, no) => { reject = no }) : base(method, payload))
    render(<ModelSwitchSettings client={client} onClose={vi.fn()} />)
    await openAccount(user); await user.click(screen.getByRole('button', { name: '在浏览器中登录' }))
    await user.click(screen.getByRole('button', { name: '重新打开浏览器' }))
    await waitFor(() => expect(screen.getAllByRole('combobox', { name: /模型|model/ })).toHaveLength(3), { timeout: 2000 })
    await act(async () => reject(new Error('迟到的重开错误')))
    expect(screen.queryByRole('alert')).toBeNull()
    expect(screen.getByLabelText('供应商名称')).toHaveProperty('value', 'ChatGPT')
  })
  it('does not let an old reopen response unlock or fail a new login attempt', async () => {
    const client = fakeClient(), user = userEvent.setup(), base = client.request.getMockImplementation()!
    let starts = 0
    const pending: { resolve(view: SubscriptionLoginView): void; reject(error: Error): void }[] = []
    client.request.mockImplementation((method, payload) => {
      if (method === 'provider-config.subscription.login-start') return Promise.resolve({ attemptId: `attempt-${++starts}`, provider: 'codex', status: 'waiting' })
      if (method === 'provider-config.subscription.login-status') return Promise.resolve({ ...(payload as { attemptId: string }), provider: 'codex', status: 'waiting' })
      if (method === 'provider-config.subscription.login-reopen') return new Promise((resolve, reject) => pending.push({ resolve, reject }))
      return base(method, payload)
    })
    render(<ModelSwitchSettings client={client} onClose={vi.fn()} />)
    await openAccount(user); await user.click(screen.getByRole('button', { name: '在浏览器中登录' }))
    await user.click(screen.getByRole('button', { name: '重新打开浏览器' }))
    await user.click(screen.getByRole('button', { name: '取消' }))
    await user.click(screen.getByRole('button', { name: '重试' }))
    const reopen = screen.getByRole('button', { name: '重新打开浏览器' })
    expect(reopen).toHaveProperty('disabled', false)
    await user.click(reopen)
    await act(async () => pending[0]!.reject(new Error('上一次登录的迟到错误')))
    expect(screen.queryByRole('alert')).toBeNull()
    expect(reopen).toHaveProperty('disabled', true)
    await act(async () => pending[1]!.resolve({ attemptId: 'attempt-2', provider: 'codex', status: 'waiting' }))
    expect(reopen).toHaveProperty('disabled', false)
  })
  it.each(['zh-CN', 'en'] as const)('keeps waiting through a failed browser launch, shows its localized reason and reopens the same attempt into models in %s', async locale => {
    const client = fakeClient(), user = userEvent.setup(), base = client.request.getMockImplementation()!
    const zh = locale === 'zh-CN'
    const copy = zh
      ? { signIn: '在浏览器中登录', waiting: '等待浏览器完成登录…', reason: '浏览器没有打开', reopen: '重新打开浏览器', retry: '重试', name: '供应商名称' }
      : { signIn: 'Sign in with browser', waiting: 'Waiting for the browser to finish signing in…', reason: "The browser didn't open", reopen: 'Open browser again', retry: 'Retry', name: 'Provider name' }
    const failed: LocalizedLoginView = {
      attemptId: 'attempt', provider: 'codex', status: 'failed', canReopen: true,
      reason: '浏览器没有打开', reasonLocalized: { zh: '浏览器没有打开', en: "The browser didn't open" }
    }
    let recovered = false
    client.request.mockImplementation((method, payload) => {
      if (method === 'provider-config.subscription.login-start') return Promise.resolve({ attemptId: 'attempt', provider: 'codex', status: 'waiting' } satisfies SubscriptionLoginView)
      if (method === 'provider-config.subscription.login-status') return Promise.resolve(recovered ? { attemptId: 'attempt', provider: 'codex', status: 'ready', details } satisfies SubscriptionLoginView : failed)
      if (method === 'provider-config.subscription.login-reopen') { recovered = true; return Promise.resolve({ attemptId: 'attempt', provider: 'codex', status: 'waiting' } satisfies SubscriptionLoginView) }
      return base(method, payload)
    })
    render(<LocaleProvider initialLocale={locale}><ModelSwitchSettings client={client} onClose={vi.fn()} /></LocaleProvider>)
    await openAccount(user, 'ChatGPT', locale)
    await user.click(screen.getByRole('button', { name: copy.signIn }))
    expect(await screen.findByText(copy.reason, undefined, { timeout: 3000 })).toBeTruthy()
    expect(screen.getByText(copy.waiting)).toBeTruthy()
    expect(screen.queryByRole('button', { name: copy.retry })).toBeNull()
    const reopen = screen.getByRole('button', { name: copy.reopen })
    expect(reopen).toHaveProperty('disabled', false)
    await user.click(reopen)
    expect(client.request.mock.calls.filter(([method]) => method === 'provider-config.subscription.login-reopen')).toEqual([['provider-config.subscription.login-reopen', { attemptId: 'attempt' }]])
    // The reopened attempt returns to waiting; the original status chain is
    // what carries it to ready, so no second login is started or polled.
    expect(await screen.findByLabelText(copy.name, undefined, { timeout: 5000 })).toHaveProperty('value', 'ChatGPT')
    expect(screen.getAllByRole('combobox', { name: /模型|model/ })).toHaveLength(3)
    expect(client.request.mock.calls.filter(([method]) => method === 'provider-config.subscription.login-start')).toHaveLength(1)
    expect(client.request.mock.calls.some(([method]) => method === 'provider-config.subscription.login-cancel')).toBe(false)
    expect(screen.queryByText(copy.reason)).toBeNull()
  })
  it.each(['zh-CN', 'en'] as const)('keeps an unrecoverable failed sign-in on the error page in %s', async locale => {
    const client = fakeClient(), user = userEvent.setup(), base = client.request.getMockImplementation()!
    const zh = locale === 'zh-CN'
    const localized: LocalizedText = { zh: '登录超时，请重试。', en: 'Sign-in timed out. Try again.' }
    client.request.mockImplementation((method, payload) => {
      if (method === 'provider-config.subscription.login-start') return Promise.resolve({ attemptId: 'attempt', provider: 'codex', status: 'waiting' } satisfies SubscriptionLoginView)
      if (method === 'provider-config.subscription.login-status') return Promise.resolve({ attemptId: 'attempt', provider: 'codex', status: 'failed', reason: 'Raw English engine failure', reasonLocalized: localized } satisfies LocalizedLoginView)
      return base(method, payload)
    })
    render(<LocaleProvider initialLocale={locale}><ModelSwitchSettings client={client} onClose={vi.fn()} /></LocaleProvider>)
    await openAccount(user, 'ChatGPT', locale)
    await user.click(screen.getByRole('button', { name: zh ? '在浏览器中登录' : 'Sign in with browser' }))
    expect(await screen.findByText(localized[zh ? 'zh' : 'en'], undefined, { timeout: 3000 })).toBeTruthy()
    expect(screen.queryByText(zh ? '等待浏览器完成登录…' : 'Waiting for the browser to finish signing in…')).toBeNull()
    expect(screen.queryByRole('button', { name: zh ? '重新打开浏览器' : 'Open browser again' })).toBeNull()
    expect(screen.getByRole('button', { name: zh ? '重试' : 'Retry' })).toHaveProperty('disabled', false)
  })
  it('reads quotas once, refreshes statuses on push without rereading, and closes the settings lease', async () => {
    const client = fakeClient([subscription()]), user = userEvent.setup(), onClose = vi.fn()
    const { unmount } = render(<ModelSwitchSettings client={client} onClose={onClose} />)
    await screen.findByText('5 小时 12% · 每周 34%')
    await act(async () => { client.setRows([subscription('router-not-running')]); client.changed() })
    expect(await screen.findByText('模型路由未运行')).toBeTruthy()
    expect(client.request.mock.calls.filter(([m]) => m.endsWith('settings-read'))).toHaveLength(1)
    await user.keyboard('{Escape}')
    expect(onClose).toHaveBeenCalledOnce()
    unmount()
    expect(client.request.mock.calls.filter(([m]) => m.endsWith('settings-close'))).toHaveLength(1)
  })
  it('keeps cards when quota read fails and displays structured switch failures with retry', async () => {
    const client = fakeClient([subscription()]), user = userEvent.setup(), base = client.request.getMockImplementation()!
    client.request.mockImplementation((method, payload) => method.endsWith('settings-read') ? Promise.reject(Error('quota offline')) : method === 'provider-config.activate' ? Promise.reject(Object.assign(Error('generic'), { providerFailure: { status: 'router-start-failed', message: { zh: '模型路由未能启动：端口占用', en: 'The router port is in use' }, recovery: 'retry' } })) : base(method, payload))
    render(<ModelSwitchSettings client={client} onClose={vi.fn()} />)
    await screen.findByText('a@b.com · Plus')
    expect(screen.queryByText('5 小时 12% · 每周 34%')).toBeNull()
    await user.click(screen.getByRole('button', { name: '切换到 ChatGPT · a@b.com' }))
    expect(await screen.findByText('模型路由未能启动：端口占用')).toBeTruthy()
    await user.click(screen.getByRole('button', { name: '重试' }))
    expect(client.request.mock.calls.filter(([m]) => m === 'provider-config.activate')).toHaveLength(2)
  })
  it.each(['zh-CN', 'en'] as const)('shows only the localized activation message and opens the same card sign-in flow in %s', async locale => {
    const client = fakeClient([subscription()]), user = userEvent.setup(), base = client.request.getMockImplementation()!
    const failure: ProviderActivationFailure = {
      status: 'needs-sign-in', recovery: 'sign-in',
      message: { zh: '需要重新登录：订阅账号登录已失效。', en: 'Sign in again: Your subscription sign-in has expired.' },
      reason: '订阅账号登录已失效。', reasonLocalized: { zh: '订阅账号登录已失效。', en: 'Your subscription sign-in has expired.' }
    }
    client.request.mockImplementation((method, payload) => method === 'provider-config.activate' ? Promise.reject(Object.assign(new Error('不应拼接的外层错误'), { providerFailure: failure })) : base(method, payload))
    render(<LocaleProvider initialLocale={locale}><ModelSwitchSettings client={client} onClose={vi.fn()} /></LocaleProvider>)
    await user.click(await screen.findByRole('button', { name: locale === 'zh-CN' ? '切换到 ChatGPT · a@b.com' : 'Switch to ChatGPT · a@b.com' }))
    const alert = await screen.findByRole('alert')
    expect(alert.querySelector('p')?.textContent).toBe(failure.message[locale === 'zh-CN' ? 'zh' : 'en'])
    expect(alert.textContent).not.toContain('不应拼接的外层错误')
    expect(within(alert).queryByRole('button', { name: locale === 'zh-CN' ? '重试' : 'Retry' })).toBeNull()
    await user.click(within(alert).getByRole('button', { name: locale === 'zh-CN' ? '重新登录' : 'Sign in again' }))
    const dialog = await screen.findByRole('dialog')
    expect(within(dialog).getByText(locale === 'zh-CN' ? '已有该类账号，重新登录会替换它，模型设置会保留。' : 'You already have this account type. Signing in again replaces it and keeps your model settings.')).toBeTruthy()
    await user.click(within(dialog).getByRole('button', { name: locale === 'zh-CN' ? '在浏览器中登录' : 'Sign in with browser' }))
    expect(client.request).toHaveBeenCalledWith('provider-config.subscription.login-start', { provider: 'codex', locale }, { timeoutMs: 30_000 })
    expect(screen.getByLabelText(locale === 'zh-CN' ? '供应商名称' : 'Provider name')).toHaveProperty('value', 'ChatGPT · a@b.com')
    expect(client.request.mock.calls.filter(([method]) => method === 'provider-config.activate')).toHaveLength(1)
  })
  it('reuses a needs-sign-in card and preserves its name and mappings, while editing never shows API fields', async () => {
    const row = { ...subscription('needs-sign-in'), name: '我的订阅', modelMapping: { opus: 'gpt-6-luna', sonnet: 'gpt-6-astra', haiku: 'gpt-6.1-sol' } }
    const client = fakeClient([row]), user = userEvent.setup()
    render(<ModelSwitchSettings client={client} onClose={vi.fn()} />)
    await screen.findByText('a@b.com · Plus')
    await user.click(screen.getByRole('button', { name: '重新登录 我的订阅' }))
    expect(screen.getByText('已有该类账号，重新登录会替换它，模型设置会保留。')).toBeTruthy()
    await user.click(screen.getByRole('button', { name: '在浏览器中登录' }))
    expect(screen.getByLabelText('供应商名称')).toHaveProperty('value', '我的订阅')
    expect(screen.getAllByRole('combobox', { name: /模型|model/ }).map(select => (select as HTMLSelectElement).value)).toEqual(['gpt-6-luna', 'gpt-6-astra', 'gpt-6.1-sol'])
    await user.click(screen.getByRole('button', { name: '保存' }))
    expect(client.request).toHaveBeenCalledWith('provider-config.upsert', { provider: expect.objectContaining({ id: 'chatgpt' }) })
    await user.click(within(screen.getByText('我的订阅').closest('article')!).getByRole('button', { name: '编辑' }))
    expect(screen.queryByLabelText('API 地址')).toBeNull()
    expect(screen.queryByLabelText('API Key')).toBeNull()
    expect(screen.getAllByRole('combobox', { name: /模型|model/ })).toHaveLength(3)
    expect(screen.queryByText('没有读到可用的模型')).toBeNull()
    expect(client.request.mock.calls.some(([method]) => method === 'provider-config.subscription.models-read')).toBe(false)
  })
  it('renders unavailable reasons and English quota/status copy', async () => {
    render(<LocaleProvider initialLocale="en"><ModelSwitchSettings client={fakeClient([subscription('account-unavailable')])} onClose={vi.fn()} /></LocaleProvider>)
    expect(await screen.findByText('Account unavailable')).toBeTruthy()
    expect(screen.getByText('服务方拒绝此账号')).toBeTruthy()
    expect(screen.queryByRole('button', { name: 'Switch back to Anthropic official' })).toBeNull()
    expect(await screen.findByText('5h 12% · Weekly 34%')).toBeTruthy()
  })
  it.each(['zh-CN', 'en'] as const)('switches an unavailable subscription straight to the built-in official provider in %s', async locale => {
    const inactive = { ...subscription('account-unavailable'), id: 'antigravity', name: 'Antigravity', subscription: { ...subscription('account-unavailable').subscription!, provider: 'antigravity' as const } }
    const client = fakeClient([subscription('account-unavailable'), inactive], false, 'chatgpt'), user = userEvent.setup()
    render(<LocaleProvider initialLocale={locale}><ModelSwitchSettings client={client} onClose={vi.fn()} /></LocaleProvider>)
    const action = await screen.findByRole('button', { name: locale === 'zh-CN' ? '切回 Anthropic 官方' : 'Switch back to Anthropic official' })
    expect(action.classList.contains('provider-link')).toBe(true)
    const inactiveCard = within(screen.getByText('Antigravity').closest('article')!)
    expect(inactiveCard.queryByRole('button', { name: /切回 Anthropic 官方|Switch back to Anthropic official/ })).toBeNull()
    expect(inactiveCard.getByText(locale === 'zh-CN' ? '账号不可用' : 'Account unavailable')).toBeTruthy()
    expect(inactiveCard.getByText('服务方拒绝此账号')).toBeTruthy()
    await user.click(action)
    expect(client.request).toHaveBeenCalledWith('provider-config.activate', { cli: 'claude-code', providerId: 'official' })
    await waitFor(() => expect(screen.getByText('Anthropic 官方').closest('article')?.classList.contains('is-current')).toBe(true))
    expect(await screen.findByRole('status')).toBeTruthy()
    expect(screen.queryByRole('button', { name: /切回 Anthropic 官方|Switch back to Anthropic official/ })).toBeNull()
  })
  it('keeps an unavailable subscription selected if switching back to official fails, and retries official', async () => {
    const client = fakeClient([subscription('account-unavailable')], false, 'chatgpt'), user = userEvent.setup(), base = client.request.getMockImplementation()!
    client.request.mockImplementation((method, payload) => method === 'provider-config.activate' ? Promise.reject(new Error('官方配置切换失败')) : base(method, payload))
    render(<ModelSwitchSettings client={client} onClose={vi.fn()} />)
    await user.click(await screen.findByRole('button', { name: '切回 Anthropic 官方' }))
    expect(await screen.findByRole('alert')).toHaveProperty('textContent', expect.stringContaining('官方配置切换失败'))
    expect(screen.getByText('a@b.com · Plus').closest('article')?.classList.contains('is-current')).toBe(true)
    await user.click(screen.getByRole('button', { name: '重试' }))
    expect(client.request.mock.calls.filter(([method]) => method === 'provider-config.activate').map(([, payload]) => payload)).toEqual([{ cli: 'claude-code', providerId: 'official' }, { cli: 'claude-code', providerId: 'official' }])
  })
  it('defaults a newly added Antigravity name to the brand only', async () => {
    const client = fakeClient([], true), user = userEvent.setup()
    render(<ModelSwitchSettings client={client} onClose={vi.fn()} />)
    await openAccount(user, 'Antigravity')
    await user.click(screen.getByRole('button', { name: '在浏览器中登录' }))
    expect(screen.getByLabelText('供应商名称')).toHaveProperty('value', 'Antigravity')
  })
  it('keeps model drafts after save failures and confirms unsaved changes before cancelling the attempt', async () => {
    const client = fakeClient(), user = userEvent.setup(), base = client.request.getMockImplementation()!
    client.request.mockImplementation((method, payload) => method === 'provider-config.upsert' ? Promise.reject(new Error('账号采用失败，请重试')) : base(method, payload))
    render(<ModelSwitchSettings client={client} onClose={vi.fn()} />)
    await openAccount(user); await user.click(screen.getByRole('button', { name: '在浏览器中登录' }))
    await user.type(screen.getByLabelText('供应商名称'), ' updated')
    await user.click(screen.getByRole('button', { name: '保存' }))
    expect(await screen.findByText('账号采用失败，请重试')).toBeTruthy()
    expect(screen.getByLabelText('供应商名称')).toHaveProperty('value', 'ChatGPT updated')
    await user.keyboard('{Escape}')
    expect(screen.getByText('有尚未保存的修改')).toBeTruthy()
    await user.click(screen.getByRole('button', { name: '放弃修改' }))
    await waitFor(() => expect(client.request).toHaveBeenCalledWith('provider-config.subscription.login-cancel', { attemptId: 'attempt' }))
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(screen.queryByText('a@b.com · Plus')).toBeNull()
  })
  it('does not accept the Antigravity risk if persistence fails', async () => {
    const client = fakeClient(), user = userEvent.setup(), base = client.request.getMockImplementation()!
    client.request.mockImplementation((method, payload) => method.endsWith('risk-confirm') ? Promise.reject(new Error('风险确认保存失败')) : base(method, payload))
    render(<ModelSwitchSettings client={client} onClose={vi.fn()} />)
    await openAccount(user, 'Antigravity')
    await user.click(screen.getByRole('checkbox'))
    await user.click(screen.getByRole('button', { name: '继续' }))
    expect(await screen.findByText('风险确认保存失败')).toBeTruthy()
    expect(screen.queryByRole('button', { name: '在浏览器中登录' })).toBeNull()
  })
  it('chooses eligible defaults and formats constrained group quotas without network calls', () => {
    expect(subscriptionDefaults('codex', [{ id: 'gpt-image-2' }, { id: 'fallback' }])).toEqual({ opus: 'fallback', sonnet: 'fallback', haiku: 'fallback' })
    expect(subscriptionDefaults('codex', [{ id: 'codex-auto-review' }, { id: 'gpt-6-luna', displayName: 'GPT-6 Luna' }, { id: 'gpt-6-astra', displayName: 'GPT-6 Astra' }, { id: 'gpt-6.1-sol', displayName: 'GPT-6.1 Sol' }])).toEqual({ opus: 'gpt-6-astra', sonnet: 'gpt-6.1-sol', haiku: 'gpt-6-luna' })
    expect(subscriptionDefaults('codex', [{ id: 'codex-auto-review' }, { id: 'gpt-image-2' }])).toEqual({ opus: '', sonnet: '', haiku: '' })
    expect(subscriptionQuotaText('antigravity', { ...details, quota: [{ name: 'a', group: 'claude-gpt', usedPercent: 20 }, { name: 'b', group: 'claude-gpt', usedPercent: 10 }, { name: 'c', group: 'gemini', usedPercent: 5 }] }, true)).toBe('Claude 剩余 80% · Gemini 剩余 95%')
    expect(subscriptionQuotaText('codex', { ...details, quota: [] }, true)).toBe('')
    expect(subscriptionQuotaTitle({ ...details, quota: [{ name: 'weekly', usedPercent: 34, resetsAt: '2026-11-14T00:00:00Z' }] }, true)).toBe('每周 · 剩余 66% · 重置: 2026-11-14T00:00:00Z')
  })
})
