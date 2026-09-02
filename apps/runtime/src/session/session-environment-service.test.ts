import { execFile } from 'node:child_process'
import { mkdir, mkdtemp, realpath, rename, rm, symlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { promisify } from 'node:util'

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { RuntimeDatabase } from '../storage/database'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { WorktreeHealthService } from '../worktrees/worktree-health-service'
import { SessionEnvironmentRepository, type OwnedWorktreeIdentity } from './session-environment-repository'
import {
  SessionEnvironmentService,
  type SessionEnvironmentServiceDependencies
} from './session-environment-service'

const exec = promisify(execFile)

let root: string
let repositoryRoot: string
let worktreePath: string
let otherWorktreePath: string
let database: RuntimeDatabase
let environments: SessionEnvironmentRepository

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-environment-service-'))
  repositoryRoot = join(root, 'repository')
  worktreePath = join(root, 'worktrees', 'first')
  otherWorktreePath = join(root, 'worktrees', 'second')
  await initializeRepository(repositoryRoot)
  await mkdir(join(root, 'worktrees'), { recursive: true })
  await exec('git', [
    '-C', repositoryRoot, 'worktree', 'add', '-b', 'feature/first', worktreePath, 'HEAD'
  ])
  await exec('git', [
    '-C', repositoryRoot, 'worktree', 'add', '-b', 'feature/second', otherWorktreePath, 'HEAD'
  ])

  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  environments = new SessionEnvironmentRepository(database)
  database.run(
    `INSERT INTO workspaces (id, name, root_directory, created_at, updated_at)
     VALUES ('workspace', 'Workspace', ?, 1, 1)`,
    repositoryRoot
  )
  database.run(
    `INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
     VALUES ('local', 'workspace', 'plain-directory', ?, 1),
            ('first-context', 'workspace', 'git-worktree', ?, 1),
            ('second-context', 'workspace', 'git-worktree', ?, 1)`,
    repositoryRoot,
    worktreePath,
    otherWorktreePath
  )
  database.run(
    `INSERT INTO worktrees (
       id, execution_context_id, repository_root, worktree_path, branch_name,
       base_ref, state, created_at, updated_at
     ) VALUES
       ('first-worktree', 'first-context', ?, ?, 'feature/first', 'HEAD', 'ready', 1, 1),
       ('second-worktree', 'second-context', ?, ?, 'feature/second', 'HEAD', 'ready', 1, 1)`,
    repositoryRoot,
    worktreePath,
    repositoryRoot,
    otherWorktreePath
  )
  database.run(
    `INSERT INTO tasks (
       id, workspace_id, execution_context_id, title, status, created_at, updated_at
     ) VALUES ('task', 'workspace', 'local', 'Task', 'active', 1, 1)`
  )
  for (const sessionId of ['first', 'second']) {
    database.run(
      `INSERT INTO sessions (
         id, task_id, execution_context_id, kind, status, title, cwd,
         created_at, updated_at, last_activity_at
       ) VALUES (?, 'task', 'local', 'shell', 'running', ?, ?, 1, 1, 1)`,
      sessionId,
      sessionId,
      repositoryRoot
    )
  }
})

afterEach(async () => {
  database.close()
  await rm(root, { recursive: true, force: true })
})

describe('SessionEnvironmentService', () => {
  it('restores the original owned Worktree identity without creating a second binding', async () => {
    environments.bindOwnedWorktree({
      sessionId: 'first', worktreeId: 'first-worktree', activate: true, now: 2
    })
    await exec('git', ['-C', repositoryRoot, 'worktree', 'remove', '--force', worktreePath])
    environments.markMissing('first', 'path-missing', 3)
    const restoredIdentities: OwnedWorktreeIdentity[] = []
    const service = createService({
      async restoreOwnedWorktree(identity) {
        restoredIdentities.push(identity)
        expect(environments.get('first')).toMatchObject({
          activeTarget: 'worktree', state: 'recovering'
        })
        await exec('git', [
          '-C', identity.repositoryRoot, 'worktree', 'add', identity.path, identity.branch
        ])
      }
    })

    await expect(service.restore({ sessionId: 'first', now: 4 })).resolves.toMatchObject({
      kind: 'environment', sessionId: 'first', activeTarget: 'worktree',
      state: 'ready', path: worktreePath, restartRequired: true
    })
    expect(restoredIdentities).toEqual([{
      sessionId: 'first',
      worktreeId: 'first-worktree',
      executionContextId: 'first-context',
      workspaceId: 'workspace',
      repositoryRoot,
      path: worktreePath,
      branch: 'feature/first',
      baseRef: 'HEAD'
    }])
    expect(database.get<{ count: number }>(
      "SELECT COUNT(*) AS count FROM session_environment_bindings WHERE session_id = 'first'"
    )?.count).toBe(1)
  })

  it('rejects an occupied wrong-repository restore path before running setup', async () => {
    environments.bindOwnedWorktree({
      sessionId: 'first', worktreeId: 'first-worktree', activate: true, now: 2
    })
    await exec('git', ['-C', repositoryRoot, 'worktree', 'remove', '--force', worktreePath])
    await initializeRepository(worktreePath)
    environments.markMissing('first', 'path-missing', 3)
    const restoreOwnedWorktree = vi.fn(async () => undefined)

    await expect(createService({ restoreOwnedWorktree }).restore({
      sessionId: 'first', now: 4
    })).resolves.toEqual({
      kind: 'rejected', sessionId: 'first', reason: 'wrong-repository'
    })
    expect(restoreOwnedWorktree).not.toHaveBeenCalled()
    expect(environments.get('first')).toMatchObject({
      activeTarget: 'worktree', state: 'missing',
      environment: { path: worktreePath, error: 'wrong-repository' }
    })
  })

  it('rejects restore when the canonical detached Worktree is owned by another Session', async () => {
    const head = (await exec('git', ['-C', repositoryRoot, 'rev-parse', 'HEAD'])).stdout.trim()
    await exec('git', ['-C', worktreePath, 'checkout', '--detach', head])
    await exec('git', ['-C', otherWorktreePath, 'checkout', '--detach', head])
    database.run(
      "UPDATE worktrees SET branch_name = '(detached)', base_revision = ? WHERE id IN ('first-worktree', 'second-worktree')",
      head
    )
    environments.bindOwnedWorktree({
      sessionId: 'first', worktreeId: 'first-worktree', activate: true, now: 2
    })
    environments.bindOwnedWorktree({
      sessionId: 'second', worktreeId: 'second-worktree', activate: true, now: 2
    })
    await exec('git', ['-C', repositoryRoot, 'worktree', 'remove', '--force', worktreePath])
    await symlink(otherWorktreePath, worktreePath)
    environments.markMissing('first', 'path-missing', 3)
    const pauseSession = vi.fn(async () => undefined)

    await expect(createService({ pauseSession }).restore({
      sessionId: 'first', now: 4
    })).resolves.toEqual({
      kind: 'rejected', sessionId: 'first', reason: 'path-owned-by-another-session'
    })
    expect(pauseSession).not.toHaveBeenCalled()
    expect(environments.get('first')).toMatchObject({
      activeTarget: 'worktree', state: 'missing', managedWorktreeId: 'first-worktree'
    })
  })

  it('locates a moved Worktree, repairs real Git metadata and persists its canonical path', async () => {
    environments.bindOwnedWorktree({
      sessionId: 'first', worktreeId: 'first-worktree', activate: true, now: 2
    })
    const movedPath = join(root, 'relocated', 'first')
    await mkdir(join(root, 'relocated'), { recursive: true })
    await rename(worktreePath, movedPath)
    environments.markMissing('first', 'path-missing', 3)
    const service = createService()

    await expect(service.locate({
      sessionId: 'first', path: movedPath, now: 4
    })).resolves.toMatchObject({
      kind: 'environment', sessionId: 'first', activeTarget: 'worktree',
      state: 'ready', path: await realpath(movedPath), restartRequired: true
    })

    const canonicalPath = await realpath(movedPath)
    expect(environments.getOwnedWorktreeIdentity('first')?.path).toBe(canonicalPath)
    const listed = (await exec('git', [
      '-C', repositoryRoot, 'worktree', 'list', '--porcelain'
    ])).stdout
    expect(listed).toContain(`worktree ${canonicalPath}`)
  })

  it('keeps the environment missing when a located directory is from the wrong repository', async () => {
    environments.bindOwnedWorktree({
      sessionId: 'first', worktreeId: 'first-worktree', activate: true, now: 2
    })
    environments.markMissing('first', 'path-missing', 3)
    const wrongRepository = join(root, 'wrong-repository')
    await initializeRepository(wrongRepository)

    await expect(createService().locate({
      sessionId: 'first', path: wrongRepository, now: 4
    })).resolves.toEqual({ kind: 'rejected', sessionId: 'first', reason: 'wrong-repository' })
    expect(environments.get('first')).toMatchObject({
      activeTarget: 'worktree', state: 'missing',
      environment: { path: worktreePath, error: 'wrong-repository' }
    })
  })

  it('keeps an active Local target ready when locating its owned Worktree fails', async () => {
    environments.bindOwnedWorktree({
      sessionId: 'first', worktreeId: 'first-worktree', activate: false, now: 2
    })
    const wrongRepository = join(root, 'wrong-local-locate')
    await initializeRepository(wrongRepository)

    await expect(createService().locate({
      sessionId: 'first', path: wrongRepository, now: 4
    })).resolves.toEqual({ kind: 'rejected', sessionId: 'first', reason: 'wrong-repository' })
    expect(environments.get('first')).toMatchObject({
      activeTarget: 'local', state: 'ready',
      environment: { kind: 'local', path: repositoryRoot, state: 'ready' }
    })
  })

  it('rolls a failed owned Worktree restore back to an active Local target', async () => {
    environments.bindOwnedWorktree({
      sessionId: 'first', worktreeId: 'first-worktree', activate: false, now: 2
    })
    await exec('git', ['-C', repositoryRoot, 'worktree', 'remove', '--force', worktreePath])
    const service = createService({
      async restoreOwnedWorktree() {
        throw new Error('restore process failed')
      }
    })

    await expect(service.restore({ sessionId: 'first', now: 4 }))
      .rejects.toThrow('restore process failed')
    expect(environments.get('first')).toMatchObject({
      activeTarget: 'local', state: 'ready',
      environment: { kind: 'local', path: repositoryRoot, state: 'ready' }
    })
  })

  it('keeps the environment missing when a located Worktree has the wrong branch', async () => {
    environments.bindOwnedWorktree({
      sessionId: 'first', worktreeId: 'first-worktree', activate: true, now: 2
    })
    environments.markMissing('first', 'path-missing', 3)
    const wrongBranchPath = join(root, 'worktrees', 'wrong-branch')
    await exec('git', [
      '-C', repositoryRoot, 'worktree', 'add', '-b', 'feature/wrong', wrongBranchPath, 'HEAD'
    ])

    await expect(createService().locate({
      sessionId: 'first', path: wrongBranchPath, now: 4
    })).resolves.toEqual({ kind: 'rejected', sessionId: 'first', reason: 'wrong-branch' })
    expect(environments.get('first')).toMatchObject({
      activeTarget: 'worktree', state: 'missing',
      environment: { path: worktreePath, error: 'wrong-branch' }
    })
  })

  it('switches to the owning Session when the user selects another managed Worktree', async () => {
    environments.bindOwnedWorktree({
      sessionId: 'first', worktreeId: 'first-worktree', activate: true, now: 2
    })
    environments.bindOwnedWorktree({
      sessionId: 'second', worktreeId: 'second-worktree', activate: true, now: 2
    })
    environments.markMissing('first', 'path-missing', 3)

    await expect(createService().locate({
      sessionId: 'first', path: otherWorktreePath, now: 4
    })).resolves.toEqual({ kind: 'switch-session', sessionId: 'second' })
    expect(environments.get('first')).toMatchObject({
      activeTarget: 'worktree', state: 'missing', managedWorktreeId: 'first-worktree'
    })
  })

  it('switches to the owner when another managed Worktree moved and its stored path is stale', async () => {
    environments.bindOwnedWorktree({
      sessionId: 'first', worktreeId: 'first-worktree', activate: true, now: 2
    })
    environments.bindOwnedWorktree({
      sessionId: 'second', worktreeId: 'second-worktree', activate: true, now: 2
    })
    const movedSecondPath = join(root, 'relocated-owner', 'second')
    await mkdir(join(root, 'relocated-owner'), { recursive: true })
    await rename(otherWorktreePath, movedSecondPath)
    environments.markMissing('first', 'path-missing', 3)

    await expect(createService().locate({
      sessionId: 'first', path: movedSecondPath, now: 4
    })).resolves.toEqual({ kind: 'switch-session', sessionId: 'second' })
    expect(environments.getOwnedWorktreeIdentity('second')?.path).toBe(otherWorktreePath)
    expect(environments.get('first')).toMatchObject({
      activeTarget: 'worktree', state: 'missing', managedWorktreeId: 'first-worktree'
    })
  })

  it('rolls a failed Handoff back to the original ready target and restarts it', async () => {
    environments.bindOwnedWorktree({
      sessionId: 'first', worktreeId: 'first-worktree', activate: true, now: 2
    })
    const calls: string[] = []
    let failLocalResume = true
    const service = createService({
      async pauseSession(sessionId) {
        calls.push(`pause:${sessionId}`)
      },
      async resumeSession(sessionId, target) {
        calls.push(`resume:${sessionId}:${target}`)
        if (target === 'local' && failLocalResume) {
          failLocalResume = false
          throw new Error('local spawn failed')
        }
      }
    })

    await expect(service.handoff({
      sessionId: 'first', target: 'local', now: 4
    })).rejects.toThrow('local spawn failed')
    expect(environments.get('first')).toMatchObject({
      activeTarget: 'worktree', state: 'ready',
      environment: { kind: 'worktree', path: worktreePath, state: 'ready' }
    })
    expect(database.get(
      "SELECT execution_context_id, cwd FROM sessions WHERE id = 'first'"
    )).toEqual({ execution_context_id: 'first-context', cwd: worktreePath })
    expect(calls).toEqual([
      'pause:first', 'resume:first:local', 'resume:first:worktree'
    ])
  })

  it('does not pause the live run when durable Handoff acceptance fails', async () => {
    environments.bindOwnedWorktree({
      sessionId: 'first', worktreeId: 'first-worktree', activate: true, now: 2
    })
    const pauseSession = vi.fn(async () => undefined)
    vi.spyOn(environments, 'beginTransition').mockImplementationOnce(() => {
      throw new Error('transition write failed')
    })

    await expect(createService({ pauseSession }).handoff({
      sessionId: 'first', target: 'local', now: 4
    })).rejects.toThrow('transition write failed')
    expect(pauseSession).not.toHaveBeenCalled()
    expect(environments.getTransition('first')).toBeUndefined()
    expect(environments.get('first')).toMatchObject({
      activeTarget: 'worktree', state: 'ready',
      environment: { kind: 'worktree', path: worktreePath, state: 'ready' }
    })
  })

  it('rolls a durable Handoff transition back and restores the original target when pause fails', async () => {
    environments.bindOwnedWorktree({
      sessionId: 'first', worktreeId: 'first-worktree', activate: true, now: 2
    })
    const calls: string[] = []
    const service = createService({
      async pauseSession(sessionId) {
        calls.push(`pause:${sessionId}`)
        throw new Error('pause failed after stopping the run')
      },
      async resumeSession(sessionId, target) {
        calls.push(`resume:${sessionId}:${target}`)
      }
    })

    await expect(service.handoff({
      sessionId: 'first', target: 'local', now: 4
    })).rejects.toThrow('pause failed after stopping the run')
    expect(environments.getTransition('first')).toBeUndefined()
    expect(environments.get('first')).toMatchObject({
      activeTarget: 'worktree', state: 'ready',
      environment: { kind: 'worktree', path: worktreePath, state: 'ready' }
    })
    expect(calls).toEqual(['pause:first', 'resume:first:worktree'])
  })

  it('rejects Handoff to a canonical detached Worktree owned by another Session', async () => {
    await configureDetachedCanonicalCollision({ firstActive: false })
    const pauseSession = vi.fn(async () => undefined)
    const resumeSession = vi.fn(async () => undefined)

    await expect(createService({ pauseSession, resumeSession }).handoff({
      sessionId: 'first', target: 'worktree', now: 4
    })).resolves.toEqual({
      kind: 'rejected', sessionId: 'first', reason: 'path-owned-by-another-session'
    })
    expect(pauseSession).not.toHaveBeenCalled()
    expect(resumeSession).not.toHaveBeenCalled()
    expect(environments.getTransition('first')).toBeUndefined()
    expect(environments.get('first')).toMatchObject({
      activeTarget: 'local', state: 'ready', managedWorktreeId: 'first-worktree',
      environment: { kind: 'local', path: repositoryRoot, state: 'ready' }
    })
    expect(database.get(
      "SELECT execution_context_id, cwd FROM sessions WHERE id = 'first'"
    )).toEqual({ execution_context_id: 'local', cwd: repositoryRoot })
  })

  it('rejects a Handoff to a missing Local target and keeps the original Worktree ready', async () => {
    environments.bindOwnedWorktree({
      sessionId: 'first', worktreeId: 'first-worktree', activate: true, now: 2
    })
    database.run(
      "UPDATE execution_contexts SET cwd = ? WHERE id = 'local'",
      join(root, 'missing-local')
    )
    const calls: string[] = []
    const service = createService({
      async pauseSession(sessionId) {
        calls.push(`pause:${sessionId}`)
      },
      async resumeSession(sessionId, target) {
        calls.push(`resume:${sessionId}:${target}`)
      }
    })

    await expect(service.handoff({
      sessionId: 'first', target: 'local', now: 4
    })).resolves.toEqual({ kind: 'rejected', sessionId: 'first', reason: 'path-missing' })
    expect(environments.get('first')).toMatchObject({
      activeTarget: 'worktree', state: 'ready'
    })
    expect(calls).toEqual([])
  })

  it('opens only the canonical path for a ready environment', async () => {
    const pathAlias = join(root, 'worktree-alias')
    await symlink(worktreePath, pathAlias)
    database.run(
      `UPDATE worktrees SET worktree_path = ? WHERE id = 'first-worktree'`,
      pathAlias
    )
    database.run(
      `UPDATE execution_contexts SET cwd = ? WHERE id = 'first-context'`,
      pathAlias
    )
    environments.bindOwnedWorktree({
      sessionId: 'first', worktreeId: 'first-worktree', activate: true, now: 2
    })

    await expect(createService().open('first')).resolves.toEqual({
      sessionId: 'first', kind: 'worktree', path: await realpath(worktreePath)
    })
  })

  it('does not open a stale ready Worktree path occupied by the wrong repository', async () => {
    environments.bindOwnedWorktree({
      sessionId: 'first', worktreeId: 'first-worktree', activate: true, now: 2
    })
    await exec('git', ['-C', repositoryRoot, 'worktree', 'remove', '--force', worktreePath])
    await initializeRepository(worktreePath)

    await expect(createService().open('first')).rejects.toThrow('identity is wrong-repository')
    expect(environments.get('first')).toMatchObject({ activeTarget: 'worktree', state: 'ready' })
  })

  it('continues an interrupted Locate from its persisted candidate path', async () => {
    environments.bindOwnedWorktree({
      sessionId: 'first', worktreeId: 'first-worktree', activate: true, now: 2
    })
    const movedPath = join(root, 'relocated-after-crash', 'first')
    await mkdir(join(root, 'relocated-after-crash'), { recursive: true })
    await rename(worktreePath, movedPath)
    environments.markMissing('first', 'path-missing', 3)
    environments.beginTransition({
      sessionId: 'first', target: 'worktree', state: 'recovering', now: 4,
      operation: { operationId: 'locate-after-crash', kind: 'locate', candidatePath: movedPath }
    })

    await expect(createService().reconcileTransitions(5)).resolves.toEqual({
      checked: 1, completed: 1, rolledBack: 0, failed: 0
    })
    expect(environments.getTransition('first')).toBeUndefined()
    expect(environments.get('first')).toMatchObject({
      activeTarget: 'worktree', state: 'ready',
      environment: { path: await realpath(movedPath) }
    })
  })

  it('does not run restore setup into an occupied wrong repository after a crash', async () => {
    environments.bindOwnedWorktree({
      sessionId: 'first', worktreeId: 'first-worktree', activate: true, now: 2
    })
    await exec('git', ['-C', repositoryRoot, 'worktree', 'remove', '--force', worktreePath])
    await initializeRepository(worktreePath)
    environments.markMissing('first', 'path-missing', 3)
    environments.beginTransition({
      sessionId: 'first', target: 'worktree', state: 'recovering', now: 4,
      operation: { operationId: 'restore-after-crash', kind: 'restore' }
    })
    const restoreOwnedWorktree = vi.fn(async () => undefined)

    await expect(createService({ restoreOwnedWorktree }).reconcileTransitions(5)).resolves.toEqual({
      checked: 1, completed: 0, rolledBack: 1, failed: 0
    })
    expect(restoreOwnedWorktree).not.toHaveBeenCalled()
    expect(environments.get('first')).toMatchObject({
      activeTarget: 'worktree', state: 'missing',
      environment: { path: worktreePath, error: 'wrong-repository' }
    })
  })

  it('rolls back a persisted Restore that resolves to another owner canonical detached Worktree', async () => {
    await configureDetachedCanonicalCollision({ firstActive: true })
    environments.markMissing('first', 'path-missing', 3)
    environments.beginTransition({
      sessionId: 'first', target: 'worktree', state: 'recovering', now: 4,
      operation: { operationId: 'restore-owner-collision', kind: 'restore' }
    })
    const pauseSession = vi.fn(async () => undefined)
    const resumeSession = vi.fn(async () => undefined)

    await expect(createService({ pauseSession, resumeSession }).reconcileTransitions(5))
      .resolves.toEqual({ checked: 1, completed: 0, rolledBack: 1, failed: 0 })
    expect(pauseSession).not.toHaveBeenCalled()
    expect(resumeSession).not.toHaveBeenCalled()
    expect(environments.getTransition('first')).toBeUndefined()
    expect(environments.get('first')).toMatchObject({
      activeTarget: 'worktree', state: 'missing', managedWorktreeId: 'first-worktree',
      environment: { path: worktreePath, error: 'path-owned-by-another-session' }
    })
  })

  it('rolls back a persisted Locate that resolves to another owner canonical detached Worktree', async () => {
    await configureDetachedCanonicalCollision({ firstActive: true })
    environments.markMissing('first', 'path-missing', 3)
    environments.beginTransition({
      sessionId: 'first', target: 'worktree', state: 'recovering', now: 4,
      operation: {
        operationId: 'locate-owner-collision', kind: 'locate',
        candidatePath: otherWorktreePath
      }
    })
    const pauseSession = vi.fn(async () => undefined)
    const resumeSession = vi.fn(async () => undefined)

    await expect(createService({ pauseSession, resumeSession }).reconcileTransitions(5))
      .resolves.toEqual({ checked: 1, completed: 0, rolledBack: 1, failed: 0 })
    expect(pauseSession).not.toHaveBeenCalled()
    expect(resumeSession).not.toHaveBeenCalled()
    expect(environments.getTransition('first')).toBeUndefined()
    expect(environments.getOwnedWorktreeIdentity('first')?.path).toBe(worktreePath)
    expect(environments.get('first')).toMatchObject({
      activeTarget: 'worktree', state: 'missing', managedWorktreeId: 'first-worktree',
      environment: { path: worktreePath, error: 'path-owned-by-another-session' }
    })
  })

  it('rolls an interrupted Handoff back to its persisted previous target', async () => {
    environments.bindOwnedWorktree({
      sessionId: 'first', worktreeId: 'first-worktree', activate: true, now: 2
    })
    environments.beginTransition({
      sessionId: 'first', target: 'local', state: 'handoff', now: 4,
      operation: { operationId: 'handoff-after-crash', kind: 'handoff' }
    })

    await expect(createService().reconcileTransitions(5)).resolves.toEqual({
      checked: 1, completed: 0, rolledBack: 1, failed: 0
    })
    expect(environments.getTransition('first')).toBeUndefined()
    expect(environments.get('first')).toMatchObject({
      activeTarget: 'worktree', state: 'ready',
      environment: { path: worktreePath }
    })
  })
})

function createService(
  overrides: Partial<SessionEnvironmentServiceDependencies> = {}
): SessionEnvironmentService {
  return new SessionEnvironmentService(environments, {
    restoreOwnedWorktree: async () => undefined,
    pauseSession: async () => undefined,
    resumeSession: async () => undefined,
    ...overrides
  }, new WorktreeHealthService())
}

async function initializeRepository(path: string): Promise<void> {
  await exec('git', ['init', path])
  await exec('git', ['-C', path, 'config', 'user.email', 'matou@example.test'])
  await exec('git', ['-C', path, 'config', 'user.name', 'Matou Test'])
  await writeFile(join(path, 'README.md'), 'root\n')
  await exec('git', ['-C', path, 'add', 'README.md'])
  await exec('git', ['-C', path, 'commit', '-m', 'initial'])
}

async function configureDetachedCanonicalCollision(input: { firstActive: boolean }): Promise<void> {
  const head = (await exec('git', ['-C', repositoryRoot, 'rev-parse', 'HEAD'])).stdout.trim()
  await exec('git', ['-C', worktreePath, 'checkout', '--detach', head])
  await exec('git', ['-C', otherWorktreePath, 'checkout', '--detach', head])
  database.run(
    "UPDATE worktrees SET branch_name = '(detached)', base_revision = ? WHERE id IN ('first-worktree', 'second-worktree')",
    head
  )
  environments.bindOwnedWorktree({
    sessionId: 'first', worktreeId: 'first-worktree', activate: input.firstActive, now: 2
  })
  environments.bindOwnedWorktree({
    sessionId: 'second', worktreeId: 'second-worktree', activate: true, now: 2
  })
  await exec('git', ['-C', repositoryRoot, 'worktree', 'remove', '--force', worktreePath])
  await symlink(otherWorktreePath, worktreePath)
}
