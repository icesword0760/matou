// Run the frozen filesystem/path service, capturing its actual transactions and inspections.
import {mkdtempSync, mkdirSync, rmdirSync, writeFileSync, readFileSync, chmodSync, unlinkSync} from 'node:fs'
import {tmpdir} from 'node:os'
import {join} from 'node:path'
import {RuntimeDatabase} from '../../apps/runtime/src/storage/database'
import {FOUNDATION_MIGRATIONS} from '../../apps/runtime/src/storage/migrations'
import {MigrationRunner} from '../../apps/runtime/src/storage/migration-runner'
import {DomainTransactionManager} from '../../apps/runtime/src/storage/domain-transaction'
import {HierarchyApplicationService} from '../../apps/runtime/src/hierarchy/hierarchy-application-service'
import {WorkspacePathService} from '../../apps/runtime/src/hierarchy/workspace-path-service'
const root=mkdtempSync(join(tmpdir(),'matou-path-contract-')),workspace=join(root,'workspace')
mkdirSync(workspace)
let db=RuntimeDatabase.open(join(root,'matou.sqlite'))
await new MigrationRunner(db,FOUNDATION_MIGRATIONS).migrate()
const h=new HierarchyApplicationService(db,new DomainTransactionManager(db))
const initial=h.bootstrapWindow({commandId:'bootstrap',commandType:'bootstrap',requestHash:'bootstrap'},{windowId:'window',defaultRootDirectory:workspace,defaultName:'路径对照',now:1000})
const workspaceId=initial.workspace!.id
await db.backupTo(join(root,'seed.sqlite'))
const seedBase64=readFileSync(join(root,'seed.sqlite')).toString('base64')
let paths=new WorkspacePathService(db,new DomainTransactionManager(db))
const steps:any[]=[]
for (const action of ['valid','unchanged','missing','not-directory','no-access','restored','reopen']) {
 if(action==='missing')rmdirSync(workspace)
 if(action==='not-directory')writeFileSync(workspace,'file')
 if(action==='no-access'){unlinkSync(workspace);mkdirSync(workspace);chmodSync(workspace,0)}
 if(action==='restored')chmodSync(workspace,0o700)
 if(action==='reopen'){db.close();db=RuntimeDatabase.open(join(root,'matou.sqlite'));paths=new WorkspacePathService(db,new DomainTransactionManager(db))}
 const state=await paths.validateWorkspace(workspaceId)
 const receipt=db.get<any>('SELECT * FROM command_deduplication ORDER BY rowid DESC LIMIT 1')
 steps.push({action,state,commandId:receipt.command_id,committedAt:receipt.committed_at,snapshot:Object.fromEntries(['workspace_path_state','domain_events','command_deduplication'].map(t=>[t,db.all(`SELECT * FROM ${t} ORDER BY rowid`)]))})
}
writeFileSync(process.argv[2]!,JSON.stringify({workspaceId,seedBase64,steps},null,2)+'\n')
db.close();console.log(JSON.stringify({root,steps:steps.length}))
