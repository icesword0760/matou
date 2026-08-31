import { readFileSync } from 'node:fs'
import { join } from 'node:path'

import { describe, expect, it } from 'vitest'

describe('terminal grid layout', () => {
  it('places visual insets on xterm so FitAddon reserves every visible row and column', () => {
    const terminalCss = css('terminal/terminal.css')
    const hierarchyCss = css('hierarchy/hierarchy.css')
    const canvasCss = css('session-canvas/session-canvas.css')

    expect(terminalCss).toContain('.terminal-surface {\n  --terminal-padding-block: 14px;')
    expect(terminalCss).toContain('padding: 0;')
    expect(terminalCss).toContain('padding: var(--terminal-padding-block) var(--terminal-padding-inline);')
    expect(hierarchyCss).toContain('--terminal-padding-block: 4px;')
    expect(canvasCss).toContain('--terminal-padding-block: 12px;')
  })
})

function css(relativePath: string): string {
  return readFileSync(join(process.cwd(), 'src/renderer/src', relativePath), 'utf8')
}
