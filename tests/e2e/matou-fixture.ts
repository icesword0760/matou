import { mkdir, mkdtemp, rm } from 'node:fs/promises'
import { join, resolve } from 'node:path'

import { _electron as electron, type ElectronApplication, type Page } from '@playwright/test'

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
}

export async function launchMatou(options: LaunchMatouOptions = {}): Promise<MatouFixture> {
  const root = await mkdtemp('/tmp/matou-e2e-')
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

export function windowId(page: Page): string {
  return new URL(page.url()).searchParams.get('windowId') ?? ''
}
