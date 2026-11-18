import { spawn } from 'node:child_process'
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { createInterface } from 'node:readline'
import { expect, it } from 'vitest'
import type { RouterProcessEvent } from './process-owner'

const supervisor = join(import.meta.dirname, 'process-supervisor.ts')
const output = resolve(import.meta.dirname, '../../../../output/model-router-recovery')

it.runIf(process.platform !== 'win32')('restarts three times with 300/600/1200 ms backoff and reports the last exit', async () => {
  await mkdir(output, { recursive: true })
  const directory = await mkdtemp(join(output, 'supervisor-'))
  const binary = join(directory, 'router')
  const config = join(directory, 'config.json')
  await writeFile(binary, '#!/bin/sh\nexit 23\n', { mode: 0o700 })
  const child = spawn(process.execPath, ['--experimental-strip-types', supervisor, binary, config, directory], { stdio: ['pipe', 'pipe', 'ignore'] })
  const observations: Array<{ event: RouterProcessEvent; time: number }> = []
  createInterface({ input: child.stdout }).on('line', (line: string) => {
    observations.push({ event: JSON.parse(line), time: performance.now() })
  })
  try {
    const code = await new Promise<number | null>((done, reject) => {
      child.once('close', done)
      child.once('error', reject)
    })
    expect(code).toBe(1)
    expect(observations.map(({ event }) => event)).toEqual([
      { state: 'spawned', pid: expect.any(Number), restarts: 0 },
      ...[1, 2, 3].map(restarts => ({ state: 'spawned', pid: expect.any(Number), restarts, reason: 'router exited with code 23' })),
      { state: 'failed', reason: 'restart-exhausted: router exited with code 23' }
    ])
    for (const [index, backoff] of [300, 600, 1200].entries()) {
      expect(observations[index + 1]!.time - observations[index]!.time).toBeGreaterThanOrEqual(backoff - 40)
    }
  } finally {
    child.kill()
    await rm(directory, { recursive: true, force: true })
  }
})
