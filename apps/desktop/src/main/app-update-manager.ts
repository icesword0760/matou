export interface AppUpdaterAdapter {
  autoDownload: boolean
  autoInstallOnAppQuit: boolean
  channel?: string
  on(event: string, listener: (...args: any[]) => void): unknown
  checkForUpdates(): Promise<unknown>
  downloadUpdate(): Promise<unknown>
  quitAndInstall(isSilent?: boolean, isForceRunAfter?: boolean): void
  setFeedURL?(options: { provider: 'generic'; url: string; channel?: string }): void
}

export interface AppUpdateProgress {
  percent: number
  transferredBytes: number
  totalBytes: number
  bytesPerSecond: number
  remainingSeconds?: number
}

export interface AppUpdateReleaseState {
  currentVersion: string
  version: string
  releaseDate?: string
  releaseNotes: string[]
  sizeBytes?: number
}

export type AppUpdateState =
  | { status: 'idle' | 'checking' | 'not-available'; currentVersion: string }
  | ({ status: 'available' | 'downloaded' } & AppUpdateReleaseState)
  | ({ status: 'downloading'; progress: AppUpdateProgress } & AppUpdateReleaseState)
  | { status: 'error'; currentVersion: string; errorMessage: string }

export interface AppUpdateManagerOptions {
  updater: AppUpdaterAdapter
  enabled: boolean
  currentVersion: string
  initialDelayMs?: number
  intervalMs?: number
  publish?: (state: AppUpdateState) => void
  prepareInstall?: () => Promise<void>
}

export class AppUpdateManager {
  private current: AppUpdateState
  private initialTimer?: ReturnType<typeof setTimeout>
  private intervalTimer?: ReturnType<typeof setInterval>
  private installPromise?: Promise<void>
  private started = false

  constructor(private readonly options: AppUpdateManagerOptions) {
    this.current = { status: 'idle', currentVersion: options.currentVersion }
  }

  start(): void {
    if (this.started) return
    this.started = true
    if (!this.options.enabled) return
    const { updater } = this.options
    updater.autoDownload = false
    updater.autoInstallOnAppQuit = true
    this.bindUpdaterEvents()
    const initialDelay = this.options.initialDelayMs ?? 15_000
    const interval = this.options.intervalMs ?? 4 * 60 * 60 * 1_000
    this.initialTimer = setTimeout(() => {
      void this.check()
      this.intervalTimer = setInterval(() => { void this.check() }, interval)
    }, initialDelay)
  }

  dispose(): void {
    if (this.initialTimer) clearTimeout(this.initialTimer)
    if (this.intervalTimer) clearInterval(this.intervalTimer)
    this.initialTimer = undefined
    this.intervalTimer = undefined
  }

  state(): AppUpdateState {
    return this.current.status === 'downloading'
      ? { ...this.current, progress: { ...this.current.progress } }
      : { ...this.current }
  }

  async check(): Promise<void> {
    if (!this.options.enabled) return
    this.update({ status: 'checking', currentVersion: this.options.currentVersion })
    try {
      await this.options.updater.checkForUpdates()
    } catch (error) {
      this.onError(error)
    }
  }

  async download(): Promise<void> {
    if (!this.options.enabled || this.current.status !== 'available') return
    try {
      await this.options.updater.downloadUpdate()
    } catch (error) {
      this.onError(error)
    }
  }

  async install(): Promise<void> {
    if (!this.options.enabled || this.current.status !== 'downloaded') return
    if (!this.installPromise) {
      this.installPromise = (async () => {
        await this.options.prepareInstall?.()
        this.options.updater.quitAndInstall(false, true)
      })()
    }
    await this.installPromise
  }

  private bindUpdaterEvents(): void {
    const { updater } = this.options
    updater.on('checking-for-update', () => {
      this.update({ status: 'checking', currentVersion: this.options.currentVersion })
    })
    updater.on('update-not-available', () => {
      this.update({ status: 'not-available', currentVersion: this.options.currentVersion })
    })
    updater.on('update-available', (info: UpdateInfoLike) => {
      this.update({ status: 'available', ...this.releaseState(info) })
    })
    updater.on('download-progress', (progress: DownloadProgressLike) => {
      const release = isReleaseState(this.current)
        ? releaseFromState(this.current)
        : this.releaseState({ version: this.options.currentVersion })
      const remainingBytes = Math.max(0, progress.total - progress.transferred)
      const remainingSeconds = progress.bytesPerSecond > 0
        ? Math.round(remainingBytes / progress.bytesPerSecond)
        : undefined
      this.update({
        status: 'downloading', ...release,
        progress: {
          percent: progress.percent,
          transferredBytes: progress.transferred,
          totalBytes: progress.total,
          bytesPerSecond: progress.bytesPerSecond,
          ...(remainingSeconds === undefined ? {} : { remainingSeconds })
        }
      })
    })
    updater.on('update-downloaded', (info: UpdateInfoLike) => {
      const existing = isReleaseState(this.current) ? releaseFromState(this.current) : undefined
      this.update({ status: 'downloaded', ...(existing ?? this.releaseState(info)), version: info.version })
    })
    updater.on('error', (error: unknown) => this.onError(error))
  }

  private releaseState(info: UpdateInfoLike): AppUpdateReleaseState {
    const size = info.files?.find(({ size }) => typeof size === 'number')?.size
    return {
      currentVersion: this.options.currentVersion,
      version: info.version,
      releaseNotes: normalizeReleaseNotes(info.releaseNotes),
      ...(info.releaseDate ? { releaseDate: info.releaseDate } : {}),
      ...(size === undefined ? {} : { sizeBytes: size })
    }
  }

  private onError(error: unknown): void {
    this.update({
      status: 'error', currentVersion: this.options.currentVersion,
      errorMessage: error instanceof Error ? error.message : String(error)
    })
  }

  private update(state: AppUpdateState): void {
    this.current = state
    this.options.publish?.(this.state())
  }
}

interface UpdateInfoLike {
  version: string
  releaseDate?: string
  releaseNotes?: string | Array<{ version?: string; note: string }>
  files?: Array<{ size?: number }>
}

interface DownloadProgressLike {
  percent: number
  transferred: number
  total: number
  bytesPerSecond: number
}

function normalizeReleaseNotes(value: UpdateInfoLike['releaseNotes']): string[] {
  const notes = Array.isArray(value) ? value.map(({ note }) => note) : value ? [value] : []
  return notes.flatMap((note) => note
    .replace(/<\/?(?:p|div|li|br)[^>]*>/gi, '\n')
    .replace(/<[^>]+>/g, '')
    .replace(/&nbsp;/gi, ' ')
    .replace(/&amp;/gi, '&')
    .split('\n')
    .map((line) => line.trim())
    .filter(Boolean))
}

function isReleaseState(state: AppUpdateState): state is AppUpdateState & AppUpdateReleaseState {
  return state.status === 'available' || state.status === 'downloading' || state.status === 'downloaded'
}

function releaseFromState(state: AppUpdateState & AppUpdateReleaseState): AppUpdateReleaseState {
  return {
    currentVersion: state.currentVersion,
    version: state.version,
    releaseNotes: [...state.releaseNotes],
    ...(state.releaseDate ? { releaseDate: state.releaseDate } : {}),
    ...(state.sizeBytes === undefined ? {} : { sizeBytes: state.sizeBytes })
  }
}
