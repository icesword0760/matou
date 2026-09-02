import { mkdir, mkdtemp, realpath, writeFile } from 'node:fs/promises'
import { execFileSync } from 'node:child_process'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { RuntimeRpcRouter } from './runtime-rpc-router'
import { NotificationProjection } from '../product/experience-foundation'
import { RuntimeDatabase } from '../storage/database'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { encodeClaudeProjectPath } from '../session/claude-session-catalog'
import { RuntimeAccessPolicy } from '../storage/runtime-access-policy'

let database: RuntimeDatabase
let router: RuntimeRpcRouter
let testRoot: string

beforeEach(async () => {
  testRoot = await mkdtemp(join(tmpdir(), 'matou-rpc-'))
  database = RuntimeDatabase.open(join(testRoot, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  router = new RuntimeRpcRouter(database)
})

afterEach(() => database.close())

describe('RuntimeRpcRouter', () => {
  it('persists and activates provider configurations through explicit RPC methods', async () => {
    const created = await router.handle('provider-config.upsert', {
      provider: {
        cli: 'codex', name: 'Team gateway', endpoint: 'https://gateway.example/v1',
        model: 'gpt-team', apiKey: 'TOKEN'
      }
    }) as { provider: { id: string } }
    const activated = await router.handle('provider-config.activate', {
      cli: 'codex', providerId: created.provider.id
    }) as { activeProviderIds: { codex: string } }

    expect(activated.activeProviderIds.codex).toBe(created.provider.id)
    const snapshot = await router.handle('provider-config.snapshot', {}) as {
      providers: { codex: Array<{ name: string; hasApiKey: boolean }> }
    }
    expect(snapshot.providers.codex).toContainEqual(expect.objectContaining({
      name: 'Team gateway', hasApiKey: true
    }))
    expect(JSON.stringify(snapshot)).not.toContain('TOKEN')
  })

  it('accepts event acknowledgements without persisting a cursor in read-only recovery mode', async () => {
    router = new RuntimeRpcRouter(database, new NotificationProjection(), {
      accessPolicy: new RuntimeAccessPolicy('read-only')
    })

    await expect(router.handle('events.ack', {
      consumerId: 'read-only-renderer', throughSequence: 7
    })).resolves.toEqual({ acknowledged: true })
    expect(database.get(
      'SELECT last_event_seq FROM consumer_cursors WHERE consumer_id = ?',
      'read-only-renderer'
    )).toBeUndefined()
  })

  it('lists workspace Claude history and loads the selected permission into the same Session', async () => {
    const workspaceRoot = join(testRoot, 'load-workspace')
    const projectsRoot = join(testRoot, 'claude-projects')
    await mkdir(workspaceRoot)
    const projectDirectory = join(projectsRoot, encodeClaudeProjectPath(workspaceRoot))
    await mkdir(projectDirectory, { recursive: true })
    await writeFile(join(projectDirectory, 'provider-load.jsonl'), [
      JSON.stringify({
        type: 'user', sessionId: 'provider-load', cwd: workspaceRoot,
        timestamp: '2026-08-31T10:00:00.000Z', permissionMode: 'default',
        message: { role: 'user', content: '载入通知中心会话' }
      }),
      JSON.stringify({
        type: 'permission-mode', sessionId: 'provider-load', cwd: workspaceRoot,
        timestamp: '2026-08-31T10:01:00.000Z', permissionMode: 'bypassPermissions'
      })
    ].join('\n'))
    router = new RuntimeRpcRouter(database, new NotificationProjection(), { projectsRoot })
    const initial = await router.handle('hierarchy.bootstrap-window', payload('load-bootstrap', {
      windowId: 'window-load', defaultRootDirectory: workspaceRoot,
      defaultName: 'load-workspace', now: 1
    })) as { session: { id: string }; scene: { id: string } }

    const list = await router.handle('claude-sessions.list', {
      sessionId: initial.session.id, query: '通知中心'
    }) as { sessions: Array<{ providerSessionId: string; permissionMode: string }> }
    expect(list.sessions).toEqual([expect.objectContaining({
      providerSessionId: 'provider-load', permissionMode: 'bypassPermissions'
    })])

    await router.handle('claude-sessions.load', payload('load-existing', {
      sessionId: initial.session.id,
      providerSessionId: 'provider-load',
      permissionMode: 'default',
      now: 2
    }))

    expect(database.get(
      `SELECT sessions.id, sessions.kind, binding.provider_session_id, binding.metadata_json
       FROM sessions JOIN provider_bindings AS binding ON binding.session_id = sessions.id
       WHERE sessions.id = ?`, initial.session.id
    )).toMatchObject({
      id: initial.session.id,
      kind: 'claude-code',
      provider_session_id: 'provider-load',
      metadata_json: expect.stringContaining('bypassPermissions')
    })
  })

  it('lists Claude history already loaded by another card instead of reporting an empty workspace', async () => {
    const workspaceRoot = join(testRoot, 'occupied-workspace')
    const projectsRoot = join(testRoot, 'claude-projects')
    await mkdir(workspaceRoot)
    const projectDirectory = join(projectsRoot, encodeClaudeProjectPath(workspaceRoot))
    await mkdir(projectDirectory, { recursive: true })
    await writeFile(join(projectDirectory, 'provider-occupied.jsonl'), JSON.stringify({
      type: 'user', sessionId: 'provider-occupied', cwd: workspaceRoot,
      timestamp: '2026-08-31T10:00:00.000Z', permissionMode: 'default',
      message: { role: 'user', content: '继续已载入的会话' }
    }))
    router = new RuntimeRpcRouter(database, new NotificationProjection(), { projectsRoot })
    const initial = await router.handle('hierarchy.bootstrap-window', payload('occupied-bootstrap', {
      windowId: 'window-occupied', defaultRootDirectory: workspaceRoot,
      defaultName: 'occupied-workspace', now: 1
    })) as {
      session: { id: string; taskId: string; executionContextId: string }
      scene: { id: string; rootNodeId: string }
    }
    await router.handle('claude-sessions.load', payload('occupy-provider', {
      sessionId: initial.session.id, providerSessionId: 'provider-occupied', now: 2
    }))
    await router.handle('session.create', payload('loader-target', {
      id: 'loader-target', taskId: initial.session.taskId,
      executionContextId: initial.session.executionContextId,
      kind: 'shell', title: '目标 Shell', now: 3
    }))
    await router.handle('scene.mount-session', payload('loader-target-mount', {
      id: 'loader-target-mount', sceneId: initial.scene.id,
      sceneNodeId: initial.scene.rootNodeId, sessionId: 'loader-target', now: 3
    }))

    const list = await router.handle('claude-sessions.list', {
      sessionId: 'loader-target', query: ''
    }) as {
      sessions: Array<{
        providerSessionId: string
        availability: string
        loadedSessionTitle?: string
      }>
    }

    expect(list.sessions).toEqual([expect.objectContaining({
      providerSessionId: 'provider-occupied',
      availability: 'loaded-elsewhere',
      loadedSessionTitle: '继续已载入的会话'
    })])
    await expect(router.handle('claude-sessions.detail', {
      sessionId: 'loader-target', providerSessionId: 'provider-occupied', query: ''
    })).resolves.toMatchObject({ providerSessionId: 'provider-occupied' })
    await expect(router.handle('claude-sessions.load', payload('duplicate-load', {
      sessionId: 'loader-target', providerSessionId: 'provider-occupied', now: 4
    }))).resolves.toMatchObject({
      load: { sessionId: 'loader-target', providerSessionId: 'provider-occupied' }
    })
    expect(database.get<{ total: number }>(
      `SELECT COUNT(*) AS total FROM provider_bindings
       WHERE provider = 'claude-code' AND provider_session_id = 'provider-occupied'`
    )?.total).toBe(2)
  })

  it('routes Git status and branch mutations for the active repository', async () => {
    const repositoryRoot = join(testRoot, 'repository')
    await mkdir(repositoryRoot)
    runGit(repositoryRoot, 'init', '-b', 'main')
    runGit(repositoryRoot, 'config', 'user.name', 'Matou Test')
    runGit(repositoryRoot, 'config', 'user.email', 'matou@example.test')
    execFileSync('sh', ['-c', 'printf baseline > README.md'], { cwd: repositoryRoot })
    runGit(repositoryRoot, 'add', 'README.md')
    runGit(repositoryRoot, 'commit', '-m', 'baseline')

    const status = await router.handle('git.status', payload('git-status', {
      cwd: repositoryRoot, now: 1
    })) as { currentBranch: string }
    expect(status.currentBranch).toBe('main')

    const created = await router.handle('git.create-branch', payload('git-branch', {
      cwd: repositoryRoot, branch: 'feature/rpc', now: 2
    })) as { currentBranch: string }
    expect(created.currentBranch).toBe('feature/rpc')
  })

  it('opens a selected Worktree as a focused Shell in the current canvas', async () => {
    const repositoryRoot = join(testRoot, 'worktree-repository')
    const worktreePath = join(testRoot, 'external-worktree')
    await mkdir(repositoryRoot)
    runGit(repositoryRoot, 'init', '-b', 'main')
    runGit(repositoryRoot, 'config', 'user.name', 'Matou Test')
    runGit(repositoryRoot, 'config', 'user.email', 'matou@example.test')
    execFileSync('sh', ['-c', 'printf baseline > README.md'], { cwd: repositoryRoot })
    runGit(repositoryRoot, 'add', 'README.md')
    runGit(repositoryRoot, 'commit', '-m', 'baseline')
    runGit(repositoryRoot, 'worktree', 'add', '-b', 'feature/open', worktreePath, 'main')
    const initial = await router.handle('hierarchy.bootstrap-window', payload('worktree-bootstrap', {
      windowId: 'window-worktree', defaultRootDirectory: repositoryRoot,
      defaultName: 'worktree-repository', now: 1
    })) as { session: { id: string }; scene: { id: string } }

    const opened = await router.handle('git.worktree-open', payload('worktree-open', {
      cwd: repositoryRoot, sessionId: initial.session.id,
      windowId: 'window-worktree', sceneId: initial.scene.id,
      repositoryRoot, path: worktreePath, branch: 'feature/open', now: 2
    })) as {
      created: boolean
      focusedSessionId: string
      session: { cwd: string }
      graph: { nodes: Array<Record<string, unknown>> }
    }
    const actualWorktreePath = await realpath(worktreePath)

    expect(opened).toMatchObject({ created: true, session: { cwd: actualWorktreePath } })
    expect(opened.focusedSessionId).toBeTruthy()
    expect(database.get(
      `SELECT execution_contexts.kind, execution_contexts.cwd FROM sessions
       JOIN execution_contexts ON execution_contexts.id = sessions.execution_context_id
       WHERE sessions.id = ?`, opened.focusedSessionId
    )).toEqual({ kind: 'git-worktree', cwd: actualWorktreePath })
    expect(opened.graph.nodes.find(({ sessionId }) => sessionId === opened.focusedSessionId))
      .toMatchObject({
        environment: { kind: 'local', state: 'ready', path: actualWorktreePath },
        git: { state: 'ready', branch: 'feature/open', dirty: false }
      })
  })

  it('routes atomic Workspace hierarchy workflows', async () => {
    const bootstrapped = await router.handle('hierarchy.bootstrap-window', payload('bootstrap', {
      windowId: 'window-1',
      defaultRootDirectory: '/tmp/matou_workspace',
      defaultName: 'matou_workspace',
      now: 1
    })) as { workspace: { id: string }; navigation: { activeWorkspaceId: string } }
    expect(bootstrapped.navigation.activeWorkspaceId).toBe(bootstrapped.workspace.id)

    const custom = await router.handle('hierarchy.create-workspace', payload('create-workspace', {
      windowId: 'window-1', rootDirectory: '/tmp/matou-rpc-custom', name: 'Custom', now: 2
    })) as { workspace: { id: string } }
    await expect(router.handle('hierarchy.rename-workspace', payload('rename-workspace', {
      workspaceId: custom.workspace.id,
      name: 'Renamed',
      now: 3
    }))).rejects.toThrow('工作空间名称跟随目录名称')

    const activated = await router.handle('hierarchy.activate-workspace', payload('activate-workspace', {
      windowId: 'window-2',
      workspaceId: custom.workspace.id,
      now: 4
    })) as { navigation: { windowId: string; activeWorkspaceId: string } }
    expect(activated.navigation).toMatchObject({
      windowId: 'window-2',
      activeWorkspaceId: custom.workspace.id
    })

    database.run(
      `INSERT INTO workspace_path_state (
         workspace_id, status, reason, checked_at, validation_generation
       ) VALUES (?, 'valid', '', 4, 2)
       ON CONFLICT(workspace_id) DO UPDATE SET
         status = 'valid', reason = '', validation_generation = 2`,
      custom.workspace.id
    )
    const createdTask = await router.handle('hierarchy.create-task', payload('create-task', {
      windowId: 'window-2', workspaceId: custom.workspace.id, now: 5
    })) as { task: { id: string; title: string } }
    expect(createdTask.task.title).toBe('新事项')
    await router.handle('hierarchy.rename-task', payload('rename-task', {
      taskId: createdTask.task.id, title: '实施事项', now: 5
    }))
    const taskActivated = await router.handle(
      'hierarchy.activate-task',
      payload('activate-task', {
        windowId: 'window-1', taskId: createdTask.task.id, now: 6
      })
    ) as { task: { id: string } }
    expect(taskActivated.task.id).toBe(createdTask.task.id)
  })

  it('keeps the final canvas as history when a detached native window closes', async () => {
    const initial = await router.handle('hierarchy.bootstrap-window', payload('detached-bootstrap', {
      windowId: 'window-1', defaultRootDirectory: '/tmp/detached-workspace',
      defaultName: 'detached-workspace', now: 1
    })) as { session: { id: string }; scene: { id: string } }

    const result = await router.handle('hierarchy.delete-session', payload('detached-close', {
      windowId: 'window-1', sessionId: initial.session.id,
      confirmedIntent: `delete-session:${initial.session.id}`,
      preserveSceneOnLastSession: true, now: 2
    })) as { outcome: string }

    expect(result.outcome).toBe('session-stopped-remains')
    expect(database.get('SELECT archived_at FROM scenes WHERE id = ?', initial.scene.id))
      .toEqual({ archived_at: null })
  })

  it('creates, modifies, archives, and projects the authoritative hierarchy', async () => {
    await router.handle('workspace.create', payload('workspace', {
      id: 'workspace-1', name: 'Workspace', rootDirectory: '/tmp/workspace', now: 1
    }))
    await router.handle('workspace.update', payload('workspace-update', {
      id: 'workspace-1', name: 'Renamed', now: 2
    }))
    await router.handle('execution-context.create-plain', payload('context', {
      id: 'context-1', workspaceId: 'workspace-1', cwd: '/tmp/workspace', now: 2
    }))
    await router.handle('task.create', payload('task', {
      id: 'task-1', workspaceId: 'workspace-1', executionContextId: 'context-1',
      title: 'Task', status: 'active', sortKey: 'a', now: 3
    }))
    await router.handle('task.update', payload('task-update', {
      id: 'task-1', title: 'Updated Task', status: 'completed', now: 4
    }))
    await router.handle('session.create', payload('session', {
      id: 'session-1', taskId: 'task-1', executionContextId: 'context-1',
      kind: 'shell', title: 'Shell', now: 5
    }))
    await router.handle('session.update', payload('session-update', {
      id: 'session-1', title: 'Renamed Shell', status: 'waiting', now: 6
    }))
    await router.handle('scene.create', payload('scene', {
      id: 'scene-1', rootNodeId: 'root-1', taskId: 'task-1', name: 'Main', mode: 'tile', now: 7
    }))
    await router.handle('scene.archive', payload('scene-archive', { id: 'scene-1', now: 8 }))
    await router.handle('session.archive', payload('session-archive', { id: 'session-1', now: 9 }))
    await router.handle('task.archive', payload('task-archive', { id: 'task-1', now: 10 }))
    await router.handle('workspace.archive', payload('workspace-archive', { id: 'workspace-1', now: 11 }))

    const snapshot = await router.handle('projection.snapshot', {}) as {
      runtimeGeneration: string
      eventSequence: number
      workspaces: Array<{ id: string; name: string; archivedAt?: number }>
      tasks: Array<{ id: string; title: string; status: string }>
      sessions: Array<{ id: string; title: string; status: string }>
      scenes: Array<{ id: string; archivedAt?: number }>
    }
    expect(snapshot.runtimeGeneration).toBe(database.runtimeGeneration)
    expect(snapshot.eventSequence).toBeGreaterThan(0)
    expect(snapshot.workspaces[0]).toMatchObject({ id: 'workspace-1', name: 'Renamed', archivedAt: 11 })
    expect(snapshot.tasks[0]).toMatchObject({ id: 'task-1', title: 'Updated Task', status: 'archived' })
    expect(snapshot.sessions[0]).toMatchObject({ id: 'session-1', title: 'Renamed Shell', status: 'archived' })
    expect(snapshot.scenes[0]).toMatchObject({ id: 'scene-1', archivedAt: 8 })
  })

  it('persists a permission switch against the same resumable AI conversation', async () => {
    await seedHierarchy()
    await router.handle('session.create', payload('session', {
      id: 'session-1', taskId: 'task-1', executionContextId: 'context-1',
      kind: 'claude-code', title: 'Claude', now: 4
    }))
    database.run(
      `INSERT INTO provider_bindings (
         id, session_id, provider, provider_session_id, resume_state,
         metadata_json, created_at, updated_at, validated_at
       ) VALUES (?, ?, 'claude-code', ?, 'available', ?, 5, 5, 5)`,
      'binding-1', 'session-1', 'claude-session-1',
      JSON.stringify({ permissionMode: 'bypassPermissions', cwd: '/tmp/workspace' })
    )

    const changed = await router.handle('session.set-permission-mode', payload('permission-plan', {
      sessionId: 'session-1', provider: 'claude-code', permissionMode: 'plan', now: 6
    })) as { result: { providerSessionId: string; validatedAt: number; metadata: unknown } }

    expect(changed.result).toMatchObject({
      providerSessionId: 'claude-session-1', validatedAt: 5,
      metadata: { permissionMode: 'plan', cwd: '/tmp/workspace' }
    })
    await expect(router.handle('session.set-permission-mode', payload('permission-invalid', {
      sessionId: 'session-1', provider: 'claude-code', permissionMode: 'owner', now: 7
    }))).rejects.toMatchObject({ code: 'INVALID_REQUEST' })
  })

  it('restores hundreds of Sessions with a bounded bulk-query count', async () => {
    const workspaceRoot = join(testRoot, 'bulk-snapshot-workspace')
    await mkdir(workspaceRoot)
    const initial = await router.handle('hierarchy.bootstrap-window', payload('bulk-bootstrap', {
      windowId: 'window-bulk', defaultRootDirectory: workspaceRoot,
      defaultName: 'bulk-workspace', now: 1
    })) as { session: { id: string } }
    database.run(
      `WITH RECURSIVE sequence(value) AS (
         SELECT 1 UNION ALL SELECT value + 1 FROM sequence WHERE value < 500
       )
       INSERT INTO sessions (
         id, task_id, execution_context_id, kind, status, created_at, updated_at,
         last_activity_at, archived_at, title, version, cwd, work_status
       )
       SELECT printf('bulk-session-%04d', value), task_id, execution_context_id,
              'shell', 'created', value + 10, value + 10, value + 10,
              NULL, printf('Bulk Session %d', value), 1, cwd, 'idle'
       FROM sessions, sequence WHERE sessions.id = ?`,
      initial.session.id
    )

    database.readStatementCount(true)
    const snapshot = await router.handle('projection.snapshot', {}) as {
      sessions: Array<{ id: string }>
    }
    const statementCount = database.readStatementCount()

    expect(snapshot.sessions).toHaveLength(501)
    expect(statementCount).toBeLessThan(40)
  })

  it('supports relation and Scene structural commands with synchronous event replay', async () => {
    await seedHierarchy()
    await router.handle('session.create', payload('parent', {
      id: 'parent', taskId: 'task-1', executionContextId: 'context-1', kind: 'shell', title: 'Parent', now: 4
    }))
    await router.handle('session.create', payload('child', {
      id: 'child', taskId: 'task-1', executionContextId: 'context-1', kind: 'shell', title: 'Child', now: 4
    }))
    await router.handle('scene.create', payload('scene', {
      id: 'scene-1', rootNodeId: 'root', taskId: 'task-1', name: 'Main', mode: 'tile', now: 5
    }))
    await router.handle('scene.mount-session', payload('mount-parent', {
      id: 'mount-parent', sceneId: 'scene-1', sceneNodeId: 'root', sessionId: 'parent', now: 6
    }))
    await router.handle('scene.mount-session', payload('mount', {
      id: 'mount-1', sceneId: 'scene-1', sceneNodeId: 'root', sessionId: 'child', now: 6
    }))
    await router.handle('relation.create', payload('relation', {
      id: 'relation-1', taskId: 'task-1', fromSessionId: 'child', toSessionId: 'parent',
      kind: 'forked-from', metadata: {}, now: 7
    }))
    await router.handle('geometry.put', {
      sceneId: 'scene-1', ownerKey: 'node:root', layoutRevision: 0,
      geometry: { ratio: 0.35 }, now: 7
    })

    expect(await router.handle('geometry.list', { sceneId: 'scene-1' })).toEqual([
      expect.objectContaining({ ownerKey: 'node:root', geometry: { ratio: 0.35 } })
    ])

    const snapshot = await router.handle('projection.snapshot', {}) as {
      sceneSnapshots: Array<{ scene: { id: string }; geometry: Array<{ geometry: { ratio: number } }> }>
    }
    expect(snapshot.sceneSnapshots.find(({ scene }) => scene.id === 'scene-1')?.geometry)
      .toEqual([expect.objectContaining({ geometry: { ratio: 0.35 } })])

    const sceneSnapshot = await router.handle('hierarchy.get-scene-snapshot', {
      sceneId: 'scene-1'
    }) as {
      scene: { id: string }
      nodes: Array<{ id: string }>
      mounts: Array<{ id: string }>
      geometry: Array<{ geometry: { ratio: number } }>
    }
    expect(sceneSnapshot).toMatchObject({
      scene: { id: 'scene-1' },
      nodes: expect.arrayContaining([expect.objectContaining({ id: 'root' })]),
      mounts: expect.arrayContaining([expect.objectContaining({ id: 'mount-parent' })]),
      geometry: [expect.objectContaining({ geometry: { ratio: 0.35 } })]
    })

    const graph = await router.handle('hierarchy.get-scene-session-graph', {
      sceneId: 'scene-1', windowId: 'window-1'
    }) as {
      runtimeGeneration: string
      eventSequence: number
      nodes: Array<{ sessionId: string; parentSessionId?: string }>
    }
    expect(graph.runtimeGeneration).toBe(database.runtimeGeneration)
    expect(graph.eventSequence).toBeGreaterThan(0)
    expect(graph.nodes).toEqual(expect.arrayContaining([
      expect.objectContaining({ sessionId: 'parent' }),
      expect.objectContaining({ sessionId: 'child', parentSessionId: 'parent' })
    ]))

    const replay = await router.handle('events.replay', { afterSequence: 0, limit: 100 }) as {
      events: Array<{ eventType: string }>
    }
    expect(replay.events.some(({ eventType }) => eventType === 'session-relation.created')).toBe(true)
    expect(replay.events.some(({ eventType }) => eventType === 'scene.session-mounted')).toBe(true)
    await router.handle('events.ack', { consumerId: 'renderer-1', throughSequence: replay.events.length })
    expect(database.get('SELECT last_event_seq FROM consumer_cursors WHERE consumer_id = ?', 'renderer-1')).toEqual({
      last_event_seq: replay.events.length
    })
  })

  it('routes canvas creation, Shell sibling creation, and focus without changing graph relations', async () => {
    const workspaceRoot = join(testRoot, 'canvas-workspace')
    await mkdir(workspaceRoot)
    const bootstrapped = await router.handle('hierarchy.bootstrap-window', payload('canvas-bootstrap', {
      windowId: 'window-canvas', defaultRootDirectory: workspaceRoot,
      defaultName: 'workspace', now: 2
    })) as { task: { id: string }; scene: { id: string }; session: { id: string } }

    const canvas = await router.handle('hierarchy.create-canvas', payload('canvas-create', {
      windowId: 'window-canvas', taskId: bootstrapped.task.id, now: 3
    })) as { scene: { id: string }; session: { id: string }; graph: { nodes: unknown[] } }
    const sibling = await router.handle('hierarchy.create-shell-sibling', payload('canvas-sibling', {
      windowId: 'window-canvas', sceneId: canvas.scene.id,
      sourceSessionId: canvas.session.id, now: 4
    })) as { session: { id: string }; graph: { nodes: unknown[]; edges: unknown[]; focusedSessionId?: string } }

    expect(canvas.graph.nodes).toHaveLength(1)
    expect(sibling.graph).toMatchObject({
      nodes: expect.arrayContaining([
        expect.objectContaining({ sessionId: canvas.session.id, currentMode: 'shell' }),
        expect.objectContaining({ sessionId: sibling.session.id, currentMode: 'shell' })
      ]),
      edges: [],
      focusedSessionId: sibling.session.id
    })

    const focused = await router.handle('hierarchy.set-focused-session', payload('canvas-focus', {
      windowId: 'window-canvas', sceneId: canvas.scene.id,
      sessionId: canvas.session.id, now: 5
    })) as { focusedSessionId?: string }
    expect(focused.focusedSessionId).toBe(canvas.session.id)
  })

  it('routes named Claude Fork child and sibling workflows through the canvas graph', async () => {
    const workspaceRoot = join(testRoot, 'fork-workspace')
    await mkdir(workspaceRoot)
    const initial = await router.handle('hierarchy.bootstrap-window', payload('fork-bootstrap', {
      windowId: 'window-fork', defaultRootDirectory: workspaceRoot,
      defaultName: 'workspace', now: 2
    })) as { scene: { id: string }; session: { id: string } }
    database.run(
      "UPDATE sessions SET kind = 'claude-code', title = 'Claude' WHERE id = ?",
      initial.session.id
    )
    database.run(
      `INSERT INTO provider_bindings (
         id, session_id, provider, provider_session_id, resume_state, restore_state,
         metadata_json, created_at, updated_at, validated_at
       ) VALUES ('binding-parent', ?, 'claude-code', 'provider-parent', 'available', 'none',
                 '{"canFork":true}', 3, 3, 3)`,
      initial.session.id
    )

    const child = await router.handle('hierarchy.create-fork-child', payload('fork-child', {
      windowId: 'window-fork', sceneId: initial.scene.id,
      sourceSessionId: initial.session.id, name: '第一分支', worktreeMode: 'current', now: 4,
      submissionKey: 'router-child-submission'
    })) as { session: { id: string; title: string }; graph: { nodes: Array<{ title: string }> } }
    const duplicate = await router.handle('hierarchy.create-fork-child', payload('fork-child-retry', {
      windowId: 'window-fork', sceneId: initial.scene.id,
      sourceSessionId: initial.session.id, name: '超时重发', worktreeMode: 'current', now: 5,
      submissionKey: 'router-child-submission'
    })) as { session: { id: string } }
    const sibling = await router.handle('hierarchy.create-fork-sibling', payload('fork-sibling', {
      windowId: 'window-fork', sceneId: initial.scene.id,
      sourceSessionId: child.session.id, name: '第二分支', worktreeMode: 'current', now: 6,
      submissionKey: 'router-sibling-submission'
    })) as { session: { id: string }; graph: { nodes: Array<{ title: string }> } }

    expect(child.session.title).toBe('第一分支')
    expect(duplicate.session.id).toBe(child.session.id)
    expect(sibling.graph.nodes.map(({ title }) => title)).toEqual(['Claude', '第一分支', '第二分支'])
    expect(database.get(
      'SELECT source_session_id FROM session_fork_intents WHERE session_id = ?', sibling.session.id
    )).toEqual({ source_session_id: initial.session.id })
  })

  it('rejects malformed payloads before reaching repositories', async () => {
    await expect(router.handle('workspace.create', { command: {}, input: { name: '' } })).rejects.toMatchObject({
      code: 'INVALID_REQUEST'
    })
    await expect(router.handle('hierarchy.create-fork-child', payload('missing-submission', {
      windowId: 'window-1', sceneId: 'scene-1', sourceSessionId: 'session-1',
      name: 'Fork', worktreeMode: 'current', now: 1
    }))).rejects.toMatchObject({ code: 'INVALID_REQUEST' })
  })

  it('projects Task unread counts and clears only the newly visible focused panel', async () => {
    const notifications = new NotificationProjection({ cooldownMs: 0 })
    const notificationRouter = new RuntimeRpcRouter(database, notifications)
    const initial = await notificationRouter.handle('hierarchy.bootstrap-window', payload('notify-bootstrap', {
      windowId: 'window-1', defaultRootDirectory: '/tmp/notify-workspace',
      defaultName: 'notify-workspace', now: 1
    })) as {
      workspace: { id: string }; task: { id: string }; session: { id: string }; mount: { id: string }
    }
    notifications.ingest({
      eventId: 'error-1', type: 'error', title: '事项出错', subtitle: 'agent', body: 'failed',
      workspaceId: initial.workspace.id, taskId: initial.task.id, sessionId: initial.session.id,
      mountId: initial.mount.id, occurredAt: 2
    })

    const before = await notificationRouter.handle('projection.snapshot', { windowId: 'window-1' }) as {
      hierarchy: { unreadByTask: Record<string, number> }
    }
    expect(before.hierarchy.unreadByTask[initial.task.id]).toBe(1)

    await notificationRouter.handle('hierarchy.activate-task', payload('notify-activate', {
      windowId: 'window-1', taskId: initial.task.id, now: 3
    }))
    const after = await notificationRouter.handle('projection.snapshot', { windowId: 'window-1' }) as {
      hierarchy: { unreadByTask: Record<string, number> }
    }
    expect(after.hierarchy.unreadByTask[initial.task.id]).toBeUndefined()
  })

  it('exposes prepare and target acknowledgement for whole-Task migration', async () => {
    const initial = await router.handle('hierarchy.bootstrap-window', payload('bootstrap-move', {
      windowId: 'window-1', defaultRootDirectory: '/tmp/move-workspace',
      defaultName: 'move-workspace', now: 1
    })) as { task: { id: string } }
    await router.handle('hierarchy.bootstrap-window', payload('bootstrap-target', {
      windowId: 'window-2', defaultRootDirectory: '/tmp/move-workspace',
      defaultName: 'move-workspace', now: 2
    }))
    await router.handle('hierarchy.move-task-to-window', payload('move-prepare', {
      phase: 'prepare', migrationId: 'migration-1', taskId: initial.task.id,
      sourceWindowId: 'window-1', targetWindowId: 'window-2', now: 3
    }))
    const committed = await router.handle('hierarchy.move-task-to-window', payload('move-ack', {
      phase: 'acknowledge', migrationId: 'migration-1', now: 4
    })) as { state: string }

    expect(committed.state).toBe('committed')
    expect(database.get('SELECT window_id FROM window_task_placements WHERE task_id = ?', initial.task.id))
      .toEqual({ window_id: 'window-2' })
  })

  async function seedHierarchy(): Promise<void> {
    await router.handle('workspace.create', payload('workspace', {
      id: 'workspace-1', name: 'Workspace', rootDirectory: '/tmp/workspace', now: 1
    }))
    await router.handle('execution-context.create-plain', payload('context', {
      id: 'context-1', workspaceId: 'workspace-1', cwd: '/tmp/workspace', now: 2
    }))
    await router.handle('task.create', payload('task', {
      id: 'task-1', workspaceId: 'workspace-1', executionContextId: 'context-1',
      title: 'Task', status: 'active', sortKey: 'a', now: 3
    }))
  }
})

function payload(commandId: string, input: Record<string, unknown>) {
  return {
    command: { commandId, commandType: commandId, requestHash: `hash-${commandId}` },
    input
  }
}

function runGit(cwd: string, ...args: string[]): string {
  return execFileSync('git', ['-C', cwd, ...args], { encoding: 'utf8' })
}
