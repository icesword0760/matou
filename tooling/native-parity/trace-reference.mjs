// Observe the isolated reference renderer. Input is supplied through the real UI.
// Trace timestamps are renderer diagnostics, not physical input-to-photon latency.
import { readFile, writeFile, mkdir } from 'node:fs/promises'
import { fileURLToPath } from 'node:url'
import path from 'node:path'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..')
const run = path.join(root, 'artifacts/native-parity/reference')
const launch = JSON.parse(await readFile(path.join(run, 'launch.json'), 'utf8'))
const seconds = Number(process.argv[2] ?? 45)
if (!Number.isFinite(seconds) || seconds < 1 || seconds > 300) throw new Error('Duration must be 1–300 seconds')
if (!launch.debug_port) throw new Error('Launch a separate diagnostic run with --debug-port first')
const pages = await (await fetch(`http://127.0.0.1:${launch.debug_port}/json/list`)).json()
const page = pages.find(p => p.type === 'page' && p.url.includes('MatouParityReference.app/') && p.url.includes('windowId=main-window-1'))
if (!page) throw new Error('Isolated reference main window not found; no other page will be used')
const out = path.join(run, 'evidence', `trace-${Date.now()}`)
await mkdir(out, { recursive: true })
const socket = new WebSocket(page.webSocketDebuggerUrl)
await new Promise((resolve, reject) => {
  socket.addEventListener('open', resolve, { once: true })
  socket.addEventListener('error', reject, { once: true })
})
let id = 0
const pending = new Map()
let complete
const tracingComplete = new Promise(resolve => { complete = resolve })
socket.addEventListener('message', ({ data }) => {
  const message = JSON.parse(data)
  if (message.id) {
    const request = pending.get(message.id)
    if (!request) return
    pending.delete(message.id)
    message.error ? request.reject(new Error(JSON.stringify(message.error))) : request.resolve(message.result)
  } else if (message.method === 'Tracing.tracingComplete') complete(message.params)
})
socket.addEventListener('close', () => {
  for (const request of pending.values()) request.reject(new Error('Reference debugger disconnected'))
  pending.clear()
})
function send(method, params = {}) {
  return new Promise((resolve, reject) => {
    const key = ++id
    pending.set(key, { resolve, reject })
    socket.send(JSON.stringify({ id: key, method, params }))
  })
}
const startedAt = new Date().toISOString()
try {
  await send('Performance.enable')
  const before = await send('Performance.getMetrics')
  await send('Tracing.start', {
    categories: 'devtools.timeline,disabled-by-default-devtools.timeline,benchmark,cc,input,latencyInfo',
    options: 'record-as-much-as-possible', transferMode: 'ReturnAsStream'
  })
  console.log(JSON.stringify({ recording: true, seconds, out, pid: launch.pid }))
  await new Promise(resolve => setTimeout(resolve, seconds * 1000))
  const after = await send('Performance.getMetrics')
  await send('Tracing.end')
  const { stream } = await tracingComplete
  let trace = ''
  while (true) {
    const chunk = await send('IO.read', { handle: stream })
    trace += chunk.base64Encoded ? Buffer.from(chunk.data, 'base64').toString('utf8') : chunk.data
    if (chunk.eof) break
  }
  await send('IO.close', { handle: stream })
  await writeFile(path.join(out, 'trace.json'), trace)
  await writeFile(path.join(out, 'metadata.json'), JSON.stringify({
    startedAt, endedAt: new Date().toISOString(), seconds, launch, pageUrl: page.url,
    interpretation: 'Real reference renderer trace; no synthesized UI input, no E2E mode. Internal timing, not input-to-photon latency.',
    before, after
  }, null, 2) + '\n')
  console.log(JSON.stringify({ recording: false, out }))
} finally {
  socket.close()
}
