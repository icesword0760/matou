// Run the frozen renderer handler and parser limit without opening a clipboard.
import { execFileSync } from 'node:child_process'
import { mkdirSync, readFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { resolve } from 'node:path'

const { build } = createRequire(import.meta.url)('../../node_modules/.pnpm/esbuild@0.27.7/node_modules/esbuild')
const source = resolve('node_modules/.pnpm/@xterm+xterm@6.0.0/node_modules/@xterm/xterm/src')
const output = resolve('artifacts/native-parity/reference/clipboard-contract-export.cjs')
mkdirSync(resolve('artifacts/native-parity/reference'), { recursive: true })

const renderer = readFileSync('apps/desktop/src/renderer/src/terminal/TerminalSurface.tsx', 'utf8')
const marker = 'terminal.parser.registerOscHandler(52, (content) => {'
const handlerStart = renderer.indexOf(marker)
const handlerEnd = renderer.indexOf('\n    }))', handlerStart)
if (handlerStart < 0 || handlerEnd < 0 || renderer.indexOf(marker, handlerStart + 1) !== -1) {
  throw new Error('Cannot locate the unique frozen clipboard handler')
}
const body = renderer.slice(handlerStart + marker.length, handlerEnd)

const encode = text => Buffer.from(text, 'utf8').toString('base64')
const payloadLimit = 4 * 1024 * 1024
const cases = [
  { name: 'ascii', content: 'c;' + encode('Hello, Matou!') },
  { name: 'chinese-and-emoji', content: 'p;' + encode('中文🙂') },
  { name: 'empty-selector', content: ';YQ==' },
  { name: 'arbitrary-selector', content: 'clip-任意🙂;YQ==' },
  { name: 'utf8-bom', content: 'c;' + encode('\uFEFFhello') },
  { name: 'double-bom', content: 'c;' + encode('\uFEFF\uFEFFhello') },
  { name: 'empty-decoded-bom', content: 'c;' + encode('\uFEFF') },
  { name: 'empty-decoded-whitespace', content: 'c; \t\r\n\f' },
  { name: 'omitted-padding-two-bits', content: 'c;YQ' },
  { name: 'omitted-padding-four-bits', content: 'c;YWI' },
  { name: 'noncanonical-tail-two-pads', content: 'c;YR==' },
  { name: 'noncanonical-tail-one-pad', content: 'c;YWJ=' },
  { name: 'base64-html-whitespace', content: 'c;Y Q\t=\r\n\f=' },
  { name: 'vertical-tab-is-not-html-whitespace', content: 'c;YQ==\v' },
  { name: 'nbsp-is-not-html-whitespace', content: 'c;YQ==\u00a0' },
  { name: 'invalid-base64-alphabet', content: 'c;Y@==' },
  { name: 'url-safe-base64-is-not-accepted', content: 'c;YQ-_' },
  { name: 'one-base64-symbol', content: 'c;A' },
  { name: 'partial-padding', content: 'c;YQ=' },
  { name: 'excess-padding', content: 'c;YQ===' },
  { name: 'internal-padding', content: 'c;Y=Q=' },
  { name: 'invalid-utf8', content: 'c;/w==' },
  { name: 'overlong-utf8', content: 'c;' + Buffer.from([0xc0, 0xaf]).toString('base64') },
  { name: 'extra-semicolon', content: 'c;YQ==;Yg==' },
  { name: 'missing-semicolon', content: 'YQ==' },
  { name: 'empty-payload', content: 'c;' },
  { name: 'read-request', content: 'c;?' },
  { name: 'read-request-with-whitespace', content: 'c; ? ' },
  {
    name: 'payload-at-limit',
    generated: { prefix: 'c;', unit: ' ', count: payloadLimit - 4, suffix: 'YQ==' },
  },
  {
    name: 'payload-over-limit',
    generated: { prefix: 'c;', unit: ' ', count: payloadLimit - 3, suffix: 'YQ==' },
  },
  {
    name: 'unicode-selector-at-osc-limit',
    generated: { prefix: '', unit: '🙂', count: 4_999_997, suffix: 'x;YQ==' },
  },
  {
    name: 'unicode-selector-over-osc-limit',
    generated: { prefix: '', unit: '🙂', count: 4_999_997, suffix: 'xx;YQ==' },
  },
]

const streams = [
  { name: 'bel-and-st', chunks: ['\x1b]52;c;SGVsbG8=\x07', '\x1b]52;p;5Lit5paH8J+Zgg==\x1b\\'] },
  { name: 'fragmented', chunks: ['\x1b]', '52;', 'c;Y', 'Q==', '\x1b', '\\'] },
  { name: 'control-cancel', chunks: ['\x1b]52;c;YQ==\x18', '\x1b]52;c;Yg==\x1a', '\x1b]52;c;Yw==\x07'] },
  { name: 'query-and-malformed', chunks: ['\x1b]52;c;?\x07\x1b]52;c;/w==\x07\x1b]52;c;Y@==\x07'] },
  { name: 'unicode-selector', chunks: ['\x1b]52;任意🙂;YQ==\x07'] },
  { name: 'control-whitespace', chunks: ['\x1b]52;c;Y Q\t=\r\n\f=\x07'] },
  { name: 'empty-result', chunks: ['\x1b]52;c;77u/\x07'] },
  { name: 'leading-zero-command', chunks: ['\x1b]0052;c;YQ==\x07'] },
]

const entry = `
import { OscHandler } from 'common/parser/OscParser'
import { writeFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { createRequire } from 'node:module'
const { Terminal } = createRequire(resolve('apps/runtime/package.json'))('@xterm/headless')

const handler = new Function('content', 'replaying', 'visibleRef', 'activeRef', 'window', ${JSON.stringify(body)})
async function main(cases, streams) {
  const results = []
  for (const input of cases) {
    const content = input.content ?? (
      input.generated.prefix + input.generated.unit.repeat(input.generated.count) + input.generated.suffix
    )
    let text = null
    const window = {
      matouDesktop: {
        writeClipboardText(value) {
          if (text !== null) throw new Error('Multiple writes from one request')
          text = value
        },
      },
    }
    const parser = new OscHandler(data => handler(data, false, { current: true }, { current: true }, window))
    parser.start()
    const buffer = new Uint32Array(4096)
    for (let position = 0; position < content.length;) {
      let count = 0
      while (count < buffer.length && position < content.length) {
        const codepoint = content.codePointAt(position)
        buffer[count++] = codepoint
        position += codepoint > 0xffff ? 2 : 1
      }
      parser.put(buffer, 0, count)
    }
    parser.end(true)
    results.push({ ...input, contentUnits: content.length, text })
  }
  const streamResults = []
  for (const input of streams) {
    const terminal = new Terminal({ allowProposedApi: true })
    const writes = []
    const responses = []
    const window = { matouDesktop: { writeClipboardText: text => writes.push(text) } }
    terminal.parser.registerOscHandler(52, content => handler(content, false, { current: true }, { current: true }, window))
    terminal.onData(data => responses.push(data))
    for (const chunk of input.chunks) await new Promise(done => terminal.write(chunk, done))
    streamResults.push({ ...input, writes, responses })
    terminal.dispose()
  }
  const path = resolve('apps/native/terminal/tests/fixtures/clipboard-reference.json')
  writeFileSync(path, JSON.stringify({ source: 'Frozen renderer OSC 52 handler and OscHandler', cases: results, streams: streamResults }, null, 2) + '\\n')
  console.log(JSON.stringify({ path, cases: results.length }))
}
main(${JSON.stringify(cases)}, ${JSON.stringify(streams)})
`

await build({
  stdin: { contents: entry, resolveDir: process.cwd(), sourcefile: 'clipboard-contract-export.ts' },
  bundle: true,
  platform: 'node',
  format: 'cjs',
  outfile: output,
  alias: { common: resolve(source, 'common') },
})
execFileSync(process.execPath, [output], { stdio: 'inherit' })
