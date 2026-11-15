// @vitest-environment jsdom
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { EgressStatusWire } from '@matou/contracts'
import { LocaleProvider } from '../i18n/LocaleProvider'
import { EgressPanel } from './EgressPanel'

const hint = '登录 Claude / ChatGPT / Antigravity 时使用'
const claudeNotice = '检测到 Claude Desktop 正在运行，它的出口不在保护范围'
const chatgptNotice = '检测到 ChatGPT 桌面版正在运行，它的出口不在保护范围'
const bridge = {
  safeBrowserStatus: vi.fn(), safeBrowserOpen: vi.fn(), safeBrowserCheckUpdate: vi.fn(),
  openSafeBrowserReleases: vi.fn(), runningDesktopClients: vi.fn()
}
const status: EgressStatusWire = {
  state: 'ok', protectionEnabled: true, ruleMode: 'country', families: [],
  checks: [{ id: 'time-zone', result: 'pass' }], sessionFamilies: {}
}
async function show(locale: 'zh-CN' | 'en' = 'zh-CN') {
  let view!: ReturnType<typeof render>
  await act(async () => {
    view = render(<LocaleProvider initialLocale={locale}><EgressPanel status={status} onClose={vi.fn()}
      onRecheck={vi.fn()} onSetRule={vi.fn()} onAcceptCurrent={vi.fn()} onSetProtection={vi.fn()} /></LocaleProvider>)
  })
  return view
}
beforeEach(() => {
  vi.useFakeTimers()
  for (const mock of Object.values(bridge)) mock.mockReset()
  bridge.safeBrowserStatus.mockResolvedValue({ installed: true, version: '148.0.7778.215' })
  bridge.safeBrowserOpen.mockResolvedValue(undefined)
  bridge.openSafeBrowserReleases.mockResolvedValue(undefined)
  bridge.runningDesktopClients.mockResolvedValue([])
  Object.assign(window, { matouDesktop: bridge })
})
afterEach(() => { cleanup(); vi.useRealTimers(); Reflect.deleteProperty(window, 'matouDesktop') })
describe('EgressPanel safe browser and client notices', () => {
  it.each(['zh-CN', 'en'] as const)('shows Mantle with the installed version, both actions and a URL-free open before reference checks in %s', async locale => {
    const { container } = await show(locale)
    const zh = locale === 'zh-CN'
    expect(screen.getByRole('region', { name: 'Mantle' })).toBeTruthy()
    expect(screen.getByText('Mantle')).toBeTruthy()
    expect(screen.getByText(`${zh ? '已安装' : 'Installed'} · 148.0.7778.215`)).toBeTruthy()
    expect(screen.getByText(zh ? hint : 'Used when signing in to Claude / ChatGPT / Antigravity')).toBeTruthy()
    expect(screen.getByRole('button', { name: zh ? '检查更新' : 'Check for updates' })).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: zh ? '打开' : 'Open' }))
    expect(bridge.safeBrowserOpen).toHaveBeenCalledExactlyOnceWith()
    expect(screen.queryByRole('button', { name: zh ? '获取安装包' : 'Get installer' })).toBeNull()
    const blocks = [...container.querySelector('.egress-panel__scroll')!.children]
    expect(blocks.indexOf(container.querySelector('.egress-safe-browser')!)).toBeLessThan(blocks.indexOf(container.querySelector('.egress-reference')!))
  })
  it('shows an uninstalled card with only the fixed installer action', async () => {
    bridge.safeBrowserStatus.mockResolvedValue({ installed: false, version: null })
    await show()
    expect(screen.getByRole('region', { name: 'Mantle' })).toBeTruthy()
    expect(screen.getByText('Mantle')).toBeTruthy()
    expect(screen.getByText('未安装')).toBeTruthy()
    expect(screen.getByText(hint)).toBeTruthy()
    expect(screen.queryByRole('button', { name: '打开' })).toBeNull()
    expect(screen.queryByRole('button', { name: '检查更新' })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: '获取安装包' }))
    expect(bridge.openSafeBrowserReleases).toHaveBeenCalledExactlyOnceWith()
  })
  it.each([
    [{ latest: null, failed: false }, '已是最新'],
    [{ latest: '148.1.0.0', failed: false }, '有新版本 · v148.1.0.0'],
    [{ latest: null, failed: true }, '检查失败']
  ])('shows update feedback %j for exactly three seconds', async (result, text) => {
    bridge.safeBrowserCheckUpdate.mockResolvedValue(result)
    await show()
    await act(async () => fireEvent.click(screen.getByRole('button', { name: '检查更新' })))
    expect(bridge.safeBrowserCheckUpdate).toHaveBeenCalledExactlyOnceWith()
    expect(screen.getByText(text)).toBeTruthy()
    expect(screen.queryByText(hint)).toBeNull()
    await act(async () => vi.advanceTimersByTimeAsync(2999))
    expect(screen.getByText(text)).toBeTruthy()
    await act(async () => vi.advanceTimersByTimeAsync(1))
    expect(screen.getByText(hint)).toBeTruthy()
    expect(screen.queryByText(text)).toBeNull()
  })
  it('disables the unchanged update button while pending and handles rejected IPC', async () => {
    let reject!: (error: Error) => void
    bridge.safeBrowserCheckUpdate.mockReturnValue(new Promise((_, no) => { reject = no }))
    await show()
    const button = screen.getByRole('button', { name: '检查更新' }) as HTMLButtonElement
    fireEvent.click(button)
    expect(button.disabled).toBe(true)
    fireEvent.click(button)
    expect(bridge.safeBrowserCheckUpdate).toHaveBeenCalledTimes(1)
    await act(async () => reject(new Error('offline')))
    expect(button.disabled).toBe(false)
    expect(screen.getByText('检查失败')).toBeTruthy()
  })
  it('shows both exact client notices after host checks and before controls; removes stopped clients every five seconds', async () => {
    bridge.runningDesktopClients.mockResolvedValueOnce(['claude', 'chatgpt']).mockResolvedValue([])
    const { container, unmount } = await show()
    expect(screen.getByText(claudeNotice)).toBeTruthy()
    expect(screen.getByText(chatgptNotice)).toBeTruthy()
    const blocks = [...container.querySelector('.egress-panel__scroll')!.children]
    const notices = blocks.indexOf(container.querySelector('.egress-client-notices')!)
    expect(notices).toBeGreaterThan(blocks.indexOf(screen.getByRole('region', { name: '本机' })))
    expect(notices).toBeLessThan(blocks.indexOf(container.querySelector('.egress-controls')!))
    await act(async () => vi.advanceTimersByTimeAsync(4999))
    expect(bridge.runningDesktopClients).toHaveBeenCalledTimes(1)
    await act(async () => vi.advanceTimersByTimeAsync(1))
    expect(bridge.runningDesktopClients).toHaveBeenCalledTimes(2)
    expect(screen.queryByText(claudeNotice)).toBeNull()
    expect(screen.queryByText(chatgptNotice)).toBeNull()
    unmount()
    await act(async () => vi.advanceTimersByTimeAsync(15000))
    expect(bridge.runningDesktopClients).toHaveBeenCalledTimes(2)
  })
  it('hides client notices on failure and stops an in-flight poll on panel close', async () => {
    bridge.runningDesktopClients.mockRejectedValueOnce(new Error('timeout'))
    const { unmount } = await show()
    expect(screen.queryByText(claudeNotice)).toBeNull()
    expect(screen.queryByText(chatgptNotice)).toBeNull()
    let resolve!: (clients: string[]) => void
    bridge.runningDesktopClients.mockReturnValue(new Promise(yes => { resolve = yes }))
    await act(async () => vi.advanceTimersByTimeAsync(5000))
    await act(async () => vi.advanceTimersByTimeAsync(15000))
    expect(bridge.runningDesktopClients).toHaveBeenCalledTimes(2)
    unmount()
    await act(async () => resolve(['claude']))
    await act(async () => vi.advanceTimersByTimeAsync(15000))
    expect(bridge.runningDesktopClients).toHaveBeenCalledTimes(2)
  })
})
