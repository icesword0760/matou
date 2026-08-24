import { mkdir, mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'

import { _electron as electron, type ElectronApplication, type Page } from '@playwright/test'

export interface MatouFixture {
  app: ElectronApplication
  page: Page
  dataDirectory: string
  workspaceDirectory: string
  close(): Promise<void>
}

export async function launchMatou(): Promise<MatouFixture> {
  const root = await mkdtemp(join(tmpdir(), 'matou-prd05-e2e-'))
  const dataDirectory = join(root, 'data')
  const workspaceDirectory = join(root, 'matou_workspace')
  await mkdir(workspaceDirectory, { recursive: true })
  const app = await electron.launch({
    args: [resolve(import.meta.dirname, '../../apps/desktop')],
    env: {
      ...process.env,
      MATOU_E2E: '1', MATOU_DATA_DIR: dataDirectory,
      MATOU_DEFAULT_WORKSPACE: workspaceDirectory,
      MATOU_RUNTIME_ENTRY: resolve(import.meta.dirname, '../../apps/runtime/dist/index.cjs')
    }
  })
  const page = await app.firstWindow()
  return {
    app, page, dataDirectory, workspaceDirectory,
    close: async () => {
      await app.close()
      await rm(root, { recursive: true, force: true })
    }
  }
}

export function windowId(page: Page): string {
  return new URL(page.url()).searchParams.get('windowId') ?? ''
}
