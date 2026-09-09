/**
 * Conflicts the Host Control facade has to classify. The code is the contract;
 * the message is localised and must never be matched on.
 */
export type HierarchyConflictCode = 'DUPLICATE_SCENE_NAME' | 'WORKSPACE_DIRECTORY_TAKEN'

export class HierarchyConflictError extends Error {
  readonly code: HierarchyConflictCode

  constructor(code: HierarchyConflictCode, message: string) {
    super(message)
    this.name = 'HierarchyConflictError'
    this.code = code
  }
}

/**
 * A hierarchy or canvas row that a command needs is gone. The message stays the
 * untranslated developer text the layer has always produced.
 */
export class HierarchyEntityMissingError extends Error {
  readonly code = 'ENTITY_NOT_FOUND' as const

  constructor(message: string) {
    super(message)
    this.name = 'HierarchyEntityMissingError'
  }
}
