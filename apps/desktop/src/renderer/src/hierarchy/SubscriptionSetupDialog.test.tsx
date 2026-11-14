// @vitest-environment jsdom
import { cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { ProviderConfigInput, ProviderConfigView, SubscriptionAccountDetails } from '@matou/contracts'
import { SubscriptionSetupDialog } from './SubscriptionSetupDialog'

afterEach(cleanup)

describe('SubscriptionSetupDialog reasoning effort mapping', () => {
  it('refreshes late maximum-window metadata without overwriting edited role choices', async () => {
    const models = roleModels().map(model => ({ ...model, contextWindow: 272_000 }))
    const existing = subscriptionProvider(models)
    const props = { provider: 'codex' as const, existing, riskAccepted: true,
      client: { request: vi.fn() }, contextWindowMode: 'maximum' as const,
      onClose: vi.fn(), onRiskAccepted: vi.fn(), onDetails: vi.fn(),
      onSave: vi.fn(async () => {}), onDelete: vi.fn(async () => {}) }
    const view = render(<SubscriptionSetupDialog {...props} />)
    expect(screen.getByRole('radio', { name: /最大可扩展/ }).closest('label')?.textContent).toContain('272,000')
    await userEvent.setup().selectOptions(screen.getByRole('combobox', { name: 'Opus 模型' }), 'gpt-sonnet')
    await userEvent.setup().selectOptions(screen.getByRole('combobox', { name: 'Opus 推理强度' }), 'medium')
    view.rerender(<SubscriptionSetupDialog {...props} details={{ account: { status: 'ready' },
      models: models.map(model => ({ ...model, maxContextWindow: 872_000 })) }} />)
    expect(screen.getByRole('radio', { name: /最大可扩展/ }).closest('label')?.textContent).toContain('872,000')
    expect(screen.getByRole('radio', { name: /最大可扩展/ })).toHaveProperty('checked', true)
    expect(screen.getByRole('combobox', { name: 'Opus 模型' })).toHaveProperty('value', 'gpt-sonnet')
    expect(screen.getByRole('combobox', { name: 'Opus 推理强度' })).toHaveProperty('value', 'medium')
  })
  it('reopens saved role efforts and includes them in the save payload', async () => {
    const models = roleModels()
    const existing = subscriptionProvider(models, { opus: 'max', sonnet: 'follow', haiku: 'high' })
    const onSave = vi.fn(async (_input: ProviderConfigInput, _use: boolean) => {})
    renderDialog(existing, models, onSave)

    expect(screen.getByRole('combobox', { name: 'Opus 模型' })).toHaveProperty('value', 'gpt-opus')
    expect(screen.getByRole('combobox', { name: 'Opus 推理强度' })).toHaveProperty('value', 'max')
    expect(screen.getByRole('combobox', { name: 'Sonnet 推理强度' })).toHaveProperty('value', 'follow')
    expect(screen.getByRole('combobox', { name: 'Haiku 推理强度' })).toHaveProperty('value', 'high')
    expect(optionValues(screen.getByRole('combobox', { name: 'Opus 推理强度' }))).toEqual(['follow', 'low', 'high', 'max'])
    expect(optionValues(screen.getByRole('combobox', { name: 'Haiku 推理强度' }))).toEqual(['follow', 'low', 'medium', 'high', 'xhigh'])
    expect(screen.getByText('推理强度选项为候选，实际可用性以供应商支持为准。')).toBeTruthy()

    await userEvent.setup().click(screen.getByRole('button', { name: '保存' }))
    expect(onSave).toHaveBeenCalledWith(expect.objectContaining({
      modelMapping: { opus: 'gpt-opus', sonnet: 'gpt-sonnet', haiku: 'gpt-haiku' },
      effortMapping: { opus: 'max', sonnet: 'follow', haiku: 'high' }
    }), false)
  })

  it('defaults every role to follow when no effort mapping or metadata is available', async () => {
    const models = roleModels().map(({ supportedEfforts: _supportedEfforts, ...model }) => model)
    const existing = subscriptionProvider(models)
    const onSave = vi.fn(async (_input: ProviderConfigInput, _use: boolean) => {})
    renderDialog(existing, models, onSave)

    for (const role of ['Opus', 'Sonnet', 'Haiku']) {
      expect(screen.getByRole('combobox', { name: `${role} 推理强度` })).toHaveProperty('value', 'follow')
      expect(optionValues(screen.getByRole('combobox', { name: `${role} 推理强度` }))[0]).toBe('follow')
    }
    expect(screen.getByText('推理强度选项为候选，实际可用性以供应商支持为准。')).toBeTruthy()
    await userEvent.setup().click(screen.getByRole('button', { name: '保存' }))
    expect(onSave).toHaveBeenCalledWith(expect.objectContaining({
      effortMapping: { opus: 'follow', sonnet: 'follow', haiku: 'follow' }
    }), false)
  })

  it('keeps a saved unsupported effort visible, reports it and blocks saving', async () => {
    const models = roleModels().map(({ supportedEfforts: _supportedEfforts, ...model }) => model)
    const existing = subscriptionProvider(models, { opus: 'follow', sonnet: 'follow', haiku: 'max' })
    const onSave = vi.fn(async (_input: ProviderConfigInput, _use: boolean) => {})
    renderDialog(existing, models, onSave)

    expect(screen.getByRole('combobox', { name: 'Haiku 推理强度' })).toHaveProperty('value', 'max')
    expect(screen.getByText('该模型不支持当前推理强度。')).toBeTruthy()
    expect(screen.getByRole('button', { name: '保存' })).toHaveProperty('disabled', true)
    expect(onSave).not.toHaveBeenCalled()
  })

  it('computes the effective context window from mapped subscription models and emits changes', async () => {
    const models: SubscriptionAccountDetails['models'] = [
      { id: 'gpt-opus', displayName: 'GPT Opus', contextWindow: 272_000, maxContextWindow: 872_000 },
      { id: 'gpt-sonnet', displayName: 'GPT Sonnet', contextWindow: 272_000, maxContextWindow: 872_000 },
      { id: 'gpt-haiku', displayName: 'GPT Haiku', contextWindow: 272_000, maxContextWindow: 400_000 }
    ]
    const onContextWindowChange = vi.fn()
    const details: SubscriptionAccountDetails = { account: { status: 'ready' }, models }
    render(<SubscriptionSetupDialog
      provider="codex"
      existing={subscriptionProvider(models)}
      details={details}
      riskAccepted
      client={{ request: vi.fn() }}
      contextWindowMode="512k"
      onContextWindowChange={onContextWindowChange}
      onClose={vi.fn()}
      onRiskAccepted={vi.fn()}
      onDetails={vi.fn()}
      onSave={vi.fn(async () => {})}
      onDelete={vi.fn(async () => {})}
    />)

    expect(screen.getByRole('radio', { name: /512K/ })).toHaveProperty('checked', true)
    expect(screen.getByText('272,000 tokens')).toBeTruthy()
    expect(screen.getByText('512,000 tokens')).toBeTruthy()
    expect(screen.getByText('400,000 tokens')).toBeTruthy()
    await userEvent.setup().click(screen.getByRole('radio', { name: /最大可扩展/ }))
    expect(onContextWindowChange).toHaveBeenCalledWith('maximum')
  })
})

function renderDialog(existing: ProviderConfigView, models: SubscriptionAccountDetails['models'], onSave: (input: ProviderConfigInput, use: boolean) => Promise<void>) {
  const details: SubscriptionAccountDetails = { account: { status: 'ready' }, models }
  return render(<SubscriptionSetupDialog provider="codex" existing={existing} details={details} riskAccepted client={{ request: vi.fn() }}
    onClose={vi.fn()} onRiskAccepted={vi.fn()} onDetails={vi.fn()} onSave={onSave} onDelete={vi.fn(async () => {})} />)
}

function subscriptionProvider(models: SubscriptionAccountDetails['models'], effortMapping?: ProviderConfigView['effortMapping']): ProviderConfigView {
  return {
    id: 'chatgpt', cli: 'claude-code', kind: 'subscription', name: 'ChatGPT', endpoint: '', model: 'gpt-6.1-sol', hasApiKey: false,
    modelMapping: { opus: 'gpt-opus', sonnet: 'gpt-sonnet', haiku: 'gpt-haiku' },
    ...(effortMapping ? { effortMapping } : {}),
    subscription: { provider: 'codex', account: { status: 'ready' }, modelMetadata: models }
  }
}

function roleModels(): SubscriptionAccountDetails['models'] {
  return [
    { id: 'gpt-opus', displayName: 'GPT Opus', supportedEfforts: ['low', 'high', 'max'] },
    { id: 'gpt-sonnet', displayName: 'GPT Sonnet', supportedEfforts: ['low', 'medium'] },
    { id: 'gpt-haiku', displayName: 'GPT Haiku' }
  ]
}

function optionValues(select: HTMLElement) {
  return [...(select as HTMLSelectElement).options].map(option => option.value)
}
