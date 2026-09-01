export type ParentPullPhase = 'idle' | 'scrolling' | 'pulling'

export interface ParentPullSnapshot {
  phase: ParentPullPhase
  edgeArmed: boolean
  pullDistance: number
  progress: number
}

export class ParentPullController {
  #phase: ParentPullPhase = 'idle'
  #edgeArmed = false
  #rawDistance = 0
  #pullDistance = 0
  #progress = 0
  #hasParent = false
  #lastMoveAt: number | undefined
  #lastPullDistance = 0
  #releaseVelocity = 0
  #readyAt: number | undefined

  begin(input: { scrollLeft: number; hasParent: boolean; timeMs?: number }): ParentPullPhase {
    this.#hasParent = input.hasParent
    this.#rawDistance = 0
    this.#pullDistance = 0
    this.#progress = 0
    this.#lastMoveAt = input.timeMs
    this.#lastPullDistance = 0
    this.#releaseVelocity = 0
    this.#readyAt = undefined
    // The gesture must begin at the edge. A gesture that starts inside the list
    // remains ordinary scrolling even if one large delta reaches scrollLeft=0.
    this.#phase = input.hasParent && input.scrollLeft <= 1
      ? 'pulling'
      : 'scrolling'
    this.#edgeArmed = input.hasParent && input.scrollLeft <= 1
    return this.#phase
  }

  move(input: {
    deltaTowardParent: number
    viewportWidth: number
    verticalDominant?: boolean
    timeMs?: number
  }): { consume: boolean; pullDistance: number; progress: number; effectIntensity: number } {
    if (this.#phase !== 'pulling' || input.verticalDominant || !this.#hasParent) {
      return { consume: false, pullDistance: 0, progress: 0, effectIntensity: 0 }
    }
    // Starting at the left edge is only a candidate for returning to the
    // parent. If the first meaningful movement goes toward later siblings,
    // release the event immediately so the horizontal list can scroll.
    if (this.#rawDistance === 0 && input.deltaTowardParent < 0) {
      this.#phase = 'scrolling'
      this.#edgeArmed = false
      return { consume: false, pullDistance: 0, progress: 0, effectIntensity: 0 }
    }
    this.#rawDistance = Math.max(0, this.#rawDistance + input.deltaTowardParent)
    const threshold = parentPullThreshold(input.viewportWidth)
    // The preview follows the hand 1:1 until it is completely exposed. Once
    // armed, it becomes a physical stop: extra travel records continued intent
    // but must not move or trigger it before release.
    this.#pullDistance = Math.min(this.#rawDistance, threshold)
    // Progress is tied to the same physical reveal distance. The gesture is
    // not armed while any part of the parent preview is still under the strip.
    this.#progress = Math.min(1, this.#rawDistance / threshold)
    if (this.#progress >= 1) {
      this.#readyAt ??= input.timeMs
    } else {
      this.#readyAt = undefined
    }
    if (input.timeMs !== undefined && this.#lastMoveAt !== undefined) {
      const elapsed = Math.max(1, input.timeMs - this.#lastMoveAt)
      const instantaneous = (this.#pullDistance - this.#lastPullDistance) * 1_000 / elapsed
      this.#releaseVelocity = clamp(instantaneous * 0.72 + this.#releaseVelocity * 0.28, -1_800, 1_800)
    }
    if (input.timeMs !== undefined) this.#lastMoveAt = input.timeMs
    this.#lastPullDistance = this.#pullDistance
    return {
      consume: true,
      pullDistance: this.#pullDistance,
      progress: this.#progress,
      effectIntensity: clamp(Math.abs(this.#releaseVelocity) / 1_600, 0, 1)
    }
  }

  end(input: { scrollLeft: number; viewportWidth: number; timeMs?: number }): {
    commit: boolean
    edgeArmed: boolean
    springBack: boolean
    pullDistance: number
    releaseVelocity: number
    commitDelayMs: number
  } {
    const wasPulling = this.#phase === 'pulling'
    const commit = wasPulling && this.#rawDistance >= parentPullThreshold(input.viewportWidth)
    const pullDistance = this.#pullDistance
    const releaseVelocity = input.timeMs !== undefined && this.#lastMoveAt !== undefined &&
      input.timeMs - this.#lastMoveAt <= 80
      ? this.#releaseVelocity
      : 0
    const commitDelayMs = commit && input.timeMs !== undefined && this.#readyAt !== undefined
      ? Math.max(0, 450 - (input.timeMs - this.#readyAt))
      : 0
    this.#edgeArmed = this.#hasParent && input.scrollLeft <= 1 && !commit
    this.#phase = 'idle'
    this.#rawDistance = 0
    this.#pullDistance = 0
    this.#progress = 0
    this.#lastMoveAt = undefined
    this.#lastPullDistance = 0
    this.#releaseVelocity = 0
    this.#readyAt = undefined
    return {
      commit,
      edgeArmed: this.#edgeArmed,
      springBack: wasPulling && !commit,
      pullDistance,
      releaseVelocity,
      commitDelayMs
    }
  }

  cancel(): void {
    this.#phase = 'idle'
    this.#rawDistance = 0
    this.#pullDistance = 0
    this.#progress = 0
    this.#lastMoveAt = undefined
    this.#lastPullDistance = 0
    this.#releaseVelocity = 0
    this.#readyAt = undefined
  }

  snapshot(): ParentPullSnapshot {
    return {
      phase: this.#phase,
      edgeArmed: this.#edgeArmed,
      pullDistance: this.#pullDistance,
      progress: this.#progress
    }
  }
}

export function parentPullThreshold(viewportWidth: number): number {
  // The preview occupies 260px. Keep another 24-80px of breathing room so its
  // card and instruction are fully readable before release can navigate.
  return Math.max(284, Math.min(340, viewportWidth * 0.4))
}

export interface ParentReturnSpringState {
  position: number
  velocity: number
}

const PARENT_RETURN_STIFFNESS = 210
const PARENT_RETURN_DAMPING = 20

export function stepParentReturnSpring(
  state: ParentReturnSpringState,
  elapsedSeconds: number
): ParentReturnSpringState {
  const step = Math.max(0, Math.min(1 / 30, elapsedSeconds))
  const acceleration = -PARENT_RETURN_STIFFNESS * state.position -
    PARENT_RETURN_DAMPING * state.velocity
  const velocity = state.velocity + acceleration * step
  return { position: state.position + velocity * step, velocity }
}

export function parentReturnSpringAtRest(state: ParentReturnSpringState): boolean {
  return Math.abs(state.position) < 0.12 && Math.abs(state.velocity) < 1
}

function clamp(value: number, minimum: number, maximum: number): number {
  return Math.min(maximum, Math.max(minimum, value))
}
