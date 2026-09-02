import { execFile } from 'node:child_process'
import { mkdir, mkdtemp, rm } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { promisify } from 'node:util'

import { _electron as electron, type ElectronApplication, type Page } from '@playwright/test'

const execFileAsync = promisify(execFile)

export interface MatouFixture {
  app: ElectronApplication
  page: Page
  dataDirectory: string
  workspaceDirectory: string
  rootDirectory: string
  electronUserDataDirectory: string
  close(): Promise<void>
}

export interface LaunchMatouOptions {
  preserveMainWindowCloseBehavior?: boolean
  env?: Record<string, string>
  root?: string
}

export async function launchMatou(options: LaunchMatouOptions = {}): Promise<MatouFixture> {
  const root = options.root ?? await mkdtemp('/tmp/matou-e2e-')
  return startMatou(root, options)
}

export async function restartMatou(
  fixture: MatouFixture,
  options: LaunchMatouOptions = {}
): Promise<MatouFixture> {
  await fixture.app.evaluate(({ app }) => { app.quit() }).catch(() => {})
  await fixture.app.close().catch(() => {})
  return startMatou(fixture.rootDirectory, options)
}

export async function stopMatouPreservingData(fixture: MatouFixture): Promise<void> {
  await fixture.app.evaluate(({ app }) => { app.quit() }).catch(() => {})
  await fixture.app.close().catch(() => {})
}

export async function restartMatouGracefully(
  fixture: MatouFixture,
  options: LaunchMatouOptions = {}
): Promise<MatouFixture> {
  await Promise.all([
    fixture.app.waitForEvent('close'),
    fixture.app.evaluate(({ app }) => { app.quit() })
  ])
  return startMatou(fixture.rootDirectory, options)
}

async function startMatou(root: string, options: LaunchMatouOptions = {}): Promise<MatouFixture> {
  await assertSecondaryAcceptanceDisplay(options.env)
  const dataDirectory = join(root, 'data')
  const workspaceDirectory = join(root, 'matou_workspace')
  const electronUserDataDirectory = join(root, 'electron-user-data')
  await mkdir(workspaceDirectory, { recursive: true })
  await mkdir(electronUserDataDirectory, { recursive: true })
  const app = await electron.launch({
    args: [
      resolve(import.meta.dirname, '../../apps/desktop'),
      `--user-data-dir=${electronUserDataDirectory}`
    ],
    env: {
      ...process.env,
      ...options.env,
      MATOU_E2E: '1', MATOU_DATA_DIR: dataDirectory,
      MATOU_DEFAULT_WORKSPACE: workspaceDirectory,
      ELECTRON_USER_DATA_DIR: electronUserDataDirectory,
      ...(options.preserveMainWindowCloseBehavior ? { MATOU_E2E_WINDOW_CLOSE: 'hide' } : {}),
      MATOU_RUNTIME_ENTRY: resolve(import.meta.dirname, '../../apps/runtime/dist/index.cjs')
    }
  })
  const page = await app.firstWindow()
  return {
    app, page, dataDirectory, workspaceDirectory, rootDirectory: root, electronUserDataDirectory,
    close: async () => {
      await app.evaluate(({ app: electronApp }) => { electronApp.quit() }).catch(() => {})
      await app.close().catch(() => {})
      await rm(root, { recursive: true, force: true })
    }
  }
}

async function assertSecondaryAcceptanceDisplay(env: Record<string, string> | undefined): Promise<void> {
  if (process.platform !== 'darwin' || env?.MATOU_E2E_DISPLAY === 'primary') return
  const { stdout } = await execFileAsync(
    '/usr/sbin/system_profiler',
    ['SPDisplaysDataType', '-json'],
    { maxBuffer: 4 * 1024 * 1024 }
  )
  const report = JSON.parse(stdout) as {
    SPDisplaysDataType?: Array<{
      spdisplays_ndrvs?: Array<Record<string, unknown>>
    }>
  }
  const displays = report.SPDisplaysDataType?.flatMap(
    ({ spdisplays_ndrvs }) => spdisplays_ndrvs ?? []
  ) ?? []
  const secondaryBuiltIn = displays.find((display) =>
    display.spdisplays_online === 'spdisplays_yes' &&
    display.spdisplays_main !== 'spdisplays_yes' &&
    typeof display.spdisplays_display_type === 'string' &&
    display.spdisplays_display_type.includes('built-in')
  )
  if (!secondaryBuiltIn) {
    throw new Error(
      'Visible Electron acceptance requires the connected built-in secondary display; no app window was opened.'
    )
  }
}

export function windowId(page: Page): string {
  return new URL(page.url()).searchParams.get('windowId') ?? ''
}
