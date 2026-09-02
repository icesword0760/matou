import { randomUUID } from 'node:crypto'
import { rename, stat, writeFile } from 'node:fs/promises'
import { join } from 'node:path'

import {
  acquireDatabaseOwner,
  releaseDatabaseOwner
} from './database-owner.ts'

const root = process.env.MATOU_OWNER_RACE_ROOT
const fixtureId = process.env.MATOU_OWNER_RACE_ID
if (!root || !fixtureId) throw new Error('owner race fixture environment is missing')

await writeFile(join(root, `ready-${fixtureId}`), 'ready')
await waitForFile(join(root, 'acquire-barrier'))

const ownerPath = join(root, 'matou.sqlite.owner')
const runtimeGeneration = randomUUID()
let owned = false
let error
try {
  acquireDatabaseOwner(ownerPath, runtimeGeneration)
  owned = true
} catch (caught) {
  error = caught instanceof Error ? caught.message : String(caught)
}
const resultPath = join(root, `result-${fixtureId}.json`)
const partialResultPath = `${resultPath}.partial`
await writeFile(partialResultPath, JSON.stringify({
  id: fixtureId,
  owned,
  runtimeGeneration: owned ? runtimeGeneration : undefined,
  error
}))
await rename(partialResultPath, resultPath)

if (owned) {
  await waitForFile(join(root, 'release-owner'))
  releaseDatabaseOwner(ownerPath, runtimeGeneration)
} else if (!error?.includes('database is already owned by a live Runtime')) {
  throw new Error(`unexpected owner rejection: ${error}`)
}

async function waitForFile(path) {
  const deadline = Date.now() + 5_000
  while (!(await stat(path).catch(() => undefined))?.isFile()) {
    if (Date.now() >= deadline) throw new Error(`file did not appear: ${path}`)
    await new Promise((resolve) => setTimeout(resolve, 10))
  }
}
