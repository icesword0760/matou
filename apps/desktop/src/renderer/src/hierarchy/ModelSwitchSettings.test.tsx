// @vitest-environment jsdom
import { act, cleanup, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type {
  ProviderActivationFailure,
  ProviderConfigView,
  ProviderConfigSnapshot,
  ProviderSessionActivationTransition
} from '@matou/contracts'
import { ModelSwitchSettings, type ProviderConfigClient } from './ModelSwitchSettings'

afterEach(cleanup)

describe('ModelSwitchSettings', () => {
  it('focuses on Claude Code providers without a Codex tab and activates a card', async () => {
    const client = fakeClient()
    render(<ModelSwitchSettings client={client} onClose={vi.fn()} />)

    expect((await screen.findAllByText('Anthropic 官方')).length).toBeGreaterThan(0)
    expect(screen.queryByRole('heading', { name: '设置' })).toBeNull()
    expect(screen.queryByRole('tab', { name: 'Codex' })).toBeNull()
    expect(screen.queryByText('OpenAI 官方')).toBeNull()
    expect(screen.queryByText('PackyCode')).toBeNull()
    await userEvent.setup().click(screen.getByRole('button', { name: '切换到 Team Gateway' }))

    expect(client.request).toHaveBeenCalledWith('provider-config.activate', {
      cli: 'claude-code', providerId: 'team-claude'
    })
    expect(await screen.findByText('已切换为 Team Gateway；Claude Code 新会话直接生效')).toBeTruthy()
  })

  it('re-applies the active provider after save-and-switch and reports session updates', async () => {
    const original = fakeClient([
      { sessionId: 'session-updated', status: 'updated' },
      { sessionId: 'session-deferred', status: 'deferred', reason: 'session-not-running' }
    ], 'team-claude')
    const request = vi.fn<ProviderConfigClient['request']>(async (method, payload) => {
        if (method === 'provider-config.upsert') {
          const input = (payload as { provider: { id?: string; cli: 'claude-code'; name: string; endpoint: string; model: string } }).provider
          return { provider: { ...input, id: input.id ?? 'created', hasApiKey: false } }
        }
        return original.request(method, payload)
    })
    const client: ProviderConfigClient = { request }
    const user = userEvent.setup()
    render(<ModelSwitchSettings client={client} onClose={vi.fn()} />)
    await screen.findAllByText('Team Gateway')
    const currentCard = screen.getByText('Team Gateway').closest('article')!
    await user.click(within(currentCard).getByRole('button', { name: '编辑' }))
    const dialog = screen.getByRole('dialog', { name: '编辑供应商' })
    await user.click(within(dialog).getByRole('button', { name: '保存并切换' }))

    await waitFor(() => expect(request.mock.calls.map(call => call[0])).toContain('provider-config.activate'))
    const methods = request.mock.calls.map(call => call[0])
    expect(methods.indexOf('provider-config.upsert')).toBeLessThan(methods.indexOf('provider-config.activate'))
    expect(client.request).toHaveBeenCalledWith('provider-config.activate', {
      cli: 'claude-code', providerId: 'team-claude'
    })
    expect(await screen.findByText('已切换为 Team Gateway；1 个 Claude Code 会话已更新，1 个会话暂缓并保持原配置')).toBeTruthy()
  })

  it('does not re-apply the active provider after save-only', async () => {
    const original = fakeClient([], 'team-claude')
    const request = vi.fn<ProviderConfigClient['request']>(async (method, payload) => {
        if (method === 'provider-config.upsert') {
          const input = (payload as { provider: { id?: string; cli: 'claude-code'; name: string; endpoint: string; model: string } }).provider
          return { provider: { ...input, id: input.id ?? 'created', hasApiKey: false } }
        }
        return original.request(method, payload)
    })
    const client: ProviderConfigClient = { request }
    const user = userEvent.setup()
    render(<ModelSwitchSettings client={client} onClose={vi.fn()} />)
    await screen.findAllByText('Team Gateway')
    const currentCard = screen.getByText('Team Gateway').closest('article')!
    await user.click(within(currentCard).getByRole('button', { name: '编辑' }))
    const dialog = screen.getByRole('dialog', { name: '编辑供应商' })
    await user.click(within(dialog).getByRole('button', { name: '保存' }))

    await waitFor(() => expect(request.mock.calls.map(call => call[0])).toContain('provider-config.upsert'))
    expect(client.request).not.toHaveBeenCalledWith('provider-config.activate', expect.anything())
  })

  it('retries a failed active-provider re-application instead of silently closing', async () => {
    const original = fakeClient([], 'team-claude')
    let activationAttempts = 0
    const request = vi.fn<ProviderConfigClient['request']>(async (method, payload) => {
        if (method === 'provider-config.upsert') {
          const input = (payload as { provider: { id?: string; cli: 'claude-code'; name: string; endpoint: string; model: string } }).provider
          return { provider: { ...input, id: input.id ?? 'created', hasApiKey: false } }
        }
        if (method === 'provider-config.activate') {
          activationAttempts++
          if (activationAttempts === 1) throw new Error('切换连接中断')
        }
        return original.request(method, payload)
    })
    const client: ProviderConfigClient = { request }
    const user = userEvent.setup()
    render(<ModelSwitchSettings client={client} onClose={vi.fn()} />)
    await screen.findAllByText('Team Gateway')
    const currentCard = screen.getByText('Team Gateway').closest('article')!
    await user.click(within(currentCard).getByRole('button', { name: '编辑' }))
    const dialog = screen.getByRole('dialog', { name: '编辑供应商' })
    await user.click(within(dialog).getByRole('button', { name: '保存并切换' }))

    expect(await screen.findByRole('alert')).toHaveProperty('textContent', expect.stringContaining('切换连接中断'))
    await user.click(screen.getByRole('button', { name: '重试' }))
    expect(await screen.findByText('已切换为 Team Gateway；Claude Code 新会话直接生效')).toBeTruthy()
    expect(activationAttempts).toBe(2)
  })

  it('shows context window badges on provider cards and edits a provider window inside its edit dialog', async () => {
    const original = fakeClient()
    let agentState = {
      preferences: {
        contextWindows: { anthropic: 'default', 'team-claude': 'default' },
        compaction: { mode: 'follow', effort: 'follow' }
      },
      providers: [{
        id: 'anthropic', name: 'Anthropic 官方', cli: 'claude-code', compactionAvailable: true,
        models: [{ id: 'opus', name: 'Opus', defaultTokens: 1_000_000, maximumTokens: 1_000_000, fixedTokens: 512_000 }]
      }, {
        id: 'team-claude', name: 'Team Gateway', cli: 'claude-code', compactionAvailable: true,
        models: [{ id: 'claude-team', name: 'claude-team', fixedTokens: 512_000 }]
      }]
    }
    const request = vi.fn<ProviderConfigClient['request']>(async (method, payload) => {
        if (method === 'agent-model-settings.get') return agentState
        if (method === 'agent-model-settings.set') {
          agentState = { ...agentState, preferences: payload as typeof agentState.preferences }
          return agentState
        }
        return original.request(method, payload)
    })
    const client: ProviderConfigClient = { request }
    render(<ModelSwitchSettings client={client} onClose={vi.fn()} />)

    expect(await screen.findByText('默认 1M 窗口')).toBeTruthy()
    expect(screen.getByText('默认窗口')).toBeTruthy()

    const user = userEvent.setup()
    const teamCard = screen.getByText('Team Gateway').closest('article')!
    await user.click(within(teamCard).getByRole('button', { name: '编辑' }))
    const dialog = screen.getByRole('dialog', { name: '编辑供应商' })
    await user.click(within(dialog).getByRole('radio', { name: /512K/ }))
    await user.click(within(dialog).getByRole('button', { name: /^保存$/ }))

    expect(client.request).toHaveBeenCalledWith('agent-model-settings.set', {
      contextWindows: { anthropic: 'default', 'team-claude': '512k' },
      compaction: { mode: 'follow', effort: 'follow' }
    })
    expect(await screen.findByText('512K 窗口')).toBeTruthy()
  })

  it('refreshes the window badge after delayed subscription metadata finishes loading', async () => {
    const ids = ['gpt-opus', 'gpt-sonnet', 'gpt-haiku']
    const provider: ProviderConfigView = {
      id: 'chatgpt', cli: 'claude-code', kind: 'subscription', name: 'ChatGPT', endpoint: '', model: 'gpt-sonnet', hasApiKey: false,
      modelMapping: { opus: 'gpt-opus', sonnet: 'gpt-sonnet', haiku: 'gpt-haiku' },
      subscription: {
        provider: 'codex',
        account: { email: 'a@b.com', plan: 'Plus', status: 'ready' },
        modelMetadata: ids.map(id => ({ id, contextWindow: 872_000, maxContextWindow: 872_000 }))
      }
    }
    const initialSnapshot: ProviderConfigSnapshot = {
      revision: 1,
      activeProviderIds: { 'claude-code': 'chatgpt', codex: 'openai' },
      providers: { 'claude-code': [provider], codex: [] }
    }
    const oldAgent = {
      preferences: { contextWindows: { chatgpt: 'maximum' }, compaction: { mode: 'follow', effort: 'follow' } },
      providers: [{ id: 'chatgpt', name: 'ChatGPT', cli: 'claude-code', compactionAvailable: true,
        models: ids.map(id => ({ id, name: id, defaultTokens: 272_000, maximumTokens: 272_000, fixedTokens: 512_000 })) }]
    }
    const updatedAgent = {
      ...oldAgent,
      providers: [{ ...oldAgent.providers[0], models: ids.map(id => ({ id, name: id, defaultTokens: 872_000, maximumTokens: 872_000, fixedTokens: 512_000 })) }]
    }
    let resolveSettingsRead!: (value: unknown) => void
    let settingsReadResolved = false
    let agentReads = 0
    const settingsRead = new Promise<unknown>(resolve => { resolveSettingsRead = resolve })
    const client: ProviderConfigClient = {
      request: vi.fn((method, payload) => {
        if (method === 'provider-config.snapshot') return Promise.resolve(initialSnapshot)
        if (method === 'provider-config.subscription.settings-read') return settingsRead
        if (method === 'provider-config.subscription.settings-close') return Promise.resolve({})
        if (method === 'agent-model-settings.get') {
          agentReads++
          return Promise.resolve(settingsReadResolved ? updatedAgent : oldAgent)
        }
        return Promise.resolve(payload)
      })
    }
    render(<ModelSwitchSettings client={client} onClose={vi.fn()} />)

    expect(await screen.findByText('最大 272K 窗口')).toBeTruthy()
    await waitFor(() => expect(agentReads).toBeGreaterThanOrEqual(2))
    const readsBeforeSettingsComplete = agentReads
    await act(async () => {
      settingsReadResolved = true
      resolveSettingsRead({
        snapshot: { ...initialSnapshot, revision: 2 },
        accounts: { chatgpt: { account: provider.subscription!.account, models: provider.subscription!.modelMetadata, quota: [] } },
        antigravityRiskAccepted: false
      })
    })

    expect(await screen.findByText('最大 872K 窗口')).toBeTruthy()
    expect(screen.queryByText('最大 272K 窗口')).toBeNull()
    expect(agentReads).toBeGreaterThan(readsBeforeSettingsComplete)
  })

  it('updates an open provider dialog when delayed agent settings load a maximum window', async () => {
    const original = fakeClient()
    let resolveAgentSettings!: (value: unknown) => void
    const delayedAgentSettings = new Promise<unknown>(resolve => { resolveAgentSettings = resolve })
    const client: ProviderConfigClient = {
      request: vi.fn((method, payload) => method === 'agent-model-settings.get'
        ? delayedAgentSettings
        : original.request(method, payload))
    }
    render(<ModelSwitchSettings client={client} onClose={vi.fn()} />)
    await screen.findAllByText('Anthropic 官方')

    const user = userEvent.setup()
    const teamCard = screen.getByText('Team Gateway').closest('article')!
    await user.click(within(teamCard).getByRole('button', { name: '编辑' }))
    const dialog = screen.getByRole('dialog', { name: '编辑供应商' })
    expect(within(dialog).getByRole('radio', { name: /默认窗口/ })).toHaveProperty('checked', true)

    await act(async () => {
      resolveAgentSettings({
        preferences: {
          contextWindows: { anthropic: 'default', 'team-claude': 'maximum' },
          compaction: { mode: 'follow', effort: 'follow' }
        },
        providers: []
      })
    })
    await waitFor(() => expect(within(dialog).getByRole('radio', { name: /最大可扩展/ })).toHaveProperty('checked', true))
    expect(within(dialog).getByRole('radio', { name: /默认窗口/ })).toHaveProperty('checked', false)
  })

  it('adds a provider from the compact core configuration form', async () => {
    const client = fakeClient()
    render(<ModelSwitchSettings client={client} onClose={vi.fn()} />)
    await screen.findAllByText('Anthropic 官方')
    const user = userEvent.setup()

    await user.click(screen.getByRole('button', { name: '新增供应商' }))
    await user.click(screen.getByRole('button', { name: /自定义服务/ }))
    await user.click(screen.getByRole('button', { name: '下一步' }))
    const dialog = screen.getByRole('dialog', { name: '配置 自定义服务' })
    await user.clear(within(dialog).getByRole('textbox', { name: '供应商名称' }))
    await user.type(within(dialog).getByRole('textbox', { name: '供应商名称' }), 'Team Gateway')
    await user.clear(within(dialog).getByRole('textbox', { name: 'API 地址' }))
    await user.type(within(dialog).getByRole('textbox', { name: 'API 地址' }), 'https://gateway.example')
    await user.clear(within(dialog).getByRole('textbox', { name: '默认模型' }))
    await user.type(within(dialog).getByRole('textbox', { name: '默认模型' }), 'claude-team')
    await user.type(within(dialog).getByLabelText('API Key'), 'TOKEN')
    await user.click(within(dialog).getByRole('button', { name: '仅保存' }))

    expect(client.request).toHaveBeenCalledWith('provider-config.upsert', {
      provider: {
        cli: 'claude-code', name: 'Team Gateway', endpoint: 'https://gateway.example',
        model: 'claude-team', apiKey: 'TOKEN',
        effortMapping: { opus: 'follow', sonnet: 'follow', haiku: 'follow' },
        modelMapping: {}, modelMetadata: []
      }
    })
  })

  it('reports Sessions that keep their old provider while activation is deferred', async () => {
    const client = fakeClient([
      { sessionId: 'session-updated', status: 'updated' },
      {
        sessionId: 'session-storage', status: 'deferred',
        reason: 'durability-fault'
      },
      {
        sessionId: 'session-recovery', status: 'deferred',
        reason: 'recovery-not-ready'
      }
    ])
    render(<ModelSwitchSettings client={client} onClose={vi.fn()} />)

    await screen.findAllByText('Anthropic 官方')
    await userEvent.setup().click(screen.getByRole('button', { name: '切换到 Team Gateway' }))

    expect(await screen.findByText(
      '已切换为 Team Gateway；1 个 Claude Code 会话已更新，2 个会话暂缓并保持原配置'
    )).toBeTruthy()
    await userEvent.setup().click(screen.getByRole('button', { name: /查看会话/ }))
    expect(screen.getByText('会话存储待恢复')).toBeTruthy()
    expect(screen.getByText('会话正在恢复')).toBeTruthy()
    expect(screen.getByText('会话 session-updated')).toBeTruthy()
  })

  it('keeps the original error and retry when the request is rejected before commit', async () => {
    const original = fakeClient()
    const client: ProviderConfigClient = {request: vi.fn((method,payload) => method === 'provider-config.activate' ? Promise.reject(new Error('切换连接中断')) : original.request(method,payload))}
    render(<ModelSwitchSettings client={client} onClose={vi.fn()} />)
    await screen.findAllByText('Anthropic 官方')
    await userEvent.setup().click(screen.getByRole('button',{name:'切换到 Team Gateway'}))
    expect(await screen.findByRole('alert')).toHaveProperty('textContent',expect.stringContaining('切换连接中断'))
    expect(screen.queryByText('默认供应商已切换')).toBeNull()
    expect(screen.getByRole('button',{name:'切换到 Team Gateway'})).toHaveProperty('disabled',false)
  })

  it('shows an unconfirmed switch notice when the activation committed before the RPC rejection', async () => {
    const original = fakeClient([{ sessionId: 'session-updated', status: 'updated' }])
    const request = vi.fn<ProviderConfigClient['request']>(async (method, payload) => {
        if (method === 'provider-config.activate') {
          await original.request(method, payload)
          throw new Error('切换请求超时')
        }
        return original.request(method, payload)
    })
    const client: ProviderConfigClient = { request }
    render(<ModelSwitchSettings client={client} onClose={vi.fn()} />)
    await screen.findAllByText('Anthropic 官方')
    await userEvent.setup().click(screen.getByRole('button', { name: '切换到 Team Gateway' }))

    expect(await screen.findByText(
      '默认供应商已切换为 Team Gateway；现有会话更新结果尚未确认，请检查会话状态。'
    )).toBeTruthy()
    expect(screen.getByText('切换结果待确认')).toBeTruthy()
    expect(screen.queryByRole('alert')).toBeNull()
    expect(screen.queryByRole('button', { name: '重试' })).toBeNull()
    expect(screen.queryByRole('button', { name: /查看会话/ })).toBeNull()
    expect(screen.getByRole('button', { name: 'Team Gateway 当前配置' })).toHaveProperty('disabled', true)
  })

  it('keeps the original error and retry when the confirmation snapshot cannot be read', async () => {
    const original = fakeClient()
    let activationAttempted = false
    const request = vi.fn<ProviderConfigClient['request']>(async (method, payload) => {
        if (method === 'provider-config.activate') {
          activationAttempted = true
          await original.request(method, payload)
          throw new Error('切换请求超时')
        }
        if (method === 'provider-config.snapshot' && activationAttempted) throw new Error('读取最新状态失败')
        return original.request(method, payload)
    })
    const client: ProviderConfigClient = { request }
    render(<ModelSwitchSettings client={client} onClose={vi.fn()} />)
    await screen.findAllByText('Anthropic 官方')
    await userEvent.setup().click(screen.getByRole('button', { name: '切换到 Team Gateway' }))

    expect(await screen.findByRole('alert')).toHaveProperty('textContent', expect.stringContaining('切换请求超时'))
    expect(screen.queryByText('切换结果待确认')).toBeNull()
    expect(screen.getByRole('button', { name: '重试' })).toHaveProperty('disabled', false)
  })

  it('does not let snapshot recovery swallow a structured activation failure', async () => {
    const failure: ProviderActivationFailure = {
      status: 'router-start-failed',
      message: { zh: '模型路由未能启动：端口占用', en: 'The router could not start: port in use' },
      recovery: 'retry'
    }
    const original = fakeClient()
    const request = vi.fn<ProviderConfigClient['request']>(async (method, payload) => {
        if (method === 'provider-config.activate') {
          await original.request(method, payload)
          throw Object.assign(new Error('外层错误'), { providerFailure: failure })
        }
        return original.request(method, payload)
    })
    const client: ProviderConfigClient = { request }
    render(<ModelSwitchSettings client={client} onClose={vi.fn()} />)
    await screen.findAllByText('Anthropic 官方')
    await userEvent.setup().click(screen.getByRole('button', { name: '切换到 Team Gateway' }))

    expect(await screen.findByRole('alert')).toHaveProperty('textContent', expect.stringContaining('模型路由未能启动：端口占用'))
    expect(screen.queryByText('切换结果待确认')).toBeNull()
    expect(screen.getByRole('button', { name: '重试' })).toHaveProperty('disabled', false)
  })
})

function fakeClient(
  sessionTransitions: ProviderSessionActivationTransition[] = [],
  activeProviderId: 'anthropic' | 'team-claude' = 'anthropic'
): ProviderConfigClient {
  let state: ProviderConfigSnapshot = {
    revision: 1,
    activeProviderIds: { 'claude-code': activeProviderId, codex: 'openai' },
    providers: {
      'claude-code': [{
        id: 'anthropic', cli: 'claude-code', name: 'Anthropic 官方',
        endpoint: 'https://api.anthropic.com', model: 'opus', hasApiKey: false
      }, {
        id: 'team-claude', cli: 'claude-code', name: 'Team Gateway',
        endpoint: 'https://gateway.example', model: 'claude-team', hasApiKey: true
      }],
      codex: [{
        id: 'openai', cli: 'codex', name: 'OpenAI 官方', endpoint: 'https://api.openai.com/v1',
        model: 'gpt-5.6-sol', hasApiKey: false
      }, {
        id: 'packy-codex', cli: 'codex', name: 'PackyCode', endpoint: 'https://api.packy.example/v1',
        model: 'gpt-5.6-sol', hasApiKey: true
      }]
    }
  }
  return {
    request: vi.fn(async (method: string, payload: unknown) => {
      if (method === 'provider-config.snapshot') return state
      if (method === 'provider-config.activate') {
        const input = payload as { cli: 'claude-code' | 'codex'; providerId: string }
        state = { ...state, revision: state.revision + 1, activeProviderIds: { ...state.activeProviderIds, [input.cli]: input.providerId } }
        return { ...state, sessionTransitions }
      }
      if (method === 'provider-config.upsert') {
        const input = (payload as { provider: { id?: string } }).provider
        return { provider: { id: input.id ?? 'created' } }
      }
      return state
    })
  }
}
