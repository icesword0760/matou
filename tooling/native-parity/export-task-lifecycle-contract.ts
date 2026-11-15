// Frozen workflows generate native expectations against disposable SQLite.
import {mkdtempSync,readFileSync,writeFileSync} from 'node:fs'
import {tmpdir} from 'node:os'
import {join} from 'node:path'
import {RuntimeDatabase} from '../../apps/runtime/src/storage/database'
import {FOUNDATION_MIGRATIONS} from '../../apps/runtime/src/storage/migrations'
import {MigrationRunner} from '../../apps/runtime/src/storage/migration-runner'
import {DomainTransactionManager} from '../../apps/runtime/src/storage/domain-transaction'
import {HierarchyApplicationService} from '../../apps/runtime/src/hierarchy/hierarchy-application-service'
import {runtimeMessages} from '../../apps/runtime/src/i18n/messages'
const root=mkdtempSync(join(tmpdir(),'matou-tasks-contract-'))
const db=RuntimeDatabase.open(join(root,'matou.sqlite'))
await new MigrationRunner(db,FOUNDATION_MIGRATIONS).migrate()
const service=new HierarchyApplicationService(db,new DomainTransactionManager(db))
const cmd=(id:string)=>({commandId:id,commandType:id,requestHash:id})
const initial=service.bootstrapWindow(cmd('bootstrap'),{windowId:'window',defaultRootDirectory:'/tmp/task-contract-workspace',defaultName:'Workspace',now:1000})
const workspaceId=initial.workspace!.id, first=initial.task!.id
service.activateTask({windowId:'other-window',taskId:first,now:1001})
const tables=['workspaces','tasks','execution_contexts','scenes','scene_nodes','sessions','session_mounts','session_canvas_memberships','session_relation_events','session_relations_current','app_windows','window_navigation','window_workspace_focus','window_task_focus','window_scene_focus','window_task_placements','workspace_path_state','task_status_entries','task_progress','task_logs','domain_events','command_deduplication']
const snapshot=()=>Object.fromEntries(tables.map(table=>[table,db.all(`SELECT * FROM ${table} ORDER BY rowid`)]))
await db.backupTo(join(root,'seed.sqlite'))
const seedBase64=readFileSync(join(root,'seed.sqlite')).toString('base64'), steps:any[]=[]
function step(op:string,label:string,input:any,sql:string[]=[],command=cmd(label)){
 for(const statement of sql)db.exec(statement)
 const existing=new Set(db.all<any>('SELECT id FROM tasks').map(x=>x.id))
 let result:any,error:any,errorCode:any
 try {result=op==='activate'?service.activateTask(input):(service as any)[op](command,input)}catch(e:any){error=e.message;errorCode=e.code}
 const created=db.get<any>('SELECT * FROM tasks ORDER BY rowid DESC LIMIT 1')
 let ids:any
 if(created&&!existing.has(created.id)){
  const scene=db.get<any>('SELECT * FROM scenes WHERE task_id=?',created.id), session=db.get<any>('SELECT * FROM sessions WHERE task_id=?',created.id), mount=db.get<any>('SELECT * FROM session_mounts WHERE session_id=?',session.id)
  ids={workspaceId:created.workspace_id,executionContextId:created.execution_context_id,taskId:created.id,sceneId:scene.id,rootNodeId:scene.root_node_id,sessionId:session.id,mountId:mount.id}
 }
 const committedAt=db.get<any>('SELECT committed_at FROM command_deduplication WHERE command_id=?',command.commandId)?.committed_at??input.now
 steps.push({op,label,input,sql,command,ids,result,error,errorCode,committedAt,snapshot:snapshot()})
 return result
}
const create=(label:string,extra:any={})=>step('createTask',label,{windowId:'window',workspaceId,now:2000+steps.length,...extra})
const second=create('create-default').created
const third=create('create-numbered').created
const fourth=create('create-trim-preserve',{title:'\uFEFF  中文事项😀  ',navigation:'preserve'}).created
create('create-duplicate',{title:second.task.title})
create('create-empty',{title:' \uFEFF '})
create('create-missing',{workspaceId:'missing'})
step('createTask','create-replay',steps[0].input,[],cmd('create-default'))
step('renameTask','rename-trim',{taskId:second.task.id,title:'\uFEFF  改名  ',now:2010})
step('renameTask','rename-duplicate',{taskId:third.task.id,title:'改名',now:2011})
step('renameTask','rename-self',{taskId:second.task.id,title:'改名',now:2012})
step('renameTask','rename-empty',{taskId:first,title:'\uFEFF ',now:2013})
step('renameTask','rename-missing',{taskId:'missing',title:'合法',now:2014})
step('setTaskPinned','pin-second',{taskId:second.task.id,pinned:true,now:2015})
step('setTaskPinned','pin-third',{taskId:third.task.id,pinned:true,now:2016})
step('reorderPinnedTask','pin-third-first',{workspaceId,taskId:third.task.id,beforeTaskId:second.task.id,now:2017})
step('reorderPinnedTask','pin-self',{workspaceId,taskId:third.task.id,beforeTaskId:third.task.id,now:2018})
step('reorderPinnedTask','pin-invalid',{workspaceId,taskId:first,now:2019})
step('setTaskPinned','unpin-second',{taskId:second.task.id,pinned:false,now:2020})
step('reorderTask','reorder-first',{windowId:'window',workspaceId,taskId:fourth.task.id,beforeTaskId:first,now:2021})
step('reorderTask','reorder-end',{windowId:'window',workspaceId,taskId:fourth.task.id,now:2022})
step('reorderTask','reorder-invalid',{windowId:'window',workspaceId,taskId:first,beforeTaskId:'missing',now:2023})
step('activate','activate-second',{windowId:'window',taskId:second.task.id,now:2024})
step('recordSessionInteraction','interaction-second',{sessionId:second.session.id,now:2025})
step('activate','activate-invalid',{windowId:'failed-window',taskId:'missing',now:2026})
step('deleteTask','delete-stale',{windowId:'window',taskId:second.task.id,confirmedIntent:'delete-task:stale',now:2027})
const remove=(label:string,taskId:string)=>step('deleteTask',label,{windowId:'window',taskId,confirmedIntent:`delete-task:${taskId}`,now:2030+steps.length})
remove('delete-active',second.task.id)
step('deleteTask','delete-replay',steps.at(-1).input,[],cmd('delete-active'))
step('renameTask','rename-archived',{taskId:second.task.id,title:'归档',now:2060})
remove('delete-background',fourth.task.id)
remove('delete-other-window-active',first)
const replacement=remove('delete-last-replacement',third.task.id)
step('createTask','create-invalid-path',{windowId:'window',workspaceId,now:2070},[`INSERT INTO workspace_path_state(workspace_id,status,reason,checked_at,validation_generation) VALUES ('${workspaceId}','invalid','missing',2070,1)`])
remove('delete-last-invalid-path',replacement.task.id)
writeFileSync(process.argv[2]!,JSON.stringify({versions:process.versions,messages:{newTask:runtimeMessages().hierarchy.newTask,defaultTask:runtimeMessages().hierarchy.defaultTask,workspacePathInvalid:runtimeMessages().hierarchy.workspacePathInvalid},seedBase64,steps},null,2)+'\n')
db.close()
console.log(JSON.stringify({root,steps:steps.length}))
