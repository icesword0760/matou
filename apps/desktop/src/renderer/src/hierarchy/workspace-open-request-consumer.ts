export class WorkspaceOpenRequestConsumer {
  #active: Promise<void> | undefined

  constructor(
    private readonly read: () => Promise<string[]>,
    private readonly open: (path: string) => Promise<void>
  ) {}

  drain(): Promise<void> {
    if (this.#active) return this.#active
    const current = this.#drainAll().finally(() => {
      if (this.#active === current) this.#active = undefined
    })
    this.#active = current
    return current
  }

  async #drainAll(): Promise<void> {
    while (true) {
      const paths = await this.read()
      if (paths.length === 0) return
      for (const path of paths) await this.open(path)
    }
  }
}
