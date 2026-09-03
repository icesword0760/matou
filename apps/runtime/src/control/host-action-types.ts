import { z } from 'zod'

import type { HostNavigationPath } from '@matou/contracts'

export type HostActionMethod =
  | 'structure.create.workspace' | 'structure.create.task'
  | 'structure.create.canvas' | 'structure.create.session'
  | 'structure.fork.child' | 'structure.fork.sibling' | 'structure.fork.children'
  | 'structure.remove.preview' | 'structure.remove.commit'
  | 'structure.canvas-close.preview' | 'structure.canvas-close.commit'
  | 'navigation.focus.session' | 'navigation.switch.workspace'
  | 'navigation.switch.task' | 'navigation.switch.canvas'

export type ForkEnvironmentChoice =
  | { mode: 'current' }
  | { mode: 'existing-worktree'; branch: string; worktreeRef: string }
  | { mode: 'new-worktree'; branch: string }

export type HostActionErrorCode =
  | 'INVALID_REQUEST' | 'TARGET_NOT_FOUND' | 'AMBIGUOUS_TARGET' | 'STALE_PROJECTION'
  | 'TARGET_NOT_READY' | 'CAPABILITY_DENIED' | 'CONFIRMATION_REQUIRED'
  | 'CONFIRMATION_EXPIRED' | 'CONFIRMATION_STALE' | 'PATH_CONFLICT'
  | 'BRANCH_CONFLICT' | 'WORKTREE_CONFLICT' | 'PARTIAL_SUCCESS'
  | 'NAVIGATION_TIMEOUT' | 'STORAGE_READ_ONLY'

/**
 * High-level structure actions bind every position-based target choice to the
 * projection that supplied the position. Terminal Host Control keeps its own
 * compatibility selector because its legacy commands still accept revisionless
 * relative and relation choices.
 */
export type HostActionTargetSelector =
  | { kind: 'self' }
  | { kind: 'relative'; direction: 'left' | 'right'; projectionRevision: string }
  | { kind: 'relation'; relation: 'parent' | 'child'; ordinal?: number; projectionRevision: string }
  | { kind: 'sibling'; ordinal: number; projectionRevision: string }
  | { kind: 'ref'; ref: string; projectionRevision: string }
  | { kind: 'session'; sessionId: string }

export type HostEntitySelector =
  | { kind: 'current'; entity: 'workspace' | 'task' | 'canvas' | 'session' }
  | HostActionTargetSelector

export interface ForkItemInput {
  itemKey: string
  title: string
  environment: ForkEnvironmentChoice
  prompt?: string
  start?: boolean
}

export type HostActionRequest =
  | { method: 'structure.create.workspace'; path: string; title?: string;
      submissionKey: string; enter?: boolean }
  | { method: 'structure.create.task'; workspace: HostEntitySelector; title?: string;
      submissionKey: string; enter?: boolean }
  | { method: 'structure.create.canvas'; task: HostEntitySelector; title?: string;
      submissionKey: string; enter?: boolean }
  | { method: 'structure.create.session'; canvas: HostEntitySelector;
      profile: 'shell' | 'claude-code' | 'codex'; title?: string;
      submissionKey: string; enter?: boolean }
  | { method: 'structure.fork.child' | 'structure.fork.sibling';
      source: HostActionTargetSelector; title: string; environment: ForkEnvironmentChoice;
      prompt?: string; start?: boolean; submissionKey: string }
  | { method: 'structure.fork.children'; source: HostActionTargetSelector;
      batchKey: string; items: ForkItemInput[]; retryItemKeys?: string[] }
  | { method: 'structure.remove.preview'; target: HostEntitySelector;
      scope: 'node' | 'subtree' }
  | { method: 'structure.remove.commit'; confirmationRef: string }
  | { method: 'structure.canvas-close.preview'; target: HostEntitySelector }
  | { method: 'structure.canvas-close.commit'; confirmationRef: string }
  | { method: 'navigation.focus.session'; target: HostEntitySelector }
  | { method: 'navigation.switch.workspace' | 'navigation.switch.task' |
      'navigation.switch.canvas'; target: HostEntitySelector }

export interface HostResultPath {
  window: { ref: string; title: string }
  workspace: { ref: string; title: string; path: string }
  task?: { ref: string; title: string }
  canvas?: { ref: string; title: string }
  session?: { ref: string; title: string }
}

export interface HostImpactSummary {
  target: HostResultPath
  scope: 'node' | 'subtree'
  tasks: number
  canvases: number
  sessions: number
  descendants: number
  liveRuns: number
  terminalProcesses: number
  preservesProjectFiles: true
  preservesBranches: true
  preservesWorktrees: true
}

export type ForkBatchItemState = 'created' | 'ready' | 'started' | 'failed'

export interface ForkBatchResult {
  kind: 'fork-batch'
  batchKey: string
  succeeded: number
  failed: number
  items: Array<{ itemKey: string; title: string; state: ForkBatchItemState;
    sessionRef?: string; environment: ForkEnvironmentChoice; error?: string }>
  retry?: { batchKey: string; itemKeys: string[] }
}

export interface HostRemovalPreview {
  kind: 'removal-preview'
  impact: HostImpactSummary
  confirmationRef: string
}

export interface HostCanvasClosePreview {
  kind: 'canvas-close-preview'
  impact: HostImpactSummary
  confirmationRef: string
}

export type HostActionResult =
  | { kind: 'created'; entity: 'workspace' | 'task' | 'canvas' | 'session';
      createdRef: string; path: HostResultPath; focusedPath: HostResultPath }
  | { kind: 'forked'; state: 'created' | 'ready' | 'started';
      sessionRef: string; path: HostResultPath; environment: ForkEnvironmentChoice }
  | ForkBatchResult
  | HostRemovalPreview | HostCanvasClosePreview
  | { kind: 'removed'; targetRef: string; removedTasks: number;
      removedCanvases: number; removedSessions: number; activePath: HostResultPath }
  | { kind: 'canvas-closed'; targetRef: string; removedSessions: number;
      activePath: HostResultPath }
  | { kind: 'navigated'; finalPath: HostNavigationPath }

const utf8Text = (minimum: number, maximum: number) => z.string().superRefine((value, context) => {
  const bytes = Buffer.byteLength(value, 'utf8')
  if (bytes < minimum || bytes > maximum) {
    context.addIssue({
      code: 'custom',
      message: `must contain ${minimum}-${maximum} UTF-8 bytes`
    })
  }
})

const nonEmptyText = (maximum = 160) => z.string().min(1).max(maximum)
const titleSchema = utf8Text(1, 160)
const promptSchema = utf8Text(0, 64 * 1024)
const keySchema = nonEmptyText(160)
const referenceSchema = nonEmptyText()
const revisionSchema = nonEmptyText()

const actionTargetSelectorSchema = z.union([
  z.object({ kind: z.literal('self') }).strict(),
  z.object({ kind: z.literal('relative'), direction: z.enum(['left', 'right']), projectionRevision: revisionSchema }).strict(),
  z.object({ kind: z.literal('relation'), relation: z.enum(['parent', 'child']), ordinal: z.number().int().positive().optional(), projectionRevision: revisionSchema }).strict(),
  z.object({ kind: z.literal('sibling'), ordinal: z.number().int().positive(), projectionRevision: revisionSchema }).strict(),
  z.object({ kind: z.literal('ref'), ref: referenceSchema, projectionRevision: revisionSchema }).strict(),
  z.object({ kind: z.literal('session'), sessionId: nonEmptyText() }).strict()
])

const entitySelectorSchema = z.union([
  z.object({ kind: z.literal('current'), entity: z.enum(['workspace', 'task', 'canvas', 'session']) }).strict(),
  actionTargetSelectorSchema
])

const environmentSchema = z.discriminatedUnion('mode', [
  z.object({ mode: z.literal('current') }).strict(),
  z.object({ mode: z.literal('existing-worktree'), branch: nonEmptyText(), worktreeRef: referenceSchema }).strict(),
  z.object({ mode: z.literal('new-worktree'), branch: nonEmptyText() }).strict()
])

const optionalTitle = titleSchema.optional()
const optionalEnter = z.boolean().optional()
const optionalPrompt = promptSchema.optional()
const optionalStart = z.boolean().optional()

const forkItemSchema = z.object({
  itemKey: keySchema,
  title: titleSchema,
  environment: environmentSchema,
  prompt: optionalPrompt,
  start: optionalStart
}).strict()

const hostActionRequestSchema = z.discriminatedUnion('method', [
  z.object({ method: z.literal('structure.create.workspace'), path: nonEmptyText(4096), title: optionalTitle, submissionKey: keySchema, enter: optionalEnter }).strict(),
  z.object({ method: z.literal('structure.create.task'), workspace: entitySelectorSchema, title: optionalTitle, submissionKey: keySchema, enter: optionalEnter }).strict(),
  z.object({ method: z.literal('structure.create.canvas'), task: entitySelectorSchema, title: optionalTitle, submissionKey: keySchema, enter: optionalEnter }).strict(),
  z.object({ method: z.literal('structure.create.session'), canvas: entitySelectorSchema, profile: z.enum(['shell', 'claude-code', 'codex']), title: optionalTitle, submissionKey: keySchema, enter: optionalEnter }).strict(),
  z.object({ method: z.enum(['structure.fork.child', 'structure.fork.sibling']), source: actionTargetSelectorSchema, title: titleSchema, environment: environmentSchema, prompt: optionalPrompt, start: optionalStart, submissionKey: keySchema }).strict(),
  z.object({ method: z.literal('structure.fork.children'), source: actionTargetSelectorSchema, batchKey: keySchema, items: z.array(forkItemSchema).max(50), retryItemKeys: z.array(keySchema).max(50).optional() }).strict().superRefine((request, context) => {
    const itemKeys = new Set<string>()
    for (const [index, item] of request.items.entries()) {
      if (itemKeys.has(item.itemKey)) {
        context.addIssue({ code: 'custom', path: ['items', index, 'itemKey'], message: 'itemKey must be unique' })
      }
      itemKeys.add(item.itemKey)
    }
    const retryItemKeys = new Set<string>()
    for (const [index, itemKey] of (request.retryItemKeys ?? []).entries()) {
      if (retryItemKeys.has(itemKey)) {
        context.addIssue({ code: 'custom', path: ['retryItemKeys', index], message: 'retry itemKey must be unique' })
      }
      if (!itemKeys.has(itemKey)) {
        context.addIssue({ code: 'custom', path: ['retryItemKeys', index], message: 'retry itemKey must belong to items' })
      }
      retryItemKeys.add(itemKey)
    }
  }),
  z.object({ method: z.literal('structure.remove.preview'), target: entitySelectorSchema, scope: z.enum(['node', 'subtree']) }).strict(),
  z.object({ method: z.literal('structure.remove.commit'), confirmationRef: referenceSchema }).strict(),
  z.object({ method: z.literal('structure.canvas-close.preview'), target: entitySelectorSchema }).strict(),
  z.object({ method: z.literal('structure.canvas-close.commit'), confirmationRef: referenceSchema }).strict(),
  z.object({ method: z.literal('navigation.focus.session'), target: entitySelectorSchema }).strict(),
  z.object({ method: z.enum(['navigation.switch.workspace', 'navigation.switch.task', 'navigation.switch.canvas']), target: entitySelectorSchema }).strict()
])

export function parseHostActionRequest(method: HostActionMethod, params: unknown): HostActionRequest {
  if (typeof params !== 'object' || params === null || Array.isArray(params)) {
    throw new z.ZodError([{ code: 'invalid_type', expected: 'object', input: params, path: [], message: 'params must be an object' }])
  }
  const input = params as Record<string, unknown>
  if (Object.prototype.hasOwnProperty.call(input, 'method')) {
    throw new z.ZodError([{
      code: 'unrecognized_keys',
      keys: ['method'],
      input,
      path: [],
      message: 'params.method is not supported'
    }])
  }
  return hostActionRequestSchema.parse({ ...input, method }) as HostActionRequest
}
