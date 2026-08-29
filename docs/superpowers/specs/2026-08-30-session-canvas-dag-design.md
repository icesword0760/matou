# Matou 会话画布与 DAG 分支交互设计规格

**日期：** 2026-08-30
**状态：** 已由产品对话确认，进入实施
**范围：** `PRD-Matou-会话画布与DAG分支交互.md` 全量范围
**产品栈：** Electron + React + xterm.js + Runtime utility process + SQLite + node-pty

## 1. 目标

把事项内的终端从传统分栏升级为“多画布、多层会话关系、日常横向浏览、全局 DAG 导航”的工作组织方式，同时保留普通 Shell 的即时使用体验。

用户最终可以：

1. 在一个事项内创建多张独立会话画布，每张画布以普通 Shell 开始。
2. 在任意同层列表横向新增 Shell 兄弟会话。
3. 在形成有效对话的 Claude Code 会话上创建 Fork 子会话或 Fork 兄弟会话。
4. 创建 Fork 时选择共享当前工作树，或使用新的 Git worktree 隔离修改。
5. 在一屏最多四个会话的横向列表中浏览、聚焦和继续输入。
6. 通过第二段右拉手势返回父会话，不与普通横向滚动混在一次手势内。
7. 长按 `Option + Tab` 打开独立于主窗口边界的 DAG 关系画布，平移、缩放、搜索并跳转节点。
8. 在用户主动退出 Claude Code、Claude 恢复失败、应用重启和单节点异常后，保留真实会话关系。

## 2. 权威来源与优先级

实现按以下顺序解释产品行为：

1. 本规格中已经收敛的产品决策。
2. `docs/prd/PRD-Matou-会话画布与DAG分支交互.md`。
3. `docs/prd/mockups/Matou-会话画布与DAG分支交互.html` 的视觉与手势演示。
4. `docs/test/E2E-Matou-会话画布与DAG分支交互.md` 的完整用户旅程与异常验收。
5. 已交付的 PRD 01–06 行为，尤其是通知、状态 HUD、持久化、四级层级与 Claude Fork。
6. Kooky 的终端创建、聚焦、持久化和 Claude 会话身份行为；当新 PRD 明确升级了交互时，以新 PRD 为准。

测试追踪矩阵位于：

- `docs/test/TRACE-Matou-会话画布与DAG分支交互.md`

## 3. 产品边界

### 3.1 本期包含

- 事项内多 Scene 会话画布。
- 新画布默认 Shell 和自动聚焦。
- 稳定会话节点身份与可变运行形态。
- 根级、普通父子、普通兄弟、Fork 父子和 Fork 兄弟关系。
- 横向同层会话列表、最大四列、悬浮扩展、滚动和第二段右拉返回。
- 基于真实用户交互的同层动态排序。
- Claude Fork 的当前工作树/新工作树流程。
- 子会话数量、类型构成和聚合状态徽章。
- Claude Code 恢复失败与重试。
- 独立 DAG 窗口、三层默认视野、远层虚影、平移、缩放、搜索、实时摘要和跳转。
- 历史节点、独立终端窗口、主题和应用重启下的关系恢复。
- 100 节点画布的交互质量门槛。

### 3.2 本期边界外

- 跨事项或跨工作空间连边。
- 自动合并或自动解决不同 worktree 的代码冲突。
- 多人实时共同编辑 DAG。
- 自动删除 worktree 或用户未提交修改。
- 纵向新增会话入口。
- 关系节点的自由手工连线。

## 4. 已收敛的产品语义

### 4.1 会话节点与运行形态分离

`Session` 是稳定的关系节点。节点的 `currentMode` 在 `shell` 与 `claude-code` 之间变化，父子关系、兄弟归属、画布归属和历史不随模式变化而重建。

因此：

- Shell 与 Claude Code 都可能拥有父节点、子节点和兄弟节点。
- 同一个兄弟列表可同时包含 Shell 和 Claude Code。
- 用户主动退出 Claude Code 后，节点回到 Shell，不展示“Claude Code 已退出”提示。
- Claude Code 恢复失败后，节点回到 Shell，展示“Claude Code 恢复失败”和“重试恢复”。
- 两种变化都保留节点已有的父子关系和子节点。
- 当前为 Shell 时隐藏新的 Fork 入口；重新形成有效 Claude Code 对话后重新获得 Fork 能力。

### 4.2 关系类型

结构关系只有两种边：

| 关系 | 用户含义 | 数据关系 |
|---|---|---|
| 普通父子 | 子会话属于该父会话下的一项并行工作 | `derived-from` |
| Claude Fork | 子会话继承父 Claude Code 对话上下文 | `forked-from` |

兄弟不是单独持久化的边。共享同一个结构父节点的会话互为兄弟；根级会话共享画布起点。

每个非根节点恰好拥有一个结构父节点。`derived-from` 与 `forked-from` 合并计算这一约束，关系图保持无环。

### 4.3 三种新增入口

1. **画布 Tab `+`**：创建新画布，自动创建一个根级 Shell。
2. **同层横向 `+`**：直接创建 Shell 兄弟，不弹出类型或工作树选择。
3. **Claude 分支按钮**：有效 Claude Code 会话可创建 Fork 子会话；非根同层列表可从共同父 Claude Code 创建 Fork 兄弟。

根级没有真实 Claude 共同父节点，因此根级列表隐藏“Fork 兄弟”；根级 Claude 仍可创建自己的 Fork 子节点。

纯 Shell 且没有子列表时只显示同层 Shell 新增。一个节点已有子列表时，子列表内的横向 `+` 可继续添加普通 Shell 子节点，它们共享该父节点。

### 4.4 有效 Claude Code 对话

Fork 入口进入可用状态需同时满足：

1. 当前进程被识别为 Claude Code。
2. 用户已经发送至少一条实际消息。
3. Runtime 收到 provider 语义事件或 hook 对应的会话身份。
4. 首轮对话收到正常完成事件。

仅出现临时 statusline 标识、首轮中断或首轮失败时，Fork 入口保持置灰，并解释“完成首轮对话后可创建分支”。

### 4.5 手动退出与恢复失败

| 场景 | 当前形态 | 用户反馈 | 再次启动应用 |
|---|---|---|---|
| 用户在 Claude Code 中主动退出 | Shell | 正常 Shell 界面 | 按 Shell 恢复 |
| 应用恢复 Claude 绑定失败 | Shell | 恢复失败说明与重试按钮 | 保留失败状态，可再次重试 |
| 重试恢复中 | Claude 恢复中 | 禁用重复提交，展示进度 | 成功进入 Claude，失败回到可重试状态 |

恢复失败属于节点附加状态，不创建新节点。恢复成功继续使用原节点、原 provider 身份和原关系。

### 4.6 子会话徽章与历史

父会话标题显示：

- 活跃直接子会话数量。
- `Claude N · Shell M` 形态构成。
- 最高优先级聚合状态。
- 存在已结束历史子节点时显示 `+H 历史`。

点击徽章默认进入活跃子会话列表；“显示历史”开关把已结束节点加入列表。DAG 始终保留历史节点，并以降低对比度的卡片展示。

### 4.7 工作状态

节点状态为：

```text
starting | idle | running | needs-input | error | interrupted | exited
```

聚合优先级：

```text
error/恢复失败 > needs-input > running > interrupted > idle > exited
```

Shell 判断：前台进程与任务存活时为 `running`；提示符等待输入为 `idle`；进程请求用户输入时为 `needs-input`；启动失败为 `error`；应用关闭时尚未完成的进程在恢复画面中为 `interrupted`。

Claude 判断：语义事件或 hook 表示生成/工具执行为 `running`，许可/选择/文本输入请求为 `needs-input`，正常轮次结束为 `idle`，provider 或恢复异常为 `error`。

### 4.8 动态排序

每个兄弟列表按 Runtime 生成的 `lastUserInteractionSeq` 倒序显示。

更新顺序的事件：

- Shell 提交命令。
- Shell 发送中断、结束输入等控制操作。
- Claude Code 发送消息。
- Claude Code 完成授权、拒绝、选项、确认、停止或继续操作。

点击、聚焦、滚动、复制、选择文本、草稿输入、后台输出、Claude 回答、通知和 DAG 查看不更新顺序。

新会话追加到队列末尾。应用整体恢复保留原顺序；用户主动重新打开历史会话时追加到末尾。

`lastUserInteractionSeq` 由 Runtime 在 SQLite 事务中单调递增。值相同时按同层创建顺序，再按 Session ID 稳定排序。

## 5. 用户旅程

### 5.1 新画布

1. 用户点击事项顶部 Tab 栏 `+`。
2. 新 Scene 追加到 Tab 末尾并成为当前画布。
3. Runtime 在当前 Workspace execution context 中创建根级 Shell。
4. 终端出现后自动聚焦，用户可直接输入。
5. 创建失败时画布保留，展示失败原因和重新创建入口。

### 5.2 普通 Shell 兄弟

1. 用户在当前同层列表点击横向 `+`。
2. Runtime 使用该列表的结构父节点；根层使用 `null`。
3. 新 Shell 追加到末尾，不展示弹框。
4. 列表滚动到新 Shell，终端自动聚焦。
5. 用户提交首条命令后，新 Shell 平滑移动到最前且保持可见和聚焦。

### 5.3 Claude Fork 子会话

1. 用户点击有效 Claude 标题行的分支按钮。
2. 弹窗要求分支显示名称，并选择当前工作树或新工作树。
3. 输入去除首尾空白后为 1–64 个 Unicode 字符，同一父节点下活跃直接子节点名称区分大小写且唯一。
4. 用户选择当前工作树时复用 execution context。
5. 用户选择新工作树时，从当前 Git worktree 的 `HEAD` 创建独立目录和分支；原目录未提交修改留在原处。
6. Runtime 先创建准备中节点，再执行 worktree 与 provider Fork。
7. 成功后显示 Claude 子会话并自动聚焦；失败时节点保留错误和重试/移除入口。

Git 分支名由 Runtime 生成 `matou/<slug>-<shortid>`，用户输入始终作为显示名称保存；Git 字符限制和名称碰撞由生成器处理。

### 5.4 浏览子会话

1. 用户点击父会话的子会话徽章。
2. 主区域切换为直接子会话兄弟列表。
3. 一屏最多横向显示四张会话卡片。
4. 触摸板或鼠标滚轮平滑左右滚动，其余会话按需进入视野。
5. 指针停留在卡片上时，该卡片扩展到适合阅读的位置；移开后恢复。
6. 终端内容、选择、输入和滚动继续由 xterm 处理。

### 5.5 第二段右拉返回父会话

1. 普通横向滚动先到列表最左边并结束。
2. 用户开始新的向右拖拽。
3. 父会话投影随阻尼逐步出现，子列表产生弹力位移。
4. 未过阈值松手时回弹到子列表。
5. 达到阈值后松手时切换到父会话所在兄弟列表，父会话完整进入视野并获得焦点。

同一次超量滚动只负责到达边缘，不触发返回。阈值采用容器宽度的 22%，最小 96px，最大 180px。

### 5.6 DAG 导航

1. 应用在前台时，用户长按 `Option + Tab` 450ms。
2. 450ms 内释放视为短按，向当前终端转发一次 Tab；达到阈值后消费该次 Tab 并打开 DAG 窗口。
3. 键盘自动重复事件不重复打开窗口。
4. DAG 默认展示当前节点、父层、当前层和直接子层；更远层以边缘虚影表示。
5. 用户可平滑平移、触摸板缩放、`⌘+/-/0` 缩放或使用画布控件。
6. 用户可按名称、目录、worktree 分支或最近语义摘要搜索，选中结果后居中节点。
7. 点击节点后 DAG 窗口关闭，主窗口切到目标 Scene 和目标兄弟列表，并确保节点完整进入视野。
8. 目标为已脱出的会话时，系统激活对应独立终端窗口。

除快捷键外，Scene Tab 尾部提供图形入口，名称为 `会话关系 (⌥Tab)`，支持键盘聚焦。

## 6. 进程与权威边界

```text
Renderer main / Renderer DAG / Renderer detached
      | 控制 RPC + 有序投影事件
      v
Runtime utility process ── SQLite / Outbox / Journal / PTY / provider hooks
      ^
      | 每个 Renderer 独立 MessagePort；终端字节不经过 Main
      |
Electron Main ── BrowserWindow 生命周期、DAG 窗口与窗口激活
```

硬约束：

- Runtime 独占 SQLite 写连接并提供权威状态。
- 结构写入在一个事务中同时修改领域表并写 Outbox。
- Renderer 只维护可重建投影。
- DAG 位置、缩放、列表滚动等几何状态防抖写入 geometry 表，不进入 Outbox。
- Electron Main 只移交端口和管理原生窗口。
- PTY 输出、输入和流控保持 Renderer 与 Runtime 直连。

## 7. 领域模型

### 7.1 新增类型

```ts
export type SessionCurrentMode = 'shell' | 'claude-code'
export type SessionWorkStatus =
  | 'starting' | 'idle' | 'running' | 'needs-input'
  | 'error' | 'interrupted' | 'exited'

export type ProviderRestoreState = 'none' | 'restoring' | 'failed'

export interface SessionCanvasMembership {
  sessionId: SessionId
  sceneId: SceneId
  siblingCreatedSeq: number
  lastUserInteractionSeq: number
  createdAt: number
  updatedAt: number
}

export interface SessionGraphNode {
  sessionId: SessionId
  sceneId: SceneId
  parentSessionId?: SessionId
  relationKind?: 'derived-from' | 'forked-from'
  currentMode: SessionCurrentMode
  workStatus: SessionWorkStatus
  providerRestoreState: ProviderRestoreState
  title: string
  cwd: string
  worktree?: { branch: string; path: string; shared: boolean }
  activeChildCount: number
  historicalChildCount: number
  childModeCounts: { shell: number; claudeCode: number }
  latestLines: string[]
  lastUserInteractionSeq: number
  archivedAt?: number
  detachedWindowId?: string
}

export interface SessionGraphEdge {
  parentSessionId: SessionId
  childSessionId: SessionId
  relationKind: 'derived-from' | 'forked-from'
  createdAt: number
}

export interface SceneSessionGraph {
  sceneId: SceneId
  nodes: SessionGraphNode[]
  edges: SessionGraphEdge[]
  focusedSessionId?: SessionId
}
```

已有 `Session.kind` 表示当前进程形态并继续兼容 PRD 01–06。关系代码统一使用 `currentMode` 投影名称，避免把当前形态误解为永久节点类型。

### 7.2 存储迁移

在当前 migration 13 后新增 migration 14：

```sql
CREATE TABLE session_canvas_memberships (
  session_id TEXT PRIMARY KEY REFERENCES sessions(id),
  scene_id TEXT NOT NULL REFERENCES scenes(id),
  sibling_created_seq INTEGER NOT NULL,
  last_user_interaction_seq INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
) STRICT;

CREATE INDEX session_canvas_memberships_scene_idx
ON session_canvas_memberships(scene_id, last_user_interaction_seq DESC, sibling_created_seq ASC);

CREATE TABLE runtime_sequences (
  name TEXT PRIMARY KEY,
  value INTEGER NOT NULL
) STRICT;

INSERT INTO runtime_sequences(name, value)
VALUES ('session-user-interaction', 0), ('session-sibling-created', 0);

ALTER TABLE provider_bindings ADD COLUMN restore_state TEXT NOT NULL DEFAULT 'none'
CHECK (restore_state IN ('none', 'restoring', 'failed'));

ALTER TABLE provider_bindings ADD COLUMN restore_error TEXT;
ALTER TABLE provider_bindings ADD COLUMN user_exited_at INTEGER;
```

迁移回填：

1. 每个 Session 从最早的 `session_mounts` 找到所属 Scene，写入 membership。
2. 同一 Scene 中没有结构父节点的 Session 作为根级兄弟。
3. 现有 `forked-from` 关系保持原样。
4. 旧分栏树只用于计算首次稳定创建顺序；迁移后会话列表由 membership 与关系投影驱动。
5. 历史和 live PTY 记录原样保留。

### 7.3 关系仓储

`SessionRelationRepository` 新增统一结构父节点能力：

```ts
getStructuralParent(sessionId: string): SessionRelation | undefined
listStructuralChildren(parentSessionId: string, options?: { includeArchived?: boolean }): SessionRelation[]
listSiblings(sessionId: string, options?: { includeArchived?: boolean }): Session[]
appendStructuralRelation(input: {
  childSessionId: string
  parentSessionId: string
  kind: 'derived-from' | 'forked-from'
}): SessionRelation
```

写入时验证：

- child 与 parent 属于同一 Scene、Task 和 Workspace。
- child 尚无 `derived-from` 或 `forked-from` 结构父关系。
- child 与 parent 不同。
- 从 parent 向上遍历不会到达 child。

关系事件继续 append-only，current projection 保持查询效率。

## 8. Runtime 应用服务

新建 `SessionCanvasService`，从当前过大的 hierarchy service 中隔离画布关系工作流。

```ts
export class SessionCanvasService {
  createCanvas(command: DomainCommandMetadata, input: CreateCanvasInput): Promise<CreateCanvasResult>
  createShellSibling(command: DomainCommandMetadata, input: CreateShellSiblingInput): Promise<CreateSessionResult>
  createForkChild(command: DomainCommandMetadata, input: CreateForkInput): Promise<CreateForkResult>
  createForkSibling(command: DomainCommandMetadata, input: CreateForkSiblingInput): Promise<CreateForkResult>
  recordUserInteraction(command: DomainCommandMetadata, input: RecordUserInteractionInput): SessionCanvasMembership
  retryProviderRestore(command: DomainCommandMetadata, input: RetryProviderRestoreInput): Promise<RetryProviderRestoreResult>
  reopenHistoricalSession(command: DomainCommandMetadata, input: ReopenHistoricalSessionInput): Promise<Session>
  projectSceneGraph(sceneId: string, windowId: string): SceneSessionGraph
}
```

### 8.1 原子工作流

创建普通 Shell 在一次事务中完成：

1. 分配 Session、mount、membership 和序号。
2. 非根节点写入 `derived-from` 关系事件和 current projection。
3. 写入 session/mount/graph 领域事件。
4. 提交后启动 PTY；启动失败回写当前节点 `error`，其余节点继续运行。

创建 Fork 分两段：

1. 事务创建 `starting` 节点、membership、关系意图和 worktree intent。
2. 事务外执行真实 Git worktree 与 provider Fork。
3. 成功事务绑定 execution context/provider identity 并进入 `idle`。
4. 失败事务保留节点、错误原因、重试策略和可移除状态。

### 8.2 新 worktree

复用现有 `WorktreeService.create()`：

- `repositoryRoot` 由 Runtime 对源 execution context 执行 `git rev-parse --show-toplevel` 得到。
- `baseRef` 为源 worktree 当前 `HEAD`。
- 路径位于 Matou 管理目录 `worktrees/<workspace-id>/<session-id>`。
- 分支名由显示名 slug 与短 Session ID 生成。
- worktree ready 后再启动 Claude Fork。
- setup script 失败时保留 `failed` worktree 和创建节点，提供真实重试。

### 8.3 用户交互排序

Renderer 检测 xterm 输入中的提交与控制字节，在对应终端输入写入前发送：

```ts
interface TerminalUserInteractionMessage {
  type: 'terminal.user-interaction'
  sessionId: string
  interactionKind: 'submit' | 'control' | 'provider-action'
}
```

Runtime 校验 Session 与端口归属，在 SQLite 事务中递增 `session-user-interaction`，更新 membership 并发出 `session.user-interacted`。普通字符、鼠标事件和输出流不发送该消息。

顺序变化后 Renderer 使用 FLIP 动画移动卡片，同时保持 xterm 实例、选择、焦点和 PTY 不变。

### 8.4 恢复状态

Runtime 启动时：

1. 当前 mode 为 Shell 的节点按 Shell 恢复，不尝试 provider 恢复。
2. 当前 mode 为 Claude 且 provider binding 有效的节点进入 `restoring`。
3. 成功后清空 restore error 并进入 Claude `idle`。
4. 失败后将当前 mode 设为 Shell，binding 标记 `failed`，保留原 provider 身份和错误。
5. 用户点击重试时使用同一个 binding；并发点击通过 command idempotency 与 `restoring` 状态合并。
6. 用户在 Claude 中主动退出时，将 mode 设为 Shell并记录 `user_exited_at`，清空恢复错误。

## 9. RPC 与投影协议

### 9.1 RPC

新增方法：

```text
hierarchy.create-canvas
hierarchy.create-shell-sibling
hierarchy.create-fork-child
hierarchy.create-fork-sibling
hierarchy.record-session-interaction
hierarchy.retry-provider-restore
hierarchy.reopen-historical-session
hierarchy.get-scene-session-graph
hierarchy.set-focused-session
```

现有 `hierarchy.fork-session` 在兼容期映射到 `create-fork-child`。

### 9.2 投影

`projection.snapshot` 增加：

```ts
sessionGraphs: Record<SceneId, SceneSessionGraph>
```

有序事件支持增量更新：

```text
scene.canvas-created
session.canvas-membership-created
session.structural-relation-created
session.user-interacted
session.mode-changed
session.restore-state-changed
session.graph-summary-changed
session.historical-state-changed
```

恢复缺口仍使用现有序号检测与全量投影重建。DAG Renderer 与主 Renderer 读取同一个 Runtime 权威投影。

## 10. 主界面结构

新增聚焦组件：

```text
SessionCanvas
  SceneTabBar
  SessionLevelHeader
  SessionCarousel
    ParentProjection
    SessionCard[]
      SessionHeader
      TerminalPane / DetachedPlaceholder / HistoricalSummary
    HorizontalCreateButton
  BranchDialog
```

### 10.1 会话卡片

- 一屏 1–4 个会话时平均分配可用宽度。
- 超过四个时默认卡片宽度为容器宽度的 25%，保留 12px 间距。
- 悬浮卡片目标宽度为 `clamp(420px, 44vw, 760px)`，相邻卡片平滑让位。
- 活跃卡片边框与标题强调，兄弟卡片保持可识别但不抢焦点。
- 列表滚动时停止 hover 扩展，滚动结束 120ms 后恢复。
- 仅可见卡片挂载 xterm DOM；离开视野后 Runtime PTY、Journal 和 Session 保持运行，再进入视野时从 marker/checkpoint 增量恢复。

### 10.2 标题行

Shell 标题行：名称、状态、cwd/worktree、已有子会话徽章、横向兄弟新增。

有效 Claude 标题行额外显示 Fork 子会话按钮。存在共同 Claude 父节点时，同层列表操作区显示 Fork 兄弟入口。

恢复失败节点显示醒目的恢复说明与重试按钮，关系徽章继续可用。

### 10.3 焦点

以下行为完成后自动聚焦 xterm：

- 新建画布。
- 新建 Shell 兄弟。
- Fork 成功。
- 点击 DAG 节点回到主界面。
- 父子层级切换。
- Claude 完成本轮回答，前提是用户没有主动聚焦其他应用或其他会话。

Renderer 通过 session focus token 避免异步输出抢走用户已经转移的焦点。

## 11. 手势状态机

`ParentPullController` 状态：

```text
idle -> horizontal-scroll -> edge-armed -> parent-pull -> committed
                                      \-> cancelled
```

规则：

- `horizontal-scroll` 只更新 `scrollLeft`。
- 一次手势触达最左仅在结束时把状态置为 `edge-armed`。
- 下一次手势从最左开始且首个有效位移向右时进入 `parent-pull`。
- 垂直位移占优、xterm 内部选择或修饰键组合时留给终端/原生滚动。
- 阻尼公式 `visible = distance * 0.55 * (1 - min(distance / width, 0.65))`。
- 达阈值后松手提交，未达阈值使用 260ms spring 回弹。
- 列表没有父节点时只显示边缘阻尼，不执行层级切换。

## 12. 独立 DAG 窗口

### 12.1 原生窗口

Electron Main 管理唯一 DAG BrowserWindow：

- `kind='dag'`，默认 1240×760，最小 760×480。
- hidden title bar、圆角背景和系统阴影。
- 显示在当前主窗口所在显示器的可用区域中央。
- 再次长按快捷键时聚焦已有 DAG 窗口并更新当前场景。
- DAG 窗口直接连接 Runtime，不经 Main 转发数据。
- 关闭或选择节点后销毁窗口，不影响 PTY。

Preload 暴露窄接口：

```ts
openDagWindow(input: { windowId: string; sceneId: string; focusedSessionId: string }): Promise<void>
selectDagNode(input: { sourceWindowId: string; sceneId: string; sessionId: string }): Promise<void>
closeDagWindow(): Promise<void>
```

### 12.2 DAG 画布

使用 React DOM + SVG edges 的可虚拟化画布：

- 节点层为 GPU transform 的绝对定位卡片。
- 边层为独立 SVG，按可见节点计算路径。
- 默认完整渲染当前节点父层、同层、子层。
- 远层只渲染聚合虚影；平移接近时替换为真实节点。
- 节点摘要来自 Runtime journal/semantic projection 最近四行，不创建 xterm 实例。
- 实时摘要以 250ms 合并频率更新。
- 缩放范围 `0.35–1.8`，默认 `1`，每次滚轮缩放以指针位置为中心。
- 平移与缩放保持 60fps 目标；100 节点连续操作无输入阻塞。

布局采用稳定分层：结构深度决定 X 轴，兄弟顺序决定 Y 轴；用户动态排序只影响主列表，不重排 DAG 稳定位置。新节点在同层末尾获得稳定 layout sequence。

### 12.3 搜索与选择

搜索字段匹配：节点标题、cwd、worktree branch 和最近语义摘要。结果按精确标题、前缀、包含、最近活动依次排序。

选择结果先平滑居中；按 Enter 或点击卡片执行导航。主窗口收到导航后：

1. 激活 Workspace、Task 和 Scene。
2. 计算目标结构父节点对应的兄弟列表。
3. 更新 focused Session。
4. `scrollIntoView({ block: 'nearest', inline: 'center' })`；边缘空间不足时使用最小位移。
5. 等待 xterm fit 后聚焦。

## 13. 几何与持久化

使用现有 geometry 存储，owner key：

```text
session-group:<scene-id>:root
session-group:<scene-id>:<parent-session-id>
dag-viewport:<scene-id>
dag-node-layout:<scene-id>:<session-id>
```

数据分别保存横向 `scrollLeft/focusedSessionId`、DAG `panX/panY/zoom` 和稳定节点位置。写入防抖 180ms；应用退出前执行最后一次 flush。

结构状态、membership、关系、恢复状态和用户交互序号进入 SQLite 与 Outbox。hover、拖拽中间帧、文本选择和弹力进度只存在 Renderer 内存。

## 14. 独立窗口、通知和主题

### 14.1 独立终端窗口

复用 PRD 05 的 `↗ 独立窗口`：

- 原 Scene 显示同一 Session 的占位卡片。
- 关闭独立窗口后，同一 live Session 回到原兄弟列表并保持关系。
- DAG 点击已脱出节点时激活独立窗口。
- 原 Scene 已归档时应用 PRD 05 的返回规则。

### 14.2 通知

复用 PRD 01 视觉语言。恢复失败、需要输入和运行错误产生对应通知；点击通知导航到节点并确保其进入视野。相同恢复失败重试只更新现有通知，避免重复堆叠。

### 14.3 主题

主界面和 DAG 共享明暗主题 token。浅色主题继续对照 Kooky 保持列表层级、选中态和边界对比度；DAG 连线与历史虚影在两种主题下均达到清晰可辨。

## 15. 应用生命周期

- 关闭最后一个主窗口：隐藏窗口，Runtime 与会话继续运行。
- `Cmd+Q`、菜单退出或 Dock 退出：执行完整关闭；未完成 Shell 标记中断，Claude 在下次启动时尝试恢复。
- 进程崩溃或强制结束：使用现有 generation/stale-run 机制恢复，语义与完整退出一致。
- 整体应用恢复保留兄弟顺序；不把恢复动作视为新的用户交互。
- 创建 worktree/Fork 中退出：节点恢复为“创建已中断”，提供真实重试或移除。

## 16. 异常与隔离

| 异常 | 当前节点 | 其他节点/画布 | 用户动作 |
|---|---|---|---|
| Shell PTY 启动失败 | error | 继续运行 | 重试启动或移除 |
| Git 仓库校验失败 | Fork 节点 error | 继续运行 | 改选当前工作树或移除 |
| worktree 创建/脚本失败 | 节点与 worktree failed | 继续运行 | 重试真实 Git 操作或移除 |
| provider Fork 失败 | 节点 error | 继续运行 | 重试 Fork 或移除 |
| Claude 恢复失败 | Shell + failed | 继续运行 | 重试恢复，或继续使用 Shell |
| 目标 cwd 被移动 | 路径异常提示 | 其他工作空间继续 | 恢复目录后重试 |
| DAG 投影事件缺口 | 暂停增量 | 主终端继续运行 | 自动请求 snapshot |
| DAG 窗口崩溃 | 主界面保留 | 全部会话继续 | 再次打开 DAG |

真实 E2E 的失败条件使用临时 Git 仓库、实际 hook/setup script、真实 PTY、真实 provider identity 失效和真实 Runtime 进程终止构造。测试配置只选择隔离数据根目录，不直接注入业务状态。

## 17. 可访问性与输入

- 所有图标按钮包含中文 aria-label 与 tooltip。
- 会话卡片焦点顺序与当前可见横向顺序一致。
- DAG 支持键盘搜索、方向键选择、Enter 导航、Escape 关闭、`⌘+/-/0` 缩放。
- `prefers-reduced-motion` 下将 FLIP、hover 让位和弹力动画缩短为淡入/即时位移，功能语义保持一致。
- `Option + Tab` 长按阈值默认 450ms，可在 350–800ms 设置范围内调整。

## 18. 测试策略

### 18.1 单元测试

- migration 14 回填与幂等性。
- combined structural parent、跨 Scene 拒绝和环检测。
- 同层排序序号、相等序号稳定性和重启保持。
- Claude 模式/恢复状态机。
- 分支显示名与 Git branch 生成。
- child badge 聚合优先级和历史计数。
- ParentPullController 的滚动/二次拖拽分离。
- DAG 分层布局、虚影、缩放边界和搜索排序。

### 18.2 Runtime 集成测试

- 原子创建 Canvas/Shell sibling/derived edge。
- 当前 worktree 与新 worktree 的真实 Git 流程。
- provider Fork 成功、失败、重试和并发去重。
- Runtime 单调交互序号和投影事件。
- 恢复失败切 Shell、手动退出切 Shell以及关系保留。
- snapshot/event gap 后完整 graph 重建。

### 18.3 Renderer 测试

- 新建自动聚焦。
- 一屏最多四卡与 hover 让位。
- 列表左右滚动和第二段右拉返回。
- 动态重排后 xterm 焦点与可见性。
- 长短 `Option + Tab` 分流。
- DAG 平移、缩放、搜索、节点选择与历史展示。
- 明暗主题和 reduced motion。

### 18.4 真实端到端测试

独立资深测试 Agent 使用：

- `MATOU_DATA_DIR=/tmp/matou-e2e-<run-id>/data`
- `ELECTRON_USER_DATA_DIR=/tmp/matou-e2e-<run-id>/electron`
- 临时 Git/非 Git Workspace。
- 真实 zsh、node-pty、Git worktree 和可用的 Claude Code CLI。

逐条执行 `docs/test/E2E-Matou-会话画布与DAG分支交互.md` 的 102 条用例，并在原文结果栏记录时间、环境、证据和结论。缺陷由主 Agent 修复，同一测试 Agent 复测失败用例及关联回归集合。

## 19. 实施切片

按可独立验收的纵向能力推进：

1. 领域类型、migration 14、membership 和统一结构关系。
2. SessionCanvasService、投影和协议。
3. 多画布默认 Shell与横向兄弟列表。
4. 用户交互排序、焦点和四卡浏览。
5. Claude 恢复状态和 Fork 子/兄弟流程。
6. 新 worktree 隔离与失败恢复。
7. 二次右拉父层导航。
8. 独立 DAG 窗口、画布、搜索和跳转。
9. 历史、独立窗口、通知、主题和生命周期回归。
10. 100 节点性能与完整 E2E 验收。

每个切片先写失败测试，确认失败原因对应缺失行为，再实现并运行目标测试、相关回归和类型检查。

## 20. 完成门槛

规格完成需要同时满足：

1. PRD F1–F19 全部有运行时代码和用户可见入口。
2. PRD AS01–AS50、UB01–UB74、EX01–EX15、QG01–QG09 全部有追踪证据。
3. 现有 PRD 01–06 自动化测试保持通过。
4. 新增单元、集成、Renderer 和 Electron 测试通过。
5. 独立 QA Agent 在隔离 App 上逐条执行 102 条真实 E2E 并全部标记通过。
6. 不使用业务状态注入或模拟终端/provider 结果替代真实端到端操作。
7. 用户数据目录未被验收流程读取或修改。
