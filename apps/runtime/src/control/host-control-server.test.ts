import { connect } from 'node:net'
import { mkdtemp, stat } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import {
  CapabilityTokenService,
  HostControlServer,
  controlEndpointForPlatform,
  type HostControlBackend,
  type HostTarget
} from './host-control-server'
import type { HostActionMethod, HostActionResult } from './host-action-types'
import {
  markHostControlCommittedResult,
  withHostControlPostResponseEffect
} from './host-control-post-response'
import { RuntimeHostActionError } from './runtime-host-action-facade'

const HOST_ACTION_SCOPES = [
  'structure.create.workspace', 'structure.create.task', 'structure.create.canvas',
  'structure.create.session', 'structure.fork.child', 'structure.fork.sibling',
  'structure.fork.children', 'structure.remove.preview', 'structure.remove.commit',
  'structure.canvas-close.preview', 'structure.canvas-close.commit',
  'navigation.focus.session', 'navigation.switch.workspace',
  'navigation.switch.task', 'navigation.switch.canvas'
] as const satisfies readonly HostActionMethod[]

let root: string
let socketPath: string
let tokenService: CapabilityTokenService
let backend: TestBackend
let server: HostControlServer

beforeEach(async () => {
  root = await mkdtemp(join(tmpdir(), 'matou-control-'))
  socketPath = join(root, 'control.sock')
  tokenService = new CapabilityTokenService('generation-1')
  backend = new TestBackend()
  server = new HostControlServer({ socketPath, tokenService, backend })
  await server.start()
})

afterEach(async () => server.stop())

describe('HostControlServer', () => {
  it('uses a stable Named Pipe endpoint on Windows and a private socket path on Unix', () => {
    expect(controlEndpointForPlatform('/Users/test/.matou', 'darwin')).toBe(
      '/Users/test/.matou/control/runtime.sock'
    )
    const first = controlEndpointForPlatform('C:\\Users\\test\\.matou', 'win32')
    expect(first).toMatch(/^\\\\\.\\pipe\\matou-[a-f0-9]{24}$/)
    expect(controlEndpointForPlatform('C:\\Users\\test\\.matou', 'win32')).toBe(first)
    expect(controlEndpointForPlatform('C:\\Users\\other\\.matou', 'win32')).not.toBe(first)
  })

  it('uses a private local socket and denies calls without a capability token', async () => {
    expect((await stat(root)).mode & 0o777).toBe(0o700)
    expect((await stat(socketPath)).mode & 0o777).toBe(0o600)

    const response = await request(socketPath, {
      version: 1, requestId: 'request-1', token: 'missing', method: 'host.list',
      params: {}, deadlineAt: Date.now() + 1000
    })
    expect(response).toMatchObject({
      ok: false, error: { code: 'CAPABILITY_DENIED' }
    })
  })

  it('issues run-bound, expiring, generation-bound capabilities', async () => {
    const expired = tokenService.issue('run-1', ['host.list'], Date.now() - 1)
    const valid = tokenService.issue('run-2', ['host.list'], Date.now() + 1000)

    expect((await request(socketPath, controlRequest('expired', expired, 'host.list', {})))).toMatchObject({
      ok: false, error: { code: 'CAPABILITY_DENIED' }
    })
    expect((await request(socketPath, controlRequest('valid', valid, 'host.list', {})))).toMatchObject({
      ok: true, result: { targets: expect.any(Array), projectionRevision: expect.any(String) }
    })
    tokenService.revokeRun('run-2')
    expect((await request(socketPath, controlRequest('revoked', valid, 'host.list', {})))).toMatchObject({
      ok: false, error: { code: 'CAPABILITY_DENIED' }
    })
  })

  it('binds capabilities to the caller SessionRun and identifies that caller', async () => {
    const token = tokenService.issue(
      { runId: 'run-caller', sessionId: 'session-2' },
      ['host.identify'],
      Date.now() + 1000
    )

    expect(await request(socketPath, controlRequest('identify', token, 'host.identify', {})))
      .toMatchObject({
        ok: true,
        result: { caller: { runId: 'run-caller', sessionId: 'session-2' }, target: { title: 'Two' } }
      })
    expect(backend.identify).toHaveBeenCalledWith({ runId: 'run-caller', sessionId: 'session-2' })
  })

  it.each(HOST_ACTION_SCOPES)('authorizes %s independently before terminal target resolution', async (scope) => {
    const caller = { runId: `run-${scope}`, sessionId: 'session-2' }
    const token = tokenService.issue(caller, [scope], Date.now() + 1_000)
    const otherScope = HOST_ACTION_SCOPES[(HOST_ACTION_SCOPES.indexOf(scope) + 1) % HOST_ACTION_SCOPES.length]!

    await expect(request(socketPath, controlRequest(`allow-${scope}`, token, scope, {
      fixture: scope
    }))).resolves.toMatchObject({ ok: true })
    expect(backend.executeHostAction).toHaveBeenLastCalledWith(scope, caller, { fixture: scope })
    expect(backend.listTargets).not.toHaveBeenCalled()

    await expect(request(socketPath, controlRequest(`deny-${scope}`, token, otherScope, {
      fixture: otherScope
    }))).resolves.toMatchObject({
      ok: false,
      error: { code: 'CAPABILITY_DENIED' }
    })
  })

  it.each([
    'TARGET_NOT_FOUND', 'AMBIGUOUS_TARGET', 'STALE_PROJECTION', 'TARGET_NOT_READY',
    'CAPABILITY_DENIED', 'CONFIRMATION_REQUIRED', 'CONFIRMATION_EXPIRED',
    'CONFIRMATION_STALE', 'PATH_CONFLICT', 'BRANCH_CONFLICT', 'WORKTREE_CONFLICT',
    'PARTIAL_SUCCESS', 'NAVIGATION_TIMEOUT', 'STORAGE_READ_ONLY'
  ] as const)('preserves the facade error code %s', async (code) => {
    const token = tokenService.issue('run-facade-error', ['structure.create.task'], Date.now() + 1_000)
    backend.executeHostAction.mockRejectedValueOnce(
      new RuntimeHostActionError(code, `fixture ${code}`)
    )

    await expect(request(socketPath, controlRequest(
      `facade-error-${code}`,
      token,
      'structure.create.task',
      { workspace: { kind: 'current', entity: 'workspace' }, submissionKey: 'fixture' }
    ))).resolves.toMatchObject({
      ok: false,
      error: { code, message: `fixture ${code}` }
    })
  })

  it('reports an uninstalled action executor as Runtime not ready', async () => {
    const token = tokenService.issue(
      'run-not-ready', ['structure.create.workspace'], Date.now() + 1_000
    )
    backend.executeHostAction.mockRejectedValueOnce(Object.assign(
      new Error('Host Action facade is not installed'),
      { code: 'RUNTIME_NOT_READY' as const }
    ))

    await expect(request(socketPath, controlRequest(
      'action-not-ready', token, 'structure.create.workspace',
      { path: '/fixture', submissionKey: 'fixture' }
    ))).resolves.toMatchObject({
      ok: false,
      error: { code: 'RUNTIME_NOT_READY' }
    })
  })

  it('returns a self-removal success frame before running the action post-response disposal', async () => {
    const token = tokenService.issue(
      { runId: 'run-self-remove-action', sessionId: 'session-1' },
      ['structure.remove.commit'],
      Date.now() + 1_000
    )
    const disposed = vi.fn(async () => server.stop())
    backend.executeHostAction.mockResolvedValueOnce(markHostControlCommittedResult(
      withHostControlPostResponseEffect(
        { kind: 'removed', targetRef: 'session:session-1' },
        disposed
      )
    ) as never)

    await expect(request(socketPath, controlRequest(
      'self-remove-action', token, 'structure.remove.commit',
      { confirmationRef: 'confirmation-1' }
    ))).resolves.toMatchObject({
      ok: true,
      result: { kind: 'removed', targetRef: 'session:session-1' }
    })
    await vi.waitFor(() => expect(disposed).toHaveBeenCalledTimes(1))
  })

  it('writes the authoritative Host Control result before running caller disposal', async () => {
    const token = tokenService.issue(
      { runId: 'run-self-remove', sessionId: 'session-1' },
      ['host.identify'],
      Date.now() + 1000
    )
    const disposed = vi.fn(async () => {
      await server.stop()
    })
    backend.identify.mockResolvedValueOnce(withHostControlPostResponseEffect(
      { kind: 'removed', targetRef: 'session:session-1' },
      disposed
    ) as never)

    const response = await request(
      socketPath,
      controlRequest('self-remove', token, 'host.identify', {})
    )

    expect(response).toMatchObject({
      ok: true,
      result: { kind: 'removed', targetRef: 'session:session-1' }
    })
    await vi.waitFor(() => expect(disposed).toHaveBeenCalledTimes(1))
  })

  it('keeps the authoritative mutation result when caller disposal is queued after the deadline', async () => {
    const token = tokenService.issue(
      { runId: 'run-deadline-remove', sessionId: 'session-1' },
      ['host.identify'],
      Date.now() + 5_000
    )
    const deadlineAt = Date.now() + 1_000
    const disposed = vi.fn(async () => undefined)
    let now: ReturnType<typeof vi.spyOn> | undefined
    backend.identify.mockImplementationOnce(async () => {
      now = vi.spyOn(Date, 'now').mockReturnValue(deadlineAt + 1)
      return markHostControlCommittedResult(withHostControlPostResponseEffect(
        { kind: 'removed', targetRef: 'session:session-1' },
        disposed
      )) as never
    })

    try {
      const response = await request(socketPath, {
        version: 1, requestId: 'deadline-self-remove', token, method: 'host.identify',
        params: {}, deadlineAt
      })

      expect(response).toMatchObject({
        ok: true,
        result: { kind: 'removed', targetRef: 'session:session-1' }
      })
      await vi.waitFor(() => expect(disposed).toHaveBeenCalledTimes(1))
    } finally {
      now?.mockRestore()
    }
  })

  it.each([
    { label: 'create', result: { kind: 'created', entity: 'task' } },
    { label: 'slow Fork batch', result: { kind: 'fork-batch', batchKey: 'slow-batch' } },
    { label: 'non-caller removal', result: { kind: 'removed', targetRef: 'session:other' } }
  ])('keeps a committed $label result across the post-dispatch deadline', async ({ label, result }) => {
    const token = tokenService.issue(`run-${label}`, ['host.identify'], Date.now() + 5_000)
    const deadlineAt = Date.now() + 1_000
    let now: ReturnType<typeof vi.spyOn> | undefined
    backend.identify.mockImplementationOnce(async () => {
      await Promise.resolve()
      now = vi.spyOn(Date, 'now').mockReturnValue(deadlineAt + 1)
      return markHostControlCommittedResult({ ...result }) as never
    })

    try {
      await expect(request(socketPath, {
        version: 1, requestId: `committed-${label}`, token, method: 'host.identify',
        params: {}, deadlineAt
      })).resolves.toMatchObject({ ok: true, result })
    } finally {
      now?.mockRestore()
    }
  })

  it('reports disposal failure after returning the committed structure result', async () => {
    const token = tokenService.issue('run-cleanup-error', ['host.identify'], Date.now() + 1_000)
    const diagnostic = vi.spyOn(console, 'error').mockImplementation(() => undefined)
    backend.identify.mockResolvedValueOnce(markHostControlCommittedResult(
      withHostControlPostResponseEffect(
        { kind: 'removed', targetRef: 'session:other' },
        async () => { throw new Error('cleanup diagnostic') }
      )
    ) as never)

    try {
      await expect(request(
        socketPath,
        controlRequest('cleanup-error', token, 'host.identify', {})
      )).resolves.toMatchObject({
        ok: true,
        result: { kind: 'removed', targetRef: 'session:other' }
      })
      await vi.waitFor(() => expect(diagnostic).toHaveBeenCalledWith(
        '[host-control.post-response] cleanup diagnostic'
      ))
    } finally {
      diagnostic.mockRestore()
    }
  })

  it('passes relative and relation selectors to the topology backend with caller context', async () => {
    const token = tokenService.issue(
      { runId: 'run-caller', sessionId: 'session-1' },
      ['terminal.read-current'],
      Date.now() + 1000
    )
    expect(await request(socketPath, controlRequest('right', token, 'terminal.read-current', {
      target: { kind: 'relative', direction: 'right' }, maxLines: 10, maxBytes: 1000
    }))).toMatchObject({ ok: true })
    expect(backend.resolveTarget).toHaveBeenCalledWith(
      { runId: 'run-caller', sessionId: 'session-1' },
      { kind: 'relative', direction: 'right' },
      expect.any(Array),
      expect.any(String)
    )
  })

  it('resolves human ordinals only with their matching projection revision', async () => {
    const token = tokenService.issue('run-1', ['host.list', 'terminal.read-current'], Date.now() + 1000)
    const listing = await request(socketPath, controlRequest('list', token, 'host.list', {})) as {
      result: { projectionRevision: string; targets: HostTarget[] }
    }
    expect(listing.result.targets.map(({ ref }) => ref)).toEqual(['surface:1', 'surface:2'])

    backend.targets.reverse()
    const stale = await request(socketPath, controlRequest('read', token, 'terminal.read-current', {
      target: { ref: 'surface:1', projectionRevision: listing.result.projectionRevision },
      maxLines: 100, maxBytes: 4096
    }))
    expect(stale).toMatchObject({ ok: false, error: { code: 'STALE_PROJECTION' } })
  })

  it('bounds terminal reads and allowlists control keys', async () => {
    const token = tokenService.issue(
      'run-1',
      ['terminal.read-current', 'terminal.send-key'],
      Date.now() + 1000
    )
    expect(await request(socketPath, controlRequest('large-read', token, 'terminal.read-current', {
      target: { sessionId: 'session-1' }, maxLines: 100_000, maxBytes: 4096
    }))).toMatchObject({ ok: false, error: { code: 'INVALID_REQUEST' } })
    expect(await request(socketPath, controlRequest('bad-key', token, 'terminal.send-key', {
      target: { sessionId: 'session-1' }, key: 'RunArbitraryMacro'
    }))).toMatchObject({ ok: false, error: { code: 'UNSUPPORTED' } })
    expect(await request(socketPath, controlRequest('enter', token, 'terminal.send-key', {
      target: { sessionId: 'session-1' }, key: 'Enter'
    }))).toMatchObject({ ok: true })
    expect(backend.sendKey).toHaveBeenCalledWith('session-1', 'Enter')

    for (const key of [
      'Backspace', 'Delete', 'Home', 'End', 'PageUp', 'PageDown', 'CtrlU'
    ]) {
      expect(await request(socketPath, controlRequest(`key-${key}`, token, 'terminal.send-key', {
        target: { sessionId: 'session-1' }, key
      }))).toMatchObject({ ok: true })
    }
  })

  it('sends text and optional Enter as one backend action', async () => {
    const token = tokenService.issue('run-1', ['terminal.send-text'], Date.now() + 1000)
    expect(await request(socketPath, controlRequest('send', token, 'terminal.send-text', {
      target: { sessionId: 'session-1' }, text: 'pnpm test', submit: true
    }))).toMatchObject({ ok: true })
    expect(backend.sendText).toHaveBeenCalledWith('session-1', 'pnpm test', true)
  })

  it('clamps external Task progress before it reaches the product data channel', async () => {
    const token = tokenService.issue('run-progress', ['task.progress.write'], Date.now() + 1000)
    expect(await request(socketPath, controlRequest('progress-high', token, 'task.progress.write', {
      taskId: 'task-1', progress: 135, label: 'finishing'
    }))).toMatchObject({ ok: true })
    expect(backend.writeTaskProgress).toHaveBeenCalledWith('task-1', 100, 'finishing')

    expect(await request(socketPath, controlRequest('progress-low', token, 'task.progress.write', {
      taskId: 'task-1', progress: -20
    }))).toMatchObject({ ok: true })
    expect(backend.writeTaskProgress).toHaveBeenCalledWith('task-1', 0)
  })

  it('isolates backend failure and remains available for later requests', async () => {
    const token = tokenService.issue('run-1', ['terminal.read-current', 'host.list'], Date.now() + 1000)
    backend.readCurrent.mockRejectedValueOnce(new Error('session journal damaged'))

    expect(await request(socketPath, controlRequest('fail', token, 'terminal.read-current', {
      target: { sessionId: 'session-1' }, maxLines: 10, maxBytes: 1000
    }))).toMatchObject({ ok: false, error: { code: 'INTERNAL_ERROR' } })
    expect(await request(socketPath, controlRequest('after', token, 'host.list', {}))).toMatchObject({ ok: true })
  })

  it('requires the dedicated capability before moving a whole Task', async () => {
    const denied = tokenService.issue('run-1', ['host.list'], Date.now() + 1000)
    const allowed = tokenService.issue('run-2', ['task.move-to-window'], Date.now() + 1000)
    const params = {
      migrationId: 'migration-1', taskId: 'task-1',
      sourceWindowId: 'window-1', targetWindowId: 'window-2'
    }
    expect(await request(socketPath, controlRequest('denied', denied, 'task.move-to-window', params)))
      .toMatchObject({ ok: false, error: { code: 'CAPABILITY_DENIED' } })
    expect(await request(socketPath, controlRequest('move', allowed, 'task.move-to-window', params)))
      .toMatchObject({ ok: true, result: { state: 'committed' } })
    expect(backend.moveTaskToWindow).toHaveBeenCalledWith(params)
  })
})

class TestBackend implements HostControlBackend {
  targets: HostTarget[] = [
    targetFixture(1, 'One'),
    targetFixture(2, 'Two')
  ]
  readCurrent = vi.fn(async () => ({ text: 'current' }))
  readHistory = vi.fn(async () => ({ text: 'history' }))
  readCommands = vi.fn(async () => [])
  sendText = vi.fn(async () => undefined)
  sendKey = vi.fn(async () => undefined)
  writeTaskStatus = vi.fn(async () => undefined)
  writeTaskProgress = vi.fn(async () => undefined)
  appendTaskLog = vi.fn(async () => undefined)
  moveTaskToWindow = vi.fn(async () => ({ state: 'committed' }))
  executeHostAction = vi.fn(async (_method: HostActionMethod): Promise<HostActionResult> => ({
    kind: 'navigated',
    finalPath: {
      windowId: 'window-1', workspaceId: 'workspace-1', taskId: 'task-1', sceneId: 'scene-1'
    }
  }))
  identify = vi.fn(async (caller: { sessionId: string }) => ({
    caller,
    target: this.targets.find(({ sessionId }) => sessionId === caller.sessionId)
  }))
  resolveTarget = vi.fn(async (
    _caller: { sessionId: string }, selector: { kind: string; sessionId?: string; direction?: string },
    targets: HostTarget[]
  ) => {
    if (selector.kind === 'session') return selector.sessionId!
    if (selector.kind === 'relative' && selector.direction === 'right') return targets[1]!.sessionId
    return targets[0]!.sessionId
  })
  listTargets = vi.fn((): HostTarget[] => this.targets.map((target) => ({ ...target })))
}

function targetFixture(ordinal: number, title: string): HostTarget {
  const sessionId = `session-${ordinal}`
  return {
    ref: `surface:${ordinal}`, workspaceId: 'workspace-1', taskId: 'task-1', sessionId,
    mountId: `mount-${ordinal}`, title, profile: 'shell', cwd: '/fixture', workStatus: 'idle',
    environment: { executionContextRef: 'context:context-1', mode: 'directory' },
    window: { id: 'window-1', kind: 'main', ordinal: 1 },
    workspace: { id: 'workspace-1', name: 'Workspace', ordinal: 1 },
    task: { id: 'task-1', name: 'Task', ordinal: 1 },
    canvas: { id: 'scene-1', name: 'Canvas', ordinal: 1 },
    session: { id: sessionId, ordinal, detached: false },
    dag: { depth: 0, childRefs: [], siblingRefs: ['surface:1', 'surface:2'] }
  }
}

function controlRequest(requestId: string, token: string, method: string, params: unknown) {
  return { version: 1, requestId, token, method, params, deadlineAt: Date.now() + 1000 }
}

async function request(path: string, value: unknown): Promise<Record<string, unknown>> {
  const socket = connect(path)
  await new Promise<void>((resolve, reject) => {
    socket.once('connect', resolve)
    socket.once('error', reject)
  })
  const body = Buffer.from(JSON.stringify(value))
  const prefix = Buffer.alloc(4)
  prefix.writeUInt32BE(body.byteLength)
  socket.write(Buffer.concat([prefix, body]))
  const response = await readFrame(socket)
  socket.end()
  return JSON.parse(response.toString('utf8')) as Record<string, unknown>
}

function readFrame(socket: import('node:net').Socket): Promise<Buffer> {
  return new Promise((resolve, reject) => {
    let buffered = Buffer.alloc(0)
    socket.on('data', (chunk) => {
      buffered = Buffer.concat([buffered, Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk)])
      if (buffered.byteLength < 4) return
      const length = buffered.readUInt32BE(0)
      if (buffered.byteLength >= 4 + length) resolve(buffered.subarray(4, 4 + length))
    })
    socket.once('error', reject)
  })
}
