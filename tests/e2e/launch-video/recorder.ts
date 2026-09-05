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

export class ClipRecorder {
  private events: RecorderEvent[] = []
  private startedAt = 0
  private scale = 2
  private viewport = { width: 0, height: 0 }

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
        size: { width: 0, height: 0 },
        startedAt: 0,
        writable: true
      }
      const subscribe = (which: 'main' | 'dag') => {
        const win = BrowserWindow.getAllWindows().find((w) => (which === 'dag') === isDag(w))
        if (!win || state.subscribed.has(win)) return
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
      state.ff.stdin!.on('drain', () => { state.writable = true })
      const expected = state.size.width * state.size.height * 4
      const tick = () => {
        const image = state.latest[state.active] ?? state.latest.main
        if (!image) return
        if (!state.writable) { state.dropped += 1; return }
        let frame = image
        if (frame.getSize().width !== state.size.width || frame.getSize().height !== state.size.height) {
          frame = frame.resize({ width: state.size.width, height: state.size.height }); state.resized += 1
        }
        const bitmap = frame.toBitmap()
        if (bitmap.length !== expected) { state.dropped += 1; return }
        if (image === state.lastWritten) state.duplicated += 1
        state.lastWritten = image
        state.writable = state.ff!.stdin!.write(bitmap)
        state.written += 1
      }
      state.startedAt = Date.now()
      tick()
      // The very first write() almost always reports backpressure immediately: ffmpeg's process
      // startup (spawn + libx264 init) is slower than one frame interval, so it hasn't started
      // reading stdin yet. Wait for that initial backpressure to clear before starting the
      // periodic clock, otherwise the first scheduled tick races the first 'drain' and is
      // guaranteed to be dropped. t=0 (startedAt) still marks the first frame's write, unchanged.
      const startTicking = () => { state.timer = setInterval(tick, 1000 / fps) }
      if (state.writable) startTicking()
      else state.ff.stdin!.once('drain', startTicking)
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
  }

  async stop(eventsPath: string): Promise<RecorderStats> {
    const stats = await this.app.evaluate(async () => {
      const { state } = (globalThis as Record<string, any>).__matouRecorder
      clearInterval(state.timer)
      for (const win of state.subscribed) { try { win.webContents.endFrameSubscription() } catch { /* window gone */ } }
      await new Promise<void>((resolve) => { state.ff.once('close', () => resolve()); state.ff.stdin.end() })
      const stoppedAt = Date.now()
      delete (globalThis as Record<string, unknown>).__matouRecorder
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
    this.events.push({ t: this.now(), type: 'move', x, y, ms })
    await page.mouse.move(x, y, { steps: Math.max(2, Math.round(ms / 16)) })
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
    if (remaining > 0) await new Promise((resolve) => setTimeout(resolve, remaining))
  }

  async hold(ms: number): Promise<void> { await new Promise((resolve) => setTimeout(resolve, ms)) }
}
