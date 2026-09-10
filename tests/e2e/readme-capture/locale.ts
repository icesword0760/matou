// Locale selection for the README capture and the launch-video recorder.
// MATOU_LOCALE=en drives the app in English and writes assets/shots/en; anything else keeps the
// Chinese default. Accessible names and default entity names are read from the app's own catalogs so
// the spec cannot drift from the UI; only demo content that exists nowhere in the app — task titles,
// card names, notification texts — is spelled out in DEMO below.
import { MESSAGES } from '../../../apps/desktop/src/renderer/src/i18n/messages/index'
import { hierarchyEn, hierarchyZhCN } from '../../../apps/runtime/src/i18n/messages/hierarchy'
import { sessionCanvasEn, sessionCanvasZhCN } from '../../../apps/runtime/src/i18n/messages/sessionCanvas'

export type CaptureLocale = 'zh-CN' | 'en'

export const LOCALE: CaptureLocale = process.env.MATOU_LOCALE === 'en' ? 'en' : 'zh-CN'

/** Renderer catalog for the capture locale: every accessible name the spec drives comes from here. */
export const ui = MESSAGES[LOCALE]

/** Runtime catalog for the capture locale: the names new tasks and canvases are created with. */
export const runtime = {
  hierarchy: LOCALE === 'en' ? hierarchyEn : hierarchyZhCN,
  sessionCanvas: LOCALE === 'en' ? sessionCanvasEn : sessionCanvasZhCN
}

interface DemoContent {
  /** Task titles in the shop-api and mobile-app workspaces. */
  tasks: {
    idempotency: string
    pagination: string
    prisma: string
    lcp: string
    checkout: string
    loginAb: string
    ciCache: string
    mobileCrash: string
  }
  /** Canvas (tab) names. */
  canvases: { implement: string; explore: string }
  /** Session card names. */
  cards: {
    implementation: string
    regression: string
    review: string
    docs: string
    coordinate: string
    planA: string
    planB: string
    vitest: string
    baseline: string
    terminal: string
  }
  /** Notification subtitles, keyed by the event type the demo pushes. */
  notificationKind: { completed: string; waiting: string; error: string }
  notificationBody: {
    mobileDone: string
    reviewDone: string
    /** Split so the notification-center button can be matched on a stable leading clause. */
    regressionWaitingLead: string
    regressionWaitingRest: string
    planBWaiting: string
    vitestError: string
  }
  /** The stub's TodoWrite payload, shown by the HUD. */
  todos: [string, string, string, string, string]
  /** `last_assistant_message` of each stub role's Stop hook. */
  summaries: {
    review: string
    docs: string
    coordinate: string
    baseline: string
    baselineThree: string
    aiRead: string
    aiFork: string
  }
  /** Text the coordinate role types into a sibling card through `mt send`. */
  coordinateHandoff: string
  /** Child-card titles the ai-fork role passes to `mt fork children`. */
  forkPlans: [string, string, string]
  /** Markdown fixtures written into the isolated HOME and the demo repository. */
  files: { homeClaudeMd: string; workspaceClaudeMd: string; paymentsDoc: string }
  /** Seeded Claude Code history, listed by the "load a session" dialog. */
  history: Array<Array<['user' | 'assistant', string]>>
}

const DEMO: Record<CaptureLocale, DemoContent> = {
  'zh-CN': {
    tasks: {
      idempotency: '支付回调幂等性',
      pagination: '订单列表分页超时',
      prisma: 'Prisma 6 升级',
      lcp: '首页 LCP 优化',
      checkout: '结算页 500 热修',
      loginAb: '登录页 A/B 实验',
      ciCache: 'CI 缓存修复',
      mobileCrash: '崩溃修复 · iOS 18'
    },
    canvases: { implement: '实现与验证', explore: '方案探索' },
    cards: {
      implementation: '实现 · Redis 幂等键',
      regression: '回归 · 支付模块测试',
      review: '审查 · 方案对比',
      docs: '文档 · 回调约定',
      coordinate: '协调 · 跨卡片',
      planA: '方案 A · Redis SETNX',
      planB: '方案 B · DB 唯一索引',
      vitest: '回归 · vitest',
      baseline: '支付回调幂等性 · 基线',
      terminal: '终端 · mt'
    },
    notificationKind: { completed: '任务完成', waiting: '等待输入', error: '出错' },
    notificationBody: {
      mobileDone: '修复已提交到 fix/ios18-crash，12 个 XCTest 全部通过。',
      reviewDone: 'ADR 已写入 docs/adr/0007，建议方案 A 为主、方案 B 兜底。',
      regressionWaitingLead: '1 个用例与新行为冲突',
      regressionWaitingRest: '，需要确认是否更新断言。',
      planBWaiting: '迁移失败：历史数据有 37 条重复 event_id，是否先写清洗脚本？',
      vitestError: 'vitest 退出码 1：webhook.duplicate.test.ts 有 1 个用例失败。'
    },
    todos: [
      '梳理回调处理链路',
      '选择幂等键存储：Redis SETNX + 24h 过期',
      '入口加幂等校验',
      '为重复回调补测试',
      '更新 docs/payments.md 回调约定'
    ],
    summaries: {
      review: '结论：方案 A（Redis）为主路径，方案 B 唯一索引兜底，ADR 已更新。',
      docs: '文档已更新，和 webhook.ts 里的实现保持一致。',
      coordinate: '已经交给回归卡片了，完成后我再读一次。',
      baseline: '建议分两条路线并行验证：Redis SETNX / DB 唯一索引。',
      baselineThree: '三个幂等方案已列出，等待选择。',
      aiRead: '结论：左边回归 27 个用例通过，1 个与新行为冲突，需要你确认是否更新断言。',
      aiFork: '三张子卡片已创建，DAG 里可以看到三条分支。'
    },
    coordinateHandoff: '改成新行为，并同步 docs/payments.md',
    forkPlans: ['方案 1 · Redis SETNX', '方案 2 · DB 唯一索引', '方案 3 · 去重表'],
    files: {
      homeClaudeMd: '# 全局约定\n\n- 回复使用中文\n',
      workspaceClaudeMd: '# shop-api\n\n- 支付相关改动必须带回归测试\n- 迁移脚本先在 staging 验证\n',
      paymentsDoc: '# 支付回调约定\n'
    },
    history: [
      [
        ['user', '支付回调幂等键设计'],
        ['assistant', '先看当前回调处理的幂等策略，是否已有唯一约束。'],
        ['user', '目前用的是 Redis SETNX，但过期时间设置不统一'],
        ['assistant', '建议统一为 24 小时过期，并给键加上 provider 前缀避免冲突'],
        ['user', 'DB 层要不要也加唯一索引兜底？'],
        ['assistant', '建议加 (provider, event_id) 唯一索引作为最后一道防线'],
        ['assistant', '已整理成 ADR，稍后同步给你']
      ],
      [
        ['user', '订单分页超时排查'],
        ['assistant', '先看慢查询日志，定位是不是 offset 分页导致的'],
        ['user', '是的，offset 越大越慢，P99 到了 8s'],
        ['assistant', '建议改成游标分页，按 (created_at, id) 做 keyset'],
        ['user', '历史数据要不要重新生成索引？'],
        ['assistant', '需要，迁移窗口建议放在低峰期'],
        ['assistant', '迁移脚本已经写好，跑一遍看看效果']
      ],
      [
        ['user', 'Prisma 6 升级评估'],
        ['assistant', '先扫一遍 breaking changes，重点看关系查询语法'],
        ['user', '有没有影响到我们现有的迁移文件？'],
        ['assistant', '迁移文件兼容，但 relationJoins 预览特性要单独开启'],
        ['user', '升级之后测试全过了吗？'],
        ['assistant', '27 个用例通过，1 个跟排序相关的失败，正在看'],
        ['assistant', '已修复排序失败用例，可以合并升级分支']
      ]
    ]
  },
  en: {
    tasks: {
      idempotency: 'Webhook idempotency',
      pagination: 'Order list timeout',
      prisma: 'Prisma 6 upgrade',
      lcp: 'Home page LCP',
      checkout: 'Checkout 500 hotfix',
      loginAb: 'Login A/B test',
      ciCache: 'CI cache fix',
      mobileCrash: 'Crash fix · iOS 18'
    },
    canvases: { implement: 'Implement and verify', explore: 'Plan exploration' },
    cards: {
      implementation: 'Implement · Redis',
      regression: 'Regression · tests',
      review: 'Review · plan A/B',
      docs: 'Docs · contract',
      coordinate: 'Coordinate · cards',
      planA: 'Plan A · Redis SETNX',
      planB: 'Plan B · unique idx',
      vitest: 'Regression · vitest',
      baseline: 'Webhook · baseline',
      terminal: 'Terminal · mt'
    },
    notificationKind: { completed: 'Task complete', waiting: 'Needs input', error: 'Error' },
    notificationBody: {
      mobileDone: 'Fix pushed to fix/ios18-crash, all 12 XCTest cases pass.',
      reviewDone: 'ADR written to docs/adr/0007: Redis as the main path, the unique index as the fallback.',
      regressionWaitingLead: '1 test conflicts with the new behavior',
      regressionWaitingRest: ', confirm whether to update the assertion.',
      planBWaiting: 'Migration failed: 37 duplicate event_id rows in the existing data. Write a cleanup script first?',
      vitestError: 'vitest exited 1: webhook.duplicate.test.ts has 1 failing test.'
    },
    todos: [
      'Map the webhook handling path',
      'Pick the idempotency store: Redis SETNX + 24h expiry',
      'Add the idempotency check at the entry point',
      'Cover repeated callbacks with a test',
      'Update the callback contract in docs/payments.md'
    ],
    summaries: {
      review: 'Verdict: Redis SETNX is the main path, the DB unique index the fallback. ADR updated.',
      docs: 'Docs updated to match the implementation in webhook.ts.',
      coordinate: 'Handed it to the regression card; I will read it again once it finishes.',
      baseline: 'Verify two routes in parallel: Redis SETNX and a DB unique index.',
      baselineThree: 'Three idempotency options listed, waiting for your pick.',
      aiRead: 'Verdict: the regression on the left passed 27 tests; 1 conflicts with the new behavior and needs your call on the assertion.',
      aiFork: 'Three child cards created; the DAG shows all three branches.'
    },
    coordinateHandoff: 'switch to the new behavior and sync docs/payments.md',
    forkPlans: ['Plan 1 · Redis SETNX', 'Plan 2 · DB unique index', 'Plan 3 · Dedupe table'],
    files: {
      homeClaudeMd: '# Global conventions\n\n- Reply in English\n',
      workspaceClaudeMd:
        '# shop-api\n\n- Payment changes ship with a regression test\n- Verify migration scripts on staging first\n',
      paymentsDoc: '# Payment callback contract\n'
    },
    history: [
      [
        ['user', 'Designing the payment idempotency key'],
        ['assistant', 'Let me look at how the callback path guards against replays today.'],
        ['user', 'It uses Redis SETNX, but the expiry is not consistent'],
        ['assistant', 'Settle on a 24 hour expiry and prefix the key with the provider to avoid collisions'],
        ['user', 'Should the database carry a unique index as a fallback?'],
        ['assistant', 'Yes, a (provider, event_id) unique index is the last line of defence'],
        ['assistant', 'Written up as an ADR, I will share it shortly']
      ],
      [
        ['user', 'Order pagination timeouts'],
        ['assistant', 'Start with the slow query log and check whether offset pagination is the cause'],
        ['user', 'It is: the larger the offset the slower it gets, P99 is at 8s'],
        ['assistant', 'Switch to cursor pagination, a keyset on (created_at, id)'],
        ['user', 'Do the existing rows need a new index?'],
        ['assistant', 'They do, and the migration window should land off peak'],
        ['assistant', 'The migration script is ready, run it and see how it behaves']
      ],
      [
        ['user', 'Prisma 6 upgrade assessment'],
        ['assistant', 'Reading the breaking changes first, mainly the relation query syntax'],
        ['user', 'Does anything affect our existing migration files?'],
        ['assistant', 'The migrations are compatible, but relationJoins is a preview feature you enable separately'],
        ['user', 'Did the tests all pass after the upgrade?'],
        ['assistant', '27 tests pass, 1 ordering test fails, looking at it now'],
        ['assistant', 'The ordering failure is fixed, the upgrade branch can be merged']
      ]
    ]
  }
}

export const demo = DEMO[LOCALE]
