import { Terminal } from '@xterm/headless'

export interface TerminalScreenSnapshot {
  text: string
  cols: number
  rows: number
}

export class TerminalScreenProjector {
  readonly #terminal: Terminal
  #chain = Promise.resolve()

  constructor(cols = 80, rows = 24) {
    this.#terminal = new Terminal({ cols, rows, allowProposedApi: true })
  }

  write(data: string): Promise<void> {
    this.#chain = this.#chain.then(() => new Promise<void>((resolve) => {
      this.#terminal.write(data, resolve)
    }))
    return this.#chain
  }

  resize(cols: number, rows: number): Promise<void> {
    this.#chain = this.#chain.then(() => {
      this.#terminal.resize(cols, rows)
    })
    return this.#chain
  }

  reset(): Promise<void> {
    this.#chain = this.#chain.then(() => this.#terminal.reset())
    return this.#chain
  }

  async snapshot(): Promise<TerminalScreenSnapshot> {
    await this.#chain
    const buffer = this.#terminal.buffer.active
    const lines: string[] = []
    for (let row = 0; row < this.#terminal.rows; row += 1) {
      lines.push(buffer.getLine(buffer.baseY + row)?.translateToString(true) ?? '')
    }
    while (lines.at(-1) === '') lines.pop()
    return {
      text: lines.join('\n'),
      cols: this.#terminal.cols,
      rows: this.#terminal.rows
    }
  }

  dispose(): void {
    this.#terminal.dispose()
  }
}
