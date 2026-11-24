// Execute the pinned terminal scrollbar implementation to freeze its geometry.
import { createRequire } from 'node:module'
import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { createHash } from 'node:crypto'
import { execFileSync } from 'node:child_process'
import { resolve } from 'node:path'

const { buildSync } = createRequire(import.meta.url)('../../node_modules/.pnpm/esbuild@0.27.7/node_modules/esbuild')
const source = resolve('node_modules/.pnpm/@xterm+xterm@6.0.0/node_modules/@xterm/xterm/src')
const state = `${source}/vs/base/browser/ui/scrollbar/scrollbarState.ts`
const scrollable = `${source}/vs/base/common/scrollable.ts`
const output = resolve('artifacts/native-parity/reference/terminal-scrollbar-export.cjs')
const fixtures = resolve('apps/native/desktop/fixtures/terminal-scrollbar.json')
mkdirSync(resolve('artifacts/native-parity/reference'), { recursive: true })
const cases = [
  [560, 560, 0], [560, 574, 0], [560, 574, 14],
  [560, 1400, 0], [560, 1400, 350], [560, 1400, 840],
  [560, 140560, 0], [560, 140560, 70000], [560, 140560, 140000],
  [561.5, 1401.5, 350.5], [560.25, 1400.25, 350.25],
  [28, 1400, 14], [14, 1400, 14],
  [560, 280, 0], [280, 1400, 350]
]
buildSync({
  stdin: { contents: `
    const { ScrollbarState } = require(${JSON.stringify(state)});
    const { Scrollable } = require(${JSON.stringify(scrollable)});
    const cases = ${JSON.stringify(cases)}.map(([viewport,extent,position]) => {
      const state = new ScrollbarState(0,14,0,0,0,0);
      state.setVisibleSize(viewport); state.setScrollSize(extent); state.setScrollPosition(position);
      const scrollable = new Scrollable({forceIntegerValues:false,smoothScrollDuration:0,scheduleAtNextAnimationFrame:()=>({dispose(){}})});
      scrollable.setScrollDimensions({height:Math.round(viewport),scrollHeight:Math.round(extent)});
      const probes=[];
      for (const op of ['track','drag']) for (const value of [-1000,0,0.5,10,280,560,1000]) {
        const desired = op==='track' ? state.getDesiredScrollPositionFromOffset(value) : state.getDesiredScrollPositionFromDelta(value);
        scrollable.setScrollPositionNow({scrollTop:desired});
        probes.push({op,value,result:scrollable.getCurrentScrollPosition().scrollTop});
      }
      return {viewport,extent,position,needed:state.isNeeded(),thumbSize:state.getSliderSize(),thumbTop:state.getSliderPosition(),probes};
    });
    console.log(JSON.stringify(cases));
  `, resolveDir: source, sourcefile: 'terminal-scrollbar-export.ts', loader: 'ts' },
  outfile: output, bundle: true, platform: 'node', format: 'cjs', logLevel: 'silent',
  alias: { vs: `${source}/vs` }
})
const results = JSON.parse(execFileSync(process.execPath,[output],{encoding:'utf8'}))
const sources = [state, scrollable].map(path=>({path:path.slice(source.length+1),sha256:createHash('sha256').update(readFileSync(path)).digest('hex')}))
writeFileSync(fixtures,JSON.stringify({version:'6.0.0',sources,cases:results},null,2)+'\n')
console.log(`Exported ${results.length} geometry cases and ${results.reduce((n,c)=>n+c.probes.length,0)} source scroll positions`)
