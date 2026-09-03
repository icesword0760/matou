import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { createServer, type Server, type Socket } from 'node:net'

import { afterEach, describe, expect, it } from 'vitest'

import { HostControlClient, HostControlClientError } from './host-control-client'

const roots: string[] = []
const servers: Server[] = []
const sockets = new Set<Socket>()

afterEach(async () => {
  for (const socket of sockets) socket.destroy()
  sockets.clear()
  await Promise.all(servers.splice(0).map((server) => new Promise<void>((resolve) => server.close(() => resolve()))))
  await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true })))
})

describe('HostControlClient', () => {
  it('sends one length-prefixed request with token and deadline', async () => {
    const { endpoint, requests } = await fixtureServer((request) => ({
      version: 1, requestId: request.requestId, ok: true, result: { identified: true }
    }))
    const client = new HostControlClient({ endpoint, token: 'secret-token', timeoutMs: 1000 })

    await expect(client.request('host.identify', {})).resolves.toEqual({ identified: true })
    expect(requests).toHaveLength(1)
    expect(requests[0]).toMatchObject({
      version: 1, token: 'secret-token', method: 'host.identify', params: {}
    })
    expect(requests[0]!.deadlineAt).toBeGreaterThan(Date.now() - 1000)
  })

  it('reads a response delivered across partial socket frames', async () => {
    const { endpoint } = await fixtureServer((request) => ({
      version: 1, requestId: request.requestId, ok: true, result: { targets: [1, 2] }
    }), true)
    const client = new HostControlClient({ endpoint, token: 'token', timeoutMs: 1000 })
    await expect(client.request('host.list', {})).resolves.toEqual({ targets: [1, 2] })
  })

  it('preserves structured service errors', async () => {
    const { endpoint } = await fixtureServer((request) => ({
      version: 1, requestId: request.requestId, ok: false,
      error: { code: 'TARGET_NOT_FOUND', message: 'gone' }
    }))
    const client = new HostControlClient({ endpoint, token: 'token', timeoutMs: 1000 })
    await expect(client.request('host.list', {})).rejects.toEqual(
      expect.objectContaining<Partial<HostControlClientError>>({ code: 'TARGET_NOT_FOUND', message: 'gone' })
    )
  })

  it('preserves only the documented ambiguity candidate fields', async () => {
    const paths = Array.from(
      { length: 6 },
      (_, index) => `Workspace / Task / Canvas / Candidate ${index + 1}`
    )
    const { endpoint } = await fixtureServer((request) => ({
      version: 1, requestId: request.requestId, ok: false,
      error: {
        code: 'AMBIGUOUS_TARGET',
        message: 'choose one',
        details: {
          candidates: [
            ...paths.map((humanPath) => ({ humanPath, internal: true }))
          ],
          internalTrace: 'hidden'
        }
      }
    }))
    const client = new HostControlClient({ endpoint, token: 'token', timeoutMs: 1000 })

    try {
      await client.request('host.list', {})
    } catch (error) {
      expect(error).toBeInstanceOf(HostControlClientError)
      expect(error).toMatchObject({
        code: 'AMBIGUOUS_TARGET',
        details: {
          candidates: [
            ...paths.map((humanPath) => ({ humanPath }))
          ]
        }
      })
      expect(Object.keys((error as HostControlClientError).details!.candidates[0]!))
        .toEqual(['humanPath'])
      return
    }
    throw new Error('expected Host Control client fault')
  })

  it('rejects mixed invalid ambiguity details instead of keeping a partial candidate list', async () => {
    const { endpoint } = await fixtureServer((request) => ({
      version: 1, requestId: request.requestId, ok: false,
      error: {
        code: 'AMBIGUOUS_TARGET', message: 'choose one',
        details: {
          candidates: [
            { humanPath: 'Workspace / Valid' },
            { humanPath: 42 },
            { humanPath: 'Workspace / Also valid' }
          ]
        }
      }
    }))

    const error = await captureClientError(new HostControlClient({
      endpoint, token: 'token', timeoutMs: 1000
    }))
    expect(error).not.toHaveProperty('details')
  })

  it('accepts a 4096-byte candidate path and rejects a longer one', async () => {
    const valid = 'v'.repeat(4_096)
    const overLimit = 'x'.repeat(4_097)
    const validServer = await fixtureServer((request) => ({
      version: 1, requestId: request.requestId, ok: false,
      error: {
        code: 'AMBIGUOUS_TARGET', message: 'choose one',
        details: { candidates: [{ humanPath: valid }] }
      }
    }))
    const invalidServer = await fixtureServer((request) => ({
      version: 1, requestId: request.requestId, ok: false,
      error: {
        code: 'AMBIGUOUS_TARGET', message: 'choose one',
        details: { candidates: [{ humanPath: overLimit }] }
      }
    }))

    const validError = await captureClientError(new HostControlClient({
      endpoint: validServer.endpoint, token: 'token', timeoutMs: 1000
    }))
    const invalidError = await captureClientError(new HostControlClient({
      endpoint: invalidServer.endpoint, token: 'token', timeoutMs: 1000
    }))
    expect(validError.details?.candidates).toEqual([{ humanPath: valid }])
    expect(invalidError).not.toHaveProperty('details')
  })

  it('reads a valid response whose protocol envelope is larger than 1 MiB', async () => {
    const payload = 'x'.repeat(1024 * 1024)
    const { endpoint } = await fixtureServer((request) => ({
      version: 1, requestId: request.requestId, ok: true, result: { payload }
    }))
    const client = new HostControlClient({ endpoint, token: 'token', timeoutMs: 1000 })

    await expect(client.request('host.identify', {})).resolves.toEqual({ payload })
  })

  it('redacts the endpoint and OS path from connection errors', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-control-missing-'))
    roots.push(root)
    const endpoint = join(root, 'private-runtime', 'control.sock')
    const error = await captureClientError(new HostControlClient({
      endpoint, token: 'token', timeoutMs: 100
    }))

    expect(error).toMatchObject({
      code: 'CONNECTION_ERROR',
      message: '未连接到 Matou Host Control；请在 Matou 托管终端中重试'
    })
    expect(error.message).not.toContain(root)
    expect(error.message).not.toContain(endpoint)
  })

  it('uses a stable actionable message when a real socket response times out', async () => {
    const { endpoint } = await rawFixtureServer()
    const error = await captureClientError(new HostControlClient({
      endpoint, token: 'token', timeoutMs: 25
    }))

    expect(error).toMatchObject({
      code: 'TIMEOUT',
      message: '等待 Matou Host Control 响应超时；请确认 Matou 仍在运行后重试'
    })
    expect(error.message).not.toContain(endpoint)
  })

  it('converts an invalid response frame into a stable redacted client error', async () => {
    const { endpoint } = await rawFixtureServer(Buffer.from('{not-json'))
    const error = await captureClientError(new HostControlClient({
      endpoint, token: 'token', timeoutMs: 1000
    }))

    expect(error).toMatchObject({
      code: 'INVALID_RESPONSE',
      message: 'Matou Host Control 返回了无效响应；请重试'
    })
    expect(error.message).not.toContain(endpoint)
  })
})

async function captureClientError(client: HostControlClient): Promise<HostControlClientError> {
  try {
    await client.request('host.list', {})
  } catch (error) {
    expect(error).toBeInstanceOf(HostControlClientError)
    return error as HostControlClientError
  }
  throw new Error('expected Host Control client fault')
}

async function fixtureServer(
  respond: (request: Record<string, any>) => Record<string, any>,
  split = false
): Promise<{ endpoint: string; requests: Record<string, any>[] }> {
  const root = await mkdtemp(join(tmpdir(), 'matou-control-client-'))
  roots.push(root)
  const endpoint = join(root, 'control.sock')
  const requests: Record<string, any>[] = []
  const server = createServer((socket) => {
    sockets.add(socket)
    socket.once('close', () => sockets.delete(socket))
    let buffered = Buffer.alloc(0)
    socket.on('data', (chunk) => {
      buffered = Buffer.concat([buffered, Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk)])
      if (buffered.byteLength < 4) return
      const length = buffered.readUInt32BE(0)
      if (buffered.byteLength < length + 4) return
      const request = JSON.parse(buffered.subarray(4, length + 4).toString('utf8')) as Record<string, any>
      requests.push(request)
      const body = Buffer.from(JSON.stringify(respond(request)))
      const prefix = Buffer.allocUnsafe(4)
      prefix.writeUInt32BE(body.byteLength)
      const frame = Buffer.concat([prefix, body])
      if (split) {
        socket.write(frame.subarray(0, 2))
        setTimeout(() => socket.write(frame.subarray(2)), 5)
      } else socket.write(frame)
    })
  })
  servers.push(server)
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject)
    server.listen(endpoint, resolve)
  })
  return { endpoint, requests }
}

async function rawFixtureServer(body?: Buffer): Promise<{ endpoint: string }> {
  const root = await mkdtemp(join(tmpdir(), 'matou-control-raw-'))
  roots.push(root)
  const endpoint = join(root, 'control.sock')
  const server = createServer((socket) => {
    sockets.add(socket)
    socket.once('close', () => sockets.delete(socket))
    if (body === undefined) return
    socket.once('data', () => {
      const prefix = Buffer.allocUnsafe(4)
      prefix.writeUInt32BE(body.byteLength)
      socket.write(Buffer.concat([prefix, body]))
    })
  })
  servers.push(server)
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject)
    server.listen(endpoint, resolve)
  })
  return { endpoint }
}
