export const LEGACY_MUTATION_TYPES = [
  'project-created', 'project-updated', 'project-removed', 'project-activated',
  'workbench-created', 'workbench-updated', 'workbench-reordered', 'workbench-removed', 'workbench-activated',
  'tab-created', 'tab-updated', 'tab-renamed', 'tab-removed', 'tab-activated',
  'workbench-tab-order-updated', 'split-tree-updated', 'leaf-activated',
  'panel-created', 'panel-updated', 'panel-session-bound', 'panel-permission-mode',
  'panel-shell-state', 'panel-removed', 'panel-detached', 'panel-attached'
] as const

export type LegacyMutationType = (typeof LEGACY_MUTATION_TYPES)[number]

export interface LegacyMutationEnvelope {
  schemaVersion: 1
  commandId: string
  type: LegacyMutationType
  timestamp: number
  payload: Record<string, unknown>
}

export function parseLegacyMutation(value: unknown): LegacyMutationEnvelope {
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error('Legacy mutation must be an object')
  const input = value as Record<string, unknown>
  if (input.schemaVersion !== 1) throw new Error('Unsupported legacy bridge schema version')
  if (typeof input.commandId !== 'string' || !input.commandId.trim()) throw new Error('Legacy mutation commandId is required')
  if (typeof input.type !== 'string' || !(LEGACY_MUTATION_TYPES as readonly string[]).includes(input.type)) {
    throw new Error('Unsupported Legacy mutation type')
  }
  if (!Number.isSafeInteger(input.timestamp) || (input.timestamp as number) < 0) throw new Error('Invalid Legacy mutation timestamp')
  if (!input.payload || typeof input.payload !== 'object' || Array.isArray(input.payload)) throw new Error('Legacy mutation payload must be an object')
  return {
    schemaVersion: 1,
    commandId: input.commandId.trim(),
    type: input.type as LegacyMutationType,
    timestamp: input.timestamp as number,
    payload: structuredClone(input.payload as Record<string, unknown>)
  }
}
