// @vitest-environment node
import { readFileSync } from 'node:fs'

import { describe, expect, it } from 'vitest'

const css = readFileSync(new URL('./hierarchy.css', import.meta.url), 'utf8')

describe('unified workspace visual contract', () => {
  it('uses one calm canvas and panel system for the board and settings pages', () => {
    expect(css).toContain('--workspace-canvas: #f7f8fa;')
    expect(css).toContain('--workspace-panel: #fff;')
    expect(css).toContain('--workspace-panel-muted: #fbfcfd;')
    expect(rule('.workspace-board')).toContain('background: var(--workspace-canvas)')
    expect(rule('.model-settings')).toContain('background: var(--workspace-canvas)')
    expect(rule('.model-settings__frame')).toContain('background: transparent')
    expect(rule('.model-settings__nav')).not.toContain('backdrop-filter')
  })

  it('keeps board status color in metadata instead of tinting whole columns', () => {
    expect(rule('.board-column')).toContain('background: var(--workspace-panel-muted)')
    expect(rule('.workspace-board__guide i')).toContain('mask:')
    expect(rule('.board-column--active')).toBe('--board-tone: #d29a35;')
    expect(rule('.board-column--blocked')).toBe('--board-tone: #d26762;')
    expect(rule('.board-column--completed')).toBe('--board-tone: #589a70;')
  })

  it('renders providers as a single terminal-density list surface', () => {
    expect(rule('.model-settings__providers')).toContain('border: 1px solid var(--workspace-line)')
    expect(rule('.model-settings__providers')).toContain('background: var(--workspace-panel-muted)')
    expect(rule('.model-provider')).toContain('border-bottom: 1px solid var(--workspace-line)')
    expect(rule('.model-provider')).toContain('border-radius: 0')
  })
})

function rule(selector: string): string {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
  const match = css.match(new RegExp(`${escaped}\\s*\\{([^}]+)\\}`))
  if (!match?.[1]) throw new Error(`Missing CSS rule ${selector}`)
  return match[1].trim()
}
