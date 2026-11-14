import { randomBytes } from 'node:crypto'
import http, { type IncomingHttpHeaders, type IncomingMessage, type ServerResponse } from 'node:http'
import https from 'node:https'
import { Transform } from 'node:stream'
import { HttpsProxyAgent } from 'https-proxy-agent'
import { SocksProxyAgent } from 'socks-proxy-agent'
import { resolveProxyForUrl } from '../egress/proxy-route'

const MAX_REQUEST_BYTES = 64 * 1024 * 1024
const HOP_BY_HOP_HEADERS = [
  'connection', 'keep-alive', 'proxy-authenticate', 'proxy-authorization',
  'proxy-connection', 'te', 'trailer', 'transfer-encoding', 'upgrade'
]
const AUTH_HEADER = /(^|[-_])(auth(?:entication|orization)?|oauth|(?:api|subscription|access)[-_]?key|token|secret|credentials?|signature)(?=$|[-_])/

export interface GatewayOverride {
  baseUrl?: string
  headers?: Record<string, string>
  model?: string
  effort?: 'low' | 'medium' | 'high' | 'xhigh' | 'max'
  thinking?: 'adaptive' | 'omit'
  maxOutputTokens?: number
}

interface GatewayOptions {
  upstreamUrl: string
  env: NodeJS.ProcessEnv
  resolveOverride: (input: { compaction: boolean; model: string }) =>
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
    void forwardRequest(request, response, controller, prefix, upstream, options.resolveOverride, getAgent).catch((error: unknown) => {
      if (controller.signal.aborted) return
      if (error instanceof RequestTooLarge) {
        sendError(response, 413, 'Claude request exceeds the gateway size limit.')
      } else {
        sendError(response, 502, 'Claude request gateway could not reach the upstream service.')
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
  resolveOverride: GatewayOptions['resolveOverride'],
  getAgent: (destination: URL) => http.Agent
): Promise<void> {
  const route = readRoute(request.url, prefix)
  if (!route) {
    sendError(response, 404, 'Claude request gateway route was not found.')
    return
  }
  if (Number(request.headers['content-length']) > MAX_REQUEST_BYTES) throw new RequestTooLarge()

  let body: Buffer | undefined
  let override: GatewayOverride | undefined
  let rewritten = false
  if (request.method === 'POST' && ['/v1/messages', '/v1/messages/count_tokens'].includes(route.path)) {
    body = await readBody(request)
    const message = parseMessage(body)
    if (message && typeof message.model === 'string') {
      const marker = request.headers['x-claude-code-compaction']
      override = await resolveOverride({
        compaction: route.path === '/v1/messages' && (marker === 'manual' || marker === 'auto'),
        model: message.model
      })
      if (route.path === '/v1/messages/count_tokens') override = override?.model ? { model: override.model } : undefined
      if (override && rewriteMessage(message, override)) {
        body = Buffer.from(JSON.stringify(message))
        rewritten = true
      }
    }
  }
  if (controller.signal.aborted) return

  const target = override?.baseUrl === undefined ? upstream : parseUpstream(override.baseUrl)
  const destination = new URL(target)
  destination.pathname = target.pathname.replace(/\/+$/, '') + route.path
  if (route.query) destination.search += destination.search ? `&${route.query}` : `?${route.query}`

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

  const transport = destination.protocol === 'https:' ? https : http
  const outgoing = transport.request(destination, {
    method: request.method,
    headers,
    signal: controller.signal,
    agent: getAgent(destination)
  }, (incoming) => {
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
    response.writeHead(incoming.statusCode ?? 502, endToEndHeaders(incoming.headers))
    incoming.pipe(response)
  })
  outgoing.once('error', () => {
    if (!controller.signal.aborted) {
      sendError(response, 502, 'Claude request gateway could not reach the upstream service.')
    }
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
  if (override.effort) {
    message.output_config = { ...object(message.output_config), effort: override.effort }
  }
  if (override.thinking === 'adaptive') message.thinking = { type: 'adaptive' }
  if (override.thinking === 'omit') delete message.thinking
  if (capped) message.max_tokens = cap
  return override.model !== undefined || override.effort !== undefined ||
    override.thinking !== undefined || capped
}

function sendError(response: ServerResponse, status: number, message: string): void {
  if (response.destroyed) return
  if (response.headersSent) {
    response.destroy()
    return
  }
  response.writeHead(status, { 'content-type': 'application/json', connection: 'close' })
  response.end(JSON.stringify({
    type: 'error',
    error: { type: status >= 500 ? 'api_error' : 'invalid_request_error', message }
  }))
}
