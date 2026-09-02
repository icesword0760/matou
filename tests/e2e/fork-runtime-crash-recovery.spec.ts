import { execFile } from 'node:child_process'
import { chmod, mkdir, readFile, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { DatabaseSync } from 'node:sqlite'
import { promisify } from 'node:util'

import { expect, test, type Locator, type Page } from '@playwright/test'

import { readSessionFrames } from '../../apps/runtime/src/journal/segment-journal'
import { launchMatou, restartMatou, type MatouFixture } from './matou-fixture'

const execFileAsync = promisify(execFile)
const killPoints = [
  'intent-accepted',
  'branch-created',
  'path-created',
  'setup-completed',
  'session-bound',
  'provider-before'
] as const

test.describe('Fork operation Runtime crash recovery', () => {
  test.setTimeout(90_000)

  for (const killPoint of killPoints) {
    test(`finishes one real isolated branch after Runtime dies at ${killPoint}`, async () => {
      let fixture: MatouFixture = await launchMatou()
      const provider = join(fixture.rootDirectory, 'fork-provider.py')
      const invocationLog = join(fixture.rootDirectory, 'fork-invocations.jsonl')
      const inputDirectory = join(fixture.rootDirectory, 'provider-inputs')
      const crashMarker = join(fixture.rootDirectory, `crash-${killPoint}.json`)
      await initializeRepository(fixture.workspaceDirectory)
      await mkdir(inputDirectory)
      await writeFile(provider, providerScript())
      await chmod(provider, 0o755)
      await writeFile(join(fixture.rootDirectory, '.zshrc'), "alias cc='claude --dangerously-skip-permissions'\n")
      const environment = {
        MATOU_CLAUDE_COMMAND: provider,
        MATOU_FORK_CRASH_INVOCATIONS: invocationLog,
        MATOU_FORK_CRASH_INPUT_DIR: inputDirectory,
        MATOU_E2E_FORK_KILLPOINT: killPoint,
        MATOU_E2E_FORK_CRASH_MARKER: crashMarker,
        MATOU_E2E_SCALE: '1',
        SHELL: '/bin/zsh',
        ZDOTDIR: fixture.rootDirectory
      }
      try {
        fixture = await restartMatou(fixture, { env: environment })
        await fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.setSize(1800, 900))
        const source = visibleSurfaces(fixture.page).first()
        await terminalCommand(source, 'cc')
        await expect(source.locator('.xterm-rows')).toContainText('READY:provider-source-crash')
        await terminalCommand(source, 'SOURCE_READY')
        await expect(source.locator('.xterm-rows')).toContainText('REPLY:provider-source-crash:SOURCE_READY')
        const sourceSessionId = await requiredAttribute(source, 'data-session-id')
        const beforeRuntimePid = await runtimePid(fixture)

        await paneForSession(fixture.page, sourceSessionId)
          .getByRole('button', { name: /创建子分支/ }).click()
        await fixture.page.getByLabel('分支名称').fill(`恢复-${killPoint}`)
        const newWorktreeMode = fixture.page.getByRole('radio', { name: /从新工作树创建/ })
        await expect(newWorktreeMode).toBeEnabled({ timeout: 15_000 })
        await newWorktreeMode.check()
        await fixture.page.getByRole('button', { name: '创建分支', exact: true }).click()

        await expect.poll(async () => {
          const contents = await readFile(crashMarker, 'utf8').catch(() => '')
          return contents ? JSON.parse(contents) : undefined
        })
          .toMatchObject({ point: killPoint, runtimePid: beforeRuntimePid })
        await expect.poll(() => runtimePid(fixture).catch(() => beforeRuntimePid), {
          message: `Runtime must be replaced after ${killPoint}`
        }).not.toBe(beforeRuntimePid)
        expect(processExists(beforeRuntimePid)).toBe(false)

        await expect.poll(() => forkState(join(fixture.dataDirectory, 'matou.sqlite')).intents
          .map(({ stage }) => stage), {
          message: `the durable Fork operation must complete after ${killPoint}`,
          timeout: 30_000
        }).toEqual(['succeeded'])

        const completedState = forkState(join(fixture.dataDirectory, 'matou.sqlite'))
        const childSessionId = completedState.intents[0]!.sessionId
        await fixture.page.getByRole('button', { name: '通知中心' }).click()
        const center = fixture.page.getByRole('region', { name: '通知中心' })
        await expect(center).toContainText('新的分支会话已经可以继续工作')
        await center.getByRole('button', {
          name: '打开通知：新的分支会话已经可以继续工作'
        }).click()

        const child = fixture.page.locator(
          `.scene-stage:not([hidden]) [data-testid="terminal-pane"]:visible ` +
          `.terminal-surface[data-session-id="${childSessionId}"]`
        )
        await expect(child).toHaveCount(1, { timeout: 30_000 })
        const childPane = paneForSession(fixture.page, childSessionId)
        await expect(child).toHaveAttribute('data-pid', /[1-9][0-9]*/, { timeout: 30_000 })
        await expect(childPane).not.toHaveAttribute('aria-busy', 'true')
        await expect(childPane.locator('.fork-progress-overlay, .session-recovery-overlay')).toHaveCount(0)
        await expect(child.locator('.xterm-helper-textarea')).toBeFocused()
        await terminalCommand(child, `AFTER_${killPoint}`)
        await expect.poll(() => readFile(
          join(inputDirectory, 'provider-fork-crash-1.txt'), 'utf8'
        ).catch(() => ''), { timeout: 30_000 }).toContain(`AFTER_${killPoint}`)
        await expect.poll(async () => terminalJournalText(
          fixture.dataDirectory, childSessionId
        ), { timeout: 30_000 }).toContain(`REPLY:provider-fork-crash-1:AFTER_${killPoint}`)
        await expect(child.locator('.xterm-rows'))
          .toContainText(`REPLY:provider-fork-crash-1:AFTER_${killPoint}`, { timeout: 30_000 })

        const state = forkState(join(fixture.dataDirectory, 'matou.sqlite'))
        expect(state.intents).toEqual([{
          sessionId: childSessionId, sourceSessionId, stage: 'succeeded'
        }])
        expect(state.relations).toEqual([{
          fromSessionId: childSessionId, toSessionId: sourceSessionId
        }])
        expect(state.worktrees).toEqual([{ state: 'ready' }])
        const worktreeList = await execFileAsync(
          'git', ['worktree', 'list', '--porcelain'], { cwd: fixture.workspaceDirectory }
        )
        expect(worktreeList.stdout.match(/^worktree /gm)).toHaveLength(2)
        expect((await invocationLines(invocationLog)).filter(({ args }) =>
          args.includes('--fork-session'))).toHaveLength(1)

        await expect(childPane).toBeVisible()
      } finally {
        await fixture.close()
      }
    })
  }
})

function visibleSurfaces(page: Page): Locator {
  return page.locator('.scene-stage:not([hidden]) [data-testid="terminal-pane"]:visible .terminal-surface')
}

async function terminalJournalText(dataRoot: string, sessionId: string): Promise<string> {
  const frames = await readSessionFrames(dataRoot, sessionId).catch(() => [])
  return frames.filter((frame) => frame.kind === 'output')
    .map((frame) => new TextDecoder().decode(frame.data)).join('')
}

function paneForSession(page: Page, sessionId: string): Locator {
  return page.locator(`[data-testid="terminal-pane"]:visible:has(.terminal-surface[data-session-id="${sessionId}"])`)
}

async function terminalCommand(surface: Locator, command: string): Promise<void> {
  const textarea = surface.locator('.xterm-helper-textarea')
  await textarea.focus()
  await textarea.pressSequentially(command)
  await textarea.press('Enter')
}

async function requiredAttribute(locator: Locator, name: string): Promise<string> {
  const value = await locator.getAttribute(name)
  if (!value) throw new Error(`Expected ${name}`)
  return value
}

async function runtimePid(fixture: MatouFixture): Promise<number> {
  const metrics = await fixture.app.evaluate(async () => {
    const read = (globalThis as typeof globalThis & {
      __matouE2eScaleMetrics?: () => Promise<{ runtimePid: number }>
    }).__matouE2eScaleMetrics
    if (!read) throw new Error('Runtime metrics bridge is unavailable')
    return read()
  })
  return metrics.runtimePid
}

function processExists(pid: number): boolean {
  try {
    process.kill(pid, 0)
    return true
  } catch {
    return false
  }
}

async function initializeRepository(directory: string): Promise<void> {
  await execFileAsync('git', ['init'], { cwd: directory })
  await execFileAsync('git', ['config', 'user.email', 'matou-e2e@example.invalid'], { cwd: directory })
  await execFileAsync('git', ['config', 'user.name', 'Matou E2E'], { cwd: directory })
  await writeFile(join(directory, 'README.md'), 'fork crash recovery\n')
  await execFileAsync('git', ['add', 'README.md'], { cwd: directory })
  await execFileAsync('git', ['commit', '-m', 'initial'], { cwd: directory })
}

async function invocationLines(path: string): Promise<Array<{ args: string[] }>> {
  const text = await readFile(path, 'utf8').catch(() => '')
  return text.trim().split('\n').filter(Boolean).map((line) => JSON.parse(line))
}

function forkState(path: string): {
  intents: Array<{ sessionId: string; sourceSessionId: string; stage: string }>
  relations: Array<{ fromSessionId: string; toSessionId: string }>
  worktrees: Array<{ state: string }>
} {
  const database = new DatabaseSync(path, { readOnly: true })
  try {
    return {
      intents: database.prepare(
        'SELECT session_id, source_session_id, stage FROM session_fork_intents'
      ).all().map((row) => ({
        sessionId: String(row.session_id), sourceSessionId: String(row.source_session_id),
        stage: String(row.stage)
      })),
      relations: database.prepare(
        "SELECT from_session_id, to_session_id FROM session_relations_current WHERE relation_kind = 'forked-from'"
      ).all().map((row) => ({
        fromSessionId: String(row.from_session_id), toSessionId: String(row.to_session_id)
      })),
      worktrees: database.prepare('SELECT state FROM worktrees').all()
        .map((row) => ({ state: String(row.state) }))
    }
  } finally {
    database.close()
  }
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

counter_path=os.environ['MATOU_FORK_CRASH_INVOCATIONS'] + '.counter'
if fork:
    with open(counter_path, 'a+') as counter:
        fcntl.flock(counter, fcntl.LOCK_EX)
        counter.seek(0)
        value=int(counter.read() or '0') + 1
        counter.seek(0); counter.truncate(); counter.write(str(value)); counter.flush()
        provider_id=f'provider-fork-crash-{value}'
elif resume:
    provider_id=resume
else:
    provider_id='provider-source-crash'

with open(os.environ['MATOU_FORK_CRASH_INVOCATIONS'], 'a') as log:
    log.write(json.dumps({'args': args, 'providerId': provider_id}) + '\\n')

settings_data=json.load(open(settings))
url=settings_data['hooks']['UserPromptSubmit'][0]['hooks'][0]['url']
def hook(name):
    body=json.dumps({'hook_event_name':name,'session_id':provider_id,'cwd':os.getcwd()}).encode()
    request=urllib.request.Request(url, data=body, headers={'content-type':'application/json'}, method='POST')
    urllib.request.urlopen(request, timeout=3).read()
hook('SessionStart')
print('READY:' + provider_id, flush=True)
input_path=pathlib.Path(os.environ['MATOU_FORK_CRASH_INPUT_DIR']) / (provider_id + '.txt')
for line in sys.stdin:
    value=line.rstrip('\\r\\n')
    hook('UserPromptSubmit')
    with input_path.open('a') as output: output.write(value + '\\n')
    print(f'REPLY:{provider_id}:{value}', flush=True)
    hook('Stop')
`
}
