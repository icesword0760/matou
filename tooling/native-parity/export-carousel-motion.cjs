// Capture Chromium's interpretation of the frozen carousel CSS in an isolated
// test window. This is a development contract, not a production UI replacement.
const {app,BrowserWindow}=require('electron')
const fs=require('node:fs'),os=require('node:os'),path=require('node:path')
const root=path.resolve(__dirname,'../..')
const profile=fs.mkdtempSync(path.join(os.tmpdir(),'matou-motion-contract-'))
app.setPath('userData',profile)
app.setPath('sessionData',path.join(profile,'session'))
app.commandLine.appendSwitch('disable-background-timer-throttling')
app.commandLine.appendSwitch('disable-renderer-backgrounding')
const css=fs.readFileSync(path.join(root,'apps/desktop/src/renderer/src/session-canvas/session-canvas.css'),'utf8')
app.whenReady().then(async()=>{
 const window=new BrowserWindow({show:false,width:1000,height:300,webPreferences:{backgroundThrottling:false}})
 try {
  await window.loadURL('data:text/html;charset=utf-8,'+encodeURIComponent(`<style>${css}</style><style>body{margin:0}.session-carousel{height:200px;--session-visible-columns:3}.session-card-slot{height:150px}</style><div class="hierarchy-shell" data-theme="light"><div class="session-carousel"><div class="session-card-slot"><div class="session-card"></div></div></div></div>`))
  const result=await window.webContents.executeJavaScript(`(async()=>{
   const slot=document.querySelector('.session-card-slot'),card=document.querySelector('.session-card');
   const frame=()=>new Promise(requestAnimationFrame);
   const read=()=>({width:slot.getBoundingClientRect().width,lift:new DOMMatrix(getComputedStyle(card).transform).m42,shadow:getComputedStyle(card).boxShadow});
   const all=()=>[...slot.getAnimations(),...card.getAnimations()];
   const set=(target)=>{slot.classList.toggle('is-focused',!!target);card.classList.toggle('is-focused',!!target);read()};
   const reset=async(target)=>{slot.style.transition='none';card.style.transition='none';set(target);await frame();await frame();slot.style.transition='';card.style.transition='';read();await frame()};
   await reset(0);const inactive=read();await reset(1);const active=read();
   const cases=[];
   for(const plan of [
    {name:'focus-complete',initial:0,segments:[{target:1,samples:[0,16,45,90,180,190,285,380]}]},
    {name:'blur-complete',initial:1,segments:[{target:0,samples:[0,16,45,90,180,190,285,380]}]},
    {name:'interrupted-and-reversed',initial:0,segments:[{target:1,samples:[0,40,80]},{target:0,samples:[0,20,50]},{target:1,samples:[0,30,90,180,380]}]},
   ]) {
    await reset(plan.initial);const segments=[];
    for(const segment of plan.segments){
     set(segment.target);await frame();const animations=all();
     for(const a of animations){a.pause();a.currentTime=0}read();
     const timing=animations.map(a=>({property:a.transitionProperty,timing:a.effect.getTiming(),keyframes:a.effect.getKeyframes()}));
     const samples=[];
     for(const ms of segment.samples){for(const a of animations)a.currentTime=ms;const r=read();samples.push({ms,...r})}
     segments.push({...segment,timing,samples});
    }
    cases.push({name:plan.name,initial:plan.initial,segments});
   }
   return {inactive,active,cases,viewportWidth:document.querySelector('.session-carousel').clientWidth,userAgent:navigator.userAgent};
  })()`)
  window.webContents.debugger.attach('1.3')
  await window.webContents.debugger.sendCommand('Emulation.setEmulatedMedia',{features:[{name:'prefers-reduced-motion',value:'reduce'}]})
  const reduced=await window.webContents.executeJavaScript(`({width:getComputedStyle(document.querySelector('.session-card-slot')).transitionDuration,lift:getComputedStyle(document.querySelector('.session-card')).transitionDuration})`)
  window.webContents.debugger.detach()
  const output={reduced,source:'apps/desktop/src/renderer/src/session-canvas/session-canvas.css',electron:process.versions.electron,chrome:process.versions.chrome,...result}
  const destination=path.join(root,'apps/native/desktop/tests/fixtures/carousel-motion-reference.json')
  fs.mkdirSync(path.dirname(destination),{recursive:true});fs.writeFileSync(destination,JSON.stringify(output,null,2)+'\n')
  console.log(destination)
 }finally{window.destroy();app.quit()}
}).catch(e=>{console.error(e);app.exit(1)})
