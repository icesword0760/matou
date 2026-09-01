# Matou 内部版性能、稳定性与健壮性分析报告

- 审计日期：2026-09-01
- 审计对象：审计开始时主工作区的内部版工作状态（包含当时尚未提交的 910 行新增、131 行删除）
- 独立工作树：`/Users/icesword/Documents/AIProjects/matou/.worktrees/internal-hardening-audit`
- 审计分支：`codex/internal-hardening-audit`
- 本轮边界：分析、故障注入和压力测试；未修改产品逻辑，未提交修复

## 1. 产品结论

当前版本的日常小规模路径已经具备较完整的功能基础：731 项单元/组件测试全部通过，类型检查通过，真实 Electron 端到端场景 72 项中 65 项通过。

但当前版本还不适合直接扩大到“极多会话、长期运行、频繁 Fork、多窗口并行”的真实内部试用。原因不是某个局部动画，而是三类风险会相互放大：

1. **数据位置可能错**：独立 worktree 恢复时存在落回原工作区的路径，用户可能在错误分支继续工作。
2. **数据可能静默消失**：数据库只读、版本回退或损坏时，界面仍可能表现正常，但重启后操作消失，或直接呈现全新空状态。
3. **规模增长不是平滑变慢，而会跨过临界点**：隐藏会话仍启动完整 PTY/xterm，全量投影持续刷新，Journal 从头读取；规模继续增加后会出现长时间冻结、内存激增和递归栈溢出。

### 建议的内部发布判断

| 判断 | 结论 |
|---|---|
| 继续少量会话的功能验收 | 可以，需避开下述数据安全场景 |
| 扩大到多工作区、多 Fork、长时间运行试用 | 建议先闭合第一批发布阻断项 |
| 开始性能优化 | 可以先做保持现有产品行为的基础项 |
| 直接调整恢复、后台运行、历史保留语义 | 需先完成第 7 节产品决策 |

## 2. 本轮实测结果

### 2.1 基线验证

| 验证 | 结果 |
|---|---:|
| 单元与组件测试 | 104 files / 731 tests passed |
| TypeScript 类型检查 | 全部通过 |
| 真实 Electron E2E | 65 passed / 7 failed，耗时约 5.3 分钟 |

### 2.2 大规模与故障压力数据

| 场景 | 实测结果 | 用户影响 |
|---|---:|---|
| 1000 Session + 1000 Scene 全量投影 | 241.72ms；约 14,014 次 SQLite 查询；2.34MB JSON | 400ms 轮询周期内几乎持续占用一次完整刷新预算 |
| 10,000 节点 Projection Store | replace 32.20ms；view 39.50ms | 尚未包含 React、布局和绘制 |
| 10,000 节点 DAG | 宽图 221.93ms；深图 407.42ms | 拖拽、缩放和搜索会明显卡顿 |
| 2000 层会话父子链 | `Maximum call stack size exceeded` | 进入画布时直接失败 |
| 1500 层 split 层级 | `Maximum call stack size exceeded` | 恢复工作现场时直接失败 |
| 单 Session 32MB Journal 回放 | 429.25ms；RSS 增加 36.63MB | 多会话恢复时内存和等待线性放大 |
| 6 Session / 120MB Journal | 读 1619ms；RSS 95→385MB；事件循环最大停顿 862.5ms | 输入、滑动和通知反馈出现可感知冻结 |
| 12 Session / 48MB 输出且 Renderer 不 ACK | RSS 113→306MB | 现有背压停止发送，但仍继续内存排队和落盘 |
| Electron 真实创建 16 个 Shell | 16 PTY + 16 xterm；renderer 约 226MB | 逻辑视口仅 3 个会话，后台资源仍全部启动 |

> SSR 数据未包含 Chromium 布局、绘制、xterm 初始化和真实 PTY 成本，因此属于成本下界。

## 3. 第一批发布阻断项

### B-01｜P0｜新 worktree 创建中断后可能在原工作区启动

**用户场景**：用户为了并行开发选择“新 worktree”，创建过程中 App 退出或 Runtime 崩溃。

**影响**：恢复后会话可能使用源 Session 的目录继续启动；用户以为处于隔离分支，实际修改原工作区，多个功能模块会互相覆盖。

**证据**：

- `apps/runtime/src/session-canvas/fork-workflow-service.ts:315-357,464-510,538-568`
- `apps/runtime/src/session/session-fork-intent-repository.ts:26-50`
- `apps/runtime/src/runtime-server.ts:911-923`

**建议**：worktree 记录、路径、分支和执行上下文未全部一致时，不启动 provider；启动时对未完成 Fork 做幂等对账。

**需产品决策**：中断后默认自动续建、展示待恢复卡片，还是让用户确认后重试。

### B-02｜P0｜worktree 丢失后静默落回主工作区

**用户场景**：独立 worktree 被移动、手动删除、磁盘卸载或被 Git 清理。

**影响**：系统会把会话落到 workspace root 或 HOME 并持久化新路径；用户进入同名会话，却在另一份代码中继续工作。

**证据**：

- `apps/runtime/src/runtime-server.ts:876-895`
- `apps/runtime/src/hierarchy/workspace-path-service.ts:123-154`

**建议**：普通 Shell 目录可保留现有回退；托管 worktree 需要校验 worktree identity，失效后进入明确恢复状态。

**需产品决策**：恢复界面提供“重新创建、定位已有目录、回到原工作区、关闭会话”中的哪些动作，以及默认动作。

### B-03｜P0｜临时数据库模式会让新操作在重启后消失

**用户场景**：数据目录变为只读，或较旧版本打开了较新 schema。

**影响**：用户仍可创建事项、会话和布局，界面也显示成功；实际只写入 `/tmp` 副本，下次启动全部消失。

**实测**：临时实例 mutation 返回成功；关闭后重新打开 durable DB，新记录不存在。

**证据**：

- `apps/runtime/src/storage/runtime-database-bootstrap.ts:31-38,61-80`
- `apps/runtime/src/index.ts:65-80,171-180`

**需产品决策**：推荐“只读浏览 + 明确修复入口”；另外两个选择是禁止进入，或允许临时操作并持续显示不可持久化状态。

### B-04｜P0｜主数据库损坏后直接呈现空产品

**用户场景**：SQLite 文件出现物理损坏。

**影响**：旧 DB/WAL/SHM 被隔离后创建新库，用户看到默认工作空间，原事项与会话像被全部删除；当前只在控制台留下信息。

**证据**：

- `apps/runtime/src/storage/runtime-database-bootstrap.ts:39-53`
- `apps/runtime/src/index.ts:69-71`
- `tests/e2e/prd-04-session-recovery.spec.ts:158-180` 当前将“静默进入空状态”写成了产品合同

**需产品决策**：推荐先进入数据恢复页面，默认尝试最近备份；“进入空状态”保留为用户明确选择。

### B-05｜P0｜磁盘写失败会污染整个 PTY 输出链

**用户场景**：长时间运行后磁盘满、权限变化或发生 I/O 故障。

**影响**：当前会话后续输出停止落盘和发送；provider 可能仍运行，界面却停住。拒绝继续传播时还可能导致 Runtime 退出，影响全部终端。

**证据**：

- `apps/runtime/src/session/pty-session.ts:137-144,209-251`
- 现有 Journal ENOSPC 测试只覆盖底层返回值，没有覆盖 live PTY 的产品状态

**需产品决策**：推荐“暂停该会话并保留进程，提示用户清理空间后继续”；另一方案是终止进程以优先保证一致性。

## 4. 大规模性能与恢复风险

### P-01｜P1｜全量投影形成常驻刷新风暴

即使用户不操作，工作区路径每 400ms 检查一次并触发全量 projection；事件已经增量应用后仍再次请求完整快照，每个 mutation 完成后又刷新一次。

**用户影响**：会话越多，滑动、悬停、页签切换越容易被后台刷新打断；空闲状态也持续占用 Runtime、SQLite 和 Renderer。

**证据**：

- `apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx:50-85,131-154`
- `apps/desktop/src/renderer/src/hierarchy/hierarchy-commands.ts:13-20`
- `apps/desktop/src/renderer/src/projection/RuntimeProjectionStore.ts:62-116`
- `apps/runtime/src/rpc/runtime-rpc-router.ts:816-855`

**建议**：增量事件成功时不再请求全量快照；路径状态未变化时不刷新；快照改为集合查询并增加耗时、实体数、查询数指标。

### P-02｜P1｜隐藏页签和视口外会话仍挂载完整 xterm/PTY

**用户影响**：恢复时间、内存、文件句柄和后台 CPU 按历史会话总量增长，而不是按当前可见会话增长。

**证据**：

- `apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx:481-652`
- `apps/desktop/src/renderer/src/session-canvas/SessionCarousel.tsx:849-892`
- `apps/desktop/src/renderer/src/hierarchy/TerminalPane.tsx:293-315`
- `apps/desktop/src/renderer/src/terminal/TerminalSurface.tsx:121-303`
- `apps/desktop/src/renderer/src/hierarchy/hierarchy.css:128`

**需产品决策**：推荐“后台 PTY 继续运行，但不可见会话解除 xterm 渲染绑定；重新进入时从 checkpoint/journal 补齐”。

### P-03｜P1｜MessagePort 背压没有反压到 PTY

信用窗只停止向 Renderer 发送；PTY 仍继续编码、创建 Promise、写盘和排队。十几个高输出会话时，Renderer 已跟不上，Runtime 内存和磁盘压力仍持续上升。

**建议**：信用耗尽后暂停 PTY read 或启用有界缓冲；按 Session 公平调度；Replay 从 sequence 索引流式读取。

**证据**：`apps/runtime/src/session/pty-session.ts:209-223,271-311`、`apps/desktop/src/renderer/src/terminal/TerminalSurface.tsx:201-204`。

### P-04｜P1｜Journal 全历史回放与同步压缩阻塞 Runtime

Checkpoint 目前没有降低读盘成本：Runtime 先读取全部 frames，再选择 checkpoint；16MB segment 使用同步 gzip/gunzip，单个高输出会话轮转时会暂停同一 Runtime 中的全部终端、RPC 和通知。

**证据**：

- `apps/runtime/src/journal/segment-journal.ts:200-202,228-258,280-301`
- `apps/runtime/src/runtime-server.ts:559-639`
- `apps/runtime/src/recovery/runtime-recovery-service.ts:45-107`

**建议**：sequence/offset 索引、流式读取、异步压缩或 worker、生产化 checkpoint、按总内存预算做有界恢复。

### P-05｜P1｜深层关系存在确定性栈溢出

2000 层会话父子链和 1500 层 split 已实测触发栈溢出。当前遍历同时存在递归深度和重复 filter/find，深链会从慢直接跨到崩溃。

**证据**：`apps/desktop/src/renderer/src/session-canvas/SessionCanvas.tsx:156-159`、`apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx:771-812`。

**建议**：预建邻接索引，使用迭代遍历，加入 cycle guard 和损坏拓扑诊断。该项保持现有用户行为，可直接进入修复。

### P-06｜P1｜DAG 的“可见层”仍渲染大量 ghost 节点

10,000 节点深图 SSR 已达 407.42ms；平移和缩放的每个事件继续更新 React state并重新处理完整图。

**证据**：`apps/desktop/src/renderer/src/dag/DagCanvas.tsx:40-47,93-113,146-177`、`DagWindowApp.tsx:23-40,70-75,166-177`。

**需产品决策**：推荐远层按分支/层聚合，进入可视范围再展开；若要求每个远层节点始终可点击，性能成本会明显提高。

### P-07｜P1｜激活场景会并发重启全部 stopped Session

进入含大量历史节点的画布会立刻并发 restart，每个 restart 又触发完整刷新，形成 PTY、Journal、SQLite 和 React 的恢复风暴。

**证据**：`apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx:270-280`、`hierarchy-commands.ts:102-104`。

**需产品决策**：推荐只自动恢复活动和可见 Session，其余按需恢复；若要求全部恢复，需要显示进度并使用有界队列。

## 5. 交互与异常健壮性风险

### R-01｜P1｜删除的失败会话可能被自动恢复

失败 Session 被移除后短暂进入 archived，Renderer 又把所有 archived 节点当成可恢复停止会话并执行 restart。重复测试中 3 次有 2 次出现删除后自动恢复。

**用户影响**：卡片重新出现、焦点漂移，用户对“删除是否生效”失去信任。

**证据**：

- `apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx:270-280`
- `apps/runtime/src/hierarchy/hierarchy-application-service.ts:1519-1549`

**建议**：持久化区分 `stopped/recoverable` 与 `deleted/archived`；自动恢复只处理显式 recoverable。

### R-02｜P1｜Spawn、关闭和删除没有共用生命周期串行器

启动中立即删除时，dispose 可能先发现 registry 没有 PTY而返回，随后 spawn 又完成注册，留下 UI 已不存在的后台进程。

**证据**：`apps/runtime/src/runtime-server.ts:195-199,799-813,1231-1244,1468-1506`。

**建议**：所有 Session 生命周期动作共用 generation/cancellation token；PTY 注册前再次核对权威状态。

### R-03｜P1｜右键菜单按 Escape 稳定失效

真实 xterm 聚焦时会截断 bubble-phase keydown，Task 菜单 Escape 连续复跑 2/2 失败，专项审计为 3/3 失败。

**用户影响**：菜单停留，后续键盘输入可能继续进入终端。

**证据**：`TaskSidebar.tsx:81-96`、`SceneTabBar.tsx:77-86`、`TerminalPane.tsx:125-143`。

**建议**：全局菜单使用 capture phase，并收敛为统一 overlay/menu manager。该项保持现有产品逻辑，可直接修复。

### R-04｜P1｜鼠标移出后会话卡片持续展开

完整 E2E 与单独复跑均失败：鼠标移到窗口角落后，`.session-card-slot.is-expanded` 15 秒仍未清理，继而阻断横向位置持久化验收。

**影响**：卡片宽度、滚动位置和视觉焦点残留；在大量会话下结构性恶化。

**证据**：`SessionCarousel.tsx:199-224,340-440,518-571,748-759`。

**边界**：修复应保持已确认 Mockup 的展开时长和轨迹；若需要改变动画节奏，需与 Kooky 基线对照后决策。

### R-05｜P1｜回到 App 时终端可能抢走弹窗焦点

窗口重新获得焦点时无条件增加 terminal focus request；普通激活路径有弹窗保护，但窗口恢复路径没有。

**用户影响**：用户正在重命名、搜索、Fork 或确认时，输入可能进入 Shell。

**证据**：`HierarchyShell.tsx:215-225`、`TerminalSurface.tsx:112-120,305-309`。

**需产品决策**：推荐保留离开 App 前的控件焦点；只有当原焦点就是终端时才恢复终端。

### R-06｜P1｜超过 1MiB 的粘贴整块丢弃且无提示

**用户场景**：粘贴长日志、大 Prompt、Base64 或 SQL。

**影响**：用户以为输入已经进入终端，实际协议拒绝整条消息。

**证据**：`packages/contracts/src/protocol.ts:30-35`、`RuntimeClient.ts:136-138,208-212`、`TerminalSurface.tsx:156-164`。

**需产品决策**：建议按 UTF-8 安全边界自动分块；同时确认最大单次粘贴量和超限提示。

### R-07｜P1｜拖入终端的路径转义不完整

当前主要处理空格，对引号、反斜线、变量替换、换行等字符覆盖不足。

**用户影响**：复杂文件名进入 Shell 后可能改变命令含义。

**证据**：`TerminalSurface.tsx:381-389`。

**需产品决策**：拖入后的可见文本和引用风格需与 Kooky 做 1:1 对照；底层路径安全引用属于必须项。

### R-08｜P1｜Fork/worktree 超时后后台操作仍继续

Renderer 10 秒后显示超时，但取消消息只在 RPC 前后检查，正在等待的 Git/setup 子进程仍会继续运行。用户重试后可能产生多个并行操作和残留 worktree。

**证据**：`RuntimeClient.ts:67-91`、`runtime-server.ts:493-540`、`worktree-service.ts:69-130`。

**需产品决策**：推荐长任务显示阶段进度并允许真实取消；若 setup 本身不适合中断，则超时后继续后台运行但禁止重复提交。

### R-09｜P2｜快速输入名称后立即确认可能提交旧值

专项实测捕获到输入框已是“修复登录”，RPC payload 仍为旧值“新事项”。

**用户影响**：弹窗关闭但名称未按用户输入更新，容易被误认为持久化失败。

**证据**：`apps/desktop/src/renderer/src/hierarchy/RenameDialog.tsx:8-16`。

### R-10｜P2｜Resize 动画会生成 SIGWINCH 与日志风暴

卡片 380ms 宽度动画期间，每次 ResizeObserver 都执行 fit、PTY resize 和 Journal resize，没有 cols/rows 去重和 frame 合并。

**需产品决策**：全屏 TUI resize 使用连续实时反馈，还是约 30/60Hz 限流；推荐 60Hz 合并且相同尺寸去重。

### R-11｜P2｜Agent transcript 与通知数据无界增长

每个 hook 最多重读 4MiB transcript；通知数组和 cooldown map 没有容量和 TTL。长期运行后会持续增加 CPU、内存和通知中心渲染成本。

**需产品决策**：确定通知保留数量/时长与 transcript 增量保留策略。

## 6. E2E 失败复核与产品合同漂移

完整 E2E 首轮失败 7 项。专项复跑后的分类如下：

| 失败项 | 复核结论 | 处理方式 |
|---|---|---|
| Task 菜单 Escape | 连续稳定失败 | 工程修复，保持现有产品逻辑 |
| 鼠标离开后卡片仍展开 | 完整套件与单独复跑均失败 | 交互回归；按 Mockup/Kooky 基线修复 |
| 显式删除后不复活 | 实际卡在菜单中找不到“删除会话”，尚未进入恢复阶段 | 产品合同决策后更新实现与测试 |
| 创建/重命名/删除会话流程 | 同样等待旧“删除会话”入口 | 产品合同决策后更新实现与测试 |
| stationary pointer 横向重定向 | 单独通过，全套压力下失败 | 纳入大规模 Chromium 基准，先处理刷新风暴 |
| Claude 历史 exact content 搜索 | 单独通过，全套压力下失败 | 视为负载下时序波动，补稳定性指标 |
| 失败 sibling 移除后焦点 | 单独有时通过；专项重复 3 次有 2 次出现自动恢复 | 修复删除/停止状态竞态 |

### “删除会话”与“移除节点”的合同冲突

当前实现的右键菜单只提供“移除节点…”，旧 PRD/Kooky E2E 等待“删除会话”。二者的用户语义不同：

- **删除会话**：删除当前终端/会话，通常由现有 Scene 布局决定后续焦点。
- **移除节点**：面向会话图，可能连同后代分支一起处理。

**需产品决策**：推荐同时保留两个动作并明确范围；若产品只保留“移除节点”，需要更新 PRD、Kooky 对照矩阵和所有恢复合同。

## 7. 需要产品决策的事项

| 编号 | 决策题 | 推荐方向 | 用户价值与代价 |
|---|---|---|---|
| D-01 | 非活动页签/视口外会话是否继续运行 | PTY 继续运行，停止 xterm 渲染 | 保留后台任务，同时显著降低恢复和滑动成本；重新进入需短暂补齐画面 |
| D-02 | stopped Session 进入场景时如何恢复 | 只恢复活动/可见，其余按需 | 首屏更快；历史卡片需显示“点击恢复”状态 |
| D-03 | 是否允许先进入 App、后台恢复其余会话 | 允许，当前活动会话优先 | 缩短等待；需增加清晰进度和失败状态 |
| D-04 | 终端历史保留策略 | checkpoint + 有限 scrollback + 可查归档 | 控制磁盘/内存；超出可见历史需明确提示 |
| D-05 | 数据库只读/损坏时怎么进入产品 | 只读恢复模式，不接受会丢失的写操作 | 避免静默数据丢失；增加一次恢复步骤 |
| D-06 | worktree 丢失后的默认动作 | 停止自动回退，展示恢复卡片 | 防止改错代码目录；用户需选择一次恢复方式 |
| D-07 | 磁盘写失败后的会话行为 | 暂停对应会话，修复后继续 | 优先保护可恢复性；长任务会被暂停 |
| D-08 | DAG 远层节点展示 | 按分支/层聚合 | 数千节点仍可浏览；远层不再逐节点同时展示 |
| D-09 | 删除会话与移除节点 | 两个动作并存，明确影响范围 | 兼容 Kooky 心智与 DAG 分支管理；菜单多一个动作 |
| D-10 | App 回焦后的输入焦点 | 保留离开前焦点 | 避免文字误入 Shell；从外部回到终端时仍可恢复原终端焦点 |
| D-11 | Fork/setup 长任务超时 | 显示进度并支持真实取消 | 避免重复 worktree；实现成本高于单纯 10 秒报错 |
| D-12 | 通知保留 | 建议 30 天或每工作空间 1000 条，先到者淘汰 | 保持近期可追溯，避免长期无界增长 |

## 8. 可直接进入修复、无需产品取舍的事项

1. 移除事件成功后的重复全量 projection refresh。
2. projection snapshot 改为批量查询并增加性能指标。
3. 深层 Session/split 遍历改为迭代并增加 cycle guard。
4. gzip/gunzip 移到异步任务或 worker。
5. Escape 监听使用 capture phase，统一菜单关闭机制。
6. Session spawn/delete/dispose 使用相同生命周期串行器。
7. deleted 与 recoverable stopped 状态分离。
8. 快速重命名提交当前输入值，而不是旧 React closure。
9. ResizeObserver 按帧合并、相同 cols/rows 去重。
10. Runtime 使用真实 ready handshake、启动超时和指数退避。

## 9. 建议实施顺序

### 阶段 1：数据与代码目录安全

- worktree 创建/恢复对账
- worktree 失效保护
- 临时数据库与损坏数据库恢复模式
- 磁盘写失败状态

### 阶段 2：会话生命周期正确性

- deleted/stopped 状态分离
- spawn/delete 串行化
- Fork 子进程取消和 fencing
- Runtime ready/退出总时限

### 阶段 3：大规模恢复与内存

- 隐藏 xterm 解绑
- 真实 PTY 反压
- Journal 流式读取、异步压缩、checkpoint
- 活动优先和有界恢复队列

### 阶段 4：渲染与滑动

- 去除重复全量刷新
- Session/Sidebar/DAG windowing
- 迭代图遍历和索引
- 50/200/1000 兄弟会话真实 Chromium 滚动基准

### 阶段 5：交互回归闭合

- 菜单 Escape、焦点租约、快速重命名
- 大粘贴分块、拖入路径引用
- 删除会话/移除节点按产品决策闭合 Kooky 对照矩阵

## 10. 建议建立的发布容量基线

| 场景 | 基准档位 | 关键指标 |
|---|---|---|
| 兄弟会话画布 | 50 / 200 / 1000 | 滚动帧耗、悬停响应、输入延迟、renderer RSS |
| 深层父子链 | 500 / 2000 / 5000 | 可恢复性、布局耗时、栈安全 |
| DAG | 1000 / 5000 / 10000 | 首次绘制、pan/zoom 帧耗、搜索时间 |
| Scene 页签 | 50 / 200 / 1000 | 首次可交互、xterm/PTY 数量 |
| 总 Session | 1000 / 5000 / 10000 | projection 大小、SQLite 查询数、空闲 CPU |
| Journal | 单 Session 32MB / 256MB；100 Session 总量 | 恢复时间、峰值 RSS、事件循环停顿 |
| 高频输出 | 16 / 50 / 100 Session | 队列上限、公平性、输入延迟、丢帧/丢数据 |
| 通知 | 1000 / 10000 / 100000 | 推送耗时、侧栏重渲染、通知中心首开 |

## 11. 验证命令与临时证据

```bash
cd /Users/icesword/Documents/AIProjects/matou/.worktrees/internal-hardening-audit

pnpm test
pnpm typecheck
pnpm test:e2e

pnpm exec vitest run /tmp/matou-scale-audit.test.ts --root /tmp --environment node
pnpm exec vitest run /tmp/matou-dag-render.test.ts --root /tmp --environment node
pnpm exec vitest run /tmp/matou-hierarchy-depth.test.ts --root /tmp --environment node
pnpm exec vitest run /tmp/matou-many-scenes.test.ts --root /tmp --environment node
pnpm --filter @matou/runtime exec vitest run --root /tmp /tmp/matou-recovery-faults.test.ts

node --expose-gc /tmp/matou-audit/pty-backpressure-bench.mjs
SESSIONS=6 MIB=20 SEGMENT_MIB=16 node --expose-gc /tmp/matou-audit/journal-bench.mjs
```

专项 Electron 证据脚本位于 `/tmp/matou-audit/`；故障注入和规模脚本均位于 `/tmp`，未进入产品代码。

## 12. 审计边界

- 本轮未对 Kooky 全部异常状态进行重新截图比对；凡涉及动画节奏、删除语义、拖入文本、历史截断提示的项目均列为产品决策或后续 Kooky 对照项。
- 部分风险为静态高确定性结论；报告中已单独标出实测数据。正式修复后仍需真实 Electron、真实 PTY、真实 Git worktree 和故障注入复验。
- 审计开始后主工作区仍有其他变化；本报告固定对应审计工作树中的起始内部版快照。
