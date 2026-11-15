// Frozen parser contracts, using the same xterm major/minor as the reference UI.
// This does not replace real-window font, selection, IME or input acceptance.
import { createRequire } from 'node:module'
import { mkdirSync, writeFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { TERMINAL_THEMES } from '../../apps/desktop/src/renderer/src/terminal/terminal-themes'

const require = createRequire(resolve('apps/runtime/package.json'))
const { Terminal } = require('@xterm/headless')
const cases = [
  { name: 'unicode-wrap', cols: 12, rows: 5, writes: ['abc中文é🙂', '\r\n1234567890123456\r\nEND', '\r\n👩‍💻 🇨🇳 👍🏽\r\nA\u1ab0B\u0301C'] },
  { name: 'style-colors', cols: 24, rows: 5, writes: ['\x1b[31;44;1mA\x1b[0mB\x1b[3;4;9mC\x1b[0m', '\r\n\x1b[38;2;1;2;3;48;5;123mD\x1b[0m\x1b[2;7mE\x1b[0m\x1b[8mSECRET\x1b[0m'] },
  { name: 'alternate', cols: 20, rows: 4, writes: ['PRIMARY', '\x1b[?1049hALT\r\n中文', '\x1b[?1049l'] },
  { name: 'input-modes-and-reply', cols: 20, rows: 4, writes: ['\x1b[?1h\x1b[?2004h\x1b[3;4H\x1b[6n', '\x1b[?1l\x1b[?2004l'] },
  { name: 'cursor-shape', cols: 20, rows: 4, writes: ['\x1b[2 qABC', '\x1b[3 q\x1b[?25l', '\x1b[6 q\x1b[?25h'] },
  { name: 'history', cols: 12, rows: 3, writes: ['one\r\ntwo\r\nthree\r\nfour\r\nfive'] },
]
const color = (cell: any, side: 'Fg' | 'Bg') => cell[`is${side}RGB`]() ? { kind: 'rgb', value: cell[`get${side}Color`]() }
  : cell[`is${side}Palette`]() ? { kind: 'palette', value: cell[`get${side}Color`]() } : { kind: 'default' }
const output = []
for (const fixture of cases) {
  const terminal = new Terminal({ cols: fixture.cols, rows: fixture.rows, allowProposedApi: true,
    scrollback: 10_000, cursorBlink: true, cursorStyle: 'bar' })
  const replies: string[] = []
  terminal.onData((data: string) => replies.push(data))
  const steps = []
  for (const data of fixture.writes) {
    await new Promise<void>(done => terminal.write(data, done))
    const buffer = terminal.buffer.active
    const rows = []
    for (let y = 0; y < fixture.rows; y++) {
      const line = buffer.getLine(buffer.viewportY + y)
      const cells = []
      for (let x = 0; x < fixture.cols; x++) {
        const cell = line.getCell(x), width = cell.getWidth()
        cells.push({ text: width === 0 ? '' : cell.getChars() || ' ', width,
          fg: color(cell, 'Fg'), bg: color(cell, 'Bg'), bold: !!cell.isBold(), italic: !!cell.isItalic(),
          dim: !!cell.isDim(), underline: !!cell.isUnderline(), inverse: !!cell.isInverse(),
          invisible: !!cell.isInvisible(), strikethrough: !!cell.isStrikethrough(), overline: !!cell.isOverline() })
      }
      rows.push({ continuation: !!line.isWrapped, cells })
    }
    // Match frozen DomRenderer.ts:445-446. Public options do not reflect DECSCUSR overrides.
    const core = terminal._core.coreService
    steps.push({ data, expected: { cols: fixture.cols, rows, cursorCol: Math.min(buffer.cursorX, fixture.cols - 1), cursorRow: buffer.cursorY,
      cursorStyle: core.decPrivateModes.cursorStyle ?? terminal.options.cursorStyle,
      cursorBlink: core.decPrivateModes.cursorBlink ?? terminal.options.cursorBlink,
      cursorVisible: !core.isCursorHidden,
      historyLines: buffer.baseY, alternate: buffer.type === 'alternate',
      applicationCursorKeys: terminal.modes.applicationCursorKeysMode,
      bracketedPaste: terminal.modes.bracketedPasteMode, replies: replies.splice(0) } })
  }
  output.push({ name: fixture.name, cols: fixture.cols, rows: fixture.rows, steps })
  terminal.dispose()
}
const directory = resolve('apps/native/terminal/tests/fixtures')
mkdirSync(directory, { recursive: true })
writeFileSync(resolve(directory, 'reference.json'), JSON.stringify({
  source: 'Captured source xterm headless 6.0.0; raw cell styles before DOM renderer bold/inverse resolution. No UI rendering claim.',
  themes: TERMINAL_THEMES, cases: output
}, null, 2) + '\n')
console.log(JSON.stringify({ directory, cases: output.length }))
