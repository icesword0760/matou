import { mkdir, readdir, rm } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'

import { SegmentJournal } from '../../../apps/runtime/src/journal/segment-journal'
import { RuntimeDatabase } from '../../../apps/runtime/src/storage/database'

type ScaleParameter = string | number | bigint | Uint8Array | null

class ScaleDatabaseConnection {
  readonly #database: RuntimeDatabase

  private constructor(database: RuntimeDatabase) {
    this.#database = database
  }

  static open(path: string, readOnly = false): ScaleDatabaseConnection {
    return new ScaleDatabaseConnection(
      readOnly ? RuntimeDatabase.openReadOnly(path) : RuntimeDatabase.open(path)
    )
  }

  exec(sql: string): void {
    this.#database.exec(sql)
  }

  prepare(sql: string) {
    return {
      run: (...params: ScaleParameter[]) => this.#database.run(sql, ...params),
      get: (...params: ScaleParameter[]) => this.#database.get(sql, ...params),
      all: (...params: ScaleParameter[]) => this.#database.all(sql, ...params)
    }
  }

  close(): void {
    this.#database.close()
  }
}

export interface ScaleDataset {
  siblingSessions: 20 | 50 | 200 | 1000
  relationshipDepth?: 5000
  dagNodes?: 10000
  scenes?: number
  journalBytesPerSession?: number
  workspaceCount?: number
  tasksPerWorkspace?: number
  sessionsPerTask?: number
}

export interface ScaleDatabaseCounts {
  siblingSessions: number
  relationshipDepth: number
  depthRelations: number
  dagNodes: number
  dagRelations: number
  scenes: number
  workspaces: number
  tasks: number
}

const DATABASE_NAME = 'matou.sqlite'
const FIXED_TIME = 1_700_000_000_000
const SCALE_WORKSPACE_ID = 'scale-workspace'
const SCALE_CONTEXT_ID = 'scale-context'
const SCALE_TASK_ID = 'scale-task'
const SCALE_WINDOW_ID = 'main-window-1'
const SIBLING_SCENE_ID = 'scale-sibling-scene'
const DEPTH_SCENE_ID = 'scale-depth-scene'
const DAG_SCENE_ID = 'scale-dag-scene'

export async function seedScaleDatabase(
  dataDirectory: string,
  dataset: ScaleDataset
): Promise<void> {
  validateDataset(dataset)
  await mkdir(dataDirectory, { recursive: true })
  await prepareScaleWorkspaceDirectories(dataDirectory, dataset)
  const database = ScaleDatabaseConnection.open(join(dataDirectory, DATABASE_NAME))
  try {
    assertMigrated(database)
    database.exec('PRAGMA foreign_keys = OFF; BEGIN IMMEDIATE;')
    try {
      removePreviousScaleSeed(database)
      archiveOrdinaryFixtureRows(database)
      insertScaleAuthority(database, dataDirectory, dataset)
      database.exec('COMMIT;')
    } catch (error) {
      database.exec('ROLLBACK;')
      throw error
    }
    database.exec('PRAGMA foreign_keys = ON; PRAGMA wal_checkpoint(TRUNCATE);')
  } finally {
    database.close()
  }
  if (dataset.journalBytesPerSession !== undefined) {
    await seedScaleJournals(dataDirectory, dataset)
  }
}

async function prepareScaleWorkspaceDirectories(
  dataDirectory: string,
  dataset: ScaleDataset
): Promise<void> {
  const parentDirectory = dirname(dataDirectory)
  const directories = [resolve(parentDirectory, 'matou_workspace')]
  if (dataset.workspaceCount) {
    for (let workspaceIndex = 1; workspaceIndex < dataset.workspaceCount; workspaceIndex += 1) {
      directories.push(resolve(parentDirectory, `matou_workspace_${fixedIndex(workspaceIndex)}`))
    }
  }
  await Promise.all(directories.map((directory) => mkdir(directory, { recursive: true })))
}

export async function readScaleDatabaseCounts(
  dataDirectory: string
): Promise<ScaleDatabaseCounts> {
  const database = ScaleDatabaseConnection.open(join(dataDirectory, DATABASE_NAME), true)
  try {
    const count = (sql: string, ...params: Array<string | number>): number => {
      const row = database.prepare(sql).get(...params) as { count: number | bigint } | undefined
      return Number(row?.count ?? 0)
    }
    return {
      siblingSessions: count(
        "SELECT COUNT(*) AS count FROM sessions WHERE id GLOB 'scale-sibling-*'"
      ),
      relationshipDepth: count(
        "SELECT COUNT(*) AS count FROM sessions WHERE id GLOB 'scale-depth-*'"
      ),
      depthRelations: count(
        "SELECT COUNT(*) AS count FROM session_relations_current WHERE relation_id GLOB 'scale-depth-relation-*'"
      ),
      dagNodes: count(
        "SELECT COUNT(*) AS count FROM sessions WHERE id GLOB 'scale-dag-*'"
      ),
      dagRelations: count(
        "SELECT COUNT(*) AS count FROM session_relations_current WHERE relation_id GLOB 'scale-dag-relation-*'"
      ),
      scenes: count(
        "SELECT COUNT(*) AS count FROM scenes WHERE id GLOB 'scale-*-scene*' AND archived_at IS NULL"
      ),
      workspaces: count("SELECT COUNT(*) AS count FROM workspaces WHERE id GLOB 'scale-workspace*' AND archived_at IS NULL"),
      tasks: count("SELECT COUNT(*) AS count FROM tasks WHERE id GLOB 'scale-task*' AND archived_at IS NULL")
    }
  } finally {
    database.close()
  }
}

function validateDataset(dataset: ScaleDataset): void {
  if (![20, 50, 81, 200, 1000].includes(dataset.siblingSessions)) {
    throw new Error('siblingSessions must be one of 20, 50, 81, 200, or 1000')
  }
  if (dataset.relationshipDepth !== undefined && dataset.relationshipDepth !== 5000) {
    throw new Error('relationshipDepth must be 5000 when supplied')
  }
  if (dataset.dagNodes !== undefined && dataset.dagNodes !== 10000) {
    throw new Error('dagNodes must be 10000 when supplied')
  }
  const minimumScenes = 1 + Number(dataset.relationshipDepth !== undefined) +
    Number(dataset.dagNodes !== undefined)
  if (
    dataset.scenes !== undefined &&
    (!Number.isSafeInteger(dataset.scenes) || dataset.scenes < minimumScenes)
  ) {
    throw new Error(`scenes must be an integer >= ${minimumScenes}`)
  }
  const hierarchyCounts = [dataset.workspaceCount, dataset.tasksPerWorkspace, dataset.sessionsPerTask]
  if (hierarchyCounts.some((value) => value !== undefined) && hierarchyCounts.some((value) => value === undefined)) {
    throw new Error('workspaceCount, tasksPerWorkspace, and sessionsPerTask must be supplied together')
  }
  for (const [label, value] of [
    ['workspaceCount', dataset.workspaceCount],
    ['tasksPerWorkspace', dataset.tasksPerWorkspace],
    ['sessionsPerTask', dataset.sessionsPerTask]
  ] as const) {
    if (value !== undefined && (!Number.isSafeInteger(value) || value < 1)) {
      throw new Error(`${label} must be a positive integer`)
    }
  }
  if (
    dataset.journalBytesPerSession !== undefined &&
    (!Number.isSafeInteger(dataset.journalBytesPerSession) || dataset.journalBytesPerSession < 0)
  ) {
    throw new Error('journalBytesPerSession must be a non-negative integer')
  }
}

function assertMigrated(database: ScaleDatabaseConnection): void {
  const required = ['workspaces', 'sessions', 'session_canvas_memberships']
  const present = new Set((database.prepare(
    "SELECT name FROM sqlite_master WHERE type = 'table'"
  ).all() as Array<{ name: string }>).map(({ name }) => name))
  const missing = required.filter((name) => !present.has(name))
  if (missing.length > 0) {
    throw new Error(`scale seed requires a migrated Matou database; missing ${missing.join(', ')}`)
  }
}

function removePreviousScaleSeed(database: ScaleDatabaseConnection): void {
  const statements = [
    "DELETE FROM window_scene_focus WHERE scene_id GLOB 'scale-*-scene*'",
    "DELETE FROM window_task_focus WHERE task_id = 'scale-task'",
    "DELETE FROM window_task_placements WHERE task_id GLOB 'scale-task*'",
    "DELETE FROM window_workspace_focus WHERE workspace_id = 'scale-workspace'",
    "DELETE FROM window_navigation WHERE active_workspace_id = 'scale-workspace'",
    "DELETE FROM workspace_path_state WHERE workspace_id = 'scale-workspace'",
    "DELETE FROM session_graph_summaries WHERE session_id GLOB 'scale-*'",
    "DELETE FROM shell_history_blocks WHERE session_id GLOB 'scale-*'",
    "DELETE FROM terminal_commands WHERE session_id GLOB 'scale-*'",
    "DELETE FROM journal_checkpoints WHERE session_id GLOB 'scale-*'",
    "DELETE FROM session_fork_intents WHERE session_id GLOB 'scale-*' OR source_session_id GLOB 'scale-*'",
    "DELETE FROM provider_bindings WHERE session_id GLOB 'scale-*'",
    "DELETE FROM session_runs WHERE session_id GLOB 'scale-*'",
    "DELETE FROM session_environment_bindings WHERE session_id GLOB 'scale-*'",
    "DELETE FROM session_mounts WHERE session_id GLOB 'scale-*' OR scene_id GLOB 'scale-*-scene*'",
    "DELETE FROM session_canvas_memberships WHERE session_id GLOB 'scale-*' OR scene_id GLOB 'scale-*-scene*'",
    "DELETE FROM session_relations_current WHERE relation_id GLOB 'scale-*' OR from_session_id GLOB 'scale-*' OR to_session_id GLOB 'scale-*'",
    "DELETE FROM session_relation_events WHERE relation_id GLOB 'scale-*' OR from_session_id GLOB 'scale-*' OR to_session_id GLOB 'scale-*'",
    "DELETE FROM sessions WHERE id GLOB 'scale-*'",
    "DELETE FROM scene_geometry WHERE scene_id GLOB 'scale-*-scene*'",
    "DELETE FROM scene_nodes WHERE scene_id GLOB 'scale-*-scene*'",
    "DELETE FROM scene_windows WHERE scene_id GLOB 'scale-*-scene*'",
    "DELETE FROM scenes WHERE id GLOB 'scale-*-scene*'",
    "DELETE FROM tasks WHERE id GLOB 'scale-task*'",
    "DELETE FROM execution_contexts WHERE id GLOB 'scale-context*'",
    "DELETE FROM workspaces WHERE id GLOB 'scale-workspace*'"
  ]
  for (const statement of statements) database.exec(statement)
}

function archiveOrdinaryFixtureRows(database: ScaleDatabaseConnection): void {
  database.prepare(
    "UPDATE sessions SET status = 'archived', work_status = 'exited', archived_at = COALESCE(archived_at, ?) WHERE id NOT GLOB 'scale-*'"
  ).run(FIXED_TIME)
  database.prepare(
    "UPDATE scenes SET archived_at = COALESCE(archived_at, ?) WHERE id NOT GLOB 'scale-*-scene*'"
  ).run(FIXED_TIME)
  database.prepare(
    "UPDATE tasks SET status = 'archived', archived_at = COALESCE(archived_at, ?) WHERE id <> 'scale-task'"
  ).run(FIXED_TIME)
  database.prepare(
    "UPDATE workspaces SET is_default = 0, archived_at = COALESCE(archived_at, ?) WHERE id <> 'scale-workspace'"
  ).run(FIXED_TIME)
}

function insertScaleAuthority(
  database: ScaleDatabaseConnection,
  dataDirectory: string,
  dataset: ScaleDataset
): void {
  const workspaceDirectory = resolve(dirname(dataDirectory), 'matou_workspace')
  database.prepare(
    `INSERT INTO workspaces (
       id, name, root_directory, task_order_json, created_at, updated_at,
       archived_at, path_identity, version, is_default, is_pinned,
       pin_sort_key, last_opened_at
     ) VALUES (?, ?, ?, ?, ?, ?, NULL, ?, 1, 1, 0, '', ?)`
  ).run(
    SCALE_WORKSPACE_ID, 'Scale Workspace', workspaceDirectory,
    JSON.stringify([SCALE_TASK_ID]), FIXED_TIME, FIXED_TIME,
    workspaceDirectory, FIXED_TIME
  )
  database.prepare(
    `INSERT INTO execution_contexts (
       id, workspace_id, kind, cwd, created_at, archived_at
     ) VALUES (?, ?, 'plain-directory', ?, ?, NULL)`
  ).run(SCALE_CONTEXT_ID, SCALE_WORKSPACE_ID, workspaceDirectory, FIXED_TIME)
  database.prepare(
    `INSERT INTO tasks (
       id, workspace_id, parent_task_id, execution_context_id, title, status,
       created_at, updated_at, archived_at, sort_key, version, is_pinned,
       pin_sort_key, last_opened_at
     ) VALUES (?, ?, NULL, ?, 'Scale Task', 'active', ?, ?, NULL, '000001', 1, 0, '', ?)`
  ).run(SCALE_TASK_ID, SCALE_WORKSPACE_ID, SCALE_CONTEXT_ID, FIXED_TIME, FIXED_TIME, FIXED_TIME)

  insertScene(database, SIBLING_SCENE_ID, 'Scale Siblings', 'card', 0)
  insertSessions(database, {
    prefix: 'scale-sibling',
    count: dataset.siblingSessions,
    sceneId: SIBLING_SCENE_ID,
    workspaceDirectory
  })

  if (dataset.workspaceCount && dataset.tasksPerWorkspace && dataset.sessionsPerTask) {
    insertHierarchyCatalog(database, dataDirectory, {
      workspaceCount: dataset.workspaceCount,
      tasksPerWorkspace: dataset.tasksPerWorkspace,
      sessionsPerTask: dataset.sessionsPerTask
    })
  }

  let createdScenes = 1
  if (dataset.relationshipDepth !== undefined) {
    insertScene(database, DEPTH_SCENE_ID, 'Scale Depth 5000', 'card', createdScenes)
    insertSessions(database, {
      prefix: 'scale-depth', count: dataset.relationshipDepth,
      sceneId: DEPTH_SCENE_ID, workspaceDirectory,
      parentIndex: (index) => index === 0 ? undefined : index - 1
    })
    createdScenes += 1
  }
  if (dataset.dagNodes !== undefined) {
    insertScene(database, DAG_SCENE_ID, 'Scale DAG 10000', 'dag', createdScenes)
    insertSessions(database, {
      prefix: 'scale-dag', count: dataset.dagNodes,
      sceneId: DAG_SCENE_ID, workspaceDirectory,
      parentIndex: (index) => index === 0 ? undefined : Math.floor((index - 1) / 4)
    })
    createdScenes += 1
  }
  const requestedScenes = dataset.scenes ?? createdScenes
  while (createdScenes < requestedScenes) {
    const id = `scale-empty-scene-${fixedIndex(createdScenes)}`
    insertScene(database, id, `Scale Empty ${createdScenes + 1}`, 'card', createdScenes)
    createdScenes += 1
  }

  const activeSessionId = entityId('scale-sibling', 0)
  database.prepare(
    `INSERT INTO app_windows (id, kind, state, created_at, updated_at)
     VALUES (?, 'main', 'visible', ?, ?)
     ON CONFLICT(id) DO UPDATE SET state = 'visible', updated_at = excluded.updated_at`
  ).run(SCALE_WINDOW_ID, FIXED_TIME, FIXED_TIME)
  database.prepare(
    `INSERT INTO window_navigation (window_id, active_workspace_id, updated_at)
     VALUES (?, ?, ?)
     ON CONFLICT(window_id) DO UPDATE SET
       active_workspace_id = excluded.active_workspace_id,
       updated_at = excluded.updated_at`
  ).run(SCALE_WINDOW_ID, SCALE_WORKSPACE_ID, FIXED_TIME)
  database.prepare(
    `INSERT INTO window_workspace_focus (window_id, workspace_id, active_task_id, updated_at)
     VALUES (?, ?, ?, ?)
     ON CONFLICT(window_id, workspace_id) DO UPDATE SET
       active_task_id = excluded.active_task_id,
       updated_at = excluded.updated_at`
  ).run(SCALE_WINDOW_ID, SCALE_WORKSPACE_ID, SCALE_TASK_ID, FIXED_TIME)
  database.prepare(
    `INSERT INTO window_task_focus (window_id, task_id, active_scene_id, updated_at)
     VALUES (?, ?, ?, ?)
     ON CONFLICT(window_id, task_id) DO UPDATE SET
       active_scene_id = excluded.active_scene_id,
       updated_at = excluded.updated_at`
  ).run(SCALE_WINDOW_ID, SCALE_TASK_ID, SIBLING_SCENE_ID, FIXED_TIME)
  database.prepare(
    `INSERT INTO window_scene_focus (window_id, scene_id, active_session_id, updated_at)
     VALUES (?, ?, ?, ?)
     ON CONFLICT(window_id, scene_id) DO UPDATE SET
       active_session_id = excluded.active_session_id,
       updated_at = excluded.updated_at`
  ).run(SCALE_WINDOW_ID, SIBLING_SCENE_ID, activeSessionId, FIXED_TIME)
  database.prepare(
    `INSERT INTO window_task_placements (window_id, task_id, ordinal, updated_at)
     VALUES (?, ?, 0, ?)
     ON CONFLICT(task_id) DO UPDATE SET
       window_id = excluded.window_id, ordinal = 0, updated_at = excluded.updated_at`
  ).run(SCALE_WINDOW_ID, SCALE_TASK_ID, FIXED_TIME)
  database.prepare(
    `INSERT INTO workspace_path_state (
       workspace_id, status, reason, checked_at, validation_generation
     ) VALUES (?, 'valid', '', ?, 1)
     ON CONFLICT(workspace_id) DO UPDATE SET
       status = 'valid', reason = '', checked_at = excluded.checked_at,
       validation_generation = excluded.validation_generation`
  ).run(SCALE_WORKSPACE_ID, FIXED_TIME)

  database.prepare(
    `INSERT INTO runtime_sequences(name, value) VALUES ('session-sibling-created', ?)
     ON CONFLICT(name) DO UPDATE SET value = excluded.value`
  ).run(dataset.siblingSessions + (dataset.relationshipDepth ?? 0) + (dataset.dagNodes ?? 0))
}

function insertScene(
  database: ScaleDatabaseConnection,
  id: string,
  name: string,
  mode: 'card' | 'dag',
  index: number
): void {
  database.prepare(
    `INSERT INTO scenes (
       id, task_id, name, mode, root_node_id, created_at, updated_at,
       archived_at, title_pinned, sort_key, layout_revision
     ) VALUES (?, ?, ?, ?, NULL, ?, ?, NULL, 0, ?, 1)`
  ).run(id, SCALE_TASK_ID, name, mode, FIXED_TIME + index, FIXED_TIME + index, fixedIndex(index))
}

function insertSessions(database: ScaleDatabaseConnection, input: {
  prefix: 'scale-sibling' | 'scale-depth' | 'scale-dag'
  count: number
  sceneId: string
  workspaceDirectory: string
  parentIndex?: (index: number) => number | undefined
  taskId?: string
  contextId?: string
  idPrefix?: string
}): void {
  const insertSession = database.prepare(
    `INSERT INTO sessions (
       id, task_id, execution_context_id, kind, status, created_at, updated_at,
       last_activity_at, archived_at, title, version, cwd, work_status
     ) VALUES (?, ?, ?, 'shell', 'created', ?, ?, ?, NULL, ?, 1, ?, 'idle')`
  )
  const insertMembership = database.prepare(
    `INSERT INTO session_canvas_memberships (
       session_id, scene_id, sibling_created_seq, last_user_interaction_seq,
       created_at, updated_at, pending_user_interaction_seq
     ) VALUES (?, ?, ?, 0, ?, ?, 0)`
  )
  const insertMount = database.prepare(
    `INSERT INTO session_mounts (
       id, scene_id, scene_node_id, session_id, created_at
     ) VALUES (?, ?, NULL, ?, ?)`
  )
  const insertRelationEvent = database.prepare(
    `INSERT INTO session_relation_events (
       event_id, relation_id, operation, task_id, from_session_id,
       to_session_id, relation_kind, metadata_json, command_id, occurred_at
     ) VALUES (?, ?, 'created', ?, ?, ?, 'derived-from', '{}', ?, ?)`
  )
  const insertRelation = database.prepare(
    `INSERT INTO session_relations_current (
       relation_id, task_id, from_session_id, to_session_id, relation_kind,
       metadata_json, created_at, updated_at, source_event_sequence
     ) VALUES (?, ?, ?, ?, 'derived-from', '{}', ?, ?, ?)`
  )
  for (let index = 0; index < input.count; index += 1) {
    const prefix = input.idPrefix ?? input.prefix
    const id = entityId(prefix, index)
    const timestamp = FIXED_TIME + index
    insertSession.run(
      id, input.taskId ?? SCALE_TASK_ID, input.contextId ?? SCALE_CONTEXT_ID, timestamp, timestamp, timestamp,
      `${prefix.slice('scale-'.length)} ${index + 1}`, input.workspaceDirectory
    )
    insertMembership.run(id, input.sceneId, index + 1, timestamp, timestamp)
    // Scale fixtures represent durable Sessions that can survive an App
    // restart. A canvas membership alone is enough to project a card, but
    // Runtime recovery intentionally follows the authoritative Scene mount.
    // Keep both projections populated so restart benchmarks exercise the same
    // recovery path as Sessions created through the product UI.
    insertMount.run(`${id}-mount`, input.sceneId, id, timestamp)
    const parentIndex = input.parentIndex?.(index)
    if (parentIndex === undefined) continue
    const parentId = entityId(prefix, parentIndex)
    const relationId = `${prefix}-relation-${fixedIndex(index)}`
    const result = insertRelationEvent.run(
      `${relationId}-event`, relationId, input.taskId ?? SCALE_TASK_ID, id, parentId,
      `${relationId}-command`, timestamp
    )
    insertRelation.run(
      relationId, input.taskId ?? SCALE_TASK_ID, id, parentId, timestamp, timestamp,
      Number(result.lastInsertRowid)
    )
  }
}

function insertHierarchyCatalog(
  database: ScaleDatabaseConnection,
  dataDirectory: string,
  dataset: Required<Pick<ScaleDataset, 'workspaceCount' | 'tasksPerWorkspace' | 'sessionsPerTask'>>
): void {
  for (let workspaceIndex = 1; workspaceIndex < dataset.workspaceCount; workspaceIndex += 1) {
    const suffix = fixedIndex(workspaceIndex)
    const workspaceId = `scale-workspace-${suffix}`
    const contextId = `scale-context-${suffix}`
    const workspaceDirectory = resolve(dirname(dataDirectory), `matou_workspace_${suffix}`)
    const taskIds = Array.from({ length: dataset.tasksPerWorkspace }, (_, taskIndex) =>
      `scale-task-${suffix}-${fixedIndex(taskIndex)}`)
    database.prepare(
      `INSERT INTO workspaces (
         id, name, root_directory, task_order_json, created_at, updated_at,
         archived_at, path_identity, version, is_default, is_pinned,
         pin_sort_key, last_opened_at
       ) VALUES (?, ?, ?, ?, ?, ?, NULL, ?, 1, 0, 0, '', ?)`
    ).run(workspaceId, `Scale Workspace ${workspaceIndex + 1}`, workspaceDirectory,
      JSON.stringify(taskIds), FIXED_TIME + workspaceIndex, FIXED_TIME + workspaceIndex,
      workspaceDirectory, FIXED_TIME + workspaceIndex)
    database.prepare(
      `INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at, archived_at)
       VALUES (?, ?, 'plain-directory', ?, ?, NULL)`
    ).run(contextId, workspaceId, workspaceDirectory, FIXED_TIME + workspaceIndex)
    for (let taskIndex = 0; taskIndex < dataset.tasksPerWorkspace; taskIndex += 1) {
      const taskId = taskIds[taskIndex]!
      const sceneId = `scale-catalog-scene-${suffix}-${fixedIndex(taskIndex)}`
      database.prepare(
        `INSERT INTO tasks (
           id, workspace_id, parent_task_id, execution_context_id, title, status,
           created_at, updated_at, archived_at, sort_key, version, is_pinned,
           pin_sort_key, last_opened_at
         ) VALUES (?, ?, NULL, ?, ?, 'active', ?, ?, NULL, ?, 1, 0, '', ?)`
      ).run(taskId, workspaceId, contextId, `Scale Task ${workspaceIndex + 1}.${taskIndex + 1}`,
        FIXED_TIME + taskIndex, FIXED_TIME + taskIndex, fixedIndex(taskIndex), FIXED_TIME + taskIndex)
      database.prepare(
        `INSERT INTO window_task_placements (window_id, task_id, ordinal, updated_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(task_id) DO UPDATE SET
           window_id = excluded.window_id, ordinal = excluded.ordinal,
           updated_at = excluded.updated_at`
      ).run(SCALE_WINDOW_ID, taskId,
        workspaceIndex * dataset.tasksPerWorkspace + taskIndex,
        FIXED_TIME + taskIndex)
      database.prepare(
        `INSERT INTO scenes (
           id, task_id, name, mode, root_node_id, created_at, updated_at,
           archived_at, title_pinned, sort_key, layout_revision
         ) VALUES (?, ?, ?, 'card', NULL, ?, ?, NULL, 0, '000001', 1)`
      ).run(sceneId, taskId, `Scale Catalog ${workspaceIndex + 1}.${taskIndex + 1}`,
        FIXED_TIME + taskIndex, FIXED_TIME + taskIndex)
      insertSessions(database, {
        prefix: 'scale-sibling', count: dataset.sessionsPerTask, sceneId, workspaceDirectory,
        taskId, contextId,
        idPrefix: `scale-catalog-${suffix}-${fixedIndex(taskIndex)}`
      })
    }
  }
}

function entityId(prefix: string, index: number): string {
  return `${prefix}-${fixedIndex(index)}`
}

function fixedIndex(index: number): string {
  return String(index + 1).padStart(5, '0')
}

async function seedScaleJournals(
  dataDirectory: string,
  dataset: ScaleDataset
): Promise<void> {
  const journalRoot = join(dataDirectory, 'journal')
  await mkdir(journalRoot, { recursive: true })
  const entries = await readdir(journalRoot)
  await Promise.all(entries.filter((entry) => entry.startsWith('scale-')).map((entry) =>
    rm(join(journalRoot, entry), { recursive: true, force: true })
  ))
  const targetBytes = dataset.journalBytesPerSession ?? 0
  if (targetBytes === 0) return
  const sessionIds = [
    ...entityIds('scale-sibling', dataset.siblingSessions),
    ...entityIds('scale-depth', dataset.relationshipDepth ?? 0),
    ...entityIds('scale-dag', dataset.dagNodes ?? 0)
  ]
  const payload = new Uint8Array(Math.min(targetBytes, 1024 * 1024)).fill(0x53)
  await runBounded(sessionIds, 8, async (sessionId) => {
    const journal = await SegmentJournal.open(dataDirectory, sessionId, {
      compressSealed: false,
      maxSegmentBytes: Math.max(128, targetBytes + 1024)
    })
    try {
      let written = 0
      let sequence = 0
      while (written < targetBytes) {
        const bytes = Math.min(payload.byteLength, targetBytes - written)
        await journal.appendOutput(++sequence, payload.subarray(0, bytes))
        written += bytes
      }
    } finally {
      await journal.close()
    }
  })
}

function entityIds(prefix: string, count: number): string[] {
  return Array.from({ length: count }, (_, index) => entityId(prefix, index))
}

async function runBounded<T>(
  values: readonly T[],
  concurrency: number,
  operation: (value: T) => Promise<void>
): Promise<void> {
  let cursor = 0
  await Promise.all(Array.from({ length: Math.min(concurrency, values.length) }, async () => {
    while (cursor < values.length) {
      const value = values[cursor++]!
      await operation(value)
    }
  }))
}
