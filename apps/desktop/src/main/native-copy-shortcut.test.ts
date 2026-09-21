import type { WebContents } from 'electron'
import { describe, expect, it, vi } from 'vitest'
import { installNativeCopyShortcut, setTerminalCopyFocus } from './native-copy-shortcut'

describe('native copy shortcut routing', () => {
  it('routes only terminal-owned Command+C to the renderer and restores native field copying', () => {
    const on = vi.fn()
    const contents = { on, setIgnoreMenuShortcuts: vi.fn() }
    const webContents = contents as unknown as WebContents
    installNativeCopyShortcut(webContents)
    const handle = on.mock.calls[0]![1]
    const copy = { type: 'keyDown', key: 'c', meta: true }
    handle({}, copy)
    expect(contents.setIgnoreMenuShortcuts).toHaveBeenLastCalledWith(false)
    setTerminalCopyFocus(webContents, true)
    handle({}, copy)
    expect(contents.setIgnoreMenuShortcuts).toHaveBeenLastCalledWith(true)
    handle({}, { ...copy, key: 'v' })
    expect(contents.setIgnoreMenuShortcuts).toHaveBeenLastCalledWith(false)
    handle({}, { ...copy, meta: false, control: true })
    expect(contents.setIgnoreMenuShortcuts).toHaveBeenLastCalledWith(false)
    setTerminalCopyFocus(webContents, false)
    handle({}, copy)
    expect(contents.setIgnoreMenuShortcuts).toHaveBeenLastCalledWith(false)
  })
})
