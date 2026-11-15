// Exercise the frozen programmatic selection and text extraction against the real parser.
import { execFileSync } from 'node:child_process'
import { mkdirSync } from 'node:fs'
import { createRequire } from 'node:module'
import { resolve } from 'node:path'

const { build } = createRequire(import.meta.url)('../../node_modules/.pnpm/esbuild@0.27.7/node_modules/esbuild')
const source = resolve('node_modules/.pnpm/@xterm+xterm@6.0.0/node_modules/@xterm/xterm/src')
const output = resolve('artifacts/native-parity/reference/programmatic-selection-export.cjs')
mkdirSync(resolve('artifacts/native-parity/reference'), { recursive: true })

const input = {
  cols: 8,
  rows: 4,
  text: 'abcdefghijk\r\n中文 tail',
  probes: [
    { name: 'ordinary', row: 0, col: 2, length: 4 },
    { name: 'soft-wrap', row: 0, col: 6, length: 5 },
    { name: 'hard-break', row: 1, col: 1, length: 12 },
    { name: 'right-edge', row: 0, col: 6, length: 2 },
    { name: 'wrapped-right-edge', row: 0, col: 6, length: 10 },
    { name: 'zero-length', row: 1, col: 1, length: 0 },
    { name: 'negative-column', row: 0, col: -2, length: 5 },
    { name: 'negative-column-and-end', row: 0, col: -3, length: 2 },
    { name: 'negative-length', row: 0, col: 3, length: -2 },
  ],
}

const entry = `
import { SelectionModel } from 'browser/selection/SelectionModel'
import { SelectionService } from 'browser/services/SelectionService'
import { createRequire } from 'node:module'
import { writeFileSync } from 'node:fs'
import { resolve } from 'node:path'

const require = createRequire(resolve('apps/runtime/package.json'))
const { Terminal } = require('@xterm/headless')

async function main({ cols, rows, text, probes }) {
  const terminal = new Terminal({ cols, rows, scrollback: 0, allowProposedApi: true })
  await new Promise(done => terminal.write(text, done))
  const service = Object.create(SelectionService.prototype)
  service._bufferService = terminal._core._bufferService
  service._model = new SelectionModel(service._bufferService)
  service._removeMouseDownListeners = () => {}
  service.refresh = () => {}
  service._fireEventIfSelectionChanged = () => {}

  const outcomes = probes.map(probe => {
    service.setSelection(probe.col, probe.row, probe.length)
    return {
      ...probe,
      expected: {
        start: service._model.finalSelectionStart,
        end: service._model.finalSelectionEnd,
        text: service.selectionText,
      },
    }
  })
  terminal.dispose()
  const path = resolve('apps/native/terminal/tests/fixtures/programmatic-selection-reference.json')
  const fixture = { source: 'Frozen SelectionService and SelectionModel', cols, rows, text, probes: outcomes }
  writeFileSync(path, JSON.stringify(fixture, null, 2) + '\\n')
  console.log(JSON.stringify({ path, probes: outcomes.length }))
}
main(${JSON.stringify(input)}).catch(error => {
  console.error(error)
  process.exitCode = 1
})
`

await build({
  stdin: { contents: entry, resolveDir: process.cwd(), sourcefile: 'programmatic-selection-export.ts' },
  bundle: true,
  platform: 'node',
  format: 'cjs',
  outfile: output,
  alias: { browser: resolve(source, 'browser'), common: resolve(source, 'common'), vs: resolve(source, 'vs') },
  tsconfigRaw: { compilerOptions: { experimentalDecorators: true } },
})
execFileSync(process.execPath, [output], { stdio: 'inherit' })
