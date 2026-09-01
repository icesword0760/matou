import type { AllowedControlKey } from './host-control-types'

export const CONTROL_KEY_SEQUENCES: Readonly<Record<AllowedControlKey, string>> = {
  Enter: '\r',
  Tab: '\t',
  Escape: '\u001b',
  Backspace: '\u007f',
  Delete: '\u001b[3~',
  ArrowUp: '\u001b[A',
  ArrowDown: '\u001b[B',
  ArrowLeft: '\u001b[D',
  ArrowRight: '\u001b[C',
  Home: '\u001b[H',
  End: '\u001b[F',
  PageUp: '\u001b[5~',
  PageDown: '\u001b[6~',
  CtrlC: '\u0003',
  CtrlD: '\u0004',
  CtrlL: '\u000c',
  CtrlU: '\u0015',
  CtrlZ: '\u001a'
}

interface QueueState {
  generation: number
  tail: Promise<void>
}

export class TerminalInputQueue {
  readonly #states = new Map<string, QueueState>()
  readonly #generations = new Map<string, number>()

  enqueue<T>(sessionId: string, action: () => T | Promise<T>): Promise<T> {
    const generation = this.#generations.get(sessionId) ?? 0
    const state = this.#states.get(sessionId)
    const previous = state?.tail ?? Promise.resolve()
    const result = previous.then(async () => {
      if ((this.#generations.get(sessionId) ?? 0) !== generation) {
        throw new Error('SessionRun ended before queued input was sent')
      }
      return action()
    })
    const tail = result.then(() => undefined, () => undefined)
    this.#states.set(sessionId, { generation, tail })
    void tail.finally(() => {
      if (this.#states.get(sessionId)?.tail === tail) this.#states.delete(sessionId)
    })
    return result
  }

  clear(sessionId: string): void {
    this.#generations.set(sessionId, (this.#generations.get(sessionId) ?? 0) + 1)
    this.#states.delete(sessionId)
  }
}
