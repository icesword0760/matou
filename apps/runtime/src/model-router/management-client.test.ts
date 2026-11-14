import { afterEach, expect, it } from 'vitest'
import { createServer, type Server } from 'node:http'
import { RouterManagementClient, type RouterCredential } from './management-client'
import { readChatGptModels } from './account-readers'
import { ANTIGRAVITY_CLIENT_USER_AGENT } from './antigravity-client'
import { CODEX_CLIENT_ORIGINATOR, CODEX_CLIENT_USER_AGENT } from './codex-client'

const servers: Server[] = []
afterEach(async () => { for (const server of servers.splice(0)) { server.closeAllConnections(); await new Promise<void>(done => server.close(() => done())) } })
it('allows an escaped >1MiB management envelope for a >600KiB model body, without relaxing other management responses', async () => {
  const body = JSON.stringify({ models: Array.from({ length: 10 }, (_, index) => ({ slug: `gpt-fixture-${index}`, instructions: '"'.repeat(34 * 1024) })) })
  const envelope = JSON.stringify({ status_code: 200, header: {}, body })
  expect(Buffer.byteLength(body)).toBeGreaterThan(600 * 1024)
  expect(Buffer.byteLength(envelope)).toBeGreaterThan(1024 * 1024)
  const server = createServer(async (request, response) => {
    const chunks: Buffer[] = []; for await (const chunk of request) chunks.push(Buffer.from(chunk))
    const input = JSON.parse(Buffer.concat(chunks).toString()) as { url: string }
    response.setHeader('Content-Type', 'application/json')
    response.end(input.url.includes('/codex/models') ? envelope : JSON.stringify({ padding: 'x'.repeat(1024 * 1024) }))
  }); servers.push(server)
  await new Promise<void>(done => server.listen(0, '127.0.0.1', done))
  const client = new RouterManagementClient({ baseUrl: `http://127.0.0.1:${(server.address() as { port: number }).port}`, clientKey: 'fixture', managementKey: 'fixture', pid: process.pid })
  const credential = { id: 'fixture', name: 'fixture.json', provider: 'codex', auth_index: 'fixture' } satisfies RouterCredential
  const models = readChatGptModels(await client.apiCall('chatgpt-models', credential))
  expect(models.ok && models.value.length).toBe(10)
  await expect(client.apiCall('chatgpt-usage', credential)).rejects.toThrow('too large')
})

it('sends the shared Antigravity Hub agent on every probe and can pin it onto the credential', async () => {
  const calls: Array<{ path: string; body: Record<string, unknown> }> = []
  const server = createServer(async (request, response) => {
    const chunks: Buffer[] = []; for await (const chunk of request) chunks.push(Buffer.from(chunk))
    calls.push({ path: request.url ?? '', body: JSON.parse(Buffer.concat(chunks).toString()) })
    response.setHeader('Content-Type', 'application/json')
    response.end(JSON.stringify({ status_code: 200, header: {}, body: '{}' }))
  }); servers.push(server)
  await new Promise<void>(done => server.listen(0, '127.0.0.1', done))
  const client = new RouterManagementClient({ baseUrl: `http://127.0.0.1:${(server.address() as { port: number }).port}`, clientKey: 'fixture', managementKey: 'fixture', pid: process.pid })
  const credential = { id: 'agy', name: 'agy.json', provider: 'antigravity', auth_index: 'agy' } satisfies RouterCredential
  for (const probe of ['antigravity-account', 'antigravity-quota', 'antigravity-models'] as const) await client.apiCall(probe, credential)
  await client.setUserAgent('agy.json', ANTIGRAVITY_CLIENT_USER_AGENT)
  const probes = calls.slice(0, 3)
  expect(probes.map(call => (call.body.header as Record<string, string>)['User-Agent'])).toEqual(Array(3).fill(ANTIGRAVITY_CLIENT_USER_AGENT))
  expect(JSON.stringify(probes)).not.toMatch(/matou/i)
  expect(calls[3]).toEqual({ path: '/v8/management/credentials/fields', body: { name: 'agy.json', user_agent: ANTIGRAVITY_CLIENT_USER_AGENT } })
})

it('sends the shared Codex agent and originator on both ChatGPT probes', async () => {
  const calls: Array<{ path: string; body: Record<string, unknown> }> = []
  const server = createServer(async (request, response) => {
    const chunks: Buffer[] = []; for await (const chunk of request) chunks.push(Buffer.from(chunk))
    calls.push({ path: request.url ?? '', body: JSON.parse(Buffer.concat(chunks).toString()) })
    response.setHeader('Content-Type', 'application/json')
    response.end(JSON.stringify({ status_code: 200, header: {}, body: '{}' }))
  }); servers.push(server)
  await new Promise<void>(done => server.listen(0, '127.0.0.1', done))
  const client = new RouterManagementClient({ baseUrl: `http://127.0.0.1:${(server.address() as { port: number }).port}`, clientKey: 'fixture', managementKey: 'fixture', pid: process.pid })
  const credential = { id: 'codex', name: 'codex.json', provider: 'codex', auth_index: 'codex' } satisfies RouterCredential
  await client.apiCall('chatgpt-usage', credential)
  await client.apiCall('chatgpt-models', credential)
  for (const call of calls) {
    const header = call.body.header as Record<string, string>
    expect(header['User-Agent']).toBe(CODEX_CLIENT_USER_AGENT)
    expect(header.originator).toBe(CODEX_CLIENT_ORIGINATOR)
  }
  await client.setHeaders('codex.json', { Originator: CODEX_CLIENT_ORIGINATOR })
  expect(calls[2]).toEqual({ path: '/v8/management/credentials/fields', body: { name: 'codex.json', headers: { Originator: CODEX_CLIENT_ORIGINATOR } } })
})
