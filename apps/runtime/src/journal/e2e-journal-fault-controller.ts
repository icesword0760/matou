import { readFile } from 'node:fs/promises'
import type { FileHandle } from 'node:fs/promises'

import type { SegmentJournalOptions } from './segment-journal'

interface FaultEnvironment {
  MATOU_E2E?: string
  MATOU_E2E_JOURNAL_FAULT_CONTROL?: string
  MATOU_E2E_JOURNAL_MAX_SEGMENT_BYTES?: string
  MATOU_E2E_JOURNAL_RAW_HOT_BYTES?: string
}

interface JournalFaultControl {
  sessionId?: unknown
  code?: unknown
}

export function createE2eJournalOptionsProvider(
  environment: FaultEnvironment
): ((sessionId: string) => SegmentJournalOptions) | undefined {
  const controlPath = environment.MATOU_E2E_JOURNAL_FAULT_CONTROL
  if (environment.MATOU_E2E !== '1') return undefined
  const maxSegmentBytes = optionalPositiveInteger(
    environment.MATOU_E2E_JOURNAL_MAX_SEGMENT_BYTES,
    'MATOU_E2E_JOURNAL_MAX_SEGMENT_BYTES',
    128
  )
  const rawHotBytes = optionalPositiveInteger(
    environment.MATOU_E2E_JOURNAL_RAW_HOT_BYTES,
    'MATOU_E2E_JOURNAL_RAW_HOT_BYTES',
    1
  )
  if (!controlPath && maxSegmentBytes === undefined && rawHotBytes === undefined) return undefined

  return (sessionId) => ({
    ...(maxSegmentBytes === undefined ? {} : { maxSegmentBytes }),
    ...(rawHotBytes === undefined ? {} : { rawHotBytes }),
    ...(controlPath ? {
      writeFrame: async (handle, encoded) => {
        if (await injectsEnospc(controlPath, sessionId)) {
          throw Object.assign(
            new Error(`E2E Journal ENOSPC injection for ${sessionId}`),
            { code: 'ENOSPC' }
          )
        }
        await writeEntireFrame(handle, encoded)
      }
    } : {})
  })
}

function optionalPositiveInteger(
  value: string | undefined,
  name: string,
  minimum: number
): number | undefined {
  if (value === undefined) return undefined
  const parsed = Number(value)
  if (!Number.isSafeInteger(parsed) || parsed < minimum) {
    throw new Error(`${name} must be an integer greater than or equal to ${minimum}`)
  }
  return parsed
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
