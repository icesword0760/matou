import { chmod, mkdir, readFile, writeFile } from 'node:fs/promises'
import { join } from 'node:path'

import { expect, test } from '@playwright/test'

import { launchMatou, restartMatou } from './matou-fixture'

test.describe('load an existing Claude Code session', () => {
  test.setTimeout(60_000)

  test('discovers current-workspace history and searches exact conversation content', async () => {
    let fixture = await launchMatou()
    try {
      const projectsRoot = join(fixture.rootDirectory, 'claude-projects')
      const providerExecutable = join(fixture.rootDirectory, 'load-session-provider.sh')
      const invocationLog = join(fixture.rootDirectory, 'load-session-provider-invocations.txt')
      const projectDirectory = join(projectsRoot, encodeClaudeProjectPath(fixture.workspaceDirectory))
      await mkdir(projectDirectory, { recursive: true })
      await writeFile(join(projectDirectory, 'load-session-e2e.jsonl'), [
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
        JSON.stringify({
          type: 'permission-mode', sessionId: 'load-session-e2e', cwd: fixture.workspaceDirectory,
          timestamp: '2026-08-31T10:02:00.000Z', permissionMode: 'bypassPermissions'
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
      await expect(dialog.getByRole('button', { name: /预览会话：检查通知中心的聚合逻辑/ }))
        .toBeVisible()
      await expect(dialog).toContainText('开放所有权限')

      await dialog.getByRole('searchbox', { name: '搜索会话内容' }).fill('hover width')
      await expect(dialog.getByRole('button', { name: '跳转到第 2 条会话内容' })).toBeVisible()
      await dialog.getByRole('button', { name: '跳转到第 2 条会话内容' }).click()
      await expect(dialog.getByLabel('会话预览')).toContainText('hover width')

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
