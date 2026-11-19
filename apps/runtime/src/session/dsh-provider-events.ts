import type { SessionWorkStatus } from '@matou/domain'
import type { ProviderNotificationEvent } from './provider-notification-event'

export interface DshObservation {
  sessionId: string
  generation: number
  sequence: number
  type: string
  data: Record<string, unknown>
  ready: boolean
  persisted: boolean
  cwd?: string
  status?: string
  pending: number
  error?: boolean
}
const object = (value: unknown): Record<string, unknown> => value !== null && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {}
const text = (value: unknown): string | undefined => typeof value === 'string' && value.trim().length > 0 && value.length <= 4096 ? value : undefined

export function parseDshObservation(payload: Record<string, unknown>, runId: string): DshObservation | undefined {
  if (payload.dsh_observation !== 1 || payload.run_id !== runId || payload.foreground !== true ||
      !Number.isSafeInteger(payload.sequence) || (payload.sequence as number) < 1 ||
      !Number.isSafeInteger(payload.generation) || (payload.generation as number) < 1) return
  const sessionId = text(payload.session_id), type = text(payload.event_type)
  if (!sessionId || !/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(sessionId) || !type) return
  return {
    sessionId, type, sequence: payload.sequence as number, generation: payload.generation as number,
    data: object(payload.event_data), ready: type === 'ready' && payload.ready === true,
    persisted: payload.persisted === true,
    ...(text(payload.cwd) ? { cwd: text(payload.cwd)! } : {}),
    ...(text(payload.agent_status) ? { status: text(payload.agent_status)! } : {}),
    pending: Number.isSafeInteger(payload.pending_count) && (payload.pending_count as number) >= 0 ? payload.pending_count as number : 0,
    ...(typeof payload.turn_error === 'boolean' ? { error: payload.turn_error } : {})
  }
}

/** Per-run, foreground-only state. Historical snapshots never emit notifications. */
export class DshProviderEvents {
  generation = 0
  sequence = 0
  sessionId: string | undefined
  #pending = new Set<string>()
  #status: SessionWorkStatus = 'idle'
  #error = false
  #turn: number | undefined

  accepts(event: DshObservation): boolean {
    if (event.sequence <= this.sequence || event.generation < this.generation) return false
    if (event.generation > this.generation) return event.ready
    return event.sessionId === this.sessionId
  }

  ingest(event: DshObservation): { workStatus: SessionWorkStatus; notification?: ProviderNotificationEvent } {
    if (event.generation > this.generation) {
      this.generation = event.generation; this.sessionId = event.sessionId
      this.#pending.clear(); this.#turn = undefined; this.#error = false; this.#status = 'idle'
    }
    this.sequence = event.sequence
    let notification: ProviderNotificationEvent | undefined
    const notice = (eventType: ProviderNotificationEvent['eventType'], subtitle: string, body: string): ProviderNotificationEvent => ({
      eventType, title: 'DSH', subtitle, body, sound: true,
      cooldownKey: eventType === 'completed' || eventType === 'error' ? 'Stop' : 'Notification'
    })
    if (event.ready) {
      this.#pending.clear()
      if (event.error !== undefined) this.#error = event.error
      this.#status = event.pending > 0 ? 'needs-input' : event.status === 'running' || event.status === 'idle' ? event.status : this.#status
    } else if (event.type === 'turn/start' && Number.isSafeInteger(event.data.turn)) {
      this.#turn = event.data.turn as number; this.#error = false; this.#pending.clear(); this.#status = 'running'
    } else if (event.type === 'agent/status' && (event.status === 'running' || event.status === 'idle')) {
      this.#status = event.status
    } else if (event.type === 'approval/pending' || event.type === 'question/pending') {
      const id = text(event.data.id)
      if (id && !this.#pending.has(id)) {
        this.#pending.add(id)
        notification = event.type === 'approval/pending'
          ? notice('permission', 'Permission', `${text(event.data.toolName) ?? 'DSH'} requires approval`)
          : notice('waiting', 'Waiting', 'DSH needs your input')
      }
    } else if (event.type === 'approval/resolved' || event.type === 'question/resolved') {
      const id = text(event.data.id)
      if (id) this.#pending.delete(id)
      if (event.status === 'running' || event.status === 'idle') this.#status = event.status
    } else if (event.type === 'turn/end' && this.#turn !== undefined && event.data.turn === this.#turn) {
      const reason = object(event.data.reason)
      this.#turn = undefined; this.#pending.clear(); this.#status = 'idle'
      if (reason.kind === 'completed') notification = notice('completed', 'Completed', 'DSH session completed')
      else if (reason.kind === 'error') {
        this.#error = true
        const error = object(reason.error)
        notification = notice('error', 'Error', (text(error.message) ?? text(error.code) ?? 'DSH session encountered an error').replace(/\s+/g, ' ').slice(0, 200))
      } else if (reason.kind === 'blocked' || reason.kind === 'max-tokens') {
        notification = notice('attention', 'Attention', 'DSH needs your attention')
      }
    }
    return { workStatus: this.#error ? 'error' : this.#pending.size > 0 ? 'needs-input' : this.#status,
      ...(notification ? { notification } : {}) }
  }
}
