import { afterEach, describe, expect, it } from 'vitest'
import { EventEmitter } from 'node:events'
import { execFileSync, spawn } from 'node:child_process'
import { mkdir, mkdtemp, readFile, realpath, stat, symlink, writeFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { createServer as createHttpServer } from 'node:http'
import { createServer as createNetServer } from 'node:net'
import { setTimeout as delay } from 'node:timers/promises'
import { ModelRouterManager } from './manager'
import { RouterProcessOwner } from './process-owner'
import { prepareRouterDirectory, routerConfig, writeRouterConfig } from './config'
import { resolveRouterProxy, type RouterProxyResolution } from './network-proxy'

const project = resolve(import.meta.dirname, '../../../..')
const binary = join(project, `.cache/model-router/8.0.69/darwin-${process.arch}/model-router`)
const supervisor = join(project, 'apps/runtime/dist/model-router-supervisor.cjs')
const output = join(project, 'output/model-router-stage2-rework')
const managers: ModelRouterManager[] = []
afterEach(async () => { await Promise.all(managers.splice(0).map(manager => manager.close())) })
async function fixture(idleMs = 50, resolveProxy: () => Promise<RouterProxyResolution> = async () => ({ source: 'direct' }), throughSymlink = false) {
  await mkdir(output, { recursive: true })
  const physicalRoot = await mkdtemp(join(output, 'manager-'))
  const dataRoot = throughSymlink ? join(physicalRoot, 'data-alias') : physicalRoot
  if (throughSymlink) await symlink(physicalRoot, dataRoot, 'dir')
  const owner = new RouterProcessOwner({ executable: process.execPath, supervisor, binary, dataRoot })
  const events: unknown[] = []
  owner.onEvent(event => events.push(event))
  const manager = new ModelRouterManager({ dataRoot, control: owner, idleMs, resolveProxy })
  managers.push(manager)
  return { manager, owner, dataRoot, canonicalDataRoot: await realpath(physicalRoot), events }
}
async function until(check: () => boolean | Promise<boolean>, timeout = 6_000) {
  const deadline = Date.now() + timeout
  while (Date.now() < deadline) { if (await check()) return; await delay(20) }
  throw new Error('Condition timed out')
}
function exists(pid: number) { try { process.kill(pid, 0); return true } catch { return false } }

describe.runIf(process.env.MATOU_TEST_MODEL_ROUTER === '1')('fixed engine manager and real supervisor', () => {
  it('starts the actual engine through a symlink data root and writes a matching canonical auth-dir', async () => {
    const { manager, dataRoot, canonicalDataRoot } = await fixture(50, async () => ({ source: 'direct' }), true)
    const lease = await manager.acquire('symlink-data-root')
    const config = JSON.parse(await readFile(join(dataRoot, 'model-router/config.json'), 'utf8'))
    expect(config.oauth['auth-dir']).toBe(join(canonicalDataRoot, 'model-router/credentials'))
    const response = await fetch(`${lease.connection.baseUrl}/v8/management/credentials`, { headers: { Authorization: `Bearer ${lease.connection.managementKey}` } })
    expect(response.ok).toBe(true)
    expect(await response.json()).toMatchObject({ files: [] })
    expect(exists(lease.connection.pid)).toBe(true)
    lease.release()
    await until(() => manager.status.state === 'stopped')
    expect(exists(lease.connection.pid)).toBe(false)
  }, 15000)
  it.each(['HTTPS', 'SOCKS'] as const)('uses the system %s fallback for real engine traffic when the login shell has no proxy', async kind => {
    const targets: string[] = []
    let systemOutput = ''
    let selection: RouterProxyResolution | undefined
    const { manager, dataRoot } = await fixture(50, async () => {
      selection = await resolveRouterProxy({}, { loadShell: async () => ({}), readSystem: async () => systemOutput, platform: 'darwin' })
      return selection
    })
    const proxy = kind === 'HTTPS' ? createHttpServer((_request, response) => { response.writeHead(502); response.end() }) : createNetServer(socket => {
      let data = Buffer.alloc(0), greeted = false
      socket.on('error', () => {})
      socket.on('data', chunk => {
        data = Buffer.concat([data, typeof chunk === 'string' ? Buffer.from(chunk) : chunk])
        if (!greeted) {
          if (data.length < 2 || data.length < 2 + data[1]!) return
          data = data.subarray(2 + data[1]!)
          greeted = true
          socket.write(Buffer.from([5, 0]))
        }
        if (data.length < 5) return
        const type = data[3], offset = type === 3 ? 5 : 4
        const length = type === 3 ? data[4]! : type === 1 ? 4 : 16
        if (data.length < offset + length + 2) return
        const host = type === 3 ? data.toString('utf8', offset, offset + length) : type === 1 ? [...data.subarray(offset, offset + length)].join('.') : 'ipv6-fixture'
        targets.push(`${host}:${data.readUInt16BE(offset + length)}`)
        // Refuse instead of forwarding; no real provider connection or token.
        socket.end(Buffer.from([5, 5, 0, 1, 0, 0, 0, 0, 0, 0]))
      })
    })
    if (kind === 'HTTPS') proxy.on('connect', (request, socket) => { targets.push(request.url ?? ''); socket.end('HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n') })
    await new Promise<void>(done => proxy.listen(0, '127.0.0.1', done))
    try {
      const port = (proxy.address() as { port: number }).port
      systemOutput = `<dictionary> {\n ${kind}Enable : 1\n ${kind}Proxy : 127.0.0.1\n ${kind}Port : ${port}\n}`
      const { directory } = await prepareRouterDirectory(dataRoot)
      await writeFile(join(directory, 'credentials/fixture-codex.json'), JSON.stringify({ type: 'codex', access_token: 'synthetic-system-proxy', expired: '2099-01-01T00:00:00Z', prefix: 'matou-chatgpt' }), { mode: 0o600 })
      const { connection } = await manager.acquire('system-proxy:test')
      await manager.waitForCredentialAdoption({ name: 'fixture-codex.json', provider: 'codex' })
      const response = await fetch(`${connection.baseUrl}/v1/messages`, { method: 'POST', headers: { Authorization: `Bearer ${connection.clientKey}`, 'Content-Type': 'application/json' }, body: JSON.stringify({ model: 'matou-chatgpt/gpt-6.1-sol', max_tokens: 8, messages: [{ role: 'user', content: 'ping' }] }), signal: AbortSignal.timeout(5_000) })
      await response.body?.cancel()
      expect(targets).toContain('chatgpt.com:443')
      expect(selection).toEqual({ source: 'system', proxyUrl: `${kind === 'HTTPS' ? 'http' : 'socks5'}://127.0.0.1:${port}` })
      await writeFile(join(dataRoot, `system-proxy-${kind.toLowerCase()}-evidence.json`), JSON.stringify({ selection, systemOutput, targets, credentialProxyOverride: false, systemSettingsChanged: false }, null, 2))
    } finally { await manager.close(); await new Promise<void>(done => proxy.close(() => done())) }
  }, 20_000)
  it('retries the same single OAuth account immediately after a transport failure', async () => {
    let environment: NodeJS.ProcessEnv = {}
    let selection: RouterProxyResolution | undefined
    const { manager, dataRoot } = await fixture(50, async () => { selection = await resolveRouterProxy(environment); return selection })
    const targets: string[] = []
    const proxy = createHttpServer()
    proxy.on('connect', (request, socket) => { targets.push(request.url ?? ''); socket.end('HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n') })
    await new Promise<void>(done => proxy.listen(0, '127.0.0.1', done))
    try {
      const proxyPort = (proxy.address() as { port: number }).port
      const home = join(dataRoot, 'login-home')
      await mkdir(home)
      await writeFile(join(home, '.zshrc'), `printf x >> "$HOME/proxy-shell-reads"\nexport https_proxy='http://127.0.0.1:${proxyPort}'\nexport no_proxy='localhost,127.0.0.1'\nexport ANTHROPIC_API_KEY='fixture-do-not-forward'\n`)
      environment = { SHELL: '/bin/zsh', HOME: home, ZDOTDIR: home, PATH: '/usr/bin:/bin' }
      const { directory } = await prepareRouterDirectory(dataRoot)
      await writeFile(join(directory, 'credentials', 'fixture-codex.json'), JSON.stringify({ type: 'codex', email: 'fixture@example.invalid', access_token: 'synthetic-not-a-real-token', expired: '2099-01-01T00:00:00Z', prefix: 'matou-chatgpt' }), { mode: 0o600 })
      const { connection } = await manager.acquire('cooldown:test')
      const adopted = await manager.waitForCredentialAdoption({ name: 'fixture-codex.json', provider: 'codex' })
      expect(adopted.status).toBe('active')
      expect(selection).toMatchObject({ source: 'shell', proxyUrl: `http://127.0.0.1:${proxyPort}` })
      const responses = []
      for (let i = 0; i < 2; i++) {
        const before = targets.filter(target => target === 'chatgpt.com:443').length
        const response = await fetch(`${connection.baseUrl}/v1/messages`, { method: 'POST', headers: { Authorization: `Bearer ${connection.clientKey}`, 'Content-Type': 'application/json' }, body: JSON.stringify({ model: 'matou-chatgpt/gpt-6.1-sol', max_tokens: 8, messages: [{ role: 'user', content: 'ping' }] }), signal: AbortSignal.timeout(5_000) })
        const body = await response.text()
        const attempts = targets.filter(target => target === 'chatgpt.com:443').length - before
        responses.push({ status: response.status, body, attempts })
        expect(attempts).toBeGreaterThan(0)
        expect(body).not.toContain('auth_unavailable')
      }
      const google = await fetch(`${connection.baseUrl}/v8/management/requests/api-call`, {
        method: 'POST', headers: { Authorization: `Bearer ${connection.managementKey}`, 'Content-Type': 'application/json' },
        body: JSON.stringify({ method: 'GET', url: 'https://oauth2.googleapis.com/token', auth_index: adopted.authIndex, header: { Authorization: 'Bearer $TOKEN$' } }),
        signal: AbortSignal.timeout(5_000)
      })
      await google.body?.cancel()
      expect(targets).toContain('oauth2.googleapis.com:443')
      expect(await readFile(join(home, 'proxy-shell-reads'), 'utf8')).toBe('x')
      await writeFile(join(dataRoot, 'cooldown-evidence.json'), JSON.stringify({ selection, adopted, targets, responses, shellReads: 1, credentialProxyOverride: false }, null, 2))
    } finally { await manager.close(); await new Promise<void>(done => proxy.close(() => done())) }
  }, 20_000)

  it('preserves 429 type and message and bounds private error logs (records protocol normalization)', async () => {
    const { owner, dataRoot } = await fixture()
    let calls = 0
    const quota = { error: { type: 'usage_limit_reached', message: 'Fixture quota exhausted. Retry in 60 seconds.', resets_in_seconds: 60 } }
    const upstream = createHttpServer((request, response) => {
      request.resume(); calls++
      response.writeHead(429, { 'Content-Type': 'application/json', 'Retry-After': '60' })
      response.end(JSON.stringify(quota))
    })
    await new Promise<void>(done => upstream.listen(0, '127.0.0.1', done))
    try {
      const { directory, keys } = await prepareRouterDirectory(dataRoot)
      const socket = createNetServer(); await new Promise<void>(done => socket.listen(0, '127.0.0.1', done))
      const port = (socket.address() as { port: number }).port
      await new Promise<void>(done => socket.close(() => done()))
      const config = {
        ...routerConfig(directory, port, keys),
        'api-keys': { codex: [{ name: 'fixture', prefix: 'fixture', 'base-url': `http://127.0.0.1:${(upstream.address() as { port: number }).port}`, models: [{ name: 'gpt-6.1-sol', alias: 'fixture-gpt' }], keys: [{ 'api-key': 'fixture-local-only' }] }] }
      }
      await writeFile(join(directory, 'config.json'), JSON.stringify(config), { mode: 0o600 })
      await owner.start()
      const base = `http://127.0.0.1:${port}`
      await until(async () => { try { return (await fetch(`${base}/healthz`)).ok } catch { return false } })
      const request = () => fetch(`${base}/v1/messages`, { method: 'POST', headers: { Authorization: `Bearer ${keys.client}`, 'Content-Type': 'application/json' }, body: JSON.stringify({ model: 'fixture/fixture-gpt', max_tokens: 8, messages: [{ role: 'user', content: 'ping' }] }), signal: AbortSignal.timeout(3_000) })
      const first = await request()
      const body = await first.text()
      expect(first.status).toBe(429)
      expect(first.headers.get('retry-after')).toBe('60')
      expect(JSON.parse(body)).toEqual({ type: 'error', error: { type: quota.error.type, message: quota.error.message } })
      expect(calls).toBe(1)
      for (let i = 0; i < 14; i++) { const response = await request(); await response.body?.cancel() }
      const { readdir } = await import('node:fs/promises')
      const logs = (await readdir(join(directory, 'logs'))).filter(name => name.endsWith('.log'))
      expect(logs.length).toBeGreaterThan(0)
      expect(logs.length).toBeLessThanOrEqual(10)
      for (const file of logs) {
        const path = join(directory, 'logs', file)
        expect((await stat(path)).mode & 0o777).toBe(0o600)
        const contents = await readFile(path, 'utf8')
        expect(contents).not.toContain(keys.client)
        expect(contents).not.toContain(keys.management)
        expect(contents).not.toContain('fixture-local-only')
      }
      await writeFile(join(dataRoot, 'quota-evidence.json'), JSON.stringify({ status: first.status, upstreamBody: quota, body: JSON.parse(body), retryAfter: first.headers.get('retry-after'), protocolNormalization: true, calls, logs: logs.length, privateModes: true }, null, 2))
    } finally { await owner.stop(); await new Promise<void>(done => upstream.close(() => done())) }
  }, 20_000)
  it('coalesces concurrent startup, authenticates and listens only on loopback; stops after last release', async () => {
    const { manager, dataRoot, events } = await fixture(1_000)
    const startedAt = performance.now()
    const leases = await Promise.all(Array.from({ length: 20 }, (_, i) => manager.acquire(`session:${i}`)))
    const startupMs = performance.now() - startedAt
    const connection = leases[0]!.connection
    expect(new Set(leases.map(lease => lease.connection.baseUrl)).size).toBe(1)
    expect(events).toHaveLength(1)
    const listener = execFileSync('lsof', ['-nP', '-a', '-p', String(connection.pid), '-iTCP', '-sTCP:LISTEN'], { encoding: 'utf8' })
    expect(listener).toContain('127.0.0.1:')
    expect(listener).not.toMatch(/\*:\d+/)
    expect((await fetch(`${connection.baseUrl}/v8/management/credentials`)).status).toBe(401)
    for (const path of ['keys.json', 'config.json']) expect((await stat(join(dataRoot, 'model-router', path))).mode & 0o777).toBe(0o600)
    leases.slice(0, -1).forEach(lease => lease.release())
    await delay(100)
    expect(exists(connection.pid)).toBe(true)
    const releasedAt = performance.now()
    leases.at(-1)!.release()
    await until(() => manager.status.state === 'stopped')
    const idleStopMs = performance.now() - releasedAt
    expect(exists(connection.pid)).toBe(false)
    await writeFile(join(dataRoot, 'evidence.json'), JSON.stringify({ concurrent: 20, events, listener, startupMs, idleMs: 1_000, idleStopMs, idleStopped: true }, null, 2))
  }, 20_000)

  it('starts when settings opens and releases only the disconnected window, including during startup', async () => {
    const { manager } = await fixture()
    const first = new EventEmitter(), second = new EventEmitter()
    const a = manager.bindSettingsConnection(first), b = manager.bindSettingsConnection(second)
    const lease = await a.open(), other = await b.open()
    first.emit('close')
    expect(manager.status.leases).toBe(1)
    await expect(a.open()).rejects.toThrow('closed')
    await delay(100)
    expect(exists(other.connection.pid)).toBe(true)
    second.emit('close')
    await until(() => manager.status.state === 'stopped')
    expect(exists(lease.connection.pid)).toBe(false)
    const third = new EventEmitter(), c = manager.bindSettingsConnection(third)
    const pending = c.open()
    third.emit('close')
    await expect(pending).rejects.toThrow('released')
    await until(() => manager.status.state === 'stopped')
  }, 20_000)

  it('restarts once on the same port, then fails without a third process', async () => {
    const { manager, events, dataRoot } = await fixture()
    const lease = await manager.acquire('session:test')
    const first = lease.connection.pid
    process.kill(first, 'SIGKILL')
    await until(() => !!manager.status.pid && manager.status.pid !== first)
    const second = manager.status.pid!
    await until(async () => { try { return (await fetch(`${lease.connection.baseUrl}/healthz`)).ok } catch { return false } })
    expect(exists(first)).toBe(false)
    process.kill(second, 'SIGKILL')
    await until(() => manager.status.state === 'failed')
    await delay(200)
    expect(events.filter(event => (event as { state: string }).state === 'spawned')).toHaveLength(2)
    expect(exists(second)).toBe(false)
    await writeFile(join(dataRoot, 'restart-evidence.json'), JSON.stringify({ first, second, events, sameBaseUrl: lease.connection.baseUrl }, null, 2))
  }, 20_000)

  it('main SIGKILL closes the stdin owner lease and reaps its router', async () => {
    const { dataRoot, manager } = await fixture()
    await manager.close()
    const { directory, keys } = await prepareRouterDirectory(dataRoot)
    const source = `import { RouterProcessOwner } from ${JSON.stringify(join(project, 'apps/runtime/src/model-router/process-owner.ts'))}; const owner = new RouterProcessOwner(${JSON.stringify({ executable: process.execPath, supervisor, binary, dataRoot })}); owner.onEvent(event => console.log(JSON.stringify(event))); await owner.start(); setInterval(() => {}, 1000)`
    // Only the lifetime probe needs a fixed free port; no HTTP request is sent.
    const net = await import('node:net')
    const socket = net.createServer(); await new Promise<void>(done => socket.listen(0, '127.0.0.1', done))
    const port = (socket.address() as { port: number }).port
    await new Promise<void>(done => socket.close(() => done()))
    await writeRouterConfig(directory, port, keys)
    const child = spawn(process.execPath, ['--experimental-strip-types', '--input-type=module', '-e', source], { stdio: ['ignore', 'pipe', 'pipe'] })
    let raw = '', errors = ''; child.stdout.on('data', chunk => { raw += chunk }); child.stderr.on('data', chunk => { errors += chunk })
    let routerPid = 0
    try {
      await until(() => { routerPid = JSON.parse(raw.trim().split('\n')[0] || '{}').pid ?? 0; if (child.exitCode !== null) throw new Error(errors); return routerPid > 0 })
      child.kill('SIGKILL')
      await until(() => !exists(routerPid))
      await writeFile(join(dataRoot, 'owner-kill-evidence.json'), JSON.stringify({ mainPid: child.pid, routerPid, routerGone: true }, null, 2))
    } finally { child.kill('SIGKILL'); if (routerPid && exists(routerPid)) process.kill(routerPid, 'SIGKILL') }
  }, 20_000)
})
