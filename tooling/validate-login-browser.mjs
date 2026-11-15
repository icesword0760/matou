// Visible, opt-in acceptance only. Build first; no real authorization request is sent.
// MATOU_SAFE_BROWSER_PATH=/path/to/Chromium node tooling/validate-login-browser.mjs
import assert from 'node:assert/strict'
import { execFile } from 'node:child_process'
import { existsSync } from 'node:fs'
import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { createServer } from 'node:http'
import { dirname, join, resolve } from 'node:path'
import { promisify } from 'node:util'
import { fileURLToPath } from 'node:url'
import { _electron } from '@playwright/test'

const project = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const desktop = join(project, 'apps/desktop')
const evidenceRoot = join(project, '.cache/login-integration')
await mkdir(evidenceRoot, { recursive: true })
// macOS Unix sockets have a short path limit; all data still stays in this worktree.
const root = await mkdtemp(join(project, '.cache/l-'))
const userData = join(root, 'userData')
const profile = join(userData, 'safe-browser/profile')
await mkdir(userData)
await mkdir(join(root, 'workspace'))
const entry = join(desktop, 'out/main/login-acceptance.mjs')
const source = await readFile(join(desktop, 'out/main/index.js'), 'utf8')
assert(source.includes('const safeBrowser = new SafeBrowserController('), 'Build the current desktop first')
const egressFixture = join(root, 'egress.json')
const exit = { ip: '169.128.10.1', country: 'JP' }
await writeFile(egressFixture, JSON.stringify({
  anthropic: { api: exit, login: exit }, openai: { api: exit, login: exit },
  browserLogin: null, systemProxyEnabled: false, tunDetected: true, ipv6Bypass: false
}))
const accountFixture = join(root, 'account-fixture.py')
await writeFile(accountFixture, 'import json,sys\njson.loads(sys.stdin.readline())\nprint(json.dumps({"ok":True,"value":{"accounts":[]}}))\n')
let reportedZone
const server = createServer((request, response) => {
  if (request.url?.startsWith('/report?')) {
    reportedZone = new URL(request.url, 'http://localhost').searchParams.get('zone')
    response.end('ok')
  } else {
    response.setHeader('Content-Type', 'text/html; charset=utf-8')
    response.end(`<!doctype html><meta charset="utf-8"><title>Matou 登录改道验证</title>
      <h1>安全浏览器已打开</h1><p>本页是本地测试替身，不连接授权服务。</p>
      <p id="zone"></p><script>
      const zone = Intl.DateTimeFormat().resolvedOptions().timeZone;
      document.getElementById('zone').textContent = '浏览器时区：' + zone;
      fetch('/report?zone=' + encodeURIComponent(zone));</script>`)
  }
})
await new Promise(done => server.listen(0, '127.0.0.1', done))
const localUrl = `http://127.0.0.1:${server.address().port}/`

// Test entry only: preserve the full real app, real RuntimeHost allowlist and
// real controller. Substitute the URL at the browser boundary so no OAuth site
// receives even a fixture request. This generated file is never packaged.
const before = `
app.getAppPath = () => ${JSON.stringify(desktop)};
let __acceptanceChild;
let __acceptanceResult;
let __acceptanceTriggered = false;
const __originalFork = utilityProcess.fork.bind(utilityProcess);
utilityProcess.fork = (...args) => {
  const child = __originalFork(...args);
  __acceptanceChild = child;
  const post = child.postMessage.bind(child);
  child.postMessage = (message, ...ports) => {
    if (message.type === 'runtime.model-router-result' && message.requestId === 'visible-sign-in-fixture') __acceptanceResult = message.ok;
    return post(message, ...ports);
  };
  return child;
};
app.on('web-contents-created', (_event, contents) => {
  contents.on('before-input-event', (event, input) => {
    if (input.type !== 'keyDown' || input.key.toLowerCase() !== 'l' || !input.control || !input.shift) return;
    event.preventDefault();
    if (__acceptanceTriggered) return;
    __acceptanceTriggered = true;
    __acceptanceChild.emit('message', {
      type: 'runtime.model-router-request', requestId: 'visible-sign-in-fixture',
      action: 'open-authorization', provider: 'codex',
      url: 'https://auth.openai.com/oauth/authorize?redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback'
    });
  });
});
`
const after = `
const __realOpenPersistent = safeBrowser.openPersistent.bind(safeBrowser);
safeBrowser.openPersistent = (_fixtureUrl, timezone) => __realOpenPersistent(${JSON.stringify(localUrl)}, timezone);
globalThis.__matouLoginAcceptance = () => ({
  timezone: safeBrowserTimeZone(), triggered: __acceptanceTriggered, result: __acceptanceResult,
  lifecycle: runtimeHost?.getLifecycle().snapshot
});
`
await writeFile(entry, before + source + after)
const app = await _electron.launch({
  args: [entry, `--user-data-dir=${userData}`],
  cwd: project,
  env: {
    ...process.env, MATOU_E2E: '1', MATOU_E2E_DISPLAY: 'primary',
    MATOU_DISABLE_AUTO_UPDATE: '1', MATOU_DATA_DIR: join(root, 'data'),
    MATOU_DEFAULT_WORKSPACE: join(root, 'workspace'), ELECTRON_USER_DATA_DIR: userData,
    MATOU_RUNTIME_ENTRY: join(project, 'apps/runtime/dist/index.cjs'),
    MATOU_E2E_EGRESS_FIXTURE: egressFixture, MATOU_E2E_ACCOUNTS_BRIDGE: accountFixture,
    MATOU_LOCALE: 'zh-CN'
  }
})
app.process().stderr?.on('data', chunk => {
  // Test diagnostics still redact addresses; no authorization URL enters logs.
  process.stderr.write(chunk.toString().replace(/https?:\/\/[^\s"'<>]+/g, '[redacted-url]'))
})
const execFileAsync = promisify(execFile)
const processRows = async (group) => (await execFileAsync('/bin/ps', ['-axo', 'pid=,ppid=,pgid=,command='])).stdout
  .split('\n').filter(row => group === undefined
    ? row.includes(`--user-data-dir=${profile}`)
    : Number(row.trim().split(/\s+/)[2]) === group)
const waitFor = async (check, label, timeout = 300_000) => {
  const deadline = Date.now() + timeout
  while (Date.now() < deadline) {
    if (await check()) return
    await new Promise(done => setTimeout(done, 200))
  }
  throw new Error(`Timed out: ${label}`)
}
let closed = false
app.on('close', () => { closed = true })
try {
  const page = await app.firstWindow()
  console.log(JSON.stringify({ stage: 'window-created', root, mainPid: app.process().pid }))
  page.on('pageerror', error => console.log(JSON.stringify({ stage: 'test-page-error', message: error.message })))
  try {
    await waitFor(async () => (await app.evaluate(() => globalThis.__matouLoginAcceptance())).timezone === 'Asia/Tokyo', 'egress IPC', 30_000)
  } catch (error) {
    console.log(JSON.stringify(await app.evaluate(() => ({
      stage: 'test-egress-timeout', context: globalThis.__matouLoginAcceptance()
    }))))
    throw error
  }
  console.log(JSON.stringify({ stage: 'ready-for-manual-key', root, profile, mainPid: app.process().pid, key: 'Control+Shift+L' }))
  await waitFor(async () => (await app.evaluate(() => globalThis.__matouLoginAcceptance())).result === true && reportedZone, 'manual sign-in browser trigger')
  assert.equal(reportedZone, 'Asia/Tokyo')
  const profileProcesses = await processRows()
  const browserProcess = profileProcesses.find(row => row.includes('--remote-debugging-pipe'))
  assert(browserProcess)
  const group = Number(browserProcess.trim().split(/\s+/)[2])
  const processesBefore = await processRows(group)
  const seed = JSON.parse(await readFile(join(userData, 'safe-browser.json'), 'utf8')).fingerprintSeed
  assert(processesBefore.some(row => row.includes('--remote-debugging-pipe') && row.includes(`--fingerprint=${seed}`)))
  assert(existsSync(profile))
  const report = {
    checkedAt: new Date().toISOString(), root, profile, mainPid: app.process().pid,
    browser: process.env.MATOU_SAFE_BROWSER_PATH, group, seed, timezone: reportedZone,
    requestPath: 'manual key → injected runtime request → real allowlist → shared controller → local URL substitute',
    processesBefore, profileExistedWhileRunning: true
  }
  await writeFile(join(evidenceRoot, 'visible-running.json'), JSON.stringify(report, null, 2))
  console.log(JSON.stringify({ stage: 'browser-open-quit-matou-after-visual-check', root, profile, timezone: reportedZone }))
  await waitFor(() => closed, 'manual Matou quit')
  await waitFor(async () => (await processRows(group)).length === 0, 'browser cleanup', 15_000)
  report.processesAfter = await processRows(group)
  report.profileExistsAfterExit = existsSync(profile)
  assert(report.profileExistsAfterExit)
  await writeFile(join(evidenceRoot, 'visible-result.json'), JSON.stringify(report, null, 2))
  console.log(JSON.stringify({ stage: 'passed', evidence: join(evidenceRoot, 'visible-result.json') }))
} finally {
  if (!closed) await app.close().catch(() => {})
  await rm(entry, { force: true })
  await new Promise(done => server.close(done))
}
