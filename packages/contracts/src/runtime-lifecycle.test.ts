import { describe, expect, it } from 'vitest'

import {
  parseRuntimeLifecycleEvent,
  parseRuntimeRecoveryCommand
} from './runtime-lifecycle'

describe('runtime lifecycle contract', () => {
  const openingSnapshot = {
    mode: 'normal',
    stage: 'opening-database',
    completed: 0,
    total: 4,
    failures: []
  } as const

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
