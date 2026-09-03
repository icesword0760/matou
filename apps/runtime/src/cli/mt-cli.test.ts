import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { describe, expect, it, vi } from 'vitest'

import type { ForkItemInput, HostActionResult } from '../control/host-action-types'
import type { HostControlScope, HostTarget } from '../control/host-control-types'
import { HostControlClientError } from '../control/host-control-client'
import {
  CapabilityTokenService,
  controlEndpointForPlatform,
  HostControlServer,
  type HostControlBackend
} from '../control/host-control-server'
import { runMt, type MtIo } from './mt-cli'

describe('mt CLI', () => {
  it('identifies the caller and prints human hierarchy without internal ids', async () => {
    const fixture = ioFixture()
    const request = vi.fn(async () => ({ caller: { runId: 'secret-run' }, target: targetFixture() }))
    expect(await runMt(['identify'], {}, fixture.io, request)).toBe(0)
    expect(fixture.out.join('')).toContain('工作空间 Workspace / 事项 Task / 画布 Canvas / 会话 2（Shell）')
    expect(fixture.out.join('')).not.toContain('secret-run')
  })

  it('lists JSON for AI and attaches a fresh revision to sibling targets', async () => {
    const fixture = ioFixture()
    const calls: Array<{ method: HostControlScope; params: any }> = []
    const request = async (method: HostControlScope, params: unknown) => {
      calls.push({ method, params })
      if (method === 'host.list') return { projectionRevision: 'revision-2', targets: [targetFixture()] }
      return { text: 'screen' }
    }
    expect(await runMt(['read', 'sibling:1', '--json'], {}, fixture.io, request)).toBe(0)
    expect(calls).toEqual([
      { method: 'host.list', params: { scope: 'current-level' } },
      {
        method: 'terminal.read-current',
        params: {
          target: { kind: 'sibling', ordinal: 1, projectionRevision: 'revision-2' },
          maxLines: 200, maxBytes: 65536
        }
      }
    ])
    expect(JSON.parse(fixture.out[0]!)).toEqual({ text: 'screen' })
  })

  it('sends text with optional Enter and normalizes reference product key aliases', async () => {
    const fixture = ioFixture()
    const calls: Array<{ method: HostControlScope; params: any }> = []
    const request = async (method: HostControlScope, params: unknown) => {
      calls.push({ method, params })
      return { sent: true }
    }
    expect(await runMt(['send', 'right', 'pnpm', 'test', '--enter'], {}, fixture.io, request)).toBe(0)
    expect(await runMt(['key', 'parent', 'ctrl+u'], {}, fixture.io, request)).toBe(0)
    expect(calls).toEqual([
      {
        method: 'terminal.send-text',
        params: {
          target: { kind: 'relative', direction: 'right' },
          text: 'pnpm test', submit: true
        }
      },
      {
        method: 'terminal.send-key',
        params: { target: { kind: 'relation', relation: 'parent' }, key: 'CtrlU' }
      }
    ])
  })

  it('maps usage, target, permission and transport failures to stable exit codes', async () => {
    const cases: Array<[Error, number]> = [
      [new HostControlClientError('TARGET_NOT_FOUND', 'gone'), 3],
      [new HostControlClientError('CAPABILITY_DENIED', 'denied'), 4],
      [new HostControlClientError('TIMEOUT', 'late'), 5]
    ]
    for (const [error, code] of cases) {
      const fixture = ioFixture()
      expect(await runMt(['list'], {}, fixture.io, async () => { throw error })).toBe(code)
      expect(fixture.err).toEqual([error.message])
    }
    const fixture = ioFixture()
    expect(await runMt(['unknown'], {}, fixture.io, async () => undefined)).toBe(2)
  })

  it('explains that external shells have no Matou control identity', async () => {
    const fixture = ioFixture()
    expect(await runMt(['identify'], {}, fixture.io)).toBe(4)
    expect(fixture.err.join('')).toContain('仅在 Matou 托管终端中可用')
  })

  it.each([
    {
      argv: ['create', 'workspace', '--path', '/tmp/项目 甲', '--title', '国际化 🚀', '--submission-key', 'create-w', '--enter', '--json'],
      method: 'structure.create.workspace' as const,
      params: { path: '/tmp/项目 甲', title: '国际化 🚀', submissionKey: 'create-w', enter: true }
    },
    {
      argv: ['create', 'task', '--workspace', 'left', '--title', '事项甲', '--submission-key', 'create-t', '--json'],
      method: 'structure.create.task' as const,
      params: {
        workspace: { kind: 'relative', direction: 'left', projectionRevision: 'revision-action' },
        title: '事项甲', submissionKey: 'create-t'
      }
    },
    {
      argv: ['create', 'canvas', '--task', 'task:one', '--title', '画布甲', '--submission-key', 'create-c', '--json'],
      method: 'structure.create.canvas' as const,
      params: {
        task: { kind: 'ref', ref: 'task:one', projectionRevision: 'revision-action' },
        title: '画布甲', submissionKey: 'create-c'
      }
    },
    {
      argv: ['create', 'session', '--canvas', 'current', '--profile', 'codex', '--title', '会话甲', '--submission-key', 'create-s', '--json'],
      method: 'structure.create.session' as const,
      params: {
        canvas: { kind: 'current', entity: 'canvas' }, profile: 'codex',
        title: '会话甲', submissionKey: 'create-s'
      }
    }
  ])('parses $method with exact titles and revision-bearing targets', async ({ argv, method, params }) => {
    const fixture = ioFixture()
    const request = actionRequest()

    expect(await runMt(argv, {}, fixture.io, request)).toBe(0)
    expect(request).toHaveBeenLastCalledWith(method, params)
  })

  it.each([
    ['child', 'structure.fork.child'],
    ['sibling', 'structure.fork.sibling']
  ] as const)('parses fork %s with explicit environment, prompt, start and submission key', async (relation, method) => {
    const fixture = ioFixture()
    const request = actionRequest()
    const environment = relation === 'child'
      ? { mode: 'new-worktree', branch: 'feature/方案-甲' }
      : { mode: 'existing-worktree', branch: 'main', worktreeRef: 'worktree:main' }

    expect(await runMt([
      'fork', relation, 'parent', '--title', '子节点 🧪',
      '--environment-json', JSON.stringify(environment), '--prompt', '保留  "引号"  与  空格',
      '--start', '--submission-key', `fork-${relation}`, '--json'
    ], {}, fixture.io, request)).toBe(0)
    expect(request).toHaveBeenLastCalledWith(method, {
      source: { kind: 'relation', relation: 'parent', projectionRevision: 'revision-action' },
      title: '子节点 🧪', environment, prompt: '保留  "引号"  与  空格', start: true,
      submissionKey: `fork-${relation}`
    })
  })

  it('submits a batch from stdin without shell quoting loss and retries only named failures', async () => {
    const fixture = ioFixture()
    const items: ForkItemInput[] = [
      { itemKey: 'one', title: '原样  "引号"  🚀', environment: { mode: 'current' }, prompt: '第一行\n第二行' },
      { itemKey: 'two', title: '新 Worktree', environment: { mode: 'new-worktree', branch: 'feature/two' }, start: true }
    ]
    const readStdin = vi.fn(async () => JSON.stringify(items))
    const request = vi.fn(async (method: HostControlScope) => method === 'host.list'
      ? { projectionRevision: 'revision-action', targets: [targetFixture()] }
      : batchResult(items))

    expect(await runMt([
      'fork', 'children', 'self', '--items-json', '-', '--batch-key', 'batch-1',
      '--retry-item-key', 'two', '--json'
    ], {}, fixture.io, request, readStdin)).toBe(0)
    expect(readStdin).toHaveBeenCalledOnce()
    expect(request).toHaveBeenCalledWith('structure.fork.children', {
      source: { kind: 'self' }, batchKey: 'batch-1', items, retryItemKeys: ['two']
    })
  })

  it('also accepts the dependency object and JSON retry-key list', async () => {
    const fixture = ioFixture()
    const items: ForkItemInput[] = [
      { itemKey: 'one', title: '方案一', environment: { mode: 'current' } },
      { itemKey: 'two', title: '方案二', environment: { mode: 'current' } }
    ]
    const request = vi.fn(async (method: HostControlScope) => method === 'host.list'
      ? { projectionRevision: 'revision-action', targets: [targetFixture()] }
      : batchResult(items))

    expect(await runMt([
      'fork', 'children', 'child:2', '--items-json', JSON.stringify(items), '--batch-key', 'batch-2',
      '--retry-item-keys-json', '["one","two"]', '--json'
    ], {}, fixture.io, { request })).toBe(0)
    expect(request).toHaveBeenLastCalledWith('structure.fork.children', {
      source: { kind: 'relation', relation: 'child', ordinal: 2, projectionRevision: 'revision-action' },
      batchKey: 'batch-2', items, retryItemKeys: ['one', 'two']
    })
  })

  it('accepts exactly 1 MiB from stdin and rejects the next byte before requesting', async () => {
    const fixture = ioFixture()
    const request = vi.fn(async () => batchResult([]))
    const atLimit = jsonArrayOfExactUtf8Bytes(1024 * 1024)

    expect(await runMt(
      ['fork', 'children', 'self', '--items-json', '-', '--batch-key', 'limit', '--json'],
      {}, fixture.io, request, async () => atLimit
    )).toBe(0)
    expect(request).toHaveBeenCalledOnce()

    request.mockClear()
    const overLimit = `${atLimit.slice(0, -1)}x]`
    expect(Buffer.byteLength(overLimit, 'utf8')).toBe(1024 * 1024 + 1)
    expect(await runMt(
      ['fork', 'children', 'self', '--items-json', '-', '--batch-key', 'over', '--json'],
      {}, fixture.io, request, async () => overLimit
    )).toBe(2)
    expect(request).not.toHaveBeenCalled()
  })

  it.each([
    ['invalid JSON', '{'],
    ['invalid UTF-8 text', '[{"itemKey":"bad","title":"\ud800","environment":{"mode":"current"}}]']
  ])('rejects %s from stdin before requesting', async (_label, stdin) => {
    const fixture = ioFixture()
    const request = vi.fn(async () => batchResult([]))
    expect(await runMt(
      ['fork', 'children', 'self', '--items-json', '-', '--batch-key', 'bad', '--json'],
      {}, fixture.io, { request, readStdin: async () => stdin }
    )).toBe(2)
    expect(request).not.toHaveBeenCalled()
  })

  it.each([
    {
      argv: ['remove', 'preview', 'session:one', '--scope', 'subtree', '--json'],
      method: 'structure.remove.preview' as const,
      params: { target: { kind: 'ref', ref: 'session:one', projectionRevision: 'revision-action' }, scope: 'subtree' }
    },
    {
      argv: ['remove', 'commit', 'confirmation-1', '--json'],
      method: 'structure.remove.commit' as const,
      params: { confirmationRef: 'confirmation-1' }
    },
    {
      argv: ['close', 'canvas-preview', 'current', '--json'],
      method: 'structure.canvas-close.preview' as const,
      params: { target: { kind: 'current', entity: 'canvas' } }
    },
    {
      argv: ['close', 'canvas-commit', 'confirmation-2', '--json'],
      method: 'structure.canvas-close.commit' as const,
      params: { confirmationRef: 'confirmation-2' }
    }
  ])('parses $method without leaking commit inputs into other fields', async ({ argv, method, params }) => {
    const fixture = ioFixture()
    const request = actionRequest()
    expect(await runMt(argv, {}, fixture.io, request)).toBe(0)
    expect(request).toHaveBeenLastCalledWith(method, params)
  })

  it.each([
    ['focus', undefined, 'navigation.focus.session'],
    ['switch', 'workspace', 'navigation.switch.workspace'],
    ['switch', 'task', 'navigation.switch.task'],
    ['switch', 'canvas', 'navigation.switch.canvas']
  ] as const)('parses %s %s navigation with a fresh target revision', async (command, entity, method) => {
    const fixture = ioFixture()
    const request = actionRequest()
    const argv = entity === undefined
      ? [command, 'right', '--json']
      : [command, entity, 'right', '--json']

    expect(await runMt(argv, {}, fixture.io, request)).toBe(0)
    expect(request).toHaveBeenLastCalledWith(method, {
      target: { kind: 'relative', direction: 'right', projectionRevision: 'revision-action' }
    })
  })

  it.each([
    ['create workspace missing path', ['create', 'workspace', '--title', '缺路径']],
    ['create session invalid profile', ['create', 'session', '--canvas', 'self', '--profile', 'other']],
    ['fork child missing environment', ['fork', 'child', 'self', '--title', '无环境']],
    ['fork child extra positional', ['fork', 'child', 'self', 'extra', '--title', '多余', '--environment-json', '{"mode":"current"}']],
    ['remove commit extra positional', ['remove', 'commit', 'confirmation', 'extra']],
    ['focus extra positional', ['focus', 'self', 'extra']],
    ['unknown create option', ['create', 'workspace', '--path', '/tmp/x', '--bogus']]
  ])('returns usage exit code for %s', async (_label, argv) => {
    const fixture = ioFixture()
    const request = vi.fn(async () => undefined)
    expect(await runMt(argv, {}, fixture.io, request)).toBe(2)
    expect(request).not.toHaveBeenCalled()
  })

  it('prints each batch item with title, environment and state followed by one summary', async () => {
    const fixture = ioFixture()
    const items: ForkItemInput[] = [
      { itemKey: 'one', title: '当前分支方案', environment: { mode: 'current' } },
      { itemKey: 'two', title: '独立方案', environment: { mode: 'new-worktree', branch: 'feature/two' } },
      { itemKey: 'three', title: '复用方案', environment: { mode: 'existing-worktree', branch: 'main', worktreeRef: 'worktree:secret' } }
    ]
    const request = vi.fn(async () => ({
      ...batchResult(items), succeeded: 2, failed: 1,
      items: [
        { ...items[0]!, state: 'ready' },
        { ...items[1]!, state: 'started' },
        { ...items[2]!, state: 'failed', error: '分支冲突' }
      ], retry: { batchKey: 'human', itemKeys: ['three'] }
    }))

    expect(await runMt([
      'fork', 'children', 'self', '--items-json', JSON.stringify(items), '--batch-key', 'human'
    ], {}, fixture.io, request)).toBe(6)
    expect(fixture.out).toHaveLength(1)
    const lines = fixture.out[0]!.split('\n')
    expect(lines).toEqual([
      '1. 当前分支方案 | 环境：当前执行环境 | 状态：已就绪',
      '2. 独立方案 | 环境：新 Worktree（feature/two） | 状态：已启动',
      '3. 复用方案 | 环境：现有 Worktree（main） | 状态：失败',
      '汇总：成功 2 项，失败 1 项。'
    ])
    expect(fixture.out[0]).not.toContain('worktree:secret')
    expect(fixture.out[0]).not.toContain('three')
    expect(fixture.out[0]).not.toContain('分支冲突')
  })

  it('prints preview impact and preservation guarantees without a confirmation ref', async () => {
    const fixture = ioFixture()
    const preview = previewResult('removal-preview')
    const request = actionRequest(preview)

    expect(await runMt(['remove', 'preview', 'self', '--scope', 'subtree'], {}, fixture.io, request)).toBe(0)
    expect(fixture.out.join('')).toContain('影响：事项 1，画布 2，会话 3，子节点 2')
    expect(fixture.out.join('')).toContain('将结束：运行或等待中会话 1，终端进程 2')
    expect(fixture.out.join('')).toContain('项目文件、Git 分支和 Worktree 保持不变')
    expect(fixture.out.join('')).not.toContain('confirmation-secret')
  })

  it('passes the authoritative result through unchanged in JSON mode', async () => {
    const fixture = ioFixture()
    const result = previewResult('canvas-close-preview')
    const request = actionRequest(result)

    expect(await runMt(['close', 'canvas-preview', 'self', '--json'], {}, fixture.io, request)).toBe(0)
    expect(JSON.parse(fixture.out[0]!)).toEqual(result)
    expect(fixture.out[0]).toContain('confirmation-secret')
  })

  it.each([
    ['INVALID_REQUEST', 2],
    ['TARGET_NOT_FOUND', 3],
    ['AMBIGUOUS_TARGET', 3],
    ['STALE_PROJECTION', 3],
    ['TARGET_NOT_READY', 4],
    ['CAPABILITY_DENIED', 4],
    ['CONFIRMATION_REQUIRED', 4],
    ['CONFIRMATION_EXPIRED', 4],
    ['CONFIRMATION_STALE', 4],
    ['PATH_CONFLICT', 4],
    ['BRANCH_CONFLICT', 4],
    ['WORKTREE_CONFLICT', 4],
    ['STORAGE_READ_ONLY', 4],
    ['TIMEOUT', 5],
    ['CONNECTION_ERROR', 5],
    ['NAVIGATION_TIMEOUT', 5],
    ['PARTIAL_SUCCESS', 6],
    ['INTERNAL_ERROR', 1]
  ])('maps Host Control error %s to exit group %i', async (errorCode, expected) => {
    const fixture = ioFixture()
    expect(await runMt(['list'], {}, fixture.io, async () => {
      throw new HostControlClientError(errorCode, errorCode)
    })).toBe(expected)
  })

  it('shows two to five ambiguity candidates and asks for a filter above five', async () => {
    const small = ioFixture()
    expect(await runMt(['focus', 'self'], {}, small.io, async () => {
      throw new HostControlClientError('AMBIGUOUS_TARGET', '匹配多个目标', {
        candidates: [{ humanPath: '窗口 1 / 工作空间 A' }, { humanPath: '窗口 2 / 工作空间 A' }]
      })
    })).toBe(3)
    expect(small.err[0]).toContain('1. 窗口 1 / 工作空间 A')
    expect(small.err[0]).toContain('2. 窗口 2 / 工作空间 A')

    const large = ioFixture()
    expect(await runMt(['focus', 'self'], {}, large.io, async () => {
      throw new HostControlClientError('AMBIGUOUS_TARGET', '匹配多个目标', {
        candidates: Array.from({ length: 6 }, (_, index) => ({ humanPath: `候选 ${index + 1}` }))
      })
    })).toBe(3)
    expect(large.err[0]).toContain('超过 5 个')
    expect(large.err[0]).toContain('补充筛选条件')
    expect(large.err[0]).not.toContain('候选 1')
  })

  it('transports exactly 1 MiB of stdin JSON through the real client and server envelope', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-mt-frame-'))
    const endpoint = controlEndpointForPlatform(root)
    const tokens = new CapabilityTokenService('runtime-task-9')
    const token = tokens.issue(
      { runId: 'run-task-9', sessionId: 'session-task-9' },
      ['structure.fork.children'],
      Date.now() + 10_000
    )
    const received: Array<{ method: HostControlScope; params: unknown }> = []
    const backend = hostControlBackend(async (method, _caller, params) => {
      received.push({ method, params })
      const request = params as { items: ForkItemInput[] }
      return batchResult(request.items)
    })
    const server = new HostControlServer({ socketPath: endpoint, tokenService: tokens, backend })
    const fixture = ioFixture()
    const batch = validBatchJsonOfExactUtf8Bytes(1024 * 1024)
    await server.start()
    try {
      expect(await runMt(
        ['fork', 'children', 'self', '--items-json', '-', '--batch-key', 'frame-boundary', '--json'],
        { MATOU_CONTROL_ENDPOINT: endpoint, MATOU_CONTROL_TOKEN: token },
        fixture.io,
        { readStdin: async () => batch.json }
      )).toBe(0)
      expect(received).toEqual([{
        method: 'structure.fork.children',
        params: { source: { kind: 'self' }, batchKey: 'frame-boundary', items: batch.items }
      }])
      expect(JSON.parse(fixture.out[0]!)).toMatchObject({ kind: 'fork-batch', failed: 0 })
    } finally {
      await server.stop()
      await rm(root, { recursive: true, force: true })
    }
  })

  it.each([
    {
      label: 'usage',
      argv: ['create', 'workspace', '--title', '缺路径', '--json'],
      error: undefined,
      expected: { code: 'INVALID_REQUEST', message: expect.stringContaining('--path') }
    },
    {
      label: 'transport',
      argv: ['list', '--json'],
      error: new HostControlClientError('CONNECTION_ERROR', '本地连接中断'),
      expected: { code: 'CONNECTION_ERROR', message: '本地连接中断' }
    },
    {
      label: 'ambiguity',
      argv: ['focus', 'self', '--json'],
      error: new HostControlClientError('AMBIGUOUS_TARGET', '匹配多个目标', {
        candidates: [{ humanPath: '窗口 1 / 事项 A' }, { humanPath: '窗口 2 / 事项 A' }]
      }),
      expected: {
        code: 'AMBIGUOUS_TARGET', message: '匹配多个目标',
        details: { candidates: [{ humanPath: '窗口 1 / 事项 A' }, { humanPath: '窗口 2 / 事项 A' }] }
      }
    }
  ])('prints stable JSON for $label failures', async ({ argv, error, expected }) => {
    const fixture = ioFixture()
    const request = vi.fn(async () => {
      if (error) throw error
      return undefined
    })
    expect(await runMt(argv, {}, fixture.io, request)).not.toBe(0)
    expect(fixture.out).toEqual([])
    expect(JSON.parse(fixture.err[0]!)).toEqual(expected)
  })

  it('treats help-like and option-like tokens as exact values after value flags', async () => {
    const fixture = ioFixture()
    const request = actionRequest()
    const environment = '--foo'
    const items = '--help'

    expect(await runMt([
      'create', 'workspace', '--path', '-h', '--title', '--help', '--submission-key', '--foo', '--json'
    ], {}, fixture.io, request)).toBe(0)
    expect(request).toHaveBeenCalledWith('structure.create.workspace', {
      path: '-h', title: '--help', submissionKey: '--foo'
    })

    expect(await runMt([
      'fork', 'child', 'self', '--title', '--foo', '--environment-json', environment,
      '--prompt', '--help', '--submission-key', 'dash-values', '--json'
    ], {}, fixture.io, request)).toBe(2)
    expect(JSON.parse(fixture.err.at(-1)!)).toMatchObject({ code: 'INVALID_REQUEST' })

    expect(await runMt([
      'fork', 'children', 'self', '--items-json', items, '--batch-key', 'dash-json', '--json'
    ], {}, fixture.io, request)).toBe(2)
    expect(JSON.parse(fixture.err.at(-1)!)).toMatchObject({ code: 'INVALID_REQUEST' })
  })

  it('preserves dash-leading Chinese values and inline JSON instead of parsing them as help', async () => {
    const fixture = ioFixture()
    const request = actionRequest()
    const items: ForkItemInput[] = [{
      itemKey: 'dash', title: '--中文方案', environment: { mode: 'current' }, prompt: '-h 仍是任务'
    }]
    expect(await runMt([
      'fork', 'child', 'self', '--title', '--中文标题',
      '--environment-json', '{"mode":"new-worktree","branch":"--中文分支"}',
      '--prompt', '--help 仍是提示', '--submission-key', 'dash-child', '--json'
    ], {}, fixture.io, request)).toBe(0)
    expect(request).toHaveBeenCalledWith('structure.fork.child', {
      source: { kind: 'self' }, title: '--中文标题',
      environment: { mode: 'new-worktree', branch: '--中文分支' },
      prompt: '--help 仍是提示', submissionKey: 'dash-child'
    })

    expect(await runMt([
      'fork', 'children', 'self', '--items-json', JSON.stringify(items), '--batch-key', 'dash-batch', '--json'
    ], {}, fixture.io, request)).toBe(0)
    expect(request).toHaveBeenCalledWith('structure.fork.children', {
      source: { kind: 'self' }, items, batchKey: 'dash-batch'
    })
  })

  it.each([
    ['--retry-item-key', 'one'],
    ['--retry-item-keys-json', '[]']
  ])('requires the original batch key whenever %s appears', async (flag, value) => {
    const fixture = ioFixture()
    const request = vi.fn(async () => batchResult([]))
    expect(await runMt([
      'fork', 'children', 'self', '--items-json', '[]', flag, value, '--json'
    ], {}, fixture.io, request)).toBe(2)
    expect(request).not.toHaveBeenCalled()
    expect(JSON.parse(fixture.err[0]!)).toMatchObject({ code: 'INVALID_REQUEST' })
  })

  it.each([
    '[{"itemKey":"bad-high","title":"\\ud800","environment":{"mode":"current"}}]',
    '[{"itemKey":"bad-low","title":"\\udfff","environment":{"mode":"current"}}]'
  ])('rejects escaped isolated surrogates recursively after JSON parsing', async (json) => {
    const fixture = ioFixture()
    const request = vi.fn(async () => batchResult([]))
    expect(await runMt([
      'fork', 'children', 'self', '--items-json', json, '--batch-key', 'surrogate', '--json'
    ], {}, fixture.io, request)).toBe(2)
    expect(request).not.toHaveBeenCalled()
  })

  it('preserves escaped surrogate pairs and emoji after JSON parsing', async () => {
    const fixture = ioFixture()
    const request = vi.fn(async () => batchResult([]))
    const json = '[{"itemKey":"pair","title":"\\ud83d\\ude80 🧪","environment":{"mode":"current"}}]'
    expect(await runMt([
      'fork', 'children', 'self', '--items-json', json, '--batch-key', 'surrogate-pair', '--json'
    ], {}, fixture.io, request)).toBe(0)
    expect(request).toHaveBeenCalledWith('structure.fork.children', {
      source: { kind: 'self' }, batchKey: 'surrogate-pair',
      items: [{ itemKey: 'pair', title: '🚀 🧪', environment: { mode: 'current' } }]
    })
  })

  it('maps RUNTIME_NOT_READY to the state exit group', async () => {
    const fixture = ioFixture()
    expect(await runMt(['focus', 'self'], {}, fixture.io, async () => {
      throw new HostControlClientError('RUNTIME_NOT_READY', '导航服务正在启动')
    })).toBe(4)
  })

  it('prints the public final navigation hierarchy without internal refs', async () => {
    const fixture = ioFixture()
    const request = vi.fn(async (method: HostControlScope) => method === 'host.list'
      ? { projectionRevision: 'revision-navigation', targets: [targetFixture()] }
      : {
          kind: 'navigated',
          finalPath: {
            windowId: 'window-1', workspaceId: 'workspace-1', taskId: 'task-1',
            sceneId: 'scene-1', sessionId: 'session-2'
          }
        })
    expect(await runMt(['focus', 'self'], {}, fixture.io, request)).toBe(0)
    expect(fixture.out[0]).toBe(
      '已切换到：窗口 1 / 工作空间 Workspace / 事项 Task / 画布 Canvas / 会话 2（Shell）'
    )
    expect(fixture.out[0]).not.toMatch(/(?:window|workspace|task|scene|session)[-:][a-z0-9]/i)
    expect(request).toHaveBeenLastCalledWith('host.list', { scope: 'all' })
  })

  it('uses operation-specific human wording for removal and Canvas-close previews', async () => {
    const removal = ioFixture()
    await runMt(
      ['remove', 'preview', 'self', '--scope', 'node'], {}, removal.io,
      actionRequest(previewResult('removal-preview'))
    )
    expect(removal.out[0]).toContain('移除预览')
    expect(removal.out[0]).not.toContain('关闭画布预览')

    const closing = ioFixture()
    await runMt(
      ['close', 'canvas-preview', 'self'], {}, closing.io,
      actionRequest(previewResult('canvas-close-preview'))
    )
    expect(closing.out[0]).toContain('关闭画布预览')
    expect(closing.out[0]).toContain('项目文件、Git 分支和 Worktree 保持不变')
  })

  it.each([
    ['focus missing target', ['focus', '--json']],
    ['create missing entity', ['create', '--json']],
    ['unknown option before JSON mode', ['focus', 'self', '--bogus', '--json']],
    ['a value flag unsupported by this subcommand', ['create', 'task', '--path', '--json']],
    ['a value flag after a missing create entity', ['create', '--path', '--json']],
    ['a value flag after an unknown create entity', ['create', 'unknown', '--path', '--json']],
    ['a value flag after an unknown fork relation', ['fork', 'unknown', '--title', '--json']]
  ])('detects requested JSON error output for $0', async (_label, argv) => {
    const fixture = ioFixture()
    const request = vi.fn(async () => undefined)

    expect(await runMt(argv, {}, fixture.io, request)).toBe(2)
    expect(fixture.out).toEqual([])
    expect(JSON.parse(fixture.err[0]!)).toMatchObject({ code: 'INVALID_REQUEST' })
  })

  it.each(validJsonValueGrammarCases())(
    'consumes a literal --json only for $0 in a valid grammar',
    async (_label, argv) => {
      const fixture = ioFixture()
      const request = vi.fn(async () => undefined)

      expect(await runMt(argv, {}, fixture.io, request)).toBe(2)
      expect(fixture.err[0]).toBe('未知选项：--bogus')
      expect(() => JSON.parse(fixture.err[0]!)).toThrow()
    }
  )

  it('does not treat --json consumed by a declared value flag as output mode', async () => {
    const fixture = ioFixture()
    const request = vi.fn(async () => undefined)

    expect(await runMt([
      'fork', 'child', 'self', '--title', '保留值', '--environment-json', '{"mode":"current"}',
      '--prompt', '--json', '--bogus'
    ], {}, fixture.io, request)).toBe(2)
    expect(fixture.err[0]).toBe('未知选项：--bogus')
    expect(() => JSON.parse(fixture.err[0]!)).toThrow()
  })

  it('stops JSON-mode discovery at the standalone option terminator', async () => {
    const fixture = ioFixture()
    const request = vi.fn(async () => undefined)

    expect(await runMt(['focus', 'self', '--', '--json'], {}, fixture.io, request)).toBe(2)
    expect(fixture.err[0]).toBe('-- 之后存在多余参数：--json')
    expect(() => JSON.parse(fixture.err[0]!)).toThrow()
  })

  it.each([
    {
      label: 'one repeated retry key',
      retryArgs: ['--retry-item-key', 'one'],
      expected: ['one']
    },
    {
      label: 'an explicitly empty repeated retry value',
      retryArgs: ['--retry-item-key', ''],
      expected: ['']
    },
    {
      label: 'a nonempty retry JSON array',
      retryArgs: ['--retry-item-keys-json', '["one","two"]'],
      expected: ['one', 'two']
    },
    {
      label: 'an explicitly empty retry JSON array',
      retryArgs: ['--retry-item-keys-json', '[]'],
      expected: []
    }
  ])('preserves retry presence for $label', async ({ retryArgs, expected }) => {
    const fixture = ioFixture()
    const request = vi.fn(async () => batchResult([]))

    expect(await runMt([
      'fork', 'children', 'self', '--items-json', '[]', '--batch-key', 'original-batch',
      ...retryArgs, '--json'
    ], {}, fixture.io, request)).toBe(0)
    expect(request).toHaveBeenCalledWith('structure.fork.children', {
      source: { kind: 'self' }, items: [], batchKey: 'original-batch', retryItemKeys: expected
    })
  })

  it('routes an explicit empty retry list to the authoritative retry result', async () => {
    const fixture = ioFixture()
    const request = vi.fn(async (_method: HostControlScope, params: unknown) => {
      if (Object.hasOwn(params as object, 'retryItemKeys')) {
        throw new HostControlClientError('TARGET_NOT_FOUND', '未找到原批次')
      }
      return batchResult([])
    })

    expect(await runMt([
      'fork', 'children', 'self', '--items-json', '[]', '--batch-key', 'missing-batch',
      '--retry-item-keys-json', '[]', '--json'
    ], {}, fixture.io, request)).toBe(3)
    expect(JSON.parse(fixture.err[0]!)).toEqual({ code: 'TARGET_NOT_FOUND', message: '未找到原批次' })
  })

  it('redacts a real missing Unix socket path from human and JSON transport errors', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-mt-missing-control-'))
    const endpoint = join(root, 'private', 'control.sock')
    try {
      const human = ioFixture()
      expect(await runMt(
        ['identify'],
        { MATOU_CONTROL_ENDPOINT: endpoint, MATOU_CONTROL_TOKEN: 'token' },
        human.io
      )).toBe(5)
      expect(human.err[0]).toContain('Matou Host Control')
      expect(human.err[0]).not.toContain(root)
      expect(human.err[0]).not.toContain(endpoint)

      const json = ioFixture()
      expect(await runMt(
        ['identify', '--json'],
        { MATOU_CONTROL_ENDPOINT: endpoint, MATOU_CONTROL_TOKEN: 'token' },
        json.io
      )).toBe(5)
      expect(JSON.parse(json.err[0]!)).toMatchObject({ code: 'CONNECTION_ERROR' })
      expect(json.err[0]).not.toContain(root)
      expect(json.err[0]).not.toContain(endpoint)
    } finally {
      await rm(root, { recursive: true, force: true })
    }
  })
})

function ioFixture(): { io: MtIo; out: string[]; err: string[] } {
  const out: string[] = []
  const err: string[] = []
  return { io: { stdout: (text) => out.push(text), stderr: (text) => err.push(text) }, out, err }
}

function targetFixture(): HostTarget {
  return {
    ref: 'session:session-2', workspaceId: 'workspace-1', taskId: 'task-1',
    sessionId: 'session-2', mountId: 'mount-2', title: 'Shell', profile: 'shell',
    cwd: '/workspace', workStatus: 'idle',
    environment: { executionContextRef: 'context:context-1', mode: 'directory' },
    window: { id: 'window-1', kind: 'main', ordinal: 1 },
    workspace: { id: 'workspace-1', name: 'Workspace', ordinal: 1 },
    task: { id: 'task-1', name: 'Task', ordinal: 1 },
    canvas: { id: 'scene-1', name: 'Canvas', ordinal: 1 },
    session: { id: 'session-2', ordinal: 2, detached: false },
    dag: { depth: 0, childRefs: [], siblingRefs: ['session:session-1', 'session:session-2'] }
  }
}

function actionRequest(result: HostActionResult = createdResult()) {
  return vi.fn(async (method: HostControlScope, _params: unknown): Promise<unknown> => {
    if (method === 'host.list') {
      return { projectionRevision: 'revision-action', targets: [
        { ...targetFixture(), ref: 'task:one' }, targetFixture()
      ] }
    }
    return result
  })
}

function createdResult(): HostActionResult {
  const path = resultPath()
  return { kind: 'created', entity: 'session', createdRef: 'session:secret', path, focusedPath: path }
}

function batchResult(items: ForkItemInput[]) {
  return {
    kind: 'fork-batch' as const,
    batchKey: 'batch-1', succeeded: items.length, failed: 0,
    items: items.map((item) => ({ ...item, state: 'ready' as const, sessionRef: `session:${item.itemKey}` }))
  }
}

function previewResult(kind: 'removal-preview' | 'canvas-close-preview'): HostActionResult {
  return {
    kind,
    impact: {
      target: resultPath(), scope: 'subtree', tasks: 1, canvases: 2, sessions: 3,
      descendants: 2, liveRuns: 1, terminalProcesses: 2,
      preservesProjectFiles: true, preservesBranches: true, preservesWorktrees: true
    },
    confirmationRef: 'confirmation-secret'
  }
}

function resultPath() {
  return {
    window: { ref: 'window:secret', title: '主窗口' },
    workspace: { ref: 'workspace:secret', title: '工作空间', path: '/tmp/项目' },
    task: { ref: 'task:secret', title: '事项' },
    canvas: { ref: 'scene:secret', title: '画布' },
    session: { ref: 'session:secret', title: '会话' }
  }
}

function jsonArrayOfExactUtf8Bytes(bytes: number): string {
  const prefix = '[{"itemKey":"one","title":"'
  const suffix = '","environment":{"mode":"current"}}]'
  return `${prefix}${'x'.repeat(bytes - Buffer.byteLength(prefix + suffix, 'utf8'))}${suffix}`
}

function validBatchJsonOfExactUtf8Bytes(bytes: number): { json: string; items: ForkItemInput[] } {
  const items: ForkItemInput[] = Array.from({ length: 16 }, (_, index) => ({
    itemKey: `item-${index + 1}`,
    title: `方案 ${index + 1}`,
    environment: { mode: 'current' },
    prompt: ''
  }))
  let remaining = bytes - Buffer.byteLength(JSON.stringify(items), 'utf8')
  for (const item of items) {
    const size = Math.min(64 * 1024, remaining)
    item.prompt = 'x'.repeat(size)
    remaining -= size
  }
  const json = JSON.stringify(items)
  if (remaining !== 0 || Buffer.byteLength(json, 'utf8') !== bytes) {
    throw new Error('test batch did not reach exact byte boundary')
  }
  return { json, items }
}

function validJsonValueGrammarCases(): Array<[string, string[]]> {
  const grammars: Array<{ prefix: string[]; valueFlags: string[] }> = [
    {
      prefix: ['create', 'workspace'],
      valueFlags: ['--path', '--title', '--submission-key']
    },
    {
      prefix: ['create', 'task'],
      valueFlags: ['--workspace', '--title', '--submission-key']
    },
    {
      prefix: ['create', 'canvas'],
      valueFlags: ['--task', '--title', '--submission-key']
    },
    {
      prefix: ['create', 'session'],
      valueFlags: ['--canvas', '--profile', '--title', '--submission-key']
    },
    {
      prefix: ['fork', 'child', 'self'],
      valueFlags: ['--title', '--environment-json', '--prompt', '--submission-key']
    },
    {
      prefix: ['fork', 'sibling', 'self'],
      valueFlags: ['--title', '--environment-json', '--prompt', '--submission-key']
    },
    {
      prefix: ['fork', 'children', 'self'],
      valueFlags: [
        '--items-json', '--batch-key', '--retry-item-key',
        '--retry-item-keys-json', '--retry-items-json'
      ]
    },
    {
      prefix: ['remove', 'preview', 'self'],
      valueFlags: ['--scope']
    }
  ]
  return grammars.flatMap(({ prefix, valueFlags }) => valueFlags.map((flag) => [
    `${prefix.slice(0, 2).join(' ')} ${flag}`,
    [...prefix, flag, '--json', '--bogus']
  ]))
}

function hostControlBackend(
  executeHostAction: HostControlBackend['executeHostAction']
): HostControlBackend {
  return {
    identify: async () => ({}),
    listTargets: async () => [],
    resolveTarget: async () => 'session-task-9',
    readCurrent: async () => ({}),
    readHistory: async () => ({}),
    readCommands: async () => [],
    sendText: async () => undefined,
    sendKey: async () => undefined,
    writeTaskStatus: async () => undefined,
    writeTaskProgress: async () => undefined,
    appendTaskLog: async () => undefined,
    moveTaskToWindow: async () => ({}),
    executeHostAction
  }
}
