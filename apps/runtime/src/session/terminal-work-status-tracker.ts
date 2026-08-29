import type { SessionWorkStatus } from '@matou/domain'

const OSC_133 = /\u001b\]133;([CD])(?:;(-?\d+))?(?:\u0007|\u001b\\)/g

export class TerminalWorkStatusTracker {
  #buffer = ''

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
    this.#buffer = combined.slice(consumedThrough).slice(-512)
    return statuses
  }
}
