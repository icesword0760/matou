// Compile the unchanged frozen component into a local Console-only pixel harness.
import {createRequire} from 'node:module'
import {readFileSync,writeFileSync,mkdirSync} from 'node:fs'
import {createHash} from 'node:crypto'
const require=createRequire(import.meta.url)
const {transformSync}=require('../../node_modules/.pnpm/esbuild@0.27.7/node_modules/esbuild')
const sourcePath='apps/desktop/src/renderer/src/hierarchy/SessionRecoveryWater.tsx'
const original=readFileSync(sourcePath,'utf8')
const sourceSha256=createHash('sha256').update(original).digest('hex')
const fixture=JSON.parse(readFileSync('apps/native/desktop/fixtures/recovery-water.json','utf8'))
if(fixture.sourceSha256!==sourceSha256)throw Error('Source differs from the previously source-executed ops fixture')
const js=transformSync(original.replace(/^import .*\n/gm,''),{loader:'tsx',format:'cjs',jsxFactory:'jsx',minify:true,logLevel:'silent'}).code
const runId='matou-water-px25-'+Date.now()
const parameters={width:560,height:600,dpr:2,reduced:false,targets:[144,288,2400],sequence:[0,16.667,144,288,1200,2400],repeats:2,shadowDprs:[1,2]}
const body=`(() => {
 const target={userAgent:navigator.userAgent,location:location.href,electron:globalThis.process?.versions?.electron??null};
 if(!target.userAgent.includes('Chrome/150.0.7871.224')||!target.userAgent.includes('Electron/43.4.1'))throw Error('Wrong target renderer; pixel batch refused: '+target.userAgent);
 const parameters=${JSON.stringify(parameters)};
 const samples=[];
 const capture=(canvas,ctx,details)=>({...details,width:canvas.width,height:canvas.height,canvasDetached:!canvas.isConnected,contextNative:ctx instanceof CanvasRenderingContext2D,contextAttributes:ctx.getContextAttributes?.()??null,png:canvas.toDataURL('image/png')});
 const fluid=(at,repeat)=>{
   const actual=document.createElement('canvas');let context=null,now=0,callback=null,resize=null,cleanup=null;
   const canvas={get width(){return actual.width},set width(value){actual.width=value},get height(){return actual.height},set height(value){actual.height=value},getBoundingClientRect:()=>({width:parameters.width,height:parameters.height}),getContext:(...args)=>(context=actual.getContext(...args))};
   const localWindow={devicePixelRatio:parameters.dpr,matchMedia:()=>({matches:false}),requestAnimationFrame:fn=>{callback=fn;return 1},cancelAnimationFrame:()=>{callback=null}};
   const localDocument={hidden:false,addEventListener(){},removeEventListener(){}};
   const module={exports:{}};
   const factory=function(module,exports,jsx,useRef,useEffect,useMessages,performance,CanvasRenderingContext2D,ResizeObserver,document,window){${js}\n};
   factory(module,module.exports,()=>null,()=>({current:canvas}),fn=>{cleanup=fn()},()=>({hierarchyTerminal:{recoveryWater:{status:{recovery:''},detail:{recovery:''},label:()=>''}}}),{now:()=>now},CanvasRenderingContext2D,class{constructor(fn){resize=fn}observe(){}disconnect(){}},localDocument,localWindow);
   module.exports.SessionRecoveryWater({sessionTitle:'pixel-contract'});
   const executed=[];
   for(const time of parameters.sequence){if(time>at)break;now=time;if(!callback)resize();const draw=callback;callback=null;if(!draw)throw Error('Missing source RAF');draw();executed.push(time)}
   const result=capture(actual,context,{name:'fluid-'+at,repeat,time:at,cssWidth:parameters.width,cssHeight:parameters.height,dpr:parameters.dpr,sequence:executed,sourceComponentExecuted:true});
   cleanup?.();return result;
 };
 const shadow=(dpr,repeat)=>{
   const canvas=document.createElement('canvas');canvas.width=128*dpr;canvas.height=96*dpr;
   const ctx=canvas.getContext('2d');
   ctx.fillStyle='rgba(128,32,64,'+(128/255)+')';ctx.fillRect(4*dpr,3*dpr,6*dpr,5*dpr);
   ctx.fillStyle='rgba(64,32,128,'+(128/255)+')';ctx.fillRect(4*dpr,80*dpr,6*dpr,5*dpr);
   ctx.shadowColor='rgba(204,51,102,1)';ctx.shadowBlur=5;ctx.shadowOffsetX=32;ctx.shadowOffsetY=0;
   ctx.fillStyle='rgba(51,102,204,1)';ctx.fillRect(20*dpr,20*dpr,1,50*dpr);
   return capture(canvas,ctx,{name:'shadow-dpr'+dpr,repeat,dpr,cssWidth:128,cssHeight:96,sourceComponentExecuted:false,control:'same existing CG diagnostic; explicit device coordinates; shadowBlur5 offsetX32 offsetY0'});
 };
 for(let repeat=1;repeat<=parameters.repeats;repeat++){
   for(const time of parameters.targets)samples.push(fluid(time,repeat));
   for(const dpr of parameters.shadowDprs)samples.push(shadow(dpr,repeat));
 }
 return {runId:${JSON.stringify(runId)},target,source:${JSON.stringify(sourcePath)},sourceSha256:${JSON.stringify(sourceSha256)},compiledSha256:${JSON.stringify(createHash('sha256').update(js).digest('hex'))},parameters,readbacks:'one toDataURL per independent canvas; no getImageData',productDomModified:false,globalOrPrototypeModified:false,samples};
})()`
const payload='copy(JSON.stringify('+body+'))'
const root=process.argv[2]??'artifacts/native-parity/native/evidence/recovery-water-pixels';mkdirSync(root,{recursive:true})
writeFileSync(root+'/console-payload.js',payload+'\n')
writeFileSync(root+'/payload-metadata.json',JSON.stringify({runId,source:sourcePath,sourceSha256,compiledSha256:createHash('sha256').update(js).digest('hex'),payloadSha256:createHash('sha256').update(payload+'\n').digest('hex'),parameters,canvasCount:10,realCanvasGetContext:true,globalsOverwritten:false},null,2)+'\n')
console.log(JSON.stringify({runId,sourceSha256,payloadBytes:Buffer.byteLength(payload),canvasCount:10}))
