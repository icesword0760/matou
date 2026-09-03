// Canned terminal output that mimics Claude Code's TUI for the README demo cards.
// Scene 1 lines stay within 52 cells: non-focused cards get one of four columns.

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
  tool('Bash(mt read left --tail 12)'),
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

const research = [
  prompt('查一下支付网关的重试策略：'),
  '  间隔、次数、什么状态码停止重试',
  '',
  tool('WebFetch(docs.pay.example/webhooks)'),
  out(`Received 24.1KB ${DIM}(200 OK)${R}`),
  '',
  say('网关重试策略：'),
  '  - 非 2xx 或超时（10s）即重试',
  '  - 指数退避：1m, 5m, 30m, 2h, 12h',
  '  - 最多 5 次，之后进入人工对账',
  '  - 幂等由商户侧负责，网关不去重',
  '',
  say('这意味着：'),
  '  1. 重复回调必须返回 2xx',
  '  2. 12h 内都可能重放，幂等键',
  '     过期时间至少要覆盖 24h',
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

// Scene-1 cards are printed while narrow and widen when focused; xterm reflows every line except the
// cursor line, so those transcripts end with a newline. DAG previews show the last four lines, so the
// scene-2 transcripts keep the cursor on their final line instead.
const trailingNewline = new Set(['implementation', 'regression', 'review', 'docs', 'research'])

export const transcripts: Record<string, string> = Object.fromEntries(
  Object.entries({ implementation, regression, review, docs, research, baseline, planA, planB, vitest })
    .map(([name, lines]) => [name, lines.join('\r\n') + (trailingNewline.has(name) ? '\r\n' : '')])
)

export function widestLine(name: string): number {
  return Math.max(...transcripts[name]!.split('\r\n').map(cells))
}
