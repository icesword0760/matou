// Shared demo-scene builders and UI drivers for readme-capture.spec.ts and tests/e2e/launch-video.
// Every session is a stub `claude` (claude-stub.py); nothing touches the real CLI or account.
import { _electron as electron, expect, type ElectronApplication, type Locator, type Page } from '@playwright/test'
import { execFile } from 'node:child_process'
import { randomUUID } from 'node:crypto'
import { chmod, cp, mkdir, readFile, stat, writeFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { promisify } from 'node:util'
import { transcripts, widestLine } from './transcripts'

const run = promisify(execFile)
export const REPO = resolve(import.meta.dirname, '../../..')
export const WINDOW = { width: 1400, height: 880 }
export type Ids = { workspaceId: string; taskId: string; sceneId: string; sessionId: string }

// ---------- fixtures ----------

export async function prepareHome(home: string, demo: string): Promise<void> {
  await mkdir(join(home, '.claude'), { recursive: true })
  await writeFile(join(home, '.zshrc'), `PROMPT='%F{blue}%~%f %# '\nexport PATH="${join(demo, 'bin')}:$PATH"\n`)
  await writeFile(join(home, '.claude', 'CLAUDE.md'), '# 全局约定\n\n- 回复使用中文\n')
  await writeFile(join(home, '.claude', 'settings.json'), JSON.stringify({
    hooks: { Notification: [], SessionStart: [] }
  }, null, 2))
  await writeFile(join(home, '.claude.json'), JSON.stringify({ mcpServers: { context7: {} } }, null, 2))
}

export async function prepareShopPlatform(dir: string): Promise<void> {
  await prepareRepo(dir, {
    'package.json': JSON.stringify({ name: 'shop-api', private: true, version: '3.14.0' }, null, 2) + '\n',
    'CLAUDE.md': '# shop-api\n\n- 支付相关改动必须带回归测试\n- 迁移脚本先在 staging 验证\n',
    '.mcp.json': JSON.stringify({ mcpServers: { postgres: {}, browser_bridge: {} } }, null, 2) + '\n',
    '.claude/settings.local.json': JSON.stringify({ hooks: { PreToolUse: [], PostToolUse: [], Stop: [] } }, null, 2) + '\n',
    'src/payments/webhook.ts': [
      "import { parseEvent } from './parse-event'",
      "import { paymentService } from './service'",
      '',
      'export async function handleWebhook(req) {',
      '  const event = parseEvent(req.body)',
      '  await paymentService.apply(event)',
      '  return ok({})',
      '}',
      ''
    ].join('\n'),
    'src/payments/apply.ts': 'export async function apply() {}\n',
    'prisma/schema.prisma': 'model PaymentCallback {\n  id       Int    @id\n  eventId  String\n}\n',
    'docs/payments.md': '# 支付回调约定\n'
  })
  await run('git', ['checkout', '-q', '-b', 'feat/webhook-idempotency'], { cwd: dir })
  await writeFile(join(dir, 'src/payments/webhook.ts'), [
    "import { parseEvent } from './parse-event'",
    "import { paymentService } from './service'",
    "import { redis } from '../infra/redis'",
    '',
    'export async function handleWebhook(req) {',
    '  const event = parseEvent(req.body)',
    '  const key = `pay:cb:${event.id}`',
    "  const fresh = await redis.set(key, '1', 'NX', 'EX', 86_400)",
    '  if (!fresh) return ok({ duplicate: true })',
    '  await paymentService.apply(event)',
    '  return ok({})',
    '}',
    ''
  ].join('\n'))
}

export async function prepareRepo(dir: string, files: Record<string, string>): Promise<void> {
  await mkdir(dir, { recursive: true })
  for (const [path, content] of Object.entries(files)) {
    await mkdir(join(dir, path, '..'), { recursive: true })
    await writeFile(join(dir, path), content)
  }
  const env = {
    ...process.env, GIT_AUTHOR_NAME: 'dev', GIT_AUTHOR_EMAIL: 'dev@example.com',
    GIT_COMMITTER_NAME: 'dev', GIT_COMMITTER_EMAIL: 'dev@example.com'
  }
  await run('git', ['init', '-q', '-b', 'main'], { cwd: dir, env })
  await run('git', ['add', '-A'], { cwd: dir, env })
  await run('git', ['commit', '-q', '-m', 'init'], { cwd: dir, env })
}

export async function prepareDemo(demo: string): Promise<void> {
  await mkdir(demo, { recursive: true })
  for (const [name, content] of Object.entries(transcripts)) {
    const widest = widestLine(name)
    if (widest > 56) throw new Error(`transcript ${name} has a ${widest}-cell line`)
    await writeFile(join(demo, `${name}.ans`), content)
  }
  await cp(resolve(import.meta.dirname, 'claude-stub.py'), join(demo, 'claude'))
  await chmod(join(demo, 'claude'), 0o755)
  await mkdir(join(demo, 'bin'), { recursive: true })
  await writeFile(join(demo, 'bin', 'pnpm'), `#!/bin/sh\ncat "${join(demo, 'vitest.ans')}"\nexit 1\n`)
  await chmod(join(demo, 'bin', 'pnpm'), 0o755)
  await writeFile(join(demo, 'roles.queue'),
    ['implementation', 'regression', 'review', 'docs', 'coordinate', 'planA1', 'planB1', 'baseline', 'planA', 'planB'].join('\n') + '\n')
  const day = 86_400
  const minute = 60_000
  const base = { model: 'Claude Opus 5', weekly: 41, resets_in: 3 * day + 5 * 3600 }
  const todos = [
    { content: '梳理回调处理链路', status: 'completed' },
    { content: '选择幂等键存储：Redis SETNX + 24h 过期', status: 'completed' },
    { content: '入口加幂等校验', status: 'in_progress' },
    { content: '为重复回调补测试', status: 'pending' },
    { content: '更新 docs/payments.md 回调约定', status: 'pending' }
  ]
  const waiting = ['hook', 'Notification', { message: 'Claude is waiting for your input' }]
  const planA = {
    ...base, transcript: 'planA', permission: 'acceptEdits', context: 41, duration_ms: 16 * minute,
    events: [
      ['hook', 'UserPromptSubmit', {}],
      ['tool', 'Edit', 'a-1', { file_path: 'src/payments/webhook.ts' }, 'ok'],
      ['tool', 'Bash', 'a-2', { command: 'pnpm vitest run src/payments' }, 'running']
    ]
  }
  const planB = {
    ...base, transcript: 'planB', permission: 'acceptEdits', context: 37, duration_ms: 14 * minute,
    events: [
      ['hook', 'UserPromptSubmit', {}],
      ['tool', 'Edit', 'b-1', { file_path: 'prisma/schema.prisma' }, 'ok'],
      ['tool', 'Bash', 'b-2', { command: 'pnpm prisma migrate dev --name callback-event-unique' }, 'fail'],
      ['hook', 'Notification', { message: 'Error: migration failed with P2002 unique constraint' }]
    ]
  }
  const regression = {
    ...base, transcript: 'regression', permission: 'default', context: 34, duration_ms: 12 * minute,
    events: [
      ['hook', 'UserPromptSubmit', {}],
      ['tool', 'Bash', 'reg-1', { command: 'pnpm vitest run src/payments' }, 'ok'],
      waiting
    ]
  }
  await writeFile(join(demo, 'roles.json'), JSON.stringify({
    implementation: {
      ...base, transcript: 'implementation', permission: 'acceptEdits', context: 62, duration_ms: 47 * minute,
      events: [
        ['hook', 'UserPromptSubmit', {}],
        ['tool', 'Read', 'impl-1', { file_path: 'src/payments/webhook.ts' }, 'ok'],
        ['tool', 'TodoWrite', 'impl-3', { todos }, 'ok'],
        ['tool', 'Edit', 'impl-4', { file_path: 'src/payments/webhook.ts' }, 'ok'],
        ['tool', 'Bash', 'impl-5', { command: 'pnpm vitest run src/payments --reporter=dot' }, 'running']
      ]
    },
    regression,
    review: {
      ...base, transcript: 'review', permission: 'acceptEdits', context: 51, duration_ms: 23 * minute,
      events: [
        ['hook', 'UserPromptSubmit', {}],
        ['tool', 'Read', 'rev-1', { file_path: 'prisma/schema.prisma' }, 'ok'],
        ['tool', 'Write', 'rev-2', { file_path: 'docs/adr/0007-idempotency.md' }, 'ok'],
        ['hook', 'UserPromptSubmit', {}],
        ['tool', 'Bash', 'rev-3', { command: 'mt read left --lines 12' }, 'ok'],
        ['hook', 'Stop', { last_assistant_message: '结论：方案 A（Redis）为主路径，方案 B 唯一索引兜底，ADR 已更新。' }]
      ]
    },
    docs: {
      ...base, transcript: 'docs', permission: 'acceptEdits', context: 22, duration_ms: 6 * minute,
      events: [
        ['hook', 'UserPromptSubmit', {}],
        ['tool', 'Read', 'doc-1', { file_path: 'docs/payments.md' }, 'ok'],
        ['tool', 'Edit', 'doc-2', { file_path: 'docs/payments.md' }, 'ok'],
        ['hook', 'Stop', { last_assistant_message: '文档已更新，和 webhook.ts 里的实现保持一致。' }]
      ]
    },
    coordinate: {
      ...base, transcript: 'coordinate', permission: 'acceptEdits', context: 19, duration_ms: 5 * minute,
      events: [
        ['hook', 'UserPromptSubmit', {}],
        ['tool', 'Bash', 'co-1', { command: 'mt list' }, 'ok'],
        ['tool', 'Bash', 'co-2', { command: 'mt read sibling:2 --lines 8' }, 'ok'],
        ['hook', 'UserPromptSubmit', {}],
        ['tool', 'Bash', 'co-3', { command: 'mt send sibling:2 "改成新行为，并同步 docs/payments.md" --enter' }, 'ok'],
        ['hook', 'Stop', { last_assistant_message: '已经交给回归卡片了，完成后我再读一次。' }]
      ]
    },
    baseline: {
      ...base, transcript: 'baseline', permission: 'default', context: 18, duration_ms: 9 * minute,
      events: [
        ['hook', 'UserPromptSubmit', {}],
        ['tool', 'Read', 'base-1', { file_path: 'src/payments/webhook.ts' }, 'ok'],
        ['hook', 'Stop', { last_assistant_message: '建议分两条路线并行验证：Redis SETNX / DB 唯一索引。' }]
      ]
    },
    planA1: planA,
    planB1: planB,
    planA,
    planB,
    'baseline-three': {
      ...base, transcript: 'baseline-three', permission: 'default', context: 21, duration_ms: 7 * minute,
      events: [['hook', 'UserPromptSubmit', {}], ['hook', 'Stop', { last_assistant_message: '三个幂等方案已列出，等待选择。' }]]
    },
    'ai-read': {
      ...base, transcript: 'ai-read', permission: 'acceptEdits', context: 12, duration_ms: 2 * minute,
      events: [
        ['hook', 'UserPromptSubmit', {}],
        ['exec', 'mt read left --lines 12', 'mt read left --lines 12'],
        ['hook', 'Stop', { last_assistant_message: '结论：左边回归 27 个用例通过，1 个与新行为冲突，需要你确认是否更新断言。' }]
      ]
    },
    'ai-fork': {
      ...base, transcript: 'ai-fork', permission: 'acceptEdits', context: 15, duration_ms: 3 * minute,
      events: [
        ['hook', 'UserPromptSubmit', {}],
        ['exec', `mt fork children self --items-json '${JSON.stringify([
          { itemKey: 'redis', title: '方案 1 · Redis SETNX', environment: { mode: 'current' } },
          { itemKey: 'unique', title: '方案 2 · DB 唯一索引', environment: { mode: 'current' } },
          { itemKey: 'dedupe', title: '方案 3 · 去重表', environment: { mode: 'current' } }
        ])}' --json`, 'mt fork children self --items-json …'],
        ['hook', 'Stop', { last_assistant_message: '三张子卡片已创建，DAG 里可以看到三条分支。' }]
      ]
    }
  }, null, 2))
}

// Seeds three fake Claude Code session transcripts into the isolated HOME's history storage so the
// "载入 Claude Code 会话" dialog has believable sessions to list. The row shape mirrors what the real
// Claude Code CLI writes and what ClaudeSessionCatalog (apps/runtime/src/session/claude-session-catalog.ts)
// reads back: one JSON object per line, keyed by `type`/`sessionId`/`cwd`/`timestamp`/`message`.
export async function seedClaudeHistory(home: string, workspace: string): Promise<void> {
  const projectDirectory = join(home, '.claude', 'projects', encodeClaudeProjectPath(workspace))
  await mkdir(projectDirectory, { recursive: true })
  const sessions: Array<{ startedAt: string; turns: Array<['user' | 'assistant', string]> }> = [
    {
      startedAt: '2026-08-25T09:00:00.000Z',
      turns: [
        ['user', '支付回调幂等键设计'],
        ['assistant', '先看当前回调处理的幂等策略，是否已有唯一约束。'],
        ['user', '目前用的是 Redis SETNX，但过期时间设置不统一'],
        ['assistant', '建议统一为 24 小时过期，并给键加上 provider 前缀避免冲突'],
        ['user', 'DB 层要不要也加唯一索引兜底？'],
        ['assistant', '建议加 (provider, event_id) 唯一索引作为最后一道防线'],
        ['assistant', '已整理成 ADR，稍后同步给你']
      ]
    },
    {
      startedAt: '2026-08-28T09:00:00.000Z',
      turns: [
        ['user', '订单分页超时排查'],
        ['assistant', '先看慢查询日志，定位是不是 offset 分页导致的'],
        ['user', '是的，offset 越大越慢，P99 到了 8s'],
        ['assistant', '建议改成游标分页，按 (created_at, id) 做 keyset'],
        ['user', '历史数据要不要重新生成索引？'],
        ['assistant', '需要，迁移窗口建议放在低峰期'],
        ['assistant', '迁移脚本已经写好，跑一遍看看效果']
      ]
    },
    {
      startedAt: '2026-08-31T09:00:00.000Z',
      turns: [
        ['user', 'Prisma 6 升级评估'],
        ['assistant', '先扫一遍 breaking changes，重点看关系查询语法'],
        ['user', '有没有影响到我们现有的迁移文件？'],
        ['assistant', '迁移文件兼容，但 relationJoins 预览特性要单独开启'],
        ['user', '升级之后测试全过了吗？'],
        ['assistant', '27 个用例通过，1 个跟排序相关的失败，正在看'],
        ['assistant', '已修复排序失败用例，可以合并升级分支']
      ]
    }
  ]
  for (const session of sessions) {
    const sessionId = randomUUID()
    const startedAt = Date.parse(session.startedAt)
    const rows = session.turns.map(([role, text], index) => JSON.stringify({
      type: role,
      sessionId,
      cwd: workspace,
      timestamp: new Date(startedAt + index * 60_000).toISOString(),
      ...(role === 'user' ? { permissionMode: 'default' } : {}),
      message: role === 'assistant'
        ? { role, model: 'claude-opus-4-6', content: [{ type: 'text', text }] }
        : { role, content: text }
    }))
    await writeFile(join(projectDirectory, `${sessionId}.jsonl`), rows.join('\n') + '\n')
  }
}

// Mirrors apps/runtime/src/session/claude-session-catalog.ts's encodeClaudeProjectPath so seeded
// sessions land in the same directory the runtime scans for the active workspace's history.
function encodeClaudeProjectPath(cwd: string): string {
  return resolve(cwd).replace(/[^A-Za-z0-9]/g, '-')
}

export async function launch(input: { root: string; home: string; workspace: string; demo: string }): Promise<ElectronApplication> {
  const dataDirectory = join(input.root, 'data')
  const userData = join(input.root, 'electron-user-data')
  await mkdir(userData, { recursive: true })
  return electron.launch({
    args: [join(REPO, 'apps/desktop'), `--user-data-dir=${userData}`],
    env: {
      ...process.env,
      HOME: input.home,
      SHELL: '/bin/zsh',
      ZDOTDIR: input.home,
      CLAUDE_CONFIG_DIR: join(input.home, '.claude'),
      MATOU_E2E: '1',
      MATOU_DATA_DIR: dataDirectory,
      MATOU_DEFAULT_WORKSPACE: input.workspace,
      ELECTRON_USER_DATA_DIR: userData,
      MATOU_RUNTIME_ENTRY: join(REPO, 'apps/runtime/dist/index.cjs'),
      MATOU_CLAUDE_COMMAND: join(input.demo, 'claude'),
      MATOU_DEMO_ROOT: input.demo,
      MATOU_DISABLE_AUTO_UPDATE: '1'
    }
  })
}

export async function placeWindow(app: ElectronApplication, zoom: number, size = WINDOW): Promise<void> {
  const placement = await app.evaluate(({ BrowserWindow, screen }, { zoom, size }) => {
    const window = BrowserWindow.getAllWindows()[0]!
    const area = screen.getDisplayMatching(window.getBounds()).workArea
    window.setBounds({
      x: area.x + Math.max(0, Math.floor((area.width - size.width) / 2)),
      y: area.y + Math.max(0, Math.floor((area.height - size.height) / 2)),
      width: Math.min(size.width, area.width),
      height: Math.min(size.height, area.height)
    })
    window.webContents.setZoomFactor(zoom)
    return { area, bounds: window.getBounds(), displays: screen.getAllDisplays().map((d) => ({ label: d.label, internal: d.internal, workArea: d.workArea, scale: d.scaleFactor })) }
  }, { zoom, size })
  console.log('window placement', JSON.stringify(placement))
}

// The DAG opens as its own window; give it the main window's frame so recorded frames share one size.
export async function alignDagWindow(app: ElectronApplication, zoom: number): Promise<void> {
  await app.evaluate(({ BrowserWindow }, zoom) => {
    const windows = BrowserWindow.getAllWindows()
    const dag = windows.find((w) => w.webContents.getURL().includes('kind=dag'))!
    const main = windows.find((w) => !w.webContents.getURL().includes('kind=dag'))!
    dag.setBounds(main.getBounds())
    dag.webContents.setZoomFactor(zoom)
  }, zoom)
}

// Captures the composited window at the display's physical resolution (the zoomed CSS viewport
// confuses Playwright's own screenshot sizing).
export async function captureWindow(app: ElectronApplication, which: 'main' | 'dag', path: string): Promise<void> {
  const size = await app.evaluate(async ({ BrowserWindow }, { which, path }) => {
    const isDag = (w: Electron.BrowserWindow) => w.webContents.getURL().includes('kind=dag')
    const window = BrowserWindow.getAllWindows().find((w) => (which === 'dag') === isDag(w))!
    const image = await window.webContents.capturePage()
    const fs = process.getBuiltinModule('node:fs') as typeof import('node:fs')
    fs.writeFileSync(path, image.toPNG())
    return image.getSize()
  }, { which, path })
  console.log(`captured ${path} ${size.width}x${size.height}`)
}

export async function resizeWindow(app: ElectronApplication, height: number): Promise<void> {
  await app.evaluate(({ BrowserWindow }, height) => {
    const window = BrowserWindow.getAllWindows()[0]!
    window.setBounds({ ...window.getBounds(), height })
  }, height)
}

// Pans the DAG so the bounding box of all node cards sits in the middle of the window.
export async function centerDagGraph(dag: Page): Promise<void> {
  const box = await dag.evaluate(() => {
    const rects = [...document.querySelectorAll('.dag-node-card')].map((node) => node.getBoundingClientRect())
    const left = Math.min(...rects.map((r) => r.left))
    const right = Math.max(...rects.map((r) => r.right))
    const top = Math.min(...rects.map((r) => r.top))
    const bottom = Math.max(...rects.map((r) => r.bottom))
    return { cx: (left + right) / 2, cy: (top + bottom) / 2, width: window.innerWidth, height: window.innerHeight }
  })
  const startX = box.width / 2
  const startY = box.height - 30
  await dag.mouse.move(startX, startY)
  await dag.mouse.down()
  await dag.mouse.move(startX + (box.width / 2 - box.cx), startY + (box.height / 2 - box.cy), { steps: 12 })
  await dag.mouse.up()
}

export async function focusCard(surface: Locator): Promise<void> {
  await surface.scrollIntoViewIfNeeded()
  await surface.click({ position: { x: 12, y: 12 } })
  await expect(paneOf(surface)).toHaveAttribute('data-active', 'true')
}

// Records a window at up to `fps` while the caller drives the UI between holds. Each frame keeps its
// real duration (capturePage takes ~100 ms) so the encoded animation plays back at true speed.
export function frameRecorder(app: ElectronApplication, dir: string, fps = 10) {
  const frames: Array<{ path: string; capturedAt: number }> = []
  const interval = 1000 / fps
  return {
    async hold(ms: number, which: 'main' | 'dag' = 'main') {
      await mkdir(dir, { recursive: true })
      const until = Date.now() + ms
      while (Date.now() < until) {
        const startedAt = Date.now()
        const path = join(dir, `frame-${String(frames.length).padStart(4, '0')}.png`)
        await app.evaluate(async ({ BrowserWindow }, { path, which }) => {
          const isDag = (w: Electron.BrowserWindow) => w.webContents.getURL().includes('kind=dag')
          const window = BrowserWindow.getAllWindows().find((w) => (which === 'dag') === isDag(w))!
          const image = await window.webContents.capturePage()
          const fs = process.getBuiltinModule('node:fs') as typeof import('node:fs')
          fs.writeFileSync(path, image.toPNG())
        }, { path, which })
        frames.push({ path, capturedAt: startedAt })
        const remaining = interval - (Date.now() - startedAt)
        if (remaining > 0) await new Promise((resolve) => setTimeout(resolve, remaining))
      }
    },
    async writeConcatList(): Promise<string> {
      const lines: string[] = []
      frames.forEach((frame, index) => {
        const next = frames[index + 1]
        const duration = next ? (next.capturedAt - frame.capturedAt) / 1000 : 1.5
        lines.push(`file '${frame.path}'`, `duration ${duration.toFixed(3)}`)
      })
      lines.push(`file '${frames.at(-1)!.path}'`)
      const list = join(dir, 'frames.txt')
      await writeFile(list, lines.join('\n') + '\n')
      return list
    }
  }
}

export async function encodeAnimation(concatList: string, gifPath: string, mp4Path: string): Promise<void> {
  const input = ['-f', 'concat', '-safe', '0', '-i', concatList]
  const scratch = join(concatList, '..')
  const intermediate = join(scratch, 'animation.mov')
  const palette = join(scratch, 'palette.png')
  // Drop the static frames inside each hold (mpdecimate) but keep their real timing through a
  // lossless intermediate; a single filtergraph loses the variable frame durations in GIF output.
  await run('ffmpeg', ['-y', '-loglevel', 'error', ...input, '-vf', 'mpdecimate=hi=768:lo=320:frac=0.33,scale=1000:-1:flags=lanczos', '-fps_mode', 'vfr', '-c:v', 'png', intermediate])
  await run('ffmpeg', ['-y', '-loglevel', 'error', '-i', intermediate, '-vf', 'palettegen=max_colors=256:stats_mode=full', palette])
  await run('ffmpeg', ['-y', '-loglevel', 'error', '-i', intermediate, '-i', palette, '-lavfi', 'paletteuse=dither=none:diff_mode=rectangle', '-fps_mode', 'vfr', '-loop', '0', gifPath])
  await run('ffmpeg', ['-y', '-loglevel', 'error', ...input, '-vf', 'scale=1400:-2:flags=lanczos,fps=15', '-c:v', 'libx264', '-pix_fmt', 'yuv420p', '-crf', '23', '-movflags', '+faststart', mp4Path])
  const sizes = await Promise.all([gifPath, mp4Path].map(async (path) => `${path} ${(await stat(path)).size} bytes`))
  console.log(sizes.join('\n'))
}

// ---------- UI helpers ----------

export function visibleSurfaces(page: Page): Locator {
  return page.locator('.scene-stage:not([hidden]) [data-testid="terminal-pane"]:visible .terminal-surface')
}

export function activeSurface(page: Page): Locator {
  return page.locator('.scene-stage:not([hidden]) [data-testid="terminal-pane"][data-active="true"]:visible .terminal-surface')
}

export async function stableSurface(surface: Locator): Promise<Locator> {
  const sessionId = await surface.getAttribute('data-session-id')
  if (!sessionId) throw new Error('Terminal Session identity is missing')
  return surface.page().locator(`.terminal-surface[data-session-id="${sessionId}"]`)
}

export async function sessionIds(page: Page): Promise<string[]> {
  return page.locator('.terminal-surface').evaluateAll((elements) =>
    elements.map((element) => element.getAttribute('data-session-id') ?? '').filter(Boolean))
}

export async function newSurfaceAfter(page: Page, action: () => Promise<unknown>): Promise<Locator> {
  const before = new Set(await sessionIds(page))
  await action()
  let created = ''
  await expect.poll(async () => {
    const fresh = (await sessionIds(page)).filter((id) => !before.has(id))
    created = fresh[0] ?? ''
    return fresh.length
  }, { timeout: 30_000 }).toBe(1)
  return page.locator(`.terminal-surface[data-session-id="${created}"]`)
}

export function paneOf(surface: Locator): Locator {
  return surface.locator('xpath=ancestor::*[@data-testid="terminal-pane"][1]')
}

export async function waitForShell(surface: Locator): Promise<void> {
  await expect(surface).toHaveAttribute('data-pid', /[1-9][0-9]*/)
}

export async function terminalCommand(surface: Locator, command: string): Promise<void> {
  const sessionId = await surface.getAttribute('data-session-id')
  if (!sessionId) throw new Error('Terminal Session identity is missing')
  const page = surface.page()
  const stable = page.locator(`.terminal-surface[data-session-id="${sessionId}"]`)
  await waitForShell(stable)
  const pane = paneOf(stable)
  const textarea = stable.locator('.xterm-helper-textarea')
  if (await pane.getAttribute('data-active') !== 'true') {
    await stable.scrollIntoViewIfNeeded()
    await stable.click({ position: { x: 12, y: 12 } })
  }
  await textarea.focus()
  await expect(pane).toHaveAttribute('data-active', 'true')
  await expect(textarea).toBeFocused()
  await page.waitForTimeout(50)
  await textarea.pressSequentially(command, { delay: 2 })
  await textarea.press('Enter')
}

export async function promoteToClaude(surface: Locator, demo: string): Promise<void> {
  const sessionId = await surface.getAttribute('data-session-id')
  const stable = surface.page().locator(`.terminal-surface[data-session-id="${sessionId}"]`)
  const before = await launches(demo)
  await terminalCommand(stable, 'claude')
  await expect(stable).toHaveAttribute('data-profile', 'claude-code')
  await expect.poll(() => launches(demo)).toBe(before + 1)
  await surface.page().waitForTimeout(900)
}

export async function waitForRole(demo: string, role: string): Promise<void> {
  await expect.poll(async () => (await readFile(join(demo, 'launches.log'), 'utf8').catch(() => '')).includes(`"role": "${role}"`)).toBe(true)
}

export async function launches(demo: string): Promise<number> {
  const log = await readFile(join(demo, 'launches.log'), 'utf8').catch(() => '')
  return log.split('\n').filter(Boolean).length
}

export async function renameSession(page: Page, surface: Locator, title: string): Promise<void> {
  const pane = paneOf(surface)
  await pane.locator('.pane-title').scrollIntoViewIfNeeded()
  await pane.locator('.pane-title').click({ button: 'right' })
  await page.getByRole('menuitem', { name: '重命名…' }).click()
  await page.getByRole('textbox', { name: '会话名称' }).fill(title)
  await page.getByRole('button', { name: '确定' }).click()
  await expect(pane.locator('.pane-title')).toHaveText(title)
}

export async function renameTask(page: Page, from: string, to: string): Promise<void> {
  await page.getByRole('button', { name: `事项菜单：${from}` }).click()
  await page.getByRole('menuitem', { name: '重命名' }).click()
  await page.getByRole('textbox', { name: '事项名称' }).fill(to)
  await page.getByRole('button', { name: '确定' }).click()
  await expect(page.getByTestId('active-task')).toHaveText(to)
}

export async function selectTask(page: Page, title: string): Promise<void> {
  await page.locator('.workspace-group.is-active .workbench-item', { hasText: title }).first().click()
  await expect(page.getByTestId('active-task')).toHaveText(title)
}

export async function renameActiveTab(page: Page, name: string): Promise<void> {
  await page.locator('.tab-item.active .tab-title').dblclick()
  await page.getByRole('textbox', { name: '页签名称' }).fill(name)
  await page.getByRole('button', { name: '确定' }).click()
  await expect(page.locator('.tab-item.active .tab-title')).toHaveText(name)
}

export async function forkChild(page: Page, pane: Locator, title: string, branch: string): Promise<void> {
  const button = pane.getByRole('button', { name: `从“${title}”创建子分支` })
  await expect(button).not.toHaveAttribute('aria-disabled', 'true')
  await button.click()
  await fillForkDialog(page, branch)
}

export async function forkSibling(page: Page, pane: Locator, title: string, branch: string): Promise<void> {
  const button = pane.getByRole('button', { name: `从共同父会话创建“${title}”的兄弟分支` })
  await expect(button).not.toHaveAttribute('aria-disabled', 'true')
  await button.click()
  await fillForkDialog(page, branch)
}

export async function fillForkDialog(page: Page, branch: string): Promise<void> {
  await page.getByLabel('分支名称').fill(branch)
  await expect(page.getByRole('radio', { name: /使用当前工作树/ })).toBeChecked()
  await page.getByRole('button', { name: '创建分支', exact: true }).click()
  await expect(page.getByLabel('分支名称')).toHaveCount(0)
}

export async function hierarchyIds(page: Page, surface: Locator): Promise<Ids> {
  const workspaceId = await page.locator('.workspace-group.is-active').getAttribute('data-workspace-id')
  const taskTestId = await page.locator('.workbench-item.is-active').getAttribute('data-testid')
  const sceneId = await page.locator('.tab-item.active').getAttribute('data-scene-id')
  const sessionId = await surface.getAttribute('data-session-id')
  if (!workspaceId || !taskTestId || !sceneId || !sessionId) throw new Error('Hierarchy identity is missing')
  return { workspaceId, taskId: taskTestId.replace(/^task-/, ''), sceneId, sessionId }
}

export async function moveTask(page: Page, title: string, column: string): Promise<void> {
  // The board keeps the dragged id in React state, so each drag event needs its own task turn.
  const steps: Array<['card' | 'column', string]> = [
    ['card', 'dragstart'], ['column', 'dragenter'], ['column', 'dragover'], ['column', 'drop'], ['card', 'dragend']
  ]
  for (const [on, type] of steps) {
    await page.evaluate(({ title, column, on, type }) => {
      const card = document.querySelector<HTMLElement>(`article.board-task-card[aria-label="${title}"]`)
      const target = document.querySelector<HTMLElement>(`section[aria-label="${column}列"]`)
      if (!card || !target) throw new Error(`board card or column missing: ${title} → ${column}`)
      const scope = window as unknown as { __readmeDragData?: DataTransfer }
      scope.__readmeDragData ??= new DataTransfer()
      const element = on === 'card' ? card : target
      element.dispatchEvent(new DragEvent(type, { bubbles: true, cancelable: true, dataTransfer: scope.__readmeDragData }))
      if (type === 'dragend') delete scope.__readmeDragData
    }, { title, column, on, type })
    await page.waitForTimeout(60)
  }
  await expect(page.locator(`section[aria-label="${column}列"] article.board-task-card[aria-label="${title}"]`)).toBeVisible()
}

export function stageRecorder(page: Page, root: string) {
  const dir = process.env.MATOU_STAGE_DIR
  return async (name: string) => {
    if (!dir) return
    await mkdir(dir, { recursive: true })
    await page.locator('.hierarchy-shell').screenshot({ path: join(dir, `${name}.png`) }).catch(() => {})
    console.log(`stage ${name} captured (root ${root})`)
  }
}
