import { useEffect, useRef } from 'react'

interface ShortcutEvent {
  key: string
  altKey: boolean
  repeat: boolean
  preventDefault(): void
  stopPropagation(): void
}

export class DagShortcutController {
  readonly #shortPress: () => void
  readonly #longPress: () => void
  readonly #holdDuration: number
  #timer: ReturnType<typeof setTimeout> | undefined
  #pending = false
  #opened = false
  #startedAt = 0

  constructor(input: { shortPress(): void; longPress(): void; holdDuration?: number }) {
    this.#shortPress = input.shortPress
    this.#longPress = input.longPress
    this.#holdDuration = clampDagHoldDuration(input.holdDuration ?? 450)
  }

  keyDown(event: ShortcutEvent): boolean {
    if (event.key !== 'Tab' || !event.altKey) return false
    event.preventDefault()
    event.stopPropagation()
    if (event.repeat) {
      if (this.#pending && !this.#opened && Date.now() - this.#startedAt >= this.#holdDuration) {
        this.#open()
      }
      return true
    }
    if (this.#pending || this.#opened) return true
    this.#pending = true
    this.#startedAt = Date.now()
    this.#timer = setTimeout(() => {
      this.#timer = undefined
      if (!this.#pending) return
      this.#open()
    }, this.#holdDuration)
    return true
  }

  keyUp(event: ShortcutEvent): boolean {
    if (event.key !== 'Tab' || (!event.altKey && !this.#pending && !this.#opened)) return false
    event.preventDefault()
    event.stopPropagation()
    const forward = this.#pending && !this.#opened
    this.#clearTimer()
    this.#pending = false
    this.#opened = false
    this.#startedAt = 0
    if (forward) this.#shortPress()
    return true
  }

  cancel(): void {
    this.#clearTimer()
    this.#pending = false
    this.#opened = false
    this.#startedAt = 0
  }

  #open(): void {
    this.#clearTimer()
    this.#opened = true
    this.#longPress()
  }

  #clearTimer(): void {
    if (this.#timer !== undefined) clearTimeout(this.#timer)
    this.#timer = undefined
  }
}

export function useDagShortcut(input: {
  enabled: boolean
  onShortPress(): void
  onLongPress(): void
  holdDuration?: number
}): void {
  const callbacks = useRef(input)
  callbacks.current = input
  useEffect(() => {
    if (!input.enabled) return
    const controller = new DagShortcutController({
      ...(input.holdDuration === undefined ? {} : { holdDuration: input.holdDuration }),
      shortPress: () => callbacks.current.onShortPress(),
      longPress: () => callbacks.current.onLongPress()
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
  }, [input.enabled, input.holdDuration])
}

export function clampDagHoldDuration(value: number): number {
  return Math.max(350, Math.min(800, Math.round(value)))
}
