export class StorageQueue {
  #tail: Promise<unknown> = Promise.resolve()
  #closed = false

  enqueue<T>(operation: () => T | Promise<T>): Promise<T> {
    if (this.#closed) {
      return Promise.reject(new Error('storage queue is closed'))
    }

    const result = this.#tail.then(operation, operation)
    this.#tail = result.then(
      () => undefined,
      () => undefined
    )
    return result
  }

  async drain(): Promise<void> {
    await this.#tail
  }

  close(): void {
    this.#closed = true
  }
}
