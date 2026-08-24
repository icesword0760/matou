import { randomUUID } from 'node:crypto'
import { mkdir, open, rename, writeFile } from 'node:fs/promises'
import { join } from 'node:path'

import type { RuntimeDatabase } from '../../storage/database'
import { legacyIdFor, readKookySnapshot } from './kooky-importer'

export type ReadAuthority = 'legacy' | 'sqlite'

export interface MigrationProjection {
  workspaces: Array<{ id: string; name: string; rootDirectory: string }>
  tasks: Array<{ id: string; workspaceId: string; title: string }>
  scenes: Array<{ id: string; taskId: string; name: string }>
  sessions: Array<{
    id: string; taskId: string; title: string; cwd: string
    providerSessionId?: string; permissionMode?: string
  }>
  relations: Array<{ id: string; taskId: string; fromSessionId: string; toSessionId: string; kind: string }>
}

export class ReadAuthorityController {
  readonly #database: RuntimeDatabase

  constructor(database: RuntimeDatabase) {
    this.#database = database
  }

  getReadAuthority(): ReadAuthority {
    const row = this.#database.get<{ value_json: string }>(
      "SELECT value_json FROM migration_authority WHERE key = 'read-authority'"
    )
    if (!row) return 'legacy'
    try { return JSON.parse(row.value_json) === 'sqlite' ? 'sqlite' : 'legacy' } catch { return 'legacy' }
  }

  setReadAuthority(authority: ReadAuthority, now = Date.now()): void {
    this.#database.run(
      `INSERT INTO migration_authority (key, value_json, updated_at) VALUES ('read-authority', ?, ?)
       ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json, updated_at = excluded.updated_at`,
      JSON.stringify(authority), now
    )
    this.recordHealth(`read-authority.${authority}`, 1, now)
  }

  async readProjection(legacySourceRoot: string): Promise<MigrationProjection> {
    return this.getReadAuthority() === 'sqlite'
      ? sqliteProjection(this.#database)
      : legacyProjection(await readKookySnapshot(legacySourceRoot))
  }

  async executeSqliteFirst<T>(
    commandId: string,
    sqliteMutation: () => T,
    compatibilityBackup: () => Promise<void>
  ): Promise<{ result: T; backupWritten: boolean; backupError?: string }> {
    if (!commandId.trim()) throw new Error('Migration commandId is required')
    const result = sqliteMutation()
    this.recordHealth('sqlite-mutation.committed', 1)
    try {
      await compatibilityBackup()
      this.recordHealth('compatibility-backup.success', 1)
      return { result, backupWritten: true }
    } catch (error) {
      this.recordHealth('compatibility-backup.failure', 1)
      return { result, backupWritten: false, backupError: errorMessage(error) }
    }
  }

  recordHealth(metric: string, amount: number, now = Date.now()): void {
    if (!/^[a-z][a-z0-9_.-]*$/i.test(metric) || !Number.isFinite(amount)) throw new Error('Invalid migration metric')
    this.#database.run(
      `INSERT INTO migration_telemetry (metric, value, updated_at) VALUES (?, ?, ?)
       ON CONFLICT(metric) DO UPDATE SET value = migration_telemetry.value + excluded.value,
       updated_at = excluded.updated_at`, metric, amount, now
    )
  }

  telemetry(): Record<string, number> {
    return Object.fromEntries(this.#database.all<{ metric: string; value: number }>(
      'SELECT metric, value FROM migration_telemetry ORDER BY metric'
    ).map(({ metric, value }) => [metric, value]))
  }
}

export class LegacyCompatibilityBackupWriter {
  readonly #database: RuntimeDatabase
  readonly #backupRoot: string

  constructor(database: RuntimeDatabase, backupRoot: string) {
    this.#database = database
    this.#backupRoot = backupRoot
  }

  async write(now = Date.now()): Promise<string> {
    const snapshot = buildKookyBackup(this.#database, now)
    await mkdir(this.#backupRoot, { recursive: true, mode: 0o700 })
    const path = join(this.#backupRoot, 'snapshot.json')
    const temporary = `${path}.tmp-${randomUUID()}`
    await writeFile(temporary, `${JSON.stringify(snapshot, null, 2)}\n`, { mode: 0o600 })
    const handle = await open(temporary, 'r'); await handle.sync(); await handle.close()
    await rename(temporary, path)
    if (process.platform !== 'win32') {
      const directory = await open(this.#backupRoot, 'r'); await directory.sync(); await directory.close()
    }
    return path
  }
}

function legacyProjection(snapshot: Awaited<ReturnType<typeof readKookySnapshot>>): MigrationProjection {
  const panels = Object.values(snapshot.panels)
  const providerToPanel = new Map<string, string>()
  for (const panel of panels) {
    const provider = string(panel.claudeSessionId)
    if (provider) providerToPanel.set(provider, string(panel.id))
  }
  const relations: MigrationProjection['relations'] = []
  for (const panel of panels) {
    const panelId = string(panel.id)
    const teamId = string(panel.teamId)
    if (!teamId || string(panel.teamRole) === 'leader') continue
    const leadPanel = providerToPanel.get(string(panel.teamLeadSessionId))
      ?? panels.find((candidate) => string(candidate.teamId) === teamId && string(candidate.teamRole) === 'leader')?.id
    if (!leadPanel) continue
    relations.push({
      id: legacyIdFor('relation-team', `${teamId}:${panelId}`),
      taskId: legacyIdFor('task-workbench', string(panel.workbenchId)),
      fromSessionId: legacyIdFor('session-panel', panelId),
      toSessionId: legacyIdFor('session-panel', string(leadPanel)),
      kind: 'team-member-of'
    })
  }
  return sortProjection({
    workspaces: snapshot.projects.list.map((item) => ({
      id: legacyIdFor('workspace-project', string(item.id)), name: string(item.name), rootDirectory: string(item.path)
    })),
    tasks: Object.values(snapshot.workbenches).map((item) => ({
      id: legacyIdFor('task-workbench', string(item.id)),
      workspaceId: legacyIdFor('workspace-project', string(item.projectId)), title: string(item.name)
    })),
    scenes: Object.values(snapshot.tabs).map((item) => ({
      id: legacyIdFor('scene-tab', string(item.id)),
      taskId: legacyIdFor('task-workbench', string(item.workbenchId)), name: string(item.name) || 'Imported Scene'
    })),
    sessions: panels.map((item) => ({
      id: legacyIdFor('session-panel', string(item.id)),
      taskId: legacyIdFor('task-workbench', string(item.workbenchId)),
      title: string(item.title) || string(item.aiSessionName) || 'Imported Session', cwd: string(item.cwd),
      ...(string(item.claudeSessionId) ? { providerSessionId: string(item.claudeSessionId) } : {}),
      ...(string(item.claudeSessionId) ? { permissionMode: string(item.aiPermissionMode) || 'default' } : {})
    })),
    relations
  })
}

function sqliteProjection(database: RuntimeDatabase): MigrationProjection {
  return sortProjection({
    workspaces: database.all<{ id: string; name: string; root_directory: string }>(
      "SELECT id, name, root_directory FROM workspaces WHERE id LIKE 'legacy-workspace-project-%'"
    ).map((row) => ({ id: row.id, name: row.name, rootDirectory: row.root_directory })),
    tasks: database.all<{ id: string; workspace_id: string; title: string }>(
      "SELECT id, workspace_id, title FROM tasks WHERE id LIKE 'legacy-task-workbench-%'"
    ).map((row) => ({ id: row.id, workspaceId: row.workspace_id, title: row.title })),
    scenes: database.all<{ id: string; task_id: string; name: string }>(
      "SELECT id, task_id, name FROM scenes WHERE id LIKE 'legacy-scene-tab-%'"
    ).map((row) => ({ id: row.id, taskId: row.task_id, name: row.name })),
    sessions: database.all<{
      id: string; task_id: string; title: string; cwd: string
      provider_session_id: string | null; metadata_json: string | null
    }>(
      `SELECT s.id, s.task_id, s.title, ec.cwd, pb.provider_session_id, pb.metadata_json
       FROM sessions s JOIN execution_contexts ec ON ec.id = s.execution_context_id
       LEFT JOIN provider_bindings pb ON pb.session_id = s.id
       WHERE s.id LIKE 'legacy-session-panel-%'`
    ).map((row) => {
      const metadata = parseRecord(row.metadata_json)
      return {
        id: row.id, taskId: row.task_id, title: row.title, cwd: row.cwd,
        ...(row.provider_session_id ? { providerSessionId: row.provider_session_id } : {}),
        ...(string(metadata.permissionMode) ? { permissionMode: string(metadata.permissionMode) } : {})
      }
    }),
    relations: database.all<{
      relation_id: string; task_id: string; from_session_id: string; to_session_id: string; relation_kind: string
    }>(
      "SELECT relation_id, task_id, from_session_id, to_session_id, relation_kind FROM session_relations_current WHERE relation_id LIKE 'legacy-relation-team-%'"
    ).map((row) => ({
      id: row.relation_id, taskId: row.task_id, fromSessionId: row.from_session_id,
      toSessionId: row.to_session_id, kind: row.relation_kind
    }))
  })
}

function buildKookyBackup(database: RuntimeDatabase, now: number): unknown {
  const run = database.get<{ id: string }>(
    "SELECT id FROM legacy_import_runs WHERE status = 'completed' ORDER BY completed_at DESC LIMIT 1"
  )
  if (!run) throw new Error('No completed legacy import exists for compatibility backup')
  const mappings = database.all<{
    legacy_type: string; legacy_id: string; entity_type: string; entity_id: string
  }>('SELECT legacy_type, legacy_id, entity_type, entity_id FROM legacy_entity_mappings WHERE import_run_id = ?', run.id)
  const legacyByEntity = new Map<string, string>()
  for (const preferredType of ['project', 'workbench', 'tab', 'panel']) {
    for (const row of mappings) {
      if (row.legacy_type === preferredType) legacyByEntity.set(`${row.entity_type}:${row.entity_id}`, row.legacy_id)
    }
  }
  const projection = sqliteProjection(database)
  const projects = projection.workspaces.map((workspace) => ({
    id: legacyByEntity.get(`workspace:${workspace.id}`) ?? workspace.id,
    name: workspace.name, path: workspace.rootDirectory,
    workbenchIds: projection.tasks.filter((task) => task.workspaceId === workspace.id).map((task) => legacyByEntity.get(`task:${task.id}`) ?? task.id)
  }))
  const workbenches = Object.fromEntries(projection.tasks.map((task) => {
    const id = legacyByEntity.get(`task:${task.id}`) ?? task.id
    const projectId = legacyByEntity.get(`workspace:${task.workspaceId}`) ?? task.workspaceId
    const tabIds = projection.scenes.filter((scene) => scene.taskId === task.id).map((scene) => legacyByEntity.get(`scene:${scene.id}`) ?? scene.id)
    return [id, { id, projectId, name: task.title, tabIds, activeTabId: tabIds[0] ?? null }]
  }))
  const tabs = Object.fromEntries(projection.scenes.map((scene) => {
    const id = legacyByEntity.get(`scene:${scene.id}`) ?? scene.id
    const workbenchId = legacyByEntity.get(`task:${scene.taskId}`) ?? scene.taskId
    const panelIds = projection.sessions.filter((session) => session.taskId === scene.taskId).map((session) => legacyByEntity.get(`session:${session.id}`) ?? session.id)
    const leaves = panelIds.map((panelId, index) => ({ type: 'leaf', id: `backup-leaf-${index}`, panelId }))
    const layoutRoot = leaves.length === 1 ? leaves[0] : { type: 'split', id: `backup-split-${id}`, direction: 'horizontal', children: leaves }
    return [id, { id, workbenchId, name: scene.name, layoutRoot, activeLeafId: leaves[0]?.id ?? null }]
  }))
  const firstTabByTask = new Map(projection.scenes.map((scene) => [scene.taskId, legacyByEntity.get(`scene:${scene.id}`) ?? scene.id]))
  const panels = Object.fromEntries(projection.sessions.map((session) => {
    const id = legacyByEntity.get(`session:${session.id}`) ?? session.id
    const workbenchId = legacyByEntity.get(`task:${session.taskId}`) ?? session.taskId
    const task = projection.tasks.find((item) => item.id === session.taskId)
    const projectId = task ? legacyByEntity.get(`workspace:${task.workspaceId}`) ?? task.workspaceId : ''
    return [id, {
      id, projectId, workbenchId, tabId: firstTabByTask.get(session.taskId) ?? null,
      mode: session.providerSessionId ? 'claude-code' : 'shell', title: session.title, cwd: session.cwd,
      claudeSessionId: session.providerSessionId ?? null,
      aiPermissionMode: session.permissionMode ?? 'default'
    }]
  }))
  return {
    version: 1, savedAt: new Date(now).toISOString(), recoveryOffsets: { metadataJournalBytes: 0 },
    projects: { list: projects, activeProjectId: projects[0]?.id ?? null }, workbenches, tabs, panels
  }
}

function sortProjection(projection: MigrationProjection): MigrationProjection {
  for (const collection of [projection.workspaces, projection.tasks, projection.scenes, projection.sessions, projection.relations]) {
    collection.sort((a, b) => a.id.localeCompare(b.id))
  }
  return projection
}

function parseRecord(json: string | null): Record<string, unknown> {
  try { return json ? JSON.parse(json) as Record<string, unknown> : {} } catch { return {} }
}
function string(value: unknown): string { return typeof value === 'string' ? value.trim() : '' }
function errorMessage(error: unknown): string { return error instanceof Error ? error.message : String(error) }
