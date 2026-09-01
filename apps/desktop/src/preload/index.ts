import { contextBridge, ipcRenderer, webUtils } from 'electron'

import type { MatouDesktopApi, RuntimeConnectionState } from '../shared/desktop-api'
import { DESKTOP_CHANNELS } from '../shared/desktop-api'

const PORT_CHANNEL = 'matou:terminal-port'
const RENDERER_READY = 'matou:renderer-ready'
let pendingPort: MessagePort | undefined
let rendererReady = false
let runtimeConnectionState: RuntimeConnectionState = 'ready'
const runtimeConnectionListeners = new Set<(state: RuntimeConnectionState) => void>()

function deliverPort(): void {
  if (!rendererReady || !pendingPort) {
    return
  }
  const port = pendingPort
  pendingPort = undefined
  window.postMessage({ type: PORT_CHANNEL }, '*', [port])
}

ipcRenderer.on(PORT_CHANNEL, (event) => {
  const [port] = event.ports
  if (!port) {
    return
  }
  pendingPort?.close()
  pendingPort = port
  deliverPort()
})
ipcRenderer.on(DESKTOP_CHANNELS.runtimeConnectionState, (_event, state: RuntimeConnectionState) => {
  runtimeConnectionState = state
  for (const listener of runtimeConnectionListeners) listener(state)
})

window.addEventListener('message', (event) => {
  if (event.source !== window || event.data?.type !== RENDERER_READY) {
    return
  }
  rendererReady = true
  deliverPort()
})

const desktopApi: MatouDesktopApi = {
  getPathForFile: (file) => webUtils.getPathForFile(file),
  selectWorkspaceDirectory: () => ipcRenderer.invoke(DESKTOP_CHANNELS.selectWorkspaceDirectory),
  selectSessionEnvironmentDirectory: () =>
    ipcRenderer.invoke(DESKTOP_CHANNELS.selectSessionEnvironmentDirectory),
  revealDirectory: (path) => ipcRenderer.invoke(DESKTOP_CHANNELS.revealDirectory, path),
  openDirectoryInTerminal: (path) =>
    ipcRenderer.invoke(DESKTOP_CHANNELS.openDirectoryInTerminal, path),
  hideWindow: (windowId) => ipcRenderer.invoke(DESKTOP_CHANNELS.hideWindow, windowId),
  showWindow: (windowId) => ipcRenderer.invoke(DESKTOP_CHANNELS.showWindow, windowId),
  createDetachedTerminalWindow: (input) => ipcRenderer.invoke(DESKTOP_CHANNELS.createDetachedTerminalWindow, input),
  closeDetachedTerminalWindow: (windowId) => ipcRenderer.invoke(DESKTOP_CHANNELS.closeDetachedTerminalWindow, windowId),
  detachedTerminalWindowExists: (windowId) =>
    ipcRenderer.invoke(DESKTOP_CHANNELS.detachedTerminalWindowExists, windowId),
  onDetachedWindowClosed: (listener) => {
    const handler = (_event: Electron.IpcRendererEvent, value: Parameters<typeof listener>[0]) => listener(value)
    ipcRenderer.on(DESKTOP_CHANNELS.detachedWindowClosed, handler)
    return () => ipcRenderer.removeListener(DESKTOP_CHANNELS.detachedWindowClosed, handler)
  },
  openDagWindow: (input) => ipcRenderer.invoke(DESKTOP_CHANNELS.openDagWindow, input),
  selectDagNode: (input) => ipcRenderer.invoke(DESKTOP_CHANNELS.selectDagNode, input),
  closeDagWindow: (mainWindowId) => ipcRenderer.invoke(DESKTOP_CHANNELS.closeDagWindow, mainWindowId),
  updateDagNotifications: (mainWindowId, sessionIds) =>
    ipcRenderer.invoke(DESKTOP_CHANNELS.updateDagNotifications, mainWindowId, sessionIds),
  onDagContext: (listener) => {
    const handler = (_event: Electron.IpcRendererEvent, value: Parameters<typeof listener>[0]) => listener(value)
    ipcRenderer.on(DESKTOP_CHANNELS.dagContext, handler)
    return () => ipcRenderer.removeListener(DESKTOP_CHANNELS.dagContext, handler)
  },
  onDagNotifications: (listener) => {
    const handler = (_event: Electron.IpcRendererEvent, value: string[]) => listener(value)
    ipcRenderer.on(DESKTOP_CHANNELS.dagNotifications, handler)
    return () => ipcRenderer.removeListener(DESKTOP_CHANNELS.dagNotifications, handler)
  },
  onDagNodeSelected: (listener) => {
    const handler = (_event: Electron.IpcRendererEvent, value: Parameters<typeof listener>[0]) => listener(value)
    ipcRenderer.on(DESKTOP_CHANNELS.dagNodeSelected, handler)
    return () => ipcRenderer.removeListener(DESKTOP_CHANNELS.dagNodeSelected, handler)
  },
  onDagShortcut: (listener) => {
    const handler = (_event: Electron.IpcRendererEvent, value: Parameters<typeof listener>[0]) => listener(value)
    ipcRenderer.on(DESKTOP_CHANNELS.dagShortcut, handler)
    return () => ipcRenderer.removeListener(DESKTOP_CHANNELS.dagShortcut, handler)
  },
  onRuntimeConnectionState: (listener) => {
    runtimeConnectionListeners.add(listener)
    listener(runtimeConnectionState)
    return () => runtimeConnectionListeners.delete(listener)
  },
  getRuntimeLifecycle: () => ipcRenderer.invoke(DESKTOP_CHANNELS.getRuntimeLifecycle),
  onRuntimeLifecycle: (listener) => {
    const handler = (_event: Electron.IpcRendererEvent, value: Parameters<typeof listener>[0]) => listener(value)
    ipcRenderer.on(DESKTOP_CHANNELS.runtimeLifecycle, handler)
    return () => ipcRenderer.removeListener(DESKTOP_CHANNELS.runtimeLifecycle, handler)
  },
  restoreDatabaseBackup: (backupId, expectedRecoveryId) =>
    ipcRenderer.invoke(
      DESKTOP_CHANNELS.restoreDatabaseBackup,
      backupId,
      expectedRecoveryId
    ),
  exportDatabaseRecoveryBundle: () =>
    ipcRenderer.invoke(DESKTOP_CHANNELS.exportDatabaseRecoveryBundle),
  retryDatabaseOpen: (expectedRecoveryId) =>
    ipcRenderer.invoke(DESKTOP_CHANNELS.retryDatabaseOpen, expectedRecoveryId),
  startWithEmptyDatabase: (expectedRecoveryId) =>
    ipcRenderer.invoke(DESKTOP_CHANNELS.startWithEmptyDatabase, expectedRecoveryId)
}
contextBridge.exposeInMainWorld('matouDesktop', desktopApi)
