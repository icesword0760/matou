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

  it('uses the full top bar as a window drag surface outside interactive controls', () => {
    const hierarchyCss = css('hierarchy/hierarchy.css')

    expect(hierarchyCss).toMatch(/\.scene-tabs\.tab-bar-left\s*\{[^}]*-webkit-app-region:\s*drag;/)
    expect(hierarchyCss).toMatch(/\.tab-bar-overflow-actions\s*\{[^}]*-webkit-app-region:\s*drag;/)
    expect(hierarchyCss).toMatch(/\.tab-bar-right\s*\{[^}]*-webkit-app-region:\s*drag;/)
    expect(hierarchyCss).toMatch(/\.tab-item\s*\{[^}]*-webkit-app-region:\s*no-drag;/)
    expect(hierarchyCss).toContain('button { -webkit-app-region: no-drag; }')
  })
})

function css(relativePath: string): string {
  return readFileSync(join(process.cwd(), 'src/renderer/src', relativePath), 'utf8')
}
