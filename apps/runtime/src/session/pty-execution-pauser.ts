export interface PausablePtyExecution {
  readonly pid: number
  pause(): void
  resume(): void
}

export interface PtyExecutionPauserOptions {
  platform?: NodeJS.Platform
  signalProcessGroup?: (pid: number, signal: 'SIGSTOP' | 'SIGCONT') => void
}

/**
 * Applies both sides of PTY backpressure. Pausing the readable stream first
 * bounds user-space output immediately; stopping the process group then keeps
 * providers and their descendants from filling the kernel PTY buffer.
 */
export class PtyExecutionPauser {
  readonly #terminal: PausablePtyExecution
  readonly #platform: NodeJS.Platform
  readonly #signalProcessGroup: (pid: number, signal: 'SIGSTOP' | 'SIGCONT') => void
  #paused = false

  constructor(terminal: PausablePtyExecution, options: PtyExecutionPauserOptions = {}) {
    this.#terminal = terminal
    this.#platform = options.platform ?? process.platform
    this.#signalProcessGroup = options.signalProcessGroup ?? ((pid, signal) => {
      process.kill(-pid, signal)
    })
  }

  get isPaused(): boolean { return this.#paused }

  async pause(): Promise<void> {
    if (this.#paused) return
    this.#terminal.pause()
    this.#paused = true
    if (this.#platform === 'win32') return
    try {
      this.#signalProcessGroup(this.#terminal.pid, 'SIGSTOP')
    } catch (error) {
      if (!isMissingProcess(error)) throw error
    }
  }

  async resume(): Promise<void> {
    if (!this.#paused) return
    if (this.#platform !== 'win32') {
      try {
        this.#signalProcessGroup(this.#terminal.pid, 'SIGCONT')
      } catch (error) {
        if (!isMissingProcess(error)) throw error
      }
    }
    this.#terminal.resume()
    this.#paused = false
  }
}

function isMissingProcess(error: unknown): boolean {
  return (error as NodeJS.ErrnoException | undefined)?.code === 'ESRCH'
}
