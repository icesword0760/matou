import { createRequire } from 'node:module'
import { writeFileSync } from 'node:fs'
const require = createRequire(new URL('../../apps/runtime/package.json', import.meta.url))
const { Terminal } = require('@xterm/headless')
const terminal = new Terminal({ allowProposedApi: true })
const sequences = ['', '\x1b[?9h', '\x1b[?1000h', '\x1b[?1002h', '\x1b[?1003h', '\x1b[?9l', '\x1b[?1002h', '\x1b[?1000l', '\x1b[?1006h', '\x1b[?1005h', '\x1b[?1005l', '\x1b[?1015h', '\x1b[?1015l', '\x1b[?1016h', '\x1b[?1006l', '\x1b[?1006h', '\x1b[?1016l', '\x1b[?1003;1016h', '\x1b[!p', '\x1bc']
const steps = []
for (const input of sequences) {
  if (input) await new Promise(done => terminal.write(input, done))
  steps.push({ input, protocol: terminal._core.coreMouseService.activeProtocol, encoding: terminal._core.coreMouseService.activeEncoding })
}
terminal.dispose()
writeFileSync('apps/native/terminal/tests/fixtures/mouse-modes-reference.json', JSON.stringify({ steps }, null, 2) + '\n')
console.log(JSON.stringify(steps))
