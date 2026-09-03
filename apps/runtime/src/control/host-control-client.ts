import { randomUUID } from 'node:crypto'
import { connect, type Socket } from 'node:net'

import {
  HOST_CONTROL_MAX_FRAME_BYTES,
  type HostControlErrorDetails,
  type HostControlScope
} from './host-control-types'

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
  declare readonly details?: HostControlErrorDetails

  constructor(code: string, message: string, details?: HostControlErrorDetails, options?: ErrorOptions) {
    super(message, options)
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
    const body = Buffer.from(JSON.stringify({
      version: 1,
      requestId,
      token: this.#token,
      method,
      params,
      deadlineAt
    }))
    if (body.byteLength > HOST_CONTROL_MAX_FRAME_BYTES) {
      throw new HostControlClientError('INVALID_REQUEST', 'Host Control 请求超过大小限制')
    }
    const socket = await connectWithin(this.#endpoint, this.#timeoutMs)
    try {
      const prefix = Buffer.allocUnsafe(4)
      prefix.writeUInt32BE(body.byteLength)
      socket.write(Buffer.concat([prefix, body]))
      const response = parseControlResponse(await readFrame(socket, this.#timeoutMs))
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

function parseControlResponse(body: Buffer): ControlResponse {
  let value: unknown
  try {
    value = JSON.parse(body.toString('utf8'))
  } catch (cause) {
    throw invalidResponse(cause)
  }
  if (typeof value !== 'object' || value === null || Array.isArray(value)) throw invalidResponse()
  const response = value as Partial<ControlResponse>
  if (
    response.version !== 1 ||
    typeof response.requestId !== 'string' ||
    typeof response.ok !== 'boolean'
  ) throw invalidResponse()
  if (response.error !== undefined && (
    typeof response.error !== 'object' || response.error === null ||
    typeof response.error.code !== 'string' || typeof response.error.message !== 'string'
  )) throw invalidResponse()
  return response as ControlResponse
}

function invalidResponse(cause?: unknown): HostControlClientError {
  return new HostControlClientError(
    'INVALID_RESPONSE',
    'Matou Host Control 返回了无效响应；请重试',
    undefined,
    cause === undefined ? undefined : { cause }
  )
}

function parseErrorDetails(code: string | undefined, value: unknown): HostControlErrorDetails | undefined {
  if (code !== 'AMBIGUOUS_TARGET' || typeof value !== 'object' || value === null || Array.isArray(value)) {
    return undefined
  }
  const rawCandidates = (value as { candidates?: unknown }).candidates
  if (!Array.isArray(rawCandidates)) return undefined
  const candidates: Array<{ humanPath: string }> = []
  for (const candidate of rawCandidates) {
    if (typeof candidate !== 'object' || candidate === null || Array.isArray(candidate)) return undefined
    const humanPath = (candidate as { humanPath?: unknown }).humanPath
    if (
      typeof humanPath !== 'string' ||
      !humanPath.trim() ||
      Buffer.byteLength(humanPath, 'utf8') > 4_096
    ) return undefined
    candidates.push({ humanPath })
  }
  return candidates.length > 0 ? { candidates } : undefined
}

function connectWithin(endpoint: string, timeoutMs: number): Promise<Socket> {
  return new Promise((resolve, reject) => {
    const socket = connect(endpoint)
    const timer = setTimeout(() => {
      cleanup()
      socket.destroy()
      reject(new HostControlClientError(
        'TIMEOUT',
        '连接 Matou Host Control 超时；请确认 Matou 仍在运行后重试'
      ))
    }, timeoutMs)
    const onConnect = () => {
      cleanup()
      resolve(socket)
    }
    const onError = (cause: Error) => {
      cleanup()
      socket.destroy()
      reject(new HostControlClientError(
        'CONNECTION_ERROR',
        '未连接到 Matou Host Control；请在 Matou 托管终端中重试',
        undefined,
        { cause }
      ))
    }
    const cleanup = () => {
      clearTimeout(timer)
      socket.off('connect', onConnect)
      socket.off('error', onError)
    }
    socket.once('connect', onConnect)
    socket.once('error', onError)
  })
}

function readFrame(socket: Socket, timeoutMs: number): Promise<Buffer> {
  return new Promise((resolve, reject) => {
    let buffered = Buffer.alloc(0)
    const timer = setTimeout(() => {
      cleanup()
      reject(new HostControlClientError(
        'TIMEOUT',
        '等待 Matou Host Control 响应超时；请确认 Matou 仍在运行后重试'
      ))
    }, timeoutMs)
    const onData = (chunk: Buffer) => {
      buffered = Buffer.concat([buffered, chunk])
      if (buffered.byteLength < 4) return
      const length = buffered.readUInt32BE(0)
      if (length > HOST_CONTROL_MAX_FRAME_BYTES) {
        cleanup()
        reject(new HostControlClientError('INVALID_RESPONSE', 'Host Control 响应超过大小限制'))
        return
      }
      if (buffered.byteLength < length + 4) return
      const body = buffered.subarray(4, length + 4)
      cleanup()
      resolve(body)
    }
    const onError = (cause: Error) => {
      cleanup()
      reject(new HostControlClientError(
        'CONNECTION_ERROR',
        '与 Matou Host Control 的连接已中断；请重试',
        undefined,
        { cause }
      ))
    }
    const onEnd = () => {
      cleanup()
      reject(invalidResponse())
    }
    const cleanup = () => {
      clearTimeout(timer)
      socket.off('data', onData)
      socket.off('error', onError)
      socket.off('end', onEnd)
    }
    socket.on('data', onData)
    socket.on('error', onError)
    socket.on('end', onEnd)
  })
}
