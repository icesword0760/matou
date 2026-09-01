import { openRecoverableRuntimeDatabase } from '../runtime-database-bootstrap'
import { FOUNDATION_MIGRATIONS } from '../migrations'

process.on('message', (message: unknown) => {
  if (!message || typeof message !== 'object' || !('dataRoot' in message)) return
  const dataRoot = String(message.dataRoot)
  void contendOnce(dataRoot)
})

async function contendOnce(dataRoot: string): Promise<void> {
  try {
    const result = await openRecoverableRuntimeDatabase(dataRoot, FOUNDATION_MIGRATIONS)
    if (result.kind === 'writable' || result.kind === 'read-only') result.database.close()
    sendAndExit({ kind: result.kind })
  } catch (error) {
    sendAndExit({
      kind: /already owned by a live Runtime/i.test(errorMessage(error))
        ? 'owner-conflict'
        : 'error',
      error: errorMessage(error)
    })
  }
}

function sendAndExit(message: unknown): void {
  process.send?.(message, () => process.exit(0))
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
