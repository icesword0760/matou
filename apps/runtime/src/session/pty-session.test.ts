import { execFile } from 'node:child_process'
import { chmod, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { promisify } from 'node:util'

import { afterEach, describe, expect, it } from 'vitest'

import type { RuntimeMessage } from '@matou/contracts'

import { PtySession, SessionEgressFrozenError } from './pty-session'
import { readSessionJournalBounds } from '../journal/journal-range-reader'
import { SegmentJournal } from '../journal/segment-journal'
import { CALLER_SESSION_MARKERS } from './independent-session-environment'

const roots: string[] = []

describe('PtySession caller environment isolation', () => {
  it.each([
    { profile: 'shell', mode: 'new' },
    { profile: 'claude-code', mode: 'new' },
    { profile: 'claude-code', mode: 'resume' },
    { profile: 'claude-code', mode: 'fork' },
    { profile: 'codex', mode: 'resume' }
  ] as const)(
    'isolates caller identity for $profile $mode even with launch overrides', async ({ profile, mode }) => {
      const root = await mkdtemp(join(tmpdir(), 'matou-independent-session-'))
      roots.push(root)
      const executable = join(root, 'provider.cjs')
      await writeFile(executable, `#!/usr/bin/env node
const keys = ${JSON.stringify(CALLER_SESSION_MARKERS)};
console.log('ISOLATION_RESULT=' + JSON.stringify({
  markers: keys.filter(key => process.env[key] !== undefined),
  auth: process.env.ANTHROPIC_API_KEY,
  caller: process.env.MATOU_CONTROL_CALLER_SESSION
}));
setInterval(() => {}, 1000);
`)
      await chmod(executable, 0o755)
      const commandKey = profile === 'shell' ? 'SHELL'
        : profile === 'claude-code' ? 'MATOU_CLAUDE_COMMAND' : 'MATOU_CODEX_COMMAND'
      const keys = [...CALLER_SESSION_MARKERS, commandKey]
      const previous = new Map(keys.map(key => [key, process.env[key]]))
      for (const key of CALLER_SESSION_MARKERS) process.env[key] = 'parent-marker'
      process.env[commandKey] = executable
      let session: PtySession | undefined
      let output = ''
      try {
        session = await PtySession.create({
          sessionId: `independent-${profile}`, executionContextId: 'local-default',
          cols: 100, rows: 24, cwd: root, dataRoot: root, profile,
          ...(mode === 'new' ? {} : { providerSessionId: 'fixture-resume' }),
          ...(mode === 'fork' ? { forkSession: true } : {}),
          env: { ANTHROPIC_API_KEY: 'fixture-key', MATOU_CONTROL_CALLER_SESSION: 'new-card',
            CLAUDE_CODE_CHILD_SESSION: 'override-parent',
            CLAUDE_CODE_SESSION_NAME: 'Unrelated launch override name',
            CLAUDE_CODE_SESSION_KIND: 'bg' },
          onOutput: data => { output += data }
        })
        await waitUntil(() => output.includes('ISOLATION_RESULT='))
        expect(output).toContain('"markers":[]')
        expect(output).toContain('"auth":"fixture-key"')
        expect(output).toContain('"caller":"new-card"')
        for (const key of CALLER_SESSION_MARKERS) expect(process.env[key]).toBe('parent-marker')
      } finally {
        await session?.shutdownForRuntime({ gracePeriodMs: 100, hardKillWaitMs: 1000 })
        for (const [key, value] of previous) restoreEnv(key, value)
      }
    }
  )
})

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
  it.each([true, false])('restores a previous Agent viewport before redraw (checkpoint: %s)', async (withCheckpoint) => {
    const root = await mkdtemp(join(tmpdir(), 'matou-pty-screen-recovery-'))
    roots.push(root)
    const executable = join(root, 'redrawing-provider.sh')
    await writeFile(executable, "#!/bin/sh\nprintf '\\033[2J\\033[Hnew provider output'\nsleep 30\n")
    await chmod(executable, 0o755)
    const journal = await SegmentJournal.open(root, 'agent-screen-recovery')
    const encode = (text: string) => new TextEncoder().encode(text)
    const splitUTF8 = encode('旧输出')
    await journal.appendOutput(1, encode('first run output\r\n'))
    await journal.appendResize(2, 84, 5)
    await journal.appendOutput(3, splitUTF8.subarray(0, 1))
    await journal.appendOutput(4, splitUTF8.subarray(1))
    await journal.appendExit(5, 0)
    await journal.close()

    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    process.env.MATOU_CLAUDE_COMMAND = executable
    const messages: RuntimeMessage[] = []
    const initialSink = (message: RuntimeMessage) => { messages.push(message) }
    let session: PtySession | undefined
    let output = ''
    try {
      session = await PtySession.create({
        sessionId: 'agent-screen-recovery', executionContextId: 'local-default',
        profile: 'claude-code', cols: 80, rows: 6, cwd: root, dataRoot: root,
        send: initialSink,
        onOutput: data => { output += data },
        screenRecovery: { initialGrid: { cols: 80, rows: 6 }, loadCheckpoint: async watermark => {
          expect(watermark).toEqual({ terminalSequence: 5, domainEventSequence: 0 })
          return withCheckpoint ? {
            terminalSequence: 1, screenEpoch: 4, cols: 80, rows: 6,
            snapshot: encode('first run output\r\n')
          } : undefined
        } }
      })
      await waitUntil(() => output.includes('new provider output'))
      const screen = await session.captureReplayScreen()
      const snapshot = new TextDecoder().decode(screen.snapshot)
      expect(snapshot.match(/first run output/g)).toHaveLength(1)
      expect(snapshot).toContain('旧输出')
      expect(snapshot).toContain('new provider output')
      expect(screen).toMatchObject({ cols: 80, rows: 6, screenEpoch: withCheckpoint ? 4 : 0 })
      expect((await session.snapshotScreen()).text).toBe('new provider output')
      expect((await session.readFrames()).find(frame => frame.sequence === 6))
        .toEqual({ kind: 'resize', sequence: 6, cols: 80, rows: 6 })
      const boundary = messages.find(message => message.type === 'terminal.data')
      expect(boundary).toMatchObject({ sequence: 7 })
      expect(output).not.toContain('\x1b[?1049l')
      const replacementMessages: RuntimeMessage[] = []
      const replacementSink = (message: RuntimeMessage) => { replacementMessages.push(message) }
      expect(session.replaceOutputSink(initialSink, replacementSink)).toBe(true)
      session.detach(replacementSink)
      const deliveredBeforeDetach = messages.length
      session.display('detached output stays in history')
      await session.readFrames()
      expect(messages).toHaveLength(deliveredBeforeDetach)
      expect(replacementMessages).toHaveLength(0)
      expect(new TextDecoder().decode((await session.captureReplayScreen()).snapshot))
        .toContain('detached output stays in history')
    } finally {
      await session?.shutdownForRuntime({ gracePeriodMs: 100 })
      restoreEnv('MATOU_CLAUDE_COMMAND', previousCommand)
    }
  })

  it('buffers output beyond one stable replay boundary instead of making replay chase a moving tail', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-pty-replay-barrier-'))
    roots.push(root)
    const executable = join(root, 'quiet-shell.js')
    await writeFile(executable, '#!/usr/bin/env node\nsetInterval(() => {}, 1_000)\n')
    await chmod(executable, 0o755)
    const previousShell = process.env.SHELL
    process.env.SHELL = executable
    const initial: RuntimeMessage[] = []
    const resumed: RuntimeMessage[] = []
    let session: PtySession | undefined
    const initialSend = (message: RuntimeMessage) => { initial.push(message) }
    try {
      session = await PtySession.create({
        sessionId: 'stable-replay-boundary', executionContextId: 'local-default',
        cols: 80, rows: 24, cwd: root, dataRoot: root, profile: 'shell',
        send: initialSend
      })
      session.display('before-replay')
      await waitUntil(() => initial.some(({ type }) => type === 'terminal.data'))
      session.detach(initialSend)

      session.beginReplayCapture()
      session.display('included-in-replay')
      const boundary = (await session.replayMetadata()).lastSequence
      session.display('arrived-during-catch-up')
      await session.replayMetadata()

      session.finishReplayCapture((message) => { resumed.push(message) }, boundary)
      session.display('live-after-replay')
      await waitUntil(() => resumed.filter(({ type }) => type === 'terminal.data').length === 2)

      const output = resumed.filter((message) => message.type === 'terminal.data')
      expect(output.map(({ sequence }) => sequence)).toEqual([boundary + 1, boundary + 2])
      expect(output.map(({ data }) => new TextDecoder().decode(data))).toEqual([
        'arrived-during-catch-up', 'live-after-replay'
      ])
    } finally {
      await session?.endDurability()
      restoreEnv('SHELL', previousShell)
    }
  })

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

describe('PtySession Claude color environment', () => {
  it('uses a deterministic color-capable environment for Claude regardless of its launcher', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-pty-claude-color-'))
    roots.push(root)
    const executable = join(root, 'claude-color-environment.sh')
    await writeFile(executable, `#!/bin/sh
printf 'NO_COLOR=%s\\n' "\${NO_COLOR-unset}"
printf 'COLORTERM=%s\\n' "\${COLORTERM-unset}"
printf 'FORCE_COLOR=%s\\n' "\${FORCE_COLOR-unset}"
sleep 5
`)
    await chmod(executable, 0o755)
    const previousCommand = process.env.MATOU_CLAUDE_COMMAND
    const previousNoColor = process.env.NO_COLOR
    const previousColorTerm = process.env.COLORTERM
    const previousForceColor = process.env.FORCE_COLOR
    process.env.MATOU_CLAUDE_COMMAND = executable
    process.env.NO_COLOR = '1'
    process.env.COLORTERM = ''
    process.env.FORCE_COLOR = '0'
    let session: PtySession | undefined
    let output = ''
    try {
      session = await PtySession.create({
        sessionId: 'claude-color-environment', executionContextId: 'local-default',
        cols: 80, rows: 24, cwd: root, dataRoot: root, profile: 'claude-code',
        send: () => {},
        onOutput: (data) => { output += data }
      })
      await waitUntil(() => output.includes('FORCE_COLOR='))

      expect(output).toContain('NO_COLOR=unset')
      expect(output).toContain('COLORTERM=truecolor')
      expect(output).toContain('FORCE_COLOR=1')
    } finally {
      session?.dispose({ notifyExit: false, reason: 'runtime-shutdown' })
      await session?.whenClosed()
      restoreEnv('MATOU_CLAUDE_COMMAND', previousCommand)
      restoreEnv('NO_COLOR', previousNoColor)
      restoreEnv('COLORTERM', previousColorTerm)
      restoreEnv('FORCE_COLOR', previousForceColor)
    }
  })
})

describe.skipIf(process.platform !== 'darwin')('PtySession Claude idle memory sweeping', () => {
  it.each([
    { profile: 'claude-code', overrides: {}, expected: '1000|unset' },
    { profile: 'claude-code', overrides: { MIMALLOC_PURGE_HOLES_MIN_INTERVAL: '250' }, expected: '250|unset' },
    { profile: 'claude-code', overrides: { mimalloc_purge_holes_min_interval: '500' }, expected: 'unset|500' },
    { profile: 'shell', overrides: {}, expected: 'unset|unset' },
    { profile: 'codex', overrides: {}, expected: 'unset|unset' }
  ] as const)('keeps the $profile launch policy scoped and respects $expected', async ({ profile, overrides, expected }) => {
    const root = await mkdtemp(join(tmpdir(), 'matou-pty-idle-sweep-'))
    roots.push(root)
    const executable = join(root, 'provider.sh')
    await writeFile(executable, `#!/bin/sh
printf 'SWEEP=%s|%s\\n' "\${MIMALLOC_PURGE_HOLES_MIN_INTERVAL-unset}" "\${mimalloc_purge_holes_min_interval-unset}"
sleep 5
`)
    await chmod(executable, 0o755)
    const commandKey = profile === 'shell' ? 'SHELL'
      : profile === 'claude-code' ? 'MATOU_CLAUDE_COMMAND' : 'MATOU_CODEX_COMMAND'
    const keys = [commandKey, 'MIMALLOC_PURGE_HOLES_MIN_INTERVAL', 'mimalloc_purge_holes_min_interval']
    const previous = new Map(keys.map(key => [key, process.env[key]]))
    delete process.env.MIMALLOC_PURGE_HOLES_MIN_INTERVAL
    delete process.env.mimalloc_purge_holes_min_interval
    process.env[commandKey] = executable
    let session: PtySession | undefined
    let output = ''
    try {
      session = await PtySession.create({
        sessionId: 'idle-sweep', executionContextId: 'local-default',
        cols: 80, rows: 24, cwd: root, dataRoot: root, profile,
        env: overrides, onOutput: data => { output += data }
      })
      await waitUntil(() => output.includes('SWEEP='))
      expect(output).toContain(`SWEEP=${expected}`)
      expect(process.env.MIMALLOC_PURGE_HOLES_MIN_INTERVAL).toBeUndefined()
    } finally {
      await session?.shutdownForRuntime({ gracePeriodMs: 100, hardKillWaitMs: 1000 })
      for (const [key, value] of previous) restoreEnv(key, value)
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

function restoreEnv(name: string, value: string | undefined): void {
  if (value === undefined) delete process.env[name]
  else process.env[name] = value
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

const execFileAsync = promisify(execFile)

async function readProcessStat(pid: number): Promise<string> {
  const { stdout } = await execFileAsync('ps', ['-o', 'stat=', '-p', String(pid)])
  return stdout.trim()
}

describe('PtySession egress freeze', () => {
  it('blocks write with SessionEgressFrozenError while allowing resize, screen snapshot and frame reads', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-pty-egress-freeze-'))
    roots.push(root)
    const previousShell = process.env.SHELL
    process.env.SHELL = '/bin/sh'
    let session: PtySession | undefined
    let output = ''
    try {
      session = await PtySession.create({
        sessionId: 'egress-freeze-ops', executionContextId: 'local-default',
        cols: 80, rows: 24, cwd: root, dataRoot: root, profile: 'shell',
        send: () => {},
        onOutput: (data) => { output += data }
      })
      session.write('echo BEFORE_FREEZE\n')
      await waitUntil(() => output.includes('BEFORE_FREEZE'))

      await session.setEgressFrozen(true)
      expect(session.egressFrozen).toBe(true)
      expect(() => session!.write('x')).toThrow(SessionEgressFrozenError)

      session.resize(100, 30)
      await expect(session.snapshotScreen()).resolves.toMatchObject({ cols: 100, rows: 30 })
      await expect(session.captureReplayScreen()).resolves.toMatchObject({ cols: 100, rows: 30 })
      const frames = await session.readFrames()
      expect(frames.some((frame) => frame.kind === 'resize' && frame.cols === 100 && frame.rows === 30)).toBe(true)
      await expect(session.replayMetadata()).resolves.toMatchObject({ lastSequence: session.lastSequence })

      await session.setEgressFrozen(false)
      expect(session.egressFrozen).toBe(false)
      session.write('echo AFTER_THAW\n')
      await waitUntil(() => output.includes('AFTER_THAW'))
    } finally {
      await session?.shutdownForRuntime({ gracePeriodMs: 200, hardKillWaitMs: 1_000 })
      restoreEnv('SHELL', previousShell)
    }
  })

  it('exits within 3 seconds when disposed while egress-frozen', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-pty-egress-dispose-'))
    roots.push(root)
    const previousShell = process.env.SHELL
    process.env.SHELL = '/bin/sh'
    let session: PtySession | undefined
    try {
      session = await PtySession.create({
        sessionId: 'egress-freeze-dispose', executionContextId: 'local-default',
        cols: 80, rows: 24, cwd: root, dataRoot: root, profile: 'shell',
        send: () => {}
      })
      await session.setEgressFrozen(true)
      expect(session.egressFrozen).toBe(true)

      const startedAt = Date.now()
      session.dispose({ notifyExit: false, reason: 'structure-removal' })
      await Promise.race([
        session.whenClosed(),
        new Promise((_, reject) => setTimeout(() => reject(new Error('frozen dispose timed out')), 3_000))
      ])
      expect(Date.now() - startedAt).toBeLessThan(3_000)
    } finally {
      if (session) await session.shutdownForRuntime({ gracePeriodMs: 100, hardKillWaitMs: 500 })
      restoreEnv('SHELL', previousShell)
    }
  })

  it('finishes shutdownForRuntime cleanly when egress-frozen', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-pty-egress-shutdown-'))
    roots.push(root)
    const previousShell = process.env.SHELL
    process.env.SHELL = '/bin/sh'
    let session: PtySession | undefined
    try {
      session = await PtySession.create({
        sessionId: 'egress-freeze-shutdown', executionContextId: 'local-default',
        cols: 80, rows: 24, cwd: root, dataRoot: root, profile: 'shell',
        send: () => {}
      })
      await session.setEgressFrozen(true)

      const startedAt = Date.now()
      await session.shutdownForRuntime({ gracePeriodMs: 500, hardKillWaitMs: 1_000 })
      expect(Date.now() - startedAt).toBeLessThan(3_000)
      await expect(session.whenClosed()).resolves.toBeUndefined()
    } finally {
      restoreEnv('SHELL', previousShell)
    }
  })

  it.skipIf(process.platform === 'win32')('keeps process stopped when durability backpressure resumes to low watermark while egress-frozen', async () => {
    const root = await mkdtemp(join(tmpdir(), 'matou-pty-egress-backpressure-'))
    roots.push(root)
    const previousShell = process.env.SHELL
    process.env.SHELL = '/bin/sh'
    let releaseBlockedWrite: () => void = () => {}
    const blockedWrite = new Promise<void>((resolve) => { releaseBlockedWrite = resolve })
    let blockFirstLargeWrite = true
    let writeEntered: () => void = () => {}
    const firstLargeWriteEntered = new Promise<void>((resolve) => { writeEntered = resolve })
    let session: PtySession | undefined
    try {
      session = await PtySession.create({
        sessionId: 'egress-freeze-backpressure', executionContextId: 'local-default',
        cols: 80, rows: 24, cwd: root, dataRoot: root, profile: 'shell',
        send: () => {},
        journalOptions: {
          writeFrame: async (handle, encoded) => {
            if (blockFirstLargeWrite && encoded.byteLength >= 512 * 1024) {
              blockFirstLargeWrite = false
              writeEntered()
              await blockedWrite
            }
            await handle.write(encoded)
          }
        }
      })

      await session.setEgressFrozen(true)
      const statWhileFrozen = await readProcessStat(session.pid)
      console.info('[egress-test] stat while frozen:', statWhileFrozen)
      expect(statWhileFrozen).toContain('T')

      // Push healthy durability retention above the 1 MiB high watermark so
      // durability backpressure pauses too, then drain back below low watermark.
      session.display('a'.repeat(700 * 1024))
      await firstLargeWriteEntered
      session.display('b'.repeat(700 * 1024))
      expect(session.retainedDurabilityBytes).toBeGreaterThanOrEqual(1024 * 1024)

      releaseBlockedWrite()
      await session.readFrames()
      expect(session.retainedDurabilityBytes).toBe(0)

      const statAfterBackpressureResume = await readProcessStat(session.pid)
      console.info('[egress-test] stat after backpressure resume while still frozen:', statAfterBackpressureResume)
      expect(statAfterBackpressureResume).toContain('T')

      await session.setEgressFrozen(false)
      await waitUntil(() => !session!.egressFrozen)
      const statAfterThaw = await readProcessStat(session.pid)
      expect(statAfterThaw).not.toContain('T')
    } finally {
      releaseBlockedWrite()
      await session?.shutdownForRuntime({ gracePeriodMs: 200, hardKillWaitMs: 1_000 })
      restoreEnv('SHELL', previousShell)
    }
  })
})
