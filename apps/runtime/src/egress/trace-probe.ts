import https from 'node:https'
import net from 'node:net'
import type { EgressHostError, EgressHostSample } from '@matou/contracts'
import { HttpsProxyAgent } from 'https-proxy-agent'
import { SocksProxyAgent } from 'socks-proxy-agent'

const DEFAULT_TIMEOUT_MS = 5000
const MAX_TRACE_BYTES = 4096

class TraceInvalidError extends Error {
  constructor(message: string) {
    super(message)
    this.name = 'TraceInvalidError'
  }
}

class TraceTimeoutError extends Error {
  readonly code = 'ETIMEDOUT'

  constructor(message = 'Trace request timed out') {
    super(message)
    this.name = 'TraceTimeoutError'
  }
}

export type TraceRequester = (
  url: string,
  proxyUrl: string | undefined,
  timeoutMs: number
) => Promise<string>

export function parseTrace(body: string): { ip?: string; country?: string } {
  const result: { ip?: string; country?: string } = {}

  for (const rawLine of body.split(/\r?\n/)) {
    const eqIndex = rawLine.indexOf('=')
    if (eqIndex === -1) continue

    const key = rawLine.slice(0, eqIndex).trim()
    const value = rawLine.slice(eqIndex + 1).trim()

    if (key === 'ip' && net.isIP(value) !== 0) {
      result.ip = value
    } else if (key === 'loc' && /^[A-Z]{2}$/.test(value)) {
      result.country = value
    }
  }

  return result
}

export async function probeTrace(
  host: string,
  proxyUrl: string | undefined,
  options?: { timeoutMs?: number; request?: TraceRequester }
): Promise<EgressHostSample> {
  const timeoutMs = options?.timeoutMs ?? DEFAULT_TIMEOUT_MS
  const request = options?.request ?? defaultTraceRequester
  const url = `https://${host}/cdn-cgi/trace`

  let body: string
  try {
    body = await request(url, proxyUrl, timeoutMs)
  } catch (error) {
    return { host, error: classifyRequestError(error) }
  }

  const parsed = parseTrace(body)
  if (!parsed.ip || !parsed.country) {
    return { host, error: 'invalid' }
  }

  return { host, ip: parsed.ip, country: parsed.country }
}

function createAgent(proxyUrl: string | undefined): https.Agent | false {
  if (!proxyUrl) return false

  let protocol: string
  try {
    protocol = new URL(proxyUrl).protocol.toLowerCase()
  } catch {
    throw new TraceInvalidError(`Invalid proxy URL: ${proxyUrl}`)
  }

  if (protocol.startsWith('socks')) {
    return new SocksProxyAgent(proxyUrl)
  }
  if (protocol === 'http:' || protocol === 'https:') {
    return new HttpsProxyAgent(proxyUrl)
  }

  throw new TraceInvalidError(`Unsupported proxy protocol: ${protocol}`)
}

function defaultTraceRequester(
  url: string,
  proxyUrl: string | undefined,
  timeoutMs: number
): Promise<string> {
  return new Promise((resolve, reject) => {
    let agent: https.Agent | false
    try {
      agent = createAgent(proxyUrl)
    } catch (error) {
      reject(error)
      return
    }

    let settled = false
    const finish = (fn: () => void) => {
      if (settled) return
      settled = true
      clearTimeout(timer)
      fn()
    }

    const req = https.get(url, { agent, timeout: timeoutMs }, (res) => {
      const status = res.statusCode ?? 0
      if (status < 200 || status >= 300) {
        res.resume()
        finish(() => reject(new TraceInvalidError(`Unexpected HTTP status ${status}`)))
        return
      }

      const chunks: Buffer[] = []
      let totalBytes = 0

      res.on('data', (chunk: Buffer | string) => {
        const buf = typeof chunk === 'string' ? Buffer.from(chunk) : chunk
        totalBytes += buf.length
        if (totalBytes > MAX_TRACE_BYTES) {
          res.destroy()
          req.destroy()
          finish(() => reject(new TraceInvalidError('Trace response exceeded 4096 bytes')))
          return
        }
        chunks.push(buf)
      })

      res.on('end', () => {
        finish(() => resolve(Buffer.concat(chunks).toString('utf8')))
      })

      res.on('error', (error) => {
        finish(() => reject(error))
      })
    })

    const timer = setTimeout(() => {
      const timeoutError = new TraceTimeoutError()
      req.destroy(timeoutError)
      finish(() => reject(timeoutError))
    }, timeoutMs)

    req.on('timeout', () => {
      const timeoutError = new TraceTimeoutError()
      req.destroy(timeoutError)
      finish(() => reject(timeoutError))
    })

    req.on('error', (error) => {
      finish(() => reject(error))
    })
  })
}

function classifyRequestError(error: unknown): EgressHostError {
  if (error instanceof TraceInvalidError) return 'invalid'
  if (error instanceof TraceTimeoutError) return 'timeout'

  if (error && typeof error === 'object') {
    const record = error as { code?: unknown; name?: unknown; message?: unknown }
    if (
      record.code === 'ETIMEDOUT' ||
      record.code === 'ESOCKETTIMEDOUT' ||
      record.code === 'ERR_TIMED_OUT' ||
      record.name === 'TimeoutError' ||
      record.name === 'AbortError'
    ) {
      return 'timeout'
    }
    if (typeof record.message === 'string' && /\bETIMEDOUT\b|timed?\s*out/i.test(record.message)) {
      return 'timeout'
    }
  }

  return 'unreachable'
}
