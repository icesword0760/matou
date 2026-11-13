import { execFile } from 'node:child_process'
import { chmod, mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { promisify } from 'node:util'

import { expect, test, type Locator } from '@playwright/test'

import { launchMatou } from './matou-fixture'

const execFileAsync = promisify(execFile)
const evidenceDirectory = resolve('docs/acceptance/evidence/network-egress-2026-11')

function egressFixturePayload(anthropicApi: { ip: string; country: string }) {
  return {
    anthropic: {
      api: anthropicApi,
      login: { ip: '169.128.10.1', country: 'US' }
    },
    openai: {
      api: { ip: '169.128.10.1', country: 'US' },
      login: { ip: '169.128.10.1', country: 'US' }
    },
    browserLogin: null,
    systemProxyEnabled: false,
    tunDetected: true,
    ipv6Bypass: false
  }
}

async function positivePid(surface: Locator): Promise<number> {
  let pid = 0
  await expect.poll(async () => {
    pid = Number(await surface.getAttribute('data-pid'))
    return pid
  }).toBeGreaterThan(0)
  return pid
}

async function processStat(pid: number): Promise<string> {
  const { stdout } = await execFileAsync('ps', ['-o', 'stat=', '-p', String(pid)])
  return stdout.trim()
}

async function terminalCommand(surface: Locator, command: string): Promise<void> {
  const textarea = surface.locator('.xterm-helper-textarea')
  await textarea.focus()
  await textarea.pressSequentially(command)
  await textarea.press('Enter')
}

function fakeClaudeScript(): string {
  return [
    '#!/bin/sh',
    "printf 'CLAUDE_TZ=%s\\n' \"${TZ:-}\"",
    'while IFS= read -r line; do',
    "  printf 'CLAUDE_ECHO:%s\\n' \"$line\"",
    'done',
    ''
  ].join('\n')
}

test('verifies network egress protection end to end', async () => {
  test.setTimeout(120_000)

  const tempRoot = await mkdtemp(join(tmpdir(), 'matou-egress-e2e-'))
  const fakeClaudePath = join(tempRoot, 'fake-claude.sh')
  const egressFixturePath = join(tempRoot, 'egress-fixture.json')

  await writeFile(fakeClaudePath, fakeClaudeScript())
  await chmod(fakeClaudePath, 0o755)
  await writeFile(
    egressFixturePath,
    JSON.stringify(egressFixturePayload({ ip: '169.128.10.1', country: 'US' }), null, 2)
  )
  await mkdir(evidenceDirectory, { recursive: true })

  const fixture = await launchMatou({
    env: {
      MATOU_E2E_DISPLAY: 'primary',
      TZ: 'Asia/Shanghai',
      MATOU_CLAUDE_COMMAND: fakeClaudePath,
      MATOU_E2E_EGRESS_FIXTURE: egressFixturePath
    }
  })

  try {
    const page = fixture.page
    await fixture.app.evaluate(({ BrowserWindow }) => {
      BrowserWindow.getAllWindows()[0]?.setSize(1440, 860)
    })

    const initialSurface = page.locator('[data-testid="terminal-pane"] .terminal-surface').first()
    await positivePid(initialSurface)
    await terminalCommand(initialSurface, 'claude')

    const claudeSurface = page.locator('[data-testid="terminal-pane"] .terminal-surface[data-profile="claude-code"]')
    await expect(claudeSurface).toHaveCount(1)
    const claudePid = await positivePid(claudeSurface)
    const claudeSessionId = await claudeSurface.getAttribute('data-session-id')
    expect(claudeSessionId).toBeTruthy()

    await page.getByRole('button', { name: '横向新增 Shell', exact: true }).click()
    const shellSurface = page.locator('[data-testid="terminal-pane"] .terminal-surface[data-profile="shell"]')
    await expect(shellSurface).toHaveCount(1)
    await positivePid(shellSurface)
    const shellSessionId = await shellSurface.getAttribute('data-session-id')
    expect(shellSessionId).toBeTruthy()

    const hud = page.locator('.shortcut-bar .status-info')
    const claudePane = page.getByTestId('terminal-pane').filter({
      has: page.locator('.terminal-surface[data-profile="claude-code"]')
    })
    const shellPane = page.getByTestId('terminal-pane').filter({
      has: page.locator('.terminal-surface[data-profile="shell"]')
    })
    const banner = claudePane.locator('.egress-freeze-banner')

    // a. 正常状态
    await claudeSurface.locator('.xterm-screen').click()
    await expect(hud).toHaveAttribute('data-session-id', claudeSessionId!)
    await expect(hud).toHaveAttribute('data-hud-mode', 'agent')
    await expect(hud.locator('.egress-chip')).toHaveText('🇺🇸 美国 · 出口正常')
    await expect(claudeSurface.locator('.xterm-rows')).toContainText('CLAUDE_TZ=Asia/Singapore')

    await shellSurface.locator('.xterm-screen').click()
    await expect(hud).toHaveAttribute('data-session-id', shellSessionId!)
    await expect(hud).toHaveAttribute('data-hud-mode', 'shell')
    await expect(hud.locator('.egress-chip')).toHaveCount(0)
    await terminalCommand(shellSurface, 'echo "SHELL_TZ=${TZ:-unset}"')
    await expect(shellSurface.locator('.xterm-rows')).toContainText('SHELL_TZ=')
    await expect(shellSurface.locator('.xterm-rows')).not.toContainText('SHELL_TZ=Asia/Singapore')

    await claudeSurface.locator('.xterm-screen').click()
    await expect(hud).toHaveAttribute('data-session-id', claudeSessionId!)
    await expect(hud.locator('.egress-chip')).toHaveText('🇺🇸 美国 · 出口正常')
    await page.screenshot({ path: join(evidenceDirectory, '01-normal.png') })

    // b. 大陆出口 → 冻结
    await writeFile(
      egressFixturePath,
      JSON.stringify(egressFixturePayload({ ip: '223.5.5.5', country: 'CN' }), null, 2)
    )
    await hud.locator('.egress-chip').click()
    const panel = page.getByRole('dialog', { name: '网络检查' })
    await expect(panel).toBeVisible()
    await page.screenshot({ path: join(evidenceDirectory, '06-panel.png') })

    await panel.getByRole('button', { name: '重新检查', exact: true }).click()
    await expect(banner).toContainText('出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。')
    await panel.getByRole('button', { name: '关闭', exact: true }).click()
    await expect(panel).toHaveCount(0)

    await expect(hud.locator('.egress-chip')).toHaveText('出口异常 · 已暂停 1 个会话')
    await expect.poll(() => processStat(claudePid)).toContain('T')

    await terminalCommand(claudeSurface, 'FROZEN_SHOULD_NOT_ECHO')
    await expect(claudeSurface.locator('.xterm-rows')).not.toContainText('FROZEN_SHOULD_NOT_ECHO')

    await shellSurface.locator('.xterm-screen').click()
    await expect(shellPane.locator('.egress-freeze-banner')).toHaveCount(0)
    await terminalCommand(shellSurface, 'echo ok')
    await expect(shellSurface.locator('.xterm-rows')).toContainText('ok')

    await claudeSurface.locator('.xterm-screen').click()
    await expect(hud.locator('.egress-chip')).toHaveText('出口异常 · 已暂停 1 个会话')
    await page.screenshot({ path: join(evidenceDirectory, '02-frozen.png') })

    // c. 自动恢复（约 20 秒内）
    await writeFile(
      egressFixturePath,
      JSON.stringify(egressFixturePayload({ ip: '169.128.10.1', country: 'US' }), null, 2)
    )
    await expect(banner).toContainText('出口已恢复，已自动继续', { timeout: 40_000 })
    await expect(banner).toHaveCount(0, { timeout: 10_000 })
    await expect.poll(() => processStat(claudePid)).not.toContain('T')
    await expect(hud.locator('.egress-chip')).toHaveText('🇺🇸 美国 · 出口正常')

    await terminalCommand(claudeSurface, 'RESUMED_ECHO_OK')
    await expect(claudeSurface.locator('.xterm-rows')).toContainText('CLAUDE_ECHO:RESUMED_ECHO_OK')
    await expect(claudeSurface.locator('.xterm-rows')).not.toContainText('FROZEN_SHOULD_NOT_ECHO')
    await page.screenshot({ path: join(evidenceDirectory, '03-resumed.png') })

    // d. 仍要继续（越过）
    await writeFile(
      egressFixturePath,
      JSON.stringify(egressFixturePayload({ ip: '223.5.5.5', country: 'CN' }), null, 2)
    )
    await hud.locator('.egress-chip').click()
    await expect(panel).toBeVisible()
    await panel.getByRole('button', { name: '重新检查', exact: true }).click()
    await expect(banner).toContainText('出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。')
    await panel.getByRole('button', { name: '关闭', exact: true }).click()
    await expect(panel).toHaveCount(0)
    await expect.poll(() => processStat(claudePid)).toContain('T')

    await banner.getByRole('button', { name: '仍要继续', exact: true }).click()
    const confirmDialog = page.getByRole('alertdialog', { name: '仍要继续？' })
    await expect(confirmDialog).toBeVisible()
    await confirmDialog.getByRole('button', { name: '继续', exact: true }).click()
    await expect(confirmDialog).toHaveCount(0)

    await expect(banner).toHaveCount(0)
    await expect.poll(() => processStat(claudePid)).not.toContain('T')
    await expect(hud.locator('.egress-chip')).toHaveText('出口异常 · 已手动继续')
    await terminalCommand(claudeSurface, 'OVERRIDDEN_ECHO_OK')
    await expect(claudeSurface.locator('.xterm-rows')).toContainText('CLAUDE_ECHO:OVERRIDDEN_ECHO_OK')
    await page.screenshot({ path: join(evidenceDirectory, '04-overridden.png') })

    // e. 关闭保护
    await writeFile(
      egressFixturePath,
      JSON.stringify(egressFixturePayload({ ip: '169.128.10.1', country: 'US' }), null, 2)
    )
    await hud.locator('.egress-chip').click()
    await expect(panel).toBeVisible()
    await panel.getByRole('button', { name: '重新检查', exact: true }).click()
    await expect(hud.locator('.egress-chip')).toHaveText('🇺🇸 美国 · 出口正常')

    const protectionCheckbox = panel.getByRole('checkbox', { name: /出口保护/ })
    await expect(protectionCheckbox).toBeChecked()
    await protectionCheckbox.click()
    await expect(protectionCheckbox).not.toBeChecked()

    await writeFile(
      egressFixturePath,
      JSON.stringify(egressFixturePayload({ ip: '223.5.5.5', country: 'CN' }), null, 2)
    )
    await panel.getByRole('button', { name: '重新检查', exact: true }).click()
    await expect(panel).toContainText('api.anthropic.com 的出口在中国大陆')
    await panel.getByRole('button', { name: '关闭', exact: true }).click()
    await expect(panel).toHaveCount(0)

    await expect(banner).toHaveCount(0)
    await expect.poll(() => processStat(claudePid)).not.toContain('T')
    await terminalCommand(claudeSurface, 'PROTECTION_OFF_ECHO_OK')
    await expect(claudeSurface.locator('.xterm-rows')).toContainText('CLAUDE_ECHO:PROTECTION_OFF_ECHO_OK')
    await page.screenshot({ path: join(evidenceDirectory, '05-protection-off.png') })
  } finally {
    await fixture.close()
    await rm(tempRoot, { recursive: true, force: true })
  }
})
