import { chmod, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, describe, expect, it } from 'vitest'

import { PtySession } from './pty-session'

const roots: string[] = []

afterEach(async () => {
  await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true })))
})

describe('PtySession Runtime shutdown', () => {
  it('escalates an unresponsive PTY and still closes its journal', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-pty-shutdown-'))
    roots.push(root)
    const executable = join(root, 'ignore-graceful-signals.js')
    await writeFile(executable, `#!/usr/bin/env node
process.on('SIGHUP', () => {})
process.on('SIGTERM', () => {})
process.stdout.write('PTY_SHUTDOWN_READY\\n')
setInterval(() => {}, 1_000)
`)
    await chmod(executable, 0o755)
    const previousShell = process.env.SHELL
    process.env.SHELL = executable
    let ready: () => void = () => {}
    const outputReady = new Promise<void>((resolve) => { ready = resolve })
    try {
      const session = await PtySession.create({
        sessionId: 'unresponsive-pty', executionContextId: 'local-default',
        cols: 80, rows: 24, cwd: root, dataRoot: root, profile: 'shell',
        send: () => {},
        onOutput: (data) => { if (data.includes('PTY_SHUTDOWN_READY')) ready() }
      })
      await Promise.race([
        outputReady,
        new Promise((_, reject) => setTimeout(() => reject(new Error('PTY did not start')), 2_000))
      ])

      const startedAt = Date.now()
      await session.shutdownForRuntime({ gracePeriodMs: 40, hardKillWaitMs: 1_000 })

      expect(Date.now() - startedAt).toBeLessThan(1_500)
      await expect(session.whenClosed()).resolves.toBeUndefined()
    } finally {
      if (previousShell === undefined) delete process.env.SHELL
      else process.env.SHELL = previousShell
    }
  })
})

describe('PtySession resize deduplication', () => {
  it('records and applies adjacent identical dimensions only once', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-pty-resize-'))
    roots.push(root)
    const previousShell = process.env.SHELL
    process.env.SHELL = '/bin/sh'
    let session: PtySession | undefined
    try {
      session = await PtySession.create({
        sessionId: 'deduplicated-resize', executionContextId: 'local-default',
        cols: 80, rows: 24, cwd: root, dataRoot: root, profile: 'shell',
        send: () => {}
      })
      session.resize(120, 42)
      session.resize(120, 42)

      const resizeFrames = (await session.readFrames())
        .filter((frame) => frame.kind === 'resize')

      expect(resizeFrames).toEqual([
        { kind: 'resize', sequence: 1, cols: 120, rows: 42 }
      ])
    } finally {
      session?.dispose({ notifyExit: false, reason: 'runtime-shutdown' })
      await session?.whenClosed()
      if (previousShell === undefined) delete process.env.SHELL
      else process.env.SHELL = previousShell
    }
  })
})
