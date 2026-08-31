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

  begin(input: { scrollLeft: number; hasParent: boolean }): ParentPullPhase {
    this.#hasParent = input.hasParent
    this.#rawDistance = 0
    this.#pullDistance = 0
    this.#progress = 0
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
  }): { consume: boolean; pullDistance: number; progress: number } {
    if (this.#phase !== 'pulling' || input.verticalDominant || !this.#hasParent) {
      return { consume: false, pullDistance: 0, progress: 0 }
    }
    this.#rawDistance = Math.max(0, this.#rawDistance + input.deltaTowardParent)
    const threshold = parentPullThreshold(input.viewportWidth)
    this.#pullDistance = resistance(this.#rawDistance, threshold)
    // Resistance is visual feedback only. Comparing the resisted pixels with
    // the intent threshold turns a 150px trackpad pull into roughly 270px of
    // required finger travel, so the parent appears and then springs back.
    this.#progress = Math.min(1, this.#rawDistance / threshold)
    return { consume: true, pullDistance: this.#pullDistance, progress: this.#progress }
  }

  end(input: { scrollLeft: number; viewportWidth: number }): {
    commit: boolean
    edgeArmed: boolean
    springBack: boolean
  } {
    const wasPulling = this.#phase === 'pulling'
    const commit = wasPulling && this.#rawDistance >= parentPullThreshold(input.viewportWidth)
    this.#edgeArmed = this.#hasParent && input.scrollLeft <= 1 && !commit
    this.#phase = 'idle'
    this.#rawDistance = 0
    this.#pullDistance = 0
    this.#progress = 0
    return { commit, edgeArmed: this.#edgeArmed, springBack: wasPulling && !commit }
  }

  cancel(): void {
    this.#phase = 'idle'
    this.#rawDistance = 0
    this.#pullDistance = 0
    this.#progress = 0
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
  return Math.max(96, Math.min(150, viewportWidth * 0.22))
}

function resistance(rawDistance: number, threshold: number): number {
  if (rawDistance <= threshold) return rawDistance * 0.72
  return threshold * 0.72 + (rawDistance - threshold) * 0.35
}
