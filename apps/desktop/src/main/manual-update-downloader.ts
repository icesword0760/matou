import { createHash } from 'node:crypto'
import { mkdir, open, rename, unlink } from 'node:fs/promises'
import { join } from 'node:path'

import type { AppUpdateProgress } from '../shared/desktop-api'

interface DownloadManualUpdateOptions {
  url: string
  destinationDirectory: string
  expectedSha512?: string
  fetcher?: (url: string) => Promise<Response>
  now?: () => number
  onProgress: (progress: AppUpdateProgress) => void
}

const PROGRESS_EMIT_INTERVAL_MS = 100

export async function downloadManualUpdate(options: DownloadManualUpdateOptions): Promise<string> {
  const response = await (options.fetcher ?? fetch)(options.url)
  if (!response.ok) throw new Error(`更新文件下载失败（HTTP ${response.status}）`)
  if (!response.body) throw new Error('更新服务器没有返回文件内容')

  await mkdir(options.destinationDirectory, { recursive: true })
  const finalPath = join(options.destinationDirectory, installerFileName(options.url))
  const partialPath = `${finalPath}.part`
  const totalBytes = Number(response.headers.get('content-length')) || 0
  const now = options.now ?? Date.now
  const startedAt = now()
  let lastProgressAt = startedAt
  const hash = createHash('sha512')
  let transferredBytes = 0
  let file: Awaited<ReturnType<typeof open>> | undefined

  try {
    file = await open(partialPath, 'w')
    const reader = response.body.getReader()
    while (true) {
      const { done, value } = await reader.read()
      if (done) break
      if (!value) continue
      await file.write(value)
      hash.update(value)
      transferredBytes += value.byteLength
      const progressAt = now()
      if (progressAt - lastProgressAt >= PROGRESS_EMIT_INTERVAL_MS) {
        lastProgressAt = progressAt
        options.onProgress(progressState(transferredBytes, totalBytes, startedAt, progressAt))
      }
    }
    await file.close()
    file = undefined

    if (options.expectedSha512 && hash.digest('base64') !== options.expectedSha512) {
      throw new Error('更新文件完整性校验失败')
    }
    await rename(partialPath, finalPath)
    options.onProgress(progressState(
      transferredBytes, totalBytes || transferredBytes, startedAt, now(), 100
    ))
    return finalPath
  } catch (error) {
    await file?.close().catch(() => undefined)
    await unlink(partialPath).catch(() => undefined)
    throw error
  }
}

function installerFileName(url: string): string {
  const pathName = new URL(url).pathname.split('/').pop() || 'Matou-update.dmg'
  const safeName = decodeURIComponent(pathName).replace(/[^A-Za-z0-9._-]/g, '_')
  return safeName.toLowerCase().endsWith('.dmg') ? safeName : `${safeName}.dmg`
}

function progressState(
  transferredBytes: number, totalBytes: number, startedAt: number, currentTime: number,
  forcedPercent?: number
): AppUpdateProgress {
  const elapsedSeconds = Math.max(0.001, (currentTime - startedAt) / 1_000)
  const bytesPerSecond = Math.round(transferredBytes / elapsedSeconds)
  const remainingBytes = Math.max(0, totalBytes - transferredBytes)
  return {
    percent: forcedPercent ?? (totalBytes > 0 ? Math.min(100, transferredBytes / totalBytes * 100) : 0),
    transferredBytes,
    totalBytes,
    bytesPerSecond,
    ...(bytesPerSecond > 0 ? { remainingSeconds: Math.round(remainingBytes / bytesPerSecond) } : {})
  }
}
