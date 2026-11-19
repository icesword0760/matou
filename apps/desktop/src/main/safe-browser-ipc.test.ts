import { beforeEach, describe, expect, it, vi } from 'vitest'
import { BrowserWindow, ipcMain, shell } from 'electron'
import { DESKTOP_CHANNELS } from '../shared/desktop-api'
import { registerSafeBrowserIpc } from './safe-browser-ipc'
import { runningDesktopClients, safeBrowserCheckUpdate, safeBrowserStatus, type SafeBrowserController } from './safe-browser'
import { SAFE_BROWSER_RELEASES_URL } from './external-url-policy'

vi.mock('electron', () => ({ BrowserWindow: { fromWebContents: vi.fn() }, ipcMain: { handle: vi.fn() }, shell: { openExternal: vi.fn() } }))
vi.mock('./safe-browser', () => ({ safeBrowserStatus: vi.fn(), safeBrowserCheckUpdate: vi.fn(), runningDesktopClients: vi.fn(), safeBrowserTimeZone: () => 'Asia/Tokyo' }))
const channels = ['safeBrowserStatus', 'safeBrowserOpen', 'safeBrowserCheckUpdate', 'openSafeBrowserReleases', 'runningDesktopClients'] as const
let window: BrowserWindow
let event: Electron.IpcMainInvokeEvent
const openPersistent = vi.fn()
const invoke = (name: typeof channels[number], ...args: unknown[]) => {
  const entry = vi.mocked(ipcMain.handle).mock.calls.find(call => call[0] === DESKTOP_CHANNELS[name])!
  return entry[1](event, ...args)
}
beforeEach(() => {
  vi.clearAllMocks()
  window = { isDestroyed: () => false } as BrowserWindow
  const frame = {}
  event = { sender: { mainFrame: frame }, senderFrame: frame } as Electron.IpcMainInvokeEvent
  vi.mocked(BrowserWindow.fromWebContents).mockReturnValue(window)
  registerSafeBrowserIpc({ openPersistent } as unknown as SafeBrowserController, new Map([['main', window]]))
})
describe('safe browser panel IPC', () => {
  it('serves fixed panel commands for an owned top-level sender and ignores renderer paths and URLs', async () => {
    vi.mocked(safeBrowserStatus).mockReturnValue({ installed: true, version: '148.0.7778.215' })
    vi.mocked(safeBrowserCheckUpdate).mockResolvedValue({ latest: null, failed: true })
    vi.mocked(runningDesktopClients).mockResolvedValue(['claude', 'chatgpt'])
    expect(await invoke('safeBrowserStatus', '/untrusted')).toEqual({ installed: true, version: '148.0.7778.215' })
    await invoke('safeBrowserOpen', 'https://untrusted.invalid', '/untrusted')
    expect(openPersistent).toHaveBeenCalledExactlyOnceWith('about:blank', 'Asia/Tokyo')
    expect(await invoke('safeBrowserCheckUpdate', 'https://untrusted.invalid')).toEqual({ latest: null, failed: true })
    expect(safeBrowserCheckUpdate).toHaveBeenCalledExactlyOnceWith()
    await invoke('openSafeBrowserReleases', 'https://untrusted.invalid')
    expect(shell.openExternal).toHaveBeenCalledExactlyOnceWith(SAFE_BROWSER_RELEASES_URL)
    expect(await invoke('runningDesktopClients', '/untrusted')).toEqual(['claude', 'chatgpt'])
    expect(runningDesktopClients).toHaveBeenCalledExactlyOnceWith()
  })
  it.each(channels)('rejects unknown, foreign-window, destroyed and subframe senders for %s', async channel => {
    const reject = async () => expect(Promise.resolve().then(() => invoke(channel))).rejects.toThrow('Invalid sender')
    vi.mocked(BrowserWindow.fromWebContents).mockReturnValue(null)
    await reject()
    vi.mocked(BrowserWindow.fromWebContents).mockReturnValue({ isDestroyed: () => false } as BrowserWindow)
    await reject()
    vi.mocked(BrowserWindow.fromWebContents).mockReturnValue(window)
    vi.spyOn(window, 'isDestroyed').mockReturnValue(true)
    await reject()
    vi.mocked(window.isDestroyed).mockReturnValue(false)
    event = { ...event, senderFrame: {} } as Electron.IpcMainInvokeEvent
    await reject()
    expect(openPersistent).not.toHaveBeenCalled()
    expect(shell.openExternal).not.toHaveBeenCalled()
    expect(safeBrowserStatus).not.toHaveBeenCalled()
    expect(safeBrowserCheckUpdate).not.toHaveBeenCalled()
    expect(runningDesktopClients).not.toHaveBeenCalled()
  })
})
