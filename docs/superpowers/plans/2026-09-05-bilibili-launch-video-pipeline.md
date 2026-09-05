# 码头 B 站发布视频全自动流水线 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 从仓库一键产出 6 到 7 分钟的 1080p B 站发布视频（合成配音、自动驱动真实码头 App、点击推近、字幕）以及 60 秒短版和封面图。

**Architecture:** 三段流水线。`tts/`（edge-tts 逐段配音，产出 mp3 + 字幕 cue）→ `tests/e2e/launch-video/record.spec.ts`（Playwright 驱动隔离环境里的真实码头，主进程用 `beginFrameSubscription` 以 30fps 恒定帧率把合成帧写入 ffmpeg，同时记录点击与章节事件，操作节奏由配音 cue 驱动）→ `marketing/launch-video/`（Remotion 读取 manifest，按事件生成推近镜头与光标，叠字幕、片头片尾，渲染 mp4）。

**Tech Stack:** Node 22.16、pnpm 10、Playwright 1.62 + Electron 43（仓库已有）、ffmpeg 6（`/opt/homebrew/bin/ffmpeg`）、edge-tts（`~/.local/bin/edge-tts`，uv 安装）、Remotion 4.0.520、React 19、vitest。

**Spec:** `docs/superpowers/specs/2026-09-05-bilibili-launch-video-design.md`

## Global Constraints

- 配音音色 `zh-CN-YunxiNeural`，语速 `--rate=-8%`。
- 说辞正文以 `marketing/launch-video/script/narration.json` 为唯一来源，内容等于 `~/Downloads/matou-tts-samples/说辞v2.txt`（不提 Codex；cmux / Ghostty 段落用软化版）。
- 录制窗口 1504×846 CSS 像素（精确 16:9），放在内建 Retina 屏（缩放 2），采集帧 3008×1692。
- 成片 1920×1080、30fps、H.264 yuv420p；Remotion 合成尺寸 1920×1080。
- `marketing/launch-video/` 不加入 `pnpm-workspace.yaml`，用自己的 `package.json` 与 `node_modules`。
- 生成物（`public/recordings/`、`public/audio/`、`out/`）进 `.gitignore`。
- 本计划不含 git 提交步骤：本会话规则是用户要求时才提交；每个任务完成后向用户报告，由用户决定是否提交。
- 所有 Playwright 录制脚本必须用环境变量门禁（`MATOU_LAUNCH_VIDEO=1`），否则 `pnpm test:e2e` 会误跑。

---

## 文件结构

```
tests/e2e/readme-capture/
  demo-scene.ts              # 从 readme-capture.spec.ts 抽出的场景搭建与 UI 操作函数（导出）
  readme-capture.spec.ts     # 只剩测试主体，import demo-scene
  claude-stub.py             # 新增 exec 事件：真实执行 mt 命令并回显
  transcripts.ts             # 新增视频用转录：resume-target、ai-read、ai-fork
tests/e2e/launch-video/
  recorder.ts                # ClipRecorder：帧订阅 → ffmpeg 管道；事件日志
  cues.ts                    # 读取配音 cue，按关键词求时间
  capture-spike.spec.ts      # 采集吞吐验证（任务 2）
  record.spec.ts             # 正式逐段录制（任务 5）
marketing/launch-video/
  package.json  tsconfig.json  remotion.config.ts  .gitignore  README.md
  script/narration.json      # 十段说辞
  tts/synthesize.mjs         # edge-tts → public/audio/*.mp3 + *.cues.json + manifest.json
  public/audio/              # 生成物
  public/recordings/         # 生成物：<section>.mp4 / <section>.events.json / manifest.json
  src/index.ts  src/Root.tsx  src/manifest.ts
  src/LaunchVideo.tsx        # 按段排序
  src/SectionScene.tsx       # 一段：视频片段 + 推近 + 光标 + 字幕 + 配音
  src/zoom.ts  src/zoom.test.ts
  src/Cursor.tsx  src/Captions.tsx
  src/Intro.tsx  src/Outro.tsx  src/Cover.tsx  src/Short60.tsx
  out/                       # 渲染结果
```

---

### Task 1: 抽出 README 演示场景为可复用模块

**Files:**
- Create: `tests/e2e/readme-capture/demo-scene.ts`
- Modify: `tests/e2e/readme-capture/readme-capture.spec.ts`（删除被移走的函数，改为 import）

**Interfaces:**
- Produces（全部 `export`）：`REPO: string`、`WINDOW: {width:number;height:number}`、`type Ids`、`prepareHome(home, demo)`、`prepareShopPlatform(dir)`、`prepareRepo(dir, files)`、`prepareDemo(demo)`、`launch({root, home, workspace, demo})`、`placeWindow(app, zoom, size = WINDOW)`、`alignDagWindow(app, zoom)`、`captureWindow(app, which, path)`、`resizeWindow(app, height)`、`centerDagGraph(dag)`、`focusCard(surface)`、`frameRecorder(app, dir, fps)`、`encodeAnimation(...)`、`visibleSurfaces(page)`、`activeSurface(page)`、`stableSurface(surface)`、`sessionIds(page)`、`newSurfaceAfter(page, action)`、`paneOf(surface)`、`waitForShell(surface)`、`terminalCommand(surface, command)`、`promoteToClaude(surface, demo)` 以及 spec 里 `// ---------- UI helpers ----------` 之后的其余函数（`renameTask`、`selectTask`、`renameActiveTab`、`renameSession`、`hierarchyIds`、`forkChild`、`forkSibling`、`waitForRole`、`moveTask`、`stageRecorder`）。

- [ ] **Step 1: 看清要移动的范围**

Run: `grep -n "^async function\|^function\|^const [A-Z_]* =\|^type \|^// ----------" tests/e2e/readme-capture/readme-capture.spec.ts`
Expected: 列出 `// ---------- fixtures ----------`（约 330 行）之后的所有函数；`WINDOW`、`ZOOM`、`REPO`、`SHOTS`、`HOLD`、`type Ids` 在文件头部。

- [ ] **Step 2: 新建 demo-scene.ts**

把 spec 中 `// ---------- fixtures ----------` 起到文件末尾的全部函数剪切到 `tests/e2e/readme-capture/demo-scene.ts`，文件头部写：

```ts
// Shared demo-scene builders and UI drivers for readme-capture.spec.ts and tests/e2e/launch-video.
// Every session is a stub `claude` (claude-stub.py); nothing touches the real CLI or account.
import { _electron as electron, expect, type ElectronApplication, type Locator, type Page } from '@playwright/test'
import { execFile } from 'node:child_process'
import { chmod, cp, mkdir, readFile, stat, writeFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'
import { promisify } from 'node:util'
import { transcripts, widestLine } from './transcripts'

const run = promisify(execFile)
export const REPO = resolve(import.meta.dirname, '../../..')
export const WINDOW = { width: 1400, height: 880 }
export type Ids = { workspaceId: string; taskId: string; sceneId: string; sessionId: string }
```

每个被移动的函数前加 `export`。`placeWindow` 签名改为：

```ts
export async function placeWindow(app: ElectronApplication, zoom: number, size = WINDOW): Promise<void> {
  const placement = await app.evaluate(({ BrowserWindow, screen }, { zoom, size }) => {
    // ...原实现不变，只是 size 来自参数
  }, { zoom, size })
  console.log('window placement', JSON.stringify(placement))
}
```

若某函数引用了 spec 顶部的 `ZOOM`、`SHOTS`、`HOLD`，把该值作为参数传入（例如 `stageRecorder(page, root, enabled = process.env.MATOU_STAGE_SHOTS === '1')`）。

- [ ] **Step 3: 修改 spec 的 import**

删除 spec 里已移走的定义，头部改为：

```ts
import { expect, test, type ElectronApplication, type Locator } from '@playwright/test'
import { mkdir, mkdtemp, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import {
  REPO, WINDOW, type Ids, prepareHome, prepareShopPlatform, prepareRepo, prepareDemo, launch, placeWindow,
  alignDagWindow, captureWindow, resizeWindow, centerDagGraph, focusCard, frameRecorder, encodeAnimation,
  visibleSurfaces, activeSurface, stableSurface, newSurfaceAfter, paneOf, waitForShell, terminalCommand,
  promoteToClaude, renameTask, selectTask, renameActiveTab, renameSession, hierarchyIds, forkChild, forkSibling,
  waitForRole, moveTask, stageRecorder
} from './demo-scene'
```

保留 `SHOTS`、`ZOOM`、`HOLD`、`BOARD_HEIGHT` 常量在 spec 内。删掉不再使用的 import（`electron`、`execFile` 等），否则 TypeScript 报未使用。

- [ ] **Step 4: 类型检查**

Run: `npx tsc --noEmit --module esnext --moduleResolution bundler --target es2022 --skipLibCheck --strict --types node tests/e2e/readme-capture/*.ts`
Expected: 无输出（0 错误）。若报 `import.meta.dirname` 类型缺失，加 `--lib es2022,dom` 与 `--module nodenext`。

- [ ] **Step 5: 回归运行 README 捕获（输出到临时目录，不覆盖 assets）**

Run: `MATOU_README_CAPTURE=1 MATOU_SHOTS_DIR=/tmp/matou-shots npx playwright test tests/e2e/readme-capture --workers=1 --reporter=line`
Expected: `1 passed`；`ls /tmp/matou-shots` 有 5 个文件（4 png + gif/mp4）。耗时约 5 分钟。

---

### Task 2: 帧订阅录制器与吞吐验证

**Files:**
- Create: `tests/e2e/launch-video/recorder.ts`
- Create: `tests/e2e/launch-video/capture-spike.spec.ts`

**Interfaces:**
- Produces:
  ```ts
  export type RecorderEvent =
    | { t: number; type: 'click'; x: number; y: number; label?: string }
    | { t: number; type: 'move'; x: number; y: number; ms: number }
    | { t: number; type: 'source'; source: 'main' | 'dag' }
    | { t: number; type: 'mark'; name: string }
  export interface RecorderStats { written: number; duplicated: number; dropped: number; resized: number; width: number; height: number; fps: number; startedAt: number; stoppedAt: number }
  export class ClipRecorder {
    constructor(app: ElectronApplication, fps?: number)
    start(outputPath: string): Promise<void>          // 首帧到达后启动 ffmpeg；t=0 为第一帧写入时刻
    stop(eventsPath: string): Promise<RecorderStats>  // 结束 ffmpeg，写 events json
    now(): number                                      // 相对录制起点的毫秒
    mark(name: string): void
    setSource(source: 'main' | 'dag'): Promise<void>
    moveTo(page: Page, x: number, y: number, ms?: number): Promise<void>
    click(locator: Locator, label?: string, position?: { x: number; y: number }): Promise<void>
    waitUntil(ms: number): Promise<void>
    hold(ms: number): Promise<void>
  }
  ```
  坐标全部是 CSS 像素（Playwright 视口坐标），events json 里另存 `scale`（设备缩放）供 Remotion 换算。

- [ ] **Step 1: 写 recorder.ts**

```ts
import type { ElectronApplication, Locator, Page } from '@playwright/test'
import { writeFile } from 'node:fs/promises'

export type RecorderEvent =
  | { t: number; type: 'click'; x: number; y: number; label?: string }
  | { t: number; type: 'move'; x: number; y: number; ms: number }
  | { t: number; type: 'source'; source: 'main' | 'dag' }
  | { t: number; type: 'mark'; name: string }

export interface RecorderStats {
  written: number; duplicated: number; dropped: number; resized: number
  width: number; height: number; fps: number; startedAt: number; stoppedAt: number
}

export class ClipRecorder {
  private events: RecorderEvent[] = []
  private startedAt = 0
  private scale = 2
  private viewport = { width: 0, height: 0 }

  constructor(private readonly app: ElectronApplication, private readonly fps = 30) {}

  async start(outputPath: string): Promise<void> {
    const started = await this.app.evaluate(async ({ BrowserWindow, screen }, { outputPath, fps }) => {
      const cp = process.getBuiltinModule('node:child_process') as typeof import('node:child_process')
      type Win = Electron.BrowserWindow
      const isDag = (w: Win) => w.webContents.getURL().includes('kind=dag')
      const state = {
        latest: {} as Record<string, Electron.NativeImage | undefined>,
        active: 'main' as 'main' | 'dag',
        subscribed: new Set<Win>(),
        written: 0, duplicated: 0, dropped: 0, resized: 0,
        lastWritten: undefined as Electron.NativeImage | undefined,
        timer: undefined as NodeJS.Timeout | undefined,
        ff: undefined as ReturnType<typeof cp.spawn> | undefined,
        size: { width: 0, height: 0 },
        startedAt: 0,
        writable: true
      }
      const subscribe = (which: 'main' | 'dag') => {
        const win = BrowserWindow.getAllWindows().find((w) => (which === 'dag') === isDag(w))
        if (!win || state.subscribed.has(win)) return
        state.subscribed.add(win)
        win.webContents.beginFrameSubscription(false, (image) => { state.latest[which] = image })
        win.webContents.invalidate()
      }
      subscribe('main')
      await new Promise<void>((resolve) => {
        const check = () => (state.latest.main ? resolve() : setTimeout(check, 16))
        check()
      })
      state.size = state.latest.main!.getSize()
      state.ff = cp.spawn('ffmpeg', [
        '-y', '-loglevel', 'error',
        '-f', 'rawvideo', '-pix_fmt', 'bgra', '-s', `${state.size.width}x${state.size.height}`, '-r', String(fps), '-i', 'pipe:0',
        '-c:v', 'libx264', '-preset', 'veryfast', '-crf', '16', '-pix_fmt', 'yuv420p', '-movflags', '+faststart', outputPath
      ], { stdio: ['pipe', 'ignore', 'inherit'] })
      state.ff.stdin!.on('drain', () => { state.writable = true })
      const expected = state.size.width * state.size.height * 4
      const tick = () => {
        const image = state.latest[state.active] ?? state.latest.main
        if (!image) return
        if (!state.writable) { state.dropped += 1; return }
        let frame = image
        if (frame.getSize().width !== state.size.width || frame.getSize().height !== state.size.height) {
          frame = frame.resize({ width: state.size.width, height: state.size.height }); state.resized += 1
        }
        const bitmap = frame.toBitmap()
        if (bitmap.length !== expected) { state.dropped += 1; return }
        if (image === state.lastWritten) state.duplicated += 1
        state.lastWritten = image
        state.writable = state.ff!.stdin!.write(bitmap)
        state.written += 1
      }
      state.startedAt = Date.now()
      tick()
      state.timer = setInterval(tick, 1000 / fps)
      const win = BrowserWindow.getAllWindows().find((w) => !isDag(w))!
      const scale = screen.getDisplayMatching(win.getBounds()).scaleFactor
      const [width, height] = win.getContentSize()
      ;(globalThis as Record<string, unknown>).__matouRecorder = { state, subscribe }
      return { startedAt: state.startedAt, scale, viewport: { width, height } }
    }, { outputPath, fps: this.fps })
    this.startedAt = started.startedAt
    this.scale = started.scale
    this.viewport = started.viewport
    this.events = []
  }

  async stop(eventsPath: string): Promise<RecorderStats> {
    const stats = await this.app.evaluate(async () => {
      const { state } = (globalThis as Record<string, any>).__matouRecorder
      clearInterval(state.timer)
      for (const win of state.subscribed) { try { win.webContents.endFrameSubscription() } catch { /* window gone */ } }
      await new Promise<void>((resolve) => { state.ff.once('close', () => resolve()); state.ff.stdin.end() })
      const stoppedAt = Date.now()
      delete (globalThis as Record<string, unknown>).__matouRecorder
      return {
        written: state.written, duplicated: state.duplicated, dropped: state.dropped, resized: state.resized,
        width: state.size.width, height: state.size.height, startedAt: state.startedAt, stoppedAt
      }
    })
    const full: RecorderStats = { ...stats, fps: this.fps }
    await writeFile(eventsPath, JSON.stringify({ ...full, scale: this.scale, viewport: this.viewport, events: this.events }, null, 2))
    return full
  }

  now(): number { return Date.now() - this.startedAt }
  mark(name: string): void { this.events.push({ t: this.now(), type: 'mark', name }) }

  async setSource(source: 'main' | 'dag'): Promise<void> {
    await this.app.evaluate((_electron, source) => {
      const recorder = (globalThis as Record<string, any>).__matouRecorder
      recorder.subscribe(source)
      recorder.state.active = source
    }, source)
    this.events.push({ t: this.now(), type: 'source', source })
  }

  async moveTo(page: Page, x: number, y: number, ms = 350): Promise<void> {
    this.events.push({ t: this.now(), type: 'move', x, y, ms })
    await page.mouse.move(x, y, { steps: Math.max(2, Math.round(ms / 16)) })
  }

  async click(locator: Locator, label?: string, position?: { x: number; y: number }): Promise<void> {
    await locator.scrollIntoViewIfNeeded()
    const box = await locator.boundingBox()
    if (!box) throw new Error(`no bounding box for ${label ?? locator.toString()}`)
    const x = box.x + (position?.x ?? box.width / 2)
    const y = box.y + (position?.y ?? box.height / 2)
    await this.moveTo(locator.page(), x, y)
    this.events.push({ t: this.now(), type: 'click', x, y, ...(label ? { label } : {}) })
    await locator.page().mouse.click(x, y)
  }

  async waitUntil(ms: number): Promise<void> {
    const remaining = ms - this.now()
    if (remaining > 0) await new Promise((resolve) => setTimeout(resolve, remaining))
  }

  async hold(ms: number): Promise<void> { await new Promise((resolve) => setTimeout(resolve, ms)) }
}
```

- [ ] **Step 2: 写吞吐验证 spec**

```ts
// tests/e2e/launch-video/capture-spike.spec.ts
// Run: MATOU_LAUNCH_VIDEO=1 npx playwright test tests/e2e/launch-video/capture-spike --workers=1 --reporter=line
import { execFile } from 'node:child_process'
import { mkdtemp, readFile, rm } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { promisify } from 'node:util'
import { expect, test } from '@playwright/test'
import { launch, placeWindow, prepareDemo, prepareHome, prepareRepo, prepareShopPlatform, renameTask, waitForShell, activeSurface } from '../readme-capture/demo-scene'
import { ClipRecorder } from './recorder'

const run = promisify(execFile)
const VIDEO_WINDOW = { width: 1504, height: 846 }
test.setTimeout(180_000)

test('records 10 seconds at 30fps without dropping frames', async () => {
  test.skip(process.env.MATOU_LAUNCH_VIDEO !== '1', 'set MATOU_LAUNCH_VIDEO=1')
  const root = await mkdtemp(join(tmpdir(), 'matou-spike-'))
  const home = join(root, 'home'); const workspace = join(home, 'work', 'shop-api'); const demo = join(root, 'demo')
  await prepareHome(home, demo); await prepareShopPlatform(workspace); await prepareDemo(demo)
  const app = await launch({ root, home, workspace, demo })
  try {
    const page = await app.firstWindow()
    await placeWindow(app, 1, VIDEO_WINDOW)
    await expect(page.getByTestId('active-task')).toHaveText('默认')
    const recorder = new ClipRecorder(app, 30)
    const clip = join(root, 'spike.mp4'); const events = join(root, 'spike.events.json')
    await recorder.start(clip)
    // keep the UI busy: create tasks while recording
    for (const title of ['A 事项', 'B 事项', 'C 事项']) {
      await recorder.click(page.getByRole('button', { name: '在 shop-api 中新增事项' }), 'new-task')
      await renameTask(page, '新事项', title)
      await waitForShell(activeSurface(page))
      await recorder.hold(1500)
    }
    await recorder.waitUntil(10_000)
    const stats = await recorder.stop(events)
    console.log('stats', JSON.stringify(stats))
    expect(stats.width).toBe(VIDEO_WINDOW.width * 2)
    expect(stats.height).toBe(VIDEO_WINDOW.height * 2)
    expect(stats.dropped).toBe(0)
    const seconds = (stats.stoppedAt - stats.startedAt) / 1000
    expect(stats.written).toBeGreaterThanOrEqual(Math.floor(seconds * 30 * 0.95))
    const { stdout } = await run('ffprobe', ['-v', 'error', '-select_streams', 'v:0', '-show_entries', 'stream=width,height,pix_fmt,r_frame_rate,nb_frames', '-of', 'json', clip])
    const stream = JSON.parse(stdout).streams[0]
    expect(stream.pix_fmt).toBe('yuv420p')
    expect(stream.r_frame_rate).toBe('30/1')
    expect(Number(stream.nb_frames)).toBe(stats.written)
    const log = JSON.parse(await readFile(events, 'utf8'))
    expect(log.events.filter((e: { type: string }) => e.type === 'click')).toHaveLength(3)
    expect(log.scale).toBe(2)
  } finally {
    await app.close().catch(() => {})
    if (!process.env.MATOU_KEEP_DEMO_ROOT) await rm(root, { recursive: true, force: true })
  }
})
```

- [ ] **Step 3: 运行验证**

Run: `MATOU_LAUNCH_VIDEO=1 MATOU_KEEP_DEMO_ROOT=1 npx playwright test tests/e2e/launch-video/capture-spike --workers=1 --reporter=line`
Expected: `1 passed`；控制台 `stats` 里 `dropped: 0`，`written ≥ 285`。

- [ ] **Step 4: 目检画质**

Run: `ffmpeg -y -v error -i <root>/spike.mp4 -vf "select=eq(n\,150)" -frames:v 1 /tmp/spike-frame.png && open /tmp/spike-frame.png`
Expected: 3008×1692，终端字体边缘锐利，无色带。若 `dropped > 0` 或 CPU 撑不住：先把 `-preset veryfast` 改 `ultrafast`，仍不行则把 `fps` 降为 24（`ClipRecorder` 构造参数），并同步改 Remotion 合成的 fps 为 24。

---

### Task 3: 说辞与配音生成

**Files:**
- Create: `marketing/launch-video/package.json`、`tsconfig.json`、`.gitignore`、`README.md`
- Create: `marketing/launch-video/script/narration.json`
- Create: `marketing/launch-video/tts/synthesize.mjs`

**Interfaces:**
- Produces: `public/audio/<id>.mp3`、`public/audio/<id>.cues.json`（`Array<{ startMs: number; endMs: number; text: string }>`）、`public/audio/manifest.json`：
  ```json
  { "voice": "zh-CN-YunxiNeural", "rate": "-8%", "sections": [ { "id": "why", "durationMs": 31240, "audio": "audio/why.mp3", "cues": "audio/why.cues.json" } ] }
  ```

- [ ] **Step 1: 初始化工程**

```bash
mkdir -p marketing/launch-video/{script,tts,public/audio,public/recordings,src,out}
cd marketing/launch-video
cat > package.json <<'JSON'
{
  "name": "matou-launch-video",
  "private": true,
  "type": "module",
  "scripts": {
    "tts": "node tts/synthesize.mjs",
    "record": "cd ../.. && MATOU_LAUNCH_VIDEO=1 npx playwright test tests/e2e/launch-video/record --workers=1 --reporter=line",
    "studio": "remotion studio src/index.ts",
    "render": "remotion render src/index.ts LaunchVideo out/matou-launch-1080p.mp4 --codec h264 --crf 18",
    "render:short": "remotion render src/index.ts Short60 out/matou-launch-60s.mp4 --codec h264 --crf 18",
    "cover": "remotion still src/index.ts Cover out/cover.png",
    "test": "vitest run"
  }
}
JSON
npm install remotion@4.0.520 @remotion/cli@4.0.520 react@19 react-dom@19
npm install -D typescript @types/react @types/react-dom @types/node vitest
printf 'node_modules/\nout/\npublic/audio/\npublic/recordings/\n' > .gitignore
```

`tsconfig.json`：

```json
{ "compilerOptions": { "target": "ES2022", "module": "ESNext", "moduleResolution": "Bundler", "jsx": "react-jsx", "strict": true, "resolveJsonModule": true, "skipLibCheck": true, "noEmit": true, "types": ["node"] }, "include": ["src", "tts"] }
```

- [ ] **Step 2: 写 narration.json**

十段，`text` 逐字复制 `~/Downloads/matou-tts-samples/说辞v2.txt` 对应段落（该文件每个空行分隔一段，顺序：intro、why、structure、focus、persist、fork-dag、ai-control、board-notify、model-switch、outro）：

```json
[
  { "id": "intro", "title": "开场", "text": "你同时开过几个 Claude Code？……做了一个码头。" },
  { "id": "why", "title": "为什么做", "text": "市面上不缺好用的终端工具……是正在推进的工作。" },
  { "id": "structure", "title": "四层结构与 HUD", "text": "结构分四层。……也不用问它\"你现在做到哪了\"。" },
  { "id": "focus", "title": "焦点伸缩", "text": "卡片多了以后……有点像手机的多任务切换。" },
  { "id": "persist", "title": "持久化与再入会话", "text": "所有会话都是持久化的。……点一下就载入。" },
  { "id": "fork-dag", "title": "Fork 与 DAG", "text": "重点来了。……你的鼠标基本不用离开卡片区域。" },
  { "id": "ai-control", "title": "让 AI 操作码头", "text": "这是我最想给你看的部分。……DAG 上同时长出三条线。" },
  { "id": "board-notify", "title": "看板与通知", "text": "项目做大之后……点一下回到事发现场。" },
  { "id": "model-switch", "title": "模型切换", "text": "还有一个很多人关心的……随时切。" },
  { "id": "outro", "title": "收尾", "text": "说说没做好的。……下期见。" }
]
```

上面省略号处必须填完整原文。校验：`node -e "const s=require('./script/narration.json');console.log(s.length, s.map(x=>x.text).join('').replace(/\s/g,'').length)"` 期望 `10 1569`（与 `说辞v2.txt` 去空白后字数一致，容许 ±3）。

- [ ] **Step 3: 写 synthesize.mjs**

```js
// marketing/launch-video/tts/synthesize.mjs
import { execFile } from 'node:child_process'
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { homedir } from 'node:os'
import { join, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { promisify } from 'node:util'

const run = promisify(execFile)
const here = dirname(fileURLToPath(import.meta.url))
const root = join(here, '..')
const EDGE_TTS = process.env.EDGE_TTS ?? join(homedir(), '.local/bin/edge-tts')
const VOICE = 'zh-CN-YunxiNeural'
const RATE = '-8%'

const sections = JSON.parse(await readFile(join(root, 'script/narration.json'), 'utf8'))
const audioDir = join(root, 'public/audio')
await mkdir(audioDir, { recursive: true })

const parseVtt = (vtt) => {
  const cues = []
  const stamp = (s) => { const [h, m, rest] = s.trim().split(':'); const [sec, ms] = rest.split(/[.,]/); return ((+h * 60 + +m) * 60 + +sec) * 1000 + +ms }
  for (const block of vtt.split(/\n\s*\n/)) {
    const lines = block.trim().split('\n')
    const timing = lines.find((l) => l.includes('-->'))
    if (!timing) continue
    const [from, to] = timing.split('-->')
    const text = lines.slice(lines.indexOf(timing) + 1).join(' ').trim()
    if (text) cues.push({ startMs: stamp(from), endMs: stamp(to), text })
  }
  return cues
}

const manifest = { voice: VOICE, rate: RATE, sections: [] }
for (const section of sections) {
  const txt = join(audioDir, `${section.id}.txt`)
  const mp3 = join(audioDir, `${section.id}.mp3`)
  const vtt = join(audioDir, `${section.id}.vtt`)
  await writeFile(txt, section.text)
  await run(EDGE_TTS, ['--voice', VOICE, `--rate=${RATE}`, '-f', txt, '--write-media', mp3, '--write-subtitles', vtt])
  const cues = parseVtt(await readFile(vtt, 'utf8'))
  await writeFile(join(audioDir, `${section.id}.cues.json`), JSON.stringify(cues, null, 2))
  const { stdout } = await run('ffprobe', ['-v', 'error', '-show_entries', 'format=duration', '-of', 'csv=p=0', mp3])
  const durationMs = Math.round(parseFloat(stdout) * 1000)
  manifest.sections.push({ id: section.id, title: section.title, durationMs, audio: `audio/${section.id}.mp3`, cues: `audio/${section.id}.cues.json` })
  console.log(`${section.id}: ${(durationMs / 1000).toFixed(1)}s, ${cues.length} cues`)
}
await writeFile(join(audioDir, 'manifest.json'), JSON.stringify(manifest, null, 2))
console.log('total', (manifest.sections.reduce((n, s) => n + s.durationMs, 0) / 1000).toFixed(1), 's')
```

- [ ] **Step 4: 运行并验收**

Run: `cd marketing/launch-video && npm run tts`
Expected: 十行 `<id>: xx.xs, n cues`，`total` 在 280 到 340 秒之间；`public/audio/manifest.json` 有 10 段；`ffplay public/audio/intro.mp3` 可听。

---

### Task 4: Claude 替身支持真实执行 mt 命令，补视频用转录与历史会话种子

**Files:**
- Modify: `tests/e2e/readme-capture/claude-stub.py`
- Modify: `tests/e2e/readme-capture/transcripts.ts`
- Modify: `tests/e2e/readme-capture/demo-scene.ts`（`prepareDemo` 新增角色；新增 `seedClaudeHistory(home, workspace)`）

**Interfaces:**
- Consumes: `mt` 命令行（`apps/runtime/dist/mt-cli.cjs` 的用法）：`mt read left --lines 12`、`mt fork children self --items-json JSON --json`，items 形如 `{ itemKey, title, environment: { mode: 'current' } }`。
- Produces：roles.json 事件新增类型 `['exec', '<shell command>', '<echo label>']`：替身先打印 `⏺ Bash(<echo label>)`，真实执行命令，把 stdout 前 12 行以 `⎿` 缩进打印，再继续后续事件。新增角色 `ai-read`（读取左侧卡片）、`ai-fork`（批量 fork 三个方案）、`baseline-three`（给出三方案并停止）。新增 `seedClaudeHistory(home, workspace)`：往 `<home>/.claude/projects/<encoded workspace>/` 写 3 个可被「载入 Claude Code 会话」对话框识别的 jsonl 会话文件。

- [ ] **Step 1: 看清「载入会话」对话框读取的文件格式**

Run: `grep -n "seedResumableProviderSession" -A40 tests/e2e/fixtures/ai-host-control-fixture.ts | head -80`
Expected: 看到 jsonl 的写入路径规则（`.claude/projects/` 下按工作目录编码的目录名）与每行字段（`type`、`sessionId`、`cwd`、`message.content` 等）。`seedClaudeHistory` 直接调用或复刻这段逻辑，写三个会话：标题分别为「支付回调幂等键设计」「订单分页超时排查」「Prisma 6 升级评估」，每个 6 到 8 条消息，中文内容。

- [ ] **Step 2: 替身增加 exec 事件**

在 `claude-stub.py` 的事件循环里，`kind == 'tool'` 分支之前加：

```python
    if kind == 'exec':
        _, command, label = event
        sys.stdout.write(f"\x1b[32m⏺\x1b[0m \x1b[1mBash({label})\x1b[0m\n")
        sys.stdout.flush()
        result = subprocess.run(command, shell=True, capture_output=True, text=True, env=os.environ)
        lines = (result.stdout or result.stderr).splitlines()[:12]
        for index, line in enumerate(lines):
            prefix = '  \x1b[90m⎿\x1b[0m  ' if index == 0 else '     '
            sys.stdout.write(prefix + line + '\n')
        sys.stdout.flush()
        time.sleep(0.4)
        continue
```

文件顶部 `import subprocess`。`mt` 在托管 shell 的 PATH 上（runtime 的 control-assets），替身由该 shell 启动，因此可直接调用。

- [ ] **Step 3: 新增转录与角色**

`transcripts.ts` 新增三段（沿用现有 `welcome`、`prompt`、`say`、`tool` 帮助函数，行宽不超过 56 格）：

```ts
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
```

把 `baselineThree`、`aiRead`、`aiFork` 加入 `transcripts` 导出对象（键名 `'baseline-three'`、`'ai-read'`、`'ai-fork'`）。

`demo-scene.ts` 的 `prepareDemo` 里 roles.json 新增：

```ts
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
```

`ai-read` 的 Stop 消息在替身打印完 `mt read` 的真实输出之后，还要把结论打出来：在替身 `exec` 分支之后的 `hook('Stop')` 处理里，若 `extra` 含 `last_assistant_message`，额外 `sys.stdout.write(say + message)`（用 `\x1b[38;5;214m⏺\x1b[0m ` 前缀）。roles.queue 的顺序由任务 5 的录制脚本自行写入（`prepareDemo` 保持原队列不变，录制脚本在需要时覆写 `roles.queue`）。

- [ ] **Step 4: 单独验证 exec 分支**

Run（隔离环境里手工跑一次替身）：
```bash
ROOT=$(mktemp -d) && mkdir -p $ROOT && printf 'ai-read\n' > $ROOT/roles.queue && \
node -e "require('fs').writeFileSync('$ROOT/roles.json', JSON.stringify({'ai-read':{transcript:'ai-read',permission:'default',model:'x',duration_ms:1,context:1,weekly:1,resets_in:1,events:[['exec','echo hello-from-mt','mt read left']]}}))" && \
printf 'x' > $ROOT/ai-read.ans && printf '{"hooks":{"UserPromptSubmit":[{"hooks":[{"url":"http://127.0.0.1:9/never"}]}]}}' > $ROOT/settings.json && \
MATOU_DEMO_ROOT=$ROOT python3 tests/e2e/readme-capture/claude-stub.py --settings $ROOT/settings.json </dev/null 2>&1 | cat -v | head -20
```
Expected: 输出里有 `Bash(mt read left)` 和缩进的 `hello-from-mt`（hook 请求会因端口不可达报错退出，那发生在 exec 之后，不影响本验证；若报错先于 exec，把 `hook('SessionStart')` 那几行包进 `try/except urllib.error.URLError: pass`）。

- [ ] **Step 5: 类型检查**

Run: `npx tsc --noEmit --module esnext --moduleResolution bundler --target es2022 --skipLibCheck --strict --types node tests/e2e/readme-capture/*.ts`
Expected: 0 错误。

---

### Task 5: 逐段录制脚本（配音 cue 驱动节奏）

**Files:**
- Create: `tests/e2e/launch-video/cues.ts`
- Create: `tests/e2e/launch-video/record.spec.ts`

**Interfaces:**
- Consumes: `marketing/launch-video/public/audio/manifest.json` 与 `*.cues.json`（任务 3）；`ClipRecorder`（任务 2）；`demo-scene.ts`（任务 1、4）。
- Produces: `marketing/launch-video/public/recordings/<section>[-<part>].mp4`、同名 `.events.json`、`public/recordings/manifest.json`：
  ```json
  { "fps": 30, "scale": 2, "viewport": { "width": 1504, "height": 846 },
    "sections": [ { "id": "structure", "clips": [ { "file": "recordings/structure.mp4", "events": "recordings/structure.events.json", "startAtMs": 0, "durationMs": 74000 } ] },
                  { "id": "persist", "clips": [ { "file": "recordings/persist-a.mp4", "events": "recordings/persist-a.events.json", "startAtMs": 0, "durationMs": 9800 },
                                                 { "file": "recordings/persist-b.mp4", "events": "recordings/persist-b.events.json", "startAtMs": 9800, "durationMs": 26000 } ] } ] }
  ```
  `startAtMs` 是该片段在本段时间轴上的起点；片段内事件时间相对片段起点。

- [ ] **Step 1: 写 cues.ts**

```ts
import { readFile } from 'node:fs/promises'
import { join, resolve } from 'node:path'

const AUDIO = resolve(import.meta.dirname, '../../../marketing/launch-video/public/audio')
export interface Cue { startMs: number; endMs: number; text: string }

export async function loadCues(sectionId: string): Promise<{ cues: Cue[]; durationMs: number }> {
  const manifest = JSON.parse(await readFile(join(AUDIO, 'manifest.json'), 'utf8'))
  const section = manifest.sections.find((s: { id: string }) => s.id === sectionId)
  if (!section) throw new Error(`no narration for section ${sectionId}; run npm run tts first`)
  const cues = JSON.parse(await readFile(join(AUDIO, `${sectionId}.cues.json`), 'utf8')) as Cue[]
  return { cues, durationMs: section.durationMs }
}

// Time (ms) when the narration reaches the first cue containing `keyword`.
export function cueTime(cues: Cue[], keyword: string, offsetMs = 0): number {
  const cue = cues.find((c) => c.text.replace(/\s/g, '').includes(keyword.replace(/\s/g, '')))
  if (!cue) throw new Error(`keyword "${keyword}" not found in cues: ${cues.map((c) => c.text).join(' | ')}`)
  return Math.max(0, cue.startMs + offsetMs)
}
```

- [ ] **Step 2: 写 record.spec.ts 骨架与公共段落函数**

```ts
// tests/e2e/launch-video/record.spec.ts
// Run: cd marketing/launch-video && npm run record   (needs `npm run tts` first and pnpm build at repo root)
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { expect, test, type ElectronApplication, type Page } from '@playwright/test'
import * as scene from '../readme-capture/demo-scene'
import { ClipRecorder } from './recorder'
import { cueTime, loadCues } from './cues'

const OUT = resolve(import.meta.dirname, '../../../marketing/launch-video/public/recordings')
const VIDEO_WINDOW = { width: 1504, height: 846 }
const TAIL_MS = 1500
test.setTimeout(0)

type Clip = { file: string; events: string; startAtMs: number; durationMs: number }
const manifest: { fps: number; scale: number; viewport: typeof VIDEO_WINDOW; sections: Array<{ id: string; clips: Clip[] }> } =
  { fps: 30, scale: 2, viewport: VIDEO_WINDOW, sections: [] }

async function recordClip(app: ElectronApplication, sectionId: string, part: string | undefined, startAtMs: number,
  body: (rec: ClipRecorder) => Promise<void>): Promise<Clip> {
  const name = part ? `${sectionId}-${part}` : sectionId
  const rec = new ClipRecorder(app, 30)
  await rec.start(join(OUT, `${name}.mp4`))
  await body(rec)
  const stats = await rec.stop(join(OUT, `${name}.events.json`))
  const clip: Clip = { file: `recordings/${name}.mp4`, events: `recordings/${name}.events.json`, startAtMs, durationMs: stats.stoppedAt - stats.startedAt }
  console.log(`clip ${name}: ${clip.durationMs} ms, dropped ${stats.dropped}, resized ${stats.resized}`)
  expect(stats.dropped).toBe(0)
  return clip
}

function addSection(id: string, ...clips: Clip[]): void { manifest.sections.push({ id, clips }) }
```

- [ ] **Step 3: 场景搭建（在 test 内、录制前完成，不录）**

按 `readme-capture.spec.ts` 的顺序搭出：shop-api 工作空间、7 个事项、画布「实现与验证」五张卡片（implementation / regression / review / docs / coordinate 角色）、画布「方案探索」（baseline + 方案 A + 方案 B + 回归 shell）、mobile-app 工作空间、通知。差异：

1. `placeWindow(app, 1, VIDEO_WINDOW)`。
2. `prepareDemo` 之后覆写队列：`await writeFile(join(demo, 'roles.queue'), ['implementation','regression','review','docs','coordinate','planA1','planB1','baseline','planA','planB','baseline-three','ai-read','ai-fork'].join('\n') + '\n')`。
3. 调用 `scene.seedClaudeHistory(home, workspace)`（任务 4）。
4. 搭完后把焦点放回「实现与验证」第一张卡片，鼠标移到 `(5, 500)`。

- [ ] **Step 4: 逐段录制**

每段模式相同：`const { cues, durationMs } = await loadCues(id)`；在 `recordClip` 的 body 里用 `await rec.waitUntil(cueTime(cues, '关键词'))` 等到说辞念到关键词再操作；最后 `await rec.waitUntil(durationMs + TAIL_MS)`。关键词取说辞原文里的短语（`cueTime` 会去空白匹配）：

| 段 | 关键词 → 操作 |
|---|---|
| why | 起点：`rec.mark('overview')`；`工作空间` 无操作；全程只有 `rec.moveTo` 缓慢扫过五张卡片（每 6 秒一次） |
| structure | `工作空间对应` → `rec.click(page.locator('.workspace-group.is-active .workspace-group__toggle'), 'workspace')`；`事项对应` → `rec.click(page.getByRole('button', { name: /支付回调幂等性/ }).first(), 'task')`；`画布是事项里的阶段` → `rec.click(page.getByRole('tab').first(), 'tab')`，再 `rec.click(page.getByRole('tab').last(), 'tab')`，回到第一个；`每张卡片是` → `rec.click(visibleSurfaces(page).nth(1), 'card', { x: 12, y: 12 })`；`每张卡片下面有一条` → `rec.mark('hud')` 并 `rec.moveTo(page, 300, 830)`；`切换卡片` → 依次 `rec.click` 第 0、2 张卡片 |
| focus | `你聚焦哪张` → 依次点击第 1、3、0、2 张卡片，每次间隔 `cueTime` 后 +1.2s |
| persist | 两段。`persist-a`：`关掉再打开` 前 300ms 停止录制 → `app.evaluate(({app}) => app.quit())`、`app.close()` → 重新 `launch` 同一 root（数据目录相同）→ `placeWindow` → 等 `visibleSurfaces` 数量恢复到 5 → `persist-b` 从 `startAtMs = cueTime(cues,'关掉再打开')` 开始，`rec.waitUntil` 用段内时间减去 `startAtMs`；`想接着一个以前的` → `rec.click(page.getByRole('button', { name: '横向新增 Shell' }))`，`waitForShell`，`rec.click(page.getByRole('button', { name: /载入 Claude Code 会话到/ }), 'load')`；`左边是历史会话` → `rec.click(dialog.getByRole('button', { name: /预览会话：支付回调幂等键设计/ }), 'pick')`；`点一下就载入` → `rec.click(dialog.getByRole('button', { name: '载入到当前卡片' }), 'load-confirm')` |
| fork-dag | 切到「方案探索」页签，新建卡片提升为 `baseline-three` 角色（`promoteToClaude`，`waitForRole(demo,'baseline-three')`），命名「幂等方案 · 讨论」；`点一下 Fork` → `rec.click(paneOf(surface).getByRole('button', { name: /Fork/ }), 'fork')` 然后按 `forkChild` 里的后续步骤完成（若 Fork 是菜单项，先 `grep -n "Fork\|分叉\|派生" apps/desktop/src/renderer/src/hierarchy/*.tsx` 找按钮名）；`按 Option 加 Tab` → `rec.click(page.getByRole('button', { name: '打开会话 DAG' }), 'dag')`，`alignDagWindow`，`rec.setSource('dag')`，`centerDagGraph`；`方案 A 还在跑` → `rec.moveTo` 到方案 A 节点、方案 B 节点、回归节点（用 `dag.getByRole('button', { name: '打开会话：方案 B · DB 唯一索引' })` 的 boundingBox）；`点节点直接落到` → `rec.click(该按钮, 'dag-node')`，`rec.setSource('main')`；`往父方向一拖` → 先 `grep -rn "返回父会话\|swipe\|deltaX" apps/desktop/src/renderer/src/hierarchy/TerminalPane.tsx apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx` 确认手势实现：若是横向滚轮手势，用 `page.mouse.wheel(-600, 0)` 分 10 步；否则 `rec.click(page.getByRole('button', { name: '返回父会话' }), 'back')`；`会闪一个蓝框` → `window.matouE2e.pushNotification` 给方案 B 的 ids 推一条 `waiting` |
| ai-control | 回到「实现与验证」；`你可以直接说` → 在最右卡片（coordinate 角色旁）新建 Shell 并 `promoteToClaude` 为 `ai-read`（队列顺序保证），替身会自行打印、执行 `mt read left`；`rec.mark('mt-read')`；`再进一步` → 切到「方案探索」，聚焦「幂等方案 · 讨论」卡片，新建子卡片 `promoteToClaude` 为 `ai-fork`，等 `visibleSurfaces` 数量 +3；`DAG 上同时长出三条线` → 打开 DAG，`setSource('dag')`，`centerDagGraph`，停 3 秒，关闭 DAG 回 main |
| board-notify | `每个工作空间都有一个看板` → `rec.click(page.getByRole('button', { name: '看板' }), 'board')`；`拖一下就行` → `scene.moveTask(page, '支付回调幂等性', '阻塞')` 前后各 `rec.mark`；`通知按来源` → 关看板，`rec.click(page.getByRole('button', { name: '通知中心' }), 'notify')`；`点一下回到事发现场` → `rec.click(page.getByRole('button', { name: /打开通知：1 个用例与新行为冲突/ }), 'notify-open')` |
| model-switch | 先 `grep -n "settingsActive" apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx` 找触发按钮的 `aria-label`（预期类似「设置」或「模型切换」）；`码头内置了供应商切换` → `rec.click(settingsButton, 'settings')`；`填上地址和密钥` → `rec.click(page.getByRole('button', { name: '新增供应商' }))`，`page.getByLabel('供应商名称').fill('DeepSeek')`，`page.getByLabel('默认模型').fill('deepseek-v4-pro')`，`page.getByLabel('API 地址').fill('https://api.deepseek.com/anthropic')`，`page.getByLabel('API Key').fill('sk-demo-not-a-real-key')`，`rec.click(dialog.getByRole('button', { name: /保存|添加/ }), 'save')`；`随时切` → `rec.click(page.getByRole('button', { name: '切换到 DeepSeek' }), 'switch')`；结束 `rec.click(page.getByRole('button', { name: '关闭设置' }))` |

`intro` 与 `outro` 不录制（Remotion 合成），但 manifest 仍写入空 `clips: []`。

- [ ] **Step 5: 写 manifest 并收尾**

test 末尾：`await mkdir(OUT, { recursive: true }); await writeFile(join(OUT, 'manifest.json'), JSON.stringify(manifest, null, 2))`；`finally` 里退出 app、按 `MATOU_KEEP_DEMO_ROOT` 决定是否删 root。

- [ ] **Step 6: 运行**

Run: `cd marketing/launch-video && npm run record`
Expected: `1 passed`；`public/recordings/` 里 9 个 mp4（persist 两段）+ 同名 events + manifest.json；每个 `clip` 日志 `dropped 0`；每段 `durationMs ≥ 该段配音 durationMs + 1500`。

- [ ] **Step 7: 抽查时序**

Run: `node -e "const e=require('./public/recordings/fork-dag.events.json');console.log(e.events.filter(x=>x.type!=='move'))"` 与 `cat public/audio/fork-dag.cues.json | grep -n Fork`
Expected: `fork` 点击的 `t` 落在包含「点一下 Fork」的 cue 的 `startMs` 到 `startMs+1000` 之间。

---

### Task 6: Remotion 工程：数据层、单段场景、字幕、配音

**Files:**
- Create: `marketing/launch-video/remotion.config.ts`、`src/index.ts`、`src/Root.tsx`、`src/manifest.ts`、`src/LaunchVideo.tsx`、`src/SectionScene.tsx`、`src/Captions.tsx`

**Interfaces:**
- Consumes: `public/audio/manifest.json`、`public/recordings/manifest.json`、各 `.cues.json` / `.events.json`。
- Produces: 合成 `LaunchVideo`（1920×1080×30fps，时长 = Σ(段配音 + 800ms 尾)）；`SectionScene` props `{ section: SectionData }`；`src/manifest.ts` 导出 `loadTimeline(): TimelineData` 与类型：
  ```ts
  export interface CueData { startMs: number; endMs: number; text: string }
  export interface ClipData { file: string; startAtMs: number; durationMs: number; events: EventsData }
  export interface EventsData { fps: number; scale: number; viewport: { width: number; height: number }; events: RecorderEvent[] }
  export interface SectionData { id: string; title: string; audio: string; durationMs: number; cues: CueData[]; clips: ClipData[]; fromFrame: number; durationInFrames: number }
  export interface TimelineData { fps: number; width: number; height: number; sections: SectionData[]; totalFrames: number }
  ```

- [ ] **Step 1: 配置与入口**

```ts
// remotion.config.ts
import { Config } from '@remotion/cli/config'
Config.setVideoImageFormat('jpeg')
Config.setOverwriteOutput(true)
Config.setConcurrency(4)
```
```ts
// src/index.ts
import { registerRoot } from 'remotion'
import { Root } from './Root'
registerRoot(Root)
```

- [ ] **Step 2: manifest.ts（构建时静态 import JSON）**

```ts
import audio from '../public/audio/manifest.json'
import recordings from '../public/recordings/manifest.json'
import type { RecorderEvent } from '../../../tests/e2e/launch-video/recorder'
export const FPS = 30, WIDTH = 1920, HEIGHT = 1080, TAIL_MS = 800
const cueModules = import.meta.glob('../public/audio/*.cues.json', { eager: true, import: 'default' }) as Record<string, CueData[]>
const eventModules = import.meta.glob('../public/recordings/*.events.json', { eager: true, import: 'default' }) as Record<string, EventsData>
export const msToFrames = (ms: number) => Math.round((ms / 1000) * FPS)
export function loadTimeline(): TimelineData {
  let cursor = 0
  const sections = audio.sections.map((a) => {
    const rec = recordings.sections.find((r) => r.id === a.id)
    const durationMs = a.durationMs + TAIL_MS
    const section: SectionData = {
      id: a.id, title: a.title, audio: a.audio, durationMs,
      cues: cueModules[`../public/audio/${a.id}.cues.json`] ?? [],
      clips: (rec?.clips ?? []).map((c) => ({ file: c.file, startAtMs: c.startAtMs, durationMs: c.durationMs, events: eventModules[`../${c.events}`]! })),
      fromFrame: cursor, durationInFrames: msToFrames(durationMs)
    }
    cursor += section.durationInFrames
    return section
  })
  return { fps: FPS, width: WIDTH, height: HEIGHT, sections, totalFrames: cursor }
}
```
（Remotion 用 Vite 打包，`import.meta.glob` 可用；如报错改为显式 `import` 每个文件。）

- [ ] **Step 3: Root 与 LaunchVideo**

```tsx
// src/Root.tsx
import { Composition } from 'remotion'
import { LaunchVideo } from './LaunchVideo'
import { loadTimeline, FPS, WIDTH, HEIGHT } from './manifest'
export const Root = () => {
  const timeline = loadTimeline()
  return <Composition id="LaunchVideo" component={LaunchVideo} durationInFrames={timeline.totalFrames} fps={FPS} width={WIDTH} height={HEIGHT} defaultProps={{ timeline }} />
}
```
```tsx
// src/LaunchVideo.tsx
import { AbsoluteFill, Sequence } from 'remotion'
import type { TimelineData } from './manifest'
import { SectionScene } from './SectionScene'
export const LaunchVideo = ({ timeline }: { timeline: TimelineData }) => (
  <AbsoluteFill style={{ background: '#0b0f14' }}>
    {timeline.sections.map((section) => (
      <Sequence key={section.id} from={section.fromFrame} durationInFrames={section.durationInFrames} name={section.title}>
        <SectionScene section={section} />
      </Sequence>
    ))}
  </AbsoluteFill>
)
```

- [ ] **Step 4: SectionScene（先不做推近与光标，只放视频、配音、字幕）**

```tsx
// src/SectionScene.tsx
import { AbsoluteFill, Audio, OffthreadVideo, Sequence, staticFile, useCurrentFrame } from 'remotion'
import { Captions } from './Captions'
import { FPS, msToFrames, type SectionData } from './manifest'
export const SectionScene = ({ section }: { section: SectionData }) => {
  const frame = useCurrentFrame()
  return (
    <AbsoluteFill>
      {section.clips.map((clip) => (
        <Sequence key={clip.file} from={msToFrames(clip.startAtMs)} durationInFrames={msToFrames(clip.durationMs)} layout="none">
          <OffthreadVideo src={staticFile(clip.file)} muted style={{ width: 1920, height: 1080, objectFit: 'cover' }} />
        </Sequence>
      ))}
      <Audio src={staticFile(section.audio)} />
      <Captions cues={section.cues} nowMs={(frame / FPS) * 1000} />
    </AbsoluteFill>
  )
}
```

- [ ] **Step 5: Captions**

```tsx
// src/Captions.tsx
import type { CueData } from './manifest'
export const Captions = ({ cues, nowMs }: { cues: CueData[]; nowMs: number }) => {
  const cue = cues.find((c) => nowMs >= c.startMs - 120 && nowMs < c.endMs + 80)
  if (!cue) return null
  return (
    <div style={{ position: 'absolute', left: 0, right: 0, bottom: 64, display: 'flex', justifyContent: 'center', pointerEvents: 'none' }}>
      <div style={{ fontFamily: '"PingFang SC", "Hiragino Sans GB", sans-serif', fontWeight: 600, fontSize: 40, lineHeight: 1.35, color: '#fff',
        padding: '10px 26px', borderRadius: 14, background: 'rgba(8,10,14,0.62)', textShadow: '0 2px 6px rgba(0,0,0,.8)', maxWidth: 1500, textAlign: 'center' }}>
        {cue.text}
      </div>
    </div>
  )
}
```

- [ ] **Step 6: 渲染前 90 帧验证**

Run: `cd marketing/launch-video && npx remotion render src/index.ts LaunchVideo out/preview.mp4 --frames=0-89 && ffprobe -v error -show_entries stream=width,height,r_frame_rate -of csv=p=0 out/preview.mp4`
Expected: `1920,1080,30/1`；`open out/preview.mp4` 前 3 秒是黑底（intro 尚未做）；再渲染 `why` 段起点：`npx remotion still src/index.ts LaunchVideo out/why.png --frame=<why.fromFrame + 60>`，能看到码头界面与字幕。

---

### Task 7: 推近镜头与光标

**Files:**
- Create: `marketing/launch-video/src/zoom.ts`、`src/zoom.test.ts`、`src/Cursor.tsx`
- Modify: `src/SectionScene.tsx`

**Interfaces:**
- Produces:
  ```ts
  export interface ZoomConfig { scale: number; inMs: number; holdMs: number; outMs: number; mergeGapMs: number }
  export const DEFAULT_ZOOM: ZoomConfig = { scale: 1.6, inMs: 450, holdMs: 1800, outMs: 650, mergeGapMs: 2500 }
  export interface Focus { scale: number; cx: number; cy: number }   // cx, cy 为输出画面像素
  export function focusAt(clicks: Array<{ t: number; x: number; y: number }>, tMs: number, toOutput: (x: number, y: number) => { x: number; y: number }, cfg?: ZoomConfig): Focus
  export function cursorAt(events: RecorderEvent[], tMs: number): { x: number; y: number; pressed: boolean } | null   // CSS 像素
  ```
  `toOutput` 把 CSS 像素换算为输出像素：`x * 1920 / viewport.width`。

- [ ] **Step 1: 先写测试**

```ts
// src/zoom.test.ts
import { describe, expect, it } from 'vitest'
import { DEFAULT_ZOOM, cursorAt, focusAt } from './zoom'
const id = (x: number, y: number) => ({ x, y })
describe('focusAt', () => {
  it('is identity before the first click', () => {
    expect(focusAt([{ t: 1000, x: 100, y: 100 }], 500, id)).toEqual({ scale: 1, cx: 960, cy: 540 })
  })
  it('reaches full scale after inMs and holds', () => {
    const f = focusAt([{ t: 1000, x: 100, y: 100 }], 1000 + DEFAULT_ZOOM.inMs + 500, id)
    expect(f.scale).toBeCloseTo(DEFAULT_ZOOM.scale, 5); expect(f.cx).toBe(100); expect(f.cy).toBe(100)
  })
  it('returns to 1 after hold + out', () => {
    const t = 1000 + DEFAULT_ZOOM.inMs + DEFAULT_ZOOM.holdMs + DEFAULT_ZOOM.outMs + 1
    expect(focusAt([{ t: 1000, x: 100, y: 100 }], t, id).scale).toBe(1)
  })
  it('pans instead of releasing when the next click is inside mergeGap', () => {
    const clicks = [{ t: 1000, x: 100, y: 100 }, { t: 2500, x: 900, y: 500 }]
    const f = focusAt(clicks, 2500 + DEFAULT_ZOOM.inMs, id)
    expect(f.scale).toBeCloseTo(DEFAULT_ZOOM.scale, 5); expect(f.cx).toBe(900); expect(f.cy).toBe(500)
  })
  it('is monotone during ease-in', () => {
    const a = focusAt([{ t: 0, x: 0, y: 0 }], 100, id).scale, b = focusAt([{ t: 0, x: 0, y: 0 }], 300, id).scale
    expect(b).toBeGreaterThan(a)
  })
})
describe('cursorAt', () => {
  const events = [
    { t: 0, type: 'move', x: 0, y: 0, ms: 0 },
    { t: 1000, type: 'move', x: 200, y: 100, ms: 400 },
    { t: 1400, type: 'click', x: 200, y: 100 }
  ] as const
  it('interpolates between moves', () => {
    const c = cursorAt([...events], 1200)!
    expect(c.x).toBeGreaterThan(0); expect(c.x).toBeLessThan(200); expect(c.pressed).toBe(false)
  })
  it('is pressed for 120ms after a click', () => { expect(cursorAt([...events], 1450)!.pressed).toBe(true); expect(cursorAt([...events], 1600)!.pressed).toBe(false) })
  it('is null before any event', () => { expect(cursorAt([], 10)).toBeNull() })
})
```

- [ ] **Step 2: 运行确认失败**

Run: `cd marketing/launch-video && npx vitest run`
Expected: FAIL，`Cannot find module './zoom'`。

- [ ] **Step 3: 实现 zoom.ts**

```ts
import type { RecorderEvent } from '../../../tests/e2e/launch-video/recorder'
export interface ZoomConfig { scale: number; inMs: number; holdMs: number; outMs: number; mergeGapMs: number }
export const DEFAULT_ZOOM: ZoomConfig = { scale: 1.6, inMs: 450, holdMs: 1800, outMs: 650, mergeGapMs: 2500 }
export interface Focus { scale: number; cx: number; cy: number }
const easeInOut = (p: number) => (p < 0.5 ? 4 * p * p * p : 1 - Math.pow(-2 * p + 2, 3) / 2)
const clamp01 = (v: number) => Math.min(1, Math.max(0, v))

// Groups clicks into zoom "shots": a shot starts at a click and extends while following clicks arrive within mergeGapMs.
function shots(clicks: Array<{ t: number; x: number; y: number }>, cfg: ZoomConfig) {
  const sorted = [...clicks].sort((a, b) => a.t - b.t)
  const out: Array<{ start: number; end: number; points: typeof sorted }> = []
  for (const click of sorted) {
    const last = out[out.length - 1]
    if (last && click.t - last.points[last.points.length - 1]!.t <= cfg.mergeGapMs) { last.points.push(click); last.end = click.t + cfg.inMs + cfg.holdMs }
    else out.push({ start: click.t, end: click.t + cfg.inMs + cfg.holdMs, points: [click] })
  }
  return out
}

export function focusAt(clicks: Array<{ t: number; x: number; y: number }>, tMs: number,
  toOutput: (x: number, y: number) => { x: number; y: number }, cfg: ZoomConfig = DEFAULT_ZOOM): Focus {
  const idle: Focus = { scale: 1, cx: 960, cy: 540 }
  for (const shot of shots(clicks, cfg)) {
    if (tMs < shot.start || tMs > shot.end + cfg.outMs) continue
    const scale = tMs < shot.start + cfg.inMs
      ? 1 + (cfg.scale - 1) * easeInOut(clamp01((tMs - shot.start) / cfg.inMs))
      : tMs <= shot.end ? cfg.scale : cfg.scale - (cfg.scale - 1) * easeInOut(clamp01((tMs - shot.end) / cfg.outMs))
    // focus point: current target, eased from the previous point over inMs after each click
    let index = 0
    while (index + 1 < shot.points.length && shot.points[index + 1]!.t <= tMs) index += 1
    const target = toOutput(shot.points[index]!.x, shot.points[index]!.y)
    if (index === 0) return { scale, cx: target.x, cy: target.y }
    const previous = toOutput(shot.points[index - 1]!.x, shot.points[index - 1]!.y)
    const p = easeInOut(clamp01((tMs - shot.points[index]!.t) / cfg.inMs))
    return { scale, cx: previous.x + (target.x - previous.x) * p, cy: previous.y + (target.y - previous.y) * p }
  }
  return idle
}

export function cursorAt(events: RecorderEvent[], tMs: number): { x: number; y: number; pressed: boolean } | null {
  const moves = events.filter((e): e is Extract<RecorderEvent, { type: 'move' }> => e.type === 'move' && e.t <= tMs)
  if (moves.length === 0) return null
  const current = moves[moves.length - 1]!
  const previous = moves.length > 1 ? moves[moves.length - 2]! : current
  const p = current.ms > 0 ? easeInOut(clamp01((tMs - current.t) / current.ms)) : 1
  const pressed = events.some((e) => e.type === 'click' && tMs >= e.t && tMs < e.t + 120)
  return { x: previous.x + (current.x - previous.x) * p, y: previous.y + (current.y - previous.y) * p, pressed }
}
```

- [ ] **Step 4: 运行测试**

Run: `npx vitest run`
Expected: 8 passed。

- [ ] **Step 5: Cursor.tsx 与接入 SectionScene**

```tsx
// src/Cursor.tsx
export const Cursor = ({ x, y, pressed }: { x: number; y: number; pressed: boolean }) => (
  <svg style={{ position: 'absolute', left: x - 4, top: y - 2, pointerEvents: 'none', filter: 'drop-shadow(0 2px 3px rgba(0,0,0,.5))' }} width="34" height="40" viewBox="0 0 17 20">
    {pressed && <circle cx="3" cy="3" r="9" fill="rgba(80,160,255,0.35)" />}
    <path d="M1 1 L1 15.5 L4.6 12.2 L7.3 18.4 L9.8 17.3 L7.2 11.3 L12 11.3 Z" fill="#fff" stroke="#000" strokeWidth="1.2" strokeLinejoin="round" />
  </svg>
)
```

`SectionScene` 改为：每个 clip 内计算 `clipMs = nowMs - clip.startAtMs`；`toOutput = (x, y) => ({ x: x * 1920 / clip.events.viewport.width, y: y * 1080 / clip.events.viewport.height })`；`const focus = focusAt(clicks, clipMs, toOutput)`；`const cursor = cursorAt(clip.events.events, clipMs)`；包一层：

```tsx
<AbsoluteFill style={{ transformOrigin: '0 0', transform: `translate(${(1 - focus.scale) * focus.cx}px, ${(1 - focus.scale) * focus.cy}px) scale(${focus.scale})` }}>
  <OffthreadVideo ... />
  {cursor && <Cursor x={toOutput(cursor.x, cursor.y).x} y={toOutput(cursor.x, cursor.y).y} pressed={cursor.pressed} />}
</AbsoluteFill>
```
字幕放在缩放层之外。

- [ ] **Step 6: 目检**

Run: `npx remotion still src/index.ts LaunchVideo out/zoom.png --frame=<structure 段第一次 click 的 t 换算帧 + 20>`
Expected: 画面放大约 1.6 倍，点击点附近居中，光标箭头压在被点击的控件上。

---

### Task 8: 片头、片尾、封面、60 秒短版

**Files:**
- Create: `src/Intro.tsx`、`src/Outro.tsx`、`src/Cover.tsx`、`src/Short60.tsx`、`script/narration-short.json`
- Modify: `src/SectionScene.tsx`（`intro`、`outro` 段渲染专用组件）、`src/Root.tsx`（注册 `Short60`、`Cover`）、`tts/synthesize.mjs`（支持 `--script narration-short.json --out audio-short`）

**Interfaces:**
- Consumes: `SectionData`（任务 6）；`Captions`。
- Produces: 合成 `Short60`（1920×1080，≤ 60s）、`Cover`（1146×717 静帧）。

- [ ] **Step 1: Intro（终端灾难）**

```tsx
// src/Intro.tsx —— 12 个假终端窗口按 spring 依次叠上来，最后一个红色报错闪烁，字幕由 Captions 负责
import { AbsoluteFill, interpolate, spring, useCurrentFrame, useVideoConfig } from 'remotion'
const LINES = ['⏺ Bash(pnpm vitest run src/payments)', '  ⎿ Running…', '⏺ Read(src/payments/webhook.ts)', '> 给支付回调加幂等处理', '⏺ Update(prisma/schema.prisma)', '✻ Welcome to Claude Code!']
const seeded = (i: number) => ((i * 9301 + 49297) % 233280) / 233280
export const Intro = () => {
  const frame = useCurrentFrame(); const { fps } = useVideoConfig()
  const windows = Array.from({ length: 12 }, (_, i) => {
    const appear = spring({ frame: frame - i * 9, fps, config: { damping: 14, stiffness: 120 } })
    const x = 120 + seeded(i) * 900, y = 60 + seeded(i + 40) * 420
    return (
      <div key={i} style={{ position: 'absolute', left: x, top: y, width: 760, height: 460, opacity: appear, transform: `scale(${0.85 + 0.15 * appear}) rotate(${(seeded(i + 7) - 0.5) * 3}deg)`,
        background: '#111418', border: '1px solid #2a2f36', borderRadius: 10, boxShadow: '0 30px 60px rgba(0,0,0,.6)', fontFamily: 'Menlo, monospace', fontSize: 18, color: '#c7d0d9', padding: '44px 22px 22px', overflow: 'hidden' }}>
        <div style={{ position: 'absolute', top: 12, left: 14, display: 'flex', gap: 8 }}>{['#ff5f57', '#febc2e', '#28c840'].map((c) => <span key={c} style={{ width: 12, height: 12, borderRadius: 6, background: c }} />)}</div>
        <div style={{ position: 'absolute', top: 10, left: 0, right: 0, textAlign: 'center', color: '#7b8794', fontSize: 14 }}>claude — session {i + 1}</div>
        {LINES.map((l, k) => <div key={k} style={{ opacity: 0.55 + seeded(i * 3 + k) * 0.45 }}>{l}</div>)}
      </div>
    )
  })
  const errorAt = 12 * 9 + 20
  const flash = frame > errorAt ? interpolate((frame - errorAt) % 30, [0, 15, 30], [0.15, 0.55, 0.15]) : 0
  return (
    <AbsoluteFill style={{ background: 'radial-gradient(ellipse at 50% 40%, #1b2129 0%, #0b0f14 70%)' }}>
      {windows}
      {frame > errorAt && (
        <div style={{ position: 'absolute', left: 560, top: 300, width: 800, padding: 28, background: `rgba(160,20,30,${flash})`, border: '2px solid #ff4d4f', borderRadius: 10, fontFamily: 'Menlo, monospace', fontSize: 22, color: '#ffd7d7' }}>
          Error: FAIL src/payments/webhook.duplicate.test.ts<br />exit code 1 · 30 min ago
        </div>
      )}
    </AbsoluteFill>
  )
}
```

- [ ] **Step 2: Outro**

深色底，四行依次淡入（用 `spring` 错开 12 帧）：「早期预览 · 仅 Apple Silicon」「安装包未签名，首次打开方法见置顶评论」「演示数据为构造」「github.com/icesword0760/matou · GPL-3.0」，右下放 `assets/qq-group.png`（复制到 `public/qq-group.png`）与 `assets/logo.png`。字体 `PingFang SC`，主字号 56。

- [ ] **Step 3: Cover（1146×717 静帧）**

左 45%：`Intro` 的静态帧（`frame` 固定为 130）加红叉叠层；右 55%：`public/workspace-demo.png`（从 `assets/shots/` 复制）圆角缩放；中央大字「10 个 Claude Code 同时跑」（字号 96，粗体，白色，黑色描边 6px），下方小字「不迷路 · 开源 · Mac」（字号 40，`#ffd166`）。

- [ ] **Step 4: SectionScene 分派**

```tsx
if (section.id === 'intro') return <AbsoluteFill><Intro /><Audio src={staticFile(section.audio)} /><Captions cues={section.cues} nowMs={nowMs} /></AbsoluteFill>
if (section.id === 'outro') return <AbsoluteFill><Outro /><Audio src={staticFile(section.audio)} /><Captions cues={section.cues} nowMs={nowMs} /></AbsoluteFill>
```

- [ ] **Step 5: Short60 说辞与合成**

`script/narration-short.json`（五段，合计不超过 48 秒配音）：

```json
[
  { "id": "s-hook", "title": "钩子", "text": "你开过几个 Claude Code？我开过十二个，然后忘了哪个在报错。" },
  { "id": "s-what", "title": "是什么", "text": "所以我开源了码头：Claude Code 的多智能体桌面工作台。" },
  { "id": "s-ai", "title": "AI 读屏", "text": "最爽的是这个：直接让一个 Claude Code 去读隔壁卡片的屏幕，不用复制粘贴。" },
  { "id": "s-dag", "title": "DAG", "text": "DAG 一眼看到哪条路线在跑、哪条挂了。重启回来，现场原样。" },
  { "id": "s-cta", "title": "收尾", "text": "开源、免费、Mac。GitHub 搜 matou，完整演示在主页。" }
]
```

`synthesize.mjs` 增加参数：`node tts/synthesize.mjs --script script/narration-short.json --out public/audio-short`（用 `process.argv` 解析，默认值保持原路径）。`Short60.tsx` 复用素材：`s-hook` → `Intro`；`s-what` → `why` 段片段前 8 秒；`s-ai` → `ai-control` 段从 `mark('mt-read')` 前 2 秒起 12 秒（读 events 的 mark）；`s-dag` → `fork-dag` 段从 `source: 'dag'` 事件起 6 秒 + `persist-b` 前 4 秒；`s-cta` → `Outro`。每段配音对齐，推近与字幕逻辑与主片一致（抽出 `ClipPlayer` 组件复用）。

- [ ] **Step 6: 验证**

Run: `npm run cover && open out/cover.png` → 1146×717，文字可读。
Run: `npm run render:short && ffprobe -v error -show_entries format=duration -of csv=p=0 out/matou-launch-60s.mp4` → ≤ 60。

---

### Task 9: 全片渲染与质检

**Files:**
- Create: `marketing/launch-video/qc/contact-sheet.sh`、`qc/check-sync.mjs`

- [ ] **Step 1: 渲染**

Run: `cd marketing/launch-video && npm run render`
Expected: `out/matou-launch-1080p.mp4`，`ffprobe` 时长 360 到 450 秒。首次渲染约 20 到 40 分钟。

- [ ] **Step 2: 抽帧墙**

```bash
# qc/contact-sheet.sh
ffmpeg -y -v error -i out/matou-launch-1080p.mp4 -vf "fps=1/15,scale=480:-1,tile=6x5:padding=4:margin=4,drawtext=fontfile=/System/Library/Fonts/Helvetica.ttc:text='%{pts\:hms}':x=8:y=8:fontsize=24:fontcolor=yellow:box=1:boxcolor=black@0.6" -frames:v 1 out/qc-sheet.png
```
Run 后 `open out/qc-sheet.png`。Expected: 无黑帧（除 intro 前 0.5 秒）、无尺寸跳变、每段画面与分镜表一致。

- [ ] **Step 3: 字幕与操作同步检查**

```js
// qc/check-sync.mjs：对三段（fork-dag/ai-control/board-notify），打印关键 click 的绝对时间与含关键词 cue 的时间差
import { readFile } from 'node:fs/promises'
const audio = JSON.parse(await readFile('public/audio/manifest.json', 'utf8'))
const rec = JSON.parse(await readFile('public/recordings/manifest.json', 'utf8'))
const checks = [['fork-dag', 'fork', '点一下 Fork'], ['fork-dag', 'dag', 'Option'], ['ai-control', 'mt-read', '直接说'], ['board-notify', 'board', '看板']]
let offset = 0
for (const a of audio.sections) {
  const cues = JSON.parse(await readFile(`public/audio/${a.id}.cues.json`, 'utf8'))
  for (const [id, label, keyword] of checks.filter((c) => c[0] === a.id)) {
    const clips = rec.sections.find((s) => s.id === id).clips
    for (const clip of clips) {
      const ev = JSON.parse(await readFile(`public/${clip.events}`, 'utf8')).events.find((e) => (e.label ?? e.name) === label)
      if (!ev) continue
      const cue = cues.find((c) => c.text.replace(/\s/g, '').includes(keyword.replace(/\s/g, '')))
      console.log(id, label, 'event', ((offset + clip.startAtMs + ev.t) / 1000).toFixed(1) + 's', 'cue', ((offset + cue.startMs) / 1000).toFixed(1) + 's', 'delta', ((clip.startAtMs + ev.t - cue.startMs) / 1000).toFixed(2) + 's')
    }
  }
  offset += a.durationMs + 800
}
```
Run: `node qc/check-sync.mjs`。Expected: 每行 `delta` 在 0 到 1.2 秒之间。用输出里的绝对秒数在播放器里跳到对应位置目检三处。

- [ ] **Step 4: 听一遍**

`open out/matou-launch-1080p.mp4` 全片播放一遍，记录读音别扭的句子，改 `narration.json` 后只需 `npm run tts` 并重录受影响段（`record.spec.ts` 支持 `MATOU_SECTIONS=fork-dag,ai-control` 只录指定段，其余段沿用旧 manifest：在 spec 里读取已有 manifest 并只替换列出的段）。

---

### Task 10: README 路线图修正与草稿替换

**Files:**
- Modify: `README.md:144-152`（路线图）
- 浏览器操作：B 站草稿

- [ ] **Step 1: 改 README 路线图**

把「自然语言创建层级结构（未实现）」一段改为已实现的描述：

```markdown
**自然语言创建层级结构。** 除了读取和控制其他卡片，托管的 Agent 还可以直接创建结构：`mt create workspace|task|canvas|session`、`mt fork child|sibling|children`，以及 `mt remove preview|commit` 带预览确认的移除。你可以对当前会话说：

> “根据这三个方案创建三个子卡片，分别验证性能、兼容性和回滚路径。”
```

保留「代码签名与公证（未完成）」。同步修改 `README.en.md` 对应段落。`<details>` 里的命令列表补上 `mt create`、`mt fork`、`mt remove`。

- [ ] **Step 2: 校验**

Run: `pnpm check:identifiers`
Expected: 通过（品牌与命名门禁）。

- [ ] **Step 3: 替换草稿视频与封面**

用 Claude in Chrome 打开 `https://member.bilibili.com/platform/upload-manager/article?group=draft`，点击草稿「开了12个Claude Code之后…」→ 编辑；「更换视频」的 file input 用 `file_upload` 传 `marketing/launch-video/out/matou-launch-1080p.mp4`；封面 file input 传 `out/cover.png`；把简介里的时间轴按 `qc/check-sync.mjs` 输出的各段起点改写；再次「存草稿」。不点「立即投稿」，由作者决定发布时间（周二到周四 18:30）。

- [ ] **Step 4: 验收清单回填**

对照 spec「验收」五条逐项确认，把结果写到本计划末尾的「验收记录」小节。

---

## 验收记录

（执行时填写）
