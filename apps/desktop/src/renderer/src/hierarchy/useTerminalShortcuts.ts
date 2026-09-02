import { useEffect, useRef } from 'react'

export interface TerminalShortcutHandlers {
  splitHorizontal?(): void
  splitVertical?(): void
  nextPane?(): void
  prevPane?(): void
  switchPaneByDirection?(direction: 'up' | 'down' | 'left' | 'right'): void
  closePane?(): void
  newTab?(): void
  nextTab?(): void
  prevTab?(): void
  jumpToTab?(index: number): void
  moveTabPosition?(direction: 'left' | 'right'): void
  openSearch?(): void
  increaseFontSize?(): void
  decreaseFontSize?(): void
  resetFontSize?(): void
  cycleTheme?(): void
  toggleShortcutPanel?(): void
}

export function useTerminalShortcuts(handlers: TerminalShortcutHandlers): boolean {
  const isMac = typeof navigator !== 'undefined' &&
    (/Mac/.test(navigator.platform ?? '') || /Mac/.test(navigator.userAgent ?? ''))
  const lastAltKeyDownTime = useRef(0)
  useEffect(() => {
    const keydown = (event: KeyboardEvent) => {
      const standaloneAlt = ['Alt', 'AltLeft', 'AltRight'].includes(event.key) && event.altKey &&
        !event.ctrlKey && !event.metaKey && !event.shiftKey
      if (standaloneAlt) {
        const now = Date.now()
        if (now - lastAltKeyDownTime.current < 300) {
          handlers.toggleShortcutPanel?.()
          lastAltKeyDownTime.current = 0
          event.preventDefault()
          event.stopPropagation()
        } else {
          lastAltKeyDownTime.current = now
        }
        return
      }

      const modKey = isMac ? event.metaKey : event.ctrlKey
      if (!modKey && !event.altKey) return
      const key = event.key
      const lower = key.toLowerCase()
      let handled = true
      if (modKey && event.shiftKey && !event.altKey && lower === 'd') handlers.splitVertical?.()
      else if (modKey && !event.shiftKey && !event.altKey && lower === 'd') handlers.splitHorizontal?.()
      else if (modKey && !event.shiftKey && !event.altKey && key === ']') handlers.nextPane?.()
      else if (modKey && !event.shiftKey && !event.altKey && key === '[') handlers.prevPane?.()
      else if (modKey && event.altKey && !event.shiftKey && key.startsWith('Arrow')) {
        handlers.switchPaneByDirection?.(key.slice(5).toLowerCase() as 'up' | 'down' | 'left' | 'right')
      } else if (modKey && !event.shiftKey && !event.altKey && lower === 'w') handlers.closePane?.()
      else if (modKey && !event.shiftKey && !event.altKey && lower === 't') handlers.newTab?.()
      else if ((modKey && event.shiftKey && !event.altKey && (key === ']' || key === '}')) ||
        (!isMac && event.ctrlKey && !event.shiftKey && !event.altKey && key === 'Tab')) handlers.nextTab?.()
      else if ((modKey && event.shiftKey && !event.altKey && (key === '[' || key === '{')) ||
        (!isMac && event.ctrlKey && event.shiftKey && !event.altKey && key === 'Tab')) handlers.prevTab?.()
      else if (modKey && !event.shiftKey && !event.altKey && /^[1-9]$/.test(key)) handlers.jumpToTab?.(Number(key) - 1)
      else if (modKey && event.shiftKey && !event.altKey && (key === 'ArrowLeft' || key === 'PageUp')) handlers.moveTabPosition?.('left')
      else if (modKey && event.shiftKey && !event.altKey && (key === 'ArrowRight' || key === 'PageDown')) handlers.moveTabPosition?.('right')
      else if (modKey && !event.shiftKey && !event.altKey && lower === 'f') handlers.openSearch?.()
      else if (modKey && !event.altKey && (key === '=' || key === '+')) handlers.increaseFontSize?.()
      else if (modKey && !event.altKey && (key === '-' || key === '_')) handlers.decreaseFontSize?.()
      else if (modKey && !event.shiftKey && !event.altKey && key === '0') handlers.resetFontSize?.()
      else if (modKey && !event.shiftKey && !event.altKey && lower === 'i') handlers.cycleTheme?.()
      else if (modKey && !event.shiftKey && !event.altKey && (key === '/' || key === '?')) handlers.toggleShortcutPanel?.()
      else handled = false

      if (handled) {
        event.preventDefault()
        event.stopPropagation()
      }
    }
    document.addEventListener('keydown', keydown, true)
    return () => document.removeEventListener('keydown', keydown, true)
  }, [handlers, isMac])
  return isMac
}
