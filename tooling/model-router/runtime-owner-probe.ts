import assert from 'node:assert/strict'
import { mkdtemp, writeFile } from 'node:fs/promises'
import { mkdirSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { setTimeout as delay } from 'node:timers/promises'
import { app } from 'electron'
import { RuntimeHost, type RuntimeScaleMetrics } from '../../apps/desktop/src/main/runtime-host'

const project = resolve(process.env.MATOU_PROBE_PROJECT!)
const output = join(project, 'output/model-router-stage2')
for (const key of ['userData', 'sessionData', 'crashDumps', 'logs'] as const) {
  const path = join(output, 'electron-probe', key)
  mkdirSync(path, { recursive: true })
  app.setPath(key, path)
}
async function main(): Promise<void> {
  await app.whenReady()
  app.dock?.hide()
  const dataRoot = await mkdtemp(join(output, 'electron-runtime-'))
  process.env.MATOU_DATA_DIR = dataRoot
  // Exercise default login-shell resolution without loading the user's startup
  // files or allowing shell plugins to write outside this isolated fixture.
  const loginHome = join(dataRoot, 'login-home')
  mkdirSync(loginHome)
  await writeFile(join(loginHome, '.zshrc'), 'unset HTTPS_PROXY https_proxy ALL_PROXY all_proxy HTTP_PROXY http_proxy NO_PROXY no_proxy\n')
  process.env.HOME = loginHome
  process.env.ZDOTDIR = loginHome
  process.env.SHELL = '/bin/zsh'
  const authorizationUrls: string[] = []
  const host = new RuntimeHost(join(output, 'runtime-fixture.cjs'), undefined, {
    openExternal: async url => { authorizationUrls.push(url) },
    modelRouterBinary: join(project, `.cache/model-router/8.0.69/darwin-${process.arch}/model-router`)
  })
  function alive(pid: number): boolean { try { process.kill(pid, 0); return true } catch { return false } }
  const evidence: { first?: RuntimeScaleMetrics; second?: RuntimeScaleMetrics; oldRouterGone?: boolean; stoppedRouterGone?: boolean; error?: string; authorizationUrls?: string[]; authorizationBridge?: boolean } = {}
  try {
    await host.start()
    evidence.first = await host.getScaleMetrics()
    assert(evidence.first.ptyPids[0])
    process.kill(evidence.first.runtimePid, 'SIGKILL')
    const deadline = Date.now() + 15_000
    while (Date.now() < deadline) {
      await delay(50)
      try {
        const next = await host.getScaleMetrics()
        if (next.runtimePid !== evidence.first.runtimePid) { evidence.second = next; break }
      } catch { /* Host is reclaiming the dead generation before restarting. */ }
    }
    assert(evidence.second, 'Replacement runtime did not become ready')
    evidence.authorizationUrls = authorizationUrls
    evidence.authorizationBridge = authorizationUrls.length === 2 && authorizationUrls.every(url => url.startsWith('https://auth.openai.com/oauth/authorize?'))
    assert(evidence.authorizationBridge, 'Runtime authorization URL did not reach the main openExternal handler')
    evidence.oldRouterGone = !alive(evidence.first.ptyPids[0]!)
    assert(evidence.oldRouterGone, 'Old router survived runtime SIGKILL')
    assert(evidence.first.ptyPids[0] !== evidence.second.ptyPids[0])
    await host.stop()
    evidence.stoppedRouterGone = !alive(evidence.second.ptyPids[0]!)
    assert(evidence.stoppedRouterGone)
    console.log('PASS real Electron RuntimeHost: allowlisted authorization bridge, SIGKILL runtime, reclaim old router before restart, then clean shutdown')
  } catch (error) {
    evidence.error = error instanceof Error ? error.message : String(error)
    console.error(evidence.error)
    process.exitCode = 1
  } finally {
    await host.stop()
    await writeFile(join(dataRoot, 'evidence.json'), JSON.stringify(evidence, null, 2))
    await writeFile(join(output, 'latest-electron-runtime.txt'), `${dataRoot}\n`)
    app.exit(process.exitCode ?? 0)
  }
}
void main().catch(error => { console.error(error); app.exit(1) })
