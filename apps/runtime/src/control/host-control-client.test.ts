import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { createServer, type Server } from 'node:net'

import { afterEach, describe, expect, it } from 'vitest'

import { HostControlClient, HostControlClientError } from './host-control-client'

const roots: string[] = []
const servers: Server[] = []

afterEach(async () => {
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
})

async function fixtureServer(
  respond: (request: Record<string, any>) => Record<string, any>,
  split = false
): Promise<{ endpoint: string; requests: Record<string, any>[] }> {
  const root = await mkdtemp(join(tmpdir(), 'matou-control-client-'))
  roots.push(root)
  const endpoint = join(root, 'control.sock')
  const requests: Record<string, any>[] = []
  const server = createServer((socket) => {
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
