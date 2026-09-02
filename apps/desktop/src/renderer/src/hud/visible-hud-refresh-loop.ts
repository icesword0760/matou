export class VisibleHudRefreshLoop {
  #timer: ReturnType<typeof setInterval> | undefined

  constructor(
    private readonly refresh: () => void,
    private readonly isVisible: () => boolean,
    private readonly intervalMs = 2_000
  ) {}

  start(): void {
    if (this.#timer) return
    this.#refreshIfVisible()
    this.#timer = setInterval(() => this.#refreshIfVisible(), this.intervalMs)
  }

  focus(): void {
    this.#refreshIfVisible()
  }

  visibilityChanged(): void {
    this.#refreshIfVisible()
  }

  stop(): void {
    if (this.#timer) clearInterval(this.#timer)
    this.#timer = undefined
  }

  #refreshIfVisible(): void {
    if (this.isVisible()) this.refresh()
  }
}
