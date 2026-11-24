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

// Opt-in gate cases. The original snapshot and its invocation stay compatible.
if (process.argv[3] === '--gate') {
  await exportGateCases(output, process.argv[4]!, process.argv[5]!)
}

async function exportGateCases(output: string, inputFile: string, gateRoot: string) {
  const { createHash } = await import('node:crypto')
  const { chmod, mkdir, rm, stat } = await import('node:fs/promises')
  const { pathToFileURL } = await import('node:url')
  const { DatabaseLifecycleService } = await import('../../apps/runtime/src/storage/database-lifecycle-service')
  const { assertStartupIntegrity } = await import('../../apps/runtime/src/storage/database-integrity')
  const input = JSON.parse(await readFile(inputFile, 'utf8'))
  const clock = () => input.timestamp as number
  const restoreClock = () => input.restoreTimestamp as number
  const cases: Record<string, unknown> = {}

  async function inCase(name: string, run: (root: string, db: RuntimeDatabase) => Promise<unknown>) {
    const root = join(gateRoot, name)
    await mkdir(root, { recursive: true })
    const db = RuntimeDatabase.open(join(root, 'source.sqlite'))
    try {
      db.exec(input.seedSql)
      const checkpoint = db.get('PRAGMA wal_checkpoint(TRUNCATE)')
      if (Object.values(checkpoint ?? {}).join() !== '0,0,0') {
        throw new Error('fixture checkpoint did not truncate the WAL')
      }
      cases[name] = await run(root, db)
    } finally {
      db.close()
    }
  }

  function clean(value: string, root: string) {
    return value.replaceAll(root, '<root>').replace(/(\.restore-\d+-)[0-9a-f-]{36}/g, '$1<uuid>')
  }

  async function errorOf(root: string, operation: () => void | Promise<unknown>) {
    try {
      await operation()
      return null
    } catch (error) {
      return clean((error as Error).message, root)
    }
  }

  function readSqlite(path: string) {
    const url = pathToFileURL(path)
    url.searchParams.set('immutable', '1')
    const db = new DatabaseSync(url, { readOnly: true })
    try {
      return {
        integrity: db.prepare('PRAGMA integrity_check').all().map(row => Object.values(row)[0]),
        rows: db.prepare('SELECT value FROM marker ORDER BY rowid').all().map(row => row.value)
      }
    } finally {
      db.close()
    }
  }

  async function directory(root: string, relative: string, prefix = '') {
    const dir = join(root, relative)
    const names = (await readdir(dir)).filter(name => name.startsWith(prefix)).sort()
    return Promise.all(names.map(async name => {
      const path = join(dir, name)
      const metadata = await stat(path)
      const file = { name: clean(name, root), kind: metadata.isDirectory() ? 'directory' : 'file' }
      if (!metadata.isFile()) return file
      const details = { ...file, bytes: metadata.size, mode: (metadata.mode & 0o777).toString(8) }
      if (name.endsWith('.json')) {
        const raw = await readFile(path, 'utf8')
        const manifest = JSON.parse(raw)
        const bytes = await readFile(manifest.path)
        const sha256Matches = createHash('sha256').update(bytes).digest('hex') === manifest.sha256
        return { ...details, text: clean(raw, root), sha256Matches, sizeMatches: bytes.length === manifest.size }
      }
      if (/\.sqlite(?:\.replaced-\d+)?$/.test(name)) {
        const bytes = await readFile(path)
        return { ...details, header: [...bytes.subarray(18, 20)], ...readSqlite(path) }
      }
      return details
    }))
  }

  async function walState(root: string) {
    const path = join(root, 'source.sqlite')
    const walBytes = await stat(`${path}-wal`).then(info => info.size, (error: NodeJS.ErrnoException) => {
      if (error.code === 'ENOENT') return null
      throw error
    })
    return { walBytes, mainRows: readSqlite(path).rows }
  }

  async function ids(service: DatabaseBackupService) {
    return (await service.listValid()).map(item => item.id)
  }

  for (const name of ['normal', 'wal', 'rotation', 'sidecar']) {
    await inCase(name, async (root, db) => {
      if (name === 'wal') db.exec(input.walSql)
      if (name === 'sidecar') db.exec(input.sidecarSql)
      const service = new DatabaseBackupService(root, { now: clock })
      const created: string[] = []
      if (name === 'rotation') {
        for (let index = 1; index < input.rotationCount; index++) {
          created.push((await service.create(db, 'pre-migration')).id)
        }
      }
      const before = await walState(root)
      let afterBackup
      const lifecycle = new DatabaseLifecycleService(db, {
        async create(database, reason) {
          const result = await service.create(database, reason)
          created.push(result.id)
          return result
        },
        async rotate() {
          await service.rotate()
          afterBackup = await walState(root)
        }
      })
      const started = performance.now()
      const error = await errorOf(root, () => lifecycle.closeCleanly())
      const withinBound = performance.now() - started <= 5000
      return {
        error, withinBound, created, retained: await ids(service),
        before, afterBackup, afterClose: await walState(root), files: await directory(root, 'backups')
      }
    })
  }

  await inCase('invalid', async (root, db) => {
    const service = new DatabaseBackupService(root, { now: clock })
    const previous = await service.create(db, 'pre-migration')
    const editor = new DatabaseSync(join(root, 'source.sqlite'))
    try {
      editor.enableDefensive(false)
      editor.exec(input.invalidSql)
    } finally {
      editor.close()
    }
    const integrity = db.all('PRAGMA integrity_check').map(row => Object.values(row)[0])
    const startupError = await errorOf(root, () => assertStartupIntegrity(db))
    const error = await errorOf(root, () => new DatabaseLifecycleService(db, service).closeCleanly())
    // Corrupt a valid candidate's size/hash independently of the failed create.
    const file = join(root, 'backups', `${previous.id}.json`)
    const manifest = await readFile(file, 'utf8')
    const invalidCandidates = []
    for (const change of [{ size: previous.size + 1 }, { sha256: '0'.repeat(64) }]) {
      await writeFile(file, JSON.stringify({ ...previous, ...change }))
      invalidCandidates.push({
        field: Object.keys(change)[0],
        retained: await ids(service),
        error: await errorOf(root, () => service.restore(previous.id, join(root, 'target.sqlite')))
      })
    }
    await writeFile(file, manifest)
    return { integrity, startupError, error, invalidCandidates, retained: await ids(service), files: await directory(root, 'backups') }
  })

  for (const name of ['existing-target', 'rollback']) {
    await inCase(name, async (root, db) => {
      const service = new DatabaseBackupService(root, { now: clock })
      const backup = await service.create(db, 'clean-exit')
      const target = join(root, 'target.sqlite')
      const old = new DatabaseSync(target)
      old.exec(input.oldTargetSql)
      try {
        const oldWalBytes = (await stat(`${target}-wal`)).size
        if (name === 'rollback') await mkdir(`${target}.replaced-${restoreClock()}`)
        const restorer = new DatabaseBackupService(root, { now: restoreClock })
        const error = await errorOf(root, () => restorer.restore(backup.id, target))
        const beforeOldClose = (await readdir(root)).filter(name => /^target\.sqlite-(wal|shm)$/.test(name)).sort()
        old.close()
        return {
          error, oldWalBytes, beforeOldClose, retained: await ids(service),
          files: await directory(root, '.', 'target.sqlite'), backups: await directory(root, 'backups')
        }
      } finally {
        if (old.isOpen) old.close()
      }
    })
  }

  for (const name of ['disk-publication', 'permission']) {
    await inCase(name, async (root, db) => {
      const service = new DatabaseBackupService(root, { now: clock })
      const previous = await service.create(db, 'pre-migration')
      const previousBytes = await readFile(previous.path)
      db.exec(input.walSql)
      const backupDir = join(root, 'backups')
      if (name === 'disk-publication') await mkdir(join(backupDir, `matou-${clock() + 1}-clean-exit-v0.json`))
      else await chmod(backupDir, 0o500)
      let error
      try {
        error = await errorOf(root, () => new DatabaseLifecycleService(db, service).closeCleanly())
      } finally {
        if (name === 'permission') await chmod(backupDir, 0o755)
      }
      return {
        error, sourceAfterFailure: await walState(root),
        previousUnchanged: previousBytes.equals(await readFile(previous.path)),
        retained: await ids(service), files: await directory(root, 'backups')
      }
    })
  }

  await inCase('timeout', async (root, db) => {
    db.exec(input.copySql)
    const service = new DatabaseBackupService(root, { now: clock })
    const pageCount = Object.values(db.get('PRAGMA page_count')!)[0]
    let copied = false
    let entered = false
    let release!: () => void
    let finish!: () => void
    const resumed = new Promise<void>(resolve => { release = resolve })
    const completed = new Promise<void>(resolve => { finish = resolve })
    const copy = db.backupTo.bind(db)
    db.backupTo = async path => {
      entered = true
      await copy(path)
      copied = true
      await resumed
    }
    const lifecycle = new DatabaseLifecycleService(db, {
      create: (database, reason) => service.create(database, reason),
      async rotate() {
        await service.rotate()
        finish()
      }
    }, { timeoutMs: input.budgetMs })
    // The production timer is unref'ed. Keep the process alive during the probe.
    const watchdog = setTimeout(() => release(), input.upperBoundMs)
    const started = performance.now()
    const error = await errorOf(root, () => lifecycle.closeCleanly())
    const withinBound = performance.now() - started <= input.upperBoundMs
    const sourceErrorOnReturn = await errorOf(root, () => db.get('SELECT 1'))
    const filesOnReturn = await directory(root, 'backups')
    release()
    clearTimeout(watchdog)
    let settleTimer: ReturnType<typeof setTimeout> | undefined
    try {
      await Promise.race([
        completed,
        new Promise<never>((_, reject) => {
          settleTimer = setTimeout(() => reject(new Error('pending backup did not settle')), input.upperBoundMs)
        })
      ])
    } finally {
      clearTimeout(settleTimer)
    }
    if (!error || !entered || !copied || pageCount <= 100 || !withinBound) throw new Error('invalid copy-timeout probe')
    return {
      error, withinBound, pageCount, sourceErrorOnReturn, filesOnReturn,
      retained: await ids(service), files: await directory(root, 'backups')
    }
  })

  await writeFile(output, JSON.stringify({ sqlite: process.versions.sqlite, cases }, null, 2) + '\n')
}
