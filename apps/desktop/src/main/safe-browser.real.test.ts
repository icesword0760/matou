// Opt-in, visible-kernel acceptance: MATOU_SAFE_BROWSER_REAL_TEST=1 and
// MATOU_SAFE_BROWSER_PATH=/path/to/Chromium pnpm --filter @matou/desktop exec vitest run src/main/safe-browser.real.test.ts
import { spawn, execFileSync } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync, writeFileSync } from 'node:fs'
import { createServer } from 'node:http'
import { join, resolve } from 'node:path'
import { afterEach, expect, it, vi } from 'vitest'
import { SafeBrowserController, safeBrowserStatus } from './safe-browser'
import { BrowserPipe } from './safe-browser-pipe'

vi.mock('electron', async () => {
  const { EventEmitter } = await import('node:events')
  return { app: Object.assign(new EventEmitter(), { getPath() { throw new Error('Real acceptance must use isolated userData') }, quit: vi.fn() }) }
})
vi.mock('node:child_process', async importOriginal => {
  const actual = await importOriginal<typeof import('node:child_process')>()
  return { ...actual, spawn: vi.fn(actual.spawn) }
})

afterEach(() => { vi.restoreAllMocks() })

it.runIf(process.env.MATOU_SAFE_BROWSER_REAL_TEST === '1')('real kernel: verifies page timezones, final-window exit, process groups and ephemeral deletion', async () => {
  const evidenceRoot = resolve(process.cwd(), '../../.cache/safe-browser')
  mkdirSync(evidenceRoot, { recursive: true })
  const userData = mkdtempSync(join(evidenceRoot, 'real-userData-'))
  const controller = new SafeBrowserController(userData)
  const pipes: BrowserPipe[] = []
  const ready = BrowserPipe.prototype.ready
  vi.spyOn(BrowserPipe.prototype, 'ready').mockImplementation(async function (this: BrowserPipe) {
    pipes.push(this)
    await ready.call(this)
  })
  const commands = vi.spyOn(BrowserPipe.prototype, 'send')
  const reports = new Map<string, { promise: Promise<string>; resolve(zone: string): void }>()
  for (const name of ['persistent', 'ephemeral']) {
    let resolve!: (zone: string) => void
    const promise = new Promise<string>(yes => { resolve = yes })
    reports.set(name, { promise, resolve })
  }
  const server = createServer((req, res) => {
    const url = new URL(req.url!, 'http://localhost')
    const kind = url.searchParams.get('kind')!
    if (url.pathname === '/report') {
      reports.get(kind)?.resolve(url.searchParams.get('zone')!)
      res.end('ok')
    } else {
      res.setHeader('Content-Type', 'text/html; charset=utf-8')
      res.end(`<title>Matou Mantle acceptance</title><script>fetch('/report?kind=${kind}&zone='+encodeURIComponent(Intl.DateTimeFormat().resolvedOptions().timeZone))</script>`)
    }
  })
  await new Promise<void>(yes => server.listen(0, '127.0.0.1', yes))
  const port = (server.address() as { port: number }).port
  const evidence: unknown[] = []
  try {
    expect(safeBrowserStatus().installed).toBe(true)
    for (const [index, kind, timezone] of [[0, 'persistent', 'Asia/Tokyo'], [1, 'ephemeral', 'America/Los_Angeles']] as const) {
      const url = `http://127.0.0.1:${port}/?kind=${kind}`
      const handle = kind === 'ephemeral' ? await controller.openEphemeral(url, timezone) : undefined
      if (kind === 'persistent') await controller.openPersistent(url, timezone)
      let timer: ReturnType<typeof setTimeout> | undefined
      const actualTimezone = await Promise.race([
        reports.get(kind)!.promise,
        new Promise<never>((_yes, no) => { timer = setTimeout(() => no(new Error('Page self-check did not report')), 15000) })
      ]).finally(() => clearTimeout(timer))
      expect(actualTimezone).toBe(timezone)
      const call = vi.mocked(spawn).mock.calls[index]!
      const child = vi.mocked(spawn).mock.results[index]!.value as ReturnType<typeof spawn>
      const args = call[1] as string[]
      const profile = args[0]!.slice('--user-data-dir='.length)
      expect(existsSync(profile)).toBe(true)
      const before = execFileSync('/bin/ps', ['-axo', 'pid=,ppid=,pgid=,command='], { encoding: 'utf8' }).split('\n').filter(line => Number(line.trim().split(/\s+/)[2]) === child.pid)
      expect(before.length).toBeGreaterThan(0)
      const pipe = pipes[index]!
      const { targetInfos } = await pipe.send('Target.getTargets')
      for (const target of targetInfos.filter((info: { type: string }) => info.type === 'page')) {
        await pipe.send('Target.closeTarget', { targetId: target.targetId }).catch(() => {})
      }
      // Do not call closeAll here: this must prove final-page auto cleanup.
      await vi.waitFor(() => {
        try { process.kill(-child.pid!, 0); throw new Error('Browser process group is still alive') }
        catch (error) { if ((error as NodeJS.ErrnoException).code !== 'ESRCH') throw error }
      }, { timeout: 12000, interval: 100 })
      if (handle) await handle.done
      expect(existsSync(profile)).toBe(kind === 'persistent')
      const after = execFileSync('/bin/ps', ['-axo', 'pid=,ppid=,pgid=,command='], { encoding: 'utf8' }).split('\n').filter(line => Number(line.trim().split(/\s+/)[2]) === child.pid)
      expect(after).toEqual([])
      evidence.push({ kind, executable: call[0], args, pid: child.pid, timezone: actualTimezone, profile,
        profileExistedWhileRunning: true, profileExistsAfterExit: existsSync(profile), processesBefore: before, processesAfter: after })
    }
    expect(commands.mock.calls.filter(call => call[0] === 'Browser.close').length).toBeGreaterThanOrEqual(2)
    writeFileSync(join(evidenceRoot, 'real-launch.json'), JSON.stringify({ checkedAt: new Date().toISOString(), userData, status: safeBrowserStatus(), launches: evidence }, null, 2))
    console.log(JSON.stringify({ userData, launches: evidence }, null, 2))
  } finally {
    await controller.closeAll()
    await new Promise<void>((yes, no) => server.close(error => error ? no(error) : yes()))
  }
}, 60000)
