import { mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

import { _electron as electron, expect, test } from '@playwright/test'

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
