import { execFile } from 'node:child_process'
import { realpath, stat } from 'node:fs/promises'
import { promisify } from 'node:util'

import type { SessionGitState } from '@matou/domain'

import type { DatabaseTransaction, RuntimeDatabase } from '../storage/database'

const exec = promisify(execFile)

interface GitStateRow {
  execution_context_id: string
  repository_root: string | null
  state: 'ready' | 'unavailable'
  branch: string | null
  detached_head: string | null
  dirty: 0 | 1
  error_message: string | null
  updated_at: number
}

interface ExecutionContextRow {
  id: string
  cwd: string
  registered_repository_root: string | null
}

export interface PersistedSessionGitState {
  executionContextId: string
  repositoryRoot?: string
  git: SessionGitState
  error?: string
  updatedAt: number
}

/** Owns durable Git status for the execution context a Session actually runs in. */
export class SessionGitStateRepository {
  readonly #database: RuntimeDatabase

  constructor(database: RuntimeDatabase) {
    this.#database = database
  }

  get(
    executionContextId: string,
    source: DatabaseTransaction = this.#database
  ): PersistedSessionGitState | undefined {
    const row = source.get<GitStateRow>(
      'SELECT * FROM execution_context_git_states WHERE execution_context_id = ?',
      executionContextId
    )
    return row ? mapRow(row) : undefined
  }

  async refresh(executionContextId: string, now = Date.now()): Promise<PersistedSessionGitState> {
    const context = requireRow(this.#database.get<ExecutionContextRow>(
      `SELECT execution_contexts.id, execution_contexts.cwd,
              worktrees.repository_root AS registered_repository_root
       FROM execution_contexts
       LEFT JOIN worktrees ON worktrees.execution_context_id = execution_contexts.id
       WHERE execution_contexts.id = ?`, executionContextId
    ), `ExecutionContext ${executionContextId}`)
    const probed = await probeGit(context.cwd)
    const repositoryRoot = context.registered_repository_root ?? probed.repositoryRoot

    this.#database.run(
      `INSERT INTO execution_context_git_states (
         execution_context_id, repository_root, state, branch, detached_head,
         dirty, error_message, updated_at
       ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
       ON CONFLICT(execution_context_id) DO UPDATE SET
         repository_root = COALESCE(excluded.repository_root, repository_root),
         state = excluded.state,
         branch = excluded.branch,
         detached_head = excluded.detached_head,
         dirty = excluded.dirty,
         error_message = excluded.error_message,
         updated_at = excluded.updated_at`,
      executionContextId,
      repositoryRoot ?? null,
      probed.git.state,
      probed.git.state === 'ready' ? probed.git.branch ?? null : null,
      probed.git.state === 'ready' ? probed.git.detachedHead ?? null : null,
      probed.git.dirty ? 1 : 0,
      probed.error ?? null,
      now
    )
    return requireRow(this.get(executionContextId), `Git state for ${executionContextId}`)
  }

  async refreshScene(sceneId: string, now = Date.now()): Promise<void> {
    const contexts = this.#database.all<{ execution_context_id: string }>(
      `SELECT DISTINCT sessions.execution_context_id
       FROM sessions
       JOIN session_canvas_memberships AS membership ON membership.session_id = sessions.id
       WHERE membership.scene_id = ?`,
      sceneId
    )
    await Promise.all(contexts.map(({ execution_context_id: id }) => this.refresh(id, now)))
  }

  async refreshAllActive(now = Date.now()): Promise<void> {
    const contexts = this.#database.all<{ execution_context_id: string }>(
      `SELECT DISTINCT sessions.execution_context_id
       FROM sessions
       WHERE sessions.archived_at IS NULL`
    )
    await Promise.all(contexts.map(({ execution_context_id: id }) => this.refresh(id, now)))
  }
}

async function probeGit(cwd: string): Promise<Omit<PersistedSessionGitState, 'executionContextId' | 'updatedAt'>> {
  try {
    const target = await stat(cwd)
    if (!target.isDirectory()) return unavailable('path-missing')
  } catch {
    return unavailable('path-missing')
  }

  let repositoryRoot: string
  try {
    const discoveredRoot = (await exec(
      'git', ['-C', cwd, 'rev-parse', '--show-toplevel']
    )).stdout.trim()
    repositoryRoot = discoveredRoot === await realpath(cwd) ? cwd : discoveredRoot
  } catch (error) {
    return unavailable(gitError(error))
  }

  let branch: string | undefined
  try {
    branch = (await exec(
      'git', ['-C', cwd, 'symbolic-ref', '--quiet', '--short', 'HEAD']
    )).stdout.trim() || undefined
  } catch {
    branch = undefined
  }

  try {
    const dirty = (await exec(
      'git', ['-C', cwd, 'status', '--porcelain', '--untracked-files=normal']
    )).stdout.length > 0
    if (branch !== undefined) {
      return { repositoryRoot, git: { state: 'ready', branch, dirty } }
    }
    const detachedHead = (await exec(
      'git', ['-C', cwd, 'rev-parse', 'HEAD']
    )).stdout.trim()
    return { repositoryRoot, git: { state: 'ready', detachedHead, dirty } }
  } catch {
    return { repositoryRoot, ...unavailable('git-unavailable') }
  }
}

function unavailable(error: string): Pick<PersistedSessionGitState, 'git' | 'error'> {
  return { git: { state: 'unavailable', dirty: false }, error }
}

function gitError(error: unknown): string {
  if (typeof error === 'object' && error !== null && 'stderr' in error) {
    const stderr = String(error.stderr)
    if (/not a git repository/i.test(stderr)) return 'not-git-repository'
  }
  return 'git-unavailable'
}

function mapRow(row: GitStateRow): PersistedSessionGitState {
  const git: SessionGitState = row.state === 'unavailable'
    ? { state: 'unavailable', dirty: false }
    : row.branch !== null
      ? { state: 'ready', branch: row.branch, dirty: row.dirty === 1 }
      : { state: 'ready', detachedHead: row.detached_head!, dirty: row.dirty === 1 }
  return {
    executionContextId: row.execution_context_id,
    ...(row.repository_root === null ? {} : { repositoryRoot: row.repository_root }),
    git,
    ...(row.error_message === null ? {} : { error: row.error_message }),
    updatedAt: row.updated_at
  }
}

function requireRow<T>(row: T | undefined, label: string): T {
  if (row === undefined) throw new Error(`${label} does not exist`)
  return row
}
