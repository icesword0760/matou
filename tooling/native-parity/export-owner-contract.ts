// Exercise the frozen ownership boundary with disposable paths and no app launch.
import { mkdtempSync, mkdirSync, writeFileSync, existsSync, lstatSync, readFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { acquireDatabaseOwner, releaseDatabaseOwner, readDatabaseOwner } from '../../apps/runtime/src/storage/database-owner'

const root = mkdtempSync(join(tmpdir(), 'matou-owner-contract-'))
const cases: unknown[] = []
for (const name of ['empty', 'live-file', 'stale-file', 'live-legacy', 'dead-legacy', 'malformed', 'invalid-pid', 'unusable-sidecar', 'generation-replaced']) {
  const directory = join(root, name); mkdirSync(directory)
  const owner = join(directory, 'matou.sqlite.owner')
  const live = name.startsWith('live')
  const initial = { pid: live ? process.pid : 2147483647, runtimeGeneration: 'previous' }
  if (name.includes('legacy')) {
    mkdirSync(owner); writeFileSync(join(owner, 'owner.json'), JSON.stringify(initial))
  } else if (name === 'malformed') writeFileSync(owner, '{')
  else if (name === 'invalid-pid') writeFileSync(owner, JSON.stringify({ pid: 0, runtimeGeneration: 'previous' }))
  else if (name === 'unusable-sidecar') { writeFileSync(owner, JSON.stringify(initial)); mkdirSync(`${owner}.takeover.sqlite`) }
  else if (name !== 'empty' && name !== 'generation-replaced') writeFileSync(owner, JSON.stringify(initial))
  let error: string | undefined, issue: string | undefined, code: string | undefined
  try { acquireDatabaseOwner(owner, 'native-contract') } catch (e: any) {
    error = e.message.replaceAll(directory, '<root>'); issue = e.issue; code = e.code
  }
  const normalize = (record: ReturnType<typeof readDatabaseOwner>) => record ? {
    ...record, pid: record.pid === process.pid ? '<self>' : record.pid,
  } : null
  const acquired = normalize(readDatabaseOwner(owner))
  const format = existsSync(owner) ? lstatSync(owner).isDirectory() ? 'directory' : 'file' : 'absent'
  if (name === 'generation-replaced') writeFileSync(owner, JSON.stringify({ pid: process.pid, runtimeGeneration: 'replacement' }))
  releaseDatabaseOwner(owner, 'native-contract')
  cases.push({ name, error, issue, code, acquired, format,
    afterRelease: normalize(readDatabaseOwner(owner)), remains: existsSync(owner) })
}
writeFileSync('apps/native/core/tests/fixtures/owner-reference.json', JSON.stringify({
  source: 'Frozen database-owner.ts on captured Electron; temporary paths; self pid normalized only.', cases
}, null, 2) + '\n')
console.log(JSON.stringify({ root, cases: cases.length }))
