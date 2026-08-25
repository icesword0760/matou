const OSC_7 = /\u001b\]7;([^\u0007\u001b]*)(?:\u0007|\u001b\\)/g

export class TerminalCwdTracker {
  #buffer = ''

  ingest(chunk: string): string | undefined {
    this.#buffer = (this.#buffer + chunk).slice(-4096)
    let cwd: string | undefined
    for (const match of this.#buffer.matchAll(OSC_7)) {
      try {
        const url = new URL(match[1]!)
        if (url.protocol === 'file:') cwd = decodeURIComponent(url.pathname)
      } catch {
        // A malformed shell integration frame is ordinary terminal output.
      }
    }
    const lastTerminator = Math.max(
      this.#buffer.lastIndexOf('\u0007'),
      this.#buffer.lastIndexOf('\u001b\\')
    )
    if (lastTerminator >= 0) this.#buffer = this.#buffer.slice(lastTerminator + 1)
    return cwd
  }
}
