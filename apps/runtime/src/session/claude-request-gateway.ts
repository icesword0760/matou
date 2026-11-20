import type { EgressFamily } from '@matou/contracts'
import { randomBytes } from 'node:crypto'
import http, { type IncomingHttpHeaders, type IncomingMessage, type ServerResponse } from 'node:http'
import https from 'node:https'
import { Transform } from 'node:stream'
import { aborted } from 'node:util'
import { brotliDecompressSync, gunzipSync, inflateSync } from 'node:zlib'
import { HttpsProxyAgent } from 'https-proxy-agent'
import { SocksProxyAgent } from 'socks-proxy-agent'
import { resolveProxyForUrl } from '../egress/proxy-route'
import { sessionEn, sessionZhCN } from '../i18n/messages/session'
import { convertDeepSeekInlineTools, DeepSeekInlineToolCompatibilityError, isDeepSeekMessagesDestination } from './deepseek-inline-tools'
import { DeepSeekContextUsageTracker, type SessionContextUsageSnapshot } from './deepseek-token-usage'
import { createGeminiSseKeepAliveStream, isGeminiModelId } from './gemini-sse-keepalive'

const MAX_REQUEST_BYTES = 64 * 1024 * 1024
const HOP_BY_HOP_HEADERS = [
  'connection', 'keep-alive', 'proxy-authenticate', 'proxy-authorization',
  'proxy-connection', 'te', 'trailer', 'transfer-encoding', 'upgrade'
]
const AUTH_HEADER = /(^|[-_])(auth(?:entication|orization)?|oauth|(?:api|subscription|access)[-_]?key|token|secret|credentials?|signature)(?=$|[-_])/
const UPSTREAM_CONNECTION_ERROR = 'Claude request gateway could not reach the upstream service.'
const ROUTER_CONNECTION_ERRORS = new Set(['ECONNREFUSED', 'ECONNRESET', 'EPIPE', 'ETIMEDOUT'])

export class GatewayRoutingError extends Error {}

interface RouterOrigin { origin?: string }

export interface GatewayAuthenticationTarget {
  providerId: string
  providerName: string
  accountIdentity?: string
  label: string
}
export interface GatewayOverride {
  routerTarget?: true
  authentication?: GatewayAuthenticationTarget
  family?: EgressFamily
  targetSignal?: AbortSignal
  baseUrl?: string
  headers?: Record<string, string>
  model?: string
  effort?: 'omit' | 'low' | 'medium' | 'high' | 'xhigh' | 'max'
  thinking?: 'adaptive' | 'omit'
  maxOutputTokens?: number
  contextWindow?: number
  streamKeepAlive?: boolean
}

interface GatewayOptions {
  upstreamUrl: string
  env: NodeJS.ProcessEnv
  contextWindow?: number | null
  readContextUsage?: () => SessionContextUsageSnapshot | undefined
  sseKeepAliveIntervalMs?: number
  assertTargetAllowed?: (family?: EgressFamily) => void | Promise<void>
  onAuthenticationFailure?: (target: GatewayAuthenticationTarget) => void | Promise<void>
  /** Restarts the shared router when needed and returns its current base URL. */
  recoverRouter?: () => Promise<string | void>
  resolveOverride: (input: { compaction: boolean; webSearch?: boolean; model: string; kind?: 'message' | 'count_tokens' }) =>
    GatewayOverride | undefined | Promise<GatewayOverride | undefined>
}

class RequestTooLarge extends Error {}

export async function startClaudeRequestGateway(options: GatewayOptions): Promise<{
  baseUrl: string
  close(): Promise<void>
}> {
  const upstream = parseUpstream(options.upstreamUrl)
  const prefix = `/${randomBytes(32).toString('base64url')}`
  const active = new Set<AbortController>()
  const agents = new Map<string, http.Agent>()
  // Sessions keep the router URL captured at launch; after a recovery that had
  // to move ports, every later router request follows the recovered origin.
  const router: RouterOrigin = {}
  const usageTracker = new DeepSeekContextUsageTracker()
  const getAgent = (destination: URL) => {
    const loopback = ['127.0.0.1', 'localhost', '[::1]'].includes(destination.hostname)
    const proxyUrl = loopback ? undefined : resolveProxyForUrl(options.env, destination.href)
    const key = `${destination.protocol} ${proxyUrl ?? ''}`
    let agent = agents.get(key)
    if (!agent) {
      agent = createAgent(destination.protocol, proxyUrl)
      agents.set(key, agent)
    }
    return agent
  }
  const server = http.createServer((request, response) => {
    const controller = new AbortController()
    active.add(controller)
    request.once('error', () => controller.abort())
    response.once('close', () => {
      active.delete(controller)
      if (!response.writableFinished || !request.complete) controller.abort()
    })
    void forwardRequest(request, response, controller, prefix, upstream, getAgent, options, router, usageTracker).catch((error: unknown) => {
      if (controller.signal.aborted) return
      if (error instanceof GatewayRoutingError) {
        sendError(response, 502, error.message)
      } else if (error instanceof RequestTooLarge) {
        sendError(response, 413, 'Claude request exceeds the gateway size limit.')
      } else if (error instanceof DeepSeekInlineToolCompatibilityError) {
        sendError(response, 400, error.message)
      } else {
        sendError(response, 502, UPSTREAM_CONNECTION_ERROR)
      }
    })
  })

  const port = await new Promise<number>((resolve, reject) => {
    server.once('error', reject)
    server.listen(0, '127.0.0.1', () => {
      server.off('error', reject)
      const address = server.address()
      if (address && typeof address === 'object') resolve(address.port)
    })
  })

  let closing: Promise<void> | undefined
  return {
    baseUrl: `http://127.0.0.1:${port}${prefix}`,
    close() {
      closing ??= new Promise<void>((resolve) => {
        server.close(() => resolve())
        for (const controller of active) controller.abort()
        for (const agent of agents.values()) agent.destroy()
        server.closeAllConnections()
      })
      return closing
    }
  }
}

async function forwardRequest(
  request: IncomingMessage,
  response: ServerResponse,
  controller: AbortController,
  prefix: string,
  upstream: URL,
  getAgent: (destination: URL) => http.Agent,
  { resolveOverride, assertTargetAllowed, onAuthenticationFailure, recoverRouter, env, contextWindow, readContextUsage, sseKeepAliveIntervalMs }: GatewayOptions,
  router: RouterOrigin,
  usageTracker: DeepSeekContextUsageTracker
): Promise<void> {
  const locale = env.MATOU_LOCALE
  const route = readRoute(request.url, prefix)
  if (!route) {
    sendError(response, 404, 'Claude request gateway route was not found.')
    return
  }
  if (Number(request.headers['content-length']) > MAX_REQUEST_BYTES) throw new RequestTooLarge()

  let body: Buffer | undefined
  let override: GatewayOverride | undefined
  let rewritten = false
  let compactionMarked = false
  let message: Record<string, unknown> | undefined
  if (request.method === 'POST' && ['/v1/messages', '/v1/messages/count_tokens'].includes(route.path)) {
    body = await readBody(request)
    message = parseMessage(body)
    if (message && typeof message.model === 'string') {
      const marker = request.headers['x-claude-code-compaction']
      compactionMarked = marker === 'manual' || marker === 'auto'
      if (route.path === '/v1/messages' && compactionMarked) usageTracker.noteCompaction()
      const webSearch = !compactionMarked && Array.isArray(message.tools) && message.tools.length > 0 &&
        message.tools.every(tool => typeof object(tool)?.type === 'string' && (object(tool)!.type as string).startsWith('web_search_'))
      override = await resolveOverride({
        compaction: route.path === '/v1/messages' && compactionMarked, ...(webSearch ? { webSearch: true } : {}),
        model: message.model, kind: route.path === '/v1/messages/count_tokens' ? 'count_tokens' : 'message'
      })
      if (route.path === '/v1/messages/count_tokens' && override) {
        const { effort: _effort, thinking: _thinking, maxOutputTokens: _maxOutputTokens, ...countOverride } = override
        override = countOverride
      }
      if (override && rewriteMessage(message, override)) {
        body = Buffer.from(JSON.stringify(message))
        rewritten = true
      }
    }
  }
  if (controller.signal.aborted) return
  const targetSignal = override?.targetSignal
  if (targetSignal) {
    const revoked = () => {
      const reason = targetSignal.reason
      sendError(response, 502, reason instanceof Error ? reason.message : 'Subscription account changed. Restart this session.')
      controller.abort()
    }
    if (targetSignal.aborted) { revoked(); return }
    targetSignal.addEventListener('abort', revoked, { once: true })
    const cleanup = () => targetSignal.removeEventListener('abort', revoked)
    response.once('close', cleanup)
    controller.signal.addEventListener('abort', cleanup, { once: true })
  }

  const target = override?.baseUrl === undefined ? upstream : parseUpstream(override.baseUrl)
  const destination = new URL(target)
  destination.pathname = target.pathname.replace(/\/+$/, '') + route.path
  if (route.query) destination.search += destination.search ? `&${route.query}` : `?${route.query}`
  const followRouter = () => { if (override?.routerTarget === true && router.origin) destination.host = new URL(router.origin).host }
  followRouter()
  if (message && isDeepSeekMessagesDestination(destination)) {
    let converted = convertDeepSeekInlineTools(message)
    if (route.path === '/v1/messages' && !compactionMarked) {
      const sessionUsage = readContextUsage?.()
      converted = usageTracker.attach(converted, {
        ...(contextWindow !== undefined ? { sessionContextWindow: contextWindow } : {}),
        ...(override?.contextWindow !== undefined ? { overrideContextWindow: override.contextWindow } : {}),
        ...(sessionUsage ? { sessionUsage } : {})
      })
    }
    if (converted !== message) {
      body = Buffer.from(JSON.stringify(converted))
      rewritten = true
    }
  }

  const headers = endToEndHeaders(request.headers)
  delete headers.host
  delete headers.expect
  if (override?.headers || target.origin !== upstream.origin) {
    for (const name of Object.keys(headers)) {
      if (name === 'cookie' || AUTH_HEADER.test(name)) {
        delete headers[name]
      }
    }
    for (const [name, value] of Object.entries(endToEndHeaders(lowercaseHeaders(override?.headers)))) {
      if (name === 'anthropic-beta' && headers[name]) headers[name] = [...new Set((String(headers[name]) + ',' + String(value)).split(',').map(v => v.trim()).filter(Boolean))].join(',')
      else headers[name] = value
    }
  }
  if (body && rewritten) {
    headers['content-length'] = String(body.length)
    delete headers['content-md5']
    delete headers['content-digest']
    delete headers.digest
  }

  await assertTargetAllowed?.(override?.family)
  if (controller.signal.aborted) return
  const transport = destination.protocol === 'https:' ? https : http
  const forwardResponse = (incoming: IncomingMessage) => {
    if (controller.signal.aborted) {
      incoming.destroy()
      return
    }
    if (incoming.statusCode && incoming.statusCode >= 300 && incoming.statusCode < 400 && incoming.headers.location) {
      incoming.destroy()
      sendError(response, 502, 'Claude request gateway cannot forward an upstream redirect.')
      return
    }
    incoming.once('error', () => response.destroy())
    if (override?.authentication && (incoming.statusCode ?? 0) >= 400) {
      void forwardAuthenticatedError(incoming, response, override.authentication, onAuthenticationFailure, locale).catch(() => {
        sendError(response, 502, 'Claude request gateway could not read the upstream response.')
      })
      return
    }
    if (request.method === 'POST' && route.path === '/v1/messages' && !compactionMarked &&
        (incoming.statusCode ?? 0) >= 200 && (incoming.statusCode ?? 0) < 300) {
      usageTracker.observeResponse(incoming)
    }
    const targetModel = override?.model ?? (typeof message?.model === 'string' ? message.model : undefined)
    const geminiKeepAlive = override?.streamKeepAlive ?? isGeminiModelId(targetModel)
    const contentType = String(incoming.headers['content-type'] ?? '').toLowerCase()
    const contentEncoding = String(incoming.headers['content-encoding'] ?? '').trim().toLowerCase()
    const injectSseKeepAlive = Boolean(
      geminiKeepAlive &&
      request.method === 'POST' &&
      route.path === '/v1/messages' &&
      message?.stream === true &&
      (incoming.statusCode ?? 0) >= 200 &&
      (incoming.statusCode ?? 0) < 300 &&
      contentType.includes('text/event-stream') &&
      (contentEncoding === '' || contentEncoding === 'identity')
    )
    const outHeaders = endToEndHeaders(incoming.headers)
    if (injectSseKeepAlive) delete outHeaders['content-length']
    response.writeHead(incoming.statusCode ?? 502, outHeaders)
    if (injectSseKeepAlive) {
      const keepAlive = createGeminiSseKeepAliveStream(
        sseKeepAliveIntervalMs === undefined ? undefined : { intervalMs: sseKeepAliveIntervalMs }
      )
      response.once('close', () => keepAlive.destroy())
      incoming.once('error', () => keepAlive.destroy())
      incoming.pipe(keepAlive).pipe(response)
      return
    }
    incoming.pipe(response)
  }
  const send = () => new Promise<NodeJS.ErrnoException | undefined>(resolve => {
    let responseStarted = false
    const outgoing = transport.request(destination, {
      method: request.method,
      headers,
      signal: controller.signal,
      agent: getAgent(destination)
    }, incoming => {
      responseStarted = true
      resolve(undefined)
      forwardResponse(incoming)
    })
    outgoing.once('error', (error: NodeJS.ErrnoException) => {
      if (responseStarted && !controller.signal.aborted) sendError(response, 502, UPSTREAM_CONNECTION_ERROR)
      resolve(error)
    })
    if (body) {
      outgoing.end(body)
      return
    }
    let size = 0
    const limiter = new Transform({
      transform(chunk: Buffer, _encoding, callback) {
        size += chunk.length
        if (size > MAX_REQUEST_BYTES) callback(new RequestTooLarge())
        else callback(null, chunk)
      }
    })
    limiter.once('error', () => {
      sendError(response, 413, 'Claude request exceeds the gateway size limit.')
      controller.abort()
    })
    request.pipe(limiter).pipe(outgoing)
  })

  const error = await send()
  if (!error || controller.signal.aborted) return
  if (!body || override?.routerTarget !== true || !recoverRouter || !ROUTER_CONNECTION_ERRORS.has(error.code ?? '')) {
    sendError(response, 502, UPSTREAM_CONNECTION_ERROR)
    return
  }
  const messages = locale?.startsWith('en') ? sessionEn : sessionZhCN
  try {
    const recovered = await Promise.race([recoverRouter(), aborted(controller.signal, response).then(() => undefined)])
    if (typeof recovered === 'string') router.origin = new URL(recovered).origin
  } catch {
    if (!controller.signal.aborted) sendError(response, 502, messages.modelRouterRecoveryFailed)
    return
  }
  if (controller.signal.aborted) return
  await assertTargetAllowed?.(override.family)
  if (controller.signal.aborted) return
  followRouter()
  const retryError = await send()
  if (retryError && !controller.signal.aborted) sendError(response, 502, messages.modelRouterRecoveryFailed)
}

/** Only failed OAuth/subscription responses are inspected; successful streams stay streaming. */
async function forwardAuthenticatedError(incoming: IncomingMessage, response: ServerResponse, target: GatewayAuthenticationTarget,
  notify: GatewayOptions['onAuthenticationFailure'], locale?: string): Promise<void> {
  const status = incoming.statusCode ?? 502
  const expired = async () => {
    // State notification must never leak service/credential diagnostics to the CLI.
    try { await notify?.(target) } catch { /* Still return actionable, sanitized feedback. */ }
    const label = target.label === 'webSearch' ? (locale?.startsWith('en') ? 'Web search' : '网页搜索') : target.label
    const message = locale?.startsWith('en')
      ? `${label} · ${target.providerName} sign-in has expired. Go to Settings → Claude Code, sign in again, then retry.`
      : `${label} · ${target.providerName} 登录已过期，请到 设置 → Claude Code 重新登录后重试。`
    sendError(response, status, message, 'authentication_error')
  }
  if (status === 401 || status === 403) { incoming.resume(); await expired(); return }
  // Inspect a bounded error body; larger non-auth errors remain byte-for-byte passthrough.
  const chunks: Buffer[] = []
  let size = 0, passthrough = false
  for await (const chunk of incoming) {
    if (passthrough) { response.write(chunk); continue }
    chunks.push(chunk); size += chunk.length
    if (size > 256 * 1024) {
      passthrough = true
      response.writeHead(status, endToEndHeaders(incoming.headers))
      for (const buffered of chunks) response.write(buffered)
      chunks.length = 0
    }
  }
  if (passthrough) { response.end(); return }
  const body = Buffer.concat(chunks, size)
  let decoded = body
  try {
    const encoding = incoming.headers['content-encoding']
    const decompress = encoding === 'gzip' ? gunzipSync : encoding === 'deflate' ? inflateSync : encoding === 'br' ? brotliDecompressSync : undefined
    if (decompress) decoded = decompress(body, { maxOutputLength: 256 * 1024 })
  } catch { /* Malformed or oversized encoded errors retain their original bytes. */ }
  const error = object(parseMessage(decoded)?.error)
  const codes = [error?.code, error?.type]
  const explicit = codes.some(code => typeof code === 'string' && ['invalid_grant', 'invalid_token', 'token_expired', 'authentication_error', 'login_required', 'reauthentication_required'].includes(code)) ||
    typeof error?.message === 'string' && /(?:\b(?:access token|refresh token|oauth token|session|authentication|credentials?)\b.{0,40}\b(?:expired|revoked)\b|\b(?:please|must|need to|required to) (?:re-?authenticate|(?:sign|log) in again)\b)/i.test(error.message)
  if (explicit) { await expired(); return }
  response.writeHead(status, endToEndHeaders(incoming.headers)); response.end(body)
}

function parseUpstream(value: string): URL {
  const url = new URL(value)
  if ((url.protocol !== 'http:' && url.protocol !== 'https:') || url.username || url.password || url.hash) {
    throw new Error('Invalid Claude request gateway upstream URL')
  }
  return url
}

function readRoute(url: string | undefined, prefix: string): { path: string; query: string } | undefined {
  if (!url?.startsWith(`${prefix}/`) || url.includes('#')) return undefined
  const route = url.slice(prefix.length)
  const queryStart = route.indexOf('?')
  const path = queryStart === -1 ? route : route.slice(0, queryStart)
  // Reject paths that could be interpreted as a different route after URL normalization.
  if (path.startsWith('//') || /[\\]|%2f|%5c|%25/i.test(path) ||
      path.split('/').some((segment) => /^\.{1,2}$/.test(segment.replace(/%2e/gi, '.')))) return undefined
  return { path, query: queryStart === -1 ? '' : route.slice(queryStart + 1) }
}

function endToEndHeaders(source: IncomingHttpHeaders): IncomingHttpHeaders {
  const headers = { ...source }
  for (const name of source.connection?.split(',') ?? []) delete headers[name.trim().toLowerCase()]
  for (const name of HOP_BY_HOP_HEADERS) delete headers[name]
  return headers
}

function lowercaseHeaders(headers?: Record<string, string>): Record<string, string> {
  return Object.fromEntries(Object.entries(headers ?? {}).map(([name, value]) => [name.toLowerCase(), value]))
}

function createAgent(protocol: string, proxyUrl: string | undefined): http.Agent {
  const options = { keepAlive: true }
  if (!proxyUrl) return protocol === 'https:' ? new https.Agent(options) : new http.Agent(options)
  const proxyProtocol = new URL(proxyUrl).protocol
  if (proxyProtocol.startsWith('socks')) return new SocksProxyAgent(proxyUrl, options)
  if (proxyProtocol === 'http:' || proxyProtocol === 'https:') return new HttpsProxyAgent(proxyUrl, options)
  throw new Error('Unsupported Claude request gateway proxy')
}

async function readBody(request: IncomingMessage): Promise<Buffer> {
  const chunks: Buffer[] = []
  let size = 0
  for await (const chunk of request.iterator({ destroyOnReturn: false })) {
    size += chunk.length
    if (size > MAX_REQUEST_BYTES) throw new RequestTooLarge()
    chunks.push(chunk)
  }
  return Buffer.concat(chunks, size)
}

function object(value: unknown): Record<string, unknown> | undefined {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    ? value as Record<string, unknown>
    : undefined
}

function parseMessage(body: Buffer): Record<string, unknown> | undefined {
  try {
    return object(JSON.parse(body.toString('utf8')))
  } catch {
    return undefined
  }
}

function rewriteMessage(message: Record<string, unknown>, override: GatewayOverride): boolean {
  const cap = override.maxOutputTokens
  const capped = cap !== undefined && typeof message.max_tokens === 'number' && message.max_tokens > cap
  if (override.model !== undefined) message.model = override.model
  if (override.effort === 'omit') {
    const config = { ...object(message.output_config) }; delete config.effort
    if (Object.keys(config).length) message.output_config = config
    else delete message.output_config
  } else if (override.effort) {
    message.output_config = { ...object(message.output_config), effort: override.effort }
  }
  if (override.thinking === 'adaptive') message.thinking = { type: 'adaptive' }
  if (override.thinking === 'omit') delete message.thinking
  if (capped) message.max_tokens = cap
  return override.model !== undefined || override.effort !== undefined ||
    override.thinking !== undefined || capped
}

function sendError(response: ServerResponse, status: number, message: string, type = status >= 500 ? 'api_error' : 'invalid_request_error'): void {
  if (response.destroyed) return
  if (response.headersSent) {
    response.destroy()
    return
  }
  response.writeHead(status, { 'content-type': 'application/json', connection: 'close' })
  response.end(JSON.stringify({
    type: 'error',
    error: { type, message }
  }))
}
