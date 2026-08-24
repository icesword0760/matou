export interface CreditWindowOptions {
  highWatermarkBytes: number
  lowWatermarkBytes: number
  onPause?: () => void
  onResume?: () => void
}

export class CreditWindow {
  readonly #highWatermarkBytes: number
  readonly #lowWatermarkBytes: number
  readonly #onPause: () => void
  readonly #onResume: () => void
  readonly #pendingBytes = new Map<number, number>()

  #latestSentSequence = -1
  #latestAcknowledgedSequence = -1
  #unackedBytes = 0
  #paused = false

  constructor(options: CreditWindowOptions) {
    if (options.lowWatermarkBytes < 0 || options.highWatermarkBytes <= options.lowWatermarkBytes) {
      throw new RangeError('high watermark must be greater than low watermark')
    }

    this.#highWatermarkBytes = options.highWatermarkBytes
    this.#lowWatermarkBytes = options.lowWatermarkBytes
    this.#onPause = options.onPause ?? (() => undefined)
    this.#onResume = options.onResume ?? (() => undefined)
  }

  get unackedBytes(): number {
    return this.#unackedBytes
  }

  get isPaused(): boolean {
    return this.#paused
  }

  recordSent(sequence: number, bytes: number): void {
    if (!Number.isInteger(sequence) || sequence <= this.#latestSentSequence) {
      throw new RangeError('sent sequence must increase monotonically')
    }
    if (!Number.isInteger(bytes) || bytes < 0) {
      throw new RangeError('sent bytes must be a non-negative integer')
    }

    this.#latestSentSequence = sequence
    this.#pendingBytes.set(sequence, bytes)
    this.#unackedBytes += bytes

    if (!this.#paused && this.#unackedBytes > this.#highWatermarkBytes) {
      this.#paused = true
      this.#onPause()
    }
  }

  acknowledge(throughSequence: number): void {
    if (!Number.isInteger(throughSequence) || throughSequence < 0) {
      throw new RangeError('acknowledged sequence must be a non-negative integer')
    }
    if (throughSequence > this.#latestSentSequence) {
      throw new RangeError('acknowledgement exceeds latest sent sequence')
    }
    if (throughSequence <= this.#latestAcknowledgedSequence) {
      return
    }

    for (const [sequence, bytes] of this.#pendingBytes) {
      if (sequence > throughSequence) {
        break
      }
      this.#unackedBytes -= bytes
      this.#pendingBytes.delete(sequence)
    }
    this.#latestAcknowledgedSequence = throughSequence

    if (this.#paused && this.#unackedBytes <= this.#lowWatermarkBytes) {
      this.#paused = false
      this.#onResume()
    }
  }
}
