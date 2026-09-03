import { describe, expect, it, vi } from 'vitest'

import type { HostControlScope, HostTarget } from '../control/host-control-types'
import { HostControlClientError } from '../control/host-control-client'
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
