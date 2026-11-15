// Execute the frozen workspace service against disposable SQLite, without native-derived expectations.
import {mkdtempSync,readFileSync,writeFileSync} from 'node:fs'
import {tmpdir} from 'node:os'
import {join} from 'node:path'
import {RuntimeDatabase} from '../../apps/runtime/src/storage/database'
import {FOUNDATION_MIGRATIONS} from '../../apps/runtime/src/storage/migrations'
import {MigrationRunner} from '../../apps/runtime/src/storage/migration-runner'
import {DomainTransactionManager} from '../../apps/runtime/src/storage/domain-transaction'
import {HierarchyApplicationService} from '../../apps/runtime/src/hierarchy/hierarchy-application-service'
import {runtimeMessages} from '../../apps/runtime/src/i18n/messages'
const root=mkdtempSync(join(tmpdir(),'matou-workspaces-contract-'))
const db=RuntimeDatabase.open(join(root,'matou.sqlite'))
await new MigrationRunner(db,FOUNDATION_MIGRATIONS).migrate()
const service=new HierarchyApplicationService(db,new DomainTransactionManager(db))
const cmd=(id:string)=>({commandId:id,commandType:id,requestHash:id})
const initial=service.bootstrapWindow(cmd('bootstrap'),{windowId:'window',defaultRootDirectory:'/tmp/workspace-contract/default',defaultName:'默认目录',now:1000})
const defaultId=initial.workspace!.id
const tables=['workspaces','tasks','execution_contexts','scenes','scene_nodes','sessions','session_mounts','session_canvas_memberships','session_relation_events','session_relations_current','app_windows','window_navigation','window_workspace_focus','window_task_focus','window_scene_focus','window_task_placements','workspace_path_state','task_status_entries','task_progress','task_logs','bootstrap_state','domain_events','command_deduplication']
const snapshot=()=>Object.fromEntries(tables.map(table=>[table,db.all(`SELECT * FROM ${table} ORDER BY rowid`)]))
await db.backupTo(join(root,'seed.sqlite'))
const seedBase64=readFileSync(join(root,'seed.sqlite')).toString('base64'),steps:any[]=[]
function step(op:string,label:string,input:any,sql:string[]=[],command=cmd(label)) {
 for(const statement of sql)db.exec(statement)
 const existing=new Set(db.all<any>('SELECT id FROM tasks').map(x=>x.id))
 let result:any,error:any,errorCode:any
 try{result=op==='activateWorkspace'?service.activateWorkspace(input):(service as any)[op](command,input)}catch(e:any){error=e.message;errorCode=e.code}
 const task=db.get<any>('SELECT * FROM tasks ORDER BY rowid DESC LIMIT 1');let ids:any
 if(task&&!existing.has(task.id)) {
  const scene=db.get<any>('SELECT * FROM scenes WHERE task_id=?',task.id),session=db.get<any>('SELECT * FROM sessions WHERE task_id=?',task.id),mount=db.get<any>('SELECT * FROM session_mounts WHERE session_id=?',session.id)
  ids={workspaceId:task.workspace_id,executionContextId:task.execution_context_id,taskId:task.id,sceneId:scene.id,rootNodeId:scene.root_node_id,sessionId:session.id,mountId:mount.id}
 }
 const committedAt=db.get<any>('SELECT committed_at FROM command_deduplication WHERE command_id=?',command.commandId)?.committed_at??input.now
 steps.push({op,label,input,sql,command,ids,result,error,errorCode,committedAt,snapshot:snapshot()});return result
}
const create=(label:string,name:string,path:string,extra:any={})=>step('createWorkspace',label,{windowId:'window',name,rootDirectory:path,now:2000+steps.length,...extra})
const a=create('create-a-trim','\uFEFF 项目甲 ','/tmp/workspace-contract/项目甲').created
const b=create('create-b-preserve','项目乙','/tmp/workspace-contract/项目乙',{navigation:'preserve'}).created
const c=create('create-c','项目丙','/tmp/workspace-contract/项目丙').created
create('existing-root-no-rename','不采用的新名称','/tmp/workspace-contract/项目甲')
create('existing-root-empty-name-ignored','  ','/tmp/workspace-contract/项目甲')
create('new-root-empty-name',' \uFEFF ','/tmp/workspace-contract/empty')
step('createWorkspace','create-replay',steps[0].input,[],cmd('create-a-trim'))
step('activateWorkspace','activate-other-window',{windowId:'other-window',workspaceId:a.workspace.id,now:2010})
step('activateWorkspace','activate-missing',{windowId:'failed-window',workspaceId:'missing',now:2011})
step('renameWorkspace','rename-rejected',{workspaceId:a.workspace.id,name:'改名',now:2012})
step('renameWorkspace','rename-missing',{workspaceId:'missing',name:'改名',now:2013})
const at=step('createTask','second-task-for-relink',{windowId:'window',workspaceId:a.workspace.id,title:'第二事项',now:2014}).created
step('relinkWorkspace','relink-default-rejected',{workspaceId:defaultId,rootDirectory:'/tmp/workspace-contract/new-default',now:2015})
step('relinkWorkspace','relink-taken',{workspaceId:a.workspace.id,rootDirectory:b.workspace.rootDirectory,now:2016})
step('relinkWorkspace','relink-a',{workspaceId:a.workspace.id,rootDirectory:'/tmp/workspace-contract/恢复甲',now:2017},[
 `UPDATE scenes SET title_pinned=1,name='保留画布名' WHERE id='${a.scene.id}'`,
 `INSERT INTO workspace_path_state(workspace_id,status,reason,checked_at,validation_generation) VALUES ('${a.workspace.id}','invalid','missing',2016,1)`
])
step('relinkWorkspace','relink-replay',steps.at(-1).input,[],cmd('relink-a'))
step('setWorkspacePinned','pin-a',{workspaceId:a.workspace.id,pinned:true,now:2018})
step('setWorkspacePinned','pin-b',{workspaceId:b.workspace.id,pinned:true,now:2019})
step('reorderPinnedWorkspace','pin-b-first',{workspaceId:b.workspace.id,beforeWorkspaceId:defaultId,now:2020})
step('reorderPinnedWorkspace','pin-self',{workspaceId:b.workspace.id,beforeWorkspaceId:b.workspace.id,now:2021})
step('reorderPinnedWorkspace','pin-invalid',{workspaceId:c.workspace.id,now:2022})
step('setWorkspacePinned','unpin-a',{workspaceId:a.workspace.id,pinned:false,now:2023})
step('removeWorkspace','remove-stale',{windowId:'window',workspaceId:c.workspace.id,confirmedIntent:'remove-workspace:stale',now:2024})
const remove=(label:string,workspaceId:string)=>step('removeWorkspace',label,{windowId:'window',workspaceId,confirmedIntent:`remove-workspace:${workspaceId}`,now:2030+steps.length})
remove('remove-default-rejected',defaultId)
remove('remove-missing','missing')
// Removing a background workspace also moves the initiating window to the fallback.
remove('remove-background-c',c.workspace.id)
remove('remove-a-other-window-active',a.workspace.id)
step('removeWorkspace','remove-replay',steps.at(-1).input,[],cmd('remove-a-other-window-active'))
step('relinkWorkspace','relink-archived',{workspaceId:a.workspace.id,rootDirectory:'/tmp/workspace-contract/归档',now:2070})
create('recreate-archived-root','重新打开甲','/tmp/workspace-contract/恢复甲')
remove('remove-pinned-b',b.workspace.id)
const m=runtimeMessages().hierarchy
writeFileSync(process.argv[2]!,JSON.stringify({versions:process.versions,messages:{defaultTask:m.defaultTask,newTask:m.newTask,workspacePathInvalid:m.workspacePathInvalid,workspaceNameFollowsDirectory:m.workspaceNameFollowsDirectory,defaultWorkspaceIsHomeDirectory:m.defaultWorkspaceIsHomeDirectory,workspaceDirectoryTaken:m.workspaceDirectoryTaken,defaultWorkspaceKept:m.defaultWorkspaceKept},seedBase64,steps},null,2)+'\n')
db.close();console.log(JSON.stringify({root,steps:steps.length}))
