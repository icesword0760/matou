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
  hideWindow: (windowId) => ipcRenderer.invoke(DESKTOP_CHANNELS.hideWindow, windowId),
  showWindow: (windowId) => ipcRenderer.invoke(DESKTOP_CHANNELS.showWindow, windowId)
}
contextBridge.exposeInMainWorld('matouDesktop', desktopApi)
