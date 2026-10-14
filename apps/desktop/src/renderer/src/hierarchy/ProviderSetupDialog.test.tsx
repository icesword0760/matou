// @vitest-environment jsdom
import { act, cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'
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
    await user.click(await screen.findByRole('option', { name: 'model-b' }))
    await user.click(screen.getByRole('button', { name: '仅保存' }))
    expect(client.request).toHaveBeenCalledWith('provider-config.models', { cli:'claude-code',endpoint:'https://api.deepseek.com/anthropic',apiKey:'TOKEN' })
    expect(onSave).toHaveBeenCalledWith(expect.objectContaining({ name:'DeepSeek',model:'model-b',modelMapping:expect.objectContaining({haiku:'deepseek-flash'}) }),false)
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
})
