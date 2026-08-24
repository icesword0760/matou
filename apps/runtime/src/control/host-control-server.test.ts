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
    expect(stale).toMatchObject({ ok: false, error: { code: 'CONFLICT' } })
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
  })

  it('isolates backend failure and remains available for later requests', async () => {
    const token = tokenService.issue('run-1', ['terminal.read-current', 'host.list'], Date.now() + 1000)
    backend.readCurrent.mockRejectedValueOnce(new Error('session journal damaged'))

    expect(await request(socketPath, controlRequest('fail', token, 'terminal.read-current', {
      target: { sessionId: 'session-1' }, maxLines: 10, maxBytes: 1000
    }))).toMatchObject({ ok: false, error: { code: 'INTERNAL_ERROR' } })
    expect(await request(socketPath, controlRequest('after', token, 'host.list', {}))).toMatchObject({ ok: true })
  })
})

class TestBackend implements HostControlBackend {
  targets: HostTarget[] = [
    { ref: 'surface:1', workspaceId: 'workspace-1', taskId: 'task-1', sessionId: 'session-1', mountId: 'mount-1', title: 'One' },
    { ref: 'surface:2', workspaceId: 'workspace-1', taskId: 'task-1', sessionId: 'session-2', mountId: 'mount-2', title: 'Two' }
  ]
  readCurrent = vi.fn(async () => ({ text: 'current' }))
  readHistory = vi.fn(async () => ({ text: 'history' }))
  readCommands = vi.fn(async () => [])
  sendText = vi.fn(async () => undefined)
  sendKey = vi.fn(async () => undefined)
  writeTaskStatus = vi.fn(async () => undefined)
  writeTaskProgress = vi.fn(async () => undefined)
  appendTaskLog = vi.fn(async () => undefined)
  listTargets(): HostTarget[] { return this.targets.map((target) => ({ ...target })) }
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
