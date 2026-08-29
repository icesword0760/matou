import { chmod, mkdir, readFile, writeFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { DatabaseSync } from 'node:sqlite'

import { expect, test, type Locator, type Page } from '@playwright/test'

import { launchMatou, restartMatou, type MatouFixture } from './matou-fixture'

const evidenceDirectory = resolve(import.meta.dirname, '../../docs/acceptance/evidence/prd-06/matou')

test.describe('PRD 06 session fork', () => {
  test.setTimeout(60_000)

  test('forks a resumable Claude conversation on its right, isolates every branch, and restores once', async () => {
    let fixture: MatouFixture = await launchMatou()
    const provider = join(fixture.rootDirectory, 'claude-fork-fixture.py')
    const invocationLog = join(fixture.rootDirectory, 'fork-invocations.jsonl')
    const inputDirectory = join(fixture.rootDirectory, 'provider-inputs')
    await mkdir(inputDirectory)
    await writeFile(provider, providerScript())
    await chmod(provider, 0o755)
    await writeFile(join(fixture.rootDirectory, '.zshrc'), "alias cc='claude --dangerously-skip-permissions'\n")
    const environment = {
      MATOU_CLAUDE_COMMAND: provider,
      MATOU_PRD06_INVOCATIONS: invocationLog,
      MATOU_PRD06_INPUT_DIR: inputDirectory,
      SHELL: '/bin/zsh',
      ZDOTDIR: fixture.rootDirectory
    }
    try {
      fixture = await restartMatou(fixture, { env: environment })
      await fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.setSize(1800, 900))
      const source = visibleSurfaces(fixture.page).first()
      const sourceSessionId = await requiredAttribute(source, 'data-session-id')
      const sourcePane = paneForSession(fixture.page, sourceSessionId)
      await expect(sourcePane.locator('.pane-title')).toHaveText('Shell')
      await openPaneMenu(sourcePane)
      await expect(fixture.page.getByRole('menuitem', { name: '⑂ Fork 会话' })).toHaveCount(0)
      await expect(fixture.page.getByRole('menuitem', { name: '↗ 独立窗口' })).toBeVisible()
      await fixture.page.locator('.detach-context-overlay').click()
      await terminalCommand(source, 'cc')
      await expect(source.locator('.xterm-rows')).toContainText('READY:provider-source-e2e')
      await terminalCommand(source, 'INITIAL_CONVERSATION')
      await expect(source.locator('.xterm-rows'))
        .toContainText('REPLY:provider-source-e2e:INITIAL_CONVERSATION')

      await expect(sourcePane.locator('.pane-title')).toHaveText('Claude')
      await expect.poll(async () => (await invocationLines(invocationLog))[0]?.args)
        .toContain('--dangerously-skip-permissions')
      await expect(sourcePane.getByRole('button', { name: '从“Claude”创建子分支' })).toBeVisible()
      await openPaneMenu(sourcePane)
      await expect(fixture.page.getByRole('menuitem', { name: '⑂ Fork 会话' })).toBeVisible()
      await expect(fixture.page.getByRole('menuitem', { name: '↗ 独立窗口' })).toBeVisible()
      await mkdir(evidenceDirectory, { recursive: true })
      await fixture.page.locator('.hierarchy-shell').screenshot({
        path: join(evidenceDirectory, 'fork-menu.png')
      })
      await writeFile(join(evidenceDirectory, 'fork-menu.json'), JSON.stringify(
        await menuGeometry(fixture.page), null, 2
      ))
      await fixture.page.getByRole('menuitem', { name: '⑂ Fork 会话' }).click()
      await confirmCurrentWorktreeFork(fixture.page, '第一子分支')

      await expect(visibleSurfaces(fixture.page)).toHaveCount(1)
      const firstFork = visibleSurfaces(fixture.page).filter({ hasText: 'READY:provider-fork-e2e-1' })
      await expect(firstFork).toHaveCount(1)
      const firstForkSessionId = await requiredAttribute(firstFork, 'data-session-id')
      await expect(firstFork.locator('.xterm-helper-textarea')).toBeFocused()
      const firstForkPane = paneForSession(fixture.page, firstForkSessionId)
      await expect(firstForkPane.locator('.pane-title')).toHaveText('Claude')
      await terminalCommand(firstFork, 'FIRST_FORK_ONLY')
      await expect(firstFork.locator('.xterm-rows')).toContainText('REPLY:provider-fork-e2e-1:FIRST_FORK_ONLY')
      expect(await readProviderInput(inputDirectory, 'provider-source-e2e'))
        .not.toContain('FIRST_FORK_ONLY')

      await openPaneMenu(firstForkPane)
      await fixture.page.getByRole('menuitem', { name: '⑂ Fork 会话' }).click()
      await confirmCurrentWorktreeFork(fixture.page, '孙分支')
      await expect(visibleSurfaces(fixture.page)).toHaveCount(1)
      const secondFork = visibleSurfaces(fixture.page).filter({ hasText: 'READY:provider-fork-e2e-2' })
      await expect(secondFork).toHaveCount(1)
      const secondForkSessionId = await requiredAttribute(secondFork, 'data-session-id')
      await expect(secondFork.locator('.xterm-helper-textarea')).toBeFocused()
      await expect(paneForSession(fixture.page, secondForkSessionId).locator('.pane-title'))
        .toHaveText('Claude')
      await terminalCommand(secondFork, 'SECOND_FORK_ONLY')
      await expect(secondFork.locator('.xterm-rows'))
        .toContainText('REPLY:provider-fork-e2e-2:SECOND_FORK_ONLY')
      expect(await readProviderInput(inputDirectory, 'provider-fork-e2e-1'))
        .not.toContain('SECOND_FORK_ONLY')

      const databaseBeforeRestart = readForkDatabase(join(fixture.dataDirectory, 'matou.sqlite'))
      expect(databaseBeforeRestart.sessions).toEqual(expect.arrayContaining([
        { id: sourceSessionId, cwd: fixture.workspaceDirectory, kind: 'claude-code' },
        { id: firstForkSessionId, cwd: fixture.workspaceDirectory, kind: 'claude-code' },
        { id: secondForkSessionId, cwd: fixture.workspaceDirectory, kind: 'claude-code' }
      ]))
      expect(databaseBeforeRestart.intents).toEqual([
        { sessionId: firstForkSessionId, sourceSessionId, state: 'succeeded' },
        { sessionId: secondForkSessionId, sourceSessionId: firstForkSessionId, state: 'succeeded' }
      ])
      expect(databaseBeforeRestart.relations).toEqual([
        { fromSessionId: firstForkSessionId, toSessionId: sourceSessionId, kind: 'forked-from' },
        { fromSessionId: secondForkSessionId, toSessionId: firstForkSessionId, kind: 'forked-from' }
      ])
      await fixture.page.locator('.hierarchy-shell').screenshot({
        path: join(evidenceDirectory, 'forked-conversations.png')
      })

      const forkLaunchCount = (await invocationLines(invocationLog))
        .filter(({ args }) => args.includes('--fork-session')).length
      expect(forkLaunchCount).toBe(2)
      fixture = await restartMatou(fixture, { env: environment })
      await expect(visibleSurfaces(fixture.page)).toHaveCount(1)
      await expect(paneForSession(fixture.page, secondForkSessionId).locator('.xterm-rows'))
        .toContainText('READY:provider-fork-e2e-2')
      await fixture.page.getByRole('button', { name: '返回父会话' }).click()
      await expect(paneForSession(fixture.page, firstForkSessionId).locator('.xterm-rows'))
        .toContainText('READY:provider-fork-e2e-1')
      await fixture.page.getByRole('button', { name: '返回父会话' }).click()
      await expect(paneForSession(fixture.page, sourceSessionId).locator('.xterm-rows'))
        .toContainText('READY:provider-source-e2e')
      await expect.poll(async () => (await invocationLines(invocationLog)).length).toBeGreaterThanOrEqual(6)
      const restoredInvocations = await invocationLines(invocationLog)
      expect(restoredInvocations.filter(({ args }) => args.includes('--fork-session'))).toHaveLength(2)
      expect(restoredInvocations.some(({ args }) =>
        args.includes('--resume') && args.includes('provider-source-e2e')
      )).toBe(true)
      expect(restoredInvocations.some(({ args }) =>
        args.includes('--resume') && args.includes('provider-fork-e2e-1') && !args.includes('--fork-session')
      )).toBe(true)
      expect(restoredInvocations.some(({ args }) =>
        args.includes('--resume') && args.includes('provider-fork-e2e-2') && !args.includes('--fork-session')
      )).toBe(true)

      const sourceAfterRestart = paneForSession(fixture.page, sourceSessionId).locator('.terminal-surface')
      const sourcePid = await positivePid(sourceAfterRestart)
      await expect(sourceAfterRestart).toHaveAttribute('data-pid', String(sourcePid))

      await openPaneMenu(paneForSession(fixture.page, sourceSessionId))
      await fixture.page.getByRole('menuitem', { name: '↗ 独立窗口' }).click()
      await expect(fixture.page.getByTestId('detached-placeholder')).toContainText('已脱出')
      await expect.poll(async () => (await fixture.app.windows()).length).toBe(2)
      const detached = (await fixture.app.windows()).find((candidate) => candidate !== fixture.page)!
      await expect(detached.locator('.terminal-surface')).toHaveAttribute('data-session-id', sourceSessionId)
      await detached.locator('.terminal-surface').dispatchEvent('contextmenu')
      await expect(detached.getByText('⑂ Fork 会话')).toHaveCount(0)
      await detached.close()
    } finally {
      await fixture.close()
    }
  })

  test('keeps a failed fork visible and inert instead of opening a Shell or reusing the source', async () => {
    let fixture: MatouFixture = await launchMatou()
    const provider = join(fixture.rootDirectory, 'claude-fork-failure-fixture.py')
    const invocationLog = join(fixture.rootDirectory, 'fork-failure-invocations.jsonl')
    const inputDirectory = join(fixture.rootDirectory, 'provider-inputs')
    await mkdir(inputDirectory)
    await writeFile(provider, providerScript())
    await chmod(provider, 0o755)
    try {
      fixture = await restartMatou(fixture, { env: {
        MATOU_CLAUDE_COMMAND: provider,
        MATOU_PRD06_INVOCATIONS: invocationLog,
        MATOU_PRD06_INPUT_DIR: inputDirectory,
        MATOU_PRD06_FAIL_FORK: '1'
      } })
      const source = visibleSurfaces(fixture.page).first()
      await terminalCommand(source, 'claude')
      await expect(source.locator('.xterm-rows')).toContainText('READY:provider-source-e2e')
      await terminalCommand(source, 'INITIAL_CONVERSATION')
      await expect(source.locator('.xterm-rows'))
        .toContainText('REPLY:provider-source-e2e:INITIAL_CONVERSATION')
      const sourceSessionId = await requiredAttribute(source, 'data-session-id')
      await expect(paneForSession(fixture.page, sourceSessionId)
        .getByRole('button', { name: '从“Claude”创建子分支' })).toBeVisible()
      await openPaneMenu(paneForSession(fixture.page, sourceSessionId))
      await fixture.page.getByRole('menuitem', { name: '⑂ Fork 会话' }).click()
      await confirmCurrentWorktreeFork(fixture.page, '失败分支')

      await expect(visibleSurfaces(fixture.page)).toHaveCount(0)
      const failedCard = fixture.page.locator('[data-session-card]').filter({
        has: fixture.page.locator('.fork-failure-card')
      })
      await expect(failedCard).toHaveCount(1)
      await expect(failedCard).toContainText('分支创建失败')
      await expect(failedCard).toContainText('provider session not found')
      const failedSessionId = await requiredAttribute(failedCard, 'data-session-card')
      const beforeInputs = await allProviderInput(fixture.rootDirectory)
      await expect(failedCard.getByRole('textbox', { name: 'Terminal input' })).toHaveCount(0)
      await expect.poll(() => allProviderInput(fixture.rootDirectory)).toBe(beforeInputs)
      expect(readForkDatabase(join(fixture.dataDirectory, 'matou.sqlite')).intents)
        .toContainEqual({ sessionId: failedSessionId, sourceSessionId, state: 'failed' })
      await mkdir(evidenceDirectory, { recursive: true })
      await fixture.page.locator('.hierarchy-shell').screenshot({
        path: join(evidenceDirectory, 'fork-failure.png')
      })
    } finally {
      await fixture.close()
    }
  })
})

function visibleSurfaces(page: Page): Locator {
  return page.locator('.scene-stage:not([hidden]) [data-testid="terminal-pane"]:visible .terminal-surface')
}

function paneForSession(page: Page, sessionId: string): Locator {
  return page.locator(`[data-testid="terminal-pane"]:visible:has(.terminal-surface[data-session-id="${sessionId}"])`)
}

async function openPaneMenu(pane: Locator): Promise<void> {
  // The menu overlay is mounted by this real context-menu pointer sequence.
  // Avoid retrying that already-successful sequence after the overlay appears.
  await pane.locator('.terminal-surface').click({
    button: 'right', position: { x: 24, y: 80 }, force: true
  })
}

async function confirmCurrentWorktreeFork(page: Page, name: string): Promise<void> {
  await page.getByLabel('分支名称').fill(name)
  await expect(page.getByRole('radio', { name: /使用当前工作树/ })).toBeChecked()
  await page.getByRole('button', { name: '创建分支', exact: true }).click()
}

async function terminalCommand(surface: Locator, command: string): Promise<void> {
  const textarea = surface.locator('.xterm-helper-textarea')
  await textarea.focus()
  await textarea.pressSequentially(command)
  await textarea.press('Enter')
}

async function positivePid(surface: Locator): Promise<number> {
  let pid = 0
  await expect.poll(async () => {
    pid = Number(await surface.getAttribute('data-pid'))
    return pid
  }).toBeGreaterThan(0)
  return pid
}

async function requiredAttribute(locator: Locator, name: string): Promise<string> {
  const value = await locator.getAttribute(name)
  if (!value) throw new Error(`Expected ${name}`)
  return value
}

async function invocationLines(path: string): Promise<Array<{ args: string[]; providerId: string }>> {
  const text = await readFile(path, 'utf8').catch(() => '')
  return text.trim().split('\n').filter(Boolean).map((line) => JSON.parse(line))
}

async function allProviderInput(root: string): Promise<string> {
  const directory = join(root, 'provider-inputs')
  const { readdir } = await import('node:fs/promises')
  const names = await readdir(directory).catch(() => [])
  const values = await Promise.all(names.sort().map(async (name) =>
    `${name}:${await readFile(join(directory, name), 'utf8').catch(() => '')}`
  ))
  return values.join('\n')
}

async function readProviderInput(directory: string, providerId: string): Promise<string> {
  return readFile(join(directory, `${providerId}.txt`), 'utf8').catch(() => '')
}

function readForkDatabase(path: string): {
  sessions: Array<{ id: string; cwd: string; kind: string }>
  intents: Array<{ sessionId: string; sourceSessionId: string; state: string }>
  relations: Array<{ fromSessionId: string; toSessionId: string; kind: string }>
} {
  const database = new DatabaseSync(path, { readOnly: true })
  try {
    return {
      sessions: database.prepare(
        `SELECT id, cwd, kind FROM sessions WHERE archived_at IS NULL ORDER BY created_at, id`
      ).all().map((row) => ({ id: String(row.id), cwd: String(row.cwd), kind: String(row.kind) })),
      intents: database.prepare(
        `SELECT session_id, source_session_id, state FROM session_fork_intents ORDER BY created_at, session_id`
      ).all().map((row) => ({
        sessionId: String(row.session_id), sourceSessionId: String(row.source_session_id),
        state: String(row.state)
      })),
      relations: database.prepare(
        `SELECT from_session_id, to_session_id, relation_kind
         FROM session_relations_current WHERE relation_kind = 'forked-from'
         ORDER BY created_at, from_session_id`
      ).all().map((row) => ({
        fromSessionId: String(row.from_session_id), toSessionId: String(row.to_session_id),
        kind: String(row.relation_kind)
      }))
    }
  } finally {
    database.close()
  }
}

async function menuGeometry(page: Page): Promise<unknown> {
  return page.locator('.detach-context-menu').evaluate((menu) => {
    const menuStyle = getComputedStyle(menu)
    const items = [...menu.querySelectorAll<HTMLElement>('.detach-menu-item')]
    return {
      text: items.map(({ textContent }) => textContent),
      menu: {
        minWidth: menuStyle.minWidth, padding: menuStyle.padding,
        borderRadius: menuStyle.borderRadius, backgroundColor: menuStyle.backgroundColor,
        border: menuStyle.border, boxShadow: menuStyle.boxShadow,
        backdropFilter: menuStyle.backdropFilter, zIndex: menuStyle.zIndex
      },
      item: items[0] ? {
        padding: getComputedStyle(items[0]).padding,
        fontSize: getComputedStyle(items[0]).fontSize
      } : null
    }
  })
}

function providerScript(): string {
  return `#!/usr/bin/env python3
import fcntl, json, os, pathlib, sys, time, urllib.request

args=sys.argv[1:]
settings=''
resume=''
fork='--fork-session' in args
for i, value in enumerate(args):
    if value == '--settings' and i + 1 < len(args): settings=args[i + 1]
    if value == '--resume' and i + 1 < len(args): resume=args[i + 1]

counter_path=os.environ['MATOU_PRD06_INVOCATIONS'] + '.counter'
if fork:
    with open(counter_path, 'a+') as counter:
        fcntl.flock(counter, fcntl.LOCK_EX)
        counter.seek(0)
        value=int(counter.read() or '0') + 1
        counter.seek(0); counter.truncate(); counter.write(str(value)); counter.flush()
        provider_id=f'provider-fork-e2e-{value}'
elif resume:
    provider_id=resume
else:
    provider_id='provider-source-e2e'

with open(os.environ['MATOU_PRD06_INVOCATIONS'], 'a') as log:
    log.write(json.dumps({'args': args, 'providerId': provider_id}) + '\\n')

if fork and os.environ.get('MATOU_PRD06_FAIL_FORK') == '1':
    print('No session found for requested id', flush=True)
    while True: time.sleep(1)

settings_data=json.load(open(settings))
url=settings_data['hooks']['UserPromptSubmit'][0]['hooks'][0]['url']
def hook(name):
    body=json.dumps({'hook_event_name':name,'session_id':provider_id,'cwd':os.getcwd()}).encode()
    request=urllib.request.Request(url, data=body, headers={'content-type':'application/json'}, method='POST')
    urllib.request.urlopen(request, timeout=3).read()
hook('SessionStart')
print('X' * 2050, flush=True)
print('READY:' + provider_id, flush=True)
input_path=pathlib.Path(os.environ['MATOU_PRD06_INPUT_DIR']) / (provider_id + '.txt')
for line in sys.stdin:
    value=line.rstrip('\\r\\n')
    hook('UserPromptSubmit')
    with input_path.open('a') as output: output.write(value + '\\n')
    print(f'REPLY:{provider_id}:{value}', flush=True)
    hook('Stop')
`
}
