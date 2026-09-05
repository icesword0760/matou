import type { ElectronApplication, Locator, Page } from '@playwright/test'
import { writeFile } from 'node:fs/promises'

export type RecorderEvent =
  | { t: number; type: 'click'; x: number; y: number; label?: string }
  | { t: number; type: 'move'; x: number; y: number; ms: number }
  | { t: number; type: 'source'; source: 'main' | 'dag' }
  | { t: number; type: 'mark'; name: string }

export interface RecorderStats {
  written: number; duplicated: number; dropped: number; resized: number
  width: number; height: number; fps: number; startedAt: number; stoppedAt: number
}

const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms))

export class ClipRecorder {
  private events: RecorderEvent[] = []
  private startedAt = 0
  private scale = 2
  private viewport = { width: 0, height: 0 }
  private pointer = { x: 0, y: 0 }

  constructor(private readonly app: ElectronApplication, private readonly fps = 30) {}

  async start(outputPath: string): Promise<void> {
    const started = await this.app.evaluate(async ({ BrowserWindow, screen }, { outputPath, fps }) => {
      const cp = process.getBuiltinModule('node:child_process') as typeof import('node:child_process')
      type Win = Electron.BrowserWindow
      const isDag = (w: Win) => w.webContents.getURL().includes('kind=dag')
      const state = {
        latest: {} as Record<string, Electron.NativeImage | undefined>,
        active: 'main' as 'main' | 'dag',
        subscribed: new Set<Win>(),
        written: 0, duplicated: 0, dropped: 0, resized: 0,
        lastWritten: undefined as Electron.NativeImage | undefined,
        timer: undefined as NodeJS.Timeout | undefined,
        ff: undefined as ReturnType<typeof cp.spawn> | undefined,
        spawnError: undefined as Error | undefined,
        size: { width: 0, height: 0 },
        startedAt: 0,
        writable: true,
        stopped: false
      }
      // Throws when the requested window doesn't exist yet - no silent fallback to main.
      // The pump's `?? state.latest.main` covers the brief gap after a successful subscribe,
      // before that window's frame subscription has delivered its first frame.
      const subscribe = (which: 'main' | 'dag') => {
        const win = BrowserWindow.getAllWindows().find((w) => (which === 'dag') === isDag(w))
        if (!win) throw new Error(`no ${which} window to subscribe to`)
        if (state.subscribed.has(win)) return
        state.subscribed.add(win)
        win.webContents.beginFrameSubscription(false, (image) => { state.latest[which] = image })
        win.webContents.invalidate()
      }
      subscribe('main')
      await new Promise<void>((resolve) => {
        const check = () => (state.latest.main ? resolve() : setTimeout(check, 16))
        check()
      })
      state.size = state.latest.main!.getSize()
      state.ff = cp.spawn('ffmpeg', [
        '-y', '-loglevel', 'error',
        '-f', 'rawvideo', '-pix_fmt', 'bgra', '-s', `${state.size.width}x${state.size.height}`, '-r', String(fps), '-i', 'pipe:0',
        '-c:v', 'libx264', '-preset', 'veryfast', '-crf', '16', '-pix_fmt', 'yuv420p', '-movflags', '+faststart', outputPath
      ], { stdio: ['pipe', 'ignore', 'inherit'] })
      state.ff.on('error', (e) => { state.spawnError = e })
      state.ff.stdin!.on('error', (e) => { state.spawnError = e })
      state.ff.stdin!.on('drain', () => { if (!state.stopped) state.writable = true })
      const expected = state.size.width * state.size.height * 4
      // Writes exactly one frame slot. Returns false only when it must wait (backpressure, no
      // frame captured yet, or recording has stopped) - the caller should stop and let the next
      // pump tick retry. A bitmap-size mismatch is the only thing that counts as `dropped`; even
      // then we still bump `written` so the wall-clock catch-up loop below can never spin forever
      // on a bad frame. The write itself is guarded because stop() ends stdin from a separate
      // evaluate() call, and a pump already mid-flight when that happens must not throw past
      // Electron's main process - any write failure is captured as spawnError and surfaced
      // through stop() instead.
      const writeOneFrame = (): boolean => {
        if (state.stopped) return false
        const image = state.latest[state.active] ?? state.latest.main
        if (!image) return false
        if (!state.writable) return false
        let frame = image
        if (frame.getSize().width !== state.size.width || frame.getSize().height !== state.size.height) {
          frame = frame.resize({ width: state.size.width, height: state.size.height, quality: 'good' })
          state.resized += 1
        }
        const bitmap = frame.toBitmap()
        if (bitmap.length !== expected) { state.dropped += 1; state.written += 1; return true }
        if (image === state.lastWritten) state.duplicated += 1
        state.lastWritten = image
        if (state.stopped || state.ff!.stdin!.writableEnded) return false
        try {
          state.writable = state.ff!.stdin!.write(bitmap)
        } catch (e) {
          state.spawnError = e instanceof Error ? e : new Error(String(e))
          state.stopped = true
          return false
        }
        state.written += 1
        return true
      }
      // Wall-clock catch-up pump (not a bare setInterval, which drifts under load - an empty
      // setInterval(fn, 1000/30) alone measures under 29fps here). Every half frame period we
      // compute how many frames SHOULD exist by now from real elapsed time and write until we
      // catch up, duplicating the latest frame when nothing repainted. A frame that couldn't be
      // written because of backpressure is never permanently lost - it's caught up on a later
      // pump once 'drain' fires.
      const frameMs = 1000 / fps
      const pump = () => {
        if (state.stopped) return
        const due = Math.floor((Date.now() - state.startedAt) / frameMs) + 1
        while (state.written < due) {
          if (!writeOneFrame()) break
        }
      }
      state.startedAt = Date.now()
      pump()
      state.timer = setInterval(pump, frameMs / 2)
      const win = BrowserWindow.getAllWindows().find((w) => !isDag(w))!
      const scale = screen.getDisplayMatching(win.getBounds()).scaleFactor
      const [width, height] = win.getContentSize()
      ;(globalThis as Record<string, unknown>).__matouRecorder = { state, subscribe }
      return { startedAt: state.startedAt, scale, viewport: { width, height } }
    }, { outputPath, fps: this.fps })
    this.startedAt = started.startedAt
    this.scale = started.scale
    this.viewport = started.viewport
    this.events = []
    this.pointer = { x: 0, y: 0 }
  }

  async stop(eventsPath: string): Promise<RecorderStats> {
    const stats = await this.app.evaluate(async () => {
      const { state } = (globalThis as Record<string, any>).__matouRecorder
      state.stopped = true
      clearInterval(state.timer)
      // Snapshot stoppedAt right when we stop pumping frames, not after ffmpeg's shutdown flush
      // below - that flush's wall-clock duration has nothing to do with the captured frame
      // cadence and would otherwise dilute written/((stoppedAt-startedAt)/1000) below the true
      // effective fps.
      const stoppedAt = Date.now()
      for (const win of state.subscribed) { try { win.webContents.endFrameSubscription() } catch { /* window gone */ } }
      // If ffmpeg already exited (crash/spawn failure), 'close' has already fired and never will
      // again - awaiting it here would hang forever. Only wait when it's still running.
      if (state.ff.exitCode === null) {
        await new Promise<void>((resolve) => { state.ff.once('close', () => resolve()); state.ff.stdin.end() })
      }
      delete (globalThis as Record<string, unknown>).__matouRecorder
      if (state.spawnError) throw state.spawnError
      if (state.ff.exitCode !== null && state.ff.exitCode !== 0) {
        throw new Error(`ffmpeg exited with code ${state.ff.exitCode}`)
      }
      return {
        written: state.written, duplicated: state.duplicated, dropped: state.dropped, resized: state.resized,
        width: state.size.width, height: state.size.height, startedAt: state.startedAt, stoppedAt
      }
    })
    const full: RecorderStats = { ...stats, fps: this.fps }
    await writeFile(eventsPath, JSON.stringify({ ...full, scale: this.scale, viewport: this.viewport, events: this.events }, null, 2))
    return full
  }

  now(): number { return Date.now() - this.startedAt }
  mark(name: string): void { this.events.push({ t: this.now(), type: 'mark', name }) }

  async setSource(source: 'main' | 'dag'): Promise<void> {
    await this.app.evaluate((_electron, source) => {
      const recorder = (globalThis as Record<string, any>).__matouRecorder
      recorder.subscribe(source)
      recorder.state.active = source
    }, source)
    this.events.push({ t: this.now(), type: 'source', source })
  }

  async moveTo(page: Page, x: number, y: number, ms = 350): Promise<void> {
    const t = this.now()
    const steps = Math.max(2, Math.round(ms / 16))
    const from = this.pointer
    const perStep = ms / steps
    for (let i = 1; i <= steps; i++) {
      const xi = from.x + ((x - from.x) * i) / steps
      const yi = from.y + ((y - from.y) * i) / steps
      await page.mouse.move(xi, yi)
      await sleep(perStep)
    }
    this.pointer = { x, y }
    this.events.push({ t, type: 'move', x, y, ms: this.now() - t })
  }

  async click(locator: Locator, label?: string, position?: { x: number; y: number }): Promise<void> {
    await locator.scrollIntoViewIfNeeded()
    const box = await locator.boundingBox()
    if (!box) throw new Error(`no bounding box for ${label ?? locator.toString()}`)
    const x = box.x + (position?.x ?? box.width / 2)
    const y = box.y + (position?.y ?? box.height / 2)
    await this.moveTo(locator.page(), x, y)
    this.events.push({ t: this.now(), type: 'click', x, y, ...(label ? { label } : {}) })
    await locator.page().mouse.click(x, y)
  }

  async waitUntil(ms: number): Promise<void> {
    const remaining = ms - this.now()
    if (remaining > 0) await sleep(remaining)
  }

  async hold(ms: number): Promise<void> { await sleep(ms) }
}
