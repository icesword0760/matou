import { EventEmitter } from 'node:events'
import { execFile } from 'node:child_process'
import { chmod, mkdir, mkdtemp, readFile, readdir, readlink, realpath, rm, stat, unlink, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { promisify } from 'node:util'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { PROTOCOL_VERSION, type RpcMethod, type RuntimeMessage } from '@matou/contracts'

import { SegmentJournal, readSegmentFrames, readSessionFrames } from './journal/segment-journal'
import {
  readSessionJournalBounds
} from './journal/journal-range-reader'
import { JournalCompressor } from './journal/journal-compressor'
import { CheckpointManager } from './checkpoints/checkpoint-manager'
import {
  RuntimeServer, terminalSummaryLines, withSessionRuntimeEnvironment,
  type PortMessageEvent, type RuntimePort
} from './runtime-server'
import { RuntimeSessionRegistry } from './session/runtime-session-registry'
import { RuntimeRpcRouter } from './rpc/runtime-rpc-router'
import { ProviderHookServer } from './session/provider-hook-server'
import { SessionRepository } from './domain/session-repository'
import { DomainTransactionManager } from './storage/domain-transaction'
import { RuntimeDatabase } from './storage/database'
import { RuntimeAccessPolicy } from './storage/runtime-access-policy'
import { MigrationRunner } from './storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from './storage/migrations'
import { WorkspacePathService } from './hierarchy/workspace-path-service'
import { AgentNotificationRepository } from './notifications/agent-notification-repository'
import { ShellHistoryRepository } from './shell-history/shell-history'
import { PreferenceRepository } from './product/experience-foundation'
import { SessionForkIntentRepository } from './session/session-fork-intent-repository'

let root: string
let database: RuntimeDatabase
let port: MockPort
let server: RuntimeServer
const execFileAsync = promisify(execFile)

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-server-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  seedReplayAuthority(database, root)
  port = new MockPort()
  server = new RuntimeServer(port, root, database)
  port.receive({ type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'renderer-1' })
  await settle()
})

afterEach(() => {
  server.close()
  database.close()
})

describe('RuntimeServer domain RPC', () => {
  it('pages and searches archived terminal history through Runtime RPC', async () => {
    const journal = await SegmentJournal.open(root, 'rpc-history', { compressSealed: false })
    await journal.appendOutput(1, new TextEncoder().encode('older line\n'))
    await journal.appendOutput(2, new TextEncoder().encode('needle line\nnewest line\n'))
    await journal.close()

    port.receive({
      type: 'rpc.request', protocolVersion: PROTOCOL_VERSION, requestId: 'history-page',
      method: 'terminal.history-page', capability: 'renderer', deadlineAt: Date.now() + 1000,
      payload: { sessionId: 'rpc-history', lineLimit: 2 }
    })
    port.receive({
      type: 'rpc.request', protocolVersion: PROTOCOL_VERSION, requestId: 'history-search',
      method: 'terminal.history-search', capability: 'renderer', deadlineAt: Date.now() + 1000,
      payload: {
        sessionId: 'rpc-history', query: 'needle', limit: 10,
        options: { caseSensitive: false, regex: false, wholeWord: false }
      }
    })
    port.receive({
      type: 'rpc.request', protocolVersion: PROTOCOL_VERSION, requestId: 'history-context',
      method: 'terminal.history-page', capability: 'renderer', deadlineAt: Date.now() + 1000,
      payload: {
        sessionId: 'rpc-history', around: { sequence: 2, lineIndex: 0 },
        beforeLines: 1, afterLines: 1
      }
    })
    await waitUntil(() => port.findRpcResponse('history-context') !== undefined)

    expect(port.findRpcResponse('history-page')?.result).toMatchObject({
      lines: [{ text: 'needle line' }, { text: 'newest line' }], hasMore: true
    })
    expect(port.findRpcResponse('history-search')?.result).toMatchObject({
      matches: [{ text: 'needle line' }], hasMore: false
    })
    expect(port.findRpcResponse('history-context')?.result).toMatchObject({
      lines: [{ text: 'older line' }, { text: 'needle line' }, { text: 'newest line' }],
      anchorIndex: 1,
      hasMoreBefore: false,
      hasMoreAfter: false
    })
  })

  it('hands one real PTY from Local to its owned Worktree and keeps the Session identity', async () => {
    const repositoryRoot = join(root, 'handoff-repository')
    const worktreePath = join(root, 'handoff-worktree')
    await mkdir(repositoryRoot)
    await execFileAsync('git', ['-C', repositoryRoot, 'init', '-b', 'main'])
    await execFileAsync('git', ['-C', repositoryRoot, 'config', 'user.name', 'Matou Test'])
    await execFileAsync('git', ['-C', repositoryRoot, 'config', 'user.email', 'matou@example.test'])
    await writeFile(join(repositoryRoot, 'README.md'), 'handoff\n')
    await execFileAsync('git', ['-C', repositoryRoot, 'add', 'README.md'])
    await execFileAsync('git', ['-C', repositoryRoot, 'commit', '-m', 'initial'])
    await execFileAsync('git', [
      '-C', repositoryRoot, 'worktree', 'add', '-b', 'feature/handoff', worktreePath, 'HEAD'
    ])
    database.run(
      "UPDATE workspaces SET root_directory = ? WHERE id = 'replay-workspace'",
      repositoryRoot
    )
    database.run(
      "UPDATE execution_contexts SET cwd = ? WHERE id = 'replay-context'",
      repositoryRoot
    )
    database.run(
      `INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
       VALUES ('handoff-worktree-context', 'replay-workspace', 'git-worktree', ?, 1)`,
      worktreePath
    )
    database.run(
      `INSERT INTO worktrees (
         id, execution_context_id, repository_root, worktree_path, branch_name,
         base_ref, state, setup_policy_json, setup_result_json,
         cleanup_policy, created_at, updated_at
       ) VALUES (
         'handoff-worktree', 'handoff-worktree-context', ?, ?, 'feature/handoff',
         'HEAD', 'ready', '[]', '[]', 'retain-dirty', 1, 1
       )`,
      repositoryRoot, worktreePath
    )
    registerSession(database, 'environment-handoff-session')
    database.run(
      "UPDATE sessions SET cwd = ? WHERE id = 'environment-handoff-session'",
      repositoryRoot
    )
    database.run(
      `UPDATE session_environment_bindings
       SET managed_worktree_id = 'handoff-worktree', active_target = 'local',
           state = 'ready', updated_at = 1
       WHERE session_id = 'environment-handoff-session'`
    )

    const observedCwds = join(root, 'handoff-cwds.txt')
    const executable = join(root, 'handoff-shell.sh')
    await writeFile(executable, [
      '#!/bin/sh',
      `pwd >> ${JSON.stringify(observedCwds)}`,
      "printf 'ready\\n'",
      'sleep 30',
      ''
    ].join('\n'))
    await chmod(executable, 0o755)
    const previousShell = process.env.SHELL
    process.env.SHELL = executable
    try {
      port.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'environment-handoff-session', executionContextId: 'replay-context',
        profile: 'shell', cols: 80, rows: 24
      })
      await waitUntil(() => port.last('terminal.spawned')?.sessionId === 'environment-handoff-session')
      const firstPid = port.last('terminal.spawned')?.pid
      const firstRun = database.get<{ id: string }>(
        `SELECT id FROM session_runs
         WHERE session_id = 'environment-handoff-session' ORDER BY ordinal DESC LIMIT 1`
      )
      expect(firstRun?.id).toEqual(expect.any(String))
      if (!firstRun) throw new Error('first terminal run was not persisted')
      await waitUntilAsync(async () => (await readFile(observedCwds, 'utf8').catch(() => '')).trim() === repositoryRoot)

      port.receive(rpc('environment-handoff', 'session.environment-handoff', {
        sessionId: 'environment-handoff-session', target: 'worktree', now: Date.now()
      }))
      await waitUntil(() => port.findRpcResponse('environment-handoff') !== undefined)
      expect(port.findRpcResponse('environment-handoff')).toMatchObject({
        result: {
          kind: 'environment', sessionId: 'environment-handoff-session',
          activeTarget: 'worktree', state: 'ready', path: worktreePath
        }
      })
      expect(database.get(
        `SELECT execution_context_id, cwd FROM sessions
         WHERE id = 'environment-handoff-session'`
      )).toEqual({ execution_context_id: 'handoff-worktree-context', cwd: worktreePath })
      expect(database.get(
        `SELECT status FROM session_runs WHERE id = ?`, firstRun.id
      )).toEqual({ status: 'interrupted' })
      await waitUntil(() => port.sent.filter((message) =>
        message.type === 'terminal.spawned' && message.sessionId === 'environment-handoff-session'
      ).length === 2)
      const spawned = port.sent.filter((message) =>
        message.type === 'terminal.spawned' && message.sessionId === 'environment-handoff-session'
      )
      expect(spawned[1]).toMatchObject({ pid: expect.any(Number) })
      expect(spawned[1]).not.toHaveProperty('reattached', true)
      expect((spawned[1] as { pid: number }).pid).not.toBe(firstPid)
      expect(database.get(
        `SELECT status FROM session_runs
         WHERE session_id = 'environment-handoff-session' ORDER BY ordinal DESC LIMIT 1`
      )).toEqual({ status: 'running' })
      await waitUntilAsync(async () => (await readFile(observedCwds, 'utf8').catch(() => ''))
        .trim().split('\n').at(-1) === worktreePath)
    } finally {
      restoreEnv('SHELL', previousShell)
    }
  })

  it('publishes a read-only capability surface and gates RPC, Git, terminal, polling, and probes', async () => {
    const repositoryRoot = join(root, 'readonly-repository')
    await mkdir(repositoryRoot)
    await execFileAsync('git', ['-C', repositoryRoot, 'init', '-b', 'main'])
    await execFileAsync('git', ['-C', repositoryRoot, 'config', 'user.name', 'Matou Test'])
    await execFileAsync('git', ['-C', repositoryRoot, 'config', 'user.email', 'matou@example.test'])
    await writeFile(join(repositoryRoot, 'README.md'), 'baseline\n')
    await execFileAsync('git', ['-C', repositoryRoot, 'add', 'README.md'])
    await execFileAsync('git', ['-C', repositoryRoot, 'commit', '-m', 'baseline'])
    await execFileAsync('git', ['-C', repositoryRoot, 'branch', 'feature/read-only-must-not-switch'])

    const loginProbeMarker = join(root, 'login-probe-ran')
    const terminalMarker = join(root, 'terminal-process-ran')
    const shell = join(root, 'read-only-shell.sh')
    await writeFile(shell, [
      '#!/bin/sh',
      `if [ "$1" = "-ic" ]; then echo probe > ${JSON.stringify(loginProbeMarker)}; exit 0; fi`,
      `echo terminal > ${JSON.stringify(terminalMarker)}`,
      'sleep 5'
    ].join('\n'))
    await chmod(shell, 0o755)
    const previousShell = process.env.SHELL
    const previousHome = process.env.HOME
    const previousZdotdir = process.env.ZDOTDIR
    process.env.SHELL = shell
    process.env.HOME = root
    process.env.ZDOTDIR = join(root, 'readonly-zdotdir')
    registerSession(database, 'readonly-environment-session')

    server.close()
    const databasePath = database.path
    database.close()
    const originalBytes = await readFile(databasePath)
    database = RuntimeDatabase.openReadOnly(databasePath)
    const policy = new RuntimeAccessPolicy('read-only')
    const router = new RuntimeRpcRouter(database, undefined, { accessPolicy: policy })
    const workspacePaths = new PollingSpyWorkspacePathService(
      database,
      new DomainTransactionManager(database)
    )
    port = new MockPort()
    server = new RuntimeServer(
      port,
      root,
      database,
      router,
      undefined,
      new RuntimeSessionRegistry(),
      undefined,
      workspacePaths,
      { accessPolicy: policy }
    )
    port.receive({ type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'readonly' })
    await settle()

    try {
      expect(port.last('protocol.ready')?.capabilities).toEqual([
        'semantic-events-v1', 'replay-v1', 'projection-v1'
      ])
      expect(port.last('session.recovery-snapshot')).toMatchObject({ statuses: [] })
      expect(workspacePaths.startCount).toBe(0)

      port.receive({
        type: 'rpc.request', protocolVersion: PROTOCOL_VERSION, requestId: 'readonly-projection',
        method: 'projection.snapshot', capability: 'renderer', deadlineAt: Date.now() + 1000,
        payload: {}
      })
      await settle()
      expect(port.findRpcResponse('readonly-projection')).toMatchObject({
        result: { hierarchy: { workspaces: [expect.objectContaining({ id: 'replay-workspace' })] } }
      })

      port.receive({
        type: 'rpc.request', protocolVersion: PROTOCOL_VERSION,
        requestId: 'readonly-environment-open', method: 'session.environment-open',
        capability: 'renderer', deadlineAt: Date.now() + 1000,
        payload: { sessionId: 'readonly-environment-session' }
      })
      await settle()
      expect(port.findRpcResponse('readonly-environment-open')).toMatchObject({
        result: {
          sessionId: 'readonly-environment-session', kind: 'local', path: await realpath(root)
        }
      })

      port.receive(rpc('readonly-bootstrap', 'hierarchy.bootstrap-window', {
        windowId: 'read-only-window', defaultRootDirectory: root,
        defaultName: 'Must Not Create', now: Date.now()
      }))
      port.receive(rpc('readonly-git', 'git.checkout', {
        cwd: repositoryRoot, branch: 'feature/read-only-must-not-switch', now: Date.now()
      }))
      port.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'read-only-transient', executionContextId: 'local-default',
        profile: 'shell', cols: 80, rows: 24
      })
      await settle()
      await new Promise((resolve) => setTimeout(resolve, 100))

      expect(port.findRpcError('readonly-bootstrap')).toMatchObject({ code: 'STORAGE_READ_ONLY' })
      expect(port.findRpcError('readonly-git')).toMatchObject({ code: 'STORAGE_READ_ONLY' })
      expect(port.last('protocol.error')).toMatchObject({ code: 'STORAGE_READ_ONLY' })
      expect((await execFileAsync('git', ['-C', repositoryRoot, 'branch', '--show-current'])).stdout.trim())
        .toBe('main')
      await expect(stat(loginProbeMarker)).rejects.toMatchObject({ code: 'ENOENT' })
      await expect(stat(terminalMarker)).rejects.toMatchObject({ code: 'ENOENT' })
      await expect(stat(join(root, 'sessions', 'read-only-transient'))).rejects.toMatchObject({
        code: 'ENOENT'
      })
      expect(await readFile(databasePath)).toEqual(originalBytes)
    } finally {
      restoreEnv('SHELL', previousShell)
      restoreEnv('HOME', previousHome)
      restoreEnv('ZDOTDIR', previousZdotdir)
    }
  })

  it('adds current cwd and Git information to a direct DAG graph response', () => {
    const result = withSessionRuntimeEnvironment({
      sceneId: 'scene-1',
      nodes: [{ sessionId: 'session-1', cwd: '/old' }],
      edges: []
    }, [{
      sessionId: 'session-1', mode: 'shell', cwd: '/repo/deep',
      gitBranch: 'feature/dag', gitDirty: true, startedAt: 1
    }])

    expect(result).toMatchObject({
      nodes: [{
        sessionId: 'session-1', cwd: '/repo/deep',
        git: { branch: 'feature/dag', dirty: true }
      }]
    })
  })

  it('keeps the latest four readable terminal lines for live DAG cards', () => {
    expect(terminalSummaryLines('\u001b[31mone\u001b[0m\r\ntwo\nthree\nfour\nfive\n')).toEqual([
      'two', 'three', 'four', 'five'
    ])
  })
  it('drops shell prompts before choosing the latest four meaningful DAG lines', () => {
    expect(terminalSummaryLines([
      'RED', '中文_码头 😀', 'LONG', 'ALT_SCREEN', 'FINAL', '%',
      'icesword@MacBook workspace %'
    ].join('\n'))).toEqual(['中文_码头 😀', 'LONG', 'ALT_SCREEN', 'FINAL'])
  })
  it('does not replace a restored DAG summary with provider chrome-only output', () => {
    expect(terminalSummaryLines([
      '\u001b(B❯',
      '───────────────────────────────────────────────────────────',
      '⏸ manual mode on · ← for agents ● high · /effort'
    ].join('\r\n'))).toEqual([])
  })
  it('preserves the prior DAG summary when a restored Shell emits only its prompt', async () => {
    const executable = join(root, 'prompt-only-shell.sh')
    await writeFile(executable, '#!/bin/sh\nprintf "%%\\r\\n"\nsleep 2\n')
    await chmod(executable, 0o755)
    const previousShell = process.env.SHELL
    process.env.SHELL = executable
    registerSession(database, 'restored-summary-session')
    database.run(
      `INSERT INTO session_graph_summaries (session_id, latest_lines_json, updated_at)
       VALUES (?, ?, ?)`,
      'restored-summary-session', JSON.stringify(['LAST_MEANINGFUL_OUTPUT']), 1
    )
    try {
      port.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'restored-summary-session', executionContextId: 'replay-context',
        profile: 'shell', cols: 80, rows: 24
      })
      await waitUntil(() => terminalText(port).includes('%'))
      await new Promise((resolve) => setTimeout(resolve, 300))

      expect(database.get<{ latest_lines_json: string }>(
        'SELECT latest_lines_json FROM session_graph_summaries WHERE session_id = ?',
        'restored-summary-session'
      )?.latest_lines_json).toBe(JSON.stringify(['LAST_MEANINGFUL_OUTPUT']))
    } finally {
      port.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'restored-summary-session'
      })
      await settle()
      restoreEnv('SHELL', previousShell)
    }
  })
  it('publishes live per-Session HUD state in projection snapshots and terminal updates', async () => {
    registerSession(database, 'hud-session')
    port.receive({
      type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION, sessionId: 'hud-session',
      executionContextId: 'replay-context', profile: 'shell', cols: 80, rows: 24
    })
    await waitUntil(() => port.last('terminal.hud') !== undefined)
    expect(port.last('terminal.hud')).toMatchObject({
      sessionId: 'hud-session', hud: { sessionId: 'hud-session', mode: 'shell' }
    })

    port.receive({
      type: 'rpc.request', protocolVersion: PROTOCOL_VERSION, requestId: 'hud-snapshot',
      method: 'projection.snapshot', capability: 'renderer', deadlineAt: Date.now() + 1000,
      payload: {}
    })
    await settle()
    expect(port.findRpcResponse('hud-snapshot')).toMatchObject({
      result: { hierarchy: { sessionHuds: [expect.objectContaining({ sessionId: 'hud-session' })] } }
    })
  })

  it('republishes attached HUD branch state immediately after a Git mutation', async () => {
    const repositoryRoot = join(root, 'hud-repository')
    await mkdir(repositoryRoot)
    await execFileAsync('git', ['-C', repositoryRoot, 'init', '-b', 'main'])
    await execFileAsync('git', ['-C', repositoryRoot, 'config', 'user.name', 'Matou Test'])
    await execFileAsync('git', ['-C', repositoryRoot, 'config', 'user.email', 'matou@example.test'])
    await writeFile(join(repositoryRoot, 'README.md'), 'baseline\n')
    await execFileAsync('git', ['-C', repositoryRoot, 'add', 'README.md'])
    await execFileAsync('git', ['-C', repositoryRoot, 'commit', '-m', 'baseline'])
    await execFileAsync('git', ['-C', repositoryRoot, 'branch', 'feature/hud-refresh'])
    database.run('UPDATE execution_contexts SET cwd = ? WHERE id = ?', repositoryRoot, 'replay-context')
    registerSession(database, 'git-hud-session')
    port.receive({
      type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'git-hud-session', executionContextId: 'replay-context',
      profile: 'shell', cols: 80, rows: 24
    })
    await waitUntil(() => port.last('terminal.hud')?.hud?.gitBranch === 'main')
    expect(database.get<{
      state: string
      branch: string | null
      detached_head: string | null
      dirty: number
    }>(
      `SELECT state, branch, detached_head, dirty
       FROM execution_context_git_states WHERE execution_context_id = 'replay-context'`
    )).toEqual({ state: 'ready', branch: 'main', detached_head: null, dirty: 0 })

    port.receive(rpc('git-hud-checkout', 'git.checkout', {
      cwd: repositoryRoot, branch: 'feature/hud-refresh', now: Date.now()
    }))

    await waitUntil(() => port.findRpcResponse('git-hud-checkout') !== undefined)
    await waitUntil(() => port.last('terminal.hud')?.hud?.gitBranch === 'feature/hud-refresh')
    expect(port.last('terminal.hud')).toMatchObject({
      sessionId: 'git-hud-session', hud: { gitBranch: 'feature/hud-refresh', gitDirty: false }
    })
  })

  it('advertises replay and replays durable output after a Runtime reconnect', async () => {
    const journal = await SegmentJournal.open(root, 'persisted-session')
    registerSession(database, 'persisted-session')
    await journal.appendOutput(1, new TextEncoder().encode('first'))
    await journal.appendResize(2, 100, 40)
    await journal.appendOutput(3, new TextEncoder().encode('second'))
    await journal.appendExit(4, 0)
    await journal.close()

    expect(port.last('protocol.ready')?.capabilities).toContain('replay-v1')
    port.receive({
      type: 'terminal.replay-request', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'persisted-session', fromSequence: 2
    })
    await settle()

    expect(port.last('terminal.replay-start')).toMatchObject({
      sessionId: 'persisted-session', availableFromSequence: 1, liveSequence: 4
    })
    expect(port.sent.filter((message) => message.type === 'terminal.data')).toEqual([
      expect.objectContaining({ sessionId: 'persisted-session', sequence: 3 })
    ])
    expect(port.last('terminal.replay-resize')).toMatchObject({
      sessionId: 'persisted-session', sequence: 2, cols: 100, rows: 40
    })
    expect(port.last('terminal.exited')).toMatchObject({ sequence: 4, exitCode: 0 })
    expect(port.last('terminal.replay-complete')).toMatchObject({ throughSequence: 4 })
  })

  it('restores completed Shell Blocks instead of replaying old raw terminal output on launch', async () => {
    registerSession(database, 'block-restore-session')
    new ShellHistoryRepository(database).complete({
      sessionId: 'block-restore-session', command: 'printf durable-block', cwd: root,
      output: 'durable-block\r\n', exitCode: 0, startedAt: 1, completedAt: 2
    })
    const journal = await SegmentJournal.open(root, 'block-restore-session')
    await journal.appendOutput(1, new TextEncoder().encode('OLD_RAW_HISTORY_MUST_NOT_RETURN'))
    await journal.close()
    const executable = join(root, 'fresh-shell.sh')
    await writeFile(executable, '#!/bin/sh\nprintf "FRESH_PROMPT\\n"\nsleep 5\n')
    await chmod(executable, 0o755)
    const previousShell = process.env.SHELL
    process.env.SHELL = executable
    try {
      port.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'block-restore-session', executionContextId: 'replay-context',
        profile: 'shell', cols: 80, rows: 24
      })
      await waitUntil(() => port.last('terminal.spawned') !== undefined)

      const restored = port.last('terminal.restored-history')
      expect(new TextDecoder().decode(restored?.data)).toContain('❯ printf durable-block')
      expect(new TextDecoder().decode(restored?.data)).toContain('会话已恢复')
      expect(new TextDecoder().decode(restored?.data)).not.toContain('OLD_RAW_HISTORY_MUST_NOT_RETURN')
      expect(port.last('terminal.spawned')).not.toHaveProperty('reattached', true)
    } finally {
      port.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'block-restore-session'
      })
      await settle()
      restoreEnv('SHELL', previousShell)
    }
  })

  it('captures only zsh commands that reach a completion boundary', async () => {
    registerSession(database, 'block-capture-session')
    const executable = join(root, 'zsh')
    const encoded = Buffer.from('printf captured', 'utf8').toString('base64')
    await writeFile(executable, [
      '#!/bin/sh',
      `printf '\\033]633;E;${encoded}\\007\\033]133;C\\007captured\\n\\033]133;D;0\\007\\033]133;A\\007'`,
      'sleep 5',
      ''
    ].join('\n'))
    await chmod(executable, 0o755)
    const previousShell = process.env.SHELL
    process.env.SHELL = executable
    try {
      port.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'block-capture-session', executionContextId: 'replay-context',
        profile: 'shell', cols: 80, rows: 24
      })
      await waitUntil(() => new ShellHistoryRepository(database).list('block-capture-session').length === 1)

      expect(new ShellHistoryRepository(database).list('block-capture-session')[0]).toMatchObject({
        command: 'printf captured', output: 'captured\r\n', exitCode: 0, cwd: root
      })
    } finally {
      port.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'block-capture-session'
      })
      await settle()
      restoreEnv('SHELL', previousShell)
    }
  })

  it('falls back to the Matou Workspace root when the saved Shell directory disappeared', async () => {
    const workspaceRoot = join(root, 'workspace-root')
    const contextRoot = join(root, 'worktree-context')
    await mkdir(workspaceRoot, { recursive: true })
    await mkdir(contextRoot, { recursive: true })
    database.run('UPDATE workspaces SET root_directory = ? WHERE id = ?', workspaceRoot, 'replay-workspace')
    database.run('UPDATE execution_contexts SET cwd = ? WHERE id = ?', contextRoot, 'replay-context')
    registerSession(database, 'missing-cwd-session')
    database.run('UPDATE sessions SET cwd = ? WHERE id = ?', join(root, 'deleted-directory'), 'missing-cwd-session')
    const observedCwd = join(root, 'observed-cwd.txt')
    const executable = join(root, 'cwd-shell.sh')
    await writeFile(executable, `#!/bin/sh\npwd > ${JSON.stringify(observedCwd)}\nsleep 5\n`)
    await chmod(executable, 0o755)
    const previousShell = process.env.SHELL
    process.env.SHELL = executable
    try {
      port.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'missing-cwd-session', executionContextId: 'replay-context',
        profile: 'shell', cols: 80, rows: 24
      })
      await waitUntilAsync(async () => (await readFile(observedCwd, 'utf8').catch(() => '')).length > 0)

      expect((await readFile(observedCwd, 'utf8')).trim()).toBe(workspaceRoot)
      expect(database.get<{ cwd: string }>('SELECT cwd FROM sessions WHERE id = ?', 'missing-cwd-session'))
        .toEqual({ cwd: workspaceRoot })
    } finally {
      port.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'missing-cwd-session'
      })
      await settle()
      restoreEnv('SHELL', previousShell)
    }
  })

  it('keeps a managed Worktree Session inert when its owned directory disappears', async () => {
    const repositoryRoot = join(root, 'managed-repository')
    const worktreePath = join(root, 'managed-worktree')
    await mkdir(repositoryRoot)
    await execFileAsync('git', ['-C', repositoryRoot, 'init', '-b', 'main'])
    await execFileAsync('git', ['-C', repositoryRoot, 'config', 'user.name', 'Matou Test'])
    await execFileAsync('git', ['-C', repositoryRoot, 'config', 'user.email', 'matou@example.test'])
    await writeFile(join(repositoryRoot, 'README.md'), 'baseline\n')
    await execFileAsync('git', ['-C', repositoryRoot, 'add', 'README.md'])
    await execFileAsync('git', ['-C', repositoryRoot, 'commit', '-m', 'baseline'])
    await execFileAsync('git', [
      '-C', repositoryRoot, 'worktree', 'add', '-b', 'feature/missing', worktreePath, 'HEAD'
    ])
    database.transaction((tx) => {
      tx.run(
        `INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
         VALUES ('managed-context', 'replay-workspace', 'git-worktree', ?, 1)`,
        worktreePath
      )
      tx.run(
        `INSERT INTO worktrees (
           id, execution_context_id, repository_root, worktree_path, branch_name,
           base_ref, state, setup_policy_json, setup_result_json,
           cleanup_policy, created_at, updated_at
         ) VALUES (
           'managed-worktree', 'managed-context', ?, ?, 'feature/missing',
           'HEAD', 'ready', '[]', '[]', 'retain-dirty', 1, 1
         )`,
        repositoryRoot, worktreePath
      )
    })
    registerSession(database, 'managed-missing-session')
    database.transaction((tx) => {
      tx.run(
        `UPDATE session_environment_bindings
         SET managed_worktree_id = 'managed-worktree', active_target = 'worktree',
             state = 'ready', updated_at = 1
         WHERE session_id = 'managed-missing-session'`
      )
      tx.run(
        `UPDATE sessions SET execution_context_id = 'managed-context', cwd = ?
         WHERE id = 'managed-missing-session'`,
        worktreePath
      )
    })
    await rm(worktreePath, { recursive: true, force: true })

    server.close()
    const sessions = new RuntimeSessionRegistry()
    port = new MockPort()
    server = new RuntimeServer(port, root, database, undefined, undefined, sessions)
    port.receive({
      type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'managed-missing'
    })
    await settle()

    port.receive({
      type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'managed-missing-session', executionContextId: 'managed-context',
      profile: 'shell', cols: 80, rows: 24
    })
    await waitUntil(() => port.last('protocol.error') !== undefined)

    expect(port.last('protocol.error')).toMatchObject({
      code: 'SESSION_ENVIRONMENT_UNAVAILABLE',
      sessionId: 'managed-missing-session'
    })
    expect(sessions.has('managed-missing-session')).toBe(false)
    expect(database.get<{ cwd: string }>(
      'SELECT cwd FROM sessions WHERE id = ?', 'managed-missing-session'
    )).toEqual({ cwd: worktreePath })
    expect(database.get<{ state: string }>(
      `SELECT state FROM session_environment_bindings
       WHERE session_id = 'managed-missing-session'`
    )).toEqual({ state: 'missing' })
  })

  it('keeps saved Shell Blocks private when restoration is disabled and stops recording new ones', async () => {
    registerSession(database, 'history-disabled-session')
    const history = new ShellHistoryRepository(database)
    history.complete({
      sessionId: 'history-disabled-session', command: 'kept-but-hidden', cwd: root,
      output: 'old\r\n', exitCode: 0, startedAt: 1, completedAt: 2
    })
    new PreferenceRepository(database).set('shell.restoreHistoryEnabled', false)
    const executable = join(root, 'zsh')
    const encoded = Buffer.from('new-command', 'utf8').toString('base64')
    await writeFile(executable, [
      '#!/bin/sh',
      `printf '\\033]633;E;${encoded}\\007new\\n\\033]133;D;0\\007'`,
      'sleep 5',
      ''
    ].join('\n'))
    await chmod(executable, 0o755)
    const previousShell = process.env.SHELL
    process.env.SHELL = executable
    try {
      port.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'history-disabled-session', executionContextId: 'replay-context',
        profile: 'shell', cols: 80, rows: 24
      })
      await waitUntil(() => terminalText(port).includes('new'))

      expect(port.sent.filter(({ type }) => type === 'terminal.restored-history')).toHaveLength(0)
      expect(history.list('history-disabled-session').map(({ command }) => command))
        .toEqual(['kept-but-hidden'])
    } finally {
      port.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'history-disabled-session'
      })
      await settle()
      restoreEnv('SHELL', previousShell)
    }
  })

  it('does not report a historical exit while replaying into a new live Shell run', async () => {
    const journal = await SegmentJournal.open(root, 'live-after-exit')
    await journal.appendOutput(1, new TextEncoder().encode('previous run'))
    await journal.appendExit(2, 1)
    await journal.close()
    registerSession(database, 'live-after-exit')
    const executable = join(root, 'live-after-exit.sh')
    await writeFile(executable, '#!/bin/sh\nprintf "LIVE_RUN_READY\\n"\nsleep 5\n')
    await chmod(executable, 0o755)
    const previousShell = process.env.SHELL
    process.env.SHELL = executable
    try {
      port.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'live-after-exit', executionContextId: 'replay-context',
        profile: 'shell', cols: 80, rows: 24
      })
      await waitUntil(() => terminalText(port).includes('LIVE_RUN_READY'))
      const beforeReplay = port.sent.length

      port.receive({
        type: 'terminal.replay-request', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'live-after-exit', fromSequence: 0
      })
      await waitUntil(() => port.sent.slice(beforeReplay).some(({ type }) =>
        type === 'terminal.replay-complete'
      ))

      expect(port.sent.slice(beforeReplay).filter(({ type }) => type === 'terminal.exited'))
        .toHaveLength(0)
    } finally {
      port.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'live-after-exit'
      })
      await settle()
      restoreEnv('SHELL', previousShell)
    }
  })

  it('reports a retention gap before replaying the available suffix', async () => {
    const journal = await SegmentJournal.open(root, 'retained-session')
    registerSession(database, 'retained-session')
    await journal.appendOutput(7, new TextEncoder().encode('retained'))
    await journal.close()

    port.receive({
      type: 'terminal.replay-request', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'retained-session', fromSequence: 1
    })
    await settle()

    expect(port.last('terminal.gap')).toMatchObject({
      requestedFromSequence: 1, availableFromSequence: 7, reason: 'retention'
    })
    expect(port.last('terminal.data')).toMatchObject({ sequence: 7 })
    expect(port.last('terminal.replay-complete')).toMatchObject({ throughSequence: 7 })
  })

  it('accepts a delayed cumulative ACK after persisted replay has completed', async () => {
    const journal = await SegmentJournal.open(root, 'completed-replay-ack')
    registerSession(database, 'completed-replay-ack')
    await journal.appendOutput(1, new TextEncoder().encode('history'))
    await journal.close()
    port.receive({
      type: 'terminal.replay-request', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'completed-replay-ack', fromSequence: 0
    })
    await waitUntil(() => port.last('terminal.replay-complete') !== undefined)
    const errorsBefore = port.sent.filter(({ type }) => type === 'protocol.error').length

    port.receive({
      type: 'terminal.ack', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'completed-replay-ack', throughSequence: 1
    })
    await settle()

    expect(port.sent.filter(({ type }) => type === 'protocol.error')).toHaveLength(errorsBefore)
  })

  it('applies cumulative ACK backpressure while replaying a large persisted journal', async () => {
    const journal = await SegmentJournal.open(root, 'large-replay')
    registerSession(database, 'large-replay')
    for (let sequence = 1; sequence <= 3; sequence += 1) {
      await journal.appendOutput(sequence, new Uint8Array(600 * 1024))
    }
    await journal.close()

    port.receive({
      type: 'terminal.replay-request', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'large-replay', fromSequence: 0
    })
    await settle()
    expect(port.sent.filter((message) => message.type === 'terminal.data')).toHaveLength(2)
    expect(port.last('terminal.replay-complete')).toBeUndefined()

    port.receive({
      type: 'terminal.ack', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'large-replay', throughSequence: 2
    })
    await settle()
    expect(port.sent.filter((message) => message.type === 'terminal.data')).toHaveLength(3)
    expect(port.last('terminal.replay-complete')).toMatchObject({ throughSequence: 3 })
    expect(port.last('protocol.error')).toBeUndefined()
  })

  it('restores the latest paired checkpoint and only replays its journal tail', async () => {
    registerSession(database, 'checkpoint-replay')
    const journal = await SegmentJournal.open(root, 'checkpoint-replay')
    await journal.appendOutput(1, Uint8Array.from([65]))
    await journal.appendDomainCursor(2, 1)
    await journal.appendOutput(3, Uint8Array.from([66]))
    await journal.close()
    await new CheckpointManager(root, database).create({
      sessionId: 'checkpoint-replay', terminalSequence: 2, domainEventSequence: 1,
      screenEpoch: 4, snapshot: Uint8Array.from([9, 8, 7])
    })

    port.receive({
      type: 'terminal.replay-request', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'checkpoint-replay', fromSequence: 0
    })
    await settle()

    expect(port.last('terminal.replay-start')).toMatchObject({
      checkpointSequence: 2,
      checkpoint: {
        terminalSequence: 2, domainEventSequence: 1, screenEpoch: 4,
        snapshot: Uint8Array.from([9, 8, 7])
      }
    })
    expect(port.sent.filter((message) => message.type === 'terminal.data')).toEqual([
      expect.objectContaining({ sequence: 3, data: Uint8Array.from([66]) })
    ])
  })

  it('starts checkpoint replay without decoding cold segments before its watermark', async () => {
    registerSession(database, 'checkpoint-range-replay')
    const journal = await SegmentJournal.open(root, 'checkpoint-range-replay', {
      maxSegmentBytes: 360,
      compressSealed: false
    })
    await journal.appendOutput(1, new TextEncoder().encode('cold-output'.repeat(30)))
    await journal.appendDomainCursor(2, 7)
    await journal.appendOutput(3, new TextEncoder().encode('visible-tail'.repeat(30)))
    await journal.close()
    await new CheckpointManager(root, database).create({
      sessionId: 'checkpoint-range-replay', terminalSequence: 2, domainEventSequence: 7,
      screenEpoch: 9, snapshot: Uint8Array.from([1, 3, 5])
    })
    const bounds = await readSessionJournalBounds(root, 'checkpoint-range-replay')
    const cold = bounds.segments.filter(({ lastSequence }) => lastSequence <= 2)
    expect(cold.length).toBeGreaterThan(0)
    await Promise.all(cold.map(({ path }) => writeFile(path, 'corrupted cold segment')))

    port.receive({
      type: 'terminal.replay-request', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'checkpoint-range-replay', fromSequence: 0
    })
    await waitUntil(() =>
      port.last('terminal.replay-complete') !== undefined || port.last('terminal.gap') !== undefined
    )

    expect(port.last('terminal.gap')).toBeUndefined()
    expect(port.last('terminal.replay-start')).toMatchObject({
      source: 'checkpoint', checkpointSequence: 2, fromSequence: 3, throughSequence: 3
    })
    expect(port.sent.filter(({ type }) => type === 'terminal.data')).toEqual([
      expect.objectContaining({ sequence: 3 })
    ])
    expect(port.last('terminal.replay-complete')).toMatchObject({ throughSequence: 3 })
  })

  it('protects the retained checkpoint segment when a real PTY restarts', async () => {
    registerSession(database, 'checkpoint-protected-restart')
    const writer = await SegmentJournal.open(root, 'checkpoint-protected-restart', {
      maxSegmentBytes: 150,
      rawHotBytes: 1,
      compressSealed: false
    })
    for (let sequence = 1; sequence <= 8; sequence += 1) {
      await writer.appendOutput(sequence, Uint8Array.from({ length: 32 }, () => sequence))
    }
    await writer.close()
    await new CheckpointManager(root, database).create({
      sessionId: 'checkpoint-protected-restart', terminalSequence: 2,
      domainEventSequence: 0, screenEpoch: 1, snapshot: Uint8Array.from([2])
    })

    server.close()
    port = new MockPort()
    const compressor = new JournalCompressor()
    server = new RuntimeServer(port, root, database, undefined, undefined, undefined, undefined, undefined, {
      journalOptionsForSession: () => ({ maxSegmentBytes: 150, rawHotBytes: 1, compressor })
    })
    port.receive({ type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'renderer-2' })
    await settle()

    const executable = join(root, 'checkpoint-restart-shell.sh')
    await writeFile(executable, '#!/bin/sh\nsleep 30\n')
    await chmod(executable, 0o755)
    const previousShell = process.env.SHELL
    process.env.SHELL = executable
    try {
      port.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'checkpoint-protected-restart', executionContextId: 'replay-context',
        profile: 'shell', cols: 80, rows: 24
      })
      await waitUntil(() => port.last('terminal.spawned')?.sessionId === 'checkpoint-protected-restart')
      await compressor.whenIdle()

      const bounds = await readSessionJournalBounds(root, 'checkpoint-protected-restart')
      expect(bounds.segments.find(({ firstSequence, lastSequence }) =>
        firstSequence <= 2 && lastSequence >= 2
      )?.path).toMatch(/\.mtj$/)

      port.receive({
        type: 'terminal.replay-request', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'checkpoint-protected-restart', fromSequence: 0
      })
      await waitUntil(() => port.last('terminal.replay-complete')?.sessionId === 'checkpoint-protected-restart')
      expect(port.last('terminal.replay-start')).toMatchObject({
        sessionId: 'checkpoint-protected-restart', source: 'checkpoint', checkpointSequence: 2
      })
      expect(port.sent.flatMap((message) =>
        message.type === 'terminal.data' && message.sessionId === 'checkpoint-protected-restart'
          ? [message.sequence]
          : []
      )).toEqual([3, 4, 5, 6, 7, 8])
    } finally {
      process.env.SHELL = previousShell
    }
  })

  it('isolates corruption discovered while streaming one Session range', async () => {
    registerSession(database, 'stream-corruption')
    registerSession(database, 'stream-healthy')
    const damagedJournal = await SegmentJournal.open(root, 'stream-corruption', {
      compressSealed: false
    })
    await damagedJournal.appendOutput(1, new TextEncoder().encode('damaged output'))
    await damagedJournal.close()
    const healthyJournal = await SegmentJournal.open(root, 'stream-healthy', {
      compressSealed: false
    })
    await healthyJournal.appendOutput(1, new TextEncoder().encode('healthy output'))
    await healthyJournal.close()
    const damaged = await readFile(damagedJournal.path)
    damaged[damaged.byteLength - 1] = damaged[damaged.byteLength - 1]! ^ 0xff
    await writeFile(damagedJournal.path, damaged)

    port.receive({
      type: 'terminal.replay-request', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'stream-corruption', fromSequence: 0
    })
    await waitUntil(() => port.last('terminal.gap') !== undefined)
    port.receive({
      type: 'terminal.replay-request', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'stream-healthy', fromSequence: 0
    })
    await waitUntil(() => port.last('terminal.replay-complete')?.sessionId === 'stream-healthy')

    expect(port.sent.find((message) =>
      message.type === 'terminal.replay-start' && message.sessionId === 'stream-corruption'
    )).toBeDefined()
    expect(port.last('terminal.gap')).toMatchObject({
      sessionId: 'stream-corruption', reason: 'corruption'
    })
    expect(port.sent.find((message) =>
      message.type === 'terminal.replay-complete' && message.sessionId === 'stream-corruption'
    )).toBeUndefined()
    expect(port.last('terminal.replay-complete')).toMatchObject({
      sessionId: 'stream-healthy', throughSequence: 1
    })
  })

  it('uses a current-run checkpoint on PTY reattach without dropping the requested prefix', async () => {
    registerSession(database, 'reattach-checkpoint')
    const journal = await SegmentJournal.open(root, 'reattach-checkpoint')
    for (let sequence = 1; sequence <= 6; sequence += 1) {
      await journal.appendOutput(sequence, Uint8Array.from([64 + sequence]))
    }
    await journal.close()
    await new CheckpointManager(root, database).create({
      sessionId: 'reattach-checkpoint', terminalSequence: 5, domainEventSequence: 0,
      screenEpoch: 0, snapshot: Uint8Array.from([9, 9])
    })

    port.receive({
      type: 'terminal.replay-request', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'reattach-checkpoint', fromSequence: 3
    })
    await settle()

    expect(port.last('terminal.replay-start')).toMatchObject({
      source: 'checkpoint', fromSequence: 6, checkpointSequence: 5
    })
    expect(port.sent.filter((message) => message.type === 'terminal.data')).toEqual([
      expect.objectContaining({ sequence: 6, data: Uint8Array.from([70]) })
    ])
  })

  it('replays the requested run tail when Renderer retains its existing VT model', async () => {
    registerSession(database, 'reattach-retained-model')
    const journal = await SegmentJournal.open(root, 'reattach-retained-model')
    for (let sequence = 1; sequence <= 6; sequence += 1) {
      await journal.appendOutput(sequence, Uint8Array.from([64 + sequence]))
    }
    await journal.close()
    await new CheckpointManager(root, database).create({
      sessionId: 'reattach-retained-model', terminalSequence: 5, domainEventSequence: 0,
      screenEpoch: 0, snapshot: Uint8Array.from([9, 9])
    })

    port.receive({
      type: 'terminal.replay-request', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'reattach-retained-model', fromSequence: 3,
      preserveExistingModel: true
    })
    await settle()

    expect(port.last('terminal.replay-start')).toMatchObject({
      source: 'tail', fromSequence: 3
    })
    expect(port.last('terminal.replay-start')).not.toHaveProperty('checkpoint')
    expect(port.sent.filter((message) => message.type === 'terminal.data').map(
      (message) => message.sequence
    )).toEqual([3, 4, 5, 6])
  })

  it('ignores a checkpoint older than the requested PTY run and replays the full run prefix', async () => {
    registerSession(database, 'older-reattach-checkpoint')
    const journal = await SegmentJournal.open(root, 'older-reattach-checkpoint')
    for (let sequence = 1; sequence <= 6; sequence += 1) {
      await journal.appendOutput(sequence, Uint8Array.from([64 + sequence]))
    }
    await journal.close()
    await new CheckpointManager(root, database).create({
      sessionId: 'older-reattach-checkpoint', terminalSequence: 2, domainEventSequence: 0,
      screenEpoch: 0, snapshot: Uint8Array.from([8, 8])
    })

    port.receive({
      type: 'terminal.replay-request', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'older-reattach-checkpoint', fromSequence: 3
    })
    await settle()

    expect(port.last('terminal.replay-start')).toMatchObject({
      source: 'tail', fromSequence: 3
    })
    expect(port.last('terminal.replay-start')).not.toHaveProperty('checkpoint')
    expect(port.sent.filter((message) => message.type === 'terminal.data').map(
      (message) => message.sequence
    )).toEqual([3, 4, 5, 6])
  })

  it('stores an acknowledged Renderer checkpoint at a durable Journal watermark', async () => {
    registerSession(database, 'renderer-checkpoint')
    const journal = await SegmentJournal.open(root, 'renderer-checkpoint')
    await journal.appendOutput(1, new TextEncoder().encode('first\n'))
    await journal.appendDomainCursor(2, 7)
    await journal.appendOutput(3, new TextEncoder().encode('second\n'))
    await journal.close()

    port.receive({
      type: 'terminal.checkpoint', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'renderer-checkpoint', throughSequence: 3, screenEpoch: 4,
      snapshot: '\u001b[2Jserialized screen'
    })
    await waitUntil(() => port.last('terminal.checkpoint-stored') !== undefined)

    expect(port.last('terminal.checkpoint-stored')).toMatchObject({
      sessionId: 'renderer-checkpoint', throughSequence: 3
    })
    await expect(new CheckpointManager(root, database).loadLatest('renderer-checkpoint', {
      terminalSequence: 3, domainEventSequence: 7
    })).resolves.toMatchObject({
      terminalSequence: 3, domainEventSequence: 7, screenEpoch: 4,
      snapshot: new TextEncoder().encode('\u001b[2Jserialized screen')
    })
  })

  it('stores a current checkpoint from indexed watermarks without materializing cold history', async () => {
    registerSession(database, 'range-indexed-checkpoint')
    const journal = await SegmentJournal.open(root, 'range-indexed-checkpoint', {
      maxSegmentBytes: 360,
      compressSealed: false
    })
    await journal.appendOutput(1, new TextEncoder().encode('cold-output'.repeat(30)))
    await journal.appendDomainCursor(2, 11)
    await journal.appendOutput(3, new TextEncoder().encode('current-output'.repeat(30)))
    await journal.close()
    const bounds = await readSessionJournalBounds(root, 'range-indexed-checkpoint')
    const cold = bounds.segments.filter(({ lastSequence }) => lastSequence <= 2)
    expect(cold.length).toBeGreaterThan(0)
    await Promise.all(cold.map(({ path }) => writeFile(path, 'corrupted cold segment')))

    port.receive({
      type: 'terminal.checkpoint', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'range-indexed-checkpoint', throughSequence: 3, screenEpoch: 4,
      snapshot: 'indexed screen'
    })
    await waitUntil(() =>
      port.last('terminal.checkpoint-stored') !== undefined ||
      port.last('terminal.checkpoint-rejected') !== undefined
    )

    expect(port.last('terminal.checkpoint-rejected')).toBeUndefined()
    expect(port.last('terminal.checkpoint-stored')).toMatchObject({ throughSequence: 3 })
    await expect(new CheckpointManager(root, database).loadLatest('range-indexed-checkpoint', {
      terminalSequence: 3,
      domainEventSequence: 11
    })).resolves.toMatchObject({ terminalSequence: 3, domainEventSequence: 11 })
  })

  it('rejects checkpoints ahead of the Journal or behind an already stored watermark', async () => {
    registerSession(database, 'invalid-renderer-checkpoint')
    const journal = await SegmentJournal.open(root, 'invalid-renderer-checkpoint')
    await journal.appendOutput(1, new TextEncoder().encode('one\n'))
    await journal.appendOutput(2, new TextEncoder().encode('two\n'))
    await journal.close()

    port.receive({
      type: 'terminal.checkpoint', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'invalid-renderer-checkpoint', throughSequence: 3, screenEpoch: 0,
      snapshot: 'future'
    })
    await waitUntil(() => port.last('terminal.checkpoint-rejected') !== undefined)
    expect(port.last('terminal.checkpoint-rejected')).toMatchObject({
      throughSequence: 3, sessionId: 'invalid-renderer-checkpoint'
    })

    port.receive({
      type: 'terminal.checkpoint', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'invalid-renderer-checkpoint', throughSequence: 2, screenEpoch: 0,
      snapshot: 'current'
    })
    await waitUntil(() => port.last('terminal.checkpoint-stored') !== undefined)
    const errorsBeforeBackwardsAttempt = port.sent.filter(
      (message) => message.type === 'terminal.checkpoint-rejected'
    ).length
    port.receive({
      type: 'terminal.checkpoint', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'invalid-renderer-checkpoint', throughSequence: 1, screenEpoch: 0,
      snapshot: 'stale'
    })
    await waitUntil(() => port.sent.filter(
      (message) => message.type === 'terminal.checkpoint-rejected'
    ).length > errorsBeforeBackwardsAttempt)
    expect(port.last('terminal.checkpoint-rejected')).toMatchObject({
      throughSequence: 1, sessionId: 'invalid-renderer-checkpoint'
    })
  })

  it('limits a checkpoint-free instant replay to the latest 10,000 indexed lines', async () => {
    registerSession(database, 'tail-limited-replay')
    const journal = await SegmentJournal.open(root, 'tail-limited-replay')
    for (let sequence = 1; sequence <= 10_001; sequence += 1) {
      await journal.appendOutput(sequence, new TextEncoder().encode(`line-${sequence}\n`))
    }
    await journal.close()

    port.receive({
      type: 'terminal.replay-request', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'tail-limited-replay', fromSequence: 0
    })
    await waitUntil(() => port.last('terminal.replay-complete') !== undefined, 10_000)

    expect(port.last('terminal.replay-start')).toMatchObject({
      source: 'tail', fromSequence: 2, throughSequence: 10_001,
      instantLineLimit: 10_000
    })
    const output = port.sent.filter((message) => message.type === 'terminal.data')
    expect(output).toHaveLength(10_000)
    expect(output.at(0)).toMatchObject({ sequence: 2 })
    expect(output.at(-1)).toMatchObject({ sequence: 10_001 })
  })

  it('keeps resize and reset frames before the first output in a checkpoint-free replay', async () => {
    registerSession(database, 'non-output-prefix-replay')
    const journal = await SegmentJournal.open(root, 'non-output-prefix-replay')
    await journal.appendResize(1, 100, 35)
    await journal.appendReset(2, 4)
    await journal.appendOutput(3, new TextEncoder().encode('visible'))
    await journal.close()

    port.receive({
      type: 'terminal.replay-request', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'non-output-prefix-replay', fromSequence: 0
    })
    await settle()

    expect(port.last('terminal.replay-start')).toMatchObject({
      source: 'tail', fromSequence: 1
    })
    expect(port.last('terminal.replay-resize')).toMatchObject({ sequence: 1, cols: 100, rows: 35 })
    expect(port.last('terminal.replay-reset')).toMatchObject({ sequence: 2, screenEpoch: 4 })
    expect(port.last('terminal.data')).toMatchObject({ sequence: 3 })
  })

  it('falls back to an older checkpoint without changing recovery sources in read-only mode', async () => {
    registerSession(database, 'readonly-corrupt-checkpoint')
    const journal = await SegmentJournal.open(root, 'readonly-corrupt-checkpoint')
    await journal.appendOutput(1, Uint8Array.from([65]))
    await journal.appendDomainCursor(2, 1)
    await journal.appendOutput(3, Uint8Array.from([66]))
    await journal.appendDomainCursor(4, 2)
    await journal.appendOutput(5, Uint8Array.from([67]))
    await journal.close()
    const checkpoints = new CheckpointManager(root, database)
    const older = await checkpoints.create({
      sessionId: 'readonly-corrupt-checkpoint', terminalSequence: 2, domainEventSequence: 1,
      screenEpoch: 3, snapshot: Uint8Array.from([1, 2, 3])
    })
    const newest = await checkpoints.create({
      sessionId: 'readonly-corrupt-checkpoint', terminalSequence: 4, domainEventSequence: 2,
      screenEpoch: 4, snapshot: Uint8Array.from([4, 5, 6])
    })
    const corrupted = await readFile(newest.filePath)
    corrupted[corrupted.byteLength - 1] = corrupted[corrupted.byteLength - 1]! ^ 0xff
    await writeFile(newest.filePath, corrupted)

    server.close()
    const databasePath = database.path
    database.close()
    const recoveryPaths = [
      databasePath, `${databasePath}-wal`, `${databasePath}-shm`, journal.path,
      older.filePath, newest.filePath
    ]
    const before = await readRecoverySourceBytes(recoveryPaths)
    await chmod(root, 0o500)
    expect((await stat(root)).mode & 0o777).toBe(0o500)
    try {
      database = RuntimeDatabase.openReadOnly(databasePath)
      port = new MockPort()
      server = new RuntimeServer(port, root, database)
      port.receive({ type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'readonly-corrupt' })
      port.receive({
        type: 'terminal.replay-request', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'readonly-corrupt-checkpoint', fromSequence: 0
      })
      await waitUntil(() => port.last('terminal.replay-complete') !== undefined)

      expect(port.last('terminal.replay-start')).toMatchObject({
        checkpointSequence: 2,
        checkpoint: {
          terminalSequence: 2, domainEventSequence: 1, screenEpoch: 3,
          snapshot: Uint8Array.from([1, 2, 3])
        }
      })
      expect(port.sent.filter((message) => message.type === 'terminal.data')).toEqual([
        expect.objectContaining({ sequence: 3, data: Uint8Array.from([66]) }),
        expect.objectContaining({ sequence: 5, data: Uint8Array.from([67]) })
      ])
      expect(port.last('protocol.error')).toBeUndefined()
      expect(await readRecoverySourceBytes(recoveryPaths)).toEqual(before)
    } finally {
      await chmod(root, 0o700)
    }
  })

  it('falls back to the raw Journal without changing recovery sources when a read-only checkpoint is missing', async () => {
    registerSession(database, 'readonly-missing-checkpoint')
    const journal = await SegmentJournal.open(root, 'readonly-missing-checkpoint')
    await journal.appendOutput(1, Uint8Array.from([71]))
    await journal.appendResize(2, 120, 42)
    await journal.appendOutput(3, Uint8Array.from([72]))
    await journal.close()
    const missing = await new CheckpointManager(root, database).create({
      sessionId: 'readonly-missing-checkpoint', terminalSequence: 2, domainEventSequence: 0,
      screenEpoch: 5, snapshot: Uint8Array.from([9, 9, 9])
    })
    await unlink(missing.filePath)

    server.close()
    const databasePath = database.path
    database.close()
    const recoveryPaths = [
      databasePath, `${databasePath}-wal`, `${databasePath}-shm`, journal.path, missing.filePath
    ]
    const before = await readRecoverySourceBytes(recoveryPaths)
    await chmod(root, 0o500)
    expect((await stat(root)).mode & 0o777).toBe(0o500)
    try {
      database = RuntimeDatabase.openReadOnly(databasePath)
      port = new MockPort()
      server = new RuntimeServer(port, root, database)
      port.receive({ type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'readonly-missing' })
      port.receive({
        type: 'terminal.replay-request', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'readonly-missing-checkpoint', fromSequence: 0
      })
      await waitUntil(() => port.last('terminal.replay-complete') !== undefined)

      expect(port.last('terminal.replay-start')).toMatchObject({
        sessionId: 'readonly-missing-checkpoint', availableFromSequence: 1, liveSequence: 3
      })
      expect(port.last('terminal.replay-start')).not.toHaveProperty('checkpoint')
      expect(port.sent.filter((message) => message.type === 'terminal.data')).toEqual([
        expect.objectContaining({ sequence: 1, data: Uint8Array.from([71]) }),
        expect.objectContaining({ sequence: 3, data: Uint8Array.from([72]) })
      ])
      expect(port.last('terminal.replay-resize')).toMatchObject({ sequence: 2, cols: 120, rows: 42 })
      expect(port.last('protocol.error')).toBeUndefined()
      expect(await readRecoverySourceBytes(recoveryPaths)).toEqual(before)
    } finally {
      await chmod(root, 0o700)
    }
  })

  it('returns projection responses with runtime generation protection', async () => {
    port.receive({
      type: 'rpc.request', protocolVersion: PROTOCOL_VERSION, requestId: 'request-1',
      method: 'projection.snapshot', capability: 'renderer', deadlineAt: Date.now() + 1000,
      payload: {}
    })
    await settle()

    expect(port.last('rpc.response')).toMatchObject({
      requestId: 'request-1', runtimeGeneration: database.runtimeGeneration,
      result: {
        eventSequence: 0,
        hierarchy: {
          windowId: 'window-1',
          workspaces: [expect.objectContaining({ id: 'replay-workspace' })],
          tasks: [expect.objectContaining({ id: 'replay-task' })]
        }
      }
    })
  })

  it('rejects expired and pre-cancelled requests with structured errors', async () => {
    port.receive({
      type: 'rpc.request', protocolVersion: PROTOCOL_VERSION, requestId: 'expired',
      method: 'projection.snapshot', capability: 'renderer', deadlineAt: 1, payload: {}
    })
    port.receive({ type: 'rpc.cancel', protocolVersion: PROTOCOL_VERSION, requestId: 'cancelled' })
    port.receive({
      type: 'rpc.request', protocolVersion: PROTOCOL_VERSION, requestId: 'cancelled',
      method: 'projection.snapshot', capability: 'renderer', deadlineAt: Date.now() + 1000,
      payload: {}
    })
    await settle()

    expect(port.findRpcError('expired')).toMatchObject({ code: 'TIMEOUT', retryable: true })
    expect(port.findRpcError('cancelled')).toMatchObject({ code: 'CANCELLED', retryable: false })
  })

  it('drops terminal callbacks already queued when the Renderer port disconnects', async () => {
    const errorsBefore = port.sent.filter(({ type }) => type === 'protocol.error').length
    port.disconnect()
    port.receive({
      type: 'terminal.ack', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'late-renderer-frame', throughSequence: 1
    })
    await settle()

    expect(port.sent.filter(({ type }) => type === 'protocol.error')).toHaveLength(errorsBefore)
  })

  it('treats an ACK for a just-disposed attached PTY as a harmless shutdown callback', async () => {
    const sessions = new RuntimeSessionRegistry()
    const shutdownPort = new MockPort()
    new RuntimeServer(shutdownPort, root, database, undefined, undefined, sessions)
    shutdownPort.receive({
      type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'shutdown-renderer'
    })
    shutdownPort.receive({
      type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'shutdown-session', executionContextId: 'local-default',
      profile: 'shell', cols: 80, rows: 24
    })
    await waitUntil(() => sessions.has('shutdown-session'))
    const errorsBefore = shutdownPort.sent.filter(({ type }) => type === 'protocol.error').length

    sessions.disposeAll()
    shutdownPort.receive({
      type: 'terminal.ack', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'shutdown-session', throughSequence: 1
    })
    shutdownPort.receive({
      type: 'terminal.resize', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'shutdown-session', resizeId: 1, cols: 100, rows: 30
    })
    await settle()

    expect(shutdownPort.sent.filter(({ type }) => type === 'protocol.error')).toHaveLength(errorsBefore)
    shutdownPort.disconnect()
  })

  it('acknowledges a resize only after the attached PTY has applied its exact offer', async () => {
    registerSession(database, 'resize-ack-session')
    port.receive({
      type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'resize-ack-session', executionContextId: 'replay-context',
      profile: 'shell', cols: 80, rows: 24
    })
    await waitUntil(() => port.last('terminal.spawned')?.sessionId === 'resize-ack-session')

    port.receive({
      type: 'terminal.resize', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'resize-ack-session', resizeId: 9, cols: 111, rows: 37
    })
    await waitUntil(() => port.last('terminal.resized')?.resizeId === 9)

    expect(port.last('terminal.resized')).toMatchObject({
      sessionId: 'resize-ack-session', resizeId: 9, cols: 111, rows: 37
    })
  })

  it('pushes replayable domain batches directly to a subscribed Renderer', async () => {
    port.receive({
      type: 'events.subscribe', protocolVersion: PROTOCOL_VERSION,
      consumerId: 'renderer-1', afterSequence: 0, batchSize: 100
    })
    port.receive({
      type: 'rpc.request', protocolVersion: PROTOCOL_VERSION, requestId: 'create-workspace',
      method: 'workspace.create', capability: 'renderer', deadlineAt: Date.now() + 1000,
      payload: {
        command: { commandId: 'cmd-workspace', commandType: 'workspace.create', requestHash: 'hash' },
        input: { id: 'workspace-1', name: 'Workspace', rootDirectory: '/tmp/workspace', now: 1 }
      }
    })
    await settle()

    expect(port.last('events.batch')).toMatchObject({
      consumerId: 'renderer-1', throughSequence: 1,
      events: [{ eventType: 'workspace.created' }]
    })
  })

  it('flushes a provider notification to an already subscribed Renderer without waiting for another RPC', async () => {
    port.receive({
      type: 'events.subscribe', protocolVersion: PROTOCOL_VERSION,
      consumerId: 'notification-renderer', afterSequence: 0, batchSize: 100
    })
    const transactions = new DomainTransactionManager(database)
    new AgentNotificationRepository(database, transactions).publish(
      { commandId: 'publish-notification', commandType: 'agent.notification.publish', requestHash: 'notification' },
      {
        eventId: 'provider-event', runId: 'run-1', sessionId: 'persisted-session', provider: 'claude-code',
        event: { eventType: 'completed', title: 'Claude Code', subtitle: 'Completed', body: '完成', sound: true, cooldownKey: 'Stop' },
        now: 10
      }
    )

    server.flushSemanticEvents()

    expect(port.last('events.batch')).toMatchObject({
      consumerId: 'notification-renderer', events: [{ eventType: 'agent.notification' }]
    })
  })

  it('binds a persisted Session to a Runtime generation SessionRun and closes it on PTY exit', async () => {
    registerSession(database, 'run-session')
    port.receive({
      type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION, sessionId: 'run-session',
      executionContextId: 'replay-context', profile: 'shell', cols: 80, rows: 24
    })
    await waitUntil(() => database.get<{ status: string }>(
      'SELECT status FROM session_runs WHERE session_id = ?', 'run-session'
    )?.status === 'running')
    const running = database.get<{ id: string; status: string; runtime_generation: string }>(
      'SELECT id, status, runtime_generation FROM session_runs WHERE session_id = ?',
      'run-session'
    )
    expect(running).toMatchObject({ status: 'running', runtime_generation: database.runtimeGeneration })

    port.receive({
      type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION, sessionId: 'run-session'
    })
    await waitUntil(() => database.get<{ status: string }>(
      'SELECT status FROM session_runs WHERE id = ?', running!.id
    )?.status === 'exited')
    expect(database.get('SELECT status FROM session_runs WHERE id = ?', running!.id)).toEqual({
      status: 'exited'
    })
  })

  it('persists an attached terminal interaction marker before its input is processed', async () => {
    registerCanvasSession(database, 'interaction-session')
    port.receive({
      type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'interaction-session', executionContextId: 'replay-context',
      profile: 'shell', cols: 80, rows: 24
    })
    await waitUntil(() => port.last('terminal.spawned') !== undefined)

    port.receive({
      type: 'terminal.user-interaction', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'interaction-session', interactionKind: 'submit'
    })
    port.receive({
      type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'interaction-session', data: "printf '__ORDERED_INPUT__\\n'\r"
    })

    await waitUntil(() => database.get<{ last_user_interaction_seq: number }>(
      `SELECT last_user_interaction_seq FROM session_canvas_memberships
       WHERE session_id = 'interaction-session'`
    )?.last_user_interaction_seq === 1)
    expect(database.get<{ event_type: string }>(
      `SELECT event_type FROM domain_events
       WHERE session_id = 'interaction-session' AND event_type = 'session.user-interacted'`
    )).toEqual({ event_type: 'session.user-interacted' })
    await waitUntil(() => terminalText(port).includes('__ORDERED_INPUT__'))
  })

  it('tracks real Shell commands as running, idle, error, and interrupted work', async () => {
    const previousShell = process.env.SHELL
    process.env.SHELL = '/bin/zsh'
    try {
      registerCanvasSession(database, 'work-status-session')
      port.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'work-status-session', executionContextId: 'replay-context',
        profile: 'shell', cols: 80, rows: 24
      })
      await waitUntil(() => port.last('terminal.spawned') !== undefined)

      port.receive({
        type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'work-status-session', data: 'sleep 0.35\r'
      })
      await waitUntil(() => workStatus('work-status-session') === 'running')
      await waitUntil(() => workStatus('work-status-session') === 'idle', 5_000)

      port.receive({
        type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'work-status-session', data: 'false\r'
      })
      await waitUntil(() => workStatus('work-status-session') === 'error')

      port.receive({
        type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'work-status-session', data: 'sleep 5\r'
      })
      await waitUntil(() => workStatus('work-status-session') === 'running')
      port.receive({
        type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'work-status-session', data: '\u0003'
      })
      await waitUntil(() => workStatus('work-status-session') === 'interrupted', 5_000)

      port.receive({
        type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'work-status-session',
        data: "printf 'enter value: '; read -r value; printf 'VALUE:%s\\n' \"$value\"\r"
      })
      await waitUntil(() => terminalText(port).includes('enter value:'), 5_000)
      await waitUntil(() => workStatus('work-status-session') === 'needs-input', 5_000)
      port.receive({
        type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'work-status-session', data: 'confirmed\r'
      })
      await waitUntil(() => terminalText(port).includes('VALUE:confirmed'), 5_000)
      await waitUntil(() => workStatus('work-status-session') === 'idle', 5_000)

      port.receive({
        type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'work-status-session',
        data: 'read "value?STA008_WAIT> "; printf \'STA008_GOT:%s\\n\' "$value"\r'
      })
      await waitUntil(() => terminalText(port).includes('STA008_WAIT>'), 5_000)
      await waitUntil(() => workStatus('work-status-session') === 'needs-input', 5_000)
      port.receive({
        type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'work-status-session', data: 'zsh-confirmed\r'
      })
      await waitUntil(() => terminalText(port).includes('STA008_GOT:zsh-confirmed'), 5_000)
      await waitUntil(() => workStatus('work-status-session') === 'idle', 5_000)
    } finally {
      port.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'work-status-session'
      })
      await settle()
      restoreEnv('SHELL', previousShell)
    }
  })

  it('keeps a failed Claude round on the same Session and retries its last submitted prompt', async () => {
    const executable = join(root, 'provider-retry-round.sh')
    const inputFile = join(root, 'provider-retry-inputs.txt')
    await writeFile(executable, [
      '#!/bin/sh',
      'while IFS= read -r line; do',
      '  printf "%s\\n" "$line" >> "$MATOU_PROVIDER_INPUT_FILE"',
      '  count=$(wc -l < "$MATOU_PROVIDER_INPUT_FILE" | tr -d " ")',
      '  if [ "$count" = "1" ]; then',
      '    printf "✻ Connection refused — a firewall or proxy may be blocking it (ConnectionRefused) · Retrying in 34s · attempt 10/10\\r\\n"',
      '  else',
      '    printf "STA007_RECOVERED\\r\\n"',
      '  fi',
      'done'
    ].join('\n'))
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    const previousInputFile = process.env.MATOU_PROVIDER_INPUT_FILE
    process.env.MATOU_CLAUDE_COMMAND = executable
    process.env.MATOU_PROVIDER_INPUT_FILE = inputFile
    try {
      registerCanvasSession(database, 'provider-round-retry', 'claude-code')
      port.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-round-retry', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })
      await waitUntil(() => port.last('terminal.spawned') !== undefined)

      port.receive({
        type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-round-retry', data: 'Reply exactly STA007_RECOVERED\r'
      })
      await waitUntil(() => workStatus('provider-round-retry') === 'error', 5_000)

      port.receive({
        type: 'terminal.retry-last-input', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-round-retry'
      })
      await waitUntilAsync(async () => (
        await readFile(inputFile, 'utf8').catch(() => '')
      ).trim().split('\n').length === 2, 5_000)
      expect((await readFile(inputFile, 'utf8')).trim().split('\n')).toEqual([
        'Reply exactly STA007_RECOVERED', 'Reply exactly STA007_RECOVERED'
      ])
      expect(workStatus('provider-round-retry')).toBe('running')
      expect(terminalText(port)).toContain('STA007_RECOVERED')
    } finally {
      port.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-round-retry'
      })
      await settle()
      restoreEnv('MATOU_CLAUDE_COMMAND', previousCommand)
      restoreEnv('MATOU_PROVIDER_INPUT_FILE', previousInputFile)
    }
  })

  it('keeps a live PTY in the Runtime registry across Renderer disconnect and reattach', async () => {
    const priorRun = await SegmentJournal.open(root, 'reload-session')
    await priorRun.appendOutput(1, new TextEncoder().encode('output from an earlier app run'))
    await priorRun.close()
    const sessions = new RuntimeSessionRegistry()
    const firstPort = new MockPort()
    new RuntimeServer(firstPort, root, database, undefined, undefined, sessions)
    firstPort.receive({ type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'reload-1' })
    firstPort.receive({
      type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION, sessionId: 'reload-session',
      executionContextId: 'local-default', profile: 'shell', cols: 80, rows: 24
    })
    await settle()
    const firstPid = firstPort.last('terminal.spawned')?.pid
    expect(firstPid).toBeTypeOf('number')
    firstPort.disconnect()

    const secondPort = new MockPort()
    new RuntimeServer(secondPort, root, database, undefined, undefined, sessions)
    secondPort.receive({ type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'reload-2' })
    secondPort.receive({
      type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION, sessionId: 'reload-session',
      executionContextId: 'local-default', profile: 'shell', cols: 80, rows: 24
    })
    await settle()

    expect(secondPort.last('terminal.spawned')).toMatchObject({
      pid: firstPid, reattached: true, replayFromSequence: 2
    })
    secondPort.receive({
      type: 'terminal.replay-request', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'reload-session', fromSequence: 2
    })
    await waitUntil(() => secondPort.last('terminal.replay-complete') !== undefined)
    secondPort.receive({
      type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'reload-session', data: "printf '__LIVE_AFTER_REPLAY__\\n'\r"
    })
    await waitUntil(() => secondPort.sent.some((message) =>
      message.type === 'terminal.data' &&
      new TextDecoder().decode(message.data).includes('__LIVE_AFTER_REPLAY__')
    ))
    secondPort.receive({
      type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION, sessionId: 'reload-session'
    })
    await settle()
  })

  it('starts queued recovery without a view, then attaches and detaches the active card independently', async () => {
    const sessions = new RuntimeSessionRegistry()
    registerSession(database, 'layered-session')
    const executable = join(root, 'layered-shell.sh')
    await writeFile(executable, '#!/bin/sh\nprintf "ready\\n"\nsleep 30\n')
    await chmod(executable, 0o755)
    const previousShell = process.env.SHELL
    process.env.SHELL = executable
    server.close()
    port = new MockPort()
    server = new RuntimeServer(port, root, database, undefined, undefined, sessions)
    port.receive({ type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'layered' })
    await settle()

    try {
      await server.ensureSessionRunning({
        sessionId: 'layered-session', sceneId: 'scene-layered',
        priority: 'active-session', enqueueSequence: 1,
        executionContextId: 'replay-context', profile: 'shell'
      })

      expect(sessions.size).toBe(1)
      expect(port.sent.some((message) =>
        message.type === 'terminal.spawned' && message.sessionId === 'layered-session')).toBe(false)

      port.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'layered-session', executionContextId: 'replay-context',
        profile: 'shell', cols: 80, rows: 24
      })
      await waitUntil(() => port.last('terminal.spawned')?.sessionId === 'layered-session')
      expect(port.last('terminal.spawned')).toMatchObject({ reattached: true })

      port.receive({
        type: 'terminal.view-detach', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'layered-session'
      })
      await settle()
      expect(sessions.size).toBe(1)
      port.receive({
        type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'layered-session', data: 'detached input'
      })
      await settle()
      expect(port.sent.some((message) =>
        message.type === 'protocol.error' &&
        message.code === 'SESSION_FORBIDDEN' &&
        message.sessionId === 'layered-session'
      )).toBe(true)
    } finally {
      restoreEnv('SHELL', previousShell)
    }
  })

  it('serializes duplicate attach requests so one persisted Session owns one live PTY', async () => {
    registerSession(database, 'duplicate-spawn-session')
    const message = {
      type: 'terminal.spawn' as const,
      protocolVersion: PROTOCOL_VERSION,
      sessionId: 'duplicate-spawn-session',
      executionContextId: 'replay-context',
      profile: 'shell' as const,
      cols: 80,
      rows: 24
    }

    port.receive(message)
    port.receive(message)
    await waitUntil(() => port.sent.filter(({ type }) => type === 'terminal.spawned').length >= 2)

    const pids = port.sent
      .filter((candidate) => candidate.type === 'terminal.spawned')
      .map((candidate) => candidate.pid)
    expect(new Set(pids).size).toBe(1)
    expect(database.all(
      'SELECT id FROM session_runs WHERE session_id = ?',
      'duplicate-spawn-session'
    )).toHaveLength(1)
    port.receive({
      type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'duplicate-spawn-session'
    })
    await settle()
  })

  it('replaces a live PTY when the same stable Session requests a newer spawn revision', async () => {
    registerSession(database, 'revised-spawn-session')
    const message = {
      type: 'terminal.spawn' as const,
      protocolVersion: PROTOCOL_VERSION,
      sessionId: 'revised-spawn-session',
      executionContextId: 'replay-context',
      profile: 'shell' as const,
      cols: 80,
      rows: 24,
      spawnRevision: 1
    }

    port.receive(message)
    await waitUntil(() => port.sent.filter(({ type }) => type === 'terminal.spawned').length === 1)
    const firstPid = port.last('terminal.spawned')?.pid

    port.receive({ ...message, spawnRevision: 2 })
    await waitUntil(() => port.sent.filter(({ type }) => type === 'terminal.spawned').length === 2)
    const secondPid = port.last('terminal.spawned')?.pid

    expect(secondPid).not.toBe(firstPid)
    port.receive({
      type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'revised-spawn-session'
    })
    await settle()
  })

  it('rejects input for an invalid Workspace while keeping the PTY alive', async () => {
    const sessions = new RuntimeSessionRegistry()
    const guardedPort = new MockPort()
    new RuntimeServer(guardedPort, root, database, undefined, undefined, sessions)
    guardedPort.receive({
      type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'guarded-renderer'
    })
    registerSession(database, 'guarded-session')
    guardedPort.receive({
      type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'guarded-session', executionContextId: 'replay-context',
      profile: 'shell', cols: 80, rows: 24
    })
    await waitUntil(() => sessions.has('guarded-session'))
    const pid = sessions.get('guarded-session')!.pid
    database.run(
      `INSERT INTO workspace_path_state (
         workspace_id, status, reason, checked_at, validation_generation
       ) VALUES ('replay-workspace', 'invalid', 'missing', ?, 1)
       ON CONFLICT(workspace_id) DO UPDATE SET
         status = 'invalid', reason = 'missing', checked_at = excluded.checked_at`,
      Date.now()
    )

    guardedPort.receive({
      type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'guarded-session', data: 'pwd\r'
    })
    await settle()

    expect(guardedPort.last('protocol.error')).toMatchObject({
      code: 'WORKSPACE_PATH_INVALID',
      message: '工作区目录不可用，请先在本地恢复原路径，或移出该工作区'
    })
    expect(sessions.get('guarded-session')?.pid).toBe(pid)
    guardedPort.receive({
      type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'guarded-session'
    })
    await settle()
  })

  it('records each Shell working directory after a completed command', async () => {
    const sessions = new RuntimeSessionRegistry()
    const cwdPort = new MockPort()
    new RuntimeServer(cwdPort, root, database, undefined, undefined, sessions)
    cwdPort.receive({
      type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'cwd-renderer'
    })
    registerSession(database, 'cwd-session')
    const target = join(root, 'nested-working-directory')
    await mkdir(target)
    cwdPort.receive({
      type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'cwd-session', executionContextId: 'replay-context',
      profile: 'shell', cols: 80, rows: 24
    })
    await waitUntil(() => sessions.has('cwd-session'))
    await new Promise((resolve) => setTimeout(resolve, 200))

    cwdPort.receive({
      type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'cwd-session', data: `cd ${JSON.stringify(target)}\r`
    })

    await waitUntil(() => database.get<{ cwd: string }>(
      'SELECT cwd FROM sessions WHERE id = ?', 'cwd-session'
    )?.cwd.endsWith('/nested-working-directory') === true, 5_000)
    expect(database.get<{ cwd: string }>('SELECT cwd FROM sessions WHERE id = ?', 'cwd-session')?.cwd)
      .toMatch(/\/nested-working-directory$/)
    cwdPort.receive({
      type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION, sessionId: 'cwd-session'
    })
    await settle()
  }, 10_000)

  it('changes Workspace and Task order only after submitted terminal input', async () => {
    const sessions = new RuntimeSessionRegistry()
    const interactionPort = new MockPort()
    new RuntimeServer(interactionPort, root, database, undefined, undefined, sessions)
    interactionPort.receive({
      type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'recency-renderer'
    })
    registerCanvasSession(database, 'recency-session')
    interactionPort.receive({
      type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'recency-session', executionContextId: 'replay-context',
      profile: 'shell', cols: 80, rows: 24
    })
    await waitUntil(() => sessions.has('recency-session'))
    const priorLastOpenedAt = database.get<{ last_opened_at: number }>(
      'SELECT last_opened_at FROM tasks WHERE id = ?', 'replay-task'
    )?.last_opened_at

    interactionPort.receive({
      type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'recency-session', data: 'echo recency'
    })
    await new Promise((resolve) => setTimeout(resolve, 50))
    expect(database.get<{ last_opened_at: number }>(
      'SELECT last_opened_at FROM tasks WHERE id = ?', 'replay-task'
    )?.last_opened_at).toBe(priorLastOpenedAt)
    interactionPort.receive({
      type: 'terminal.user-interaction', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'recency-session', interactionKind: 'submit'
    })
    interactionPort.receive({
      type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'recency-session', data: '\r'
    })

    await waitUntil(() => (database.get<{ last_opened_at: number }>(
      'SELECT last_opened_at FROM tasks WHERE id = ?', 'replay-task'
    )?.last_opened_at ?? 0) > 1)
    const taskLastOpenedAt = database.get<{ last_opened_at: number }>(
      'SELECT last_opened_at FROM tasks WHERE id = ?', 'replay-task'
    )?.last_opened_at
    expect(database.get<{ last_opened_at: number }>(
      'SELECT last_opened_at FROM workspaces WHERE id = ?', 'replay-workspace'
    )?.last_opened_at).toBe(taskLastOpenedAt)
  })

  it('publishes the Shell HUD after a chained relative cd command', async () => {
    const sessions = new RuntimeSessionRegistry()
    const cwdPort = new MockPort()
    new RuntimeServer(cwdPort, root, database, undefined, undefined, sessions)
    cwdPort.receive({
      type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'cwd-hud-renderer'
    })
    registerSession(database, 'cwd-hud-session')
    cwdPort.receive({
      type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'cwd-hud-session', executionContextId: 'replay-context',
      profile: 'shell', cols: 80, rows: 24
    })
    await waitUntil(() => sessions.has('cwd-hud-session'))
    await new Promise((resolve) => setTimeout(resolve, 200))

    cwdPort.receive({
      type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'cwd-hud-session', data: 'mkdir -p outside && cd outside\r'
    })

    await waitUntil(() => cwdPort.last('terminal.hud')?.hud?.cwd?.endsWith('/outside') === true, 5_000)
    expect(cwdPort.last('terminal.hud')).toMatchObject({
      sessionId: 'cwd-hud-session', hud: { mode: 'shell', cwd: join(root, 'outside') }
    })
    cwdPort.receive({
      type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION, sessionId: 'cwd-hud-session'
    })
    await settle()
  }, 10_000)

  it('starts a restored Shell in that Session own last working directory', async () => {
    const sessions = new RuntimeSessionRegistry()
    const restorePort = new MockPort()
    new RuntimeServer(restorePort, root, database, undefined, undefined, sessions)
    restorePort.receive({
      type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'cwd-restore-renderer'
    })
    registerSession(database, 'cwd-restore-session')
    const target = join(root, 'restored-working-directory')
    await mkdir(target)
    database.run('UPDATE sessions SET cwd = ? WHERE id = ?', target, 'cwd-restore-session')

    restorePort.receive({
      type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'cwd-restore-session', executionContextId: 'replay-context',
      profile: 'shell', cols: 80, rows: 24
    })
    await waitUntil(() => sessions.has('cwd-restore-session'))

    expect(await childProcessCwd(sessions.get('cwd-restore-session')!.pid))
      .toMatch(/\/restored-working-directory$/)
    restorePort.receive({
      type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
      sessionId: 'cwd-restore-session'
    })
    await settle()
  })

  it('launches an AI panel with its validated resume identity and permission mode', async () => {
    const executable = join(root, 'provider-fixture.sh')
    const argumentFile = join(root, 'provider-arguments.txt')
    await writeFile(
      executable,
      '#!/bin/sh\nprintf "%s\\n" "$@" > "$MATOU_TEST_ARGUMENT_FILE"\nsleep 30\n'
    )
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    const previousArgumentFile = process.env.MATOU_TEST_ARGUMENT_FILE
    process.env.MATOU_CLAUDE_COMMAND = executable
    process.env.MATOU_TEST_ARGUMENT_FILE = argumentFile
    try {
      registerSession(database, 'provider-resume-session', 'claude-code')
      database.run(
        `INSERT INTO provider_bindings (
           id, session_id, provider, provider_session_id, resume_state, metadata_json,
           created_at, updated_at, validated_at
        ) VALUES (?, ?, 'claude-code', ?, 'available', ?, 1, 1, 1)`,
        'binding-resume', 'provider-resume-session', 'provider-session-42',
        JSON.stringify({ permissionMode: 'default' })
      )
      const sessions = new RuntimeSessionRegistry()
      const providerPort = new MockPort()
      new RuntimeServer(providerPort, root, database, undefined, undefined, sessions)
      providerPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION,
        clientId: 'provider-resume-renderer'
      })
      providerPort.receive({
        type: 'rpc.request', protocolVersion: PROTOCOL_VERSION,
        requestId: 'permission-bypass', method: 'session.set-permission-mode',
        capability: 'renderer', deadlineAt: Date.now() + 1000,
        payload: {
          command: {
            commandId: 'permission-bypass', commandType: 'session.set-permission-mode',
            requestHash: 'hash-permission-bypass'
          },
          input: {
            sessionId: 'provider-resume-session', provider: 'claude-code',
            permissionMode: 'bypassPermissions', now: 2
          }
        }
      })
      await waitUntil(() => providerPort.last('rpc.response')?.requestId === 'permission-bypass')
      providerPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-resume-session', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })

      await waitUntilAsync(async () => (await readFile(argumentFile, 'utf8').catch(() => '')) !== '')
      expect((await readFile(argumentFile, 'utf8')).trim().split('\n')).toEqual([
        '--resume', 'provider-session-42', '--dangerously-skip-permissions'
      ])
      providerPort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-resume-session'
      })
      await settle()
    } finally {
      if (previousCommand === undefined) delete process.env.MATOU_CLAUDE_COMMAND
      else process.env.MATOU_CLAUDE_COMMAND = previousCommand
      if (previousArgumentFile === undefined) delete process.env.MATOU_TEST_ARGUMENT_FILE
      else process.env.MATOU_TEST_ARGUMENT_FILE = previousArgumentFile
    }
  })

  it('keeps a quiet restored Claude conversation running when statusline confirms its launch', async () => {
    const executable = join(root, 'provider-quiet-resume-fixture.sh')
    const argumentFile = join(root, 'provider-quiet-resume-arguments.txt')
    await writeFile(executable, '#!/bin/sh\nprintf "%s\\n" "$@" > "$MATOU_TEST_ARGUMENT_FILE"\nsleep 30\n')
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    const previousArgumentFile = process.env.MATOU_TEST_ARGUMENT_FILE
    process.env.MATOU_CLAUDE_COMMAND = executable
    process.env.MATOU_TEST_ARGUMENT_FILE = argumentFile
    const repository = new SessionRepository(database, new DomainTransactionManager(database))
    let resumeServer: RuntimeServer | undefined
    const providerHooks = new ProviderHookServer(root, repository, {
      onIdentityRecorded: ({ sessionId, runId }) => {
        resumeServer?.providerIdentityRecorded(sessionId, runId)
        void resumeServer?.refreshSessionHud(sessionId)
      }
    })
    await providerHooks.start()
    try {
      registerSession(database, 'quiet-resume-session', 'claude-code')
      database.run(
        `INSERT INTO provider_bindings (
           id, session_id, provider, provider_session_id, resume_state, metadata_json,
           created_at, updated_at, validated_at
         ) VALUES (?, ?, 'claude-code', ?, 'available', '{}', 1, 1, 1)`,
        'binding-quiet-resume', 'quiet-resume-session', 'provider-quiet-resume'
      )
      const sessions = new RuntimeSessionRegistry()
      const resumePort = new MockPort()
      resumeServer = new RuntimeServer(
        resumePort, root, database, undefined, undefined, sessions,
        providerHooks, undefined, { providerResumeTimeoutMs: 500 }
      )
      resumePort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'quiet-resume-renderer'
      })
      resumePort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'quiet-resume-session', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })

      await waitUntilAsync(async () => (await readFile(argumentFile, 'utf8').catch(() => '')) !== '')
      const arguments_ = (await readFile(argumentFile, 'utf8')).trim().split('\n')
      const settingsIndex = arguments_.indexOf('--settings')
      const settings = JSON.parse(await readFile(arguments_[settingsIndex + 1]!, 'utf8')) as {
        hooks: { Stop: Array<{ hooks: Array<{ url: string }> }> }
      }
      const hookUrl = settings.hooks.Stop[0]!.hooks[0]!.url
      expect((await fetch(hookUrl, {
        method: 'POST', headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ session_id: 'provider-quiet-resume', cwd: root })
      })).status).toBe(200)

      await new Promise((resolve) => setTimeout(resolve, 600))
      expect(sessions.get('quiet-resume-session')?.profile).toBe('claude-code')
      expect(repository.getResumeBinding('quiet-resume-session', 'claude-code')).toMatchObject({
        providerSessionId: 'provider-quiet-resume', resumeState: 'available'
      })

      resumePort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION, sessionId: 'quiet-resume-session'
      })
      await settle()
    } finally {
      resumeServer?.close()
      await providerHooks.stop()
      restoreEnv('MATOU_CLAUDE_COMMAND', previousCommand)
      restoreEnv('MATOU_TEST_ARGUMENT_FILE', previousArgumentFile)
    }
  })

  it('accepts a fresh Claude statusline immediately when it supersedes a stale restore failure', async () => {
    const executable = join(root, 'provider-replacement-fixture.sh')
    const argumentFile = join(root, 'provider-replacement-arguments.txt')
    await writeFile(executable, '#!/bin/sh\nprintf "%s\\n" "$@" > "$MATOU_TEST_ARGUMENT_FILE"\nsleep 30\n')
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    const previousArgumentFile = process.env.MATOU_TEST_ARGUMENT_FILE
    process.env.MATOU_CLAUDE_COMMAND = executable
    process.env.MATOU_TEST_ARGUMENT_FILE = argumentFile
    const repository = new SessionRepository(database, new DomainTransactionManager(database))
    let replacementServer: RuntimeServer | undefined
    let replacementPort: MockPort | undefined
    const providerHooks = new ProviderHookServer(root, repository, {
      onIdentityRecorded: ({ sessionId, runId }) => {
        replacementServer?.providerIdentityRecorded(sessionId, runId)
      }
    })
    await providerHooks.start()
    try {
      registerSession(database, 'provider-replacement-session', 'claude-code')
      database.run(
        `INSERT INTO provider_bindings (
           id, session_id, provider, provider_session_id, resume_state, restore_state,
           restore_error, metadata_json, created_at, updated_at, validated_at, invalidated_at
         ) VALUES ('binding-stale-failure', 'provider-replacement-session', 'claude-code',
                   'provider-missing', 'failed', 'failed', 'provider session not found',
                   '{}', 1, 1, 1, 1)`
      )
      const sessions = new RuntimeSessionRegistry()
      replacementPort = new MockPort()
      replacementServer = new RuntimeServer(
        replacementPort, root, database, undefined, undefined, sessions,
        providerHooks, undefined, { providerResumeTimeoutMs: 500 }
      )
      replacementPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION,
        clientId: 'provider-replacement-renderer'
      })
      database.run(
        `UPDATE provider_bindings
         SET resume_state = 'expired', restore_state = 'none', restore_error = NULL, updated_at = 2
         WHERE id = 'binding-stale-failure'`
      )
      replacementPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-replacement-session', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })

      await waitUntilAsync(async () => (await readFile(argumentFile, 'utf8').catch(() => '')) !== '')
      const arguments_ = (await readFile(argumentFile, 'utf8')).trim().split('\n')
      expect(arguments_).not.toContain('--resume')
      const settingsIndex = arguments_.indexOf('--settings')
      const settings = JSON.parse(await readFile(arguments_[settingsIndex + 1]!, 'utf8')) as {
        hooks: { Stop: Array<{ hooks: Array<{ url: string }> }> }
      }
      const hookUrl = settings.hooks.Stop[0]!.hooks[0]!.url
      expect((await fetch(hookUrl, {
        method: 'POST', headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ session_id: 'provider-replacement', cwd: root })
      })).status).toBe(200)

      await waitUntil(() => repository.getResumeBinding(
        'provider-replacement-session', 'claude-code'
      )?.providerSessionId === 'provider-replacement')
    } finally {
      replacementPort?.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-replacement-session'
      })
      await settle()
      replacementServer?.close()
      await providerHooks.stop()
      restoreEnv('MATOU_CLAUDE_COMMAND', previousCommand)
      restoreEnv('MATOU_TEST_ARGUMENT_FILE', previousArgumentFile)
    }
  })

  it('consumes a fork launch exactly once and passes Claude the reference product fork arguments', async () => {
    const executable = join(root, 'provider-fork-fixture.sh')
    const argumentFile = join(root, 'provider-fork-arguments.txt')
    await writeFile(executable, '#!/bin/sh\nprintf "%s\\n" "$@" > "$MATOU_TEST_ARGUMENT_FILE"\nsleep 30\n')
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    const previousArgumentFile = process.env.MATOU_TEST_ARGUMENT_FILE
    process.env.MATOU_CLAUDE_COMMAND = executable
    process.env.MATOU_TEST_ARGUMENT_FILE = argumentFile
    try {
      registerSession(database, 'fork-source', 'claude-code')
      registerSession(database, 'fork-derived', 'claude-code')
      database.run(
        `INSERT INTO session_fork_intents (
           session_id, source_session_id, source_provider, source_provider_session_id,
           state, created_at
         ) VALUES (?, ?, 'claude-code', ?, 'pending', 1)`,
        'fork-derived', 'fork-source', 'provider-source-42'
      )
      const sessions = new RuntimeSessionRegistry()
      const forkPort = new MockPort()
      const forkServer = new RuntimeServer(
        forkPort, root, database, undefined, undefined, sessions,
        undefined, undefined, { providerResumeTimeoutMs: 1_000 }
      )
      forkPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'fork-renderer'
      })
      forkPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'fork-derived', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })

      await waitUntilAsync(async () => (await readFile(argumentFile, 'utf8').catch(() => '')) !== '')
      expect((await readFile(argumentFile, 'utf8')).trim().split('\n')).toEqual([
        '--resume', 'provider-source-42', '--fork-session'
      ])
      expect(database.get(
        'SELECT state, started_at FROM session_fork_intents WHERE session_id = ?', 'fork-derived'
      )).toMatchObject({ state: 'starting', started_at: expect.any(Number) })

      new SessionRepository(database, new DomainTransactionManager(database))
        .recordResumableProviderIdentity({
          commandId: 'fork-derived-binding', commandType: 'provider-hook',
          requestHash: 'fork-derived-binding'
        }, {
          id: 'fork-derived-binding', sessionId: 'fork-derived', provider: 'claude-code',
          providerSessionId: 'provider-derived-43', metadata: {}, now: 2
        })
      await forkServer.refreshSessionHud('fork-derived')
      await new Promise((resolve) => setTimeout(resolve, 1_100))
      expect(sessions.has('fork-derived')).toBe(true)

      forkPort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION, sessionId: 'fork-derived'
      })
      await settle()
      forkServer.close()

      await writeFile(argumentFile, '')
      const restoredRegistry = new RuntimeSessionRegistry()
      const restoredPort = new MockPort()
      const restoredServer = new RuntimeServer(
        restoredPort, root, database, undefined, undefined, restoredRegistry
      )
      restoredPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'fork-restored-renderer'
      })
      restoredPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'fork-derived', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })
      await waitUntilAsync(async () => (await readFile(argumentFile, 'utf8').catch(() => '')) !== '')
      expect((await readFile(argumentFile, 'utf8')).trim().split('\n')).toEqual([
        '--resume', 'provider-derived-43'
      ])
      restoredPort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION, sessionId: 'fork-derived'
      })
      await settle()
      restoredServer.close()
    } finally {
      restoreEnv('MATOU_CLAUDE_COMMAND', previousCommand)
      restoreEnv('MATOU_TEST_ARGUMENT_FILE', previousArgumentFile)
    }
  })

  it('lets Renderer attach but not launch an in-flight durable Fork', async () => {
    const executable = join(root, 'provider-durable-renderer-fixture.sh')
    const argumentFile = join(root, 'provider-durable-renderer-arguments.txt')
    await writeFile(executable, [
      '#!/bin/sh',
      'printf "%s\\n" "$@" > "$MATOU_TEST_ARGUMENT_FILE"',
      'head -c 2101 /dev/zero | tr "\\0" x',
      'sleep 30'
    ].join('\n'))
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    const previousArgumentFile = process.env.MATOU_TEST_ARGUMENT_FILE
    process.env.MATOU_CLAUDE_COMMAND = executable
    process.env.MATOU_TEST_ARGUMENT_FILE = argumentFile
    const forkPort = new MockPort()
    const sessions = new RuntimeSessionRegistry()
    const forkServer = new RuntimeServer(
      forkPort, root, database, undefined, undefined, sessions,
      undefined, undefined, { providerResumeTimeoutMs: 25 }
    )
    try {
      registerSession(database, 'fork-durable-source', 'claude-code')
      registerSession(database, 'fork-durable-derived', 'claude-code')
      const intents = new SessionForkIntentRepository(database)
      const now = Date.now()
      intents.accept({
        operationId: 'operation-renderer-gate', submissionKey: 'submission-renderer-gate',
        sessionId: 'fork-durable-derived', sourceSessionId: 'fork-durable-source',
        sourceProviderSessionId: 'provider-source-durable', displayName: 'Derived',
        worktreeMode: 'current', totalSteps: 2, now
      })
      const decision = intents.acquireLease({
        operationId: 'operation-renderer-gate', owner: 'coordinator', now, ttlMs: 60_000
      })
      if (decision.kind !== 'acquired') throw new Error('durable lease missing')
      intents.advanceStage({
        operationId: 'operation-renderer-gate', lease: decision.lease,
        stage: 'restoring-provider', now
      })
      forkPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'fork-durable-renderer'
      })
      await settle()
      forkPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'fork-durable-derived', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24, spawnRevision: 99
      })

      await settle()
      await new Promise((resolve) => setTimeout(resolve, 100))
      expect(await readFile(argumentFile, 'utf8').catch(() => '')).toBe('')
      expect(sessions.has('fork-durable-derived')).toBe(false)
      expect(forkPort.sent.filter(({ type }) => type === 'terminal.spawned')).toHaveLength(0)
      expect(forkPort.last('protocol.error')).toBeUndefined()
      expect(intents.progressByOperation('operation-renderer-gate')).toMatchObject({
        stage: 'restoring-provider'
      })

      await expect(forkServer.startOrResumeSession({
        sessionId: 'fork-durable-derived', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      }, {
        operationId: 'operation-renderer-gate', runId: 'run-renderer-gate',
        lease: decision.lease
      })).resolves.toMatchObject({ kind: 'started' })
      await waitUntilAsync(async () => (await readFile(argumentFile, 'utf8').catch(() => '')) !== '')
      expect((await readFile(argumentFile, 'utf8')).trim().split('\n')).toEqual([
        '--resume', 'provider-source-durable', '--fork-session'
      ])
      expect(sessions.get('fork-durable-derived')?.runId).toBe('run-renderer-gate')
      await new Promise((resolve) => setTimeout(resolve, 75))
      expect(sessions.has('fork-durable-derived')).toBe(true)
      expect(intents.progressByOperation('operation-renderer-gate')).toMatchObject({
        stage: 'restoring-provider'
      })

      forkPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'fork-durable-derived', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24, spawnRevision: 100
      })
      await waitUntil(() => forkPort.sent.some((message) =>
        message.type === 'terminal.spawned' && message.reattached === true
      ))
      // The coordinator starts the provider without binding a presentation;
      // only the later Renderer attach publishes a visible spawn event.
      expect(forkPort.sent.filter(({ type }) => type === 'terminal.spawned')).toHaveLength(1)
      expect(sessions.get('fork-durable-derived')?.runId).toBe('run-renderer-gate')
      expect((await readFile(argumentFile, 'utf8')).trim().split('\n')).toEqual([
        '--resume', 'provider-source-durable', '--fork-session'
      ])
    } finally {
      forkServer.close()
      restoreEnv('MATOU_CLAUDE_COMMAND', previousCommand)
      restoreEnv('MATOU_TEST_ARGUMENT_FILE', previousArgumentFile)
    }
  })

  it('records a durable Fork provider exit through its fenced operation authority', async () => {
    const executable = join(root, 'provider-durable-exit-fixture.sh')
    await writeFile(executable, '#!/bin/sh\nexit 7\n')
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    process.env.MATOU_CLAUDE_COMMAND = executable
    const forkPort = new MockPort()
    const sessions = new RuntimeSessionRegistry()
    const forkServer = new RuntimeServer(
      forkPort, root, database, undefined, undefined, sessions
    )
    try {
      registerSession(database, 'fork-durable-exit-source', 'claude-code')
      registerSession(database, 'fork-durable-exit-derived', 'claude-code')
      const intents = new SessionForkIntentRepository(database)
      const now = Date.now()
      intents.accept({
        operationId: 'operation-durable-exit', submissionKey: 'submission-durable-exit',
        sessionId: 'fork-durable-exit-derived', sourceSessionId: 'fork-durable-exit-source',
        sourceProviderSessionId: 'provider-source-exit', displayName: 'Derived',
        worktreeMode: 'current', totalSteps: 2, now
      })
      const decision = intents.acquireLease({
        operationId: 'operation-durable-exit', owner: 'coordinator', now, ttlMs: 60_000
      })
      if (decision.kind !== 'acquired') throw new Error('durable lease missing')
      intents.advanceStage({
        operationId: 'operation-durable-exit', lease: decision.lease,
        stage: 'restoring-provider', now
      })

      await forkServer.startOrResumeSession({
        sessionId: 'fork-durable-exit-derived', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      }, {
        operationId: 'operation-durable-exit', runId: 'run-durable-exit',
        lease: decision.lease
      })

      await waitUntil(() => intents.progressByOperation('operation-durable-exit')?.stage === 'failed')
      expect(intents.progressByOperation('operation-durable-exit')).toMatchObject({
        stage: 'failed', error: expect.stringContaining('代码：7')
      })
      expect(sessions.has('fork-durable-exit-derived')).toBe(false)
    } finally {
      forkServer.close()
      restoreEnv('MATOU_CLAUDE_COMMAND', previousCommand)
    }
  })

  it('marks a quiet real-style Fork ready from its inherited statusline identity', async () => {
    const executable = join(root, 'provider-quiet-fork-fixture.sh')
    const argumentFile = join(root, 'provider-quiet-fork-arguments.txt')
    await writeFile(executable, '#!/bin/sh\nprintf "%s\\n" "$@" > "$MATOU_TEST_ARGUMENT_FILE"\nsleep 30\n')
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    const previousArgumentFile = process.env.MATOU_TEST_ARGUMENT_FILE
    process.env.MATOU_CLAUDE_COMMAND = executable
    process.env.MATOU_TEST_ARGUMENT_FILE = argumentFile
    const repository = new SessionRepository(database, new DomainTransactionManager(database))
    let forkServer: RuntimeServer | undefined
    const providerHooks = new ProviderHookServer(root, repository, {
      onIdentityRecorded: ({ sessionId, runId }) => {
        forkServer?.providerIdentityRecorded(sessionId, runId)
        void forkServer?.refreshSessionHud(sessionId)
      }
    })
    await providerHooks.start()
    try {
      registerSession(database, 'fork-quiet-source', 'claude-code')
      registerSession(database, 'fork-quiet-derived', 'claude-code')
      database.run(
        `INSERT INTO session_fork_intents (
           session_id, source_session_id, source_provider, source_provider_session_id,
           state, created_at
         ) VALUES (?, ?, 'claude-code', ?, 'pending', 1)`,
        'fork-quiet-derived', 'fork-quiet-source', 'provider-source-quiet'
      )
      const sessions = new RuntimeSessionRegistry()
      const forkPort = new MockPort()
      forkServer = new RuntimeServer(
        forkPort, root, database, undefined, undefined, sessions,
        providerHooks, undefined, { providerResumeTimeoutMs: 500 }
      )
      forkPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'fork-quiet-renderer'
      })
      forkPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'fork-quiet-derived', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })

      await waitUntilAsync(async () => (await readFile(argumentFile, 'utf8').catch(() => '')) !== '')
      const arguments_ = (await readFile(argumentFile, 'utf8')).trim().split('\n')
      const settingsIndex = arguments_.indexOf('--settings')
      expect(settingsIndex).toBeGreaterThanOrEqual(0)
      const settings = JSON.parse(await readFile(arguments_[settingsIndex + 1]!, 'utf8')) as {
        hooks: { Stop: Array<{ hooks: Array<{ url: string }> }> }
      }
      const hookUrl = settings.hooks.Stop[0]!.hooks[0]!.url
      expect((await fetch(hookUrl, {
        method: 'POST', headers: { 'content-type': 'application/json' },
        body: JSON.stringify({
          session_id: 'provider-derived-quiet', cwd: root,
          model: { display_name: 'Claude Opus 4.6' }
        })
      })).status).toBe(200)

      await new Promise((resolve) => setTimeout(resolve, 600))
      expect(sessions.has('fork-quiet-derived')).toBe(true)
      expect(database.get<{ state: string }>(
        'SELECT state FROM session_fork_intents WHERE session_id = ?', 'fork-quiet-derived'
      )).toEqual({ state: 'succeeded' })
      expect(repository.getResumeBinding('fork-quiet-derived', 'claude-code')).toMatchObject({
        providerSessionId: 'provider-derived-quiet', resumeState: 'available',
        metadata: expect.objectContaining({ inheritedConversation: true, canFork: true })
      })

      forkPort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION, sessionId: 'fork-quiet-derived'
      })
      await settle()
    } finally {
      forkServer?.close()
      await providerHooks.stop()
      restoreEnv('MATOU_CLAUDE_COMMAND', previousCommand)
      restoreEnv('MATOU_TEST_ARGUMENT_FILE', previousArgumentFile)
    }
  })

  it('keeps a failed fork panel inert and never falls back to Shell or repeats the fork', async () => {
    const executable = join(root, 'provider-fork-failure.sh')
    await writeFile(executable, '#!/bin/sh\nprintf "No session found for requested id\\n"\nsleep 30\n')
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    process.env.MATOU_CLAUDE_COMMAND = executable
    try {
      registerSession(database, 'fork-failure-source', 'claude-code')
      registerSession(database, 'fork-failure-derived', 'claude-code')
      database.run(
        `INSERT INTO session_fork_intents (
           session_id, source_session_id, source_provider, source_provider_session_id,
           state, created_at
         ) VALUES (?, ?, 'claude-code', ?, 'pending', 1)`,
        'fork-failure-derived', 'fork-failure-source', 'missing-provider-42'
      )
      const sessions = new RuntimeSessionRegistry()
      const forkPort = new MockPort()
      new RuntimeServer(forkPort, root, database, undefined, undefined, sessions)
      forkPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'fork-failure-renderer'
      })
      forkPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'fork-failure-derived', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })

      await waitUntil(() => database.get<{ state: string }>(
        'SELECT state FROM session_fork_intents WHERE session_id = ?', 'fork-failure-derived'
      )?.state === 'failed')
      await waitUntil(() => terminalText(forkPort).includes('[Fork 未完成，请检查上方原因后重试]'))
      expect(database.get(
        'SELECT kind FROM sessions WHERE id = ?', 'fork-failure-derived'
      )).toEqual({ kind: 'claude-code' })
      expect(sessions.has('fork-failure-derived')).toBe(false)
      expect(forkPort.sent.filter(({ type }) => type === 'terminal.exited')).toHaveLength(0)

      await settle()
      forkPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'fork-failure-derived', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })
      await waitUntil(() => forkPort.last('terminal.spawned')?.pid === 0)
      const durableText = new TextDecoder().decode(Uint8Array.from(
        (await readSessionFrames(root, 'fork-failure-derived'))
          .filter((frame) => frame.kind === 'output')
          .flatMap((frame) => [...frame.data])
      ))
      expect(durableText.match(/\[Fork 未完成，请检查上方原因后重试\]/g)).toHaveLength(1)
    } finally {
      restoreEnv('MATOU_CLAUDE_COMMAND', previousCommand)
    }
  })

  it('does not repeat a durable fork failure banner when unrelated cold history is corrupt', async () => {
    const journalOptions = {
      maxSegmentBytes: 256,
      rawHotBytes: 4_096,
      compressSealed: false
    }
    let forkServer: RuntimeServer | undefined
    try {
      registerSession(database, 'fork-corrupt-source', 'claude-code')
      registerSession(database, 'fork-corrupt-derived', 'claude-code')
      database.run(
        `INSERT INTO session_fork_intents (
           session_id, source_session_id, source_provider, source_provider_session_id,
           state, error_message, created_at
         ) VALUES (?, ?, 'claude-code', ?, 'failed', ?, 1)`,
        'fork-corrupt-derived', 'fork-corrupt-source', 'missing-provider-corrupt',
        'No session found for requested id'
      )
      const seeded = await SegmentJournal.open(root, 'fork-corrupt-derived', journalOptions)
      for (let sequence = 1; sequence <= 8; sequence += 1) {
        await seeded.appendOutput(sequence, new TextEncoder().encode(
          `historical-output-${sequence}-${'x'.repeat(180)}\r\n`
        ))
      }
      await seeded.appendOutput(9, new TextEncoder().encode(
        '\r\n\u001b[31mNo session found for requested id\u001b[0m\r\n' +
        '\u001b[33m[Fork 未完成，请检查上方原因后重试]\u001b[0m\r\n'
      ))
      await seeded.close()

      await writeFile(
        join(root, 'journal', 'fork-corrupt-derived', 'segment-000001.mtj'),
        'corrupt-cold-history'
      )

      const sessions = new RuntimeSessionRegistry()
      const forkPort = new MockPort()
      forkServer = new RuntimeServer(
        forkPort,
        root,
        database,
        undefined,
        undefined,
        sessions,
        undefined,
        undefined,
        { journalOptionsForSession: () => journalOptions }
      )
      forkPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'fork-corrupt-renderer'
      })
      forkPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'fork-corrupt-derived', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })
      await waitUntil(() => forkPort.last('terminal.spawned')?.pid === 0)

      const journalDirectory = join(root, 'journal', 'fork-corrupt-derived')
      let durableText = ''
      for (const entry of (await readdir(journalDirectory)).sort()) {
        if (!/^segment-\d+\.mtj$/.test(entry) || entry === 'segment-000001.mtj') continue
        for (const frame of await readSegmentFrames(join(journalDirectory, entry))) {
          if (frame.kind === 'output') durableText += new TextDecoder().decode(frame.data)
        }
      }
      expect(durableText.match(/\[Fork 未完成，请检查上方原因后重试\]/g)).toHaveLength(1)
    } finally {
      forkServer?.close()
    }
  })

  it('shows the reference product fork failure banner when the fork process exits before producing output', async () => {
    const executable = join(root, 'provider-fork-exit.sh')
    await writeFile(executable, '#!/bin/sh\nexit 7\n')
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    process.env.MATOU_CLAUDE_COMMAND = executable
    try {
      registerSession(database, 'fork-exit-source', 'claude-code')
      registerSession(database, 'fork-exit-derived', 'claude-code')
      database.run(
        `INSERT INTO session_fork_intents (
           session_id, source_session_id, source_provider, source_provider_session_id,
           state, created_at
         ) VALUES (?, ?, 'claude-code', ?, 'pending', 1)`,
        'fork-exit-derived', 'fork-exit-source', 'provider-source-exit'
      )
      const sessions = new RuntimeSessionRegistry()
      const forkPort = new MockPort()
      new RuntimeServer(forkPort, root, database, undefined, undefined, sessions)
      forkPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'fork-exit-renderer'
      })
      forkPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'fork-exit-derived', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })

      await waitUntil(() => database.get<{ state: string }>(
        'SELECT state FROM session_fork_intents WHERE session_id = ?', 'fork-exit-derived'
      )?.state === 'failed')
      await waitUntil(() => terminalText(forkPort).includes('[Fork 未完成，请检查上方原因后重试]'))
      expect(database.get('SELECT kind FROM sessions WHERE id = ?', 'fork-exit-derived'))
        .toEqual({ kind: 'claude-code' })
      expect(forkPort.sent.filter(({ type }) => type === 'terminal.exited')).toHaveLength(0)
    } finally {
      restoreEnv('MATOU_CLAUDE_COMMAND', previousCommand)
    }
  })

  it('switches live permission modes in place and respawns across the Bypass boundary', async () => {
    const executable = join(root, 'provider-live-permission.sh')
    const argumentFile = join(root, 'provider-live-permission-arguments.txt')
    const inputFile = join(root, 'provider-live-permission-input.txt')
    await writeFile(executable, [
      '#!/bin/sh',
      'printf "%s\\n" "$@" > "$MATOU_TEST_ARGUMENT_FILE"',
      'head -c 2100 /dev/zero | tr "\\0" x',
      'stty raw -echo',
      'cat >> "$MATOU_TEST_INPUT_FILE"'
    ].join('\n'))
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    const previousArgumentFile = process.env.MATOU_TEST_ARGUMENT_FILE
    const previousInputFile = process.env.MATOU_TEST_INPUT_FILE
    process.env.MATOU_CLAUDE_COMMAND = executable
    process.env.MATOU_TEST_ARGUMENT_FILE = argumentFile
    process.env.MATOU_TEST_INPUT_FILE = inputFile
    const sessions = new RuntimeSessionRegistry()
    const livePort = new MockPort()
    try {
      registerSession(database, 'provider-live-permission', 'claude-code')
      database.run(
        `INSERT INTO provider_bindings (
           id, session_id, provider, provider_session_id, resume_state, metadata_json,
           created_at, updated_at, validated_at
         ) VALUES (?, ?, 'claude-code', ?, 'available', ?, 1, 1, 1)`,
        'binding-live-permission', 'provider-live-permission', 'provider-live-42',
        JSON.stringify({ permissionMode: 'default' })
      )
      new RuntimeServer(livePort, root, database, undefined, undefined, sessions)
      livePort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'live-permission-renderer'
      })
      livePort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-live-permission', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })
      await waitUntil(() => sessions.has('provider-live-permission'))
      const firstPid = sessions.get('provider-live-permission')!.pid

      livePort.receive(rpc('permission-plan', 'session.set-permission-mode', {
        sessionId: 'provider-live-permission', provider: 'claude-code',
        permissionMode: 'plan', respawn: false, now: 2
      }))
      await waitUntil(() => livePort.findRpcResponse('permission-plan') !== undefined)
      await waitUntilAsync(async () => (await readFile(inputFile, 'utf8').catch(() => '')).includes('\u001b[Z\u001b[Z'))
      expect(sessions.get('provider-live-permission')?.pid).toBe(firstPid)

      livePort.receive(rpc('model-sonnet', 'session.set-model', {
        sessionId: 'provider-live-permission', modelStrategy: 'claude-sonnet-4-6'
      }))
      await waitUntil(() => livePort.findRpcResponse('model-sonnet') !== undefined)
      await waitUntilAsync(async () => (await readFile(inputFile, 'utf8').catch(() => ''))
        .includes('/model claude-sonnet-4-6\r'))
      expect(sessions.get('provider-live-permission')?.pid).toBe(firstPid)
      expect(livePort.last('terminal.hud')).toMatchObject({
        hud: { modelStrategy: 'claude-sonnet-4-6', mode: 'agent' }
      })

      livePort.receive(rpc('permission-bypass-live', 'session.set-permission-mode', {
        sessionId: 'provider-live-permission', provider: 'claude-code',
        permissionMode: 'bypassPermissions', respawn: true, now: 3
      }))
      await waitUntil(() => sessions.get('provider-live-permission')?.pid !== firstPid)
      await waitUntil(() => livePort.findRpcResponse('permission-bypass-live') !== undefined)
      await waitUntilAsync(async () => (await readFile(argumentFile, 'utf8').catch(() => '')).includes('--dangerously-skip-permissions'))

      expect((await readFile(argumentFile, 'utf8')).trim().split('\n')).toEqual([
        '--resume', 'provider-live-42', '--dangerously-skip-permissions'
      ])
      expect(terminalText(livePort)).toContain('\u001b[2J\u001b[3J\u001b[H')
      expect(livePort.last('terminal.hud')).toMatchObject({
        hud: { permissionMode: 'bypassPermissions', mode: 'agent' }
      })
      expect(livePort.sent.filter(({ type }) => type === 'terminal.exited')).toHaveLength(0)
    } finally {
      livePort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-live-permission'
      })
      await settle()
      restoreEnv('MATOU_CLAUDE_COMMAND', previousCommand)
      restoreEnv('MATOU_TEST_ARGUMENT_FILE', previousArgumentFile)
      restoreEnv('MATOU_TEST_INPUT_FILE', previousInputFile)
    }
  })

  it('starts a fresh live AI process when Bypass is confirmed before a resumable identity exists', async () => {
    const executable = join(root, 'provider-fresh-permission.sh')
    const argumentFile = join(root, 'provider-fresh-permission-arguments.txt')
    await writeFile(executable, '#!/bin/sh\nprintf "%s\\n" "$@" > "$MATOU_TEST_ARGUMENT_FILE"\nstty raw -echo\ncat\n')
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    const previousArgumentFile = process.env.MATOU_TEST_ARGUMENT_FILE
    process.env.MATOU_CLAUDE_COMMAND = executable
    process.env.MATOU_TEST_ARGUMENT_FILE = argumentFile
    const sessions = new RuntimeSessionRegistry()
    const freshPort = new MockPort()
    try {
      registerSession(database, 'provider-fresh-permission', 'claude-code')
      new RuntimeServer(freshPort, root, database, undefined, undefined, sessions)
      freshPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'fresh-permission-renderer'
      })
      freshPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-fresh-permission', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })
      await waitUntil(() => sessions.has('provider-fresh-permission'))
      const firstPid = sessions.get('provider-fresh-permission')!.pid

      freshPort.receive(rpc('fresh-permission-bypass', 'session.set-permission-mode', {
        sessionId: 'provider-fresh-permission', provider: 'claude-code',
        permissionMode: 'bypassPermissions', respawn: true, now: 2
      }))

      await waitUntil(() => freshPort.findRpcResponse('fresh-permission-bypass') !== undefined)
      await waitUntil(() => sessions.get('provider-fresh-permission')?.pid !== firstPid)
      await waitUntilAsync(async () => (await readFile(argumentFile, 'utf8').catch(() => '')).length > 0)
      expect((await readFile(argumentFile, 'utf8')).trim().split('\n')).toEqual([
        '--dangerously-skip-permissions'
      ])
      expect(freshPort.findRpcError('fresh-permission-bypass')).toBeUndefined()
    } finally {
      freshPort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-fresh-permission'
      })
      await settle()
      restoreEnv('MATOU_CLAUDE_COMMAND', previousCommand)
      restoreEnv('MATOU_TEST_ARGUMENT_FILE', previousArgumentFile)
    }
  })

  it('reports a failed Bypass respawn instead of presenting the Shell fallback as success', async () => {
    const executable = join(root, 'provider-failed-permission.sh')
    await writeFile(executable, '#!/bin/sh\nstty raw -echo\ncat\n')
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    process.env.MATOU_CLAUDE_COMMAND = executable
    const sessions = new RuntimeSessionRegistry()
    const failedPort = new MockPort()
    try {
      registerSession(database, 'provider-failed-permission', 'claude-code')
      database.run(
        `INSERT INTO provider_bindings (
           id, session_id, provider, provider_session_id, resume_state, metadata_json,
           created_at, updated_at, validated_at
         ) VALUES (?, ?, 'claude-code', ?, 'available', ?, 1, 1, 1)`,
        'binding-failed-permission', 'provider-failed-permission', 'provider-failed-42',
        JSON.stringify({ permissionMode: 'default' })
      )
      new RuntimeServer(failedPort, root, database, undefined, undefined, sessions)
      failedPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION,
        clientId: 'failed-permission-renderer'
      })
      failedPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-failed-permission', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })
      await waitUntil(() => sessions.get('provider-failed-permission')?.profile === 'claude-code')
      await rm(executable)

      failedPort.receive(rpc('failed-permission-bypass', 'session.set-permission-mode', {
        sessionId: 'provider-failed-permission', provider: 'claude-code',
        permissionMode: 'bypassPermissions', respawn: true, now: 2
      }))

      await waitUntil(() => failedPort.findRpcError('failed-permission-bypass') !== undefined)
      expect(failedPort.findRpcResponse('failed-permission-bypass')).toBeUndefined()
      expect(failedPort.last('terminal.hud')).toMatchObject({ hud: null })
    } finally {
      failedPort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-failed-permission'
      })
      await settle()
      restoreEnv('MATOU_CLAUDE_COMMAND', previousCommand)
    }
  })

  it('returns a naturally completed Agent panel to a usable Shell HUD without an exit flash', async () => {
    const executable = join(root, 'provider-completes.sh')
    await writeFile(executable, '#!/bin/sh\nsleep 0.1\nexit 0\n')
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    process.env.MATOU_CLAUDE_COMMAND = executable
    const sessions = new RuntimeSessionRegistry()
    const completedPort = new MockPort()
    try {
      registerSession(database, 'provider-completed', 'claude-code')
      new RuntimeServer(completedPort, root, database, undefined, undefined, sessions)
      completedPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'completed-renderer'
      })
      completedPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-completed', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })

      await waitUntil(() => sessions.get('provider-completed')?.profile === 'shell')
      expect(database.get<{ kind: string }>('SELECT kind FROM sessions WHERE id = ?', 'provider-completed'))
        .toEqual({ kind: 'shell' })
      expect(completedPort.last('terminal.hud')).toMatchObject({ hud: { mode: 'shell' } })
      expect(completedPort.sent.filter(({ type }) => type === 'terminal.exited')).toHaveLength(0)
    } finally {
      completedPort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION, sessionId: 'provider-completed'
      })
      await settle()
      restoreEnv('MATOU_CLAUDE_COMMAND', previousCommand)
    }
  })

  it('promotes a Shell panel to bypass Claude when a real-world zsh config loads slowly', async () => {
    const executable = join(root, 'claude')
    const argumentFile = join(root, 'shell-promoted-provider-arguments.txt')
    await writeFile(executable, '#!/bin/sh\nprintf "%s\\n" "$@" > "$MATOU_TEST_ARGUMENT_FILE"\nstty raw -echo\ncat\n')
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    const previousArgumentFile = process.env.MATOU_TEST_ARGUMENT_FILE
    const previousPath = process.env.PATH
    const previousShell = process.env.SHELL
    const previousZdotdir = process.env.ZDOTDIR
    const aliasReadyFile = join(root, 'alias-ready.txt')
    await writeFile(join(root, '.zshrc'), [
      'sleep 2.2',
      "alias cc='claude --dangerously-skip-permissions'",
      `print -r -- ready > ${JSON.stringify(aliasReadyFile)}`,
      ''
    ].join('\n'))
    process.env.MATOU_CLAUDE_COMMAND = executable
    process.env.MATOU_TEST_ARGUMENT_FILE = argumentFile
    process.env.PATH = `${root}:${previousPath ?? ''}`
    process.env.SHELL = '/bin/zsh'
    process.env.ZDOTDIR = root
    const sessions = new RuntimeSessionRegistry()
    const promotedPort = new MockPort()
    try {
      registerSession(database, 'shell-promoted-provider', 'shell')
      new RuntimeServer(promotedPort, root, database, undefined, undefined, sessions)
      promotedPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'shell-promoted-renderer'
      })
      promotedPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'shell-promoted-provider', executionContextId: 'replay-context',
        profile: 'shell', cols: 80, rows: 24
      })
      await waitUntil(() => sessions.get('shell-promoted-provider')?.profile === 'shell')
      await waitUntilAsync(async () => (await readFile(aliasReadyFile, 'utf8').catch(() => '')) === 'ready\n', 6_000)
      await new Promise((resolve) => setTimeout(resolve, 50))

      const submittedAt = Date.now()
      promotedPort.receive({
        type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'shell-promoted-provider', data: 'cc\r'
      })

      await waitUntil(() => sessions.get('shell-promoted-provider')?.profile === 'claude-code', 6_000)
      expect(Date.now() - submittedAt).toBeLessThan(750)
      await waitUntilAsync(async () => (await readFile(argumentFile, 'utf8').catch(() => '')).length > 0, 6_000)
      expect((await readFile(argumentFile, 'utf8')).trim().split('\n')).toEqual([
        '--dangerously-skip-permissions'
      ])
      expect(database.get<{ kind: string; title: string }>(
        'SELECT kind, title FROM sessions WHERE id = ?', 'shell-promoted-provider'
      )).toEqual({ kind: 'claude-code', title: 'Claude' })
      expect(promotedPort.last('terminal.hud')).toMatchObject({
        sessionId: 'shell-promoted-provider', hud: {
          mode: 'agent', permissionMode: 'bypassPermissions', modelStrategy: 'opusplan'
        }
      })
      expect(terminalText(promotedPort)).not.toContain('\u001b[2J\u001b[3J\u001b[H')
    } finally {
      promotedPort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'shell-promoted-provider'
      })
      await settle()
      restoreEnv('MATOU_CLAUDE_COMMAND', previousCommand)
      restoreEnv('MATOU_TEST_ARGUMENT_FILE', previousArgumentFile)
      restoreEnv('PATH', previousPath)
      restoreEnv('SHELL', previousShell)
      restoreEnv('ZDOTDIR', previousZdotdir)
    }
  })

  it('keeps a Shell to Claude replacement atomic across Renderer connections', async () => {
    const slowShell = join(root, 'slow-closing-shell.sh')
    const provider = join(root, 'atomic-provider.sh')
    await writeFile(slowShell, [
      '#!/bin/sh',
      "trap 'sleep 0.35; exit 0' HUP TERM",
      "printf 'ready\\n'",
      'while :; do sleep 1; done',
      ''
    ].join('\n'))
    await writeFile(provider, '#!/bin/sh\nstty raw -echo\ncat\n')
    await chmod(slowShell, 0o755)
    await chmod(provider, 0o755)
    const previousShell = process.env.SHELL
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    process.env.SHELL = slowShell
    process.env.MATOU_CLAUDE_COMMAND = provider
    const sessions = new RuntimeSessionRegistry()
    const firstPort = new MockPort()
    const secondPort = new MockPort()
    const firstServer = new RuntimeServer(firstPort, root, database, undefined, undefined, sessions)
    const secondServer = new RuntimeServer(secondPort, root, database, undefined, undefined, sessions)
    try {
      registerSession(database, 'atomic-shell-promotion', 'shell')
      firstPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'atomic-first'
      })
      secondPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'atomic-second'
      })
      const spawn = {
        type: 'terminal.spawn' as const, protocolVersion: PROTOCOL_VERSION,
        sessionId: 'atomic-shell-promotion', executionContextId: 'replay-context',
        profile: 'shell' as const, cols: 80, rows: 24
      }
      firstPort.receive(spawn)
      await waitUntil(() => sessions.get('atomic-shell-promotion')?.profile === 'shell')
      secondPort.receive(spawn)
      await waitUntil(() => secondPort.last('terminal.spawned')?.reattached === true)

      firstPort.receive({
        type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'atomic-shell-promotion', data: 'claude\r'
      })
      await waitUntil(() => !sessions.has('atomic-shell-promotion'))
      secondPort.receive(spawn)

      await waitUntil(() => sessions.get('atomic-shell-promotion')?.profile === 'claude-code', 4_000)
      await new Promise((resolve) => setTimeout(resolve, 500))
      const runs = database.all<{ profile: string }>(
        'SELECT profile FROM session_runs WHERE session_id = ? ORDER BY ordinal',
        'atomic-shell-promotion'
      )
      expect(runs.map(({ profile }) => profile)).toEqual(['shell', 'claude-code'])
      const frames = await readSessionFrames(root, 'atomic-shell-promotion')
      expect(frames.every((frame, index) => index === 0 || frame.sequence > frames[index - 1]!.sequence))
        .toBe(true)
    } finally {
      firstPort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'atomic-shell-promotion'
      })
      await waitUntil(() => !sessions.has('atomic-shell-promotion')).catch(() => undefined)
      firstServer.close()
      secondServer.close()
      restoreEnv('SHELL', previousShell)
      restoreEnv('MATOU_CLAUDE_COMMAND', previousCommand)
    }
  })

  it('replaces an existing Shell when the same persisted Session explicitly starts Claude restore', async () => {
    const executable = join(root, 'provider-restore-retry.sh')
    await writeFile(executable, '#!/bin/sh\nstty raw -echo\ncat\n')
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    process.env.MATOU_CLAUDE_COMMAND = executable
    const sessions = new RuntimeSessionRegistry()
    const retryPort = new MockPort()
    try {
      registerSession(database, 'provider-restore-retry', 'shell')
      new RuntimeServer(retryPort, root, database, undefined, undefined, sessions)
      retryPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'restore-retry-renderer'
      })
      retryPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-restore-retry', executionContextId: 'replay-context',
        profile: 'shell', cols: 80, rows: 24
      })
      await waitUntil(() => sessions.get('provider-restore-retry')?.profile === 'shell')
      database.run(
        `UPDATE sessions SET kind = 'claude-code', title = 'Claude', status = 'starting',
           work_status = 'starting' WHERE id = ?`,
        'provider-restore-retry'
      )
      database.run(
        `INSERT INTO provider_bindings (
           id, session_id, provider, provider_session_id, resume_state, restore_state,
           metadata_json, created_at, updated_at, validated_at
         ) VALUES (?, ?, 'claude-code', ?, 'available', 'restoring', '{}', 1, 2, 1)`,
        'binding-restore-retry', 'provider-restore-retry', 'provider-retry-identity'
      )

      retryPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-restore-retry', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })

      await waitUntil(() => sessions.get('provider-restore-retry')?.profile === 'claude-code')
      expect(retryPort.sent.filter(({ type }) => type === 'protocol.error')).toHaveLength(0)
      expect(retryPort.last('terminal.hud')).toMatchObject({
        sessionId: 'provider-restore-retry', hud: { mode: 'agent' }
      })
    } finally {
      retryPort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-restore-retry'
      })
      await settle()
      restoreEnv('MATOU_CLAUDE_COMMAND', previousCommand)
    }
  })

  it('captures a newly started Claude conversation for the next automatic restore', async () => {
    const executable = join(root, 'new-provider-fixture.sh')
    const argumentFile = join(root, 'new-provider-arguments.txt')
    await writeFile(
      executable,
      '#!/bin/sh\nprintf "%s\\n" "$@" > "$MATOU_TEST_ARGUMENT_FILE"\nsleep 30\n'
    )
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    const previousArgumentFile = process.env.MATOU_TEST_ARGUMENT_FILE
    process.env.MATOU_CLAUDE_COMMAND = executable
    process.env.MATOU_TEST_ARGUMENT_FILE = argumentFile
    const repository = new SessionRepository(database, new DomainTransactionManager(database))
    const providerHooks = new ProviderHookServer(root, repository)
    await providerHooks.start()
    try {
      registerSession(database, 'provider-new-session', 'claude-code')
      const registry = new RuntimeSessionRegistry()
      const providerPort = new MockPort()
      new RuntimeServer(
        providerPort, root, database, undefined, undefined, registry, providerHooks
      )
      providerPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION,
        clientId: 'provider-new-renderer'
      })
      providerPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-new-session', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })

      await waitUntilAsync(async () => (await readFile(argumentFile, 'utf8').catch(() => '')) !== '')
      const arguments_ = (await readFile(argumentFile, 'utf8')).trim().split('\n')
      expect(arguments_.slice(0, 2)).toEqual(['--settings', expect.any(String)])
      const settings = JSON.parse(await readFile(arguments_[1]!, 'utf8')) as {
        hooks: { Stop: Array<{ hooks: Array<{ url: string }> }> }
      }
      const hookUrl = settings.hooks.Stop[0]!.hooks[0]!.url
      expect((await fetch(hookUrl, {
        method: 'POST', headers: { 'content-type': 'application/json' },
        body: JSON.stringify({
          hook_event_name: 'Stop', session_id: 'claude-new-session-42', cwd: root
        })
      })).status).toBe(200)
      expect(repository.getResumeBinding('provider-new-session', 'claude-code')).toMatchObject({
        providerSessionId: 'claude-new-session-42', metadata: {
          cwd: root, lastHookEvent: 'Stop'
        }
      })

      providerPort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-new-session'
      })
      await settle()
    } finally {
      await providerHooks.stop()
      if (previousCommand === undefined) delete process.env.MATOU_CLAUDE_COMMAND
      else process.env.MATOU_CLAUDE_COMMAND = previousCommand
      if (previousArgumentFile === undefined) delete process.env.MATOU_TEST_ARGUMENT_FILE
      else process.env.MATOU_TEST_ARGUMENT_FILE = previousArgumentFile
    }
  })

  it('lets Claude finish its SessionEnd hook after the provider PTY exits', async () => {
    const executable = join(root, 'provider-session-end-fixture.sh')
    const argumentFile = join(root, 'provider-session-end-arguments.txt')
    await writeFile(
      executable,
      '#!/bin/sh\nprintf "%s\\n" "$@" > "$MATOU_TEST_ARGUMENT_FILE"\nsleep 0.2\n'
    )
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    const previousArgumentFile = process.env.MATOU_TEST_ARGUMENT_FILE
    process.env.MATOU_CLAUDE_COMMAND = executable
    process.env.MATOU_TEST_ARGUMENT_FILE = argumentFile
    const repository = new SessionRepository(database, new DomainTransactionManager(database))
    const providerHooks = new ProviderHookServer(root, repository)
    await providerHooks.start()
    const registry = new RuntimeSessionRegistry()
    const providerPort = new MockPort()
    const server = new RuntimeServer(
      providerPort, root, database, undefined, undefined, registry, providerHooks
    )
    try {
      registerSession(database, 'provider-session-end', 'claude-code')
      providerPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION,
        clientId: 'provider-session-end-renderer'
      })
      providerPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-session-end', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })

      await waitUntilAsync(async () => (await readFile(argumentFile, 'utf8').catch(() => '')) !== '')
      const arguments_ = (await readFile(argumentFile, 'utf8')).trim().split('\n')
      const settingsIndex = arguments_.indexOf('--settings')
      const settings = JSON.parse(await readFile(arguments_[settingsIndex + 1]!, 'utf8')) as {
        hooks: { SessionEnd: Array<{ hooks: Array<{ url: string }> }> }
      }
      const hookUrl = settings.hooks.SessionEnd[0]!.hooks[0]!.url
      await waitUntil(() => registry.get('provider-session-end')?.profile === 'shell')

      expect((await fetch(hookUrl, {
        method: 'POST', headers: { 'content-type': 'application/json' },
        body: JSON.stringify({
          hook_event_name: 'SessionEnd', session_id: 'provider-ending-after-pty'
        })
      })).status).toBe(200)
      expect(repository.getResumeBinding('provider-session-end', 'claude-code')).toBeUndefined()
    } finally {
      providerPort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-session-end'
      })
      await settle()
      server.close()
      await providerHooks.stop()
      restoreEnv('MATOU_CLAUDE_COMMAND', previousCommand)
      restoreEnv('MATOU_TEST_ARGUMENT_FILE', previousArgumentFile)
    }
  })

  it('parks a missing provider identity on its original Claude node without retrying automatically', async () => {
    const executable = join(root, 'missing-provider-session.sh')
    const launchMarker = join(root, 'missing-provider-launches.txt')
    await writeFile(executable, [
      '#!/bin/sh',
      'printf "invoked\\n" >> "$MATOU_TEST_LAUNCH_MARKER"',
      'printf "No session found for requested id\\n"',
      'sleep 30',
      ''
    ].join('\n'))
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    const previousMarker = process.env.MATOU_TEST_LAUNCH_MARKER
    process.env.MATOU_CLAUDE_COMMAND = executable
    process.env.MATOU_TEST_LAUNCH_MARKER = launchMarker
    try {
      registerSession(database, 'provider-fallback-session', 'claude-code')
      database.run(
        `INSERT INTO provider_bindings (
           id, session_id, provider, provider_session_id, resume_state, metadata_json,
           created_at, updated_at, validated_at
         ) VALUES (?, ?, 'claude-code', ?, 'available', '{}', 1, 1, 1)`,
        'binding-fallback', 'provider-fallback-session', 'missing-provider-42'
      )
      const sessions = new RuntimeSessionRegistry()
      const fallbackPort = new MockPort()
      new RuntimeServer(fallbackPort, root, database, undefined, undefined, sessions)
      fallbackPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION,
        clientId: 'provider-fallback-renderer'
      })
      fallbackPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-fallback-session', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })

      await waitUntil(() => database.get<{ restore_state: string }>(
        'SELECT restore_state FROM provider_bindings WHERE id = ?', 'binding-fallback'
      )?.restore_state === 'failed')
      await new Promise((resolve) => setTimeout(resolve, 100))

      expect(database.get<{ kind: string }>(
        'SELECT kind FROM sessions WHERE id = ?', 'provider-fallback-session'
      )).toEqual({ kind: 'claude-code' })
      expect(database.get<{ resume_state: string; invalidated_at: number | null }>(
        'SELECT resume_state, invalidated_at FROM provider_bindings WHERE id = ?',
        'binding-fallback'
      )).toMatchObject({ resume_state: 'failed', invalidated_at: expect.any(Number) })
      expect(sessions.get('provider-fallback-session')).toBeUndefined()
      expect(terminalText(fallbackPort)).not.toContain('已回到普通终端')
      expect(fallbackPort.sent.filter(({ type }) => type === 'terminal.exited')).toHaveLength(0)

      fallbackPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-fallback-session', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })
      await new Promise((resolve) => setTimeout(resolve, 100))
      expect((await readFile(launchMarker, 'utf8')).trim().split('\n')).toEqual(['invoked'])

      fallbackPort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-fallback-session'
      })
      await settle()
    } finally {
      if (previousCommand === undefined) delete process.env.MATOU_CLAUDE_COMMAND
      else process.env.MATOU_CLAUDE_COMMAND = previousCommand
      if (previousMarker === undefined) delete process.env.MATOU_TEST_LAUNCH_MARKER
      else process.env.MATOU_TEST_LAUNCH_MARKER = previousMarker
    }
  })

  it('parks an unresponsive provider resume at the product deadline', async () => {
    const executable = join(root, 'unresponsive-provider-session.sh')
    await writeFile(executable, '#!/bin/sh\nsleep 30\n')
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    process.env.MATOU_CLAUDE_COMMAND = executable
    const sessions = new RuntimeSessionRegistry()
    const timeoutPort = new MockPort()
    try {
      registerSession(database, 'provider-timeout-session', 'claude-code')
      database.run(
        `INSERT INTO provider_bindings (
           id, session_id, provider, provider_session_id, resume_state, metadata_json,
           created_at, updated_at, validated_at
         ) VALUES (?, ?, 'claude-code', ?, 'available', '{}', 1, 1, 1)`,
        'binding-timeout', 'provider-timeout-session', 'unresponsive-provider-42'
      )
      new RuntimeServer(
        timeoutPort, root, database, undefined, undefined, sessions, undefined, undefined,
        { providerResumeTimeoutMs: 25 }
      )
      timeoutPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION,
        clientId: 'provider-timeout-renderer'
      })
      timeoutPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-timeout-session', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })

      await waitUntil(() => database.get<{ restore_state: string }>(
        'SELECT restore_state FROM provider_bindings WHERE id = ?', 'binding-timeout'
      )?.restore_state === 'failed')
      expect(database.get<{ kind: string }>(
        'SELECT kind FROM sessions WHERE id = ?', 'provider-timeout-session'
      )).toEqual({ kind: 'claude-code' })
      expect(sessions.get('provider-timeout-session')).toBeUndefined()
      expect(database.get<{ resume_state: string }>(
        'SELECT resume_state FROM provider_bindings WHERE id = ?', 'binding-timeout'
      )).toEqual({ resume_state: 'failed' })
    } finally {
      timeoutPort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-timeout-session'
      })
      await settle()
      if (previousCommand === undefined) delete process.env.MATOU_CLAUDE_COMMAND
      else process.env.MATOU_CLAUDE_COMMAND = previousCommand
    }
  })

  it('keeps the Claude node failed when its resume process cannot start', async () => {
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    process.env.MATOU_CLAUDE_COMMAND = join(root, 'provider-command-does-not-exist')
    try {
      registerSession(database, 'provider-launch-fallback', 'claude-code')
      database.run(
        `INSERT INTO provider_bindings (
           id, session_id, provider, provider_session_id, resume_state, metadata_json,
           created_at, updated_at, validated_at
         ) VALUES (?, ?, 'claude-code', ?, 'available', '{}', 1, 1, 1)`,
        'binding-launch-fallback', 'provider-launch-fallback', 'provider-session-9'
      )
      const sessions = new RuntimeSessionRegistry()
      const launchPort = new MockPort()
      new RuntimeServer(launchPort, root, database, undefined, undefined, sessions)
      launchPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION,
        clientId: 'provider-launch-fallback-renderer'
      })
      launchPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-launch-fallback', executionContextId: 'replay-context',
        profile: 'claude-code', cols: 80, rows: 24
      })

      await waitUntil(() => database.get<{ restore_state: string }>(
        'SELECT restore_state FROM provider_bindings WHERE id = ?', 'binding-launch-fallback'
      )?.restore_state === 'failed')

      expect(database.get<{ kind: string }>(
        'SELECT kind FROM sessions WHERE id = ?', 'provider-launch-fallback'
      )).toEqual({ kind: 'claude-code' })
      expect(sessions.get('provider-launch-fallback')).toBeUndefined()
      expect(launchPort.sent.filter(({ type }) => type === 'protocol.error')).toHaveLength(0)
      launchPort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'provider-launch-fallback'
      })
      await settle()
    } finally {
      if (previousCommand === undefined) delete process.env.MATOU_CLAUDE_COMMAND
      else process.env.MATOU_CLAUDE_COMMAND = previousCommand
    }
  })

  it('does not retry or repeat the fallback hint after a failed identity was cleared', async () => {
    const executable = join(root, 'provider-must-not-run.sh')
    const launchMarker = join(root, 'provider-launch-marker')
    await writeFile(
      executable,
      '#!/bin/sh\nprintf launched > "$MATOU_TEST_LAUNCH_MARKER"\n'
    )
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    const previousMarker = process.env.MATOU_TEST_LAUNCH_MARKER
    process.env.MATOU_CLAUDE_COMMAND = executable
    process.env.MATOU_TEST_LAUNCH_MARKER = launchMarker
    try {
      registerSession(database, 'cleared-provider-session', 'shell')
      database.run(
        `INSERT INTO provider_bindings (
           id, session_id, provider, provider_session_id, resume_state, metadata_json,
           created_at, updated_at, validated_at, invalidated_at
         ) VALUES (?, ?, 'claude-code', ?, 'failed', ?, 1, 2, 1, 2)`,
        'binding-already-cleared', 'cleared-provider-session', 'provider-old',
        JSON.stringify({ invalidationReason: 'provider session not found' })
      )
      const sessions = new RuntimeSessionRegistry()
      const secondStartPort = new MockPort()
      new RuntimeServer(secondStartPort, root, database, undefined, undefined, sessions)
      secondStartPort.receive({
        type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION,
        clientId: 'second-start-renderer'
      })
      secondStartPort.receive({
        type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'cleared-provider-session', executionContextId: 'replay-context',
        profile: 'shell', cols: 80, rows: 24
      })

      await waitUntil(() => sessions.get('cleared-provider-session')?.profile === 'shell')
      await new Promise((resolve) => setTimeout(resolve, 50))
      expect(await readFile(launchMarker, 'utf8').catch(() => '')).toBe('')
      expect(terminalText(secondStartPort)).not.toContain('[上次会话无法续接，已回到普通终端]')
      expect(database.get<{ resume_state: string }>(
        'SELECT resume_state FROM provider_bindings WHERE id = ?', 'binding-already-cleared'
      )).toEqual({ resume_state: 'failed' })
      secondStartPort.receive({
        type: 'terminal.dispose', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'cleared-provider-session'
      })
      await settle()
    } finally {
      if (previousCommand === undefined) delete process.env.MATOU_CLAUDE_COMMAND
      else process.env.MATOU_CLAUDE_COMMAND = previousCommand
      if (previousMarker === undefined) delete process.env.MATOU_TEST_LAUNCH_MARKER
      else process.env.MATOU_TEST_LAUNCH_MARKER = previousMarker
    }
  })
})

describe('RuntimeServer session-scoped journal recovery', () => {
  it('pauses only the affected card, replays retained output, and lets another Session continue', async () => {
    server.close()
    registerSession(database, 'faulted-session')
    registerSession(database, 'healthy-session')
    const executable = join(root, 'journal-recovery-shell.js')
    await writeFile(executable, [
      '#!/usr/bin/env node',
      "process.stdin.setEncoding('utf8')",
      "process.stdin.on('data', (chunk) => process.stdout.write(`reply:${chunk}`))",
      'setInterval(() => {}, 1000)',
      ''
    ].join('\n'))
    await chmod(executable, 0o755)
    const previousShell = process.env.SHELL
    process.env.SHELL = executable
    let writable = false
    port = new MockPort()
    server = new RuntimeServer(
      port, root, database, undefined, undefined, undefined, undefined, undefined,
      {
        journalOptionsForSession: (sessionId) => sessionId === 'faulted-session'
          ? {
              writeFrame: async (handle, encoded) => {
                if (!writable) throw Object.assign(new Error('disk quota reached'), { code: 'ENOSPC' })
                await handle.write(encoded)
              }
            }
          : undefined
      }
    )
    try {
      port.receive({ type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'storage-test' })
      for (const sessionId of ['faulted-session', 'healthy-session']) {
        port.receive({
          type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
          sessionId, executionContextId: 'replay-context', profile: 'shell', cols: 80, rows: 24
        })
      }
      await waitUntil(() => port.sent.filter(({ type }) => type === 'terminal.spawned').length === 2)
      port.receive({
        type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'faulted-session', data: 'held\r'
      })
      port.receive({
        type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'healthy-session', data: 'live\r'
      })
      await waitUntil(() => port.last('terminal.storage-fault')?.sessionId === 'faulted-session')
      await waitUntil(() => terminalText(port).includes('reply:live'))
      expect(port.last('terminal.storage-fault')).toMatchObject({
        code: 'STORAGE_QUOTA_EXCEEDED', retainedBytes: expect.any(Number)
      })

      writable = true
      port.receive({
        type: 'terminal.storage-retry', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'faulted-session'
      })
      await waitUntil(() => port.last('terminal.storage-recovered')?.sessionId === 'faulted-session')
      await waitUntil(() => terminalText(port).includes('reply:held'))

      writable = false
      port.receive({
        type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'faulted-session', data: 'end\r'
      })
      await waitUntil(() => port.sent.filter((message) =>
        message.type === 'terminal.storage-fault' && message.sessionId === 'faulted-session'
      ).length >= 2)
      port.receive({
        type: 'terminal.storage-end', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'faulted-session'
      })
      await waitUntil(() => port.last('terminal.exited')?.sessionId === 'faulted-session')
      port.receive({
        type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
        sessionId: 'healthy-session', data: 'still-live\r'
      })
      await waitUntil(() => terminalText(port).includes('reply:still-live'))
    } finally {
      if (previousShell === undefined) delete process.env.SHELL
      else process.env.SHELL = previousShell
    }
  })
})

class PollingSpyWorkspacePathService extends WorkspacePathService {
  startCount = 0

  override startPolling(): void {
    this.startCount += 1
  }

  override stopPolling(): void {}
}

class MockPort extends EventEmitter implements RuntimePort {
  readonly sent: RuntimeMessage[] = []

  postMessage(message: RuntimeMessage): void {
    this.sent.push(message)
  }
  start(): void {}
  close(): void {}
  receive(data: unknown): void {
    this.emit('message', { data } satisfies PortMessageEvent)
  }
  disconnect(): void { this.emit('close') }
  last<T extends RuntimeMessage['type']>(type: T): Extract<RuntimeMessage, { type: T }> | undefined {
    return this.sent.filter((message) => message.type === type).at(-1) as Extract<RuntimeMessage, { type: T }> | undefined
  }
  findRpcError(requestId: string): Extract<RuntimeMessage, { type: 'rpc.error' }> | undefined {
    return this.sent.find(
      (message): message is Extract<RuntimeMessage, { type: 'rpc.error' }> =>
        message.type === 'rpc.error' && message.requestId === requestId
    )
  }
  findRpcResponse(requestId: string): Extract<RuntimeMessage, { type: 'rpc.response' }> | undefined {
    return this.sent.find(
      (message): message is Extract<RuntimeMessage, { type: 'rpc.response' }> =>
        message.type === 'rpc.response' && message.requestId === requestId
    )
  }
}

async function settle(): Promise<void> {
  for (let index = 0; index < 200; index += 1) {
    await new Promise((resolve) => setImmediate(resolve))
  }
}

async function waitUntil(predicate: () => boolean, timeoutMs = 2_000): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (!predicate()) {
    if (Date.now() >= deadline) throw new Error('condition did not become true before timeout')
    await new Promise((resolve) => setTimeout(resolve, 10))
  }
}

async function waitUntilAsync(predicate: () => Promise<boolean>, timeoutMs = 2_000): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (!(await predicate())) {
    if (Date.now() >= deadline) throw new Error('async condition did not become true before timeout')
    await new Promise((resolve) => setTimeout(resolve, 10))
  }
}

function terminalText(port: MockPort): string {
  const decoder = new TextDecoder()
  return port.sent
    .filter((message): message is Extract<RuntimeMessage, { type: 'terminal.data' }> =>
      message.type === 'terminal.data'
    )
    .map(({ data }) => decoder.decode(data))
    .join('')
}

function workStatus(sessionId: string): string | undefined {
  return database.get<{ work_status: string }>(
    'SELECT work_status FROM sessions WHERE id = ?', sessionId
  )?.work_status
}

function seedReplayAuthority(database: RuntimeDatabase, root: string): void {
  database.transaction((tx) => {
    tx.run(
      'INSERT INTO workspaces (id, name, root_directory, created_at, updated_at) VALUES (?, ?, ?, ?, ?)',
      'replay-workspace', 'Replay', root, 1, 1
    )
    tx.run(
      'INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at) VALUES (?, ?, ?, ?, ?)',
      'replay-context', 'replay-workspace', 'plain-directory', root, 1
    )
    tx.run(
      'INSERT INTO tasks (id, workspace_id, execution_context_id, title, status, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)',
      'replay-task', 'replay-workspace', 'replay-context', 'Replay', 'active', 1, 1
    )
  })
}

function registerSession(
  database: RuntimeDatabase,
  sessionId: string,
  kind: 'shell' | 'claude-code' | 'codex' = 'shell'
): void {
  database.run(
    `INSERT INTO sessions (
       id, task_id, execution_context_id, kind, status, title,
       created_at, updated_at, last_activity_at
     ) VALUES (?, 'replay-task', 'replay-context', ?, 'exited', ?, 1, 1, 1)`,
    sessionId, kind, sessionId
  )
}

function registerCanvasSession(
  database: RuntimeDatabase,
  sessionId: string,
  kind: 'shell' | 'claude-code' | 'codex' = 'shell'
): void {
  registerSession(database, sessionId, kind)
  database.run(
    `INSERT INTO scenes (
       id, task_id, name, mode, root_node_id, title_pinned, sort_key,
       layout_revision, created_at, updated_at
     ) VALUES (?, 'replay-task', 'Canvas', 'tile', ?, 0, 'a0', 1, 1, 1)`,
    `scene-${sessionId}`, `node-${sessionId}`
  )
  database.run(
    `INSERT INTO scene_nodes (id, scene_id, kind, ordinal, created_at)
     VALUES (?, ?, 'root', 0, 1)`,
    `node-${sessionId}`, `scene-${sessionId}`
  )
  database.run(
    `INSERT INTO session_mounts (id, scene_id, scene_node_id, session_id, created_at)
     VALUES (?, ?, ?, ?, 1)`,
    `mount-${sessionId}`, `scene-${sessionId}`, `node-${sessionId}`, sessionId
  )
}

function rpc(requestId: string, method: RpcMethod, input: Record<string, unknown>) {
  return {
    type: 'rpc.request' as const, protocolVersion: PROTOCOL_VERSION, requestId,
    method, capability: 'renderer' as const,
    deadlineAt: Date.now() + 2_000,
    payload: {
      command: { commandId: requestId, commandType: method, requestHash: `hash-${requestId}` },
      input
    }
  }
}

function restoreEnv(name: string, value: string | undefined): void {
  if (value === undefined) delete process.env[name]
  else process.env[name] = value
}

async function readRecoverySourceBytes(paths: string[]): Promise<Array<Buffer | undefined>> {
  return Promise.all(paths.map(async (path) => {
    try {
      return await readFile(path)
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code === 'ENOENT') return undefined
      throw error
    }
  }))
}

async function childProcessCwd(pid: number): Promise<string> {
  if (process.platform === 'linux') return readlink(`/proc/${pid}/cwd`)
  const { stdout } = await execFileAsync('lsof', ['-a', '-p', String(pid), '-d', 'cwd', '-Fn'])
  return stdout.split('\n').find((line) => line.startsWith('n'))?.slice(1) ?? ''
}
