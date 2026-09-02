import { chmod, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

import { afterEach, describe, expect, it } from 'vitest'

import type { RuntimeMessage } from '@matou/contracts'

import { PtySession } from './pty-session'
import { readSessionJournalBounds } from '../journal/journal-range-reader'

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

describe('PtySession live catch-up', () => {
  it('streams a credit-resumed suffix without decoding a cold segment', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-pty-range-replay-'))
    roots.push(root)
    const executable = join(root, 'quiet-shell.js')
    await writeFile(executable, '#!/usr/bin/env node\nsetInterval(() => {}, 1_000)\n')
    await chmod(executable, 0o755)
    const previousShell = process.env.SHELL
    process.env.SHELL = executable
    const sent: RuntimeMessage[] = []
    let session: PtySession | undefined
    try {
      session = await PtySession.create({
        sessionId: 'credit-range-session', executionContextId: 'local-default',
        cols: 80, rows: 24, cwd: root, dataRoot: root, profile: 'shell',
        send: (message) => { sent.push(message) },
        journalOptions: { maxSegmentBytes: 700 * 1024, compressSealed: false }
      })
      session.display('a'.repeat(600 * 1024))
      session.display('b'.repeat(600 * 1024))
      session.display('c'.repeat(600 * 1024))
      await waitUntil(() => sent.filter(({ type }) => type === 'terminal.data').length === 2)
      await session.replayMetadata()
      const bounds = await readSessionJournalBounds(root, 'credit-range-session')
      const cold = bounds.segments.find(({ lastSequence }) => lastSequence < 3)
      expect(cold).toBeDefined()
      await writeFile(cold!.path, 'corrupted cold segment')

      session.acknowledge(2)
      await waitUntil(() => sent.filter(({ type }) => type === 'terminal.data').length === 3)

      expect(sent.filter(({ type }) => type === 'terminal.data').at(-1)).toMatchObject({
        sequence: 3
      })
    } finally {
      await session?.endDurability()
      if (previousShell === undefined) delete process.env.SHELL
      else process.env.SHELL = previousShell
    }
  }, 10_000)
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

async function waitUntil(predicate: () => boolean, timeoutMs = 3_000): Promise<void> {
  const deadline = Date.now() + timeoutMs
  while (!predicate()) {
    if (Date.now() >= deadline) throw new Error('condition did not become true')
    await new Promise((resolve) => setTimeout(resolve, 10))
  }
}

describe('PtySession durability recovery', () => {
  it('publishes output metadata only after the retained frame is durably retried', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-pty-durability-'))
    roots.push(root)
    const executable = join(root, 'quiet-shell.js')
    await writeFile(executable, '#!/usr/bin/env node\nsetInterval(() => {}, 1_000)\n')
    await chmod(executable, 0o755)
    const previousShell = process.env.SHELL
    process.env.SHELL = executable
    let failOnce = true
    const output: string[] = []
    let faulted: () => void = () => {}
    const storageFault = new Promise<void>((resolve) => { faulted = resolve })
    let session: PtySession | undefined
    try {
      session = await PtySession.create({
        sessionId: 'durability-retry', executionContextId: 'local-default',
        cols: 80, rows: 24, cwd: root, dataRoot: root, profile: 'shell',
        send: () => {},
        onOutput: (data) => { output.push(data) },
        onDurabilityFault: () => { faulted() },
        journalOptions: {
          writeFrame: async (handle, encoded) => {
            if (failOnce) {
              failOnce = false
              await handle.write(encoded.subarray(0, 9))
              throw Object.assign(new Error('disk full'), { code: 'ENOSPC' })
            }
            await handle.write(encoded)
          }
        }
      })

      session.display('retained-marker')
      await storageFault
      expect(session.durabilityState).toBe('paused')
      expect(output).toEqual([])
      expect(() => session!.write('echo blocked\n')).toThrow('session storage is paused')
      session.display('after-marker')

      await session.retryDurability()

      expect(session.durabilityState).toBe('healthy')
      expect(output).toEqual(['retained-marker', 'after-marker'])
      await expect(session.readFrames()).resolves.toMatchObject([
        { kind: 'output', sequence: 1, data: new TextEncoder().encode('retained-marker') },
        { kind: 'output', sequence: 2, data: new TextEncoder().encode('after-marker') }
      ])
    } finally {
      await session?.endDurability()
      if (previousShell === undefined) delete process.env.SHELL
      else process.env.SHELL = previousShell
    }
  })

  it('finishes Runtime shutdown within its bounds while journal storage is faulted', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-pty-durability-shutdown-'))
    roots.push(root)
    const executable = join(root, 'quiet-shell.js')
    await writeFile(executable, '#!/usr/bin/env node\nsetInterval(() => {}, 1_000)\n')
    await chmod(executable, 0o755)
    const previousShell = process.env.SHELL
    process.env.SHELL = executable
    let faulted: () => void = () => {}
    const storageFault = new Promise<void>((resolve) => { faulted = resolve })
    try {
      const session = await PtySession.create({
        sessionId: 'faulted-shutdown', executionContextId: 'local-default',
        cols: 80, rows: 24, cwd: root, dataRoot: root, profile: 'shell',
        send: () => {},
        onDurabilityFault: () => { faulted() },
        journalOptions: {
          writeFrame: async () => { throw Object.assign(new Error('read only'), { code: 'EACCES' }) }
        }
      })
      session.display('fault')
      await storageFault

      const startedAt = Date.now()
      await session.shutdownForRuntime({ gracePeriodMs: 100, hardKillWaitMs: 500 })

      expect(Date.now() - startedAt).toBeLessThan(1_000)
      await expect(session.whenClosed()).resolves.toBeUndefined()
    } finally {
      if (previousShell === undefined) delete process.env.SHELL
      else process.env.SHELL = previousShell
    }
  })
})
