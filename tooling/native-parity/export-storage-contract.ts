// Execute frozen source services against disposable data and export their contracts.
// This is baseline tooling; the native runtime must not execute Node or this script.
import { createHash } from 'node:crypto'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { performance } from 'node:perf_hooks'
import { RuntimeDatabase } from '../../apps/runtime/src/storage/database'
import { FOUNDATION_MIGRATIONS } from '../../apps/runtime/src/storage/migrations'
import { MigrationRunner } from '../../apps/runtime/src/storage/migration-runner'
import { DomainTransactionManager } from '../../apps/runtime/src/storage/domain-transaction'
import { HierarchyApplicationService } from '../../apps/runtime/src/hierarchy/hierarchy-application-service'
import type { DomainCommandMetadata, DomainEventInput } from '../../packages/domain/src'

const gate = process.env.MATOU_STORAGE_GATE
if (gate) {
  await exportFixedStorageCases(gate)
  process.exit(0)
}

const output = resolve('artifacts/native-parity/baseline/storage-contract')
mkdirSync(output, { recursive: true })
const temporary = mkdtempSync(join(tmpdir(), 'matou-native-storage-contract-'))
const workspace = join(temporary, 'workspace')
mkdirSync(workspace)
const migrations = FOUNDATION_MIGRATIONS.map(m => ({ ...m,
  checksum: `v2:${createHash('sha256').update(`${m.version}\0${m.sql}`).digest('hex')}`,
  legacyChecksum: createHash('sha256').update(`${m.version}\0${m.name}\0${m.sql}`).digest('hex')
}))
const samples: { migrationMs: number; bootstrapMs: number; replayMs: number; reopenBootstrapMs: number }[] = []
let contract: unknown
for (let sample = 0; sample < 10; sample++) {
  const dbPath = join(temporary, `sample-${sample}.sqlite`)
  let db = RuntimeDatabase.open(dbPath)
  let start = performance.now()
  const migration = await new MigrationRunner(db, FOUNDATION_MIGRATIONS).migrate()
  const migrationMs = performance.now() - start
  const command = { commandId: 'contract-bootstrap', commandType: 'hierarchy.bootstrap-window', requestHash: 'contract-bootstrap-v1' }
  const input = { windowId: 'main-window-1', defaultRootDirectory: workspace, defaultName: 'workspace', now: 1000 }
  let hierarchy = new HierarchyApplicationService(db, new DomainTransactionManager(db))
  start = performance.now()
  const first = hierarchy.bootstrapWindow(command, input)
  const bootstrapMs = performance.now() - start
  start = performance.now()
  const replay = hierarchy.bootstrapWindow(command, input)
  const replayMs = performance.now() - start
  if (JSON.stringify(first) !== JSON.stringify(replay)) throw new Error('Source bootstrap replay changed response')
  if (sample === 0) {
    const contractDb = RuntimeDatabase.open(join(temporary, 'transaction.sqlite'))
    await new MigrationRunner(contractDb, FOUNDATION_MIGRATIONS).migrate()
    const tx = new DomainTransactionManager(contractDb)
    const metadata = { commandId: 'transaction-contract', commandType: 'parity.contract', requestHash: 'same-request', causationId: 'cause-1', correlationId: 'correlation-1' }
    const result = tx.execute(metadata, ({ tx, emit }) => {
      tx.run('INSERT INTO _runtime_meta (key, value) VALUES (?, ?)', 'parity-contract', 'committed')
      emit({ eventId: 'contract-event-1', eventType: 'workspace.parity-contract', aggregateType: 'workspace', aggregateId: 'workspace-contract', payload: { value: '中文🙂', retained: true }, occurredAt: 1100 })
      return { outcome: 'committed', count: 1 }
    })
    const replayed = tx.execute(metadata, () => { throw new Error('Replayed command must not run mutation') })
    let conflict = ''
    try { tx.execute({ ...metadata, requestHash: 'different-request' }, () => null) } catch (e) { conflict = (e as Error).message }
    let rollback = ''
    try {
      tx.execute({ commandId: 'rollback-contract', commandType: 'parity.contract', requestHash: 'rollback' }, ({ tx, emit }) => {
        tx.run('INSERT INTO _runtime_meta (key, value) VALUES (?, ?)', 'must-rollback', 'bad')
        emit({ eventId: 'must-not-commit', eventType: 'workspace.parity-contract', aggregateType: 'workspace', aggregateId: 'workspace-contract', payload: {}, occurredAt: 1200 })
        throw new Error('forced mutation failure')
      })
    } catch (e) { rollback = (e as Error).message }
    contract = {
      migration, pragmas: db.pragmas(), schema: db.all("SELECT type, name, tbl_name, sql FROM sqlite_master WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name"),
      bootstrap: first, bootstrapEvents: db.all("SELECT event_type, aggregate_type, payload_json FROM domain_events WHERE command_id = 'contract-bootstrap' ORDER BY seq"),
      transaction: { result, replayed, conflict, rollback,
        events: contractDb.all("SELECT * FROM domain_events WHERE command_id = 'transaction-contract' ORDER BY seq"),
        dedup: contractDb.get("SELECT command_id, command_type, request_hash, response_json, first_event_seq, last_event_seq FROM command_deduplication WHERE command_id = 'transaction-contract'"),
        rollbackRowPresent: Boolean(contractDb.get("SELECT 1 FROM _runtime_meta WHERE key = 'must-rollback'")),
        rollbackEventPresent: Boolean(contractDb.get("SELECT 1 FROM domain_events WHERE event_id = 'must-not-commit'")),
        rollbackCommandPresent: Boolean(contractDb.get("SELECT 1 FROM command_deduplication WHERE command_id = 'rollback-contract'")) }
    }
    contractDb.close()
  }
  db.close()
  db = RuntimeDatabase.open(dbPath)
  await new MigrationRunner(db, FOUNDATION_MIGRATIONS).migrate()
  hierarchy = new HierarchyApplicationService(db, new DomainTransactionManager(db))
  start = performance.now()
  const reopened = hierarchy.bootstrapWindow({ ...command, commandId: 'after-reopen' }, { ...input, now: 2000 })
  const reopenBootstrapMs = performance.now() - start
  if (first.workspace?.id !== reopened.workspace?.id || first.session?.id !== reopened.session?.id) throw new Error('Source reopen changed identity')
  samples.push({ migrationMs, bootstrapMs, replayMs, reopenBootstrapMs })
  db.close()
}
writeFileSync(join(output, 'migrations.json'), JSON.stringify(migrations, null, 2) + '\n')
writeFileSync(join(output, 'reference.json'), JSON.stringify({ source: 'frozen TypeScript storage and hierarchy services', executable: process.execPath, versions: process.versions, temporary, workspace, contract, samples,
  scope: 'Source service + real SQLite, not GUI startup/input/render or PTY. Ten fresh databases; OS caches not cleared.' }, null, 2) + '\n')
console.log(JSON.stringify({ output, temporary, migrations: migrations.length, samples }))

interface FixedStorageInputs {
  catalog: string
  backupPath: string
  queries: Record<string, string>
  groups: {
    id: string
    seedThrough?: number
    setup?: string[]
    steps: {
      id: string
      at: number
      reopen?: boolean
      setup?: string[]
      historyEdits?: {
        version: number
        name: string | null
        checksum: 'checksum' | 'legacyChecksum' | 'acceptedLegacyChecksums'
      }[]
      migration?: { backup: 'none' | 'ready' | 'fail' }
      transaction?: {
        command: DomainCommandMetadata
        sql?: string
        events?: DomainEventInput[]
        failure?: string
        result: unknown
      }
      before?: string[]
      read: string[]
    }[]
  }[]
}

interface SeedMigration {
  version: number
  name: string
  sql: string
  checksum: string
  legacyChecksum: string
  acceptedLegacyChecksums?: string[]
}

function describeStorageError(error: unknown) {
  if (!(error instanceof Error)) throw error
  const { code, errcode, errstr } = error as Error & {
    code?: string
    errcode?: number
    errstr?: string
  }
  return {
    message: error.message,
    ...(code && code !== 'ERR_SQLITE_ERROR' ? { code } : {}),
    ...(errcode === undefined ? {} : { sqliteCode: errcode }),
    driver: { name: error.name, code, errcode, errstr }
  }
}

async function exportFixedStorageCases(directory: string) {
  const inputs: FixedStorageInputs = JSON.parse(readFileSync(join(directory, 'inputs.json'), 'utf8'))
  const catalog: SeedMigration[] = JSON.parse(readFileSync(inputs.catalog, 'utf8'))
  const cases: Record<string, unknown> = {}
  const temporary = mkdtempSync(join(directory, '.work', 'frozen-'))
  const realNow = Date.now
  try {
    for (const group of inputs.groups) {
      const path = join(temporary, `${group.id}.sqlite`)
      let db = RuntimeDatabase.open(path)
      const read = (keys: string[] = []) =>
        Object.fromEntries(keys.map(key => [key, db.all(inputs.queries[key])]))
      try {
        if (group.seedThrough !== undefined) {
          db.exec('CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, name TEXT NOT NULL, checksum TEXT NOT NULL, applied_at INTEGER NOT NULL) STRICT')
          for (const migration of catalog.slice(0, group.seedThrough)) {
            db.exec(migration.sql)
            db.run('INSERT INTO schema_migrations VALUES (?, ?, ?, 1000)', migration.version, migration.name, migration.checksum)
          }
        }
        for (const sql of group.setup ?? []) db.exec(sql)
        for (const step of group.steps) {
          if (step.reopen) {
            db.close()
            db = RuntimeDatabase.open(path)
          }
          for (const edit of step.historyEdits ?? []) {
            const entry = catalog.find(migration => migration.version === edit.version)
            if (!entry) throw new Error(`missing seed migration ${edit.version}`)
            const checksum = edit.checksum === 'acceptedLegacyChecksums'
              ? entry.acceptedLegacyChecksums?.[0]
              : entry[edit.checksum]
            if (!checksum) throw new Error(`missing ${edit.checksum} for seed migration ${edit.version}`)
            db.run('UPDATE schema_migrations SET name = COALESCE(?, name), checksum = ? WHERE version = ?', edit.name, checksum, edit.version)
          }
          for (const sql of step.setup ?? []) db.exec(sql)
          const before = read(step.before)
          let callbackCalls = 0
          let outcome: unknown
          Date.now = () => step.at
          try {
            let value: unknown
            if (step.migration) {
              const backup = step.migration.backup
              const backups = backup === 'none' ? undefined : {
                async create() {
                  callbackCalls += 1
                  if (backup === 'fail') throw new Error('fixture backup refused')
                  return {
                    id: 'fixture', path: inputs.backupPath, createdAt: step.at,
                    reason: 'pre-migration' as const, schemaVersion: 0, size: 0, sha256: ''
                  }
                },
                async rotate() {}
              }
              value = await new MigrationRunner(db, FOUNDATION_MIGRATIONS, backups).migrate()
            } else if (step.transaction) {
              const transaction = step.transaction
              value = new DomainTransactionManager(db).execute(transaction.command, ({ tx, emit }) => {
                callbackCalls += 1
                if (transaction.sql) tx.exec(transaction.sql)
                for (const event of transaction.events ?? []) emit(event)
                if (transaction.failure) throw new Error(transaction.failure)
                return transaction.result
              })
            } else {
              throw new Error(`missing operation for ${step.id}`)
            }
            outcome = { kind: 'ok', value }
          } catch (error) {
            outcome = { kind: 'error', error: describeStorageError(error) }
          } finally {
            Date.now = realNow
          }
          cases[step.id] = { outcome, callbackCalls, before, after: read(step.read) }
        }
      } finally {
        db.close()
      }
    }
  } finally {
    rmSync(temporary, { recursive: true, force: true })
  }
  writeFileSync(join(directory, 'frozen.json'), JSON.stringify(cases, null, 2) + '\n')
}
