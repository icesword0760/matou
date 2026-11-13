// @vitest-environment jsdom
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { EgressStatusWire } from '@matou/contracts'
import { LocaleProvider } from '../i18n/LocaleProvider'
import { EgressPanel } from './EgressPanel'

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
})

function buildSampleStatus(overrides: Partial<EgressStatusWire> = {}): EgressStatusWire {
  return {
    state: 'abnormal',
    protectionEnabled: true,
    ruleMode: 'country',
    checkedAt: new Date(2026, 10, 13, 9, 5).getTime(),
    families: [
      {
        family: 'anthropic',
        state: 'abnormal',
        violation: 'rule',
        exit: { ip: '169.128.44.9', country: 'US' },
        hosts: [],
        overridden: false,
        frozenSessionIds: ['session-secret-1'],
        recovering: false
      },
      {
        family: 'openai',
        state: 'ok',
        exit: { ip: '198.51.100.42', country: 'JP' },
        hosts: [],
        overridden: false,
        frozenSessionIds: [],
        recovering: false
      }
    ],
    checks: [
      { id: 'mainland', family: 'anthropic', result: 'fail', facts: { host: 'api.anthropic.com' } },
      {
        id: 'exit-rule',
        family: 'anthropic',
        result: 'fail',
        facts: {
          baselineCountry: 'US',
          baselineIp: '169.128.44.9',
          currentCountry: 'HK',
          currentIp: '103.50.10.8'
        }
      },
      {
        id: 'login-api-match',
        family: 'anthropic',
        result: 'warn',
        facts: { loginIp: '169.128.1.2', apiIp: '169.128.3.4' }
      },
      { id: 'mainland', family: 'openai', result: 'pass' },
      {
        id: 'browser-path',
        result: 'warn',
        facts: { browserIp: '203.0.113.5', agentIp: '198.51.100.42' }
      },
      { id: 'agent-proxy-path', result: 'warn' },
      { id: 'time-zone', result: 'unknown' },
      { id: 'ipv6', result: 'warn' }
    ],
    sessionFamilies: {
      'session-secret-1': 'anthropic',
      'session-secret-2': 'openai'
    },
    ...overrides
  }
}

describe('EgressPanel', () => {
  it('renders exact zh copy, grouped checks, actions, and masks report IPs without session IDs', async () => {
    const onClose = vi.fn()
    const onRecheck = vi.fn()
    const onSetRule = vi.fn()
    const onAcceptCurrent = vi.fn()
    const onSetProtection = vi.fn()
    const writeClipboardText = vi.fn().mockResolvedValue(undefined)
    const openExternal = vi.fn().mockResolvedValue(true)
    const windowOpen = vi.spyOn(window, 'open').mockImplementation(() => null)
    ;(window as unknown as { matouDesktop?: unknown }).matouDesktop = {
      writeClipboardText,
      openExternal
    }

    render(
      <LocaleProvider initialLocale="zh-CN">
        <EgressPanel
          status={buildSampleStatus()}
          onClose={onClose}
          onRecheck={onRecheck}
          onSetRule={onSetRule}
          onAcceptCurrent={onAcceptCurrent}
          onSetProtection={onSetProtection}
        />
      </LocaleProvider>
    )

    expect(screen.getByRole('heading', { name: '网络检查' })).toBeTruthy()
    expect(screen.getByText('检测于 09:05')).toBeTruthy()
    expect(screen.getByRole('heading', { name: 'Claude（Anthropic）' })).toBeTruthy()
    expect(screen.getByRole('heading', { name: 'Codex（OpenAI）' })).toBeTruthy()
    expect(screen.queryByRole('heading', { name: 'Claude（ChatGPT 订阅）' })).toBeNull()
    const thisMacSection = screen.getByRole('region', { name: '本机' })
    expect(thisMacSection).toBeTruthy()
    expect(thisMacSection.textContent).toContain(
      '浏览器 203.0.113.5，终端 198.51.100.42。授权登录和使用时 IP 不同。'
    )

    expect(
      screen.getByText(
        'api.anthropic.com 的出口在中国大陆。检查代理是否开启；规则模式下确认该域名走代理。'
      )
    ).toBeTruthy()
    expect(
      screen.getByText(
        '美国 → 中国香港特别行政区（103.50.10.8）。确认节点；如果是预期变化，点「以当前出口为准」。'
      )
    ).toBeTruthy()
    expect(
      screen.getByText(
        '登录 169.128.1.2，API 169.128.3.4。多半是分流规则把两个域名分到了不同节点。'
      )
    ).toBeTruthy()
    expect(screen.getByText('日本 · 198.51.100.42')).toBeTruthy()
    expect(
      screen.getByText(
        '浏览器 203.0.113.5，终端 198.51.100.42。授权登录和使用时 IP 不同。'
      )
    ).toBeTruthy()
    expect(
      screen.getByText(
        '已开系统代理，但终端里的 agent 不读系统代理。请在 Clash 开启 TUN，或在 shell 配置里设置 HTTPS_PROXY。'
      )
    ).toBeTruthy()
    expect(screen.getByText('无法确认（检测超时）')).toBeTruthy()
    expect(
      screen.getByText(
        '本机有公网 IPv6，部分 agent 可能经 IPv6 直连。可在 Clash 开启 IPv6 接管，或在系统里关闭 IPv6。'
      )
    ).toBeTruthy()

    // Use current exit button only appears on anthropic (violation === 'rule'), not openai
    const acceptButtons = screen.getAllByRole('button', { name: '以当前出口为准' })
    expect(acceptButtons).toHaveLength(1)
    fireEvent.click(acceptButtons[0]!)
    expect(onAcceptCurrent).toHaveBeenCalledWith('anthropic')

    fireEvent.click(screen.getByRole('button', { name: '重新检查' }))
    expect(onRecheck).toHaveBeenCalledTimes(1)

    fireEvent.click(screen.getByRole('radio', { name: '固定 IP' }))
    expect(onSetRule).toHaveBeenCalledWith('ip')

    fireEvent.click(
      screen.getByRole('checkbox', {
        name: '出口保护：出口异常时暂停 Claude / Codex 会话'
      })
    )
    expect(onSetProtection).toHaveBeenCalledWith(false)

    // Reference links go through restricted IPC, never window.open
    fireEvent.click(screen.getByRole('button', { name: /IP 质量/ }))
    expect(openExternal).toHaveBeenCalledWith('https://ip.net.coffee/')
    fireEvent.click(screen.getByRole('button', { name: /浏览器环境/ }))
    expect(openExternal).toHaveBeenCalledWith('https://fuck-claude.vercel.app/')
    expect(windowOpen).not.toHaveBeenCalled()

    // Copy report masks IPs and excludes session IDs
    fireEvent.click(screen.getByRole('button', { name: '复制报告' }))
    await waitFor(() => {
      expect(writeClipboardText).toHaveBeenCalledTimes(1)
    })
    const copiedReport = writeClipboardText.mock.calls[0]![0] as string
    expect(copiedReport).toContain('169.128.x.x')
    expect(copiedReport).toContain('103.50.x.x')
    expect(copiedReport).not.toContain('169.128.44.9')
    expect(copiedReport).not.toContain('103.50.10.8')
    expect(copiedReport).not.toContain('198.51.100.42')
    expect(copiedReport).not.toContain('session-secret-1')
    expect(copiedReport).not.toContain('session-secret-2')
  })

  it('renders exact English copy', () => {
    render(
      <LocaleProvider initialLocale="en">
        <EgressPanel
          status={buildSampleStatus({
            checks: [
              { id: 'mainland', family: 'anthropic', result: 'fail', facts: { host: 'api.anthropic.com' } },
              { id: 'time-zone', result: 'warn' }
            ]
          })}
          onClose={vi.fn()}
          onRecheck={vi.fn()}
          onSetRule={vi.fn()}
          onAcceptCurrent={vi.fn()}
          onSetProtection={vi.fn()}
        />
      </LocaleProvider>
    )

    expect(screen.getByRole('heading', { name: 'Network check' })).toBeTruthy()
    expect(screen.getByText('Checked at 09:05')).toBeTruthy()
    expect(
      screen.getByText(
        'api.anthropic.com exits from mainland China. Check that your proxy is on and that this domain is routed through it.'
      )
    ).toBeTruthy()
    expect(
      screen.getByText(
        'Sessions would use a mainland China time zone. In Settings → Claude Code → Time zone, choose Automatic.'
      )
    ).toBeTruthy()
    expect(
      screen.getByText(
        'Sites are only contacted when you click, and they will see your exit IP.'
      )
    ).toBeTruthy()
    expect(
      screen.getByText(
        'Checks are sampled every minute and cannot guarantee every connection uses the same exit.'
      )
    ).toBeTruthy()
  })

  it('shows the ChatGPT subscription group in panel and copied report only when present in status', async () => {
    const writeClipboardText = vi.fn().mockResolvedValue(undefined)
    ;(window as unknown as { matouDesktop?: unknown }).matouDesktop = {
      writeClipboardText
    }

    const withSubscription = buildSampleStatus({
      families: [
        ...buildSampleStatus().families,
        {
          family: 'chatgpt-subscription',
          state: 'ok',
          exit: { ip: '203.0.113.88', country: 'SG' },
          hosts: [],
          overridden: false,
          frozenSessionIds: [],
          recovering: false
        }
      ],
      checks: [
        ...buildSampleStatus().checks,
        { id: 'mainland', family: 'chatgpt-subscription', result: 'pass' }
      ]
    })

    render(
      <LocaleProvider initialLocale="zh-CN">
        <EgressPanel
          status={withSubscription}
          onClose={vi.fn()}
          onRecheck={vi.fn()}
          onSetRule={vi.fn()}
          onAcceptCurrent={vi.fn()}
          onSetProtection={vi.fn()}
        />
      </LocaleProvider>
    )

    expect(screen.getByRole('heading', { name: 'Claude（ChatGPT 订阅）' })).toBeTruthy()
    expect(screen.getByText('新加坡 · 203.0.113.88')).toBeTruthy()

    fireEvent.click(screen.getByRole('button', { name: '复制报告' }))
    await waitFor(() => {
      expect(writeClipboardText).toHaveBeenCalledTimes(1)
    })
    const copiedReport = writeClipboardText.mock.calls[0]![0] as string
    expect(copiedReport).toContain('[Claude（ChatGPT 订阅）]')
    expect(copiedReport).toContain('203.0.x.x')
    expect(copiedReport).not.toContain('203.0.113.88')

    cleanup()
    render(
      <LocaleProvider initialLocale="en">
        <EgressPanel
          status={withSubscription}
          onClose={vi.fn()}
          onRecheck={vi.fn()}
          onSetRule={vi.fn()}
          onAcceptCurrent={vi.fn()}
          onSetProtection={vi.fn()}
        />
      </LocaleProvider>
    )
    expect(screen.getByRole('heading', { name: 'Claude (ChatGPT subscription)' })).toBeTruthy()
  })
})
