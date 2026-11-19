// Project-local OpenCLI 1.8.8 diagnostic copy. Never writes to the source package.
import {cpSync, existsSync, lstatSync, mkdirSync, readFileSync, readdirSync, realpathSync, statSync, symlinkSync, writeFileSync} from 'node:fs'
import {createHash} from 'node:crypto'
import {dirname, join, relative, resolve} from 'node:path'
import {fileURLToPath} from 'node:url'

const root=resolve(dirname(fileURLToPath(import.meta.url)),'../..')
const source=realpathSync('/Users/icesword/.opencli/node_modules/@jackwener/opencli')
const output=join(root,'artifacts/native-parity/tools/opencli-shadow-ref-1.8.8')
const dest=join(output,'package')
const target='dist/src/browser/target-resolver.js'
const expected='5f35210690782eec2a738f6a61a916b355c249fcfb797409a31f8f676234bc02'
const sha=bytes=>createHash('sha256').update(bytes).digest('hex')
const manifest=dir=>{
  const rows={}
  function visit(path){for(const item of readdirSync(path,{withFileTypes:true})){
    if(item.name==='node_modules')continue
    const full=join(path,item.name)
    if(item.isSymbolicLink())throw Error('Unexpected package symlink: '+full)
    if(item.isDirectory())visit(full)
    else if(item.isFile())rows[relative(dir,full)]=sha(readFileSync(full))
  }}
  visit(dir);return rows
}
const original=readFileSync(join(source,target),'utf8')
if(JSON.parse(readFileSync(join(source,'package.json'),'utf8')).version!=='1.8.8'||sha(original)!==expected)throw Error('Unknown installed OpenCLI; refusing preparation')
const before=`        const fp = identity[ref];
        let el = document.querySelector('[data-opencli-ref="' + ref + '"]');
        if (!el) el = document.querySelector('[data-ref="' + ref + '"]');`
const after=`        // Snapshot refs include open-shadow descendants. Search the same roots,
        // deduplicate both ref attributes, and refuse any ambiguous annotation.
        function findRefCandidates() {
          const found = new Set();
          const roots = [document];
          for (let i = 0; i < roots.length; i++) {
            const scope = roots[i];
            for (const node of scope.querySelectorAll('[data-opencli-ref="' + ref + '"], [data-ref="' + ref + '"]')) found.add(node);
            for (const node of scope.querySelectorAll('*')) {
              if (node.shadowRoot) roots.push(node.shadowRoot);
            }
          }
          return Array.from(found);
        }
        const refCandidates = findRefCandidates();
        if (refCandidates.length > 1) {
          return {
            ok: false,
            code: 'ref_ambiguous',
            message: 'ref=' + ref + ' matched ' + refCandidates.length + ' elements',
            hint: 'Ref annotations are ambiguous. Re-run browser state before acting.',
            matches_n: refCandidates.length,
          };
        }
        const fp = identity[ref];
        let el = refCandidates[0];`
if(original.split(before).length!==2)throw Error('Expected one exact numeric lookup block')
const patched=original.replace(before,after)
// The original startup rewrites a global compatibility symlink to PACKAGE_ROOT.
// Root-approved isolation guard: preserve that global link while using this copy.
const startup='dist/src/main.js'
const startupOriginal=readFileSync(join(source,startup),'utf8')
if(sha(startupOriginal)!=='246004200e381e5aecdfaef13e904953c0d18e0600ca66d02b956c4b1820ec02')throw Error('Unknown installed startup; refusing isolation guard')
const startupBefore='        ensureUserCliCompatShims(),'
if(startupOriginal.split(startupBefore).length!==2)throw Error('Expected one startup shim call')
const startupPatched=startupOriginal.replace(startupBefore,'        Promise.resolve(), // Project-local diagnostics: do not rewrite global compatibility shims.')
const sourceManifest=manifest(source)
if(existsSync(dest)){
  const copy=manifest(dest)
  const priorCopy={...sourceManifest,[target]:sha(patched)}
  const expectedCopy={...priorCopy,[startup]:sha(startupPatched)}
  if(JSON.stringify(copy)===JSON.stringify(priorCopy))writeFileSync(join(dest,startup),startupPatched)
  else if(JSON.stringify(copy)!==JSON.stringify(expectedCopy))throw Error('Existing isolated copy has unknown files or edits; refusing overwrite')
}else{
  mkdirSync(output,{recursive:true})
  cpSync(source,dest,{recursive:true,filter:p=>relative(source,p).split('/')[0]!=='node_modules'})
  if(statSync(join(source,target)).ino===statSync(join(dest,target)).ino)throw Error('Copy unexpectedly shares source inode')
  writeFileSync(join(dest,target),patched)
  writeFileSync(join(dest,startup),startupPatched)
}
const deps=resolve(source,'../..')
const link=join(dest,'node_modules')
if(existsSync(link)){if(!lstatSync(link).isSymbolicLink()||realpathSync(link)!==deps)throw Error('Unexpected dependency path')}
else symlinkSync(deps,link,'dir')
const globalAfter=manifest(source)
if(JSON.stringify(globalAfter)!==JSON.stringify(sourceManifest))throw Error('Source changed during preparation')
const copyManifest=manifest(dest)
const changes=Object.keys(sourceManifest).filter(f=>sourceManifest[f]!==copyManifest[f])
if(changes.length!==2||!changes.includes(target)||!changes.includes(startup)||Object.keys(copyManifest).length!==Object.keys(sourceManifest).length)throw Error('Unexpected copy differences')
writeFileSync(join(output,'target-resolver.patch'),'--- a/'+target+'\n+++ b/'+target+'\n@@ -151,3 +151,'+after.split('\n').length+' @@\n'+before.split('\n').map(x=>'-'+x).join('\n')+'\n'+after.split('\n').map(x=>'+'+x).join('\n')+'\n')
writeFileSync(join(output,'startup-isolation.patch'),'--- a/'+startup+'\n+++ b/'+startup+'\n@@ -118,1 +118,1 @@\n-'+startupBefore+'\n+'+startupPatched.split('\n').find(x=>x.includes('Project-local diagnostics'))+'\n')
const record={version:'1.8.8',sourceAlias:'/Users/icesword/.opencli/node_modules/@jackwener/opencli',source,dest,dependencyReadOnlyReuse:deps,sourceManifest,copyManifest,changedFiles:changes,targetSourceSha256:expected,targetCopySha256:sha(patched),startupSourceSha256:sha(startupOriginal),startupCopySha256:sha(startupPatched),sourceInode:statSync(join(source,target)).ino,copyInode:statSync(join(dest,target)).ino,scriptSha256:sha(readFileSync(fileURLToPath(import.meta.url)))}
writeFileSync(join(output,'preparation.json'),JSON.stringify(record,null,2)+'\n')
console.log(JSON.stringify({dest,version:record.version,files:Object.keys(sourceManifest).length,changedFiles:changes,sourceSha256:expected,copySha256:record.targetCopySha256,sourceUnchanged:true}))
