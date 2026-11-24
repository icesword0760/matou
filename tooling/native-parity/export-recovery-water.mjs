// Execute the frozen component's own effect/math/draw body; only the host APIs are stubbed.
import {createRequire} from 'node:module'
import {readFileSync,writeFileSync,mkdirSync} from 'node:fs'
import {createHash} from 'node:crypto'
import vm from 'node:vm'
const require=createRequire(import.meta.url)
const {transformSync}=require('../../node_modules/.pnpm/esbuild@0.27.7/node_modules/esbuild')
const path='apps/desktop/src/renderer/src/hierarchy/SessionRecoveryWater.tsx'
const original=readFileSync(path,'utf8')
const source=original.replace(/^import .*\n/gm,'').replace("      stepSurface(frameScale, rise, Math.min(1.5, energy * introIntensity))",`      stepSurface(frameScale, rise, Math.min(1.5, energy * introIntensity))
      globalThis.sample = {width:cssWidth,height:cssHeight,dpr,frameScale,rise,alpha,surface,nodes:[...surfaceState.nodes],velocities:[...surfaceState.velocities],signals:[energy,introIntensity,bulkPhase,bulkSignal,innerSignal,counterSignal,slosh,turbulence]}`)
const js=transformSync(source,{loader:'tsx',format:'cjs',jsxFactory:'jsx',logLevel:'silent'}).code
const scenarios=[
 {name:'normal',reduced:false,dpr:2,frames:[0,16.667,144,288,1200,2400,7199,7200,7216.667].map(time=>({time,width:560,height:600}))},
 {name:'resize',reduced:false,dpr:1.25,frames:[{time:16.667,width:336.4,height:280.4},{time:33.334,width:336.6,height:280.6},{time:50,width:900,height:500},{time:75,width:900,height:500}]},
 {name:'reduced',reduced:true,dpr:3,frames:[{time:16.667,width:560,height:600},{time:4000,width:561,height:601},{time:7300,width:561,height:601}]}
]
// The K gate supplies extra cases and an isolated destination. The default
// exporter still generates the original fixture at its original path.
const gateCases = process.argv[2] === '--gate-cases'
if (gateCases) scenarios.splice(0, scenarios.length, ...JSON.parse(readFileSync(process.argv[3], 'utf8')).clock)
const cases=[]
for(const scenario of scenarios){
 let now=0,bounds=scenario.frames[0],callback,resizeCallback,ops=[],stack=[]
 let visibilityCallback
 const gradients=[]; let state={alpha:1,strokeStyle:null,lineWidth:1,shadowBlur:0,shadowColor:null,fillStyle:null},path=[]
 const ctx={
  setTransform(){},clearRect(){ops=[];gradients.length=0},
  beginPath(){path=[]},moveTo(...p){path.push(['M',...p])},lineTo(...p){path.push(['L',...p])},bezierCurveTo(...p){path.push(['C',...p])},arc(...p){path.push(['A',...p])},closePath(){path.push(['Z'])},
  save(){stack.push({...state})},restore(){state=stack.pop()},clip(){ops.push({op:'clip',path:structuredClone(path)})},
  createLinearGradient(...points){const g={points,stops:[]};gradients.push(g);return{g,addColorStop(offset,color){g.stops.push([offset,color])}}},
  fill(){ops.push({op:'fill',path:structuredClone(path),style:state.fillStyle?.g??state.fillStyle,alpha:state.alpha})},
  fillRect(...rect){ops.push({op:'rect',rect,style:state.fillStyle?.g??state.fillStyle,alpha:state.alpha})},
  stroke(){ops.push({op:'stroke',path:structuredClone(path),style:state.strokeStyle,width:state.lineWidth,blur:state.shadowBlur,shadow:state.shadowColor,alpha:state.alpha})}
 }
 for(const key of ['strokeStyle','lineWidth','shadowBlur','shadowColor','fillStyle']) Object.defineProperty(ctx,key,{get:()=>state[key],set:v=>state[key]=v})
 Object.defineProperty(ctx,'globalAlpha',{get:()=>state.alpha,set:v=>state.alpha=v})
 const canvas={width:0,height:0,getBoundingClientRect:()=>bounds,getContext:()=>ctx}
 const box={module:{exports:{}},exports:{},jsx:()=>null,useRef:()=>({current:canvas}),useEffect:fn=>fn(),useMessages:()=>({hierarchyTerminal:{recoveryWater:{status:{recovery:''},detail:{recovery:''},label:()=>''}}}),
  performance:{now:()=>now},CanvasRenderingContext2D:function(){},ResizeObserver:class{constructor(fn){resizeCallback=fn}observe(){}disconnect(){}},
  document:{hidden:false,addEventListener(_name,fn){visibilityCallback=fn},removeEventListener(){}},window:{devicePixelRatio:scenario.dpr,matchMedia:()=>({matches:scenario.reduced}),requestAnimationFrame:fn=>{callback=fn;return 1},cancelAnimationFrame(){callback=undefined}}, console}
 box.exports=box.module.exports
 vm.createContext(box);vm.runInContext(js,box);box.module.exports.SessionRecoveryWater({sessionTitle:'fixture'})
 const frames=[]
 for(const f of scenario.frames){
  now=f.time;bounds=f
  if (gateCases) {
   if (f.dpr !== undefined) box.window.devicePixelRatio=f.dpr
   if (f.event === 'hide' || f.event === 'show') {
    box.document.hidden=f.event === 'hide'
    visibilityCallback()
   }
   if (f.event === 'resize') resizeCallback()
   const pendingBefore=Boolean(callback)
   box.sample=undefined
   if (f.event !== 'hide' && f.event !== 'show' && callback) {
    const run=callback;callback=undefined;run()
   }
   const sample=box.sample === undefined ? null : {...JSON.parse(JSON.stringify(box.sample)),ops:JSON.parse(JSON.stringify(ops))}
   frames.push({time:f.time,event:f.event,pendingBefore,pendingAfter:Boolean(callback),backing:[canvas.width,canvas.height],sample})
   continue
  }
  if(!callback)resizeCallback();const run=callback;callback=undefined;run();frames.push({time:f.time,...JSON.parse(JSON.stringify(box.sample)),ops:JSON.parse(JSON.stringify(ops))})
 }
 cases.push({name:scenario.name,reduced:scenario.reduced,frames})
}
if (gateCases) {
 writeFileSync(process.argv[4], JSON.stringify({source:path,sourceSha256:createHash('sha256').update(original).digest('hex'),cases})+'\n')
 console.log(`Frozen clock and Canvas operations: ${cases.length} cases`)
 process.exit(0)
}
const target='apps/native/desktop/fixtures/recovery-water.json';mkdirSync('apps/native/desktop/fixtures',{recursive:true})
writeFileSync(target,JSON.stringify({source:path,sourceSha256:createHash('sha256').update(original).digest('hex'),precision:'absolute 1e-8 numerical; not a pixel threshold',cases},null,2)+'\n')
console.log(`Executed original component: ${cases.length} scenarios, ${cases.reduce((n,c)=>n+c.frames.length,0)} frames -> ${target}`)
