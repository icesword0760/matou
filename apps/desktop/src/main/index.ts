import { join, resolve } from 'node:path'

import { app, BrowserWindow } from 'electron'

import { RuntimeHost } from './runtime-host'

let runtimeHost: RuntimeHost | undefined

async function createWindow(): Promise<BrowserWindow> {
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

  window.once('ready-to-show', () => window.show())
  window.webContents.on('did-finish-load', () => runtimeHost?.connect(window.webContents))

  if (process.env.ELECTRON_RENDERER_URL) {
    const rendererUrl = new URL(process.env.ELECTRON_RENDERER_URL)
    if (process.env.MATOU_E2E === '1') {
      rendererUrl.searchParams.set('e2e', '1')
    }
    await window.loadURL(rendererUrl.toString())
  } else {
    await window.loadFile(join(__dirname, '../renderer/index.html'), {
      query: process.env.MATOU_E2E === '1' ? { e2e: '1' } : {}
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

  app.on('activate', async () => {
    if (BrowserWindow.getAllWindows().length === 0) {
      await createWindow()
    }
  })
})

app.on('before-quit', () => runtimeHost?.stop())

app.on('window-all-closed', () => {
  if (process.platform !== 'darwin' || process.env.MATOU_E2E === '1') {
    app.quit()
  }
})
