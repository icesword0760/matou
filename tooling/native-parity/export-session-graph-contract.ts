// Execute the frozen graph projection and work-status service against disposable data.
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { RuntimeDatabase } from '../../apps/runtime/src/storage/database'
import { FOUNDATION_MIGRATIONS } from '../../apps/runtime/src/storage/migrations'
import { MigrationRunner } from '../../apps/runtime/src/storage/migration-runner'
import { DomainTransactionManager } from '../../apps/runtime/src/storage/domain-transaction'
import { HierarchyApplicationService } from '../../apps/runtime/src/hierarchy/hierarchy-application-service'
import { projectSceneGraphFrom } from '../../apps/runtime/src/session-canvas/session-graph-repository'
import { SessionWorkStatusService } from '../../apps/runtime/src/session-canvas/session-work-status-service'
import { ForkWorkflowService } from '../../apps/runtime/src/session-canvas/fork-workflow-service'
import { ProviderModeService } from '../../apps/runtime/src/session-canvas/provider-mode-service'

const root = mkdtempSync(join(tmpdir(), 'matou-graph-contract-'))
const path = join(root, 'matou.sqlite')
let database = RuntimeDatabase.open(path)
await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
new HierarchyApplicationService(database, new DomainTransactionManager(database)).bootstrapWindow(
  { commandId: 'bootstrap', commandType: 'window.bootstrap', requestHash: 'bootstrap' },
  { windowId: 'window', defaultRootDirectory: '/tmp/graph-workspace', defaultName: 'Workspace', now: 1000 }
)
const node = database.get<any>(`SELECT m.session_id AS sessionId,m.scene_id AS sceneId,s.task_id AS taskId,
  s.execution_context_id AS contextId FROM session_canvas_memberships m JOIN sessions s ON s.id=m.session_id LIMIT 1`)!
await database.backupTo(join(root, 'seed.sqlite'))
const seedBase64 = readFileSync(join(root, 'seed.sqlite')).toString('base64')
const service = new SessionWorkStatusService(database, new DomainTransactionManager(database))
const steps: any[] = []
function snapshot() {
  return Object.fromEntries(['sessions','domain_events','command_deduplication'].map(table => [table,database.all(`SELECT * FROM ${table} ORDER BY rowid`)]))
}
function capture(label: string, sql: string[] = [], sceneId = node.sceneId, windowId: string | null = 'window') {
  for (const query of sql) database.exec(query)
  let result: unknown, error: string | undefined
  try { result = projectSceneGraphFrom(database, sceneId, windowId ?? undefined) } catch (e) { error = (e as Error).message }
  steps.push({ op:'project',label,sql,sceneId,windowId,result,error })
}
capture('initial')
for (const [index, workStatus] of ['running','needs-input','idle','error','interrupted','interrupted'].entries()) {
  const command = { commandId:`status-${index}`,commandType:'session.work-status',requestHash:`status-${index}` }
  const input = { sessionId: node.sessionId, workStatus: workStatus as any, now: 2000+index }
  const result = service.set(command,input)
  const committedAt = database.get<any>('SELECT committed_at FROM command_deduplication WHERE command_id=?',command.commandId)!.committed_at
  steps.push({op:'status',command,input,result,committedAt,snapshot:snapshot()})
  if (index === 0) steps.push({op:'status',command,input,result:service.set(command,input),committedAt,snapshot:snapshot()})
}
const q = (value: string) => `'${value.replaceAll("'", "''")}'`
capture('mixed-children', [
  ...[['child','shell',null],['agent','claude-code',null],['archived','shell',9]].flatMap(([id,kind,archived],index) => [
    `INSERT INTO sessions (id,task_id,execution_context_id,kind,status,title,cwd,created_at,updated_at,last_activity_at,archived_at)
     VALUES (${q(String(id))},${q(node.taskId)},${q(node.contextId)},${q(String(kind))},'created',${q(String(id))},'/tmp/graph-workspace',${10+index},${10+index},${10+index},${archived??'NULL'})`,
    `INSERT INTO session_canvas_memberships (session_id,scene_id,sibling_created_seq,last_user_interaction_seq,created_at,updated_at)
     VALUES (${q(String(id))},${q(node.sceneId)},${10+index},${index},10,10)`,
    `INSERT INTO session_relation_events (event_id,relation_id,operation,task_id,from_session_id,to_session_id,relation_kind,metadata_json,command_id,occurred_at)
     VALUES ('event-${id}','relation-${id}','created',${q(node.taskId)},${q(String(id))},${q(node.sessionId)},'derived-from','{}','relation-${id}',10)`,
    `INSERT INTO session_relations_current (relation_id,task_id,from_session_id,to_session_id,relation_kind,metadata_json,created_at,updated_at,source_event_sequence)
     VALUES ('relation-${id}',${q(node.taskId)},${q(String(id))},${q(node.sessionId)},'derived-from','{}',10,10,(SELECT sequence FROM session_relation_events WHERE event_id='event-${id}'))`
  ]),
  `INSERT INTO session_graph_summaries VALUES ('child','["zero",4,"one","two","three","four"]',20)`,
  `INSERT INTO session_graph_summaries VALUES ('agent','invalid json',20)`
])
capture('git-ready', [`INSERT INTO execution_context_git_states(execution_context_id,repository_root,state,branch,dirty,updated_at)
 VALUES (${q(node.contextId)},'/tmp/graph-workspace','ready','main',1,30)`])
capture('local-failed', [`UPDATE session_environment_bindings SET state='failed',error_message='local-failure' WHERE session_id='child'`])
capture('detached-git', [`UPDATE execution_context_git_states SET branch=NULL,detached_head='abc123'`])
capture('git-unavailable', [`UPDATE execution_context_git_states SET state='unavailable',branch=NULL,detached_head=NULL,dirty=0`])
capture('no-window', [],node.sceneId, null)
capture('missing-scene', [],'missing', null)
const input = { sessionId:'missing',workStatus:'running' as const,now:3000 }
const command = {commandId:'missing',commandType:'session.work-status',requestHash:'missing'}
try {service.set(command,input)} catch(e) {steps.push({op:'status',command,input,error:(e as Error).message,committedAt:3000,snapshot:snapshot()})}

const graphTables = ['sessions', 'session_runs', 'session_canvas_memberships', 'window_scene_focus',
  'scenes', 'scene_nodes', 'session_mounts', 'session_fork_intents', 'provider_bindings',
  'session_relation_events', 'session_relations_current', 'session_graph_summaries', 'session_environment_bindings',
  'domain_events', 'command_deduplication']
const transactions = new DomainTransactionManager(database)
const forks = new ForkWorkflowService(root, database, transactions, { stopRuns: async () => {} })
const providerMode = new ProviderModeService(database, transactions)
const metadata = (label: string) => ({ commandId: label, commandType: `session.${label}`, requestHash: label })
async function observe(label: string, op: string, input: { now: number }, sql: string[], action: () => unknown,
  command = metadata(label)) {
  for (const statement of sql) database.exec(statement)
  let result: unknown, error: string | undefined, errorCode: string | undefined, errorDetail: number | undefined
  try { result = await action() } catch (failure) {
    const cause = failure as Error & { code?: string, errcode?: number }
    error = cause.message
    errorCode = cause.code
    errorDetail = cause.errcode
  }
  steps.push({ op, label, input, sql, command, result, error, errorCode, errorDetail,
    projection: projectSceneGraphFrom(database, node.sceneId, 'window'),
    committedAt: database.get<{ committed_at: number }>(
      'SELECT committed_at FROM command_deduplication WHERE command_id = ?', command.commandId
    )?.committed_at ?? input.now,
    snapshot: Object.fromEntries(graphTables.map(table => [table, database.all(`SELECT * FROM ${table} ORDER BY rowid`)])) })
}

const forbiddenStatus = { sessionId: node.sessionId, workStatus: 'fixture-invalid', now: 3100 }
await observe('invalid-work-status', 'work-status', forbiddenStatus, [], () =>
  service.set(metadata('invalid-work-status'), forbiddenStatus as any))

const fork = { windowId: 'window', sceneId: node.sceneId, sourceSessionId: node.sessionId,
  placement: 'child', name: '  固定分支  ', worktreeMode: 'current', submissionKey: 'fixed-fork', now: 3200 } as const
await observe('fork-source-not-ready', 'fork', fork, [], () =>
  forks.createForkChild(metadata('fork-source-not-ready'), fork))
await observe('fork-accepted', 'fork', fork, [
  `UPDATE sessions SET kind='claude-code', status='running', work_status='idle' WHERE id=${q(node.sessionId)}`,
  `INSERT INTO provider_bindings (id,session_id,provider,provider_session_id,resume_state,restore_state,metadata_json,created_at,updated_at,validated_at)
   VALUES ('fork-source-binding',${q(node.sessionId)},'claude-code','fixture-parent-history','available','none',
    '{"canFork":true,"model":"fixture-model","permissionMode":"plan"}',3190,3190,3190)`
], () => forks.createForkChild(metadata('fork-accepted'), fork))
const forkIds = database.get(`SELECT m.session_id AS sessionId,m.id AS mountId,m.scene_node_id AS rootNodeId,
    n.parent_node_id AS secondaryNodeId,r.relation_id AS relationId,m.scene_id AS sceneId
  FROM session_fork_intents f JOIN session_mounts m ON m.session_id=f.session_id
  JOIN scene_nodes n ON n.id=m.scene_node_id JOIN session_relations_current r ON r.from_session_id=m.session_id
  WHERE f.submission_key='fixed-fork'`)
if (!forkIds) throw new Error('Expected a durable fork and its generated identifiers')
steps.at(-1).ids = forkIds
await observe('fork-submission-replay', 'fork', { ...fork, now: 3201 }, [], () =>
  forks.createForkChild(metadata('fork-submission-replay'), { ...fork, now: 3201 }))
await observe('fork-name-conflict', 'fork', { ...fork, submissionKey: 'other-fork', now: 3202 }, [], () =>
  forks.createForkChild(metadata('fork-name-conflict'), { ...fork, submissionKey: 'other-fork', now: 3202 }))

const load = { sessionId: 'child', sceneId: node.sceneId, cwd: '/tmp/graph-workspace', bindingId: 'catalog-binding',
  providerSessionId: 'fixture-history', title: '固定历史会话', permissionMode: 'plan', model: 'fixture-model', now: 3300 } as const
await observe('catalog-load', 'catalog', load, [], () => providerMode.loadClaudeSession(metadata('catalog-load'), load))
await observe('catalog-load-replay', 'catalog', load, [], () =>
  providerMode.loadClaudeSession(metadata('catalog-load'), load), metadata('catalog-load'))
const overlappingLoad = { ...load, providerSessionId: 'fixture-other-history', bindingId: 'catalog-other', now: 3301 }
await observe('catalog-with-active-run', 'catalog', overlappingLoad, [
  `INSERT INTO session_runs (id,session_id,ordinal,runtime_generation,profile,status,cols,rows,started_at)
   VALUES ('catalog-active-run','child',1,'fixture-generation','claude-code','running',80,24,3301)`
], () => providerMode.loadClaudeSession(metadata('catalog-with-active-run'), overlappingLoad))
const archivedLoad = { ...load, sessionId: 'archived', now: 3302 }
await observe('catalog-archived', 'catalog', archivedLoad, [], () =>
  providerMode.loadClaudeSession(metadata('catalog-archived'), archivedLoad))
await observe('graph-after-reopen', 'reopen', { now: 3400 }, [], () => {
  database.close()
  database = RuntimeDatabase.open(path)
  return projectSceneGraphFrom(database, node.sceneId, 'window')
})
capture('shared-cwd-across-scenes', [
  `INSERT INTO scenes (id,task_id,name,mode,created_at,updated_at)
    VALUES ('other-scene',${q(node.taskId)},'其他画布','tile',3410,3410)`,
  ...[['local-card', node.sceneId], ['other-card', 'other-scene']].flatMap(([id, scene], index) => [
    `INSERT INTO sessions (id,task_id,execution_context_id,kind,status,title,cwd,created_at,updated_at,last_activity_at)
      VALUES (${q(id)},${q(node.taskId)},${q(node.contextId)},'shell','created',${q(id)},'/tmp/graph-shared-across-scenes',3410,3410,3410)`,
    `INSERT INTO session_canvas_memberships (session_id,scene_id,sibling_created_seq,created_at,updated_at)
      VALUES (${q(id)},${q(scene)},${40 + index},3410,3410)`
  ])
], 'other-scene', null)
capture('shared-cwd-after-other-card-archived', [
  "UPDATE sessions SET status='archived',archived_at=3420 WHERE id='local-card'"
], 'other-scene', null)
database.close()
writeFileSync(process.argv[2]!,JSON.stringify({versions:process.versions,seedBase64,steps},null,2)+'\n')
console.log(JSON.stringify({root,steps:steps.length}))
