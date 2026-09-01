import { randomUUID } from 'node:crypto'

import type { ParentPort } from 'electron'
import type { RuntimeMode, RuntimeRecoverySnapshot } from '@matou/contracts'

import type { RuntimeDatabaseBootstrapResult } from './storage/runtime-database-bootstrap'

type RecoveryRequired = Extract<RuntimeDatabaseBootstrapResult, { kind: 'recovery-required' }>

export class RuntimeLifecyclePublisher {
  readonly #parentPort: Pick<ParentPort, 'postMessage'>
  readonly #newId: () => string
  #recoveryId: string
  #revision = 0

  constructor(
    parentPort: Pick<ParentPort, 'postMessage'>,
    newId: () => string = randomUUID
  ) {
    this.#parentPort = parentPort
    this.#newId = newId
    this.#recoveryId = this.#newId()
  }

  opening(): void {
    this.#publish('normal', 'opening-database', 0)
  }

  openingNewAttempt(): void {
    this.#publish('normal', 'opening-database', 0)
  }

  recoveryRequired(recovery: RecoveryRequired): void {
    this.#parentPort.postMessage({
      type: 'runtime.recovery-details',
      recovery: {
        recoveryId: recovery.recoveryId,
        reason: recovery.reason,
        durableDatabasePath: recovery.durableDatabasePath,
        quarantinedPath: recovery.quarantinedPath,
        ...(recovery.ownershipIssue ? { ownershipIssue: recovery.ownershipIssue } : {}),
        backups: recovery.backups.map(({ path: _path, ...backup }) => backup),
        ...(recovery.backupListError || recovery.markerError || recovery.moveError
          ? {
              error: [
                recovery.backupListError,
                recovery.markerError,
                recovery.moveError
              ].filter(Boolean).map((item) => item!.message).join('；')
            }
          : {})
      }
    })
    this.#publish('recovery-required', 'opening-database', 0)
  }

  ready(mode: Exclude<RuntimeMode, 'recovery-required'>): void {
    this.#publish(mode, 'ready', 1)
  }

  #publish(
    mode: RuntimeMode,
    stage: RuntimeRecoverySnapshot['stage'],
    completed: number
  ): void {
    this.#revision += 1
    this.#parentPort.postMessage({
      type: 'runtime.lifecycle',
      snapshot: {
        recoveryId: this.#recoveryId,
        revision: this.#revision,
        mode,
        stage,
        completed,
        total: 1,
        failures: []
      }
    })
  }
}
