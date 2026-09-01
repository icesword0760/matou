# Matou 投影、恢复调度、Session 画布与 DAG 大规模实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development`（推荐）或 `superpowers:executing-plans` 按任务实施。每项必须走 RED → GREEN → REFACTOR，并以复选框跟踪。

**Goal:** 在不改变 Kooky 动画和核心导航语义的前提下，让 Matou 在大量会话、深层关系和大 Journal 下仍能优先恢复当前工作现场、持续流畅交互，并以真实 Electron 基准阻止性能回退。

**Architecture:** Runtime 保持 SQLite、Journal、PTY 和恢复顺序的权威；Renderer 通过增量投影消费权威状态。当前活动横向列表的全部 Session 都属于前台，即使滑出视野也保持 xterm；非活动 Scene 解除 xterm 视图绑定，但 PTY 与 Journal 继续运行。恢复使用活动 Scene 优先的有界队列；DAG 只渲染视口内节点和按分支/层聚合的远层摘要。

**Tech Stack:** TypeScript 7、React 19、Electron 43、xterm.js 6、node-pty、Node SQLite、Vitest 4、Playwright 1.62。

**Spec:** `docs/audits/2026-09-01-internal-hardening-analysis.md`、`docs/prd/PRD-Matou-会话画布与DAG分支交互.md`，以及本计划“已确认产品决策”章节。本计划中的新决策优先于旧设计文档中“只挂载视口可见卡片 xterm”的描述。

## 已确认产品决策

1. 当前横向列表中的全部 Session 都是前台；即使卡片滑出视野，仍保持 xterm 挂载和即时输入能力。
2. 非活动 Scene 解除 xterm 视图绑定；对应 PTY、Journal 和 Session 继续运行。
3. 恢复按活动 Scene 优先，其余 Session 使用有界后台队列；切换 Scene 后新活动 Scene 立即提升优先级。
4. 恢复中的卡片使用整卡 loading，不在可交互终端上叠加局部小提示。
5. DAG 远层节点按分支和层级自动聚合；进入视口或聚焦分支后自动展开。
6. Session 删除入口只保留文案 `移除节点…`；确认弹窗内选择“仅移除当前节点”或“移除当前节点及后代”。
7. 动画时长、轨迹和悬停交接保持 Kooky；所有 resize 观察回调最多按 60Hz 合并一次。
8. 必须建立 50、200、1000 Session，5000 深链和 10000 DAG 节点的真实基准。

## 全局约束

- Runtime 是结构、恢复状态、PTY 生命周期和投影序号的唯一权威源；Renderer 不推断持久事实。
- 增量事件连续时不得补拉全量 snapshot；仅 event gap、runtime generation 改变或显式重连可重建。
- 非活动 Scene 卸载 xterm 不等于 dispose Session；`terminal.dispose` 只用于用户移除或权威生命周期结束。
- 当前活动横向列表不做卡片/xterm windowing；性能优化使用结构共享、组件隔离、布局索引和帧合并。
- 自动恢复只处理上次 Runtime generation 中仍为活动/中断的 Session；用户停止或移除的 archived Session 不自动重启。
- “仅移除当前节点”保留其后代：直接子节点接到被移除节点原父级；原节点为根时，直接子节点成为同层根节点。子节点自身关系种类保持不变。
- 移除节点不删除工作区文件、Git 分支或 worktree。
- 所有性能门槛记录机器信息、样本规模、warm-up、p50、p95、最大值、renderer/runtime RSS 和 PTY 数；单次最好值不得作为验收证据。
- 普通单元/E2E 套件保持稳定；重型规模基准使用 `pnpm test:scale` 独立执行。

## 性能门槛

### 硬门槛

| 场景 | 门槛 |
|---|---|
| 空闲活动工作区 | 0 次/分钟无理由全量 `projection.snapshot`；路径状态未变化时 0 次 React projection 更新 |
| 1000 Session + 1000 Scene snapshot | 本机 warm p95 ≤ 75ms；SQLite statement 数 ≤ 40；无 per-entity 查询增长 |
| 10000 节点 ProjectionStore 增量 batch | p95 ≤ 16.7ms；未变化 scene graph 引用保持 `===` |
| 5000 层 Session 链 / split | 不栈溢出；索引与后代/布局计算各 p95 ≤ 50ms |
| Session resize | 同一 xterm 每动画帧最多 1 次 `fit` 和 1 次不同尺寸 RPC；连续相同尺寸不发送 |
| 悬停重定向 | 横向滚动后 stationary pointer 在 100ms 内命中新卡片 |
| 悬停退出 | pointer leave 后 300ms 内清除临时 `.is-expanded`；500ms 内保存最终可达位置 |
| DAG 10000 节点 | 首次可操作 p95 ≤ 300ms；DOM 中 node/aggregate ≤ 400、edge ≤ 800；pan/zoom p95 frame ≤ 16.7ms |
| 恢复队列 | 同时启动/回放任务不超过 4；活动 Scene 首个任务在队列可用后 100ms 内开始；后台任务不饿死 |
| 非活动 Scene | xterm DOM 数为 0；PTY PID 不变；回到 Scene 后从 last acknowledged sequence 补齐且不重复输出 |

### 规模分层门槛

| 真实 Electron 数据集 | 验收门槛 |
|---|---|
| 50 个当前横向 Session | 首屏整卡状态 ≤ 2s；滑动/hover p95 frame ≤ 16.7ms；无 >50ms Renderer Long Task |
| 200 个当前横向 Session | 首屏整卡状态 ≤ 3s；滑动/hover p95 frame ≤ 33.3ms；焦点/菜单反馈 ≤ 100ms |
| 1000 个当前横向 Session | 2s 内出现可操作应用壳和活动卡片 loading；DAG/侧栏可操作 ≤ 300ms；恢复并发始终 ≤4；无崩溃或失去输入 |
| 5000 层关系 | 恢复和父子导航成功；无递归错误；单次结构计算 ≤ 50ms |
| 10000 DAG 节点 | 满足上表 DAG DOM、首交互和帧耗门槛；搜索结果 ≤ 100ms |

1000 Session 基准不等待 1000 个 PTY 全部完成后才判断应用可用；它验证真实 SQLite、真实 Electron、真实调度器和真实 node-pty 在持续后台恢复期间的前台响应。50 为每次合并硬门槛，200/1000 为 nightly 和发布候选硬门槛。

---

## 文件结构与职责

### 新建文件

- `apps/runtime/src/projection/runtime-projection-reader.ts`：集合查询和 projection snapshot 组装。
- `apps/runtime/src/projection/runtime-projection-reader.test.ts`：snapshot 完整性、查询预算和 1000/1000 规模测试。
- `apps/runtime/src/recovery/runtime-session-recovery-scheduler.ts`：活动 Scene 优先、有界并发、可重排的恢复队列。
- `apps/runtime/src/recovery/runtime-session-recovery-scheduler.test.ts`：优先级、并发、公平、取消测试。
- `apps/runtime/src/journal/journal-range-reader.ts`：按 sequence/segment 流式读取 Journal。
- `apps/runtime/src/journal/journal-range-reader.test.ts`：checkpoint 后读取和内存预算测试。
- `apps/desktop/src/renderer/src/session-canvas/session-graph-index.ts`：迭代邻接索引、后代遍历和 cycle 诊断。
- `apps/desktop/src/renderer/src/session-canvas/session-graph-index.test.ts`：5000 深链、宽树、损坏环测试。
- `apps/desktop/src/renderer/src/hierarchy/scene-layout-index.ts`：迭代 split 布局恢复。
- `apps/desktop/src/renderer/src/hierarchy/scene-layout-index.test.ts`：5000 深层 split 测试。
- `apps/desktop/src/renderer/src/runtime/useSessionRecovery.ts`：Renderer 队列状态订阅与 Scene 优先级桥接。
- `apps/desktop/src/renderer/src/terminal/frame-coalescer.ts`：60Hz resize/fit 合并器。
- `apps/desktop/src/renderer/src/terminal/frame-coalescer.test.ts`：一帧一次、同值去重、卸载取消测试。
- `apps/desktop/src/renderer/src/dag/dag-render-model.ts`：节点、边、远层分支/层聚合模型。
- `apps/desktop/src/renderer/src/dag/dag-render-model.test.ts`：10000 节点聚合、稳定 key 和展开测试。
- `tests/e2e/scale/scale-database.ts`：直接在已迁移真实 SQLite 中生成 50/200/1000、深链和 DAG 数据。
- `tests/e2e/scale/scale-metrics.ts`：PerformanceObserver、帧耗、RSS、DOM、PTY 和查询计数采集。
- `tests/e2e/scale/scale-benchmark.spec.ts`：真实 Electron 规模验收。

### 主要修改文件

- `packages/contracts/src/protocol.ts`、`packages/contracts/src/protocol.test.ts`：headless ensure-running、view detach、恢复状态消息。
- `apps/runtime/src/rpc/runtime-rpc-router.ts`：委托 projection reader，删除 snapshot N+1。
- `apps/runtime/src/runtime-server.ts`：区分 PTY ensure-running、view attach/detach 和 dispose。
- `apps/runtime/src/session/pty-session.ts`：无 view 时仅写 Journal，重新 attach 后从 sequence 补齐。
- `apps/runtime/src/index.ts`：构造共享恢复调度器并启动非阻塞恢复。
- `apps/desktop/src/renderer/src/projection/RuntimeProjectionStore.ts`：结构共享、完整增量事件和 gap 标记。
- `apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx`：移除 400ms 全刷、按活动 Scene 挂载、接入恢复队列。
- `apps/desktop/src/renderer/src/hierarchy/hierarchy-commands.ts`：mutation 不再自动 full refresh。
- `apps/desktop/src/renderer/src/hierarchy/TerminalPane.tsx`：整卡 loading、统一“移除节点…”弹窗。
- `apps/desktop/src/renderer/src/session-canvas/SessionCanvas.tsx`：使用迭代 graph index。
- `apps/desktop/src/renderer/src/session-canvas/SessionCarousel.tsx`：稳定 wheel/hover 状态机、全前台列表、60Hz 布局更新。
- `apps/desktop/src/renderer/src/terminal/TerminalSurface.tsx`：可显式解绑 view、resize 合并。
- `apps/desktop/src/renderer/src/dag/DagCanvas.tsx`、`DagWindowApp.tsx`：聚合 render model、rAF transform、revision 更新。
- `apps/runtime/src/session-canvas/session-canvas-service.ts`：仅当前节点/含后代两种事务语义。
- `package.json`：新增 `test:scale`。

---

## Task 1：建立可重复的真实规模基准与指标合同

**Files:**
- Create: `tests/e2e/scale/scale-database.ts`
- Create: `tests/e2e/scale/scale-metrics.ts`
- Create: `tests/e2e/scale/scale-benchmark.spec.ts`
- Modify: `tests/e2e/matou-fixture.ts`
- Modify: `package.json`

**Interfaces:**

```ts
export interface ScaleDataset {
  siblingSessions: 50 | 200 | 1000
  relationshipDepth?: 5000
  dagNodes?: 10000
  scenes?: number
  journalBytesPerSession?: number
}

export async function seedScaleDatabase(
  dataDirectory: string,
  dataset: ScaleDataset
): Promise<void>

export interface ScaleSample {
  name: string
  count: number
  p50: number
  p95: number
  max: number
  rendererRssMb: number
  runtimeRssMb: number
  ptyCount: number
  domNodes: number
}
```

- [ ] **Step 1: 写 failing harness tests**

在 `scale-benchmark.spec.ts` 先写三个数据集可启动、指标字段完整、样本至少 120 帧的测试；初次运行应因 `seedScaleDatabase` 和 `collectScaleSample` 不存在而失败。

- [ ] **Step 2: 验证 RED**

```bash
pnpm build && pnpm exec playwright test tests/e2e/scale/scale-benchmark.spec.ts --grep "harness"
```

Expected: FAIL，缺少 scale fixture/metrics 实现。

- [ ] **Step 3: 实现最小 harness**

先启动一次 Matou 完成 migration，关闭应用，用 `node:sqlite` 写入固定 ID、固定时间戳的真实表数据，再重启 Electron。指标使用浏览器 `PerformanceObserver({ type: 'longtask' })`、连续 `requestAnimationFrame`、Electron `app.getAppMetrics()` 和 Runtime 测试状态接口采集；每组预热 2 次、正式 5 次。

- [ ] **Step 4: 增加独立命令**

```json
{
  "scripts": {
    "test:scale": "pnpm build && playwright test tests/e2e/scale/scale-benchmark.spec.ts --workers=1"
  }
}
```

- [ ] **Step 5: 验证 GREEN**

```bash
pnpm test:scale --grep "harness"
```

Expected: PASS，并输出机器、样本数、p50/p95/max、RSS、PTY 与 DOM 数；此任务只验证 harness，不应用最终性能门槛。

- [ ] **Step 6: Commit**

```bash
git add package.json tests/e2e/matou-fixture.ts tests/e2e/scale
git commit -m "test: add real scale benchmark harness"
```

**验收：** 相同 seed 连续两次得到相同实体/关系数量；测试退出后所有 Electron、Runtime 和 PTY 子进程被清理。

---

## Task 2：用迭代索引消除深链递归和 O(N²) 扫描

**Files:**
- Create: `apps/desktop/src/renderer/src/session-canvas/session-graph-index.ts`
- Create: `apps/desktop/src/renderer/src/session-canvas/session-graph-index.test.ts`
- Create: `apps/desktop/src/renderer/src/hierarchy/scene-layout-index.ts`
- Create: `apps/desktop/src/renderer/src/hierarchy/scene-layout-index.test.ts`
- Modify: `apps/desktop/src/renderer/src/session-canvas/SessionCanvas.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx`

**Interfaces:**

```ts
export interface SessionGraphIndex {
  byId: ReadonlyMap<string, SessionGraphNodeView>
  childrenByParent: ReadonlyMap<string, readonly SessionGraphNodeView[]>
  descendantsOf(sessionId: string): readonly SessionGraphNodeView[]
}

export function indexSessionGraph(nodes: readonly SessionGraphNodeView[]): SessionGraphIndex
export function layoutFromSnapshot(snapshot: SceneSnapshotView): LayoutNode | undefined
export function orderedSessionIds(snapshot: SceneSnapshotView): readonly string[]
```

- [ ] **Step 1: 写 5000 深链、10000 宽树和 cycle failing tests**

断言结果顺序与当前实现一致；环形损坏数据返回带节点 ID 的诊断错误，不进入无限循环。

- [ ] **Step 2: 验证 RED**

```bash
pnpm --filter @matou/desktop test -- session-graph-index scene-layout-index
```

Expected: FAIL，接口不存在。

- [ ] **Step 3: 实现显式栈遍历**

每次 graph/snapshot 引用变化只建一次 `byId` 和 `childrenByParent`。遍历栈保存 `{ node, nextChildIndex }`，不得调用递归、`nodes.filter` 或 `nodes.find`。

- [ ] **Step 4: 替换组件热路径**

`SessionCanvas`、descendant impact、Hierarchy split 恢复和 `orderedSessionIds` 共用索引；卡片 render 内不得重新计算整棵后代树。

- [ ] **Step 5: 验证 GREEN 与门槛**

```bash
pnpm --filter @matou/desktop test -- SessionCanvas HierarchyShell session-graph-index scene-layout-index
```

Expected: 5000 深链无栈溢出，索引、后代和 layout 各 p95 ≤ 50ms。

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/src/renderer/src/session-canvas apps/desktop/src/renderer/src/hierarchy
git commit -m "perf: make session graph traversal iterative"
```

**验收：** 节点顺序、直接父子、descendant impact 和 split ratio 与现有小规模 fixture 完全一致。

---

## Task 3：将 projection snapshot 改为集合查询

**Files:**
- Create: `apps/runtime/src/projection/runtime-projection-reader.ts`
- Create: `apps/runtime/src/projection/runtime-projection-reader.test.ts`
- Modify: `apps/runtime/src/rpc/runtime-rpc-router.ts`
- Modify: `apps/runtime/src/session-canvas/session-graph-repository.ts`
- Modify: `apps/runtime/src/scenes/scene-repository.ts`

**Interfaces:**

```ts
export class RuntimeProjectionReader {
  constructor(database: RuntimeDatabase)
  snapshot(windowId?: string): RuntimeProjectionSnapshot
}

export interface ProjectionReadMetrics {
  statementCount: number
  entityCount: number
  serializedBytes: number
  elapsedMs: number
}
```

- [ ] **Step 1: 写 snapshot 等价与查询预算 failing tests**

用同一 migrated DB 比较旧 fixture 的 workspaces/tasks/sessions/runs/bindings/scenes/geometry/graphs/navigation/pathState/unread；再生成 1000 Session + 1000 Scene，断言 statementCount ≤ 40。

- [ ] **Step 2: 验证 RED**

```bash
pnpm --filter @matou/runtime test -- runtime-projection-reader
```

Expected: FAIL，当前查询数随实体线性增长。

- [ ] **Step 3: 实现集合查询和内存 join**

每张表最多一次有序查询；在 reader 内建立 `taskId`、`sceneId`、`sessionId` Map 后组装。Scene graph repository 增加批量入口：

```ts
projectSceneGraphs(sceneIds: readonly string[], windowId?: string): Record<string, SessionGraphProjection>
```

该入口一次读取 memberships、relations、bindings、runs 和 HUD 所需投影字段，不逐 Scene 回调 SQL。

- [ ] **Step 4: Router 委托 reader**

`RuntimeRpcRouter.#snapshot` 只解析 windowId 并调用 `this.#projectionReader.snapshot(windowId)`；删除 `.map(getWorkspace/getTask/getSession/projectSceneGraph)`。

- [ ] **Step 5: 验证 GREEN 与门槛**

```bash
pnpm --filter @matou/runtime test -- runtime-projection-reader runtime-rpc-router
```

Expected: 1000/1000 warm p95 ≤ 75ms、statementCount ≤ 40，JSON 字段和排序与原合同一致。

- [ ] **Step 6: Commit**

```bash
git add apps/runtime/src/projection apps/runtime/src/rpc/runtime-rpc-router.ts apps/runtime/src/session-canvas/session-graph-repository.ts apps/runtime/src/scenes/scene-repository.ts
git commit -m "perf: read runtime projection in bounded queries"
```

**依赖：** Task 1 的数据库 seed 可复用；不依赖 Renderer 任务。

---

## Task 4：让连续事件成为 Renderer 的正常投影路径

**Files:**
- Modify: `apps/desktop/src/renderer/src/projection/RuntimeProjectionStore.ts`
- Modify: `apps/desktop/src/renderer/src/projection/RuntimeProjectionStore.test.ts`
- Modify: `apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/hierarchy-commands.ts`
- Modify: `apps/runtime/src/hierarchy/workspace-path-service.ts`
- Modify: relevant hierarchy service tests that emit navigation/order/path events

**Interfaces:**

```ts
export interface ProjectionApplyResult {
  changed: boolean
  changedSceneIds: readonly string[]
  requiresSnapshot: boolean
}

applyBatch(runtimeGeneration: string, events: readonly DomainEventWireEnvelope[]): ProjectionApplyResult
```

- [ ] **Step 1: 写所有 mutation 的增量合同 tests**

覆盖 workspace/task/scene 创建、重命名、排序、固定、归档、激活，session focus、mount、detach/return、graph、path-state 和 unread。每个测试从 snapshot 开始，只投递 event，然后断言 projection 与 mutation result 一致。

- [ ] **Step 2: 写刷新次数 failing tests**

断言 100 个连续 event batch 后 `projection.snapshot` 调用次数为 0；event gap 和 runtime generation 改变各触发 1 次；路径状态未变化触发 0 次 setProjection。

- [ ] **Step 3: 验证 RED**

```bash
pnpm --filter @matou/desktop test -- RuntimeProjectionStore HierarchyShell hierarchy-commands
```

Expected: FAIL，当前 event batch、mutation 和 400ms path check 会 full refresh。

- [ ] **Step 4: 补齐权威事件 payload 和 Store reducer**

事件 payload 对 navigation/order/path 使用显式 patch；Store 只替换发生变化的 entity/scene graph，未变化引用保持稳定。gap 不抛弃当前 UI，返回 `requiresSnapshot: true` 后由 Shell 单飞重建。

- [ ] **Step 5: 移除正常路径 full refresh**

- `events.batch` 成功后直接发布 Store view。
- `hierarchy-commands` 不再调用通用 `afterMutation`。
- path service 仅在 status/reason/generation 实际变化时发 `workspace.path-status-changed`。
- 初始化、gap、generation 变化、显式 reconnect 保留 snapshot。

- [ ] **Step 6: 验证 GREEN 与门槛**

```bash
pnpm --filter @matou/desktop test -- RuntimeProjectionStore HierarchyShell hierarchy-commands
pnpm --filter @matou/runtime test -- workspace-path-service hierarchy-application-service
```

Expected: 空闲 60 秒 0 次无理由 snapshot；10000 节点增量 batch p95 ≤16.7ms；未变化 graph `===`。

- [ ] **Step 7: Commit**

```bash
git add apps/desktop/src/renderer/src/projection apps/desktop/src/renderer/src/hierarchy apps/runtime/src/hierarchy
git commit -m "perf: make projection updates event driven"
```

**依赖：** Task 3 先完成，确保 gap 重建本身已是有界查询。

---

## Task 5：增加“解绑视图但保留 PTY”的协议与生命周期

**Files:**
- Modify: `packages/contracts/src/protocol.ts`
- Modify: `packages/contracts/src/protocol.test.ts`
- Modify: `apps/desktop/src/renderer/src/runtime/RuntimeClient.ts`
- Modify: `apps/desktop/src/renderer/src/runtime/RuntimeClient.test.ts`
- Modify: `apps/runtime/src/runtime-server.ts`
- Modify: `apps/runtime/src/runtime-server.test.ts`
- Modify: `apps/runtime/src/session/pty-session.ts`
- Modify: `apps/runtime/src/session/pty-session.test.ts`

**Interfaces:**

```ts
type TerminalSpawnViewMode = 'attached' | 'headless'

// Renderer -> Runtime
{ type: 'terminal.view-detach'; protocolVersion: 1; sessionId: string }
{ type: 'terminal.spawn'; /* existing fields */ viewMode?: TerminalSpawnViewMode; recoveryRequestId?: string }

// Runtime -> Renderer
{ type: 'terminal.running'; protocolVersion: 1; sessionId: string; pid: number; recoveryRequestId: string }

RuntimeClient.ensureTerminalRunning(config: TerminalAttachment): Promise<{ pid: number }>
RuntimeClient.detachTerminalView(sessionId: string): void
```

- [ ] **Step 1: 写协议、RuntimeClient 和 PtySession failing tests**

验证最后一个 view listener 卸载时发送 `terminal.view-detach` 而非 `terminal.dispose`；PTY 继续写 Journal；重新 attach 的 PID 不变并从最后 ACK sequence 补齐且只补一次。

- [ ] **Step 2: 验证 RED**

```bash
pnpm --filter @matou/contracts test -- protocol
pnpm --filter @matou/desktop test -- RuntimeClient
pnpm --filter @matou/runtime test -- pty-session runtime-server
```

Expected: FAIL，当前没有 per-session view detach 消息。

- [ ] **Step 3: 实现协议和 client 引用计数**

第一个 listener 使用 `viewMode:'attached'`；最后一个 listener 卸载发送 detach。`disposeDeletedTerminal` 继续发送 dispose，二者不可复用。

- [ ] **Step 4: 实现 Runtime attach/detach**

headless ensure-running 创建或复用共享 `RuntimeSessionRegistry` 中的 PTY，完成后立即 `PtySession.detach(sendToPort)`；view detach 只清除此连接的 attached capability。重新 attach 复用相同 PTY 并返回 replay sequence。

- [ ] **Step 5: 验证 GREEN**

```bash
pnpm --filter @matou/contracts test -- protocol
pnpm --filter @matou/desktop test -- RuntimeClient
pnpm --filter @matou/runtime test -- pty-session runtime-server
```

Expected: hidden Scene xterm listener 数为 0，PTY PID 不变，Journal sequence 连续，无重复 frame。

- [ ] **Step 6: Commit**

```bash
git add packages/contracts/src/protocol* apps/desktop/src/renderer/src/runtime apps/runtime/src/runtime-server* apps/runtime/src/session/pty-session*
git commit -m "feat: detach terminal views without stopping ptys"
```

**依赖：** 无；必须先于 Scene 挂载策略和恢复调度。

---

## Task 6：让 Journal 从 checkpoint 后按范围流式读取

**Files:**
- Create: `apps/runtime/src/journal/journal-range-reader.ts`
- Create: `apps/runtime/src/journal/journal-range-reader.test.ts`
- Modify: `apps/runtime/src/journal/segment-journal.ts`
- Modify: `apps/runtime/src/journal/segment-journal.test.ts`
- Modify: `apps/runtime/src/checkpoints/checkpoint-manager.ts`
- Modify: `apps/runtime/src/runtime-server.ts`
- Modify: `apps/runtime/src/recovery/journal-event-alignment.ts`

**Interfaces:**

```ts
export interface JournalBounds {
  firstSequence: number
  lastSequence: number
  segments: readonly { path: string; firstSequence: number; lastSequence: number }[]
}

export async function* iterateSessionFrames(
  dataRoot: string,
  sessionId: string,
  options: { fromSequence: number }
): AsyncGenerator<DecodedJournalFrame>
```

- [ ] **Step 1: 写 32MB/256MB failing tests**

checkpoint 位于 90% 位置时只解码末尾 10%；验证 output/resize/reset/exit 顺序、损坏 segment 诊断和 retained gap 不变。

- [ ] **Step 2: 验证 RED**

```bash
pnpm --filter @matou/runtime test -- journal-range-reader segment-journal checkpoint
```

Expected: FAIL，当前先读取并复制全部 frames。

- [ ] **Step 3: 实现 segment bounds 和异步 iterator**

sealed segment 写入/关闭时记录 first/last sequence；旧数据首次打开时逐 segment 建 bounds，后续不重复全历史扫描。iterator 跳过 `lastSequence < fromSequence` 的 segment，并逐 frame yield，不累积总数组。

- [ ] **Step 4: replay 先选 checkpoint 再读取**

CheckpointManager 先从 SQLite metadata 返回 watermark；RuntimeServer 使用 `checkpoint.terminalSequence + 1` 调 iterator。`readFrames()` 保留为兼容测试 wrapper，但 production replay/recovery 不调用它。

- [ ] **Step 5: 验证 GREEN 与门槛**

```bash
pnpm --filter @matou/runtime test -- journal segment-journal runtime-server journal-event-alignment
```

Expected: 32MB checkpoint 后 replay p95 ≤100ms、额外 RSS ≤16MB；256MB 不创建全历史 frame 数组。

- [ ] **Step 6: Commit**

```bash
git add apps/runtime/src/journal apps/runtime/src/checkpoints apps/runtime/src/runtime-server.ts apps/runtime/src/recovery/journal-event-alignment.ts
git commit -m "perf: stream terminal replay from checkpoints"
```

**依赖：** Task 1 提供内存与时间采集；保持完整历史语义，不裁剪用户历史。

---

## Task 7：实现活动 Scene 优先的有界恢复队列

**Files:**
- Create: `apps/runtime/src/recovery/runtime-session-recovery-scheduler.ts`
- Create: `apps/runtime/src/recovery/runtime-session-recovery-scheduler.test.ts`
- Modify: `apps/runtime/src/index.ts`
- Modify: `apps/runtime/src/runtime-server.ts`
- Modify: `apps/desktop/src/renderer/src/runtime/RuntimeClient.ts`
- Create: `apps/desktop/src/renderer/src/runtime/useSessionRecovery.ts`
- Modify: `apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx`
- Modify: `packages/contracts/src/protocol.ts`

**Interfaces:**

```ts
export type RecoveryState = 'queued' | 'restoring' | 'ready' | 'failed'

export interface RecoveryJob {
  sessionId: string
  sceneId: string
  priority: 'active-scene' | 'background'
  enqueueSequence: number
}

export class RuntimeSessionRecoveryScheduler {
  constructor(options: { concurrency: 4; start(job: RecoveryJob): Promise<void> })
  enqueue(jobs: readonly RecoveryJob[]): void
  prioritizeScene(sceneId: string): void
  cancel(sessionIds: readonly string[]): void
  snapshot(): readonly (RecoveryJob & { state: RecoveryState })[]
}
```

- [ ] **Step 1: 写调度器 failing tests**

覆盖并发始终 ≤4、活动 Scene FIFO 优先、切换 Scene 后未开始任务重排、背景任务每完成 8 个活动任务至少运行 1 个、删除取消 queued job、单任务失败不阻断队列。

- [ ] **Step 2: 写“stopped 不自动恢复” failing integration test**

活动/中断 Session 入队；`archived_at IS NOT NULL` 或删除中的 Session 不入队。移除 `HierarchyShell` 当前遍历所有 archived 节点自动 restart 的行为。

- [ ] **Step 3: 验证 RED**

```bash
pnpm --filter @matou/runtime test -- runtime-session-recovery-scheduler runtime-server
pnpm --filter @matou/desktop test -- HierarchyShell useSessionRecovery
```

Expected: FAIL，当前恢复由所有 TerminalSurface 同时 spawn，archived 节点也会被自动 restart。

- [ ] **Step 4: 实现调度器与 headless start**

Runtime 启动只完成数据库/Journal 一致性后即 ready；Scheduler 通过 Task 5 的 headless ensure-running 启动 PTY。恢复状态通过带 sessionId/sceneId 的 protocol message 广播；不为每个 job full snapshot。

- [ ] **Step 5: 接入 Scene 优先级**

HierarchyShell 初始 activeSceneId 和后续 scene activation 都调用 `prioritizeScene`；当前 scene 全部 session 排在前面，其他 scene 保持后台 FIFO。用户显式“重新启动” stopped 节点使用最高优先级单 job，但不批量重启其兄弟。

- [ ] **Step 6: 验证 GREEN 与门槛**

```bash
pnpm --filter @matou/runtime test -- recovery runtime-server
pnpm --filter @matou/desktop test -- HierarchyShell RuntimeClient
```

Expected: 最大并发 4；新活动 Scene 在可用 slot 后 100ms 内开始；后台无饥饿；删除节点不复活。

- [ ] **Step 7: Commit**

```bash
git add packages/contracts/src/protocol* apps/runtime/src/recovery apps/runtime/src/index.ts apps/runtime/src/runtime-server.ts apps/desktop/src/renderer/src/runtime apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx
git commit -m "feat: prioritize bounded session recovery"
```

**依赖：** Task 4、5、6。Task 5 提供 headless PTY，Task 6 限制 replay 内存，Task 4 防止每个状态更新补拉 snapshot。

---

## Task 8：按 Scene 挂载 xterm，并显示整卡恢复 loading

**Files:**
- Modify: `apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/HierarchyShell.test.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/TerminalPane.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/TerminalPane.test.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/hierarchy.css`
- Modify: `apps/desktop/src/renderer/src/terminal/TerminalSurface.tsx`
- Modify: `apps/desktop/src/renderer/src/terminal/TerminalSurface.test.tsx`

**Interfaces:**

```ts
interface TerminalPaneProps {
  recoveryState: 'queued' | 'restoring' | 'ready' | 'failed'
  viewAttached: boolean
}
```

- [ ] **Step 1: 写 Scene 挂载 failing tests**

3 个 Scene、每个 5 个 Session：活动 Scene 有 5 个 xterm，另外两个 Scene 为 0；切换后旧 Scene 变 0、新 Scene 变 5，15 个 PTY PID 均不变。

- [ ] **Step 2: 写整卡 loading failing tests**

queued/restoring 时 `.terminal-pane` 为 `aria-busy=true`，整卡只有名称、阶段和进度视觉，不存在可输入 textarea；ready 后同一 stable sessionId 显示 xterm；failed 显示原因和重试入口。

- [ ] **Step 3: 验证 RED**

```bash
pnpm --filter @matou/desktop test -- HierarchyShell TerminalPane TerminalSurface
```

Expected: FAIL，当前所有隐藏 Scene 都挂载 TerminalSurface，恢复只显示局部 banner。

- [ ] **Step 4: 实现按 Scene 挂载**

Scene/tab 数据和轻量卡片摘要可保留；只有 activeSceneId 的 SessionCanvas 调用真实 `renderSession`/TerminalSurface。当前 Scene 内不根据 `inViewport` 卸载 TerminalSurface。

- [ ] **Step 5: 实现整卡 loading**

loading 覆盖整个卡片内容区，阻止输入、搜索、resize 和 hover 内部焦点；卡片外形、宽度、关系徽章位置保持 Kooky。失败时退出 loading，显示单节点错误，不遮挡其他卡片。

- [ ] **Step 6: 验证 GREEN**

```bash
pnpm --filter @matou/desktop test -- HierarchyShell TerminalPane TerminalSurface SessionCanvas
pnpm exec playwright test tests/e2e/session-canvas-recovery.spec.ts tests/e2e/session-canvas-navigation.spec.ts
```

Expected: 非活动 Scene xterm=0、PTY PID 不变；切回后输出连续且不重复；当前横向列表的 offscreen 卡片仍有 xterm。

- [ ] **Step 7: Commit**

```bash
git add apps/desktop/src/renderer/src/hierarchy apps/desktop/src/renderer/src/terminal
git commit -m "perf: bind terminal views only for the active scene"
```

**依赖：** Task 5、7。

---

## Task 9：统一“移除节点…”入口和两种删除范围

**Files:**
- Modify: `apps/runtime/src/session-canvas/session-canvas-service.ts`
- Modify: `apps/runtime/src/session-canvas/session-canvas-service.test.ts`
- Modify: `apps/desktop/src/renderer/src/hierarchy/TerminalPane.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/TerminalPane.test.tsx`
- Modify: `apps/desktop/src/renderer/src/session-canvas/StoppedSessionCard.tsx`
- Modify: `apps/desktop/src/renderer/src/session-canvas/SessionCanvas.test.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/hierarchy-types.ts`
- Modify: `tests/e2e/session-canvas-lifecycle.spec.ts`

**Interfaces:**

```ts
type RemoveNodeScope = 'node-only' | 'node-and-descendants'

removeSessionBranch(
  sceneId: string,
  sessionId: string,
  scope: RemoveNodeScope
): unknown
```

- [ ] **Step 1: 写 Runtime failing tests**

覆盖叶子、普通父节点、根节点、运行中子树、聚焦节点和 detached 节点。`node-only` 时直接子节点接到原父级；原根的直接子节点成为根级；`node-and-descendants` 删除完整子树；两种范围都保留 worktree 文件。

- [ ] **Step 2: 写 UI failing tests**

断言页面只出现一个入口文案 `移除节点…`；父节点弹窗有两个 radio/choice，默认“仅移除当前节点”；叶子只显示当前节点范围；确认前列出运行中/待输入影响。

- [ ] **Step 3: 验证 RED**

```bash
pnpm --filter @matou/runtime test -- session-canvas-service
pnpm --filter @matou/desktop test -- TerminalPane StoppedSessionCard SessionCanvas
```

Expected: FAIL，当前禁止保留后代地移除父节点，且存在直接图标、失败会话移除和旧 close/delete 路径。

- [ ] **Step 4: 实现事务重连**

在同一 transaction 中读取 target parent 和 direct children，重写 direct child structural relation，撤销 target membership/relation，更新 focus，取消 Task 7 queued recovery，再 emit 一份完整 graph。任何一步失败整体回滚。

- [ ] **Step 5: 收敛 UI 入口**

移除顶栏独立 remove icon、旧 `onDelete` Session close 流程和“移除失败会话”直达动作；右键菜单、停止卡片、启动失败卡片和键盘 close request 都进入同一 `移除节点…` 弹窗。

- [ ] **Step 6: 验证 GREEN**

```bash
pnpm --filter @matou/runtime test -- session-canvas-service
pnpm --filter @matou/desktop test -- TerminalPane StoppedSessionCard SessionCanvas
pnpm exec playwright test tests/e2e/session-canvas-lifecycle.spec.ts
```

Expected: 双边投影一致、子节点保留/删除范围正确、移除后无 PTY、队列或卡片复活。

- [ ] **Step 7: Commit**

```bash
git add apps/runtime/src/session-canvas apps/desktop/src/renderer/src/hierarchy apps/desktop/src/renderer/src/session-canvas tests/e2e/session-canvas-lifecycle.spec.ts
git commit -m "feat: unify session node removal scopes"
```

**依赖：** Task 2 的 graph index、Task 4 的 event projection、Task 7 的 queue cancellation。

---

## Task 10：稳定 SessionCarousel 交互并将 resize 合并到 60Hz

**Files:**
- Create: `apps/desktop/src/renderer/src/terminal/frame-coalescer.ts`
- Create: `apps/desktop/src/renderer/src/terminal/frame-coalescer.test.ts`
- Modify: `apps/desktop/src/renderer/src/terminal/TerminalSurface.tsx`
- Modify: `apps/desktop/src/renderer/src/terminal/TerminalSurface.test.tsx`
- Modify: `apps/desktop/src/renderer/src/session-canvas/SessionCarousel.tsx`
- Modify: `apps/desktop/src/renderer/src/session-canvas/SessionCarousel.test.tsx`
- Modify: `apps/desktop/src/renderer/src/session-canvas/session-canvas.css`
- Modify: `tests/e2e/session-canvas-lifecycle.spec.ts`
- Modify: `tests/e2e/session-canvas-navigation.spec.ts`

**Interfaces:**

```ts
export function createFrameCoalescer<T>(options: {
  equals(left: T, right: T): boolean
  flush(value: T): void
}): { schedule(value: T): void; cancel(): void }
```

- [ ] **Step 1: 写 resize failing tests**

同一帧触发 20 次 ResizeObserver，只用最后尺寸执行一次 `fit`/resize RPC；下一帧尺寸相同不发送；卸载取消 pending frame。

- [ ] **Step 2: 固化 Kooky 动画合同 tests**

保留当前 transition duration/easing、活动卡最大宽度、悬停交接、pointer leave 回位和父级右拉轨迹。增加 stationary pointer 横向滚动 100ms 重定向和 leave 300ms 收起测试。

- [ ] **Step 3: 验证 RED**

```bash
pnpm --filter @matou/desktop test -- frame-coalescer TerminalSurface SessionCarousel
pnpm exec playwright test tests/e2e/session-canvas-lifecycle.spec.ts --grep "stationary|expanded|scroll"
```

Expected: RED，复现现有 100ms 未命中和 15s 未收起问题。

- [ ] **Step 4: 实现一帧一次 resize**

TerminalSurface、SceneTabBar 和 SessionCarousel 的 ResizeObserver 使用同一 coalescer；只在最终 cols/rows 改变时发送 Runtime resize。

- [ ] **Step 5: 收敛 Carousel 状态机**

wheel listener 只注册一次并通过 refs 读取最新状态；每帧最多一次 `elementFromPoint`、一次 visible-window 更新和一次 geometry candidate。取消逻辑集中清理所有 hover rAF/timer。当前 Scene 的所有卡片继续 render，不使用 xterm windowing。

- [ ] **Step 6: 隔离 React 更新**

`SessionCard`/TerminalPane 使用稳定 key 和 memo；projection 中未变化节点引用不变；scrollLeft、pointer 和中间动画帧留在 refs/CSS，settle 后才提交 React/persistence。

- [ ] **Step 7: 验证 GREEN 与门槛**

```bash
pnpm --filter @matou/desktop test -- SessionCarousel TerminalSurface
pnpm exec playwright test tests/e2e/session-canvas-lifecycle.spec.ts tests/e2e/session-canvas-navigation.spec.ts
pnpm test:scale --grep "50 sessions|200 sessions"
```

Expected: 100ms retarget、300ms 收起、500ms persistence；50 p95 frame≤16.7ms，200≤33.3ms；动画截图/轨迹与 Kooky 基线不变。

- [ ] **Step 8: Commit**

```bash
git add apps/desktop/src/renderer/src/terminal apps/desktop/src/renderer/src/session-canvas apps/desktop/src/renderer/src/hierarchy/SceneTabBar.tsx tests/e2e/session-canvas-*.spec.ts
git commit -m "perf: coalesce canvas layout work at frame rate"
```

**依赖：** Task 2、4、8。Task 8 确保只有活动 Scene 参与 xterm resize。

---

## Task 11：实现 DAG 分支/层聚合和帧内平移

**Files:**
- Create: `apps/desktop/src/renderer/src/dag/dag-render-model.ts`
- Create: `apps/desktop/src/renderer/src/dag/dag-render-model.test.ts`
- Modify: `apps/desktop/src/renderer/src/dag/dag-layout.ts`
- Modify: `apps/desktop/src/renderer/src/dag/dag-layout.test.ts`
- Modify: `apps/desktop/src/renderer/src/dag/DagCanvas.tsx`
- Modify: `apps/desktop/src/renderer/src/dag/DagCanvas.test.tsx`
- Modify: `apps/desktop/src/renderer/src/dag/DagWindowApp.tsx`
- Modify: `apps/desktop/src/renderer/src/dag/DagWindowApp.test.tsx`
- Modify: `tests/e2e/session-dag-window.spec.ts`

**Interfaces:**

```ts
export type DagRenderItem =
  | { kind: 'node'; key: string; sessionId: string; positioned: PositionedDagNode }
  | { kind: 'aggregate'; key: string; depth: number; branchRootId: string; count: number; bounds: Rect }

export interface DagRenderModel {
  items: readonly DagRenderItem[]
  edges: readonly PositionedDagEdge[]
  nodeById: ReadonlyMap<string, PositionedDagNode>
}

export function buildDagRenderModel(
  layout: DagLayout,
  viewport: Rect,
  focusSessionId: string,
  scale: number
): DagRenderModel
```

- [ ] **Step 1: 写聚合 failing tests**

10000 宽图、深图和混合分支：父/当前/子可视节点完整；远层按 `(depth, branchRootId)` 聚合；相同 graph/pan/zoom 的 key 稳定；聚合进入 overscan 后自动替换为真实节点。

- [ ] **Step 2: 写 DOM 和手势 failing tests**

10000 节点时 node+aggregate≤400、edge≤800；100 次 pointermove/wheel 最多触发对应帧数的 render；edge lookup 不调用 `layout.nodes.find`。

- [ ] **Step 3: 验证 RED**

```bash
pnpm --filter @matou/desktop test -- dag-render-model DagCanvas DagWindowApp
```

Expected: FAIL，当前所有远层 ghost node 仍进入 DOM，edge lookup 为 O(E×N)。

- [ ] **Step 4: 实现 render model**

layout 阶段建立 `nodeById`、depth bucket 和 branch root。视口 full node 使用 overscan；远层只产出 aggregate，aggregate count/status/notified 使用预聚合数据。

- [ ] **Step 5: 实现 rAF transform**

手势期间只更新 `dag-world.style.transform` 和 ref；每动画帧重算一次 render model；pointerup/wheel settle 后写 React transform 与 geometry。保留 40%–200%、聚焦和 Kooky 动画。

- [ ] **Step 6: 删除 500ms 全图轮询**

DagWindowApp 订阅 Task 4 的 graph revision/event；revision 未变不 stringify nodes/latestLines/edges。断线时保留最后模型并显示现有错误提示。

- [ ] **Step 7: 验证 GREEN 与门槛**

```bash
pnpm --filter @matou/desktop test -- dag
pnpm exec playwright test tests/e2e/session-dag-window.spec.ts
pnpm test:scale --grep "10000 DAG"
```

Expected: 10000 节点首次可操作 p95≤300ms，DOM/edge 上限满足，pan/zoom p95≤16.7ms，搜索≤100ms。

- [ ] **Step 8: Commit**

```bash
git add apps/desktop/src/renderer/src/dag tests/e2e/session-dag-window.spec.ts
git commit -m "perf: aggregate distant dag branches"
```

**依赖：** Task 2 的索引方法、Task 4 的增量 graph revision。

---

## Task 12：闭合 50/200/1000、5000 深链和 10000 DAG 发布门槛

**Files:**
- Modify: `tests/e2e/scale/scale-database.ts`
- Modify: `tests/e2e/scale/scale-metrics.ts`
- Modify: `tests/e2e/scale/scale-benchmark.spec.ts`
- Modify: `tests/e2e/session-canvas-recovery.spec.ts`
- Modify: `docs/audits/2026-09-01-internal-hardening-analysis.md` only to append verified post-fix measurements during implementation

- [ ] **Step 1: 将全局门槛写成 failing assertions**

每个数据集输出 JSON artifact，断言本计划“性能门槛”全部满足；禁止只打印数字不判定。

- [ ] **Step 2: 验证 RED**

```bash
pnpm test:scale
```

Expected: 在尚未整合全部任务或存在回退时明确列出失败指标、样本规模和超额值。

- [ ] **Step 3: 逐项只修实现，不放宽预算**

修复应回到对应 Task 的生产代码和单元测试；基准代码只在计量错误时修改。若硬件噪声，增加样本和 warm-up，不删除 p95/max。

- [ ] **Step 4: 跑完整验证矩阵**

```bash
pnpm typecheck
pnpm test
pnpm test:e2e
pnpm test:scale
```

Expected:

- 单元/组件、现有真实 E2E 全绿。
- 50/200/1000 Session 真实 Electron 数据集满足对应分层门槛。
- 5000 深链无崩溃且结构计算≤50ms。
- 10000 DAG 满足 DOM、首交互、搜索和 60Hz pan/zoom 门槛。
- stationary pointer、pointer leave、geometry persistence 恢复为稳定通过。
- 非活动 Scene xterm=0，但 PTY PID 和 Journal 连续。
- 恢复并发≤4、活动 Scene 优先、后台无饥饿。

- [ ] **Step 5: 记录验收证据**

将机器信息、五轮原始 JSON、p50/p95/max、截图/trace、PTY/RSS 峰值和与本次审计基线的差值追加到审计报告；不得用 SSR 数字替代 Electron 数字。

- [ ] **Step 6: Commit**

```bash
git add tests/e2e/scale tests/e2e/session-canvas-recovery.spec.ts docs/audits/2026-09-01-internal-hardening-analysis.md
git commit -m "test: enforce scale performance release gates"
```

**依赖：** Task 1–11 全部完成。

---

## 关键依赖图

```text
Task 1 真实基准 ───────────────┬───────────────┬───────────────┐
Task 2 迭代索引 ───────┬───────┼──────┐        │               │
Task 3 集合快照 ──> Task 4 增量投影 ──┼──> Task 10 Carousel  │
Task 5 PTY/view 解耦 ──────────┼──> Task 8 Scene/xterm ─────┤
Task 6 Journal range ──┐       │                            │
Task 4 + 5 + 6 ──> Task 7 恢复调度 ──┘                      │
Task 2 + 4 + 7 ──> Task 9 移除节点                           │
Task 2 + 4 ───────────────> Task 11 DAG 聚合                 │
Task 1–11 ───────────────────────────────> Task 12 发布门槛 ─┘
```

## 推荐执行批次

1. **基础批次：** Task 1、2、3、5 可并行；各自独立 review。
2. **数据路径批次：** Task 4 依赖 3；Task 6 可与 Task 4 并行。
3. **恢复批次：** Task 7 依赖 4/5/6；Task 8 依赖 5/7。
4. **交互批次：** Task 9 依赖 2/4/7；Task 10 依赖 2/4/8；Task 11 依赖 2/4，三项可在前置完成后并行。
5. **发布批次：** Task 12 串行收口，不与产品逻辑修改并行。

## 自审结果

- 已覆盖：投影 N+1、正常路径全量刷新、结构共享、深链恢复、非活动 Scene xterm 解绑、PTY 保活、有界恢复、活动 Scene 优先、整卡 loading、删除范围、Kooky 动画、60Hz resize、DAG 聚合和五档真实基准。
- 产品决策无缺口：当前横向列表未采用 viewport xterm windowing；非活动 Scene 与当前 Scene 的资源语义明确分离。
- 类型一致：`RecoveryState`、`RemoveNodeScope`、`TerminalSpawnViewMode` 和 `DagRenderModel` 仅定义一次，后续任务沿用同名接口。
- 测试边界明确：每个生产任务先有预期失败，再最小实现并跑局部回归；最终由 Task 12 执行全矩阵。
