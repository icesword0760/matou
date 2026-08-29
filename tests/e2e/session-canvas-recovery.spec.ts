import { expect, test } from '@playwright/test'

import { restartMatou } from './matou-fixture'
import { activeSurface, launchSessionCanvas, terminalCommand } from './fixtures/session-canvas-fixture'

test.describe('session recovery uses real process state', () => {
  test.setTimeout(90_000)

  test('marks an unfinished command as interrupted after restart without executing it a second time', async () => {
    let fixture = await launchSessionCanvas()
    try {
      const marker = `${fixture.rootDirectory}/command-count.txt`
      await terminalCommand(activeSurface(fixture.page),
        `printf x >> '${marker}'; printf 'LONG_COMMAND_STARTED\\n'; sleep 30; printf y >> '${marker}'`)
      await expect(activeSurface(fixture.page).locator('.xterm-rows')).toContainText('LONG_COMMAND_STARTED')
      const { readFile } = await import('node:fs/promises')
      await expect.poll(() => readFile(marker, 'utf8').catch(() => '')).toBe('x')
      fixture = await restartMatou(fixture)
      expect(await readFile(marker, 'utf8')).toBe('x')
      await expect(activeSurface(fixture.page).locator('.xterm-rows')).toContainText('上次命令已中断')
    } finally {
      await fixture.close()
    }
  })
})
