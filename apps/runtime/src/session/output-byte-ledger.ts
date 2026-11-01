const DEFAULT_ENTRY_LIMIT = 8_192

interface LedgerEntry {
  sequence: number
  startBytes: number
}

/**
 * Remembers how many output bytes each recent journal frame carried, so a
 * catch-up replay can tell a short backlog from one large enough that the
 * live screen snapshot paints faster than re-parsing every frame.
 */
export class OutputByteLedger {
  readonly #entryLimit: number
  #entries: LedgerEntry[] = []
  #totalBytes = 0

  constructor(entryLimit = DEFAULT_ENTRY_LIMIT) {
    if (!Number.isSafeInteger(entryLimit) || entryLimit < 1) {
      throw new RangeError('entryLimit must be a positive integer')
    }
    this.#entryLimit = entryLimit
  }

  record(sequence: number, byteLength: number): void {
    this.#entries.push({ sequence, startBytes: this.#totalBytes })
    this.#totalBytes += byteLength
    // Trim in bulk so the steady state stays amortized O(1) per frame.
    if (this.#entries.length >= this.#entryLimit * 2) {
      this.#entries = this.#entries.slice(-this.#entryLimit)
    }
  }

  /** Output bytes recorded at or after `sequence`; Infinity when that range predates the ledger. */
  bytesSince(sequence: number): number {
    const entries = this.#entries
    const last = entries.at(-1)
    if (!last || sequence > last.sequence) return 0
    if (sequence < entries[0]!.sequence) return Number.POSITIVE_INFINITY
    let low = 0
    let high = entries.length - 1
    while (low < high) {
      const middle = (low + high) >> 1
      if (entries[middle]!.sequence < sequence) low = middle + 1
      else high = middle
    }
    return this.#totalBytes - entries[low]!.startBytes
  }
}
