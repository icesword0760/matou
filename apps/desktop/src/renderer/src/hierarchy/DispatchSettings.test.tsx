// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
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
    expect(screen.getByLabelText('没动静判卡')).toHaveProperty('value', '10')
    expect(screen.getByText(/只影响新派的任务/)).toBeTruthy()
    expect(screen.getByText('~/.claude/agent-foreman/config.json')).toBeTruthy()
    expect(saves(client)).toHaveLength(0)
  })

  it('saves the complete draft once after editing every field', async () => {
    const client = await ready()
    const user = userEvent.setup()
    const save = screen.getByRole('button', { name: '保存' })
    expect(save).toHaveProperty('disabled', true)
    await user.selectOptions(screen.getByRole('combobox', { name: '默认执行者' }), 'agy')
    await user.selectOptions(screen.getByRole('combobox', { name: '默认模型' }), 'hydrogen')
    await user.selectOptions(screen.getByRole('combobox', { name: '推理档位' }), 'xhigh')
    fireEvent.change(screen.getByLabelText('没动静判卡'), { target: { value: '1.5' } })
    fireEvent.change(screen.getByLabelText('单轮最长'), { target: { value: '90' } })
    fireEvent.change(screen.getByLabelText('等回答'), { target: { value: '20' } })
    const retries = screen.getByLabelText('重试次数')
    fireEvent.change(retries, { target: { value: '2' } })
    fireEvent.keyDown(retries, { key: 'Enter' })
    fireEvent.blur(retries)
    expect(saves(client)).toHaveLength(0)
    await user.click(save)
    await screen.findByText('已保存，新派任务生效')
    expect(saves(client).map(([, payload]) => payload)).toEqual([{
      defaults: { agent: 'agy', model: 'hydrogen', effort: 'xhigh', idleMin: 1.5, maxMin: 90, askWaitMin: 20, retries: 2 }
    }])
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('keeps the last saved model on an executor switch and disables Orchestrator effort', async () => {
    const client = await ready()
    const user = userEvent.setup()
    await user.selectOptions(screen.getByRole('combobox', { name: '默认模型' }), 'astra')
    await user.selectOptions(screen.getByRole('combobox', { name: '默认执行者' }), 'agy')
    expect(screen.getByRole('combobox', { name: '默认模型' })).toHaveProperty('value', 'gemini-pro-orchestrator')
    expect(screen.getByText(/不在该执行者的预设列表/)).toBeTruthy()
    expect(screen.getByRole('combobox', { name: '推理档位' })).toHaveProperty('disabled', true)
    expect(saves(client)).toHaveLength(0)
    await user.click(screen.getByRole('button', { name: '保存' }))
    expect(saves(client)[0]?.[1]).toEqual({ defaults: { ...DISPATCH_DEFAULTS, agent: 'agy' } })
  })

  it('shows the fixed DSH model and clamps the displayed effort without changing those saved values', async () => {
    const client = await ready(fixture({ effort: 'xhigh', model: 'astra' }))
    const user = userEvent.setup()
    await user.selectOptions(screen.getByRole('combobox', { name: '默认执行者' }), 'dsh')
    expect(screen.getByDisplayValue('deepseek-flash')).toHaveProperty('readOnly', true)
    expect(screen.getByText('DSH 型号固定，配置文件里的 model 对它不生效')).toBeTruthy()
    const effort = screen.getByRole('combobox', { name: '推理档位' }) as HTMLSelectElement
    expect([...effort.options].map(option => option.value)).toEqual(['low', 'high'])
    expect(effort.value).toBe('high')
    expect(screen.getByText('DSH 只支持低/高')).toBeTruthy()
    fireEvent.change(screen.getByLabelText('重试次数'), { target: { value: '2' } })
    expect(saves(client)).toHaveLength(0)
    const save = screen.getByRole('button', { name: '保存' })
    await user.click(save)
    expect(saves(client)[0]?.[1]).toEqual({
      defaults: { ...DISPATCH_DEFAULTS, agent: 'dsh', model: 'astra', effort: 'xhigh', retries: 2 }
    })
    await user.selectOptions(effort, 'low')
    await user.click(save)
    expect((saves(client)[1]?.[1] as { defaults: DispatchDefaults }).defaults.effort).toBe('low')
    await user.selectOptions(screen.getByRole('combobox', { name: '默认执行者' }), 'codex')
    expect(screen.getByRole('combobox', { name: '默认模型' })).toHaveProperty('value', 'astra')
  })

  it('displays a fallback model without rewriting a valid custom model on unrelated edits', async () => {
    const client = await ready(fixture({ model: 'custom/model' }))
    const user = userEvent.setup()
    expect(screen.getByRole('combobox', { name: '默认模型' })).toHaveProperty('value', 'sol')
    await user.selectOptions(screen.getByRole('combobox', { name: '推理档位' }), 'low')
    expect(saves(client)).toHaveLength(0)
    await user.click(screen.getByRole('button', { name: '保存' }))
    expect((saves(client)[0]?.[1] as { defaults: DispatchDefaults }).defaults.model).toBe('custom/model')
  })

  it('disables edits during save and retains save focus and scroll through success', async () => {
    const client = fixture({ agent: 'dsh' })
    const { getByTestId } = render(<div style={{ overflow: 'auto' }} data-testid="settings-scroll">
      <DispatchSettings client={client} />
    </div>)
    const save = await screen.findByRole('button', { name: '保存' })
    const input = screen.getByLabelText('重试次数')
    const restore = screen.getByRole('button', { name: '恢复默认' })
    let finish!: (value: DispatchConfigSnapshot) => void
    client.request.mockImplementationOnce(() => new Promise(resolve => { finish = resolve }))
    fireEvent.change(input, { target: { value: '1' } })
    restore.focus()
    const user = userEvent.setup()
    await user.tab()
    expect(document.activeElement).toBe(save)
    const scroller = getByTestId('settings-scroll')
    scroller.scrollTop = 140
    await user.keyboard('{Enter}')
    expect(save.textContent).toBe('保存中…')
    expect(save.getAttribute('aria-disabled')).toBe('true')
    for (const control of [...screen.getAllByRole('combobox'), ...screen.getAllByRole('textbox'), restore]) {
      expect(control).toHaveProperty('disabled', true)
    }
    await user.keyboard('{Enter}')
    await user.click(save)
    expect(saves(client)).toHaveLength(1)
    await act(async () => finish({ exists: true, path: '/test/.claude/agent-foreman/config.json', defaults: { ...DISPATCH_DEFAULTS, agent: 'dsh', retries: 1 } }))
    expect(input).toHaveProperty('disabled', false)
    expect(input).toHaveProperty('value', '1')
    expect(screen.getByRole('status').textContent).toBe('已保存，新派任务生效')
    expect(document.activeElement).toBe(save)
    expect(scroller.scrollTop).toBe(140)
    expect(save.getAttribute('aria-disabled')).toBe('true')
    await user.keyboard('{Enter}')
    expect(saves(client)).toHaveLength(1)
  })

  it('ignores equivalent number text and reverting edits to the saved values', async () => {
    const client = await ready(fixture({ maxMin: 200 }))
    const user = userEvent.setup()
    const save = screen.getByRole('button', { name: '保存' })
    const max = screen.getByLabelText('单轮最长')
    fireEvent.change(max, { target: { value: ' 0200.0 ' } })
    fireEvent.blur(max)
    expect(screen.queryByRole('alert')).toBeNull()
    expect(save).toHaveProperty('disabled', true)
    const retries = screen.getByLabelText('重试次数')
    fireEvent.change(retries, { target: { value: '2' } })
    expect(save).toHaveProperty('disabled', false)
    fireEvent.change(retries, { target: { value: '03.0' } })
    await user.click(screen.getByRole('button', { name: '恢复默认' }))
    expect(max).toHaveProperty('value', '45')
    expect(save).toHaveProperty('disabled', false)
    fireEvent.change(max, { target: { value: '200' } })
    expect(save).toHaveProperty('disabled', true)
    expect(saves(client)).toHaveLength(0)
  })

  it('rechecks unblurred numeric fields on save and focuses the first invalid input', async () => {
    const client = await ready()
    const idle = screen.getByLabelText('没动静判卡')
    const max = screen.getByLabelText('单轮最长')
    fireEvent.change(max, { target: { value: '4' } })
    fireEvent.change(idle, { target: { value: 'abc' } })
    expect(screen.queryByRole('alert')).toBeNull()
    const save = screen.getByRole('button', { name: '保存' })
    expect(save.getAttribute('aria-disabled')).toBe('true')
    await userEvent.setup().click(save)
    expect(saves(client)).toHaveLength(0)
    expect(document.activeElement).toBe(idle)
    expect(idle).toHaveProperty('value', 'abc')
    expect(screen.getAllByRole('alert')).toHaveLength(2)
    fireEvent.change(idle, { target: { value: '11' } })
    expect(max.getAttribute('aria-invalid')).toBe('true')
    fireEvent.change(max, { target: { value: '50' } })
    await userEvent.setup().click(save)
    expect(saves(client)[0]?.[1]).toEqual({ defaults: { ...DISPATCH_DEFAULTS, idleMin: 11, maxMin: 50 } })
  })

  it.each([
    ['没动静判卡', '0'], ['没动静判卡', '61'], ['单轮最长', '4'], ['单轮最长', '181'],
    ['等回答', '0'], ['等回答', '61'], ['重试次数', '-1'], ['重试次数', '6'], ['重试次数', '1.5'], ['等回答', '']
  ])('rejects invalid %s=%s in place without saving', async (label, value) => {
    const client = await ready()
    const input = screen.getByLabelText(label)
    fireEvent.change(input, { target: { value } })
    fireEvent.blur(input)
    expect(screen.getByRole('alert')).toBeTruthy()
    expect(saves(client)).toHaveLength(0)
    expect(input).toHaveProperty('value', value)
  })

  it('keeps the entire draft on a failed save and allows retry', async () => {
    const client = await ready()
    const user = userEvent.setup()
    const model = screen.getByRole('combobox', { name: '默认模型' })
    const idle = screen.getByLabelText('没动静判卡')
    await user.selectOptions(model, 'luna')
    fireEvent.change(idle, { target: { value: ' 20.0 ' } })
    client.request.mockRejectedValueOnce(new Error('disk full'))
    const save = screen.getByRole('button', { name: '保存' })
    await user.click(save)
    expect((await screen.findByRole('alert')).textContent).toContain('disk full')
    expect(model).toHaveProperty('value', 'luna')
    expect(idle).toHaveProperty('value', ' 20.0 ')
    expect(save).toHaveProperty('disabled', false)
    await user.click(save)
    await screen.findByRole('status')
    expect(saves(client)).toHaveLength(2)
    expect(saves(client)[1]?.[1]).toEqual({ defaults: { ...DISPATCH_DEFAULTS, model: 'luna', idleMin: 20 } })
    expect(screen.queryByRole('alert')).toBeNull()
  })

  it('treats an invalid save response as a failed save and retains the draft', async () => {
    const client = await ready()
    const user = userEvent.setup()
    const model = screen.getByRole('combobox', { name: '默认模型' })
    client.request.mockResolvedValueOnce({} as DispatchConfigSnapshot)
    await user.selectOptions(model, 'luna')
    await user.click(screen.getByRole('button', { name: '保存' }))
    expect((await screen.findByRole('alert')).textContent).toContain('服务器返回的配置无法识别')
    expect(model).toHaveProperty('value', 'luna')
  })

  it('shows a load error for an invalid snapshot instead of silently hiding the section', async () => {
    const client = fixture()
    client.request.mockResolvedValueOnce({} as DispatchConfigSnapshot)
    render(<DispatchSettings client={client} />)
    await screen.findByRole('alert')
    expect(screen.getByRole('button', { name: '重试' })).toBeTruthy()
    expect(screen.queryByRole('button', { name: '保存' })).toBeNull()
    expect(saves(client)).toHaveLength(0)
  })

  it('restores built-in defaults locally and applies them only when saved', async () => {
    const client = await ready(fixture({ agent: 'dsh', retries: 0, model: 'custom', effort: 'medium' }))
    const user = userEvent.setup()
    const restore = screen.getByRole('button', { name: '恢复默认' })
    await user.click(restore)
    expect(saves(client)).toHaveLength(0)
    expect(screen.getByRole('combobox', { name: '默认执行者' })).toHaveProperty('value', 'codex')
    const save = screen.getByRole('button', { name: '保存' })
    expect(save).toHaveProperty('disabled', false)
    await user.click(save)
    expect(saves(client)[0]?.[1]).toEqual({ defaults: DISPATCH_DEFAULTS })
    await user.click(restore)
    expect(save).toHaveProperty('disabled', true)
    expect(saves(client)).toHaveLength(1)
  })

  it('loads the saved values when settings are closed and reopened', async () => {
    const client = fixture({ retries: 1 })
    const view = render(<DispatchSettings client={client} />)
    const retries = await screen.findByLabelText('重试次数')
    const user = userEvent.setup()
    fireEvent.change(retries, { target: { value: '2' } })
    await user.click(screen.getByRole('button', { name: '保存' }))
    fireEvent.change(retries, { target: { value: '3' } })
    view.unmount()
    render(<DispatchSettings client={client} />)
    expect(await screen.findByLabelText('重试次数')).toHaveProperty('value', '2')
    expect(saves(client)).toHaveLength(1)
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
    expect(screen.getByRole('button', { name: 'Save' })).toHaveProperty('disabled', true)
    expect(screen.getByText(/newly dispatched tasks/i)).toBeTruthy()
    expect(screen.queryByText('默认执行者')).toBeNull()
  })
})
