import { ipcRenderer } from 'electron'

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
