// @vitest-environment jsdom
import { cleanup, render, screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import type { Locale, LocalePreference } from '@matou/contracts'

import { ModelSwitchSettings } from './ModelSwitchSettings'

afterEach(() => { cleanup(); Reflect.deleteProperty(window, 'matouDesktop') })

describe('Settings language category', () => {
  it('stores the chosen preference through the desktop bridge', async () => {
    const bridge = fakeBridge('system')
    render(<ModelSwitchSettings client={null} onClose={vi.fn()} />)
    const user = userEvent.setup()

    await user.click(screen.getByRole('button', { name: '语言' }))
    expect(checked(await screen.findByRole('radio', { name: '跟随系统' }))).toBe(true)
    await user.click(screen.getByRole('radio', { name: 'English' }))

    expect(bridge.setLocalePreference).toHaveBeenCalledWith('en')
    expect(checked(screen.getByRole('radio', { name: 'English' }))).toBe(true)
  })

  it('keeps the pane usable on "system" when the stored preference cannot be read', async () => {
    const bridge = fakeBridge('system')
    bridge.getLocalePreference.mockRejectedValueOnce(new Error('bridge unavailable'))
    render(<ModelSwitchSettings client={null} onClose={vi.fn()} />)
    const user = userEvent.setup()

    await user.click(screen.getByRole('button', { name: '语言' }))
    expect(checked(screen.getByRole('radio', { name: '跟随系统' }))).toBe(true)
    await user.click(screen.getByRole('radio', { name: '中文' }))

    expect(bridge.setLocalePreference).toHaveBeenCalledWith('zh-CN')
  })

  it('reports the locale MATOU_LOCALE forces when it differs from the choice', async () => {
    fakeBridge('system', 'en')
    render(<ModelSwitchSettings client={null} onClose={vi.fn()} />)
    const user = userEvent.setup()

    await user.click(screen.getByRole('button', { name: '语言' }))
    await user.click(screen.getByRole('radio', { name: '中文' }))

    expect(await screen.findByText('当前由环境变量 MATOU_LOCALE 固定为 English')).toBeTruthy()
  })
})

/** `forced` stands in for MATOU_LOCALE: the main process then ignores the stored preference. */
function fakeBridge(stored: LocalePreference, forced?: Locale) {
  const bridge = {
    getLocalePreference: vi.fn(async (): Promise<LocalePreference> => stored),
    setLocalePreference: vi.fn(async (preference: LocalePreference): Promise<Locale> =>
      forced ?? (preference === 'system' ? 'zh-CN' : preference))
  }
  Object.assign(window, { matouDesktop: bridge })
  return bridge
}

function checked(element: HTMLElement) { return (element as HTMLInputElement).checked }
