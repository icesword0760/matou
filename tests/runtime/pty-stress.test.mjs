import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { createRequire } from 'node:module'
import { fileURLToPath } from 'node:url'
import test from 'node:test'

const require = createRequire(import.meta.url)
const electron = require('electron')
const fixture = fileURLToPath(new URL('./fixtures/runtime-pty-stress-electron.mjs', import.meta.url))

test('real Runtime PTY retains a 20 MiB single line without crashing', {
  timeout: 120_000
}, async () => {
  const result = await runElectronFixture('single-line')

  assert.equal(result.windowCount, 0, 'the headless Runtime fixture must not create a visible window')
  assert.equal(result.runtimeExitCode, 0)
  assert.equal(result.singleLineBytes, 20 * 1024 * 1024)
})

test('real Runtime PTY retains 100,000 ANSI status changes without crashing', {
  timeout: 120_000
}, async () => {
  const result = await runElectronFixture('ansi-states')

  assert.equal(result.windowCount, 0, 'the headless Runtime fixture must not create a visible window')
  assert.equal(result.runtimeExitCode, 0)
  assert.equal(result.ansiStateChanges, 100_000)
})

test('real less and vim alternate screens exit and recover across a Runtime restart', {
  timeout: 120_000
}, async () => {
  const result = await runElectronFixture('alternate-restart')

  assert.equal(result.windowCount, 0, 'the headless Runtime fixture must not create a visible window')
  assert.deepEqual(result.alternateScreens, {
    less: { entered: true, exited: true },
    vim: { entered: true, exited: true }
  })
  assert.equal(result.runtimeRestart.exitCode, 0)
  assert.equal(result.runtimeRestart.replayedBeforeRestart, true)
  assert.equal(result.runtimeRestart.acceptedInputAfterRestart, true)
  assert.equal(result.runtimeRestart.crashed, false)
})

test('six real PTYs keep 320 MiB histories compressed, searchable, and interactive', {
  timeout: 12 * 60_000
}, async () => {
  const result = await runElectronFixture('multi-session-history')
  console.log(`[multi-session-history-baseline] ${JSON.stringify(result)}`)

  assert.equal(result.windowCount, 0, 'the headless Runtime fixture must not create a visible window')
  assert.equal(result.runtimeExitCode, 0)
  assert.equal(result.historySessionCount, 6)
  assert.equal(result.bytesPerHistorySession, 320 * 1024 * 1024)
  assert.equal(result.logicalHistoryBytesBySession.length, 6)
  assert.equal(result.coldCompressedLogicalBytesBySession.length, 6)
  for (const bytes of result.logicalHistoryBytesBySession) {
    assert.ok(bytes >= 320 * 1024 * 1024, `history retained only ${bytes} logical bytes`)
  }
  for (const bytes of result.coldCompressedLogicalBytesBySession) {
    assert.ok(bytes >= 64 * 1024 * 1024, `cold compressed history retained only ${bytes} logical bytes`)
  }
  // Gate sustained Runtime responsiveness here; the single-sample maximum is
  // retained in the result for diagnosis because host scheduling and GC make
  // it noisy over this 60-second workload. The probe below gates user input.
  assert.ok(
    result.compressionEventLoopDelayP99Ms < 50,
    `Runtime sustained event-loop delay p99 was ${result.compressionEventLoopDelayP99Ms}ms ` +
      `(single-sample max ${result.compressionEventLoopDelayMaxMs}ms)`
  )
  assert.ok(
    result.firstPagePeakRssDeltaBytes < 64 * 1024 * 1024,
    `first history page grew Runtime RSS by ${result.firstPagePeakRssDeltaBytes} bytes`
  )
  assert.ok(
    result.probeInputP95Ms < 100,
    `sibling terminal input p95 was ${result.probeInputP95Ms}ms`
  )
  assert.equal(result.firstPageLineCount, 100)
  assert.equal(result.probeInputSamples >= 20, true)
})

function runElectronFixture(scenario) {
  return new Promise((resolve, reject) => {
    const child = spawn(electron, [fixture], {
      cwd: fileURLToPath(new URL('../..', import.meta.url)),
      env: {
        ...process.env,
        ELECTRON_DISABLE_SECURITY_WARNINGS: 'true',
        MATOU_PTY_STRESS_SCENARIO: scenario
      },
      stdio: ['ignore', 'pipe', 'pipe']
    })
    let stdout = ''
    let stderr = ''
    child.stdout.setEncoding('utf8')
    child.stderr.setEncoding('utf8')
    child.stdout.on('data', (chunk) => { stdout += chunk })
    child.stderr.on('data', (chunk) => { stderr += chunk })
    child.on('error', reject)
    child.on('exit', (code, signal) => {
      if (code !== 0) {
        reject(new Error(
          `headless Electron fixture exited with code ${code} signal ${signal}\n${stderr}\n${stdout}`
        ))
        return
      }
      const resultLine = stdout.split('\n').find((line) => line.startsWith('MATOU_PTY_STRESS_RESULT='))
      if (!resultLine) {
        reject(new Error(`headless Electron fixture emitted no result\n${stderr}\n${stdout}`))
        return
      }
      resolve(JSON.parse(resultLine.slice('MATOU_PTY_STRESS_RESULT='.length)))
    })
  })
}
