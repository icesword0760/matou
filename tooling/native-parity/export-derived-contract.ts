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
 ['\x1b]133;A\x07user@host ~/中文 % ', '\x1b]133;B\x07printf ok\r\n',
  '\x1b]133;C\x07', '\r\n\n', '\x1b[32m\x1b[0m', 'ok\r\n',
  '\x1b]133;D;0\x07\x1b]133;A\x07user@host ~/中文 % '], // shell-22: prompt, echo, running, idle
 ['\x1b]133;C\x07', '请输入姓名：', '\x1b[0m\x1b[2K', '\r\n\r\n',
  '\x1b]133;D;7\x1b', '\\', '\x1b]133;C\x07\x1b]133;D;130\x07'], // shell-23: wait, blanks, failure, interrupt
 ['\x1b[2K\x1b]0;Password:\x07\x1b[0m', '\x1b]133;X;0\x07', 'Password: \r\n\r\n'], // shell-24: control only, bad marker
 ['\x1b]133;A\x07❯ ', 'printf ok\r\n', 'ok\r\n', '\x1b[0m'], // shell-25: no execution marker
]
const cwdChunks = [
 ['\x1b]7;file://host/tmp/first\x1b\\','\x1b]7;file://host/tmp/space%20name\x07'],
 ['prefix\x1b]7;file://host/tmp/spl','it\x1b\\prompt'],
 ['\x1b]7;https://host/tmp\x07','\x1b]7;broken\x07'],
 ['\x1b]7;file:///tmp/中文\x07','\x1b]7;file:///tmp/%E4%B8%AD%E6%96%87\x07'],
 ['\x1b]7;file:///tmp/%ZZ\x07','\x1b]7;file:///tmp/%FF\x07','\x1b]7;file:///tmp/a/../b?x=1#hash\x07'],
 ['\x1b]7;file:///tmp/first\x07\x1b]7;file:///tmp/last\x1b\\','trailing'],
 ['x'.repeat(4100)+'\x1b]7;file:///tmp/tail\x07'],
 ['\x1b]0;/tmp/中文 空格\x07', '\x1b]2;user@host:~/中文 空格\x1b\\',
  '\x1b]7;file:///tmp/%E4%B8%AD%E6%96%87%20%E7%A9%BA%E6%A0%BC\x07'], // cwd-7: title vs. OSC 7
 ['user@host ~ % cd "./中文 空格"\r\n', 'user@host /tmp/中文 空格 % ',
  '/tmp/中文 空格\r\n', '\x1b]7;file:///tmp/中文 空格\x1b\\'], // cwd-8: echo, prompt and pwd vs. OSC 7
 ['\x1b]7;./中文%20空格\x07', '\x1b]7;file:../中文%20空格\x07',
  '\x1b]7;file:///tmp/base/../中文%20空格\x07'], // cwd-9: relative payload and dot segments
 ['\x1b]7;file:///tmp/%E4%B8\x07', '\x1b]7;file:///tmp/%ED%A0%80\x07',
  '\x1b]7;file:///tmp/bad\x1b[0m\x07', '\x1b]7;file:///tmp/recovered\x07'], // cwd-10: malformed frame and recovery
 ['\x1b]7;file://localhost/C|/a%20b\x07', '\x1b]7;file://host/share/%2E%2E/中文\x07',
  '\x1b]7;file:///tmp/a%2Fb%23c%3Fd\x07'], // cwd-11: file-URL syntax and decoded delimiters
]
const summaryRaw = [
 'one\r\ntwo\r\nthree\r\nfour\r\nfive\r\n',
 '\x1b[31m中文🙂\x1b[0m\r\n\x1b]7;file:///tmp\x07next\r\n',
 'user@host ~ %\r\n%\r\n$\r\n#\r\n>\r\n',
 '(venv) ➜ repo git:(main) ✗\r\n❯\r\n───\r\n⏸ manual mode on\r\nkept output',
 '\x1b(Balpha\rbravo\n\uFEFF\n\u0085\n',
 'Password: \r\nvalue accepted\r\n',
 'partial\x1b]0;title\x1b\\tail\x1b[2K\r\nlatest',
 '\x1b(B\x1b[1;31m全角Ａ中文\x1b[0m\r\n\x1b]0;隐藏标题\x07  e\u0301 ≠ é\r\n✈️ 👍🏽 👩‍💻 👨‍👩‍👧‍👦', // summary-7: exact Unicode text
 'obsolete\r\n第一行\r\n\r\n  重复 \t\n\x1b[32m  重复\x1b[0m\r最后一行\n%\n', // summary-8: line selection, order and duplicates
 'before\x1b]0;title\x1b\\中间👩‍💻\x1b]8;;https://example.invalid\x1b\\after', // summary-9: visible text between two ST-terminated OSC frames
 '\x1b[0m\x1b[2K\x1b]0;title\x07\x1b(B\r\n', // summary-10: no visible lines
 '前缀\x1b]7;file:///tmp/%ZZ\x07\x1b[31m后缀\x1b[0m\r\n中断\x1b]0;unfinished', // summary-11: malformed URL and incomplete ANSI
]

// New cases also exercise the rolling window used before the summary parser.
// Use the frozen expression verbatim, including the placement of the UTF-16 slice.
const summaryWindowExpression = server.match(/this\.#summaryBuffers\.set\(sessionId, (\(current \+ data\)\.slice\(-[\d_]+\))\)/)?.[1]
if (!summaryWindowExpression) throw new Error('Frozen summary window expression not found')
const appendSummary = new Function('current', 'data', `return ${summaryWindowExpression}`) as (current: string, data: string) => string
const visibleTail = '界e\u0301\n'
const bufferedSummaryCases = [
  {
    name: 'surrogate-at-window-edge',
    chunks: ['older\n😀' + visibleTail + '\n'.repeat(32_768 - visibleTail.length - 2), '\n', '\n'],
  },
  {
    name: 'split-ansi-and-merged-lines',
    chunks: ['\x1b[31', 'm中文👩‍💻\x1b[0m\r', '\n\r\nrepeat\nrepeat\n末', '尾 e\u0301\n'],
  },
]
const payload = {
 versions:process.versions,
 work:workChunks.map((chunks,index)=>{const tracker=new TerminalWorkStatusTracker();return {name:`shell-${index}`,chunks,expected:chunks.map(c=>tracker.ingest(c))}}),
 cwd:cwdChunks.map((chunks,index)=>{const tracker=new TerminalCwdTracker();return {name:`cwd-${index}`,chunks,expected:chunks.map(c=>tracker.ingest(c)??null)}}),
 summary:summaryRaw.map((raw,index)=>({name:`summary-${index}`,raw,expected:summary(raw)})),
 tail:[
  {name:'utf16-boundary',raw:'ab😀z',limit:2,expected:Buffer.from('ab😀z'.slice(-2)).toString('utf8')},
  {name:'utf16-whole-surrogate-pair',raw:'ab😀z',limit:3,expected:'ab😀z'.slice(-3)},
  {name:'utf16-inside-zwj',raw:'a👩‍💻z',limit:4,expected:'a👩‍💻z'.slice(-4)},
  {name:'utf16-inside-combining-sequence',raw:'ae\u0301z',limit:2,expected:'ae\u0301z'.slice(-2)},
 ],
 bufferedSummary: bufferedSummaryCases.map(({ name, chunks }) => {
   let current = ''
   return { name, chunks, expected: chunks.map(chunk => {
     current = appendSummary(current, chunk)
     return summary(current)
   }) }
 }),
}
writeFileSync(process.argv[2]!,JSON.stringify(payload,null,2)+'\n')
console.log(JSON.stringify({work:payload.work.length,cwd:payload.cwd.length,summary:payload.summary.length}))
