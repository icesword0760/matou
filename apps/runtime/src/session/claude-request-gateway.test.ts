import http, { type RequestListener, type Server } from 'node:http'
import { afterEach, describe, expect, it, vi } from 'vitest'
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
  vi.restoreAllMocks()
})

describe('Claude request gateway', () => {
  async function captureOfficialRequests() {
    const capture = await captureRequests()
    const local = new URL(capture.url)
    const original = http.request
    vi.spyOn(http, 'request').mockImplementation(((url: URL, ...args: unknown[]) => {
      if (url instanceof URL && url.hostname === 'api.deepseek.com') {
        url = new URL(url)
        url.hostname = local.hostname
        url.port = local.port
      }
      return Reflect.apply(original, http, [url, ...args])
    }) as typeof http.request)
    return { ...capture, official: 'http://api.deepseek.com/anthropic' }
  }

  const inlineDefinition = { name: 'lookup', input_schema: { type: 'object' } }
  const inlineRequest = () => ({
    model: 'original', tools: [{ name: 'search', input_schema: { type: 'object' } }],
    messages: [{ role: 'system', content: [{ type: 'tool_addition', tool: { type: 'tool_definition', definition: inlineDefinition } }] }]
  })

  it('converts official message and token-count requests without an override and updates body headers', async () => {
    const upstream = await captureOfficialRequests()
    const gateway = await start(upstream.official)
    const body = JSON.stringify(inlineRequest())
    for (const path of ['/v1/messages', '/v1/messages/count_tokens']) {
      const response = await fetch(gateway.baseUrl + path, { method: 'POST', body, headers: {
        'content-type': 'application/json', 'content-length': String(Buffer.byteLength(body)),
        'content-md5': 'old-md5', 'content-digest': 'old-content-digest', digest: 'old-digest'
      } })
      expect(response.status).toBe(200)
      expect(await response.text()).toBe('accepted')
    }
    expect(upstream.requests.map(request => request.url)).toEqual(['/anthropic/v1/messages', '/anthropic/v1/messages/count_tokens'])
    for (const request of upstream.requests) {
      const parsed = JSON.parse(request.body)
      expect(parsed.tools).toEqual([...inlineRequest().tools, { ...inlineDefinition, defer_loading: true }])
      expect(parsed.messages[0].content[0].tool).toEqual({ type: 'tool_reference', name: 'lookup' })
      expect(request.headers['content-length']).toBe(String(Buffer.byteLength(request.body)))
      expect(request.headers['content-md5']).toBeUndefined()
      expect(request.headers['content-digest']).toBeUndefined()
      expect(request.headers.digest).toBeUndefined()
    }
  })

  it('uses the final destination, preserving other providers and converting overrides into DeepSeek', async () => {
    const official = await captureOfficialRequests()
    const other = await captureRequests()
    const fromOther = await start(other.url, ({ compaction }) => compaction ? { baseUrl: official.official, model: 'deepseek-flash' } : undefined)
    const fromOfficial = await start(official.official, ({ compaction }) => compaction ? { baseUrl: other.url, model: 'other' } : undefined)
    const body = JSON.stringify(inlineRequest())
    for (const [gateway, headers] of [[fromOther, {}], [fromOther, { 'x-claude-code-compaction': 'manual' }], [fromOfficial, { 'x-claude-code-compaction': 'manual' }]] as const) {
      expect(await (await fetch(gateway.baseUrl + '/v1/messages', { method: 'POST', body, headers })).text()).toBe('accepted')
    }
    expect(other.requests).toHaveLength(2)
    expect(other.requests[0]!.body).toBe(body)
    expect(JSON.parse(other.requests[1]!.body).messages).toEqual(inlineRequest().messages)
    expect(official.requests).toHaveLength(1)
    expect(JSON.parse(official.requests[0]!.body).messages[0].content[0].tool.type).toBe('tool_reference')
  })

  it('reports incompatible inline tools as a client error before sending upstream', async () => {
    const upstream = await captureOfficialRequests()
    const gateway = await start(upstream.official)
    for (const tools of [[], [{ ...inlineDefinition, input_schema: { type: 'string' } }]]) {
      const response = await fetch(gateway.baseUrl + '/v1/messages', { method: 'POST', body: JSON.stringify({ ...inlineRequest(), tools }) })
      expect(response.status).toBe(400)
      expect(await response.json()).toMatchObject({ type: 'error', error: { type: 'invalid_request_error', message: expect.stringContaining('DeepSeek') } })
    }
    expect(upstream.requests).toHaveLength(0)
  })

  it('folds removals for official message and count requests while preserving other destinations', async () => {
    const official = await captureOfficialRequests()
    const other = await captureRequests()
    const gateway = await start(official.official)
    const otherGateway = await start(other.url)
    const original = inlineRequest()
    const body = JSON.stringify({ ...original, messages: [...original.messages, { role: 'system', content: [{ type: 'tool_removal', tool: { type: 'tool_reference', name: 'lookup' } }] }] })
    for (const path of ['/v1/messages', '/v1/messages/count_tokens']) {
      const response = await fetch(gateway.baseUrl + path, { method: 'POST', body, headers: { 'content-digest': 'old-digest' } })
      expect(response.status).toBe(200)
      await response.text()
    }
    expect(await (await fetch(otherGateway.baseUrl + '/v1/messages', { method: 'POST', body })).text()).toBe('accepted')
    expect(other.requests[0]!.body).toBe(body)
    for (const request of official.requests) {
      const parsed = JSON.parse(request.body)
      expect(parsed.tools).toEqual(original.tools)
      expect(parsed.messages.flatMap((message: { content: Array<{ type: string }> }) => message.content).every((block: { type: string }) => block.type === 'text')).toBe(true)
      expect(request.headers['content-length']).toBe(String(Buffer.byteLength(request.body)))
      expect(request.headers['content-digest']).toBeUndefined()
    }
    const invalid = await fetch(gateway.baseUrl + '/v1/messages', { method: 'POST', body: JSON.stringify({ ...JSON.parse(body), tool_choice: { type: 'tool', name: 'lookup' } }) })
    expect(invalid.status).toBe(400)
    expect(await invalid.json()).toMatchObject({ error: { type: 'invalid_request_error', message: expect.stringContaining('tool_choice') } })
    expect(official.requests).toHaveLength(2)
  })

  it('normalizes custom tools and unavailable discovery results even without inline events', async () => {
    const official = await captureOfficialRequests()
    const other = await captureRequests()
    const gateway = await start(official.official)
    const otherGateway = await start(other.url)
    const source = {
      model: 'deepseek-flash', tools: [{ ...inlineDefinition, type: 'custom' }],
      messages: [{ role: 'user', content: [{ type: 'tool_result', tool_use_id: 'call', content: [
        { type: 'tool_reference', tool_name: 'retired' }, { type: 'tool_reference', tool_name: 'lookup' }
      ] }] }]
    }
    const body = JSON.stringify(source)
    for (const path of ['/v1/messages', '/v1/messages/count_tokens']) {
      expect(await (await fetch(gateway.baseUrl + path, { method: 'POST', body })).text()).toBe('accepted')
    }
    expect(await (await fetch(otherGateway.baseUrl + '/v1/messages', { method: 'POST', body })).text()).toBe('accepted')
    expect(other.requests[0]!.body).toBe(body)
    for (const request of official.requests) {
      const parsed = JSON.parse(request.body)
      expect(parsed.tools).toEqual([inlineDefinition])
      expect(parsed.messages[0].content[0].content).toEqual([
        { type: 'text', text: expect.stringContaining('"retired"') }, { type: 'tool_reference', tool_name: 'lookup' }
      ])
      expect(request.headers['content-length']).toBe(String(Buffer.byteLength(request.body)))
    }
  })

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
