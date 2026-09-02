import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { createHash, randomUUID } from 'node:crypto'
import {
  mkdtemp,
  mkdir,
  readFile,
  readdir,
  rm,
  stat,
  writeFile
} from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { fileURLToPath } from 'node:url'
import { join, resolve } from 'node:path'
import test from 'node:test'

import { FOUNDATION_MIGRATIONS } from '../../apps/runtime/src/storage/migrations.ts'

const { DatabaseSync, backup } = process.getBuiltinModule('node:sqlite')
const projectRoot = fileURLToPath(new URL('../..', import.meta.url))
const runtimeEntry = resolve(projectRoot, 'apps/runtime/dist/index.cjs')
const runtimeParent = fileURLToPath(new URL(
  './fixtures/runtime-background-parent.cjs',
  import.meta.url
))

const pendingMigrations = FOUNDATION_MIGRATIONS.filter(({ version }) => version > 24)
const latestSchemaVersion = FOUNDATION_MIGRATIONS.at(-1).version
const interruptionCases = [
  { stage: 'pre-migration-backup-ready' },
  ...pendingMigrations.flatMap(({ version }) => [
    { stage: 'migration-transaction-prepared', migrationVersion: version },
    { stage: 'migration-committed', migrationVersion: version }
  ])
]

for (const { stage, migrationVersion } of interruptionCases) {
  test(`real Runtime recovers after SIGKILL at ${stage}${migrationVersion ? ` v${migrationVersion}` : ''}`, {
    timeout: 90_000
  }, async () => {
    const result = await runScenario(stage, false, migrationVersion)
    assert.equal(result.runtimeHost, 'background-process')
    assert.equal(result.killedStage, stage)
    assert.equal(result.killedWith, 'SIGKILL')
    assert.equal(result.databaseIntegrity, 'ok')
    assert.ok(result.validPreMigrationBackups >= 1)
    assert.deepEqual(result.assetCounts, expectedAssetCounts)
    assert.equal(result.schemaVersion, latestSchemaVersion)
    assert.equal(result.pendingMigrationRows, pendingMigrations.length)
    assert.equal(result.halfMigratedColumns, 0)
    assert.equal(result.forkOperationId, 'legacy-operation:session-child')
    assert.equal(result.forkSubmissionKey, 'legacy-submission:session-child')
    assert.equal(result.forkStage, 'succeeded')
    const interruptedVersion = stage === 'pre-migration-backup-ready'
      ? 24
      : stage === 'migration-committed' ? migrationVersion : migrationVersion - 1
    assert.equal(result.killedMigrationVersion, migrationVersion)
    assert.equal(result.interruptedSchemaVersion, interruptedVersion)
    assert.equal(result.interruptedLatestColumns, interruptedVersion >= 27 ? 11 : 0)
    assert.equal(result.interruptedStructuralIndex, interruptedVersion >= 28)
  })
}

test('real Runtime selects the next valid migration backup after the newest is damaged', {
  timeout: 90_000
}, async () => {
  const result = await runScenario('migration-transaction-prepared', true, 27)
  assert.equal(result.runtimeHost, 'background-process')
  assert.equal(result.databaseIntegrity, 'ok')
  assert.equal(result.damagedBackupOffered, false)
  assert.equal(result.selectedFallbackSchemaVersion, 24)
  assert.equal(result.recoveryEvidencePreserved, true)
  assert.equal(result.recoveryMarkerResolved, true)
  assert.equal(result.schemaVersion, latestSchemaVersion)
  assert.equal(result.pendingMigrationRows, pendingMigrations.length)
  assert.deepEqual(result.assetCounts, expectedAssetCounts)
})

const expectedAssetCounts = {
  workspaces: 1,
  tasks: 1,
  sessions: 2,
  domainEvents: 1,
  relationEvents: 1,
  currentRelations: 1
}

async function runScenario(stage, corruptNewest, migrationVersion) {
  const root = await mkdtemp(join(tmpdir(), 'matou-migration-gate-'))
  const databasePath = join(root, 'matou.sqlite')
  const reachedPath = join(root, 'fault-reached.json')
  const controlPath = join(root, 'fault-control.json')
  try {
    await createLegacyDatabase(databasePath, root)

    await writeFile(controlPath, JSON.stringify({
      stage, reachedPath, hold: true,
      ...(migrationVersion === undefined ? {} : { migrationVersion })
    }))
    const faulting = launchRuntime(root, { MATOU_E2E_MIGRATION_CONTROL: controlPath })
    const reached = await waitForJson(reachedPath, 30_000)
    assert.equal(reached.pid, faulting.child.pid)
    process.kill(faulting.child.pid, 'SIGKILL')
    const faultExit = await faulting.exited
    assert.equal(faultExit.signal, 'SIGKILL', faultExit.diagnostics)

    const interrupted = inspectMigrationState(databasePath)
    const validPreMigrationBackups = await countValidPreMigrationBackups(root)
    let fallback = {
      damagedBackupOffered: undefined,
      selectedFallbackSchemaVersion: undefined,
      recoveryEvidencePreserved: undefined,
      recoveryMarkerResolved: undefined
    }

    if (corruptNewest) {
      const newest = (await readBackupManifests(root))[0]
      assert.equal(newest?.reason, 'pre-migration')
      await corruptHeader(newest.path)
      await corruptHeader(databasePath)
      await rm(`${databasePath}-wal`, { force: true })
      await rm(`${databasePath}-shm`, { force: true })
      const recoveryObservationPath = join(root, 'recovery-observation.json')
      await startAndStopRuntime(root, {
        MATOU_E2E_MIGRATION_AUTO_RECOVER: '1',
        MATOU_E2E_MIGRATION_RECOVERY_OBSERVATION: recoveryObservationPath
      })
      const observation = JSON.parse(await readFile(recoveryObservationPath, 'utf8'))
      const resolvedPath = `${databasePath}.recovery.json.resolved-${observation.recoveryId}`
      const resolved = JSON.parse(await readFile(resolvedPath, 'utf8'))
      fallback = {
        damagedBackupOffered: observation.backups.some(({ id }) => id === newest.id),
        selectedFallbackSchemaVersion: observation.selectedBackup.schemaVersion,
        recoveryEvidencePreserved: await isFile(observation.quarantinedPath),
        recoveryMarkerResolved: resolved.state === 'resolved' &&
          resolved.recoveryId === observation.recoveryId
      }
    } else {
      await startAndStopRuntime(root)
    }

    return {
      runtimeHost: 'background-process',
      killedStage: stage,
      killedMigrationVersion: reached.migrationVersion,
      killedWith: faultExit.signal,
      interruptedSchemaVersion: interrupted.schemaVersion,
      interruptedLatestColumns: interrupted.latestColumns,
      interruptedStructuralIndex: interrupted.structuralIndex,
      validPreMigrationBackups,
      ...fallback,
      ...inspectFinalState(databasePath)
    }
  } finally {
    await rm(root, { recursive: true, force: true })
  }
}

async function startAndStopRuntime(dataRoot, extraEnvironment = {}) {
  const running = launchRuntime(dataRoot, extraEnvironment)
  await waitForFile(running.readyPath, 30_000, () => running.diagnostics())
  process.kill(running.child.pid, 'SIGTERM')
  const result = await withTimeout(running.exited, 30_000, 'Runtime did not stop cleanly')
  assert.equal(result.code, 0, result.diagnostics)
  assert.equal(result.signal, null, result.diagnostics)
}

function launchRuntime(dataRoot, extraEnvironment = {}) {
  const launchId = randomUUID()
  const readyPath = join(dataRoot, `runtime-ready-${launchId}.json`)
  const child = spawn(process.execPath, [runtimeParent], {
    cwd: projectRoot,
    env: {
      ...process.env,
      MATOU_E2E: '1',
      MATOU_DATA_DIR: dataRoot,
      MATOU_E2E_TERMINAL_DIAGNOSTICS: '0',
      MATOU_E2E_RUNTIME_ENTRY: runtimeEntry,
      MATOU_E2E_RUNTIME_READY_PATH: readyPath,
      ...extraEnvironment
    },
    stdio: ['ignore', 'pipe', 'pipe']
  })
  let output = ''
  child.stdout.setEncoding('utf8')
  child.stderr.setEncoding('utf8')
  child.stdout.on('data', (chunk) => { output = `${output}${chunk}`.slice(-65_536) })
  child.stderr.on('data', (chunk) => { output = `${output}${chunk}`.slice(-65_536) })
  const exited = new Promise((resolveExit, reject) => {
    child.once('error', reject)
    child.once('exit', (code, signal) => resolveExit({ code, signal, diagnostics: output }))
  })
  if (!child.pid) throw new Error('Runtime process did not start')
  return { child, readyPath, exited, diagnostics: () => output }
}

async function createLegacyDatabase(databasePath, dataRoot) {
  const database = new DatabaseSync(databasePath)
  try {
    database.exec(`
      CREATE TABLE schema_migrations (
        version INTEGER PRIMARY KEY,
        name TEXT NOT NULL,
        checksum TEXT NOT NULL,
        applied_at INTEGER NOT NULL
      ) STRICT;
    `)
    for (const migration of FOUNDATION_MIGRATIONS.filter(({ version }) => version <= 24)) {
      database.exec('BEGIN IMMEDIATE')
      try {
        database.exec(migration.sql)
        database.prepare(`
          INSERT INTO schema_migrations (version, name, checksum, applied_at)
          VALUES (?, ?, ?, ?)
        `).run(migration.version, migration.name, migrationChecksum(migration), migration.version)
        database.exec('COMMIT')
      } catch (error) {
        database.exec('ROLLBACK')
        throw error
      }
    }
    seedLegacyAssets(database, dataRoot)
    database.exec('PRAGMA wal_checkpoint(TRUNCATE)')
    const backupTimestamp = Date.now() - 10_000
    await createBackup(database, dataRoot, backupTimestamp)
    await createBackup(database, dataRoot, backupTimestamp + 1)
  } finally {
    database.close()
  }
}

function seedLegacyAssets(database, dataRoot) {
  try {
    database.exec('PRAGMA foreign_keys = ON; BEGIN IMMEDIATE;')
    database.prepare(`
      INSERT INTO workspaces (id, name, root_directory, created_at, updated_at)
      VALUES (?, ?, ?, ?, ?)
    `).run('workspace-preserved', 'Preserved Workspace', dataRoot, 1, 1)
    database.prepare(`
      INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
      VALUES (?, ?, 'plain-directory', ?, ?)
    `).run('context-preserved', 'workspace-preserved', dataRoot, 1)
    database.prepare(`
      INSERT INTO tasks (
        id, workspace_id, execution_context_id, title, status, created_at, updated_at
      ) VALUES (?, ?, ?, ?, 'completed', ?, ?)
    `).run('task-preserved', 'workspace-preserved', 'context-preserved', 'Preserved Task', 2, 2)
    for (const [id, title, createdAt] of [
      ['session-parent', 'Parent', 3],
      ['session-child', 'Child', 4]
    ]) {
      database.prepare(`
        INSERT INTO sessions (
          id, task_id, execution_context_id, kind, status, created_at, updated_at,
          last_activity_at, title, cwd, work_status
        ) VALUES (?, 'task-preserved', 'context-preserved', 'shell', 'exited', ?, ?, ?, ?, ?, 'exited')
      `).run(id, createdAt, createdAt, createdAt, title, dataRoot)
    }
    database.prepare(`
      INSERT INTO domain_events (
        event_id, event_type, aggregate_type, aggregate_id, workspace_id, task_id,
        session_id, payload_json, schema_version, command_id, occurred_at
      ) VALUES (?, ?, ?, ?, ?, ?, ?, '{}', 1, ?, ?)
    `).run(
      'domain-event-preserved', 'session.preserved', 'session', 'session-child',
      'workspace-preserved', 'task-preserved', 'session-child', 'command-preserved', 5
    )
    const relation = database.prepare(`
      INSERT INTO session_relation_events (
        event_id, relation_id, operation, task_id, from_session_id, to_session_id,
        relation_kind, metadata_json, command_id, occurred_at
      ) VALUES (?, ?, 'created', ?, ?, ?, 'derived-from', '{}', ?, ?)
    `).run(
      'relation-event-preserved', 'relation-preserved', 'task-preserved',
      'session-child', 'session-parent', 'relation-command-preserved', 6
    )
    database.prepare(`
      INSERT INTO session_relations_current (
        relation_id, task_id, from_session_id, to_session_id, relation_kind,
        metadata_json, created_at, updated_at, source_event_sequence
      ) VALUES (?, ?, ?, ?, 'derived-from', '{}', ?, ?, ?)
    `).run(
      'relation-preserved', 'task-preserved', 'session-child', 'session-parent',
      6, 6, Number(relation.lastInsertRowid)
    )
    database.prepare(`
      INSERT INTO session_fork_intents (
        session_id, source_session_id, source_provider, source_provider_session_id,
        state, created_at, completed_at, display_name, worktree_mode, attempt_count, updated_at
      ) VALUES (?, ?, 'claude-code', ?, 'succeeded', ?, ?, ?, 'current', 1, ?)
    `).run(
      'session-child', 'session-parent', 'provider-session-preserved',
      7, 7, 'Preserved Fork', 7
    )
    database.exec('COMMIT; PRAGMA wal_checkpoint(TRUNCATE);')
  } catch (error) {
    try { database.exec('ROLLBACK') } catch {}
    throw error
  }
}

async function createBackup(database, dataRoot, createdAt) {
  const directory = join(dataRoot, 'backups')
  await mkdir(directory, { recursive: true })
  const id = `matou-${createdAt}-clean-exit-v24`
  const path = join(directory, `${id}.sqlite`)
  await backup(database, path)
  const bytes = await readFile(path)
  await writeFile(join(directory, `${id}.json`), JSON.stringify({
    id,
    path,
    createdAt,
    reason: 'clean-exit',
    schemaVersion: 24,
    size: bytes.length,
    sha256: createHash('sha256').update(bytes).digest('hex')
  }))
}

function migrationChecksum(migration) {
  return createHash('sha256')
    .update(`${migration.version}\0${migration.name}\0${migration.sql}`)
    .digest('hex')
}

function inspectMigrationState(databasePath) {
  const database = new DatabaseSync(databasePath, { readOnly: true })
  try {
    return {
      schemaVersion: Number(database.prepare(
        'SELECT MAX(version) AS version FROM schema_migrations'
      ).get().version),
      latestColumns: latestMigrationColumns(database),
      structuralIndex: structuralIndexPresent(database)
    }
  } finally {
    database.close()
  }
}

function inspectFinalState(databasePath) {
  const database = new DatabaseSync(databasePath, { readOnly: true })
  try {
    const fork = database.prepare(`
      SELECT operation_id, submission_key, stage
      FROM session_fork_intents WHERE session_id = 'session-child'
    `).get()
    const columns = latestMigrationColumns(database)
    return {
      databaseIntegrity: String(Object.values(database.prepare('PRAGMA integrity_check').get())[0]),
      schemaVersion: Number(database.prepare(
        'SELECT MAX(version) AS version FROM schema_migrations'
      ).get().version),
      pendingMigrationRows: Number(database.prepare(
        'SELECT COUNT(*) AS count FROM schema_migrations WHERE version > 24'
      ).get().count),
      halfMigratedColumns: columns === 11 ? 0 : columns,
      assetCounts: {
        workspaces: count(database, 'workspaces'),
        tasks: count(database, 'tasks'),
        sessions: count(database, 'sessions'),
        domainEvents: count(database, 'domain_events'),
        relationEvents: count(database, 'session_relation_events'),
        currentRelations: count(database, 'session_relations_current')
      },
      forkOperationId: fork.operation_id,
      forkSubmissionKey: fork.submission_key,
      forkStage: fork.stage
    }
  } finally {
    database.close()
  }
}

function structuralIndexPresent(database) {
  return database.prepare(
    "SELECT 1 AS present FROM sqlite_master WHERE type = 'index' AND name = 'session_relations_structural_lookup_idx'"
  ).get()?.present === 1
}

function latestMigrationColumns(database) {
  const expected = new Set([
    'operation_id', 'submission_key', 'stage', 'completed_steps', 'total_steps',
    'attempt', 'lease_owner', 'lease_token', 'lease_expires_at', 'lease_fence',
    'last_heartbeat_at'
  ])
  return database.prepare('PRAGMA table_info(session_fork_intents)').all()
    .filter(({ name }) => expected.has(name)).length
}

function count(database, table) {
  return Number(database.prepare(`SELECT COUNT(*) AS count FROM ${table}`).get().count)
}

async function readBackupManifests(dataRoot) {
  const directory = join(dataRoot, 'backups')
  const names = (await readdir(directory)).filter((name) => name.endsWith('.json'))
  const manifests = await Promise.all(names.map(async (name) => (
    JSON.parse(await readFile(join(directory, name), 'utf8'))
  )))
  return manifests.sort((left, right) => right.createdAt - left.createdAt)
}

async function countValidPreMigrationBackups(dataRoot) {
  const manifests = await readBackupManifests(dataRoot)
  let count = 0
  for (const manifest of manifests.filter(({ reason }) => reason === 'pre-migration')) {
    const bytes = await readFile(manifest.path)
    if (
      bytes.length !== manifest.size ||
      createHash('sha256').update(bytes).digest('hex') !== manifest.sha256
    ) continue
    const database = new DatabaseSync(manifest.path, { readOnly: true })
    try {
      if (String(Object.values(database.prepare('PRAGMA integrity_check').get())[0]) === 'ok') count += 1
    } finally {
      database.close()
    }
  }
  return count
}

async function corruptHeader(path) {
  const bytes = await readFile(path)
  bytes.fill(0x5a, 0, Math.min(16, bytes.length))
  await writeFile(path, bytes)
}

async function isFile(path) {
  return (await stat(path).catch(() => undefined))?.isFile() === true
}

async function waitForJson(path, timeoutMs) {
  const deadline = Date.now() + timeoutMs
  for (;;) {
    try {
      return JSON.parse(await readFile(path, 'utf8'))
    } catch (error) {
      if (Date.now() >= deadline) throw error
      await new Promise((resolveWait) => setTimeout(resolveWait, 10))
    }
  }
}

async function waitForFile(path, timeoutMs, diagnostics) {
  const deadline = Date.now() + timeoutMs
  while (!await isFile(path)) {
    if (Date.now() >= deadline) {
      throw new Error(`Runtime did not become ready\n${diagnostics()}`)
    }
    await new Promise((resolveWait) => setTimeout(resolveWait, 10))
  }
}

function withTimeout(promise, timeoutMs, message) {
  return new Promise((resolveWait, reject) => {
    const timer = setTimeout(() => reject(new Error(message)), timeoutMs)
    promise.then(
      (value) => {
        clearTimeout(timer)
        resolveWait(value)
      },
      (error) => {
        clearTimeout(timer)
        reject(error)
      }
    )
  })
}
