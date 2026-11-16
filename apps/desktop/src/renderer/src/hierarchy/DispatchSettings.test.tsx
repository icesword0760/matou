// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { DISPATCH_DEFAULTS, type DispatchDefaults, type DispatchConfigSnapshot } from '@matou/contracts'
import { DispatchSettings } from './DispatchSettings'
import { LocaleProvider } from '../i18n/LocaleProvider'
import { setCurrentLocale } from '../i18n/current'

afterEach(() => { cleanup(); setCurrentLocale('zh-CN') })

function fixture(patch: Partial<DispatchDefaults> = {}) {
  let state: DispatchConfigSnapshot = {
    exists: false, path: '/test/.claude/agent-foreman/config.json', defaults: { ...DISPATCH_DEFAULTS, ...patch }
  }
  const request = vi.fn(async (method: string, payload: unknown) => {
    if (method === 'dispatch-config.snapshot') return state
    if (method === 'dispatch-config.save') {
      state = { ...state, exists: true, defaults: { ...(payload as { defaults: DispatchDefaults }).defaults } }
      return state
    }
    throw new Error(`Unexpected method: ${method}`)
  })
  return { request }
}
const saves = (client: ReturnType<typeof fixture>) => client.request.mock.calls.filter(([method]) => method === 'dispatch-config.save')
async function ready(client = fixture()) {
  render(<DispatchSettings client={client} />)
  await screen.findByRole('combobox', { name: '默认执行者' })
  return client
}

describe('DispatchSettings', () => {
  it('shows built-in values and the global scope/path without creating a file', async () => {
    const client = await ready()
    expect(screen.getByRole('combobox', { name: '默认执行者' })).toHaveProperty('value', 'codex')
    expect(screen.getByRole('combobox', { name: '默认模型' })).toHaveProperty('value', 'sol')
    expect(screen.getByRole('spinbutton', { name: '没动静判卡' })).toHaveProperty('value', '10')
    expect(screen.getByText(/只影响新派的任务/)).toBeTruthy()
    expect(screen.getByText('~/.claude/agent-foreman/config.json')).toBeTruthy()
    expect(saves(client)).toHaveLength(0)
  })

  it('saves model and effort changes immediately without a success dialog', async () => {
    const client = await ready()
    const user = userEvent.setup()
    await user.selectOptions(screen.getByRole('combobox', { name: '默认模型' }), 'astra')
    await waitFor(() => expect(saves(client)).toHaveLength(1))
    await user.selectOptions(screen.getByRole('combobox', { name: '推理档位' }), 'xhigh')
    await waitFor(() => expect(saves(client)).toHaveLength(2))
    expect(saves(client)[1]?.[1]).toEqual({ defaults: { ...DISPATCH_DEFAULTS, model: 'astra', effort: 'xhigh' } })
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('preserves the stored model on executor changes and disables Orchestrator effort', async () => {
    const client = await ready()
    const user = userEvent.setup()
    await user.selectOptions(screen.getByRole('combobox', { name: '默认执行者' }), 'agy')
    await waitFor(() => expect(saves(client)).toHaveLength(1))
    expect(saves(client)[0]?.[1]).toEqual({ defaults: { ...DISPATCH_DEFAULTS, agent: 'agy' } })
    expect(screen.getByRole('combobox', { name: '默认模型' })).toHaveProperty('value', 'gemini-pro-orchestrator')
    expect(screen.getByRole('combobox', { name: '推理档位' })).toHaveProperty('disabled', true)
    await user.selectOptions(screen.getByRole('combobox', { name: '默认模型' }), 'hydrogen')
    await waitFor(() => expect(screen.getByRole('combobox', { name: '推理档位' })).toHaveProperty('disabled', false))
    expect(saves(client)[1]?.[1]).toEqual({ defaults: { ...DISPATCH_DEFAULTS, agent: 'agy', model: 'hydrogen' } })
  })

  it('shows fixed DSH model and low/high effort while preserving stored model/effort until edited', async () => {
    const client = await ready(fixture({ effort: 'xhigh', model: 'astra' }))
    const user = userEvent.setup()
    await user.selectOptions(screen.getByRole('combobox', { name: '默认执行者' }), 'dsh')
    await waitFor(() => expect(saves(client)).toHaveLength(1))
    expect(saves(client)[0]?.[1]).toEqual({ defaults: { ...DISPATCH_DEFAULTS, model: 'astra', effort: 'xhigh', agent: 'dsh' } })
    expect(screen.getByDisplayValue('deepseek-flash')).toHaveProperty('readOnly', true)
    expect(screen.getByText('DSH 型号固定，配置文件里的 model 对它不生效')).toBeTruthy()
    const effort = screen.getByRole('combobox', { name: '推理档位' }) as HTMLSelectElement
    expect([...effort.options].map(option => option.value)).toEqual(['low', 'high'])
    expect(effort.value).toBe('high')
    expect(screen.getByText('DSH 只支持低/高')).toBeTruthy()
    const retries = screen.getByRole('spinbutton', { name: '重试次数' })
    fireEvent.change(retries, { target: { value: '2' } }); fireEvent.blur(retries)
    await waitFor(() => expect(saves(client)).toHaveLength(2))
    expect((saves(client)[1]?.[1] as { defaults: DispatchDefaults }).defaults.effort).toBe('xhigh')
    await user.selectOptions(effort, 'low')
    await waitFor(() => expect(saves(client)).toHaveLength(3))
    expect((saves(client)[2]?.[1] as { defaults: DispatchDefaults }).defaults.effort).toBe('low')
    await user.selectOptions(screen.getByRole('combobox', { name: '默认执行者' }), 'codex')
    await waitFor(() => expect(screen.getByRole('combobox', { name: '默认模型' })).toHaveProperty('value', 'astra'))
  })

  it('displays a fallback model without rewriting a valid custom model on unrelated edits', async () => {
    const client = await ready(fixture({ model: 'custom/model' }))
    expect(screen.getByRole('combobox', { name: '默认模型' })).toHaveProperty('value', 'sol')
    await userEvent.setup().selectOptions(screen.getByRole('combobox', { name: '推理档位' }), 'low')
    await waitFor(() => expect(saves(client)).toHaveLength(1))
    expect((saves(client)[0]?.[1] as { defaults: DispatchDefaults }).defaults.model).toBe('custom/model')
  })

  it('saves numbers only on blur or Enter and does not save twice after Enter then blur', async () => {
    const client = await ready()
    const input = screen.getByRole('spinbutton', { name: '单轮最长' })
    fireEvent.change(input, { target: { value: '90' } })
    expect(saves(client)).toHaveLength(0)
    fireEvent.keyDown(input, { key: 'Enter' })
    fireEvent.blur(input)
    await waitFor(() => expect(saves(client)).toHaveLength(1))
    expect((saves(client)[0]?.[1] as { defaults: DispatchDefaults }).defaults.maxMin).toBe(90)
    fireEvent.change(input, { target: { value: '120' } }); fireEvent.blur(input)
    await waitFor(() => expect(saves(client)).toHaveLength(2))
  })

  it('disables edits during a pending save and prevents Enter plus blur from duplicating it', async () => {
    const client = await ready()
    let finish!: (value: DispatchConfigSnapshot) => void
    client.request.mockImplementationOnce(() => new Promise(resolve => { finish = resolve }))
    const input = screen.getByRole('spinbutton', { name: '重试次数' })
    fireEvent.change(input, { target: { value: '1' } })
    fireEvent.keyDown(input, { key: 'Enter' }); fireEvent.blur(input)
    expect(screen.getByRole('combobox', { name: '默认执行者' })).toHaveProperty('disabled', true)
    expect(saves(client)).toHaveLength(1)
    await act(async () => finish({ exists: true, path: '/test/.claude/agent-foreman/config.json', defaults: { ...DISPATCH_DEFAULTS, retries: 1 } }))
    expect(input).toHaveProperty('disabled', false)
    expect(input).toHaveProperty('value', '1')
  })

  it.each([
    ['没动静判卡', '0'], ['没动静判卡', '61'], ['单轮最长', '4'], ['单轮最长', '181'],
    ['等回答', '0'], ['等回答', '61'], ['重试次数', '-1'], ['重试次数', '6'], ['重试次数', '1.5'], ['等回答', '']
  ])('rejects invalid %s=%s in place without saving', async (label, value) => {
    const client = await ready()
    const input = screen.getByRole('spinbutton', { name: label })
    fireEvent.change(input, { target: { value } }); fireEvent.blur(input)
    expect(screen.getByRole('alert')).toBeTruthy()
    expect(saves(client)).toHaveLength(0)
    expect(input).toHaveProperty('value', value)
  })

  it('accepts fractional minutes within range', async () => {
    const client = await ready()
    const input = screen.getByRole('spinbutton', { name: '没动静判卡' })
    fireEvent.change(input, { target: { value: '1.5' } }); fireEvent.blur(input)
    await waitFor(() => expect(saves(client)).toHaveLength(1))
    expect((saves(client)[0]?.[1] as { defaults: DispatchDefaults }).defaults.idleMin).toBe(1.5)
  })

  it('treats an invalid save response as a failed save and restores the control', async () => {
    const client = await ready()
    client.request.mockResolvedValueOnce({} as DispatchConfigSnapshot)
    await userEvent.setup().selectOptions(screen.getByRole('combobox', { name: '默认模型' }), 'luna')
    await screen.findByRole('alert')
    expect(screen.getByRole('combobox', { name: '默认模型' })).toHaveProperty('value', 'sol')
  })

  it('shows a load error for an invalid snapshot instead of silently hiding the section', async () => {
    const client = fixture()
    client.request.mockResolvedValueOnce({} as DispatchConfigSnapshot)
    render(<DispatchSettings client={client} />)
    await screen.findByRole('alert')
    expect(screen.getByRole('button', { name: '重试' })).toBeTruthy()
    expect(saves(client)).toHaveLength(0)
  })

  it('rolls a failed dropdown save back and shows the error on its row', async () => {
    const client = await ready()
    client.request.mockRejectedValueOnce(new Error('model: write denied'))
    await userEvent.setup().selectOptions(screen.getByRole('combobox', { name: '默认模型' }), 'luna')
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toContain('write denied')
    expect(screen.getByRole('combobox', { name: '默认模型' })).toHaveProperty('value', 'sol')
    expect(screen.getByRole('combobox', { name: '默认模型' }).getAttribute('aria-describedby')).toContain(alert.id)
  })

  it('rolls a failed number save back to the last saved number', async () => {
    const client = await ready()
    client.request.mockRejectedValueOnce(new Error('idleMin: disk full'))
    const input = screen.getByRole('spinbutton', { name: '没动静判卡' })
    fireEvent.change(input, { target: { value: '20' } }); fireEvent.blur(input)
    expect((await screen.findByRole('alert')).textContent).toContain('disk full')
    expect(input).toHaveProperty('value', '10')
  })

  it('writes all built-in defaults on reset', async () => {
    const client = await ready(fixture({ agent: 'dsh', retries: 0, model: 'custom', effort: 'medium' }))
    await userEvent.setup().click(screen.getByRole('button', { name: '恢复默认' }))
    await waitFor(() => expect(saves(client)).toHaveLength(1))
    expect(saves(client)[0]?.[1]).toEqual({ defaults: DISPATCH_DEFAULTS })
    expect(screen.getByRole('combobox', { name: '默认执行者' })).toHaveProperty('value', 'codex')
  })

  it('reports load failure and retries without saving unknown defaults', async () => {
    const client = fixture()
    client.request.mockRejectedValueOnce(new Error('offline'))
    render(<DispatchSettings client={client} />)
    await screen.findByRole('alert')
    expect(screen.queryByRole('combobox', { name: '默认执行者' })).toBeNull()
    await userEvent.setup().click(screen.getByRole('button', { name: '重试' }))
    await screen.findByRole('combobox', { name: '默认执行者' })
    expect(saves(client)).toHaveLength(0)
  })

  it('renders the complete English controls and scope note', async () => {
    render(<LocaleProvider initialLocale="en"><DispatchSettings client={fixture()} /></LocaleProvider>)
    await screen.findByRole('combobox', { name: 'Default executor' })
    expect(screen.getByRole('combobox', { name: 'Default model' })).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Restore defaults' })).toBeTruthy()
    expect(screen.getByText(/newly dispatched tasks/i)).toBeTruthy()
    expect(screen.queryByText('默认执行者')).toBeNull()
  })
})
