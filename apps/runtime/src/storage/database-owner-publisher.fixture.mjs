import { writeFileSync } from 'node:fs'
import { join } from 'node:path'

import { publishDatabaseOwnerRecord } from './database-owner.ts'

const root = process.env.MATOU_OWNER_PUBLISH_ROOT
const stage = process.env.MATOU_OWNER_PUBLISH_STAGE
if (!root || !['prepared', 'published'].includes(stage)) {
  throw new Error('owner publisher fixture environment is missing')
}

publishDatabaseOwnerRecord(
  join(root, 'matou.sqlite.owner'),
  { pid: process.pid, runtimeGeneration: 'publisher-generation' },
  {
    onPrepared() {
      if (stage === 'prepared') pause('prepared')
    },
    onPublished() {
      if (stage === 'published') pause('published')
    }
  }
)

function pause(currentStage) {
  writeFileSync(join(root, `publisher-${currentStage}`), 'ready')
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0)
}
