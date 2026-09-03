const POST_RESPONSE_EFFECTS = Symbol('matou.host-control.post-response-effects')

type PostResponseCarrier = {
  [POST_RESPONSE_EFFECTS]?: Array<() => void | Promise<void>>
}

/**
 * Attaches Runtime-local work that must begin only after Host Control has
 * queued the authoritative response. Symbol metadata is omitted from JSON.
 */
export function withHostControlPostResponseEffect<T extends object>(
  result: T,
  effect: () => void | Promise<void>
): T {
  const carrier = result as T & PostResponseCarrier
  const effects = carrier[POST_RESPONSE_EFFECTS] ?? []
  if (effects.length === 0) {
    Object.defineProperty(carrier, POST_RESPONSE_EFFECTS, {
      configurable: false,
      enumerable: false,
      writable: false,
      value: effects
    })
  }
  effects.push(effect)
  return result
}

export async function runHostControlPostResponseEffects(value: unknown): Promise<void> {
  if (typeof value !== 'object' || value === null) return
  const effects = (value as PostResponseCarrier)[POST_RESPONSE_EFFECTS]
  if (!effects) return
  for (const effect of effects.splice(0)) await effect()
}

export function hasHostControlPostResponseEffects(value: unknown): boolean {
  return typeof value === 'object' && value !== null &&
    ((value as PostResponseCarrier)[POST_RESPONSE_EFFECTS]?.length ?? 0) > 0
}
