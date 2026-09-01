import { mkdirSync } from 'node:fs'
import { join, resolve } from 'node:path'

import { app, BrowserWindow, dialog, ipcMain, Menu, nativeImage, screen, shell, Tray } from 'electron'

import { RuntimeHost } from './runtime-host'
import { resolveDefaultWorkspacePath } from './default-workspace-policy'
import { claimSingleInstance } from './single-instance-policy'
import { WindowManager } from './window-manager'
import { DagWindowManager, type DagWindowAdapter, type Rectangle } from './dag-window-manager'
import {
  DESKTOP_CHANNELS,
  type DagNodeSelection,
  type DagWindowContext,
  type DetachedTerminalWindowInput,
  type DetachedWindowClosedEvent
} from '../shared/desktop-api'

let runtimeHost: RuntimeHost | undefined
const windows = new WindowManager()
const browserWindows = new Map<string, BrowserWindow>()
const dagBrowserWindows = new Map<string, BrowserWindow>()
let tray: Tray | undefined
let quitting = false
let runtimeShutdownComplete = false
let runtimeShutdownPromise: Promise<void> | undefined
let mainWindowSequence = 0

if (process.env.ELECTRON_USER_DATA_DIR) {
  mkdirSync(process.env.ELECTRON_USER_DATA_DIR, { recursive: true })
  app.setPath('userData', process.env.ELECTRON_USER_DATA_DIR)
}

const dagWindows = new DagWindowManager({
  createWindow: ({ context, bounds }) => createDagBrowserWindow(context, bounds),
  displayBounds: (mainWindowId) => {
    const owner = browserWindows.get(mainWindowId)
    return screen.getDisplayMatching(owner?.getBounds() ?? screen.getPrimaryDisplay().bounds).workArea
  },
  connectRuntime: (window) => {
    const browserWindow = dagBrowserWindows.get(window.id)
    if (browserWindow && !browserWindow.isDestroyed()) runtimeHost?.connect(browserWindow.webContents)
  },
  routeSelection: (mainWindowId, selection) => {
    const mainWindow = browserWindows.get(mainWindowId)
    if (!mainWindow || mainWindow.isDestroyed()) return
    mainWindow.show()
    mainWindow.focus()
    mainWindow.webContents.send(DESKTOP_CHANNELS.dagNodeSelected, selection)
  },
  activateTargetWindow: (windowId) => {
    const target = browserWindows.get(windowId)
    if (!target || target.isDestroyed()) return false
    target.show()
    target.focus()
    return true
  }
})

const primaryInstance = claimSingleInstance({
  requestSingleInstanceLock: () => app.requestSingleInstanceLock(),
  quit: () => app.quit(),
  onSecondInstance: (listener) => { app.on('second-instance', listener) }
}, windows, app.isPackaged)

async function createWindow(): Promise<BrowserWindow> {
  const windowId = `main-window-${++mainWindowSequence}`
  const { rootDirectory: defaultRootDirectory, name: defaultName } =
    resolveDefaultWorkspacePath(process.env.MATOU_DEFAULT_WORKSPACE, app.getPath('home'))
  const window = new BrowserWindow({
    width: 1200,
    height: 780,
    minWidth: 720,
    minHeight: 480,
    show: false,
    ...(process.platform === 'darwin' ? {
      transparent: true,
      backgroundColor: '#00000000',
      vibrancy: 'sidebar' as const,
      visualEffectState: 'active' as const,
      titleBarStyle: 'hiddenInset' as const,
      trafficLightPosition: { x: 14, y: 16 }
    } : { backgroundColor: '#F7F8FA' }),
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
    if (!quitting && (process.env.MATOU_E2E !== '1' || process.env.MATOU_E2E_WINDOW_CLOSE === 'hide')) {
      event.preventDefault()
      window.hide()
    }
  })

  window.once('ready-to-show', () => window.show())
  window.webContents.on('did-finish-load', () => runtimeHost?.connect(window.webContents))
  installNativeDagShortcut(window)
  installNativeScrollGesture(window)
  window.webContents.setWindowOpenHandler(() => {
    void createWindow()
    return { action: 'deny' }
  })

  if (process.env.ELECTRON_RENDERER_URL) {
    const rendererUrl = new URL(process.env.ELECTRON_RENDERER_URL)
    rendererUrl.searchParams.set('windowId', windowId)
    rendererUrl.searchParams.set('defaultRootDirectory', defaultRootDirectory)
    rendererUrl.searchParams.set('defaultName', defaultName)
    if (process.env.MATOU_E2E === '1') {
      rendererUrl.searchParams.set('e2e', '1')
    }
    await window.loadURL(rendererUrl.toString())
  } else {
    await window.loadFile(join(__dirname, '../renderer/index.html'), {
      query: {
        windowId,
        defaultRootDirectory,
        defaultName,
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
    show: false, backgroundColor: '#F7F8FA',
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
  installNativeDagShortcut(window)
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

function createDagBrowserWindow(context: DagWindowContext, bounds: Rectangle): DagWindowAdapter {
  const id = `dag-window:${context.mainWindowId}`
  const window = new BrowserWindow({
    ...bounds,
    minWidth: 680,
    minHeight: 480,
    show: false,
    frame: true,
    resizable: true,
    maximizable: true,
    title: 'Matou 会话 DAG',
    backgroundColor: context.theme === 'light' ? '#F7F8FA' : '#171717',
    ...(process.platform === 'darwin' ? {
      titleBarStyle: 'hidden' as const,
      trafficLightPosition: { x: 14, y: 16 }
    } : {}),
    webPreferences: {
      preload: join(__dirname, '../preload/index.cjs'),
      nodeIntegration: false,
      contextIsolation: true,
      sandbox: true
    }
  })
  dagBrowserWindows.set(id, window)
  const readyListeners: Array<() => void> = []
  const closedListeners: Array<() => void> = []
  window.webContents.once('did-finish-load', () => readyListeners.splice(0).forEach((listener) => listener()))
  window.on('closed', () => {
    dagBrowserWindows.delete(id)
    closedListeners.splice(0).forEach((listener) => listener())
  })
  const query = {
    kind: 'dag', mainWindowId: context.mainWindowId,
    sceneId: context.sceneId, sessionId: context.sessionId, theme: context.theme
  }
  if (process.env.ELECTRON_RENDERER_URL) {
    const rendererUrl = new URL(process.env.ELECTRON_RENDERER_URL)
    for (const [key, value] of Object.entries(query)) rendererUrl.searchParams.set(key, value)
    void window.loadURL(rendererUrl.toString())
  } else {
    void window.loadFile(join(__dirname, '../renderer/index.html'), { query })
  }
  return {
    id,
    isDestroyed: () => window.isDestroyed(),
    show: () => window.show(),
    focus: () => window.focus(),
    close: () => window.close(),
    send: (channel, value) => window.webContents.send(channel, value),
    onReady: (listener) => { readyListeners.push(listener) },
    onClosed: (listener) => { closedListeners.push(listener) }
  }
}

function installNativeDagShortcut(window: BrowserWindow): void {
  window.webContents.on('before-input-event', (event, input) => {
    if (input.key !== 'Tab' || !input.alt || input.control || input.meta || input.shift) return
    event.preventDefault()
    if (input.type !== 'keyDown' || input.isAutoRepeat || window.isDestroyed()) return
    window.webContents.send(DESKTOP_CHANNELS.dagShortcut, 'long')
  })
}

function installNativeScrollGesture(window: BrowserWindow): void {
  window.webContents.on('input-event', (_event, input) => {
    const phase = input.type === 'gestureScrollBegin'
      ? 'begin'
      : input.type === 'gestureScrollEnd' ? 'end' : undefined
    if (!phase || window.isDestroyed()) return
    window.webContents.send(DESKTOP_CHANNELS.scrollGesture, phase)
  })
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

if (primaryInstance) app.whenReady().then(async () => {
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
    const windowId = windows.firstLiveWindowId()
    if (windowId) windows.showWindow(windowId)
    else await createWindow()
  })
})

ipcMain.handle(DESKTOP_CHANNELS.selectWorkspaceDirectory, async () => {
  const result = await dialog.showOpenDialog({
    properties: ['openDirectory', 'createDirectory']
  })
  return result.canceled ? null : result.filePaths[0] ?? null
})
ipcMain.handle(DESKTOP_CHANNELS.revealDirectory, async (_event, path: string) => {
  await shell.openPath(path)
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
ipcMain.handle(DESKTOP_CHANNELS.openDagWindow, (_event, input: DagWindowContext) => {
  dagWindows.open(input)
})
ipcMain.handle(DESKTOP_CHANNELS.selectDagNode, (_event, input: DagNodeSelection) => {
  dagWindows.selectNode(input)
})
ipcMain.handle(DESKTOP_CHANNELS.closeDagWindow, (_event, mainWindowId: string) => {
  dagWindows.close(mainWindowId)
})
ipcMain.handle(DESKTOP_CHANNELS.updateDagNotifications, (
  _event, mainWindowId: string, sessionIds: string[]
) => {
  dagWindows.updateNotifications(mainWindowId, sessionIds)
})

app.on('before-quit', () => {
  quitting = true
  tray?.destroy()
})

app.on('will-quit', (event) => {
  if (runtimeShutdownComplete || !runtimeHost) return
  event.preventDefault()
  runtimeShutdownPromise ??= runtimeHost.stop().then(() => {
    runtimeShutdownComplete = true
    app.quit()
  })
})

app.on('window-all-closed', () => {
  if (process.platform !== 'darwin' || process.env.MATOU_E2E === '1') {
    app.quit()
  }
})
