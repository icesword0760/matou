// Execute the frozen search engine and highlight traversal against the real parser.
import { execFileSync } from 'node:child_process'
import { mkdirSync } from 'node:fs'
import { createRequire } from 'node:module'
import { resolve } from 'node:path'

const { build } = createRequire(import.meta.url)('../../node_modules/.pnpm/esbuild@0.27.7/node_modules/esbuild')
const addon = resolve('node_modules/.pnpm/@xterm+addon-search@0.16.0/node_modules/@xterm/addon-search/src')
const source = resolve('node_modules/.pnpm/@xterm+xterm@6.0.0/node_modules/@xterm/xterm/src')
const output = resolve('artifacts/native-parity/reference/search-contract-export.cjs')
mkdirSync(resolve('artifacts/native-parity/reference'), { recursive: true })

const cases = [
  {
    name: 'cycles-and-incremental-query',
    cols: 24, rows: 4, text: 'one KEY two key\r\nkey alone\r\nlast KEY',
    probes: [
      ...Array.from({ length: 5 }, () => ({ op: 'next', term: 'key' })),
      { op: 'next', term: 'ke' },
      ...Array.from({ length: 5 }, () => ({ op: 'previous', term: 'key' })),
      { op: 'previous', term: 'KEY', options: { caseSensitive: true } },
      { op: 'find', term: 'KEY', row: 0, col: 4, options: { caseSensitive: true } },
      { op: 'find', term: '', row: 0, col: 0 },
    ],
  },
  {
    name: 'whole-word-boundaries',
    cols: 24, rows: 4, text: 'untest test\r\n[test] next\r\nfoo_test~test\r\nSTOP',
    probes: [
      { op: 'find', term: 'test', row: 0, col: 0, options: { wholeWord: true } },
      { op: 'find', term: 'test', row: 0, col: 5, options: { wholeWord: true } },
      { op: 'find', term: 'test', row: 2, col: 0, options: { wholeWord: true } },
      { op: 'find', term: 'test', row: 2, col: 9, options: { wholeWord: true } },
      { op: 'all', term: 'test', options: { wholeWord: true } },
    ],
  },
  {
    name: 'wide-padding-and-unicode-offsets',
    cols: 8, rows: 5, text: 'abc1234中文🙂e\u0301fin\r\nlast',
    probes: [
      { op: 'find', term: '4中文', row: 0, col: 0 },
      { op: 'find', term: '🙂e\u0301', row: 0, col: 0 },
      { op: 'find', term: '中文', row: 1, col: 0 },
      { op: 'find', term: '.', row: 1, col: 4, options: { regex: true } },
      { op: 'find', term: '🙂.', row: 0, col: 0, options: { regex: true } },
      { op: 'find', term: 'fin', row: 1, col: 4 },
      { op: 'previous', term: '中文' },
    ],
  },
  {
    name: 'regex-slices-lookbehind-backrefs-and-overlap',
    cols: 24, rows: 4, text: 'cat42 cat42 noon\r\nababa\r\nlast',
    probes: [
      { op: 'find', term: '(?<=cat)\\d+', row: 0, col: 0, options: { regex: true } },
      { op: 'find', term: '(?<=cat)\\d+', row: 0, col: 3, options: { regex: true } },
      { op: 'find', term: '([a-z])\\1', row: 0, col: 0, options: { regex: true } },
      { op: 'previous', term: 'aba', options: { regex: true } },
      { op: 'previous', term: 'aba', options: { regex: true } },
      { op: 'find', term: '(?=a)|a', row: 1, col: 0, options: { regex: true } },
      { op: 'previous', term: '(?=a)|a', options: { regex: true } },
      { op: 'previous', term: '$', options: { regex: true } },
      { op: 'find', term: '[', row: 0, col: 0, options: { regex: true } },
    ],
  },
  {
    name: 'signed-cell-mapping-inside-combining-graphemes',
    cols: 10, rows: 2, text: 'e\u0301\u0327\u0300abc',
    probes: [
      { op: 'find', term: '.', row: 0, col: 0, options: { regex: true } },
      { op: 'find', term: '\u0327', row: 0, col: 0 },
      { op: 'next', term: '\u0327' },
      { op: 'next', term: '\u0327' },
      { op: 'previous', term: '\u0327' },
      { op: 'all', term: '.', options: { regex: true } },
    ],
  },
  {
    name: 'thousand-highlight-limit',
    cols: 20, rows: 4, history: 200,
    generated: { unit: 'a '.repeat(9) + '\r\n', count: 116 },
    probes: [{ op: 'all', term: 'a', summarize: true }],
  },
]

const entry = `
import { SearchAddon } from 'addon/SearchAddon'
import { SearchEngine } from 'addon/SearchEngine'
import { SearchLineCache } from 'addon/SearchLineCache'
import { SelectionModel } from 'browser/selection/SelectionModel'
import { createRequire } from 'node:module'
import { writeFileSync } from 'node:fs'
import { resolve } from 'node:path'

const require = createRequire(resolve('apps/runtime/package.json'))
const { Terminal } = require('@xterm/headless')
const match = result => ({
  term: Array.from({ length: result.term.length }, (_, i) => result.term.charCodeAt(i)),
  row: result.row,
  col: result.col,
  size: result.size,
})
const point = ([x, y]) => ({ x, y })

async function main(cases) {
  const results = []
  for (const { probes, ...input } of cases) {
    const terminal = new Terminal({
      cols: input.cols, rows: input.rows, scrollback: input.history ?? 0, allowProposedApi: true,
    })
    const text = input.text ?? input.generated.unit.repeat(input.generated.count)
    await new Promise(done => terminal.write(text, done))
    const selection = new SelectionModel(terminal._core._bufferService)
    terminal.clearSelection = () => selection.clearSelection()
    terminal.getSelectionPosition = () => {
      const start = selection.finalSelectionStart
      const end = selection.finalSelectionEnd
      if (!start || !end || (start[0] === end[0] && start[1] === end[1])) return undefined
      return { start: point(start), end: point(end) }
    }
    const cache = new SearchLineCache(terminal)
    const engine = new SearchEngine(terminal, cache)
    const searchAddon = new SearchAddon()
    searchAddon._terminal = terminal
    searchAddon._engine = engine
    searchAddon._decorationManager = { clearHighlightDecorations() {} }
    let all = []
    searchAddon._resultTracker.updateResults = values => { all = values }
    let cachedTerm
    const outcomes = []
    for (const action of probes) {
      const { op, term, options = {} } = action
      const previous = terminal.getSelectionPosition() ?? null
      const beforeTerm = cachedTerm ?? null
      let expected
      try {
        if (op === 'all') {
          all = []
          searchAddon._highlightAllMatches(term, options)
          expected = action.summarize
            ? { count: all.length, first: match(all[0]), last: match(all.at(-1)) }
            : all.map(match)
        } else {
          const result = op === 'find'
            ? engine.find(term, action.row, action.col, options)
            : op === 'next'
              ? engine.findNextWithSelection(term, options, cachedTerm)
              : engine.findPreviousWithSelection(term, options, cachedTerm)
          expected = result ? match(result) : null
          if (op !== 'find') {
            cachedTerm = term
            if (result) {
              selection.selectionStart = [result.col, result.row]
              selection.selectionStartLength = result.size
            }
          }
        }
      } catch (error) {
        if (!(error instanceof SyntaxError)) throw error
        expected = { error: true }
      }
      outcomes.push({ ...action, previous, cachedTerm: beforeTerm, expected })
    }
    results.push({ ...input, probes: outcomes })
    searchAddon.dispose()
    cache.dispose()
    terminal.dispose()
  }
  const path = resolve('apps/native/terminal/tests/fixtures/search-reference.json')
  writeFileSync(path, JSON.stringify({ source: 'Frozen SearchEngine, SearchLineCache and SearchAddon', cases: results }, null, 2) + '\\n')
  console.log(JSON.stringify({ path, cases: results.length, probes: results.reduce((n, c) => n + c.probes.length, 0) }))
}
main(${JSON.stringify(cases)}).catch(error => {
  console.error(error)
  process.exitCode = 1
})
`

await build({
  stdin: { contents: entry, resolveDir: process.cwd(), sourcefile: 'search-contract-export.ts' },
  bundle: true,
  platform: 'node',
  format: 'cjs',
  outfile: output,
  alias: { addon, browser: resolve(source, 'browser'), common: resolve(source, 'common'), vs: resolve(source, 'vs') },
  tsconfigRaw: { compilerOptions: { experimentalDecorators: true } },
})
execFileSync(process.execPath, [output], { stdio: 'inherit' })
