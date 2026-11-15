// Execute the frozen UI engine's selection code against its real parser buffer.
// This fixture proves selection semantics; it does not replace native mouse tests.
import { createRequire } from 'node:module'
import { mkdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { execFileSync } from 'node:child_process'

const { build } = createRequire(import.meta.url)('../../node_modules/.pnpm/esbuild@0.27.7/node_modules/esbuild')

const source = resolve('node_modules/.pnpm/@xterm+xterm@6.0.0/node_modules/@xterm/xterm/src')
const output = resolve('artifacts/native-parity/reference/terminal-selection-export.cjs')
mkdirSync(resolve('artifacts/native-parity/reference'), { recursive: true })
const entry = `
import { SelectionService } from 'browser/services/SelectionService';
import { SelectionModel } from 'browser/selection/SelectionModel';
import { CellData } from 'common/buffer/CellData';
import { createRequire } from 'node:module';
import { mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
const require = createRequire(resolve('apps/runtime/package.json'));
const { Terminal } = require('@xterm/headless');
const cases = [
 {name:'paths-and-cjk',cols:64,rows:3,text:'SELECT_ALPHA beta-gamma/path 中文词尾',points:[[3,0],[18,0],[31,0],[30,0]],drags:[['word',[18,0],[3,0]],['word',[3,0],[31,0]]]},
 {name:'separators-and-spaces',cols:32,rows:3,text:'aa(bb) cc,dd   end',points:[[2,0],[5,0],[9,0],[11,0],[13,0],[25,0]],drags:[]},
 {name:'soft-wrap',cols:10,rows:5,text:'ab LONGPATH/abcdefghij END\\r\\nHARD next',points:[[7,0],[3,1],[2,2],[2,3]],drags:[['word',[3,1],[2,3]],['word',[2,3],[3,1]],['line',[3,1],[2,3]],['line',[2,3],[3,1]],['line',[3,1],[3,0]]]},
 {name:'wide-combining',cols:16,rows:4,text:'A 中文é🙂 tail\\r\\nX👩‍💻Y',points:[[3,0],[7,0],[9,0],[2,1]],drags:[]},
 {name:'word-drag-start-row',cols:5,rows:4,text:'ABCD(EFGHI\\r\\nK',points:[[4,0]],drags:[['word',[4,0],[0,0]]]},
];
async function main(){
 const results=[];
 for(const fixture of cases){
  const term=new Terminal({cols:fixture.cols,rows:fixture.rows,allowProposedApi:true,scrollback:0});
  await new Promise(done=>term.write(fixture.text,done));
  const service=Object.create(SelectionService.prototype);
  service._bufferService=term._core._bufferService;
  service._optionsService={rawOptions:term.options};
  service._workCell=new CellData();
  service._model=new SelectionModel(service._bufferService);
  const grid=[];
  for(let y=0;y<fixture.rows;y++){
   const row=term.buffer.active.getLine(y);
   const cells=[];
   for(let x=0;x<fixture.cols;x++){const c=row.getCell(x);cells.push({text:c.getWidth()===0?'':c.getChars()||' ',width:c.getWidth(),hasContent:!!c.getChars()});}
   grid.push({continuation:row.isWrapped,cells});
  }
  const result=()=>({start:service._model.finalSelectionStart,end:service._model.finalSelectionEnd,text:service.selectionText});
  const probes=[];
  for(const point of fixture.points){
   for(const mode of ['word','line']){
    service._model.clearSelection();
    if(mode==='word')service._selectWordAt([...point],true);else service._selectLineAt(point[1]);
    probes.push({mode,point,expected:result()});
   }
  }
  for(const [mode,point,target] of fixture.drags){
   service._model.clearSelection();
   if(mode==='word')service._selectWordAt([...point],true);else service._selectLineAt(point[1]);
   service._model.selectionEnd=[...target];
   if(mode==='word')service._selectToWordAt([...target]);
   else service._model.selectionEnd[0]=target[1]<service._model.selectionStart[1]?0:fixture.cols;
   probes.push({mode,point,target,expected:result()});
  }
  results.push({name:fixture.name,cols:fixture.cols,rows:fixture.rows,text:fixture.text,grid,probes});
  term.dispose();
 }
 const path=resolve('apps/native/desktop/tests/fixtures/terminal-selection-reference.json');
 writeFileSync(path,JSON.stringify({source:'Frozen xterm 6.0.0 SelectionService and SelectionModel with real headless parser; coordinates [column,row]; default word separators; no UI claim.',cases:results},null,2)+'\\n');
 console.log(JSON.stringify({path,cases:results.length,probes:results.reduce((n,c)=>n+c.probes.length,0)}));
}
main();
`
await build({stdin:{contents:entry,resolveDir:process.cwd(),sourcefile:'terminal-selection-export.ts'},bundle:true,platform:'node',format:'cjs',outfile:output,
  alias:{browser:resolve(source,'browser'),common:resolve(source,'common'),vs:resolve(source,'vs')},
  tsconfigRaw:{compilerOptions:{experimentalDecorators:true}}})
execFileSync(process.execPath,[output],{stdio:'inherit'})
