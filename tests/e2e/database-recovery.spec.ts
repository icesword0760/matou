import { DatabaseSync } from 'node:sqlite'
import { readFile, readdir, stat, writeFile } from 'node:fs/promises'
import { join } from 'node:path'

import { expect, test, type Page } from '@playwright/test'

import { SessionRelationRepository } from '../../apps/runtime/src/relations/session-relation-repository'
import { RuntimeDatabase } from '../../apps/runtime/src/storage/database'
import { DomainTransactionManager } from '../../apps/runtime/src/storage/domain-transaction'

import {
  launchMatou,
  restartMatou,
  restartMatouGracefully,
  type MatouFixture
} from './matou-fixture'

test.describe('real database recovery in Electron', () => {
  test.setTimeout(120_000)

  test('shows only recovery UI after SQLite header corruption and restores the latest backup', async () => {
    let fixture: MatouFixture | undefined = await launchMatou()
    try {
      await expect(fixture.page.getByRole('main')).toBeVisible()
      await expect(fixture.page.locator('.workspace-group').first()).toBeVisible()
      const workspaceName = await fixture.page.locator('.workspace-group__name').first().textContent()

      await quitMatou(fixture)
      const databasePath = join(fixture.dataDirectory, 'matou.sqlite')
      await corruptHeader(databasePath)

      fixture = await restartMatou(fixture)
      await expectRecoveryOnly(fixture.page)
      await expect(fixture.page.getByRole('radio').first()).toBeChecked()
      expect(await fixture.page.getByRole('radio').count()).toBeLessThanOrEqual(7)

      const recoveryBeforeReplay = await fixture.page.evaluate(() => (
        window.matouDesktop.getRuntimeLifecycle()
      ))
      const markerPath = `${databasePath}.recovery.json`
      const markerBeforeReplay = await readFile(markerPath)
      const quarantinedBeforeReplay = await readFile(
        recoveryBeforeReplay.recovery!.quarantinedPath
      )
      const staleReplayError = await fixture.page.evaluate(async (expectedRecoveryId) => {
        try {
          await window.matouDesktop.retryDatabaseOpen(expectedRecoveryId)
          return ''
        } catch (error) {
          return error instanceof Error ? error.message : String(error)
        }
      }, `stale-${recoveryBeforeReplay.recovery!.recoveryId}`)
      expect(staleReplayError).toContain('恢复周期已更新')
      expect(await readFile(markerPath)).toEqual(markerBeforeReplay)
      expect(await readFile(recoveryBeforeReplay.recovery!.quarantinedPath))
        .toEqual(quarantinedBeforeReplay)
      expect(await fixture.page.evaluate(() => window.matouDesktop.getRuntimeLifecycle()))
        .toEqual(recoveryBeforeReplay)

      await fixture.page.getByRole('button', { name: '恢复所选备份' }).click()
      await expect(fixture.page.locator('.workspace-group').first()).toBeVisible()
      await expect(fixture.page.locator('.workspace-group__name').first()).toHaveText(workspaceName ?? '')
    } finally {
      await fixture?.close()
    }
  })

  test('detects middle-page corruption and restores Workspace, Task, Session, and relation state', async () => {
    let fixture: MatouFixture | undefined = await launchMatou()
    try {
      await createTask(fixture.page)
      await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
      await expect(fixture.page.locator('[data-testid="terminal-pane"]:visible')).toHaveCount(2)
      await quitMatou(fixture)
      const databasePath = join(fixture.dataDirectory, 'matou.sqlite')
      addOwnedStructuralRelation(databasePath)
      fixture = await restartMatou(fixture)
      await fixture.page.getByText('新事项', { exact: true }).click()
      await fixture.page.getByRole('button', { name: '打开会话 DAG' }).click()
      await expect.poll(async () => (await fixture!.app.windows()).length).toBe(2)
      const dag = (await fixture.app.windows()).find((candidate) => candidate !== fixture!.page)!
      await expect(dag.locator('.dag-node-card')).toHaveCount(2)
      await expect(dag.locator('.dag-edge')).toHaveCount(1)
      await dag.close()
      await quitMatou(fixture)
      await corruptOwnedMiddlePage(databasePath)

      fixture = await restartMatou(fixture)
      await expectRecoveryOnly(fixture.page)
      await fixture.page.getByRole('button', { name: '恢复所选备份' }).click()

      await expect(fixture.page.getByText('新事项', { exact: true })).toBeVisible()
      await fixture.page.getByText('新事项', { exact: true }).click()
      await expect(fixture.page.locator('[data-testid="terminal-pane"]:visible')).toHaveCount(1)
      await fixture.page.getByRole('button', { name: '打开会话 DAG' }).click()
      await expect.poll(async () => (await fixture!.app.windows()).length).toBe(2)
      const restoredDag = (await fixture.app.windows()).find((candidate) => candidate !== fixture!.page)!
      await expect(restoredDag.locator('.dag-node-card')).toHaveCount(2)
      await expect(restoredDag.locator('.dag-edge')).toHaveCount(1)
    } finally {
      await fixture?.close()
    }
  })

  test('keeps seven clean backups, skips an invalid newest backup, and falls back to the next valid generation', async () => {
    let fixture: MatouFixture | undefined = await launchMatou()
    try {
      for (let cycle = 0; cycle < 8; cycle += 1) {
        fixture = await restartMatouGracefully(fixture)
        await expect(fixture.page.locator('.workspace-group').first()).toBeVisible()
      }
      await createTask(fixture.page)
      await quitMatou(fixture)

      const manifests = await readBackupManifests(fixture.dataDirectory)
      expect(manifests).toHaveLength(7)
      const newest = manifests[0]!
      const fallback = manifests[1]!
      const newestBytes = await readFile(newest.path)
      newestBytes.fill(0x5a, 0, Math.min(64, newestBytes.byteLength))
      await writeFile(newest.path, newestBytes)

      const databasePath = join(fixture.dataDirectory, 'matou.sqlite')
      await corruptHeader(databasePath)
      fixture = await restartMatou(fixture)

      await expectRecoveryOnly(fixture.page)
      await expect(fixture.page.getByRole('radio')).toHaveCount(6)
      await expect(fixture.page.getByRole('radio').first()).toHaveValue(fallback.id)
      await fixture.page.getByRole('button', { name: '恢复所选备份' }).click()
      await expect(fixture.page.locator('.workspace-group').first()).toBeVisible()
      await expect(fixture.page.getByText('新事项', { exact: true })).toHaveCount(0)
    } finally {
      await fixture?.close()
    }
  })

  test('creates an empty database only after explicit confirmation and preserves recovery evidence', async () => {
    let fixture: MatouFixture | undefined = await launchMatou()
    try {
      await createTask(fixture.page)
      await quitMatou(fixture)
      const databasePath = join(fixture.dataDirectory, 'matou.sqlite')
      await corruptHeader(databasePath)

      fixture = await restartMatou(fixture)
      await expectRecoveryOnly(fixture.page)
      const lifecycle = await fixture.page.evaluate(() => window.matouDesktop.getRuntimeLifecycle())
      const quarantinedPath = lifecycle.recovery!.quarantinedPath
      await fixture.page.getByRole('button', { name: '创建全新空数据库' }).click()
      const dialog = fixture.page.getByRole('dialog', { name: '确认创建全新空数据库' })
      await expect(dialog).toBeVisible()
      await expect(fixture.page.getByRole('heading', { name: '数据库需要恢复' })).toBeVisible()
      await fixture.page.getByRole('button', { name: '返回' }).click()
      await expect(dialog).toHaveCount(0)
      await expect(fixture.page.getByRole('heading', { name: '数据库需要恢复' })).toBeVisible()

      await fixture.page.getByRole('button', { name: '创建全新空数据库' }).click()
      await fixture.page.getByRole('button', { name: '确认创建空数据库' }).click()
      await expect(fixture.page.locator('.workspace-group').first()).toBeVisible()
      await expect(fixture.page.getByText('新事项', { exact: true })).toHaveCount(0)
      expect((await stat(quarantinedPath)).isFile()).toBe(true)
      const evidenceEntries = await readdir(join(fixture.dataDirectory, 'recovery-evidence'))
      expect(evidenceEntries.length).toBeGreaterThan(0)
    } finally {
      await fixture?.close()
    }
  })
})

async function createTask(page: Page): Promise<void> {
  await expect(page.locator('.terminal-surface').first()).toHaveAttribute('data-pid', /\d+/)
  await page.getByRole('button', { name: /^在 .* 中新增事项$/ }).click()
  await expect(page.getByTestId('active-task')).toHaveText('新事项')
}

async function expectRecoveryOnly(page: Page): Promise<void> {
  await expect(page.getByRole('heading', { name: '数据库需要恢复' })).toBeVisible()
  await expect(page.locator('.hierarchy-shell')).toHaveCount(0)
}

async function quitMatou(fixture: MatouFixture): Promise<void> {
  await Promise.all([
    fixture.app.waitForEvent('close'),
    fixture.app.evaluate(({ app }) => { app.quit() })
  ])
}

async function corruptHeader(databasePath: string): Promise<void> {
  const bytes = await readFile(databasePath)
  bytes.fill(0x5a, 0, Math.min(16, bytes.byteLength))
  await writeFile(databasePath, bytes)
}

async function corruptOwnedMiddlePage(databasePath: string): Promise<void> {
  const database = new DatabaseSync(databasePath)
  let rootPage: number
  let pageSize: number
  try {
    database.exec(`
      CREATE TABLE e2e_middle_page_sentinel (
        id INTEGER PRIMARY KEY,
        payload BLOB NOT NULL
      );
      WITH RECURSIVE rows(value) AS (
        SELECT 1 UNION ALL SELECT value + 1 FROM rows WHERE value < 256
      )
      INSERT INTO e2e_middle_page_sentinel(payload)
      SELECT randomblob(2048) FROM rows;
      PRAGMA wal_checkpoint(TRUNCATE);
    `)
    rootPage = Number((database.prepare(
      "SELECT rootpage FROM sqlite_schema WHERE name = 'e2e_middle_page_sentinel'"
    ).get() as { rootpage: number }).rootpage)
    pageSize = Number((database.prepare('PRAGMA page_size').get() as { page_size: number }).page_size)
  } finally {
    database.close()
  }
  const bytes = await readFile(databasePath)
  const start = (rootPage - 1) * pageSize
  expect(start, 'the owned table root must be outside the SQLite header page').toBeGreaterThan(0)
  expect(start + pageSize).toBeLessThanOrEqual(bytes.byteLength)
  bytes.fill(0x5a, start, start + pageSize)
  await writeFile(databasePath, bytes)
}

function addOwnedStructuralRelation(databasePath: string): void {
  const database = RuntimeDatabase.open(databasePath)
  try {
    const sessions = database.all<{ id: string; task_id: string }>(`
      SELECT sessions.id, sessions.task_id
      FROM sessions
      JOIN session_canvas_memberships memberships ON memberships.session_id = sessions.id
      JOIN tasks ON tasks.id = sessions.task_id
      WHERE tasks.title = '新事项' AND sessions.archived_at IS NULL
      ORDER BY memberships.sibling_created_seq, sessions.created_at
    `)
    expect(sessions).toHaveLength(2)
    new SessionRelationRepository(
      database,
      new DomainTransactionManager(database)
    ).create({
      commandId: 'e2e-middle-page-relation',
      commandType: 'relation.create',
      requestHash: 'e2e-middle-page-relation'
    }, {
      id: 'e2e-middle-page-relation',
      taskId: sessions[0]!.task_id,
      fromSessionId: sessions[1]!.id,
      toSessionId: sessions[0]!.id,
      kind: 'derived-from',
      metadata: { source: 'database-recovery-e2e' },
      now: Date.now()
    })
  } finally {
    database.close()
  }
}

interface BackupManifest {
  id: string
  path: string
  createdAt: number
}

async function readBackupManifests(dataDirectory: string): Promise<BackupManifest[]> {
  const directory = join(dataDirectory, 'backups')
  const names = (await readdir(directory)).filter((name) => name.endsWith('.json'))
  const manifests = await Promise.all(names.map(async (name) =>
    JSON.parse(await readFile(join(directory, name), 'utf8')) as BackupManifest
  ))
  return manifests.sort((left, right) => right.createdAt - left.createdAt)
}
