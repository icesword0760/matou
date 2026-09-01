import { randomUUID } from 'node:crypto'
import { cp, mkdir, rename, rm, stat, writeFile } from 'node:fs/promises'
import { basename, join } from 'node:path'

import type { RuntimeRecoveryCommand } from '@matou/contracts'

import { DatabaseBackupService } from './database-backup-service'
import type { Migration } from './migration-runner'
import {
  openRecoverableRuntimeDatabase,
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
        await new DatabaseBackupService(this.#dataRoot).restore(
          command.backupId,
          recovery.durableDatabasePath
        )
        await this.#archiveOwnershipState(recovery, command.requestId)
        return { value: {}, bootstrap: await this.#openAfterAction(recovery, command.requestId) }
      case 'retry-open':
        if (!await isFile(recovery.durableDatabasePath)) {
          throw new Error('原数据库尚未回到可检查的位置，请先恢复备份或导出恢复资料')
        }
        return { value: {}, bootstrap: await this.#openAfterAction(recovery, command.requestId) }
      case 'start-empty-database':
        await this.#preserveBeforeEmpty(recovery, command.requestId)
        return { value: {}, bootstrap: await this.#openAfterAction(recovery, command.requestId) }
    }
  }

  async #openAfterAction(
    recovery: RecoveryRequired,
    requestId: string
  ): Promise<RuntimeDatabaseBootstrapResult> {
    const stagedMarker = `${recovery.markerPath}.action-${requestId}`
    await rm(stagedMarker, { force: true })
    await rename(recovery.markerPath, stagedMarker)
    try {
      const result = await openRecoverableRuntimeDatabase(
        this.#dataRoot,
        this.#migrations,
        this.#observer
      )
      await rm(stagedMarker, { force: true })
      return result
    } catch (error) {
      await rename(stagedMarker, recovery.markerPath).catch(() => undefined)
      throw error
    }
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
      `${recovery.durableDatabasePath}-shm`,
      `${recovery.durableDatabasePath}.owner`,
      `${recovery.durableDatabasePath}.owner.takeover.sqlite`
    ]
    for (const path of paths) {
      await moveIfPresent(path, join(evidence, basename(path)))
    }
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
