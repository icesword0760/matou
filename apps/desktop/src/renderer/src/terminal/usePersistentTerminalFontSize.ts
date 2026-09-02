import { useCallback, useEffect, useState, type Dispatch, type SetStateAction } from 'react'

export const TERMINAL_FONT_SIZE_STORAGE_KEY = 'matou:terminal-font-size'
export const DEFAULT_TERMINAL_FONT_SIZE = 11
export const MIN_TERMINAL_FONT_SIZE = 10
export const MAX_TERMINAL_FONT_SIZE = 24

export function normalizeTerminalFontSize(value: unknown): number {
  const numeric = typeof value === 'number' ? value : Number(value)
  if (!Number.isFinite(numeric)) return DEFAULT_TERMINAL_FONT_SIZE
  return Math.min(MAX_TERMINAL_FONT_SIZE, Math.max(MIN_TERMINAL_FONT_SIZE, Math.round(numeric)))
}

export function readTerminalFontSize(): number {
  try {
    const stored = window.localStorage.getItem(TERMINAL_FONT_SIZE_STORAGE_KEY)
    return stored === null ? DEFAULT_TERMINAL_FONT_SIZE : normalizeTerminalFontSize(stored)
  } catch {
    return DEFAULT_TERMINAL_FONT_SIZE
  }
}

export function writeTerminalFontSize(value: number): number {
  const normalized = normalizeTerminalFontSize(value)
  try {
    window.localStorage.setItem(TERMINAL_FONT_SIZE_STORAGE_KEY, String(normalized))
  } catch {
    // Keep the current window usable when browser storage is unavailable.
  }
  return normalized
}

export function usePersistentTerminalFontSize(): [number, Dispatch<SetStateAction<number>>] {
  const [fontSize, setFontSizeState] = useState(readTerminalFontSize)
  const setFontSize = useCallback<Dispatch<SetStateAction<number>>>((next) => {
    setFontSizeState((current) => {
      const value = normalizeTerminalFontSize(typeof next === 'function' ? next(current) : next)
      writeTerminalFontSize(value)
      return value
    })
  }, [])

  useEffect(() => {
    const syncFromStorage = (event: StorageEvent) => {
      if (event.key === TERMINAL_FONT_SIZE_STORAGE_KEY) setFontSizeState(readTerminalFontSize())
    }
    window.addEventListener('storage', syncFromStorage)
    return () => window.removeEventListener('storage', syncFromStorage)
  }, [])

  return [fontSize, setFontSize]
}
