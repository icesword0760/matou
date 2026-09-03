import { execFile } from 'node:child_process'
import { mkdir, mkdtemp, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { promisify } from 'node:util'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { HierarchyApplicationService } from '../hierarchy/hierarchy-application-service'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { ForkWorkflowService, type ForkWorkflowError } from './fork-workflow-service'

const exec = promisify(execFile)

describe('ForkWorkflowService explicit environment integration', () => {
  let dataRoot: string
  let workspaceRoot: string
  let database: RuntimeDatabase
  let hierarchy: HierarchyApplicationService
  let workflow: ForkWorkflowService

  beforeEach(async () => {
    dataRoot = await mkdtemp(join(tmpdir(), 'matou-fork-environment-'))
    workspaceRoot = join(dataRoot, 'workspace')
    await mkdir(workspaceRoot)
    await initializeGitRepository(workspaceRoot)
    database = RuntimeDatabase.open(join(dataRoot, 'matou.sqlite'))
    await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
    const transactions = new DomainTransactionManager(database)
    hierarchy = new HierarchyApplicationService(database, transactions)
    workflow = new ForkWorkflowService(dataRoot, database, transactions, {
      stopRuns: async () => undefined
    })
  })

  afterEach(() => database.close())

  it('rejects a submitted branch collision before inserting a Fork scene node', async () => {
    const source = bootstrapClaude()
    seedReadyGitState(source.executionContextId)
    await exec('git', ['-C', workspaceRoot, 'branch', 'feature/already-exists'])
    const before = database.get<{ sessions: number; nodes: number }>(
      `SELECT
         (SELECT COUNT(*) FROM sessions) AS sessions,
         (SELECT COUNT(*) FROM scene_nodes) AS nodes`
    )!

    await expect(workflow.createForkChild(command('branch-collision'), {
      windowId: 'window-1', sceneId: source.sceneId, sourceSessionId: source.sessionId,
      name: '重名分支', environment: {
        mode: 'new-worktree', branch: 'feature/already-exists'
      }, now: 30
    })).rejects.toMatchObject({ code: 'BRANCH_CONFLICT' } satisfies Partial<ForkWorkflowError>)
    expect(database.get(
      `SELECT
         (SELECT COUNT(*) FROM sessions) AS sessions,
         (SELECT COUNT(*) FROM scene_nodes) AS nodes`
    )).toEqual(before)
  })

  it('rejects an invalid submitted branch before inserting a Fork scene node', async () => {
    const source = bootstrapClaude()
    seedReadyGitState(source.executionContextId)
    const before = database.get<{ sessions: number; nodes: number }>(
      `SELECT
         (SELECT COUNT(*) FROM sessions) AS sessions,
         (SELECT COUNT(*) FROM scene_nodes) AS nodes`
    )!

    await expect(workflow.createForkChild(command('invalid-branch'), {
      windowId: 'window-1', sceneId: source.sceneId, sourceSessionId: source.sessionId,
      name: '无效分支', environment: {
        mode: 'new-worktree', branch: 'feature/invalid branch'
      }, now: 30
    })).rejects.toMatchObject({ code: 'INVALID_BRANCH' } satisfies Partial<ForkWorkflowError>)
    expect(database.get(
      `SELECT
         (SELECT COUNT(*) FROM sessions) AS sessions,
         (SELECT COUNT(*) FROM scene_nodes) AS nodes`
    )).toEqual(before)
  })

  function bootstrapClaude() {
    const result = hierarchy.bootstrapWindow(command('bootstrap'), {
      windowId: 'window-1', defaultRootDirectory: workspaceRoot,
      defaultName: 'workspace', now: 10
    })
    database.run(
      "UPDATE sessions SET kind = 'claude-code', title = 'Claude' WHERE id = ?",
      result.session!.id
    )
    database.run(
      `INSERT INTO provider_bindings (
         id, session_id, provider, provider_session_id, resume_state, restore_state,
         metadata_json, created_at, updated_at, validated_at
       ) VALUES ('binding-source', ?, 'claude-code', 'provider-source',
                 'available', 'none', '{"canFork":true}', 20, 20, 20)`,
      result.session!.id
    )
    return {
      sceneId: result.scene!.id,
      sessionId: result.session!.id,
      executionContextId: result.executionContext!.id
    }
  }

  function seedReadyGitState(executionContextId: string): void {
    database.run(
      `INSERT INTO execution_context_git_states (
         execution_context_id, repository_root, state, branch, detached_head,
         dirty, error_message, updated_at
       ) VALUES (?, ?, 'ready', 'main', NULL, 0, NULL, 20)`,
      executionContextId, workspaceRoot
    )
  }
})

async function initializeGitRepository(path: string): Promise<void> {
  await exec('git', ['init', '-b', 'main', path])
  await exec('git', ['-C', path, 'config', 'user.email', 'matou@example.test'])
  await exec('git', ['-C', path, 'config', 'user.name', 'Matou Test'])
  await writeFile(join(path, 'README.md'), 'root\n')
  await exec('git', ['-C', path, 'add', 'README.md'])
  await exec('git', ['-C', path, 'commit', '-m', 'initial'])
}

function command(commandId: string) {
  return { commandId, commandType: 'fork-workflow', requestHash: `hash-${commandId}` }
}
