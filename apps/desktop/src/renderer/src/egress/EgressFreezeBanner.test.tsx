// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { EgressStatusWire } from '@matou/contracts'
import { LocaleProvider } from '../i18n/LocaleProvider'
import { EgressFreezeBanner } from './EgressFreezeBanner'

afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

function frozenStatus(
  violation: 'mainland' | 'rule' = 'mainland',
  overrides: Partial<EgressStatusWire> = {}
): EgressStatusWire {
  return {
    state: 'abnormal',
    protectionEnabled: true,
    ruleMode: 'country',
    checkedAt: 1_700_000_000_000,
    families: [
      {
        family: 'anthropic',
        state: 'abnormal',
        violation,
        baseline: { ip: '169.128.10.5', country: 'US' },
        exit: { ip: '103.50.10.8', country: 'JP' },
        hosts: [],
        overridden: false,
        frozenSessionIds: ['session-1'],
        recovering: false
      }
    ],
    checks: [],
    sessionFamilies: { 'session-1': 'anthropic' },
    ...overrides
  }
}

describe('EgressFreezeBanner', () => {
  it('renders mainland frozen copy in zh and en', () => {
    render(
      <LocaleProvider initialLocale="zh-CN">
        <EgressFreezeBanner
          sessionId="session-1"
          status={frozenStatus('mainland')}
          onRecheck={vi.fn()}
          onOverride={vi.fn()}
        />
      </LocaleProvider>
    )
    expect(
      screen.getByText('出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。')
    ).toBeTruthy()

    cleanup()
    render(
      <LocaleProvider initialLocale="en">
        <EgressFreezeBanner
          sessionId="session-1"
          status={frozenStatus('mainland')}
          onRecheck={vi.fn()}
          onOverride={vi.fn()}
        />
      </LocaleProvider>
    )
    expect(
      screen.getByText(
        'Exit problem (mainland China IP detected). Paused; resumes automatically when the network recovers.'
      )
    ).toBeTruthy()
  })

  it('renders rule violation copy with baseline and current country in zh and en', () => {
    render(
      <LocaleProvider initialLocale="zh-CN">
        <EgressFreezeBanner
          sessionId="session-1"
          status={frozenStatus('rule')}
          onRecheck={vi.fn()}
          onOverride={vi.fn()}
        />
      </LocaleProvider>
    )
    expect(
      screen.getByText('出口与规则不符（美国 → 日本），已暂停。网络恢复后自动继续。')
    ).toBeTruthy()

    cleanup()
    render(
      <LocaleProvider initialLocale="en">
        <EgressFreezeBanner
          sessionId="session-1"
          status={frozenStatus('rule')}
          onRecheck={vi.fn()}
          onOverride={vi.fn()}
        />
      </LocaleProvider>
    )
    expect(
      screen.getByText(
        "Exit doesn't match your rule (United States → Japan). Paused; resumes automatically when the network recovers."
      )
    ).toBeTruthy()
  })

  it('triggers recheck and requires confirmation before sending override', async () => {
    const onRecheck = vi.fn()
    const onOverride = vi.fn()
    render(
      <LocaleProvider initialLocale="zh-CN">
        <EgressFreezeBanner
          sessionId="session-1"
          status={frozenStatus('mainland')}
          onRecheck={onRecheck}
          onOverride={onOverride}
        />
      </LocaleProvider>
    )

    fireEvent.click(screen.getByRole('button', { name: '重新检查' }))
    expect(onRecheck).toHaveBeenCalledTimes(1)

    fireEvent.click(screen.getByRole('button', { name: '仍要继续' }))
    expect(screen.getByRole('alertdialog', { name: '仍要继续？' })).toBeTruthy()
    expect(
      screen.getByText(
        '出口仍然异常，继续后 agent 会用当前出口访问服务商。出口再次变化前不会再暂停。'
      )
    ).toBeTruthy()
    expect(onOverride).not.toHaveBeenCalled()

    // Canceling closes dialog without overriding.
    fireEvent.click(screen.getByRole('button', { name: '取消' }))
    expect(screen.queryByRole('alertdialog')).toBeNull()
    expect(onOverride).not.toHaveBeenCalled()

    // Confirming sends override for the session's family.
    fireEvent.click(screen.getByRole('button', { name: '仍要继续' }))
    fireEvent.click(screen.getByRole('button', { name: '继续' }))
    expect(onOverride).toHaveBeenCalledWith('anthropic')
  })

  it('shows resumed banner only for matching family and hides it after 3000 ms', () => {
    vi.useFakeTimers()
    const resumedStatus: EgressStatusWire = {
      state: 'ok',
      protectionEnabled: true,
      ruleMode: 'country',
      checkedAt: 1_700_000_060_000,
      families: [
        {
          family: 'anthropic',
          state: 'ok',
          exit: { ip: '169.128.10.5', country: 'US' },
          hosts: [],
          overridden: false,
          frozenSessionIds: [],
          recovering: false
        }
      ],
      checks: [],
      sessionFamilies: {
        'session-1': 'anthropic',
        'session-openai': 'openai'
      },
      lastTransition: {
        id: 't-resume-1',
        kind: 'resumed',
        family: 'anthropic',
        sessionCount: 1,
        at: 1_700_000_060_000
      }
    }

    render(
      <LocaleProvider initialLocale="zh-CN">
        <EgressFreezeBanner
          sessionId="session-1"
          status={resumedStatus}
          onRecheck={vi.fn()}
          onOverride={vi.fn()}
        />
      </LocaleProvider>
    )
    expect(screen.getByText('出口已恢复，已自动继续')).toBeTruthy()

    act(() => {
      vi.advanceTimersByTime(2999)
    })
    expect(screen.getByText('出口已恢复，已自动继续')).toBeTruthy()

    act(() => {
      vi.advanceTimersByTime(1)
    })
    expect(screen.queryByText('出口已恢复，已自动继续')).toBeNull()

    // Non-matching session (e.g., shell) renders nothing.
    cleanup()
    const { container } = render(
      <LocaleProvider initialLocale="zh-CN">
        <EgressFreezeBanner
          sessionId="session-shell"
          status={resumedStatus}
          onRecheck={vi.fn()}
          onOverride={vi.fn()}
        />
      </LocaleProvider>
    )
    expect(container.firstChild).toBeNull()
  })
})
