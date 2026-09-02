import type {
  AppUpdateErrorStage, AppUpdateInstallMode, AppUpdateProgress, AppUpdateReleaseState,
  AppUpdateState
} from '../shared/desktop-api'

export interface AppUpdaterAdapter {
  autoDownload: boolean
  autoInstallOnAppQuit: boolean
  channel?: string | null
  on(event: string, listener: (...args: any[]) => void): unknown
  checkForUpdates(): Promise<unknown>
  downloadUpdate(): Promise<unknown>
  quitAndInstall(isSilent?: boolean, isForceRunAfter?: boolean): void
  setFeedURL?(options: { provider: 'generic'; url: string; channel?: string }): void
}

export interface AppUpdateManagerOptions {
  updater: AppUpdaterAdapter
  enabled: boolean
  currentVersion: string
  initialDelayMs?: number
  intervalMs?: number
  checkRetryDelaysMs?: number[]
  sleep?: (milliseconds: number) => Promise<void>
  installMode?: AppUpdateInstallMode
  updateBaseUrl?: string
  downloadManualInstaller?: (input: {
    url: string
    expectedSha512?: string
    onProgress: (progress: AppUpdateProgress) => void
  }) => Promise<string>
  openManualInstaller?: (path: string) => Promise<void>
  publish?: (state: AppUpdateState) => void
  prepareInstall?: () => Promise<void>
}

export class AppUpdateManager {
  private current: AppUpdateState
  private initialTimer: ReturnType<typeof setTimeout> | undefined
  private intervalTimer: ReturnType<typeof setInterval> | undefined
  private checkPromise: Promise<void> | undefined
  private checkCycleActive = false
  private pendingCheckError: unknown
  private installPromise?: Promise<void>
  private manualInstaller: ManualInstallerInfo | undefined
  private manualInstallerPath: string | undefined
  private lastRelease: AppUpdateReleaseState | undefined
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
    this.checkPromise ??= this.checkWithRetry().finally(() => {
      this.checkPromise = undefined
    })
    await this.checkPromise
  }

  async download(): Promise<void> {
    if (!this.options.enabled) return
    const manualDownloadUrl = this.current.status === 'available'
      ? this.current.installMode === 'manual' ? this.current.manualDownloadUrl : undefined
      : this.current.status === 'error' ? this.current.manualDownloadUrl : undefined
    try {
      if (manualDownloadUrl) {
        if (!this.options.downloadManualInstaller) throw new Error('应用内下载器尚未初始化')
        const previous = this.current.status === 'available'
          ? releaseFromState(this.current)
          : this.lastRelease
        const release: AppUpdateReleaseState = {
          currentVersion: this.options.currentVersion,
          version: previous?.version ?? (this.current.status === 'error' ? this.current.version : undefined)
            ?? this.options.currentVersion,
          releaseNotes: previous?.releaseNotes ?? [],
          installMode: 'manual',
          manualDownloadUrl,
          ...(previous?.releaseDate ? { releaseDate: previous.releaseDate } : {}),
          ...(this.manualInstaller?.size === undefined
            ? previous?.sizeBytes === undefined ? {} : { sizeBytes: previous.sizeBytes }
            : { sizeBytes: this.manualInstaller.size })
        }
        this.lastRelease = release
        this.manualInstallerPath = undefined
        const onProgress = (progress: AppUpdateProgress) => {
          this.update({ status: 'downloading', ...release, progress })
        }
        onProgress({
          percent: 0, transferredBytes: 0,
          totalBytes: this.manualInstaller?.size ?? release.sizeBytes ?? 0,
          bytesPerSecond: 0
        })
        this.manualInstallerPath = await this.options.downloadManualInstaller({
          url: manualDownloadUrl,
          ...(this.manualInstaller?.sha512 ? { expectedSha512: this.manualInstaller.sha512 } : {}),
          onProgress
        })
        this.update({ status: 'downloaded', ...release })
        return
      }
      if (this.current.status !== 'available') return
      await this.options.updater.downloadUpdate()
    } catch (error) {
      this.onError(error, 'download')
    }
  }

  async install(): Promise<void> {
    if (!this.options.enabled || this.current.status !== 'downloaded') return
    if (this.current.installMode === 'manual') {
      try {
        if (!this.manualInstallerPath || !this.options.openManualInstaller) {
          throw new Error('已下载的 DMG 路径不存在')
        }
        await this.options.openManualInstaller(this.manualInstallerPath)
      } catch (error) {
        this.onError(error, 'install')
      }
      return
    }
    if (!this.installPromise) {
      this.installPromise = (async () => {
        try {
          await this.options.prepareInstall?.()
          this.options.updater.quitAndInstall(false, true)
        } catch (error) {
          this.onError(error, 'install')
        }
      })()
    }
    await this.installPromise
  }

  private bindUpdaterEvents(): void {
    const { updater } = this.options
    updater.on('checking-for-update', () => {
      const retry = this.current.status === 'checking' && this.current.retryAttempt
        ? {
            retryAttempt: this.current.retryAttempt,
            ...(this.current.maxRetryAttempts === undefined
              ? {} : { maxRetryAttempts: this.current.maxRetryAttempts })
          }
        : {}
      this.update({ status: 'checking', currentVersion: this.options.currentVersion, ...retry })
    })
    updater.on('update-not-available', () => {
      this.update({ status: 'not-available', currentVersion: this.options.currentVersion })
    })
    updater.on('update-available', (info: UpdateInfoLike) => {
      this.manualInstaller = resolveManualInstaller(info.files, this.options.updateBaseUrl)
      const release = this.releaseState(info, this.manualInstaller)
      this.lastRelease = release
      this.update({ status: 'available', ...release })
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
    updater.on('error', (error: unknown) => {
      if (this.checkCycleActive) {
        this.pendingCheckError = error
        return
      }
      this.onError(error)
    })
  }

  private async checkWithRetry(): Promise<void> {
    const retryDelays = this.options.checkRetryDelaysMs ?? [600, 1_600]
    const sleep = this.options.sleep ?? delay
    let lastError: unknown
    this.checkCycleActive = true
    this.update({ status: 'checking', currentVersion: this.options.currentVersion })
    try {
      for (let attempt = 0; attempt <= retryDelays.length; attempt += 1) {
        this.pendingCheckError = undefined
        try {
          await this.options.updater.checkForUpdates()
          if (this.pendingCheckError) throw this.pendingCheckError
          return
        } catch (error) {
          lastError = this.pendingCheckError ?? error
          if (attempt >= retryDelays.length) break
          this.update({
            status: 'checking', currentVersion: this.options.currentVersion,
            retryAttempt: attempt + 1, maxRetryAttempts: retryDelays.length
          })
          await sleep(retryDelays[attempt] ?? 0)
        }
      }
    } finally {
      this.checkCycleActive = false
      this.pendingCheckError = undefined
    }
    this.onError(lastError ?? new Error('更新检查失败'), 'check')
  }

  private releaseState(
    info: UpdateInfoLike,
    manualInstaller = resolveManualInstaller(info.files, this.options.updateBaseUrl)
  ): AppUpdateReleaseState {
    const automaticSize = info.files?.find(({ size }) => typeof size === 'number')?.size
    const size = (this.options.installMode ?? 'automatic') === 'manual'
      ? manualInstaller?.size ?? automaticSize
      : automaticSize
    return {
      currentVersion: this.options.currentVersion,
      version: info.version,
      releaseNotes: normalizeReleaseNotes(info.releaseNotes),
      installMode: this.options.installMode ?? 'automatic',
      ...(info.releaseDate ? { releaseDate: info.releaseDate } : {}),
      ...(size === undefined ? {} : { sizeBytes: size }),
      ...(manualInstaller?.url ? { manualDownloadUrl: manualInstaller.url } : {})
    }
  }

  private onError(error: unknown, stage?: AppUpdateErrorStage): void {
    const message = error instanceof Error ? error.message : String(error)
    const release = isReleaseState(this.current) ? releaseFromState(this.current) : undefined
    const errorStage = stage ?? inferErrorStage(this.current, message)
    this.update({
      status: 'error', currentVersion: this.options.currentVersion,
      errorMessage: message, errorStage,
      ...(release?.version ? { version: release.version } : {}),
      ...(release?.manualDownloadUrl ? { manualDownloadUrl: release.manualDownloadUrl } : {})
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
  files?: Array<{ size?: number; url?: string; sha512?: string }>
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
    installMode: state.installMode,
    ...(state.releaseDate ? { releaseDate: state.releaseDate } : {}),
    ...(state.sizeBytes === undefined ? {} : { sizeBytes: state.sizeBytes }),
    ...(state.manualDownloadUrl ? { manualDownloadUrl: state.manualDownloadUrl } : {})
  }
}

interface ManualInstallerInfo {
  url: string
  size?: number
  sha512?: string
}

function resolveManualInstaller(
  files: UpdateInfoLike['files'], updateBaseUrl?: string
): ManualInstallerInfo | undefined {
  const file = files?.find(({ url }) => url && /\.dmg(?:$|[?#])/i.test(url))
  if (!file?.url) return undefined
  try {
    const url = updateBaseUrl
      ? new URL(file.url, `${updateBaseUrl.replace(/\/+$/, '')}/`).toString()
      : new URL(file.url).toString()
    return {
      url,
      ...(file.size === undefined ? {} : { size: file.size }),
      ...(file.sha512 ? { sha512: file.sha512 } : {})
    }
  } catch {
    return undefined
  }
}

function inferErrorStage(state: AppUpdateState, message: string): AppUpdateErrorStage {
  if (/code signature|signature.+(?:validation|valid)|did not pass validation|签名.+(?:验证|资源)/i.test(message)) {
    return 'verify'
  }
  if (state.status === 'checking') return 'check'
  if (state.status === 'available' || state.status === 'downloading') return 'download'
  if (state.status === 'downloaded') return 'verify'
  return 'check'
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds))
}
