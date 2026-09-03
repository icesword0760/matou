import { randomUUID } from 'node:crypto'
import { connect, type Socket } from 'node:net'

import type { HostControlErrorDetails, HostControlScope } from './host-control-types'

export interface HostControlClientOptions {
  endpoint: string
  token: string
  timeoutMs?: number
}

interface ControlResponse {
  version: number
  requestId: string
  ok: boolean
  result?: unknown
  error?: { code: string; message: string; details?: unknown }
}

export class HostControlClientError extends Error {
  readonly code: string
  readonly details?: HostControlErrorDetails

  constructor(code: string, message: string, details?: HostControlErrorDetails) {
    super(message)
    this.code = code
    if (details !== undefined) this.details = details
  }
}

export class HostControlClient {
  readonly #endpoint: string
  readonly #token: string
  readonly #timeoutMs: number

  constructor(options: HostControlClientOptions) {
    this.#endpoint = options.endpoint
    this.#token = options.token
    this.#timeoutMs = options.timeoutMs ?? 5_000
  }

  async request(method: HostControlScope, params: unknown): Promise<unknown> {
    const requestId = randomUUID()
    const deadlineAt = Date.now() + this.#timeoutMs
    const socket = await connectWithin(this.#endpoint, this.#timeoutMs)
    try {
      const body = Buffer.from(JSON.stringify({
        version: 1,
        requestId,
        token: this.#token,
        method,
        params,
        deadlineAt
      }))
      const prefix = Buffer.allocUnsafe(4)
      prefix.writeUInt32BE(body.byteLength)
      socket.write(Buffer.concat([prefix, body]))
      const response = JSON.parse((await readFrame(socket, this.#timeoutMs)).toString('utf8')) as ControlResponse
      if (response.requestId !== requestId) {
        throw new HostControlClientError('INVALID_RESPONSE', 'Host Control 返回了不匹配的请求标识')
      }
      if (!response.ok) {
        throw new HostControlClientError(
          response.error?.code ?? 'INTERNAL_ERROR',
          response.error?.message ?? 'Host Control 请求失败',
          parseErrorDetails(response.error?.code, response.error?.details)
        )
      }
      return response.result
    } finally {
      socket.end()
    }
  }
}

function parseErrorDetails(code: string | undefined, value: unknown): HostControlErrorDetails | undefined {
  if (code !== 'AMBIGUOUS_TARGET' || typeof value !== 'object' || value === null || Array.isArray(value)) {
    return undefined
  }
  const rawCandidates = (value as { candidates?: unknown }).candidates
  if (!Array.isArray(rawCandidates)) return undefined
  const candidates = rawCandidates.slice(0, 5).flatMap((candidate) => {
    if (typeof candidate !== 'object' || candidate === null || Array.isArray(candidate)) return []
    const humanPath = (candidate as { humanPath?: unknown }).humanPath
    if (
      typeof humanPath !== 'string' ||
      !humanPath.trim() ||
      Buffer.byteLength(humanPath, 'utf8') > 4_096
    ) return []
    return [{ humanPath }]
  })
  return candidates.length > 0 ? { candidates } : undefined
}

function connectWithin(endpoint: string, timeoutMs: number): Promise<Socket> {
  return new Promise((resolve, reject) => {
    const socket = connect(endpoint)
    const timer = setTimeout(() => {
      socket.destroy()
      reject(new HostControlClientError('TIMEOUT', '连接 Matou Host Control 超时'))
    }, timeoutMs)
    socket.once('connect', () => {
      clearTimeout(timer)
      resolve(socket)
    })
    socket.once('error', (error) => {
      clearTimeout(timer)
      reject(new HostControlClientError('CONNECTION_ERROR', error.message))
    })
  })
}

function readFrame(socket: Socket, timeoutMs: number): Promise<Buffer> {
  return new Promise((resolve, reject) => {
    let buffered = Buffer.alloc(0)
    const timer = setTimeout(() => {
      cleanup()
      reject(new HostControlClientError('TIMEOUT', '等待 Matou Host Control 响应超时'))
    }, timeoutMs)
    const onData = (chunk: Buffer) => {
      buffered = Buffer.concat([buffered, chunk])
      if (buffered.byteLength < 4) return
      const length = buffered.readUInt32BE(0)
      if (length > 1024 * 1024) {
        cleanup()
        reject(new HostControlClientError('INVALID_RESPONSE', 'Host Control 响应超过大小限制'))
        return
      }
      if (buffered.byteLength < length + 4) return
      const body = buffered.subarray(4, length + 4)
      cleanup()
      resolve(body)
    }
    const onError = (error: Error) => {
      cleanup()
      reject(new HostControlClientError('CONNECTION_ERROR', error.message))
    }
    const cleanup = () => {
      clearTimeout(timer)
      socket.off('data', onData)
      socket.off('error', onError)
    }
    socket.on('data', onData)
    socket.on('error', onError)
  })
}
