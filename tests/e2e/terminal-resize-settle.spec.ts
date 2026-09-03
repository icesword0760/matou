import { chmod, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { join } from 'node:path'

import { expect, test } from '@playwright/test'

import {
  activeSurface, launchSessionCanvas, terminalCommand, visibleSurfaces, waitForShell
} from './fixtures/session-canvas-fixture'
import { launchMatou } from './matou-fixture'

test('sends one real PTY resize after a focused card width transition settles', async () => {
  test.setTimeout(60_000)
  const fixture = await launchSessionCanvas()
  try {
    await waitForShell(activeSurface(fixture.page))
    await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
    await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
    await expect(visibleSurfaces(fixture.page)).toHaveCount(3)

    const first = visibleSurfaces(fixture.page).first()
    const last = visibleSurfaces(fixture.page).last()
    await waitForShell(first)
    await waitForShell(last)

    const resizeLog = join(fixture.rootDirectory, 'terminal-resize.log')
    await terminalCommand(
      first,
      `: > '${resizeLog}'; trap 'printf "WINCH\\n" >> "${resizeLog}"' WINCH; printf 'RESIZE_TRAP_READY\\n'`
    )
    await expect(first.locator('.xterm-rows')).toContainText('RESIZE_TRAP_READY')
    await fixture.page.waitForTimeout(700)
    await writeFile(resizeLog, '')

    await last.click({ position: { x: 24, y: 90 } })
    await expect(last.locator('xpath=ancestor::*[@data-testid="terminal-pane"][1]'))
      .toHaveAttribute('data-active', 'true')
    await fixture.page.waitForTimeout(700)

    const resizeSignals = (await readFile(resizeLog, 'utf8'))
      .split('\n')
      .filter(Boolean)
    expect(resizeSignals).toEqual(['WINCH'])
  } finally {
    await fixture.close()
  }
})

test('starts cc at the settled card size without a failure banner and preserves Claude color', async () => {
  test.setTimeout(60_000)
  const root = await mkdtemp('/tmp/matou-e2e-cc-promotion-')
  const provider = join(root, 'claude-color-fixture.sh')
  const sizeFile = join(root, 'provider-size.txt')
  const environmentFile = join(root, 'provider-environment.txt')
  await writeFile(join(root, '.zshrc'), [
    "alias cc='claude --dangerously-skip-permissions'",
    "trap 'sleep 0.35; exit' HUP TERM",
    ''
  ].join('\n'))
  await writeFile(provider, `#!/bin/sh
stty size > "$MATOU_TEST_SIZE_FILE"
printf 'NO_COLOR=%s\\nCOLORTERM=%s\\nFORCE_COLOR=%s\\n' \
  "\${NO_COLOR-unset}" "\${COLORTERM-unset}" "\${FORCE_COLOR-unset}" > "$MATOU_TEST_ENVIRONMENT_FILE"
printf '\\033[31mCLAUDE_LOGO_READY\\033[0m\\r\\n'
stty raw -echo
cat
`)
  await chmod(provider, 0o755)
  const fixture = await launchMatou({
    root,
    env: {
      SHELL: '/bin/zsh', ZDOTDIR: root, MATOU_CLAUDE_COMMAND: provider,
      MATOU_TEST_SIZE_FILE: sizeFile, MATOU_TEST_ENVIRONMENT_FILE: environmentFile,
      NO_COLOR: '1', COLORTERM: ''
    }
  })
  try {
    await waitForShell(activeSurface(fixture.page))
    await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
    await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
    await expect(visibleSurfaces(fixture.page)).toHaveCount(3)

    const first = visibleSurfaces(fixture.page).first()
    await waitForShell(first)
    await first.click({ position: { x: 30, y: 100 } })
    const pane = first.locator('xpath=ancestor::*[@data-testid="terminal-pane"][1]')
    await expect(pane).toHaveAttribute('data-active', 'true')
    await fixture.page.waitForTimeout(700)
    const viewport = first.locator('.terminal-surface__viewport')
    const expectedCols = await viewport.getAttribute('data-terminal-cols')
    const expectedRows = await viewport.getAttribute('data-terminal-rows')
    expect(Number(expectedCols)).toBeGreaterThan(20)
    expect(Number(expectedRows)).toBeGreaterThan(10)

    await fixture.page.evaluate(() => {
      const state = { seenFailureBanner: false }
      ;(window as typeof window & { __ccPromotionProbe?: typeof state }).__ccPromotionProbe = state
      const inspect = () => {
        const text = Array.from(document.querySelectorAll(
          '.session-start-failure-card,.provider-work-failure-banner,.provider-restore-banner'
        )).map((node) => node.textContent ?? '').join('\n')
        if (/失败/.test(text)) state.seenFailureBanner = true
      }
      new MutationObserver(inspect).observe(document.body, { childList: true, subtree: true })
      inspect()
    })

    await terminalCommand(first, 'cc')
    const promoted = fixture.page.locator(
      `.terminal-surface[data-session-id="${await first.getAttribute('data-session-id')}"]`
    )
    await expect(promoted).toHaveAttribute('data-profile', 'claude-code', { timeout: 10_000 })
    await expect(promoted.locator('.xterm-rows')).toContainText('CLAUDE_LOGO_READY')
    await expect.poll(() => readFile(sizeFile, 'utf8').catch(() => '')).not.toBe('')

    expect((await readFile(sizeFile, 'utf8')).trim()).toBe(`${expectedRows} ${expectedCols}`)
    expect((await readFile(environmentFile, 'utf8')).trim().split('\n')).toEqual([
      'NO_COLOR=unset', 'COLORTERM=truecolor', 'FORCE_COLOR=1'
    ])
    expect(await fixture.page.evaluate(() => (
      window as typeof window & { __ccPromotionProbe?: { seenFailureBanner: boolean } }
    ).__ccPromotionProbe?.seenFailureBanner)).toBe(false)
  } finally {
    await fixture.close()
    await rm(root, { recursive: true, force: true })
  }
})
