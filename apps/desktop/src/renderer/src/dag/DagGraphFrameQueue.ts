import type { SessionGraphView } from '../hierarchy/hierarchy-types'

export interface DagGraphFrameUpdate {
  graph: SessionGraphView
  sequence: number
  runtimeGeneration: string
}

type ScheduleFrame = (callback: FrameRequestCallback) => number
type CancelFrame = (handle: number) => void

/** Keeps graph layout/render work to at most one authoritative update per frame. */
export class DagGraphFrameQueue {
  #pending: DagGraphFrameUpdate | undefined
  #frame: number | undefined
  readonly #publish: (update: DagGraphFrameUpdate) => void
  readonly #scheduleFrame: ScheduleFrame
  readonly #cancelFrame: CancelFrame

  constructor(
    publish: (update: DagGraphFrameUpdate) => void,
    scheduleFrame: ScheduleFrame = (callback) => requestAnimationFrame(callback),
    cancelFrame: CancelFrame = (handle) => cancelAnimationFrame(handle)
  ) {
    this.#publish = publish
    this.#scheduleFrame = scheduleFrame
    this.#cancelFrame = cancelFrame
  }

  enqueue(update: DagGraphFrameUpdate): void {
    const pending = this.#pending
    if (pending && pending.runtimeGeneration === update.runtimeGeneration &&
      pending.sequence > update.sequence) return
    this.#pending = update
    if (this.#frame !== undefined) return

    // -1 reserves the slot while schedulers used by tests or embedded hosts
    // invoke their callback synchronously.
    this.#frame = -1
    const handle = this.#scheduleFrame(() => {
      this.#frame = undefined
      const latest = this.#pending
      this.#pending = undefined
      if (latest) this.#publish(latest)
    })
    if (this.#frame === -1) this.#frame = handle
  }

  cancel(): void {
    if (this.#frame !== undefined && this.#frame >= 0) this.#cancelFrame(this.#frame)
    this.#frame = undefined
    this.#pending = undefined
  }
}
