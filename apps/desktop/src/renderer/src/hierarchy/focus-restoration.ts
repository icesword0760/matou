export type AnimationFrameScheduler = (callback: FrameRequestCallback) => number
export type AnimationFrameCanceller = (handle: number) => void

/**
 * Remembers the user's last concrete control instead of treating window focus
 * as a request to move them back into the terminal.
 */
export class AppFocusRestorer {
  #lastFocused: HTMLElement | null = null
  #pendingFrame: number | undefined
  readonly #requestFrame: AnimationFrameScheduler
  readonly #cancelFrame: AnimationFrameCanceller

  constructor(
    requestFrame: AnimationFrameScheduler = requestAnimationFrame,
    cancelFrame: AnimationFrameCanceller = cancelAnimationFrame
  ) {
    this.#requestFrame = requestFrame
    this.#cancelFrame = cancelFrame
  }

  remember(target: EventTarget | null): void {
    if (!(target instanceof HTMLElement) || !meaningfulFocusTarget(target)) return
    this.#lastFocused = target
  }

  scheduleRestore(fallback: () => void): void {
    if (this.#pendingFrame !== undefined) return
    this.#pendingFrame = this.#requestFrame(() => {
      this.#pendingFrame = undefined
      const target = this.#lastFocused
      if (target && restorableFocusTarget(target)) {
        target.focus({ preventScroll: true })
        return
      }
      this.#lastFocused = null
      fallback()
    })
  }

  dispose(): void {
    if (this.#pendingFrame !== undefined) this.#cancelFrame(this.#pendingFrame)
    this.#pendingFrame = undefined
    this.#lastFocused = null
  }
}

export function meaningfulFocusTarget(target: HTMLElement): boolean {
  return target !== document.body && target !== document.documentElement
}

export function restorableFocusTarget(target: HTMLElement): boolean {
  if (!target.isConnected || !meaningfulFocusTarget(target)) return false
  if (target.hidden || target.closest('[hidden], [inert], [aria-hidden="true"]')) return false
  if ('disabled' in target && target.disabled === true) return false
  return true
}
