import { execFile } from 'node:child_process'
import { randomUUID } from 'node:crypto'
import { chmod, mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { promisify } from 'node:util'

import { expect, type Locator, type Page } from '@playwright/test'

import { launchMatou, type MatouFixture } from '../matou-fixture'
import { terminalCommand, visibleSurfaces, waitForShell } from './session-canvas-fixture'

const execFileAsync = promisify(execFile)
const MT_REQUEST_PREFIX = '__MATOU_E2E_MT_REQUEST__:'

export interface AiHostControlFixture extends MatouFixture {
  providerInputDirectory: string
  providerEventLog: string
  repositoryDirectory: string
}

export interface MtInvocation<T = unknown> {
  status: number
  stdout: string
  stderr: string
  value: T
}

export interface ResumableProviderSession {
  sessionId: string
  providerId: string
  surface: Locator
}

export interface BranchCollisionSetup {
  triggerBranch: string
  blockerBranch: string
  collisionBranch: string
}

export interface LaunchAiHostControlOptions {
  branchCollision?: BranchCollisionSetup
  confirmationTtlMs?: number
  forkProviderReadyDelayMs?: number
}

export async function launchAiHostControl(
  options: LaunchAiHostControlOptions = {}
): Promise<AiHostControlFixture> {
  const root = await mkdtemp('/tmp/matou-ai-control-e2e-')
  const repositoryDirectory = join(root, 'matou_workspace')
  const provider = join(root, 'provider-fixture.py')
  const providerInputDirectory = join(root, 'provider-inputs')
  const providerEventLog = join(root, 'provider-events.jsonl')
  const environment: Record<string, string> = {
    MATOU_CLAUDE_COMMAND: provider,
    MATOU_AI_CONTROL_INPUT_DIR: providerInputDirectory,
    MATOU_AI_CONTROL_EVENT_LOG: providerEventLog,
    SHELL: '/bin/zsh',
    ZDOTDIR: root,
    // These acceptance cases verify real Runtime/Desktop behavior rather than
    // collecting display-specific screenshot evidence.
    MATOU_E2E_DISPLAY: 'primary'
  }
  if (options.confirmationTtlMs !== undefined) {
    environment.MATOU_E2E_CONFIRMATION_TTL_MS = String(options.confirmationTtlMs)
  }
  if (options.forkProviderReadyDelayMs !== undefined) {
    environment.MATOU_AI_CONTROL_FORK_READY_DELAY_MS = String(options.forkProviderReadyDelayMs)
  }

  try {
    await mkdir(repositoryDirectory, { recursive: true })
    await mkdir(providerInputDirectory, { recursive: true })
    await initializeRepository(repositoryDirectory)
    await writeFile(provider, providerScript())
    await chmod(provider, 0o755)
    await writeFile(join(root, '.zshrc'), "alias cc='claude --dangerously-skip-permissions'\n")

    if (options.branchCollision) {
      const setupScript = join(root, 'branch-collision-setup.sh')
      const setupControl = join(root, 'branch-collision-setup.json')
      const marker = join(root, 'branch-collision-ready')
      await writeFile(setupScript, branchCollisionScript())
      await chmod(setupScript, 0o755)
      await writeFile(setupControl, JSON.stringify({
        idempotencyKey: 'ai-control-branch-collision',
        command: setupScript,
        args: [
          repositoryDirectory,
          options.branchCollision.triggerBranch,
          options.branchCollision.blockerBranch,
          options.branchCollision.collisionBranch,
          marker
        ]
      }))
      environment.MATOU_E2E_FORK_SETUP_CONTROL = setupControl
    }

    const fixture = await launchMatou({ root, env: environment })
    return {
      ...fixture,
      providerInputDirectory,
      providerEventLog,
      repositoryDirectory,
      close: async () => {
        await fixture.app.evaluate(({ app }) => { app.quit() }).catch(() => {})
        await fixture.app.close().catch(() => {})
        await rm(root, {
          recursive: true, force: true, maxRetries: 10, retryDelay: 100
        })
      }
    }
  } catch (error) {
    await rm(root, { recursive: true, force: true })
    throw error
  }
}

export async function seedResumableProviderSession(
  fixture: AiHostControlFixture
): Promise<ResumableProviderSession> {
  await fixture.app.evaluate(({ BrowserWindow }) => BrowserWindow.getAllWindows()[0]?.setSize(1600, 900))
  const surface = visibleSurfaces(fixture.page).first()
  await waitForShell(surface)
  await expect(surface.locator('.xterm-rows')).toContainText('%')
  const sessionId = await requiredAttribute(surface, 'data-session-id')
  const textarea = surface.locator('.xterm-helper-textarea')
  await textarea.focus()
  await textarea.press('Control+U')
  await fixture.page.waitForTimeout(100)
  await terminalCommand(surface, 'cc')
  await expect(surface.locator('.xterm-rows')).toContainText('READY:provider-source-control-e2e', {
    timeout: 30_000
  })
  await terminalCommand(surface, 'INITIAL_CONTEXT')
  await expect(surface.locator('.xterm-rows'))
    .toContainText('REPLY:provider-source-control-e2e:INITIAL_CONTEXT', { timeout: 30_000 })
  await expect.poll(async () => (await providerEvents(fixture)).some((event) =>
    event.event === 'stopped' && event.providerId === 'provider-source-control-e2e'
  ), {
    message: '等待源 provider 完成可恢复状态登记',
    timeout: 30_000
  }).toBe(true)
  return { sessionId, providerId: 'provider-source-control-e2e', surface }
}

export async function runMtInSession<T>(
  fixture: AiHostControlFixture,
  sessionId: string,
  args: string[],
  expectedStatus = 0
): Promise<MtInvocation<T>> {
  const surface = fixture.page.locator(`.terminal-surface[data-session-id="${sessionId}"]`)
  await expect(surface).toBeVisible()
  const resultPath = join(fixture.rootDirectory, `mt-result-${randomUUID()}.json`)
  const payload = Buffer.from(JSON.stringify({ args, resultPath }), 'utf8').toString('base64')
  const textarea = surface.locator('.xterm-helper-textarea')
  await textarea.focus()
  await textarea.pressSequentially(`${MT_REQUEST_PREFIX}${payload}`, { delay: 1 })
  await textarea.press('Enter')

  await expect.poll(async () => {
    try {
      JSON.parse(await readFile(resultPath, 'utf8')) as { status: number }
      return true
    } catch {
      return false
    }
  }, {
    message: `等待托管 provider 内的 mt 命令结束：${args.join(' ')}`,
    timeout: 180_000,
    intervals: [100, 250, 500, 1_000]
  }).toBe(true)

  const captured = JSON.parse(await readFile(resultPath, 'utf8')) as {
    status: number
    stdout: string
    stderr: string
  }
  expect(captured.status, [
    `mt status mismatch: ${args.join(' ')}`,
    `stdout: ${captured.stdout}`,
    `stderr: ${captured.stderr}`
  ].join('\n')).toBe(expectedStatus)
  const serialized = captured.status === 0 || captured.status === 6
    ? captured.stdout
    : captured.stderr
  let value: T
  try {
    value = JSON.parse(serialized) as T
  } catch {
    throw new Error(
      `mt returned invalid JSON (status ${captured.status}): ${args.join(' ')}\n` +
      `stdout: ${captured.stdout}\nstderr: ${captured.stderr}`
    )
  }
  return { ...captured, value }
}

export async function runShellMtJson<T>(
  surface: Locator,
  command: string,
  outputPath: string
): Promise<T> {
  const statusPath = `${outputPath}.status`
  const stderrPath = `${outputPath}.stderr`
  const textarea = surface.locator('.xterm-helper-textarea')
  await textarea.focus()
  await textarea.pressSequentially(
    `${command} > ${shellQuote(outputPath)} 2> ${shellQuote(stderrPath)}; ` +
      `printf '%s' $? > ${shellQuote(statusPath)}`,
    { delay: 1 }
  )
  await textarea.press('Enter')

  await expect.poll(async () => {
    try {
      return (await readFile(statusPath, 'utf8')).trim()
    } catch {
      return undefined
    }
  }, {
    message: `等待真实 mt 命令结束：${command}`,
    timeout: 30_000
  }).toBe('0')

  const json = await readFile(outputPath, 'utf8')
  try {
    return JSON.parse(json) as T
  } catch {
    const stderr = await readFile(stderrPath, 'utf8').catch(() => '')
    throw new Error(`mt command returned invalid JSON: ${command}\nstdout: ${json}\nstderr: ${stderr}`)
  }
}

export function sessionCard(page: Page, title: string): Locator {
  return page.getByRole('article', { name: `会话：${title}`, exact: true })
}

export async function showChildLevel(page: Page, count: number): Promise<void> {
  const button = page.getByRole('button', { name: `查看 ${count} 个子会话`, exact: true })
  await expect(button).toBeVisible({ timeout: 30_000 })
  await button.click()
}

export async function providerEvents(
  fixture: AiHostControlFixture
): Promise<Array<Record<string, unknown>>> {
  const content = await readFile(fixture.providerEventLog, 'utf8').catch(() => '')
  return content.trim() === ''
    ? []
    : content.trim().split('\n').map((line) => JSON.parse(line) as Record<string, unknown>)
}

export async function providerInput(
  fixture: AiHostControlFixture,
  providerId: string
): Promise<string> {
  return readFile(join(fixture.providerInputDirectory, `${providerId}.txt`), 'utf8').catch(() => '')
}

export async function git(
  fixture: Pick<AiHostControlFixture, 'repositoryDirectory'>,
  args: string[]
): Promise<string> {
  return (await execFileAsync('git', ['-C', fixture.repositoryDirectory, ...args])).stdout.trim()
}

export async function requiredAttribute(locator: Locator, name: string): Promise<string> {
  const value = await locator.getAttribute(name)
  if (!value) throw new Error(`Expected ${name}`)
  return value
}

export function shellQuote(value: string): string {
  return `'${value.replaceAll("'", `'\\''`)}'`
}

async function initializeRepository(repository: string): Promise<void> {
  await execFileAsync('git', ['init', '-b', 'main', repository])
  await execFileAsync('git', ['-C', repository, 'config', 'user.name', 'Matou E2E'])
  await execFileAsync('git', ['-C', repository, 'config', 'user.email', 'matou-e2e@example.invalid'])
  await writeFile(join(repository, 'baseline.txt'), 'baseline preserved\n')
  await execFileAsync('git', ['-C', repository, 'add', 'baseline.txt'])
  await execFileAsync('git', ['-C', repository, 'commit', '-m', 'fixture baseline'])
  await writeFile(join(repository, 'project-state.txt'), 'project files stay in place\n')
  await execFileAsync('git', ['-C', repository, 'add', 'project-state.txt'])
  await execFileAsync('git', ['-C', repository, 'commit', '-m', 'fixture project state'])
}

function providerScript(): string {
  return `#!/usr/bin/env python3
import fcntl, json, os, pathlib, subprocess, sys, time, urllib.request

args=sys.argv[1:]
settings=''
resume=''
fork='--fork-session' in args
for i, value in enumerate(args):
    if value == '--settings' and i + 1 < len(args): settings=args[i + 1]
    if value == '--resume' and i + 1 < len(args): resume=args[i + 1]

event_log=os.environ['MATOU_AI_CONTROL_EVENT_LOG']
counter_path=event_log + '.counter'
if fork:
    with open(counter_path, 'a+') as counter:
        fcntl.flock(counter, fcntl.LOCK_EX)
        counter.seek(0)
        value=int(counter.read() or '0') + 1
        counter.seek(0); counter.truncate(); counter.write(str(value)); counter.flush()
        provider_id=f'provider-fork-control-e2e-{value}'
elif resume:
    provider_id=resume
else:
    provider_id='provider-source-control-e2e'

def record(event, **details):
    with open(event_log, 'a') as log:
        log.write(json.dumps({'event':event,'providerId':provider_id,**details}) + '\\n')
        log.flush()

record('invoked', args=args, cwd=os.getcwd())
settings_data=json.load(open(settings))
url=settings_data['hooks']['UserPromptSubmit'][0]['hooks'][0]['url']
def post(payload):
    body=json.dumps(payload).encode()
    request=urllib.request.Request(url, data=body, headers={'content-type':'application/json'}, method='POST')
    urllib.request.urlopen(request, timeout=3).read()
def hook(name):
    post({'hook_event_name':name,'session_id':provider_id,'cwd':os.getcwd()})

if fork:
    delay_ms=int(os.environ.get('MATOU_AI_CONTROL_FORK_READY_DELAY_MS', '0'))
    if delay_ms > 0: time.sleep(delay_ms / 1000)
print('READY:' + provider_id, flush=True)
record('ready')
post({'session_id':provider_id,'cwd':os.getcwd()})
input_path=pathlib.Path(os.environ['MATOU_AI_CONTROL_INPUT_DIR']) / (provider_id + '.txt')
for line in sys.stdin:
    value=line.rstrip('\\r\\n')
    if value.startswith('${MT_REQUEST_PREFIX}'):
        request=json.loads(__import__('base64').b64decode(value.split(':', 1)[1]).decode())
        completed=subprocess.run(['mt', *request['args']], capture_output=True, text=True, timeout=180)
        result={'status':completed.returncode,'stdout':completed.stdout,'stderr':completed.stderr}
        target=pathlib.Path(request['resultPath'])
        temporary=target.with_suffix(target.suffix + '.tmp')
        temporary.write_text(json.dumps(result))
        temporary.replace(target)
        record('mt-completed', status=completed.returncode)
        print('MT_COMPLETED:' + str(completed.returncode), flush=True)
        continue
    hook('UserPromptSubmit')
    with input_path.open('a') as output: output.write(value + '\\n')
    record('input', value=value)
    print(f'REPLY:{provider_id}:{value}', flush=True)
    hook('Stop')
    record('stopped')
`
}

function branchCollisionScript(): string {
  return `#!/bin/sh
set -eu
repository="$1"
trigger_branch="$2"
blocker_branch="$3"
collision_branch="$4"
marker="$5"
current_branch="$(git branch --show-current)"
if [ "$current_branch" = "$trigger_branch" ]; then
  git -C "$repository" branch "$collision_branch" HEAD~1
  printf ready > "$marker"
elif [ "$current_branch" = "$blocker_branch" ]; then
  count=0
  while [ ! -f "$marker" ] && [ "$count" -lt 100 ]; do
    sleep 0.05
    count=$((count + 1))
  done
  test -f "$marker"
  sleep 1
fi
`
}
