import { createHash } from 'node:crypto'
import { readFile, readdir, stat } from 'node:fs/promises'
import { join } from 'node:path'

import type { DomainCommit } from '@matou/domain'

import { SegmentJournal } from '../../journal/segment-journal'
import type { RuntimeDatabase, DatabaseTransaction } from '../../storage/database'
import type { DomainTransactionManager } from '../../storage/domain-transaction'

type LegacyRecord = Record<string, unknown>

interface LegacySnapshot extends LegacyRecord {
  version: number
  projects: { list: LegacyRecord[]; activeProjectId?: string }
  workbenches: Record<string, LegacyRecord>
  tabs: Record<string, LegacyRecord>
  panels: Record<string, LegacyRecord>
  recoveryOffsets?: { metadataJournalBytes?: number }
}

export interface LegacyImportReport {
  source: 'snapshot.json' | 'checkpoint.json' | 'checkpoint.prev.json'
  counts: {
    workspaces: number; tasks: number; scenes: number; sessions: number
    providerBindings: number; relations: number; journals: number
  }
  ignored: Array<{ legacyType: string; legacyId: string; reason: string }>
  repaired: Array<{ code: string; detail: string }>
  consistency: { danglingLayoutPanels: string[]; unresolvedTeamLeads: string[] }
}

export interface LegacyImportResult {
  importRunId: string
  sourceFingerprint: string
  report: LegacyImportReport
  replayed: boolean
}

interface LoadedSource {
  snapshot: LegacySnapshot
  source: LegacyImportReport['source']
  repaired: LegacyImportReport['repaired']
}

export class LegacyImporter {
  readonly #dataRoot: string
  readonly #database: RuntimeDatabase
  readonly #transactions: DomainTransactionManager

  constructor(dataRoot: string, database: RuntimeDatabase, transactions: DomainTransactionManager) {
    this.#dataRoot = dataRoot
    this.#database = database
    this.#transactions = transactions
  }

  async importSource(sourceRoot: string): Promise<LegacyImportResult> {
    const sourceFingerprint = await fingerprintSource(sourceRoot)
    const existing = this.#database.get<{ id: string; report_json: string }>(
      "SELECT id, report_json FROM legacy_import_runs WHERE source_fingerprint = ? AND status = 'completed'",
      sourceFingerprint
    )
    if (existing) {
      return {
        importRunId: existing.id, sourceFingerprint,
        report: JSON.parse(existing.report_json) as LegacyImportReport, replayed: true
      }
    }
    const loaded = await loadSource(sourceRoot)
    const metadataRepair = await applyMetadataTail(sourceRoot, loaded.snapshot)
    loaded.repaired.push(...metadataRepair)
    const importRunId = `legacy-import-${sourceFingerprint.slice(0, 24)}`
    const report = emptyReport(loaded.source, loaded.repaired)
    const panels = sanitizePanels(loaded.snapshot.panels, report)
    loaded.snapshot.panels = panels
    const commandId = `legacy-import:${sourceFingerprint}`
    const command = { commandId, commandType: 'legacy.import', requestHash: sourceFingerprint }

    const commit: DomainCommit<LegacyImportResult> = this.#transactions.execute(command, ({ tx, emit }) => {
      tx.run(
        `INSERT INTO legacy_import_runs (id, source_fingerprint, status, report_json, started_at)
         VALUES (?, ?, 'running', '{}', ?)`, importRunId, sourceFingerprint, Date.now()
      )
      const ids = this.#importSnapshot(tx, importRunId, loaded.snapshot, report)
      const result: LegacyImportResult = { importRunId, sourceFingerprint, report, replayed: false }
      tx.run(
        "UPDATE legacy_import_runs SET status = 'completed', report_json = ?, completed_at = ? WHERE id = ?",
        JSON.stringify(report), Date.now(), importRunId
      )
      emit({
        eventId: commandId, eventType: 'legacy.import-completed', aggregateType: 'legacy-import',
        aggregateId: importRunId, payload: { importRunId, sourceFingerprint, counts: report.counts }, occurredAt: Date.now()
      })
      // Keep the mapping available for terminal history import after the SQLite commit.
      void ids
      return result
    })

    report.counts.journals = await this.#importTerminalHistory(sourceRoot, importRunId, panels, report)
    this.#database.run('UPDATE legacy_import_runs SET report_json = ? WHERE id = ?', JSON.stringify(report), importRunId)
    return { ...commit.result, report }
  }

  #importSnapshot(
    tx: DatabaseTransaction,
    importRunId: string,
    snapshot: LegacySnapshot,
    report: LegacyImportReport
  ): Map<string, string> {
    const panelSessionIds = new Map<string, string>()
    const projectWorkspaceIds = new Map<string, string>()
    const workbenchTaskIds = new Map<string, string>()
    const taskContextIds = new Map<string, string>()
    const now = snapshotTime(snapshot)

    for (const project of snapshot.projects.list) {
      const legacyId = text(project.id)
      const name = text(project.name)
      if (!legacyId || !name) {
        report.ignored.push({ legacyType: 'project', legacyId: legacyId || '<missing>', reason: 'missing id or name' })
        continue
      }
      const workspaceId = legacyIdFor('workspace-project', legacyId)
      const root = text(project.path) || process.env.HOME || '/'
      tx.run(
        `INSERT OR IGNORE INTO workspaces (
           id, name, root_directory, path_identity, task_order_json, created_at, updated_at
         ) VALUES (?, ?, ?, ?, '[]', ?, ?)`,
        workspaceId, name, root, `legacy:${root}`, now, now
      )
      mapEntity(tx, importRunId, 'project', legacyId, 'workspace', workspaceId)
      projectWorkspaceIds.set(legacyId, workspaceId)
      report.counts.workspaces += 1
    }

    for (const [key, workbench] of Object.entries(snapshot.workbenches)) {
      const legacyId = text(workbench.id) || key
      const projectId = text(workbench.projectId)
      const workspaceId = projectWorkspaceIds.get(projectId)
      if (!workspaceId) {
        report.ignored.push({ legacyType: 'workbench', legacyId, reason: 'project missing' })
        continue
      }
      const taskId = legacyIdFor('task-workbench', legacyId)
      const contextId = legacyIdFor('context-workbench', legacyId)
      const root = tx.get<{ root_directory: string }>('SELECT root_directory FROM workspaces WHERE id = ?', workspaceId)!.root_directory
      tx.run(
        `INSERT OR IGNORE INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
         VALUES (?, ?, 'plain-directory', ?, ?)`, contextId, workspaceId, root, now
      )
      tx.run(
        `INSERT OR IGNORE INTO tasks (
           id, workspace_id, execution_context_id, title, status, sort_key, created_at, updated_at
         ) VALUES (?, ?, ?, ?, 'active', ?, ?, ?)`,
        taskId, workspaceId, contextId, text(workbench.name) || 'Imported Task', legacyId, now, now
      )
      mapEntity(tx, importRunId, 'workbench', legacyId, 'task', taskId)
      workbenchTaskIds.set(legacyId, taskId)
      taskContextIds.set(legacyId, contextId)
      report.counts.tasks += 1
    }

    for (const [key, panel] of Object.entries(snapshot.panels)) {
      const legacyId = text(panel.id) || key
      const legacyWorkbenchId = text(panel.workbenchId)
      const taskId = workbenchTaskIds.get(legacyWorkbenchId)
      if (!taskId) {
        report.ignored.push({ legacyType: 'panel', legacyId, reason: 'workbench missing' })
        continue
      }
      const workspaceId = tx.get<{ workspace_id: string }>('SELECT workspace_id FROM tasks WHERE id = ?', taskId)!.workspace_id
      const fallbackContext = taskContextIds.get(legacyWorkbenchId)!
      const fallbackCwd = tx.get<{ cwd: string }>('SELECT cwd FROM execution_contexts WHERE id = ?', fallbackContext)!.cwd
      const cwd = text(panel.cwd) || fallbackCwd
      const contextId = legacyIdFor('context-panel', legacyId)
      tx.run(
        `INSERT OR IGNORE INTO execution_contexts (id, workspace_id, kind, cwd, created_at)
         VALUES (?, ?, 'plain-directory', ?, ?)`, contextId, workspaceId, cwd, now
      )
      const sessionId = legacyIdFor('session-panel', legacyId)
      tx.run(
        `INSERT OR IGNORE INTO sessions (
           id, task_id, execution_context_id, kind, status, title,
           created_at, updated_at, last_activity_at
         ) VALUES (?, ?, ?, ?, 'created', ?, ?, ?, ?)`,
        sessionId, taskId, contextId, sessionKind(panel), text(panel.title) || text(panel.aiSessionName) || 'Imported Session', now, now, now
      )
      mapEntity(tx, importRunId, 'panel', legacyId, 'session', sessionId)
      const terminalId = text(panel.terminalId)
      if (terminalId) mapEntity(tx, importRunId, 'terminal', terminalId, 'session', sessionId)
      panelSessionIds.set(legacyId, sessionId)
      report.counts.sessions += 1

      const providerSessionId = text(panel.claudeSessionId)
      if (providerSessionId) {
        const provider = providerFor(panel)
        const providerBindingId = legacyIdFor(
          'provider-binding', `${sessionId}:${provider}:${providerSessionId}`
        )
        try {
          tx.run(
            `INSERT INTO provider_bindings (
               id, session_id, provider, provider_session_id, resume_state,
               metadata_json, created_at, updated_at
             ) VALUES (?, ?, ?, ?, 'available', ?, ?, ?)`,
            providerBindingId, sessionId, provider,
            providerSessionId, JSON.stringify(providerMetadata(panel)), now, now
          )
          mapEntity(tx, importRunId, 'provider-session', `${sessionId}:${providerSessionId}`,
            'provider-binding', providerBindingId)
          report.counts.providerBindings += 1
        } catch (error) {
          report.ignored.push({ legacyType: 'provider-session', legacyId: providerSessionId, reason: `duplicate or invalid: ${errorMessage(error)}` })
        }
      }
    }

    for (const [key, tab] of Object.entries(snapshot.tabs)) {
      const legacyId = text(tab.id) || key
      const taskId = workbenchTaskIds.get(text(tab.workbenchId))
      if (!taskId) {
        report.ignored.push({ legacyType: 'tab', legacyId, reason: 'workbench missing' })
        continue
      }
      const sceneId = legacyIdFor('scene-tab', legacyId)
      const rootNodeId = legacyIdFor('scene-root', legacyId)
      tx.run(
        `INSERT OR IGNORE INTO scenes (id, task_id, name, mode, root_node_id, created_at, updated_at)
         VALUES (?, ?, ?, 'tile', ?, ?, ?)`, sceneId, taskId, text(tab.name) || 'Imported Scene', rootNodeId, now, now
      )
      tx.run(
        "INSERT OR IGNORE INTO scene_nodes (id, scene_id, kind, ordinal, created_at) VALUES (?, ?, 'root', 0, ?)",
        rootNodeId, sceneId, now
      )
      mapEntity(tx, importRunId, 'tab', legacyId, 'scene', sceneId)
      const mounted = new Set<string>()
      const dangling = report.consistency.danglingLayoutPanels
      const layout = isRecord(tab.layoutRoot) ? tab.layoutRoot : undefined
      if (layout) {
        importLayout(tx, layout, sceneId, rootNodeId, panelSessionIds, mounted, dangling, now, 0)
      }
      for (const [panelId, panel] of Object.entries(snapshot.panels)) {
        if (text(panel.tabId) !== legacyId || mounted.has(panelId)) continue
        const sessionId = panelSessionIds.get(panelId)
        if (!sessionId) continue
        const nodeId = legacyIdFor('scene-node-fallback', `${legacyId}:${panelId}`)
        tx.run(
          "INSERT OR IGNORE INTO scene_nodes (id, scene_id, parent_node_id, kind, ordinal, created_at) VALUES (?, ?, ?, 'mount', ?, ?)",
          nodeId, sceneId, rootNodeId, mounted.size, now
        )
        mount(tx, sceneId, nodeId, sessionId, panelId, now)
      }
      report.counts.scenes += 1
    }

    this.#importTeams(tx, snapshot.panels, panelSessionIds, report, now)
    return panelSessionIds
  }

  #importTeams(
    tx: DatabaseTransaction,
    panels: Record<string, LegacyRecord>,
    panelSessionIds: Map<string, string>,
    report: LegacyImportReport,
    now: number
  ): void {
    const providerToPanel = new Map<string, string>()
    for (const [panelId, panel] of Object.entries(panels)) {
      const providerId = text(panel.claudeSessionId)
      if (providerId) providerToPanel.set(providerId, panelId)
    }
    for (const [panelId, panel] of Object.entries(panels)) {
      if (!text(panel.teamId) || text(panel.teamRole) === 'leader') continue
      const memberSession = panelSessionIds.get(panelId)
      const declaredLead = text(panel.teamLeadSessionId)
      const leadPanelId = declaredLead
        ? providerToPanel.get(declaredLead)
        : findTeamLeader(panels, text(panel.teamId))
      const leadSession = leadPanelId ? panelSessionIds.get(leadPanelId) : undefined
      if (!memberSession || !leadSession) {
        report.consistency.unresolvedTeamLeads.push(panelId)
        continue
      }
      const memberTask = tx.get<{ task_id: string }>('SELECT task_id FROM sessions WHERE id = ?', memberSession)!.task_id
      const leadTask = tx.get<{ task_id: string }>('SELECT task_id FROM sessions WHERE id = ?', leadSession)!.task_id
      if (memberTask !== leadTask) {
        report.consistency.unresolvedTeamLeads.push(panelId)
        continue
      }
      const relationId = legacyIdFor('relation-team', `${text(panel.teamId)}:${panelId}`)
      const eventId = `${relationId}:created`
      const metadata = { teamId: text(panel.teamId), role: text(panel.teamRole), label: text(panel.teamLabel) }
      const insertion = tx.run(
        `INSERT INTO session_relation_events (
           event_id, relation_id, operation, task_id, from_session_id, to_session_id,
           relation_kind, metadata_json, command_id, occurred_at
         ) VALUES (?, ?, 'created', ?, ?, ?, 'team-member-of', ?, ?, ?)`,
        eventId, relationId, memberTask, memberSession, leadSession, JSON.stringify(metadata), eventId, now
      )
      tx.run(
        `INSERT INTO session_relations_current (
           relation_id, task_id, from_session_id, to_session_id, relation_kind,
           metadata_json, created_at, updated_at, source_event_sequence
         ) VALUES (?, ?, ?, ?, 'team-member-of', ?, ?, ?, ?)`,
        relationId, memberTask, memberSession, leadSession, JSON.stringify(metadata), now, now, Number(insertion.lastInsertRowid)
      )
      report.counts.relations += 1
    }
    report.consistency.unresolvedTeamLeads.sort()
    report.consistency.danglingLayoutPanels = [...new Set(report.consistency.danglingLayoutPanels)].sort()
  }

  async #importTerminalHistory(
    sourceRoot: string,
    importRunId: string,
    panels: Record<string, LegacyRecord>,
    report: LegacyImportReport
  ): Promise<number> {
    let imported = 0
    for (const [panelId, panel] of Object.entries(panels)) {
      const terminalId = text(panel.terminalId)
      if (!terminalId) continue
      const mapping = this.#database.get<{ entity_id: string }>(
        `SELECT entity_id FROM legacy_entity_mappings
         WHERE import_run_id = ? AND legacy_type = 'panel' AND legacy_id = ?`, importRunId, panelId
      )
      if (!mapping) continue
      const candidates = [
        join(sourceRoot, 'journals', 'terminals', `${terminalId}.log`),
        join(sourceRoot, 'scrollback', `${terminalId}.txt`)
      ]
      let content: Buffer | undefined
      for (const path of candidates) {
        try { content = await readFile(path); if (content.byteLength > 0) break } catch {}
      }
      if (!content || content.byteLength === 0) continue
      try {
        const journal = await SegmentJournal.open(this.#dataRoot, mapping.entity_id)
        if (journal.lastSequence === 0) await journal.appendOutput(1, content)
        await journal.close()
        imported += 1
      } catch (error) {
        report.ignored.push({ legacyType: 'terminal-journal', legacyId: terminalId, reason: errorMessage(error) })
      }
    }
    return imported
  }
}

async function loadSource(sourceRoot: string): Promise<LoadedSource> {
  const repaired: LegacyImportReport['repaired'] = []
  let firstFailure = false
  for (const source of ['snapshot.json', 'checkpoint.json', 'checkpoint.prev.json'] as const) {
    try {
      const parsed = JSON.parse(await readFile(join(sourceRoot, source), 'utf8')) as unknown
      if (!isSnapshot(parsed)) throw new Error('invalid snapshot shape')
      if (firstFailure) repaired.push({ code: 'snapshot-fallback', detail: `loaded ${source}` })
      return { snapshot: structuredClone(parsed), source, repaired }
    } catch {
      firstFailure = true
    }
  }
  throw new Error('Legacy source has no valid snapshot or checkpoint')
}

export async function readLegacySnapshot(sourceRoot: string): Promise<LegacySnapshot> {
  const loaded = await loadSource(sourceRoot)
  await applyMetadataTail(sourceRoot, loaded.snapshot)
  return loaded.snapshot
}

async function applyMetadataTail(sourceRoot: string, snapshot: LegacySnapshot): Promise<LegacyImportReport['repaired']> {
  const repaired: LegacyImportReport['repaired'] = []
  let raw: Buffer
  try { raw = await readFile(join(sourceRoot, 'journals', 'metadata.ndjson')) } catch { return repaired }
  const offset = Number.isSafeInteger(snapshot.recoveryOffsets?.metadataJournalBytes)
    ? Math.min(raw.byteLength, snapshot.recoveryOffsets!.metadataJournalBytes!) : 0
  const tail = raw.subarray(offset).toString('utf8')
  const lines = tail.split('\n')
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index]!.trim()
    if (!line) continue
    try {
      const event = JSON.parse(line) as { type?: unknown; payload?: unknown }
      if (typeof event.type !== 'string' || !isRecord(event.payload)) continue
      applyLegacyEvent(snapshot, event.type, event.payload)
    } catch {
      if (index === lines.length - 1) repaired.push({ code: 'metadata-tail-truncated', detail: `line ${index + 1}` })
      else repaired.push({ code: 'metadata-record-ignored', detail: `line ${index + 1}` })
    }
  }
  return repaired
}

export function applyLegacyEvent(snapshot: LegacySnapshot, type: string, payload: LegacyRecord): void {
  const projectId = text(payload.projectId) || text(payload.id)
  const workbenchId = text(payload.workbenchId) || text(payload.id)
  const tabId = text(payload.tabId) || text(payload.id)
  const panelId = text(payload.panelId) || text(payload.id)
  const projects = snapshot.projects.list
  if (type === 'project-created' || type === 'project-updated') {
    const existing = projects.find((item) => text(item.id) === projectId)
    if (existing) Object.assign(existing, payload); else if (projectId) projects.push({ ...payload, id: projectId })
  } else if (type === 'project-removed') {
    snapshot.projects.list = projects.filter((item) => text(item.id) !== projectId)
  } else if (type === 'workbench-created' || type === 'workbench-updated') {
    if (workbenchId) snapshot.workbenches[workbenchId] = { ...(snapshot.workbenches[workbenchId] ?? {}), ...payload, id: workbenchId }
  } else if (type === 'workbench-removed') {
    delete snapshot.workbenches[workbenchId]
  } else if (['tab-created', 'tab-updated', 'tab-renamed'].includes(type)) {
    if (tabId) snapshot.tabs[tabId] = {
      ...(snapshot.tabs[tabId] ?? {}), ...payload, id: tabId,
      ...(Object.prototype.hasOwnProperty.call(payload, 'title') ? { name: payload.title } : {})
    }
  } else if (type === 'split-tree-updated' && tabId) {
    snapshot.tabs[tabId] = { ...(snapshot.tabs[tabId] ?? {}), id: tabId, workbenchId, layoutRoot: payload.tree, activeLeafId: payload.activeLeafId }
  } else if (type === 'tab-removed') {
    delete snapshot.tabs[tabId]
  } else if (['panel-created', 'panel-updated', 'panel-session-bound', 'panel-permission-mode', 'panel-shell-state', 'panel-detached', 'panel-attached'].includes(type)) {
    if (panelId) snapshot.panels[panelId] = { ...(snapshot.panels[panelId] ?? {}), ...payload, id: panelId, detached: type === 'panel-detached' ? true : type === 'panel-attached' ? false : payload.detached }
  } else if (type === 'panel-removed') {
    delete snapshot.panels[panelId]
  }
}

async function fingerprintSource(root: string): Promise<string> {
  const digest = createHash('sha256')
  async function walk(directory: string, prefix = ''): Promise<void> {
    let entries
    try { entries = await readdir(directory, { withFileTypes: true }) } catch { return }
    for (const entry of entries.sort((a, b) => a.name.localeCompare(b.name))) {
      const relative = prefix ? `${prefix}/${entry.name}` : entry.name
      const path = join(directory, entry.name)
      if (entry.isDirectory()) await walk(path, relative)
      else if (entry.isFile()) {
        digest.update(relative).update('\0').update(await readFile(path)).update('\0')
      }
    }
  }
  await walk(root)
  return digest.digest('hex')
}

function sanitizePanels(input: Record<string, LegacyRecord>, report: LegacyImportReport): Record<string, LegacyRecord> {
  const output: Record<string, LegacyRecord> = {}
  for (const [key, panel] of Object.entries(input ?? {})) {
    const id = text(panel?.id) || key
    if (!isRecord(panel) || !id || !text(panel.projectId) || !text(panel.workbenchId)) {
      report.ignored.push({ legacyType: 'panel', legacyId: id || key, reason: 'invalid panel shape' })
      continue
    }
    output[id] = { ...panel, id }
  }
  return output
}

function importLayout(
  tx: DatabaseTransaction,
  node: LegacyRecord,
  sceneId: string,
  parentNodeId: string,
  sessions: Map<string, string>,
  mounted: Set<string>,
  dangling: string[],
  now: number,
  ordinal: number
): void {
  const legacyNodeId = text(node.id) || `${parentNodeId}:${ordinal}`
  const nodeId = legacyIdFor('scene-node', `${sceneId}:${legacyNodeId}`)
  if (node.type === 'leaf') {
    const panelId = text(node.panelId)
    const sessionId = sessions.get(panelId)
    if (!sessionId) { if (panelId) dangling.push(panelId); return }
    tx.run(
      "INSERT OR IGNORE INTO scene_nodes (id, scene_id, parent_node_id, kind, ordinal, created_at) VALUES (?, ?, ?, 'mount', ?, ?)",
      nodeId, sceneId, parentNodeId, ordinal, now
    )
    mount(tx, sceneId, nodeId, sessionId, panelId, now)
    mounted.add(panelId)
    return
  }
  if (node.type !== 'split') return
  const direction = node.direction === 'vertical' ? 'vertical' : 'horizontal'
  tx.run(
    "INSERT OR IGNORE INTO scene_nodes (id, scene_id, parent_node_id, kind, direction, ordinal, created_at) VALUES (?, ?, ?, 'split', ?, ?, ?)",
    nodeId, sceneId, parentNodeId, direction, ordinal, now
  )
  const children = Array.isArray(node.children) ? node.children : []
  children.forEach((child, index) => { if (isRecord(child)) importLayout(tx, child, sceneId, nodeId, sessions, mounted, dangling, now, index) })
}

function mount(tx: DatabaseTransaction, sceneId: string, nodeId: string, sessionId: string, panelId: string, now: number): void {
  tx.run(
    `INSERT OR IGNORE INTO session_mounts (id, scene_id, scene_node_id, session_id, created_at)
     VALUES (?, ?, ?, ?, ?)`, legacyIdFor('mount-panel', `${sceneId}:${panelId}`), sceneId, nodeId, sessionId, now
  )
}

function mapEntity(tx: DatabaseTransaction, run: string, legacyType: string, legacyId: string, entityType: string, entityId: string): void {
  tx.run(
    `INSERT OR REPLACE INTO legacy_entity_mappings
     (import_run_id, legacy_type, legacy_id, entity_type, entity_id) VALUES (?, ?, ?, ?, ?)`,
    run, legacyType, legacyId, entityType, entityId
  )
}

export function legacyIdFor(kind: string, legacyId: string): string {
  return `legacy-${kind}-${createHash('sha256').update(legacyId).digest('hex').slice(0, 20)}`
}

function sessionKind(panel: LegacyRecord): 'shell' | 'claude-code' | 'codex' | 'agent-team-member' {
  if (text(panel.teamId)) return 'agent-team-member'
  const brand = text(panel.cliBrand).toLowerCase()
  if (brand === 'codex') return 'codex'
  return text(panel.mode) === 'shell' && !text(panel.claudeSessionId) ? 'shell' : 'claude-code'
}

function providerFor(panel: LegacyRecord): 'claude-code' | 'codex' | 'generic' {
  const brand = text(panel.cliBrand).toLowerCase()
  return brand === 'codex' ? 'codex' : brand && brand !== 'claude-code' ? 'generic' : 'claude-code'
}

function providerMetadata(panel: LegacyRecord): unknown {
  return {
    permissionMode: text(panel.aiPermissionMode) || 'default',
    modelStrategy: text(panel.claudeModelStrategy),
    cliBrand: text(panel.cliBrand) || 'claude-code',
    teamId: text(panel.teamId) || undefined,
    teamRole: text(panel.teamRole) || undefined
  }
}

function findTeamLeader(panels: Record<string, LegacyRecord>, teamId: string): string | undefined {
  return Object.entries(panels).find(([, panel]) => text(panel.teamId) === teamId && text(panel.teamRole) === 'leader')?.[0]
}

function isSnapshot(value: unknown): value is LegacySnapshot {
  if (!isRecord(value) || !Number.isFinite(value.version) || !isRecord(value.projects) || !Array.isArray(value.projects.list)) return false
  return isRecord(value.workbenches) && isRecord(value.tabs) && isRecord(value.panels)
}

function isRecord(value: unknown): value is LegacyRecord {
  return Boolean(value) && typeof value === 'object' && !Array.isArray(value)
}

function text(value: unknown): string {
  return typeof value === 'string' ? value.trim() : ''
}

function snapshotTime(snapshot: LegacySnapshot): number {
  const parsed = Date.parse(text(snapshot.savedAt))
  return Number.isFinite(parsed) ? parsed : Date.now()
}

function emptyReport(source: LegacyImportReport['source'], repaired: LegacyImportReport['repaired']): LegacyImportReport {
  return {
    source,
    counts: { workspaces: 0, tasks: 0, scenes: 0, sessions: 0, providerBindings: 0, relations: 0, journals: 0 },
    ignored: [], repaired: [...repaired],
    consistency: { danglingLayoutPanels: [], unresolvedTeamLeads: [] }
  }
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error)
}
