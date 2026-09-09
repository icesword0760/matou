/**
 * A referenced entity is gone. The `code` is the contract every caller matches on;
 * the message is free to be localised, so it must never be matched as text.
 */
export class EntityMissingError extends Error {
  readonly code = 'ENTITY_NOT_FOUND' as const

  constructor(message: string) {
    super(message)
    this.name = 'EntityMissingError'
  }
}
