import { randomUUID } from 'node:crypto'
import { mkdir } from 'node:fs/promises'
import { join, resolve } from 'node:path'

import { app, BrowserWindow, dialog, ipcMain, Menu, nativeImage, Tray } from 'electron'

import { RuntimeHost } from './runtime-host'
import { WindowManager } from './window-manager'
import {
  DESKTOP_CHANNELS,
  type DetachedTerminalWindowInput,
  type DetachedWindowClosedEvent
} from '../shared/desktop-api'

let runtimeHost: RuntimeHost | undefined
const windows = new WindowManager()
const browserWindows = new Map<string, BrowserWindow>()
let tray: Tray | undefined
let quitting = false

async function createWindow(): Promise<BrowserWindow> {
  const windowId = randomUUID()
  const defaultRootDirectory = process.env.MATOU_DEFAULT_WORKSPACE ?? join(app.getPath('home'), 'matou_workspace')
  await mkdir(defaultRootDirectory, { recursive: true })
  const window = new BrowserWindow({
    width: 1200,
    height: 780,
    minWidth: 720,
    minHeight: 480,
    show: false,
    backgroundColor: '#0b0e14',
    webPreferences: {
      preload: join(__dirname, '../preload/index.cjs'),
      nodeIntegration: false,
      contextIsolation: true,
      sandbox: true
    }
  })
  windows.register(windowId, window)
  browserWindows.set(windowId, window)
  window.on('closed', () => { windows.unregister(windowId); browserWindows.delete(windowId) })
  window.on('close', (event) => {
    if (!quitting && process.env.MATOU_E2E !== '1') {
      event.preventDefault()
      window.hide()
    }
  })

  window.once('ready-to-show', () => window.show())
  window.webContents.on('did-finish-load', () => runtimeHost?.connect(window.webContents))
  window.webContents.setWindowOpenHandler(() => {
    void createWindow()
    return { action: 'deny' }
  })

  if (process.env.ELECTRON_RENDERER_URL) {
    const rendererUrl = new URL(process.env.ELECTRON_RENDERER_URL)
    rendererUrl.searchParams.set('windowId', windowId)
    rendererUrl.searchParams.set('defaultRootDirectory', defaultRootDirectory)
    if (process.env.MATOU_E2E === '1') {
      rendererUrl.searchParams.set('e2e', '1')
    }
    await window.loadURL(rendererUrl.toString())
  } else {
    await window.loadFile(join(__dirname, '../renderer/index.html'), {
      query: {
        windowId,
        defaultRootDirectory,
        ...(process.env.MATOU_E2E === '1' ? { e2e: '1' } : {})
      }
    })
  }

  return window
}

async function createDetachedTerminalWindow(input: DetachedTerminalWindowInput): Promise<void> {
  if (browserWindows.has(input.windowId)) {
    windows.showWindow(input.windowId)
    return
  }
  const window = new BrowserWindow({
    width: 760, height: 520, minWidth: 420, minHeight: 260,
    show: false, backgroundColor: '#0b0e14',
    title: input.title,
    webPreferences: {
      preload: join(__dirname, '../preload/index.cjs'),
      nodeIntegration: false, contextIsolation: true, sandbox: true
    }
  })
  windows.register(input.windowId, window)
  browserWindows.set(input.windowId, window)
  window.once('ready-to-show', () => window.show())
  window.webContents.on('did-finish-load', () => runtimeHost?.connect(window.webContents))
  window.on('closed', () => {
    windows.unregister(input.windowId)
    browserWindows.delete(input.windowId)
    if (quitting) return
    const event: DetachedWindowClosedEvent = {
      windowId: input.windowId, mainWindowId: input.mainWindowId,
      sceneId: input.sceneId, mountId: input.mountId, sessionId: input.sessionId
    }
    browserWindows.get(input.mainWindowId)?.webContents.send(
      DESKTOP_CHANNELS.detachedWindowClosed, event
    )
  })
  const query = {
    kind: 'detached-terminal', windowId: input.windowId,
    mainWindowId: input.mainWindowId, sceneId: input.sceneId,
    mountId: input.mountId, sessionId: input.sessionId,
    executionContextId: input.executionContextId,
    profile: input.profile, title: input.title
  }
  if (process.env.ELECTRON_RENDERER_URL) {
    const rendererUrl = new URL(process.env.ELECTRON_RENDERER_URL)
    for (const [key, value] of Object.entries(query)) rendererUrl.searchParams.set(key, value)
    await window.loadURL(rendererUrl.toString())
  } else {
    await window.loadFile(join(__dirname, '../renderer/index.html'), { query })
  }
}

function resolveRuntimeEntry(): string {
  if (process.env.MATOU_RUNTIME_ENTRY) {
    return resolve(process.env.MATOU_RUNTIME_ENTRY)
  }
  if (app.isPackaged) {
    return join(process.resourcesPath, 'runtime', 'index.cjs')
  }
  return resolve(app.getAppPath(), '../runtime/dist/index.cjs')
}

app.whenReady().then(async () => {
  runtimeHost = new RuntimeHost(resolveRuntimeEntry())
  await runtimeHost.start()
  await createWindow()
  tray = new Tray(nativeImage.createEmpty())
  tray.setToolTip('Matou')
  tray.setContextMenu(Menu.buildFromTemplate([
    {
      label: '显示 Matou', click: () => {
        const windowId = windows.firstLiveWindowId()
        if (windowId) windows.showWindow(windowId)
        else void createWindow()
      }
    },
    { type: 'separator' },
    { label: '退出', click: () => { quitting = true; app.quit() } }
  ]))

  app.on('activate', async () => {
    if (BrowserWindow.getAllWindows().length === 0) {
      await createWindow()
    }
  })
})

ipcMain.handle(DESKTOP_CHANNELS.selectWorkspaceDirectory, async () => {
  const result = await dialog.showOpenDialog({
    properties: ['openDirectory', 'createDirectory']
  })
  return result.canceled ? null : result.filePaths[0] ?? null
})
ipcMain.handle(DESKTOP_CHANNELS.hideWindow, (_event, windowId: string) => {
  windows.hideWindow(windowId)
})
ipcMain.handle(DESKTOP_CHANNELS.showWindow, (_event, windowId: string) => {
  windows.showWindow(windowId)
})
ipcMain.handle(DESKTOP_CHANNELS.createDetachedTerminalWindow, (
  _event, input: DetachedTerminalWindowInput
) => createDetachedTerminalWindow(input))
ipcMain.handle(DESKTOP_CHANNELS.closeDetachedTerminalWindow, (_event, windowId: string) => {
  browserWindows.get(windowId)?.close()
})

app.on('before-quit', () => {
  quitting = true
  tray?.destroy()
  runtimeHost?.stop()
})

app.on('window-all-closed', () => {
  if (process.platform !== 'darwin' || process.env.MATOU_E2E === '1') {
    app.quit()
  }
})
