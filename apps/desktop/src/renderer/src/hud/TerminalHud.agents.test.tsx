// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, expect, it, vi } from 'vitest'

import { LocaleProvider } from '../i18n/LocaleProvider'
import { setCurrentLocale } from '../i18n/current'
import { TerminalHud } from './TerminalHud'

afterEach(() => {
  cleanup()
  vi.useRealTimers()
  setCurrentLocale('zh-CN')
})

it.each([
  ['zh-CN', 'Codex：matou-订阅账号-01（gpt-6.1-sol · 高档推理）', '2分31秒', '2分32秒'],
  ['en', 'Codex: matou-subscription-accounts-01 (gpt-6.1-sol · High reasoning)', '2m 31s', '2m 32s'],
  ['zh-CN', '名'.repeat(120), '2分31秒', '2分32秒'],
  ['en', 'A'.repeat(120), '2m 31s', '2m 32s']
] as const)('preserves the full %s agent label and independently updates its duration: %s', (locale, label, duration, nextDuration) => {
  vi.useFakeTimers()
  vi.setSystemTime(1_000_000)
  render(<LocaleProvider initialLocale={locale}><TerminalHud hud={{
    sessionId: 'long-agent-label', mode: 'agent', startedAt: Date.now(),
    agents: [{ id: 'worker', label, source: 'external', state: 'running',
      startedAt: Date.now() - 151_000, detail: 'Working on task' }]
  }} /></LocaleProvider>)

  fireEvent.focus(screen.getByText('1 Agents'))
  const name = screen.getByTitle(label)
  expect(name.textContent).toBe(label)
  expect(name.className).toBe('hud-agent-label')
  const timer = name.parentElement?.querySelector('.hud-agent-duration')
  expect(timer?.textContent).toBe(duration)
  expect(screen.getByText('Working on task').className).toBe('hud-agent-detail')
  act(() => vi.advanceTimersByTime(1_000))
  expect(timer?.textContent).toBe(nextDuration)
  expect(name.title).toBe(label)
})

it('clamps only agent details before paint and keeps them inside a resized viewport', () => {
  let viewport = 800
  const originalWidth = window.innerWidth
  const bounds = vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    return this.classList.contains('hud-detail-tooltip')
      ? { width: Math.min(440, viewport - 24) } as DOMRect
      : { left: viewport - 42, width: 30, top: 300 } as DOMRect
  })
  try {
    Object.defineProperty(window, 'innerWidth', { configurable: true, value: viewport })
    render(<TerminalHud hud={{ sessionId: 'edge', mode: 'agent', startedAt: Date.now(),
      agents: [{ id: 'worker', label: 'Long worker', source: 'external', state: 'running' }],
      configCounts: { instructionFiles: 0, mcpServers: 0, hooks: 1, hookNames: ['Stop'] }
    }} />)
    const trigger = screen.getByText('1 Agents')
    fireEvent.focus(trigger)
    expect(screen.getByRole('tooltip').style.left).toBe('568px')
    viewport = 320
    Object.defineProperty(window, 'innerWidth', { configurable: true, value: viewport })
    fireEvent(window, new Event('resize'))
    expect(screen.getByRole('tooltip').style.left).toBe('160px')
    fireEvent.blur(trigger)
    fireEvent.focus(screen.getByText('1 hooks'))
    const hooks = screen.getAllByRole('tooltip').find(el => !el.classList.contains('hud-agents-tooltip'))
    expect(hooks?.style.left).toBe('293px')
  } finally {
    cleanup()
    bounds.mockRestore()
    Object.defineProperty(window, 'innerWidth', { configurable: true, value: originalWidth })
  }
})
