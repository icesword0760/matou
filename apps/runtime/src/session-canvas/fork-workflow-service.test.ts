import { execFile } from 'node:child_process'
import { mkdir, mkdtemp, realpath, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { promisify } from 'node:util'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { HierarchyApplicationService } from '../hierarchy/hierarchy-application-service'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { WorktreeService } from '../worktrees/worktree-service'
import {
  ForkWorkflowError,
  ForkWorkflowService,
  type ExecuteForkInput
} from './fork-workflow-service'
import { SessionForkIntentRepository } from '../session/session-fork-intent-repository'

const exec = promisify(execFile)

let dataRoot: string
let workspaceRoot: string
let database: RuntimeDatabase
let hierarchy: HierarchyApplicationService
let service: ForkWorkflowService

beforeEach(async () => {
  dataRoot = await mkdtemp(join(tmpdir(), 'matou-fork-workflow-'))
  workspaceRoot = join(dataRoot, 'workspace')
  await mkdir(workspaceRoot)
  database = RuntimeDatabase.open(join(dataRoot, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  const transactions = new DomainTransactionManager(database)
  hierarchy = new HierarchyApplicationService(database, transactions)
  service = new ForkWorkflowService(dataRoot, database, transactions, {
    stopRuns: async () => undefined
  })
})

afterEach(() => database.close())

describe('ForkWorkflowService', () => {
  it('reserves one repository branch for only one durable Fork before background execution', async () => {
    await initializeGitRepository(workspaceRoot)
    const source = bootstrapClaude('provider-branch-reservation')
    seedReadyGitState(source.executionContextId)
    const environment = {
      mode: 'new-worktree' as const,
      branch: 'feature/reserved-before-execution'
    }
    const first = await service.createForkChild(command('reserve-first'), {
      windowId: 'window-1', sceneId: source.sceneId, sourceSessionId: source.sessionId,
      name: '首个预留', environment, submissionKey: 'reservation-first', now: 30
    })
    const beforeSecond = database.get<{ sessions: number; nodes: number }>(
      `SELECT
         (SELECT COUNT(*) FROM sessions) AS sessions,
         (SELECT COUNT(*) FROM scene_nodes) AS nodes`
    )!

    await expect(service.createForkChild(command('reserve-second'), {
      windowId: 'window-1', sceneId: source.sceneId, sourceSessionId: source.sessionId,
      name: '第二个预留', environment, submissionKey: 'reservation-second', now: 31
    })).rejects.toMatchObject({
      code: 'BRANCH_CONFLICT', input: environment.branch
    } satisfies Partial<ForkWorkflowError>)

    expect(database.get(
      `SELECT
         (SELECT COUNT(*) FROM sessions) AS sessions,
         (SELECT COUNT(*) FROM scene_nodes) AS nodes`
    )).toEqual(beforeSecond)
    expect(database.all(
      `SELECT operation_id, submission_key, branch_name
       FROM session_fork_intents WHERE branch_name = ?`, environment.branch
    )).toEqual([{
      operation_id: first.forkProgress!.operationId,
      submission_key: 'reservation-first',
      branch_name: environment.branch
    }])
  })

  it('keeps the Fork source cwd for the Renderer current-mode compatibility path', async () => {
    const source = bootstrapClaude('provider-renderer-current-cwd')
    const contextCwd = join(dataRoot, 'context-row-cwd')
    database.run(
      'UPDATE execution_contexts SET cwd = ? WHERE id = ?',
      contextCwd, source.executionContextId
    )

    const result = await service.createForkChild(command('renderer-current-cwd'), {
      windowId: 'window-1', sceneId: source.sceneId, sourceSessionId: source.sessionId,
      name: '继续当前会话', worktreeMode: 'current', now: 30
    })

    expect(result.session).toMatchObject({
      executionContextId: source.executionContextId,
      cwd: workspaceRoot
    })
    expect(result.session!.cwd).not.toBe(contextCwd)
  })

  it('uses the submitted branch when creating a new Worktree', async () => {
    await initializeGitRepository(workspaceRoot)
    const source = bootstrapClaude('provider-explicit-branch')
    seedReadyGitState(source.executionContextId)

    const accepted = await service.createForkChild(command('fork-explicit-branch'), {
      windowId: 'window-1', sceneId: source.sceneId, sourceSessionId: source.sessionId,
      name: '服务层重构', environment: {
        mode: 'new-worktree', branch: 'feature/service-refactor'
      }, submissionKey: 'fork-explicit-branch', now: 30
    })
    const result = await executeAccepted(
      accepted, source.sceneId, 'fork-explicit-branch-execute', 31
    )

    expect(result.worktree?.branch).toBe('feature/service-refactor')
    expect(database.get(
      'SELECT worktree_mode, branch_name FROM session_fork_intents WHERE session_id = ?',
      result.session!.id
    )).toEqual({ worktree_mode: 'new', branch_name: 'feature/service-refactor' })
  })

  it('reuses an existing Worktree context and leaves its ownership unchanged', async () => {
    await initializeGitRepository(workspaceRoot)
    const source = bootstrapClaude('provider-existing-worktree')
    seedReadyGitState(source.executionContextId)
    const existing = await seedExistingOwnedWorktree(source.sessionId, 'feature/existing-context')

    const accepted = await service.createForkChild(command('fork-existing-worktree'), {
      windowId: 'window-1', sceneId: source.sceneId, sourceSessionId: source.sessionId,
      name: 'Main 环境方案', environment: {
        mode: 'existing-worktree', branch: existing.branch,
        worktreeRef: `worktree:${existing.worktreeId}`,
        worktreeId: existing.worktreeId,
        executionContextId: existing.executionContextId
      }, submissionKey: 'fork-existing-worktree', now: 30
    })
    const result = await executeAccepted(
      accepted, source.sceneId, 'fork-existing-worktree-execute', 31
    )

    expect(result.session).toMatchObject({
      executionContextId: existing.executionContextId,
      cwd: existing.path
    })
    expect(result.forkProgress).toMatchObject({ stage: 'restoring-provider' })
    expect(database.get(
      `SELECT source_provider_session_id, worktree_mode, worktree_id, target_execution_context_id
       FROM session_fork_intents WHERE session_id = ?`,
      result.session!.id
    )).toEqual({
      source_provider_session_id: 'provider-existing-worktree',
      worktree_mode: 'current', worktree_id: null,
      target_execution_context_id: existing.executionContextId
    })
    expect(database.all(
      `SELECT session_id, managed_worktree_id, active_target
       FROM session_environment_bindings
       WHERE managed_worktree_id = ? OR session_id = ?
       ORDER BY session_id`,
      existing.worktreeId, result.session!.id
    )).toEqual([
      { session_id: existing.ownerSessionId, managed_worktree_id: existing.worktreeId, active_target: 'worktree' },
      { session_id: result.session!.id, managed_worktree_id: null, active_target: 'local' }
    ].sort((left, right) => left.session_id.localeCompare(right.session_id)))
    expect(database.get(
      'SELECT state, branch_name FROM worktrees WHERE id = ?', existing.worktreeId
    )).toEqual({ state: 'ready', branch_name: existing.branch })
  })

  it('rejects an existing Worktree whose recorded Git branch changes before final acceptance', async () => {
    await initializeGitRepository(workspaceRoot)
    const source = bootstrapClaude('provider-existing-branch-switch')
    seedReadyGitState(source.executionContextId)
    const existing = await seedExistingOwnedWorktree(source.sessionId, 'feature/stable-existing')
    const before = database.get<{ sessions: number; nodes: number }>(
      `SELECT
         (SELECT COUNT(*) FROM sessions) AS sessions,
         (SELECT COUNT(*) FROM scene_nodes) AS nodes`
    )!

    const pending = service.createForkChild(command('existing-branch-switch'), {
      windowId: 'window-1', sceneId: source.sceneId, sourceSessionId: source.sessionId,
      name: '切换中的工作树', environment: {
        mode: 'existing-worktree', branch: existing.branch,
        worktreeRef: `worktree:${existing.worktreeId}`,
        worktreeId: existing.worktreeId,
        executionContextId: existing.executionContextId
      }, now: 30
    })
    database.run(
      `UPDATE execution_context_git_states
       SET branch = 'feature/switched-elsewhere', updated_at = 31
       WHERE execution_context_id = ?`,
      existing.executionContextId
    )

    await expect(pending).rejects.toMatchObject({
      code: 'BRANCH_CONFLICT', input: existing.branch
    } satisfies Partial<ForkWorkflowError>)
    expect(database.get(
      `SELECT
         (SELECT COUNT(*) FROM sessions) AS sessions,
         (SELECT COUNT(*) FROM scene_nodes) AS nodes`
    )).toEqual(before)
  })

  it('creates a current-worktree Fork child from a valid Claude conversation', async () => {
    await initializeGitRepository(workspaceRoot)
    const source = bootstrapClaude('provider-parent')
    seedReadyGitState(source.executionContextId)

    const result = await service.createForkChild(command('fork-child'), {
      windowId: 'window-1', sceneId: source.sceneId, sourceSessionId: source.sessionId,
      name: '  修复登录  ', worktreeMode: 'current', now: 30
    })

    expect(result.session).toMatchObject({
      kind: 'claude-code', status: 'starting', title: '修复登录',
      executionContextId: source.executionContextId
    })
    expect(database.get(
      `SELECT from_session_id, to_session_id, relation_kind
       FROM session_relations_current WHERE from_session_id = ?`,
      result.session!.id
    )).toEqual({
      from_session_id: result.session!.id,
      to_session_id: source.sessionId,
      relation_kind: 'forked-from'
    })
    expect(database.get(
      `SELECT source_session_id, source_provider_session_id, state, worktree_mode, stage
       FROM session_fork_intents WHERE session_id = ?`,
      result.session!.id
    )).toEqual({
      source_session_id: source.sessionId,
      source_provider_session_id: 'provider-parent',
      state: 'pending',
      worktree_mode: 'current',
      stage: 'queued'
    })
    expect(database.get(
      `SELECT local_execution_context_id, managed_worktree_id, active_target, state
       FROM session_environment_bindings WHERE session_id = ?`, result.session!.id
    )).toEqual({
      local_execution_context_id: source.executionContextId,
      managed_worktree_id: null,
      active_target: 'local',
      state: 'ready'
    })
    expect(result.graph.nodes.find(({ sessionId }) => sessionId === result.session!.id))
      .toMatchObject({
        environment: { kind: 'local', state: 'ready' },
        git: { state: 'ready', branch: 'main', dirty: false }
      })
    expect(result.graph.focusedSessionId).toBe(result.session!.id)
  })

  it('durably accepts one complete Fork asset set for repeated submission keys', async () => {
    await initializeGitRepository(workspaceRoot)
    const source = bootstrapClaude('provider-parent')
    seedReadyGitState(source.executionContextId)
    const input = {
      windowId: 'window-1', sceneId: source.sceneId, sourceSessionId: source.sessionId,
      name: '幂等分支', worktreeMode: 'new' as const,
      submissionKey: 'stable-fork-submission', now: 30
    }

    const first = await service.createForkChild(command('durable-first'), input)
    hierarchy.activateSession({
      windowId: 'window-1', sessionId: source.sessionId, now: 31
    })
    const navigationBeforeReplay = database.get(
      'SELECT * FROM window_navigation WHERE window_id = ?', 'window-1'
    )
    const focusBeforeReplay = database.get(
      'SELECT * FROM window_scene_focus WHERE window_id = ? AND scene_id = ?',
      'window-1', source.sceneId
    )
    const eventCountBeforeReplay = database.get<{ count: number }>(
      'SELECT COUNT(*) AS count FROM domain_events'
    )!.count
    const duplicate = await service.createForkSibling(command('durable-timeout-replay'), {
      windowId: 'window-1', sceneId: 'stale-scene', sourceSessionId: 'stale-source',
      name: '', worktreeMode: 'current', submissionKey: input.submissionKey, now: 32
    })
    const operation = database.get<{
      operation_id: string
      submission_key: string
      session_id: string
      worktree_id: string
      worktree_path: string
      target_execution_context_id: string
    }>(
      'SELECT operation_id, submission_key, session_id, worktree_id, worktree_path, target_execution_context_id FROM session_fork_intents WHERE submission_key = ?',
      input.submissionKey
    )!

    expect(duplicate.session!.id).toBe(first.session!.id)
    expect(duplicate.scene!.id).toBe(source.sceneId)
    expect(duplicate.graph.sceneId).toBe(source.sceneId)
    expect(duplicate.forkProgress?.operationId).toBe(first.forkProgress?.operationId)
    expect(database.get(
      'SELECT * FROM window_navigation WHERE window_id = ?', 'window-1'
    )).toEqual(navigationBeforeReplay)
    expect(database.get(
      'SELECT * FROM window_scene_focus WHERE window_id = ? AND scene_id = ?',
      'window-1', source.sceneId
    )).toEqual(focusBeforeReplay)
    expect(database.get<{ count: number }>(
      'SELECT COUNT(*) AS count FROM domain_events'
    )!.count).toBe(eventCountBeforeReplay)
    expect(operation).toMatchObject({
      submission_key: input.submissionKey,
      session_id: first.session!.id,
      operation_id: first.forkProgress!.operationId
    })
    expect(database.get<{ sessions: number; mounts: number; memberships: number; relations: number; intents: number; worktrees: number }>(
      `SELECT
         (SELECT COUNT(*) FROM sessions WHERE id = ?) AS sessions,
         (SELECT COUNT(*) FROM session_mounts WHERE session_id = ?) AS mounts,
         (SELECT COUNT(*) FROM session_canvas_memberships WHERE session_id = ?) AS memberships,
         (SELECT COUNT(*) FROM session_relations_current WHERE from_session_id = ?) AS relations,
         (SELECT COUNT(*) FROM session_fork_intents WHERE submission_key = ?) AS intents,
         (SELECT COUNT(*) FROM worktrees WHERE id = ?) AS worktrees`,
      operation.session_id, operation.session_id, operation.session_id,
      operation.session_id, input.submissionKey, operation.worktree_id
    )).toEqual({ sessions: 1, mounts: 1, memberships: 1, relations: 1, intents: 1, worktrees: 1 })
    expect(database.get(
      'SELECT managed_worktree_id, active_target, state FROM session_environment_bindings WHERE session_id = ?',
      operation.session_id
    )).toEqual({
      managed_worktree_id: operation.worktree_id,
      active_target: 'worktree',
      state: 'recovering'
    })
    expect(database.get(
      'SELECT state FROM worktrees WHERE id = ?', operation.worktree_id
    )).toEqual({ state: 'creating' })
    await expect(realpath(operation.worktree_path)).rejects.toThrow()
  })

  it('concurrently replays one acceptance across child and sibling endpoints without duplicate assets', async () => {
    await initializeGitRepository(workspaceRoot)
    const source = bootstrapClaude('provider-parent')
    seedReadyGitState(source.executionContextId)
    const common = {
      windowId: 'window-1', sceneId: source.sceneId, sourceSessionId: source.sessionId,
      name: '并发分支', worktreeMode: 'new' as const,
      submissionKey: 'concurrent-submission', now: 30
    }

    const [child, replay] = await Promise.all([
      service.createForkChild(command('concurrent-child'), common),
      service.createForkSibling(command('concurrent-sibling'), {
        ...common, sceneId: 'stale-scene', sourceSessionId: 'stale-source',
        name: '冲突名称也被忽略', worktreeMode: 'current', now: 31
      })
    ])

    expect(replay.session!.id).toBe(child.session!.id)
    expect(replay.forkProgress).toEqual(child.forkProgress)
    expect(replay.scene!.id).toBe(source.sceneId)
    expect(database.get(
      `SELECT
         (SELECT COUNT(*) FROM session_fork_intents WHERE submission_key = ?) AS intents,
         (SELECT COUNT(*) FROM sessions WHERE id = ?) AS sessions,
         (SELECT COUNT(*) FROM session_mounts WHERE session_id = ?) AS mounts,
         (SELECT COUNT(*) FROM session_relations_current WHERE from_session_id = ?) AS relations`,
      common.submissionKey, child.session!.id, child.session!.id, child.session!.id
    )).toEqual({ intents: 1, sessions: 1, mounts: 1, relations: 1 })
  })

  it('creates a Fork sibling from the common Claude parent rather than the selected sibling output', async () => {
    const parent = bootstrapClaude('provider-parent')
    const first = await service.createForkChild(command('first-child'), {
      windowId: 'window-1', sceneId: parent.sceneId, sourceSessionId: parent.sessionId,
      name: '第一分支', worktreeMode: 'current', now: 30
    })
    seedClaudeBinding(first.session!.id, 'provider-child-later', true, 31)

    const sibling = await service.createForkSibling(command('fork-sibling'), {
      windowId: 'window-1', sceneId: parent.sceneId, sourceSessionId: first.session!.id,
      name: '并行分支', worktreeMode: 'current', now: 32
    })

    expect(database.get(
      `SELECT source_session_id, source_provider_session_id
       FROM session_fork_intents WHERE session_id = ?`, sibling.session!.id
    )).toEqual({
      source_session_id: parent.sessionId,
      source_provider_session_id: 'provider-parent'
    })
    expect(sibling.graph.nodes.filter(({ parentSessionId }) => parentSessionId === parent.sessionId)
      .map(({ title }) => title)).toEqual(['第一分支', '并行分支'])
    expect(sibling.graph.focusedSessionId).toBe(first.session!.id)
    expect(sibling.graph.nodes.filter(({ parentSessionId }) => parentSessionId === parent.sessionId)
      .map(({ sessionId }) => sessionId)).toEqual([first.session!.id, sibling.session!.id])
  })

  it('returns typed product errors for root sibling Fork and ineligible Claude attempts', async () => {
    const root = bootstrapShell()

    await expect(service.createForkSibling(command('root-sibling'), {
      windowId: 'window-1', sceneId: root.sceneId, sourceSessionId: root.sessionId,
      name: '并行分支', worktreeMode: 'current', now: 30
    })).rejects.toMatchObject({ code: 'ROOT_HAS_NO_FORK_PARENT' } satisfies Partial<ForkWorkflowError>)

    await expect(service.createForkChild(command('shell-child'), {
      windowId: 'window-1', sceneId: root.sceneId, sourceSessionId: root.sessionId,
      name: '子分支', worktreeMode: 'current', now: 31
    })).rejects.toMatchObject({ code: 'FORK_SOURCE_NOT_READY' } satisfies Partial<ForkWorkflowError>)

    database.run(
      "UPDATE sessions SET kind = 'claude-code', title = 'Claude' WHERE id = ?",
      root.sessionId
    )
    seedClaudeBinding(root.sessionId, 'provider-unfinished', false, 32)
    await expect(service.createForkChild(command('unfinished-child'), {
      windowId: 'window-1', sceneId: root.sceneId, sourceSessionId: root.sessionId,
      name: '子分支', worktreeMode: 'current', now: 33
    })).rejects.toMatchObject({ code: 'FORK_SOURCE_NOT_READY' } satisfies Partial<ForkWorkflowError>)
  })

  it('rejects a duplicate active sibling name while preserving the submitted value', async () => {
    const source = bootstrapClaude('provider-parent')
    await service.createForkChild(command('first-name'), {
      windowId: 'window-1', sceneId: source.sceneId, sourceSessionId: source.sessionId,
      name: '修复登录', worktreeMode: 'current', now: 30
    })

    await expect(service.createForkChild(command('duplicate-name'), {
      windowId: 'window-1', sceneId: source.sceneId, sourceSessionId: source.sessionId,
      name: '修复登录', worktreeMode: 'current', now: 31
    })).rejects.toMatchObject({
      code: 'DUPLICATE_NAME', input: '修复登录'
    } satisfies Partial<ForkWorkflowError>)
  })

  it('creates a real isolated worktree from HEAD while leaving source changes in place', async () => {
    await initializeGitRepository(workspaceRoot)
    const source = bootstrapClaude('provider-parent')
    seedReadyGitState(source.executionContextId)
    await writeFile(join(workspaceRoot, 'source-only.txt'), 'uncommitted source change')

    const accepted = await service.createForkChild(command('new-worktree'), {
      windowId: 'window-1', sceneId: source.sceneId, sourceSessionId: source.sessionId,
      name: '隔离修复 / API', worktreeMode: 'new', now: 30
    })
    const checkpoints: string[] = []
    const result = await executeAccepted(
      accepted, source.sceneId, 'new-worktree-execute', 31,
      { reach: (point) => { checkpoints.push(point) } }
    )

    expect(result.forkState).toBe('starting')
    expect(result.worktree).toMatchObject({ state: 'ready' })
    expect(result.session!.executionContextId).not.toBe(source.executionContextId)
    expect(result.session!.cwd).toBe(await realpath(result.worktree!.path))
    await expect(exec('git', ['-C', result.worktree!.path, 'status', '--porcelain']))
      .resolves.toMatchObject({ stdout: '' })
    await expect(exec('git', ['-C', workspaceRoot, 'status', '--porcelain']))
      .resolves.toMatchObject({ stdout: expect.stringContaining('source-only.txt') })
    expect(result.worktree!.branch).toMatch(/^matou\/隔离修复-api-[a-f0-9]{8}$/)
    expect(database.get(
      `SELECT local_execution_context_id, managed_worktree_id, active_target, state
       FROM session_environment_bindings WHERE session_id = ?`, result.session!.id
    )).toEqual({
      local_execution_context_id: source.executionContextId,
      managed_worktree_id: result.worktree!.id,
      active_target: 'worktree',
      state: 'ready'
    })
    expect(result.graph.nodes.find(({ sessionId }) => sessionId === result.session!.id))
      .toMatchObject({
        environment: { kind: 'worktree', state: 'ready' },
        git: { state: 'ready', branch: result.worktree!.branch, dirty: false }
      })
    expect(checkpoints).toEqual([
      'branch-created', 'path-created', 'setup-completed', 'session-bound'
    ])
  })

  it('forks from the revision visible when the request is accepted even if the source advances', async () => {
    await initializeGitRepository(workspaceRoot)
    const source = bootstrapClaude('provider-parent')
    seedReadyGitState(source.executionContextId)
    const acceptedRevision = (await exec('git', ['-C', workspaceRoot, 'rev-parse', 'HEAD']))
      .stdout.trim()

    const accepted = await service.createForkChild(command('frozen-worktree-base'), {
      windowId: 'window-1', sceneId: source.sceneId, sourceSessionId: source.sessionId,
      name: '固定代码版本', worktreeMode: 'new', now: 30
    })
    const acceptedWorktree = database.get<{ base_ref: string; base_revision: string | null }>(
      `SELECT worktrees.base_ref, worktrees.base_revision
       FROM worktrees
       JOIN session_fork_intents ON session_fork_intents.worktree_id = worktrees.id
       WHERE session_fork_intents.session_id = ?`,
      accepted.session!.id
    )
    expect(acceptedWorktree).toEqual({
      base_ref: acceptedRevision,
      base_revision: acceptedRevision
    })

    await writeFile(join(workspaceRoot, 'README.md'), 'source advanced after Fork acceptance\n')
    await exec('git', ['-C', workspaceRoot, 'add', 'README.md'])
    await exec('git', ['-C', workspaceRoot, 'commit', '-m', 'advance source'])
    const advancedRevision = (await exec('git', ['-C', workspaceRoot, 'rev-parse', 'HEAD']))
      .stdout.trim()
    expect(advancedRevision).not.toBe(acceptedRevision)

    const result = await executeAccepted(
      accepted, source.sceneId, 'frozen-worktree-base-execute', 31
    )
    const forkRevision = (await exec('git', ['-C', result.worktree!.path, 'rev-parse', 'HEAD']))
      .stdout.trim()
    expect(forkRevision).toBe(acceptedRevision)
  })

  it('fails a durable Fork when its reserved branch moves after acceptance', async () => {
    await initializeGitRepository(workspaceRoot)
    const source = bootstrapClaude('provider-branch-moved-after-acceptance')
    seedReadyGitState(source.executionContextId)
    const branch = 'feature/moved-after-acceptance'
    const frozenRevision = (await exec(
      'git', ['-C', workspaceRoot, 'rev-parse', 'HEAD']
    )).stdout.trim()

    const accepted = await service.createForkChild(command('branch-moved-accept'), {
      windowId: 'window-1', sceneId: source.sceneId, sourceSessionId: source.sessionId,
      name: '接受后分支竞争', environment: { mode: 'new-worktree', branch },
      submissionKey: 'branch-moved-after-acceptance', now: 30
    })
    const claim = database.get<{
      worktree_id: string
      worktree_path: string
      base_revision: string
      execution_context_id: string
      repository_root: string
      workspace_id: string
    }>(
      `SELECT intent.worktree_id, intent.worktree_path, worktrees.base_revision,
              worktrees.execution_context_id, worktrees.repository_root,
              execution_contexts.workspace_id
       FROM session_fork_intents AS intent
       JOIN worktrees ON worktrees.id = intent.worktree_id
       JOIN execution_contexts ON execution_contexts.id = worktrees.execution_context_id
       WHERE intent.operation_id = ?`,
      accepted.forkProgress!.operationId
    )!
    expect(claim.base_revision).toBe(frozenRevision)
    await exec('git', [
      '-C', workspaceRoot, 'worktree', 'add', '-b', branch,
      claim.worktree_path, frozenRevision
    ])
    await exec('git', [
      '-C', claim.worktree_path, 'checkout', '--detach', frozenRevision
    ])

    await writeFile(join(workspaceRoot, 'README.md'), 'external branch revision\n')
    await exec('git', ['-C', workspaceRoot, 'add', 'README.md'])
    await exec('git', ['-C', workspaceRoot, 'commit', '-m', 'external branch revision'])
    const externalRevision = (await exec(
      'git', ['-C', workspaceRoot, 'rev-parse', 'HEAD']
    )).stdout.trim()
    await exec('git', [
      '-C', workspaceRoot, 'update-ref', `refs/heads/${branch}`, externalRevision
    ])

    const worktrees = new WorktreeService(
      database, new DomainTransactionManager(database), { stopRuns: async () => undefined }
    )
    await expect(worktrees.create(command('branch-moved-worktree-create'), {
      id: claim.worktree_id,
      executionContextId: claim.execution_context_id,
      workspaceId: claim.workspace_id,
      repositoryRoot: claim.repository_root,
      path: claim.worktree_path,
      branch,
      baseRef: frozenRevision,
      setupPolicy: [],
      now: 31
    })).rejects.toThrow('frozen revision')
    expect(database.get(
      'SELECT state, base_revision FROM worktrees WHERE id = ?', claim.worktree_id
    )).toEqual({ state: 'failed', base_revision: frozenRevision })

    const failed = await executeAccepted(
      accepted, source.sceneId, 'branch-moved-execute', 32
    )

    expect(failed).toMatchObject({
      forkState: 'failed',
      forkProgress: { stage: 'failed' },
      error: expect.stringContaining('frozen revision')
    })
    expect(database.get(
      'SELECT state, base_revision FROM worktrees WHERE id = ?', claim.worktree_id
    )).toEqual({ state: 'failed', base_revision: frozenRevision })
    await expect(realpath(claim.worktree_path)).resolves.toEqual(expect.any(String))
    expect((await exec('git', [
      '-C', claim.worktree_path, 'rev-parse', 'HEAD'
    ])).stdout.trim()).toBe(frozenRevision)
    expect((await exec('git', [
      '-C', workspaceRoot, 'rev-parse', `refs/heads/${branch}`
    ])).stdout.trim()).toBe(externalRevision)
  })

  it('does not start external work or mutate the accepted assets with an already expired lease', async () => {
    await initializeGitRepository(workspaceRoot)
    const source = bootstrapClaude('provider-parent')
    seedReadyGitState(source.executionContextId)
    const accepted = await service.createForkChild(command('expired-accept'), {
      windowId: 'window-1', sceneId: source.sceneId, sourceSessionId: source.sessionId,
      name: '过期租约', worktreeMode: 'new', submissionKey: 'expired-submission', now: 30
    })
    const operationId = accepted.forkProgress!.operationId
    const repository = new SessionForkIntentRepository(database)
    const decision = repository.acquireLease({
      operationId, owner: 'expired-worker', now: 30, ttlMs: 1
    })
    if (decision.kind !== 'acquired') throw new Error('expired test lease missing')
    const path = database.get<{ worktree_path: string }>(
      'SELECT worktree_path FROM session_fork_intents WHERE operation_id = ?', operationId
    )!.worktree_path

    await expect(service.executeFork(command('expired-execute'), {
      windowId: 'window-1', sceneId: source.sceneId,
      operationId, lease: decision.lease, now: 31
    })).rejects.toThrow('stale Fork lease')

    await expect(realpath(path)).rejects.toThrow()
    expect(database.get(
      `SELECT fork.stage, fork.state, sessions.status, environment.state AS environment_state
       FROM session_fork_intents AS fork
       JOIN sessions ON sessions.id = fork.session_id
       JOIN session_environment_bindings AS environment ON environment.session_id = fork.session_id
       WHERE fork.operation_id = ?`, operationId
    )).toEqual({
      stage: 'queued', state: 'pending', status: 'starting', environment_state: 'recovering'
    })
  })

  it('heartbeats long external work and lets a takeover fence its later bind and failure writes', async () => {
    await initializeGitRepository(workspaceRoot)
    const source = bootstrapClaude('provider-parent')
    seedReadyGitState(source.executionContextId)
    service = new ForkWorkflowService(
      dataRoot, database, new DomainTransactionManager(database), {
        stopRuns: async () => undefined,
        setupPolicyForWorkspace: () => [{
          idempotencyKey: 'slow-setup', command: '/bin/sh', args: ['-c', 'sleep 0.2']
        }]
      }
    )
    const accepted = await service.createForkChild(command('takeover-accept'), {
      windowId: 'window-1', sceneId: source.sceneId, sourceSessionId: source.sessionId,
      name: '接管分支', worktreeMode: 'new', submissionKey: 'takeover-submission', now: 30
    })
    const operationId = accepted.forkProgress!.operationId
    const repository = new SessionForkIntentRepository(database)
    const first = repository.acquireLease({
      operationId, owner: 'worker-a', now: 30, ttlMs: 30
    })
    if (first.kind !== 'acquired') throw new Error('worker A lease missing')
    let executionError: unknown
    const executing = service.executeFork(command('takeover-execute'), {
      windowId: 'window-1', sceneId: source.sceneId,
      operationId, lease: first.lease, now: 30
    }).catch((error: unknown) => {
      executionError = error
    })

    await new Promise((resolve) => setTimeout(resolve, 50))
    expect(database.get<{ last_heartbeat_at: number }>(
      'SELECT last_heartbeat_at FROM session_fork_intents WHERE operation_id = ?', operationId
    )!.last_heartbeat_at).toBeGreaterThan(30)
    const second = repository.acquireLease({
      operationId, owner: 'worker-b', now: 10_000, ttlMs: 1_000
    })
    expect(second.kind).toBe('acquired')
    if (second.kind !== 'acquired') throw new Error('worker B lease missing')
    const stageAtTakeover = database.get<{ stage: string }>(
      'SELECT stage FROM session_fork_intents WHERE operation_id = ?', operationId
    )!.stage

    await executing
    expect(executionError).toBeInstanceOf(Error)
    expect((executionError as Error).message).toContain('stale Fork lease')
    expect(database.get(
      `SELECT fork.stage, fork.state, fork.lease_token, sessions.status,
              environment.state AS environment_state
       FROM session_fork_intents AS fork
       JOIN sessions ON sessions.id = fork.session_id
       JOIN session_environment_bindings AS environment ON environment.session_id = fork.session_id
       WHERE fork.operation_id = ?`, operationId
    )).toEqual({
      stage: stageAtTakeover, state: 'starting', lease_token: second.lease.token,
      status: 'starting', environment_state: 'recovering'
    })
  })

  it('publishes applying-setup while the real setup process is still running', async () => {
    await initializeGitRepository(workspaceRoot)
    const source = bootstrapClaude('provider-parent')
    seedReadyGitState(source.executionContextId)
    const releaseSetup = join(dataRoot, 'release-slow-setup')
    service = new ForkWorkflowService(
      dataRoot, database, new DomainTransactionManager(database), {
        stopRuns: async () => undefined,
        setupPolicyForWorkspace: () => [{
          idempotencyKey: 'observable-slow-setup',
          command: '/bin/sh',
          args: ['-c', `while [ ! -f ${JSON.stringify(releaseSetup)} ]; do sleep 0.02; done`]
        }]
      }
    )
    const accepted = await service.createForkChild(command('observable-setup-accept'), {
      windowId: 'window-1', sceneId: source.sceneId, sourceSessionId: source.sessionId,
      name: '后台准备', worktreeMode: 'new', submissionKey: 'observable-setup', now: 30
    })
    const operationId = accepted.forkProgress!.operationId
    const repository = new SessionForkIntentRepository(database)
    const decision = repository.acquireLease({
      operationId, owner: 'observable-worker', now: 30, ttlMs: 30_000
    })
    if (decision.kind !== 'acquired') throw new Error('observable setup lease missing')
    const executing = service.executeFork(command('observable-setup-execute'), {
      windowId: 'window-1', sceneId: source.sceneId,
      operationId, lease: decision.lease, now: 30
    })

    let stageError: unknown
    try {
      await expect.poll(() => repository.progressByOperation(operationId)?.stage, {
        timeout: 500
      }).toBe('applying-setup')
    } catch (error) {
      stageError = error
    }
    await writeFile(releaseSetup, 'continue')
    await expect(executing).resolves.toMatchObject({
      forkProgress: { stage: 'restoring-provider' }
    })
    if (stageError) throw stageError
  })

  it('reports that the new-worktree choice needs a Git repository before creating a node', async () => {
    const source = bootstrapClaude('provider-parent')
    const before = database.get<{ count: number }>(
      'SELECT COUNT(*) AS count FROM sessions'
    )!.count

    await expect(service.createForkChild(command('not-git'), {
      windowId: 'window-1', sceneId: source.sceneId, sourceSessionId: source.sessionId,
      name: '隔离分支', worktreeMode: 'new', now: 30
    })).rejects.toMatchObject({ code: 'GIT_REPOSITORY_REQUIRED' } satisfies Partial<ForkWorkflowError>)
    expect(database.get<{ count: number }>('SELECT COUNT(*) AS count FROM sessions')!.count)
      .toBe(before)
  })

  it('keeps a failed worktree Fork node and retries the same node, branch, and directory', async () => {
    await initializeGitRepository(workspaceRoot)
    const source = bootstrapClaude('provider-parent')
    seedReadyGitState(source.executionContextId)
    const allowSetup = join(dataRoot, 'allow-setup')
    service = new ForkWorkflowService(
      dataRoot, database, new DomainTransactionManager(database), {
        stopRuns: async () => undefined,
        setupPolicyForWorkspace: () => [{
          idempotencyKey: 'allow-setup',
          command: '/bin/sh', args: ['-c', `test -f ${JSON.stringify(allowSetup)}`]
        }]
      }
    )

    const accepted = await service.createForkChild(command('setup-fails'), {
      windowId: 'window-1', sceneId: source.sceneId, sourceSessionId: source.sessionId,
      name: '安装依赖', worktreeMode: 'new', now: 30
    })
    const failed = await executeAccepted(accepted, source.sceneId, 'setup-fails-execute', 30)
    expect(failed).toMatchObject({ forkState: 'failed', error: expect.any(String) })
    expect(failed.graph.nodes.find(({ sessionId }) => sessionId === failed.session!.id))
      .toMatchObject({
        forkProgress: { stage: 'failed', attempt: 0, error: expect.any(String) },
        workStatus: 'error'
      })
    const intentBefore = database.get<{
      worktree_id: string; target_execution_context_id: string; worktree_path: string; branch_name: string
    }>(
      `SELECT worktree_id, target_execution_context_id, worktree_path, branch_name
       FROM session_fork_intents WHERE session_id = ?`, failed.session!.id
    )!

    await writeFile(allowSetup, 'ready')
    const retried = await service.retryFork(command('retry-setup'), {
      windowId: 'window-1', sceneId: source.sceneId, sessionId: failed.session!.id, now: 31
    })

    expect(retried.forkState).toBe('pending')
    expect(retried.session!.id).toBe(failed.session!.id)
    expect(retried.worktree).toBeUndefined()
    expect(database.get(
      `SELECT worktree_id, target_execution_context_id, worktree_path, branch_name
       FROM session_fork_intents WHERE session_id = ?`, failed.session!.id
    )).toEqual(intentBefore)
    expect(database.get(
      'SELECT state, attempt_count FROM session_fork_intents WHERE session_id = ?',
      failed.session!.id
    )).toEqual({ state: 'starting', attempt_count: 1 })
    expect(retried.graph.nodes.find(({ sessionId }) => sessionId === retried.session!.id))
      .toMatchObject({
        forkProgress: { stage: 'applying-setup', attempt: 1 },
        workStatus: 'starting'
      })
    const resumed = await executeAccepted(retried, source.sceneId, 'retry-setup-execute', 32)
    expect(resumed.worktree).toMatchObject({ state: 'ready' })
    expect(resumed.session!.id).toBe(failed.session!.id)
    expect(resumed.forkProgress).toMatchObject({ stage: 'restoring-provider', attempt: 1 })
  })

  it('removes a failed Fork card and relation while retaining its real worktree', async () => {
    await initializeGitRepository(workspaceRoot)
    const source = bootstrapClaude('provider-parent')
    seedReadyGitState(source.executionContextId)
    service = new ForkWorkflowService(
      dataRoot, database, new DomainTransactionManager(database), {
        stopRuns: async () => undefined,
        setupPolicyForWorkspace: () => [{
          idempotencyKey: 'fail-setup', command: '/bin/sh', args: ['-c', 'exit 19']
        }]
      }
    )
    const accepted = await service.createForkChild(command('remove-failed-create'), {
      windowId: 'window-1', sceneId: source.sceneId, sourceSessionId: source.sessionId,
      name: '失败分支', worktreeMode: 'new', now: 30
    })
    const failed = await executeAccepted(accepted, source.sceneId, 'remove-failed-execute', 30)
    const retainedPath = database.get<{ worktree_path: string }>(
      'SELECT worktree_path FROM session_fork_intents WHERE session_id = ?', failed.session!.id
    )!.worktree_path

    const removed = service.removeFailedFork(command('remove-failed'), {
      windowId: 'window-1', sceneId: source.sceneId, sessionId: failed.session!.id, now: 31
    })

    expect(removed.graph.nodes.some(({ sessionId }) => sessionId === failed.session!.id)).toBe(false)
    expect(database.get('SELECT archived_at FROM sessions WHERE id = ?', failed.session!.id))
      .toEqual({ archived_at: 31 })
    expect(database.get(
      'SELECT relation_id FROM session_relations_current WHERE from_session_id = ?',
      failed.session!.id
    )).toBeUndefined()
    expect(database.get(
      'SELECT state FROM worktrees WHERE worktree_path = ?', retainedPath
    )).toEqual({ state: 'failed' })
    await expect(realpath(retainedPath)).resolves.toBe(retainedPath)
  })
})

function bootstrapShell() {
  const result = hierarchy.bootstrapWindow(command('bootstrap'), {
    windowId: 'window-1', defaultRootDirectory: workspaceRoot,
    defaultName: 'workspace', now: 10
  })
  return {
    sceneId: result.scene!.id,
    sessionId: result.session!.id,
    executionContextId: result.executionContext!.id
  }
}

function bootstrapClaude(providerSessionId: string) {
  const result = bootstrapShell()
  database.run(
    "UPDATE sessions SET kind = 'claude-code', title = 'Claude' WHERE id = ?",
    result.sessionId
  )
  seedClaudeBinding(result.sessionId, providerSessionId, true, 20)
  return result
}

function seedClaudeBinding(sessionId: string, providerSessionId: string, canFork: boolean, now: number) {
  database.run(
    `INSERT INTO provider_bindings (
       id, session_id, provider, provider_session_id, resume_state, restore_state,
       metadata_json, created_at, updated_at, validated_at
     ) VALUES (?, ?, 'claude-code', ?, 'available', 'none', ?, ?, ?, ?)`,
    `binding-${providerSessionId}`, sessionId, providerSessionId,
    JSON.stringify({ canFork }), now, now, now
  )
}

function seedReadyGitState(executionContextId: string): void {
  database.run(
    `INSERT INTO execution_context_git_states (
       execution_context_id, repository_root, state, branch, detached_head,
       dirty, error_message, updated_at
     ) VALUES (?, ?, 'ready', 'main', NULL, 0, NULL, 20)
     ON CONFLICT(execution_context_id) DO UPDATE SET
       repository_root = excluded.repository_root, state = 'ready', branch = 'main',
       detached_head = NULL, dirty = 0, error_message = NULL, updated_at = 20`,
    executionContextId, workspaceRoot
  )
}

async function executeAccepted(
  accepted: Awaited<ReturnType<ForkWorkflowService['createForkChild']>>,
  sceneId: string,
  commandId: string,
  now: number,
  observer?: ExecuteForkInput['observer']
) {
  const operationId = accepted.forkProgress!.operationId
  const decision = new SessionForkIntentRepository(database).acquireLease({
    operationId, owner: `test:${commandId}`, now, ttlMs: 30_000
  })
  if (decision.kind !== 'acquired') throw new Error('Fork lease was not acquired')
  return service.executeFork(command(commandId), {
    windowId: 'window-1', sceneId, operationId, lease: decision.lease, now,
    ...(observer ? { observer } : {})
  })
}

async function initializeGitRepository(path: string): Promise<void> {
  await exec('git', ['init', path])
  await exec('git', ['-C', path, 'config', 'user.email', 'matou@example.test'])
  await exec('git', ['-C', path, 'config', 'user.name', 'Matou Test'])
  await writeFile(join(path, 'README.md'), 'root\n')
  await exec('git', ['-C', path, 'add', 'README.md'])
  await exec('git', ['-C', path, 'commit', '-m', 'initial'])
}

async function seedExistingOwnedWorktree(sourceSessionId: string, branch: string) {
  const ownerSessionId = 'existing-worktree-owner'
  const executionContextId = 'context-existing-worktree'
  const worktreeId = 'existing-worktree'
  const path = join(dataRoot, 'existing-worktree')
  const taskId = database.get<{ task_id: string }>(
    'SELECT task_id FROM sessions WHERE id = ?', sourceSessionId
  )!.task_id
  const workspaceId = database.get<{ workspace_id: string }>(
    'SELECT workspace_id FROM tasks WHERE id = ?', taskId
  )!.workspace_id
  await exec('git', ['-C', workspaceRoot, 'worktree', 'add', '-b', branch, path, 'HEAD'])
  const revision = (await exec('git', ['-C', path, 'rev-parse', 'HEAD'])).stdout.trim()
  database.run(
    `INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
     VALUES (?, ?, 'git-worktree', ?, 20)`,
    executionContextId, workspaceId, path
  )
  database.run(
    `INSERT INTO worktrees (
       id, execution_context_id, repository_root, worktree_path, branch_name,
       base_ref, base_revision, state, setup_policy_json, setup_result_json,
       cleanup_policy, created_at, updated_at
     ) VALUES (?, ?, ?, ?, ?, ?, ?, 'ready', '[]', '[]', 'retain-dirty', 20, 20)`,
    worktreeId, executionContextId, workspaceRoot, path, branch, revision, revision
  )
  database.run(
    `INSERT INTO execution_context_git_states (
       execution_context_id, repository_root, state, branch, detached_head,
       dirty, error_message, updated_at
     ) VALUES (?, ?, 'ready', ?, NULL, 0, NULL, 20)`,
    executionContextId, workspaceRoot, branch
  )
  database.run(
    `INSERT INTO sessions (
       id, task_id, execution_context_id, kind, status, title, cwd,
       created_at, updated_at, last_activity_at
     ) VALUES (?, ?, ?, 'shell', 'running', 'Existing owner', ?, 20, 20, 20)`,
    ownerSessionId, taskId, executionContextId, path
  )
  database.run(
    `UPDATE session_environment_bindings
     SET managed_worktree_id = ?, active_target = 'worktree', state = 'ready', updated_at = 20
     WHERE session_id = ?`,
    worktreeId, ownerSessionId
  )
  return { ownerSessionId, executionContextId, worktreeId, branch, path }
}

function command(commandId: string) {
  return { commandId, commandType: 'fork-workflow', requestHash: `hash-${commandId}` }
}
