const RESUME_FAILURE_PATTERNS = [
  /no session.{0,96}found/i,
  /session.{0,96}not found/i,
  /no conversation.{0,96}found/i,
  /invalid session/i,
  /failed to resume/i
]

const CLAUDE_FULL_RESUME_PROMPT_MARKERS = [
  'resuming the full session will consume',
  'resume from summary',
  'resume full session as-is',
  "don't ask me again",
  'enter to confirm'
]

const CLAUDE_WORKSPACE_TRUST_PROMPT_MARKERS = [
  'accessing workspace:',
  'quick safety check:',
  "claude code'll be able to read, edit, and execute files here.",
  'no, exit',
  'yes, i trust this folder',
  'enter to confirm'
]

export class ClaudeFullResumePromptMonitor {
  #recentOutput = ''
  #handled = false

  ingest(data: string): boolean {
    if (this.#handled) return false
    this.#recentOutput = normalizeProviderOutput(`${this.#recentOutput}${data}`)
      .toLowerCase()
      .slice(-8_192)
    if (!CLAUDE_FULL_RESUME_PROMPT_MARKERS.every((marker) => this.#recentOutput.includes(marker))) {
      return false
    }
    this.#handled = true
    this.#recentOutput = ''
    return true
  }
}

export class ClaudeWorkspaceTrustPromptMonitor {
  #recentOutput = ''
  #handled = false

  ingest(data: string): boolean {
    if (this.#handled) return false
    this.#recentOutput = normalizeProviderOutput(`${this.#recentOutput}${data}`)
      .toLowerCase()
      .slice(-8_192)
    if (!CLAUDE_WORKSPACE_TRUST_PROMPT_MARKERS.every(
      (marker) => this.#recentOutput.includes(marker)
    )) {
      return false
    }
    this.#handled = true
    this.#recentOutput = ''
    return true
  }
}

export class ProviderResumeMonitor {
  readonly #expectedProviderSessionId: string
  #recentOutput = ''
  #failed = false
  #settled = false

  constructor(expectedProviderSessionId: string) {
    this.#expectedProviderSessionId = expectedProviderSessionId.trim().toLowerCase()
  }

  get isMonitoring(): boolean {
    return !this.#failed && !this.#settled
  }

  get isSettled(): boolean {
    return this.#settled
  }

  get hasVisibleOutput(): boolean {
    return this.#recentOutput.length > 0
  }

  ingest(data: string): string | undefined {
    if (this.#failed || this.#settled) return undefined
    this.#recentOutput = normalizeProviderOutput(`${this.#recentOutput}${data}`)
      .slice(-8_192)
    if (matchesExpectedResumeFailure(this.#recentOutput, this.#expectedProviderSessionId)) {
      this.#failed = true
      return 'provider session not found'
    }
    if (this.#recentOutput.length > 2_000) {
      this.#settled = true
      this.#recentOutput = ''
    }
    return undefined
  }

  timeout(): string | undefined {
    if (!this.isMonitoring) return undefined
    // Claude may pause on an interactive startup screen (for example, the
    // workspace trust confirmation) before it can emit the identity hook.
    // Visible provider text proves the PTY is live and must remain available
    // for the user's answer; only a completely silent launch is unresponsive.
    if (this.#recentOutput.length > 0) {
      this.#settled = true
      this.#recentOutput = ''
      return undefined
    }
    this.#failed = true
    return 'provider resume timed out'
  }
}

function matchesExpectedResumeFailure(output: string, expectedProviderSessionId: string): boolean {
  if (!expectedProviderSessionId) return false
  const normalized = output.toLowerCase()
  const identityAt = normalized.lastIndexOf(expectedProviderSessionId)
  if (identityAt < 0) return false
  const context = normalized.slice(
    Math.max(0, identityAt - 320),
    Math.min(normalized.length, identityAt + expectedProviderSessionId.length + 320)
  )
  return RESUME_FAILURE_PATTERNS.some((pattern) => pattern.test(context))
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
