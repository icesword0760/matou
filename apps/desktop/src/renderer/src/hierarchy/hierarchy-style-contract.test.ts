import { readFileSync } from 'node:fs'

import { describe, expect, it } from 'vitest'

const stylesheet = readFileSync(new URL('./hierarchy.css', import.meta.url), 'utf8')

describe('hierarchy visual style contract', () => {
  it('keeps SVG chrome controls and notification indicators styled', () => {
    expect(stylesheet).toContain('.tab-close svg')
    expect(stylesheet).toContain('.tab-overflow-btn svg')
    expect(stylesheet).toContain('.tab-add-btn svg')
    expect(stylesheet).toContain('.toolbar-btn svg')
    expect(stylesheet).toContain('.flat-sidebar__notify svg')
    expect(stylesheet).toContain('.flat-sidebar__notify-dot')
    expect(stylesheet).toContain('.project-dropdown__notify svg')
    expect(stylesheet).toContain('.project-dropdown__notify-dot')
  })

  it('keeps the update entry and all of its expanded states styled', () => {
    expect(stylesheet).toContain('.app-update-control')
    expect(stylesheet).toContain('.app-update-trigger')
    expect(stylesheet).toContain('.app-update-popover')
    expect(stylesheet).toContain('.app-update-download__track')
    expect(stylesheet).toContain('.app-update-session-warning')
    expect(stylesheet).toContain('.hierarchy-shell[data-theme="light"] .app-update-popover')
  })

  it('keeps every current Git and Worktree menu surface styled', () => {
    expect(stylesheet).toContain('.git-control-menu .sr-only')
    expect(stylesheet).toContain('.git-picker-view')
    expect(stylesheet).toContain('.git-search-field input')
    expect(stylesheet).toContain('.git-branch-list')
    expect(stylesheet).toContain('.git-picker-actions > button')
    expect(stylesheet).toContain('.git-subview-header')
    expect(stylesheet).toContain('.git-form-actions')
    expect(stylesheet).toContain('.git-worktree-row-actions')
    expect(stylesheet).toContain('.git-commit-actions')
  })
})
