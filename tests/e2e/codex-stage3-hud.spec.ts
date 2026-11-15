import { chmod, mkdir, mkdtemp, readFile, writeFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { expect, test } from '@playwright/test'
import { launchMatou } from './matou-fixture'

// Real Electron/PTY/HUD pipeline with a deterministic Codex provider fixture.
// This does not substitute for the live Codex TUI probe documented separately.
test('Codex HUD, late automatic title, manual lock/restore, and AGENTS.md save', async () => {
  const root = await mkdtemp('/tmp/matou-codex-hud-')
  const codexRoot = join(root, 'codex')
  const provider = join(root, 'codex-fixture.cjs')
  await mkdir(codexRoot)
  await writeFile(provider, providerScript)
  await chmod(provider, 0o755)
  const fixture = await launchMatou({ root, env: { CODEX_HOME: codexRoot, MATOU_CODEX_COMMAND: provider } })
  const evidence = resolve('docs/acceptance/evidence/codex-stage3')
  await mkdir(evidence, { recursive: true })
  try {
    await fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.setSize(1500, 850))
    await writeFile(join(fixture.workspaceDirectory, 'AGENTS.md'), '# Codex instructions\n')
    await writeFile(join(fixture.workspaceDirectory, 'CLAUDE.md'), '# Leave unchanged\n')
    const surface = fixture.page.locator('.scene-stage:not([hidden]) .terminal-surface:visible').first()
    await expect.poll(async () => Number(await surface.getAttribute('data-pid'))).toBeGreaterThan(0)
    const input = surface.locator('.xterm-helper-textarea')
    await input.focus()
    await input.pressSequentially('codex', { delay: 20 })
    await input.press('Enter')
    const hud = fixture.page.locator('.shortcut-bar .status-info').first()
    await expect(hud).toHaveAttribute('data-hud-mode', 'agent')
    await expect(hud.locator('.status-model')).toContainText('gpt-6-astra (medium)')
    await expect(hud).toContainText('13%')
    await expect(hud).toContainText('1 Tools')
    await expect(hud.getByRole('button', { name: 'AGENTS.md', exact: true })).toBeVisible()
    await expect(hud.locator('.status-usage, .claude-accounts, .status-todos')).toHaveCount(0)
    await expect(hud).not.toContainText(/opusplan|Weekly|MCPs|Agents|ClaudeMd|等待真实用量/)
    const title = fixture.page.locator('[data-testid="terminal-pane"]:visible .pane-title').first()
    await expect(title).toHaveText('Codex 自动标题样本')
    await fixture.page.locator('.hierarchy-shell').screenshot({ path: join(evidence, 'hud.png') })

    await hud.getByRole('button', { name: 'AGENTS.md', exact: true }).click()
    const editor = fixture.page.getByRole('textbox', { name: 'AGENTS.md 内容' })
    await expect(editor).toHaveValue('# Codex instructions\n')
    await editor.fill('# Saved through Matou\n')
    await fixture.page.getByRole('dialog', { name: 'AGENTS.md', exact: true }).screenshot({ path: join(evidence, 'agents-editor.png') })
    await fixture.page.getByRole('button', { name: '保存', exact: true }).click()
    await expect.poll(() => readFile(join(fixture.workspaceDirectory, 'AGENTS.md'), 'utf8')).toBe('# Saved through Matou\n')
    expect(await readFile(join(fixture.workspaceDirectory, 'CLAUDE.md'), 'utf8')).toBe('# Leave unchanged\n')

    const header = fixture.page.locator('[data-testid="terminal-pane"]:visible .terminal-pane-header').first()
    await header.click({ button: 'right' })
    await fixture.page.getByRole('menuitem', { name: '重命名…', exact: true }).click()
    await fixture.page.getByRole('textbox', { name: '会话名称' }).fill('手动标题样本')
    await fixture.page.getByRole('button', { name: '确定', exact: true }).click()
    await writeFile(join(codexRoot, 'session_index.jsonl'), JSON.stringify({
      id: '01a1ddd6-4064-70e1-a006-79d310600776', thread_name: 'Codex 更新后的自动标题', updated_at: '2026-11-15T07:00:00Z'
    }) + '\n')
    await expect(title).toHaveText('手动标题样本')
    await header.click({ button: 'right' })
    await expect(fixture.page.getByRole('menuitem', { name: '恢复 Codex 自动标题', exact: true })).toBeVisible()
    await fixture.page.getByRole('menuitem', { name: '恢复 Codex 自动标题', exact: true }).click()
    await expect(title).toHaveText('Codex 更新后的自动标题')
    await fixture.page.locator('.hierarchy-shell').screenshot({ path: join(evidence, 'restored-title.png') })
  } finally { await fixture.close() }
})

const providerScript = `#!/usr/bin/env node
const fs = require('node:fs');
const path = require('node:path');
const { spawn } = require('node:child_process');
if (process.argv.includes('--version')) { console.log('codex-cli 0.162.1 (fixture)'); process.exit(0); }
const config = process.argv.find(arg => arg.startsWith('hooks=')) || '';
const match = /command = ("(?:[^"\\\\]|\\\\.)*")/.exec(config);
if (!match) { console.error('Missing injected hooks'); process.exit(1); }
const command = JSON.parse(match[1]);
const id = '01a1ddd6-4064-70e1-a006-79d310600776';
const rollout = path.join(process.env.CODEX_HOME, 'rollout.jsonl');
const record = (type, payload) => JSON.stringify({ type, payload });
fs.writeFileSync(rollout, [record('session_meta', { id, cwd: process.cwd() }),
  record('turn_context', { model: 'gpt-6-astra', effort: 'medium' }),
  record('event_msg', { type: 'token_count', info: { last_token_usage: { total_tokens: 34175 }, model_context_window: 258400 } }),
  record('event_msg', { type: 'item_completed', item: { type: 'CommandExecution', id: 'exec-sample', status: 'completed', exit_code: 0 } })
].join('\\n') + '\\n');
const hook = (event) => new Promise((resolve, reject) => {
  const child = spawn('/bin/sh', ['-c', command], { stdio: ['pipe', 'ignore', 'inherit'] });
  child.on('error', reject); child.on('exit', resolve);
  child.stdin.end(JSON.stringify({ hook_event_name: event, session_id: id, transcript_path: rollout, cwd: process.cwd(),
    model: 'gpt-6-astra', tool_name: 'Bash', tool_use_id: 'exec-sample', tool_input: { command: 'print(2)' } }));
});
(async () => {
  for (const event of ['SessionStart', 'UserPromptSubmit', 'PreToolUse', 'PostToolUse', 'Stop']) await hook(event);
  setTimeout(() => fs.writeFileSync(path.join(process.env.CODEX_HOME, 'session_index.jsonl'), JSON.stringify({
    id, thread_name: 'Codex 自动标题样本', updated_at: '2026-11-15T06:13:09Z' }) + '\\n'), 200);
  console.log('Codex HUD fixture ready');
  process.stdin.resume(); setInterval(() => {}, 1000);
})().catch(error => { console.error(error); process.exit(1); });
`
