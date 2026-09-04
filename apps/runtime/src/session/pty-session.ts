import os from 'node:os'
import { readFile } from 'node:fs/promises'
import { join } from 'node:path'

import type { RuntimeMessage, StorageFaultCode } from '@matou/contracts'
import { PROTOCOL_VERSION } from '@matou/contracts'
import * as pty from 'node-pty'

import { CreditWindow } from '../flow-control/credit-window'
import { TerminalScreenProjector, type TerminalScreenSnapshot } from '../control/terminal-screen-projector'
import { SegmentJournal, type SegmentJournalOptions } from '../journal/segment-journal'
import {
  SessionDurabilityGate,
  type SessionDurabilityFaultEvent,
  type SessionDurabilityRecoveredEvent,
  type SessionDurabilityState
} from './session-durability-gate'
import { PtyExecutionPauser } from './pty-execution-pauser'
import { PtyOutputBatcher } from './pty-output-batcher'
import { resolveProviderCommandEnvironment } from './provider-command-environment'
import { resolvePtyCommand } from './provider-launch-plan'
import { shellIntegrationEnvironment } from './shell-integration'

export interface PtySessionOptions {
  sessionId: string
  executionContextId: string
  cols: number
  rows: number
  cwd: string
  dataRoot: string
  profile?: 'shell' | 'claude-code' | 'codex'
  providerSessionId?: string
  forkSession?: boolean
  permissionMode?: string
  settingsPath?: string
  controlAssetRoot?: string
  model?: string
  env?: Record<string, string>
  send?: (message: RuntimeMessage) => void
  onExit?: (
    session: PtySession,
    exitCode: number,
    signal?: number,
    reason?: 'runtime-shutdown' | 'environment-transition'
  ) => boolean | void
  onOutput?: (data: string) => void
  onDurabilityFault?: (event: SessionDurabilityFaultEvent) => void
  onDurabilityRecovered?: (event: SessionDurabilityRecoveredEvent) => void
  journalOptions?: SegmentJournalOptions
  runId?: string
}

export class PtySession {
  readonly sessionId: string
  readonly pid: number
  readonly executionContextId: string
  readonly profile: 'shell' | 'claude-code' | 'codex'
  readonly runId: string | undefined
  readonly replayFromSequence: number

  readonly #pty: pty.IPty
  readonly #journal: SegmentJournal
  readonly #durabilityGate: SessionDurabilityGate
  #send: ((message: RuntimeMessage) => void) | undefined
  #creditWindow: CreditWindow
  readonly #onExit: ((
    session: PtySession,
    exitCode: number,
    signal?: number,
    reason?: 'runtime-shutdown' | 'environment-transition'
  ) => boolean | void) | undefined
  readonly #onOutput: ((data: string) => void) | undefined
  readonly #encoder = new TextEncoder()
  readonly #outputBatcher: PtyOutputBatcher
  readonly #screen: TerminalScreenProjector

  #sequence: number
  #writeChain = Promise.resolve()
  #disposed = false
  #notifyExit = true
  #exitReason: 'runtime-shutdown' | 'environment-transition' | undefined
  #forceFinalized = false
  #closedResolved = false
  #exitFinalized = false
  #ending: Promise<void> | undefined
  #pendingReplayFrom: number | undefined
  #replayCapture: RuntimeMessage[] | undefined
  #durabilityFault: SessionDurabilityFaultEvent | undefined
  #maximumUnackedBytes = 0
  #lastCols: number
  #lastRows: number
  readonly #closed: Promise<void>
  #resolveClosed: () => void = () => {}

  get lastSequence(): number { return this.#sequence }
  get durabilityState(): SessionDurabilityState { return this.#durabilityGate.state }
  get retainedDurabilityBytes(): number { return this.#durabilityGate.retainedBytes }
  get maximumUnackedBytes(): number { return this.#maximumUnackedBytes }
  tailStart(maxLines = 10_000): number { return this.#journal.tailStart(maxLines) }
  domainEventSequenceAtOrBefore(sequence: number): number {
    return this.#journal.domainEventSequenceAtOrBefore(sequence)
  }
  protectCheckpointSequences(sequences: readonly number[]): Promise<void> {
    return this.#journal.protectCheckpointSequences(sequences)
  }

  private constructor(options: PtySessionOptions, journal: SegmentJournal, terminal: pty.IPty) {
    this.sessionId = options.sessionId
    this.pid = terminal.pid
    this.executionContextId = options.executionContextId
    this.profile = options.profile ?? 'shell'
    this.runId = options.runId
    this.#pty = terminal
    this.#journal = journal
    this.#sequence = journal.lastSequence
    this.#lastCols = options.cols
    this.#lastRows = options.rows
    this.replayFromSequence = journal.lastSequence + 1
    this.#send = options.send
    this.#creditWindow = this.#newCreditWindow()
    this.#onExit = options.onExit
    this.#onOutput = options.onOutput
    this.#screen = new TerminalScreenProjector(options.cols, options.rows)
    this.#closed = new Promise<void>((resolve) => { this.#resolveClosed = resolve })
    this.#durabilityGate = new SessionDurabilityGate({
      sessionId: options.sessionId,
      initialSequence: journal.lastSequence,
      pauser: new PtyExecutionPauser(terminal),
      onFault: (event) => {
        this.#durabilityFault = event
        this.#sendDurabilityFault(event)
        options.onDurabilityFault?.(event)
      },
      onRecovered: (event) => {
        this.#durabilityFault = undefined
        this.#send?.({
          type: 'terminal.storage-recovered',
          protocolVersion: PROTOCOL_VERSION,
          sessionId: this.sessionId,
          sequence: event.throughSequence
        })
        options.onDurabilityRecovered?.(event)
      }
    })
    this.#outputBatcher = new PtyOutputBatcher((data) => this.#enqueueOutputNow(data))

    terminal.onData((data) => this.#outputBatcher.offer(data))
    terminal.onExit(({ exitCode, signal }) => this.#enqueueExit(exitCode, signal))
  }

  static async create(options: PtySessionOptions): Promise<PtySession> {
    const journal = await SegmentJournal.open(options.dataRoot, options.sessionId, options.journalOptions)
    const profile = options.profile ?? 'shell'
    const codexDeveloperInstructions = profile === 'codex' && options.controlAssetRoot
      ? await readProviderInstructions(options.controlAssetRoot)
      : undefined
    const command = resolvePtyCommand({
      profile,
      executable: resolveExecutable(profile),
      ...(options.providerSessionId === undefined ? {} : {
        providerSessionId: options.providerSessionId
      }),
      ...(options.forkSession === undefined ? {} : { forkSession: options.forkSession }),
      ...(options.permissionMode === undefined ? {} : {
        permissionMode: options.permissionMode
      }),
      ...(options.settingsPath === undefined ? {} : {
        settingsPath: options.settingsPath
      }),
      ...(options.controlAssetRoot === undefined ? {} : {
        controlAssetRoot: options.controlAssetRoot
      }),
      ...(codexDeveloperInstructions === undefined ? {} : { codexDeveloperInstructions }),
      ...(options.model === undefined ? {} : { model: options.model })
    })
    const integrationEnvironment = profile === 'shell'
      ? await shellIntegrationEnvironment(options.dataRoot, command.file)
      : {}
    const baseEnvironment: NodeJS.ProcessEnv = {
      ...process.env,
      ...integrationEnvironment,
      ...options.env,
      TERM: 'xterm-256color'
    }
    if (profile === 'claude-code') {
      delete baseEnvironment.NO_COLOR
      baseEnvironment.COLORTERM = 'truecolor'
      baseEnvironment.FORCE_COLOR = '1'
    }
    const providerCommandEnvironment = profile === 'shell'
      ? {}
      : await resolveProviderCommandEnvironment(command.file, baseEnvironment)
    const terminal = pty.spawn(command.file, command.args, {
      name: 'xterm-256color',
      cols: options.cols,
      rows: options.rows,
      cwd: options.cwd,
      env: {
        ...baseEnvironment,
        ...providerCommandEnvironment
      }
    })
    return new PtySession(options, journal, terminal)
  }

  write(data: string): void {
    if (this.#disposed) {
      throw new Error('session is disposed')
    }
    if (this.#durabilityGate.state !== 'healthy') {
      throw new Error('session storage is paused')
    }
    this.#pty.write(data)
  }

  display(data: string): void {
    if (this.#disposed) return
    // Runtime-authored notices and screen resets are control feedback rather
    // than PTY transport bursts; publish them immediately.
    this.#enqueueOutputNow(data)
  }

  resize(cols: number, rows: number): void {
    if (this.#disposed) {
      throw new Error('session is disposed')
    }
    if (cols === this.#lastCols && rows === this.#lastRows) return
    this.#outputBatcher.flush()
    this.#pty.resize(cols, rows)
    void this.#screen.resize(cols, rows)
    this.#lastCols = cols
    this.#lastRows = rows
    const sequence = this.#sequence + 1
    let operation: Promise<void>
    try {
      operation = this.#durabilityGate.append({
        sequence,
        kind: 'resize',
        bytes: this.#encoder.encode(`${cols}:${rows}`),
        persist: () => this.#journal.appendResize(sequence, cols, rows)
      })
    } catch {
      void this.endDurability()
      return
    }
    this.#sequence = sequence
    this.#trackWrite(operation)
  }

  acknowledge(throughSequence: number): void {
    this.#creditWindow.acknowledge(throughSequence)
  }

  readFrames() {
    this.#outputBatcher.flush()
    return this.#writeChain.then(() => this.#journal.readFrames())
  }

  async replayMetadata(maxLines = 10_000) {
    await this.#writeChain
    return this.#journal.replayMetadata(maxLines)
  }

  snapshotScreen(): Promise<TerminalScreenSnapshot> {
    return this.#screen.snapshot()
  }

  iterateFrames(options: { fromSequence: number; throughSequence?: number }) {
    return this.#journal.iterateFrames(options)
  }

  whenClosed(): Promise<void> { return this.#closed }

  async retryDurability(): Promise<void> {
    this.#outputBatcher.flush()
    const operation = this.#durabilityGate.retry()
    this.#trackWrite(operation)
    await operation
  }

  endDurability(): Promise<void> {
    if (this.#ending !== undefined) return this.#ending
    this.#ending = this.#endDurability()
    return this.#ending
  }

  async shutdownForRuntime(options: {
    gracePeriodMs?: number
    hardKillWaitMs?: number
  } = {}): Promise<void> {
    this.#notifyExit = false
    this.#exitReason = 'runtime-shutdown'
    if (this.#durabilityGate.state !== 'healthy') {
      await this.endDurability()
      return
    }
    this.dispose({ notifyExit: false, reason: 'runtime-shutdown' })
    if (await settlesWithin(this.#closed, options.gracePeriodMs ?? 1_500)) return
    try {
      if (process.platform === 'win32') this.#pty.kill()
      else this.#pty.kill('SIGKILL')
    } catch {
      // The PTY may have exited between the grace timeout and escalation.
    }
    if (await settlesWithin(this.#closed, options.hardKillWaitMs ?? 1_000)) return

    // A provider can leave its PTY exit callback unresolved even after the
    // process was killed. Stop accepting new frames, drain all output already
    // observed, and sync the journal so the desktop quit is never held open.
    this.#forceFinalized = true
    try {
      await this.#writeChain
    } finally {
      await this.#closeJournalIgnoringStorageFault()
      this.#resolveClosedOnce()
    }
  }

  attach(send: (message: RuntimeMessage) => void): void {
    this.#replayCapture = undefined
    this.#send = send
    this.#pendingReplayFrom = undefined
    this.#creditWindow = this.#newCreditWindow()
    if (this.#durabilityFault) this.#sendDurabilityFault(this.#durabilityFault)
  }

  detach(send: (message: RuntimeMessage) => void): void {
    this.#replayCapture = undefined
    if (this.#send === send) {
      this.#send = undefined
      this.#pendingReplayFrom = undefined
      this.#creditWindow = this.#newCreditWindow()
    }
  }

  beginReplayCapture(): void {
    this.#replayCapture = []
  }

  cancelReplayCapture(): void {
    this.#replayCapture = undefined
  }

  finishReplayCapture(send: (message: RuntimeMessage) => void, throughSequence: number): void {
    const captured = this.#replayCapture ?? []
    this.#replayCapture = undefined
    this.attach(send)

    for (const message of captured) {
      if (!('sequence' in message) || message.sequence <= throughSequence) continue
      if (message.type === 'terminal.data') {
        if (this.#creditWindow.isPaused) {
          this.#pendingReplayFrom ??= message.sequence
          return
        }
        this.#sendOutput(message.sequence, message.data)
      } else if (message.type === 'terminal.exited') {
        if (this.#creditWindow.isPaused) {
          this.#pendingReplayFrom ??= message.sequence
          return
        }
        this.#send?.(message)
      }
    }
  }

  dispose(options: {
    notifyExit?: boolean
    reason?: 'runtime-shutdown' | 'environment-transition'
  } = {}): void {
    if (this.#disposed) {
      return
    }
    this.#notifyExit = options.notifyExit ?? true
    this.#exitReason = options.reason
    this.#disposed = true
    this.#outputBatcher.flush()
    if (this.#durabilityGate.state !== 'healthy') {
      void this.endDurability()
      return
    }
    this.#pty.kill()
  }

  #enqueueOutputNow(data: string): void {
    void this.#screen.write(data)
    if (this.#forceFinalized) return
    const bytes = this.#encoder.encode(data)
    const sequence = this.#sequence + 1
    let operation: Promise<void>
    try {
      operation = this.#durabilityGate.append({
        sequence,
        kind: 'output',
        bytes,
        persist: () => this.#journal.appendOutput(sequence, bytes),
        afterPersist: () => {
          this.#onOutput?.(data)
          if (this.#replayCapture) {
            this.#replayCapture.push({
              type: 'terminal.data',
              protocolVersion: PROTOCOL_VERSION,
              sessionId: this.sessionId,
              sequence,
              data: bytes
            })
            return
          }
          if (!this.#send) return
          if (this.#creditWindow.isPaused) {
            this.#pendingReplayFrom ??= sequence
            return
          }
          this.#sendOutput(sequence, bytes)
        }
      })
    } catch {
      // No sequence is consumed and no output was published. Terminating the
      // affected execution prevents further output after the bounded FIFO is full.
      void this.endDurability()
      return
    }
    this.#sequence = sequence
    this.#trackWrite(operation)
  }

  #enqueueExit(exitCode: number, signal?: number): void {
    if (this.#forceFinalized) return
    this.#outputBatcher.flush()
    const sequence = this.#sequence + 1
    let operation: Promise<void>
    try {
      operation = this.#durabilityGate.append({
        sequence,
        kind: 'exit',
        bytes: this.#encoder.encode(`${exitCode}:${signal ?? ''}`),
        persist: () => this.#journal.appendExit(sequence, exitCode, signal),
        afterPersist: () => { void this.#finalizeExit(sequence, exitCode, signal) }
      })
    } catch {
      this.#resolveClosedOnce()
      return
    }
    this.#sequence = sequence
    this.#trackWrite(operation)
    void operation.then(() => {
      if (this.#durabilityGate.state !== 'paused') return
      if (this.#disposed) void this.endDurability()
      else this.#resolveClosedOnce()
    })
  }

  async #finalizeExit(sequence: number, exitCode: number, signal?: number): Promise<void> {
    if (this.#exitFinalized) return
    this.#exitFinalized = true
    try {
      await this.#closeJournalIgnoringStorageFault()
      const allowNotification = this.#onExit?.(
        this, exitCode, signal, this.#exitReason
      ) !== false
      if (!this.#notifyExit || !allowNotification) return
      const message: RuntimeMessage = {
        type: 'terminal.exited',
        protocolVersion: PROTOCOL_VERSION,
        sessionId: this.sessionId,
        sequence,
        exitCode,
        ...(signal === undefined ? {} : { signal })
      }
      if (this.#replayCapture) {
        this.#replayCapture.push(message)
        return
      }
      if (!this.#send) {
        return
      }
      if (this.#creditWindow.isPaused) {
        this.#pendingReplayFrom ??= sequence
        return
      }
      this.#send(message)
    } finally {
      this.#resolveClosedOnce()
    }
  }

  async #endDurability(): Promise<void> {
    this.#disposed = true
    this.#forceFinalized = true
    try {
      await this.#durabilityGate.end()
    } catch {
      // Ending remains authoritative even if a dead process group rejects resume.
    } finally {
      try {
        if (process.platform === 'win32') this.#pty.kill()
        else this.#pty.kill('SIGKILL')
      } catch {
        // The process may have already exited while storage was paused.
      }
      await this.#closeJournalIgnoringStorageFault()
      this.#resolveClosedOnce()
    }
  }

  async #closeJournalIgnoringStorageFault(): Promise<void> {
    try {
      await this.#journal.close()
    } catch {
      // Closing must release the file handle even when sync reports the same
      // storage fault that paused this Session.
    }
  }

  #trackWrite(operation: Promise<void>): void {
    this.#writeChain = this.#writeChain.then(
      () => operation,
      () => operation
    ).catch(() => undefined)
  }

  #resolveClosedOnce(): void {
    if (this.#closedResolved) return
    this.#closedResolved = true
    this.#resolveClosed()
  }

  #sendOutput(sequence: number, bytes: Uint8Array): void {
    this.#send?.({
      type: 'terminal.data',
      protocolVersion: PROTOCOL_VERSION,
      sessionId: this.sessionId,
      sequence,
      data: bytes
    })
    this.#creditWindow.recordSent(sequence, bytes.byteLength)
    this.#maximumUnackedBytes = Math.max(this.#maximumUnackedBytes, this.#creditWindow.unackedBytes)
  }

  #sendDurabilityFault(event: SessionDurabilityFaultEvent): void {
    this.#send?.({
      type: 'terminal.storage-fault',
      protocolVersion: PROTOCOL_VERSION,
      sessionId: this.sessionId,
      sequence: event.failedSequence,
      code: storageFaultCode(event.error),
      message: storageFaultMessage(event.error),
      retainedBytes: event.retainedBytes
    })
  }

  #newCreditWindow(): CreditWindow {
    return new CreditWindow({
      highWatermarkBytes: 1024 * 1024,
      lowWatermarkBytes: 512 * 1024,
      onResume: () => this.#scheduleReplay()
    })
  }

  #scheduleReplay(): void {
    if (this.#pendingReplayFrom === undefined) {
      return
    }
    this.#writeChain = this.#writeChain.then(async () => {
      const replayFrom = this.#pendingReplayFrom
      if (replayFrom === undefined) {
        return
      }
      this.#pendingReplayFrom = undefined
      const throughSequence = this.#sequence
      for await (const frame of this.#journal.iterateFrames({
        fromSequence: replayFrom,
        throughSequence
      })) {
        if (this.#creditWindow.isPaused) {
          this.#pendingReplayFrom = frame.sequence
          return
        }
        if (frame.kind === 'output') {
          this.#sendOutput(frame.sequence, frame.data)
        } else if (frame.kind === 'exit') {
          this.#send?.({
            type: 'terminal.exited',
            protocolVersion: PROTOCOL_VERSION,
            sessionId: this.sessionId,
            sequence: frame.sequence,
            exitCode: frame.exitCode,
            ...(frame.signal === undefined ? {} : { signal: frame.signal })
          })
        }
      }
    })
  }
}

function storageFaultCode(error: unknown): StorageFaultCode {
  const code = (error as NodeJS.ErrnoException | undefined)?.code
  if (code === 'ENOSPC' || code === 'EDQUOT') return 'STORAGE_QUOTA_EXCEEDED'
  if (code === 'EROFS' || code === 'EACCES' || code === 'EPERM') return 'STORAGE_READ_ONLY'
  return 'STORAGE_WRITE_FAILED'
}

function storageFaultMessage(error: unknown): string {
  if (error instanceof Error && error.message.trim()) return error.message
  return 'terminal journal write failed'
}

function settlesWithin(promise: Promise<void>, timeoutMs: number): Promise<boolean> {
  return new Promise((resolve) => {
    let settled = false
    const timer = setTimeout(() => {
      if (settled) return
      settled = true
      resolve(false)
    }, Math.max(0, timeoutMs))
    void promise.then(() => {
      if (settled) return
      settled = true
      clearTimeout(timer)
      resolve(true)
    })
  })
}

function resolveShell(): string {
  if (process.platform === 'win32') {
    return process.env.COMSPEC ?? 'powershell.exe'
  }
  return process.env.SHELL ?? (os.platform() === 'darwin' ? '/bin/zsh' : '/bin/bash')
}

function resolveExecutable(profile: 'shell' | 'claude-code' | 'codex'): string {
  if (profile === 'claude-code') {
    return process.env.MATOU_CLAUDE_COMMAND ?? 'claude'
  }
  if (profile === 'codex') {
    return process.env.MATOU_CODEX_COMMAND ?? 'codex'
  }
  return resolveShell()
}

async function readProviderInstructions(controlAssetRoot: string): Promise<string | undefined> {
  try {
    return await readFile(
      join(controlAssetRoot, 'providers', 'codex-developer-instructions.md'),
      'utf8'
    )
  } catch {
    return undefined
  }
}
