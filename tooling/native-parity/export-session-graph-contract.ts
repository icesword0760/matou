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

const root = mkdtempSync(join(tmpdir(), 'matou-graph-contract-'))
const database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
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
database.close()
writeFileSync(process.argv[2]!,JSON.stringify({versions:process.versions,seedBase64,steps},null,2)+'\n')
console.log(JSON.stringify({root,steps:steps.length}))
