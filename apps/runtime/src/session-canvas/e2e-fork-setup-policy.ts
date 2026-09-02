import { readFileSync } from 'node:fs'

import type { WorktreeSetupStep } from '../worktrees/worktree-service'

interface E2eForkSetupEnvironment {
  MATOU_E2E?: string
  MATOU_E2E_FORK_SETUP_CONTROL?: string
}

/** Loads one real setup command only for an explicit visible E2E Runtime. */
export function createE2eForkSetupPolicyProvider(
  environment: E2eForkSetupEnvironment
): ((workspaceId: string) => WorktreeSetupStep[]) | undefined {
  const controlPath = environment.MATOU_E2E_FORK_SETUP_CONTROL
  if (environment.MATOU_E2E !== '1' || !controlPath) return undefined
  const step = parseStep(readFileSync(controlPath, 'utf8'))
  return () => [{ ...step, args: [...step.args] }]
}

function parseStep(serialized: string): Required<WorktreeSetupStep> {
  const value = JSON.parse(serialized) as Partial<WorktreeSetupStep>
  if (
    typeof value.idempotencyKey !== 'string' || value.idempotencyKey.trim() === '' ||
    typeof value.command !== 'string' || value.command.trim() === '' ||
    !Array.isArray(value.args) || value.args.some((argument) => typeof argument !== 'string')
  ) {
    throw new Error('invalid MATOU_E2E Fork setup control')
  }
  return {
    idempotencyKey: value.idempotencyKey,
    command: value.command,
    args: value.args
  }
}
