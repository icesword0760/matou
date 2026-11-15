// Record mouse reports from the frozen parser's actual mouse service.
import { execFileSync } from 'node:child_process'
import { mkdirSync } from 'node:fs'
import { createRequire } from 'node:module'
import { resolve } from 'node:path'

const { build } = createRequire(import.meta.url)('../../node_modules/.pnpm/esbuild@0.27.7/node_modules/esbuild')
const source = resolve('node_modules/.pnpm/@xterm+xterm@6.0.0/node_modules/@xterm/xterm/src')
const output = resolve('artifacts/native-parity/reference/mouse-contract-export.cjs')
mkdirSync(resolve('artifacts/native-parity/reference'), { recursive: true })

const cases = [
  {
    name: 'tracking-disabled',
    protocol: 'NONE', encoding: 'DEFAULT', cols: 80, rows: 24,
    steps: [{}, { button: 3, action: 32 }, { button: 4, action: 1 }],
  },
  {
    name: 'x10-down-only-and-no-modifiers',
    protocol: 'X10', encoding: 'DEFAULT', cols: 80, rows: 24,
    steps: [
      { ctrl: true, alt: true, shift: true },
      { button: 1, ctrl: true }, { button: 2, shift: true },
      { action: 0 }, { button: 4, action: 1 },
      { action: 32 }, { button: 3, action: 32 },
    ],
  },
  {
    name: 'vt200-buttons-wheel-and-release',
    protocol: 'VT200', encoding: 'SGR', cols: 80, rows: 24,
    steps: [
      { button: 2, ctrl: true, alt: true, shift: true },
      { button: 2, action: 0, ctrl: true, alt: true, shift: true },
      { button: 4, action: 0 }, { button: 4, action: 1 },
      { button: 4, action: 2, alt: true }, { button: 4, action: 3, ctrl: true },
      { action: 32 }, { button: 3, action: 32 },
      { encoding: 'DEFAULT', button: 2, action: 0, shift: true },
    ],
  },
  {
    name: 'drag-held-moves-and-rejected-events-do-not-clear-debounce',
    protocol: 'DRAG', encoding: 'SGR', cols: 80, rows: 24,
    steps: [
      {}, { action: 32 }, { button: 3, action: 32 },
      { action: 32 }, { action: 32, col: 4 },
      { action: 32, col: 4, shift: true }, { action: 0, col: 4 },
      { button: 4, action: 1 }, { button: 4, action: 1 },
    ],
  },
  {
    name: 'any-hover-grid-and-pixel-debounce',
    protocol: 'ANY', encoding: 'SGR', cols: 80, rows: 24,
    steps: [
      { button: 3, action: 32 },
      { button: 3, action: 32, x: 48, y: 59 },
      { button: 3, action: 32, row: 4 },
      { button: 3, action: 32, row: 4, alt: true },
      { button: 3, action: 32, row: 4, alt: true, ctrl: true },
      { button: 3, action: 32, row: 4, alt: true, ctrl: true, shift: true },
      { encoding: 'SGR_PIXELS', button: 3, action: 32, row: 5, alt: true, ctrl: true, shift: true },
      { button: 3, action: 32, row: 5, x: 48, alt: true, ctrl: true, shift: true },
      { button: 3, action: 32, row: 6, x: 48, alt: true, ctrl: true, shift: true },
      { button: 2, action: 0, x: 70, y: 89 },
      { button: 4, action: 0, x: 70, y: 89 },
    ],
  },
  {
    name: 'protocol-change-retains-the-last-event',
    protocol: 'ANY', encoding: 'SGR', cols: 80, rows: 24,
    steps: [
      { action: 32 }, { protocol: 'DRAG', action: 32 },
      { protocol: 'NONE', action: 32, row: 7 },
      { protocol: 'ANY', action: 32 }, { action: 32, row: 7 },
    ],
  },
  {
    name: 'default-binary-and-coordinate-ceiling',
    protocol: 'ANY', encoding: 'DEFAULT', cols: 260, rows: 260,
    steps: [
      { col: 222, row: 222 },
      { col: 223, row: 222 },
      { col: 222, row: 223 },
      { button: 8, col: 127, row: 127 },
      { button: 15, action: 32 },
      { action: 32, col: 223 },
      { encoding: 'SGR', action: 32, col: 223 },
      { action: 32, col: 224 },
    ],
  },
  {
    name: 'invalid-grid-and-button-action-pairs',
    protocol: 'ANY', encoding: 'SGR_PIXELS', cols: 80, rows: 24,
    steps: [
      { col: -1 }, { row: -1 }, { col: 80 }, { row: 24 },
      { button: 4, action: 32 }, { button: 3, action: 0 }, { button: 3, action: 1 },
      { button: 0, action: 2 }, { button: 2, action: 3 },
    ],
  },
]

const wheelCases = [
  {
    name: 'signed-pixel-remainder-across-direction-changes',
    cellHeight: 10,
    steps: [{ deltaY: 20 }, { deltaY: -10 }, { deltaY: 24 }, { deltaY: -20 }, { deltaY: -18 }],
  },
  {
    name: 'trackpad-threshold',
    cellHeight: 30,
    steps: [{ deltaY: 49 }, { deltaY: 50 }, { deltaY: -50 }],
  },
  {
    name: 'line-input-and-modifiers',
    cellHeight: 20,
    steps: [
      { deltaY: 25 },
      { deltaY: 1.5, pixel: false },
      { deltaY: -2, pixel: false, alt: true },
      { deltaY: 0.5, pixel: false, ctrl: true },
      { deltaY: 100, shift: true, ctrl: true },
      { deltaY: 0, ctrl: true },
      { deltaY: 10, alt: true, ctrl: true },
    ],
  },
]

const entry = `
import { CoreMouseService } from 'common/services/CoreMouseService'
import { writeFileSync } from 'node:fs'
import { resolve } from 'node:path'

function main(cases, wheelCases) {
  const results = []
  for (const { steps, ...input } of cases) {
    let output
    const service = new CoreMouseService(
      { cols: input.cols, rows: input.rows },
      {
        triggerBinaryEvent: data => { output = { channel: 'binary', bytes: Array.from(data, char => char.charCodeAt(0)) } },
        triggerDataEvent: data => { output = { channel: 'data', bytes: Array.from(Buffer.from(data)) } },
      },
      { rawOptions: {} },
    )
    service.activeProtocol = input.protocol
    service.activeEncoding = input.encoding
    const probes = []
    for (const { protocol, encoding, ...overrides } of steps) {
      if (protocol) service.activeProtocol = protocol
      if (encoding) service.activeEncoding = encoding
      const event = {
        col: 2, row: 3, x: 43, y: 55, button: 0, action: 1,
        ctrl: false, alt: false, shift: false,
        ...overrides,
      }
      output = null
      const accepted = service.triggerMouseEvent({ ...event })
      probes.push({ protocol, encoding, event, accepted, output })
    }
    results.push({ ...input, probes })
    service.dispose()
  }
  globalThis.WheelEvent = { DOM_DELTA_PIXEL: 0, DOM_DELTA_LINE: 1, DOM_DELTA_PAGE: 2 }
  const wheelResults = wheelCases.map(({ steps, ...input }) => {
    const service = new CoreMouseService(
      { cols: 80, rows: 24 }, {}, { rawOptions: { scrollSensitivity: 1, fastScrollSensitivity: 5 } },
    )
    const probes = steps.map(({ deltaY, pixel = true, shift = false, alt = false, ctrl = false }) => {
      const amount = service.consumeWheelEvent({
        deltaY,
        deltaMode: pixel ? WheelEvent.DOM_DELTA_PIXEL : WheelEvent.DOM_DELTA_LINE,
        shiftKey: shift,
        altKey: alt,
        ctrlKey: ctrl,
      }, input.cellHeight, 1)
      return { deltaY, pixel, shift, alt, ctrl, amount }
    })
    service.dispose()
    return { ...input, probes }
  })
  const path = resolve('apps/native/terminal/tests/fixtures/mouse-reference.json')
  writeFileSync(path, JSON.stringify({ source: 'Frozen xterm 6.0.0 CoreMouseService', cases: results, wheelCases: wheelResults }, null, 2) + '\\n')
  console.log(JSON.stringify({
    path,
    cases: results.length,
    probes: results.reduce((n, c) => n + c.probes.length, 0),
    wheelCases: wheelResults.length,
    wheelProbes: wheelResults.reduce((n, c) => n + c.probes.length, 0),
  }))
}
main(${JSON.stringify(cases)}, ${JSON.stringify(wheelCases)})
`

await build({
  stdin: { contents: entry, resolveDir: process.cwd(), sourcefile: 'mouse-contract-export.ts' },
  bundle: true,
  platform: 'node',
  format: 'cjs',
  outfile: output,
  alias: { common: resolve(source, 'common'), vs: resolve(source, 'vs') },
  tsconfigRaw: { compilerOptions: { experimentalDecorators: true } },
})
execFileSync(process.execPath, [output], { stdio: 'inherit' })
