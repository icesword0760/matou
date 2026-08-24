import { createHash } from 'node:crypto'
import { readFile, stat } from 'node:fs/promises'
import { join } from 'node:path'

import { parseKookyMutation, type KookyMutationEnvelope } from '@matou/contracts'

import type { RuntimeDatabase } from '../../storage/database'
import type { DomainTransactionManager } from '../../storage/domain-transaction'
import { KookyImporter, legacyIdFor, readKookySnapshot } from './kooky-importer'

interface MirrorResult {
  legacyWritten: true
  shadowApplied: boolean
  repairQueued: boolean
  error?: string
}

export class ShadowWriteBridge {
  readonly #database: RuntimeDatabase
  readonly #transactions: DomainTransactionManager
  readonly #importer: KookyImporter

  constructor(database: RuntimeDatabase, transactions: DomainTransactionManager, importer: KookyImporter) {
    this.#database = database
    this.#transactions = transactions
    this.#importer = importer
  }

  async bootstrap(sourceId: string, sourceRoot: string): Promise<ReturnType<KookyImporter['importSource']> extends Promise<infer T> ? T : never> {
    const result = await this.#importer.importSource(sourceRoot)
    const metadataPath = join(sourceRoot, 'journals', 'metadata.ndjson')
    const size = await fileSize(metadataPath)
    this.#database.run(
      `INSERT INTO legacy_source_cursors (
         source_id, source_fingerprint, metadata_offset, checkpoint_fingerprint, updated_at
       ) VALUES (?, ?, ?, ?, ?)
       ON CONFLICT(source_id) DO UPDATE SET source_fingerprint = excluded.source_fingerprint,
         metadata_offset = MAX(legacy_source_cursors.metadata_offset, excluded.metadata_offset),
         checkpoint_fingerprint = excluded.checkpoint_fingerprint, updated_at = excluded.updated_at`,
      sourceId, result.sourceFingerprint, size, result.sourceFingerprint, Date.now()
    )
    return result
  }

  async mirrorMutation(
    raw: KookyMutationEnvelope,
    legacyWriter: (mutation: KookyMutationEnvelope) => Promise<void>
  ): Promise<MirrorResult> {
    this.#assertNotRetired()
    const mutation = parseKookyMutation(raw)
    await legacyWriter(mutation)
    try {
      this.#apply(mutation)
      return { legacyWritten: true, shadowApplied: true, repairQueued: false }
    } catch (error) {
      this.#queueRepair(mutation, errorMessage(error))
      return {
        legacyWritten: true, shadowApplied: false, repairQueued: true,
        error: errorMessage(error)
      }
    }
  }

  async tailMetadata(sourceId: string, sourceRoot: string): Promise<{
    applied: number; queued: number; ignored: number; pendingBytes: number; repairQueueDepth: number
  }> {
    this.#assertNotRetired()
    const path = join(sourceRoot, 'journals', 'metadata.ndjson')
    const cursor = this.#database.get<{ metadata_offset: number }>(
      'SELECT metadata_offset FROM legacy_source_cursors WHERE source_id = ?', sourceId
    )?.metadata_offset ?? 0
    let raw: Buffer
    try { raw = await readFile(path) } catch { raw = Buffer.alloc(0) }
    const start = Math.min(cursor, raw.byteLength)
    const tail = raw.subarray(start)
    const lastNewline = tail.lastIndexOf(0x0a)
    if (lastNewline < 0) {
      return { applied: 0, queued: 0, ignored: 0, pendingBytes: tail.byteLength, repairQueueDepth: this.#queueDepth() }
    }
    const complete = tail.subarray(0, lastNewline + 1)
    let relativeOffset = 0
    let applied = 0
    let queued = 0
    let ignored = 0
    for (const encodedLine of complete.toString('utf8').split('\n')) {
      const byteLength = Buffer.byteLength(encodedLine) + 1
      if (!encodedLine.trim()) { relativeOffset += byteLength; continue }
      try {
        const event = JSON.parse(encodedLine) as { type?: unknown; ts?: unknown; payload?: unknown }
        const commandId = `legacy-tail-${createHash('sha256').update(`${sourceId}:${start + relativeOffset}:`).update(encodedLine).digest('hex')}`
        const mutation = parseKookyMutation({
          schemaVersion: 1, commandId, type: event.type,
          timestamp: Number.isFinite(event.ts) ? event.ts : Date.now(), payload: event.payload
        })
        try { this.#apply(mutation); applied += 1 } catch (error) { this.#queueRepair(mutation, errorMessage(error)); queued += 1 }
      } catch {
        ignored += 1
      }
      relativeOffset += byteLength
    }
    const nextOffset = start + complete.byteLength
    this.#database.run(
      `INSERT INTO legacy_source_cursors (source_id, source_fingerprint, metadata_offset, updated_at)
       VALUES (?, ?, ?, ?)
       ON CONFLICT(source_id) DO UPDATE SET metadata_offset = excluded.metadata_offset,
         source_fingerprint = excluded.source_fingerprint, updated_at = excluded.updated_at`,
      sourceId, sourceId, nextOffset, Date.now()
    )
    return {
      applied, queued, ignored, pendingBytes: raw.byteLength - nextOffset,
      repairQueueDepth: this.#queueDepth()
    }
  }

  async compareProjection(sourceId: string, sourceRoot: string): Promise<{
    equal: boolean; legacyFingerprint: string; sqliteFingerprint: string; diff: string[]
  }> {
    const legacy = canonicalLegacy(await readKookySnapshot(sourceRoot))
    const sqlite = canonicalSqlite(this.#database)
    const legacyJson = stableJson(legacy)
    const sqliteJson = stableJson(sqlite)
    const legacyFingerprint = hash(legacyJson)
    const sqliteFingerprint = hash(sqliteJson)
    const diff = legacyJson === sqliteJson ? [] : structuralDiff(legacy, sqlite)
    const equal = diff.length === 0
    this.#database.run(
      `INSERT INTO legacy_projection_diffs (
         source_id, equal, diff_json, legacy_fingerprint, sqlite_fingerprint, created_at
       ) VALUES (?, ?, ?, ?, ?, ?)`,
      sourceId, equal ? 1 : 0, JSON.stringify(diff), legacyFingerprint, sqliteFingerprint, Date.now()
    )
    return { equal, legacyFingerprint, sqliteFingerprint, diff }
  }

  async processRepairQueue(now = Date.now(), limit = 100): Promise<{ completed: number; failed: number }> {
    this.#assertNotRetired()
    const rows = this.#database.all<{ command_id: string; payload_json: string; attempt_count: number }>(
      `SELECT command_id, payload_json, attempt_count FROM shadow_repair_queue
       WHERE completed_at IS NULL AND next_attempt_at <= ? ORDER BY created_at LIMIT ?`, now, limit
    )
    let completed = 0
    let failed = 0
    for (const row of rows) {
      try {
        this.#apply(parseKookyMutation(JSON.parse(row.payload_json)))
        this.#database.run('UPDATE shadow_repair_queue SET completed_at = ?, last_error = NULL WHERE command_id = ?', now, row.command_id)
        completed += 1
      } catch (error) {
        const attempts = row.attempt_count + 1
        this.#database.run(
          `UPDATE shadow_repair_queue SET attempt_count = ?, next_attempt_at = ?, last_error = ?
           WHERE command_id = ?`, attempts, now + Math.min(60_000, 1000 * (2 ** attempts)), errorMessage(error), row.command_id
        )
        failed += 1
      }
    }
    return { completed, failed }
  }

  metrics(sourceId: string, sourceRoot: string): Promise<{ pendingBytes: number; repairQueueDepth: number }> {
    return (async () => {
      const size = await fileSize(join(sourceRoot, 'journals', 'metadata.ndjson'))
      const offset = this.#database.get<{ metadata_offset: number }>(
        'SELECT metadata_offset FROM legacy_source_cursors WHERE source_id = ?', sourceId
      )?.metadata_offset ?? 0
      return { pendingBytes: Math.max(0, size - offset), repairQueueDepth: this.#queueDepth() }
    })()
  }

  #apply(mutation: KookyMutationEnvelope): void {
    const p = mutation.payload
    this.#transactions.execute({
      commandId: `legacy-shadow:${mutation.commandId}`,
      commandType: `legacy.${mutation.type}`,
      requestHash: hash(stableJson(mutation))
    }, ({ tx, emit }) => {
      const projectId = string(p.projectId) || string(p.id)
      const workbenchId = string(p.workbenchId) || string(p.id)
      const tabId = string(p.tabId) || string(p.id)
      const panelId = string(p.panelId) || string(p.id)
      switch (mutation.type) {
        case 'project-created': {
          requireText(projectId, 'projectId')
          const workspaceId = legacyIdFor('workspace-project', projectId)
          const root = string(p.path) || process.env.HOME || '/'
          tx.run(
            `INSERT INTO workspaces (id, name, root_directory, path_identity, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?)`, workspaceId, string(p.name) || 'Imported Project', root, `legacy:${root}`, mutation.timestamp, mutation.timestamp
          )
          break
        }
        case 'project-updated': {
          const workspaceId = legacyIdFor('workspace-project', requireText(projectId, 'projectId'))
          if (!tx.get('SELECT id FROM workspaces WHERE id = ?', workspaceId)) throw new Error('legacy project is not mapped')
          if (p.name !== undefined) tx.run('UPDATE workspaces SET name = ?, updated_at = ?, version = version + 1 WHERE id = ?', requireText(string(p.name), 'name'), mutation.timestamp, workspaceId)
          if (p.path !== undefined) tx.run('UPDATE workspaces SET root_directory = ?, path_identity = ?, updated_at = ?, version = version + 1 WHERE id = ?', requireText(string(p.path), 'path'), `legacy:${string(p.path)}`, mutation.timestamp, workspaceId)
          break
        }
        case 'project-removed':
          tx.run('UPDATE workspaces SET archived_at = ?, updated_at = ?, version = version + 1 WHERE id = ?', mutation.timestamp, mutation.timestamp, legacyIdFor('workspace-project', requireText(projectId, 'projectId')))
          break
        case 'workbench-created': {
          const legacyProject = requireText(projectId, 'projectId')
          const legacyWorkbench = requireText(workbenchId, 'workbenchId')
          const workspaceId = legacyIdFor('workspace-project', legacyProject)
          if (!tx.get('SELECT id FROM workspaces WHERE id = ?', workspaceId)) throw new Error('legacy workbench project is not mapped')
          const contextId = legacyIdFor('context-workbench', legacyWorkbench)
          const cwd = tx.get<{ root_directory: string }>('SELECT root_directory FROM workspaces WHERE id = ?', workspaceId)!.root_directory
          tx.run("INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at) VALUES (?, ?, 'plain-directory', ?, ?)", contextId, workspaceId, cwd, mutation.timestamp)
          tx.run(
            `INSERT INTO tasks (id, workspace_id, execution_context_id, title, status, sort_key, created_at, updated_at)
             VALUES (?, ?, ?, ?, 'active', ?, ?, ?)`, legacyIdFor('task-workbench', legacyWorkbench), workspaceId, contextId,
            string(p.name) || 'Imported Task', legacyWorkbench, mutation.timestamp, mutation.timestamp
          )
          break
        }
        case 'workbench-updated': {
          const taskId = legacyIdFor('task-workbench', requireText(workbenchId, 'workbenchId'))
          if (!tx.get('SELECT id FROM tasks WHERE id = ?', taskId)) throw new Error('legacy workbench is not mapped')
          if (p.name !== undefined) tx.run('UPDATE tasks SET title = ?, updated_at = ?, version = version + 1 WHERE id = ?', requireText(string(p.name), 'name'), mutation.timestamp, taskId)
          break
        }
        case 'workbench-removed':
          tx.run("UPDATE tasks SET status = 'archived', archived_at = ?, updated_at = ?, version = version + 1 WHERE id = ?", mutation.timestamp, mutation.timestamp, legacyIdFor('task-workbench', requireText(workbenchId, 'workbenchId')))
          break
        case 'tab-created': {
          const legacyTab = requireText(tabId, 'tabId')
          const taskId = legacyIdFor('task-workbench', requireText(workbenchId, 'workbenchId'))
          if (!tx.get('SELECT id FROM tasks WHERE id = ?', taskId)) throw new Error('legacy tab workbench is not mapped')
          const sceneId = legacyIdFor('scene-tab', legacyTab)
          const rootNodeId = legacyIdFor('scene-root', legacyTab)
          tx.run("INSERT INTO scenes (id, task_id, name, mode, root_node_id, created_at, updated_at) VALUES (?, ?, ?, 'tile', ?, ?, ?)", sceneId, taskId, string(p.title) || string(p.name) || 'Imported Scene', rootNodeId, mutation.timestamp, mutation.timestamp)
          tx.run("INSERT INTO scene_nodes (id, scene_id, kind, ordinal, created_at) VALUES (?, ?, 'root', 0, ?)", rootNodeId, sceneId, mutation.timestamp)
          break
        }
        case 'tab-updated':
        case 'tab-renamed':
          tx.run('UPDATE scenes SET name = ?, updated_at = ? WHERE id = ?', string(p.title) || string(p.name) || 'Imported Scene', mutation.timestamp, legacyIdFor('scene-tab', requireText(tabId, 'tabId')))
          break
        case 'tab-removed':
          tx.run('UPDATE scenes SET archived_at = ?, updated_at = ? WHERE id = ?', mutation.timestamp, mutation.timestamp, legacyIdFor('scene-tab', requireText(tabId, 'tabId')))
          break
        case 'panel-created': {
          const legacyPanel = requireText(panelId, 'panelId')
          const taskId = legacyIdFor('task-workbench', requireText(workbenchId, 'workbenchId'))
          const task = tx.get<{ workspace_id: string; execution_context_id: string }>('SELECT workspace_id, execution_context_id FROM tasks WHERE id = ?', taskId)
          if (!task) throw new Error('legacy panel workbench is not mapped')
          const contextId = legacyIdFor('context-panel', legacyPanel)
          const fallback = tx.get<{ cwd: string }>('SELECT cwd FROM execution_contexts WHERE id = ?', task.execution_context_id)!.cwd
          tx.run("INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at) VALUES (?, ?, 'plain-directory', ?, ?)", contextId, task.workspace_id, string(p.cwd) || fallback, mutation.timestamp)
          const sessionId = legacyIdFor('session-panel', legacyPanel)
          tx.run(
            `INSERT INTO sessions (id, task_id, execution_context_id, kind, status, title, created_at, updated_at, last_activity_at)
             VALUES (?, ?, ?, ?, 'created', ?, ?, ?, ?)`, sessionId, taskId, contextId,
            string(p.mode) === 'shell' && !string(p.claudeSessionId) ? 'shell' : 'claude-code', string(p.title) || 'Imported Session', mutation.timestamp, mutation.timestamp, mutation.timestamp
          )
          this.#bindProvider(tx, sessionId, p, mutation.timestamp)
          const legacyTab = string(p.tabId)
          if (legacyTab) this.#mountPanel(tx, legacyTab, legacyPanel, sessionId, mutation.timestamp)
          break
        }
        case 'panel-updated':
        case 'panel-session-bound':
        case 'panel-permission-mode':
        case 'panel-shell-state':
        case 'panel-detached':
        case 'panel-attached': {
          const legacyPanel = requireText(panelId, 'panelId')
          const sessionId = legacyIdFor('session-panel', legacyPanel)
          const session = tx.get<{ execution_context_id: string }>('SELECT execution_context_id FROM sessions WHERE id = ?', sessionId)
          if (!session) throw new Error('legacy panel is not mapped')
          if (p.cwd !== undefined) tx.run('UPDATE execution_contexts SET cwd = ? WHERE id = ?', requireText(string(p.cwd), 'cwd'), session.execution_context_id)
          if (p.title !== undefined) tx.run('UPDATE sessions SET title = ?, updated_at = ?, version = version + 1 WHERE id = ?', requireText(string(p.title), 'title'), mutation.timestamp, sessionId)
          this.#bindProvider(tx, sessionId, p, mutation.timestamp)
          break
        }
        case 'panel-removed':
          tx.run("UPDATE sessions SET status = 'archived', archived_at = ?, updated_at = ?, version = version + 1 WHERE id = ?", mutation.timestamp, mutation.timestamp, legacyIdFor('session-panel', requireText(panelId, 'panelId')))
          break
        case 'split-tree-updated':
        case 'leaf-activated':
        case 'project-activated':
        case 'workbench-activated':
        case 'tab-activated':
        case 'workbench-reordered':
        case 'workbench-tab-order-updated':
          // Activation/order are projection hints. Structural entities remain authoritative.
          break
      }
      emit({
        eventId: `legacy-shadow:${mutation.commandId}:event`, eventType: 'legacy.mutation-applied',
        aggregateType: 'legacy-mutation', aggregateId: mutation.commandId,
        payload: { type: mutation.type, legacyIds: { projectId, workbenchId, tabId, panelId } }, occurredAt: mutation.timestamp
      })
      return { commandId: mutation.commandId, type: mutation.type }
    })
  }

  #bindProvider(tx: Parameters<Parameters<DomainTransactionManager['execute']>[1]>[0]['tx'], sessionId: string, payload: Record<string, unknown>, now: number): void {
    const providerSession = string(payload.claudeSessionId)
    if (!providerSession) return
    const id = legacyIdFor('provider-binding', `claude-code:${providerSession}`)
    tx.run(
      `INSERT INTO provider_bindings (
         id, session_id, provider, provider_session_id, resume_state, metadata_json, created_at, updated_at
       ) VALUES (?, ?, 'claude-code', ?, 'available', ?, ?, ?)
       ON CONFLICT(provider, provider_session_id) DO UPDATE SET resume_state = 'available',
         metadata_json = excluded.metadata_json, updated_at = excluded.updated_at`,
      id, sessionId, providerSession, JSON.stringify({ permissionMode: string(payload.aiPermissionMode) || 'default' }), now, now
    )
  }

  #mountPanel(tx: Parameters<Parameters<DomainTransactionManager['execute']>[1]>[0]['tx'], legacyTab: string, legacyPanel: string, sessionId: string, now: number): void {
    const sceneId = legacyIdFor('scene-tab', legacyTab)
    const rootNodeId = legacyIdFor('scene-root', legacyTab)
    if (!tx.get('SELECT id FROM scenes WHERE id = ?', sceneId)) return
    const nodeId = legacyIdFor('scene-node-fallback', `${legacyTab}:${legacyPanel}`)
    tx.run("INSERT OR IGNORE INTO scene_nodes (id, scene_id, parent_node_id, kind, ordinal, created_at) VALUES (?, ?, ?, 'mount', 0, ?)", nodeId, sceneId, rootNodeId, now)
    tx.run('INSERT OR IGNORE INTO session_mounts (id, scene_id, scene_node_id, session_id, created_at) VALUES (?, ?, ?, ?, ?)', legacyIdFor('mount-panel', `${sceneId}:${legacyPanel}`), sceneId, nodeId, sessionId, now)
  }

  #queueRepair(mutation: KookyMutationEnvelope, error: string): void {
    this.#database.run(
      `INSERT INTO shadow_repair_queue (
         command_id, mutation_type, payload_json, next_attempt_at, last_error, created_at
       ) VALUES (?, ?, ?, ?, ?, ?)
       ON CONFLICT(command_id) DO UPDATE SET last_error = excluded.last_error,
         next_attempt_at = MIN(shadow_repair_queue.next_attempt_at, excluded.next_attempt_at)`,
      mutation.commandId, mutation.type, JSON.stringify(mutation), mutation.timestamp, error, mutation.timestamp
    )
  }

  #queueDepth(): number {
    return this.#database.get<{ count: number }>('SELECT COUNT(*) AS count FROM shadow_repair_queue WHERE completed_at IS NULL')?.count ?? 0
  }

  #assertNotRetired(): void {
    const row = this.#database.get<{ value_json: string }>(
      "SELECT value_json FROM migration_authority WHERE key = 'migration-phase'"
    )
    if (row) {
      try { if (JSON.parse(row.value_json) === 'retired') throw new Error('legacy shadow writes are retired') } catch (error) {
        if (error instanceof Error && error.message.includes('retired')) throw error
      }
    }
  }
}

function canonicalLegacy(snapshot: Awaited<ReturnType<typeof readKookySnapshot>>): unknown {
  const workspaces = snapshot.projects.list.map((project) => ({
    id: legacyIdFor('workspace-project', string(project.id)), name: string(project.name), root: string(project.path)
  })).sort(byId)
  const tasks = Object.values(snapshot.workbenches).map((task) => ({
    id: legacyIdFor('task-workbench', string(task.id)), workspaceId: legacyIdFor('workspace-project', string(task.projectId)), title: string(task.name)
  })).sort(byId)
  const scenes = Object.values(snapshot.tabs).map((scene) => ({
    id: legacyIdFor('scene-tab', string(scene.id)), taskId: legacyIdFor('task-workbench', string(scene.workbenchId)), name: string(scene.name)
  })).sort(byId)
  const sessions = Object.values(snapshot.panels).map((session) => ({
    id: legacyIdFor('session-panel', string(session.id)), taskId: legacyIdFor('task-workbench', string(session.workbenchId)),
    title: string(session.title) || 'Imported Session', cwd: string(session.cwd), provider: string(session.claudeSessionId)
  })).sort(byId)
  return { workspaces, tasks, scenes, sessions }
}

function canonicalSqlite(database: RuntimeDatabase): unknown {
  return {
    workspaces: database.all<{ id: string; name: string; root_directory: string }>("SELECT id, name, root_directory FROM workspaces WHERE id LIKE 'legacy-workspace-project-%'").map((row) => ({ id: row.id, name: row.name, root: row.root_directory })).sort(byId),
    tasks: database.all<{ id: string; workspace_id: string; title: string }>("SELECT id, workspace_id, title FROM tasks WHERE id LIKE 'legacy-task-workbench-%'").map((row) => ({ id: row.id, workspaceId: row.workspace_id, title: row.title })).sort(byId),
    scenes: database.all<{ id: string; task_id: string; name: string }>("SELECT id, task_id, name FROM scenes WHERE id LIKE 'legacy-scene-tab-%'").map((row) => ({ id: row.id, taskId: row.task_id, name: row.name })).sort(byId),
    sessions: database.all<{ id: string; task_id: string; title: string; cwd: string; provider_session_id: string | null }>(
      `SELECT s.id, s.task_id, s.title, ec.cwd, pb.provider_session_id
       FROM sessions s JOIN execution_contexts ec ON ec.id = s.execution_context_id
       LEFT JOIN provider_bindings pb ON pb.session_id = s.id
       WHERE s.id LIKE 'legacy-session-panel-%'`
    ).map((row) => ({ id: row.id, taskId: row.task_id, title: row.title, cwd: row.cwd, provider: row.provider_session_id ?? '' })).sort(byId)
  }
}

function structuralDiff(legacy: unknown, sqlite: unknown): string[] {
  const left = legacy as Record<string, unknown[]>
  const right = sqlite as Record<string, unknown[]>
  return [...new Set([...Object.keys(left), ...Object.keys(right)])].sort()
    .filter((key) => stableJson(left[key] ?? []) !== stableJson(right[key] ?? []))
}

function byId(a: { id: string }, b: { id: string }): number { return a.id.localeCompare(b.id) }
function string(value: unknown): string { return typeof value === 'string' ? value.trim() : '' }
function requireText(value: string, label: string): string { if (!value) throw new Error(`${label} is required`); return value }
function hash(value: string): string { return createHash('sha256').update(value).digest('hex') }
function stableJson(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(stableJson).join(',')}]`
  if (value && typeof value === 'object') return `{${Object.entries(value as Record<string, unknown>).sort(([a], [b]) => a.localeCompare(b)).map(([key, item]) => `${JSON.stringify(key)}:${stableJson(item)}`).join(',')}}`
  return JSON.stringify(value)
}
function errorMessage(error: unknown): string { return error instanceof Error ? error.message : String(error) }
async function fileSize(path: string): Promise<number> { try { return (await stat(path)).size } catch { return 0 } }
