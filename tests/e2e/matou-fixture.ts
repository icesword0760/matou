import { mkdir, mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

import { _electron as electron, type ElectronApplication, type Page } from '@playwright/test'

export interface MatouFixture {
  app: ElectronApplication
  page: Page
  dataDirectory: string
  workspaceDirectory: string
  rootDirectory: string
  close(): Promise<void>
}

export interface LaunchMatouOptions {
  preserveMainWindowCloseBehavior?: boolean
  env?: Record<string, string>
}

export async function launchMatou(options: LaunchMatouOptions = {}): Promise<MatouFixture> {
  const root = await mkdtemp(join(tmpdir(), 'matou-prd05-e2e-'))
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

async function startMatou(root: string, options: LaunchMatouOptions = {}): Promise<MatouFixture> {
  const dataDirectory = join(root, 'data')
  const workspaceDirectory = join(root, 'matou_workspace')
  await mkdir(workspaceDirectory, { recursive: true })
  const app = await electron.launch({
    args: [resolve(import.meta.dirname, '../../apps/desktop')],
    env: {
      ...process.env,
      ...options.env,
      MATOU_E2E: '1', MATOU_DATA_DIR: dataDirectory,
      MATOU_DEFAULT_WORKSPACE: workspaceDirectory,
      ...(options.preserveMainWindowCloseBehavior ? { MATOU_E2E_WINDOW_CLOSE: 'hide' } : {}),
      MATOU_RUNTIME_ENTRY: resolve(import.meta.dirname, '../../apps/runtime/dist/index.cjs')
    }
  })
  const page = await app.firstWindow()
  return {
    app, page, dataDirectory, workspaceDirectory, rootDirectory: root,
    close: async () => {
      await app.close()
      await rm(root, { recursive: true, force: true })
    }
  }
}

export function windowId(page: Page): string {
  return new URL(page.url()).searchParams.get('windowId') ?? ''
}
