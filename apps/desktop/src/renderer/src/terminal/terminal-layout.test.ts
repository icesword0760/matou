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

  it('measures repeated CJK punctuation at the same width used for rendered rows', () => {
    const terminalCss = css('terminal/terminal.css')

    expect(terminalCss).toMatch(
      /\.terminal-surface \.xterm\s*\{[^}]*text-spacing-trim:\s*space-all;/
    )
  })

  it('keeps the focused terminal canvas at one-to-one scale for sharp text', () => {
    const canvasCss = css('session-canvas/session-canvas.css')
    const focusedCardRule = canvasCss.match(/\.session-card\.is-focused\s*\{([^}]*)\}/)?.[1]

    expect(focusedCardRule).toContain('transform: translateY(-4px);')
    expect(focusedCardRule).not.toContain('scale(')
  })

  it('aligns the sidebar footer to the 38px terminal status bar', () => {
    const hierarchyCss = css('hierarchy/hierarchy.css')

    expect(hierarchyCss).toContain(
      '.flat-sidebar__toolbar { position: relative; z-index: 2; display: flex; flex: 0 0 38px;'
    )
    expect(hierarchyCss).toContain('height: 38px; min-height: 38px;')
  })

  it('uses the full top bar as a window drag surface outside interactive controls', () => {
    const hierarchyCss = css('hierarchy/hierarchy.css')

    expect(hierarchyCss).toMatch(/\.scene-tabs\.tab-bar-left\s*\{[^}]*-webkit-app-region:\s*drag;/)
    expect(hierarchyCss).toMatch(/\.tab-bar-overflow-actions\s*\{[^}]*-webkit-app-region:\s*drag;/)
    expect(hierarchyCss).toMatch(/\.tab-bar-right\s*\{[^}]*-webkit-app-region:\s*drag;/)
    expect(hierarchyCss).toMatch(/\.tab-item\s*\{[^}]*-webkit-app-region:\s*no-drag;/)
    expect(hierarchyCss).toContain('button { -webkit-app-region: no-drag; }')
  })

  it('keeps the Task delete action visually destructive in dark and light menus', () => {
    const hierarchyCss = css('hierarchy/hierarchy.css')

    expect(hierarchyCss).toMatch(/\.workbench-action-popover \.is-delete\s*\{[^}]*border-top:[^}]*color:\s*#ef7770;/)
    expect(hierarchyCss).toMatch(/\.light-theme \.workbench-action-popover \.is-delete\s*\{[^}]*color:\s*#c5443e;/)
    expect(hierarchyCss).toMatch(/\.light-theme \.workbench-action-popover \.is-delete:hover\s*\{[^}]*background:\s*#fff1f0;/)
  })

  it('keeps confirmation actions readable and removal actions destructive', () => {
    const hierarchyCss = css('hierarchy/hierarchy.css')

    expect(hierarchyCss).toMatch(/\[role="alertdialog"\] footer \.dialog-primary\s*\{[^}]*color:\s*#fff;[^}]*background:\s*#347fd6;/)
    expect(hierarchyCss).toMatch(/\.dialog-primary\.is-danger\s*\{[^}]*color:\s*#fff;[^}]*background:\s*#c5443e;/)
    expect(hierarchyCss).toMatch(/\.light-theme \[role="alertdialog"\] footer \.dialog-primary\s*\{[^}]*color:\s*#fff;/)
  })
})

function css(relativePath: string): string {
  return readFileSync(join(process.cwd(), 'src/renderer/src', relativePath), 'utf8')
}
