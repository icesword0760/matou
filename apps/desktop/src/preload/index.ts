import { contextBridge, ipcRenderer } from 'electron'

import type { MatouDesktopApi } from '../shared/desktop-api'
import { DESKTOP_CHANNELS } from '../shared/desktop-api'

const PORT_CHANNEL = 'matou:terminal-port'
const RENDERER_READY = 'matou:renderer-ready'
let pendingPort: MessagePort | undefined
let rendererReady = false

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

window.addEventListener('message', (event) => {
  if (event.source !== window || event.data?.type !== RENDERER_READY) {
    return
  }
  rendererReady = true
  deliverPort()
})

const desktopApi: MatouDesktopApi = {
  selectWorkspaceDirectory: () => ipcRenderer.invoke(DESKTOP_CHANNELS.selectWorkspaceDirectory),
  revealDirectory: (path) => ipcRenderer.invoke(DESKTOP_CHANNELS.revealDirectory, path),
  hideWindow: (windowId) => ipcRenderer.invoke(DESKTOP_CHANNELS.hideWindow, windowId),
  showWindow: (windowId) => ipcRenderer.invoke(DESKTOP_CHANNELS.showWindow, windowId),
  createDetachedTerminalWindow: (input) => ipcRenderer.invoke(DESKTOP_CHANNELS.createDetachedTerminalWindow, input),
  closeDetachedTerminalWindow: (windowId) => ipcRenderer.invoke(DESKTOP_CHANNELS.closeDetachedTerminalWindow, windowId),
  onDetachedWindowClosed: (listener) => {
    const handler = (_event: Electron.IpcRendererEvent, value: Parameters<typeof listener>[0]) => listener(value)
    ipcRenderer.on(DESKTOP_CHANNELS.detachedWindowClosed, handler)
    return () => ipcRenderer.removeListener(DESKTOP_CHANNELS.detachedWindowClosed, handler)
  }
}
contextBridge.exposeInMainWorld('matouDesktop', desktopApi)
