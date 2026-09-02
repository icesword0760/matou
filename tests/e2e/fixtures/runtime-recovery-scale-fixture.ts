import { execFile } from 'node:child_process'
import { mkdir, realpath, writeFile } from 'node:fs/promises'
import { dirname, join, resolve } from 'node:path'
import { promisify } from 'node:util'

import { SessionGitStateRepository } from '../../../apps/runtime/src/session/session-git-state-repository'
import { SessionEnvironmentRepository } from '../../../apps/runtime/src/session/session-environment-repository'
import { ForkWorkflowService } from '../../../apps/runtime/src/session-canvas/fork-workflow-service'
import { RuntimeDatabase } from '../../../apps/runtime/src/storage/database'
import { DomainTransactionManager } from '../../../apps/runtime/src/storage/domain-transaction'
import { WorktreeService } from '../../../apps/runtime/src/worktrees/worktree-service'
import { seedScaleDatabase } from '../scale/scale-database'

const execFileAsync = promisify(execFile)

export const RECOVERY_SCALE_FOREGROUND_IDS = sessionIds('scale-sibling', 16)
export const RECOVERY_SCALE_BACKGROUND_IDS = sessionIds(
  'scale-catalog-00002-00001', 4
)
export const RECOVERY_SCALE_SESSION_IDS = [
  ...RECOVERY_SCALE_FOREGROUND_IDS,
  ...RECOVERY_SCALE_BACKGROUND_IDS
]
export const RECOVERY_SCALE_ACTIVE_SESSION_ID = RECOVERY_SCALE_FOREGROUND_IDS[0]!
export const RECOVERY_SCALE_BACKGROUND_WORKSPACE_ID = 'scale-workspace-00002'
export const RECOVERY_SCALE_BACKGROUND_TASK_ID = 'scale-task-00002-00001'
export const RECOVERY_SCALE_HEALTHY_WORKTREE_SESSION_IDS = RECOVERY_SCALE_FOREGROUND_IDS.slice(0, 5)
export const RECOVERY_SCALE_STOPPED_SESSION_IDS = [
  'scale-sibling-00018',
  'scale-catalog-00002-00002-00005',
  'scale-catalog-00003-00002-00005',
  'scale-catalog-00004-00002-00005',
  'scale-catalog-00005-00002-00005'
]
export const RECOVERY_SCALE_ARCHIVED_SESSION_IDS = [
  'scale-sibling-00020',
  'scale-catalog-00002-00004-00005',
  'scale-catalog-00003-00004-00005',
  'scale-catalog-00004-00004-00005',
  'scale-catalog-00005-00004-00005'
]

const DATABASE_NAME = 'matou.sqlite'
const IDLE_SCENE_ID = 'scale-idle-scene'
const BACKGROUND_IDLE_SCENE_ID = 'scale-background-idle-scene'
const FIXED_TIME = 1_700_000_100_000

export interface RecoveryScaleDatabaseCounts {
  sessions: number
  recoverySessions: number
  recoveryWorkspaces: number
  recoveryTasks: number
  recoveryScenes: number
  workspaces: number
  tasks: number
  scenes: number
  worktrees: number
  managedWorktreeBindings: number
  forkOperations: number
  interruptedForkOperations: number
}

export interface RecoveryScaleReconciliation {
  worktreeReady: number
  worktreeFailed: number
  managedBindingReady: number
  managedBindingFailed: number
  forkFailed: number
  forkFailureNotifications: number
  forkSessionIds: string[]
}

export interface RecoveryScaleManagedSession {
  sessionId: string
  worktreePath: string
}

export async function seedRuntimeRecoveryScale(dataDirectory: string): Promise<void> {
  await seedScaleDatabase(dataDirectory, {
    siblingSessions: 20,
    workspaceCount: 5,
    tasksPerWorkspace: 4,
    sessionsPerTask: 5
  })
  const repositories = scaleRepositories(dataDirectory)
  await Promise.all(repositories.map(initializeRepository))
  const database = RuntimeDatabase.open(join(dataDirectory, DATABASE_NAME))
  try {
    database.exec('BEGIN IMMEDIATE;')
    try {
      seedAdditionalBaseTasks(database)
      database.run(
        `INSERT INTO scenes (
           id, task_id, name, mode, root_node_id, created_at, updated_at,
           archived_at, title_pinned, sort_key, layout_revision
         ) VALUES (?, 'scale-task', 'Scale Idle Sessions', 'card', NULL, ?, ?, NULL, 0, '000002', 1)`,
        IDLE_SCENE_ID, FIXED_TIME, FIXED_TIME
      )
      database.run(
        `INSERT INTO scenes (
           id, task_id, name, mode, root_node_id, created_at, updated_at,
           archived_at, title_pinned, sort_key, layout_revision
         ) VALUES (?, ?, 'Scale Background Idle Sessions', 'card', NULL, ?, ?, NULL, 0, '000002', 1)`,
        BACKGROUND_IDLE_SCENE_ID, RECOVERY_SCALE_BACKGROUND_TASK_ID, FIXED_TIME + 1, FIXED_TIME + 1
      )
      for (const sessionId of sessionIds('scale-sibling', 20).slice(16)) {
        database.run(
          'UPDATE session_canvas_memberships SET scene_id = ?, updated_at = ? WHERE session_id = ?',
          IDLE_SCENE_ID, FIXED_TIME, sessionId
        )
        database.run(
          'UPDATE session_mounts SET scene_id = ? WHERE session_id = ?',
          IDLE_SCENE_ID, sessionId
        )
      }
      for (const sessionId of sessionIds('scale-catalog-00002-00001', 8).slice(4)) {
        database.run(
          'UPDATE session_canvas_memberships SET scene_id = ?, updated_at = ? WHERE session_id = ?',
          BACKGROUND_IDLE_SCENE_ID, FIXED_TIME, sessionId
        )
        database.run(
          'UPDATE session_mounts SET scene_id = ? WHERE session_id = ?',
          BACKGROUND_IDLE_SCENE_ID, sessionId
        )
      }
      const placeholders = RECOVERY_SCALE_SESSION_IDS.map(() => '?').join(', ')
      database.run(
        `UPDATE sessions
         SET work_status = 'interrupted', last_activity_at = ?, updated_at = ?
         WHERE id IN (${placeholders})`,
        FIXED_TIME, FIXED_TIME, ...RECOVERY_SCALE_SESSION_IDS
      )
      const stoppedPlaceholders = RECOVERY_SCALE_STOPPED_SESSION_IDS.map(() => '?').join(', ')
      database.run(
        `UPDATE sessions SET status = 'exited', work_status = 'exited', updated_at = ?
         WHERE id IN (${stoppedPlaceholders})`,
        FIXED_TIME + 2,
        ...RECOVERY_SCALE_STOPPED_SESSION_IDS
      )
      const archivedPlaceholders = RECOVERY_SCALE_ARCHIVED_SESSION_IDS.map(() => '?').join(', ')
      database.run(
        `UPDATE sessions
         SET status = 'archived', work_status = 'exited', archived_at = ?, updated_at = ?
         WHERE id IN (${archivedPlaceholders})`,
        FIXED_TIME + 3,
        FIXED_TIME + 3,
        ...RECOVERY_SCALE_ARCHIVED_SESSION_IDS
      )
      prepareDedicatedForkSourceScenes(database)
      seedForkSources(database)
      database.exec('COMMIT;')
    } catch (error) {
      database.exec('ROLLBACK;')
      throw error
    }

    const transactions = new DomainTransactionManager(database)
    const worktrees = new WorktreeService(database, transactions, {
      stopRuns: async () => undefined
    })
    const environments = new SessionEnvironmentRepository(database)
    const gitStates = new SessionGitStateRepository(database)
    for (const source of forkSources()) {
      await gitStates.refresh(source.executionContextId, FIXED_TIME + 4)
    }
    await seedHealthyManagedWorktrees(
      dataDirectory,
      database,
      worktrees,
      environments
    )
    await seedInterruptedForkOperations(dataDirectory, database, transactions, worktrees)
    restoreForegroundFocus(database)
  } finally {
    database.close()
  }
}

export function historyMarker(sessionId: string): string {
  const index = RECOVERY_SCALE_SESSION_IDS.indexOf(sessionId)
  if (index < 0) throw new Error(`Unknown recovery scale Session: ${sessionId}`)
  // The 16-card level deliberately makes each real PTY narrow. Keep the
  // marker shorter than the Worktree-aware zsh prompt so shell cursor redraws
  // do not overwrite the evidence during a real resize/restart cycle.
  return `MH${String(index + 1).padStart(2, '0')}`
}

export async function readRecoveryScaleCounts(
  dataDirectory: string
): Promise<RecoveryScaleDatabaseCounts> {
  const database = RuntimeDatabase.openReadOnly(join(dataDirectory, DATABASE_NAME))
  try {
    const count = (sql: string): number => Number(database.get<{ count: number | bigint }>(sql)?.count ?? 0)
    return {
      sessions: count('SELECT COUNT(*) AS count FROM sessions WHERE archived_at IS NULL'),
      recoverySessions: count(
        "SELECT COUNT(*) AS count FROM sessions WHERE archived_at IS NULL AND work_status = 'interrupted'"
      ),
      recoveryWorkspaces: count(
        `SELECT COUNT(DISTINCT tasks.workspace_id) AS count
           FROM sessions JOIN tasks ON tasks.id = sessions.task_id
          WHERE sessions.archived_at IS NULL AND sessions.work_status = 'interrupted'`
      ),
      recoveryTasks: count(
        "SELECT COUNT(DISTINCT task_id) AS count FROM sessions WHERE archived_at IS NULL AND work_status = 'interrupted'"
      ),
      recoveryScenes: count(
        `SELECT COUNT(DISTINCT session_mounts.scene_id) AS count
           FROM sessions JOIN session_mounts ON session_mounts.session_id = sessions.id
          WHERE sessions.archived_at IS NULL AND sessions.work_status = 'interrupted'`
      ),
      workspaces: count('SELECT COUNT(*) AS count FROM workspaces WHERE archived_at IS NULL'),
      tasks: count('SELECT COUNT(*) AS count FROM tasks WHERE archived_at IS NULL'),
      scenes: count('SELECT COUNT(*) AS count FROM scenes WHERE archived_at IS NULL'),
      worktrees: count("SELECT COUNT(*) AS count FROM worktrees WHERE state <> 'removed'"),
      managedWorktreeBindings: count(
        'SELECT COUNT(*) AS count FROM session_environment_bindings WHERE managed_worktree_id IS NOT NULL'
      ),
      forkOperations: count(
        "SELECT COUNT(*) AS count FROM session_fork_intents WHERE submission_key GLOB 'scale-interrupted-fork-*'"
      ),
      interruptedForkOperations: count(
        `SELECT COUNT(*) AS count FROM session_fork_intents
         WHERE submission_key GLOB 'scale-interrupted-fork-*'
           AND stage NOT IN ('succeeded', 'failed')`
      )
    }
  } finally {
    database.close()
  }
}

export async function readRecoveryScaleReconciliation(
  dataDirectory: string
): Promise<RecoveryScaleReconciliation> {
  const database = RuntimeDatabase.openReadOnly(join(dataDirectory, DATABASE_NAME))
  try {
    const count = (sql: string): number => Number(
      database.get<{ count: number | bigint }>(sql)?.count ?? 0
    )
    const forkSessionIds = database.all<{ session_id: string }>(
      `SELECT session_id FROM session_fork_intents
       WHERE submission_key GLOB 'scale-interrupted-fork-*'
       ORDER BY submission_key`
    ).map(({ session_id }) => session_id)
    return {
      worktreeReady: count("SELECT COUNT(*) AS count FROM worktrees WHERE state = 'ready'"),
      worktreeFailed: count("SELECT COUNT(*) AS count FROM worktrees WHERE state = 'failed'"),
      managedBindingReady: count(
        `SELECT COUNT(*) AS count FROM session_environment_bindings
         WHERE managed_worktree_id IS NOT NULL AND state = 'ready'`
      ),
      managedBindingFailed: count(
        `SELECT COUNT(*) AS count FROM session_environment_bindings
         WHERE managed_worktree_id IS NOT NULL AND state = 'failed'`
      ),
      forkFailed: count(
        `SELECT COUNT(*) AS count FROM session_fork_intents
         WHERE submission_key GLOB 'scale-interrupted-fork-*' AND stage = 'failed'`
      ),
      forkFailureNotifications: count(
        `SELECT COUNT(*) AS count FROM domain_events
         WHERE event_type = 'agent.notification'
           AND event_id GLOB 'fork-operation:*:failed'`
      ),
      forkSessionIds
    }
  } finally {
    database.close()
  }
}

export async function countRegisteredScaleLinkedWorktrees(dataDirectory: string): Promise<number> {
  let count = 0
  for (const repository of scaleRepositories(dataDirectory)) {
    const output = await execFileAsync('git', ['-C', repository.directory, 'worktree', 'list', '--porcelain'])
    count += output.stdout.split('\n').filter((line) => line.startsWith('worktree ')).length - 1
  }
  return count
}

export async function readHealthyManagedSessions(
  dataDirectory: string
): Promise<RecoveryScaleManagedSession[]> {
  const database = RuntimeDatabase.openReadOnly(join(dataDirectory, DATABASE_NAME))
  try {
    const placeholders = RECOVERY_SCALE_HEALTHY_WORKTREE_SESSION_IDS.map(() => '?').join(', ')
    return database.all<{ session_id: string; worktree_path: string }>(
      `SELECT bindings.session_id, worktrees.worktree_path
         FROM session_environment_bindings AS bindings
         JOIN worktrees ON worktrees.id = bindings.managed_worktree_id
        WHERE bindings.session_id IN (${placeholders})
        ORDER BY bindings.session_id`,
      ...RECOVERY_SCALE_HEALTHY_WORKTREE_SESSION_IDS
    ).map(({ session_id, worktree_path }) => ({
      sessionId: session_id,
      worktreePath: worktree_path
    }))
  } finally {
    database.close()
  }
}

function seedAdditionalBaseTasks(database: RuntimeDatabase): void {
  const extraTaskIds: string[] = []
  for (let index = 1; index <= 3; index += 1) {
    const suffix = String(index).padStart(5, '0')
    const taskId = `scale-task-extra-${suffix}`
    const sceneId = `scale-extra-scene-${suffix}`
    extraTaskIds.push(taskId)
    database.run(
      `INSERT INTO tasks (
         id, workspace_id, parent_task_id, execution_context_id, title, status,
         created_at, updated_at, archived_at, sort_key, version, is_pinned,
         pin_sort_key, last_opened_at
       ) VALUES (?, 'scale-workspace', NULL, 'scale-context', ?, 'active',
         ?, ?, NULL, ?, 1, 0, '', ?)`,
      taskId,
      `Scale Task 1.${index + 1}`,
      FIXED_TIME + index,
      FIXED_TIME + index,
      `00000${index + 1}`,
      FIXED_TIME + index
    )
    database.run(
      `INSERT INTO scenes (
         id, task_id, name, mode, root_node_id, created_at, updated_at,
         archived_at, title_pinned, sort_key, layout_revision
       ) VALUES (?, ?, ?, 'card', NULL, ?, ?, NULL, 0, '000001', 1)`,
      sceneId,
      taskId,
      `Scale Extra ${index}`,
      FIXED_TIME + index,
      FIXED_TIME + index
    )
    database.run(
      `INSERT INTO window_task_placements (window_id, task_id, ordinal, updated_at)
       VALUES ('main-window-1', ?, ?, ?)`,
      taskId,
      index,
      FIXED_TIME + index
    )
  }
  database.run(
    `UPDATE workspaces SET task_order_json = ?, updated_at = ? WHERE id = 'scale-workspace'`,
    JSON.stringify(['scale-task', ...extraTaskIds]),
    FIXED_TIME + 4
  )
}

interface ScaleRepository {
  workspaceId: string
  executionContextId: string
  directory: string
}

function scaleRepositories(dataDirectory: string): ScaleRepository[] {
  const parent = dirname(dataDirectory)
  return Array.from({ length: 5 }, (_, index) => {
    if (index === 0) {
      return {
        workspaceId: 'scale-workspace',
        executionContextId: 'scale-context',
        directory: resolve(parent, 'matou_workspace')
      }
    }
    const suffix = String(index + 1).padStart(5, '0')
    return {
      workspaceId: `scale-workspace-${suffix}`,
      executionContextId: `scale-context-${suffix}`,
      directory: resolve(parent, `matou_workspace_${suffix}`)
    }
  })
}

async function initializeRepository(repository: ScaleRepository): Promise<void> {
  await execFileAsync('git', ['init', '-b', 'main'], { cwd: repository.directory })
  await execFileAsync('git', ['config', 'user.email', 'matou-scale@example.invalid'], {
    cwd: repository.directory
  })
  await execFileAsync('git', ['config', 'user.name', 'Matou Scale'], {
    cwd: repository.directory
  })
  await writeFile(join(repository.directory, 'README.md'), `${repository.workspaceId}\n`)
  await execFileAsync('git', ['add', 'README.md'], { cwd: repository.directory })
  await execFileAsync('git', ['commit', '-m', 'scale recovery fixture'], {
    cwd: repository.directory
  })
  repository.directory = await realpath(repository.directory)
}

interface ForkSource extends ScaleRepository {
  sessionId: string
}

function forkSources(dataDirectory?: string): ForkSource[] {
  const repositories = scaleRepositories(dataDirectory ?? join(process.cwd(), 'matou-data'))
  return repositories.map((repository, index) => ({
    ...repository,
    sessionId: index === 0
      ? 'scale-sibling-00017'
      : `scale-catalog-${String(index + 1).padStart(5, '0')}-00001-00005`
  }))
}

function seedForkSources(database: RuntimeDatabase): void {
  const sources = forkSources()
  for (const [index, source] of sources.entries()) {
    const mount = database.get<{ id: string; scene_id: string }>(
      `SELECT id, scene_id FROM session_mounts WHERE session_id = ? ORDER BY created_at, id LIMIT 1`,
      source.sessionId
    )
    if (!mount) throw new Error(`Scale Fork source ${source.sessionId} has no mount`)
    const nodeId = `scale-fork-source-node-${String(index + 1).padStart(2, '0')}`
    database.run(
      `INSERT INTO scene_nodes (id, scene_id, parent_node_id, kind, direction, ordinal, created_at)
       VALUES (?, ?, NULL, 'mount', NULL, 0, ?)`,
      nodeId,
      mount.scene_id,
      FIXED_TIME + index
    )
    database.run('UPDATE session_mounts SET scene_node_id = ? WHERE id = ?', nodeId, mount.id)
    database.run('UPDATE scenes SET root_node_id = ? WHERE id = ?', nodeId, mount.scene_id)
    database.run(
      `UPDATE sessions SET kind = 'claude-code', title = ?, updated_at = ? WHERE id = ?`,
      `Scale Fork Source ${index + 1}`,
      FIXED_TIME + index,
      source.sessionId
    )
    database.run(
      `INSERT INTO provider_bindings (
         id, session_id, provider, provider_session_id, resume_state, restore_state,
         metadata_json, created_at, updated_at, validated_at
       ) VALUES (?, ?, 'claude-code', ?, 'available', 'none', ?, ?, ?, ?)`,
      `scale-source-binding-${index + 1}`,
      source.sessionId,
      `scale-provider-source-${index + 1}`,
      JSON.stringify({ canFork: true }),
      FIXED_TIME + index,
      FIXED_TIME + index,
      FIXED_TIME + index
    )
  }
}

function prepareDedicatedForkSourceScenes(database: RuntimeDatabase): void {
  for (const [index, source] of forkSources().entries()) {
    if (index === 0) continue
    const owner = database.get<{ task_id: string }>(
      'SELECT task_id FROM sessions WHERE id = ?',
      source.sessionId
    )
    if (!owner) throw new Error(`Scale Fork source ${source.sessionId} has no Task`)
    const sceneId = `scale-fork-source-scene-${String(index + 1).padStart(2, '0')}`
    database.run(
      `INSERT INTO scenes (
         id, task_id, name, mode, root_node_id, created_at, updated_at,
         archived_at, title_pinned, sort_key, layout_revision
       ) VALUES (?, ?, ?, 'card', NULL, ?, ?, NULL, 0, '000003', 1)`,
      sceneId,
      owner.task_id,
      `Scale Fork Source ${index + 1}`,
      FIXED_TIME + index,
      FIXED_TIME + index
    )
    database.run(
      'UPDATE session_canvas_memberships SET scene_id = ?, updated_at = ? WHERE session_id = ?',
      sceneId,
      FIXED_TIME + index,
      source.sessionId
    )
    database.run(
      'UPDATE session_mounts SET scene_id = ? WHERE session_id = ?',
      sceneId,
      source.sessionId
    )
  }
}

async function seedHealthyManagedWorktrees(
  dataDirectory: string,
  database: RuntimeDatabase,
  worktrees: WorktreeService,
  environments: SessionEnvironmentRepository
): Promise<void> {
  const repository = scaleRepositories(dataDirectory)[0]!
  const root = join(dataDirectory, 'scale-managed-worktrees')
  await mkdir(root, { recursive: true })
  for (const [index, sessionId] of RECOVERY_SCALE_HEALTHY_WORKTREE_SESSION_IDS.entries()) {
    const ordinal = index + 1
    const worktreeId = `scale-managed-worktree-${ordinal}`
    const path = join(root, `healthy-${ordinal}`)
    await worktrees.create(scaleCommand(`healthy-worktree-${ordinal}`), {
      id: worktreeId,
      executionContextId: `scale-managed-context-${ordinal}`,
      workspaceId: repository.workspaceId,
      repositoryRoot: repository.directory,
      path,
      branch: `scale-managed-${ordinal}`,
      baseRef: 'HEAD',
      setupPolicy: [],
      now: FIXED_TIME + 10 + ordinal
    })
    environments.bindOwnedWorktree({
      sessionId,
      worktreeId,
      activate: true,
      now: FIXED_TIME + 10 + ordinal
    })
  }
  expectManagedBindingCount(database, 5)
}

async function seedInterruptedForkOperations(
  dataDirectory: string,
  database: RuntimeDatabase,
  transactions: DomainTransactionManager,
  worktrees: WorktreeService
): Promise<void> {
  const workflow = new ForkWorkflowService(dataDirectory, database, transactions, {
    stopRuns: async () => undefined
  })
  for (const [index, source] of forkSources(dataDirectory).entries()) {
    const mount = database.get<{ scene_id: string }>(
      `SELECT scene_id FROM session_mounts WHERE session_id = ? ORDER BY created_at, id LIMIT 1`,
      source.sessionId
    )
    if (!mount) throw new Error(`Scale Fork source ${source.sessionId} has no scene`)
    const ordinal = index + 1
    const accepted = await workflow.createForkChild(scaleCommand(`interrupted-fork-${ordinal}`), {
      windowId: 'main-window-1',
      sceneId: mount.scene_id,
      sourceSessionId: source.sessionId,
      name: `Interrupted Scale Fork ${ordinal}`,
      worktreeMode: 'new',
      submissionKey: `scale-interrupted-fork-${ordinal}`,
      now: FIXED_TIME + 30 + ordinal
    })
    const operationId = accepted.forkProgress?.operationId
    if (!operationId) throw new Error(`Scale Fork ${ordinal} did not persist an operation`)
    const identity = database.get<{
      worktree_id: string
      target_execution_context_id: string
      worktree_path: string
      branch_name: string
      repository_root: string
    }>(
      `SELECT fork.worktree_id, fork.target_execution_context_id, fork.worktree_path,
              fork.branch_name, worktrees.repository_root
       FROM session_fork_intents AS fork
       JOIN worktrees ON worktrees.id = fork.worktree_id
       WHERE fork.operation_id = ?`,
      operationId
    )
    if (!identity) throw new Error(`Scale Fork ${ordinal} has no Worktree identity`)
    await worktrees.create(scaleCommand(`interrupted-fork-worktree-${ordinal}`), {
      id: identity.worktree_id,
      executionContextId: identity.target_execution_context_id,
      workspaceId: source.workspaceId,
      repositoryRoot: identity.repository_root,
      path: identity.worktree_path,
      branch: identity.branch_name,
      baseRef: 'HEAD',
      setupPolicy: [],
      now: FIXED_TIME + 40 + ordinal
    })
    // Reproduce the real crash boundary where the durable identity says one
    // branch while the already-created Git worktree was externally switched.
    // Startup must isolate these five Forks instead of launching providers in
    // an untrusted directory.
    await execFileAsync('git', [
      '-C', identity.worktree_path, 'checkout', '-b', `scale-foreign-${ordinal}`
    ])
  }
  expectManagedBindingCount(database, 10)
}

function restoreForegroundFocus(database: RuntimeDatabase): void {
  database.run(
    `UPDATE window_navigation SET active_workspace_id = 'scale-workspace', updated_at = ?
     WHERE window_id = 'main-window-1'`,
    FIXED_TIME + 100
  )
  database.run(
    `INSERT INTO window_workspace_focus (window_id, workspace_id, active_task_id, updated_at)
     VALUES ('main-window-1', 'scale-workspace', 'scale-task', ?)
     ON CONFLICT(window_id, workspace_id) DO UPDATE SET
       active_task_id = excluded.active_task_id, updated_at = excluded.updated_at`,
    FIXED_TIME + 100
  )
  database.run(
    `INSERT INTO window_task_focus (window_id, task_id, active_scene_id, updated_at)
     VALUES ('main-window-1', 'scale-task', 'scale-sibling-scene', ?)
     ON CONFLICT(window_id, task_id) DO UPDATE SET
       active_scene_id = excluded.active_scene_id, updated_at = excluded.updated_at`,
    FIXED_TIME + 100
  )
  database.run(
    `INSERT INTO window_scene_focus (window_id, scene_id, active_session_id, updated_at)
     VALUES ('main-window-1', 'scale-sibling-scene', ?, ?)
     ON CONFLICT(window_id, scene_id) DO UPDATE SET
       active_session_id = excluded.active_session_id, updated_at = excluded.updated_at`,
    RECOVERY_SCALE_ACTIVE_SESSION_ID,
    FIXED_TIME + 100
  )
  database.run(
    `INSERT INTO window_task_focus (window_id, task_id, active_scene_id, updated_at)
     VALUES ('main-window-1', ?, 'scale-catalog-scene-00002-00001', ?)
     ON CONFLICT(window_id, task_id) DO UPDATE SET
       active_scene_id = excluded.active_scene_id, updated_at = excluded.updated_at`,
    RECOVERY_SCALE_BACKGROUND_TASK_ID,
    FIXED_TIME + 100
  )
}

function expectManagedBindingCount(database: RuntimeDatabase, expected: number): void {
  const row = database.get<{ count: number | bigint }>(
    `SELECT COUNT(*) AS count FROM session_environment_bindings
     WHERE managed_worktree_id IS NOT NULL`
  )
  if (Number(row?.count ?? 0) !== expected) {
    throw new Error(`Expected ${expected} managed Worktree bindings`)
  }
}

function scaleCommand(suffix: string) {
  return {
    commandId: `scale-recovery-${suffix}`,
    commandType: 'scale-recovery-fixture',
    requestHash: suffix
  }
}

function sessionIds(prefix: string, count: number): string[] {
  return Array.from({ length: count }, (_, index) =>
    `${prefix}-${String(index + 1).padStart(5, '0')}`)
}
