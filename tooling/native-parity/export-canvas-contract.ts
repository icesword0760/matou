// Execute frozen canvas/navigation services; expected values never come from Rust.
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { RuntimeDatabase } from '../../apps/runtime/src/storage/database'
import { FOUNDATION_MIGRATIONS } from '../../apps/runtime/src/storage/migrations'
import { MigrationRunner } from '../../apps/runtime/src/storage/migration-runner'
import { DomainTransactionManager } from '../../apps/runtime/src/storage/domain-transaction'
import { HierarchyApplicationService, readCreatedHierarchyPathForSession } from '../../apps/runtime/src/hierarchy/hierarchy-application-service'
import { SessionCanvasService } from '../../apps/runtime/src/session-canvas/session-canvas-service'
import { SceneRepository } from '../../apps/runtime/src/scenes/scene-repository'
import { runtimeMessages } from '../../apps/runtime/src/i18n/messages'

const root = mkdtempSync(join(tmpdir(), 'matou-canvas-contract-'))
const database = RuntimeDatabase.open(join(root, 'matou.sqlite'))
await new MigrationRunner(database, FOUNDATION_MIGRATIONS).migrate()
const transactions = new DomainTransactionManager(database)
const hierarchy = new HierarchyApplicationService(database, transactions)
const canvas = new SessionCanvasService(database, transactions)
const scenes = new SceneRepository(database, transactions)
const bootstrap = hierarchy.bootstrapWindow(
  {commandId:'bootstrap',commandType:'window.bootstrap',requestHash:'bootstrap'},
  {windowId:'window',defaultRootDirectory:'/tmp/canvas-workspace',defaultName:'Workspace',now:1000})
const taskId = bootstrap.task!.id, workspaceId = bootstrap.workspace!.id
const base = {windowId:'window',taskId,now:2000}
const messages = {
  newCanvas:runtimeMessages().sessionCanvas.newCanvas,
  duplicateSceneName:runtimeMessages().hierarchy.duplicateSceneName,
  workspacePathInvalid:runtimeMessages().hierarchy.workspacePathInvalid,
  detachedSessionMustReturn:runtimeMessages().sessionCanvas.detachedSessionMustReturn,
  sessionNotInCanvas:runtimeMessages().sessionCanvas.sessionNotInCanvas
}
const q = (v:string) => `'${v.replaceAll("'","''")}'`
const tables = ['workspaces','tasks','execution_contexts','scenes','scene_nodes','sessions','session_mounts',
  'session_canvas_memberships','session_relation_events','session_relations_current','app_windows',
  'window_navigation','window_workspace_focus','window_task_focus','window_scene_focus','window_task_placements',
  'workspace_path_state','domain_events','command_deduplication']
const snapshot = () => Object.fromEntries(tables.map(table => [table,database.all(`SELECT * FROM ${table} ORDER BY rowid`)]))
async function seed(name:string) {
  await database.backupTo(join(root,name+'.sqlite'))
  return readFileSync(join(root,name+'.sqlite')).toString('base64')
}
const creationSeed = await seed('creation')
const steps:any[]=[]
function create(op:'canvas'|'sibling', label:string, input:any, sql:string[]=[] , replayCommand?:any) {
  for(const statement of sql) database.exec(statement)
  const beforeNodes = new Set(database.all<any>('SELECT id FROM scene_nodes').map(r=>r.id))
  const beforeRelations = new Set(database.all<any>('SELECT relation_id FROM session_relations_current').map(r=>r.relation_id))
  const command = replayCommand ?? {commandId:label,commandType:`hierarchy.create-${op}`,requestHash:JSON.stringify(input)}
  let result:any,error:string|undefined,errorCode:string|undefined
  try { result = op==='canvas' ? canvas.createCanvas(command,input) : canvas.createShellSibling(command,input) }
  catch(e:any) { error=e.message; errorCode=e.code }
  const secondary = database.all<any>('SELECT id,kind FROM scene_nodes').find(r=>!beforeNodes.has(r.id)&&r.kind==='split')
  const relation = database.all<any>('SELECT relation_id FROM session_relations_current').find(r=>!beforeRelations.has(r.relation_id))
  const ids = {sceneId:result?.created.scene.id??'',rootNodeId:result?.created.mount.sceneNodeId??'',
    secondaryNodeId:secondary?.id??'',sessionId:result?.created.session.id??'',mountId:result?.created.mount.id??'',relationId:relation?.relation_id??''}
  const committedAt = database.get<any>('SELECT committed_at FROM command_deduplication WHERE command_id=?',command.commandId)?.committed_at??input.now
  steps.push({op,label,input,sql,command,ids,result,error,errorCode,committedAt,snapshot:snapshot()})
  return result
}
const first=create('canvas','canvas-first',base)
create('canvas','canvas-replay',base,[],steps[0].command)
create('canvas','canvas-second',{...base,now:2001})
const named=create('canvas','canvas-named',{...base,title:'  命名画布  ',now:2002,navigation:'preserve'})
create('canvas','canvas-name-conflict',{...base,title:'命名画布',now:2003})
create('canvas','canvas-empty-name',{...base,title:'\uFEFF  ',now:2004})
create('canvas','canvas-missing-task',{...base,taskId:'missing',now:2005})
create('canvas','canvas-invalid-path',{...base,now:2006},[
 `INSERT INTO workspace_path_state VALUES (${q(workspaceId)},'invalid','missing',2006,1)`])
const siblingBase = {windowId:'window',sceneId:first.created.scene.id,sourceSessionId:first.created.session.id,now:2100}
const sibling=create('sibling','sibling-root',siblingBase,[
 `UPDATE workspace_path_state SET status='valid',reason='' WHERE workspace_id=${q(workspaceId)}`,
 `UPDATE sessions SET cwd='/tmp/changed-cwd' WHERE id=${q(first.created.session.id)}`])
create('sibling','sibling-root-replay',siblingBase,[],steps[steps.length-1].command)
const child=create('sibling','sibling-child',{...siblingBase,parentSessionId:first.created.session.id,title:'  子 Shell  ',now:2101})
create('sibling','sibling-inherited-parent',{...siblingBase,sourceSessionId:child.created.session.id,navigation:'preserve',now:2102})
create('sibling','sibling-context',{...siblingBase,executionContextId:'other-context',now:2103},[
 `INSERT INTO execution_contexts(id,workspace_id,kind,cwd,created_at) VALUES ('other-context',${q(workspaceId)},'plain-directory','/tmp/context-cwd',2103)`])
create('sibling','sibling-wrong-source',{...siblingBase,sourceSessionId:bootstrap.session!.id,now:2104})
create('sibling','sibling-empty-title',{...siblingBase,title:'\n ',now:2105})
create('sibling','sibling-missing-context',{...siblingBase,executionContextId:'missing',now:2106})
writeFileSync(process.argv[2]!,JSON.stringify({versions:process.versions,messages,seedBase64:creationSeed,steps},null,2)+'\n')

const navigationSeed=await seed('navigation')
const navigationSteps:any[]=[]
function navigate(op:string,label:string,input:any,sql:string[]=[]) {
  for(const statement of sql) database.exec(statement)
  let result:any,error:string|undefined
  try {
    if(op==='select') result=hierarchy.activateScene(input)
    else if(op==='focus') result=canvas.setFocusedSession(input)
    else if(op==='created') result=readCreatedHierarchyPathForSession(database,input.sessionId)
    else if(op==='list') result=scenes.listSnapshots().map(s=>s.scene).filter(s=>s.taskId===input.taskId&&s.archivedAt===undefined)
  } catch(e:any) {error=e.message}
  navigationSteps.push({op,label,input,sql,result,error,snapshot:snapshot()})
}
navigate('list','list-task',{taskId})
navigate('select','select-original',{windowId:'window',sceneId:bootstrap.scene!.id,now:3000})
navigate('select','select-created',{windowId:'window',sceneId:first.created.scene.id,now:3001})
navigate('focus','focus-first',{windowId:'window',sceneId:first.created.scene.id,sessionId:first.created.session.id,now:3002})
navigate('focus','focus-second-window',{windowId:'other-window',sceneId:first.created.scene.id,sessionId:first.created.session.id,now:3003})
navigate('focus','retain-pending-other-window',{windowId:'window',sceneId:first.created.scene.id,sessionId:sibling.created.session.id,now:3004},[
 `UPDATE session_canvas_memberships SET pending_user_interaction_seq=77 WHERE session_id=${q(first.created.session.id)}`])
navigate('focus','clear-pending-last-window',{windowId:'other-window',sceneId:first.created.scene.id,sessionId:sibling.created.session.id,now:3005})
navigate('select','select-missing-rollback',{windowId:'failed-window',sceneId:'missing',now:3006})
navigate('focus','focus-wrong-canvas',{windowId:'window',sceneId:named.created.scene.id,sessionId:first.created.session.id,now:3007})
navigate('created','created-path',{sessionId:child.created.session.id})
navigate('created','created-missing',{sessionId:'missing'})
navigate('list','list-archive-and-ties',{taskId},[
 `UPDATE scenes SET archived_at=3010 WHERE id=${q(named.created.scene.id)}`,
 `UPDATE scenes SET sort_key='same',created_at=3010 WHERE id<>${q(named.created.scene.id)}`])
writeFileSync(process.argv[3]!,JSON.stringify({versions:process.versions,messages,seedBase64:navigationSeed,steps:navigationSteps},null,2)+'\n')
database.close()
console.log(JSON.stringify({root,creationSteps:steps.length,navigationSteps:navigationSteps.length}))
