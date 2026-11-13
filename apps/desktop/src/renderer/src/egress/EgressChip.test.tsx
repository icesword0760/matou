// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
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

  it('renders nothing for shell/third-party cards or when RPC fails, and opens EgressPanel on click', async () => {
    const { container: shellContainer } = await renderChip(baseStatus(), 'session-shell', 'zh-CN')
    expect(shellContainer.firstChild).toBeNull()

    cleanup()
    const { container: errContainer } = await renderChip(new Error('offline'), 'session-claude', 'zh-CN')
    expect(errContainer.firstChild).toBeNull()

    cleanup()
    await renderChip(baseStatus(), 'session-claude', 'zh-CN')
    fireEvent.click(screen.getByRole('button', { name: '🇺🇸 美国 · 出口正常' }))
    expect(screen.getByRole('dialog', { name: '网络检查' })).toBeTruthy()
  })
})
