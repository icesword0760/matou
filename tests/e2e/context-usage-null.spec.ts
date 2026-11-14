import { chmod, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { expect, test } from '@playwright/test'
import { launchMatou } from './matou-fixture'

test('invalidates stale context usage and displays the next provider measurement', async () => {
  const root = await mkdtemp('/tmp/matou-context-ui-')
  const provider = join(root, 'provider.py')
  const endpoint = join(root, 'endpoint')
  await writeFile(provider, `#!/usr/bin/env python3
import json,os,sys,time
args=sys.argv[1:]
settings=json.load(open(args[args.index('--settings')+1]))
open(os.environ['MATOU_CONTEXT_PROBE_ENDPOINT'],'w').write(settings['hooks']['UserPromptSubmit'][0]['hooks'][0]['url'])
while True: time.sleep(1)
`)
  await chmod(provider, 0o755)
  const f = await launchMatou({ env: { MATOU_CLAUDE_COMMAND: provider, MATOU_CONTEXT_PROBE_ENDPOINT: endpoint } })
  try {
    const terminal = f.page.locator('.scene-stage:not([hidden]) .terminal-surface').first()
    await expect(terminal).toHaveAttribute('data-pid', /^[1-9]\d*$/)
    const input = terminal.locator('.xterm-helper-textarea')
    await input.focus(); await input.pressSequentially('claude'); await input.press('Enter')
    await expect.poll(async () => readFile(endpoint, 'utf8').catch(() => '')).not.toBe('')
    const url = await readFile(endpoint, 'utf8')
    const post = async (context: object) => {
      const response = await fetch(url, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({
        hook_event_name: 'UserPromptSubmit', session_id: 'context-ui-provider', cwd: f.workspaceDirectory,
        model: { display_name: 'GPT context fixture' }, context_window: { context_window_size: 512000, ...context }
      }) })
      expect(response.status).toBe(200)
    }
    const hud = f.page.locator('.shortcut-bar .status-info')
    await post({ used_percentage: 100 }); await expect(hud).toContainText('100%')
    await post({ used_percentage: null, current_usage: null })
    await expect(hud).toContainText('等待真实用量'); await expect(hud.locator('.status-context-ring')).toHaveCount(0)
    await f.page.locator('.hierarchy-shell').screenshot({ path: resolve('docs/acceptance/evidence/provider-activation-2026-11-14/context-waiting.png') })
    await post({ used_percentage: 0 }); await expect(hud).toContainText('0%'); await expect(hud).not.toContainText('等待真实用量')
    await post({ used_percentage: 21 }); await expect(hud).toContainText('21%')
    await f.page.locator('.hierarchy-shell').screenshot({ path: resolve('docs/acceptance/evidence/provider-activation-2026-11-14/context-measured.png') })
  } finally { await f.close(); await rm(root, { recursive: true, force: true }) }
})
