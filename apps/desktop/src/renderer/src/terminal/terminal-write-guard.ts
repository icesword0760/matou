interface TerminalWriter {
  write(data: string | Uint8Array, callback?: () => void): void
}

/** Detects both thrown writes and xterm's asynchronously stalled parser queue. */
export class TerminalWriteGuard {
  #pending = 0
  #failed = false
  #disposed = false
  #timer: ReturnType<typeof setTimeout> | undefined

  constructor(
    private readonly terminal: TerminalWriter,
    private readonly onFailure: () => void,
    private readonly timeoutMs = 5_000
  ) {}

  get healthy(): boolean { return !this.#failed && !this.#disposed }

  write(data: string | Uint8Array, callback?: () => void): void {
    if (!this.healthy) return
    this.#pending += 1
    // Further incoming output must not postpone an already stalled write.
    if (this.#timer === undefined) this.#watch()
    try {
      this.terminal.write(data, () => {
        if (!this.healthy) return
        this.#pending -= 1
        this.#clearTimer()
        if (this.#pending > 0) this.#watch()
        try {
          callback?.()
        } catch {
          // A resize/reset callback can throw inside xterm's write pump too.
          this.#fail()
        }
      })
    } catch {
      this.#fail()
    }
  }

  dispose(): void {
    this.#disposed = true
    this.#clearTimer()
  }

  #watch(): void {
    this.#timer = setTimeout(() => this.#fail(), this.timeoutMs)
  }

  #clearTimer(): void {
    if (this.#timer !== undefined) clearTimeout(this.#timer)
    this.#timer = undefined
  }

  #fail(): void {
    if (!this.healthy) return
    this.#failed = true
    this.#clearTimer()
    this.onFailure()
  }
}
