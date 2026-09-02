const RESUME_FAILURE_PATTERNS = [
  /no session found/i,
  /session not found/i,
  /no conversation found/i,
  /invalid session/i,
  /error[^\r\n]*resume/i,
  /failed to resume/i
]

export class ProviderResumeMonitor {
  #recentOutput = ''
  #failed = false
  #settled = false

  get isMonitoring(): boolean {
    return !this.#failed && !this.#settled
  }

  get isSettled(): boolean {
    return this.#settled
  }

  ingest(data: string): string | undefined {
    if (this.#failed || this.#settled) return undefined
    this.#recentOutput = normalizeProviderOutput(`${this.#recentOutput}${data}`)
      .slice(-8_192)
    if (this.#recentOutput.length > 2_000) {
      this.#settled = true
      this.#recentOutput = ''
      return undefined
    }
    if (!RESUME_FAILURE_PATTERNS.some((pattern) => pattern.test(this.#recentOutput))) {
      return undefined
    }
    this.#failed = true
    return 'provider session not found'
  }

  timeout(): string | undefined {
    if (!this.isMonitoring) return undefined
    this.#failed = true
    return 'provider resume timed out'
  }
}

function normalizeProviderOutput(output: string): string {
  return output
    // Current Claude Code paints failure sentences by moving the cursor to each
    // word column. Preserve a word boundary for horizontal cursor movement
    // instead of joining "No" and "conversation" while stripping ANSI.
    .replace(/\u001b\[[0-?]*[ -/]*[CG]/g, ' ')
    .replace(/\u001b\[[0-?]*[ -/]*[@-~]/g, '')
    .replace(/[\u0000-\u001f\u007f\s]+/g, ' ')
}
