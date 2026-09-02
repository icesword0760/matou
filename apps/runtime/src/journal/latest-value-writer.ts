/**
 * Serializes sidecar writes while retaining only the newest value waiting
 * behind the in-flight write. Journal frames remain authoritative, so writing
 * every intermediate index snapshot would add fsync pressure and retain a
 * growing queue of large arrays without improving recovery correctness.
 */
export class LatestValueWriter<T> {
  readonly #write: (value: T) => Promise<void>
  #pending: T | undefined
  #running: Promise<void> | undefined

  constructor(write: (value: T) => Promise<void>) {
    this.#write = write
  }

  schedule(value: T): void {
    this.#pending = value
    this.#running ??= this.#drain().finally(() => {
      this.#running = undefined
    })
  }

  async whenIdle(): Promise<void> {
    while (this.#running) await this.#running
  }

  async #drain(): Promise<void> {
    while (this.#pending !== undefined) {
      const value = this.#pending
      this.#pending = undefined
      try {
        await this.#write(value)
      } catch {
        // The journal is the authority and startup can rebuild this sidecar.
        // A newer scheduled snapshot still gets a chance to repair it.
      }
    }
  }
}
