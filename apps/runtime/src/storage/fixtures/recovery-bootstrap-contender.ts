import { chmodSync, existsSync } from 'node:fs'

import { DatabaseRecoveryController } from '../database-recovery-controller'
import { openRecoverableRuntimeDatabase } from '../runtime-database-bootstrap'
import { FOUNDATION_MIGRATIONS } from '../migrations'

process.on('message', (message: unknown) => {
  if (!message || typeof message !== 'object' || !('dataRoot' in message)) return
  const dataRoot = String(message.dataRoot)
  if ('mode' in message && message.mode === 'recover-fault') {
    void recoverWithFault(
      dataRoot,
      'fault' in message ? String(message.fault) : 'file-sync'
    )
    return
  }
  if ('mode' in message && message.mode === 'claim-crash') {
    void crashAfterClaim(dataRoot)
    return
  }
  const barrierPath = 'barrierPath' in message ? String(message.barrierPath) : undefined
  void contendOnce(dataRoot, barrierPath)
})

async function contendOnce(dataRoot: string, barrierPath?: string): Promise<void> {
  try {
    if (barrierPath) {
      process.send?.({ kind: 'barrier-ready' })
      while (!existsSync(barrierPath)) {
        await new Promise((resolve) => setTimeout(resolve, 5))
      }
    }
    const result = await openRecoverableRuntimeDatabase(dataRoot, FOUNDATION_MIGRATIONS)
    if (result.kind === 'writable' || result.kind === 'read-only') result.database.close()
    sendAndExit({
      kind: result.kind,
      ...('recoveryId' in result ? { recoveryId: result.recoveryId } : {})
    })
  } catch (error) {
    sendAndExit({
      kind: /already owned by a live Runtime/i.test(errorMessage(error))
        ? 'owner-conflict'
        : 'error',
      error: errorMessage(error)
    })
  }
}

async function crashAfterClaim(dataRoot: string): Promise<void> {
  try {
    await openRecoverableRuntimeDatabase(dataRoot, FOUNDATION_MIGRATIONS, {
      async onRecoveryGenerationClaimPublished(claim) {
        await new Promise<void>(() => {
          process.send?.({ kind: 'claim-published', ...claim }, () => process.exit(0))
        })
      }
    })
    sendAndExit({ kind: 'error', error: 'bootstrap did not stop after durable claim' })
  } catch (error) {
    sendAndExit({ kind: 'error', error: errorMessage(error) })
  }
}

async function recoverWithFault(dataRoot: string, fault: string): Promise<void> {
  try {
    const recovery = await openRecoverableRuntimeDatabase(dataRoot, FOUNDATION_MIGRATIONS)
    if (recovery.kind !== 'recovery-required') {
      sendAndExit({ kind: recovery.kind, ok: false, error: 'expected recovery-required' })
      return
    }
    const controller = new DatabaseRecoveryController(
      dataRoot,
      FOUNDATION_MIGRATIONS,
      {},
      {
        markerFinalizationObserver: {
          beforeFileSync: () => {
            if (fault === 'file-sync') throw new Error('injected persistent file fsync failure')
          },
          beforePublish: () => {
            if (fault !== 'precommit-readonly') return
            chmodSync(dataRoot, 0o500)
            throw new Error('injected persistent precommit readonly failure')
          },
          afterPublish: () => {
            if (fault !== 'postcommit-readonly') return
            chmodSync(dataRoot, 0o500)
            throw new Error('injected persistent postcommit readonly failure')
          }
        }
      }
    )
    const outcome = await controller.execute(recovery, {
      type: 'runtime.recovery-command', requestId: `fault-${fault}`,
      action: 'retry-open', expectedRecoveryId: recovery.recoveryId
    }).then(
      () => ({ ok: true as const }),
      (error: unknown) => ({ ok: false as const, error: errorMessage(error) })
    )
    sendAndExit({ kind: 'recovery-result', recoveryId: recovery.recoveryId, ...outcome })
  } catch (error) {
    sendAndExit({ kind: 'error', ok: false, error: errorMessage(error) })
  }
}

function sendAndExit(message: unknown): void {
  process.send?.(message, () => process.exit(0))
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
