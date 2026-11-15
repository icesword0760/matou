import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { HierarchyApplicationService } from '../../apps/runtime/src/hierarchy/hierarchy-application-service'
import { GeometryRepository, type GeometryUpdate } from '../../apps/runtime/src/scenes/geometry-repository'
import { SessionCanvasService } from '../../apps/runtime/src/session-canvas/session-canvas-service'
import { RuntimeDatabase } from '../../apps/runtime/src/storage/database'
import { DomainTransactionManager } from '../../apps/runtime/src/storage/domain-transaction'
import { MigrationRunner } from '../../apps/runtime/src/storage/migration-runner'
import { FOUNDATION_MIGRATIONS } from '../../apps/runtime/src/storage/migrations'

type Operation =
  | { op: 'put'; update: GeometryUpdate }
  | { op: 'get'; sceneId: string; ownerKey: string }
  | { op: 'list' | 'discard'; sceneId: string }
  | { op: 'listAll' | 'reopen' }

const output = process.argv[2]
if (!output) throw new Error('Specify the geometry fixture output path')
const root = mkdtempSync(join(tmpdir(), 'matou-geometry-contract-'))
const filename = join(root, 'matou.sqlite')
const backup = join(root, 'seed.sqlite')
let database = RuntimeDatabase.open(filename)
try {
  await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
  const transactions = new DomainTransactionManager(database)
  const hierarchy = new HierarchyApplicationService(database, transactions)
  const canvas = new SessionCanvasService(database, transactions)
  const first = hierarchy.bootstrapWindow(
    { commandId: 'bootstrap', commandType: 'window.bootstrap', requestHash: 'bootstrap' },
    { windowId: 'geometry-window', defaultRootDirectory: '/tmp/geometry-workspace', defaultName: 'Geometry', now: 1000 }
  )
  const second = canvas.createCanvas(
    { commandId: 'second', commandType: 'hierarchy.create-canvas', requestHash: 'second' },
    { windowId: 'geometry-window', taskId: first.task!.id, now: 1001 }
  )
  const sceneId = first.scene!.id
  const otherSceneId = second.created.scene.id
  const nodeId = first.mount!.sceneNodeId!
  const mountId = first.mount!.id
  const ownSession = first.session!.id
  const otherSession = second.created.session.id
  const q = (value: string): string => `'${value.replaceAll("'", "''")}'`
  const sceneWindow = 'geometry-scene-window'
  database.run(
    `INSERT INTO scene_windows (id, scene_id, native_window_key, state, created_at, updated_at)
     VALUES (?, ?, 'geometry-window', 'detached', 1002, 1002)`, sceneWindow, sceneId
  )
  await database.backupTo(backup)
  const seedBase64 = readFileSync(backup).toString('base64')
  let repository = new GeometryRepository(database)
  const steps: Array<Operation & { label: string; sql: string[]; result?: unknown; error?: string; rows: unknown[] }> = []

  function record(label: string, operation: Operation, sql: string[] = []): void {
    for (const statement of sql) database.exec(statement)
    let result: unknown
    let error: string | undefined
    try {
      switch (operation.op) {
        case 'put': result = repository.put(operation.update); break
        case 'get': result = repository.get(operation.sceneId, operation.ownerKey) ?? null; break
        case 'list': result = repository.list(operation.sceneId); break
        case 'listAll': result = repository.listAll(); break
        case 'discard': result = repository.discardInvalid(operation.sceneId); break
        case 'reopen':
          database.close()
          database = RuntimeDatabase.open(filename)
          repository = new GeometryRepository(database)
          result = repository.listAll()
          break
      }
    } catch (cause) {
      if (!(cause instanceof Error)) throw cause
      error = cause.message
    }
    const rows = database.all('SELECT * FROM scene_geometry ORDER BY scene_id, owner_key')
    steps.push({ ...operation, label, sql, result, error, rows })
  }

  const viewport = `dag-viewport:${sceneId}`
  const put = (label: string, sceneId: string, ownerKey: string, layoutRevision: number,
    geometry: unknown, now: number): void => {
    record(label, { op: 'put', update: { sceneId, ownerKey, layoutRevision, geometry, now } })
  }
  record('initially empty', { op: 'listAll' })
  put('save viewport', sceneId, viewport, 4, { '20': 'later', '2': 'first', panX: 13, zoom: 1.25 }, 1100)
  put('same revision and geometry keeps old time', sceneId, viewport, 4,
    { '2': 'first', '20': 'later', panX: 13, zoom: 1.25 }, 1101)
  put('same revision but different property order', sceneId, viewport, 4,
    { '2': 'first', '20': 'later', zoom: 1.25, panX: 13 }, 1102)
  put('same revision and new geometry', sceneId, viewport, 4, { panX: 99, zoom: 1.25 }, 1103)
  put('stale revision', sceneId, viewport, 3, { panX: 5 }, 1104)
  put('fractional revision', sceneId, viewport, 4.5, { panX: 5 }, 1105)
  put('negative revision', sceneId, viewport, -1, { panX: 5 }, 1106)
  put('unsafe revision', sceneId, viewport, Number.MAX_SAFE_INTEGER + 1, { panX: 5 }, 1107)
  put('maximum safe revision', sceneId, 'scene', Number.MAX_SAFE_INTEGER, { value: [1e-7, 1e21, -0] }, 1108)
  put('different scene owner', otherSceneId, 'scene', 0, null, 1109)
  put('root group', sceneId, `session-group:${sceneId}:root`, 1, { scrollLeft: 105 }, 1110)
  put('member group', sceneId, `session-group:${sceneId}:${ownSession}`, 1, { scrollLeft: 9 }, 1111)
  put('member node layout', sceneId, `dag-node-layout:${sceneId}:${ownSession}`, 1, [0, 20], 1112)
  put('node owner', sceneId, `node:${nodeId}`, 1, { x: 4 }, 1113)
  put('mount owner', sceneId, `mount:${mountId}`, 1, { x: 5 }, 1114)
  put('window owner', sceneId, `window:${sceneWindow}`, 1, { width: 540 }, 1115)
  put('cross scene viewport', otherSceneId, viewport, 4, {}, 1116)
  put('cross scene member', sceneId, `dag-node-layout:${sceneId}:${otherSession}`, 1, {}, 1117)
  put('cross scene node', otherSceneId, `node:${nodeId}`, 1, {}, 1118)
  put('missing mount', sceneId, 'mount:missing', 1, {}, 1119)
  put('missing scene', 'missing', 'scene', 1, {}, 1120)
  put('malformed canvas owner', sceneId, `session-group:${sceneId}:root:extra`, 1, {}, 1121)
  record('get latest viewport', { op: 'get', sceneId, ownerKey: viewport })
  record('get absent owner', { op: 'get', sceneId, ownerKey: 'mount:missing' })
  record('get malformed JSON', { op: 'get', sceneId, ownerKey: `node:${nodeId}` }, [
    `UPDATE scene_geometry SET geometry_json = '{bad' WHERE scene_id = ${q(sceneId)} AND owner_key = ${q(`node:${nodeId}`)}`
  ])
  record('list skips bad JSON in one scene', { op: 'list', sceneId })
  record('list all scene and owner ordering', { op: 'listAll' })
  record('discard deleted window but keep bad JSON for a valid owner', { op: 'discard', sceneId }, [
    `DELETE FROM scene_windows WHERE id = ${q(sceneWindow)}`
  ])
  record('nothing left to discard', { op: 'discard', sceneId })
  record('reopen disk database', { op: 'reopen' })
  record('get viewport after reopen', { op: 'get', sceneId, ownerKey: viewport })
  writeFileSync(output, JSON.stringify({ versions: process.versions, seedBase64, steps }, null, 2) + '\n')
  console.log(JSON.stringify({ steps: steps.length, electron: process.versions.electron, output }))
} finally {
  database.close()
  rmSync(root, { recursive: true, force: true })
}
