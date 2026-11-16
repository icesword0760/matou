import { BrowserWindow, dialog, ipcMain, shell } from 'electron'
import { PROVIDER_PRESETS } from '@matou/contracts'
import { isSubscriptionAuthorizationUrl } from '../../../runtime/src/model-router/authorization-url'
import { DESKTOP_CHANNELS } from '../shared/desktop-api'
import { isAllowedExternalUrl, SAFE_BROWSER_RELEASES_URL } from './external-url-policy'
import { type SafeBrowserController, safeBrowserTimeZone } from './safe-browser'

let browser: SafeBrowserController

export async function openWebLink(url: string, options?: { source?: 'link' | 'sign-in' }): Promise<void> {
  if (!isAllowedExternalUrl(url) &&
      !Object.values(PROVIDER_PRESETS).some(presets => presets.some(preset => preset.keyUrl === url)) &&
      !(options?.source === 'sign-in' &&
        (isSubscriptionAuthorizationUrl('codex', url) || isSubscriptionAuthorizationUrl('antigravity', url)))) {
    throw new Error('External URL is not allowed')
  }
  const parent = BrowserWindow.getFocusedWindow()
  let missing = false
  try {
    await browser.openPersistent(url, safeBrowserTimeZone())
    return
  } catch (error) {
    missing = (error as { code?: unknown } | null)?.code === 'safe-browser-missing'
  }
  const message: Electron.MessageBoxOptions = missing ? {
    type: 'info',
    title: 'Mantle 未安装',
    message: '从 Matou 打开的网页默认使用 Mantle 浏览器。',
    detail: '安装 Mantle 后，登录和网页都会在受控环境里打开。',
    buttons: ['安装 Mantle', '用系统浏览器打开', '取消'],
    defaultId: 0,
    cancelId: 2
  } : {
    type: 'warning',
    title: 'Mantle 无法打开',
    message: '这次没能用 Mantle 打开网页。',
    buttons: ['用系统浏览器打开', '取消'],
    defaultId: 0,
    cancelId: 0
  }
  const { response } = parent && !parent.isDestroyed()
    ? await dialog.showMessageBox(parent, message)
    : await dialog.showMessageBox(message)
  if (missing && response === 0) await shell.openExternal(SAFE_BROWSER_RELEASES_URL)
  else if (response === (missing ? 1 : 0)) await shell.openExternal(url)
}

/** Keep the existing controller/profile shared with panel commands and account sign-in. */
export function registerWebLinkIpc(controller: SafeBrowserController, windows: ReadonlyMap<string, BrowserWindow>): void {
  browser = controller
  function assertSender(event: Electron.IpcMainInvokeEvent): void {
    const window = BrowserWindow.fromWebContents(event.sender)
    if (!window || window.isDestroyed() || ![...windows.values()].includes(window) ||
      event.senderFrame !== event.sender.mainFrame) throw new Error('Invalid sender')
  }
  ipcMain.handle(DESKTOP_CHANNELS.openProviderKeyPage, async (event, cli: unknown, presetId: unknown) => {
    assertSender(event)
    if (cli !== 'claude-code' && cli !== 'codex') throw new Error('Invalid CLI')
    const preset = PROVIDER_PRESETS[cli].find(p => p.id === presetId)
    if (!preset) throw new Error('Unknown provider preset')
    await openWebLink(preset.keyUrl)
  })
  ipcMain.handle(DESKTOP_CHANNELS.openExternal, async (event, url: unknown) => {
    assertSender(event)
    if (!isAllowedExternalUrl(url)) return false
    await openWebLink(url)
    return true
  })
}
