// Execute frozen source services against disposable data and export their contracts.
// This is baseline tooling; the native runtime must not execute Node or this script.
import { createHash } from 'node:crypto'
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { performance } from 'node:perf_hooks'
import { RuntimeDatabase } from '../../apps/runtime/src/storage/database'
import { FOUNDATION_MIGRATIONS } from '../../apps/runtime/src/storage/migrations'
import { MigrationRunner } from '../../apps/runtime/src/storage/migration-runner'
import { DomainTransactionManager } from '../../apps/runtime/src/storage/domain-transaction'
import { HierarchyApplicationService } from '../../apps/runtime/src/hierarchy/hierarchy-application-service'

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
