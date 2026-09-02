import { mkdtemp } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it } from 'vitest'

import { RuntimeDatabase } from '../storage/database'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import {
  ShellCommandBlockCollector,
  ShellHistoryRepository,
  SHELL_HISTORY_OUTPUT_CHARACTER_LIMIT,
  SHELL_HISTORY_OUTPUT_LINE_LIMIT,
  encodeShellCommandMarker,
  formatShellHistoryForTerminal
} from './shell-history'

let database: RuntimeDatabase

beforeEach(async () => {
  const root = await mkdtemp(join(tmpdir(), 'matou-shell-history-'))
  database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  seedShellSession(database)
})

afterEach(() => database.close())

describe('ShellHistoryRepository', () => {
  it('keeps only the newest 100 completed command Blocks in chronological display order', () => {
    const history = new ShellHistoryRepository(database)
    for (let index = 1; index <= 105; index += 1) {
      history.complete({
        sessionId: 'session-1', command: `printf command-${index}`,
        cwd: '/tmp/workspace', output: `output-${index}\r\n`, exitCode: 0,
        startedAt: index * 10, completedAt: index * 10 + 1
      })
    }

    const blocks = history.list('session-1')
    expect(blocks).toHaveLength(100)
    expect(blocks[0]?.command).toBe('printf command-6')
    expect(blocks.at(-1)?.command).toBe('printf command-105')
  })

  it('caps each completed Block at the newest 5000 output lines', () => {
    const history = new ShellHistoryRepository(database)
    history.complete({
      sessionId: 'session-1', command: 'many-lines', cwd: '/tmp/workspace',
      output: Array.from({ length: 5_010 }, (_, index) => `line-${index + 1}`).join('\n'),
      exitCode: 0, startedAt: 1, completedAt: 2
    })

    const output = history.list('session-1')[0]!.output
    expect(output.split('\n')).toHaveLength(5_000)
    expect(output.startsWith('line-11\n')).toBe(true)
    expect(output.endsWith('line-5010')).toBe(true)
  })

  it('preserves stored Blocks when restoration is disabled while hiding them from launch', () => {
    const history = new ShellHistoryRepository(database)
    history.complete({
      sessionId: 'session-1', command: 'kept', cwd: '/tmp/workspace', output: 'done',
      exitCode: 0, startedAt: 1, completedAt: 2
    })

    expect(history.listForLaunch('session-1', false)).toEqual([])
    expect(history.list('session-1')).toHaveLength(1)
  })

  it('caps a single completed command by retained characters as well as lines', () => {
    const history = new ShellHistoryRepository(database)
    history.complete({
      sessionId: 'session-1', command: 'large', cwd: '/tmp/workspace',
      output: `old-${'x'.repeat(SHELL_HISTORY_OUTPUT_CHARACTER_LIMIT)}-new`,
      exitCode: 0, startedAt: 1, completedAt: 2
    })

    const output = history.list('session-1')[0]!.output
    expect(output).toHaveLength(SHELL_HISTORY_OUTPUT_CHARACTER_LIMIT)
    expect(output).not.toContain('old-')
    expect(output).toContain('-new')
  })
})

describe('ShellCommandBlockCollector', () => {
  it('reassembles split integration markers and persists only completed commands', () => {
    const collector = new ShellCommandBlockCollector()
    const marker = encodeShellCommandMarker('printf "hello world"')

    expect(collector.ingest(marker.slice(0, 9), 10)).toEqual([])
    expect(collector.ingest(`${marker.slice(9)}hello\r\n`, 11)).toEqual([])
    expect(collector.ingest('\u001b]133;D;', 12)).toEqual([])
    expect(collector.ingest('0\u0007\u001b]133;A\u0007', 13)).toEqual([{
      command: 'printf "hello world"', output: 'hello\r\n', exitCode: 0,
      startedAt: 10, completedAt: 13
    }])
  })

  it('drops an unfinished command when a newer command starts', () => {
    const collector = new ShellCommandBlockCollector()
    collector.ingest(`${encodeShellCommandMarker('sleep 10')}partial`, 1)

    const completed = collector.ingest(
      `${encodeShellCommandMarker('echo next')}next\r\n\u001b]133;D;0\u0007`, 2
    )

    expect(completed).toEqual([{
      command: 'echo next', output: 'next\r\n', exitCode: 0,
      startedAt: 2, completedAt: 2
    }])
  })

  it('retains only the newest launch-history lines while a command is still streaming', () => {
    const collector = new ShellCommandBlockCollector()
    collector.ingest(encodeShellCommandMarker('large output'), 1)
    collector.ingest(Array.from({ length: 3_000 }, (_, index) => `line-${index}`).join('\n') + '\n', 2)
    collector.ingest(Array.from({ length: 3_005 }, (_, index) => `line-${index + 3_000}`).join('\n') + '\n', 3)

    const [completed] = collector.ingest('\u001b]133;D;0\u0007', 4)
    const lines = completed!.output.split('\n')

    expect(lines).toHaveLength(SHELL_HISTORY_OUTPUT_LINE_LIMIT)
    expect(lines[0]).toBe('line-1006')
    expect(lines.at(-2)).toBe('line-6004')
    expect(lines.at(-1)).toBe('')
  })
})

describe('formatShellHistoryForTerminal', () => {
  it('renders completed Blocks above a neutral restoration boundary', () => {
    const text = formatShellHistoryForTerminal([{
      id: 'block-1', sessionId: 'session-1', command: 'printf hello',
      cwd: '/tmp/workspace', output: 'hello\r\n', exitCode: 0,
      startedAt: 1, completedAt: 2
    }])

    expect(text).toContain('❯ printf hello\r\nhello\r\n')
    expect(text).toContain('会话已恢复')
    expect(text).not.toContain('上次命令已中断')
  })
})

function seedShellSession(db: RuntimeDatabase): void {
  db.transaction((tx) => {
    tx.run('INSERT INTO workspaces (id, name, root_directory, created_at, updated_at) VALUES (?, ?, ?, ?, ?)', 'workspace-1', 'Workspace', '/tmp/workspace', 1, 1)
    tx.run('INSERT INTO execution_contexts (id, workspace_id, kind, cwd, created_at) VALUES (?, ?, ?, ?, ?)', 'context-1', 'workspace-1', 'plain-directory', '/tmp/workspace', 1)
    tx.run('INSERT INTO tasks (id, workspace_id, execution_context_id, title, status, sort_key, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)', 'task-1', 'workspace-1', 'context-1', 'Task', 'active', 'a', 1, 1)
    tx.run('INSERT INTO sessions (id, task_id, execution_context_id, kind, status, title, created_at, updated_at, last_activity_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)', 'session-1', 'task-1', 'context-1', 'shell', 'running', 'Shell', 1, 1, 1)
  })
}
