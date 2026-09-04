import { execFile } from 'node:child_process'
import { chmod, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { promisify } from 'node:util'

import { expect, test, type Locator } from '@playwright/test'

import { launchMatou } from './matou-fixture'

const execFileAsync = promisify(execFile)
const evidenceDirectory = resolve(import.meta.dirname, '../../docs/acceptance/evidence/prd-02/matou')

test('shows the live Shell environment with reference product geometry and refreshes cwd and Git silently', async () => {
  const fixture = await launchMatou()
  try {
    const surface = activeSurface(fixture.page.locator('body'))
    await positivePid(surface)
    const hud = fixture.page.locator('.shortcut-bar .status-info')
    await expect(hud).toHaveAttribute('data-hud-mode', 'shell')
    await expect(hud).toContainText(`~/${fixture.workspaceDirectory.split('/').at(-1)}`)
    await expect.poll(() => fixture.page.locator('.shortcut-bar').evaluate((element) => {
      const style = getComputedStyle(element)
      return { height: style.height, paddingLeft: style.paddingLeft, paddingRight: style.paddingRight }
    })).toEqual({ height: '38px', paddingLeft: '20px', paddingRight: '20px' })

    await execFileAsync('git', ['-C', fixture.workspaceDirectory, 'init', '-b', 'hud-main'])
    await execFileAsync('git', ['-C', fixture.workspaceDirectory, 'config', 'user.name', 'Matou E2E'])
    await execFileAsync('git', ['-C', fixture.workspaceDirectory, 'config', 'user.email', 'matou@example.test'])
    await writeFile(join(fixture.workspaceDirectory, 'README.md'), 'clean\n')
    await execFileAsync('git', ['-C', fixture.workspaceDirectory, 'add', 'README.md'])
    await execFileAsync('git', ['-C', fixture.workspaceDirectory, 'commit', '-m', 'initial'])
    await terminalCommand(surface, 'true')
    await expect(hud.locator('.status-git')).toHaveText('hud-main')
    await expect(hud.locator('.status-git')).toHaveCSS('color', 'rgb(255, 107, 53)')

    await writeFile(join(fixture.workspaceDirectory, 'README.md'), 'dirty\n')
    await terminalCommand(surface, 'true')
    await expect(hud.locator('.status-git')).toHaveText('hud-main*')
    await mkdir(evidenceDirectory, { recursive: true })
    await fixture.page.locator('.hierarchy-shell').screenshot({
      path: join(evidenceDirectory, 'shell-hud.png')
    })
    await writeFile(join(evidenceDirectory, 'shell-hud.json'), JSON.stringify(
      await hudGeometry(fixture.page.locator('body')), null, 2
    ))

    await terminalCommand(surface, 'mkdir -p ../outside && cd ../outside')
    await expect(hud).toContainText('~/outside')
    await expect(hud.locator('.status-git')).toHaveCount(0)
    await expect(hud).not.toContainText(/unknown|N\/A|--/i)
  } finally {
    await fixture.close()
  }
})

test('operates the compact Git controller for branches, Worktrees, commits, and push availability', async () => {
  const fixture = await launchMatou()
  try {
    await fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.setSize(1400, 820))
    const surface = activeSurface(fixture.page.locator('body'))
    await positivePid(surface)
    await execFileAsync('git', ['-C', fixture.workspaceDirectory, 'init', '-b', 'main'])
    await execFileAsync('git', ['-C', fixture.workspaceDirectory, 'config', 'user.name', 'Matou E2E'])
    await execFileAsync('git', ['-C', fixture.workspaceDirectory, 'config', 'user.email', 'matou@example.test'])
    await writeFile(join(fixture.workspaceDirectory, 'README.md'), 'initial\n')
    await execFileAsync('git', ['-C', fixture.workspaceDirectory, 'add', 'README.md'])
    await execFileAsync('git', ['-C', fixture.workspaceDirectory, 'commit', '-m', 'initial'])
    await writeFile(join(fixture.workspaceDirectory, 'README.md'), 'changed\n')
    await terminalCommand(surface, 'true')

    const gitTrigger = fixture.page.getByRole('button', { name: '打开 Git' })
    await expect(gitTrigger).toHaveText('main*')
    await gitTrigger.click()
    const controller = fixture.page.getByRole('dialog', { name: 'Git 与 Worktree' })
    const search = controller.getByPlaceholder('搜索 matou 分支')
    await expect(search).toBeFocused()
    await expect(controller.getByRole('navigation')).toHaveCount(0)
    await expect(controller.getByRole('button', { name: '创建并检出新分支…' })).toBeVisible()
    await expect(controller.getByRole('button', { name: '管理 Worktree… 0' })).toBeVisible()
    await expect(controller.getByRole('button', { name: '提交与推送…' })).toBeVisible()
    await mkdir(evidenceDirectory, { recursive: true })
    await controller.screenshot({ path: join(evidenceDirectory, 'git-control.png') })

    await controller.getByRole('button', { name: '创建并检出新分支…' }).click()
    await controller.getByPlaceholder('例如 feature/improve-git-menu').fill('feature/e2e-git-control')
    await controller.getByRole('button', { name: '创建并检出' }).click()
    await expect(controller).toHaveCount(0)
    await terminalCommand(surface, 'true')
    await expect(gitTrigger).toHaveText('feature/e2e-git-control*')

    await gitTrigger.click()
    await controller.getByRole('button', { name: '提交与推送…' }).click()
    await expect(controller.getByRole('button', { name: '推送', exact: true })).toBeDisabled()
    await controller.getByRole('button', { name: '提交', exact: true }).click()
    await expect(controller.getByRole('status')).toContainText('提交已完成')
    const { stdout: latestCommit } = await execFileAsync('git', [
      '-C', fixture.workspaceDirectory, 'log', '-1', '--pretty=%s'
    ])
    expect(latestCommit.trim()).toBe('chore: update 1 file')

    await fixture.page.keyboard.press('Escape')
    await expect(search).toBeFocused()
    await controller.getByRole('button', { name: '管理 Worktree… 0' }).click()
    await expect(controller.getByText('Worktree', { exact: true })).toBeVisible()
    await expect(controller.getByText('当前')).toBeVisible()
    await controller.getByRole('button', { name: '创建新 Worktree…' }).click()
    await controller.getByPlaceholder('例如 feature/new-worktree').fill('feature/e2e-worktree')
    await controller.getByRole('button', { name: '创建' }).click()
    await expect(controller.getByText('feature/e2e-worktree')).toBeVisible()
    await controller.getByRole('button', { name: 'feature/e2e-worktree 更多操作' }).click()
    await expect(controller.getByRole('button', { name: '在 Finder 中显示' })).toBeVisible()
    await expect(controller.getByRole('button', { name: '移除 Worktree' })).toBeEnabled()
  } finally {
    await fixture.close()
  }
})

test('moves from Shell to the full Agent HUD with switchable permissions and a read-only model', async () => {
  const providerRoot = await mkdtemp(join(tmpdir(), 'matou-prd02-provider-'))
  const provider = join(providerRoot, 'claude-fixture.sh')
  const providerConfig = join(providerRoot, 'provider-config')
  const invocationLog = join(providerRoot, 'invocations.txt')
  const exitFile = join(providerRoot, 'exit-now')
  await mkdir(providerConfig, { recursive: true })
  await writeFile(provider, providerScript())
  await chmod(provider, 0o755)
  const fixture = await launchMatou({ env: {
    MATOU_CLAUDE_COMMAND: provider,
    MATOU_PRD02_INVOCATIONS: invocationLog,
    MATOU_PRD02_EXIT_FILE: exitFile,
    CLAUDE_CONFIG_DIR: providerConfig
  } })
  try {
    await fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.setSize(1800, 900))
    const surface = activeSurface(fixture.page.locator('body'))
    const shellPid = await positivePid(surface)
    await mkdir(join(fixture.workspaceDirectory, '.claude'), { recursive: true })
    await writeFile(join(fixture.workspaceDirectory, 'CLAUDE.md'), 'project instructions\n')
    await writeFile(join(fixture.workspaceDirectory, '.mcp.json'), JSON.stringify({
      mcpServers: { browser_bridge: {} }
    }))
    await writeFile(join(fixture.workspaceDirectory, '.claude', 'settings.local.json'), JSON.stringify({
      hooks: { Notification: [], Stop: [] }
    }))
    await terminalCommand(surface, 'claude')

    const hud = fixture.page.locator('.shortcut-bar .status-info')
    await expect(fixture.page.locator('[data-testid="terminal-pane"]:visible .pane-title'))
      .toHaveText('Claude')
    await expect(hud).toHaveAttribute('data-hud-mode', 'agent')
    await expect.poll(() => positivePid(surface)).not.toBe(shellPid)
    await expect(hud.locator('.status-perm-badge')).toHaveText('A')
    await expect(hud.locator('.status-model')).toHaveText('Fable 5 (1M context)')
    await expect(hud.getByRole('button', { name: /当前权限模式：Auto/ })).toBeVisible()
    await expect(hud.getByRole('button', { name: /切换模型/ })).toHaveCount(0)
    await hud.getByRole('button', { name: /当前权限模式：Auto/ }).click()
    await expect(fixture.page.getByRole('menu', { name: '权限模式' })).toBeVisible()
    await expect(fixture.page.getByRole('menuitem', { name: 'Plan Mode' })).toBeVisible()
    await fixture.page.keyboard.press('Escape')
    await expect(fixture.page.getByRole('button', { name: '设置' })).toBeVisible()
    await expect(hud).toContainText('72%')
    await expect(hud).toContainText('Weekly 8%')
    await expect(hud.getByRole('button', { name: '编辑 ClaudeMd' })).toBeVisible()
    await expect(hud).toContainText('1 MCPs')
    await expect(hud).toContainText('2 hooks')
    await expect(hud).toContainText(/\d+ Tools/)
    await expect(hud).toContainText('⚠ browser_bridge')
    await expect(hud.locator('.context-ring-fg')).toHaveAttribute('stroke', '#d29922')
    await expect(hud).not.toContainText('任务中')
    await expect(hud).toContainText('▸实现 HUD(1/2)')
    await hud.getByText('1 MCPs').hover()
    const mcpTooltip = fixture.page.getByRole('tooltip')
    await expect(mcpTooltip).toContainText('browser_bridge')
    await mcpTooltip.hover()
    await expect(mcpTooltip).toBeVisible()
    await fixture.page.mouse.move(900, 500)

    await hud.getByRole('button', { name: '编辑 ClaudeMd' }).click()
    const instructionDialog = fixture.page.getByRole('dialog', { name: '编辑 ClaudeMd' })
    await expect(instructionDialog).toBeVisible()
    await expect(instructionDialog.getByRole('textbox', { name: 'ClaudeMd 内容' }))
      .toHaveValue('project instructions\n')
    await mkdir(evidenceDirectory, { recursive: true })
    await fixture.page.screenshot({
      path: join(evidenceDirectory, 'claude-md-editor.png')
    })
    await instructionDialog.getByRole('textbox', { name: 'ClaudeMd 内容' })
      .fill('# updated project instructions\n')
    await instructionDialog.getByRole('button', { name: '保存', exact: true }).click()
    await expect(instructionDialog).toBeHidden()
    await expect.poll(() => readFile(join(fixture.workspaceDirectory, 'CLAUDE.md'), 'utf8'))
      .toBe('# updated project instructions\n')
    await fixture.page.locator('.hierarchy-shell').screenshot({
      path: join(evidenceDirectory, 'agent-hud.png')
    })
    await writeFile(join(evidenceDirectory, 'agent-hud.json'), JSON.stringify(
      await hudGeometry(fixture.page.locator('body')), null, 2
    ))
    await rm(join(fixture.workspaceDirectory, 'CLAUDE.md'))
    await expect(hud.getByRole('button', { name: '编辑 ClaudeMd' })).toHaveCount(0)

    await fixture.page.getByRole('button', { name: '设置' }).click()
    const providerSettings = fixture.page.getByRole('region', { name: '模型切换设置' })
    await expect(providerSettings.getByRole('heading', { name: '模型切换' })).toBeVisible()
    await expect(providerSettings).toContainText('Anthropic 官方')
    const settingsBounds = await providerSettings.boundingBox()
    const frameBounds = await providerSettings.locator('.model-settings__frame').boundingBox()
    expect(settingsBounds).not.toBeNull()
    expect(frameBounds).not.toBeNull()
    expect({
      left: roundedGap(frameBounds!.x - settingsBounds!.x),
      top: roundedGap(frameBounds!.y - settingsBounds!.y),
      right: roundedGap(settingsBounds!.x + settingsBounds!.width - frameBounds!.x - frameBounds!.width),
      bottom: roundedGap(settingsBounds!.y + settingsBounds!.height - frameBounds!.y - frameBounds!.height)
    }).toEqual({ left: 0, top: 0, right: 0, bottom: 0 })
    expect(await providerSettings.evaluate((element) => {
      const frame = element.querySelector<HTMLElement>('.model-settings__frame')!
      const providers = element.querySelector<HTMLElement>('.model-settings__providers')!
      const row = element.querySelector<HTMLElement>('.model-provider')!
      return {
        canvas: getComputedStyle(element).backgroundColor,
        frame: getComputedStyle(frame).backgroundColor,
        providerBorder: getComputedStyle(providers).borderTopWidth,
        providerRadius: getComputedStyle(providers).borderRadius,
        rowRadius: getComputedStyle(row).borderRadius
      }
    })).toEqual({
      canvas: 'rgb(247, 248, 250)',
      frame: 'rgba(0, 0, 0, 0)',
      providerBorder: '1px',
      providerRadius: '10px',
      rowRadius: '0px'
    })
    await fixture.page.waitForTimeout(250)
    await fixture.page.locator('.hierarchy-shell').screenshot({
      path: join(evidenceDirectory, 'model-switch-settings.png')
    })
    await providerSettings.getByRole('button', { name: '关闭设置' }).click()

    await fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.setSize(700, 520))
    await expect.poll(() => hud.evaluate((element) => ({
      whiteSpace: getComputedStyle(element).whiteSpace,
      overflow: getComputedStyle(element).overflow
    }))).toEqual({ whiteSpace: 'nowrap', overflow: 'hidden' })

    const beforeExitPid = await positivePid(surface)
    await writeFile(exitFile, 'exit')
    await expect(hud).toHaveAttribute('data-hud-mode', 'shell')
    await expect(fixture.page.locator('[data-testid="terminal-pane"]:visible .pane-title'))
      .toHaveText('Shell')
    await expect(hud.locator('.status-perm-badge')).toHaveCount(0)
    await expect.poll(() => positivePid(surface)).not.toBe(beforeExitPid)
  } finally {
    await fixture.close()
    await rm(providerRoot, { recursive: true, force: true })
  }
})

function roundedGap(value: number): number {
  const rounded = Math.round(value)
  return Object.is(rounded, -0) ? 0 : rounded
}

function activeSurface(root: Locator): Locator {
  return root.locator('.scene-stage:not([hidden]) [data-testid="terminal-pane"] .terminal-surface').first()
}

async function positivePid(surface: Locator): Promise<number> {
  let pid = 0
  await expect.poll(async () => {
    pid = Number(await surface.getAttribute('data-pid'))
    return pid
  }).toBeGreaterThan(0)
  return pid
}

async function terminalCommand(surface: Locator, command: string): Promise<void> {
  const textarea = surface.locator('.xterm-helper-textarea')
  await textarea.focus()
  await textarea.pressSequentially(command)
  await textarea.press('Enter')
}

async function hudGeometry(root: Locator) {
  return root.evaluate((body) => {
    const bar = body.querySelector<HTMLElement>('.shortcut-bar')!
    const hud = body.querySelector<HTMLElement>('.shortcut-bar .status-info')!
    const barStyle = getComputedStyle(bar)
    const hudStyle = getComputedStyle(hud)
    return {
      mode: hud.dataset.hudMode,
      text: hud.textContent,
      fields: [...hud.querySelectorAll(':scope .status-field, :scope .team-role-badge, :scope .status-divider')].map((element) => ({
        className: element.className,
        text: element.textContent
      })),
      bar: {
        height: barStyle.height,
        paddingLeft: barStyle.paddingLeft,
        paddingRight: barStyle.paddingRight,
        gap: barStyle.gap
      },
      hud: {
        maxWidth: hudStyle.maxWidth,
        fontSize: hudStyle.fontSize,
        gap: hudStyle.gap,
        whiteSpace: hudStyle.whiteSpace,
        overflow: hudStyle.overflow
      }
    }
  })
}

function providerScript(): string {
  return `#!/bin/sh
printf '%s\\n' "$*" >> "$MATOU_PRD02_INVOCATIONS"
settings=''
while [ "$#" -gt 0 ]; do
  if [ "$1" = '--settings' ]; then settings="$2"; shift 2; else shift; fi
done
python3 - "$settings" <<'PY'
import json, os, sys, urllib.request
settings=json.load(open(sys.argv[1]))
url=settings['hooks']['UserPromptSubmit'][0]['hooks'][0]['url']
def post(value):
    request=urllib.request.Request(url, data=json.dumps(value).encode(), headers={'content-type':'application/json'}, method='POST')
    urllib.request.urlopen(request, timeout=2).read()
post({'session_id':'prd02-provider-session','cwd':os.getcwd(),'permission_mode':'auto','model':{'display_name':'Claude Fable 5'},'cost':{'total_duration_ms':3600000},'context_window':{'used_percentage':72,'context_window_size':1000000},'rate_limits':{'seven_day':{'used_percentage':8,'resets_at':int(__import__('time').time())+424800}}})
post({'hook_event_name':'PreToolUse','session_id':'prd02-provider-session','tool_name':'Read','tool_use_id':'read-1','tool_input':{'file_path':'src/App.tsx'}})
post({'hook_event_name':'PostToolUse','session_id':'prd02-provider-session','tool_name':'Read','tool_use_id':'read-1','tool_input':{'file_path':'src/App.tsx'}})
post({'hook_event_name':'PreToolUse','session_id':'prd02-provider-session','tool_name':'TodoWrite','tool_use_id':'todo-1','tool_input':{'todos':[{'content':'实现 HUD','status':'in_progress'},{'content':'完成对照','status':'completed'}]}})
post({'hook_event_name':'PreToolUse','session_id':'prd02-provider-session','tool_name':'mcp__browser_bridge__open','tool_use_id':'mcp-1','tool_input':{'query':'open'}})
post({'hook_event_name':'PostToolUseFailure','session_id':'prd02-provider-session','tool_name':'mcp__browser_bridge__open','tool_use_id':'mcp-1','tool_input':{'query':'open'}})
PY
while [ ! -f "$MATOU_PRD02_EXIT_FILE" ]; do sleep 0.1; done
exit 0
`
}
