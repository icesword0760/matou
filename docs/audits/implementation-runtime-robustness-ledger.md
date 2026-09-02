# Terminal Runtime Robustness 实施台账

更新日期：2026-09-02

## 总表

| Task | 用户结果 | 状态 | 当前权威证据 |
|---|---|---|---|
| 1 大段 UTF-8 输入 | 大粘贴立即进入终端，透明分块且保持顺序 | **已闭合** | `runtime-task-1-utf8-input-chunking-report.md` |
| 2 拖入路径 | 普通/空格路径保持既有可见形式；native 文件/目录和复杂名称只形成精确 zsh argv，拖入不执行 | **已闭合** | `runtime-task-2-dropped-paths-report.md`；runtime fix `37f080f` |
| 3 Resize 合并 | 卡片即时适配；Runtime 更新不超过 60Hz，最终 PTY 尺寸准确 | **已闭合** | `runtime-task-3-resize-coalescing-report.md` |
| 4 App 回焦 | 回到 App 时优先回到离开前正在编辑的控件；原控件已消失时才回到当前终端 | **已实现（定向验证）** | `focus-restoration.ts` / `.test.ts`；`HierarchyShell.test.tsx`；`9f2ebda` |
| 5 通知容量与 TTL | 每个工作空间独立保留最新 1,000 条；未读持续保留，已读 30 天后清理 | **已闭合** | `runtime-task-5-notification-capacity-report.md` |
| 6 Journal 策略 | 16 MiB 轮转已生效；256 MiB raw 热窗口规则已定义，但当前自动压缩链尚未按候选规则保留热窗口 | **部分实现** | `journal-policy.ts` / `.test.ts`；`segment-journal.ts` 的 `compressionCandidates()` 与 `#scheduleSealedCompression()` |
| 7 Checkpoint / tail index | 有 checkpoint 时恢复屏幕后补尾部；无 checkpoint 时从最近 10,000 行开始 | **已实现（定向验证）** | `journal-tail-index.ts` / `.test.ts`；`checkpoint-manager.test.ts`；`runtime-server.test.ts` |
| 8 压缩历史读取 | raw、gzip 与旧格式可统一分页和搜索；单页最多 1,000 行，单段损坏只返回该段缺口 | **已实现（定向验证）** | `journal-history-reader.ts` / `.test.ts`；`runtime-server.test.ts`；`437ea1f`、`b6349bc` |
| 9 Journal 压缩与写入故障隔离 | gzip 在并发队列中异步生成并原子发布；单会话写入失败暂停该会话并保留最多 4 MiB 待补写输出 | **已实现（定向验证）** | `journal-compressor.ts` / `.test.ts`；`session-durability-gate.ts` / `.test.ts`；`pty-execution-pauser.test.ts` |
| 10 前台 xterm 绑定 | 非活动 Scene 已解绑 xterm 且 PTY 保活；普通横向列表的离屏会话保持绑定，但超过 80 个同级会话时会被窗口化卸载 | **部分实现** | `terminal.view-detach` 协议；`TerminalPane.tsx`；`SessionCarousel.tsx` 的 `VIRTUALIZE_THRESHOLD = 80` |
| 11 每卡恢复遮罩 | queued/restoring/failed 都只覆盖所属卡片；其他卡片、侧栏和画布仍可操作 | **已实现（定向验证）** | `useSessionRecovery.ts` / `.test.tsx`；`TerminalPane.tsx` / `.test.tsx`；`10415c4` |
| 12 压缩历史搜索 | 现有终端搜索在实时 10,000 行无命中时查询归档并支持前/后切换；当前只展示命中摘要，尚未进入命中前后分页只读视图 | **部分实现** | `TerminalSurface.tsx` / `.test.tsx`；`RuntimeClient.test.ts`；`JournalHistoryReader.search()` |
| 13 终端恢复边界 | Unicode 跨 frame、长行、alternate-screen 行计数、侧索引重建和损坏 segment gap 已覆盖；完整 alternate-screen 真实 PTY 恢复矩阵尚未闭合 | **部分实现** | `journal-tail-index.test.ts`；`journal-history-reader.test.ts`；`segment-journal.test.ts`；`b6349bc` |
| 14 全系统门禁 | 真实规模、完整交互、打包运行与故障注入统一收口 | **待最终门禁** | Task 14 的全量命令及证据尚未在本轮统一完成 |

## 状态口径

- **已闭合**：该 Task 的产品行为、定向测试和要求中的真实验收证据均已闭合。
- **已实现（定向验证）**：用户可见主路径已落地并有单元/组件或专项运行证据；仍需随 Task 14 进入本轮最终全量门禁。
- **部分实现**：已有可用能力，但仍存在与已确认产品边界或原 Task 验收合同不一致的缺口；不得据此宣称 Task 已完成。
- **待最终门禁**：生产能力可包含多项已落地内容，但发布结论仍以最终完整命令矩阵为准。

## Task 4、6–14 当前落地核对

| Task | 用户当前能获得的结果 | 尚未闭合的用户影响 | 权威实现/测试证据 |
|---|---|---|---|
| 4 App 回焦 | 用户从其他 App 返回后，搜索框、重命名输入等仍保持输入位置；目标控件关闭后才把键盘交给当前终端 | 本轮尚未用真实 BrowserWindow 对终端、搜索、重命名和 Fork 弹窗做一组完整回焦验收，因此保留在最终门禁中 | `AppFocusRestorer`；`focus-restoration.test.ts`；`HierarchyShell.test.tsx` |
| 6 Journal 热窗口 | 输出按 16 MiB segment 轮转，策略函数能识别应保留的最近 256 MiB raw 与受 checkpoint 保护的段 | 当前 `#scheduleSealedCompression()` 会调度全部 sealed raw，未调用 `compressionCandidates()`；大历史场景下“最近 256 MiB 保持 raw”尚未成为运行时事实 | `SEGMENT_BYTES`、`RAW_HOT_BYTES`、`selectCompressionCandidates()`；`journal-policy.test.ts` |
| 7 即时恢复 | checkpoint 保存屏幕与水位，重连时只补 checkpoint 后内容；没有 checkpoint 时 tail index 将即时画面限制到最近 10,000 行 | replay 仍先物化完整 Journal，内存/恢复时延问题归入 Scale Task 6，不影响 10,000 行可见边界本身 | `JournalTailIndex`；`CheckpointManager`；`RuntimeServer.#replay()`；对应 tests |
| 8 历史分页 | 用户可按 cursor 向前读取 raw/gzip 历史，页大小被硬限制为 1,000 行；压缩发布期间 raw/gzip 重叠不会重复读取 | 6 会话 × 320 MiB 压缩期间的事件循环、RSS 与输入延迟门槛尚无本轮统一验收记录 | `JournalHistoryReader.page()`；`journal-history-reader.test.ts`；`journal-compressor.test.ts` |
| 9 压缩与存储故障 | 冷段压缩不阻塞 PTY 写链；单会话磁盘错误显示整卡操作，重试成功后按原 sequence 补写，其他会话继续运行 | Task 14 的真实 ENOSPC 全系统场景仍待统一门禁 | `JournalCompressor`；`SessionDurabilityGate`；`PtyExecutionPauser`；`StorageFaultOverlay` 及各自 tests |
| 10 前台绑定 | 切到其他 Scene 后旧 Scene 不再保有 xterm，但其 PTY 仍由 Runtime 继续运行；重新进入时复用原会话 | 80 个以上同级会话会由 Carousel 窗口化，滑出渲染窗的会话 xterm 被卸载，这与“横向列表内即使滑出视野仍定义为前台并保持绑定”的已确认规则冲突 | `RuntimeClient.attachTerminal()` 的 listener 引用计数；`terminal.view-detach`；`RuntimeServer` headless/reattach tests；`SessionCarousel.tsx` |
| 11 每卡恢复 | 每张卡分别显示等待、恢复、失败与重试；一张卡失败不会替换全局页面或遮挡其他卡 | 恢复重连后的权威快照同步仍需随恢复调度集成修复后进入最终门禁 | `useSessionRecovery`；`TerminalPane` 的 `.session-recovery-overlay`；相关 component tests |
| 12 归档搜索 | 用户仍从原搜索入口查旧输出，支持大小写、全词、正则以及 Next/Previous；损坏段会提示历史缺口 | 原计划中的“命中前后各 250 行只读历史视图、退出后回到实时终端”尚未实现；现在只显示单条命中摘要 | `JournalHistoryReader.search()`；`TerminalSurface` 的 `terminal-history-result`；相关 tests |
| 13 异常边界 | UTF-8 跨 frame、CRLF、超长单行的有界读取、alternate-screen 不计入 tail 行数、损坏/缺失 segment 的局部 gap 均有定向测试 | 20 MiB 单行、100k ANSI 变化及 less/vim 运行中 alternate-screen 崩溃恢复的真实 PTY 组合尚未形成完整验收矩阵 | `journal-tail-index.test.ts`；`journal-history-reader.test.ts`；`segment-journal.test.ts` |
| 14 发布收口 | 无单独用户功能；它负责证明上述能力可同时存在且不会互相回归 | 只有 `pnpm typecheck`、`pnpm test`、`pnpm test:e2e`、`pnpm test:scale`、打包运行与故障注入在同一最终版本全部通过后才能改为已闭合 | `implementation-runtime-robustness-plan.md` Task 14 |

## Task 1 RED → GREEN 记录

| 阶段 | 证据 | 结果 |
|---|---|---|
| RED：UTF-8 纯函数 | `terminal-input-chunker.test.ts` 首次运行 | 缺少模块与导出；目标失败成立 |
| RED：RuntimeClient 多消息 | `RuntimeClient.test.ts` 首次运行 | 仅发送 1 条消息，预期多于 4 条；目标失败成立 |
| GREEN：chunker + client | 定向 Vitest 13 tests | 通过；默认 256 KiB、顺序拼接、协议解析与 surrogate 边界全部闭合 |
| GREEN：Desktop 全量 | `pnpm --filter @matou/desktop test` | 43 files / 361 tests 通过 |
| GREEN：真实 Electron / PTY | `terminal-channel.spec.ts` large UTF-8 paste 场景 | 2.5 MiB + 中文 + Emoji + 组合字符完整；byte count 与 tail hex 一致，无确认或错误 UI |

Task 1 只在 Renderer → Runtime 输入边界分块；PTY、协议语义、bracketed paste、交互排序与只读写入栅栏均保持原合同。

## Task 2 RED → GREEN 记录

| 阶段 | 证据 | 结果 |
|---|---|---|
| RED：纯引用模块 | `pnpm --filter @matou/desktop test -- shell-path-quote.test.ts` | 模块不存在；目标失败成立 |
| RED：结构化拖入优先 | 同次 component run | 旧实现透传恶意 `text/plain`；目标失败成立 |
| RED：特殊 Finder 路径 | 同次 component run | 旧实现对 `$()` 使用双引号；目标失败成立 |
| 审查 RED：zsh leading `=` / NUL | `shell-path-quote.test.ts` + `TerminalSurface.test.tsx` | 3 项按预期失败：`=ls` 被 zsh 展开为 `/bin/ls`；NUL 未丢弃；结构化多路径包含无效 NUL 项 |
| GREEN：真实 zsh + component | clean `37f080f` 定向 Vitest | 2 files / 27 tests 通过；leading `=`、CR、Unicode、multiple 精确 round-trip，NUL fail-closed |
| GREEN：结构化 Electron / PTY | `terminal-channel.spec.ts` structured drop | 8 个真实文件名逐字回显；恶意 `text/plain` 被忽略；绝对副作用文件不存在 |
| GREEN：native Electron / PTY | CDP native `files` → FileList → preload `webUtils.getPathForFile()` | 3 个真实文件 + 1 个空格目录同序成为精确 argv；URI-only `file://` 不进入终端 |
| GREEN：clean 全量 | detached `37f080f` | Desktop 43/355、contracts 4/45、domain 2/7、Runtime 72/524、typecheck/build 通过；terminal-channel 4 passed |

Task 2 只改变路径拖入解析、引用和对应验收。普通键入与 paste 仍走原输入链路，未加入确认、自动提交或错误提示。native 主链已在 macOS 15.7.4 / Electron 43.4.1 / zsh 实跑；Windows/Linux host 与 PowerShell/cmd 不在本轮运行时结论内。

## Task 3 RED → GREEN 记录

| 阶段 | 证据 | 结果 |
|---|---|---|
| RED：fake-rAF 合并器 | `resize-coalescer.test.ts` 首次运行 | 模块不存在；目标失败成立 |
| GREEN：单帧合并与 60Hz | 定向 Vitest 5 tests | 单帧只发最终尺寸、跨帧去重、60Hz 间隔、flush/dispose 全部通过 |
| RED：Runtime 二次去重 | `pty-session.test.ts` 首次运行 | 相同尺寸产生 2 个 Journal frame；目标失败成立 |
| GREEN：Runtime / Journal | 定向 Vitest 2 tests | 相同尺寸只调用一次有效 resize 路径并只写 1 个 frame |
| GREEN：真实 Electron / PTY | `session-canvas-navigation.spec.ts` 16 sibling resize 场景 | 每会话任意连续一秒不超过 60 条；最终 `stty size` 匹配；拖动中输入回显连续 |

Task 3 只合并 Runtime resize，不延迟卡片的本地 `fit.fit()`，也不改变横向列表中滑出视野会话的前台身份。

## Task 5 RED → GREEN 记录

| 阶段 | 证据 | 结果 |
|---|---|---|
| RED：容量、TTL 与 cooldown | `AgentNotificationStore.test.ts` 首次运行 | 6 个目标场景按预期失败：工作空间超量、确定性淘汰、已读 TTL、容量触发 cooldown 清理均未实现 |
| GREEN：Store 规则 | 定向 Vitest 22 tests | 每工作空间与未分配 bucket 各自限制 1,000；未读不因 30 天过期；已读超过 30 天在 push/snapshot 清理 |
| GREEN：通知 UI 集成 | 定向 Vitest 4 files / 41 tests | replacement、cooldown、badge、通知中心和导航既有场景保持通过 |
| GREEN：10k 基准 | Node 22 真实 Store 进程 | 10 个工作空间共 10,000 条后，单次 overflow push/prune 为 0.71 ms，最终仍为 10,000 条 |
| GREEN：真实 Electron | `prd-01-agent-notifications.spec.ts` 双 bucket 容量场景 | 注入 2,002 条后通知中心保留 2,000；两个 bucket 各淘汰最旧一条；导航后 1,999；清空后 0 |

Task 5 只限制每个工作空间的通知历史，并清理已读旧记录；未读提醒、通知导航、声音偏好及当前会话提示规则保持原产品行为。
