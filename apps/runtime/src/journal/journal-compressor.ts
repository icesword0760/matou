import { createReadStream, createWriteStream } from 'node:fs'
import { open, rename, rm, stat } from 'node:fs/promises'
import { dirname } from 'node:path'
import { pipeline } from 'node:stream/promises'
import { createGzip } from 'node:zlib'

export type JournalCompressionPhase = 'temp-written' | 'published' | 'raw-deleted'

export interface JournalCompressionCandidate {
  sessionId: string
  index: number
  path: string
}

export interface JournalCompressionResult {
  sessionId: string
  index: number
  path: string
}

export interface JournalCompressorOptions {
  concurrency?: number
  afterPhase?(phase: JournalCompressionPhase, candidate: JournalCompressionCandidate): Promise<void> | void
}

interface ScheduledCompression {
  candidate: JournalCompressionCandidate
  resolve(value: JournalCompressionResult): void
  reject(error: unknown): void
}

export class JournalCompressor {
  readonly #concurrency: number
  readonly #afterPhase: NonNullable<JournalCompressorOptions['afterPhase']>
  readonly #scheduled = new Map<string, Promise<JournalCompressionResult>>()
  readonly #queue: ScheduledCompression[] = []
  readonly #idleWaiters = new Set<() => void>()
  #active = 0

  constructor(options: JournalCompressorOptions = {}) {
    this.#concurrency = Math.max(1, Math.floor(options.concurrency ?? 2))
    this.#afterPhase = options.afterPhase ?? (() => {})
  }

  schedule(candidate: JournalCompressionCandidate): Promise<JournalCompressionResult> {
    const key = compressionKey(candidate)
    const existing = this.#scheduled.get(key)
    if (existing) return existing
    let resolve!: (value: JournalCompressionResult) => void
    let reject!: (error: unknown) => void
    const promise = new Promise<JournalCompressionResult>((done, fail) => {
      resolve = done
      reject = fail
    })
    this.#scheduled.set(key, promise)
    this.#queue.push({ candidate, resolve, reject })
    this.#pump()
    return promise
  }

  compress(candidate: JournalCompressionCandidate): Promise<JournalCompressionResult> {
    return this.#compress(candidate)
  }

  whenIdle(): Promise<void> {
    if (this.#active === 0 && this.#queue.length === 0) return Promise.resolve()
    return new Promise((resolve) => this.#idleWaiters.add(resolve))
  }

  #pump(): void {
    while (this.#active < this.#concurrency) {
      const scheduled = this.#queue.shift()
      if (!scheduled) break
      this.#active += 1
      void this.#compress(scheduled.candidate).then(
        scheduled.resolve,
        scheduled.reject
      ).finally(() => {
        this.#active -= 1
        this.#scheduled.delete(compressionKey(scheduled.candidate))
        this.#pump()
        this.#resolveIdleIfNeeded()
      })
    }
    this.#resolveIdleIfNeeded()
  }

  #resolveIdleIfNeeded(): void {
    if (this.#active !== 0 || this.#queue.length !== 0) return
    for (const resolve of this.#idleWaiters) resolve()
    this.#idleWaiters.clear()
  }

  async #compress(candidate: JournalCompressionCandidate): Promise<JournalCompressionResult> {
    if (!candidate.path.endsWith('.mtj')) {
      throw new Error('Journal compression requires a raw .mtj segment')
    }
    const compressedPath = `${candidate.path}.gz`
    const partialPath = `${compressedPath}.partial`

    if (await isReadableJournalGzip(compressedPath)) {
      await rm(candidate.path, { force: true })
      await syncDirectory(dirname(compressedPath))
      return { sessionId: candidate.sessionId, index: candidate.index, path: compressedPath }
    }

    await rm(compressedPath, { force: true })
    await rm(partialPath, { force: true })
    await pipeline(
      createReadStream(candidate.path),
      createGzip({ level: 6 }),
      createWriteStream(partialPath, { flags: 'wx', mode: 0o600 })
    )
    await syncFile(partialPath)
    await this.#afterPhase('temp-written', candidate)

    await rename(partialPath, compressedPath)
    await syncDirectory(dirname(compressedPath))
    await this.#afterPhase('published', candidate)

    await rm(candidate.path)
    await syncDirectory(dirname(compressedPath))
    await this.#afterPhase('raw-deleted', candidate)
    return { sessionId: candidate.sessionId, index: candidate.index, path: compressedPath }
  }
}

export const defaultJournalCompressor = new JournalCompressor()

async function isReadableJournalGzip(path: string): Promise<boolean> {
  try {
    if ((await stat(path)).size === 0) return false
    const { readSegmentFrames } = await import('./segment-journal')
    await readSegmentFrames(path)
    return true
  } catch {
    return false
  }
}

async function syncFile(path: string): Promise<void> {
  const handle = await open(path, 'r')
  try {
    await handle.sync()
  } finally {
    await handle.close()
  }
}

async function syncDirectory(path: string): Promise<void> {
  const handle = await open(path, 'r')
  try {
    await handle.sync()
  } finally {
    await handle.close()
  }
}

function compressionKey(candidate: JournalCompressionCandidate): string {
  return `${candidate.sessionId}:${candidate.index}:${candidate.path}`
}
