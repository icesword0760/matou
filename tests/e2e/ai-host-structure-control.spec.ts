import { access, readFile } from 'node:fs/promises'
import { join } from 'node:path'
import { DatabaseSync } from 'node:sqlite'

import { expect, test } from '@playwright/test'

import {
  git,
  launchAiHostControl,
  providerEvents,
  providerInput,
  runMtInSession,
  seedResumableProviderSession,
  sessionCard,
  showChildLevel,
  type AiHostControlFixture
} from './fixtures/ai-host-control-fixture'

interface ForkBatchItem {
  itemKey: string
  title: string
  state: 'created' | 'ready' | 'started' | 'failed'
  sessionRef?: string
  error?: string
  environment: ForkEnvironment
}

interface ForkBatchResult {
  kind: 'fork-batch'
  batchKey: string
  succeeded: number
  failed: number
  items: ForkBatchItem[]
  retry?: { batchKey: string; itemKeys: string[] }
}

interface ForkedResult {
  kind: 'forked'
  state: 'created' | 'ready' | 'started'
  sessionRef: string
  environment: ForkEnvironment
}

interface RemovalPreview {
  kind: 'removal-preview'
  impact: {
    sessions: number
    descendants: number
    preservesProjectFiles: true
    preservesBranches: true
    preservesWorktrees: true
  }
  confirmationRef: string
}

interface RemovedResult {
  kind: 'removed'
  removedSessions: number
}

interface ErrorResult {
  code: string
  message: string
}

type ForkEnvironment =
  | { mode: 'current' }
  | { mode: 'existing-worktree'; branch: string; worktreeRef: string }
  | { mode: 'new-worktree'; branch: string }

interface ForkItemInput {
  itemKey: string
  title: string
  environment: ForkEnvironment
  prompt?: string
  start?: boolean
}

const optionTitles = ['轻量适配方案', '服务层重构方案', '完整架构升级'] as const

function readDatabase<T>(
  fixture: AiHostControlFixture,
  read: (database: DatabaseSync) => T
): T {
  const database = new DatabaseSync(join(fixture.dataDirectory, 'matou.sqlite'), { readOnly: true })
  try {
    return read(database)
  } finally {
    database.close()
  }
}

test.describe('AI Host structure control in the real app', () => {
  test.setTimeout(240_000)

  test('creates three named child sessions with mixed user-selected environments', async () => {
    const fixture = await launchAiHostControl()
    try {
      const parent = await seedResumableProviderSession(fixture)
      const existing = await runMtInSession<ForkedResult>(fixture, parent.sessionId, [
        'fork', 'child', 'self', '--title', '既有 Worktree 基线',
        '--environment-json', JSON.stringify({ mode: 'new-worktree', branch: 'e2e/existing-base' }),
        '--submission-key', 'e2e-existing-base', '--json'
      ])
      const existingSessionId = sessionId(existing.value.sessionRef)
      await waitForFork(fixture, existingSessionId, 'succeeded')
      const existingWorktree = worktreeForSession(fixture, existingSessionId)

      const items: ForkItemInput[] = [
        { itemKey: 'light', title: optionTitles[0], environment: { mode: 'current' } },
        {
          itemKey: 'service', title: optionTitles[1],
          environment: {
            mode: 'existing-worktree', branch: existingWorktree.branch,
            worktreeRef: `worktree:${existingWorktree.id}`
          }
        },
        {
          itemKey: 'architecture', title: optionTitles[2],
          environment: { mode: 'new-worktree', branch: 'e2e/complete-architecture' }
        }
      ]
      const result = await forkChildren(fixture, parent.sessionId, items, 'e2e-three-options')

      expect(result.items.map(({ state }) => state)).toEqual(['ready', 'ready', 'ready'])
      expect(result.items.map(({ title }) => title)).toEqual(optionTitles)
      expect(result.items.map(({ environment }) => environment)).toEqual(items.map(({ environment }) => environment))
      await expectCallerFocus(fixture, parent.sessionId)
      await showChildLevel(fixture.page, 4)
      for (const title of optionTitles) await expect(sessionCard(fixture.page, title)).toBeVisible()
      expect(childTitles(fixture, parent.sessionId)).toEqual([
        '既有 Worktree 基线', ...optionTitles
      ])
    } finally {
      await fixture.close()
    }
  })

  test('keeps all-current options in the caller environment and preserves focus', async () => {
    const fixture = await launchAiHostControl()
    try {
      const parent = await seedResumableProviderSession(fixture)
      const items: ForkItemInput[] = optionTitles.map((title, index) => ({
        itemKey: `current-${index + 1}`, title, environment: { mode: 'current' }
      }))
      const result = await forkChildren(fixture, parent.sessionId, items, 'e2e-all-current')

      expect(result.items.map(({ state }) => state)).toEqual(['ready', 'ready', 'ready'])
      expect(result.items.every(({ environment }) => environment.mode === 'current')).toBe(true)
      expect(sessionCwds(fixture, result.items)).toEqual([
        fixture.repositoryDirectory, fixture.repositoryDirectory, fixture.repositoryDirectory
      ])
      expect(worktreeRows(fixture)).toHaveLength(0)
      await expectCallerFocus(fixture, parent.sessionId)
      await showChildLevel(fixture.page, 3)
      for (const title of optionTitles) await expect(sessionCard(fixture.page, title)).toBeVisible()
    } finally {
      await fixture.close()
    }
  })

  test('creates every selected new Worktree at its requested branch', async () => {
    const fixture = await launchAiHostControl()
    try {
      const parent = await seedResumableProviderSession(fixture)
      const branches = ['e2e/light-option', 'e2e/service-option', 'e2e/architecture-option']
      const items: ForkItemInput[] = optionTitles.map((title, index) => ({
        itemKey: `worktree-${index + 1}`, title,
        environment: { mode: 'new-worktree', branch: branches[index]! }
      }))
      const result = await forkChildren(fixture, parent.sessionId, items, 'e2e-all-worktrees')

      expect(result.items.map(({ state }) => state)).toEqual(['ready', 'ready', 'ready'])
      expect(worktreeRows(fixture).map(({ branch, state }) => ({ branch, state })))
        .toEqual(branches.map((branch) => ({ branch, state: 'ready' })))
      const porcelain = await git(fixture, ['worktree', 'list', '--porcelain'])
      for (const branch of branches) expect(porcelain).toContain(`branch refs/heads/${branch}`)
      await expectCallerFocus(fixture, parent.sessionId)
      await showChildLevel(fixture.page, 3)
      for (const title of optionTitles) await expect(sessionCard(fixture.page, title)).toBeVisible()
    } finally {
      await fixture.close()
    }
  })

  test('reports one branch collision and retries only that failed option', async () => {
    const collision = {
      triggerBranch: 'e2e/collision-trigger',
      blockerBranch: 'e2e/collision-blocker',
      collisionBranch: 'e2e/collision-target'
    }
    const fixture = await launchAiHostControl({ branchCollision: collision })
    try {
      const parent = await seedResumableProviderSession(fixture)
      const items: ForkItemInput[] = [
        {
          itemKey: 'trigger', title: optionTitles[0],
          environment: { mode: 'new-worktree', branch: collision.triggerBranch }
        },
        {
          itemKey: 'blocker', title: optionTitles[1],
          environment: { mode: 'new-worktree', branch: collision.blockerBranch }
        },
        {
          itemKey: 'collision', title: optionTitles[2],
          environment: { mode: 'new-worktree', branch: collision.collisionBranch }
        }
      ]
      const initial = await forkChildren(
        fixture, parent.sessionId, items, 'e2e-one-collision', 6
      )

      expect(initial.items.map(({ state }) => state)).toEqual(['ready', 'ready', 'failed'])
      expect(initial.retry).toEqual({ batchKey: 'e2e-one-collision', itemKeys: ['collision'] })
      expect(initial.items[2]?.error).toMatch(/branch|revision|分支/i)
      const stableRefs = initial.items.slice(0, 2).map(({ sessionRef }) => sessionRef)
      await git(fixture, ['branch', '-D', collision.collisionBranch])

      const retry = await forkChildren(
        fixture, parent.sessionId, items, 'e2e-one-collision', 0, ['collision']
      )
      expect(retry.items.map(({ state }) => state)).toEqual(['ready', 'ready', 'ready'])
      expect(retry.items.slice(0, 2).map(({ sessionRef }) => sessionRef)).toEqual(stableRefs)
      expect(sessionTitleCounts(fixture, optionTitles)).toEqual([1, 1, 1])
      expect(worktreeRows(fixture).map(({ branch }) => branch).sort()).toEqual([
        collision.blockerBranch, collision.collisionBranch, collision.triggerBranch
      ].sort())
    } finally {
      await fixture.close()
    }
  })

  test('delivers a start prompt only after its provider is ready', async () => {
    const fixture = await launchAiHostControl()
    try {
      const parent = await seedResumableProviderSession(fixture)
      const prompt = '落实已选实施方案'
      const items: ForkItemInput[] = [{
        itemKey: 'start-selected', title: '已启动实施方案', environment: { mode: 'current' },
        prompt, start: true
      }]
      const result = await forkChildren(fixture, parent.sessionId, items, 'e2e-start-options')

      expect(result.items.map(({ state }) => state)).toEqual(['started'])
      const providers = result.items.map(({ sessionRef }) => providerForSession(
        fixture, sessionId(sessionRef)
      ))
      for (const [index, providerId] of providers.entries()) {
        await expect.poll(() => providerInput(fixture, providerId), {
          message: `等待 ${items[index]!.title} 的首个任务进入 provider`, timeout: 30_000
        }).toContain(prompt)
      }
      const events = await providerEvents(fixture)
      for (const providerId of providers) {
        const readyIndex = events.findIndex((event) =>
          event.event === 'ready' && event.providerId === providerId
        )
        const inputIndex = events.findIndex((event) =>
          event.event === 'input' && event.providerId === providerId && event.value === prompt
        )
        expect(readyIndex).toBeGreaterThanOrEqual(0)
        expect(inputIndex).toBeGreaterThan(readyIndex)
      }
      await expectCallerFocus(fixture, parent.sessionId)
    } finally {
      await fixture.close()
    }
  })

  test('revalidates a changed subtree and preserves project files, branches, and Worktrees', async () => {
    const fixture = await launchAiHostControl()
    try {
      const parent = await seedResumableProviderSession(fixture)
      const child = await createFork(fixture, parent.sessionId, `session:${parent.sessionId}`, {
        title: '待移除方案树', submissionKey: 'e2e-remove-root',
        environment: { mode: 'new-worktree', branch: 'e2e/preserved-branch' }
      })
      const childId = sessionId(child.sessionRef)
      await waitForFork(fixture, childId, 'succeeded')
      const worktree = worktreeForSession(fixture, childId)
      const grandchild = await createFork(fixture, parent.sessionId, child.sessionRef, {
        title: '第一层实现', submissionKey: 'e2e-remove-grandchild',
        environment: { mode: 'current' }
      })
      await waitForFork(fixture, sessionId(grandchild.sessionRef), 'succeeded')

      const firstPreview = (await runMtInSession<RemovalPreview>(fixture, parent.sessionId, [
        'remove', 'preview', child.sessionRef, '--scope', 'subtree', '--json'
      ])).value
      expect(firstPreview.impact).toMatchObject({
        sessions: 2,
        descendants: 1,
        preservesProjectFiles: true,
        preservesBranches: true,
        preservesWorktrees: true
      })

      const secondGrandchild = await createFork(fixture, parent.sessionId, child.sessionRef, {
        title: '第二层实现', submissionKey: 'e2e-remove-mutation',
        environment: { mode: 'current' }
      })
      await waitForFork(fixture, sessionId(secondGrandchild.sessionRef), 'succeeded')
      const stale = await runMtInSession<ErrorResult>(fixture, parent.sessionId, [
        'remove', 'commit', firstPreview.confirmationRef, '--json'
      ], 4)
      expect(stale.value.code).toBe('CONFIRMATION_STALE')

      const secondPreview = (await runMtInSession<RemovalPreview>(fixture, parent.sessionId, [
        'remove', 'preview', child.sessionRef, '--scope', 'subtree', '--json'
      ])).value
      expect(secondPreview.impact).toMatchObject({ sessions: 3, descendants: 2 })
      const projectFileBefore = await readFile(`${fixture.repositoryDirectory}/project-state.txt`, 'utf8')
      const branchesBefore = await git(fixture, ['branch', '--format=%(refname:short)'])
      const worktreesBefore = await git(fixture, ['worktree', 'list', '--porcelain'])

      const removed = (await runMtInSession<RemovedResult>(fixture, parent.sessionId, [
        'remove', 'commit', secondPreview.confirmationRef, '--json'
      ])).value
      expect(removed.removedSessions).toBe(3)
      expect(archivedSessionCount(fixture, [
        childId, sessionId(grandchild.sessionRef), sessionId(secondGrandchild.sessionRef)
      ])).toBe(3)
      expect(await readFile(`${fixture.repositoryDirectory}/project-state.txt`, 'utf8'))
        .toBe(projectFileBefore)
      expect(await git(fixture, ['branch', '--format=%(refname:short)'])).toBe(branchesBefore)
      expect(await git(fixture, ['worktree', 'list', '--porcelain'])).toBe(worktreesBefore)
      await expect(access(worktree.path)).resolves.toBeUndefined()
      expect(worktreeForId(fixture, worktree.id)).toMatchObject({
        path: worktree.path, branch: worktree.branch
      })
      await expectCallerFocus(fixture, parent.sessionId)
    } finally {
      await fixture.close()
    }
  })
})

async function forkChildren(
  fixture: AiHostControlFixture,
  callerSessionId: string,
  items: ForkItemInput[],
  batchKey: string,
  status = 0,
  retryItemKeys: string[] = []
): Promise<ForkBatchResult> {
  return (await runMtInSession<ForkBatchResult>(fixture, callerSessionId, [
    'fork', 'children', 'self', '--items-json', JSON.stringify(items),
    '--batch-key', batchKey,
    ...retryItemKeys.flatMap((itemKey) => ['--retry-item-key', itemKey]),
    '--json'
  ], status)).value
}

async function createFork(
  fixture: AiHostControlFixture,
  callerSessionId: string,
  source: string,
  input: { title: string; submissionKey: string; environment: ForkEnvironment }
): Promise<ForkedResult> {
  return (await runMtInSession<ForkedResult>(fixture, callerSessionId, [
    'fork', 'child', source, '--title', input.title,
    '--environment-json', JSON.stringify(input.environment),
    '--submission-key', input.submissionKey, '--json'
  ])).value
}

async function expectCallerFocus(
  fixture: AiHostControlFixture,
  sessionIdValue: string
): Promise<void> {
  const card = fixture.page.locator(`[data-session-card="${sessionIdValue}"]`)
  await expect(card).toHaveAttribute('aria-current', 'true')
  expect(readDatabase(fixture, (database) => {
    const row = database.prepare(
      'SELECT active_session_id FROM window_scene_focus WHERE active_session_id = ?'
    ).get(sessionIdValue) as { active_session_id?: unknown } | undefined
    return row?.active_session_id
  })).toBe(sessionIdValue)
}

async function waitForFork(
  fixture: AiHostControlFixture,
  sessionIdValue: string,
  stage: 'succeeded' | 'failed'
): Promise<void> {
  await expect.poll(() => readDatabase(fixture, (database) => {
    const row = database.prepare(
      'SELECT stage FROM session_fork_intents WHERE session_id = ?'
    ).get(sessionIdValue) as { stage?: unknown } | undefined
    return row?.stage
  }), { message: `等待 Fork ${sessionIdValue} 进入 ${stage}`, timeout: 90_000 }).toBe(stage)
}

function sessionId(ref: string | undefined): string {
  if (!ref?.startsWith('session:')) throw new Error(`Expected session ref, received ${String(ref)}`)
  return ref.slice('session:'.length)
}

function worktreeForSession(
  fixture: AiHostControlFixture,
  sessionIdValue: string
): { id: string; path: string; branch: string; state: string }
{
  return readDatabase(fixture, (database) => {
    const row = database.prepare(
      `SELECT worktrees.id, worktrees.worktree_path AS path,
              worktrees.branch_name AS branch, worktrees.state
       FROM session_environment_bindings AS bindings
       JOIN worktrees ON worktrees.id = bindings.managed_worktree_id
       WHERE bindings.session_id = ?`
    ).get(sessionIdValue) as Record<string, unknown> | undefined
    if (!row) throw new Error(`Expected Worktree for ${sessionIdValue}`)
    return { id: String(row.id), path: String(row.path), branch: String(row.branch), state: String(row.state) }
  })
}

function worktreeForId(
  fixture: AiHostControlFixture,
  worktreeId: string
): { path: string; branch: string; state: string }
{
  return readDatabase(fixture, (database) => {
    const row = database.prepare(
      'SELECT worktree_path AS path, branch_name AS branch, state FROM worktrees WHERE id = ?'
    ).get(worktreeId) as Record<string, unknown> | undefined
    if (!row) throw new Error(`Expected retained Worktree ${worktreeId}`)
    return { path: String(row.path), branch: String(row.branch), state: String(row.state) }
  })
}

function worktreeRows(
  fixture: AiHostControlFixture
): Array<{ id: string; path: string; branch: string; state: string }> {
  return readDatabase(fixture, (database) => database.prepare(
    'SELECT id, worktree_path AS path, branch_name AS branch, state FROM worktrees ORDER BY created_at, id'
  ).all().map((row) => ({
    id: String(row.id), path: String(row.path), branch: String(row.branch), state: String(row.state)
  })))
}

function sessionCwds(fixture: AiHostControlFixture, items: ForkBatchItem[]): string[] {
  return readDatabase(fixture, (database) => items.map(({ sessionRef }) => {
    const row = database.prepare('SELECT cwd FROM sessions WHERE id = ?')
      .get(sessionId(sessionRef)) as { cwd?: unknown } | undefined
    return String(row?.cwd)
  }))
}

function providerForSession(fixture: AiHostControlFixture, sessionIdValue: string): string {
  return readDatabase(fixture, (database) => {
    const row = database.prepare('SELECT provider_session_id FROM provider_bindings WHERE session_id = ?')
      .get(sessionIdValue) as { provider_session_id?: unknown } | undefined
    if (typeof row?.provider_session_id !== 'string') {
      throw new Error(`Expected provider binding for ${sessionIdValue}`)
    }
    return row.provider_session_id
  })
}

function childTitles(fixture: AiHostControlFixture, parentSessionId: string): string[] {
  return readDatabase(fixture, (database) => database.prepare(
    `SELECT sessions.title
     FROM session_relations_current AS relations
     JOIN sessions ON sessions.id = relations.from_session_id
     WHERE relations.to_session_id = ? AND relations.relation_kind = 'forked-from'
       AND sessions.archived_at IS NULL
     ORDER BY relations.created_at, sessions.id`
  ).all(parentSessionId).map((row) => String(row.title)))
}

function sessionTitleCounts(
  fixture: AiHostControlFixture,
  titles: readonly string[]
): number[] {
  return readDatabase(fixture, (database) => titles.map((title) => {
    const row = database.prepare(
      'SELECT COUNT(*) AS count FROM sessions WHERE title = ? AND archived_at IS NULL'
    ).get(title) as { count: number }
    return row.count
  }))
}

function archivedSessionCount(fixture: AiHostControlFixture, sessionIds: string[]): number {
  return readDatabase(fixture, (database) => {
    const placeholders = sessionIds.map(() => '?').join(', ')
    const row = database.prepare(
      `SELECT COUNT(*) AS count FROM sessions WHERE id IN (${placeholders}) AND archived_at IS NOT NULL`
    ).get(...sessionIds) as { count: number }
    return row.count
  })
}
