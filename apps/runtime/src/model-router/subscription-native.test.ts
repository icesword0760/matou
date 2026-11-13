/** Native Claude Code 2.1.323 + pinned router, fully local provider fixtures.
 * Antigravity's supported credential base_url is pointed at the local fake
 * upstream only in this test. No OAuth exchange/real model access is claimed.
 */
import { afterEach, describe, expect, it, vi } from 'vitest'
import { EventEmitter } from 'node:events'
import { createServer, type Server } from 'node:http'
import { execFileSync } from 'node:child_process'
import { constants } from 'node:fs'
import { mkdir, mkdtemp, readFile, readdir, writeFile, chmod, copyFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { setTimeout as delay } from 'node:timers/promises'
import { PROTOCOL_VERSION, type RuntimeMessage, type RpcMethod } from '@matou/contracts'
import { RuntimeServer } from '../runtime-server'
import { RuntimeRpcRouter } from '../rpc/runtime-rpc-router'
import { ProviderConfigStore } from '../provider-config/provider-config-store'
import { RuntimeSessionRegistry } from '../session/runtime-session-registry'
import { ProviderHookServer } from '../session/provider-hook-server'
import { SessionRepository } from '../domain/session-repository'
import { DomainTransactionManager } from '../storage/domain-transaction'
import { RuntimeDatabase } from '../storage/database'
import { MigrationRunner } from '../storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../storage/migrations'
import { ModelRouterManager, type RouterConnection } from './manager'
import { RouterProcessOwner } from './process-owner'
import { prepareRouterDirectory } from './config'
import { SubscriptionAccounts } from './subscription-accounts'
import { RouterManagementClient } from './management-client'

const project = resolve(import.meta.dirname, '../../../..')
const output = join(project, 'output/model-router-stage4-rework')
const disposals: Array<() => Promise<void>> = []
afterEach(async () => { while (disposals.length) await disposals.pop()!(); vi.unstubAllEnvs() })
async function until(check: () => boolean | Promise<boolean>, label = 'condition', timeout = 15000) { const deadline = Date.now() + timeout; while (Date.now() < deadline) { if (await check()) return; await delay(20) } throw new Error(`Timed out: ${label}`) }
async function listen(server: Server) { await new Promise<void>(done => server.listen(0, '127.0.0.1', done)); return `http://127.0.0.1:${(server.address() as { port: number }).port}` }
async function close(server: Server) { server.closeAllConnections(); if (server.listening) await new Promise<void>(done => server.close(() => done())) }
type Json = Record<string, any> // heterogeneous native/provider wire evidence, local tests only
class Port extends EventEmitter {
  sent: RuntimeMessage[] = []
  buffers = new Map<string, string>()
  events = new Map<string, Json[]>()
  postMessage(message: RuntimeMessage) {
    this.sent.push(message)
    if (message.type === 'terminal.data') {
      const text = (this.buffers.get(message.sessionId) ?? '') + new TextDecoder().decode(message.data)
      const lines = text.split('\n'); this.buffers.set(message.sessionId, lines.pop() ?? '')
      for (const line of lines) { try { const event = JSON.parse(line.trim()); const rows = this.events.get(message.sessionId) ?? []; rows.push(event); this.events.set(message.sessionId, rows) } catch { /* PTY echo and startup notices are not SDK events. */ } }
      this.send({ type: 'terminal.ack', protocolVersion: PROTOCOL_VERSION, sessionId: message.sessionId, throughSequence: message.sequence })
    }
  }
  send(data: unknown) { this.emit('message', { data }) }
  start() {}
  close() { this.emit('close') }
}
function seed(database: RuntimeDatabase, cwd: string, id: string) {
  database.run('INSERT INTO workspaces (id,name,root_directory,created_at,updated_at) VALUES (?,?,?,?,?)', 'ws', 'Fixture', cwd, 1, 1)
  database.run('INSERT INTO execution_contexts (id,workspace_id,kind,cwd,created_at) VALUES (?,?,?,?,?)', 'ctx', 'ws', 'plain-directory', cwd, 1)
  database.run('INSERT INTO tasks (id,workspace_id,execution_context_id,title,status,created_at,updated_at) VALUES (?,?,?,?,?,?,?)', 'task', 'ws', 'ctx', 'Fixture', 'active', 1, 1)
  database.run("INSERT INTO sessions (id,task_id,execution_context_id,kind,status,title,created_at,updated_at,last_activity_at) VALUES (?,'task','ctx','claude-code','exited',?,1,1,1)", id, id)
  database.run("INSERT INTO scenes (id,task_id,name,mode,root_node_id,title_pinned,sort_key,layout_revision,created_at,updated_at) VALUES ('scene','task','Fixture','tile','node',0,'a0',1,1,1)")
  database.run("INSERT INTO scene_nodes (id,scene_id,kind,ordinal,created_at) VALUES ('node','scene','root',0,1)")
  database.run("INSERT INTO session_mounts (id,scene_id,scene_node_id,session_id,created_at) VALUES ('mount','scene','node',?,1)", id)
}

async function fixture() {
  await mkdir(output, { recursive: true }); const root = await mkdtemp(join(output, 'native-')), configDir = join(root, 'claude-config')
  await mkdir(configDir); await writeFile(join(root, 'harmless.txt'), 'STAGE4_SAFE_FILE_CONTENT\n')
  await writeFile(join(configDir, 'settings.json'), JSON.stringify({ env: { ANTHROPIC_API_KEY: 'OLD_SETTINGS_API', CLAUDE_CODE_OAUTH_TOKEN: 'OLD_SETTINGS_OAUTH' }, autoCompactWindow: 400000 }))
  const installedNative = process.env.MATOU_TEST_CLAUDE_COMMAND || execFileSync('/bin/zsh', ['-c', 'command -v claude'], { encoding: 'utf8' }).trim()
  // A concurrent CLI installer can replace the global executable between
  // verification and spawn. Keep this probe on its own verified local copy.
  const native = join(root, 'claude-native')
  await copyFile(installedNative, native, constants.COPYFILE_FICLONE)
  await chmod(native, 0o700)
  expect(execFileSync(native, ['--version'], { encoding: 'utf8' })).toContain('2.1.323')
  const calls: Json[] = [], routerHeaders: Json[] = [], blocked: string[] = [], steps: Json[] = []
  let responseNumber = 0
  const upstream = createServer(async (request, response) => {
    try {
      const chunks: Buffer[] = []; for await (const chunk of request) chunks.push(Buffer.from(chunk))
      let body: Json = {}; try { body = JSON.parse(Buffer.concat(chunks).toString()) } catch {}
      const path = request.url ?? '', isGoogle = path.includes('GenerateContent') || path.includes('generateContent')
      if (path.includes('fetchAvailableModels')) { response.setHeader('Content-Type', 'application/json'); response.end(JSON.stringify({ models: { 'claude-opus-4-6-thinking': {}, 'claude-sonnet-4-6': {}, 'gemini-3.8-flash-high': {} } })); return }
      if (path.includes('count_tokens')) { response.setHeader('Content-Type', 'application/json'); response.end('{"input_tokens":10}'); return }
      if (!isGoogle && !path.includes('messages')) { response.setHeader('Content-Type', 'application/json'); response.end('{"data":[]}'); return }
      calls.push({ path, body, authorization: request.headers.authorization, apiKey: request.headers['x-api-key'] })
      responseNumber++
      if (!isGoogle) {
        if (body.stream !== true) {
          response.setHeader('Content-Type', 'application/json')
          response.end(JSON.stringify({ id: `official-${responseNumber}`, type: 'message', role: 'assistant', model: body.model, content: [{ type: 'text', text: 'OFFICIAL_FIXTURE_OK' }], stop_reason: 'end_turn', usage: { input_tokens: 10, output_tokens: 4 } }))
          return
        }
        response.writeHead(200, { 'Content-Type': 'text/event-stream' })
        const emit = (value: Json) => response.write(`event: ${value.type}\ndata: ${JSON.stringify(value)}\n\n`)
        emit({ type: 'message_start', message: { id: `official-${responseNumber}`, type: 'message', role: 'assistant', model: body.model, content: [], usage: { input_tokens: 10, output_tokens: 0 } } })
        emit({ type: 'content_block_start', index: 0, content_block: { type: 'text', text: '' } }); emit({ type: 'content_block_delta', index: 0, delta: { type: 'text_delta', text: 'OFFICIAL_FIXTURE_OK' } }); emit({ type: 'content_block_stop', index: 0 }); emit({ type: 'message_delta', delta: { stop_reason: 'end_turn' }, usage: { output_tokens: 4 } }); emit({ type: 'message_stop' }); response.end(); return
      }
      const contents = body.request?.contents ?? [], serialized = JSON.stringify([...contents].reverse().find((content: Json) => content.role === 'user' && (content.parts ?? []).some((part: Json) => part.functionResponse || (typeof part.text === 'string' && !part.text.startsWith('<system-reminder>')))) ?? {})
      const toolResult = serialized.includes('STAGE4_SAFE_FILE_CONTENT')
      const wantsRead = serialized.includes('READ_STAGE4_FILE') && !toolResult
      const declarations = (body.request?.tools ?? []).flatMap((tool: Json) => tool.functionDeclarations ?? [])
      const readName = declarations.find((tool: Json) => /read$/i.test(tool.name))?.name ?? 'Read'
      const parts = wantsRead ? [{ functionCall: { name: readName, args: { file_path: join(root, 'harmless.txt') }, id: 'call_safe_read' } }] : [{ text: toolResult ? 'READ_CONFIRMED_STAGE4' : 'SUBSCRIPTION_FIXTURE_OK' }]
      const payload = { response: { candidates: [{ content: { role: 'model', parts }, finishReason: 'STOP' }], usageMetadata: { promptTokenCount: 20, candidatesTokenCount: 8, totalTokenCount: 28 }, modelVersion: body.model ?? 'fixture', responseId: `sub-${responseNumber}` } }
      if (path.includes('streamGenerateContent')) { response.writeHead(200, { 'Content-Type': 'text/event-stream' }); response.end(`data: ${JSON.stringify(payload)}\n\n`) }
      else { response.setHeader('Content-Type', 'application/json'); response.end(JSON.stringify(payload)) }
    } catch (error) { response.writeHead(500); response.end(JSON.stringify({ error: String(error) })) }
  })
  const upstreamUrl = await listen(upstream)
  const outbound = createServer(async (request, response) => {
    if (!request.url?.startsWith(upstreamUrl + '/')) { response.writeHead(502); response.end(); return }
    const chunks: Buffer[] = []; for await (const chunk of request) chunks.push(Buffer.from(chunk))
    const result = await fetch(request.url, { method: request.method!, headers: { ...(typeof request.headers.authorization === 'string' ? { Authorization: request.headers.authorization } : {}), 'Content-Type': 'application/json' }, ...(chunks.length ? { body: Buffer.concat(chunks) } : {}) })
    response.writeHead(result.status, { 'Content-Type': result.headers.get('Content-Type') ?? 'application/json' }); if (result.body) for await (const chunk of result.body) response.write(chunk); response.end()
  })
  outbound.on('connect', (request, socket) => { blocked.push(request.url!); socket.end('HTTP/1.1 502 External network blocked\r\nContent-Length: 0\r\n\r\n') })
  const outboundUrl = await listen(outbound)
  await prepareRouterDirectory(root)
  await writeFile(join(root, 'model-router/credentials/google-fixture.json'), JSON.stringify({ type: 'antigravity', email: 'fixture@example.invalid', access_token: 'SYNTHETIC_GOOGLE_TOKEN', refresh_token: 'SYNTHETIC_GOOGLE_REFRESH', expired: '2099-01-01T00:00:00Z', project_id: 'fixture-project', base_url: upstreamUrl, prefix: 'matou-antigravity' }), { mode: 0o600 })
  const store = new ProviderConfigStore(root); await store.confirmAntigravityRisk()
  const subscription = await store.adoptSubscription({ name: 'Fixture subscription', modelMapping: { opus: 'claude-opus-4-6-thinking', sonnet: 'claude-sonnet-4-6', haiku: 'gemini-3.8-flash-high' }, subscription: { provider: 'antigravity', credentialRef: { name: 'google-fixture.json' }, account: { status: 'ready', email: 'fixture@example.invalid' }, modelMetadata: [{ id: 'claude-opus-4-6-thinking', displayName: 'Fixture Opus', contextWindow: 400000, maxOutputTokens: 128000 }, { id: 'claude-sonnet-4-6', displayName: 'Fixture Sonnet', contextWindow: 200000, maxOutputTokens: 64000 }, { id: 'gemini-3.8-flash-high', displayName: 'Fixture Haiku', contextWindow: 100000, maxOutputTokens: 32000 }] } })
  await store.upsert({ id: 'anthropic-official', cli: 'claude-code', name: 'Anthropic official fixture', endpoint: upstreamUrl, model: 'claude-sonnet-4-6' })
  const owner = new RouterProcessOwner({ executable: process.execPath, supervisor: join(project, 'apps/runtime/dist/model-router-supervisor.cjs'), binary: join(project, `.cache/model-router/8.0.69/darwin-${process.arch}/model-router`), dataRoot: root })
  const manager = new ModelRouterManager({ dataRoot: root, control: owner, idleMs: 100, resolveProxy: async () => ({ source: 'system', proxyUrl: outboundUrl }) })
  let real: RouterConnection | undefined
  let failModelReads = false, expiredCredential = false
  const recording = createServer(async (request, response) => {
    try {
      if (!real) throw new Error('Router is not ready')
      if (failModelReads && request.url?.startsWith('/v8/management/credentials/models')) { response.setHeader('Content-Type', 'application/json'); response.end('{"models":[]}'); return }
      const chunks: Buffer[] = []; for await (const chunk of request) chunks.push(Buffer.from(chunk))
      const raw = Buffer.concat(chunks)
      if (request.url?.startsWith('/v1/')) {
        let body: Json = {}; try { body = JSON.parse(raw.toString()) } catch {}
        routerHeaders.push({ path: request.url, model: body.model, authorizationIsClientKey: request.headers.authorization === `Bearer ${real.clientKey}`, oldCredentialPresent: [request.headers.authorization, request.headers['x-api-key'], request.headers['anthropic-api-key']].some(value => typeof value === 'string' && /OLD_/.test(value)), apiKeyHeader: request.headers['x-api-key'] ?? null })
      }
      const result = await fetch(real.baseUrl + request.url, { method: request.method!, headers: Object.fromEntries(Object.entries(request.headers).filter(([key, value]) => !['host', 'content-length', 'connection', 'transfer-encoding'].includes(key) && typeof value === 'string')) as Record<string, string>, ...(raw.length ? { body: raw } : {}), signal: AbortSignal.timeout(30000) })
      if (expiredCredential && new URL(request.url!, recordingUrl).pathname === '/v8/management/credentials') {
        const body = await result.json() as Json
        for (const file of body.files ?? []) { file.requires_sign_in = true; file.renewal_state = 'expired_sign_in_required' }
        response.writeHead(result.status, { 'Content-Type': 'application/json' }); response.end(JSON.stringify(body)); return
      }
      response.writeHead(result.status, { 'Content-Type': result.headers.get('Content-Type') ?? 'application/json' }); if (result.body) for await (const chunk of result.body) response.write(chunk); response.end()
    } catch (error) { if (!response.headersSent) response.writeHead(502); response.end(JSON.stringify({ error: String(error) })) }
  })
  const recordingUrl = await listen(recording), acquire = manager.acquire.bind(manager)
  manager.acquire = async scope => { const lease = await acquire(scope); real = lease.connection; return { connection: { ...lease.connection, baseUrl: recordingUrl }, release: lease.release } }
  const accounts = new SubscriptionAccounts({ dataRoot: root, manager, store, openAuthorizationUrl: async () => {} }); await accounts.initialize()
  const wrapper = join(root, 'claude-headless.cjs')
  await writeFile(wrapper, `#!/usr/bin/env node
const fs=require('fs'), cp=require('child_process'), path=require('path');const argv=process.argv.slice(2);const keys=['ANTHROPIC_API_KEY','CLAUDE_CODE_OAUTH_TOKEN','ANTHROPIC_AUTH_TOKEN','ANTHROPIC_BASE_URL','ANTHROPIC_DEFAULT_OPUS_MODEL','ANTHROPIC_DEFAULT_OPUS_MODEL_NAME','ANTHROPIC_DEFAULT_SONNET_MODEL_NAME','ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME','CLAUDE_CODE_MAX_CONTEXT_TOKENS','CLAUDE_CODE_MAX_OUTPUT_TOKENS'];
const settings=argv.includes('--settings')?JSON.parse(fs.readFileSync(argv[argv.indexOf('--settings')+1],'utf8')):null;
fs.writeFileSync(path.join(${JSON.stringify(root)},'launch-'+process.pid+'.json'),JSON.stringify({argv,settings,env:Object.fromEntries(keys.filter(key=>process.env[key]!==undefined).map(key=>[key,key==='ANTHROPIC_AUTH_TOKEN'&&!process.env[key].startsWith('OLD_')?'[local-client-key]':process.env[key]]))}));
const child=cp.spawn(${JSON.stringify(native)},[...argv,'-p','--input-format','stream-json','--output-format','stream-json','--verbose','--include-partial-messages','--setting-sources','user','--strict-mcp-config','--mcp-config','{"mcpServers":{}}','--tools','Read','--allowedTools','Read'],{stdio:'pipe',env:process.env});process.stdin.pipe(child.stdin);child.stdout.pipe(process.stdout);child.stderr.pipe(process.stderr);for(const signal of ['SIGHUP','SIGTERM','SIGINT'])process.on(signal,()=>child.kill(signal));child.on('exit',code=>process.exit(code??0));
`); await chmod(wrapper, 0o755)
  for (const [key, value] of Object.entries({ MATOU_CLAUDE_COMMAND: wrapper, CLAUDE_CONFIG_DIR: configDir, ANTHROPIC_API_KEY: 'OLD_ENV_API', CLAUDE_CODE_OAUTH_TOKEN: 'OLD_ENV_OAUTH', ANTHROPIC_AUTH_TOKEN: 'OLD_ENV_AUTH', CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: '1', CLAUDE_CODE_AUTO_MODE_SERVER: '0', DISABLE_AUTOUPDATER: '1', HTTPS_PROXY: outboundUrl, HTTP_PROXY: outboundUrl, NO_PROXY: '127.0.0.1,localhost', CLAUDE_CODE_USE_BEDROCK: '0', CLAUDE_CODE_USE_VERTEX: '0', CLAUDE_CODE_USE_FOUNDRY: '0' })) vi.stubEnv(key, value)
  const database = RuntimeDatabase.open(join(root, 'matou.sqlite')); await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate(); seed(database, root, 'live')
  const sessions = new RuntimeSessionRegistry(), port = new Port()
  let runtime: RuntimeServer
  const repository = new SessionRepository(database, new DomainTransactionManager(database))
  const hooks = new ProviderHookServer(root, repository, { currentRunId: id => sessions.get(id)?.runId, onIdentityRecorded: event => { runtime.providerIdentityRecorded(event.sessionId, event.runId) } }); await hooks.start()
  const rpcRouter = new RuntimeRpcRouter(database, undefined, { providerConfigs: store })
  runtime = new RuntimeServer(port, root, database, rpcRouter, undefined, sessions, hooks, undefined, { providerConfigs: store, subscriptionAccounts: accounts, subscriptionConnection: accounts.bindConnection(port), providerResumeTimeoutMs: 30000 })
  port.send({ type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION, clientId: 'native-fixture' })
  let requestNumber = 0
  async function rpc(method: RpcMethod, payload: unknown) { const requestId = `rpc-${++requestNumber}`; port.send({ type: 'rpc.request', protocolVersion: PROTOCOL_VERSION, requestId, capability: 'renderer', method, payload, deadlineAt: Date.now() + 60000 }); await until(() => port.sent.some(message => (message.type === 'rpc.response' || message.type === 'rpc.error') && message.requestId === requestId), method, 60000); return port.sent.find(message => (message.type === 'rpc.response' || message.type === 'rpc.error') && message.requestId === requestId)! }
  function input(value: Json, id = 'live') { port.send({ type: 'terminal.input', protocolVersion: PROTOCOL_VERSION, sessionId: id, data: JSON.stringify(value) + '\r' }) }
  async function control(request: Json, id = 'live') { const request_id = `control-${++requestNumber}`; input({ type: 'control_request', request_id, request }, id); await until(() => port.events.get(id)?.some(event => event.type === 'control_response' && event.response?.request_id === request_id) === true, request.subtype); const event = port.events.get(id)!.find(event => event.type === 'control_response' && event.response?.request_id === request_id)!; expect(event.response.subtype, JSON.stringify(event)).toBe('success'); return event.response.response as Json }
  async function prompt(text: string, id = 'live') { const count = port.events.get(id)?.filter(event => event.type === 'result').length ?? 0; input({ type: 'user', session_id: '', message: { role: 'user', content: text } }, id); await until(() => (port.events.get(id)?.filter(event => event.type === 'result').length ?? 0) > count, text, 30000); const result = port.events.get(id)!.filter(event => event.type === 'result').at(-1)!; expect(result.is_error, JSON.stringify(result)).toBe(false); return result }
  function spawn(id = 'live') { port.send({ type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION, sessionId: id, executionContextId: 'ctx', profile: 'claude-code', cols: 240, rows: 40 }) }
  disposals.push(async () => { await sessions.shutdownAll(); runtime.close(); await hooks.stop(); await accounts.close(); await manager.close(); database.close(); await Promise.all([close(recording), close(upstream), close(outbound)]); await writeFile(join(root, 'evidence.json'), JSON.stringify({ boundary: 'Real RuntimeServer/PtySession + native Claude Code 2.1.323 + official router 8.0.69; synthetic Antigravity credential with loopback base_url and fake provider; no real account exchange or inference.', steps, routerHeaders, calls, blocked, runtimeMessages: port.sent.filter(message => message.type === 'rpc.response' || message.type === 'rpc.error' || message.type === 'protocol.error'), sdkEvents: Object.fromEntries(port.events), rawOutput: port.sent.filter(message => message.type === 'terminal.data').map(message => new TextDecoder().decode((message as Extract<RuntimeMessage, { type: 'terminal.data' }>).data)).join('').slice(-64000) }, null, 2)); await writeFile(join(output, 'latest-native.txt'), root + '\n') })
  return { root, store, subscription, manager, accounts, sessions, runtime, port, database, calls, routerHeaders, steps, rpc, control, prompt, spawn, failModels: (value: boolean) => { failModelReads = value }, expireCredential: (value: boolean) => { expiredCredential = value }, launches: async () => { const names = (await readdir(root)).filter(name => /^launch-\d+\.json$/.test(name)); return Promise.all(names.map(async name => ({ name, ...JSON.parse(await readFile(join(root, name), 'utf8')) }))) } }
}

describe.runIf(process.env.MATOU_TEST_CLAUDE_SUBSCRIPTION === '1')('native isolated subscription switch and recovery', () => {
  it('resets subscription history to the CLI default and compares warm API key and subscription switches', async () => {
    const timings: Json[] = []
    const instrument = (object: any, method: string, label: string) => {
      const original = object[method]
      vi.spyOn(object, method).mockImplementation(async function (this: unknown, ...args: unknown[]) {
        const start = performance.now()
        try { return await original.apply(this, args) }
        finally { timings.push({ stage: label, scope: typeof args[0] === 'string' ? args[0] : undefined, ms: performance.now() - start }) }
      })
    }
    instrument(RouterProcessOwner.prototype, 'start', 'router-process-start')
    instrument(ModelRouterManager.prototype, 'acquire', 'router-acquire')
    instrument(ModelRouterManager.prototype, 'waitForCredentialAdoption', 'credential-adoption')
    instrument(SubscriptionAccounts.prototype, 'prepareLaunch', 'subscription-preparation')
    for (const method of ['credentials', 'setAliases', 'aliases', 'setPrefix', 'models']) instrument(RouterManagementClient.prototype, method, method)
    try {
      const f = await fixture()
      const snapshot = await f.store.snapshot()
      const official = snapshot.providers['claude-code'].find(row => row.id === 'anthropic-official')!
      await f.store.upsert({ id: official.id, cli: 'claude-code', name: official.name, endpoint: official.endpoint, model: '__cli_default__' })
      const api = await f.store.upsert({ cli: 'claude-code', name: 'API Key timing fixture', endpoint: official.endpoint, model: 'claude-sonnet-4-6', apiKey: 'SYNTHETIC_API_KEY' })
      f.spawn(); await until(() => f.sessions.has('live'), 'official PTY')
      await f.control({ subtype: 'initialize' })
      await f.prompt('OFFICIAL_DEFAULT_BEFORE')
      await until(() => f.sessions.providerIdentityConfirmed(f.sessions.get('live')!.runId!), 'official binding')
      // Keep settings open exactly as in a user-triggered provider switch.
      const warm = await f.manager.openSettings('timing-fixture')
      const samples: Json[] = []
      for (let round = 0; round < 3; round++) {
        for (const [kind, id] of [['api-key', api.id], ['subscription', f.subscription.id], ['official-default', 'anthropic-official']]) {
          const start = performance.now(), stageStart = timings.length, callStart = f.calls.length
          const previousIdentity = f.database.get<{ provider_session_id: string }>("SELECT provider_session_id FROM provider_bindings WHERE session_id='live' ORDER BY updated_at DESC LIMIT 1")!.provider_session_id
          expect(await f.rpc('provider-config.activate', { cli: 'claude-code', providerId: id })).toMatchObject({ type: 'rpc.response' })
          const activationMs = performance.now() - start
          const nativeStart = performance.now()
          await f.control({ subtype: 'initialize' })
          const readyMs = performance.now() - start, nativeReadyAfterRpcMs = performance.now() - nativeStart
          const settings = await f.control({ subtype: 'get_settings' })
          const result = await f.prompt(`SWITCH_${round}_${kind}`)
          expect(result.session_id).toBe(previousIdentity)
          const newCalls = f.calls.slice(callStart)
          if (kind === 'subscription') {
            expect(settings.applied.model).toMatch(/^matou-antigravity\//)
            expect(f.routerHeaders.at(-1)?.model).toMatch(/^matou-antigravity\//)
            expect(result.result).toContain('SUBSCRIPTION_FIXTURE_OK')
          } else {
            expect(settings.applied.model).not.toMatch(/^matou-/)
            expect(newCalls.filter(call => call.path.includes('messages')).at(-1)?.body.model).not.toMatch(/^matou-/)
            expect(result.result).toContain('OFFICIAL_FIXTURE_OK')
          }
          const launch = (await f.launches()).find(row => row.name === `launch-${f.sessions.get('live')!.pid}.json`)!
          if (kind === 'official-default') {
            expect(launch.argv.slice(launch.argv.indexOf('--model'), launch.argv.indexOf('--model') + 2)).toEqual(['--model', 'default'])
            const metadata = f.database.get<{ metadata_json: string }>("SELECT metadata_json FROM provider_bindings WHERE session_id='live' ORDER BY updated_at DESC LIMIT 1")!
            expect(JSON.parse(metadata.metadata_json).model).toBe('')
          }
          samples.push({ round, kind, activationMs, nativeReadyAfterRpcMs, readyMs, appliedModel: settings.applied.model, argv: launch.argv, stages: timings.slice(stageStart) })
          await until(() => f.sessions.providerIdentityConfirmed(f.sessions.get('live')!.runId!), 'switched identity')
        }
      }
      // The transition reset must not pin all future official resumes to default.
      await f.control({ subtype: 'set_model', model: 'claude-sonnet-4-6' })
      await f.prompt('OFFICIAL_MANUAL_MODEL_AFTER_TRANSITION')
      await f.sessions.shutdownAll()
      f.spawn(); await until(() => f.sessions.has('live'), 'official manual-model restore PTY')
      await f.control({ subtype: 'initialize' })
      expect((await f.control({ subtype: 'get_settings' })).applied.model).toBe('claude-sonnet-4-6')
      expect((await f.launches()).find(row => row.name === `launch-${f.sessions.get('live')!.pid}.json`)!.argv).not.toContain('--model')
      warm.release(); f.manager.releaseConnection('timing-fixture')
      const evidence = { samples, bootstrapStages: timings.slice(0, timings.findIndex(row => row.stage === 'subscription-preparation')), timings }
      await mkdir(join(project, 'output/model-switch-rework'), { recursive: true })
      await writeFile(join(project, 'output/model-switch-rework', process.env.MATOU_SWITCH_TIMING_LABEL || 'switch-timing.json'), JSON.stringify(evidence, null, 2))
      console.log('Warm native switch samples', samples.map(({ round, kind, activationMs, nativeReadyAfterRpcMs, readyMs }) => ({ round, kind, activationMs, nativeReadyAfterRpcMs, readyMs })))
    } finally { vi.restoreAllMocks() }
  }, 180000)
  it('round trips official -> subscription -> official with real resume, one answer, a harmless Read and a headless Haiku window probe', async () => {
    const f = await fixture()
    f.spawn(); await until(() => f.sessions.has('live'), 'official PTY')
    await f.control({ subtype: 'initialize' })
    const before = await f.prompt('OFFICIAL_BEFORE_STAGE4')
    expect(before.result).toContain('OFFICIAL_FIXTURE_OK')
    await until(() => f.sessions.providerIdentityConfirmed(f.sessions.get('live')!.runId!), 'official binding')
    const initialPid = f.sessions.get('live')!.pid, providerIdentity = f.database.get<{ provider_session_id: string }>("SELECT provider_session_id FROM provider_bindings WHERE session_id='live' ORDER BY updated_at DESC LIMIT 1")!.provider_session_id
    f.steps.push({ step: 'official-before', pid: initialPid, providerIdentity, result: before.result })
    const failures: Json[] = []
    const assertFailure = async (label: string, id = f.subscription.id, needsSignIn = false) => {
      const failure = await f.rpc('provider-config.activate', { cli: 'claude-code', providerId: id })
      expect(failure).toMatchObject({ type: 'rpc.error', providerFailure: { status: needsSignIn ? 'needs-sign-in' : 'router-start-failed', recovery: needsSignIn ? 'sign-in' : 'retry' } })
      expect((await f.store.snapshot()).activeProviderIds['claude-code']).toBe('anthropic-official')
      expect(f.sessions.get('live')!.pid).toBe(initialPid)
      const snapshot = await f.accounts.snapshot()
      if (needsSignIn) expect(snapshot.providers['claude-code'].find(row => row.id === id)?.subscription?.account.status).toBe('needs-sign-in')
      failures.push({ label, failure, snapshot, unchangedPid: initialPid })
    }
    const failedStart = vi.spyOn(f.manager, 'acquire').mockRejectedValueOnce(new Error('Fixture router startup failure'))
    await assertFailure('router readiness'); failedStart.mockRestore()
    const missing = await f.store.adoptSubscription({ name: 'Missing credential fixture', modelMapping: { opus: 'gpt-6.1-sol', sonnet: 'gpt-6.1-sol', haiku: 'gpt-6.1-sol' }, subscription: { provider: 'codex', credentialRef: { name: 'missing.json' }, account: { status: 'ready' }, modelMetadata: [{ id: 'gpt-6.1-sol', contextWindow: 200000 }] } })
    await assertFailure('credential readiness', missing.id, true)
    f.expireCredential(true); await assertFailure('expired credential', f.subscription.id, true); f.expireCredential(false)
    f.failModels(true); await assertFailure('model registration'); f.failModels(false)
    const original = (await f.store.subscriptions()).find(row => row.id === f.subscription.id)!
    await f.store.adoptSubscription({ ...original, subscription: { ...original.subscription, modelMetadata: original.subscription.modelMetadata.map(model => ({ ...model, contextWindow: 80000 })) } })
    await assertFailure('launch configuration')
    await f.store.adoptSubscription(original)
    f.steps.push({ step: 'activation-failures', failures })
    await writeFile(join(output, 'failure-samples.json'), JSON.stringify({ boundary: 'Real RuntimeServer RPC; startup/model-read faults and terminal credential state deliberately injected, missing credential real; bilingual copy generated by production mapper.', samples: failures }, null, 2))
    const activated = await f.rpc('provider-config.activate', { cli: 'claude-code', providerId: f.subscription.id })
    expect(activated).toMatchObject({ type: 'rpc.response', result: { activeProviderIds: { 'claude-code': f.subscription.id }, sessionTransitions: [{ sessionId: 'live', status: 'updated' }] } })
    expect(f.sessions.get('live')!.pid).not.toBe(initialPid)
    await f.control({ subtype: 'initialize' })
    const models = await f.control({ subtype: 'list_models' }), settingsBefore = await f.control({ subtype: 'get_settings' }), contextBefore = await f.control({ subtype: 'get_context_usage', detail: 'summary' })
    await writeFile(join(output, 'model-menu.json'), JSON.stringify({ client: 'Claude Code 2.1.323', menu: models.models, effectivePicker: settingsBefore.effective.modelPicker }, null, 2))
    expect(models.models.find((row: Json) => row.value === 'default').resolvedModel).toBe(settingsBefore.applied.model)
    expect(models.models.find((row: Json) => row.value === 'opus').resolvedModel).toBe(settingsBefore.applied.model)
    expect(JSON.stringify(models.models)).not.toMatch(/Fable|claude-fable/i)
    expect(models.models.every((row: Json) => row.resolvedModel.startsWith('matou-antigravity/'))).toBe(true)
    expect(models.models.filter((row: Json) => row.value !== 'default')).toHaveLength(3)
    expect(JSON.stringify(models.models.filter((row: Json) => row.value !== 'default'))).not.toMatch(/\$|per Mtok/)
    expect(models.models.find((model: Json) => model.value === 'opus')).toMatchObject({ displayName: 'Opus · Fixture Opus', description: 'Fixture subscription · claude-opus-4-6-thinking · 400K' })
    expect(models.models.find((model: Json) => model.value === 'haiku')).toMatchObject({ displayName: 'Haiku · Fixture Haiku' })
    expect(settingsBefore.effective.env).toMatchObject({ ANTHROPIC_API_KEY: '', CLAUDE_CODE_OAUTH_TOKEN: '' })
    expect(settingsBefore.effective.autoCompactWindow).toBe(100000)
    expect(settingsBefore.applied.model).toContain('matou-antigravity/')
    const answer = await f.prompt('SUBSCRIPTION_QUESTION_STAGE4')
    expect(answer.result).toContain('SUBSCRIPTION_FIXTURE_OK')
    expect(answer.session_id).toBe(providerIdentity)
    const read = await f.prompt('READ_STAGE4_FILE: read harmless.txt using Read')
    expect(read.result).toContain('READ_CONFIRMED_STAGE4')
    expect(f.calls.some(call => JSON.stringify(call.body.request?.contents ?? []).includes('STAGE4_SAFE_FILE_CONTENT'))).toBe(true)
    expect(f.routerHeaders.filter(row => row.path.includes('/messages')).length).toBeGreaterThanOrEqual(3)
    expect(f.routerHeaders.every(row => row.authorizationIsClientKey && !row.oldCredentialPresent)).toBe(true)
    const pickerSelections: Json[] = []
    for (const role of ['opus', 'sonnet', 'haiku']) {
      const selection = await f.control({ subtype: 'set_model', model: role })
      expect(selection.model).toMatch(/^matou-antigravity\//)
      pickerSelections.push({ role, resolvedModel: selection.model })
    }
    const fableFallback = await f.control({ subtype: 'set_model', model: 'fable' })
    expect(fableFallback.model).toBe(settingsBefore.applied.model)
    expect((await f.prompt('FABLE_FALLBACK_STAGE4')).result).toContain('SUBSCRIPTION_FIXTURE_OK')
    const rawFableFallback = await f.control({ subtype: 'set_model', model: 'claude-fable-5-2' })
    expect(rawFableFallback.model).toMatch(/claude-fable-5-2|matou-antigravity\/matou-opus-fallback/)
    expect((await f.prompt('RAW_FABLE_FALLBACK_STAGE4')).result).toContain('SUBSCRIPTION_FIXTURE_OK')
    expect(f.routerHeaders.at(-1)?.model).toBe('matou-antigravity/matou-opus-fallback')
    expect(f.calls.at(-1)?.body.model).toBe('claude-opus-4-6-thinking')
    const literalFallbacks: Json[] = [{ selector: 'claude-fable-5-2', wireModel: f.routerHeaders.at(-1)?.model, upstreamModel: f.calls.at(-1)?.body.model }]
    for (const selector of ['claude-fable-5', 'claude-fable-5-2[1M]']) {
      await f.control({ subtype: 'set_model', model: selector })
      expect((await f.prompt('FABLE_LITERAL_VARIANT_STAGE4')).result).toContain('SUBSCRIPTION_FIXTURE_OK')
      expect(f.routerHeaders.at(-1)?.model).toMatch(/^matou-antigravity\/matou-opus-fallback(?:\[1m\])?$/i)
      expect(f.calls.at(-1)?.body.model).toBe('claude-opus-4-6-thinking')
      literalFallbacks.push({ selector, wireModel: f.routerHeaders.at(-1)?.model, upstreamModel: f.calls.at(-1)?.body.model })
    }
    await writeFile(join(output, 'model-menu.json'), JSON.stringify({ client: 'Claude Code 2.1.323', menu: models.models, effectivePicker: settingsBefore.effective.modelPicker, pickerSelections, fableFallback, rawFableFallback, literalFallbacks, acceptedKnownLimitation: 'Default pricing label remains in the fixed client; accepted in answer 10.' }, null, 2))
    const switched = await f.control({ subtype: 'set_model', model: 'haiku' })
    expect(switched.model).toBe('matou-antigravity/matou-haiku')
    const settingsAfter = await f.control({ subtype: 'get_settings' }), contextAfter = await f.control({ subtype: 'get_context_usage', detail: 'summary' })
    expect(settingsAfter.effective.autoCompactWindow).toBe(100000)
    expect(contextBefore.maxTokens).toBe(100000)
    expect(contextAfter.maxTokens).toBe(100000)
    f.steps.push({ step: 'subscription-native', pid: f.sessions.get('live')!.pid, providerIdentity: answer.session_id, answer: answer.result, read: read.result, models: models.models, settingsBefore, settingsAfter, contextBefore: { maxTokens: contextBefore.maxTokens, rawMaxTokens: contextBefore.rawMaxTokens, autocompactSource: contextBefore.autocompactSource }, contextAfter: { maxTokens: contextAfter.maxTokens, rawMaxTokens: contextAfter.rawMaxTokens, autocompactSource: contextAfter.autocompactSource }, smallestTierWindow: 100000, allTiersSafeFromStartup: settingsBefore.effective.autoCompactWindow === 100000 && settingsAfter.effective.autoCompactWindow === 100000 })
    const permissionPid = f.sessions.get('live')!.pid
    const permission = await f.rpc('session.set-permission-mode', { command: { commandId: 'permission-respawn', commandType: 'session.set-permission-mode', requestHash: 'permission-respawn' }, input: { sessionId: 'live', provider: 'claude-code', permissionMode: 'acceptEdits', respawn: true, now: Date.now() } })
    expect(permission.type).toBe('rpc.response')
    expect(f.sessions.get('live')!.pid).not.toBe(permissionPid)
    await f.control({ subtype: 'initialize' })
    const permissionAnswer = await f.prompt('SUBSCRIPTION_PERMISSION_STAGE4')
    expect(permissionAnswer.session_id).toBe(providerIdentity)
    expect(permissionAnswer.result).toContain('SUBSCRIPTION_FIXTURE_OK')
    expect((await f.launches()).find(launch => launch.name === `launch-${f.sessions.get('live')!.pid}.json`)?.argv).toContain('--permission-mode')
    f.steps.push({ step: 'permission-respawn', previousPid: permissionPid, pid: f.sessions.get('live')!.pid, providerIdentity: permissionAnswer.session_id })
    const restorePid = f.sessions.get('live')!.pid
    await f.sessions.shutdownAll()
    f.spawn(); await until(() => f.sessions.has('live'), 'subscription restore PTY')
    expect(f.sessions.get('live')!.pid).not.toBe(restorePid)
    await f.control({ subtype: 'initialize' })
    const resumeAnswer = await f.prompt('SUBSCRIPTION_RESTORE_STAGE4')
    expect(resumeAnswer.session_id).toBe(providerIdentity)
    expect(resumeAnswer.result).toContain('SUBSCRIPTION_FIXTURE_OK')
    f.steps.push({ step: 'subscription-restore', previousPid: restorePid, pid: f.sessions.get('live')!.pid, providerIdentity: resumeAnswer.session_id })
    f.database.run("INSERT INTO sessions (id,task_id,execution_context_id,kind,status,title,created_at,updated_at,last_activity_at) VALUES ('fork-child','task','ctx','claude-code','exited','Fork',1,1,1)")
    f.database.run("INSERT INTO session_fork_intents (session_id,source_session_id,source_provider,source_provider_session_id,permission_mode,state,created_at) VALUES ('fork-child','live','claude-code',?,'acceptEdits','pending',1)", providerIdentity)
    f.spawn('fork-child'); await until(() => f.sessions.has('fork-child'), 'subscription fork PTY')
    await f.control({ subtype: 'initialize' }, 'fork-child')
    const forkAnswer = await f.prompt('SUBSCRIPTION_FORK_STAGE4', 'fork-child')
    expect(forkAnswer.result).toContain('SUBSCRIPTION_FIXTURE_OK')
    expect(forkAnswer.session_id).not.toBe(providerIdentity)
    const forkPid = f.sessions.get('fork-child')!.pid
    const forkLaunch = (await f.launches()).find(launch => launch.name === `launch-${forkPid}.json`)!
    expect(forkLaunch.argv).toContain('--fork-session')
    expect(forkLaunch.argv).toContain(providerIdentity)
    expect(forkLaunch.settings.env).toEqual({ ANTHROPIC_API_KEY: '', CLAUDE_CODE_OAUTH_TOKEN: '', CLAUDE_CODE_DISABLE_1M_CONTEXT: '1' })
    f.steps.push({ step: 'subscription-fork', pid: forkPid, sourceProviderIdentity: providerIdentity, providerIdentity: forkAnswer.session_id, argv: forkLaunch.argv })
    await f.sessions.get('fork-child')!.shutdownForRuntime()
    f.sessions.delete('fork-child')
    const subPid = f.sessions.get('live')!.pid
    await until(() => f.sessions.providerIdentityConfirmed(f.sessions.get('live')!.runId!), 'subscription identity')
    const restored = await f.rpc('provider-config.activate', { cli: 'claude-code', providerId: 'anthropic-official' })
    expect(restored).toMatchObject({ type: 'rpc.response', result: { activeProviderIds: { 'claude-code': 'anthropic-official' }, sessionTransitions: [{ sessionId: 'live', status: 'updated' }] } })
    expect(f.sessions.get('live')!.pid).not.toBe(subPid)
    await f.control({ subtype: 'initialize' })
    const officialSettings = await f.control({ subtype: 'get_settings' }), returned = await f.prompt('OFFICIAL_AFTER_STAGE4')
    expect(returned.result).toContain('OFFICIAL_FIXTURE_OK')
    expect(returned.session_id).toBe(providerIdentity)
    expect(officialSettings.effective.env.ANTHROPIC_API_KEY).toBe('OLD_SETTINGS_API')
    expect(officialSettings.effective.autoCompactWindow).toBe(400000)
    expect((await f.launches()).find(launch => launch.name === `launch-${f.sessions.get('live')!.pid}.json`)?.settings.env).toBeUndefined()
    f.steps.push({ step: 'official-after', pid: f.sessions.get('live')!.pid, providerIdentity: returned.session_id, settings: officialSettings, result: returned.result })
    await until(() => f.manager.status.leases === 0, 'subscription session lease release')
    await until(() => f.manager.status.state === 'stopped', 'idle router shutdown')
  // Multiple native startups, recovery, menus and tool turns share this budget.
  // Keep each wait bounded above; allow the aggregate to survive full-suite load.
  }, 180000)
})
