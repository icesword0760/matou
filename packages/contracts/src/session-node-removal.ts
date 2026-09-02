export const REMOVE_NODE_SCOPES = ['node-only', 'node-and-descendants'] as const

export type RemoveNodeScope = (typeof REMOVE_NODE_SCOPES)[number]
