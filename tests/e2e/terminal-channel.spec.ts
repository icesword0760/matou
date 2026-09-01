import { access, mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { pathToFileURL } from 'node:url'

import {
  _electron as electron, expect, test, type CDPSession, type Locator
} from '@playwright/test'

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
    'command$(touch$IFS$MATOU_DROP_SIDE_EFFECT).txt',
    'back\\slash.txt',
    'backtick`touch$IFS$MATOU_DROP_TICK_SIDE_EFFECT`.txt',
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
      MATOU_DROP_SIDE_EFFECT: sideEffectPath,
      MATOU_DROP_TICK_SIDE_EFFECT: tickSideEffectPath,
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

    const ordinaryPath = join(dropDirectory, names[0]!)
    await dispatchStructuredPathDrop(surface, [ordinaryPath], 'echo TEXT_PLAIN_MUST_BE_IGNORED')
    await expect.poll(() => terminalText(surface)).toContain(` ${ordinaryPath}`)
    expect(await terminalText(surface)).not.toContain('TEXT_PLAIN_MUST_BE_IGNORED')
    await clearTerminalInput(textarea, surface, ordinaryPath)

    const spacedPath = join(dropDirectory, names[1]!)
    await dispatchStructuredPathDrop(surface, [spacedPath], 'echo TEXT_PLAIN_MUST_BE_IGNORED')
    await expect.poll(() => terminalText(surface)).toContain(` "${spacedPath}"`)
    await clearTerminalInput(textarea, surface, spacedPath)

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

test('drops native files and directories through Electron webUtils as exact zsh argv', async () => {
  const dataDirectory = await mkdtemp(join(tmpdir(), 'matou-e2e-'))
  const dropDirectory = await mkdtemp(join(tmpdir(), 'matou-native-drop-'))
  const sideEffectPath = join(dropDirectory, 'NATIVE_SIDE_EFFECT')
  const ordinaryPath = join(dropDirectory, 'plain.txt')
  const spacedDirectory = join(dropDirectory, 'with space')
  const specialPath = join(
    dropDirectory,
    'command$(touch$IFS$MATOU_NATIVE_DROP_SIDE_EFFECT).txt'
  )
  const unicodePath = join(dropDirectory, '你好🚀.txt')
  await writeFile(ordinaryPath, 'fixture')
  await mkdir(spacedDirectory)
  await writeFile(specialPath, 'fixture')
  await writeFile(unicodePath, 'fixture')

  const app = await electron.launch({
    args: [resolve(import.meta.dirname, '../../apps/desktop')],
    env: {
      ...process.env,
      MATOU_E2E: '1',
      MATOU_DATA_DIR: dataDirectory,
      MATOU_NATIVE_DROP_SIDE_EFFECT: sideEffectPath,
      MATOU_RUNTIME_ENTRY: resolve(import.meta.dirname, '../../apps/runtime/dist/index.cjs')
    }
  })

  try {
    const page = await app.firstWindow()
    await expect(page.getByTestId('runtime-status')).toHaveText('streaming')
    const surface = activeTerminalSurface(page)
    await expect(surface).toHaveAttribute('data-pid', /[1-9][0-9]*/)
    const textarea = surface.locator('.xterm-helper-textarea')
    await textarea.focus()

    const paths = [ordinaryPath, spacedDirectory, specialPath, unicodePath]
    const expected = Buffer.from(JSON.stringify(paths)).toString('base64')
    await page.keyboard.type([
      "python3 -c 'import base64,json,sys;",
      ' print("__NATIVE_ARGV__"+base64.b64encode(json.dumps(sys.argv[2:],ensure_ascii=False,separators=(",", ":")).encode()).decode())\' --'
    ].join(''))

    await dispatchNativeFileDrop(page, surface, paths)

    await expect(access(sideEffectPath).then(() => true, () => false)).resolves.toBe(false)
    expect(await terminalText(surface)).not.toContain('__NATIVE_ARGV__' + expected)
    await page.keyboard.press('Enter')
    await expect.poll(() => terminalText(surface)).toContain(`__NATIVE_ARGV__${expected}`)
    await expect(access(sideEffectPath).then(() => true, () => false)).resolves.toBe(false)

    await page.keyboard.type('printf URI_ONLY_SENTINEL')
    await expect.poll(() => terminalText(surface)).toContain('URI_ONLY_SENTINEL')
    const beforeUriDrop = await terminalText(surface)
    await dispatchUriOnlyDrop(page, surface, pathToFileURL(spacedDirectory).href)
    await page.waitForTimeout(100)
    expect(await terminalText(surface)).toBe(beforeUriDrop)
    await page.keyboard.press('Control+C')
  } finally {
    await app.close()
    await rm(dataDirectory, { recursive: true, force: true })
    await rm(dropDirectory, { recursive: true, force: true })
  }
})

async function dispatchStructuredPathDrop(
  surface: Locator,
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

async function dispatchNativeFileDrop(
  page: import('@playwright/test').Page,
  surface: Locator,
  paths: string[]
): Promise<void> {
  const session = await page.context().newCDPSession(page)
  try {
    await dispatchCdpDrop(session, surface, { items: [], files: paths, dragOperationsMask: 1 })
  } finally {
    await session.detach()
  }
}

async function dispatchUriOnlyDrop(
  page: import('@playwright/test').Page,
  surface: Locator,
  uri: string
): Promise<void> {
  const session = await page.context().newCDPSession(page)
  try {
    await dispatchCdpDrop(session, surface, {
      items: [{ mimeType: 'text/uri-list', data: uri }], dragOperationsMask: 1
    })
  } finally {
    await session.detach()
  }
}

async function dispatchCdpDrop(
  session: CDPSession,
  surface: Locator,
  data: {
    items: Array<{ mimeType: string; data: string }>
    files?: string[]
    dragOperationsMask: number
  }
): Promise<void> {
  const box = await surface.boundingBox()
  if (!box) throw new Error('active terminal surface has no bounding box')
  const x = box.x + box.width / 2
  const y = box.y + box.height / 2
  await session.send('Input.dispatchDragEvent', { type: 'dragEnter', x, y, data })
  await session.send('Input.dispatchDragEvent', { type: 'dragOver', x, y, data })
  await session.send('Input.dispatchDragEvent', { type: 'drop', x, y, data })
}

function activeTerminalSurface(page: import('@playwright/test').Page): Locator {
  return page.locator(
    '.scene-stage:not([hidden]) [data-testid="terminal-pane"][data-active="true"] .terminal-surface'
  )
}

async function terminalText(surface: import('@playwright/test').Locator): Promise<string> {
  return (await surface.locator('.xterm-rows').innerText()).replace(/\r?\n/g, '')
}

async function clearTerminalInput(
  textarea: Locator,
  surface: Locator,
  visiblePath: string
): Promise<void> {
  await textarea.focus()
  await textarea.press('Control+U')
  await expect.poll(() => terminalText(surface)).not.toContain(visiblePath)
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
