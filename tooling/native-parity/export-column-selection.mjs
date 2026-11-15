// Exercise the frozen selection service with actual terminal cells.
import { execFileSync } from 'node:child_process'
import { mkdirSync } from 'node:fs'
import { createRequire } from 'node:module'
import { resolve } from 'node:path'

const { build } = createRequire(import.meta.url)('../../node_modules/.pnpm/esbuild@0.27.7/node_modules/esbuild')
const source = resolve('node_modules/.pnpm/@xterm+xterm@6.0.0/node_modules/@xterm/xterm/src')
const output = resolve('artifacts/native-parity/reference/column-selection-export.cjs')
mkdirSync(resolve('artifacts/native-parity/reference'), { recursive: true })

const cases = [
  {
    name: 'four-directions-and-zero-width',
    cols: 8, rows: 4, text: 'abcdef\r\nghijkl\r\nmnopqr', column: true,
    probes: [
      { start: [1, 0], steps: [{ move: [4, 2] }] },
      { start: [4, 0], steps: [{ move: [1, 2] }] },
      { start: [1, 2], steps: [{ move: [4, 0] }] },
      { start: [4, 2], steps: [{ move: [1, 0] }] },
      { start: [2, 0], steps: [{ move: [2, 2] }] },
    ],
  },
  {
    name: 'soft-wrap-blank-row-nbsp-and-written-space',
    cols: 8, rows: 5, text: 'ABCD12X4EFGH56Y8\r\n\r\nQ\u00a0S \r\n', column: true,
    probes: [{ start: [0, 0], steps: [{ move: [4, 3] }] }],
  },
  {
    name: 'wide-characters-on-edges',
    cols: 8, rows: 4, text: 'abcdef\r\n中Z文Q\r\nghijkl', column: true,
    probes: [
      { start: [1, 0], steps: [{ move: [3, 2] }] },
      { start: [1, 1], steps: [{ move: [5, 2] }] },
      { start: [0, 0], steps: [{ move: [4, 1] }] },
    ],
  },
  {
    name: 'linear-incremental-reverse-at-wide-tail',
    cols: 8, rows: 4, text: '中Z\u00a0\r\nSECOND', column: false,
    probes: [{ start: [3, 1], steps: [{ incremental: [1, 0] }] }],
  },
  {
    name: 'column-beyond-viewport-and-scroll',
    cols: 10, rows: 4, history: 40, column: true,
    text: Array.from({ length: 16 }, (_, row) => 'line' + String(row).padStart(2, '0')).join('\r\n'),
    probes: [
      { start: [5, 14], steps: [{ move: [1, 12], outside: -2 }, { scroll: 2 }, { scroll: -10 }] },
      { start: [1, 13], steps: [{ move: [4, 15], outside: 2 }] },
    ],
  },
]

const entry = `
import { SelectionService } from 'browser/services/SelectionService'
import { SelectionModel } from 'browser/selection/SelectionModel'
import { createRequire } from 'node:module'
import { writeFileSync } from 'node:fs'
import { resolve } from 'node:path'

const require = createRequire(resolve('apps/runtime/package.json'))
const { Terminal } = require('@xterm/headless')

async function main(cases) {
  const results = []
  for (const { probes, ...input } of cases) {
    const resultsForCase = []
    for (const { start, steps } of probes) {
      const terminal = new Terminal({
        cols: input.cols, rows: input.rows, allowProposedApi: true, scrollback: input.history ?? 0,
        macOptionClickForcesSelection: false,
      })
      await new Promise(done => terminal.write(input.text, done))
      const bufferService = terminal._core._bufferService
      const model = new SelectionModel(bufferService)
      const selection = Object.create(SelectionService.prototype)
      selection._bufferService = bufferService
      selection._optionsService = { rawOptions: terminal.options }
      selection._model = model
      selection._getMouseBufferCoords = event => [...event.point]
      selection._getMouseEventScrollAmount = event => event.outside ?? 0
      selection.refresh = () => {}
      selection._onRequestScrollLines = { fire: ({ amount }) => terminal.scrollLines(amount) }
      selection._handleSingleClick({ altKey: input.column, point: start })

      const resultsAfterSteps = []
      for (const action of steps) {
        if (action.move) {
          selection._handleMouseMove({
            point: action.move,
            outside: action.outside,
            stopImmediatePropagation() {},
          })
        } else if (action.scroll !== undefined) {
          selection._dragScrollAmount = -action.scroll
          selection._dragScroll()
        } else if (action.incremental) {
          selection._handleIncrementalClick({ point: action.incremental })
        }
        resultsAfterSteps.push({
          ...action,
          expected: {
            start: [...model.finalSelectionStart],
            end: [...model.finalSelectionEnd],
            offset: terminal.buffer.active.viewportY,
            text: selection.selectionText,
          },
        })
      }
      resultsForCase.push({ start, steps: resultsAfterSteps })
      terminal.dispose()
    }
    results.push({ ...input, probes: resultsForCase })
  }
  const path = resolve('apps/native/terminal/tests/fixtures/column-selection-reference.json')
  writeFileSync(path, JSON.stringify({ source: 'Frozen xterm 6.0.0 SelectionService', cases: results }, null, 2) + '\\n')
  console.log(JSON.stringify({ path, cases: results.length, probes: results.reduce((n, c) => n + c.probes.length, 0) }))
}
main(${JSON.stringify(cases)}).catch(error => {
  console.error(error)
  process.exitCode = 1
})
`

await build({
  stdin: { contents: entry, resolveDir: process.cwd(), sourcefile: 'column-selection-export.ts' },
  bundle: true,
  platform: 'node',
  format: 'cjs',
  outfile: output,
  alias: { browser: resolve(source, 'browser'), common: resolve(source, 'common'), vs: resolve(source, 'vs') },
  tsconfigRaw: { compilerOptions: { experimentalDecorators: true } },
})
execFileSync(process.execPath, [output], { stdio: 'inherit' })
