import type { WebContents } from 'electron'

const terminalFocused = new WeakSet<WebContents>()
export function setTerminalCopyFocus(contents: WebContents, focused: boolean): void {
  if (focused) terminalFocused.add(contents)
  else terminalFocused.delete(contents)
}
export function installNativeCopyShortcut(contents: WebContents): void {
  contents.on('before-input-event', (_event, input) => {
    // Native Copy sees only xterm's empty helper textarea, not its VT selection.
    // Only terminal-owned Cmd+C bypasses the menu; ordinary fields stay native.
    contents.setIgnoreMenuShortcuts(terminalFocused.has(contents) &&
      Boolean(input.meta) && !input.control && !input.alt && !input.shift &&
      input.key.toLowerCase() === 'c')
  })
}
