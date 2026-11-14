import { chmod, copyFile, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'

import { expect, test, type Locator } from '@playwright/test'

import { terminalCommand, waitForShell } from './fixtures/session-canvas-fixture'
import { launchMatou, restartMatouGracefully, type MatouFixture } from './matou-fixture'

const conversationId = 'compacted-history-e2e'
const beforeUser = '压缩前原始问题，逐项核对部署记录'
const beforeAssistant = '压缩前完整答复，先检查回滚计划'
const afterAssistant = '压缩后答复，继续核对部署记录'
const oldScreenMarker = 'FIRST_RUN_SCROLLBACK_MARKER'
const fillerCount = 600
const messageCount = fillerCount + 4

interface ProviderLaunch {
  pid: number
  ppid: number
  sessionId: string
  timestamp: number
  argv: string[]
}

test('reads the bound conversation across compaction and keeps terminal scrollback after restart', async ({}, testInfo) => {
  test.setTimeout(90_000)
  const root = await mkdtemp('/tmp/matou-e2e-session-history-')
  const workspace = join(root, 'matou_workspace')
  const configDirectory = join(root, 'claude-config')
  const projectsRoot = join(configDirectory, 'projects')
  const project = join(projectsRoot, workspace.replace(/[^A-Za-z0-9]/g, '-'))
  const transcriptPath = join(project, `${conversationId}.jsonl`)
  const provider = join(root, 'session-history-provider.py')
  const launchLog = join(root, 'provider-launches.jsonl')
  const env = {
    SHELL: '/bin/sh',
    CLAUDE_CONFIG_DIR: configDirectory,
    CLAUDE_SECURESTORAGE_CONFIG_DIR: configDirectory,
    MATOU_CLAUDE_PROJECTS_ROOT: projectsRoot,
    MATOU_E2E_ACCOUNTS_BRIDGE: resolve(import.meta.dirname, 'fixtures/claude-accounts-engine.py'),
    MATOU_E2E_ACCOUNTS_SCENARIO: 'empty',
    MATOU_CLAUDE_COMMAND: provider,
    MATOU_HISTORY_TRANSCRIPT: transcriptPath,
    MATOU_HISTORY_LAUNCHES: launchLog,
    ANTHROPIC_API_KEY: '',
    ANTHROPIC_AUTH_TOKEN: '',
    ANTHROPIC_BASE_URL: 'http://127.0.0.1:1',
    CLAUDE_CODE_OAUTH_TOKEN: ''
  }
  const launchStages: Array<{ stage: string; timestamp: number; launches: ProviderLaunch[] }> = []
  const recordLaunches = async (stage: string): Promise<ProviderLaunch[]> => {
    const launches = await providerLaunches(launchLog)
    launchStages.push({ stage, timestamp: Date.now(), launches })
    return launches
  }
  let fixture: MatouFixture | undefined
  try {
    await mkdir(project, { recursive: true })
    await copyFile(resolve(import.meta.dirname, 'fixtures/session-history-provider.py'), provider)
    await chmod(provider, 0o755)
    const originalTranscript = createTranscript(workspace)
    await writeFile(transcriptPath, originalTranscript)
    fixture = await launchMatou({ root, env })

    const initialCard = fixture.page.getByTestId('terminal-pane').first()
    const initialSurface = initialCard.locator('.terminal-surface')
    await waitForShell(initialSurface)
    const sessionId = await initialSurface.getAttribute('data-session-id')
    expect(sessionId).toBeTruthy()
    const cardFor = ({ page }: MatouFixture) => page.getByTestId('terminal-pane').filter({
      has: page.locator(`.terminal-surface[data-session-id="${sessionId}"]`)
    })

    await initialCard.getByRole('button', { name: /载入 Claude Code 会话到/ }).click()
    const loader = fixture.page.getByRole('dialog', { name: '载入 Claude Code 会话' })
    await loader.getByRole('button', { name: `预览会话：${beforeUser}` }).click()
    await loader.getByRole('button', { name: '载入到当前卡片' }).click()
    const confirmLoad = loader.getByRole('button', { name: '结束当前运行并载入' })
    if (await confirmLoad.isVisible()) await confirmLoad.click()
    await expect(loader).toHaveCount(0)

    let card = cardFor(fixture)
    let surface = card.locator('.terminal-surface')
    await expect(surface).toHaveAttribute('data-profile', 'claude-code')
    await expect(surface.locator('.xterm-rows')).toContainText(oldScreenMarker)
    const originalPid = await surface.getAttribute('data-pid')
    expect(originalPid).toMatch(/^\d+$/)
    const beforeFirstHistory = await recordLaunches('before first history window')
    expect(beforeFirstHistory).toMatchObject([{ pid: Number(originalPid), sessionId: conversationId }])
    const history = await openHistory(card)
    const preview = history.getByLabel('会话预览')
    const status = history.getByRole('status')
    await expect(status).toHaveText(new RegExp(`^已加载 \\d+ / ${messageCount} 条$`))
    await findHistoryText(history, afterAssistant)
    await history.getByRole('button', { name: '清除内容查找' }).click()
    await expect(status).toHaveText(new RegExp(`^已加载 \\d+ / ${messageCount} 条$`))
    const messages = history.locator('.session-loader-events')
    await messages.hover()
    await expect(async () => {
      await fixture!.page.mouse.wheel(0, -100_000)
      await expect(status).toHaveText(`已加载 ${messageCount} / ${messageCount} 条`, { timeout: 500 })
    }).toPass({ timeout: 10_000 })
    await expect(preview).toContainText(beforeAssistant)
    await findHistoryText(history, beforeAssistant)
    await history.screenshot({ path: testInfo.outputPath('pre-compaction-history.png') })
    await closeHistory(history, surface, originalPid!)
    expect(await recordLaunches('after first history window')).toEqual(beforeFirstHistory)

    // Crossing the Bypass boundary replaces the live PTY through the real HUD.
    const hud = fixture.page.locator('.shortcut-bar .status-info')
    await hud.locator('.status-perm-badge').click()
    await fixture.page.getByRole('menu', { name: '权限模式' })
      .getByRole('menuitem', { name: 'Bypass Permissions' }).click()
    await fixture.page.getByRole('alertdialog', { name: '切换到高权限模式' })
      .getByRole('button', { name: '确认切换' }).click()
    await expect(surface).not.toHaveAttribute('data-pid', originalPid!)
    await expect(hud.locator('.status-perm-badge')).toHaveText('BP')
    const replacementPid = await surface.getAttribute('data-pid')
    expect(replacementPid).toMatch(/^\d+$/)
    await expect(surface.locator('.xterm-rows')).toContainText(`RESUMED_PROCESS:${replacementPid}`)
    await showOldScrollback(surface)
    const beforeSecondHistory = await recordLaunches('after permission switch, before second history window')
    expect(beforeSecondHistory.slice(beforeFirstHistory.length)).toMatchObject([
      { pid: Number(replacementPid), sessionId: conversationId }
    ])
    const afterPermission = await openHistory(card)
    await findHistoryText(afterPermission, beforeUser)
    await closeHistory(afterPermission, surface, replacementPid!)
    expect(await recordLaunches('after second history window')).toEqual(beforeSecondHistory)

    const beforeRestart = await recordLaunches('before application restart')
    fixture = await restartMatouGracefully(fixture, { env })
    card = cardFor(fixture)
    surface = card.locator('.terminal-surface')
    await expect(surface).toHaveAttribute('data-profile', 'claude-code')
    await waitForShell(surface)
    const restoredPid = await surface.getAttribute('data-pid')
    expect(restoredPid).not.toBe(replacementPid)
    await expect(surface.locator('.xterm-rows')).toContainText(`RESUMED_PROCESS:${restoredPid}`)
    const afterApplicationRestart = await recordLaunches('after application restart')
    const recoveryLaunches = afterApplicationRestart.slice(beforeRestart.length)
    expect(recoveryLaunches.at(-1)).toMatchObject({ pid: Number(restoredPid), sessionId: conversationId })
    expect(recoveryLaunches.every(launch => launch.sessionId === conversationId)).toBe(true)
    for (const savedLaunch of recoveryLaunches.slice(0, -1)) {
      await expect.poll(() => processIsAlive(savedLaunch.pid)).toBe(false)
    }
    await showOldScrollback(surface)
    const beforeThirdHistory = await recordLaunches('before third history window')
    const afterRestart = await openHistory(card)
    await findHistoryText(afterRestart, beforeAssistant)
    await afterRestart.screenshot({ path: testInfo.outputPath('pre-compaction-history-after-restart.png') })
    await closeHistory(afterRestart, surface, restoredPid!)
    expect(await recordLaunches('after third history window')).toEqual(beforeThirdHistory)
    await terminalCommand(surface, 'HISTORY_VIEW_LEFT_PROVIDER_ACTIVE')
    await expect(surface.locator('.xterm-rows')).toContainText('INPUT_OK:HISTORY_VIEW_LEFT_PROVIDER_ACTIVE')

    expect(await readFile(transcriptPath, 'utf8')).toBe(originalTranscript)
  } finally {
    await writeFile(testInfo.outputPath('stage-launches.json'), JSON.stringify({
      stages: launchStages,
      processStatus: launchStages.at(-1)?.launches.map(({ pid }) => ({ pid, alive: processIsAlive(pid) }))
    }, null, 2))
    if (fixture) await fixture.close()
    else await rm(root, { recursive: true, force: true })
  }
})

function createTranscript(cwd: string): string {
  const message = (role: 'user' | 'assistant', content: string) => ({
    type: role,
    message: { role, content }
  })
  const rows = [
    message('user', beforeUser),
    message('assistant', beforeAssistant),
    { type: 'system', subtype: 'compact_boundary' },
    { ...message('user', '压缩摘要，后续从这里继续'), isCompactSummary: true },
    ...Array.from({ length: fillerCount }, (_, index) => message('assistant', `压缩后记录 ${index + 1}`)),
    message('assistant', afterAssistant)
  ]
  return rows.map((row, index) => JSON.stringify({
    ...row, sessionId: conversationId, cwd,
    timestamp: new Date(Date.UTC(2026, 10, 14, 0, 0, index)).toISOString()
  })).join('\n') + '\n'
}

async function openHistory(card: Locator): Promise<Locator> {
  await card.getByRole('button', { name: /查看.*的会话历史/ }).click()
  const dialog = card.page().getByRole('dialog', { name: '会话历史' })
  await expect(dialog.getByRole('searchbox', { name: '查找右侧会话内容' })).toBeEnabled()
  await expect(dialog.getByRole('button', { name: /载入到当前卡片|结束当前运行并载入/ })).toHaveCount(0)
  await expect(dialog.getByRole('searchbox', { name: '筛选左侧会话' })).toHaveCount(0)
  return dialog
}

async function findHistoryText(dialog: Locator, content: string): Promise<void> {
  await dialog.getByRole('searchbox', { name: '查找右侧会话内容' }).fill(content)
  await expect(dialog.getByLabel('右侧内容匹配位置')).toContainText('1/1')
  await expect(dialog.getByLabel('会话预览').locator('.active-match')).toContainText(content)
}

async function closeHistory(dialog: Locator, surface: Locator, pid: string): Promise<void> {
  await dialog.page().keyboard.press('Escape')
  await expect(dialog).toHaveCount(0)
  await expect(surface).toHaveAttribute('data-pid', pid)
}

async function showOldScrollback(surface: Locator): Promise<void> {
  const rows = surface.locator('.xterm-rows')
  await expect(rows).not.toContainText(oldScreenMarker)
  const input = surface.locator('.xterm-helper-textarea')
  await input.focus()
  await expect(async () => {
    await input.press('Shift+PageUp')
    await expect(rows).toContainText(oldScreenMarker, { timeout: 300 })
  }).toPass({ timeout: 5_000 })
}

async function providerLaunches(path: string): Promise<ProviderLaunch[]> {
  return (await readFile(path, 'utf8')).trim().split('\n').map(line => JSON.parse(line))
}

function processIsAlive(pid: number): boolean {
  try {
    process.kill(pid, 0)
    return true
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === 'ESRCH') return false
    throw error
  }
}
