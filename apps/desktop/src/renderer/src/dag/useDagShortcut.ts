import { useEffect, useRef } from 'react'

interface ShortcutEvent {
  key: string
  altKey: boolean
  repeat: boolean
  preventDefault(): void
  stopPropagation(): void
}

export class DagShortcutController {
  readonly #open: () => void

  constructor(input: { open(): void }) {
    this.#open = input.open
  }

  keyDown(event: ShortcutEvent): boolean {
    if (event.key !== 'Tab' || !event.altKey) return false
    event.preventDefault()
    event.stopPropagation()
    if (!event.repeat) this.#open()
    return true
  }

  keyUp(event: ShortcutEvent): boolean {
    if (event.key !== 'Tab' || !event.altKey) return false
    event.preventDefault()
    event.stopPropagation()
    return true
  }

  cancel(): void {}
}

export function useDagShortcut(input: {
  enabled: boolean
  onPress(): void
}): void {
  const callbacks = useRef(input)
  callbacks.current = input
  useEffect(() => {
    if (!input.enabled) return
    const controller = new DagShortcutController({
      open: () => callbacks.current.onPress()
    })
    const down = (event: KeyboardEvent) => { controller.keyDown(event) }
    const up = (event: KeyboardEvent) => { controller.keyUp(event) }
    window.addEventListener('keydown', down, true)
    window.addEventListener('keyup', up, true)
    return () => {
      controller.cancel()
      window.removeEventListener('keydown', down, true)
      window.removeEventListener('keyup', up, true)
    }
  }, [input.enabled])
}
