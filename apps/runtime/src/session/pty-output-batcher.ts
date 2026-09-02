const DEFAULT_DELAY_MS = 16
const DEFAULT_MAX_CODE_UNITS = 64 * 1024

/**
 * Batches adjacent node-pty data callbacks before assigning a Journal frame.
 * PTY callback boundaries are transport details rather than terminal
 * semantics; preserving their byte order while reducing tiny fs writes keeps
 * sustained multi-session output durable without building a memory backlog.
 */
export class PtyOutputBatcher {
  readonly #emit: (data: string) => void
  readonly #delayMs: number
  readonly #maxCodeUnits: number
  #chunks: string[] = []
  #codeUnits = 0
  #timer: ReturnType<typeof setTimeout> | undefined

  constructor(
    emit: (data: string) => void,
    options: { delayMs?: number; maxCodeUnits?: number } = {}
  ) {
    this.#emit = emit
    this.#delayMs = positiveInteger(options.delayMs, DEFAULT_DELAY_MS, 'delayMs')
    this.#maxCodeUnits = positiveInteger(
      options.maxCodeUnits,
      DEFAULT_MAX_CODE_UNITS,
      'maxCodeUnits'
    )
  }

  offer(data: string): void {
    if (data.length === 0) return
    this.#chunks.push(data)
    this.#codeUnits += data.length
    if (this.#codeUnits >= this.#maxCodeUnits) {
      this.flush()
      return
    }
    if (this.#timer !== undefined) return
    this.#timer = setTimeout(() => {
      this.#timer = undefined
      this.flush()
    }, this.#delayMs)
    this.#timer.unref?.()
  }

  flush(): void {
    if (this.#timer !== undefined) clearTimeout(this.#timer)
    this.#timer = undefined
    if (this.#chunks.length === 0) return
    const data = this.#chunks.length === 1 ? this.#chunks[0]! : this.#chunks.join('')
    this.#chunks = []
    this.#codeUnits = 0
    this.#emit(data)
  }
}

function positiveInteger(value: number | undefined, fallback: number, label: string): number {
  const effective = value ?? fallback
  if (!Number.isSafeInteger(effective) || effective < 1) {
    throw new RangeError(`${label} must be a positive integer`)
  }
  return effective
}
