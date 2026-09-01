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
import { ForkWorkflowError, ForkWorkflowService } from './fork-workflow-service'

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
  it('creates a current-worktree Fork child from a valid Claude conversation', async () => {
    const source = bootstrapClaude('provider-parent')

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
      `SELECT source_session_id, source_provider_session_id, state, worktree_mode
       FROM session_fork_intents WHERE session_id = ?`,
      result.session!.id
    )).toEqual({
      source_session_id: source.sessionId,
      source_provider_session_id: 'provider-parent',
      state: 'pending',
      worktree_mode: 'current'
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
    expect(result.graph.focusedSessionId).toBe(result.session!.id)
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
    await writeFile(join(workspaceRoot, 'source-only.txt'), 'uncommitted source change')

    const result = await service.createForkChild(command('new-worktree'), {
      windowId: 'window-1', sceneId: source.sceneId, sourceSessionId: source.sessionId,
      name: '隔离修复 / API', worktreeMode: 'new', now: 30
    })

    expect(result.forkState).toBe('pending')
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
    const allowSetup = join(dataRoot, 'allow-setup')
    service = new ForkWorkflowService(
      dataRoot, database, new DomainTransactionManager(database), {
        stopRuns: async () => undefined,
        setupPolicyForWorkspace: () => [{
          command: '/bin/sh', args: ['-c', `test -f ${JSON.stringify(allowSetup)}`]
        }]
      }
    )

    const failed = await service.createForkChild(command('setup-fails'), {
      windowId: 'window-1', sceneId: source.sceneId, sourceSessionId: source.sessionId,
      name: '安装依赖', worktreeMode: 'new', now: 30
    })
    expect(failed).toMatchObject({ forkState: 'failed', error: expect.any(String) })
    expect(failed.graph.nodes.find(({ sessionId }) => sessionId === failed.session!.id))
      .toMatchObject({ forkState: 'failed', forkAttempt: 0, workStatus: 'error' })
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
    expect(retried.worktree).toMatchObject({ state: 'ready' })
    expect(database.get(
      `SELECT worktree_id, target_execution_context_id, worktree_path, branch_name
       FROM session_fork_intents WHERE session_id = ?`, failed.session!.id
    )).toEqual(intentBefore)
    expect(database.get(
      'SELECT state, attempt_count FROM session_fork_intents WHERE session_id = ?',
      failed.session!.id
    )).toEqual({ state: 'pending', attempt_count: 1 })
    expect(retried.graph.nodes.find(({ sessionId }) => sessionId === retried.session!.id))
      .toMatchObject({ forkState: 'pending', forkAttempt: 1, workStatus: 'starting' })
  })

  it('removes a failed Fork card and relation while retaining its real worktree', async () => {
    await initializeGitRepository(workspaceRoot)
    const source = bootstrapClaude('provider-parent')
    service = new ForkWorkflowService(
      dataRoot, database, new DomainTransactionManager(database), {
        stopRuns: async () => undefined,
        setupPolicyForWorkspace: () => [{ command: '/bin/sh', args: ['-c', 'exit 19'] }]
      }
    )
    const failed = await service.createForkChild(command('remove-failed-create'), {
      windowId: 'window-1', sceneId: source.sceneId, sourceSessionId: source.sessionId,
      name: '失败分支', worktreeMode: 'new', now: 30
    })
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

async function initializeGitRepository(path: string): Promise<void> {
  await exec('git', ['init', path])
  await exec('git', ['-C', path, 'config', 'user.email', 'matou@example.test'])
  await exec('git', ['-C', path, 'config', 'user.name', 'Matou Test'])
  await writeFile(join(path, 'README.md'), 'root\n')
  await exec('git', ['-C', path, 'add', 'README.md'])
  await exec('git', ['-C', path, 'commit', '-m', 'initial'])
}

function command(commandId: string) {
  return { commandId, commandType: 'fork-workflow', requestHash: `hash-${commandId}` }
}
