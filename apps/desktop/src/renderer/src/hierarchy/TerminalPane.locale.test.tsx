// @vitest-environment jsdom
import { cleanup, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { LocaleProvider } from '../i18n/LocaleProvider'
import { setCurrentLocale } from '../i18n/current'
import { TerminalPane } from './TerminalPane'
import { TerminalSearchBar } from './TerminalSearchBar'
import { foregroundTerminalModels } from '../terminal/terminal-model-cache'

vi.mock('../terminal/TerminalSurface', () => ({
  TerminalSurface: (props: { sessionId: string }) =>
    <div data-testid={`surface-${props.sessionId}`} />
}))

afterEach(() => {
  cleanup()
  foregroundTerminalModels.clear()
  setCurrentLocale('zh-CN')
})

describe('Terminal pane in English', () => {
  it('labels the loading badge and the loading water in English', () => {
    render(<LocaleProvider initialLocale="en"><TerminalPane {...fixture()} /></LocaleProvider>)

    expect(screen.getByText('Loading')).toBeTruthy()
    expect(screen.getByRole('status', { name: 'Loading the terminal: Claude main session' })).toBeTruthy()
  })

  it('names the terminal search input in English', () => {
    render(<LocaleProvider initialLocale="en"><TerminalSearchBar open themeKey="light"
      resultIndex={0} resultCount={0} onSearch={() => undefined} onNext={() => undefined}
      onPrevious={() => undefined} onClose={() => undefined} /></LocaleProvider>)

    const input = screen.getByRole('textbox', { name: 'Search terminal' })
    expect(input.getAttribute('placeholder')).toBe('Search terminal')
  })
})

function fixture() {
  return {
    session: {
      id: 'session-1', taskId: 'task-1', title: 'Claude main session',
      kind: 'claude-code' as const, executionContextId: 'context-1'
    },
    active: true,
    pathValid: true,
    onActivate: vi.fn()
  }
}
