import { randomUUID } from 'node:crypto'
import { cp, mkdir, readFile, rename, rm, stat, writeFile } from 'node:fs/promises'
import { basename, join } from 'node:path'

import type { RuntimeRecoveryCommand } from '@matou/contracts'

import { DatabaseBackupService } from './database-backup-service'
import { RuntimeDatabase } from './database'
import { withDatabaseRecoveryActionFence } from './database-owner'
import type { Migration } from './migration-runner'
import {
  openRecoverableRuntimeDatabaseWithOwnership,
  type RuntimeDatabaseBootstrapObserver,
  type RuntimeDatabaseBootstrapResult
} from './runtime-database-bootstrap'

type RecoveryRequired = Extract<RuntimeDatabaseBootstrapResult, { kind: 'recovery-required' }>

export interface DatabaseRecoveryExecution {
  bootstrap?: RuntimeDatabaseBootstrapResult
  value: { exportedPath?: string }
}

export class DatabaseRecoveryController {
  readonly #dataRoot: string
  readonly #migrations: readonly Migration[]
  readonly #observer: RuntimeDatabaseBootstrapObserver

  constructor(
    dataRoot: string,
    migrations: readonly Migration[],
    observer: RuntimeDatabaseBootstrapObserver = {}
  ) {
    this.#dataRoot = dataRoot
    this.#migrations = migrations
    this.#observer = observer
  }

  async execute(
    recovery: RecoveryRequired,
    command: RuntimeRecoveryCommand
  ): Promise<DatabaseRecoveryExecution> {
    switch (command.action) {
      case 'export-recovery-bundle':
        return { value: { exportedPath: await this.#export(recovery) } }
      case 'restore-backup':
        return this.#executeOpeningAction(recovery, command.requestId, async () => {
          await new DatabaseBackupService(this.#dataRoot).restore(
            command.backupId,
            recovery.durableDatabasePath
          )
        })
      case 'retry-open':
        return this.#executeOpeningAction(recovery, command.requestId, async () => {
          if (!await isFile(recovery.durableDatabasePath)) {
            throw new Error('原数据库尚未回到可检查的位置，请先恢复备份或导出恢复资料')
          }
        })
      case 'start-empty-database':
        return this.#executeOpeningAction(recovery, command.requestId, () => (
          this.#preserveBeforeEmpty(recovery, command.requestId)
        ))
    }
  }

  async #executeOpeningAction(
    recovery: RecoveryRequired,
    requestId: string,
    mutate: () => Promise<void>
  ): Promise<DatabaseRecoveryExecution> {
    return withDatabaseRecoveryActionFence(recovery.durableDatabasePath, async () => {
      await assertRecoveryStillActive(recovery)
      await this.#archiveOwnershipState(recovery, requestId)
      const ownership = RuntimeDatabase.acquireOwnership(recovery.durableDatabasePath)
      let database: RuntimeDatabase | undefined
      try {
        await this.#observer.onRecoveryActionFenced?.()
        await mutate()
        const bootstrap = await openRecoverableRuntimeDatabaseWithOwnership(
          this.#dataRoot,
          this.#migrations,
          ownership,
          this.#observer
        )
        database = bootstrap.database
        await rm(recovery.markerPath)
        return { value: {}, bootstrap }
      } catch (error) {
        if (database) {
          database.close()
          this.#observer.onDatabaseClosed?.(database)
        } else {
          ownership.release()
        }
        throw error
      }
    })
  }

  async #export(recovery: RecoveryRequired): Promise<string> {
    const exportPath = join(
      this.#dataRoot,
      'recovery-exports',
      `${Date.now()}-${randomUUID()}`
    )
    await mkdir(exportPath, { recursive: true })
    const candidates = new Set([
      recovery.markerPath,
      recovery.quarantinedPath,
      `${recovery.quarantinedPath}-wal`,
      `${recovery.quarantinedPath}-shm`,
      `${recovery.durableDatabasePath}.owner`,
      `${recovery.durableDatabasePath}.owner.takeover.sqlite`
    ])
    const exportedFiles: string[] = []
    for (const path of candidates) {
      if (!await exists(path)) continue
      const target = join(exportPath, basename(path))
      await cp(path, target, { recursive: true, errorOnExist: true })
      exportedFiles.push(basename(path))
    }
    await writeFile(join(exportPath, 'manifest.json'), JSON.stringify({
      exportedAt: Date.now(),
      reason: recovery.reason,
      ownershipIssue: recovery.ownershipIssue,
      durableDatabasePath: recovery.durableDatabasePath,
      quarantinedPath: recovery.quarantinedPath,
      backupCount: recovery.backups.length,
      backups: recovery.backups.map(({ path: _path, ...descriptor }) => descriptor),
      exportedFiles
    }, null, 2), { encoding: 'utf8', mode: 0o600 })
    return exportPath
  }

  async #archiveOwnershipState(recovery: RecoveryRequired, requestId: string): Promise<void> {
    if (recovery.reason !== 'ownership-recovery-required') return
    const evidence = join(this.#dataRoot, 'recovery-evidence', requestId)
    await mkdir(evidence, { recursive: true })
    await moveIfPresent(
      `${recovery.durableDatabasePath}.owner`,
      join(evidence, `${basename(recovery.durableDatabasePath)}.owner`)
    )
    await moveIfPresent(
      `${recovery.durableDatabasePath}.owner.takeover.sqlite`,
      join(evidence, `${basename(recovery.durableDatabasePath)}.owner.takeover.sqlite`)
    )
  }

  async #preserveBeforeEmpty(recovery: RecoveryRequired, requestId: string): Promise<void> {
    const evidence = join(this.#dataRoot, 'recovery-evidence', requestId)
    await mkdir(evidence, { recursive: true })
    const paths = [
      recovery.durableDatabasePath,
      `${recovery.durableDatabasePath}-wal`,
      `${recovery.durableDatabasePath}-shm`
    ]
    for (const path of paths) {
      await moveIfPresent(path, join(evidence, basename(path)))
    }
  }
}

async function assertRecoveryStillActive(recovery: RecoveryRequired): Promise<void> {
  let marker: Partial<RecoveryRequired>
  try {
    marker = JSON.parse(await readFile(recovery.markerPath, 'utf8')) as Partial<RecoveryRequired>
  } catch (error) {
    if (errorCode(error) === 'ENOENT') {
      throw new Error('数据库恢复已由其他 Runtime 完成，本次操作已停止')
    }
    throw error
  }
  if (
    marker.reason !== recovery.reason ||
    marker.durableDatabasePath !== recovery.durableDatabasePath ||
    marker.quarantinedPath !== recovery.quarantinedPath ||
    marker.markerPath !== recovery.markerPath
  ) {
    throw new Error('数据库恢复状态已更新，请使用最新恢复页面重试')
  }
}

async function moveIfPresent(from: string, to: string): Promise<void> {
  if (!await exists(from)) return
  await rename(from, to)
}

async function exists(path: string): Promise<boolean> {
  try {
    await stat(path)
    return true
  } catch (error) {
    if (errorCode(error) === 'ENOENT') return false
    throw error
  }
}

async function isFile(path: string): Promise<boolean> {
  try {
    return (await stat(path)).isFile()
  } catch (error) {
    if (errorCode(error) === 'ENOENT') return false
    throw error
  }
}

function errorCode(error: unknown): string | undefined {
  return typeof error === 'object' && error !== null && 'code' in error
    ? String(error.code)
    : undefined
}
