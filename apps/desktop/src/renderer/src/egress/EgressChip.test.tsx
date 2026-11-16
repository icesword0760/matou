// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { EgressStatusWire } from '@matou/contracts'
import { LocaleProvider } from '../i18n/LocaleProvider'
import type { RuntimeClient } from '../runtime/RuntimeClient'

let currentClient: RuntimeClient | null = null

vi.mock('../runtime/RuntimeProvider', () => ({
  useRuntimeClient: () => currentClient
}))

import { EgressChip } from './EgressChip'

afterEach(() => {
  cleanup()
  currentClient = null
  Reflect.deleteProperty(window, 'matouDesktop')
})

function baseStatus(overrides: Partial<EgressStatusWire> = {}): EgressStatusWire {
  return {
    state: 'ok',
    protectionEnabled: true,
    ruleMode: 'country',
    checkedAt: 1_700_000_000_000,
    families: [
      {
        family: 'anthropic',
        state: 'ok',
        exit: { ip: '169.128.4.5', country: 'US' },
        hosts: [],
        overridden: false,
        frozenSessionIds: [],
        recovering: false
      }
    ],
    checks: [
      { id: 'mainland', family: 'anthropic', result: 'pass' },
      { id: 'exit-rule', family: 'anthropic', result: 'pass' },
      { id: 'login-api-match', family: 'anthropic', result: 'pass' },
      { id: 'agent-proxy-path', result: 'pass' },
      { id: 'time-zone', result: 'pass' },
      { id: 'ipv6', result: 'pass' }
    ],
    sessionFamilies: { 'session-claude': 'anthropic' },
    ...overrides
  }
}

async function renderChip(
  status: EgressStatusWire | Error,
  sessionId = 'session-claude',
  locale: 'zh-CN' | 'en' = 'zh-CN'
) {
  const request = status instanceof Error
    ? vi.fn().mockRejectedValue(status)
    : vi.fn().mockResolvedValue(status)
  currentClient = {
    request,
    subscribeEgressStatus: vi.fn(() => () => {})
  } as unknown as RuntimeClient

  const rendered = render(
    <LocaleProvider initialLocale={locale}>
      <EgressChip sessionId={sessionId} />
    </LocaleProvider>
  )

  await waitFor(() => {
    expect(request).toHaveBeenCalledWith('egress.status', undefined)
  })
  return rendered
}

describe('EgressChip', () => {
  it('renders the shared HUD exit label for a registered Google session and opens its panel', async () => {
    const status = baseStatus()
    status.families[0]!.family = 'google'
    status.checks = [
      { id: 'mainland', family: 'google', result: 'pass' },
      { id: 'exit-rule', family: 'google', result: 'pass' }
    ]
    status.sessionFamilies = { 'session-antigravity': 'google' }
    await renderChip(status, 'session-antigravity')
    const chip = await screen.findByRole('button', { name: '🇺🇸 美国 · 出口正常' })
    expect(chip.getAttribute('data-tone')).toBe('ok')
    fireEvent.click(chip)
    expect(await screen.findByRole('heading', { name: 'Google（通用出口检测）' })).toBeTruthy()
    const google = screen.getByRole('region', { name: 'Google（通用出口检测）' })
    expect(within(google).getAllByRole('listitem')).toHaveLength(2)
    expect(google.textContent).toContain('出口不在中国大陆')
    expect(google.textContent).toContain('出口符合出口规则')
    expect(google.textContent).not.toContain('登录与 API 同一出口')
    expect(google.textContent).not.toContain('无法确认')
    expect(screen.getByText(/Google 线路使用通用出口检测，不能保证 Google 实际看到的出口/)).toBeTruthy()
  })

  it('renders checking copy before the first check completes', async () => {
    await renderChip(
      baseStatus({ state: 'checking', families: [] }),
      'session-claude',
      'zh-CN'
    )
    expect(screen.getByRole('button', { name: '出口检查中…' })).toBeTruthy()
  })

  it('renders ok copy with regional indicator flag and localized country in zh and en', async () => {
    await renderChip(baseStatus(), 'session-claude', 'zh-CN')
    const zhChip = screen.getByRole('button', { name: '🇺🇸 美国 · 出口正常' })
    expect(zhChip.getAttribute('data-tone')).toBe('ok')

    cleanup()
    await renderChip(baseStatus(), 'session-claude', 'en')
    expect(screen.getByRole('button', { name: '🇺🇸 United States · Exit OK' })).toBeTruthy()
  })

  it('counts family warnings plus global warnings and shows warn tone', async () => {
    const warnStatus = baseStatus({
      state: 'warning',
      checks: [
        { id: 'mainland', family: 'anthropic', result: 'pass' },
        { id: 'login-api-match', family: 'anthropic', result: 'warn', facts: { loginIp: '1.1.1.1', apiIp: '2.2.2.2' } },
        { id: 'login-api-match', family: 'openai', result: 'warn', facts: { loginIp: '3.3.3.3', apiIp: '4.4.4.4' } },
        { id: 'time-zone', result: 'warn' }
      ]
    })
    await renderChip(warnStatus, 'session-claude', 'zh-CN')
    const chip = screen.getByRole('button', { name: '🇺🇸 美国 · 2 项提醒' })
    expect(chip.getAttribute('data-tone')).toBe('warn')

    cleanup()
    await renderChip(warnStatus, 'session-claude', 'en')
    expect(screen.getByRole('button', { name: '🇺🇸 United States · 2 warning(s)' })).toBeTruthy()
  })

  it('renders unknown, frozen, and overridden states with expected tones and copy', async () => {
    await renderChip(
      baseStatus({
        families: [
          {
            family: 'anthropic',
            state: 'unknown',
            hosts: [],
            overridden: false,
            frozenSessionIds: [],
            recovering: false
          }
        ]
      }),
      'session-claude',
      'zh-CN'
    )
    expect(screen.getByRole('button', { name: '无法确认出口' }).getAttribute('data-tone')).toBe('warn')

    cleanup()
    await renderChip(
      baseStatus({
        state: 'abnormal',
        families: [
          {
            family: 'anthropic',
            state: 'abnormal',
            violation: 'mainland',
            hosts: [],
            overridden: false,
            frozenSessionIds: ['session-claude', 'session-2'],
            recovering: false
          }
        ]
      }),
      'session-claude',
      'zh-CN'
    )
    const frozenChip = screen.getByRole('button', { name: '出口异常 · 已暂停 2 个会话' })
    expect(frozenChip.getAttribute('data-tone')).toBe('danger')

    cleanup()
    await renderChip(
      baseStatus({
        state: 'abnormal',
        families: [
          {
            family: 'anthropic',
            state: 'abnormal',
            violation: 'mainland',
            hosts: [],
            overridden: true,
            frozenSessionIds: [],
            recovering: false
          }
        ]
      }),
      'session-claude',
      'zh-CN'
    )
    const overriddenChip = screen.getByRole('button', { name: '出口异常 · 已手动继续' })
    expect(overriddenChip.getAttribute('data-tone')).toBe('danger-outline')
  })

  it('still opens the existing panel for a monitored session', async () => {
    await renderChip(baseStatus(), 'session-claude', 'zh-CN')
    fireEvent.click(screen.getByRole('button', { name: '🇺🇸 美国 · 出口正常' }))
    expect(screen.getByRole('dialog', { name: '网络检查' })).toBeTruthy()
  })
})

describe('EgressChip global entry', () => {
  it.each(['session-shell', 'session-deepseek', 'session-glm', 'session-kimi'])('exposes network check for unmonitored %s', async (sessionId) => {
    await renderChip(baseStatus(), sessionId)
    const button = screen.getByRole('button', { name: '网络检查' })
    expect(button.textContent).toBe('出口正常')
    expect(button.getAttribute('data-tone')).toBe('ok')
    expect(button.getAttribute('aria-expanded')).toBe('false')
  })

  it.each([
    ['checking', '检查中…', 'neutral'],
    ['ok', '出口正常', 'ok'],
    ['warning', '1 项提醒', 'warn'],
    ['abnormal', '出口异常', 'danger'],
    ['unknown', '无法确认出口', 'warn']
  ] as const)('renders global %s state', async (state, label, tone) => {
    // Unknown values may arrive from an unavailable or newer runtime.
    await renderChip(baseStatus({ state: state as EgressStatusWire['state'] }), 'session-shell')
    const button = screen.getByRole('button', { name: '网络检查' })
    expect(button.textContent).toBe(label)
    expect(button.getAttribute('data-tone')).toBe(tone)
  })

  it('shows checking while the initial status request is pending', async () => {
    currentClient = {
      request: vi.fn(() => new Promise(() => {})),
      subscribeEgressStatus: vi.fn(() => () => {})
    } as unknown as RuntimeClient
    render(<LocaleProvider initialLocale="zh-CN"><EgressChip sessionId="session-shell" /></LocaleProvider>)
    expect(screen.getByRole('button', { name: '网络检查' }).textContent).toBe('检查中…')
  })

  it('counts paused sessions across every family', async () => {
    const status = baseStatus({ state: 'abnormal' })
    status.families[0]!.frozenSessionIds = ['session-claude', 'session-2']
    status.families.push({ ...status.families[0]!, family: 'google', frozenSessionIds: ['session-google'] })
    await renderChip(status, 'session-shell')
    expect(screen.getByRole('button', { name: '网络检查' }).textContent).toBe('出口异常 · 已暂停 3 个会话')
  })

  it('counts warn and fail checks globally and ignores pass and unknown', async () => {
    await renderChip(baseStatus({ state: 'warning', checks: [
      { id: 'mainland', family: 'google', result: 'fail' },
      { id: 'exit-rule', family: 'openai', result: 'warn' },
      { id: 'time-zone', result: 'warn' },
      { id: 'ipv6', result: 'unknown' },
      { id: 'agent-proxy-path', result: 'pass' }
    ] }), 'session-shell')
    expect(screen.getByRole('button', { name: '网络检查' }).textContent).toBe('3 项提醒')
  })

  it('toggles the shared panel and closes with Escape or an outside click', async () => {
    await renderChip(baseStatus(), 'session-shell')
    const button = screen.getByRole('button', { name: '网络检查' })
    fireEvent.click(button)
    expect(screen.getByRole('dialog', { name: '网络检查' })).toBeTruthy()
    expect(button.getAttribute('aria-expanded')).toBe('true')
    fireEvent.pointerDown(button)
    expect(screen.getByRole('dialog', { name: '网络检查' })).toBeTruthy()
    fireEvent.click(button)
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(button.getAttribute('aria-expanded')).toBe('false')
    fireEvent.click(button)
    fireEvent.keyDown(document, { key: 'Escape' })
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(button.getAttribute('aria-expanded')).toBe('false')
    fireEvent.click(button)
    fireEvent.pointerDown(document.body)
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(button.getAttribute('aria-expanded')).toBe('false')
  })

  it('can be focused and opened with the keyboard', async () => {
    await renderChip(baseStatus(), 'session-shell')
    const user = userEvent.setup()
    await user.tab()
    expect(document.activeElement).toBe(screen.getByRole('button', { name: '网络检查' }))
    await user.keyboard('{Enter}')
    expect(screen.getByRole('dialog', { name: '网络检查' })).toBeTruthy()
    await user.keyboard('{Escape}')
    expect(screen.queryByRole('dialog')).toBeNull()
  })

  it('keeps Mantle, client notices, Google diagnostics and global actions accessible', async () => {
    Object.assign(window, { matouDesktop: {
      safeBrowserStatus: vi.fn().mockResolvedValue({ installed: true, version: '148.0.7778.215' }),
      runningDesktopClients: vi.fn().mockResolvedValue(['claude'])
    } })
    const status = baseStatus()
    status.families[0]!.violation = 'rule'
    status.families.push({ ...status.families[0]!, family: 'google' })
    await renderChip(status, 'session-deepseek')
    const request = vi.mocked(currentClient!.request)
    fireEvent.click(screen.getByRole('button', { name: '网络检查' }))
    expect(await screen.findByRole('region', { name: 'Mantle' })).toBeTruthy()
    expect(await screen.findByText('检测到 Claude Desktop 正在运行，它的出口不在保护范围')).toBeTruthy()
    expect(screen.getByRole('heading', { name: 'Google（通用出口检测）' })).toBeTruthy()
    await act(async () => fireEvent.click(screen.getByRole('button', { name: '重新检查' })))
    expect(request).toHaveBeenCalledWith('egress.recheck', undefined)
    await act(async () => fireEvent.click(screen.getByRole('radio', { name: '固定 IP' })))
    expect(request).toHaveBeenCalledWith('egress.set-rule', { mode: 'ip' })
    await act(async () => fireEvent.click(screen.getAllByRole('button', { name: '以当前出口为准' })[0]!))
    expect(request).toHaveBeenCalledWith('egress.accept-current', { family: 'anthropic' })
    await act(async () => fireEvent.click(screen.getByRole('checkbox', { name: /出口保护/ })))
    expect(request).toHaveBeenCalledWith('egress.set-protection', { enabled: false })
  })

  it('updates the global label when the shared status changes', async () => {
    let status = baseStatus()
    let notify = () => {}
    currentClient = {
      getEgressStatusSnapshot: () => status,
      subscribeEgressStatus: (listener: () => void) => { notify = listener; return () => {} }
    } as unknown as RuntimeClient
    const view = render(<LocaleProvider initialLocale="zh-CN"><EgressChip sessionId="session-shell" /></LocaleProvider>)
    act(() => { status = baseStatus({ state: 'warning' }); notify() })
    expect(screen.getByRole('button', { name: '网络检查' }).textContent).toBe('1 项提醒')
    view.rerender(<LocaleProvider initialLocale="zh-CN"><EgressChip sessionId="session-claude" /></LocaleProvider>)
    expect(screen.getByRole('button', { name: '🇺🇸 美国 · 出口正常' })).toBeTruthy()
  })

  it.each([null, {}])('does not crash without an egress client or bridge: %s', (client) => {
    currentClient = client as RuntimeClient | null
    const { container } = render(<LocaleProvider initialLocale="zh-CN"><EgressChip sessionId="session-shell" /></LocaleProvider>)
    const button = container.querySelector('button')
    expect(button === null || button.textContent === '无法确认出口').toBe(true)
  })

  it('uses the current locale for the global entry', async () => {
    await renderChip(baseStatus(), 'session-shell', 'en')
    expect(screen.getByRole('button', { name: 'Network check' }).textContent).toBe('Exit OK')
  })
})
