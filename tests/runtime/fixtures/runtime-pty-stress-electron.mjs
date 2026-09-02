import { app, BrowserWindow, MessageChannelMain, utilityProcess } from 'electron'
import { randomUUID } from 'node:crypto'
import { access, mkdir, mkdtemp, readdir, rm, stat, writeFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'

import { PROTOCOL_VERSION } from '../../../packages/contracts/dist/index.js'

let root
let dataRoot
let workspaceRoot
const runtimeEntry = resolve('apps/runtime/dist/index.cjs')
const liveChildren = new Set()

app.whenReady().then(run, fail)

async function run() {
 try {
  root = await mkdtemp('/tmp/matou-pty-stress-')
  dataRoot = join(root, 'data')
  workspaceRoot = join(root, 'workspace')
  app.dock?.hide()
  await mkdir(dataRoot, { recursive: true })
  await mkdir(workspaceRoot, { recursive: true })
  const producers = await writeProducerFixtures(workspaceRoot)

  const firstRuntime = await launchRuntime()
  const firstClient = await firstRuntime.connect()
  const hierarchy = await firstClient.rpc('hierarchy.bootstrap-window', {
    command: command('bootstrap-window'),
    input: {
      windowId: 'pty-stress-window',
      defaultRootDirectory: workspaceRoot,
      defaultName: 'workspace',
      now: Date.now()
    }
  })
  const projection = await firstClient.rpc('projection.snapshot', {
    windowId: 'pty-stress-window'
  })
  const session = projection.sessions.find(({ id }) => id === hierarchy.session?.id) ??
    projection.sessions[0]
  if (!session) throw new Error('Runtime bootstrap did not create a Session')

  const observer = new TerminalObserver(session.id)
  firstClient.observeTerminal(observer)
  firstClient.send({
    type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
    sessionId: session.id, executionContextId: session.executionContextId,
    profile: 'shell', cols: 120, rows: 40
  })
  await firstClient.waitForMessage((message) =>
    message.type === 'terminal.spawned' && message.sessionId === session.id
  )
  firstClient.send({
    type: 'terminal.input', protocolVersion: PROTOCOL_VERSION,
    sessionId: session.id, data: 'stty -echo\r'
  })
  await delay(100)

  const scenario = process.env.MATOU_PTY_STRESS_SCENARIO
  if (scenario === 'single-line') {
    await runCommand(firstClient, session.id,
      `/usr/bin/env node ${shellQuote(producers.singleLine)} ${shellQuote(producers.singleLineDone)}\r`)
    await waitForFile(producers.singleLineDone, 60_000)
    await waitForJournalToSettle(dataRoot, 0, 60_000)
    observer.prepareReplay('single')
    await replayAll(firstClient, session.id)
    observer.finalizeReplay()
    const runtimeExitCode = await firstRuntime.stop()
    await finishResult({
      windowCount: BrowserWindow.getAllWindows().length,
      singleLineBytes: observer.singleLineBytes,
      runtimeExitCode
    })
    return
  }

  if (scenario === 'ansi-states') {
    await runCommand(firstClient, session.id,
      `/usr/bin/env node ${shellQuote(producers.ansiStates)} ${shellQuote(producers.ansiStatesDone)}\r`)
    await waitForFile(producers.ansiStatesDone, 60_000)
    await waitForJournalToSettle(dataRoot, 0, 60_000)
    observer.prepareReplay('ansi')
    await replayAll(firstClient, session.id)
    observer.finalizeReplay()
    const runtimeExitCode = await firstRuntime.stop()
    await finishResult({
      windowCount: BrowserWindow.getAllWindows().length,
      ansiStateChanges: observer.ansiStateChanges,
      runtimeExitCode
    })
    return
  }

  if (scenario !== 'alternate-restart') throw new Error(`Unknown stress scenario: ${scenario}`)

  observer.beginAlternateScreen('less')
  await runCommand(
    firstClient,
    session.id,
    `LESS= /usr/bin/less ${shellQuote(producers.document)}\r`
  )
  await observer.waitForAlternateEnter('less')
  firstClient.input(session.id, 'q')
  await observer.waitForAlternateExit('less')
  firstClient.input(session.id, "printf '__LESS_EXITED__\\n'\r")
  await observer.waitForText('__LESS_EXITED__')

  observer.beginAlternateScreen('vim')
  await runCommand(
    firstClient,
    session.id,
    `/usr/bin/vim -Nu NONE -n ${shellQuote(producers.document)}\r`
  )
  await observer.waitForAlternateEnter('vim')
  firstClient.input(session.id, ':q!\r')
  await observer.waitForAlternateExit('vim')
  firstClient.input(session.id, "printf '__VIM_EXITED__\\n'\r")
  await observer.waitForText('__VIM_EXITED__')

  firstClient.input(session.id, "printf '__BEFORE_RUNTIME_RESTART__\\n'\r")
  await observer.waitForText('__BEFORE_RUNTIME_RESTART__')
  const firstExitCode = await firstRuntime.stop()

  const secondRuntime = await launchRuntime()
  const secondClient = await secondRuntime.connect()
  const replayObserver = new TerminalObserver(session.id)
  secondClient.observeTerminal(replayObserver)
  secondClient.send({
    type: 'terminal.spawn', protocolVersion: PROTOCOL_VERSION,
    sessionId: session.id, executionContextId: session.executionContextId,
    profile: 'shell', cols: 120, rows: 40
  })
  await secondClient.waitForMessage((message) =>
    message.type === 'terminal.spawned' && message.sessionId === session.id
  )
  secondClient.send({
    type: 'terminal.replay-request', protocolVersion: PROTOCOL_VERSION,
    sessionId: session.id, fromSequence: 0
  })
  await secondClient.waitForMessage((message) =>
    message.type === 'terminal.replay-complete' && message.sessionId === session.id,
  60_000)
  const replayedBeforeRestart = replayObserver.sawText('__BEFORE_RUNTIME_RESTART__')

  secondClient.input(session.id, "printf '__AFTER_RUNTIME_RESTART__\\n'\r")
  await replayObserver.waitForText('__AFTER_RUNTIME_RESTART__')
  const secondExitCode = await secondRuntime.stop()

  const result = {
    windowCount: BrowserWindow.getAllWindows().length,
    alternateScreens: observer.alternateScreens,
    runtimeRestart: {
      exitCode: firstExitCode,
      replayedBeforeRestart,
      acceptedInputAfterRestart: replayObserver.sawText('__AFTER_RUNTIME_RESTART__'),
      crashed: firstExitCode !== 0 || secondExitCode !== 0
    }
  }
  await finishResult(result)
 } catch (error) {
  await fail(error)
 }
}

async function finishResult(result) {
  process.stdout.write(`MATOU_PTY_STRESS_RESULT=${JSON.stringify(result)}\n`)
  await rm(root, { recursive: true, force: true })
  app.exit(0)
}

async function replayAll(client, sessionId) {
  const replayOffset = client.checkpoint()
  client.send({
    type: 'terminal.replay-request', protocolVersion: PROTOCOL_VERSION,
    sessionId, fromSequence: 0
  })
  await client.waitForMessage((message) =>
    message.type === 'terminal.replay-complete' && message.sessionId === sessionId,
  60_000, replayOffset)
}

async function fail(error) {
  for (const child of liveChildren) child.kill()
  if (root) await rm(root, { recursive: true, force: true }).catch(() => {})
  process.stderr.write(`${error instanceof Error ? error.stack : String(error)}\n`)
  app.exit(1)
}

async function launchRuntime() {
  const child = utilityProcess.fork(runtimeEntry, [], {
    serviceName: 'Matou Runtime PTY stress fixture',
    stdio: 'pipe',
    env: {
      ...process.env,
      SHELL: '/bin/sh',
      LESS: '',
      MATOU_DATA_DIR: dataRoot,
      MATOU_DEFAULT_WORKSPACE: workspaceRoot
    }
  })
  liveChildren.add(child)
  let diagnostics = ''
  child.stdout?.on('data', (chunk) => {
    diagnostics = `${diagnostics}${chunk}`.slice(-32_768)
  })
  child.stderr?.on('data', (chunk) => {
    diagnostics = `${diagnostics}${chunk}`.slice(-32_768)
  })
  const exited = new Promise((resolveExit) => {
    child.once('exit', (code) => {
      liveChildren.delete(child)
      resolveExit(code ?? 1)
    })
  })
  await waitForEmitter(child, 'spawn', 10_000)
  await waitForRuntimeReady(child, () => diagnostics)
  return {
    connect: () => connectRuntime(child),
    stop: async () => {
      child.kill()
      return withTimeout(exited, 15_000, `Runtime did not stop cleanly\n${diagnostics}`)
    }
  }
}

async function connectRuntime(child) {
  const { port1, port2 } = new MessageChannelMain()
  const client = new RuntimeClient(port2)
  child.postMessage({ type: 'runtime.connect', protocolVersion: PROTOCOL_VERSION }, [port1])
  port2.start()
  client.send({
    type: 'protocol.hello', protocolVersion: PROTOCOL_VERSION,
    clientId: `pty-stress-${randomUUID()}`
  })
  await client.waitForMessage((message) => message.type === 'protocol.ready')
  return client
}

class RuntimeClient {
  #port
  #messages = []
  #terminalObservers = new Set()

  constructor(port) {
    this.#port = port
    port.on('message', ({ data }) => {
      this.#messages.push(data)
      if (data?.type === 'terminal.replay-start') {
        for (const observer of this.#terminalObservers) observer.replayStarted(data.sessionId)
      }
      if (data?.type === 'terminal.data') {
        for (const observer of this.#terminalObservers) observer.ingest(data)
        this.send({
          type: 'terminal.ack', protocolVersion: PROTOCOL_VERSION,
          sessionId: data.sessionId, throughSequence: data.sequence
        })
      }
    })
  }

  observeTerminal(observer) {
    this.#terminalObservers.add(observer)
  }

  checkpoint() {
    return this.#messages.length
  }

  send(message) {
    this.#port.postMessage(message)
  }

  input(sessionId, data) {
    this.send({ type: 'terminal.input', protocolVersion: PROTOCOL_VERSION, sessionId, data })
  }

  async rpc(method, payload) {
    const requestId = `rpc-${randomUUID()}`
    this.send({
      type: 'rpc.request', protocolVersion: PROTOCOL_VERSION,
      requestId, method, capability: 'renderer', deadlineAt: Date.now() + 30_000, payload
    })
    const response = await this.waitForMessage((message) =>
      (message.type === 'rpc.response' || message.type === 'rpc.error') &&
      message.requestId === requestId
    )
    if (response.type === 'rpc.error') {
      throw new Error(`${method}: ${response.code}: ${response.message}`)
    }
    return response.result
  }

  waitForMessage(predicate, timeoutMs = 15_000, after = 0) {
    return waitUntil(() => this.#messages.slice(after).find(predicate), timeoutMs, 'Runtime message timed out')
  }
}

class TerminalObserver {
  sessionId
  singleLineBytes = 0
  ansiStateChanges = 0
  alternateScreens = {
    less: { entered: false, exited: false },
    vim: { entered: false, exited: false }
  }
  #tail = ''
  #singlePhase = 'idle'
  #singleCarry = ''
  #ansiPhase = 'idle'
  #ansiCarry = ''
  #alternateProgram
  #preparedReplay

  constructor(sessionId) {
    this.sessionId = sessionId
  }

  ingest(message) {
    if (message.sessionId !== this.sessionId) return
    const text = Buffer.from(message.data).toString('utf8')
    this.#tail = `${this.#tail}${text}`.slice(-32_768)
    this.#ingestSingle(text)
    this.#ingestAnsi(text)
    if (this.#alternateProgram) {
      const combined = `${this.#alternateProgram.carry}${text}`
      const state = this.alternateScreens[this.#alternateProgram.name]
      state.entered ||= /\u001b\[\?(?:47|1047|1049)h/.test(combined)
      state.exited ||= /\u001b\[\?(?:47|1047|1049)l/.test(combined)
      this.#alternateProgram.carry = combined.slice(-32)
    }
  }

  beginSingleLine() {
    this.singleLineBytes = 0
    this.#singlePhase = 'waiting'
    this.#singleCarry = ''
  }

  beginAnsiStates() {
    this.ansiStateChanges = 0
    this.#ansiPhase = 'waiting'
    this.#ansiCarry = ''
  }

  prepareReplay(kind) {
    this.#preparedReplay = kind
    this.#singlePhase = 'idle'
    this.#ansiPhase = 'idle'
  }

  replayStarted(sessionId) {
    if (sessionId !== this.sessionId) return
    if (this.#preparedReplay === 'single') this.beginSingleLine()
    if (this.#preparedReplay === 'ansi') this.beginAnsiStates()
    this.#preparedReplay = undefined
  }

  finalizeReplay() {
    if (this.#singlePhase === 'counting') {
      this.singleLineBytes += countOccurrences(this.#singleCarry, 'X')
      this.#singleCarry = ''
    }
    if (this.#ansiPhase === 'counting') {
      this.ansiStateChanges += countOccurrences(this.#ansiCarry, 'STATE:')
      this.#ansiCarry = ''
    }
  }

  beginAlternateScreen(name) {
    this.#alternateProgram = { name, carry: '' }
  }

  waitForSingleLine() {
    return waitUntil(
      () => this.#singlePhase === 'done',
      60_000,
      `20 MiB single-line producer stopped at ${this.singleLineBytes} bytes; tail=${JSON.stringify(this.#tail.slice(-2_000))}`
    )
  }

  waitForAnsiStates() {
    return waitUntil(
      () => this.#ansiPhase === 'done',
      60_000,
      `ANSI producer stopped at ${this.ansiStateChanges} changes`
    )
  }

  waitForAlternateEnter(name) {
    return waitUntil(() => this.alternateScreens[name].entered, 10_000, `${name} did not enter alternate screen`)
  }

  waitForAlternateExit(name) {
    return waitUntil(() => this.alternateScreens[name].exited, 10_000, `${name} did not exit alternate screen`)
  }

  waitForText(text) {
    return waitUntil(() => this.sawText(text), 15_000, `terminal did not emit ${text}`)
  }

  sawText(text) {
    return this.#tail.includes(text)
  }

  #ingestSingle(text) {
    if (this.#singlePhase === 'idle' || this.#singlePhase === 'done') return
    let combined = `${this.#singleCarry}${text}`
    if (this.#singlePhase === 'waiting') {
      const start = combined.indexOf('BEGIN_SINGLE')
      if (start < 0) {
        this.#singleCarry = combined.slice(-32)
        return
      }
      combined = combined.slice(start + 'BEGIN_SINGLE'.length)
      this.#singlePhase = 'counting'
    }
    const end = combined.indexOf('END_SINGLE')
    if (end >= 0) {
      this.singleLineBytes += countOccurrences(combined.slice(0, end), 'X')
      this.#singleCarry = ''
      this.#singlePhase = 'done'
      return
    }
    const safeLength = Math.max(0, combined.length - 32)
    this.singleLineBytes += countOccurrences(combined.slice(0, safeLength), 'X')
    this.#singleCarry = combined.slice(safeLength)
  }

  #ingestAnsi(text) {
    if (this.#ansiPhase === 'idle' || this.#ansiPhase === 'done') return
    let combined = `${this.#ansiCarry}${text}`
    if (this.#ansiPhase === 'waiting') {
      const start = combined.indexOf('BEGIN_ANSI')
      if (start < 0) {
        this.#ansiCarry = combined.slice(-32)
        return
      }
      combined = combined.slice(start + 'BEGIN_ANSI'.length)
      this.#ansiPhase = 'counting'
    }
    const end = combined.indexOf('END_ANSI')
    if (end >= 0) {
      this.ansiStateChanges += countOccurrences(combined.slice(0, end), 'STATE:')
      this.#ansiCarry = ''
      this.#ansiPhase = 'done'
      return
    }
    const safeLength = Math.max(0, combined.length - 32)
    this.ansiStateChanges += countOccurrencesBefore(combined, 'STATE:', safeLength)
    this.#ansiCarry = combined.slice(safeLength)
  }
}

async function writeProducerFixtures(directory) {
  const singleLine = join(directory, 'single-line-producer.mjs')
  const singleLineDone = join(directory, 'single-line.done')
  const ansiStates = join(directory, 'ansi-state-producer.mjs')
  const ansiStatesDone = join(directory, 'ansi-states.done')
  const document = join(directory, 'alternate-screen-document.txt')
  await writeFile(singleLine, `
process.stdout.write('BEGIN_SINGLE\\n')
const chunk = Buffer.alloc(64 * 1024, 0x58)
for (let index = 0; index < 320; index += 1) {
  if (!process.stdout.write(chunk)) await new Promise((resolve) => process.stdout.once('drain', resolve))
}
await new Promise((resolve) => process.stdout.write('\\nEND_SINGLE\\n', resolve))
await import('node:fs/promises').then(({ writeFile }) => writeFile(process.argv[2], 'done'))
`)
  await writeFile(ansiStates, `
process.stdout.write('BEGIN_ANSI\\n')
for (let start = 0; start < 100_000; start += 1_000) {
  let batch = ''
  for (let index = start; index < start + 1_000; index += 1) {
    batch += '\\u001b[2K\\rSTATE:' + String(index).padStart(6, '0')
  }
  if (!process.stdout.write(batch)) await new Promise((resolve) => process.stdout.once('drain', resolve))
}
await new Promise((resolve) => process.stdout.write('\\nEND_ANSI\\n', resolve))
await import('node:fs/promises').then(({ writeFile }) => writeFile(process.argv[2], 'done'))
`)
  await writeFile(
    document,
    Array.from({ length: 240 }, (_, index) => `alternate-screen-line-${index + 1}`).join('\n')
  )
  return { singleLine, singleLineDone, ansiStates, ansiStatesDone, document }
}

function command(label) {
  return { commandId: `${label}-${randomUUID()}`, commandType: label, requestHash: label }
}

function shellQuote(value) {
  return `'${value.replaceAll("'", `'\\''`)}'`
}

function runCommand(client, sessionId, data) {
  client.input(sessionId, data)
}

function countOccurrences(value, needle) {
  let count = 0
  let offset = 0
  while ((offset = value.indexOf(needle, offset)) >= 0) {
    count += 1
    offset += needle.length
  }
  return count
}

function countOccurrencesBefore(value, needle, boundary) {
  let count = 0
  let offset = 0
  while ((offset = value.indexOf(needle, offset)) >= 0 && offset < boundary) {
    count += 1
    offset += needle.length
  }
  return count
}

function waitForRuntimeReady(child, diagnostics) {
  return new Promise((resolveReady, reject) => {
    const timeout = setTimeout(() => reject(new Error(`Runtime did not become ready\n${diagnostics()}`)), 15_000)
    const onMessage = (message) => {
      if (message?.type === 'runtime.lifecycle' && message.snapshot?.stage === 'ready') {
        cleanup()
        resolveReady()
      }
    }
    const onExit = (code) => {
      cleanup()
      reject(new Error(`Runtime exited before ready with code ${code}\n${diagnostics()}`))
    }
    const cleanup = () => {
      clearTimeout(timeout)
      child.off('message', onMessage)
      child.off('exit', onExit)
    }
    child.on('message', onMessage)
    child.once('exit', onExit)
  })
}

function waitForEmitter(emitter, event, timeoutMs) {
  return withTimeout(new Promise((resolveEvent, reject) => {
    emitter.once(event, resolveEvent)
    emitter.once('error', reject)
  }), timeoutMs, `${event} event timed out`)
}

async function waitUntil(read, timeoutMs, message) {
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    const value = await read()
    if (value) return value
    await delay(10)
  }
  throw new Error(message)
}

function withTimeout(promise, timeoutMs, message) {
  return Promise.race([
    promise,
    new Promise((_, reject) => setTimeout(() => reject(new Error(message)), timeoutMs))
  ])
}

function delay(ms) {
  return new Promise((resolveDelay) => setTimeout(resolveDelay, ms))
}

function waitForFile(path, timeoutMs) {
  return waitUntil(async () => {
    try {
      await access(path)
      return true
    } catch {
      return false
    }
  }, timeoutMs, `producer did not create ${path}`)
}

async function waitForJournalToSettle(root, minimumBytes, timeoutMs) {
  const deadline = Date.now() + timeoutMs
  let previousBytes = -1
  let stableSince = 0
  while (Date.now() < deadline) {
    const bytes = await directoryBytes(join(root, 'journal'))
    if (bytes >= minimumBytes && bytes === previousBytes) {
      stableSince ||= Date.now()
      if (Date.now() - stableSince >= 2_000) return
    } else {
      stableSince = 0
    }
    previousBytes = bytes
    await delay(50)
  }
  throw new Error(`Journal did not settle above ${minimumBytes} bytes; last size was ${previousBytes}`)
}

async function directoryBytes(path) {
  let total = 0
  for (const entry of await readdir(path, { withFileTypes: true })) {
    const child = join(path, entry.name)
    total += entry.isDirectory() ? await directoryBytes(child) : (await stat(child)).size
  }
  return total
}
