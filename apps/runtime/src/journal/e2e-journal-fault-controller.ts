import { readFile } from 'node:fs/promises'
import type { FileHandle } from 'node:fs/promises'

import type { SegmentJournalOptions } from './segment-journal'

interface FaultEnvironment {
  MATOU_E2E?: string
  MATOU_E2E_JOURNAL_FAULT_CONTROL?: string
}

interface JournalFaultControl {
  sessionId?: unknown
  code?: unknown
}

export function createE2eJournalOptionsProvider(
  environment: FaultEnvironment
): ((sessionId: string) => SegmentJournalOptions) | undefined {
  const controlPath = environment.MATOU_E2E_JOURNAL_FAULT_CONTROL
  if (environment.MATOU_E2E !== '1' || !controlPath) return undefined

  return (sessionId) => ({
    writeFrame: async (handle, encoded) => {
      if (await injectsEnospc(controlPath, sessionId)) {
        throw Object.assign(
          new Error(`E2E Journal ENOSPC injection for ${sessionId}`),
          { code: 'ENOSPC' }
        )
      }
      await writeEntireFrame(handle, encoded)
    }
  })
}

async function injectsEnospc(controlPath: string, sessionId: string): Promise<boolean> {
  let contents: string
  try {
    contents = await readFile(controlPath, 'utf8')
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ENOENT') return false
    throw error
  }
  const control = JSON.parse(contents) as JournalFaultControl
  return control.sessionId === sessionId && control.code === 'ENOSPC'
}

async function writeEntireFrame(handle: FileHandle, encoded: Buffer): Promise<void> {
  let offset = 0
  while (offset < encoded.byteLength) {
    const { bytesWritten } = await handle.write(encoded, offset, encoded.byteLength - offset)
    if (bytesWritten <= 0) {
      throw Object.assign(new Error('journal frame write made no progress'), { code: 'EIO' })
    }
    offset += bytesWritten
  }
}
