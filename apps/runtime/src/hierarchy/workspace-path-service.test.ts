import { chmod, mkdtemp, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { WorkspacePathService } from './workspace-path-service'
import { RuntimeDatabase } from '../storage/database'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'

let database: RuntimeDatabase
let service: WorkspacePathService
let root: string

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-path-state-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  service = new WorkspacePathService(database, new DomainTransactionManager(database))
})

afterEach(() => {
  service.stopPolling()
  database.close()
})

describe('WorkspacePathService', () => {
  it('shares one real path inspection and persistence pass across concurrent validation requests', async () => {
    const baselineDirectory = await mkdtemp(join(root, 'baseline-'))
    const concurrentDirectory = await mkdtemp(join(root, 'concurrent-'))
    seedWorkspace('workspace-baseline', baselineDirectory)
    seedWorkspace('workspace-concurrent', concurrentDirectory)

    database.readStatementCount(true)
    await service.validateWorkspace('workspace-baseline')
    const singleValidationStatements = database.readStatementCount(true)

    const [first, second] = await Promise.all([
      service.validateWorkspace('workspace-concurrent'),
      service.validateWorkspace('workspace-concurrent')
    ])

    expect(first).toEqual(second)
    expect(database.readStatementCount()).toBe(singleValidationStatements)
  })

  it('derives missing and not-directory states without changing Workspace ownership', async () => {
    const missingPath = join(root, 'missing')
    const filePath = join(root, 'file.txt')
    await writeFile(filePath, 'file')
    seedWorkspace('workspace-missing', missingPath)
    seedWorkspace('workspace-file', filePath)

    await expect(service.validateWorkspace('workspace-missing')).resolves.toMatchObject({
      status: 'invalid', reason: 'missing'
    })
    await expect(service.validateWorkspace('workspace-file')).resolves.toMatchObject({
      status: 'invalid', reason: 'not-directory'
    })
    expect(readWorkspaceRoot('workspace-missing')).toBe(missingPath)
    expect(readWorkspaceRoot('workspace-file')).toBe(filePath)
  })

  it('derives no-access and then publishes recovery when access returns', async () => {
    const directory = await mkdtemp(join(root, 'restricted-'))
    seedWorkspace('workspace-1', directory)
    await chmod(directory, 0o000)
    try {
      await expect(service.validateWorkspace('workspace-1')).resolves.toMatchObject({
        status: 'invalid', reason: 'no-access'
      })
    } finally {
      await chmod(directory, 0o700)
    }

    await expect(service.validateWorkspace('workspace-1')).resolves.toMatchObject({
      status: 'valid', reason: ''
    })
    expect(database.all<{ event_type: string }>(
      `SELECT event_type FROM domain_events
       WHERE event_type = 'workspace.path-status-changed' ORDER BY seq`
    )).toHaveLength(2)
  })

  it('rejects execution with the fixed product message while retaining stored hierarchy', async () => {
    seedWorkspace('workspace-1', join(root, 'gone'))

    await expect(service.validateBeforeExecution('workspace-1')).rejects.toMatchObject({
      code: 'WORKSPACE_PATH_INVALID',
      message: '工作区目录不可用，请先在本地恢复原路径，或移出该工作区'
    })
    expect(database.get('SELECT id FROM workspaces WHERE id = ?', 'workspace-1')).toEqual({
      id: 'workspace-1'
    })
  })
})

function seedWorkspace(id: string, rootDirectory: string): void {
  database.run(
    `INSERT INTO workspaces (
       id, name, root_directory, path_identity, task_order_json,
       created_at, updated_at, version
     ) VALUES (?, ?, ?, ?, '[]', 1, 1, 1)`,
    id,
    id,
    rootDirectory,
    `path:${rootDirectory}`
  )
}

function readWorkspaceRoot(id: string): string | undefined {
  return database.get<{ root_directory: string }>(
    'SELECT root_directory FROM workspaces WHERE id = ?', id
  )?.root_directory
}
