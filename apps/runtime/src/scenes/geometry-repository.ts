import type { RuntimeDatabase } from '../storage/database'

export interface GeometryUpdate {
  sceneId: string
  ownerKey: string
  layoutRevision: number
  geometry: unknown
  now: number
}

export interface StoredGeometry extends GeometryUpdate {}

interface GeometryRow {
  scene_id: string
  owner_key: string
  layout_revision: number
  geometry_json: string
  updated_at: number
}

export class GeometryRepository {
  readonly #database: RuntimeDatabase

  constructor(database: RuntimeDatabase) {
    this.#database = database
  }

  put(update: GeometryUpdate): StoredGeometry {
    if (!Number.isSafeInteger(update.layoutRevision) || update.layoutRevision < 0) {
      throw new Error('layout revision must be a non-negative safe integer')
    }
    this.#assertOwnerExists(update.sceneId, update.ownerKey)
    const current = this.get(update.sceneId, update.ownerKey)
    if (current && update.layoutRevision < current.layoutRevision) {
      throw new Error(
        `stale layout revision ${update.layoutRevision}; current revision is ${current.layoutRevision}`
      )
    }
    if (current && update.layoutRevision === current.layoutRevision) {
      if (JSON.stringify(current.geometry) === JSON.stringify(update.geometry)) return current
    }
    this.#database.run(
      `INSERT INTO scene_geometry (
         scene_id, owner_key, layout_revision, geometry_json, updated_at
       ) VALUES (?, ?, ?, ?, ?)
       ON CONFLICT(scene_id, owner_key) DO UPDATE SET
         layout_revision = excluded.layout_revision,
         geometry_json = excluded.geometry_json,
         updated_at = excluded.updated_at`,
      update.sceneId,
      update.ownerKey,
      update.layoutRevision,
      JSON.stringify(update.geometry),
      update.now
    )
    return update
  }

  get(sceneId: string, ownerKey: string): StoredGeometry | undefined {
    const row = this.#database.get<GeometryRow>(
      'SELECT * FROM scene_geometry WHERE scene_id = ? AND owner_key = ?', sceneId, ownerKey
    )
    return row ? mapGeometry(row) : undefined
  }

  list(sceneId: string): StoredGeometry[] {
    return this.#database.all<GeometryRow>(
      'SELECT * FROM scene_geometry WHERE scene_id = ? ORDER BY owner_key', sceneId
    ).flatMap((row) => {
      const value = mapGeometry(row)
      return value ? [value] : []
    })
  }

  discardInvalid(sceneId: string): number {
    const rows = this.#database.all<GeometryRow>('SELECT * FROM scene_geometry WHERE scene_id = ?', sceneId)
    let removed = 0
    for (const row of rows) {
      if (!this.#ownerExists(sceneId, row.owner_key)) {
        this.#database.run('DELETE FROM scene_geometry WHERE scene_id = ? AND owner_key = ?', sceneId, row.owner_key)
        removed += 1
      }
    }
    return removed
  }

  #assertOwnerExists(sceneId: string, ownerKey: string): void {
    if (!this.#ownerExists(sceneId, ownerKey)) {
      throw new Error(`geometry owner ${ownerKey} does not exist in Scene ${sceneId}`)
    }
  }

  #ownerExists(sceneId: string, ownerKey: string): boolean {
    if (ownerKey === 'scene') {
      return Boolean(this.#database.get('SELECT id FROM scenes WHERE id = ?', sceneId))
    }
    const canvasOwner = parseCanvasOwner(ownerKey)
    if (canvasOwner) {
      if (canvasOwner.sceneId !== sceneId || !this.#database.get('SELECT id FROM scenes WHERE id = ?', sceneId)) {
        return false
      }
      if (canvasOwner.kind === 'dag-viewport' || canvasOwner.value === 'root') return true
      return Boolean(this.#database.get(
        `SELECT session_id FROM session_canvas_memberships
         WHERE scene_id = ? AND session_id = ?`,
        sceneId, canvasOwner.value
      ))
    }
    const separator = ownerKey.indexOf(':')
    if (separator < 1) return false
    const kind = ownerKey.slice(0, separator)
    const id = ownerKey.slice(separator + 1)
    const table = kind === 'node' ? 'scene_nodes' : kind === 'mount' ? 'session_mounts' : kind === 'window' ? 'scene_windows' : undefined
    if (!table) return false
    return Boolean(this.#database.get(`SELECT id FROM ${table} WHERE id = ? AND scene_id = ?`, id, sceneId))
  }
}

export class GeometryWriteBuffer {
  readonly #repository: GeometryRepository
  readonly #delayMs: number
  readonly #pending = new Map<string, GeometryUpdate>()
  #timer: ReturnType<typeof setTimeout> | undefined

  constructor(repository: GeometryRepository, delayMs = 180) {
    this.#repository = repository
    this.#delayMs = delayMs
  }

  schedule(update: GeometryUpdate): void {
    const key = `${update.sceneId}\0${update.ownerKey}`
    const pending = this.#pending.get(key)
    if (!pending || update.layoutRevision >= pending.layoutRevision) {
      this.#pending.set(key, update)
    }
    if (this.#timer === undefined) {
      this.#timer = setTimeout(() => this.flush(), this.#delayMs)
    }
  }

  flush(): void {
    if (this.#timer !== undefined) clearTimeout(this.#timer)
    this.#timer = undefined
    const updates = [...this.#pending.values()]
    this.#pending.clear()
    for (const update of updates) this.#repository.put(update)
  }
}

function parseCanvasOwner(ownerKey: string): {
  kind: 'session-group' | 'dag-viewport' | 'dag-node-layout'
  sceneId: string
  value: string
} | undefined {
  const parts = ownerKey.split(':')
  if (parts[0] === 'dag-viewport' && parts.length === 2 && parts[1]) {
    return { kind: 'dag-viewport', sceneId: parts[1], value: 'viewport' }
  }
  if ((parts[0] === 'session-group' || parts[0] === 'dag-node-layout') &&
      parts.length === 3 && parts[1] && parts[2]) {
    return { kind: parts[0], sceneId: parts[1], value: parts[2] }
  }
  return undefined
}

function mapGeometry(row: GeometryRow): StoredGeometry | undefined {
  try {
    return {
      sceneId: row.scene_id,
      ownerKey: row.owner_key,
      layoutRevision: row.layout_revision,
      geometry: JSON.parse(row.geometry_json) as unknown,
      now: row.updated_at
    }
  } catch {
    return undefined
  }
}
