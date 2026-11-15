// Run frozen scene workflows on disposable SQLite; native code never produces expected values.
import {mkdtempSync,readFileSync,writeFileSync} from 'node:fs'
import {tmpdir} from 'node:os'
import {join} from 'node:path'
import {RuntimeDatabase} from '../../apps/runtime/src/storage/database'
import {FOUNDATION_MIGRATIONS} from '../../apps/runtime/src/storage/migrations'
import {MigrationRunner} from '../../apps/runtime/src/storage/migration-runner'
import {DomainTransactionManager} from '../../apps/runtime/src/storage/domain-transaction'
import {HierarchyApplicationService} from '../../apps/runtime/src/hierarchy/hierarchy-application-service'
import {SessionCanvasService} from '../../apps/runtime/src/session-canvas/session-canvas-service'
import {runtimeMessages} from '../../apps/runtime/src/i18n/messages'
const root=mkdtempSync(join(tmpdir(),'matou-tabs-contract-'))
const db=RuntimeDatabase.open(join(root,'matou.sqlite'))
await new MigrationRunner(db,FOUNDATION_MIGRATIONS).migrate()
const tx=new DomainTransactionManager(db), hierarchy=new HierarchyApplicationService(db,tx),canvas=new SessionCanvasService(db,tx)
const cmd=(id:string)=>({commandId:id,commandType:id,requestHash:id})
const initial=hierarchy.bootstrapWindow(cmd('bootstrap'),{windowId:'window',defaultRootDirectory:'/tmp/tab-contract-workspace',defaultName:'Workspace',now:1000})
const taskId=initial.task!.id, workspaceId=initial.workspace!.id, first=initial.scene!.id
const second=canvas.createCanvas(cmd('second'),{windowId:'window',taskId,now:1001}).created
const third=canvas.createCanvas(cmd('third'),{windowId:'window',taskId,title:'固定名称',now:1002}).created
const sibling=canvas.createShellSibling(cmd('sibling'),{windowId:'window',sceneId:second.scene.id,sourceSessionId:second.session.id,now:1003}).created
hierarchy.activateScene({windowId:'other-window',sceneId:second.scene.id,now:1004})
const tables=['workspaces','tasks','execution_contexts','scenes','scene_nodes','sessions','session_mounts','session_canvas_memberships','session_relation_events','session_relations_current','app_windows','window_navigation','window_workspace_focus','window_task_focus','window_scene_focus','window_task_placements','workspace_path_state','domain_events','command_deduplication']
const snapshot=()=>Object.fromEntries(tables.map(table=>[table,db.all(`SELECT * FROM ${table} ORDER BY rowid`)]))
await db.backupTo(join(root,'seed.sqlite'))
const seedBase64=readFileSync(join(root,'seed.sqlite')).toString('base64')
const steps:any[]=[]
function step(op:'rename'|'reorder'|'close',label:string,input:any,sql:string[]=[],command=cmd(label)){
 for(const statement of sql)db.exec(statement)
 let result:any,error:any,errorCode:any
 try {result=op==='rename'?hierarchy.renameScene(command,input):op==='reorder'?hierarchy.reorderScene(command,input):hierarchy.closeScene(command,input)} catch(e:any){error=e.message;errorCode=e.code}
 const committedAt=db.get<any>('SELECT committed_at FROM command_deduplication WHERE command_id=?',command.commandId)?.committed_at??input.now
 steps.push({op,label,input,sql,command,result,error,errorCode,committedAt,snapshot:snapshot()})
}
step('rename','rename-trim',{sceneId:second.scene.id,name:'\uFEFF  标签中文😀  ',now:2000})
step('rename','rename-replay',{sceneId:second.scene.id,name:'\uFEFF  标签中文😀  ',now:2000},[],cmd('rename-trim'))
step('rename','rename-conflict',{sceneId:second.scene.id,name:'固定名称',now:2001})
step('rename','rename-self',{sceneId:third.scene.id,name:'固定名称',now:2002})
step('rename','rename-empty',{sceneId:first,name:'\uFEFF \n',now:2003})
step('rename','rename-missing',{sceneId:'missing',name:'合法',now:2004})
step('reorder','reorder-first',{windowId:'window',sceneId:third.scene.id,beforeSceneId:first,now:2005})
step('reorder','reorder-end',{windowId:'window',sceneId:third.scene.id,now:2006})
step('reorder','reorder-self',{windowId:'window',sceneId:second.scene.id,beforeSceneId:second.scene.id,now:2007})
step('reorder','reorder-invalid-before',{windowId:'window',sceneId:first,beforeSceneId:'missing',now:2008})
step('close','close-stale',{windowId:'window',sceneId:second.scene.id,now:2009},[`UPDATE sessions SET work_status='running' WHERE id='${second.session.id}'`,`UPDATE sessions SET work_status='needs-input' WHERE id='${sibling.session.id}'`])
step('close','close-confirmed',{windowId:'window',sceneId:second.scene.id,confirmedIntent:`close-scene:${second.scene.id}`,now:2010})
step('close','close-replay',{windowId:'window',sceneId:second.scene.id,confirmedIntent:`close-scene:${second.scene.id}`,now:2010},[],cmd('close-confirmed'))
step('rename','rename-archived',{sceneId:second.scene.id,name:'已关闭',now:2011})
step('close','close-third',{windowId:'window',sceneId:third.scene.id,now:2012})
step('close','close-last-not-archived',{windowId:'window',sceneId:first,now:2013})
// Multi-task branch uses an existing task with no scenes; source may keep this empty task focused.
const context=initial.executionContext!.id
const addOther=[`INSERT INTO tasks(id,workspace_id,execution_context_id,title,status,sort_key,created_at,updated_at,version) VALUES ('other-task','${workspaceId}','${context}','第二事项','planned','a00000001',2014,2014,1)`]
step('close','close-last-of-task',{windowId:'window',sceneId:first,now:2014},addOther)
step('reorder','reorder-archived',{windowId:'window',sceneId:first,now:2015})
writeFileSync(process.argv[2]!,JSON.stringify({versions:process.versions,messages:{duplicateSceneName:runtimeMessages().hierarchy.duplicateSceneName},seedBase64,steps},null,2)+'\n')
db.close()
