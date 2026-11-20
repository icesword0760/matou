// Read-only renderer observations for the isolated frozen modal-focus window.
import { execFileSync } from 'node:child_process'
import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'

const recordPath = resolve(process.argv[2] ?? '')
const evidence = '/artifacts/native-parity/native/evidence/modal-focus-real-01/'
const accepted = new Map([
  [`${evidence}reference/launch.json`, 49381],
  [`${evidence}recovery/reference/launch.json`, 49382],
  [`${evidence}fork/reference/launch.json`, 49383],
])
const port = [...accepted].find(([suffix]) => recordPath.endsWith(suffix))?.[1]
if (!port) throw new Error('Expected the frozen launch record for this gate')

const record = JSON.parse(readFileSync(recordPath, 'utf8'))
const processPath = execFileSync('/bin/ps', ['-p', String(record.pid), '-o', 'comm='], { encoding: 'utf8' }).trim()
const listeners = execFileSync('/usr/sbin/lsof', ['-nP', '-a', '-p', String(record.pid), `-iTCP:${port}`, '-sTCP:LISTEN'], { encoding: 'utf8' })
if (record.side !== 'reference' || record.title !== '码头' || processPath !== record.executable ||
    dirname(record.app) !== dirname(recordPath) || !listeners.includes(`127.0.0.1:${port} (LISTEN)`)) {
  throw new Error('Frozen process and its loopback listener did not match')
}

const pages = (await (await fetch(`http://127.0.0.1:${port}/json/list`, { signal: AbortSignal.timeout(2000) })).json())
  .filter(target => target.type === 'page' && target.url.startsWith('file:') && target.title === record.title)
if (pages.length !== 1) throw new Error(`Expected a single frozen renderer, found ${pages.length}`)
const endpoint = new URL(pages[0].webSocketDebuggerUrl)
if (endpoint.protocol !== 'ws:' || endpoint.hostname !== '127.0.0.1' || Number(endpoint.port) !== port) {
  throw new Error('Unexpected renderer debugger endpoint')
}

const pointer = process.argv.length === 5 ? process.argv.slice(3).map(Number) : null
if (pointer && !pointer.every(Number.isFinite)) throw new Error('Pointer must be in viewport CSS pixels')

function observe(pointer) {
  const rect = element => {
    const { x, y, width, height } = element.getBoundingClientRect()
    return { x, y, width, height }
  }
  const brief = element => {
    if (!element) return null
    const name = element.getAttribute('aria-label') || element.title || null
    return {
      tag: element.tagName, role: element.getAttribute('role'), id: element.id || null,
      class: element.getAttribute('class'), name, tabIndex: element.tabIndex,
      disabled: element.disabled ?? null, readOnly: element.readOnly ?? null, frame: rect(element),
      label: element.matches('button, [role=menuitem], [role=tab]') ? element.textContent.trim().slice(0, 100) : null,
      selection: typeof element.selectionStart === 'number' ? [element.selectionStart, element.selectionEnd] : null,
      value: element.matches('input:not([type=password]), textarea') ? element.value.slice(0, 180) : null,
    }
  }
  const selectors = '[role=dialog], [role=alertdialog], [role=menu], .dialog-overlay, .database-recovery-dialog-backdrop, .instruction-file-overlay, .detach-context-overlay'
  return {
    documentFocus: document.hasFocus(),
    viewport: { width: innerWidth, height: innerHeight, scale: devicePixelRatio, screenX, screenY, outerWidth, outerHeight },
    focus: brief(document.activeElement),
    visibleHeaderControls: [...document.querySelectorAll('.terminal-pane-header button, .tab-bar button')]
      .filter(element => element.getClientRects().length > 0).map(brief),
    pointer,
    hit: pointer ? document.elementsFromPoint(...pointer).slice(0, 8).map(brief) : null,
    overlays: [...document.querySelectorAll(selectors)].filter(element => rect(element).height > 0).map(element => ({
      ...brief(element),
      pointerEvents: getComputedStyle(element).pointerEvents,
      background: getComputedStyle(element).backgroundColor,
      controls: [...element.querySelectorAll('button, input, textarea, [role=menuitem]')].map(brief),
      alerts: [...element.querySelectorAll('[role=alert]')].map(alert => alert.textContent.trim()),
    })),
  }
}

const socket = new WebSocket(endpoint)
const result = await new Promise((resolveResult, reject) => {
  const timer = setTimeout(() => reject(new Error('Renderer observation timed out')), 3000)
  socket.addEventListener('open', () => socket.send(JSON.stringify({
    id: 1,
    method: 'Runtime.evaluate',
    params: { expression: `(${observe})(${JSON.stringify(pointer)})`, returnByValue: true, throwOnSideEffect: false },
  })))
  socket.addEventListener('message', event => {
    const message = JSON.parse(event.data)
    if (message.id !== 1) return
    clearTimeout(timer)
    if (message.error || message.result?.exceptionDetails) reject(new Error(JSON.stringify(message)))
    else resolveResult(message.result.result.value)
  })
  socket.addEventListener('error', () => {
    clearTimeout(timer)
    reject(new Error('Could not connect to frozen renderer'))
  })
}).finally(() => socket.close())

console.log(JSON.stringify({ pid: record.pid, observed_at: new Date().toISOString(), ...result }))
