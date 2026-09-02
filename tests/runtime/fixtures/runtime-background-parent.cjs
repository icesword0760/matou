const { EventEmitter } = require('node:events')
const { writeFileSync } = require('node:fs')
const { randomUUID } = require('node:crypto')

if (process.env.MATOU_E2E !== '1') {
  throw new Error('background Runtime parent is reserved for MATOU_E2E')
}
const runtimeEntry = process.env.MATOU_E2E_RUNTIME_ENTRY
const readyPath = process.env.MATOU_E2E_RUNTIME_READY_PATH
if (!runtimeEntry || !readyPath) throw new Error('background Runtime fixture paths are required')

const parentPort = new EventEmitter()
parentPort.postMessage = (message) => {
  if (message?.type === 'runtime.lifecycle' && message.snapshot?.stage === 'ready') {
    writeFileSync(readyPath, JSON.stringify({ pid: process.pid, snapshot: message.snapshot }))
  }
  if (
    message?.type === 'runtime.recovery-details' &&
    process.env.MATOU_E2E_MIGRATION_AUTO_RECOVER === '1'
  ) {
    const recovery = message.recovery
    const selectedBackup = recovery.backups?.[0]
    if (!selectedBackup) throw new Error('Runtime recovery exposed no valid backup')
    const observationPath = process.env.MATOU_E2E_MIGRATION_RECOVERY_OBSERVATION
    if (observationPath) {
      writeFileSync(observationPath, JSON.stringify({
        recoveryId: recovery.recoveryId,
        quarantinedPath: recovery.quarantinedPath,
        backups: recovery.backups,
        selectedBackup
      }))
    }
    queueMicrotask(() => parentPort.emit('message', {
      data: {
        type: 'runtime.recovery-command',
        requestId: `migration-gate-${randomUUID()}`,
        action: 'restore-backup',
        backupId: selectedBackup.id,
        expectedRecoveryId: recovery.recoveryId
      },
      ports: []
    }))
  }
}
Object.defineProperty(process, 'parentPort', { value: parentPort })
require(runtimeEntry)
