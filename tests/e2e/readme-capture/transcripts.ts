// Canned terminal output that mimics Claude Code's TUI for the README demo cards.
// Scene 1 lines stay within 52 cells: non-focused cards get one of four columns.
// Each transcript exists once per locale; the English set mirrors the Chinese one line for line and
// keeps the same cell budget, because the card widths do not change with the locale.
import { LOCALE } from './locale'

const R = '\x1b[0m'
const B = '\x1b[1m'
const DIM = '\x1b[2m'
const GREEN = '\x1b[32m'
const RED = '\x1b[31m'
const YELLOW = '\x1b[33m'
const GRAY = '\x1b[90m'
const ORANGE = '\x1b[38;5;214m'

const cells = (s: string): number =>
  [...s.replace(/\x1b\[[0-9;]*m/g, '')].reduce((n, ch) => n + (/[　-鿿＀-￯]/.test(ch) ? 2 : 1), 0)
const pad = (s: string, w: number): string => s + ' '.repeat(Math.max(0, w - cells(s)))

const box = (lines: string[], width = 56): string[] => [
  `${GRAY}╭${'─'.repeat(width - 2)}╮${R}`,
  ...lines.map((l) => `${GRAY}│${R} ${pad(l, width - 4)} ${GRAY}│${R}`),
  `${GRAY}╰${'─'.repeat(width - 2)}╯${R}`
]
const prompt = (text: string): string => `${B}>${R} ${text}`
const say = (text: string): string => `${ORANGE}⏺${R} ${text}`
const tool = (label: string, done = true): string => `${done ? GREEN : YELLOW}⏺${R} ${B}${label}${R}`
const toolErr = (label: string): string => `${RED}⏺${R} ${B}${label}${R}`
const out = (text: string): string => `  ${GRAY}⎿${R}  ${text}`
const cont = (text: string): string => `     ${text}`
const inputBox = (): string[] => box([`${B}>${R}`], 50)
const welcome = (cwd: string): string[] => box([
  `${ORANGE}✻${R} Welcome to Claude Code!`,
  '',
  `  ${DIM}/help for help, /status for your status${R}`,
  '',
  `  ${DIM}cwd: ${cwd}${R}`
], 50)

const implementation = [
  ...welcome('~/work/shop-api'),
  '',
  prompt('给支付回调加幂等处理：同一个 event_id 重复回调'),
  '  直接返回 200，不要重复入账',
  '',
  say('先看现在回调是怎么处理的，再决定在哪一层做幂等。'),
  '',
  tool('Read(src/payments/webhook.ts)'),
  out(`Read 142 lines ${DIM}(ctrl+r to expand)${R}`),
  '',
  tool('Search(pattern: "event_id", path: "src")'),
  out(`Found 6 lines ${DIM}(ctrl+r to expand)${R}`),
  '',
  tool('Update Todos'),
  out(`${GREEN}☒${R} ${DIM}梳理回调处理链路${R}`),
  cont(`${GREEN}☒${R} ${DIM}选择幂等键存储：Redis SETNX + 24h 过期${R}`),
  cont('☐ 在 handleWebhook 入口加幂等校验'),
  cont('☐ 为重复回调补测试'),
  cont('☐ 更新 docs/payments.md 回调约定'),
  '',
  tool('Update(src/payments/webhook.ts)'),
  out(`Updated src/payments/webhook.ts with ${GREEN}8 additions${R}`),
  `       ${DIM}21${R}    export async function handleWebhook(req) {`,
  `       ${DIM}22${R}      const event = parseEvent(req.body)`,
  `       ${DIM}23${R} ${GREEN}+    const key = \`pay:cb:\${event.id}\`${R}`,
  `       ${DIM}24${R} ${GREEN}+    const fresh = await redis.set(${R}`,
  `       ${DIM}25${R} ${GREEN}+      key, '1', 'NX', 'EX', 86_400)${R}`,
  `       ${DIM}26${R} ${GREEN}+    if (!fresh) {${R}`,
  `       ${DIM}27${R} ${GREEN}+      log.info({ id: event.id }, 'dup')${R}`,
  `       ${DIM}28${R} ${GREEN}+      return ok({ duplicate: true })${R}`,
  `       ${DIM}29${R} ${GREEN}+    }${R}`,
  `       ${DIM}30${R}      await paymentService.apply(event)`,
  '',
  say('幂等校验已加在入口，先跑一遍支付模块测试。'),
  '',
  tool('Bash(pnpm vitest run src/payments --reporter=dot)', false),
  out(`${DIM}Running…${R}`),
  '',
  `${ORANGE}✻${R} ${DIM}Running… (8s · esc to interrupt)${R}`
]

// Sibling cards in the three-column canvas get ~42 cells; only the focused card is wide.
const narrowInput = (): string[] => box([`${B}>${R}`], 40)

const regression = [
  prompt('跑一遍支付模块回归，'),
  '  看幂等改动有没有影响现有用例',
  '',
  tool('Bash(pnpm vitest run src/payments)'),
  out(`${DIM}RUN${R}  v4.1.11 ~/work/shop-api`),
  cont(`${GREEN}✓${R} parse-event.test.ts ${DIM}(8) 41ms${R}`),
  cont(`${GREEN}✓${R} apply.test.ts ${DIM}(12) 187ms${R}`),
  cont(`${GREEN}✓${R} refund.test.ts ${DIM}(5) 96ms${R}`),
  cont(`${RED}❯${R} webhook.duplicate.test.ts ${RED}(1)${R}`),
  cont(`  ${RED}×${R} 重复回调应返回 409`),
  cont(`    ${RED}AssertionError: expected 200${R}`),
  cont(`    ${RED}to be 409${R}`),
  '',
  cont(`${DIM}Test Files${R}  ${RED}1 failed${R} | ${GREEN}3 passed${R}`),
  cont(`${DIM}     Tests${R}  ${RED}1 failed${R} | ${GREEN}27 passed${R}`),
  '',
  say('27 个用例通过，1 个失败。'),
  '  失败的用例断言重复回调返回 409，',
  '  但新逻辑改成了 200 + duplicate: true',
  '  （网关只认 2xx 才停止重试）。',
  '',
  '  这是行为变更，不是回归。',
  '  要把这个断言改成新行为吗？',
  '  改的话会同步更新 docs/payments.md。',
  '',
  ...narrowInput(),
  `  ${DIM}? for shortcuts${R}`
]

const review = [
  prompt('对比 Redis SETNX 和 DB 唯一索引，'),
  '  写成 ADR',
  '',
  tool('Read(prisma/schema.prisma)'),
  out('Read 88 lines'),
  '',
  say(`${B}维度        Redis      DB 唯一索引${R}`),
  '  延迟        ~1ms       ~8ms',
  `  Redis 故障  ${YELLOW}不幂等 ⚠${R}   不受影响`,
  '  改动        8 行       迁移 + 12 行',
  '',
  tool('Write(docs/adr/0007-idempotency.md)'),
  out('Wrote 46 lines'),
  '',
  prompt('看看左边卡片的回归跑完没，'),
  '  把结果纳入结论',
  '',
  tool('Bash(mt read left --lines 12)'),
  out(`Test Files  ${RED}1 failed${R} | ${GREEN}3 passed${R}`),
  cont(`${RED}×${R} 重复回调应返回 409`),
  cont(`${DIM}（会话正在等待用户确认）${R}`),
  '',
  say('回归只剩一条与新行为冲突的旧断言，'),
  '  不是功能回归。结论：',
  '  方案 A（Redis）为主路径，',
  '  方案 B 唯一索引兜底，ADR 已更新。',
  '',
  ...narrowInput()
]

const docs = [
  prompt('把新的回调约定写进 docs/payments.md'),
  '',
  tool('Read(docs/payments.md)'),
  out('Read 64 lines'),
  '',
  tool('Update(docs/payments.md)'),
  out(`Updated with ${GREEN}11 additions${R}`),
  `       ${DIM}18${R}    ## 回调幂等`,
  `       ${DIM}19${R} ${GREEN}+  网关可能对同一 event_id${R}`,
  `       ${DIM}20${R} ${GREEN}+  重试多次，服务端按它去重：${R}`,
  `       ${DIM}21${R} ${GREEN}+  - 首次：入账，返回 200${R}`,
  `       ${DIM}22${R} ${GREEN}+  - 重复：不入账，返回 200 +${R}`,
  `       ${DIM}23${R} ${GREEN}+    { duplicate: true }${R}`,
  `       ${DIM}24${R} ${GREEN}+  网关只认 2xx 才停止重试，${R}`,
  `       ${DIM}25${R} ${GREEN}+  重复回调不能返回 409。${R}`,
  '',
  say('文档已更新，与实现保持一致。'),
  '',
  ...narrowInput()
]

const coordinate = [
  prompt('看看回归那张卡片的测试跑到哪了，'),
  '  给我结论',
  '',
  tool('Bash(mt list)'),
  out('1. 实现 · Redis 幂等键 [claude-code]'),
  cont('2. 回归 · 支付模块测试 [claude-code]'),
  cont('3. 审查 · 方案对比 [claude-code]'),
  cont('4. 文档 · 回调约定 [claude-code]'),
  cont('5. 协调 · 跨卡片 [claude-code]'),
  '',
  tool('Bash(mt read sibling:2 --lines 8)'),
  out(`Test Files  ${RED}1 failed${R} | ${GREEN}3 passed${R}`),
  cont(`     Tests  ${RED}1 failed${R} | ${GREEN}27 passed${R}`),
  cont('要把这个断言改成新行为吗？'),
  '',
  say('回归跑完了：27 过 1 挂。挂的那条'),
  '  断言重复回调返回 409，和新的幂等',
  '  行为冲突，不是功能回归。它在等你',
  '  决定要不要改断言。',
  '',
  prompt('让它改成新行为，顺手更新文档'),
  '',
  tool('Bash(mt send sibling:2 "改成新行为，'),
  '       并同步 docs/payments.md" --enter)',
  out('已发送到「回归 · 支付模块测试」'),
  '',
  say('已经交给回归卡片了，完成后我再读一次。'),
  '',
  ...narrowInput()
]

// Scene 2 lives in a four-column canvas (~38 cells per card); the DAG shows each node's last four lines.
// Scene-2 transcripts keep the cursor on their last line (DAG previews show it), and that line is
// never reflowed when the card widens, so it must fit the narrowest card: 26 cells.
const narrowBox = (): string[] => box([`${B}>${R}`], 26)

const baseline = [
  prompt('梳理支付回调链路和幂等性的现状'),
  '',
  tool('Read(src/payments/webhook.ts)'),
  out('Read 142 lines'),
  '',
  say('回调链路：webhook → apply → 状态机'),
  '  当前没有幂等保护，重试会重复入账。',
  '',
  say('建议并行验证两条路线：'),
  '  A：Redis SETNX，B：DB 唯一索引。'
]

const planA = [
  prompt('按 Redis SETNX 方案实现并跑测试'),
  '',
  tool('Update(src/payments/webhook.ts)'),
  out(`Updated with ${GREEN}8 additions${R}`),
  '',
  tool('Bash(pnpm vitest run src/payments)', false),
  out(`${GREEN}✓${R} 27 passed ${DIM}· 还在跑最后 1 个${R}`),
  '',
  `${ORANGE}✻${R} ${DIM}Running… (21s · esc)${R}`
]

const planB = [
  prompt('给 event_id 加唯一约束并执行迁移'),
  '',
  tool('Update(prisma/schema.prisma)'),
  out(`Updated with ${GREEN}2 additions${R}`),
  '',
  toolErr('Bash(pnpm prisma migrate dev)'),
  out(`${RED}Error: P2002 unique constraint${R}`),
  cont(`${RED}历史数据有 37 条重复 event_id${R}`),
  '',
  say('迁移中止，需要先清洗历史数据。'),
  '  要我先写一个清洗脚本吗？',
  '',
  ...narrowBox()
]

const vitest = [
  '',
  ` ${B}RUN${R}  ${DIM}v4.1.11 ~/work/shop-api${R}`,
  '',
  ` ${GREEN}✓${R} parse-event.test.ts ${DIM}(8) 41ms${R}`,
  ` ${GREEN}✓${R} apply.test.ts ${DIM}(12) 187ms${R}`,
  ` ${GREEN}✓${R} refund.test.ts ${DIM}(5) 96ms${R}`,
  ` ${RED}❯${R} webhook.duplicate.test.ts ${RED}(1)${R}`,
  `   ${RED}× 重复回调应返回 409${R}`,
  '',
  ` ${DIM}Test Files${R}  ${RED}1 failed${R} | ${GREEN}3 passed${R}`,
  ` ${DIM}     Tests${R}  ${RED}1 failed${R} | ${GREEN}27 passed${R}`,
  ` ${DIM}  Duration${R}  1.42s`,
  ''
]

// Roles that hand the stub an `exec` event (mt read/fork actually run) so viewers see real output.
const baselineThree = [
  ...welcome('~/work/shop-api'),
  '',
  prompt('支付回调重复入账，给我几个幂等方案，先别动代码'),
  '',
  say('三个方向，各有取舍：'),
  cont('1. Redis SETNX 幂等键，24h 过期，最快落地'),
  cont('2. DB 唯一索引 (provider, event_id)，最稳'),
  cont('3. 消费侧去重表 + 定时清理，兼容历史数据'),
  '',
  say('建议各开一条路验证，我在这里等你决定。'),
  '',
  ...inputBox()
]
const aiRead = [
  ...welcome('~/work/shop-api'),
  '',
  prompt('看看左边那张卡片的测试跑到哪了，给我结论'),
  '',
  say('我先读一下左边卡片的实时输出。')
]
const aiFork = [
  '',
  prompt('为这三个方案各开一张子卡片'),
  '',
  say('好，按方案 1、2、3 各建一张子卡片，继承当前上下文。')
]

// ---------- English ----------

const implementationEn = [
  ...welcome('~/work/shop-api'),
  '',
  prompt('Make the payment webhook idempotent: a repeat'),
  '  of the same event_id must return 200 without',
  '  posting the payment twice',
  '',
  say('Reading the current handler before picking a layer.'),
  '',
  tool('Read(src/payments/webhook.ts)'),
  out(`Read 142 lines ${DIM}(ctrl+r to expand)${R}`),
  '',
  tool('Search(pattern: "event_id", path: "src")'),
  out(`Found 6 lines ${DIM}(ctrl+r to expand)${R}`),
  '',
  tool('Update Todos'),
  out(`${GREEN}☒${R} ${DIM}Map the webhook handling path${R}`),
  cont(`${GREEN}☒${R} ${DIM}Pick the store: Redis SETNX + 24h expiry${R}`),
  cont('☐ Add the idempotency check to handleWebhook'),
  cont('☐ Cover repeated callbacks with a test'),
  cont('☐ Update the contract in docs/payments.md'),
  '',
  tool('Update(src/payments/webhook.ts)'),
  out(`Updated src/payments/webhook.ts with ${GREEN}8 additions${R}`),
  `       ${DIM}21${R}    export async function handleWebhook(req) {`,
  `       ${DIM}22${R}      const event = parseEvent(req.body)`,
  `       ${DIM}23${R} ${GREEN}+    const key = \`pay:cb:\${event.id}\`${R}`,
  `       ${DIM}24${R} ${GREEN}+    const fresh = await redis.set(${R}`,
  `       ${DIM}25${R} ${GREEN}+      key, '1', 'NX', 'EX', 86_400)${R}`,
  `       ${DIM}26${R} ${GREEN}+    if (!fresh) {${R}`,
  `       ${DIM}27${R} ${GREEN}+      log.info({ id: event.id }, 'dup')${R}`,
  `       ${DIM}28${R} ${GREEN}+      return ok({ duplicate: true })${R}`,
  `       ${DIM}29${R} ${GREEN}+    }${R}`,
  `       ${DIM}30${R}      await paymentService.apply(event)`,
  '',
  say('Guard added at the entry point. Running the tests.'),
  '',
  tool('Bash(pnpm vitest run src/payments --reporter=dot)', false),
  out(`${DIM}Running…${R}`),
  '',
  `${ORANGE}✻${R} ${DIM}Running… (8s · esc to interrupt)${R}`
]

const regressionEn = [
  prompt('Run the payments regression and'),
  '  see what the idempotency change broke',
  '',
  tool('Bash(pnpm vitest run src/payments)'),
  out(`${DIM}RUN${R}  v4.1.11 ~/work/shop-api`),
  cont(`${GREEN}✓${R} parse-event.test.ts ${DIM}(8) 41ms${R}`),
  cont(`${GREEN}✓${R} apply.test.ts ${DIM}(12) 187ms${R}`),
  cont(`${GREEN}✓${R} refund.test.ts ${DIM}(5) 96ms${R}`),
  cont(`${RED}❯${R} webhook.duplicate.test.ts ${RED}(1)${R}`),
  cont(`  ${RED}×${R} repeat callback returns 409`),
  cont(`    ${RED}AssertionError: expected 200${R}`),
  cont(`    ${RED}to be 409${R}`),
  '',
  cont(`${DIM}Test Files${R}  ${RED}1 failed${R} | ${GREEN}3 passed${R}`),
  cont(`${DIM}     Tests${R}  ${RED}1 failed${R} | ${GREEN}27 passed${R}`),
  '',
  say('27 tests pass, 1 fails.'),
  '  It expects 409 for a repeat',
  '  callback, but the new code returns',
  '  200 + duplicate: true (the gateway',
  '  only stops retrying on 2xx).',
  '',
  '  That is a behavior change, not a',
  '  regression. Update the assertion to',
  '  the new behavior? I will sync',
  '  docs/payments.md with it.',
  '',
  ...narrowInput(),
  `  ${DIM}? for shortcuts${R}`
]

const reviewEn = [
  prompt('Compare Redis SETNX with a DB'),
  '  unique index, write it up as an ADR',
  '',
  tool('Read(prisma/schema.prisma)'),
  out('Read 88 lines'),
  '',
  say(`${B}Axis       Redis       DB unique${R}`),
  '  Latency    ~1ms        ~8ms',
  `  Redis down ${YELLOW}unguarded ⚠${R} unaffected`,
  '  Change     8 lines     migration + 12',
  '',
  tool('Write(docs/adr/0007-idempotency.md)'),
  out('Wrote 46 lines'),
  '',
  prompt('Check the regression card on the'),
  '  left and fold the result in',
  '',
  tool('Bash(mt read left --lines 12)'),
  out(`Test Files  ${RED}1 failed${R} | ${GREEN}3 passed${R}`),
  cont(`${RED}×${R} repeat callback returns 409`),
  cont(`${DIM}(that session is waiting for input)${R}`),
  '',
  say('The only failure is an old assertion'),
  '  that conflicts with the new behavior,',
  '  not a functional regression. Verdict:',
  '  Redis is the main path, the unique',
  '  index the fallback. ADR updated.',
  '',
  ...narrowInput()
]

const docsEn = [
  prompt('Write the new callback contract'),
  '  into docs/payments.md',
  '',
  tool('Read(docs/payments.md)'),
  out('Read 64 lines'),
  '',
  tool('Update(docs/payments.md)'),
  out(`Updated with ${GREEN}11 additions${R}`),
  `       ${DIM}18${R}    ## Callback idempotency`,
  `       ${DIM}19${R} ${GREEN}+  The gateway may retry the${R}`,
  `       ${DIM}20${R} ${GREEN}+  same event_id; the server${R}`,
  `       ${DIM}21${R} ${GREEN}+  de-duplicates on it:${R}`,
  `       ${DIM}22${R} ${GREEN}+  - first: post, return 200${R}`,
  `       ${DIM}23${R} ${GREEN}+  - repeat: no post, 200 +${R}`,
  `       ${DIM}24${R} ${GREEN}+    { duplicate: true }${R}`,
  `       ${DIM}25${R} ${GREEN}+  Only 2xx stops the retries,${R}`,
  `       ${DIM}26${R} ${GREEN}+  so never answer 409 here.${R}`,
  '',
  say('Docs now match the implementation.'),
  '',
  ...narrowInput()
]

const coordinateEn = [
  prompt('How far did the regression card'),
  '  get? Give me the conclusion',
  '',
  tool('Bash(mt list)'),
  out('1. Implement · Redis [claude-code]'),
  cont('2. Regression · tests [claude-code]'),
  cont('3. Review · plan A/B [claude-code]'),
  cont('4. Docs · contract [claude-code]'),
  cont('5. Coordinate · cards [claude-code]'),
  '',
  tool('Bash(mt read sibling:2 --lines 8)'),
  out(`Test Files  ${RED}1 failed${R} | ${GREEN}3 passed${R}`),
  cont(`     Tests  ${RED}1 failed${R} | ${GREEN}27 passed${R}`),
  cont('Update the assertion to the new'),
  cont('behavior?'),
  '',
  say('It finished: 27 pass, 1 fails. The'),
  '  failing one expects 409 for a repeat',
  '  callback, which the new idempotency',
  '  behavior contradicts. Not a',
  '  regression; it is waiting on your',
  '  call about the assertion.',
  '',
  prompt('Tell it to take the new behavior'),
  '  and update the docs too',
  '',
  tool('Bash(mt send sibling:2 "switch to the'),
  '       new behavior and sync',
  '       docs/payments.md" --enter)',
  out('Sent to "Regression · tests"'),
  '',
  say('Handed to the regression card; I will'),
  '  read it again once it finishes.',
  '',
  ...narrowInput()
]

const baselineEn = [
  prompt('Map the payment callback path'),
  '  and where idempotency stands',
  '',
  tool('Read(src/payments/webhook.ts)'),
  out('Read 142 lines'),
  '',
  say('Path: webhook → apply → state'),
  '  machine. No guard today, so',
  '  a retry posts the payment twice.',
  '',
  say('Verify two routes in parallel:'),
  '  A Redis SETNX, B DB unique index.'
]

const planAEn = [
  prompt('Implement the Redis SETNX plan'),
  '  and run the tests',
  '',
  tool('Update(src/payments/webhook.ts)'),
  out(`Updated with ${GREEN}8 additions${R}`),
  '',
  tool('Bash(pnpm vitest run src/payments)', false),
  out(`${GREEN}✓${R} 27 passed ${DIM}· 1 still running${R}`),
  '',
  `${ORANGE}✻${R} ${DIM}Running… (21s · esc)${R}`
]

const planBEn = [
  prompt('Add a unique constraint on'),
  '  event_id and run the migration',
  '',
  tool('Update(prisma/schema.prisma)'),
  out(`Updated with ${GREEN}2 additions${R}`),
  '',
  toolErr('Bash(pnpm prisma migrate dev)'),
  out(`${RED}Error: P2002 unique constraint${R}`),
  cont(`${RED}37 duplicate event_id rows${R}`),
  '',
  say('Migration aborted. The existing'),
  '  rows need cleaning up first.',
  '  Want me to write that script?',
  '',
  ...narrowBox()
]

const vitestEn = [
  '',
  ` ${B}RUN${R}  ${DIM}v4.1.11 ~/work/shop-api${R}`,
  '',
  ` ${GREEN}✓${R} parse-event.test.ts ${DIM}(8) 41ms${R}`,
  ` ${GREEN}✓${R} apply.test.ts ${DIM}(12) 187ms${R}`,
  ` ${GREEN}✓${R} refund.test.ts ${DIM}(5) 96ms${R}`,
  ` ${RED}❯${R} webhook.duplicate.test.ts ${RED}(1)${R}`,
  `   ${RED}× repeat callback returns 409${R}`,
  '',
  ` ${DIM}Test Files${R}  ${RED}1 failed${R} | ${GREEN}3 passed${R}`,
  ` ${DIM}     Tests${R}  ${RED}1 failed${R} | ${GREEN}27 passed${R}`,
  ` ${DIM}  Duration${R}  1.42s`,
  ''
]

const baselineThreeEn = [
  ...welcome('~/work/shop-api'),
  '',
  prompt('Payment callbacks post twice. Give me a few'),
  '  idempotency options, do not touch the code yet',
  '',
  say('Three directions, each with a trade-off:'),
  cont('1. Redis SETNX key with 24h expiry, fastest'),
  cont('2. DB unique index on event_id, safest'),
  cont('3. Dedupe table on the consumer + cleanup job'),
  '',
  say('Open one path per option; I will wait here.'),
  '',
  ...inputBox()
]
const aiReadEn = [
  ...welcome('~/work/shop-api'),
  '',
  prompt('How far did the card on the left get? Give me'),
  '  the conclusion',
  '',
  say('Let me read that card\'s live output first.')
]
const aiForkEn = [
  '',
  prompt('Open a child card for each of the three plans'),
  '',
  say('One child card per plan, inheriting the context.')
]

// Scene-1 cards are printed while narrow and widen when focused; xterm reflows every line except the
// cursor line, so those transcripts end with a newline. DAG previews show the last four lines, so the
// scene-2 transcripts keep the cursor on their final line instead.
const trailingNewline = new Set(['implementation', 'regression', 'review', 'docs', 'coordinate'])

const transcriptSource: Record<'zh-CN' | 'en', Record<string, string[]>> = {
  'zh-CN': {
    implementation, regression, review, docs, coordinate, baseline, planA, planB, vitest,
    'baseline-three': baselineThree, 'ai-read': aiRead, 'ai-fork': aiFork
  },
  en: {
    implementation: implementationEn, regression: regressionEn, review: reviewEn, docs: docsEn,
    coordinate: coordinateEn, baseline: baselineEn, planA: planAEn, planB: planBEn, vitest: vitestEn,
    'baseline-three': baselineThreeEn, 'ai-read': aiReadEn, 'ai-fork': aiForkEn
  }
}

export const transcripts: Record<string, string> = Object.fromEntries(
  Object.entries(transcriptSource[LOCALE])
    .map(([name, lines]) => [name, lines.join('\r\n') + (trailingNewline.has(name) ? '\r\n' : '')])
)

export function widestLine(name: string): number {
  return Math.max(...transcripts[name]!.split('\r\n').map(cells))
}
