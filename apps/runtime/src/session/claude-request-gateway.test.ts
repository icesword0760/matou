import http, { type RequestListener, type Server } from 'node:http'
import { afterEach, describe, expect, it } from 'vitest'
import { startClaudeRequestGateway, type GatewayOverride } from './claude-request-gateway'

const dispose: Array<() => Promise<void>> = []

async function listen(handler: RequestListener): Promise<string> {
  const server: Server = http.createServer(handler)
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve))
  dispose.unshift(() => new Promise<void>((resolve) => {
    server.close(() => resolve())
    server.closeAllConnections()
  }))
  const address = server.address()
  if (!address || typeof address === 'string') throw new Error('Expected HTTP server')
  return `http://127.0.0.1:${address.port}`
}

async function start(
  upstreamUrl: string,
  resolveOverride: (input: { compaction: boolean; model: string }) => GatewayOverride | undefined = () => undefined
) {
  const gateway = await startClaudeRequestGateway({ upstreamUrl, env: {}, resolveOverride })
  dispose.unshift(gateway.close)
  return gateway
}

async function captureRequests() {
  const requests: Array<{ url: string | undefined; headers: http.IncomingHttpHeaders; body: string }> = []
  const url = await listen(async (request, response) => {
    const chunks = []
    for await (const chunk of request) chunks.push(chunk)
    requests.push({ url: request.url, headers: request.headers, body: Buffer.concat(chunks).toString() })
    response.end('accepted')
  })
  return { url, requests }
}

afterEach(async () => {
  for (const close of dispose.splice(0)) await close()
})

describe('Claude request gateway', () => {
  it('routes manual and automatic compaction separately and keeps subsequent chat on the primary model', async () => {
    const primary = await captureRequests()
    const secondary = await captureRequests()
    const gateway = await start(`${primary.url}/primary`, ({ compaction, model }) => {
      if (compaction) {
        return {
          baseUrl: `${secondary.url}/secondary`,
          headers: { Authorization: 'Bearer target', 'x-api-key': 'target-key' },
          model: 'summary-model',
          thinking: 'adaptive',
          effort: 'low',
          maxOutputTokens: 2048
        }
      }
      if (model === 'main-model') return { effort: 'high' }
      return undefined
    })
    const body = {
      model: 'main-model',
      max_tokens: 12000,
      messages: [{ role: 'user', content: 'sample' }],
      thinking: { type: 'enabled', budget_tokens: 1024 },
      output_config: { effort: 'medium', format: { type: 'json_schema' } }
    }
    const send = (compaction?: string, maxTokens = body.max_tokens) => fetch(`${gateway.baseUrl}/v1/messages?beta=true`, {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        'anthropic-version': '2023-06-01',
        authorization: 'Bearer source',
        cookie: 'source-cookie',
        'x-api-key': 'source-key',
        'x-custom-auth-token': 'source-token',
        'x-claude-code-compaction': compaction ?? ''
      },
      body: JSON.stringify({ ...body, max_tokens: maxTokens })
    })

    for (const marker of [undefined, 'manual', 'auto', 'other']) {
      expect(await (await send(marker)).text()).toBe('accepted')
    }
    expect(await (await send('auto', 1024)).text()).toBe('accepted')
    expect(primary.requests).toHaveLength(2)
    for (const request of primary.requests) {
      expect(request.url).toBe('/primary/v1/messages?beta=true')
      expect(request.headers.authorization).toBe('Bearer source')
      expect(JSON.parse(request.body)).toEqual({
        ...body, output_config: { ...body.output_config, effort: 'high' }
      })
    }
    expect(secondary.requests).toHaveLength(3)
    for (const [index, request] of secondary.requests.entries()) {
      expect(request.url).toBe('/secondary/v1/messages?beta=true')
      expect(request.headers.authorization).toBe('Bearer target')
      expect(request.headers['x-api-key']).toBe('target-key')
      expect(request.headers['x-custom-auth-token']).toBeUndefined()
      expect(request.headers.cookie).toBeUndefined()
      expect(request.headers['anthropic-version']).toBe('2023-06-01')
      expect(request.headers['content-length']).toBe(String(Buffer.byteLength(request.body)))
      expect(JSON.parse(request.body)).toEqual({
        ...body,
        model: 'summary-model',
        max_tokens: index === 2 ? 1024 : 2048,
        thinking: { type: 'adaptive' },
        output_config: { ...body.output_config, effort: 'low' }
      })
    }
  })

  it('passes original bytes through when there is no override and limits detection to message creation', async () => {
    const upstream = await captureRequests()
    const resolved: string[] = []
    const gateway = await start(upstream.url, ({ model }) => {
      resolved.push(model)
      return undefined
    })
    const body = '  { "model" : "original", "messages": [] }\n'
    for (const path of ['/v1/messages', '/v1/messages/count_tokens']) {
      const response = await fetch(gateway.baseUrl + path, {
        method: 'POST',
        headers: { 'x-claude-code-compaction': 'manual' },
        body
      })
      expect(await response.text()).toBe('accepted')
    }
    expect(resolved).toEqual(['original', 'original'])
    expect(upstream.requests.map((request) => request.body)).toEqual([body, body])
  })

  it('only remaps the model for count_tokens, even with a compaction marker', async () => {
    const primary = await captureRequests(), secondary = await captureRequests()
    const calls: boolean[] = []
    const gateway = await start(primary.url, ({ compaction }) => {
      calls.push(compaction)
      return { model: 'actual-model', baseUrl: secondary.url, headers: { authorization: 'Bearer secondary' }, effort: 'low' }
    })
    const body = { model: 'matou-api/sonnet', output_config: { effort: 'high' }, messages: [] }
    const response = await fetch(gateway.baseUrl + '/v1/messages/count_tokens', { method: 'POST', headers: { authorization: 'Bearer source', 'x-claude-code-compaction': 'manual' }, body: JSON.stringify(body) })
    expect(response.status).toBe(200)
    expect(calls).toEqual([false]); expect(secondary.requests).toHaveLength(0)
    expect(primary.requests[0]!.headers.authorization).toBe('Bearer source')
    expect(JSON.parse(primary.requests[0]!.body)).toEqual({ ...body, model: 'actual-model' })
  })

  it('keeps SSE data streaming and passes through upstream HTTP errors', async () => {
    let finish!: () => void
    const upstream = await listen((request, response) => {
      if (request.url === '/v1/models') {
        response.writeHead(429, { 'content-type': 'application/json', 'retry-after': '2' })
        response.end('{"type":"rate_limit_error"}')
        return
      }
      response.writeHead(200, { 'content-type': 'text/event-stream' })
      response.write('event: message_start\ndata: first\n\n')
      finish = () => response.end('event: error\ndata: upstream error\n\n')
    })
    const gateway = await start(upstream)
    const streamed = await fetch(`${gateway.baseUrl}/v1/messages`, { method: 'POST', body: '{}' })
    const reader = streamed.body!.getReader()
    const first = await reader.read()
    expect(new TextDecoder().decode(first.value)).toBe('event: message_start\ndata: first\n\n')
    finish()
    expect(new TextDecoder().decode((await reader.read()).value)).toBe('event: error\ndata: upstream error\n\n')
    expect((await reader.read()).done).toBe(true)

    const error = await fetch(`${gateway.baseUrl}/v1/models`)
    expect(error.status).toBe(429)
    expect(error.headers.get('retry-after')).toBe('2')
    expect(await error.text()).toBe('{"type":"rate_limit_error"}')
  })

  it('rejects invalid gateway paths and upstream redirects', async () => {
    const redirect = await captureRequests()
    const upstream = await listen((_request, response) => {
      response.writeHead(307, { location: redirect.url })
      response.end()
    })
    const gateway = await start(upstream)
    const noSecret = await fetch(new URL('/v1/models', gateway.baseUrl))
    expect(noSecret.status).toBe(404)
    const invalid = await fetch(gateway.baseUrl + '/%2Fv1/models')
    expect(invalid.status).toBe(404)
    const blocked = await fetch(gateway.baseUrl + '/v1/models', { headers: { 'x-api-key': 'source-key' } })
    expect(blocked.status).toBe(502)
    expect(blocked.headers.get('location')).toBeNull()
    expect(await blocked.text()).not.toContain(redirect.url)
    expect(redirect.requests).toHaveLength(0)
  })

  it('rejects oversized requests before contacting the upstream', async () => {
    const upstream = await captureRequests()
    const gateway = await start(upstream.url)
    const status = await new Promise<number | undefined>((resolve, reject) => {
      const request = http.request(gateway.baseUrl + '/v1/messages', {
        method: 'POST',
        headers: { 'content-length': String(64 * 1024 * 1024 + 1) }
      }, (response) => {
        response.resume()
        response.once('end', () => resolve(response.statusCode))
        response.once('error', reject)
      })
      request.once('error', reject)
      request.end()
    })
    expect(status).toBe(413)
    expect(upstream.requests).toHaveLength(0)
  })

  it('keeps local router traffic off configured network proxies', async () => {
    const router = await captureRequests()
    const proxy = await captureRequests()
    const gateway = await startClaudeRequestGateway({
      upstreamUrl: router.url,
      env: { HTTP_PROXY: proxy.url },
      resolveOverride: () => undefined
    })
    dispose.unshift(gateway.close)
    const response = await fetch(gateway.baseUrl + '/v1/models', {
      headers: { authorization: 'Bearer local-router-client-key' }
    })
    expect(await response.text()).toBe('accepted')
    expect(router.requests[0]?.headers.authorization).toBe('Bearer local-router-client-key')
    expect(proxy.requests).toHaveLength(0)
  })

  it('cancels upstream streams on client disconnect and gateway shutdown', async () => {
    let upstreamClosed!: () => void
    const upstream = await listen((_request, response) => {
      response.once('close', upstreamClosed)
      response.writeHead(200, { 'content-type': 'text/event-stream' })
      response.write('data: first\n\n')
    })
    const gateway = await start(upstream)
    const controller = new AbortController()
    const afterAbort = new Promise<void>((resolve) => { upstreamClosed = resolve })
    const stream = await fetch(gateway.baseUrl + '/v1/models', { signal: controller.signal })
    expect(stream.status).toBe(200)
    controller.abort()
    await afterAbort

    const afterShutdown = new Promise<void>((resolve) => { upstreamClosed = resolve })
    const open = await fetch(gateway.baseUrl + '/v1/models')
    expect(open.status).toBe(200)
    await gateway.close()
    await afterShutdown
    await expect(fetch(gateway.baseUrl + '/v1/models')).rejects.toThrow()
  })
})
