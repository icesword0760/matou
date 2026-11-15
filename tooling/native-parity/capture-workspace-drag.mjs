// Passive geometry sampling of the isolated reference. All interaction comes
// from the actual UI; this does not set styles, animation time or app state.
import {readFile,writeFile} from 'node:fs/promises'
import path from 'node:path'
const root=path.resolve(import.meta.dirname,'../..')
const launch=JSON.parse(await readFile(path.join(root,'artifacts/native-parity/reference/launch.json'),'utf8'))
const pages=await(await fetch(`http://127.0.0.1:${launch.debug_port}/json/list`)).json()
const page=pages.find(p=>p.type==='page'&&p.url.includes('MatouParityReference.app/')&&p.url.includes('windowId=main-window-1'))
if(!page)throw Error('Isolated reference page missing')
const socket=new WebSocket(page.webSocketDebuggerUrl)
await new Promise((resolve,reject)=>{socket.addEventListener('open',resolve,{once:true});socket.addEventListener('error',reject,{once:true})})
const reply=new Promise((resolve,reject)=>socket.addEventListener('message',({data})=>{
 const m=JSON.parse(data);if(m.id!==1)return;m.error?reject(Error(JSON.stringify(m.error))):resolve(m.result)
}))
console.log('Sampling isolated reference for 20 seconds; perform real workspace drags now.')
socket.send(JSON.stringify({id:1,method:'Runtime.evaluate',params:{returnByValue:true,awaitPromise:true,expression:`new Promise(resolve=>{
 const start=performance.now(),frames=[],events=[];
 const record=event=>{const group=event.target.closest?.('[data-workspace-id]'); events.push({at:performance.now()-start,type:event.type,x:event.clientX,y:event.clientY,buttons:event.buttons,key:event.key,workspace:group?.dataset.workspaceId});};
 const types=['pointerdown','pointerup','dragstart','dragover','drop','dragend','keydown'];
 for(const type of types)document.addEventListener(type,record,true);
 function sample(){
  const at=performance.now()-start;
  frames.push({at,groups:[...document.querySelectorAll('[data-workspace-id]')].map(group=>{
    const r=group.getBoundingClientRect(),s=getComputedStyle(group),h=group.querySelector('.workspace-group__header').getBoundingClientRect();
    return {id:group.dataset.workspaceId,name:group.querySelector('.workspace-group__name').textContent,className:group.className,
      x:r.x,y:r.y,width:r.width,height:r.height,layoutWidth:group.offsetWidth,layoutHeight:group.offsetHeight,
      opacity:s.opacity,transform:s.transform,header:{x:h.x,y:h.y,width:h.width,height:h.height}};
  })});
  if(at<20000)requestAnimationFrame(sample);else {for(const type of types)document.removeEventListener(type,record,true);resolve({frames,events});}
 }requestAnimationFrame(sample);
})`}}))
try{
 const result=await reply
 if(result.exceptionDetails)throw Error(JSON.stringify(result.exceptionDetails))
 const out=path.join(root,process.argv[2]||'artifacts/native-parity/native/evidence/workspace-drag-reference-frames.json')
 await writeFile(out,JSON.stringify({launch,pageUrl:page.url,interpretation:'Passive renderer frame geometry; no synthesized input or altered styles. Not physical presentation timestamps.',...result.result.value},null,2)+'\n')
 console.log(out)
}finally{socket.close()}
