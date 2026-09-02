const DEFAULT_DELAY_MS = 100
const DEFAULT_MAX_BYTES = 256 * 1024

/**
 * Coalesces output for foreground terminals that are outside the viewport.
 * Their VT models stay live, but painting every PTY frame into hidden xterm
 * DOM would make the card the user is actually viewing stutter. The bounded
 * delay also keeps Runtime ACK credit comfortably below its high watermark.
 */
export class TerminalOutputCoalescer {
  readonly #write: (data: Uint8Array, throughSequence: number) => void
  readonly #delayMs: number
  readonly #maxBytes: number
  #chunks: Uint8Array[] = []
  #bytes = 0
  #throughSequence = 0
  #timer: ReturnType<typeof setTimeout> | undefined

  constructor(
    write: (data: Uint8Array, throughSequence: number) => void,
    options: { delayMs?: number; maxBytes?: number } = {}
  ) {
    this.#write = write
    this.#delayMs = positiveInteger(options.delayMs, DEFAULT_DELAY_MS, 'delayMs')
    this.#maxBytes = positiveInteger(options.maxBytes, DEFAULT_MAX_BYTES, 'maxBytes')
  }

  offer(data: Uint8Array, sequence: number, immediate: boolean): void {
    if (data.byteLength === 0) return
    if (immediate) {
      this.flush()
      this.#write(data, sequence)
      return
    }
    this.#chunks.push(data)
    this.#bytes += data.byteLength
    this.#throughSequence = sequence
    if (this.#bytes >= this.#maxBytes) {
      this.flush()
      return
    }
    this.#timer ??= setTimeout(() => {
      this.#timer = undefined
      this.flush()
    }, this.#delayMs)
  }

  flush(): void {
    if (this.#timer !== undefined) clearTimeout(this.#timer)
    this.#timer = undefined
    if (this.#chunks.length === 0) return
    const data = this.#chunks.length === 1
      ? this.#chunks[0]!
      : concatenate(this.#chunks, this.#bytes)
    const sequence = this.#throughSequence
    this.#chunks = []
    this.#bytes = 0
    this.#throughSequence = 0
    this.#write(data, sequence)
  }

  dispose(): void {
    this.flush()
  }
}

function concatenate(chunks: readonly Uint8Array[], bytes: number): Uint8Array {
  const joined = new Uint8Array(bytes)
  let offset = 0
  for (const chunk of chunks) {
    joined.set(chunk, offset)
    offset += chunk.byteLength
  }
  return joined
}

function positiveInteger(value: number | undefined, fallback: number, label: string): number {
  const effective = value ?? fallback
  if (!Number.isSafeInteger(effective) || effective < 1) {
    throw new RangeError(`${label} must be a positive integer`)
  }
  return effective
}
