import { execFile } from 'node:child_process'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { promisify } from 'node:util'

import { expect, type Locator, type Page } from '@playwright/test'

import { launchMatou, restartMatou, type MatouFixture } from '../matou-fixture'

const exec = promisify(execFile)

export interface SessionCanvasFixture extends MatouFixture {
  nonGitDirectory: string
}

export async function launchSessionCanvas(): Promise<SessionCanvasFixture> {
  let fixture = await launchMatou()
  await exec('git', ['init', '-b', 'main'], { cwd: fixture.workspaceDirectory })
  await exec('git', ['config', 'user.name', 'Matou E2E'], { cwd: fixture.workspaceDirectory })
  await exec('git', ['config', 'user.email', 'matou-e2e@example.invalid'], { cwd: fixture.workspaceDirectory })
  await writeFile(join(fixture.workspaceDirectory, 'baseline.txt'), 'baseline\n')
  await exec('git', ['add', 'baseline.txt'], { cwd: fixture.workspaceDirectory })
  await exec('git', ['commit', '-m', 'e2e baseline'], { cwd: fixture.workspaceDirectory })
  await writeFile(join(fixture.workspaceDirectory, 'uncommitted.txt'), 'uncommitted\n')
  const nonGitDirectory = join(fixture.rootDirectory, 'workspace-non-git')
  await mkdir(nonGitDirectory)
  await writeFile(join(nonGitDirectory, 'readme.txt'), 'non-git\n')
  fixture = await restartMatou(fixture)
  return { ...fixture, nonGitDirectory }
}

export function visibleSurfaces(page: Page): Locator {
  return page.locator('.scene-stage:not([hidden]) [data-testid="terminal-pane"]:visible .terminal-surface')
}

export function activeSurface(page: Page): Locator {
  return page.locator('.scene-stage:not([hidden]) [data-testid="terminal-pane"][data-active="true"] .terminal-surface')
}

export async function terminalCommand(surface: Locator, command: string): Promise<void> {
  const sessionId = await surface.getAttribute('data-session-id')
  if (!sessionId) throw new Error('Terminal Session identity is missing')
  const stableSurface = surface.page().locator(`.terminal-surface[data-session-id="${sessionId}"]`)
  await waitForShell(stableSurface)
  const pane = stableSurface.locator('xpath=ancestor::*[@data-testid="terminal-pane"][1]')
  const textarea = stableSurface.locator('.xterm-helper-textarea')
  if (await pane.getAttribute('data-active') !== 'true') {
    await stableSurface.click({ position: { x: 12, y: 12 } })
  }
  await textarea.focus()
  await expect(pane).toHaveAttribute('data-active', 'true')
  await expect(textarea).toBeFocused()
  await surface.page().waitForTimeout(50)
  await textarea.focus()
  await textarea.pressSequentially(command, { delay: 2 })
  await textarea.press('Enter')
}

export async function waitForShell(surface: Locator): Promise<void> {
  await expect(surface).toHaveAttribute('data-pid', /[1-9][0-9]*/)
}

export async function readText(path: string): Promise<string> {
  return readFile(path, 'utf8')
}
