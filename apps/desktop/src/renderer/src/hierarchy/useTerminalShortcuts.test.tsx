// @vitest-environment jsdom
import { fireEvent, render } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'

import { useTerminalShortcuts } from './useTerminalShortcuts'

function ShortcutHarness({ revision, onToggle }: { revision: number; onToggle(): void }) {
  useTerminalShortcuts({
    toggleShortcutPanel: () => {
      void revision
      onToggle()
    }
  })
  return null
}

describe('useTerminalShortcuts', () => {
  it('recognizes double Option when the interface rerenders between the two taps', () => {
    const onToggle = vi.fn()
    const view = render(<ShortcutHarness revision={0} onToggle={onToggle} />)

    fireEvent.keyDown(document, { key: 'Alt', code: 'AltLeft', altKey: true })
    fireEvent.keyUp(document, { key: 'Alt', code: 'AltLeft' })
    view.rerender(<ShortcutHarness revision={1} onToggle={onToggle} />)
    fireEvent.keyDown(document, { key: 'Alt', code: 'AltLeft', altKey: true })
    fireEvent.keyUp(document, { key: 'Alt', code: 'AltLeft' })

    expect(onToggle).toHaveBeenCalledTimes(1)
  })
})
