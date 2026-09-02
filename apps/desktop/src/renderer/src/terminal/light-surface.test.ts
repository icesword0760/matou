import { readFileSync } from 'node:fs'
import { join } from 'node:path'

import { describe, expect, it } from 'vitest'

import { TERMINAL_THEMES } from './terminal-themes'

describe('light terminal surface', () => {
  it('uses one clean neutral surface without a focused-card tint overlay', () => {
    const canvasCss = readFileSync(
      join(process.cwd(), 'src/renderer/src/session-canvas/session-canvas.css'),
      'utf8'
    )

    expect(TERMINAL_THEMES.light.background).toBe('#FCFCFD')
    expect(canvasCss).toContain(
      '.hierarchy-shell[data-theme="light"] .session-card { background: #fcfcfd;'
    )
    expect(canvasCss).toContain(
      '.hierarchy-shell[data-theme="light"] .session-card.is-focused::before { content: none; }'
    )
    expect(canvasCss).toContain(
      '.session-card .terminal-pane .terminal-surface { background: #fcfcfd; }'
    )
  })
})
