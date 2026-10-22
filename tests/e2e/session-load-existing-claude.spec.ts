import { chmod, mkdir, readFile, writeFile } from 'node:fs/promises'
import { join } from 'node:path'

import { expect, test } from '@playwright/test'

import { launchMatou, restartMatou } from './matou-fixture'

test.describe('load an existing Claude Code session', () => {
  test.setTimeout(60_000)

  test('loads the selected long conversation through the current resume confirmation and accepts input', async ({}, testInfo) => {
    let fixture = await launchMatou()
    const projectsRoot = join(fixture.rootDirectory, 'claude-projects')
    const projectDirectory = join(projectsRoot, encodeClaudeProjectPath(fixture.workspaceDirectory))
    const provider = join(fixture.rootDirectory, 'confirmed-resume.py')
    await mkdir(projectDirectory, { recursive: true })
    await writeFile(join(projectDirectory, 'long-conversation.jsonl'), JSON.stringify({
      type: 'user', sessionId: 'long-conversation', cwd: fixture.workspaceDirectory,
      timestamp: new Date().toISOString(), message: { role: 'user', content: '长会话恢复验证' }
    }) + '\n')
    await writeFile(provider, `#!/usr/bin/env python3
import os, sys, termios, tty
previous = termios.tcgetattr(0)
tty.setraw(0)
sys.stdout.write('Resume this conversation?\\r\\nThis conversation has been inactive for 17h 28m and is 745.7k tokens long.\\r\\n1. Resume\\r\\n2. Start a new conversation\\r\\nEnter to confirm · Esc to resume\\r\\n')
sys.stdout.flush()
selected = os.read(0, 1)
termios.tcsetattr(0, termios.TCSADRAIN, previous)
if selected != b'\\r': sys.exit(2)
print('RESUMED:' + sys.argv[sys.argv.index('--resume') + 1], flush=True)
for line in sys.stdin:
    print('LOADED_INPUT:' + line.strip(), flush=True)
`)
    await chmod(provider, 0o755)
    try {
      fixture = await restartMatou(fixture, { env: { MATOU_CLAUDE_PROJECTS_ROOT: projectsRoot, MATOU_CLAUDE_COMMAND: provider } })
      const card = fixture.page.getByTestId('terminal-pane').first()
      const sessionId = await card.locator('.terminal-surface').getAttribute('data-session-id')
      await fixture.page.getByRole('button', { name: /载入 Claude Code 会话到/ }).click()
      const dialog = fixture.page.getByRole('dialog', { name: '载入 Claude Code 会话' })
      await dialog.getByRole('button', { name: '预览会话：长会话恢复验证' }).click()
      await dialog.getByRole('button', { name: '载入到当前卡片' }).click()
      const confirmation = dialog.getByRole('button', { name: '结束当前运行并载入' })
      if (await confirmation.isVisible().catch(() => false)) await confirmation.click()
      await expect(dialog).toHaveCount(0)
      await expect(card.locator('.xterm-rows')).toContainText('RESUMED:long-conversation')
      await expect(card.locator('.terminal-surface')).toHaveAttribute('data-session-id', sessionId!)
      const input = card.locator('.xterm-helper-textarea')
      await input.focus()
      await input.pressSequentially('LOAD_INPUT_OK')
      await input.press('Enter')
      await expect(card.locator('.xterm-rows')).toContainText('LOADED_INPUT:LOAD_INPUT_OK')
      await fixture.page.screenshot({ path: testInfo.outputPath('loaded-long-conversation.png') })
    } finally {
      await fixture.close()
    }
  })

  test('separates left-side session filtering from right-side conversation search', async () => {
    let fixture = await launchMatou()
    try {
      const projectsRoot = join(fixture.rootDirectory, 'claude-projects')
      const providerExecutable = join(fixture.rootDirectory, 'load-session-provider.sh')
      const invocationLog = join(fixture.rootDirectory, 'load-session-provider-invocations.txt')
      const projectDirectory = join(projectsRoot, encodeClaudeProjectPath(fixture.workspaceDirectory))
      await mkdir(projectDirectory, { recursive: true })
      const transcriptRows = [
        JSON.stringify({
          type: 'user', sessionId: 'load-session-e2e', cwd: fixture.workspaceDirectory,
          timestamp: '2026-08-31T10:00:00.000Z', permissionMode: 'default',
          message: { role: 'user', content: '检查通知中心的聚合逻辑' }
        }),
        JSON.stringify({
          type: 'assistant', sessionId: 'load-session-e2e', cwd: fixture.workspaceDirectory,
          timestamp: '2026-08-31T10:01:00.000Z',
          message: {
            role: 'assistant', model: 'claude-opus-4-6', content: [
              { type: 'text', text: '开始定位卡片 hover width 闪烁。' },
              { type: 'tool_use', name: 'Read', input: { file_path: 'SessionCanvas.tsx' } }
            ]
          }
        }),
        ...Array.from({ length: 398 }, (_, index) => {
          const eventIndex = index + 3
          const content = eventIndex === 50 || eventIndex === 350
            ? `跨页搜索标记 ${eventIndex}`
            : `历史内容 ${eventIndex}`
          return JSON.stringify({
            type: 'assistant', sessionId: 'load-session-e2e', cwd: fixture.workspaceDirectory,
            timestamp: new Date(Date.UTC(2026, 7, 31, 10, 2, index)).toISOString(),
            message: { role: 'assistant', model: 'claude-opus-4-6', content }
          })
        }),
        JSON.stringify({
          type: 'permission-mode', sessionId: 'load-session-e2e', cwd: fixture.workspaceDirectory,
          timestamp: '2026-08-31T10:02:00.000Z', permissionMode: 'bypassPermissions'
        })
      ]
      await writeFile(join(projectDirectory, 'load-session-e2e.jsonl'), transcriptRows.join('\n'))
      await writeFile(join(projectDirectory, 'changed-cwd-session-e2e.jsonl'), [
        JSON.stringify({
          type: 'user', sessionId: 'changed-cwd-session-e2e', cwd: fixture.workspaceDirectory,
          timestamp: '2026-09-01T10:00:00.000Z',
          message: { role: 'user', content: '整理最新项目材料' }
        }),
        JSON.stringify({
          type: 'assistant', sessionId: 'changed-cwd-session-e2e',
          cwd: join(fixture.workspaceDirectory, 'generated-output'),
          timestamp: '2026-09-01T10:01:00.000Z',
          message: { role: 'assistant', model: 'claude-opus-5', content: '材料已经整理完成。' }
        }),
        JSON.stringify({
          type: 'ai-title', sessionId: 'changed-cwd-session-e2e', aiTitle: '最新项目材料'
        })
      ].join('\n'))
      await writeFile(providerExecutable, [
        '#!/bin/sh',
        'printf "%s\\n" "$*" >> "$MATOU_LOAD_SESSION_INVOCATIONS"',
        "printf '%02050d' 0",
        'printf "\\nLOADED_EXISTING_SESSION\\n"',
        'while IFS= read -r line; do printf "LOADED_INPUT: %s\\n" "$line"; done',
        ''
      ].join('\n'))
      await chmod(providerExecutable, 0o755)
      fixture = await restartMatou(fixture, {
        env: {
          MATOU_CLAUDE_PROJECTS_ROOT: projectsRoot,
          MATOU_CLAUDE_COMMAND: providerExecutable,
          MATOU_LOAD_SESSION_INVOCATIONS: invocationLog
        }
      })

      const targetCard = fixture.page.getByTestId('terminal-pane').first()
      const stableCardId = await targetCard.locator('.terminal-surface').getAttribute('data-session-id')
      expect(stableCardId).toBeTruthy()
      await fixture.page.getByRole('button', { name: /载入 Claude Code 会话到/ }).click()
      const dialog = fixture.page.getByRole('dialog', { name: '载入 Claude Code 会话' })
      await expect(dialog).toBeVisible()
      await expect(dialog.getByRole('button', { name: /预览会话：最新项目材料/ }))
        .toBeVisible()
      await expect(dialog.getByRole('button', { name: /预览会话：检查通知中心的聚合逻辑/ }))
        .toBeVisible()
      await dialog.getByRole('button', { name: /预览会话：检查通知中心的聚合逻辑/ }).click()
      await expect(dialog).toContainText('开放所有权限')
      await expect(dialog.getByRole('status')).toContainText('已加载 200 / 400 条')

      const sessionFilter = dialog.getByRole('searchbox', { name: '筛选左侧会话' })
      const contentSearch = dialog.getByRole('searchbox', { name: '查找右侧会话内容' })
      await sessionFilter.fill('hover width')
      await expect(dialog.getByText('左侧没有匹配的会话')).toBeVisible()
      await sessionFilter.fill('检查通知中心')
      await expect(dialog.getByRole('button', { name: /预览会话：检查通知中心的聚合逻辑/ }))
        .toBeVisible()

      await contentSearch.fill('hover width')
      await expect(dialog.getByLabel('右侧内容匹配位置')).toContainText('1/1')
      await expect(dialog.getByLabel('会话预览')).toContainText('hover width')

      await contentSearch.fill('跨页搜索标记')
      await expect(dialog.getByLabel('右侧内容匹配位置')).toContainText('1/2')
      await expect(dialog.getByLabel('会话预览')).toContainText('跨页搜索标记 50')
      await dialog.getByRole('button', { name: '下一个匹配' }).click()
      await expect(dialog.getByLabel('右侧内容匹配位置')).toContainText('2/2')
      await expect(dialog.getByLabel('会话预览')).toContainText('跨页搜索标记 350')

      await dialog.getByRole('button', { name: '载入到当前卡片' }).click()
      const confirmation = dialog.getByRole('button', { name: '结束当前运行并载入' })
      if (await confirmation.isVisible().catch(() => false)) await confirmation.click()
      await expect(dialog).toHaveCount(0)
      await expect(fixture.page.getByTestId('terminal-pane').first().locator('.terminal-surface'))
        .toHaveAttribute('data-session-id', stableCardId!)
      await expect(fixture.page.locator('[data-testid="terminal-pane"]:visible .xterm-rows').first())
        .toContainText('LOADED_EXISTING_SESSION')
      const invocation = await readFile(invocationLog, 'utf8')
      expect(invocation).toContain('--resume load-session-e2e')
      expect(invocation).toContain('--dangerously-skip-permissions')
    } finally {
      await fixture.close()
    }
  })
})

function encodeClaudeProjectPath(cwd: string): string {
  return cwd.replace(/[^A-Za-z0-9]/g, '-')
}
