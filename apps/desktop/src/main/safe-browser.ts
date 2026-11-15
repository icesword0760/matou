import { spawn, execFileSync, type ChildProcess } from 'node:child_process'
import { randomInt } from 'node:crypto'
import {
  accessSync, constants, closeSync, fchmodSync, fstatSync, lstatSync, mkdirSync,
  mkdtempSync, openSync, readFileSync, rmSync, statSync, writeFileSync, chmodSync
} from 'node:fs'
import { dirname, join } from 'node:path'
import type { Readable, Writable } from 'node:stream'
import { app } from 'electron'

import { BrowserPipe } from './safe-browser-pipe'

export interface SafeBrowserStatus { installed: boolean; version: string | null }

const installedExecutable = '/Applications/安全浏览器.app/Contents/MacOS/Chromium'
const executablePath = () => process.env.MATOU_SAFE_BROWSER_PATH || installedExecutable

export function safeBrowserStatus(): SafeBrowserStatus {
  const executable = executablePath()
  try {
    if (!statSync(executable).isFile()) return { installed: false, version: null }
    accessSync(executable, constants.X_OK)
  } catch { return { installed: false, version: null } }
  let version: string | null = null
  try {
    version = execFileSync('/usr/bin/plutil', [
      '-extract', 'CFBundleShortVersionString', 'raw', '-o', '-',
      join(dirname(dirname(executable)), 'Info.plist')
    ], { encoding: 'utf8', timeout: 2000, maxBuffer: 4096, stdio: ['ignore', 'pipe', 'ignore'] }).trim() || null
  } catch { /* An executable without bundle metadata remains usable for tests. */ }
  return { installed: true, version }
}

export function browserTimeZone(country?: string): string {
  const zones: Record<string, string> = {
    US: 'America/Los_Angeles', JP: 'Asia/Tokyo', SG: 'Asia/Singapore',
    TW: 'Asia/Taipei', HK: 'Asia/Hong_Kong', GB: 'Europe/London', DE: 'Europe/Berlin'
  }
  return zones[country?.toUpperCase() ?? ''] ?? 'Asia/Singapore'
}

function privateDirectory(path: string, repairMode = true): void {
  try { mkdirSync(path, { mode: 0o700 }) } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error
  }
  const stat = lstatSync(path)
  if (stat.isSymbolicLink() || !stat.isDirectory()) throw new Error('Unsafe browser profile directory')
  if (repairMode) chmodSync(path, 0o700)
}

function fingerprintSeed(userData: string): number {
  const file = join(userData, 'safe-browser.json')
  let fd: number
  try {
    fd = openSync(file, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW, 0o600)
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== 'EEXIST') throw error
    fd = openSync(file, constants.O_RDONLY | constants.O_NOFOLLOW)
    try {
      if (!fstatSync(fd).isFile()) throw new Error('Unsafe browser seed file')
      const parsed: unknown = JSON.parse(readFileSync(fd, 'utf8'))
      const seed = (parsed as { fingerprintSeed?: unknown } | null)?.fingerprintSeed
      if (typeof seed !== 'number' || !Number.isInteger(seed) || seed < 1 || seed > 2147483647) {
        throw new Error('Invalid browser fingerprint seed')
      }
      fchmodSync(fd, 0o600)
      return seed
    } finally { closeSync(fd) }
  }
  try {
    const seed = randomInt(1, 2147483648)
    fchmodSync(fd, 0o600)
    writeFileSync(fd, JSON.stringify({ fingerprintSeed: seed }))
    return seed
  } finally { closeSync(fd) }
}

const delay = (ms: number) => new Promise<void>(resolve => setTimeout(resolve, ms))
function groupAlive(pid: number): boolean {
  try { process.kill(-pid, 0); return true } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ESRCH') return false
    if ((error as NodeJS.ErrnoException).code === 'EPERM') return true
    throw error
  }
}
function signalGroup(pid: number, signal: NodeJS.Signals): void {
  try { process.kill(-pid, signal) } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== 'ESRCH') throw error
  }
}
async function stopProcessGroup(pid: number): Promise<void> {
  if (!groupAlive(pid)) return
  signalGroup(pid, 'SIGTERM')
  const deadline = Date.now() + 2000
  while (groupAlive(pid) && Date.now() < deadline) await delay(50)
  if (groupAlive(pid)) signalGroup(pid, 'SIGKILL')
  const killDeadline = Date.now() + 2000
  while (groupAlive(pid) && Date.now() < killDeadline) await delay(50)
  if (groupAlive(pid)) throw new Error('Browser process group did not exit')
}

interface BrowserSession {
  child: ChildProcess
  pipe: BrowserPipe
  exited: boolean
  closing: boolean
  done: Promise<void>
  reject(error: unknown): void
  fallback?: ReturnType<typeof setTimeout> | undefined
}

export class SafeBrowserController {
  private readonly sessions = new Set<BrowserSession>()
  private persistent: BrowserSession | undefined
  private persistentQueue: Promise<void> = Promise.resolve()
  private stopping: Promise<void> | undefined
  private quitting = false
  private quitComplete = false

  // Callers/tests can supply an isolated root without changing Electron's paths.
  constructor(private readonly userData: string = app.getPath('userData')) {
    app.on('will-quit', this.onQuit)
  }

  private onQuit = (event: Electron.Event): void => {
    if (this.quitComplete) return
    this.quitting = true
    if (!this.sessions.size) { this.quitComplete = true; return }
    event.preventDefault()
    void this.closeAll().then(() => {
      this.quitComplete = true
      app.removeListener('will-quit', this.onQuit)
      app.quit()
    }).catch(error => { console.error('Safe browser cleanup failed', error) })
  }

  openPersistent(url: string, timezone: string): Promise<void> {
    if (this.quitting || this.stopping) return Promise.reject(new Error('Safe browser is closing'))
    const opening = this.persistentQueue.then(async () => {
      if (this.quitting || this.stopping) throw new Error('Safe browser is closing')
      if (this.persistent && !this.persistent.exited) {
        await this.persistent.pipe.open(url)
      } else {
        this.persistent = await this.launch(url, timezone, false)
      }
    })
    this.persistentQueue = opening.catch(() => {})
    return opening
  }

  async openEphemeral(url: string, timezone: string): Promise<{ done: Promise<void>; close(): Promise<void> }> {
    const session = await this.launch(url, timezone, true)
    return { done: session.done, close: () => this.closeSession(session) }
  }

  private async launch(url: string, timezone: string, ephemeral: boolean): Promise<BrowserSession> {
    if (this.quitting || this.stopping) throw new Error('Safe browser is closing')
    if (!safeBrowserStatus().installed) {
      throw Object.assign(new Error('Safe browser is not installed'), { code: 'safe-browser-missing' })
    }
    privateDirectory(this.userData, false)
    const root = join(this.userData, 'safe-browser')
    privateDirectory(root)
    const seed = fingerprintSeed(this.userData)
    let profile = join(root, 'profile')
    if (ephemeral) {
      const tmp = join(root, 'tmp')
      privateDirectory(tmp)
      profile = mkdtempSync(join(tmp, 'profile-'))
    }
    privateDirectory(profile)
    let child: ChildProcess
    try {
      child = spawn(executablePath(), [
        `--user-data-dir=${profile}`,
        `--timezone=${timezone}`,
        '--accept-lang=zh-CN,zh',
        `--fingerprint=${seed}`,
        '--fingerprint-platform=macos',
        '--no-first-run',
        '--no-default-browser-check',
        '--remote-debugging-pipe',
        url
      ], { detached: true, stdio: ['ignore', 'ignore', 'ignore', 'pipe', 'pipe'] })
    } catch (error) {
      // No child was created, so this directory cannot be in use.
      if (ephemeral) rmSync(profile, { recursive: true, force: true })
      throw error
    }
    let resolve!: () => void
    let reject!: (error: unknown) => void
    const done = new Promise<void>((yes, no) => { resolve = yes; reject = no })
    // A startup failure can occur before the handle reaches its caller.
    void done.catch(() => {})
    const pipe = new BrowserPipe(child.stdio[4] as Readable, child.stdio[3] as Writable, () => {
      this.scheduleFallback(session, false)
    })
    const session: BrowserSession = { child, pipe, done, reject, exited: false, closing: false }
    this.sessions.add(session)
    const finish = () => {
      if (session.exited) return
      session.exited = true
      clearTimeout(session.fallback)
      pipe.dispose()
      if (this.persistent === session) this.persistent = undefined
      void (async () => {
        // The parent's exit is insufficient: helpers may still use the profile.
        if (child.pid) await stopProcessGroup(child.pid)
        if (ephemeral) rmSync(profile, { recursive: true, force: true })
        this.sessions.delete(session)
        resolve()
      })().catch(reject)
    }
    child.once('exit', finish)
    // A failed spawn has no process and emits no exit event.
    child.once('error', () => { if (!child.pid) finish() })
    try {
      await new Promise<void>((yes, no) => {
        child.once('spawn', yes)
        child.once('error', no)
      })
      await pipe.ready()
      if (session.exited) throw new Error('Safe browser exited during startup')
      return session
    } catch (error) {
      await this.closeSession(session)
      throw error
    }
  }

  private scheduleFallback(session: BrowserSession, force: boolean): void {
    session.closing ||= force
    clearTimeout(session.fallback)
    session.fallback = setTimeout(() => {
      if (!session.exited && (session.closing || !session.pipe.hasPages()) && session.child.pid) {
        void stopProcessGroup(session.child.pid).catch(session.reject)
      }
    }, 2000)
  }

  private closeSession(session: BrowserSession): Promise<void> {
    if (!session.exited && !session.closing) {
      void session.pipe.send('Browser.close').catch(() => {})
      this.scheduleFallback(session, true)
    }
    return session.done
  }

  closeAll(): Promise<void> {
    if (!this.stopping) {
      this.stopping = Promise.all([...this.sessions].map(session => this.closeSession(session)))
        .then(() => this.persistentQueue)
        .finally(() => { this.stopping = undefined })
    }
    return this.stopping
  }
}
