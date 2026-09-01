import { execFile } from 'node:child_process'
import { access, readFile, realpath, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { promisify } from 'node:util'

import { expect, test } from '@playwright/test'

import { RuntimeDatabase } from '../../apps/runtime/src/storage/database'
import {
  launchMatou,
  restartMatou,
  stopMatouPreservingData,
  type MatouFixture
} from './matou-fixture'
import { activeSurface, terminalCommand } from './fixtures/session-canvas-fixture'

const exec = promisify(execFile)

test.describe('real Session environment recovery', () => {
  test.setTimeout(120_000)

  test('restores a deleted owned Worktree and hands one real PTY between Worktree and Local', async () => {
    let fixture = await launchMatou()
    const worktreePath = join(fixture.rootDirectory, 'owned-worktree')
    const localCwdMarker = join(fixture.rootDirectory, 'local-cwd.txt')
    const worktreeCwdMarker = join(fixture.rootDirectory, 'worktree-cwd.txt')
    try {
      const bootstrapSurface = activeSurface(fixture.page)
      await expect(bootstrapSurface).toHaveAttribute('data-session-id', /.+/, { timeout: 20_000 })
      const bootstrapSessionId = requiredAttribute(
        await bootstrapSurface.getAttribute('data-session-id'), 'bootstrap Session identity'
      )
      const bootstrapPid = await positivePid(bootstrapSurface)
      await initializeRepository(fixture.workspaceDirectory)
      await stopMatouPreservingData(fixture)
      await expect.poll(() => processExists(bootstrapPid)).toBe(false)
      await exec('git', [
        '-C', fixture.workspaceDirectory, 'worktree', 'add', '-b', 'feature/environment-e2e',
        worktreePath, 'HEAD'
      ])
      seedOwnedWorktree(fixture, worktreePath)

      fixture = await restartMatou(fixture)
      await expect(fixture.page.getByRole('button', { name: '打开运行环境：Worktree' }))
        .toBeVisible({ timeout: 20_000 })
      await expect(fixture.page.getByRole('button', { name: '打开 Git' }))
        .toContainText('feature/environment-e2e')
      const initialSurface = activeSurface(fixture.page)
      const sessionId = requiredAttribute(await initialSurface.getAttribute('data-session-id'), 'Session identity')
      expect(sessionId).toBe(bootstrapSessionId)
      const initialWorktreePid = await positivePid(initialSurface)

      await stopMatouPreservingData(fixture)
      await expect.poll(() => processExists(initialWorktreePid)).toBe(false)
      const assetsBeforeMissing = readSessionAssets(fixture, sessionId)
      const runsBeforeMissing = readSessionRuns(fixture, sessionId)
      await exec('git', ['-C', fixture.workspaceDirectory, 'worktree', 'remove', '--force', worktreePath])
      fixture = await restartMatou(fixture)

      const pane = fixture.page.getByTestId('terminal-pane')
      await expect(pane.getByRole('status', { name: '运行环境Worktree 需要恢复' }))
        .toBeVisible({ timeout: 20_000 })
      await expect(pane.locator('.terminal-surface')).not.toHaveAttribute('data-pid', /[1-9][0-9]*/)
      await expect(fixture.page.getByRole('button', { name: '打开运行环境：待恢复' })).toBeVisible()
      await expect(fixture.page.getByRole('button', { name: '打开 Git' })).toContainText('Git 不可用')
      expect(readSessionAssets(fixture, sessionId)).toEqual(assetsBeforeMissing)
      expect(readSessionRuns(fixture, sessionId)).toEqual(runsBeforeMissing)
      expect(readLatestRun(fixture, sessionId)?.status).not.toMatch(/^(starting|running)$/)

      await pane.getByRole('button', { name: '恢复 Worktree' }).click()
      await expect(pane.getByRole('status', { name: '运行环境Worktree 需要恢复' }))
        .toHaveCount(0, { timeout: 30_000 })
      await expect(fixture.page.getByRole('button', { name: '打开运行环境：Worktree' }))
        .toBeVisible()
      const restoredSurface = activeSurface(fixture.page)
      const worktreePid = await positivePid(restoredSurface)
      const worktreeRun = requiredRun(readLatestRun(fixture, sessionId))
      expect(worktreeRun.status).toBe('running')
      expect(worktreeRun.pid).toBe(worktreePid)
      expect(readSessionAssets(fixture, sessionId)).toEqual(assetsBeforeMissing)
      await expect.poll(async () => realpath(await processCwd(worktreePid)))
        .toBe(await realpath(worktreePath))
      await terminalCommand(restoredSurface, `pwd > '${worktreeCwdMarker}'`)
      await expect.poll(() => readTrimmed(worktreeCwdMarker)).toBe(worktreePath)

      await fixture.page.getByRole('button', { name: '打开运行环境：Worktree' }).click()
      await fixture.page.getByRole('button', { name: '交接到 Local' }).click()
      await expect(fixture.page.getByRole('button', { name: '打开运行环境：Local' }))
        .toBeVisible({ timeout: 20_000 })
      const localSurface = activeSurface(fixture.page)
      const localPid = await differentPositivePid(localSurface, worktreePid)
      await expect.poll(() => processExists(worktreePid)).toBe(false)
      await expect.poll(() => readRun(fixture, worktreeRun.id)).toMatchObject({
        status: 'interrupted', endedAt: expect.any(Number)
      })
      const localRun = requiredRun(readLatestRun(fixture, sessionId))
      expect(localRun).toMatchObject({ status: 'running', pid: localPid })
      expect(readSessionAssets(fixture, sessionId)).toEqual(assetsBeforeMissing)
      await expect.poll(async () => realpath(await processCwd(localPid)))
        .toBe(await realpath(fixture.workspaceDirectory))
      await terminalCommand(localSurface, `pwd > '${localCwdMarker}'`)
      await expect.poll(() => readTrimmed(localCwdMarker)).toBe(fixture.workspaceDirectory)

      await fixture.page.getByRole('button', { name: '打开运行环境：Local' }).click()
      await fixture.page.getByRole('button', { name: '交接到自有 Worktree' }).click()
      await expect(fixture.page.getByRole('button', { name: '打开运行环境：Worktree' }))
        .toBeVisible({ timeout: 20_000 })
      const returnedSurface = activeSurface(fixture.page)
      const returnedPid = await differentPositivePid(returnedSurface, localPid)
      await expect.poll(() => processExists(localPid)).toBe(false)
      await expect.poll(() => readRun(fixture, localRun.id)).toMatchObject({
        status: 'interrupted', endedAt: expect.any(Number)
      })
      const returnedRun = requiredRun(readLatestRun(fixture, sessionId))
      expect(returnedRun).toMatchObject({ status: 'running', pid: returnedPid })
      expect(readSessionAssets(fixture, sessionId)).toEqual(assetsBeforeMissing)
      await expect.poll(async () => realpath(await processCwd(returnedPid)))
        .toBe(await realpath(worktreePath))
      await terminalCommand(returnedSurface, `pwd > '${worktreeCwdMarker}'`)
      await expect.poll(() => readTrimmed(worktreeCwdMarker)).toBe(worktreePath)

      const authority = readEnvironmentAuthority(fixture)
      expect(authority.bindingCount).toBe(1)
      expect(authority.activeTarget).toBe('worktree')
      expect(authority.cwd).toBe(worktreePath)
      await expect(access(worktreePath).then(() => true, () => false)).resolves.toBe(true)
    } finally {
      await fixture.close()
    }
  })
})

async function initializeRepository(path: string): Promise<void> {
  await exec('git', ['-C', path, 'init', '-b', 'main'])
  await exec('git', ['-C', path, 'config', 'user.name', 'Matou E2E'])
  await exec('git', ['-C', path, 'config', 'user.email', 'matou-e2e@example.invalid'])
  await writeFile(join(path, 'baseline.txt'), 'environment e2e\n')
  await exec('git', ['-C', path, 'add', 'baseline.txt'])
  await exec('git', ['-C', path, 'commit', '-m', 'environment baseline'])
}

function seedOwnedWorktree(fixture: MatouFixture, worktreePath: string): void {
  const database = RuntimeDatabase.open(join(fixture.dataDirectory, 'matou.sqlite'))
  try {
    const session = database.get<{
      session_id: string
      workspace_id: string
      local_context_id: string
    }>(
      `SELECT sessions.id AS session_id, tasks.workspace_id,
              sessions.execution_context_id AS local_context_id
       FROM sessions
       JOIN tasks ON tasks.id = sessions.task_id
       WHERE sessions.archived_at IS NULL
       ORDER BY sessions.created_at ASC LIMIT 1`
    )
    if (!session) throw new Error('Matou did not bootstrap an active Session')
    const now = Date.now()
    database.exec('BEGIN IMMEDIATE')
    try {
      database.run(
        `INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
         VALUES ('environment-e2e-worktree-context', ?, 'git-worktree', ?, ?)`,
        session.workspace_id, worktreePath, now
      )
      database.run(
        `INSERT INTO worktrees (
           id, execution_context_id, repository_root, worktree_path, branch_name,
           base_ref, state, setup_policy_json, setup_result_json, cleanup_policy,
           created_at, updated_at
         ) VALUES (
           'environment-e2e-worktree', 'environment-e2e-worktree-context', ?, ?,
           'feature/environment-e2e', 'HEAD', 'ready', '[]', '[]', 'retain-dirty', ?, ?
         )`,
        fixture.workspaceDirectory, worktreePath, now, now
      )
      database.run(
        `UPDATE session_environment_bindings
         SET local_execution_context_id = ?, managed_worktree_id = 'environment-e2e-worktree',
             active_target = 'worktree', state = 'ready', updated_at = ?
         WHERE session_id = ?`,
        session.local_context_id, now, session.session_id
      )
      database.run(
        `UPDATE sessions
         SET execution_context_id = 'environment-e2e-worktree-context', cwd = ?, updated_at = ?
         WHERE id = ?`,
        worktreePath, now, session.session_id
      )
      database.exec('COMMIT')
    } catch (error) {
      database.exec('ROLLBACK')
      throw error
    }
  } finally {
    database.close()
  }
}

function readEnvironmentAuthority(fixture: MatouFixture): {
  bindingCount: number
  activeTarget: string
  cwd: string
} {
  const database = RuntimeDatabase.openReadOnly(join(fixture.dataDirectory, 'matou.sqlite'))
  try {
    const row = database.get<{ binding_count: number; active_target: string; cwd: string }>(
      `SELECT COUNT(*) AS binding_count, bindings.active_target, sessions.cwd
       FROM session_environment_bindings AS bindings
       JOIN sessions ON sessions.id = bindings.session_id
       WHERE bindings.managed_worktree_id = 'environment-e2e-worktree'`
    )
    if (!row) throw new Error('owned Worktree authority was not persisted')
    return { bindingCount: row.binding_count, activeTarget: row.active_target, cwd: row.cwd }
  } finally {
    database.close()
  }
}

async function readTrimmed(path: string): Promise<string> {
  return (await readFile(path, 'utf8').catch(() => '')).trim()
}

interface SessionRunEvidence {
  id: string
  pid: number | null
  status: string
  endedAt: number | null
  ordinal: number
}

function readLatestRun(fixture: MatouFixture, sessionId: string): SessionRunEvidence | undefined {
  const database = RuntimeDatabase.openReadOnly(join(fixture.dataDirectory, 'matou.sqlite'))
  try {
    const row = database.get<{
      id: string
      pid: number | null
      status: string
      ended_at: number | null
      ordinal: number
    }>(
      `SELECT id, pid, status, ended_at, ordinal
       FROM session_runs WHERE session_id = ? ORDER BY ordinal DESC LIMIT 1`,
      sessionId
    )
    return row ? {
      id: row.id, pid: row.pid, status: row.status, endedAt: row.ended_at, ordinal: row.ordinal
    } : undefined
  } finally {
    database.close()
  }
}

function readRun(fixture: MatouFixture, runId: string): SessionRunEvidence | undefined {
  const database = RuntimeDatabase.openReadOnly(join(fixture.dataDirectory, 'matou.sqlite'))
  try {
    const row = database.get<{
      id: string
      pid: number | null
      status: string
      ended_at: number | null
      ordinal: number
    }>('SELECT id, pid, status, ended_at, ordinal FROM session_runs WHERE id = ?', runId)
    return row ? {
      id: row.id, pid: row.pid, status: row.status, endedAt: row.ended_at, ordinal: row.ordinal
    } : undefined
  } finally {
    database.close()
  }
}

interface SessionAssetEvidence {
  session: Array<Record<string, unknown>>
  mounts: Array<Record<string, unknown>>
  memberships: Array<Record<string, unknown>>
  relations: Array<Record<string, unknown>>
  bindings: Array<Record<string, unknown>>
}

function readSessionAssets(fixture: MatouFixture, sessionId: string): SessionAssetEvidence {
  const database = RuntimeDatabase.openReadOnly(join(fixture.dataDirectory, 'matou.sqlite'))
  try {
    return {
      session: database.all<Record<string, unknown>>(
        `SELECT id, task_id, kind, title, created_at, archived_at
         FROM sessions WHERE id = ? ORDER BY id`, sessionId
      ),
      mounts: database.all<Record<string, unknown>>(
        `SELECT id, scene_id, scene_node_id, scene_window_id, session_id, created_at
         FROM session_mounts WHERE session_id = ? ORDER BY id`, sessionId
      ),
      memberships: database.all<Record<string, unknown>>(
        `SELECT session_id, scene_id, sibling_created_seq, created_at
         FROM session_canvas_memberships WHERE session_id = ? ORDER BY session_id`, sessionId
      ),
      relations: database.all<Record<string, unknown>>(
        `SELECT relation_id, task_id, from_session_id, to_session_id, relation_kind,
                metadata_json, created_at, source_event_sequence
         FROM session_relations_current
         WHERE from_session_id = ? OR to_session_id = ? ORDER BY relation_id`,
        sessionId, sessionId
      ),
      bindings: database.all<Record<string, unknown>>(
        `SELECT session_id, local_execution_context_id, managed_worktree_id
         FROM session_environment_bindings WHERE session_id = ? ORDER BY session_id`, sessionId
      )
    }
  } finally {
    database.close()
  }
}

function readSessionRuns(fixture: MatouFixture, sessionId: string): Array<Record<string, unknown>> {
  const database = RuntimeDatabase.openReadOnly(join(fixture.dataDirectory, 'matou.sqlite'))
  try {
    return database.all<Record<string, unknown>>(
      `SELECT id, session_id, ordinal, runtime_generation, pid, status,
              started_at, ended_at, exit_code, signal
       FROM session_runs WHERE session_id = ? ORDER BY ordinal`, sessionId
    )
  } finally {
    database.close()
  }
}

function requiredRun(run: SessionRunEvidence | undefined): SessionRunEvidence {
  if (!run) throw new Error('expected Session run evidence')
  return run
}

function requiredAttribute(value: string | null, label: string): string {
  if (!value) throw new Error(`${label} is missing`)
  return value
}

async function positivePid(surface: ReturnType<typeof activeSurface>): Promise<number> {
  let pid = 0
  await expect.poll(async () => {
    pid = Number(await surface.getAttribute('data-pid'))
    return pid
  }).toBeGreaterThan(0)
  return pid
}

async function differentPositivePid(
  surface: ReturnType<typeof activeSurface>, previousPid: number
): Promise<number> {
  let pid = 0
  await expect.poll(async () => {
    pid = Number(await surface.getAttribute('data-pid'))
    return pid > 0 && pid !== previousPid
  }).toBe(true)
  return pid
}

function processExists(pid: number): boolean {
  try {
    process.kill(pid, 0)
    return true
  } catch (error) {
    return (error as NodeJS.ErrnoException).code !== 'ESRCH'
  }
}

async function processCwd(pid: number): Promise<string> {
  if (process.platform === 'linux') {
    const { stdout } = await exec('readlink', [`/proc/${pid}/cwd`])
    return stdout.trim()
  }
  const { stdout } = await exec('lsof', ['-a', '-p', String(pid), '-d', 'cwd', '-Fn'])
  return stdout.split('\n').find((line) => line.startsWith('n'))?.slice(1) ?? ''
}
