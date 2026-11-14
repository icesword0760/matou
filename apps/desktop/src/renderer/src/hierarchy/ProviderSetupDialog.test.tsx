// @vitest-environment jsdom
import { act, cleanup, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { ProviderConfigInput, ProviderConfigView, ProviderModelMetadata } from '@matou/contracts'
import { ProviderSetupDialog } from './ProviderSetupDialog'
import { LocaleProvider } from '../i18n/LocaleProvider'
import { setCurrentLocale } from '../i18n/current'

afterEach(() => { cleanup(); setCurrentLocale('zh-CN') })
const setup = () => {
  const props = { cli: 'claude-code' as const, onClose: vi.fn(), onSave: vi.fn(async () => {}), onUseOfficial: vi.fn(async () => {}), onDelete: vi.fn(async () => {}), client: { request: vi.fn(async () => ({ models: ['model-a','model-b'] })) } }
  render(<ProviderSetupDialog {...props} />)
  return { ...props, user: userEvent.setup() }
}
const choose = async (user: ReturnType<typeof userEvent.setup>) => {
  await user.click(screen.getByRole('button', { name: /DeepSeek/ }))
  await user.click(screen.getByRole('button', { name: '下一步' }))
}
const DEEPSEEK_METADATA: ProviderModelMetadata[] = [
  { id: 'deepseek-flash', displayName: 'DeepSeek-V4.1-Flash', contextWindow: 1_048_576, maxContextWindow: 1_048_576, maxOutputTokens: 393_216, supportedEfforts: ['low', 'high', 'max'] },
  { id: 'deepseek-v4-pro', displayName: 'DeepSeek-V4-Pro', contextWindow: 1_048_576, maxContextWindow: 1_048_576, maxOutputTokens: 393_216, supportedEfforts: ['low', 'high', 'max'] }
]

describe('ProviderSetupDialog', () => {
  it('starts with focused search, filters presets and offers custom fallback', async () => {
    const { user } = setup()
    expect(document.activeElement).toBe(screen.getByRole('searchbox'))
    await user.type(screen.getByRole('searchbox'), 'no-such-service')
    expect(screen.queryByRole('button', { name: /DeepSeek/ })).toBeNull()
    expect(screen.getByRole('button', { name: /自定义服务/ })).toBeTruthy()
    expect(screen.getByRole('button', { name: '下一步' })).toHaveProperty('disabled', true)
  })
  it('prefills a preset, fetches models from the unsaved draft and adds without activating', async () => {
    const { user, client, onSave } = setup(); await choose(user)
    expect(screen.getByLabelText('API 地址')).toHaveProperty('value','https://api.deepseek.com/anthropic')
    await user.type(screen.getByLabelText('API Key'), 'TOKEN')
    await user.click(screen.getByRole('button', { name: '获取模型' }))
    const picker = await screen.findByRole('listbox', { name: '可用模型' })
    await user.click(within(picker).getByRole('option', { name: /model-b/ }))
    await user.click(screen.getByRole('button', { name: '仅保存' }))
    expect(client.request).toHaveBeenCalledWith('provider-config.models', { cli:'claude-code',endpoint:'https://api.deepseek.com/anthropic',apiKey:'TOKEN' })
    expect(onSave).toHaveBeenCalledWith(expect.objectContaining({ name:'DeepSeek',model:'model-b',modelMapping:expect.objectContaining({haiku:'deepseek-flash'}) }),false)
  })
  it('shows fetched names and windows, searches names and IDs, then saves the raw ID with metadata', async () => {
    const { user, client, onSave } = setup(); await choose(user)
    client.request.mockResolvedValue({ models: ['deepseek-flash', 'deepseek-v4-pro'], modelMetadata: DEEPSEEK_METADATA } as never)
    await user.type(screen.getByLabelText('API Key'), 'TOKEN')
    await user.click(screen.getByRole('button', { name: '获取模型' }))

    expect((await screen.findAllByText('DeepSeek-V4-Pro')).length).toBeGreaterThan(0)
    expect(screen.getAllByText('1,048,576 tokens').length).toBeGreaterThan(0)
    expect(screen.getByText('已在本次获取的模型列表中确认该 ID。')).toBeTruthy()
    expect(screen.getByText(/已获取 2 个模型；尚未验证实际对话能力/)).toBeTruthy()
    expect(screen.queryByText('更多设置')).toBeNull()
    expect(screen.getByLabelText('供应商名称')).toHaveProperty('value', 'DeepSeek')
    const opusModel = screen.getByRole('combobox', { name: 'Opus 模型' })
    expect(within(opusModel).getByRole('option', { name: 'DeepSeek-V4-Pro' })).toBeTruthy()
    expect(within(opusModel).getByRole('option', { name: 'DeepSeek-V4.1-Flash' })).toBeTruthy()
    expect(optionValues(screen.getByRole('combobox', { name: 'Opus 推理强度' }))).toEqual(['follow', 'low', 'high', 'max'])
    expect(screen.queryByText(/推理强度选项为候选/)).toBeNull()

    const search = screen.getByRole('searchbox', { name: '搜索模型' })
    const listbox = screen.getByRole('listbox', { name: '可用模型' })
    await waitFor(() => expect(document.activeElement).toBe(search))
    await user.type(search, 'V4.1')
    expect(within(listbox).getByRole('option', { name: /DeepSeek-V4.1-Flash/ })).toBeTruthy()
    expect(within(listbox).queryByRole('option', { name: /DeepSeek-V4-Pro/ })).toBeNull()
    await user.clear(search); await user.type(search, 'deepseek-v4-pro')
    expect(within(listbox).getByRole('option', { name: /DeepSeek-V4-Pro/ })).toBeTruthy()
    expect(within(listbox).queryByRole('option', { name: /DeepSeek-V4.1-Flash/ })).toBeNull()

    await user.clear(search); await user.type(search, 'V4.1')
    await user.click(within(listbox).getByRole('option', { name: /DeepSeek-V4.1-Flash/ }))
    const modelInput = screen.getByLabelText('默认模型')
    expect(modelInput).toHaveProperty('value', 'deepseek-flash')
    expect(document.activeElement).toBe(modelInput)
    await user.click(screen.getByRole('button', { name: '从已获取列表选择' }))
    await waitFor(() => expect(document.activeElement).toBe(screen.getByRole('searchbox', { name: '搜索模型' })))
    await user.click(within(screen.getByRole('listbox', { name: '可用模型' })).getByRole('option', { name: /DeepSeek-V4.1-Flash/ }))
    expect(document.activeElement).toBe(modelInput)
    await user.click(screen.getByRole('button', { name: '仅保存' }))
    await waitFor(() => expect(onSave).toHaveBeenCalledWith(expect.objectContaining({
      model: 'deepseek-flash',
      modelMetadata: DEEPSEEK_METADATA
    }), false))
  })
  it('repositions the open picker when filtering changes the visible result count', async () => {
    const originalScrollIntoView = HTMLElement.prototype.scrollIntoView
    const scrollIntoView = vi.fn()
    HTMLElement.prototype.scrollIntoView = scrollIntoView
    try {
      const { user, client } = setup(); await choose(user)
      client.request.mockResolvedValue({ models: ['deepseek-flash', 'deepseek-v4-pro'], modelMetadata: DEEPSEEK_METADATA } as never)
      await user.type(screen.getByLabelText('API Key'), 'TOKEN')
      await user.click(screen.getByRole('button', { name: '获取模型' }))
      await within(await screen.findByRole('listbox', { name: '可用模型' })).findByRole('option', { name: /DeepSeek-V4-Pro/ })
      const search = screen.getByRole('searchbox', { name: '搜索模型' })
      await waitFor(() => expect(document.activeElement).toBe(search))

      scrollIntoView.mockClear()
      await user.type(search, 'V4.1')
      expect(within(screen.getByRole('listbox', { name: '可用模型' })).getAllByRole('option')).toHaveLength(1)
      await waitFor(() => expect(scrollIntoView).toHaveBeenCalledWith({ block: 'nearest' }))

      scrollIntoView.mockClear()
      await user.clear(search)
      expect(within(screen.getByRole('listbox', { name: '可用模型' })).getAllByRole('option')).toHaveLength(2)
      await waitFor(() => expect(scrollIntoView).toHaveBeenCalledWith({ block: 'nearest' }))

      scrollIntoView.mockClear()
      await user.type(search, 'deepseek-v4')
      expect(within(screen.getByRole('listbox', { name: '可用模型' })).getAllByRole('option')).toHaveLength(2)
      expect(scrollIntoView).not.toHaveBeenCalled()
    } finally {
      HTMLElement.prototype.scrollIntoView = originalScrollIntoView
    }
  })
  it('labels a changed preset ID as manual instead of keeping the preset hint', async () => {
    const { user } = setup(); await choose(user)
    expect(screen.getByText('预设模型尚未向服务验证名称、窗口或对话能力。')).toBeTruthy()
    await user.clear(screen.getByLabelText('默认模型'))
    await user.type(screen.getByLabelText('默认模型'), 'another-model')
    expect(screen.getByText('手动填写模型 ID；名称与窗口未知。')).toBeTruthy()
    expect(screen.queryByText('预设模型尚未向服务验证名称、窗口或对话能力。')).toBeNull()
  })
  it('reopens saved metadata with its real name and saved-source hint', () => {
    const existing: ProviderConfigView = {
      id: 'deepseek', cli: 'claude-code', name: 'DeepSeek', endpoint: 'https://api.deepseek.com/anthropic',
      model: 'deepseek-v4-pro', hasApiKey: true, modelMetadata: DEEPSEEK_METADATA
    }
    render(<ProviderSetupDialog cli="claude-code" existing={existing} client={null} onClose={vi.fn()} onSave={vi.fn(async () => {})} onUseOfficial={vi.fn(async () => {})} onDelete={vi.fn(async () => {})} />)

    expect(screen.getAllByText('DeepSeek-V4-Pro').length).toBeGreaterThan(0)
    expect(screen.getByText('显示已保存的模型信息，可获取模型刷新。')).toBeTruthy()
    expect(screen.queryByText(/预设模型尚未向服务验证/)).toBeNull()
  })
  it('treats a successful response without metadata as an explicit empty result and keeps the change dirty', async () => {
    const existing: ProviderConfigView = {
      id: 'official', cli: 'claude-code', name: 'Anthropic 官方', endpoint: 'https://api.anthropic.com',
      model: 'claude-a', hasApiKey: true, builtIn: true
    }
    const client = { request: vi.fn(async () => ({ models: ['claude-a'] })) }
    const onSave = vi.fn(async (_input: ProviderConfigInput, _use: boolean) => {})
    render(<ProviderSetupDialog cli="claude-code" existing={existing} client={client} contextWindowMode="default"
      contextModels={[{ id: 'claude-a', name: 'Claude A', defaultTokens: 1_000_000, maximumTokens: 1_000_000, fixedTokens: 512_000 }]}
      onClose={vi.fn()} onSave={onSave} onUseOfficial={vi.fn(async () => {})} onDelete={vi.fn(async () => {})} />)

    expect(screen.getAllByText('1,000,000 tokens')).toHaveLength(2)
    await userEvent.setup().click(screen.getByRole('button', { name: '获取模型' }))
    expect(await screen.findByText(/已获取 1 个模型/)).toBeTruthy()
    expect(screen.queryByText('1,000,000 tokens')).toBeNull()
    expect(screen.getByText('已在本次获取的模型列表中确认该 ID。')).toBeTruthy()
    expect(screen.getByText('本次响应只返回模型 ID，未提供显示名称与窗口。')).toBeTruthy()
    expect(screen.getByText('服务未提供完整的窗口信息，将沿用 CLI 默认设置。')).toBeTruthy()

    await userEvent.setup().click(screen.getByRole('button', { name: '关闭' }))
    expect(screen.getByText('有尚未保存的修改')).toBeTruthy()
    await userEvent.setup().click(screen.getByRole('button', { name: '继续编辑' }))
    await userEvent.setup().click(screen.getByRole('button', { name: '保存' }))
    await waitFor(() => expect(onSave).toHaveBeenCalledWith(expect.objectContaining({ modelMetadata: [] }), false))
  })
  it('replaces a successful fetch message on failure and labels retained metadata as last recorded', async () => {
    const { user, client } = setup(); await choose(user)
    client.request.mockResolvedValue({ models: ['deepseek-flash', 'deepseek-v4-pro'], modelMetadata: DEEPSEEK_METADATA } as never)
    await user.type(screen.getByLabelText('API Key'), 'TOKEN')
    await user.click(screen.getByRole('button', { name: '获取模型' }))
    expect(await screen.findByText(/已获取 2 个模型；尚未验证实际对话能力/)).toBeTruthy()

    client.request.mockResolvedValue({ error: 'network' } as never)
    await user.click(screen.getByRole('button', { name: '获取模型' }))
    expect(await screen.findByText(/连接未完成/)).toBeTruthy()
    expect(screen.queryByText(/已获取 2 个模型；尚未验证实际对话能力/)).toBeNull()
    expect(screen.getByText(/本次刷新失败；以下为上次记录的模型信息/)).toBeTruthy()
    expect(screen.getAllByText('DeepSeek-V4-Pro').length).toBeGreaterThan(0)
  })
  it.each([
    ['API 地址', 'https://changed.example'],
    ['API Key', 'NEW-TOKEN']
  ])('clears fetched model information immediately after changing %s', async (label, value) => {
    const existing: ProviderConfigView = {
      id: 'deepseek', cli: 'claude-code', name: 'DeepSeek', endpoint: 'https://api.deepseek.com/anthropic',
      model: 'deepseek-v4-pro', hasApiKey: true, modelMetadata: DEEPSEEK_METADATA
    }
    const user = userEvent.setup()
    render(<ProviderSetupDialog cli="claude-code" existing={existing} client={null} onClose={vi.fn()} onSave={vi.fn(async () => {})} onUseOfficial={vi.fn(async () => {})} onDelete={vi.fn(async () => {})} />)
    expect(screen.getAllByText('DeepSeek-V4-Pro').length).toBeGreaterThan(0)
    const field = screen.getByLabelText(label)
    await user.clear(field); await user.type(field, value)
    expect(screen.queryByText('DeepSeek-V4-Pro')).toBeNull()
    expect(screen.queryByText('1,048,576 tokens')).toBeNull()
    expect(screen.getByText('当前仅有模型 ID；尚未获取名称或窗口信息。')).toBeTruthy()
  })
  it('includes the default model plus every non-empty mapping when resolving a window', async () => {
    const metadata: ProviderModelMetadata[] = [
      { id: 'default-model', displayName: 'Default Model', contextWindow: 32_768, maxContextWindow: 65_536 },
      { id: 'flash-model', displayName: 'Flash Model', contextWindow: 1_048_576, maxContextWindow: 1_048_576 }
    ]
    const existing: ProviderConfigView = {
      id: 'custom', cli: 'claude-code', name: 'Custom', endpoint: 'https://custom.example', model: 'default-model', hasApiKey: true,
      modelMetadata: metadata, modelMapping: { opus: 'flash-model', sonnet: 'missing-model', haiku: '' }
    }
    const user = userEvent.setup()
    render(<ProviderSetupDialog cli="claude-code" existing={existing} client={null} contextWindowMode="default"
      contextModels={[{ id: 'default', name: 'Stale native', defaultTokens: 999_999, maximumTokens: 999_999, fixedTokens: 512_000 }]}
      onClose={vi.fn()} onSave={vi.fn(async () => {})} onUseOfficial={vi.fn(async () => {})} onDelete={vi.fn(async () => {})} />)

    expect(screen.queryByText('999,999 tokens')).toBeNull()
    expect(screen.getByText('服务未提供完整的窗口信息，将沿用 CLI 默认设置。')).toBeTruthy()
    expect(screen.getAllByText(/Flash Model/).length).toBeGreaterThan(0)
    const sonnetModel = screen.getByRole('combobox', { name: 'Sonnet 模型' })
    expect(sonnetModel).toHaveProperty('value', 'provider-model:missing-model')
    const missingOption = within(sonnetModel).getByRole('option', { name: 'missing-model（尚未获取列表）' })
    expect(missingOption).toBeTruthy()
    await user.selectOptions(sonnetModel, within(sonnetModel).getByRole('option', { name: 'Flash Model' }))
    expect((await screen.findAllByText('32,768 tokens')).length).toBeGreaterThan(0)
  })
  it('keeps the draft on discovery failure and allows manual entry', async () => {
    const { user, client, onSave } = setup()
    client.request.mockResolvedValue({ error: 'unsupported' } as never)
    await choose(user); await user.type(screen.getByLabelText('API Key'),'TOKEN')
    await user.click(screen.getByRole('button', { name:'获取模型' }))
    expect(await screen.findByText(/暂不支持模型列表/)).toBeTruthy()
    await user.clear(screen.getByLabelText('默认模型')); await user.type(screen.getByLabelText('默认模型'),'manual-model')
    await user.click(screen.getByRole('button', { name:'添加并使用' }))
    expect(onSave).toHaveBeenCalledWith(expect.objectContaining({model:'manual-model',apiKey:'TOKEN'}),true)
  })
  it('protects unsaved keys on Escape and changing presets', async () => {
    const { user,onClose } = setup(); await choose(user)
    await user.type(screen.getByLabelText('API Key'),'TOKEN')
    await user.keyboard('{Escape}'); expect(onClose).not.toHaveBeenCalled()
    expect(screen.getByText('有尚未保存的修改')).toBeTruthy()
    await user.click(screen.getByRole('button',{name:'继续编辑'}))
    await user.click(screen.getByRole('button',{name:'更换'}))
    await user.click(screen.getByRole('button',{name:'放弃修改'}))
    expect(screen.getByRole('searchbox')).toBeTruthy()
  })
  it('keeps failures in the dialog and blocks duplicate submissions', async () => {
    const { user,onSave } = setup(); await choose(user)
    await user.type(screen.getByLabelText('API Key'),'TOKEN')
    let reject!: (e: Error) => void
    onSave.mockImplementation(() => new Promise((_resolve,no) => { reject = no }))
    await user.click(screen.getByRole('button',{name:'添加并使用'}))
    expect(screen.getByRole('button',{name:'处理中…'})).toHaveProperty('disabled',true)
    reject(new Error('磁盘保存失败'))
    expect(await screen.findByRole('alert')).toHaveProperty('textContent','磁盘保存失败')
    expect(screen.getByLabelText('API Key')).toHaveProperty('value','TOKEN')
  })
  it('ignores an old model response after the endpoint changes', async () => {
    const { user, client } = setup(); await choose(user)
    await user.type(screen.getByLabelText('API Key'), 'TOKEN')
    let resolve!: (value: {models:string[]}) => void
    client.request.mockImplementation(() => new Promise(yes => { resolve = yes }))
    await user.click(screen.getByRole('button',{name:'获取模型'}))
    await user.clear(screen.getByLabelText('API 地址'))
    await user.type(screen.getByLabelText('API 地址'), 'https://changed.example')
    await act(async () => resolve({models:['stale-model']}))
    expect(screen.queryByRole('option', {name:'stale-model'})).toBeNull()
    expect(screen.getByRole('button',{name:'获取模型'})).toHaveProperty('disabled', false)
  })
  it('renders the same workflow in English', async () => {
    const user = userEvent.setup()
    render(<LocaleProvider initialLocale="en"><ProviderSetupDialog cli="codex" client={null} onClose={vi.fn()} onSave={vi.fn()} onUseOfficial={vi.fn()} onDelete={vi.fn()} /></LocaleProvider>)
    await user.click(screen.getByRole('button',{name:/DeepSeek/})); await user.click(screen.getByRole('button',{name:'Next'}))
    expect(screen.getByLabelText('API URL')).toHaveProperty('value','https://api.deepseek.com')
    expect(screen.getByRole('button',{name:'Add and use'})).toBeTruthy()
  })

  it('reopens per-role model effort mappings and saves them for Claude Code', async () => {
    const existing: ProviderConfigView = {
      id: 'gateway', cli: 'claude-code', name: 'Gateway', endpoint: 'https://gateway.example', model: 'claude-default', hasApiKey: true,
      modelMapping: { opus: 'claude-opus', sonnet: '', haiku: 'gpt-5' },
      effortMapping: { opus: 'max', sonnet: 'follow', haiku: 'high' }
    }
    const onSave = vi.fn(async (_input: ProviderConfigInput, _use: boolean) => {})
    render(<ProviderSetupDialog cli="claude-code" existing={existing} client={null} onClose={vi.fn()} onSave={onSave} onUseOfficial={vi.fn()} onDelete={vi.fn()} />)

    expect(screen.getByRole('combobox', { name: 'Opus 模型' })).toHaveProperty('value', 'provider-model:claude-opus')
    expect((screen.getByRole('combobox', { name: 'Sonnet 模型' }) as HTMLSelectElement).selectedOptions[0]?.textContent).toBe('沿用 CLI 默认')
    expect(screen.getByRole('combobox', { name: 'Haiku 模型' })).toHaveProperty('value', 'provider-model:gpt-5')
    expect(screen.getByRole('combobox', { name: 'Opus 推理强度' })).toHaveProperty('value', 'max')
    expect(screen.getByRole('combobox', { name: 'Sonnet 推理强度' })).toHaveProperty('value', 'follow')
    expect(screen.getByRole('combobox', { name: 'Haiku 推理强度' })).toHaveProperty('value', 'high')
    expect(optionValues(screen.getByRole('combobox', { name: 'Haiku 推理强度' }))).not.toContain('max')
    expect(screen.getByText('推理强度选项为候选，实际可用性以供应商支持为准。')).toBeTruthy()
    await userEvent.setup().selectOptions(screen.getByRole('combobox', { name: 'Haiku 推理强度' }), 'xhigh')
    await userEvent.setup().click(screen.getByRole('button', { name: '保存' }))

    await waitFor(() => expect(onSave).toHaveBeenCalledWith(expect.objectContaining({
      id: 'gateway',
      effortMapping: { opus: 'max', sonnet: 'follow', haiku: 'xhigh' }
    }), false))
    expect(onSave.mock.calls[0]?.[0]).not.toHaveProperty('reasoningEffort')
  })

  it('renders GPT-style Claude role selects with metadata names and saves the raw mapped IDs', async () => {
    const existing: ProviderConfigView = {
      id: 'deepseek', cli: 'claude-code', name: 'DeepSeek', endpoint: 'https://api.deepseek.com/anthropic', model: 'deepseek-v4-pro', hasApiKey: true,
      modelMetadata: DEEPSEEK_METADATA,
      modelMapping: { opus: 'deepseek-v4-pro', sonnet: 'deepseek-flash', haiku: 'deepseek-flash' },
      effortMapping: { opus: 'high', sonnet: 'low', haiku: 'max' }
    }
    const onSave = vi.fn(async (_input: ProviderConfigInput, _use: boolean) => {})
    const user = userEvent.setup()
    render(<ProviderSetupDialog cli="claude-code" existing={existing} client={null} onClose={vi.fn()} onSave={onSave} onUseOfficial={vi.fn(async () => {})} onDelete={vi.fn(async () => {})} />)

    expect(screen.queryByText('更多设置')).toBeNull()
    expect(screen.getByRole('group', { name: 'Opus' })).toBeTruthy()
    expect(screen.getByRole('group', { name: 'Sonnet' })).toBeTruthy()
    expect(screen.getByRole('group', { name: 'Haiku' })).toBeTruthy()
    const opusModel = screen.getByRole('combobox', { name: 'Opus 模型' }) as HTMLSelectElement
    expect(opusModel.value).toBe('provider-model:deepseek-v4-pro')
    expect(within(opusModel).getByRole('option', { name: 'DeepSeek-V4-Pro' })).toBeTruthy()
    expect(within(opusModel).getByRole('option', { name: 'DeepSeek-V4.1-Flash' })).toBeTruthy()
    const opusEffort = screen.getByRole('combobox', { name: 'Opus 推理强度' }) as HTMLSelectElement
    expect(optionValues(opusEffort)).toEqual(['follow', 'low', 'high', 'max'])
    expect(optionValues(opusEffort)).not.toContain('medium')
    expect(optionValues(opusEffort)).not.toContain('xhigh')
    expect(screen.queryByText(/推理强度选项为候选/)).toBeNull()

    const sonnetModel = screen.getByRole('combobox', { name: 'Sonnet 模型' })
    await user.selectOptions(sonnetModel, within(sonnetModel).getByRole('option', { name: 'DeepSeek-V4-Pro' }))
    expect(sonnetModel).toHaveProperty('value', 'provider-model:deepseek-v4-pro')
    await user.click(screen.getByRole('button', { name: '保存' }))
    await waitFor(() => expect(onSave).toHaveBeenCalledWith(expect.objectContaining({
      modelMapping: { opus: 'deepseek-v4-pro', sonnet: 'deepseek-v4-pro', haiku: 'deepseek-flash' },
      modelMetadata: DEEPSEEK_METADATA
    }), false))
  })

  it('keeps an unknown API mapping and supports manual entry with a return to the list', async () => {
    const existing: ProviderConfigView = {
      id: 'custom', cli: 'claude-code', name: 'Custom', endpoint: 'https://custom.example', model: 'default-model', hasApiKey: true,
      modelMapping: { opus: 'legacy-opus', sonnet: '', haiku: '' },
      effortMapping: { opus: 'follow', sonnet: 'follow', haiku: 'follow' }
    }
    const onSave = vi.fn(async (_input: ProviderConfigInput, _use: boolean) => {})
    const user = userEvent.setup()
    render(<ProviderSetupDialog cli="claude-code" existing={existing} client={null} onClose={vi.fn()} onSave={onSave} onUseOfficial={vi.fn(async () => {})} onDelete={vi.fn(async () => {})} />)

    const opusModel = screen.getByRole('combobox', { name: 'Opus 模型' }) as HTMLSelectElement
    expect(opusModel.value).toBe('provider-model:legacy-opus')
    expect(within(opusModel).getByRole('option', { name: 'legacy-opus（尚未获取列表）' })).toBeTruthy()
    await user.selectOptions(opusModel, within(opusModel).getByRole('option', { name: '手动填写模型 ID…' }))
    const manual = screen.getByLabelText('Opus 手动填写模型 ID') as HTMLInputElement
    expect(manual.value).toBe('legacy-opus')
    await user.clear(manual); await user.type(manual, 'manual-opus')
    await user.click(screen.getByRole('button', { name: 'Opus 从列表选择' }))

    const returned = screen.getByRole('combobox', { name: 'Opus 模型' }) as HTMLSelectElement
    expect(returned.value).toBe('provider-model:manual-opus')
    expect(within(returned).getByRole('option', { name: 'manual-opus（尚未获取列表）' })).toBeTruthy()
    await user.click(screen.getByRole('button', { name: '保存' }))
    await waitFor(() => expect(onSave).toHaveBeenCalledWith(expect.objectContaining({
      modelMapping: { opus: 'manual-opus', sonnet: '', haiku: '' }
    }), false))
  })

  it('keeps an unsupported mapped effort selected and disables both API save actions', async () => {
    const metadata: ProviderModelMetadata[] = [
      { id: 'old-model', displayName: 'Old Model', supportedEfforts: ['low', 'high', 'max'] },
      { id: 'limited-model', displayName: 'Limited Model', supportedEfforts: ['low'] }
    ]
    const existing: ProviderConfigView = {
      id: 'gateway', cli: 'claude-code', name: 'Gateway', endpoint: 'https://gateway.example', model: 'old-model', hasApiKey: true,
      modelMetadata: metadata,
      modelMapping: { opus: 'old-model', sonnet: 'old-model', haiku: 'old-model' },
      effortMapping: { opus: 'max', sonnet: 'follow', haiku: 'follow' }
    }
    const onSave = vi.fn(async (_input: ProviderConfigInput, _use: boolean) => {})
    const user = userEvent.setup()
    render(<ProviderSetupDialog cli="claude-code" existing={existing} client={null} onClose={vi.fn()} onSave={onSave} onUseOfficial={vi.fn(async () => {})} onDelete={vi.fn(async () => {})} />)

    const opusModel = screen.getByRole('combobox', { name: 'Opus 模型' })
    await user.selectOptions(opusModel, within(opusModel).getByRole('option', { name: 'Limited Model' }))
    const effort = screen.getByRole('combobox', { name: 'Opus 推理强度' }) as HTMLSelectElement
    expect(effort.value).toBe('max')
    expect(within(effort).getByRole('option', { name: /最大\s*（不支持）/ })).toBeTruthy()
    expect(screen.getByText('该模型不支持当前推理强度。')).toBeTruthy()
    expect(screen.getByRole('button', { name: '保存' })).toHaveProperty('disabled', true)
    expect(screen.getByRole('button', { name: '保存并切换' })).toHaveProperty('disabled', true)
    await user.click(screen.getByRole('button', { name: '保存' }))
    expect(onSave).not.toHaveBeenCalled()
  })

  it('uses false and true for the two GPT-style save actions', async () => {
    const existing: ProviderConfigView = {
      id: 'gateway', cli: 'claude-code', name: 'Gateway', endpoint: 'https://gateway.example', model: 'claude-default', hasApiKey: true
    }
    const onSave = vi.fn(async (_input: ProviderConfigInput, _use: boolean) => {})
    const user = userEvent.setup()
    render(<ProviderSetupDialog cli="claude-code" existing={existing} client={null} onClose={vi.fn()} onSave={onSave} onUseOfficial={vi.fn(async () => {})} onDelete={vi.fn(async () => {})} />)

    await user.click(screen.getByRole('button', { name: '保存' }))
    await user.click(screen.getByRole('button', { name: '保存并切换' }))
    await waitFor(() => expect(onSave).toHaveBeenCalledTimes(2))
    expect(onSave.mock.calls.map(call => call[1])).toEqual([false, true])
  })

  it('defaults legacy effort mappings to follow and includes them in the payload', async () => {
    const existing: ProviderConfigView = {
      id: 'gateway', cli: 'claude-code', name: 'Gateway', endpoint: 'https://gateway.example', model: 'claude-default', hasApiKey: true,
      modelMapping: { opus: 'claude-opus', sonnet: '', haiku: 'gpt-5' }
    }
    const onSave = vi.fn(async (_input: ProviderConfigInput, _use: boolean) => {})
    render(<ProviderSetupDialog cli="claude-code" existing={existing} client={null} onClose={vi.fn()} onSave={onSave} onUseOfficial={vi.fn()} onDelete={vi.fn()} />)

    for (const role of ['opus', 'sonnet', 'haiku']) {
      const select = screen.getByRole('combobox', { name: `${role[0]!.toUpperCase()}${role.slice(1)} 推理强度` }) as HTMLSelectElement
      expect(select.value).toBe('follow')
      expect(select.options[0]?.value).toBe('follow')
    }
    await userEvent.setup().click(screen.getByRole('button', { name: '保存' }))
    await waitFor(() => expect(onSave).toHaveBeenCalledWith(expect.objectContaining({
      effortMapping: { opus: 'follow', sonnet: 'follow', haiku: 'follow' }
    }), false))
  })

  it('puts reasoning effort beside the single Codex model and saves it', async () => {
    const existing: ProviderConfigView = {
      id: 'gateway', cli: 'codex', name: 'Gateway', endpoint: 'https://gateway.example', model: 'gpt-5', hasApiKey: true,
      reasoningEffort: 'high'
    }
    const onSave = vi.fn(async (_input: ProviderConfigInput, _use: boolean) => {})
    render(<ProviderSetupDialog cli="codex" existing={existing} client={null} onClose={vi.fn()} onSave={onSave} onUseOfficial={vi.fn()} onDelete={vi.fn()} />)

    const effort = screen.getByRole('combobox', { name: '推理强度' }) as HTMLSelectElement
    expect(effort.value).toBe('high')
    expect(optionValues(effort)).toEqual(['follow', 'low', 'medium', 'high', 'xhigh'])
    expect(screen.getByText('推理强度选项为候选，实际可用性以供应商支持为准。')).toBeTruthy()
    await userEvent.setup().selectOptions(effort, 'xhigh')
    await userEvent.setup().click(screen.getByRole('button', { name: '保存' }))
    await waitFor(() => expect(onSave).toHaveBeenCalledWith(expect.objectContaining({ reasoningEffort: 'xhigh' }), false))
    expect(onSave.mock.calls[0]?.[0]).not.toHaveProperty('effortMapping')
  })

  it('keeps an unsupported saved Codex effort visible and blocks saving', async () => {
    const existing: ProviderConfigView = {
      id: 'gateway', cli: 'codex', name: 'Gateway', endpoint: 'https://gateway.example', model: 'gpt-5', hasApiKey: true,
      reasoningEffort: 'max'
    }
    const onSave = vi.fn(async (_input: ProviderConfigInput, _use: boolean) => {})
    render(<ProviderSetupDialog cli="codex" existing={existing} client={null} onClose={vi.fn()} onSave={onSave} onUseOfficial={vi.fn()} onDelete={vi.fn()} />)

    expect(screen.getByRole('combobox', { name: '推理强度' })).toHaveProperty('value', 'max')
    expect(screen.getByText('当前模型不支持所选推理强度。')).toBeTruthy()
    expect(screen.getByRole('button', { name: '保存' })).toHaveProperty('disabled', true)
    expect(screen.getByRole('button', { name: '保存并切换' })).toHaveProperty('disabled', true)
    await userEvent.setup().click(screen.getByRole('button', { name: '保存' }))
    expect(onSave).not.toHaveBeenCalled()
  })

  it('syncs a late loaded window preference without marking the untouched dialog dirty', async () => {
    const existing: ProviderConfigView = {
      id: 'gateway', cli: 'claude-code', name: 'Gateway', endpoint: 'https://gateway.example', model: 'claude-team', hasApiKey: true
    }
    const onClose = vi.fn()
    const renderWith = (mode: 'default' | 'maximum') => <ProviderSetupDialog cli="claude-code" existing={existing} client={null}
      contextWindowMode={mode} onClose={onClose} onSave={vi.fn(async () => {})} onUseOfficial={vi.fn(async () => {})} onDelete={vi.fn(async () => {})} />
    const { rerender } = render(renderWith('default'))
    expect(screen.getByRole('radio', { name: /默认窗口/ })).toHaveProperty('checked', true)

    rerender(renderWith('maximum'))
    await waitFor(() => expect(screen.getByRole('radio', { name: /最大可扩展/ })).toHaveProperty('checked', true))
    expect(screen.getByRole('radio', { name: /默认窗口/ })).toHaveProperty('checked', false)
    await userEvent.setup().click(screen.getByRole('button', { name: '关闭' }))
    expect(onClose).toHaveBeenCalled()
    expect(screen.queryByText('有尚未保存的修改')).toBeNull()
  })

  it('does not let a late window preference overwrite a user selection', async () => {
    const existing: ProviderConfigView = {
      id: 'gateway', cli: 'claude-code', name: 'Gateway', endpoint: 'https://gateway.example', model: 'claude-team', hasApiKey: true
    }
    const onClose = vi.fn()
    const renderWith = (mode: 'default' | 'maximum') => <ProviderSetupDialog cli="claude-code" existing={existing} client={null}
      contextWindowMode={mode} onClose={onClose} onSave={vi.fn(async () => {})} onUseOfficial={vi.fn(async () => {})} onDelete={vi.fn(async () => {})} />
    const user = userEvent.setup()
    const { rerender } = render(renderWith('default'))
    await user.click(screen.getByRole('radio', { name: /512K/ }))
    rerender(renderWith('maximum'))

    await waitFor(() => expect(screen.getByRole('radio', { name: /512K/ })).toHaveProperty('checked', true))
    expect(screen.getByRole('radio', { name: /最大可扩展/ })).toHaveProperty('checked', false)
    await user.click(screen.getByRole('button', { name: '关闭' }))
    expect(screen.getByText('有尚未保存的修改')).toBeTruthy()
    expect(onClose).not.toHaveBeenCalled()
  })

  it('displays live context window tokens in the provider edit dialog and tracks changes', async () => {
    const existing: ProviderConfigView = {
      id: 'official', cli: 'claude-code', name: 'Anthropic 官方', endpoint: 'https://api.anthropic.com', model: '', hasApiKey: false, builtIn: true
    }
    const onContextWindowChange = vi.fn()
    render(<ProviderSetupDialog
      cli="claude-code"
      existing={existing}
      client={null}
      contextWindowMode="default"
      contextModels={[{ id: 'default', name: 'Default', defaultTokens: 1_000_000, maximumTokens: 1_000_000, fixedTokens: 512_000 }]}
      onContextWindowChange={onContextWindowChange}
      onClose={vi.fn()}
      onSave={vi.fn(async () => {})}
      onUseOfficial={vi.fn(async () => {})}
      onDelete={vi.fn(async () => {})}
    />)

    expect(screen.getByRole('group', { name: '上下文窗口长度' })).toBeTruthy()
    expect(screen.getAllByText('1,000,000 tokens')).toHaveLength(2)
    expect(screen.getByText('512,000 tokens')).toBeTruthy()
    await userEvent.setup().click(screen.getByRole('radio', { name: /512K/ }))
    expect(onContextWindowChange).toHaveBeenCalledWith('512k')
  })
})

function optionValues(select: HTMLElement) {
  return [...(select as HTMLSelectElement).options].map(option => option.value)
}
