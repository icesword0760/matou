// Frozen output derivation functions, executed without starting the runtime server.
import { readFileSync, writeFileSync } from 'node:fs'
import { TerminalWorkStatusTracker } from '../../apps/runtime/src/session/terminal-work-status-tracker'
import { TerminalCwdTracker } from '../../apps/runtime/src/session/terminal-cwd-tracker'
const server = readFileSync('apps/runtime/src/runtime-server.ts','utf8')
const start = server.indexOf('export function terminalSummaryLines(')
const end = server.indexOf('function updateShellInputBuffer(', start)
// Strip only TypeScript function annotations; the frozen function bodies remain exact.
const code = server.slice(start,end).replace('export function','function').replaceAll(': string[]','').replaceAll(': string','').replaceAll(': boolean','')
const summary = new Function(code+'; return terminalSummaryLines;')() as (s:string)=>string[]
const workChunks: string[][] = [
 ['\x1b]133;C\x07','output\r\n\x1b]133;D;0\x07','\x1b]133;C\x07\x1b]133;D;2\x07','\x1b]133;D;130\x07'],
 ['text\x1b]133;D;','0\x1b\\prompt','more prompt'],
 ['\x1b]133;C\x07',"printf 'enter value: '; read -r value\r\n",'enter ','value: '],
 ['\x1b]133;C\x07',"python3 -c 'input()'\r\n"],
 ['Password: ','continue? [y/N] ','Password: accepted\r\n'],
 ['\x1b]133;D;0\x07\x1b]133;A\x07\x1b]133;C\x07STA008_WAIT> '],
 ...['Select an option: ','password for user bob: ','inputs: ','Selector: ','Which item should I select?',
 'Reading the file so I can select the right section: ','steps:\r\n  input: build','STA008_WAIT> ',
 '请输入姓名：','[y/n]','\x1b[31mPassword:\x1b[0m ','\uFEFFPassword:','\u0085Password:',
 `Type ${'😀'.repeat(32)}?`,`Type ${'😀'.repeat(33)}?`].map(raw=>[raw]),
 ['\x1b]133;D;-1\x1b\\','\x1b]133;D\x07','\x1b]133;D;١٣٠\x07','x'.repeat(600),'Password: '],
]
const cwdChunks = [
 ['\x1b]7;file://host/tmp/first\x1b\\','\x1b]7;file://host/tmp/space%20name\x07'],
 ['prefix\x1b]7;file://host/tmp/spl','it\x1b\\prompt'],
 ['\x1b]7;https://host/tmp\x07','\x1b]7;broken\x07'],
 ['\x1b]7;file:///tmp/中文\x07','\x1b]7;file:///tmp/%E4%B8%AD%E6%96%87\x07'],
 ['\x1b]7;file:///tmp/%ZZ\x07','\x1b]7;file:///tmp/%FF\x07','\x1b]7;file:///tmp/a/../b?x=1#hash\x07'],
 ['\x1b]7;file:///tmp/first\x07\x1b]7;file:///tmp/last\x1b\\','trailing'],
 ['x'.repeat(4100)+'\x1b]7;file:///tmp/tail\x07'],
]
const summaryRaw = [
 'one\r\ntwo\r\nthree\r\nfour\r\nfive\r\n',
 '\x1b[31m中文🙂\x1b[0m\r\n\x1b]7;file:///tmp\x07next\r\n',
 'user@host ~ %\r\n%\r\n$\r\n#\r\n>\r\n',
 '(venv) ➜ repo git:(main) ✗\r\n❯\r\n───\r\n⏸ manual mode on\r\nkept output',
 '\x1b(Balpha\rbravo\n\uFEFF\n\u0085\n',
 'Password: \r\nvalue accepted\r\n',
 'partial\x1b]0;title\x1b\\tail\x1b[2K\r\nlatest',
]
const payload = {
 versions:process.versions,
 work:workChunks.map((chunks,index)=>{const tracker=new TerminalWorkStatusTracker();return {name:`shell-${index}`,chunks,expected:chunks.map(c=>tracker.ingest(c))}}),
 cwd:cwdChunks.map((chunks,index)=>{const tracker=new TerminalCwdTracker();return {name:`cwd-${index}`,chunks,expected:chunks.map(c=>tracker.ingest(c)??null)}}),
 summary:summaryRaw.map((raw,index)=>({name:`summary-${index}`,raw,expected:summary(raw)})),
 tail:[{name:'utf16-boundary',raw:'ab😀z',limit:2,expected:Buffer.from('ab😀z'.slice(-2)).toString('utf8')}]
}
writeFileSync(process.argv[2]!,JSON.stringify(payload,null,2)+'\n')
console.log(JSON.stringify({work:payload.work.length,cwd:payload.cwd.length,summary:payload.summary.length}))
