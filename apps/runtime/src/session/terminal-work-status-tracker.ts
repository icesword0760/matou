import type { SessionWorkStatus } from '@matou/domain'

const OSC_133 = /\u001b\]133;([CD])(?:;(-?\d+))?(?:\u0007|\u001b\\)/g

export class TerminalWorkStatusTracker {
  #buffer = ''
  readonly #provider: 'claude-code' | undefined
  #providerFailureEmitted = false

  constructor(options: { provider?: 'claude-code' } = {}) {
    this.#provider = options.provider
  }

  beginAttempt(): void {
    this.#providerFailureEmitted = false
    if (this.#provider) this.#buffer = ''
  }

  ingest(chunk: string): SessionWorkStatus[] {
    const combined = this.#buffer + chunk
    const statuses: SessionWorkStatus[] = []
    let consumedThrough = 0
    OSC_133.lastIndex = 0
    for (const match of combined.matchAll(OSC_133)) {
      consumedThrough = (match.index ?? 0) + match[0].length
      if (match[1] === 'C') {
        statuses.push('running')
        continue
      }
      const exitCode = Number(match[2] ?? '0')
      statuses.push(exitCode === 0
        ? 'idle'
        : exitCode === 130
          ? 'interrupted'
          : 'error')
    }
    this.#buffer = combined.slice(consumedThrough).slice(this.#provider ? -8_192 : -512)
    const completed = statuses.some((status) =>
      status === 'idle' || status === 'error' || status === 'interrupted'
    )
    if (!completed && isExplicitBlockingPrompt(this.#buffer)) statuses.push('needs-input')
    if (
      this.#provider === 'claude-code' &&
      !this.#providerFailureEmitted &&
      isTerminalClaudeFailure(this.#buffer)
    ) {
      this.#providerFailureEmitted = true
      statuses.push('error')
    }
    return statuses
  }
}

function isTerminalClaudeFailure(raw: string): boolean {
  const visible = visibleTerminalText(raw)
  return /(?:Connection refused|ConnectionRefused|ECONNREFUSED)[\s\S]{0,240}attempt\s*10\s*\/\s*10/i.test(visible) ||
    /(?:API Error|authentication failed|invalid api key|OAuth token (?:is )?(?:invalid|expired)|account (?:is )?(?:disabled|unavailable))[^\r\n]{0,240}$/i.test(visible) ||
    /(?:rate limit|overloaded|service unavailable)[^\r\n]{0,180}(?:final attempt|attempt\s*10\s*\/\s*10)$/i.test(visible)
}

function visibleTerminalText(raw: string): string {
  return raw
    .replace(/\u001b\][^\u0007]*(?:\u0007|\u001b\\)/g, '')
    .replace(/\u001b\[[0-?]*[ -\/]*[@-~]/g, '')
    .replace(/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/g, '')
}

function isExplicitBlockingPrompt(raw: string): boolean {
  const visible = visibleTerminalText(raw)
  const line = visible.split(/\r\n|\r|\n/).at(-1)?.trimEnd() ?? ''
  if (!line) return false
  return /(?:^|\s)(?:enter|input|type|provide)\s+[^:：?？\r\n]{1,64}[:：?？]$/i.test(line) ||
    /(?:^|\s)(?:password|passphrase|pin|token)\s*[:：?？]$/i.test(line) ||
    /(?:^|\s)(?:请输入|输入|密码|口令|请选择)[^:：?？\r\n]{0,64}[:：?？]$/.test(line) ||
    // zsh's native `read "name?prompt"` form and many interactive CLIs use
    // a labelled chevron prompt. A label is required so ordinary redirected
    // output or the Agent UI's single `❯` glyph does not become waiting work.
    /(?:^|\s)[\p{L}\p{N}_][\p{L}\p{N} _.:/@+-]{1,63}>$/u.test(line) ||
    /(?:\[[yYnN](?:\/[yYnN])?\]|\([yYnN](?:\/[yYnN])?\))$/.test(line)
}
