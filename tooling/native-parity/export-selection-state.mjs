// Produce selection-state outcomes with the frozen source model.
import { execFileSync } from 'node:child_process'
import { mkdirSync } from 'node:fs'
import { createRequire } from 'node:module'
import { resolve } from 'node:path'

const { build } = createRequire(import.meta.url)('../../node_modules/.pnpm/esbuild@0.27.7/node_modules/esbuild')
const source = resolve('node_modules/.pnpm/@xterm+xterm@6.0.0/node_modules/@xterm/xterm/src')
const output = resolve('artifacts/native-parity/reference/selection-state-export.cjs')
mkdirSync(resolve('artifacts/native-parity/reference'), { recursive: true })

const cases = [
  {
    name: 'wrapped-word-without-explicit-end',
    cols: 5, rows: 5, text: 'TOP\r\nxx ABCDEFG\r\nTAIL', mode: 'word', point: [4, 1],
    steps: [{ trim: 1 }, { trim: 2 }, { clear: true }],
  },
  {
    name: 'wrapped-word-drag-on-start-row',
    cols: 5, rows: 5, text: 'TOP\r\nxx ABCDEFG\r\nTAIL', mode: 'word', point: [4, 1],
    steps: [{ end: [5, 1] }, { trim: 1 }, { trim: 1 }],
  },
  {
    name: 'wrapped-word-reversed-exact-end-trim',
    cols: 5, rows: 5, text: 'TOP\r\nxx ABCDEFG\r\nTAIL', mode: 'word', point: [4, 1],
    steps: [{ end: [2, 1] }, { trim: 1 }, { trim: 1 }],
  },
  {
    name: 'trimmed-start-retains-its-column',
    cols: 10, rows: 5, text: 'FIRST\r\nxx TOKEN\r\nTHIRD\r\nLAST', mode: 'word', point: [5, 1],
    steps: [{ end: [2, 3] }, { trim: 2 }, { trim: 1 }, { trim: 1 }],
  },
  {
    name: 'wrapped-line-forward-and-reversed',
    cols: 10, rows: 6, text: 'TOP\r\nab LONGPATH/abcdefghij END\r\nLAST', mode: 'line', point: [4, 2],
    steps: [{ end: [0, 0] }, { end: [10, 1] }, { end: [10, 2] }, { trim: 2 }],
  },
  {
    name: 'reverse-character-selection',
    cols: 10, rows: 4, text: 'first\r\nsecond\r\nthird', mode: 'character', point: [3, 2],
    steps: [{ end: [4, 1] }, { trim: 1 }, { trim: 1 }],
  },
]

const entry = `
import { SelectionService } from 'browser/services/SelectionService'
import { SelectionModel } from 'browser/selection/SelectionModel'
import { CellData } from 'common/buffer/CellData'
import { createRequire } from 'node:module'
import { writeFileSync } from 'node:fs'
import { resolve } from 'node:path'

const require = createRequire(resolve('apps/runtime/package.json'))
const { Terminal } = require('@xterm/headless')

async function main(cases) {
  const results = []
  for (const { steps, ...input } of cases) {
    const { cols, rows, text, mode, point } = input
    const terminal = new Terminal({ cols, rows, allowProposedApi: true, scrollback: 0 })
    await new Promise(done => terminal.write(text, done))
    const bufferService = terminal._core._bufferService
    const model = new SelectionModel(bufferService)
    const selection = Object.create(SelectionService.prototype)
    selection._bufferService = bufferService
    selection._optionsService = { rawOptions: terminal.options }
    selection._workCell = new CellData()
    selection._model = model
    if (mode === 'word') selection._selectWordAt([...point], true)
    else if (mode === 'line') selection._selectLineAt(point[1])
    else model.selectionStart = [...point]

    const current = () => {
      const start = model.finalSelectionStart
      const end = model.finalSelectionEnd
      return start && end ? { start: [...start], end: [...end] } : null
    }
    const initial = current()
    const resultsAfterSteps = []
    for (const action of steps) {
      if (action.end) model.selectionEnd = [...action.end]
      else if (action.trim !== undefined) model.handleTrim(action.trim)
      else if (action.clear) model.clearSelection()
      resultsAfterSteps.push({ ...action, expected: current() })
    }
    results.push({ ...input, initial, steps: resultsAfterSteps })
    terminal.dispose()
  }
  const path = resolve('apps/native/terminal/tests/fixtures/selection-state-reference.json')
  writeFileSync(path, JSON.stringify({ source: 'Frozen xterm 6.0.0 SelectionModel and SelectionService', cases: results }, null, 2) + '\\n')
  console.log(JSON.stringify({ path, cases: results.length, steps: results.reduce((n, c) => n + c.steps.length, 0) }))
}
main(${JSON.stringify(cases)}).catch(error => {
  console.error(error)
  process.exitCode = 1
})
`

await build({
  stdin: { contents: entry, resolveDir: process.cwd(), sourcefile: 'selection-state-export.ts' },
  bundle: true,
  platform: 'node',
  format: 'cjs',
  outfile: output,
  alias: { browser: resolve(source, 'browser'), common: resolve(source, 'common'), vs: resolve(source, 'vs') },
  tsconfigRaw: { compilerOptions: { experimentalDecorators: true } },
})
execFileSync(process.execPath, [output], { stdio: 'inherit' })
