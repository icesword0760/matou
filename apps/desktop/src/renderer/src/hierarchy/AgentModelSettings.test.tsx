// @vitest-environment jsdom
import { act, cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { AgentModelPreferences, AgentModelSettingsSnapshot } from '@matou/contracts'
import { AgentModelSettings } from './AgentModelSettings'
import { LocaleProvider } from '../i18n/LocaleProvider'

afterEach(cleanup)

describe('AgentModelSettings', () => {
  it('stays compact by default, filters out Codex providers, and saves separate compaction settings', async () => {
    const data = fixture({
      preferences: {
        contextWindows: { anthropic: 'default', 'claude-alt': '512k', 'codex-alt': '512k' },
        compaction: { mode: 'follow', effort: 'follow' }
      }
    })
    const request = vi.fn(async (method: string, payload: unknown) => method === 'agent-model-settings.set'
      ? { ...data, preferences: payload as AgentModelPreferences }
      : data)
    render(<AgentModelSettings client={{ request }} activeProviderId="anthropic" />)

    await screen.findByRole('heading', { name: '对话压缩模型' })
    expect(screen.getByRole('radio', { name: /跟随会话/ })).toHaveProperty('checked', true)
    expect(screen.queryByRole('combobox', { name: '压缩供应商' })).toBeNull()
    expect(screen.queryByText(/跨供应商压缩时/)).toBeNull()

    const user = userEvent.setup()
    await user.click(screen.getByRole('radio', { name: /独立配置/ }))
    const providerSelect = screen.getByRole('combobox', { name: '压缩供应商' }) as HTMLSelectElement
    expect([...providerSelect.options].map(option => option.value)).not.toContain('codex-alt')

    await user.selectOptions(providerSelect, 'claude-alt')
    const compactionModel = screen.getByRole('combobox', { name: '压缩模型' }) as HTMLSelectElement
    expect(screen.getByRole('option', { name: 'Sonnet Alt' })).toHaveProperty('selected', true)
    await user.selectOptions(compactionModel, screen.getByRole('option', { name: '手动填写模型 ID…' }))
    const manualModel = screen.getByRole('textbox', { name: '压缩模型' })
    expect((manualModel as HTMLInputElement).value).toBe('sonnet-alt')
    await user.clear(manualModel)
    await user.type(manualModel, 'vendor/custom-compact')
    await user.selectOptions(screen.getByRole('combobox', { name: '推理强度' }), 'xhigh')
    expect(screen.getByText('推理强度选项为候选，实际可用性以供应商支持为准。')).toBeTruthy()
    expect(screen.getByText(/跨供应商压缩时，当前对话历史会交给所选供应商/)).toBeTruthy()

    await user.click(screen.getByRole('button', { name: '保存' }))
    expect(request).toHaveBeenCalledWith('agent-model-settings.set', {
      contextWindows: { anthropic: 'default', 'claude-alt': '512k', 'codex-alt': '512k' },
      compaction: { mode: 'custom', providerId: 'claude-alt', model: 'vendor/custom-compact', effort: 'xhigh' }
    })
    expect((await screen.findByRole('status')).textContent).toContain('已保存')
  })

  it('shows every provider model by display name and saves the selected raw ID', async () => {
    const data = fixture({
      preferences: {
        contextWindows: { anthropic: 'default', 'claude-alt': '512k', 'antigravity': 'default', 'codex-alt': '512k' },
        compaction: { mode: 'custom', providerId: 'antigravity', model: 'claude-opus-5-5-high', effort: 'high' }
      }
    })
    const request = vi.fn(async (method: string, payload: unknown) => method === 'agent-model-settings.set'
      ? { ...data, preferences: payload as AgentModelPreferences }
      : data)
    render(<AgentModelSettings client={{ request }} />)

    await screen.findByRole('heading', { name: '对话压缩模型' })
    const model = screen.getByRole('combobox', { name: '压缩模型' }) as HTMLSelectElement
    expect(screen.getByRole('option', { name: 'Claude Opus 5.5 High' })).toHaveProperty('selected', true)
    expect(screen.getByRole('option', { name: 'Gemini 3 Pro' })).toBeTruthy()
    expect(screen.getByRole('option', { name: 'Claude Sonnet 5.5' })).toBeTruthy()

    await userEvent.setup().selectOptions(model, screen.getByRole('option', { name: 'Gemini 3 Pro' }))
    expect(screen.getByRole('combobox', { name: '推理强度' })).toHaveProperty('value', 'high')
    await userEvent.setup().click(screen.getByRole('button', { name: '保存' }))
    expect(request).toHaveBeenCalledWith('agent-model-settings.set', {
      contextWindows: { anthropic: 'default', 'claude-alt': '512k', 'antigravity': 'default', 'codex-alt': '512k' },
      compaction: { mode: 'custom', providerId: 'antigravity', model: 'gemini-3-pro', effort: 'high' }
    })
  })

  it('keeps the draft unchanged when opening and leaving manual entry', async () => {
    const data = fixture()
    const request = vi.fn(async () => data)
    render(<AgentModelSettings client={{ request }} />)

    await screen.findByRole('heading', { name: '对话压缩模型' })
    const user = userEvent.setup()
    const model = screen.getByRole('combobox', { name: '压缩模型' }) as HTMLSelectElement
    await user.selectOptions(model, screen.getByRole('option', { name: '手动填写模型 ID…' }))
    expect(screen.getByRole('textbox', { name: '压缩模型' })).toHaveProperty('value', 'sonnet-alt')

    await user.click(screen.getByRole('button', { name: '从列表选择' }))
    const restoredModel = screen.getByRole('combobox', { name: '压缩模型' }) as HTMLSelectElement
    const restoredOption = screen.getByRole('option', { name: 'Sonnet Alt' }) as HTMLOptionElement
    expect(restoredModel.value).toBe(restoredOption.value)
    expect(restoredOption.selected).toBe(true)
    expect(request).not.toHaveBeenCalledWith('agent-model-settings.set', expect.anything())
  })

  it('retains an unknown saved model ID instead of replacing it', async () => {
    const data = fixture({
      preferences: {
        contextWindows: { anthropic: 'default', 'claude-alt': '512k', 'codex-alt': '512k' },
        compaction: { mode: 'custom', providerId: 'claude-alt', model: 'legacy-compaction-model', effort: 'high' }
      }
    })
    render(<AgentModelSettings client={clientFor(data)} />)

    await screen.findByRole('heading', { name: '对话压缩模型' })
    const model = screen.getByRole('combobox', { name: '压缩模型' }) as HTMLSelectElement
    const retained = screen.getByRole('option', { name: 'legacy-compaction-model（不在当前列表中）' }) as HTMLOptionElement
    expect(model.value).toBe(retained.value)
    expect(retained.selected).toBe(true)
    expect(screen.getByRole('option', { name: 'Sonnet Alt' })).toBeTruthy()
  })

  it('clears manual entry and switches model options when the provider changes', async () => {
    const data = fixture()
    render(<AgentModelSettings client={clientFor(data)} />)

    await screen.findByRole('heading', { name: '对话压缩模型' })
    const user = userEvent.setup()
    await user.selectOptions(screen.getByRole('combobox', { name: '压缩模型' }), screen.getByRole('option', { name: '手动填写模型 ID…' }))
    expect(screen.getByRole('textbox', { name: '压缩模型' })).toBeTruthy()

    await user.selectOptions(screen.getByRole('combobox', { name: '压缩供应商' }), 'antigravity')
    expect(screen.queryByRole('textbox', { name: '压缩模型' })).toBeNull()
    expect(screen.getByRole('combobox', { name: '压缩模型' })).toBeTruthy()
    expect(screen.getByRole('option', { name: 'Claude Opus 5.5 High' })).toHaveProperty('selected', true)
    expect(screen.queryByRole('option', { name: 'Sonnet Alt' })).toBeNull()
    expect(screen.queryByRole('option', { name: 'Limited' })).toBeNull()
  })

  it('does not change effort with the model and blocks saving when the kept effort is unsupported', async () => {
    const data = fixture({
      preferences: {
        contextWindows: { anthropic: 'default', 'claude-alt': '512k', 'codex-alt': '512k' },
        compaction: { mode: 'custom', providerId: 'claude-alt', model: 'sonnet-alt', effort: 'max' }
      }
    })
    render(<AgentModelSettings client={clientFor(data)} />)

    await screen.findByRole('heading', { name: '对话压缩模型' })
    await userEvent.setup().selectOptions(screen.getByRole('combobox', { name: '压缩模型' }), screen.getByRole('option', { name: 'Limited' }))
    expect(screen.getByRole('combobox', { name: '推理强度' })).toHaveProperty('value', 'max')
    expect(screen.getByRole('alert')).toHaveProperty('textContent', expect.stringContaining('当前推理强度不受该模型支持'))
    expect(screen.getByRole('button', { name: '保存' })).toHaveProperty('disabled', true)
  })

  it('auto-saves when switching back from custom to follow session', async () => {
    const data = fixture()
    const request = vi.fn(async (method: string, payload: unknown) => method === 'agent-model-settings.set'
      ? { ...data, preferences: payload as AgentModelPreferences }
      : data)
    render(<AgentModelSettings client={{ request }} activeProviderId="claude-alt" />)

    await screen.findByRole('heading', { name: '对话压缩模型' })
    expect(screen.queryByText(/跨供应商压缩时/)).toBeNull()
    expect(screen.getByText(/仅压缩请求使用所选模型与推理强度/)).toBeTruthy()

    await userEvent.setup().click(screen.getByRole('radio', { name: /跟随会话/ }))
    expect(request).toHaveBeenCalledWith('agent-model-settings.set', {
      contextWindows: { anthropic: 'default', 'claude-alt': '512k', 'codex-alt': '512k' },
      compaction: { mode: 'follow', providerId: 'claude-alt', model: 'sonnet-alt', effort: 'medium' }
    })
    expect((await screen.findByRole('status')).textContent).toContain('已保存')
  })

  it('applies the snapshot returned by set to the saved controls', async () => {
    const data = fixture({
      preferences: {
        contextWindows: { anthropic: 'default', 'claude-alt': '512k', 'codex-alt': '512k' },
        compaction: { mode: 'follow', effort: 'follow' }
      }
    })
    const savedPreferences: AgentModelPreferences = {
      contextWindows: { anthropic: 'default', 'claude-alt': '512k', 'codex-alt': '512k' },
      compaction: { mode: 'custom', providerId: 'claude-alt', model: 'limited', effort: 'high' }
    }
    const request = vi.fn(async (method: string) => method === 'agent-model-settings.set'
      ? { ...data, preferences: savedPreferences }
      : data)
    render(<AgentModelSettings client={{ request }} />)

    await screen.findByRole('heading', { name: '对话压缩模型' })
    const user = userEvent.setup()
    await user.click(screen.getByRole('radio', { name: /独立配置/ }))
    await user.selectOptions(screen.getByRole('combobox', { name: '压缩供应商' }), 'claude-alt')
    await user.selectOptions(screen.getByRole('combobox', { name: '压缩模型' }), screen.getByRole('option', { name: 'Limited' }))
    await user.selectOptions(screen.getByRole('combobox', { name: '推理强度' }), 'high')
    await user.click(screen.getByRole('button', { name: '保存' }))

    expect((await screen.findByRole('status')).textContent).toContain('已保存')
    expect(screen.getByRole('combobox', { name: '压缩供应商' })).toHaveProperty('value', 'claude-alt')
    expect(screen.getByRole('option', { name: 'Limited' })).toHaveProperty('selected', true)
    expect(screen.getByRole('combobox', { name: '推理强度' })).toHaveProperty('value', 'high')
    expect(screen.getByRole('button', { name: '保存' })).toHaveProperty('disabled', true)
  })

  it('offers max when the selected model supports it and sends it in the preferences payload', async () => {
    const data = fixture()
    const request = vi.fn(async (method: string, payload: unknown) => method === 'agent-model-settings.set'
      ? { ...data, preferences: payload as AgentModelPreferences }
      : data)
    render(<AgentModelSettings client={{ request }} />)

    await screen.findByRole('heading', { name: '对话压缩模型' })
    const effort = screen.getByRole('combobox', { name: '推理强度' }) as HTMLSelectElement
    expect([...effort.options].map(option => option.value)).toContain('max')
    const user = userEvent.setup()
    await user.selectOptions(effort, 'max')
    await user.click(screen.getByRole('button', { name: '保存' }))
    expect(request).toHaveBeenCalledWith('agent-model-settings.set', {
      contextWindows: { anthropic: 'default', 'claude-alt': '512k', 'codex-alt': '512k' },
      compaction: { mode: 'custom', providerId: 'claude-alt', model: 'sonnet-alt', effort: 'max' }
    })
  })

  it('keeps an unsupported saved effort visible and blocks saving until it is corrected', async () => {
    const data = fixture({
      preferences: {
        contextWindows: { anthropic: 'default', 'claude-alt': '512k', 'codex-alt': '512k' },
        compaction: { mode: 'custom', providerId: 'claude-alt', model: 'limited', effort: 'max' }
      }
    })
    render(<AgentModelSettings client={clientFor(data)} />)

    expect(await screen.findByRole('alert')).toHaveProperty('textContent', expect.stringContaining('当前推理强度不受该模型支持'))
    expect(screen.getByRole('combobox', { name: '推理强度' })).toHaveProperty('value', 'max')
    expect(screen.getByRole('button', { name: '保存' })).toHaveProperty('disabled', true)
    await userEvent.setup().selectOptions(screen.getByRole('combobox', { name: '推理强度' }), 'high')
    expect(screen.queryByRole('alert')).toBeNull()
    expect(screen.getByRole('button', { name: '保存' })).toHaveProperty('disabled', false)
  })

  it('keeps the draft and permits retry when saving fails', async () => {
    const data = fixture({
      preferences: {
        contextWindows: { anthropic: 'default', 'claude-alt': '512k', 'codex-alt': '512k' },
        compaction: { mode: 'follow', effort: 'follow' }
      }
    })
    const request = vi.fn(async (method: string) => {
      if (method === 'agent-model-settings.set') throw new Error('disk full')
      return data
    })
    render(<AgentModelSettings client={{ request }} />)

    await screen.findByRole('heading', { name: '对话压缩模型' })
    const user = userEvent.setup()
    await user.click(screen.getByRole('radio', { name: /独立配置/ }))
    await user.selectOptions(screen.getByRole('combobox', { name: '压缩供应商' }), 'claude-alt')
    await user.selectOptions(screen.getByRole('combobox', { name: '压缩模型' }), screen.getByRole('option', { name: '手动填写模型 ID…' }))
    const model = screen.getByRole('textbox', { name: '压缩模型' })
    await user.clear(model)
    await user.type(model, 'custom-after-failure')
    await user.selectOptions(screen.getByRole('combobox', { name: '推理强度' }), 'high')
    await user.click(screen.getByRole('button', { name: '保存' }))

    expect(await screen.findByRole('alert')).toHaveProperty('textContent', '保存未确认，请重试。')
    expect(screen.getByRole('combobox', { name: '压缩供应商' })).toHaveProperty('value', 'claude-alt')
    expect(screen.getByRole('textbox', { name: '压缩模型' })).toHaveProperty('value', 'custom-after-failure')
    expect(screen.getByRole('combobox', { name: '推理强度' })).toHaveProperty('value', 'high')
    expect(screen.getByRole('button', { name: '保存' })).toHaveProperty('disabled', false)
  })

  it('disables every editable control while a save is pending', async () => {
    const data = fixture()
    let resolveSave!: (value: AgentModelSettingsSnapshot) => void
    const request = vi.fn((method: string) => method === 'agent-model-settings.set'
      ? new Promise<AgentModelSettingsSnapshot>(resolve => { resolveSave = resolve })
      : Promise.resolve(data))
    render(<AgentModelSettings client={{ request }} />)

    await screen.findByRole('heading', { name: '对话压缩模型' })
    const user = userEvent.setup()
    await user.selectOptions(screen.getByRole('combobox', { name: '推理强度' }), 'high')
    await user.click(screen.getByRole('button', { name: '保存' }))

    expect(screen.getByRole('button', { name: '保存中…' })).toHaveProperty('disabled', true)
    for (const control of screen.getAllByRole('radio')) expect(control).toHaveProperty('disabled', true)
    for (const control of screen.getAllByRole('combobox')) expect(control).toHaveProperty('disabled', true)

    await act(async () => {
      resolveSave({
        ...data,
        preferences: {
          contextWindows: data.preferences.contextWindows,
          compaction: { ...data.preferences.compaction, effort: 'high' }
        }
      })
    })
  })

  it('keeps a removed provider selection, shows an error and blocks custom save', async () => {
    const data = fixture({
      preferences: {
        contextWindows: { anthropic: 'default', 'claude-alt': '512k', 'codex-alt': '512k' },
        compaction: { mode: 'custom', providerId: 'removed-provider', model: 'kept-model', effort: 'high' }
      }
    })
    render(<AgentModelSettings client={clientFor(data)} />)

    expect(await screen.findByRole('alert')).toHaveProperty('textContent', expect.stringContaining('压缩供应商已不存在'))
    expect(screen.getByRole('combobox', { name: '压缩供应商' })).toHaveProperty('value', 'removed-provider')
    expect(screen.getByRole('option', { name: 'removed-provider（不可用于压缩）' })).toHaveProperty('disabled', true)
    const retainedModel = screen.getByRole('option', { name: 'kept-model（不在当前列表中）' }) as HTMLOptionElement
    expect(screen.getByRole('combobox', { name: '压缩模型' })).toHaveProperty('value', retainedModel.value)
    expect(screen.getByRole('combobox', { name: '压缩模型' })).toHaveProperty('disabled', true)
    expect(screen.getByRole('button', { name: '保存' })).toHaveProperty('disabled', true)
  })

  it('shows an English missing-model error and blocks save', async () => {
    const data = fixture({
      preferences: {
        contextWindows: { anthropic: 'default', 'claude-alt': '512k', 'codex-alt': '512k' },
        compaction: { mode: 'custom', providerId: 'claude-alt', model: '', effort: 'medium' }
      }
    })
    render(<LocaleProvider initialLocale="en"><AgentModelSettings client={clientFor(data)} /></LocaleProvider>)

    expect(await screen.findByRole('alert')).toHaveProperty('textContent', 'Choose or enter a compaction model.')
    expect(screen.getByRole('combobox', { name: 'Compaction provider' })).toHaveProperty('value', 'claude-alt')
    expect(screen.getByRole('combobox', { name: 'Compaction model' })).toHaveProperty('value', '')
    expect(screen.getByRole('option', { name: 'Choose a compaction model' })).toHaveProperty('disabled', true)
    expect(screen.getByRole('button', { name: 'Save' })).toHaveProperty('disabled', true)
  })

  it('shows a load error and retries the snapshot request', async () => {
    const data = fixture()
    let attempts = 0
    const request = vi.fn(async () => {
      attempts += 1
      if (attempts === 1) throw new Error('offline')
      return data
    })
    render(<AgentModelSettings client={{ request }} />)

    expect(await screen.findByRole('alert')).toHaveProperty('textContent', expect.stringContaining('无法读取压缩模型设置'))
    await userEvent.setup().click(screen.getByRole('button', { name: '重试' }))
    expect(await screen.findByRole('heading', { name: '对话压缩模型' })).toBeTruthy()
    expect(request).toHaveBeenCalledTimes(2)
  })
})

function clientFor(data: AgentModelSettingsSnapshot) {
  return {
    request: vi.fn(async (method: string, payload: unknown) => method === 'agent-model-settings.set'
      ? { ...data, preferences: payload as AgentModelPreferences }
      : data)
  }
}

function fixture(overrides: Partial<AgentModelSettingsSnapshot> = {}): AgentModelSettingsSnapshot {
  return {
    preferences: {
      contextWindows: { anthropic: 'default', 'claude-alt': '512k', 'codex-alt': '512k' },
      compaction: { mode: 'custom', providerId: 'claude-alt', model: 'sonnet-alt', effort: 'medium' }
    },
    providers: [{
      id: 'anthropic', name: 'Anthropic 官方', cli: 'claude-code', compactionAvailable: false,
      models: [{ id: 'opus', name: 'Opus', defaultTokens: 200_000, maximumTokens: 1_000_000, fixedTokens: 200_000 }]
    }, {
      id: 'claude-alt', name: 'Claude Alt', cli: 'claude-code', compactionAvailable: true,
      models: [{
        id: 'sonnet-alt', name: 'Sonnet Alt', defaultTokens: 200_000, maximumTokens: 512_000, fixedTokens: 512_000,
        supportedEfforts: ['low', 'medium', 'high', 'xhigh', 'max']
      }, {
        id: 'limited', name: 'Limited', defaultTokens: 128_000, maximumTokens: 200_000, fixedTokens: 200_000,
        supportedEfforts: ['low', 'medium', 'high']
      }, {
        id: 'sparse', name: 'Sparse', fixedTokens: 512_000
      }]
    }, {
      id: 'antigravity', name: 'Antigravity', cli: 'claude-code', compactionAvailable: true,
      models: [{
        id: 'claude-opus-5-5-high', name: 'Claude Opus 5.5 High', defaultTokens: 200_000, maximumTokens: 1_000_000, fixedTokens: 200_000,
        supportedEfforts: ['low', 'high', 'max']
      }, {
        id: 'gemini-3-pro', name: 'Gemini 3 Pro', defaultTokens: 1_000_000, maximumTokens: 1_000_000, fixedTokens: 1_000_000,
        supportedEfforts: ['low', 'medium', 'high']
      }, {
        id: 'claude-sonnet-5-5', name: 'Claude Sonnet 5.5', defaultTokens: 200_000, maximumTokens: 1_000_000, fixedTokens: 200_000,
        supportedEfforts: ['low', 'medium', 'high']
      }]
    }, {
      id: 'codex-alt', name: 'Codex Alt', cli: 'codex', compactionAvailable: false,
      models: [{ id: 'gpt-codex', name: 'GPT Codex', defaultTokens: 272_000, maximumTokens: 1_000_000, fixedTokens: 272_000 }]
    }],
    ...overrides
  }
}
