import { randomUUID } from 'node:crypto'
import { DatabaseSync } from 'node:sqlite'
import { join } from 'node:path'

import { expect, test, type ElectronApplication, type Page } from '@playwright/test'

import {
  launchMatou, stopMatouPreservingData, type MatouFixture
} from './matou-fixture'
import {
  launchSessionCanvas, visibleSurfaces
} from './fixtures/session-canvas-fixture'

test.describe('session node removal scopes', () => {
  test.setTimeout(120_000)

  test('reconnects descendants or removes the complete branch and keeps both results after restart', async () => {
    let fixture: MatouFixture = await launchSessionCanvas()
    try {
      for (let index = 0; index < 4; index += 1) {
        await fixture.page.getByRole('button', { name: '横向新增 Shell' }).click()
        await expect(visibleSurfaces(fixture.page)).toHaveCount(index + 2)
      }
      await expect(visibleSurfaces(fixture.page)).toHaveCount(5)
      for (const surface of await visibleSurfaces(fixture.page).all()) {
        await expect(surface).toHaveAttribute('data-pid', /[1-9][0-9]*/)
      }

      const root = fixture.rootDirectory
      const dataDirectory = fixture.dataDirectory
      await stopMatouPreservingData(fixture)
      const graph = seedTwoRemovalBranches(dataDirectory)
      fixture = await launchMatou({ root })

      await expectVisibleWindowsOnColorLcd(fixture.app)
      const firstChild = fixture.page.locator(
        `.terminal-surface[data-session-id="${graph.firstChild}"]`
      )
      await expect(firstChild).toBeVisible()
      await expect(firstChild).toHaveAttribute('data-pid', /[1-9][0-9]*/)
      await openRemovalDialog(fixture.page, graph.firstChild)
      const firstDialog = fixture.page.getByRole('alertdialog', {
        name: '移除节点“Shell”？'
      })
      await expect(firstDialog.getByRole('radio', { name: /仅移除当前节点/ })).toBeChecked()
      await expect(firstDialog).toContainText('影响 1 个会话、0 个自有 Worktree')
      await expect(firstDialog).toContainText('影响 2 个会话、0 个自有 Worktree')
      await expect(firstDialog).toContainText('后代会话将重连到当前节点的父级')
      await firstDialog.getByRole('button', { name: '移除当前节点' }).click()

      await expect.poll(() => membershipIds(dataDirectory)).not.toContain(graph.firstChild)
      await expect.poll(() => structuralParent(dataDirectory, graph.firstGrandchild))
        .toBe(graph.root)

      fixture = await restartWithRoot(fixture)
      await expectVisibleWindowsOnColorLcd(fixture.app)
      let dag = await openDag(fixture)
      await expect(dag.locator(`.dag-node-card[data-session-id="${graph.firstChild}"]`))
        .toHaveCount(0)
      await expect(dag.locator(`.dag-node-card[data-session-id="${graph.firstGrandchild}"]`))
        .toHaveCount(1)
      await dag.locator(`.dag-node-card[data-session-id="${graph.secondChild}"]`).click()
      await expect.poll(async () => (await fixture.app.windows()).length).toBe(1)

      await openRemovalDialog(fixture.page, graph.secondChild)
      const secondDialog = fixture.page.getByRole('alertdialog', {
        name: '移除节点“Shell”？'
      })
      await secondDialog.getByRole('radio', { name: /移除当前节点及全部后代/ }).click()
      await secondDialog.getByRole('button', { name: '移除 2 个会话' }).click()
      await expect.poll(() => membershipIds(dataDirectory)).not.toContain(graph.secondChild)
      await expect.poll(() => membershipIds(dataDirectory)).not.toContain(graph.secondGrandchild)

      fixture = await restartWithRoot(fixture)
      await expectVisibleWindowsOnColorLcd(fixture.app)
      dag = await openDag(fixture)
      await expect(dag.locator(`.dag-node-card[data-session-id="${graph.firstChild}"]`))
        .toHaveCount(0)
      await expect(dag.locator(`.dag-node-card[data-session-id="${graph.firstGrandchild}"]`))
        .toHaveCount(1)
      await expect(dag.locator(`.dag-node-card[data-session-id="${graph.secondChild}"]`))
        .toHaveCount(0)
      await expect(dag.locator(`.dag-node-card[data-session-id="${graph.secondGrandchild}"]`))
        .toHaveCount(0)
    } finally {
      await fixture.close()
    }
  })
})

async function restartWithRoot(fixture: MatouFixture): Promise<MatouFixture> {
  const root = fixture.rootDirectory
  await stopMatouPreservingData(fixture)
  return launchMatou({ root })
}

async function openRemovalDialog(page: Page, sessionId: string): Promise<void> {
  const pane = page.locator(
    `[data-testid="terminal-pane"]:visible:has(.terminal-surface[data-session-id="${sessionId}"])`
  )
  await pane.locator('.terminal-pane-header').click({ button: 'right' })
  await page.getByRole('menuitem', { name: '移除节点…' }).click()
}

async function openDag(fixture: MatouFixture): Promise<Page> {
  await fixture.page.getByRole('button', { name: '打开会话 DAG' }).click()
  await expect.poll(async () => (await fixture.app.windows()).length).toBe(2)
  await expectVisibleWindowsOnColorLcd(fixture.app)
  return (await fixture.app.windows()).find((page) => page !== fixture.page)!
}

async function expectVisibleWindowsOnColorLcd(app: ElectronApplication): Promise<void> {
  await expect.poll(() => app.evaluate(({ BrowserWindow, screen }) => {
    const primaryId = screen.getPrimaryDisplay().id
    const target = screen.getAllDisplays().filter(({ id }) => id !== primaryId)
      .find(({ internal, label }) => internal || /color\s*lcd/i.test(label))
    const visible = BrowserWindow.getAllWindows().filter((window) => window.isVisible())
    return Boolean(target) && visible.length > 0 && visible.every((window) =>
      screen.getDisplayMatching(window.getBounds()).id === target!.id
    )
  }), { message: 'all visible Matou windows must be on the secondary Color LCD' }).toBe(true)
}

function seedTwoRemovalBranches(dataDirectory: string): {
  root: string
  firstChild: string
  firstGrandchild: string
  secondChild: string
  secondGrandchild: string
} {
  const database = new DatabaseSync(join(dataDirectory, 'matou.sqlite'))
  try {
    const sessions = database.prepare(
      `SELECT id, task_id FROM sessions
       WHERE archived_at IS NULL ORDER BY created_at, id`
    ).all() as Array<{ id: string; task_id: string }>
    if (sessions.length !== 5) throw new Error(`Expected 5 live Sessions, received ${sessions.length}`)
    const [root, firstChild, firstGrandchild, secondChild, secondGrandchild] = sessions
    database.exec('BEGIN IMMEDIATE')
    try {
      insertRelation(database, firstChild!, root!)
      insertRelation(database, firstGrandchild!, firstChild!)
      insertRelation(database, secondChild!, root!)
      insertRelation(database, secondGrandchild!, secondChild!)
      database.prepare('UPDATE window_scene_focus SET active_session_id = ?')
        .run(firstChild!.id)
      database.exec('COMMIT')
    } catch (error) {
      database.exec('ROLLBACK')
      throw error
    }
    return {
      root: root!.id,
      firstChild: firstChild!.id,
      firstGrandchild: firstGrandchild!.id,
      secondChild: secondChild!.id,
      secondGrandchild: secondGrandchild!.id
    }
  } finally {
    database.close()
  }
}

function insertRelation(
  database: DatabaseSync,
  child: { id: string; task_id: string },
  parent: { id: string }
): void {
  const relationId = randomUUID()
  const now = Date.now()
  const event = database.prepare(
    `INSERT INTO session_relation_events (
       event_id, relation_id, operation, task_id, from_session_id, to_session_id,
       relation_kind, metadata_json, command_id, occurred_at
     ) VALUES (?, ?, 'created', ?, ?, ?, 'derived-from', '{}', 'e2e-seed-removal-scope', ?)`
  ).run(`${relationId}:created`, relationId, child.task_id, child.id, parent.id, now)
  database.prepare(
    `INSERT INTO session_relations_current (
       relation_id, task_id, from_session_id, to_session_id, relation_kind,
       metadata_json, created_at, updated_at, source_event_sequence
     ) VALUES (?, ?, ?, ?, 'derived-from', '{}', ?, ?, ?)`
  ).run(relationId, child.task_id, child.id, parent.id, now, now, Number(event.lastInsertRowid))
}

function membershipIds(dataDirectory: string): string[] {
  const database = new DatabaseSync(join(dataDirectory, 'matou.sqlite'), { readOnly: true })
  try {
    return (database.prepare(
      'SELECT session_id FROM session_canvas_memberships ORDER BY session_id'
    ).all() as Array<{ session_id: string }>).map(({ session_id }) => session_id)
  } finally {
    database.close()
  }
}

function structuralParent(dataDirectory: string, sessionId: string): string | undefined {
  const database = new DatabaseSync(join(dataDirectory, 'matou.sqlite'), { readOnly: true })
  try {
    return (database.prepare(
      `SELECT to_session_id FROM session_relations_current
       WHERE from_session_id = ? AND relation_kind IN ('derived-from', 'forked-from')`
    ).get(sessionId) as { to_session_id: string } | undefined)?.to_session_id
  } finally {
    database.close()
  }
}
