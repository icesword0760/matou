import { randomUUID } from 'node:crypto'
import { join, resolve } from 'node:path'

import { app, BrowserWindow, dialog, ipcMain, Menu, nativeImage, Tray } from 'electron'

import { RuntimeHost } from './runtime-host'
import { WindowManager } from './window-manager'
import { DESKTOP_CHANNELS } from '../shared/desktop-api'

let runtimeHost: RuntimeHost | undefined
const windows = new WindowManager()
let tray: Tray | undefined
let quitting = false

async function createWindow(): Promise<BrowserWindow> {
  const windowId = randomUUID()
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
  window.on('closed', () => windows.unregister(windowId))
  window.on('close', (event) => {
    if (!quitting && process.env.MATOU_E2E !== '1') {
      event.preventDefault()
      window.hide()
    }
  })

  window.once('ready-to-show', () => window.show())
  window.webContents.on('did-finish-load', () => runtimeHost?.connect(window.webContents))

  if (process.env.ELECTRON_RENDERER_URL) {
    const rendererUrl = new URL(process.env.ELECTRON_RENDERER_URL)
    rendererUrl.searchParams.set('windowId', windowId)
    if (process.env.MATOU_E2E === '1') {
      rendererUrl.searchParams.set('e2e', '1')
    }
    await window.loadURL(rendererUrl.toString())
  } else {
    await window.loadFile(join(__dirname, '../renderer/index.html'), {
      query: {
        windowId,
        ...(process.env.MATOU_E2E === '1' ? { e2e: '1' } : {})
      }
    })
  }

  return window
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
