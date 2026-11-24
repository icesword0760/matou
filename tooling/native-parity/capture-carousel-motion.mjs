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
console.log('Sampling isolated reference for 25 seconds; perform real card switches now.')
socket.send(JSON.stringify({id:1,method:'Runtime.evaluate',params:{returnByValue:true,awaitPromise:true,expression:`new Promise(resolve=>{
 const start=performance.now(),frames=[],events=[];
 const record=event=>{const card=event.target.closest?.('[data-session-card]'); events.push({at:performance.now()-start,type:event.type,x:event.clientX,y:event.clientY,buttons:event.buttons,card:card?.dataset.sessionCard,focused:card?.getAttribute('aria-current')});};
 const types=['pointerover','pointerout','pointerdown','pointerup'];
 for(const type of types)document.addEventListener(type,record,true);
 function sample(){
  const carousel=[...document.querySelectorAll('.session-carousel')].find(el=>el.getBoundingClientRect().width>0), at=performance.now()-start;
  if(carousel)frames.push({at,viewport:carousel.clientWidth,scroll:carousel.scrollLeft,
   cards:[...carousel.querySelectorAll('.session-card-slot')].map(slot=>{
    const card=slot.querySelector('.session-card'),r=card.getBoundingClientRect();
    return {session:slot.dataset.sessionId,focused:slot.classList.contains('is-focused'),expanded:slot.classList.contains('is-expanded'),
     x:r.x,y:r.y,width:r.width,height:r.height};})});
  if(at<25000)requestAnimationFrame(sample);else {for(const type of types)document.removeEventListener(type,record,true);resolve({frames,events});}
 }requestAnimationFrame(sample);
})`}}))
try{
 const result=await reply
 if(result.exceptionDetails)throw Error(JSON.stringify(result.exceptionDetails))
 const out=path.join(root,process.argv[2]||'artifacts/native-parity/native/evidence/motion-reference-frames.json')
 await writeFile(out,JSON.stringify({launch,pageUrl:page.url,interpretation:'Passive renderer frame geometry; no synthesized input or altered styles. Not physical presentation timestamps.',...result.result.value},null,2)+'\n')
 console.log(out)
}finally{socket.close()}
