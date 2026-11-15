// Observe a just-launched isolated reference. No input or page mutation.
import { readFile, writeFile, mkdir } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import path from 'node:path'
const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..')
const launch = JSON.parse(await readFile(path.join(root, 'artifacts/native-parity/reference/launch.json'), 'utf8'))
if (!launch.debug_port || !launch.startup_trace) throw new Error('Needs a new --debug-port --startup-trace launch')
const started = launch.started_unix_ns / 1e6
if (Date.now() - started > 4000) throw new Error('Launch is too old for a startup capture')
const out = launch.startup_trace.replace(/\.json$/, '-frames')
await mkdir(out, { recursive: true })
let target
while (Date.now() - started < 8000 && !target) {
  try {
    const pages = await (await fetch(`http://127.0.0.1:${launch.debug_port}/json/list`)).json()
    target = pages.find(p => p.type === 'page' && p.url.includes('MatouParityReference.app/') && p.url.includes('windowId=main-window-1'))
  } catch { /* The debugger socket is not ready until the app starts. */ }
  if (!target) await new Promise(resolve => setTimeout(resolve, 10))
}
if (!target) throw new Error('Isolated main window did not become available within 8 seconds')
const socket = new WebSocket(target.webSocketDebuggerUrl)
await new Promise((resolve, reject) => {
  socket.addEventListener('open', resolve, { once: true })
  socket.addEventListener('error', reject, { once: true })
})
let id = 0
const frames = [], writes = [], pending = new Map()
function send(method, params = {}) {
  return new Promise((resolve, reject) => {
    const key = ++id; pending.set(key, { resolve, reject })
    socket.send(JSON.stringify({ id: key, method, params }))
  })
}
socket.addEventListener('message', ({ data }) => {
  const msg = JSON.parse(data)
  if (msg.id) {
    const task = pending.get(msg.id)
    if (task) { pending.delete(msg.id); msg.error ? task.reject(new Error(JSON.stringify(msg.error))) : task.resolve(msg.result) }
  } else if (msg.method === 'Page.screencastFrame') {
    const i = frames.length, p = msg.params
    frames.push({ i, metadata: p.metadata, received_at_ms: Date.now(), since_spawn_ms: p.metadata.timestamp * 1000 - started })
    writes.push(writeFile(path.join(out, `${String(i).padStart(4, '0')}.jpg`), Buffer.from(p.data, 'base64')))
    writes.push(send('Page.screencastFrameAck', { sessionId: p.sessionId }))
  }
})
try {
  await send('Page.enable')
  const captureStartedAt = Date.now()
  await send('Page.startScreencast', { format: 'jpeg', quality: 85, maxWidth: 1200, maxHeight: 780, everyNthFrame: 1 })
  await new Promise(resolve => setTimeout(resolve, Math.max(0, 8000 - (Date.now() - started))))
  await send('Page.stopScreencast')
  await Promise.all(writes)
  await writeFile(path.join(out, 'metadata.json'), JSON.stringify({ launch, captureStartedAt, frames,
    interpretation: 'Actual renderer compositor frame capture. Includes observer attachment delay, not physical display latency. First usable frame must be inspected; first screenshot alone is not readiness.' }, null, 2) + '\n')
  console.log(JSON.stringify({ out, frames: frames.length, observerAttachedMs: captureStartedAt - started }))
} finally { socket.close() }
