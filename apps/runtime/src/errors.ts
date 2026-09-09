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

/**
 * One command id was replayed with a different request payload. The `code` is the
 * contract every caller matches on; the message stays developer-facing detail and
 * must never be matched as text.
 */
export class CommandReplayConflictError extends Error {
  readonly code = 'COMMAND_REPLAY_CONFLICT' as const

  constructor(message: string) {
    super(message)
    this.name = 'CommandReplayConflictError'
  }
}
