import os from 'node:os'

import type { RuntimeMessage } from '@matou/contracts'
import { PROTOCOL_VERSION } from '@matou/contracts'
import * as pty from 'node-pty'

import { CreditWindow } from '../flow-control/credit-window'
import { SegmentJournal } from '../journal/segment-journal'
import { resolvePtyCommand } from './provider-launch-plan'

interface PtySessionOptions {
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
  env?: Record<string, string>
  send: (message: RuntimeMessage) => void
  onExit?: (session: PtySession, exitCode: number, signal?: number) => boolean | void
  onOutput?: (data: string) => void
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
  #send: ((message: RuntimeMessage) => void) | undefined
  #creditWindow: CreditWindow
  readonly #onExit: ((session: PtySession, exitCode: number, signal?: number) => boolean | void) | undefined
  readonly #onOutput: ((data: string) => void) | undefined
  readonly #encoder = new TextEncoder()

  #sequence: number
  #writeChain = Promise.resolve()
  #disposed = false
  #notifyExit = true
  #pendingReplayFrom: number | undefined
  readonly #closed: Promise<void>
  #resolveClosed: () => void = () => {}

  get lastSequence(): number { return this.#sequence }

  private constructor(options: PtySessionOptions, journal: SegmentJournal, terminal: pty.IPty) {
    this.sessionId = options.sessionId
    this.pid = terminal.pid
    this.executionContextId = options.executionContextId
    this.profile = options.profile ?? 'shell'
    this.runId = options.runId
    this.#pty = terminal
    this.#journal = journal
    this.#sequence = journal.lastSequence
    this.replayFromSequence = journal.lastSequence + 1
    this.#send = options.send
    this.#creditWindow = this.#newCreditWindow()
    this.#onExit = options.onExit
    this.#onOutput = options.onOutput
    this.#closed = new Promise<void>((resolve) => { this.#resolveClosed = resolve })

    terminal.onData((data) => this.#enqueueOutput(data))
    terminal.onExit(({ exitCode, signal }) => this.#enqueueExit(exitCode, signal))
  }

  static async create(options: PtySessionOptions): Promise<PtySession> {
    const journal = await SegmentJournal.open(options.dataRoot, options.sessionId)
    const profile = options.profile ?? 'shell'
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
      })
    })
    const terminal = pty.spawn(command.file, command.args, {
      name: 'xterm-256color',
      cols: options.cols,
      rows: options.rows,
      cwd: options.cwd,
      env: { ...process.env, ...options.env, TERM: 'xterm-256color' }
    })
    return new PtySession(options, journal, terminal)
  }

  write(data: string): void {
    if (this.#disposed) {
      throw new Error('session is disposed')
    }
    this.#pty.write(data)
  }

  display(data: string): void {
    if (this.#disposed) return
    this.#enqueueOutput(data)
  }

  resize(cols: number, rows: number): void {
    if (this.#disposed) {
      throw new Error('session is disposed')
    }
    this.#pty.resize(cols, rows)
    const sequence = ++this.#sequence
    this.#writeChain = this.#writeChain.then(() => this.#journal.appendResize(sequence, cols, rows))
  }

  acknowledge(throughSequence: number): void {
    this.#creditWindow.acknowledge(throughSequence)
  }

  readFrames() {
    return this.#writeChain.then(() => this.#journal.readFrames())
  }

  whenClosed(): Promise<void> { return this.#closed }

  attach(send: (message: RuntimeMessage) => void): void {
    this.#send = send
    this.#pendingReplayFrom = undefined
    this.#creditWindow = this.#newCreditWindow()
  }

  detach(send: (message: RuntimeMessage) => void): void {
    if (this.#send === send) {
      this.#send = undefined
      this.#pendingReplayFrom = undefined
      this.#creditWindow = this.#newCreditWindow()
    }
  }

  dispose(options: { notifyExit?: boolean } = {}): void {
    if (this.#disposed) {
      return
    }
    this.#notifyExit = options.notifyExit ?? true
    this.#disposed = true
    this.#pty.kill()
  }

  #enqueueOutput(data: string): void {
    this.#onOutput?.(data)
    const bytes = this.#encoder.encode(data)
    const sequence = ++this.#sequence
    this.#writeChain = this.#writeChain.then(async () => {
      await this.#journal.appendOutput(sequence, bytes)
      if (!this.#send) return
      if (this.#creditWindow.isPaused) {
        this.#pendingReplayFrom ??= sequence
        return
      }
      this.#sendOutput(sequence, bytes)
    })
  }

  #enqueueExit(exitCode: number, signal?: number): void {
    const sequence = ++this.#sequence
    this.#writeChain = this.#writeChain.then(async () => {
      await this.#journal.appendExit(sequence, exitCode, signal)
      await this.#journal.close()
      const allowNotification = this.#onExit?.(this, exitCode, signal) !== false
      this.#resolveClosed()
      if (!this.#notifyExit || !allowNotification) return
      if (!this.#send) {
        return
      }
      if (this.#creditWindow.isPaused) {
        this.#pendingReplayFrom ??= sequence
        return
      }
      const send = this.#send
      send({
        type: 'terminal.exited',
        protocolVersion: PROTOCOL_VERSION,
        sessionId: this.sessionId,
        sequence,
        exitCode,
        ...(signal === undefined ? {} : { signal })
      })
    })
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
      const frames = await this.#journal.readFrames()
      for (const frame of frames) {
        if (frame.sequence < replayFrom) {
          continue
        }
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
