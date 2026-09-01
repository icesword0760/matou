const MAX_RESIZE_HZ = 60
const MIN_SEND_INTERVAL_MS = 1_000 / MAX_RESIZE_HZ

interface TerminalDimensions {
  cols: number
  rows: number
}

export class ResizeCoalescer {
  readonly #send: (cols: number, rows: number) => void
  #pending: TerminalDimensions | undefined
  #lastSent: TerminalDimensions | undefined
  #lastSentAt = Number.NEGATIVE_INFINITY
  #frameId: number | undefined
  #disposed = false

  constructor(send: (cols: number, rows: number) => void) {
    this.#send = send
  }

  offer(cols: number, rows: number): void {
    if (this.#disposed) return
    this.#pending = { cols, rows }
    this.#schedule()
  }

  flush(): void {
    if (this.#disposed) return
    this.#cancelFrame()
    this.#sendPending()
  }

  dispose(): void {
    if (this.#disposed) return
    this.#disposed = true
    this.#pending = undefined
    this.#cancelFrame()
  }

  #schedule(): void {
    if (this.#frameId !== undefined) return
    this.#frameId = requestAnimationFrame((timestamp) => this.#onFrame(timestamp))
  }

  #onFrame(timestamp: number): void {
    this.#frameId = undefined
    if (this.#disposed || !this.#pending) return
    if (timestamp - this.#lastSentAt < MIN_SEND_INTERVAL_MS) {
      this.#schedule()
      return
    }
    this.#sendPending(timestamp)
  }

  #sendPending(timestamp = performance.now()): void {
    const pending = this.#pending
    this.#pending = undefined
    if (!pending || sameDimensions(pending, this.#lastSent)) return
    this.#send(pending.cols, pending.rows)
    this.#lastSent = pending
    this.#lastSentAt = timestamp
  }

  #cancelFrame(): void {
    if (this.#frameId === undefined) return
    cancelAnimationFrame(this.#frameId)
    this.#frameId = undefined
  }
}

function sameDimensions(
  left: TerminalDimensions,
  right: TerminalDimensions | undefined
): boolean {
  return right !== undefined && left.cols === right.cols && left.rows === right.rows
}
