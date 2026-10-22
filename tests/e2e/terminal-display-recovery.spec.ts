import { writeFile } from 'node:fs/promises'
import { expect, test, type Locator } from '@playwright/test'

import { launchMatou } from './matou-fixture'

test('rebuilds a failed real xterm parser and resumes input/output on the same PTY', async ({}, testInfo) => {
  test.setTimeout(60_000)
  const fixture = await launchMatou()
  try {
    const { page } = fixture
    const surface = page.locator('.scene-stage:not([hidden]) .terminal-surface').first()
    await expect(surface).toHaveAttribute('data-pid', /[1-9][0-9]*/)
    const pid = await surface.getAttribute('data-pid')
    const sessionId = await surface.getAttribute('data-session-id')
    await command(surface, printCommand('BEFORE_DISPLAY_FAILURE'))
    await expect(surface.locator('.xterm-rows')).toContainText('BEFORE_DISPLAY_FAILURE')
    const errors: string[] = []
    page.on('pageerror', (error) => errors.push(error.message))
    const started = Date.now()
    await command(surface, printCommand('OUTPUT_AFTER_DISPLAY_FAILURE')
      .replace('import base64;', 'import base64,time; time.sleep(1);'))
    await surface.evaluate((element) => {
      // Fault-inject the real renderer parser. No production diagnostic API is
      // needed: the component's terminal ref is reachable through its React hook.
      type Writer = { _action: (...args: unknown[]) => unknown; _didUserInput: boolean }
      type Hook = { memoizedState?: { current?: { _core?: { _writeBuffer?: Writer } } }; next?: Hook }
      type Fiber = { memoizedState?: Hook; return?: Fiber }
      const key = Object.keys(element).find((candidate) => candidate.startsWith('__reactFiber$'))!
      let fiber = (element as unknown as Record<string, Fiber>)[key]
      while (fiber) {
        let hook = fiber.memoizedState
        while (hook) {
          const writer = hook.memoizedState?.current?._core?._writeBuffer
          if (writer) {
            const original = writer._action
            writer._didUserInput = false
            writer._action = (...args: unknown[]) => {
              writer._action = original
              void args
              throw new RangeError('MATOU_TEST_PARSER_ALLOCATION_FAILURE')
            }
            return
          }
          hook = hook.next
        }
        fiber = fiber.return
      }
      throw new Error('Real terminal writer was not found')
    })
    await expect(surface.locator('.xterm-rows')).toContainText('OUTPUT_AFTER_DISPLAY_FAILURE')
    await expect(surface).toHaveAttribute('data-pid', pid!)
    await expect(surface).toHaveAttribute('data-session-id', sessionId!)
    await expect(surface.locator('.xterm-helper-textarea')).toBeFocused()
    await command(surface, printCommand('INPUT_AND_OUTPUT_RECOVERED'))
    await expect(surface.locator('.xterm-rows')).toContainText('INPUT_AND_OUTPUT_RECOVERED')
    expect(errors.some((error) => error.includes('MATOU_TEST_PARSER_ALLOCATION_FAILURE'))).toBe(true)
    expect(errors.filter((error) => !error.includes('MATOU_TEST_PARSER_ALLOCATION_FAILURE'))).toEqual([])
    await fixture.app.evaluate(async ({ app, BrowserWindow }) => {
      const main = BrowserWindow.getAllWindows()[0]!
      const bounds = main.getBounds()
      const sink = new BrowserWindow({ x: bounds.x + 30, y: bounds.y + 30, width: 240, height: 160, show: true, title: 'Matou focus verification' })
      await sink.loadURL('data:text/html,<title>Matou focus verification</title>Focus verification')
      app.focus({ steal: true })
      sink.focus()
    })
    await expect.poll(() => fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()
      .find((window) => window.getTitle() === 'Matou focus verification')?.isFocused())).toBe(true)
    await fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()
      .find((window) => window.getTitle() !== 'Matou focus verification')?.focus())
    await expect(surface.locator('.xterm-helper-textarea')).toBeFocused()
    await command(surface, printCommand('INPUT_AFTER_NATIVE_FOCUS_RETURN'))
    await expect(surface.locator('.xterm-rows')).toContainText('INPUT_AFTER_NATIVE_FOCUS_RETURN')
    await fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()
      .find((window) => window.getTitle() === 'Matou focus verification')?.destroy())
    expect(errors.filter((error) => !error.includes('MATOU_TEST_PARSER_ALLOCATION_FAILURE'))).toEqual([])
    await page.screenshot({ path: testInfo.outputPath('display-recovered.png') })
    await writeFile(testInfo.outputPath('display-recovery-proof.json'), JSON.stringify({
      sessionId, pid, recoveryMs: Date.now() - started, injectedErrors: errors,
      output: await surface.locator('.xterm-rows').innerText()
    }, null, 2))
  } finally {
    await fixture.close()
  }
})

test('bounds replay backing buffers in the real Electron MessagePort and keeps history usable', async ({}, testInfo) => {
  test.setTimeout(60_000)
  const fixture = await launchMatou()
  try {
    const { page } = fixture
    await page.addInitScript(() => {
      const probe = { frames: 0, payloadBytes: 0, backingBytes: 0, inflated: 0, replaying: false }
      Object.assign(window, { __matouReplayMemory: probe })
      const observed = new WeakSet<MessagePort>()
      const original = MessagePort.prototype.postMessage
      MessagePort.prototype.postMessage = function (message, options?: StructuredSerializeOptions) {
        if (!observed.has(this)) {
          observed.add(this)
          this.addEventListener('message', ({ data }) => {
            if (data?.type === 'terminal.replay-start') probe.replaying = true
            if (data?.type === 'terminal.replay-complete') probe.replaying = false
            if (probe.replaying && data?.type === 'terminal.data' && data.data instanceof Uint8Array) {
              probe.frames += 1
              probe.payloadBytes += data.data.byteLength
              probe.backingBytes += data.data.buffer.byteLength
              if (data.data.buffer.byteLength !== data.data.byteLength) probe.inflated += 1
            }
          })
        }
        // Exercise journal replay rather than bypassing it with a fresh snapshot.
        if (message?.type === 'terminal.checkpoint') return
        if (options === undefined) original.call(this, message)
        else original.call(this, message, options)
      }
    })
    await page.reload()
    const surface = page.locator('.scene-stage:not([hidden]) .terminal-surface').first()
    await expect(surface).toHaveAttribute('data-pid', /[1-9][0-9]*/)
    const pid = await surface.getAttribute('data-pid')
    await command(surface, 'python3 -u -c "import time; [(print(\'history-%04d \'%i+\'x\'*80), time.sleep(.003)) for i in range(600)]; print(\'REPLAY_HISTORY_DONE\')"')
    await expect(surface.locator('.xterm-rows')).toContainText('history-0599')
    await page.reload()
    await expect(surface.locator('.xterm-rows')).toContainText('history-0599')
    await expect(surface).toHaveAttribute('data-pid', pid!)
    const readProbe = () => page.evaluate(() => (window as unknown as {
      __matouReplayMemory: { frames: number; payloadBytes: number; backingBytes: number; inflated: number; replaying: boolean }
    }).__matouReplayMemory)
    await expect.poll(async () => (await readProbe()).replaying).toBe(false)
    const memory = await readProbe()
    expect(memory.frames).toBeGreaterThan(100)
    expect(memory.payloadBytes).toBeGreaterThan(45_000)
    expect(memory.backingBytes).toBe(memory.payloadBytes)
    expect(memory.inflated).toBe(0)
    await command(surface, printCommand('INPUT_AFTER_HISTORY_REPLAY'))
    await expect(surface.locator('.xterm-rows')).toContainText('INPUT_AFTER_HISTORY_REPLAY')
    await page.screenshot({ path: testInfo.outputPath('history-replayed.png') })
    await writeFile(testInfo.outputPath('replay-memory-proof.json'), JSON.stringify(memory, null, 2))
  } finally {
    await fixture.close()
  }
})

async function command(surface: Locator, text: string): Promise<void> {
  const input = surface.locator('.xterm-helper-textarea')
  await input.focus()
  await input.pressSequentially(text)
  await input.press('Enter')
}

function printCommand(marker: string): string {
  // The marker must come from the process output, not the echoed command text.
  return `python3 -c "import base64; print(base64.b64decode('${Buffer.from(marker).toString('base64')}').decode())"`
}
