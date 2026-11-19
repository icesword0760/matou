import { beforeEach, describe, expect, it, vi } from 'vitest'
import { BrowserWindow, dialog, ipcMain, shell } from 'electron'
import { PROVIDER_PRESETS } from '@matou/contracts'
import { DESKTOP_CHANNELS } from '../shared/desktop-api'
import { SAFE_BROWSER_RELEASES_URL } from './external-url-policy'
import type { SafeBrowserController } from './safe-browser'
import { openWebLink, registerWebLinkIpc } from './web-links'

vi.mock('electron', () => ({
  BrowserWindow: { fromWebContents: vi.fn(), getFocusedWindow: vi.fn() },
  ipcMain: { handle: vi.fn() },
  dialog: { showMessageBox: vi.fn() },
  shell: { openExternal: vi.fn() }
}))
vi.mock('./safe-browser', () => ({ safeBrowserTimeZone: () => 'Asia/Tokyo' }))

const url = 'https://ip.net.coffee/'
const authorizationUrl = 'https://auth.openai.com/oauth/authorize?redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback'
const openPersistent = vi.fn()
let window: BrowserWindow
let event: Electron.IpcMainInvokeEvent
const invoke = (channel: 'openExternal' | 'openProviderKeyPage', ...args: unknown[]) => {
  const entry = vi.mocked(ipcMain.handle).mock.calls.find(call => call[0] === DESKTOP_CHANNELS[channel])!
  return entry[1](event, ...args)
}

beforeEach(() => {
  vi.resetAllMocks()
  openPersistent.mockResolvedValue(undefined)
  window = { isDestroyed: vi.fn(() => false) } as unknown as BrowserWindow
  const frame = {}
  event = { sender: { mainFrame: frame }, senderFrame: frame } as Electron.IpcMainInvokeEvent
  vi.mocked(BrowserWindow.fromWebContents).mockReturnValue(window)
  vi.mocked(BrowserWindow.getFocusedWindow).mockReturnValue(window)
  registerWebLinkIpc({ openPersistent } as unknown as SafeBrowserController, new Map([['main', window]]))
})

describe('openWebLink', () => {
  it('opens allowed links in the shared persistent Mantle with the current timezone without a dialog', async () => {
    await openWebLink(url)
    expect(openPersistent).toHaveBeenCalledExactlyOnceWith(url, 'Asia/Tokyo')
    expect(dialog.showMessageBox).not.toHaveBeenCalled()
    expect(shell.openExternal).not.toHaveBeenCalled()
  })

  it.each([0, 1, 2])('shows the missing-Mantle modal and handles button %s', async response => {
    openPersistent.mockRejectedValue(Object.assign(new Error('missing'), { code: 'safe-browser-missing' }))
    vi.mocked(dialog.showMessageBox).mockResolvedValue({ response, checkboxChecked: false })
    await openWebLink(url)
    expect(dialog.showMessageBox).toHaveBeenCalledExactlyOnceWith(window, {
      type: 'info', title: 'Mantle 未安装',
      message: '从 Matou 打开的网页默认使用 Mantle 浏览器。',
      detail: '安装 Mantle 后，登录和网页都会在受控环境里打开。',
      buttons: ['安装 Mantle', '用系统浏览器打开', '取消'], defaultId: 0, cancelId: 2
    })
    if (response === 2) expect(shell.openExternal).not.toHaveBeenCalled()
    else expect(shell.openExternal).toHaveBeenCalledExactlyOnceWith(response === 0 ? SAFE_BROWSER_RELEASES_URL : url)
    expect(openPersistent).toHaveBeenCalledOnce()
  })

  it.each([0, 1])('shows the launch-failure modal and handles button %s', async response => {
    openPersistent.mockRejectedValue(new Error('launch failed'))
    vi.mocked(dialog.showMessageBox).mockResolvedValue({ response, checkboxChecked: false })
    await openWebLink(url)
    expect(dialog.showMessageBox).toHaveBeenCalledExactlyOnceWith(window, {
      type: 'warning', title: 'Mantle 无法打开', message: '这次没能用 Mantle 打开网页。',
      buttons: ['用系统浏览器打开', '取消'], defaultId: 0, cancelId: 0
    })
    if (response === 1) expect(shell.openExternal).not.toHaveBeenCalled()
    else expect(shell.openExternal).toHaveBeenCalledExactlyOnceWith(url)
  })

  it('waits for user choice before opening anything in the system browser', async () => {
    openPersistent.mockRejectedValue({ code: 'safe-browser-missing' })
    let choose!: (value: Electron.MessageBoxReturnValue) => void
    vi.mocked(dialog.showMessageBox).mockReturnValue(new Promise(resolve => { choose = resolve }))
    const opening = openWebLink(url)
    await vi.waitFor(() => expect(dialog.showMessageBox).toHaveBeenCalledOnce())
    expect(shell.openExternal).not.toHaveBeenCalled()
    choose({ response: 2, checkboxChecked: false })
    await opening
    expect(shell.openExternal).not.toHaveBeenCalled()
  })

  it('can show a dialog when no live window is focused', async () => {
    vi.mocked(window.isDestroyed).mockReturnValue(true)
    openPersistent.mockRejectedValue({ code: 'safe-browser-missing' })
    vi.mocked(dialog.showMessageBox).mockResolvedValue({ response: 2, checkboxChecked: false })
    await openWebLink(url)
    expect(dialog.showMessageBox).toHaveBeenCalledWith(expect.objectContaining({ title: 'Mantle 未安装' }))
  })

  it.each(['https://example.invalid', 'http://ip.net.coffee', 'https://ip.net.coffee.evil.test', authorizationUrl,
    `${PROVIDER_PRESETS.codex[0]!.keyUrl}/extra`])('rejects untrusted ordinary URL %s without browser or dialog', async candidate => {
    await expect(openWebLink(candidate)).rejects.toThrow('External URL is not allowed')
    expect(openPersistent).not.toHaveBeenCalled()
    expect(dialog.showMessageBox).not.toHaveBeenCalled()
    expect(shell.openExternal).not.toHaveBeenCalled()
  })

  it('accepts only preset key pages and revalidates controlled sign-in URLs', async () => {
    for (const preset of PROVIDER_PRESETS.codex) await openWebLink(preset.keyUrl)
    await openWebLink(authorizationUrl, { source: 'sign-in' })
    expect(openPersistent).toHaveBeenLastCalledWith(authorizationUrl, 'Asia/Tokyo')
    await expect(openWebLink(authorizationUrl.replace('1455', '9999'), { source: 'sign-in' })).rejects.toThrow('External URL is not allowed')
    await expect(openWebLink('https://example.invalid', { source: 'sign-in' })).rejects.toThrow('External URL is not allowed')
    expect(openPersistent).toHaveBeenCalledTimes(PROVIDER_PRESETS.codex.length + 1)
  })
})

describe('web link IPC', () => {
  it('routes allowed ordinary links and preset key pages through Mantle', async () => {
    expect(await invoke('openExternal', url)).toBe(true)
    await invoke('openProviderKeyPage', 'codex', 'deepseek')
    expect(openPersistent.mock.calls).toEqual([[url, 'Asia/Tokyo'], [PROVIDER_PRESETS.codex[0]!.keyUrl, 'Asia/Tokyo']])
  })

  it('keeps ordinary IPC restricted to the original external allowlist', async () => {
    for (const candidate of [authorizationUrl, PROVIDER_PRESETS.codex[0]!.keyUrl, 'https://example.invalid', null]) {
      expect(await invoke('openExternal', candidate)).toBe(false)
    }
    await expect(invoke('openProviderKeyPage', 'unknown', 'deepseek')).rejects.toThrow('Invalid CLI')
    await expect(invoke('openProviderKeyPage', 'codex', 'unknown')).rejects.toThrow('Unknown provider preset')
    expect(openPersistent).not.toHaveBeenCalled()
    expect(dialog.showMessageBox).not.toHaveBeenCalled()
  })

  it.each(['openExternal', 'openProviderKeyPage'] as const)('rejects unknown, foreign, destroyed and subframe senders for %s', async channel => {
    const reject = async () => expect(Promise.resolve().then(() => invoke(channel, url))).rejects.toThrow('Invalid sender')
    vi.mocked(BrowserWindow.fromWebContents).mockReturnValue(null)
    await reject()
    vi.mocked(BrowserWindow.fromWebContents).mockReturnValue({ isDestroyed: () => false } as BrowserWindow)
    await reject()
    vi.mocked(BrowserWindow.fromWebContents).mockReturnValue(window)
    vi.mocked(window.isDestroyed).mockReturnValue(true)
    await reject()
    vi.mocked(window.isDestroyed).mockReturnValue(false)
    event = { ...event, senderFrame: {} } as Electron.IpcMainInvokeEvent
    await reject()
    expect(openPersistent).not.toHaveBeenCalled()
    expect(dialog.showMessageBox).not.toHaveBeenCalled()
    expect(shell.openExternal).not.toHaveBeenCalled()
  })
})
