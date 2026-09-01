import { access, mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

import { _electron as electron, expect, test } from '@playwright/test'

import { launchMatou } from './matou-fixture'

test('streams PTY output from UtilityProcess to xterm over a transferred MessagePort', async () => {
  const dataDirectory = await mkdtemp(join(tmpdir(), 'matou-e2e-'))
  const app = await electron.launch({
    args: [resolve(import.meta.dirname, '../../apps/desktop')],
    env: {
      ...process.env,
      MATOU_E2E: '1',
      MATOU_DATA_DIR: dataDirectory,
      MATOU_RUNTIME_ENTRY: resolve(import.meta.dirname, '../../apps/runtime/dist/index.cjs')
    }
  })
  app.process().stdout?.on('data', (data) => process.stdout.write(`[electron:stdout] ${String(data)}`))
  app.process().stderr?.on('data', (data) => process.stderr.write(`[electron:stderr] ${String(data)}`))

  try {
    const page = await app.firstWindow()
    page.on('console', (message) => console.log(`[renderer:${message.type()}] ${message.text()}`))
    page.on('pageerror', (error) => console.error(`[renderer:error] ${error.message}`))

    await expect(page.getByTestId('runtime-status')).toHaveText('streaming')
    await expect(page.getByTestId('smoke-marker')).toHaveText('__MATOU_CHANNEL_READY__')
    await expect(page.getByTestId('replay-marker')).toHaveText(/^replayed-through:\d+$/)
    await page.waitForTimeout(200)
    await expect(page.getByTestId('runtime-status')).toHaveText('streaming')
  } finally {
    await app.close()
    await rm(dataDirectory, { recursive: true, force: true })
  }
})

test('drops reference-visible paths as safe single argv without executing them', async () => {
  const dataDirectory = await mkdtemp(join(tmpdir(), 'matou-e2e-'))
  const dropDirectory = join('/tmp', `matou-drop-e2e-${process.pid}-${Date.now()}`)
  const sideEffectPath = join(dropDirectory, 'PWNED')
  const tickSideEffectPath = join(dropDirectory, 'PWNED_TICK')
  const names = [
    'plain.txt',
    'with space.txt',
    "single'quote.txt",
    'command$(touch PWNED).txt',
    'back\\slash.txt',
    'backtick`touch PWNED_TICK`.txt',
    'line\nbreak.txt',
    '你好🚀.txt'
  ]
  await mkdir(dropDirectory, { recursive: true })
  await Promise.all(names.map((name) => writeFile(join(dropDirectory, name), 'fixture')))

  const app = await electron.launch({
    args: [resolve(import.meta.dirname, '../../apps/desktop')],
    env: {
      ...process.env,
      MATOU_E2E: '1',
      MATOU_DATA_DIR: dataDirectory,
      MATOU_RUNTIME_ENTRY: resolve(import.meta.dirname, '../../apps/runtime/dist/index.cjs')
    }
  })

  try {
    const page = await app.firstWindow()
    await expect(page.getByTestId('runtime-status')).toHaveText('streaming')
    const surface = page.locator(
      '.scene-stage:not([hidden]) [data-testid="terminal-pane"][data-active="true"] .terminal-surface'
    )
    await expect(surface).toHaveAttribute('data-pid', /[1-9][0-9]*/)
    const textarea = surface.locator('.xterm-helper-textarea')
    await textarea.focus()

    await page.keyboard.type(`cd ${dropDirectory} && printf '__DROP_CWD_READY__\\n'`)
    await page.keyboard.press('Enter')
    await expect.poll(() => terminalText(surface)).toContain('__DROP_CWD_READY__')

    const ordinaryPath = join(dropDirectory, names[0]!)
    await dispatchStructuredPathDrop(surface, [ordinaryPath], 'echo TEXT_PLAIN_MUST_BE_IGNORED')
    await expect.poll(() => terminalText(surface)).toContain(` ${ordinaryPath}`)
    expect(await terminalText(surface)).not.toContain('TEXT_PLAIN_MUST_BE_IGNORED')
    await page.keyboard.press('Control+C')

    const spacedPath = join(dropDirectory, names[1]!)
    await dispatchStructuredPathDrop(surface, [spacedPath], 'echo TEXT_PLAIN_MUST_BE_IGNORED')
    await expect.poll(() => terminalText(surface)).toContain(` "${spacedPath}"`)
    await page.keyboard.press('Control+C')

    for (const name of names.slice(2)) {
      const path = join(dropDirectory, name)
      const expected = Buffer.from(path).toString('base64')
      await page.keyboard.type(
        `python3 -c 'import base64,sys; print("__ARGV__"+base64.b64encode(sys.argv[-1].encode()).decode())' --`
      )
      await dispatchStructuredPathDrop(surface, [path], 'echo TEXT_PLAIN_MUST_BE_IGNORED')
      await page.keyboard.press('Enter')
      await expect.poll(() => terminalText(surface)).toContain(`__ARGV__${expected}`)
    }

    await expect(access(sideEffectPath).then(() => true, () => false)).resolves.toBe(false)
    await expect(access(tickSideEffectPath).then(() => true, () => false)).resolves.toBe(false)
  } finally {
    await app.close()
    await rm(dataDirectory, { recursive: true, force: true })
    await rm(dropDirectory, { recursive: true, force: true })
  }
})

async function dispatchStructuredPathDrop(
  surface: import('@playwright/test').Locator,
  paths: string[],
  plainText: string
): Promise<void> {
  await surface.evaluate((element, payload) => {
    const transfer = new DataTransfer()
    transfer.setData('application/x-file-tree-nodes', JSON.stringify(
      payload.paths.map((path) => ({ path, name: path.split('/').at(-1), type: 'file' }))
    ))
    transfer.setData('text/plain', payload.plainText)
    element.dispatchEvent(new DragEvent('drop', {
      bubbles: true,
      cancelable: true,
      dataTransfer: transfer
    }))
  }, { paths, plainText })
}

async function terminalText(surface: import('@playwright/test').Locator): Promise<string> {
  return (await surface.locator('.xterm-rows').innerText()).replace(/\r?\n/g, '')
}

test('transparently chunks a large UTF-8 paste into one continuous PTY input', async () => {
  test.setTimeout(60_000)
  const fixture = await launchMatou()

  try {
    const { page } = fixture
    page.on('console', (message) => console.log(`[renderer:${message.type()}] ${message.text()}`))
    page.on('pageerror', (error) => console.error(`[renderer:error] ${error.message}`))

    const surface = page.locator(
      '.scene-stage:not([hidden]) [data-testid="terminal-pane"][data-active="true"] .terminal-surface'
    )
    await expect(surface).toHaveAttribute('data-pid', /[1-9][0-9]*/)
    const textarea = surface.locator('.xterm-helper-textarea')
    const rows = surface.locator('.xterm-rows')
    await textarea.focus()
    await pasteIntoTerminal(textarea, [
      "python3 -c 'import sys,termios; a=termios.tcgetattr(0); b=a[:];",
      ' b[3]&=~(termios.ECHO|termios.ICANON); b[6][termios.VMIN]=1; b[6][termios.VTIME]=0;',
      ' termios.tcsetattr(0,termios.TCSANOW,b);',
      ' print(bytes.fromhex("52454144595f464f525f4c415247455f494e505554").decode(),flush=True);',
      ' d=sys.stdin.buffer.readline();',
      ' termios.tcsetattr(0,termios.TCSANOW,a);',
      ' print("LARGE_INPUT_RESULT",len(d),d[-8:].hex(),flush=True)\''
    ].join(''))
    await textarea.press('Enter')
    await expect(rows).toContainText('READY_FOR_LARGE_INPUT')

    const payloadByteLength = Math.floor(2.5 * 1024 * 1024)
    const unicodeCore = '中文🙂e\u0301'.repeat(4096)
    const suffix = 'TAILEND!'
    const unicodeBytes = Buffer.byteLength(unicodeCore)
    const payload = `${'x'.repeat(payloadByteLength - unicodeBytes - suffix.length)}${unicodeCore}${suffix}`
    const expectedLine = Buffer.from(`${payload}\n`)
    const expectedTailHex = expectedLine.subarray(-8).toString('hex')

    await pasteIntoTerminal(textarea, payload)
    await textarea.press('Enter')

    await expect(rows).toContainText(
      `LARGE_INPUT_RESULT ${payloadByteLength + 1} ${expectedTailHex}`,
      { timeout: 30_000 }
    )
    await expect(page.locator([
      '.reference-toast:visible',
      '[role="dialog"]:visible',
      '[role="alertdialog"]:visible',
      '.provider-work-failure-banner:visible'
    ].join(', '))).toHaveCount(0)
  } finally {
    await fixture.close()
  }
})

async function pasteIntoTerminal(
  textarea: import('@playwright/test').Locator,
  value: string
): Promise<void> {
  await textarea.evaluate((element, text) => {
    const clipboard = new DataTransfer()
    clipboard.setData('text/plain', text)
    element.dispatchEvent(new ClipboardEvent('paste', {
      bubbles: true,
      cancelable: true,
      clipboardData: clipboard
    }))
  }, value)
}
