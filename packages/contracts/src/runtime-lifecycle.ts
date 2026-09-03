import { z } from 'zod'

export const RUNTIME_MODES = ['normal', 'read-only', 'recovery-required'] as const
export const RUNTIME_RECOVERY_STAGES = [
  'opening-database',
  'reconciling-worktrees',
  'reconciling-forks',
  'recovering-active-session',
  'recovering-background-sessions',
  'ready'
] as const
export const RUNTIME_RECOVERY_FAILURE_LAYERS = [
  'database',
  'worktree',
  'fork',
  'session',
  'journal'
] as const
export const RUNTIME_RECOVERY_COMMAND_ACTIONS = [
  'restore-backup',
  'export-recovery-bundle',
  'retry-open',
  'start-empty-database'
] as const
export const RUNTIME_ERROR_CODES = [
  'STORAGE_READ_ONLY',
  'DATABASE_RECOVERY_REQUIRED',
  'SESSION_ENVIRONMENT_UNAVAILABLE',
  'FORK_ALREADY_RUNNING'
] as const
export const RUNTIME_STARTUP_FAILURE_CODES = [
  'MIGRATION_HISTORY_MISMATCH',
  'DATABASE_SCHEMA_UNSUPPORTED',
  'RUNTIME_INITIALIZATION_FAILED'
] as const
export const STORAGE_FAULT_CODES = [
  ...RUNTIME_ERROR_CODES,
  'STORAGE_WRITE_FAILED',
  'STORAGE_QUOTA_EXCEEDED'
] as const

export type RuntimeMode = (typeof RUNTIME_MODES)[number]
export type RuntimeRecoveryStage = (typeof RUNTIME_RECOVERY_STAGES)[number]
export type RuntimeRecoveryFailureLayer = (typeof RUNTIME_RECOVERY_FAILURE_LAYERS)[number]
export type RuntimeRecoveryCommandAction = (typeof RUNTIME_RECOVERY_COMMAND_ACTIONS)[number]
export type RuntimeErrorCode = (typeof RUNTIME_ERROR_CODES)[number]
export type RuntimeStartupFailureCode = (typeof RUNTIME_STARTUP_FAILURE_CODES)[number]
export type StorageFaultCode = (typeof STORAGE_FAULT_CODES)[number]

const identifier = z
  .string()
  .min(1)
  .max(160)
  .regex(/^[A-Za-z0-9][A-Za-z0-9._-]*$/, 'identifier contains unsupported characters')

const runtimeRecoveryFailureSchema = z.object({
  layer: z.enum(RUNTIME_RECOVERY_FAILURE_LAYERS),
  resourceId: identifier,
  code: z.string().min(1).max(160),
  message: z.string().min(1).max(4_096)
}).strict()

export const runtimeRecoverySnapshotSchema = z.object({
  recoveryId: identifier,
  revision: z.number().int().nonnegative(),
  mode: z.enum(RUNTIME_MODES),
  stage: z.enum(RUNTIME_RECOVERY_STAGES),
  completed: z.number().int().nonnegative(),
  total: z.number().int().nonnegative(),
  activeSessionId: identifier.optional(),
  failures: z.array(runtimeRecoveryFailureSchema)
}).strict().superRefine((snapshot, context) => {
  if (snapshot.completed > snapshot.total) {
    context.addIssue({
      code: 'custom',
      path: ['completed'],
      message: 'completed recovery work must not exceed total recovery work'
    })
  }
})

export type RuntimeRecoverySnapshot = z.infer<typeof runtimeRecoverySnapshotSchema>

export const runtimeLifecycleEventSchema = z.object({
  type: z.literal('runtime.lifecycle'),
  snapshot: runtimeRecoverySnapshotSchema
}).strict()

export type RuntimeLifecycleEvent = z.infer<typeof runtimeLifecycleEventSchema>

export const runtimeStartupFailureSchema = z.object({
  type: z.literal('runtime.startup-failure'),
  code: z.enum(RUNTIME_STARTUP_FAILURE_CODES),
  message: z.string().min(1).max(4_096),
  retryable: z.literal(false)
}).strict()

export type RuntimeStartupFailure = z.infer<typeof runtimeStartupFailureSchema>

const recoveryCommandBase = {
  type: z.literal('runtime.recovery-command'),
  requestId: identifier
}

const mutatingRecoveryCommandBase = {
  ...recoveryCommandBase,
  expectedRecoveryId: identifier
}

export const runtimeRecoveryCommandSchema = z.discriminatedUnion('action', [
  z.object({
    ...mutatingRecoveryCommandBase,
    action: z.literal('restore-backup'),
    backupId: identifier
  }).strict(),
  z.object({
    ...recoveryCommandBase,
    action: z.literal('export-recovery-bundle')
  }).strict(),
  z.object({
    ...mutatingRecoveryCommandBase,
    action: z.literal('retry-open')
  }).strict(),
  z.object({
    ...mutatingRecoveryCommandBase,
    action: z.literal('start-empty-database')
  }).strict()
])

export type RuntimeRecoveryCommand = z.infer<typeof runtimeRecoveryCommandSchema>

export function validateRuntimeRecoveryTransition(
  previous: RuntimeRecoverySnapshot | undefined,
  next: RuntimeRecoverySnapshot
): void {
  if (!previous || previous.recoveryId !== next.recoveryId) return

  if (next.revision <= previous.revision) {
    throw new Error('runtime recovery revision must increase within one recovery')
  }

  if (RUNTIME_RECOVERY_STAGES.indexOf(next.stage) < RUNTIME_RECOVERY_STAGES.indexOf(previous.stage)) {
    throw new Error('runtime recovery stage must not regress within one recovery')
  }

  if (next.completed < previous.completed) {
    throw new Error('runtime recovery completed work must not regress within one recovery')
  }
}

export function parseRuntimeLifecycleEvent(
  value: unknown,
  previous?: RuntimeRecoverySnapshot
): RuntimeLifecycleEvent {
  const event = runtimeLifecycleEventSchema.parse(value)
  validateRuntimeRecoveryTransition(previous, event.snapshot)
  return event
}

export function parseRuntimeStartupFailure(value: unknown): RuntimeStartupFailure {
  return runtimeStartupFailureSchema.parse(value)
}

export function parseRuntimeRecoveryCommand(value: unknown): RuntimeRecoveryCommand {
  return runtimeRecoveryCommandSchema.parse(value)
}
