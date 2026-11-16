import { BrowserWindow, ipcMain, shell } from 'electron'
import { DESKTOP_CHANNELS } from '../shared/desktop-api'
import { isAllowedExternalUrl, SAFE_BROWSER_RELEASES_URL } from './external-url-policy'
import { SafeBrowserController, runningDesktopClients, safeBrowserCheckUpdate, safeBrowserStatus, safeBrowserTimeZone } from './safe-browser'

/** Narrow panel commands: all paths and URLs stay in the main process. */
export function registerSafeBrowserIpc(browser: SafeBrowserController, windows: ReadonlyMap<string, BrowserWindow>): void {
  function assertSender(event: Electron.IpcMainInvokeEvent): void {
    const window = BrowserWindow.fromWebContents(event.sender)
    if (!window || window.isDestroyed() || ![...windows.values()].includes(window) ||
      event.senderFrame !== event.sender.mainFrame) throw new Error('Invalid sender')
  }
  ipcMain.handle(DESKTOP_CHANNELS.safeBrowserStatus, event => {
    assertSender(event)
    return safeBrowserStatus()
  })
  ipcMain.handle(DESKTOP_CHANNELS.safeBrowserOpen, async event => {
    assertSender(event)
    await browser.openPersistent('about:blank', safeBrowserTimeZone())
  })
  ipcMain.handle(DESKTOP_CHANNELS.safeBrowserCheckUpdate, event => {
    assertSender(event)
    return safeBrowserCheckUpdate()
  })
  ipcMain.handle(DESKTOP_CHANNELS.openSafeBrowserReleases, async event => {
    assertSender(event)
    if (!isAllowedExternalUrl(SAFE_BROWSER_RELEASES_URL)) throw new Error('Release URL is not allowed')
    await shell.openExternal(SAFE_BROWSER_RELEASES_URL)
  })
  ipcMain.handle(DESKTOP_CHANNELS.runningDesktopClients, event => {
    assertSender(event)
    return runningDesktopClients()
  })
}
