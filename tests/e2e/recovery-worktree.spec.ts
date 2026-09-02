import { execFile } from 'node:child_process'
import { access, mkdir, readFile, realpath, rename, writeFile } from 'node:fs/promises'
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

  test('locates a moved Worktree after rejecting the wrong repository and branch', async () => {
    let fixture = await launchMatou()
    const worktreePath = join(fixture.rootDirectory, 'owned-worktree')
    const movedWorktreePath = join(fixture.rootDirectory, 'relocated', 'owned-worktree')
    const wrongBranchPath = join(fixture.rootDirectory, 'wrong-branch-worktree')
    const wrongRepositoryPath = join(fixture.rootDirectory, 'wrong-repository')
    const worktreeCwdMarker = join(fixture.rootDirectory, 'moved-worktree-cwd.txt')
    try {
      await assertVisibleWindowsOnSecondaryColorLcd(fixture)
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
      await exec('git', [
        '-C', fixture.workspaceDirectory, 'worktree', 'add', '-b', 'feature/wrong-environment-e2e',
        wrongBranchPath, 'HEAD'
      ])
      await mkdir(wrongRepositoryPath, { recursive: true })
      await initializeRepository(wrongRepositoryPath)
      seedOwnedWorktree(fixture, worktreePath)

      fixture = await restartMatou(fixture)
      await assertVisibleWindowsOnSecondaryColorLcd(fixture)
      await expect(fixture.page.getByRole('button', { name: '打开运行环境：Worktree' }))
        .toBeVisible({ timeout: 20_000 })
      const initialSurface = activeSurface(fixture.page)
      expect(await initialSurface.getAttribute('data-session-id')).toBe(bootstrapSessionId)
      const initialWorktreePid = await positivePid(initialSurface)

      await stopMatouPreservingData(fixture)
      await expect.poll(() => processExists(initialWorktreePid)).toBe(false)
      const assetsBeforeMove = readSessionAssets(fixture, bootstrapSessionId)
      const runsBeforeMove = readSessionRuns(fixture, bootstrapSessionId)
      await mkdir(join(fixture.rootDirectory, 'relocated'), { recursive: true })
      await rename(worktreePath, movedWorktreePath)
      const canonicalMovedWorktreePath = await realpath(movedWorktreePath)
      fixture = await restartMatou(fixture)
      await assertVisibleWindowsOnSecondaryColorLcd(fixture)

      const pane = fixture.page.getByTestId('terminal-pane')
      await expect(pane.getByRole('status', { name: '运行环境Worktree 需要恢复' }))
        .toBeVisible({ timeout: 20_000 })
      await expect(pane.locator('.terminal-surface')).not.toHaveAttribute('data-pid', /[1-9][0-9]*/)
      await expect(fixture.page.getByRole('tab', { name: /Shell/ })).toBeVisible()
      expect(readSessionAssets(fixture, bootstrapSessionId)).toEqual(assetsBeforeMove)
      expect(readSessionRuns(fixture, bootstrapSessionId)).toEqual(runsBeforeMove)
      expect(readEnvironmentAuthority(fixture)).toMatchObject({
        activeTarget: 'worktree', state: 'missing', cwd: worktreePath, worktreePath
      })

      await setNextEnvironmentDirectory(fixture, wrongRepositoryPath)
      await fixture.page.getByRole('button', { name: '打开运行环境：待恢复' }).click()
      let environmentDialog = fixture.page.getByRole('dialog', { name: '运行环境' })
      await environmentDialog.getByRole('button', { name: '定位已移动的 Worktree' }).click()
      await expect(environmentDialog.locator('.environment-control-menu__feedback'))
        .toHaveText('所选目录不属于当前仓库')
      expect(readEnvironmentAuthority(fixture)).toMatchObject({
        activeTarget: 'worktree', state: 'missing', cwd: worktreePath, worktreePath
      })

      await setNextEnvironmentDirectory(fixture, wrongBranchPath)
      await environmentDialog.getByRole('button', { name: '定位已移动的 Worktree' }).click()
      await expect(environmentDialog.locator('.environment-control-menu__feedback'))
        .toHaveText('所选 Worktree 的分支与原会话不一致')
      expect(readEnvironmentAuthority(fixture)).toMatchObject({
        activeTarget: 'worktree', state: 'missing', cwd: worktreePath, worktreePath
      })

      await setNextEnvironmentDirectory(fixture, movedWorktreePath)
      await environmentDialog.getByRole('button', { name: '定位已移动的 Worktree' }).click()
      await expect(environmentDialog).toHaveCount(0, { timeout: 30_000 })
      await expect(fixture.page.getByRole('button', { name: '打开运行环境：Worktree' }))
        .toBeVisible({ timeout: 20_000 })
      const movedSurface = activeSurface(fixture.page)
      const movedPid = await differentPositivePid(movedSurface, initialWorktreePid)
      await expect.poll(async () => realpath(await processCwd(movedPid)))
        .toBe(await realpath(movedWorktreePath))
      await terminalCommand(movedSurface, `pwd > '${worktreeCwdMarker}'`)
      await expect.poll(() => readTrimmed(worktreeCwdMarker)).toBe(canonicalMovedWorktreePath)
      expect(readSessionAssets(fixture, bootstrapSessionId)).toEqual(assetsBeforeMove)
      expect(readEnvironmentAuthority(fixture)).toMatchObject({
        activeTarget: 'worktree', state: 'ready', cwd: canonicalMovedWorktreePath,
        worktreePath: canonicalMovedWorktreePath
      })
      const worktrees = (await exec('git', [
        '-C', fixture.workspaceDirectory, 'worktree', 'list', '--porcelain'
      ])).stdout
      expect(worktrees).toContain(`worktree ${canonicalMovedWorktreePath}`)
      expect(worktrees).not.toContain(`worktree ${worktreePath}\n`)

      await fixture.page.getByRole('button', { name: '打开运行环境：Worktree' }).click()
      environmentDialog = fixture.page.getByRole('dialog', { name: '运行环境' })
      await environmentDialog.getByRole('button', { name: '交接到 Local' }).click()
      await expect(fixture.page.getByRole('button', { name: '打开运行环境：Local' }))
        .toBeVisible({ timeout: 20_000 })
      const localSurface = activeSurface(fixture.page)
      const localPid = await differentPositivePid(localSurface, movedPid)
      await expect.poll(async () => realpath(await processCwd(localPid)))
        .toBe(await realpath(fixture.workspaceDirectory))
      expect(readEnvironmentAuthority(fixture)).toMatchObject({
        activeTarget: 'local', state: 'ready', cwd: fixture.workspaceDirectory,
        worktreePath: canonicalMovedWorktreePath
      })

      await fixture.page.getByRole('button', { name: '打开运行环境：Local' }).click()
      environmentDialog = fixture.page.getByRole('dialog', { name: '运行环境' })
      await environmentDialog.getByRole('button', { name: '交接到自有 Worktree' }).click()
      await expect(fixture.page.getByRole('button', { name: '打开运行环境：Worktree' }))
        .toBeVisible({ timeout: 20_000 })
      const returnedSurface = activeSurface(fixture.page)
      const returnedPid = await differentPositivePid(returnedSurface, localPid)
      await expect.poll(async () => realpath(await processCwd(returnedPid)))
        .toBe(await realpath(movedWorktreePath))
      expect(readEnvironmentAuthority(fixture)).toMatchObject({
        activeTarget: 'worktree', state: 'ready', cwd: canonicalMovedWorktreePath,
        worktreePath: canonicalMovedWorktreePath
      })
      await assertVisibleWindowsOnSecondaryColorLcd(fixture)
    } finally {
      await fixture.close()
    }
  })

  test('restores a deleted owned Worktree and hands one real PTY between Worktree and Local', async () => {
    let fixture = await launchMatou()
    const worktreePath = join(fixture.rootDirectory, 'owned-worktree')
    const localCwdMarker = join(fixture.rootDirectory, 'local-cwd.txt')
    const worktreeCwdMarker = join(fixture.rootDirectory, 'worktree-cwd.txt')
    try {
      await assertVisibleWindowsOnSecondaryColorLcd(fixture)
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
      await assertVisibleWindowsOnSecondaryColorLcd(fixture)
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
      await assertVisibleWindowsOnSecondaryColorLcd(fixture)

      const pane = fixture.page.getByTestId('terminal-pane')
      await expect(pane.getByRole('status', { name: '运行环境Worktree 需要恢复' }))
        .toBeVisible({ timeout: 20_000 })
      await expect(pane.locator('.terminal-surface')).not.toHaveAttribute('data-pid', /[1-9][0-9]*/)
      await expect(fixture.page.getByRole('button', { name: '打开运行环境：待恢复' })).toBeVisible()
      await expect(fixture.page.getByRole('button', { name: '打开 Git' })).toContainText('Git 不可用')
      expect(readSessionAssets(fixture, sessionId)).toEqual(assetsBeforeMissing)
      expect(readSessionRuns(fixture, sessionId)).toEqual(runsBeforeMissing)
      expect(readLatestRun(fixture, sessionId)?.status).not.toMatch(/^(starting|running)$/)
      expect(readEnvironmentAuthority(fixture)).toMatchObject({
        activeTarget: 'worktree', state: 'missing', cwd: worktreePath, worktreePath
      })

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
      expect(authority.state).toBe('ready')
      expect(authority.cwd).toBe(worktreePath)
      expect(authority.worktreePath).toBe(worktreePath)
      await expect(access(worktreePath).then(() => true, () => false)).resolves.toBe(true)
      await assertVisibleWindowsOnSecondaryColorLcd(fixture)
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
  state: string
  cwd: string
  worktreePath: string
} {
  const database = RuntimeDatabase.openReadOnly(join(fixture.dataDirectory, 'matou.sqlite'))
  try {
    const row = database.get<{
      binding_count: number
      active_target: string
      state: string
      cwd: string
      worktree_path: string
    }>(
      `SELECT COUNT(*) AS binding_count, bindings.active_target, bindings.state,
              sessions.cwd, worktrees.worktree_path
       FROM session_environment_bindings AS bindings
       JOIN sessions ON sessions.id = bindings.session_id
       JOIN worktrees ON worktrees.id = bindings.managed_worktree_id
       WHERE bindings.managed_worktree_id = 'environment-e2e-worktree'`
    )
    if (!row) throw new Error('owned Worktree authority was not persisted')
    return {
      bindingCount: row.binding_count, activeTarget: row.active_target,
      state: row.state, cwd: row.cwd, worktreePath: row.worktree_path
    }
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

async function setNextEnvironmentDirectory(fixture: MatouFixture, path: string): Promise<void> {
  await fixture.app.evaluate(({ ipcMain }, selectedPath) => {
    const channel = 'matou:select-session-environment-directory'
    ipcMain.removeHandler(channel)
    ipcMain.handle(channel, () => selectedPath)
  }, path)
}

async function assertVisibleWindowsOnSecondaryColorLcd(fixture: MatouFixture): Promise<void> {
  await expect.poll(() => fixture.app.evaluate(({ BrowserWindow, screen }) => {
    const primary = screen.getPrimaryDisplay()
    const colorLcd = screen.getAllDisplays().filter(({ id, internal, label }) =>
      id !== primary.id && (internal || /color\s*lcd|内建视网膜显示器/i.test(label)))
    const visibleWindows = BrowserWindow.getAllWindows().filter((window) => window.isVisible())
    const displays = visibleWindows.map((window) => screen.getDisplayMatching(window.getBounds()))
    return {
      primaryLabel: primary.label,
      colorLcdCount: colorLcd.length,
      visibleWindowCount: visibleWindows.length,
      allWindowsOnColorLcd: displays.every(({ id }) => colorLcd.some((display) => display.id === id)),
      windowsOnPrimary: displays.filter(({ id }) => id === primary.id).length
    }
  })).toEqual({
    primaryLabel: 'XV272U',
    colorLcdCount: 1,
    visibleWindowCount: 1,
    allWindowsOnColorLcd: true,
    windowsOnPrimary: 0
  })
}
