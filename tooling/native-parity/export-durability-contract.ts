// Cross-language state contract; no product or user data is opened.
import { writeFile } from 'node:fs/promises'
import { SessionDurabilityGate } from '../../apps/runtime/src/session/session-durability-gate'

type Action = { op: 'append', sequence: number, bytes: number } | { op: 'burst', frames: { sequence: number, bytes: number }[] }
  | { op: 'retry' | 'end' }
const cases: { name: string, max?: number, failures?: Record<number, number>, sideFailure?: number, resumeFailures?: number, actions: Action[] }[] = [
  { name: 'retain-and-retry', failures: {2: 1}, actions: [{op:'append',sequence:1,bytes:8},{op:'append',sequence:2,bytes:8},{op:'append',sequence:3,bytes:8},{op:'retry'}] },
  { name: 'bounded-failure', max:16, failures:{1:1}, actions:[{op:'append',sequence:1,bytes:8},{op:'append',sequence:2,bytes:8},{op:'append',sequence:3,bytes:1},{op:'retry'},{op:'append',sequence:3,bytes:1}] },
  { name: 'healthy-burst', max:32, actions:[{op:'burst',frames:[{sequence:1,bytes:10},{sequence:2,bytes:10},{sequence:3,bytes:10}]}] },
  { name: 'repeat-fault-and-end', failures:{1:3}, actions:[{op:'append',sequence:1,bytes:1},{op:'retry'},{op:'end'},{op:'retry'},{op:'append',sequence:2,bytes:1}] },
  { name: 'side-effect-once', sideFailure:1, actions:[{op:'append',sequence:1,bytes:0},{op:'retry'},{op:'append',sequence:3,bytes:1},{op:'append',sequence:2,bytes:1}] },
  { name: 'resume-failure', failures:{1:1}, resumeFailures:1, actions:[{op:'append',sequence:1,bytes:1},{op:'retry'},{op:'retry'}] },
  ...(process.env.MATOU_DURABILITY_GATE === '1' ? [
    {
      name: 'end-discards-pending',
      failures: { 2: 1 },
      actions: [
        { op: 'append', sequence: 1, bytes: 4 },
        { op: 'append', sequence: 2, bytes: 5 },
        { op: 'append', sequence: 3, bytes: 6 },
        { op: 'end' },
      ],
    },
    {
      name: 'empty-frame-at-limit',
      max: 1,
      failures: { 1: 1 },
      actions: [
        { op: 'append', sequence: 1, bytes: 0 },
        { op: 'append', sequence: 2, bytes: 0 },
        { op: 'retry' },
        { op: 'append', sequence: 2, bytes: 0 },
      ],
    },
    {
      name: 'duplicate-and-sequence-gap',
      actions: [
        { op: 'append', sequence: 1, bytes: 1 },
        { op: 'append', sequence: 1, bytes: 1 },
        { op: 'append', sequence: 3, bytes: 1 },
        { op: 'append', sequence: 2, bytes: 1 },
      ],
    },
  ] satisfies { name: string, max?: number, failures?: Record<number, number>, actions: Action[] }[] : []),
]
const results=[]
for (const input of cases) {
  const trace: string[]=[]; const failures={...input.failures}; let resumes=input.resumeFailures??0
  const gate=new SessionDurabilityGate({sessionId:input.name,maxRetainedBytes:input.max,
    pauser:{pause(){trace.push('pause')},resume(){trace.push('resume'); if(resumes-- >0)throw new Error('resume-fault')}},
    onFault(e){trace.push(`fault:${e.failedSequence}:${e.retainedBytes}`)},
    onRecovered(e){trace.push(`recovered:${e.throughSequence}`)},
    onSideEffectError(_e,frame){trace.push(`side-error:${frame.sequence}`)}})
  const frame=(sequence:number,bytes:number)=>({sequence,kind:'output' as const,bytes:new Uint8Array(bytes),
    async persist(){trace.push(`attempt:${sequence}`);if((failures[sequence]??0)>0){failures[sequence]--;throw new Error('storage-fault')}trace.push(`persisted:${sequence}`)},
    afterPersist(){trace.push(`publish:${sequence}`);if(input.sideFailure===sequence)throw new Error('side-fault')}})
  const snapshot=()=>({state:gate.state,retainedBytes:gate.retainedBytes,lastAcceptedSequence:gate.lastAcceptedSequence,lastPersistedSequence:gate.lastPersistedSequence,trace:[...trace]})
  const steps=[]
  for(const action of input.actions){
    let error:string|undefined, admitted:ReturnType<typeof snapshot>|undefined
    try{
      if(action.op==='append')await gate.append(frame(action.sequence,action.bytes))
      else if(action.op==='burst'){const work=action.frames.map(f=>gate.append(frame(f.sequence,f.bytes)));admitted=snapshot();await Promise.all(work)}
      else if(action.op==='retry')await gate.retry()
      else await gate.end()
    }catch(e){error=(e as Error).message}
    steps.push({action,...(admitted?{admitted}:{}),...snapshot(),...(error?{error}:{})})
  }
  results.push({input,steps})
}
await writeFile(process.argv[2]!,JSON.stringify({cases:results},null,2)+'\n')
console.log(JSON.stringify({cases:results.length,output:process.argv[2]}))
