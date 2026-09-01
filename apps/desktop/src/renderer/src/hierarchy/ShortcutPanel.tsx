import { createPortal } from 'react-dom'

import darkMac from '../assets/terminal-reference/terminal/shortcuts/dark_mac.png'
import darkWin from '../assets/terminal-reference/terminal/shortcuts/dark_win.png'
import whiteMac from '../assets/terminal-reference/terminal/shortcuts/white_mac.png'
import whiteWin from '../assets/terminal-reference/terminal/shortcuts/white_win.png'
import type { TerminalThemeKey } from '../terminal/terminal-themes'

export function ShortcutPanel({ open, isMac, themeKey, onClose }: {
  open: boolean
  isMac: boolean
  themeKey: TerminalThemeKey
  onClose(): void
}) {
  if (!open) return null
  const source = themeKey === 'dark'
    ? (isMac ? darkMac : darkWin)
    : (isMac ? whiteMac : whiteWin)
  return createPortal(<div className="shortcut-panel-overlay" onPointerDown={(event) => {
    if (event.target === event.currentTarget) onClose()
  }}>
    <section className={`shortcut-panel theme-${themeKey}`} role="dialog" aria-modal="true"
      aria-labelledby="shortcut-panel-title">
      <header className="shortcut-panel__header">
        <h2 id="shortcut-panel-title">快捷键列表</h2>
        <button aria-label="关闭快捷键列表" onClick={onClose}>×</button>
      </header>
      <div className="shortcut-panel__content">
        <img src={source} alt="快捷键说明" data-theme={themeKey} />
      </div>
    </section>
  </div>, document.body)
}
