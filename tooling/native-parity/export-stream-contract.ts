import {writeFile} from 'node:fs/promises'
import {StringDecoder} from 'node:string_decoder'
import {PtyOutputBatcher} from '../../apps/runtime/src/session/pty-output-batcher'
import {CreditWindow} from '../../apps/runtime/src/flow-control/credit-window'
const sequences=[[[0x61,0xe4],[0xb8],[0xad,0xf0,0x9f],[0x99,0x82,0x62]],[[0xf0,0x9f]],[[0xe2],[0x28,0xa1]],[[0xef],[0xbb],[0xbf,0x41]],[[0xc0,0xaf,0xed,0xa0,0x80,0xff]],[[0xf0],[0x9f],[0x41,0x80]],[[0xe4,0xb8],[],[0xad]]]
const decoding=sequences.map(chunks=>{const d=new StringDecoder('utf8');return{chunks,outputs:chunks.map(c=>d.write(Buffer.from(c))),end:d.end()}})
const batching=[]
for(const chunks of [['ab','cd','e'],['🙂','中','a'],['','one','二']]){
 const emitted:string[]=[];const b=new PtyOutputBatcher(data=>emitted.push(data),{maxCodeUnits:4});const steps=[]
 for(const chunk of chunks){b.offer(chunk);steps.push({chunk,emitted:[...emitted]})}b.flush();batching.push({maxCodeUnits:4,steps,flushed:emitted})
}
const credit=[]
for(const actions of [[['send',1,70],['send',2,50],['ack',1],['ack',2],['ack',2],['ack',3]],[['send',1,100],['send',2,1],['send',2,1],['ack',2]]]){
 const events:string[]=[];const w=new CreditWindow({highWatermarkBytes:100,lowWatermarkBytes:40,onPause(){events.push('pause')},onResume(){events.push('resume')}});const steps=[]
 for(const action of actions){let error:string|undefined;try{if(action[0]==='send')w.recordSent(action[1] as number,action[2] as number);else w.acknowledge(action[1] as number)}catch(e){error=(e as Error).message}steps.push({action,unackedBytes:w.unackedBytes,paused:w.isPaused,events:[...events],...(error?{error}:{})})}credit.push({steps})
}
await writeFile(process.argv[2]!,JSON.stringify({decoding,batching,credit},null,2)+'\n')
console.log(JSON.stringify({decoding:decoding.length,batching:batching.length,credit:credit.length}))
