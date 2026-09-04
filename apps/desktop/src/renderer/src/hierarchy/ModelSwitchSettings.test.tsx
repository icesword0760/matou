// @vitest-environment jsdom
import { cleanup, render, screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type {
  ProviderConfigSnapshot,
  ProviderSessionActivationTransition
} from '@matou/contracts'
import { ModelSwitchSettings, type ProviderConfigClient } from './ModelSwitchSettings'

afterEach(cleanup)

describe('ModelSwitchSettings', () => {
  it('shows separate global providers for Claude Code and Codex and activates a card', async () => {
    const client = fakeClient()
    render(<ModelSwitchSettings client={client} onClose={vi.fn()} />)

    expect((await screen.findAllByText('Anthropic 官方')).length).toBeGreaterThan(0)
    expect(screen.queryByRole('heading', { name: '设置' })).toBeNull()
    await userEvent.setup().click(screen.getByRole('tab', { name: 'Codex' }))
    expect(screen.getAllByText('OpenAI 官方').length).toBeGreaterThan(0)
    await userEvent.setup().click(screen.getByRole('button', { name: '切换到 PackyCode' }))

    expect(client.request).toHaveBeenCalledWith('provider-config.activate', {
      cli: 'codex', providerId: 'packy-codex'
    })
    expect(await screen.findByText('已切换为 PackyCode；Codex 新会话直接生效')).toBeTruthy()
  })

  it('adds a provider from the compact core configuration form', async () => {
    const client = fakeClient()
    render(<ModelSwitchSettings client={client} onClose={vi.fn()} />)
    await screen.findAllByText('Anthropic 官方')
    const user = userEvent.setup()

    await user.click(screen.getByRole('button', { name: '新增供应商' }))
    const dialog = screen.getByRole('dialog', { name: '新增供应商' })
    await user.clear(within(dialog).getByRole('textbox', { name: '供应商名称' }))
    await user.type(within(dialog).getByRole('textbox', { name: '供应商名称' }), 'Team Gateway')
    await user.clear(within(dialog).getByRole('textbox', { name: 'API 地址' }))
    await user.type(within(dialog).getByRole('textbox', { name: 'API 地址' }), 'https://gateway.example')
    await user.clear(within(dialog).getByRole('textbox', { name: '默认模型' }))
    await user.type(within(dialog).getByRole('textbox', { name: '默认模型' }), 'claude-team')
    await user.type(within(dialog).getByLabelText('API Key'), 'TOKEN')
    await user.click(within(dialog).getByRole('button', { name: '保存配置' }))

    expect(client.request).toHaveBeenCalledWith('provider-config.upsert', {
      provider: {
        cli: 'claude-code', name: 'Team Gateway', endpoint: 'https://gateway.example',
        model: 'claude-team', apiKey: 'TOKEN'
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
  })
})

function fakeClient(
  sessionTransitions: ProviderSessionActivationTransition[] = []
): ProviderConfigClient {
  let state: ProviderConfigSnapshot = {
    revision: 1,
    activeProviderIds: { 'claude-code': 'anthropic', codex: 'openai' },
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
        state = { ...state, activeProviderIds: { ...state.activeProviderIds, [input.cli]: input.providerId } }
        return { ...state, sessionTransitions }
      }
      if (method === 'provider-config.upsert') {
        return { provider: { id: 'created' } }
      }
      return state
    })
  }
}
