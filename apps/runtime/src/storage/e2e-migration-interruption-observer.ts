import {
  closeSync,
  fsyncSync,
  mkdirSync,
  openSync,
  readFileSync,
  writeFileSync
} from 'node:fs'
import { dirname } from 'node:path'

import type { DatabaseBackupDescriptor } from './database-backup-service'
import type { Migration, MigrationRunnerObserver } from './migration-runner'

type InterruptionStage =
  | 'pre-migration-backup-ready'
  | 'migration-transaction-prepared'
  | 'migration-committed'

interface E2eMigrationControl {
  stage: InterruptionStage
  reachedPath: string
  hold?: boolean
}

interface E2eMigrationEnvironment {
  MATOU_E2E?: string
  MATOU_E2E_MIGRATION_CONTROL?: string
}

/** Fault injection observer that is instantiated only by explicit MATOU_E2E runs. */
export function createE2eMigrationInterruptionObserver(
  environment: E2eMigrationEnvironment
): MigrationRunnerObserver | undefined {
  const controlPath = environment.MATOU_E2E_MIGRATION_CONTROL
  if (environment.MATOU_E2E !== '1' || !controlPath) return undefined
  const control = parseControl(readFileSync(controlPath, 'utf8'))

  return {
    onPreMigrationBackupReady(backup) {
      reach(control, 'pre-migration-backup-ready', { backupId: backup.id })
    },
    onMigrationTransactionPrepared(migration) {
      reachMigration(control, 'migration-transaction-prepared', migration)
    },
    onMigrationCommitted(migration) {
      reachMigration(control, 'migration-committed', migration)
    }
  }
}

function reachMigration(
  control: E2eMigrationControl,
  stage: InterruptionStage,
  migration: Migration
): void {
  reach(control, stage, { migrationVersion: migration.version })
}

function reach(
  control: E2eMigrationControl,
  stage: InterruptionStage,
  details: Record<string, string | number>
): void {
  if (control.stage !== stage) return
  mkdirSync(dirname(control.reachedPath), { recursive: true })
  const descriptor = openSync(control.reachedPath, 'w', 0o600)
  try {
    writeFileSync(descriptor, JSON.stringify({ stage, pid: process.pid, ...details }))
    fsyncSync(descriptor)
  } finally {
    closeSync(descriptor)
  }
  if (control.hold === false) return
  const barrier = new Int32Array(new SharedArrayBuffer(4))
  for (;;) Atomics.wait(barrier, 0, 0)
}

function parseControl(serialized: string): E2eMigrationControl {
  const value = JSON.parse(serialized) as Partial<E2eMigrationControl>
  if (
    ![
      'pre-migration-backup-ready',
      'migration-transaction-prepared',
      'migration-committed'
    ].includes(String(value.stage)) ||
    typeof value.reachedPath !== 'string' ||
    value.reachedPath.trim() === '' ||
    (value.hold !== undefined && typeof value.hold !== 'boolean')
  ) {
    throw new Error('invalid MATOU_E2E migration interruption control')
  }
  return value as E2eMigrationControl
}
