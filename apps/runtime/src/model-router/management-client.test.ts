import { afterEach, expect, it } from 'vitest'
import { createServer, type Server } from 'node:http'
import { RouterManagementClient, type RouterCredential } from './management-client'
import { readChatGptModels } from './account-readers'

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
