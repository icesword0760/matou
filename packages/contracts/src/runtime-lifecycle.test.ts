import { describe, expect, it } from 'vitest'

import {
  parseRuntimeLifecycleEvent,
  type RuntimeRecoverySnapshot,
  validateRuntimeRecoveryTransition,
  parseRuntimeRecoveryCommand
} from './runtime-lifecycle'

describe('runtime lifecycle contract', () => {
  const openingSnapshot = {
    recoveryId: 'recovery-1',
    revision: 0,
    mode: 'normal',
    stage: 'opening-database',
    completed: 0,
    total: 4,
    failures: []
  } satisfies RuntimeRecoverySnapshot

  it.each([
    'opening-database',
    'reconciling-worktrees',
    'reconciling-forks',
    'recovering-active-session',
    'recovering-background-sessions',
    'ready'
  ])('accepts the %s recovery stage', (stage) => {
    expect(parseRuntimeLifecycleEvent({
      type: 'runtime.lifecycle',
      snapshot: { ...openingSnapshot, stage }
    })).toMatchObject({ type: 'runtime.lifecycle', snapshot: { stage } })
  })

  it('rejects an unknown recovery stage', () => {
    expect(() => parseRuntimeLifecycleEvent({
      type: 'runtime.lifecycle',
      snapshot: { ...openingSnapshot, stage: 'skipping-user-assets' }
    })).toThrow(/stage/)
  })

  it('rejects a ready snapshot that regresses to an earlier stage in the same recovery', () => {
    expect(() => validateRuntimeRecoveryTransition(
      { ...openingSnapshot, stage: 'ready', completed: 4, revision: 5 },
      { ...openingSnapshot, stage: 'recovering-active-session', completed: 4, revision: 6 }
    )).toThrow(/stage/)
  })

  it('rejects a same-stage completed counter regression in the same recovery', () => {
    expect(() => validateRuntimeRecoveryTransition(
      { ...openingSnapshot, stage: 'reconciling-worktrees', completed: 2, revision: 3 },
      { ...openingSnapshot, stage: 'reconciling-worktrees', completed: 1, revision: 4 }
    )).toThrow(/completed/)
  })

  it('rejects a revision that does not increase in the same recovery', () => {
    expect(() => validateRuntimeRecoveryTransition(
      { ...openingSnapshot, revision: 3 },
      { ...openingSnapshot, revision: 2 }
    )).toThrow(/revision/)
  })

  it('accepts a fresh recovery ID restarting at opening-database and zero completed work', () => {
    expect(() => validateRuntimeRecoveryTransition(
      { ...openingSnapshot, stage: 'ready', completed: 4, revision: 9 },
      { ...openingSnapshot, recoveryId: 'recovery-2', revision: 0 }
    )).not.toThrow()
  })

  it('rejects recovery progress that exceeds its total', () => {
    expect(() => parseRuntimeLifecycleEvent({
      type: 'runtime.lifecycle',
      snapshot: { ...openingSnapshot, completed: 5 }
    })).toThrow(/completed/)
  })

  it.each([
    { ...openingSnapshot, completed: -1 },
    { ...openingSnapshot, total: -1 }
  ])('rejects negative recovery counters', (snapshot) => {
    expect(() => parseRuntimeLifecycleEvent({ type: 'runtime.lifecycle', snapshot })).toThrow()
  })

  it('accepts restore commands only when a backup ID is supplied', () => {
    expect(parseRuntimeRecoveryCommand({
      type: 'runtime.recovery-command',
      requestId: 'restore-1',
      action: 'restore-backup',
      backupId: 'backup-1'
    })).toMatchObject({ action: 'restore-backup', backupId: 'backup-1' })

    expect(() => parseRuntimeRecoveryCommand({
      type: 'runtime.recovery-command',
      requestId: 'restore-1',
      action: 'restore-backup'
    })).toThrow(/backupId/)
  })

  it('fails closed for an unknown recovery command action', () => {
    expect(() => parseRuntimeRecoveryCommand({
      type: 'runtime.recovery-command',
      requestId: 'unknown-1',
      action: 'drop-database'
    })).toThrow(/action/)
  })
})
