import type {
  DomainCommandMetadata,
  LayoutNode,
  LayoutSplitNode
} from '@matou/domain'
import { normalizeLayout } from '@matou/domain'

import type { DatabaseTransaction, RuntimeDatabase } from '../storage/database'
import type { DomainTransactionManager } from '../storage/domain-transaction'

interface SceneRevisionRow {
  id: string
  task_id: string
  layout_revision: number
  root_node_id: string | null
}

interface NodeRow {
  id: string
  parent_node_id: string | null
  kind: 'root' | 'split' | 'mount' | 'group'
  direction: LayoutSplitNode['direction'] | null
  ordinal: number
}

export interface ReplaceLayoutInput {
  sceneId: string
  expectedRevision: number
  root: LayoutNode
  now: number
}

export interface ReplaceLayoutResult {
  sceneId: string
  layoutRevision: number
  root: LayoutNode
}

export class SceneLayoutService {
  readonly #database: RuntimeDatabase
  readonly #transactions: DomainTransactionManager

  constructor(database: RuntimeDatabase, transactions: DomainTransactionManager) {
    this.#database = database
    this.#transactions = transactions
  }

  replaceLayout(
    command: DomainCommandMetadata,
    input: ReplaceLayoutInput
  ): ReplaceLayoutResult {
    const normalized = normalizeLayout(input.root)
    return this.#transactions.execute(command, ({ tx, emit }) => {
      const scene = tx.get<SceneRevisionRow>(
        `SELECT id, task_id, layout_revision, root_node_id FROM scenes
         WHERE id = ? AND archived_at IS NULL`,
        input.sceneId
      )
      if (!scene) throw new Error(`Scene ${input.sceneId} does not exist`)
      if (scene.layout_revision !== input.expectedRevision) {
        throw new Error(
          `Scene layout revision conflict: expected ${input.expectedRevision}, current ${scene.layout_revision}`
        )
      }

      const layoutMounts = collectMountIds(normalized).sort()
      const storedMounts = tx.all<{ id: string }>(
        'SELECT id FROM session_mounts WHERE scene_id = ? ORDER BY id',
        input.sceneId
      ).map(({ id }) => id)
      if (
        layoutMounts.length !== storedMounts.length ||
        layoutMounts.some((id, index) => id !== storedMounts[index])
      ) {
        throw new Error('layout mounts must exactly match the Scene mounts')
      }

      tx.run('UPDATE session_mounts SET scene_node_id = NULL WHERE scene_id = ?', input.sceneId)
      tx.run('DELETE FROM scene_nodes WHERE scene_id = ?', input.sceneId)
      insertLayoutNodes(tx, input.sceneId, normalized, null, input.now)
      const layoutRevision = scene.layout_revision + 1
      tx.run(
        `UPDATE scenes SET root_node_id = ?, layout_revision = ?, updated_at = ?
         WHERE id = ?`,
        normalized.id,
        layoutRevision,
        input.now,
        input.sceneId
      )
      emit({
        eventId: `${command.commandId}:scene-layout-replaced`,
        eventType: 'scene.layout-replaced',
        aggregateType: 'scene',
        aggregateId: input.sceneId,
        taskId: scene.task_id,
        payload: { root: normalized, layoutRevision },
        occurredAt: input.now
      })
      return { sceneId: input.sceneId, layoutRevision, root: normalized }
    }).result
  }

  getLayout(sceneId: string): LayoutNode | undefined {
    const scene = this.#database.get<{ root_node_id: string | null }>(
      'SELECT root_node_id FROM scenes WHERE id = ? AND archived_at IS NULL',
      sceneId
    )
    if (!scene?.root_node_id) return undefined
    const nodes = this.#database.all<NodeRow>(
      `SELECT id, parent_node_id, kind, direction, ordinal
       FROM scene_nodes WHERE scene_id = ? ORDER BY ordinal, id`,
      sceneId
    )
    const mountByNode = new Map(this.#database.all<{
      scene_node_id: string
      id: string
    }>(
      `SELECT scene_node_id, id FROM session_mounts
       WHERE scene_id = ? AND scene_node_id IS NOT NULL`,
      sceneId
    ).map((row) => [row.scene_node_id, row.id]))
    return readLayoutNode(scene.root_node_id, nodes, mountByNode)
  }
}

function collectMountIds(root: LayoutNode): string[] {
  if (root.kind === 'mount') return [root.mountId]
  return root.children.flatMap(collectMountIds)
}

function insertLayoutNodes(
  tx: DatabaseTransaction,
  sceneId: string,
  node: LayoutNode,
  parentNodeId: string | null,
  now: number,
  ordinal = 0
): void {
  tx.run(
    `INSERT INTO scene_nodes (
       id, scene_id, parent_node_id, kind, direction, ordinal, created_at
     ) VALUES (?, ?, ?, ?, ?, ?, ?)`,
    node.id,
    sceneId,
    parentNodeId,
    node.kind,
    node.kind === 'split' ? node.direction : null,
    ordinal,
    now
  )
  if (node.kind === 'mount') {
    tx.run(
      'UPDATE session_mounts SET scene_node_id = ? WHERE id = ? AND scene_id = ?',
      node.id,
      node.mountId,
      sceneId
    )
    return
  }
  node.children.forEach((child, index) => {
    insertLayoutNodes(tx, sceneId, child, node.id, now, index)
  })
}

function readLayoutNode(
  nodeId: string,
  nodes: NodeRow[],
  mountByNode: Map<string, string>
): LayoutNode {
  const node = nodes.find(({ id }) => id === nodeId)
  if (!node) throw new Error(`stored layout node ${nodeId} does not exist`)
  const mountId = mountByNode.get(nodeId)
  if (mountId !== undefined) return { id: nodeId, kind: 'mount', mountId }
  if (node.kind !== 'split' || node.direction === null) {
    throw new Error(`stored layout node ${nodeId} has no mount or split definition`)
  }
  const children = nodes
    .filter(({ parent_node_id }) => parent_node_id === nodeId)
    .sort((left, right) => left.ordinal - right.ordinal)
    .map((child) => readLayoutNode(child.id, nodes, mountByNode))
  return { id: nodeId, kind: 'split', direction: node.direction, children }
}
