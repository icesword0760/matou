import { randomUUID } from 'node:crypto'
import type { SubscriptionProvider } from '@matou/contracts'
import { isSubscriptionAuthorizationUrl } from './authorization-url'
import type { RouterProcessControl, RouterProcessEvent } from './process-owner'

/** Private main/runtime transport; secrets never cross it. Authorization URLs are checked at both ends. */
export class RouterHostBridge implements RouterProcessControl {
  readonly #send: (message: object) => void
  readonly #listeners = new Set<(event: RouterProcessEvent) => void>()
  readonly #pending = new Map<string, { resolve(): void; reject(error: Error): void }>()
  constructor(send: (message: object) => void) { this.#send = send }
  openAuthorizationUrl(provider: SubscriptionProvider, url: string): Promise<void> {
    if (!isSubscriptionAuthorizationUrl(provider, url)) return Promise.reject(new Error('Sign-in authorization URL is not allowed'))
    return this.#request('open-authorization', { provider, url })
  }
  start(): Promise<void> { return this.#request('start') }
  stop(): Promise<void> { return this.#request('stop') }
  onEvent(listener: (event: RouterProcessEvent) => void): () => void { this.#listeners.add(listener); return () => this.#listeners.delete(listener) }
  receive(message: unknown): boolean {
    if (!message || typeof message !== 'object' || !('type' in message)) return false
    const value = message as { type: string; requestId?: string; ok?: boolean; event?: RouterProcessEvent }
    if (value.type === 'runtime.model-router-event' && value.event) {
      for (const listener of this.#listeners) listener(value.event)
      return true
    }
    if (value.type !== 'runtime.model-router-result' || !value.requestId) return false
    const pending = this.#pending.get(value.requestId)
    this.#pending.delete(value.requestId)
    if (value.ok) pending?.resolve()
    else pending?.reject(new Error('Model router process operation failed'))
    return true
  }
  #request(action: 'start' | 'stop' | 'open-authorization', details: object = {}): Promise<void> {
    const requestId = randomUUID()
    return new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => { this.#pending.delete(requestId); reject(new Error('Model router host timed out')) }, 6_000)
      this.#pending.set(requestId, { resolve: () => { clearTimeout(timer); resolve() }, reject: error => { clearTimeout(timer); reject(error) } })
      this.#send({ type: 'runtime.model-router-request', requestId, action, ...details })
    })
  }
}
