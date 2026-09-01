import { execFile } from 'node:child_process'
import { mkdir, mkdtemp, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { promisify } from 'node:util'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { RuntimeDatabase } from '../storage/database'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { SessionGitStateRepository } from './session-git-state-repository'

const exec = promisify(execFile)

let database: RuntimeDatabase
let states: SessionGitStateRepository
let root: string
let repositoryRoot: string

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-session-git-state-'))
  repositoryRoot = join(root, 'repository')
  await mkdir(repositoryRoot)
  await initializeRepository(repositoryRoot)
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  database.run(
    `INSERT INTO workspaces (id, name, root_directory, created_at, updated_at)
     VALUES ('workspace', 'Workspace', ?, 1, 1)`,
    repositoryRoot
  )
  database.run(
    `INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
     VALUES ('local', 'workspace', 'plain-directory', ?, 1)`,
    repositoryRoot
  )
  states = new SessionGitStateRepository(database)
})

afterEach(() => database.close())

describe('SessionGitStateRepository', () => {
  it('persists Local repository branch and dirty state by execution context', async () => {
    expect(await states.refresh('local', 10)).toMatchObject({
      executionContextId: 'local', repositoryRoot,
      git: { state: 'ready', branch: 'main', dirty: false }
    })

    await writeFile(join(repositoryRoot, 'dirty.txt'), 'dirty')

    expect(await states.refresh('local', 11)).toMatchObject({
      executionContextId: 'local', repositoryRoot,
      git: { state: 'ready', branch: 'main', dirty: true }, updatedAt: 11
    })
    expect(states.get('local')).toMatchObject({
      git: { state: 'ready', branch: 'main', dirty: true }
    })
  })

  it('persists detached HEAD without manufacturing a branch', async () => {
    await exec('git', ['-C', repositoryRoot, 'checkout', '--detach', 'HEAD'])
    const head = (await exec('git', ['-C', repositoryRoot, 'rev-parse', 'HEAD'])).stdout.trim()

    expect(await states.refresh('local', 20)).toMatchObject({
      git: { state: 'ready', detachedHead: head, dirty: false }
    })
    expect(states.get('local')?.git).not.toHaveProperty('branch')
  })

  it('tracks a registered shared Worktree independently from ownership', async () => {
    const path = join(root, 'shared-worktree')
    await exec('git', ['-C', repositoryRoot, 'worktree', 'add', '-b', 'feature/shared', path])
    database.run(
      `INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
       VALUES ('shared', 'workspace', 'git-worktree', ?, 2)`,
      path
    )
    database.run(
      `INSERT INTO worktrees (
         id, execution_context_id, repository_root, worktree_path, branch_name,
         state, created_at, updated_at
       ) VALUES ('shared-worktree', 'shared', ?, ?, 'feature/shared', 'ready', 2, 2)`,
      repositoryRoot,
      path
    )

    expect(await states.refresh('shared', 21)).toMatchObject({
      executionContextId: 'shared', repositoryRoot,
      git: { state: 'ready', branch: 'feature/shared', dirty: false }
    })
  })

  it('persists unavailable when the registered context path disappears', async () => {
    database.run("UPDATE execution_contexts SET cwd = ? WHERE id = 'local'", join(root, 'missing'))

    expect(await states.refresh('local', 30)).toMatchObject({
      executionContextId: 'local',
      git: { state: 'unavailable', dirty: false },
      error: 'path-missing', updatedAt: 30
    })
  })
})

async function initializeRepository(path: string): Promise<void> {
  await exec('git', ['init', '-b', 'main', path])
  await exec('git', ['-C', path, 'config', 'user.email', 'matou@example.test'])
  await exec('git', ['-C', path, 'config', 'user.name', 'Matou Test'])
  await writeFile(join(path, 'README.md'), 'root\n')
  await exec('git', ['-C', path, 'add', 'README.md'])
  await exec('git', ['-C', path, 'commit', '-m', 'initial'])
}
