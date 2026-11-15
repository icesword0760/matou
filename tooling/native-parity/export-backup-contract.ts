// Execute the frozen backup service with disposable databases, including real WAL data.
import { mkdtemp, readFile, readdir, rename, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { RuntimeDatabase } from '../../apps/runtime/src/storage/database'
import { DatabaseBackupService } from '../../apps/runtime/src/storage/database-backup-service'

const { DatabaseSync } = process.getBuiltinModule('node:sqlite') as typeof import('node:sqlite')
const temporary = await mkdtemp(join(tmpdir(), 'matou-backup-contract-'))
function inspect(path: string, table: string) {
  const db = new DatabaseSync(path, { readOnly: true })
  try { return { rows: db.prepare(`SELECT * FROM ${table}`).all(), integrity: db.prepare('PRAGMA integrity_check').all() } }
  finally { db.close() }
}
function oldDatabase(path: string) {
  const db = new DatabaseSync(path)
  db.exec("PRAGMA journal_mode=WAL; CREATE TABLE original_rows(value TEXT NOT NULL) STRICT;")
  db.prepare('INSERT INTO original_rows VALUES (?)').run('committed-old-wal')
  return db
}
const source = RuntimeDatabase.open(join(temporary, 'source.sqlite'))
source.exec('CREATE TABLE restored_rows(value TEXT NOT NULL) STRICT;')
source.run('INSERT INTO restored_rows VALUES (?)', 'from-backup')
const service = new DatabaseBackupService(temporary, { now: () => 2000 })
const descriptor = await service.create(source, 'clean-exit')
const wal = {
  descriptor: { ...descriptor, path: descriptor.path.replace(temporary, '<root>') },
  bytesBase64: (await readFile(descriptor.path)).toString('base64'),
  ...inspect(descriptor.path, 'restored_rows')
}
const created = [descriptor]
for (let i = 0; i < 8; i++) created.push(await service.create(source, 'clean-exit'))
await service.rotate()
const rotation = { created: created.map(({ id, createdAt }) => ({ id, createdAt })), retained: (await service.listValid()).map(({ id }) => id) }
const current = (await service.listValid())[0]!
const target = join(temporary, 'target.sqlite')
const old = oldDatabase(target)
await service.restore(current.id, target)
old.close()
const replaced = (await readdir(temporary)).find(name => /^target\.sqlite\.replaced-\d+$/.test(name))!
const sidecarsPresent = (await readdir(temporary)).filter(name => /^target\.sqlite-(wal|shm)$/.test(name))
const restored = {
  target: inspect(target, 'restored_rows'), rollback: inspect(join(temporary, replaced), 'original_rows'),
  sidecarsPresent,
}
const interruptions = []
for (const [targetExists, phase] of [[true, 'old-rollback'], [true, 'publish'], [false, 'publish']] as const) {
  const targetPath = join(temporary, `${targetExists}-${phase}.sqlite`)
  if (targetExists) oldDatabase(targetPath).close()
  let injected = false
  const broken = new DatabaseBackupService(temporary, {
    now: () => 4000,
    rename: async (from, to) => {
      const hit = phase === 'old-rollback' ? to.includes('.replaced-') && !to.endsWith('.partial') : to === targetPath
      if (!injected && hit) { injected = true; throw new Error(`injected ${phase} interruption`) }
      await rename(from, to)
    }
  })
  let error: string | undefined
  try { await broken.restore(current.id, targetPath) } catch (e) { error = (e as Error).message }
  interruptions.push({ targetExists, phase, error, canonical: inspect(targetPath, targetExists ? 'original_rows' : 'restored_rows') })
}
source.close()
const output = process.argv[2]!
await writeFile(output, JSON.stringify({ versions: process.versions, wal, rotation, restored, interruptions }, null, 2) + '\n')
console.log(JSON.stringify({ output, temporary, interruptedCases: interruptions.length }))
