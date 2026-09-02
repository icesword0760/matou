import { createHash } from 'node:crypto'
import { mkdtemp, readFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, describe, expect, it, vi } from 'vitest'

import { downloadManualUpdate } from './manual-update-downloader'

const temporaryDirectories: string[] = []

afterEach(async () => {
  const { rm } = await import('node:fs/promises')
  await Promise.all(temporaryDirectories.splice(0).map((path) => rm(path, { recursive: true, force: true })))
})

describe('downloadManualUpdate', () => {
  it('downloads the DMG into the app cache and reports determinate progress', async () => {
    const directory = await temporaryDirectory()
    const payload = Buffer.from('matou update package')
    const expectedSha512 = createHash('sha512').update(payload).digest('base64')
    const progress: number[] = []

    const path = await downloadManualUpdate({
      url: 'https://updates.example.com/stable/Matou-1.2.0-mac-arm64.dmg',
      destinationDirectory: directory,
      expectedSha512,
      fetcher: vi.fn(async () => new Response(payload, {
        headers: { 'content-length': String(payload.byteLength) }
      })),
      onProgress: (state) => progress.push(state.percent)
    })

    expect(path).toBe(join(directory, 'Matou-1.2.0-mac-arm64.dmg'))
    expect(await readFile(path)).toEqual(payload)
    expect(progress.at(-1)).toBe(100)
  })

  it('rejects a damaged DMG before exposing it as ready to install', async () => {
    const directory = await temporaryDirectory()

    await expect(downloadManualUpdate({
      url: 'https://updates.example.com/stable/Matou-1.2.0-mac-arm64.dmg',
      destinationDirectory: directory,
      expectedSha512: createHash('sha512').update('expected').digest('base64'),
      fetcher: vi.fn(async () => new Response('damaged')),
      onProgress: vi.fn()
    })).rejects.toThrow('更新文件完整性校验失败')

    await expect(readFile(join(directory, 'Matou-1.2.0-mac-arm64.dmg'))).rejects.toThrow()
  })

  it('throttles progress events so the renderer remains responsive during large downloads', async () => {
    const directory = await temporaryDirectory()
    const chunks = Array.from({ length: 20 }, () => new Uint8Array(1_024))
    let chunkIndex = 0
    let now = 0
    const onProgress = vi.fn()

    await downloadManualUpdate({
      url: 'https://updates.example.com/stable/Matou-1.2.0-mac-arm64.dmg',
      destinationDirectory: directory,
      fetcher: vi.fn(async () => new Response(new ReadableStream({
        pull(controller) {
          const chunk = chunks[chunkIndex++]
          if (chunk) controller.enqueue(chunk)
          else controller.close()
        }
      }), { headers: { 'content-length': String(chunks.length * 1_024) } })),
      now: () => now += 20,
      onProgress
    })

    expect(onProgress.mock.calls.length).toBeLessThan(chunks.length)
    expect(onProgress.mock.calls.length).toBeGreaterThan(1)
    expect(onProgress.mock.calls.at(-1)?.[0].percent).toBe(100)
  })
})

async function temporaryDirectory(): Promise<string> {
  const directory = await mkdtemp(join(tmpdir(), 'matou-update-test-'))
  temporaryDirectories.push(directory)
  return directory
}
