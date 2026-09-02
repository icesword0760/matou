import { mkdirSync } from 'node:fs'
import { spawnSync } from 'node:child_process'
import { dirname, join, resolve } from 'node:path'

import { app, BrowserWindow, dialog, ipcMain, Menu, nativeImage, net, screen, shell, Tray } from 'electron'
import electronUpdater from 'electron-updater'

import { AppUpdateManager } from './app-update-manager'
import { resolveAppUpdateInstallMode } from './app-update-install-mode'
import { RuntimeHost } from './runtime-host'
import { resolvePackagedApplication } from './app-environment'
import { applyApplicationBrand } from './application-brand'
import { resolveDefaultWorkspacePath } from './default-workspace-policy'
import { claimSingleInstance } from './single-instance-policy'
import { WindowManager } from './window-manager'
import { DagWindowManager, type DagWindowAdapter, type Rectangle } from './dag-window-manager'
import { installDevelopmentDockIcon } from './app-icon'
import { downloadManualUpdate } from './manual-update-downloader'
import { readUpdateBaseUrl } from './update-feed'
import {
  DESKTOP_CHANNELS,
  type DagNodeSelection,
  type DagWindowContext,
  type DetachedTerminalWindowInput,
  type DetachedWindowClosedEvent
} from '../shared/desktop-api'
import { APP_DISPLAY_NAME, APP_STORAGE_DIRECTORY_NAME } from '../shared/brand'

let runtimeHost: RuntimeHost | undefined
const windows = new WindowManager()
const browserWindows = new Map<string, BrowserWindow>()
const dagBrowserWindows = new Map<string, BrowserWindow>()
let tray: Tray | undefined
let quitting = false
let runtimeShutdownComplete = false
let runtimeShutdownPromise: Promise<void> | undefined
let mainWindowSequence = 0
let updateManager: AppUpdateManager | undefined

const { autoUpdater } = electronUpdater
const isPackagedApplication = resolvePackagedApplication({
  electronPackaged: app.isPackaged,
  developmentBundle: process.env.MATOU_DEV_BUNDLE
})

applyApplicationBrand(app, APP_DISPLAY_NAME)

if (process.env.ELECTRON_USER_DATA_DIR) {
  mkdirSync(process.env.ELECTRON_USER_DATA_DIR, { recursive: true })
  app.setPath('userData', process.env.ELECTRON_USER_DATA_DIR)
} else {
  app.setPath('userData', join(app.getPath('appData'), APP_STORAGE_DIRECTORY_NAME))
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
}, windows, isPackagedApplication)

async function createWindow(): Promise<BrowserWindow> {
  const windowId = `main-window-${++mainWindowSequence}`
  const { rootDirectory: defaultRootDirectory, name: defaultName } =
    resolveDefaultWorkspacePath(process.env.MATOU_DEFAULT_WORKSPACE, app.getPath('home'))
  const width = 1200
  const height = 780
  const window = new BrowserWindow({
    width,
    height,
    ...automatedWindowPlacement(width, height),
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
      if (process.env.MATOU_E2E_TERMINAL_DIAGNOSTICS === '0') {
        rendererUrl.searchParams.set('terminalDiagnostics', '0')
      }
    }
    await window.loadURL(rendererUrl.toString())
  } else {
    await window.loadFile(join(__dirname, '../renderer/index.html'), {
      query: {
        windowId,
        defaultRootDirectory,
        defaultName,
        ...(process.env.MATOU_E2E === '1' ? {
          e2e: '1',
          ...(process.env.MATOU_E2E_TERMINAL_DIAGNOSTICS === '0'
            ? { terminalDiagnostics: '0' }
            : {})
        } : {})
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
  const width = 760
  const height = 520
  const window = new BrowserWindow({
    width, height, ...automatedWindowPlacement(width, height), minWidth: 420, minHeight: 260,
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
  let closeNotified = false
  const notifyOwner = () => {
    if (quitting || closeNotified) return
    closeNotified = true
    const event: DetachedWindowClosedEvent = {
      windowId: input.windowId, mainWindowId: input.mainWindowId,
      sceneId: input.sceneId, mountId: input.mountId, sessionId: input.sessionId
    }
    browserWindows.get(input.mainWindowId)?.webContents.send(
      DESKTOP_CHANNELS.detachedWindowClosed, event
    )
  }
  // Notify while the native window is entering its close transaction. Waiting
  // for `closed` makes the owning Renderer race BrowserWindow teardown and can
  // leave a stale detached placeholder until the next app restart.
  window.on('close', notifyOwner)
  window.on('closed', () => {
    windows.unregister(input.windowId)
    browserWindows.delete(input.windowId)
    notifyOwner()
  })
  const query = {
    kind: 'detached-terminal', windowId: input.windowId,
    mainWindowId: input.mainWindowId, sceneId: input.sceneId,
    mountId: input.mountId, sessionId: input.sessionId,
    executionContextId: input.executionContextId,
    profile: input.profile, title: input.title,
    ...(process.env.MATOU_E2E === '1' ? {
      e2e: '1',
      ...(process.env.MATOU_E2E_TERMINAL_DIAGNOSTICS === '0'
        ? { terminalDiagnostics: '0' }
        : {})
    } : {})
  }
  if (process.env.ELECTRON_RENDERER_URL) {
    const rendererUrl = new URL(process.env.ELECTRON_RENDERER_URL)
    for (const [key, value] of Object.entries(query)) rendererUrl.searchParams.set(key, value)
    await window.loadURL(rendererUrl.toString())
  } else {
    await window.loadFile(join(__dirname, '../renderer/index.html'), { query })
  }
}

function automatedWindowPlacement(width: number, height: number): { x?: number; y?: number } {
  const placement = secondaryDisplayWindowBounds({
    enabled: process.env.MATOU_E2E === '1' && process.env.MATOU_E2E_DISPLAY !== 'primary',
    width,
    height,
    primaryDisplayId: screen.getPrimaryDisplay().id,
    displays: screen.getAllDisplays()
  })
  return placement ?? {}
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
    title: `${APP_DISPLAY_NAME} · 会话 DAG`,
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
    sceneId: context.sceneId, sessionId: context.sessionId, theme: context.theme,
    requestedAt: String(context.requestedAt ?? Date.now())
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
  if (isPackagedApplication) {
    return join(process.resourcesPath, 'runtime', 'index.cjs')
  }
  return resolve(app.getAppPath(), '../runtime/dist/index.cjs')
}

if (primaryInstance) app.whenReady().then(async () => {
  installDevelopmentDockIcon({
    platform: process.platform,
    isPackaged: isPackagedApplication,
    appPath: app.getAppPath(),
    dock: app.dock
  })
  runtimeHost = new RuntimeHost(resolveRuntimeEntry())
  await runtimeHost.start()
  await createWindow()
  autoUpdater.channel = process.env.MATOU_UPDATE_CHANNEL ?? 'stable'
  const updateBaseUrl = process.env.MATOU_UPDATE_BASE_URL
    ?? readUpdateBaseUrl(join(process.resourcesPath, 'app-update.yml'))
  if (process.env.MATOU_UPDATE_BASE_URL) {
    autoUpdater.setFeedURL({
      provider: 'generic', url: process.env.MATOU_UPDATE_BASE_URL,
      channel: process.env.MATOU_UPDATE_CHANNEL ?? 'stable'
    })
  }
  updateManager = new AppUpdateManager({
    updater: autoUpdater,
    enabled: app.isPackaged && process.env.MATOU_DISABLE_AUTO_UPDATE !== '1',
    currentVersion: app.getVersion(),
    installMode: resolveAppUpdateInstallMode({
      platform: process.platform,
      isPackaged: app.isPackaged,
      inspectSignature: () => {
        const bundlePath = resolve(dirname(app.getPath('exe')), '../..')
        const result = spawnSync('/usr/bin/codesign', ['-dv', '--verbose=4', bundlePath], {
          encoding: 'utf8'
        })
        return {
          status: result.status,
          output: `${result.stdout ?? ''}\n${result.stderr ?? ''}`
        }
      }
    }),
    ...(updateBaseUrl ? { updateBaseUrl } : {}),
    downloadManualInstaller: ({ url, expectedSha512, onProgress }) => downloadManualUpdate({
      url,
      destinationDirectory: join(app.getPath('userData'), 'pending-updates'),
      ...(expectedSha512 ? { expectedSha512 } : {}),
      fetcher: (input) => net.fetch(input),
      onProgress
    }),
    openManualInstaller: async (path) => {
      const error = await shell.openPath(path)
      if (error) throw new Error(error)
    },
    publish: (state) => {
      for (const window of browserWindows.values()) {
        if (!window.isDestroyed()) window.webContents.send(DESKTOP_CHANNELS.appUpdateState, state)
      }
    },
    prepareInstall: async () => {
      quitting = true
      await shutdownRuntime()
    }
  })
  updateManager.start()
  tray = new Tray(nativeImage.createEmpty())
  tray.setToolTip(APP_DISPLAY_NAME)
  tray.setContextMenu(Menu.buildFromTemplate([
    {
      label: `显示${APP_DISPLAY_NAME}`, click: () => {
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
ipcMain.handle(DESKTOP_CHANNELS.selectSessionEnvironmentDirectory, async () => {
  const result = await dialog.showOpenDialog({ properties: ['openDirectory'] })
  return result.canceled ? null : result.filePaths[0] ?? null
})
ipcMain.handle(DESKTOP_CHANNELS.revealDirectory, async (_event, path: string) => {
  await shell.openPath(path)
})
ipcMain.handle(DESKTOP_CHANNELS.openDirectoryInTerminal, async (_event, path: string) => {
  if (process.platform === 'darwin') {
    await execFileAsync('/usr/bin/open', ['-a', 'Terminal', path])
    return
  }
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
ipcMain.handle(DESKTOP_CHANNELS.detachedTerminalWindowExists, (_event, windowId: string) => {
  const window = browserWindows.get(windowId)
  return Boolean(window && !window.isDestroyed())
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
ipcMain.handle(DESKTOP_CHANNELS.getAppUpdateState, () => updateManager?.state() ?? ({
  status: 'idle', currentVersion: app.getVersion()
}))
ipcMain.handle(DESKTOP_CHANNELS.checkForAppUpdates, () => updateManager?.check())
ipcMain.handle(DESKTOP_CHANNELS.downloadAppUpdate, () => updateManager?.download())
ipcMain.handle(DESKTOP_CHANNELS.installAppUpdate, () => updateManager?.install())

app.on('before-quit', () => {
  quitting = true
  updateManager?.dispose()
  tray?.destroy()
})

app.on('will-quit', (event) => {
  if (runtimeShutdownComplete || !runtimeHost) return
  event.preventDefault()
  void shutdownRuntime().then(() => app.quit())
})

app.on('window-all-closed', () => {
  if (process.platform !== 'darwin' || process.env.MATOU_E2E === '1') {
    app.quit()
  }
})

async function shutdownRuntime(): Promise<void> {
  if (runtimeShutdownComplete || !runtimeHost) return
  runtimeShutdownPromise ??= runtimeHost.stop().then(() => {
    runtimeShutdownComplete = true
  })
  await runtimeShutdownPromise
}
