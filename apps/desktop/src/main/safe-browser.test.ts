import { EventEmitter } from 'node:events'
import { accessSync, chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, symlinkSync, writeFileSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { PassThrough, Writable } from 'node:stream'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { app } from 'electron'
import { spawn, execFileSync } from 'node:child_process'
import { SafeBrowserController, browserTimeZone, safeBrowserStatus, reportSafeBrowserCountry, safeBrowserTimeZone, safeBrowserAccountEnv } from './safe-browser'
import { BrowserPipe } from './safe-browser-pipe'

vi.mock('electron', async () => {
  const { EventEmitter } = await import('node:events')
  return { app: Object.assign(new EventEmitter(), { getPath: vi.fn(), quit: vi.fn() }) }
})
vi.mock('node:child_process', () => ({ spawn: vi.fn(), execFile: vi.fn(), execFileSync: vi.fn() }))
vi.mock('node:fs', async importOriginal => {
  const actual = await importOriginal<typeof import('node:fs')>()
  return { ...actual, statSync: vi.fn(actual.statSync), accessSync: vi.fn(actual.accessSync) }
})

type Command = { id: number; method: string; params: Record<string, unknown> }
class Kernel extends EventEmitter {
  pid = 80000 + kernels.length
  input = new PassThrough()
  commands: Command[] = []
  pages = new Set(['initial'])
  autoClose = true
  ignoreTerm = false
  alive = true
  stdio = [null, null, null, new Writable({ write: (chunk, _encoding, callback) => {
    const message = JSON.parse(chunk.toString().replace(/\0$/, '')) as Command
    this.commands.push(message)
    queueMicrotask(() => {
      if (!this.alive) return
      let result: Record<string, unknown> = {}
      if (message.method === 'Target.setDiscoverTargets') this.event('Target.targetCreated', { targetInfo: { type: 'page', targetId: 'initial' } })
      if (message.method === 'Target.getTargets') result = { targetInfos: [...this.pages].map(targetId => ({ type: 'page', targetId })) }
      if (message.method === 'Target.createTarget') {
        const targetId = `page-${this.commands.length}`
        this.pages.add(targetId)
        result = { targetId }
        this.event('Target.targetCreated', { targetInfo: { type: 'page', targetId } })
      }
      this.input.write(`${JSON.stringify({ id: message.id, result })}\0`)
      if (message.method === 'Browser.close' && this.autoClose) this.exit()
    })
    callback()
  } }), this.input]
  event(method: string, params: Record<string, unknown>) { this.input.write(`${JSON.stringify({ method, params })}\0`) }
  closePages() {
    for (const targetId of this.pages) this.event('Target.targetDestroyed', { targetId })
    this.pages.clear()
  }
  exit() { this.alive = false; this.emit('exit', 0, null) }
}
let root: string
let executable: string
let kernels: Kernel[]
let kill: ReturnType<typeof vi.spyOn>
const controllers: SafeBrowserController[] = []
const controller = () => { const value = new SafeBrowserController(root); controllers.push(value); return value }
const flush = async () => { for (let i = 0; i < 30; i++) await Promise.resolve() }
const mode = (path: string) => statSync(path).mode & 0o777
const argsFor = (index = 0) => vi.mocked(spawn).mock.calls[index]![1] as string[]
const profileFor = (index = 0) => argsFor(index)[0]!.slice('--user-data-dir='.length)

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'Date'] })
  const base = resolve(process.cwd(), '../../.cache/safe-browser-tests')
  mkdirSync(base, { recursive: true })
  root = mkdtempSync(join(base, 'case-'))
  executable = join(root, 'Chromium')
  writeFileSync(executable, 'test kernel', { mode: 0o700 })
  vi.stubEnv('MATOU_SAFE_BROWSER_PATH', executable)
  vi.mocked(execFileSync).mockReset().mockReturnValue('144.0.1\n')
  vi.mocked(app.getPath).mockReturnValue(root)
  vi.mocked(app.quit).mockReset()
  kernels = []
  vi.mocked(spawn).mockReset().mockImplementation(() => {
    const child = new Kernel()
    kernels.push(child)
    queueMicrotask(() => child.emit('spawn'))
    return child as unknown as ReturnType<typeof spawn>
  })
  kill = vi.spyOn(process, 'kill').mockImplementation((pid, signal) => {
    const kernel = kernels.find(value => value.pid === -Number(pid))
    if (!kernel?.alive) throw Object.assign(new Error('gone'), { code: 'ESRCH' })
    if (signal === 'SIGKILL' || (signal === 'SIGTERM' && !kernel.ignoreTerm)) queueMicrotask(() => kernel.exit())
    return true
  })
})
afterEach(async () => {
  for (const child of kernels) if (child.alive) child.exit()
  await Promise.allSettled(controllers.splice(0).map(value => value.closeAll()))
  app.removeAllListeners('will-quit')
  kill.mockRestore()
  vi.unstubAllEnvs()
  vi.useRealTimers()
  rmSync(root, { recursive: true, force: true })
})

describe('browserTimeZone', () => {
  it.each([
    ['US', 'America/Los_Angeles'], ['JP', 'Asia/Tokyo'], ['SG', 'Asia/Singapore'],
    ['TW', 'Asia/Taipei'], ['HK', 'Asia/Hong_Kong'], ['GB', 'Europe/London'],
    ['DE', 'Europe/Berlin'], ['XX', 'Asia/Singapore'], [undefined, 'Asia/Singapore'],
    ['us', 'America/Los_Angeles']
  ])('maps %s to %s', (country, timezone) => { expect(browserTimeZone(country)).toBe(timezone) })
})

describe('safeBrowserStatus', () => {
  it('detects an executable and reads the bundle short version without starting it', () => {
    expect(safeBrowserStatus()).toEqual({ installed: true, version: '144.0.1' })
    expect(execFileSync).toHaveBeenCalledWith('/usr/bin/plutil', [
      '-extract', 'CFBundleShortVersionString', 'raw', '-o', '-', join(root, '..', 'Info.plist')
    ], expect.any(Object))
    expect(spawn).not.toHaveBeenCalled()
  })
  it('uses the fixed installed application path when there is no override', () => {
    vi.stubEnv('MATOU_SAFE_BROWSER_PATH', '')
    const stat = statSync(executable)
    vi.mocked(statSync).mockReturnValueOnce(stat)
    vi.mocked(accessSync).mockImplementationOnce(() => {})
    expect(safeBrowserStatus()).toEqual({ installed: true, version: '144.0.1' })
    expect(statSync).toHaveBeenLastCalledWith('/Applications/安全浏览器.app/Contents/MacOS/Chromium')
    expect(vi.mocked(execFileSync).mock.calls[0]![1]).toContain('/Applications/安全浏览器.app/Contents/Info.plist')
    expect(spawn).not.toHaveBeenCalled()
  })
  it('reports missing, non-executable and directory paths as not installed', () => {
    rmSync(executable)
    expect(safeBrowserStatus()).toEqual({ installed: false, version: null })
    writeFileSync(executable, 'not executable', { mode: 0o600 })
    expect(safeBrowserStatus().installed).toBe(false)
    vi.stubEnv('MATOU_SAFE_BROWSER_PATH', root)
    expect(safeBrowserStatus().installed).toBe(false)
  })
  it('keeps an executable usable when its plist is missing or invalid', () => {
    vi.mocked(execFileSync).mockImplementation(() => { throw new Error('bad plist') })
    expect(safeBrowserStatus()).toEqual({ installed: true, version: null })
  })
})

describe('sign-in browser context', () => {
  it('uses Singapore until a country arrives, retains the last valid country and maps unknown countries', async () => {
    vi.resetModules()
    const context = await import('./safe-browser')
    expect(context.safeBrowserTimeZone()).toBe('Asia/Singapore')
    context.reportSafeBrowserCountry(' jp ')
    expect(context.safeBrowserTimeZone()).toBe('Asia/Tokyo')
    for (const value of [undefined, null, '', {}, 'Asia/Tokyo', 'JPN']) context.reportSafeBrowserCountry(value)
    expect(context.safeBrowserTimeZone()).toBe('Asia/Tokyo')
    context.reportSafeBrowserCountry('XX')
    expect(context.safeBrowserTimeZone()).toBe('Asia/Singapore')
  })
  it('shares the persisted seed with account login and reads current country on each env call', async () => {
    const value = controller()
    const seed = value.getFingerprintSeed()
    reportSafeBrowserCountry('US')
    expect(safeBrowserAccountEnv(value, { PATH: '/fixture/bin' })).toEqual({
      PATH: '/fixture/bin', MATOU_SAFE_BROWSER_BIN: executable,
      MATOU_SAFE_BROWSER_TZ: 'America/Los_Angeles', MATOU_SAFE_BROWSER_SEED: String(seed)
    })
    reportSafeBrowserCountry('JP')
    expect(safeBrowserAccountEnv(value).MATOU_SAFE_BROWSER_TZ).toBe('Asia/Tokyo')
    await value.openPersistent('about:blank', safeBrowserTimeZone())
    expect(argsFor()).toContain(`--fingerprint=${seed}`)
    expect(value.getFingerprintSeed()).toBe(seed)
  })
  it('omits all safe-browser env when missing, including inherited values, without creating a seed', () => {
    rmSync(executable)
    expect(safeBrowserAccountEnv(controller(), {
      PATH: '/fixture/bin', MATOU_SAFE_BROWSER_BIN: '/stale/kernel', MATOU_SAFE_BROWSER_TZ: 'Asia/Tokyo', MATOU_SAFE_BROWSER_SEED: '42'
    })).toEqual({ PATH: '/fixture/bin' })
    expect(existsSync(join(root, 'safe-browser.json'))).toBe(false)
  })
})

describe('SafeBrowserController', () => {
  it('builds exactly the approved arguments and uses a private detached pipe', async () => {
    await controller().openPersistent('https://example.invalid/login', 'Asia/Tokyo')
    const seed = JSON.parse(readFileSync(join(root, 'safe-browser.json'), 'utf8')).fingerprintSeed
    expect(argsFor()).toEqual([
      `--user-data-dir=${join(root, 'safe-browser/profile')}`, '--timezone=Asia/Tokyo',
      '--accept-lang=zh-CN,zh', `--fingerprint=${seed}`, '--fingerprint-platform=macos',
      '--no-first-run', '--no-default-browser-check', '--remote-debugging-pipe', 'https://example.invalid/login'
    ])
    expect(vi.mocked(spawn).mock.calls[0]![2]).toEqual({ detached: true, stdio: ['ignore', 'ignore', 'ignore', 'pipe', 'pipe'] })
    expect(seed).toBeGreaterThanOrEqual(1)
    expect(seed).toBeLessThanOrEqual(2147483647)
    expect(Number.isInteger(seed)).toBe(true)
  })
  it('defaults to app userData while allowing an isolated constructor root', async () => {
    const value = new SafeBrowserController()
    controllers.push(value)
    await value.openPersistent('about:blank', 'Asia/Singapore')
    expect(app.getPath).toHaveBeenCalledWith('userData')
    expect(profileFor()).toBe(join(root, 'safe-browser/profile'))
  })
  it('persists one seed across controller instances and repairs private permissions', async () => {
    const first = controller()
    await first.openPersistent('about:blank', 'Asia/Tokyo')
    const seedFile = join(root, 'safe-browser.json')
    const stored = readFileSync(seedFile, 'utf8')
    await first.closeAll()
    chmodSync(seedFile, 0o644)
    chmodSync(join(root, 'safe-browser/profile'), 0o755)
    await controller().openPersistent('about:blank', 'Asia/Tokyo')
    expect(readFileSync(seedFile, 'utf8')).toBe(stored)
    expect(argsFor(1)[3]).toBe(argsFor()[3])
    expect(mode(seedFile)).toBe(0o600)
    expect(mode(join(root, 'safe-browser'))).toBe(0o700)
    expect(mode(profileFor())).toBe(0o700)
  })
  it.each(['safe-browser', 'safe-browser/profile', 'safe-browser/tmp', 'safe-browser.json'])('rejects a symlink at %s before spawning', async link => {
    const target = join(root, 'outside')
    mkdirSync(target)
    if (link.includes('/')) mkdirSync(join(root, 'safe-browser'))
    symlinkSync(target, join(root, link))
    const value = controller()
    await expect(link.endsWith('tmp') ? value.openEphemeral('about:blank', 'Asia/Tokyo') : value.openPersistent('about:blank', 'Asia/Tokyo')).rejects.toThrow()
    expect(spawn).not.toHaveBeenCalled()
    expect(readdirSync(target)).toEqual([])
  })
  it('rejects invalid persisted seeds instead of changing the fingerprint', async () => {
    writeFileSync(join(root, 'safe-browser.json'), '{"fingerprintSeed":0}')
    await expect(controller().openPersistent('about:blank', 'Asia/Tokyo')).rejects.toThrow('Invalid browser fingerprint seed')
    expect(spawn).not.toHaveBeenCalled()
  })
  it('serializes concurrent persistent opens into one process and opens extra pages by pipe', async () => {
    const value = controller()
    await Promise.all([value.openPersistent('https://one.invalid', 'Asia/Tokyo'), value.openPersistent('https://two.invalid', 'Asia/Tokyo')])
    expect(spawn).toHaveBeenCalledTimes(1)
    expect(kernels[0]!.commands).toContainEqual(expect.objectContaining({ method: 'Target.createTarget', params: { url: 'https://two.invalid' } }))
    expect(kernels[0]!.commands.at(-1)!.method).toBe('Target.activateTarget')
  })
  it('opens a new persistent process after the previous one exits', async () => {
    const value = controller()
    await value.openPersistent('about:blank', 'Asia/Tokyo')
    kernels[0]!.exit()
    await flush()
    await value.openPersistent('about:blank', 'Asia/Tokyo')
    expect(spawn).toHaveBeenCalledTimes(2)
    expect(profileFor(1)).toBe(profileFor())
  })
  it('sends Browser.close only when the last page is destroyed', async () => {
    const value = controller()
    await value.openPersistent('about:blank', 'Asia/Tokyo')
    await value.openPersistent('about:blank', 'Asia/Tokyo')
    const kernel = kernels[0]!
    kernel.event('Target.targetDestroyed', { targetId: 'initial' })
    expect(kernel.commands.some(command => command.method === 'Browser.close')).toBe(false)
    kernel.closePages()
    await flush()
    expect(kernel.commands.filter(command => command.method === 'Browser.close')).toHaveLength(1)
    expect(kernel.alive).toBe(false)
  })
  it('waits two seconds then terminates a windowless process group', async () => {
    await controller().openPersistent('about:blank', 'Asia/Tokyo')
    const kernel = kernels[0]!
    kernel.autoClose = false
    kernel.closePages()
    await vi.advanceTimersByTimeAsync(1999)
    expect(kill.mock.calls.some((call: unknown[]) => call[1] === 'SIGTERM')).toBe(false)
    await vi.advanceTimersByTimeAsync(51)
    expect(kill).toHaveBeenCalledWith(-kernel.pid, 'SIGTERM')
    expect(kernel.alive).toBe(false)
  })
  it('keeps the two-second fallback alive when Chromium closes its pipe', async () => {
    await controller().openPersistent('about:blank', 'Asia/Tokyo')
    const kernel = kernels[0]!
    kernel.autoClose = false
    kernel.closePages()
    kernel.input.emit('end')
    await vi.advanceTimersByTimeAsync(2050)
    expect(kernel.alive).toBe(false)
  })
  it('does not terminate a process if a new page appeared during the fallback', async () => {
    await controller().openPersistent('about:blank', 'Asia/Tokyo')
    const kernel = kernels[0]!
    kernel.autoClose = false
    kernel.closePages()
    kernel.event('Target.targetCreated', { targetInfo: { type: 'page', targetId: 'new' } })
    await vi.advanceTimersByTimeAsync(4000)
    expect(kill.mock.calls.some((call: unknown[]) => call[1] === 'SIGTERM')).toBe(false)
    expect(kernel.alive).toBe(true)
  })
  it('escalates from SIGTERM to SIGKILL after a further two seconds', async () => {
    const handle = await controller().openEphemeral('about:blank', 'Asia/Tokyo')
    const kernel = kernels[0]!
    kernel.autoClose = false
    kernel.ignoreTerm = true
    const closing = handle.close()
    await vi.advanceTimersByTimeAsync(3999)
    expect(kill).toHaveBeenCalledWith(-kernel.pid, 'SIGTERM')
    expect(kill.mock.calls.some((call: unknown[]) => call[1] === 'SIGKILL')).toBe(false)
    await vi.advanceTimersByTimeAsync(101)
    await closing
    expect(kill).toHaveBeenCalledWith(-kernel.pid, 'SIGKILL')
  })
  it('keeps explicit close authoritative and idempotent across late target events', async () => {
    const handle = await controller().openEphemeral('about:blank', 'Asia/Tokyo')
    const kernel = kernels[0]!
    kernel.autoClose = false
    const closing = handle.close()
    const again = handle.close()
    expect(kernel.commands.filter(command => command.method === 'Browser.close')).toHaveLength(1)
    kernel.closePages()
    kernel.event('Target.targetCreated', { targetInfo: { type: 'page', targetId: 'late' } })
    await vi.advanceTimersByTimeAsync(2050)
    await Promise.all([closing, again])
    expect(kernel.alive).toBe(false)
  })
  it('gives every ephemeral launch a private directory and deletes it only after exit', async () => {
    const value = controller()
    const one = await value.openEphemeral('about:blank', 'Asia/Tokyo')
    const two = await value.openEphemeral('about:blank', 'Asia/Tokyo')
    const first = profileFor(), second = profileFor(1)
    expect(first).not.toBe(second)
    expect(mode(first)).toBe(0o700)
    expect(mode(join(root, 'safe-browser/tmp'))).toBe(0o700)
    let done = false
    void one.done.then(() => { done = true })
    kernels[0]!.autoClose = false
    const closing = one.close()
    await flush()
    expect(done).toBe(false)
    expect(existsSync(first)).toBe(true)
    kernels[0]!.exit()
    await closing
    expect(done).toBe(true)
    expect(existsSync(first)).toBe(false)
    expect(existsSync(second)).toBe(true)
    await two.close()
    expect(existsSync(second)).toBe(false)
  })
  it('cleans surviving helpers before deleting an exited parent profile', async () => {
    const handle = await controller().openEphemeral('about:blank', 'Asia/Tokyo')
    const kernel = kernels[0]!
    kernel.emit('exit', 0, null)
    expect(existsSync(profileFor())).toBe(true)
    await vi.advanceTimersByTimeAsync(50)
    await handle.done
    expect(kill).toHaveBeenCalledWith(-kernel.pid, 'SIGTERM')
    expect(existsSync(profileFor())).toBe(false)
  })
  it('cleans a failed spawn and allows a subsequent attempt', async () => {
    vi.mocked(spawn).mockImplementationOnce(() => {
      const child = new Kernel()
      Object.assign(child, { pid: undefined })
      kernels.push(child)
      queueMicrotask(() => child.emit('error', new Error('spawn failed')))
      return child as unknown as ReturnType<typeof spawn>
    })
    const value = controller()
    await expect(value.openEphemeral('about:blank', 'Asia/Tokyo')).rejects.toThrow('spawn failed')
    expect(readdirSync(join(root, 'safe-browser/tmp'))).toEqual([])
    await value.openEphemeral('about:blank', 'Asia/Tokyo')
  })
  it('returns a stable missing error code without creating browser data', async () => {
    rmSync(executable)
    const value = controller()
    await expect(value.openPersistent('about:blank', 'Asia/Tokyo')).rejects.toMatchObject({ code: 'safe-browser-missing' })
    await expect(value.openEphemeral('about:blank', 'Asia/Tokyo')).rejects.toMatchObject({ code: 'safe-browser-missing' })
    expect(existsSync(join(root, 'safe-browser'))).toBe(false)
    expect(existsSync(join(root, 'safe-browser.json'))).toBe(false)
  })
  it('closes all processes on app quit and preserves the persistent profile', async () => {
    const value = controller()
    await value.openPersistent('about:blank', 'Asia/Tokyo')
    const handle = await value.openEphemeral('about:blank', 'Asia/Tokyo')
    const event = { preventDefault: vi.fn() }
    app.emit('will-quit', event)
    await handle.done
    await flush()
    expect(event.preventDefault).toHaveBeenCalled()
    expect(app.quit).toHaveBeenCalledTimes(1)
    expect(kernels.every(kernel => !kernel.alive)).toBe(true)
    expect(existsSync(profileFor())).toBe(true)
    expect(existsSync(profileFor(1))).toBe(false)
    await expect(value.openPersistent('about:blank', 'Asia/Tokyo')).rejects.toThrow('closing')
  })
  it('closes an in-flight launch without allowing another process to start', async () => {
    const value = controller()
    const opening = value.openEphemeral('about:blank', 'Asia/Tokyo')
    const rejected = expect(opening).rejects.toThrow()
    await value.closeAll()
    await rejected
    expect(kernels.every(kernel => !kernel.alive)).toBe(true)
    expect(readdirSync(join(root, 'safe-browser/tmp'))).toEqual([])
  })
})

describe('BrowserPipe transport', () => {
  it('handles fragmented messages and rejects pending commands when the pipe ends', async () => {
    const input = new PassThrough(), output = new PassThrough()
    const pipe = new BrowserPipe(input, output)
    const command = pipe.send('Target.getTargets')
    input.write('{"id":1,"result":')
    input.write('{"targetInfos":[]}}\0')
    await expect(command).resolves.toEqual({ targetInfos: [] })
    const pending = pipe.send('Browser.close')
    input.emit('end')
    await expect(pending).rejects.toThrow('Browser pipe closed')
  })
})
