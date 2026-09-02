# Terminal Runtime Robustness 实施台账

更新日期：2026-09-02

## 总表

| Task | 用户结果 | 状态 | 当前权威证据 |
|---|---|---|---|
| 1 大段 UTF-8 输入 | 大粘贴立即进入终端，透明分块且保持顺序 | **已闭合** | `runtime-task-1-utf8-input-chunking-report.md` |
| 2 拖入路径 | 路径形成精确 Shell 参数，不自动执行 | **已闭合** | `runtime-task-2-dropped-paths-report.md` |
| 3 Resize 合并 | 卡片即时适配，Runtime 更新约 60Hz，最终尺寸准确 | **已闭合** | `runtime-task-3-resize-coalescing-report.md` |
| 4 App 回焦 | 返回 App 时恢复离开前控件，目标消失时回到当前终端 | **已闭合** | native focus E2E；`focus-restoration.test.ts` |
| 5 通知容量与 TTL | 每工作空间 1000 条；未读保留，已读 30 天清理 | **已闭合** | `runtime-task-5-notification-capacity-report.md` |
| 6 Journal 策略 | 16 MiB 轮转，最近 256 MiB raw 热窗口，旧段异步压缩 | **已闭合** | `journal-policy`、`SegmentJournal.compressionCandidates()` |
| 7 Checkpoint / tail index | checkpoint 后增量恢复；无 checkpoint 时最近 10,000 行 | **已闭合** | `journal-tail-index`、`checkpoint-manager`、长历史 E2E |
| 8 压缩历史读取 | raw/gzip 统一分页搜索，损坏段局部隔离 | **已闭合** | 6×320 MiB 压力门禁；真实损坏冷分片 1/1 |
| 9 压缩与写入故障隔离 | 单会话写失败暂停并可续写，其他会话继续 | **已闭合** | storage fault E2E；真实 Claude storage resume |
| 10 前台 xterm 绑定 | 当前横向层离屏会话保留 VT model；其他 Scene 释放渲染 | **已闭合** | `terminal-model-cache`；81 会话连续离屏往返 3 次 |
| 11 每卡恢复遮罩 | 恢复阶段只覆盖所属卡片，其他工作保持可用 | **已闭合** | 20 PTY recovery scale；组件与完整 E2E |
| 12 压缩历史搜索 | 同一搜索入口进入命中上下文并可返回实时终端 | **已闭合** | `TerminalHistoryContextView`；长历史 E2E |
| 13 终端恢复边界 | 长行、ANSI、alternate screen、损坏和 Unicode 均有真实/定向门禁 | **已闭合** | PTY stress 4/4；long history；corruption E2E |
| 14 全系统门禁 | unit、type、build、E2E、scale、stress、package 同版本收口 | **已闭合** | `2f27432`；`2026-09-02-internal-hardening-final.md` |

## 状态口径

- **已闭合**：该 Task 的产品行为、定向测试和要求中的真实验收证据均已闭合。
- **已实现（定向验证）**：用户可见主路径已落地并有单元/组件或专项运行证据；仍需随 Task 14 进入本轮最终全量门禁。
- **部分实现**：已有可用能力，但仍存在与已确认产品边界或原 Task 验收合同不一致的缺口；不得据此宣称 Task 已完成。
- **待最终门禁**：生产能力可包含多项已落地内容，但发布结论仍以最终完整命令矩阵为准。

## Task 4、6–14 最终落地核对

此前保留的真实窗口回焦、256 MiB 热窗口、流式 Journal、恢复队列、离屏前台模型、历史上下文、alternate-screen 压力和全系统门禁均已闭合。最后一轮还增加了恢复身份副作用门：身份确认前的 Provider 输出不写摘要、cwd 和工作状态，错误身份及过期 Fork owner 不产生 HUD、通知和 Team DAG；同时增加 Fork 身份截止时间、Fork 接受时 commit 固定，以及打包只读历史的不可写验证。用户可见结果、异常边界和最终数字统一记录在 `docs/audits/2026-09-02-internal-hardening-final.md`；底层容量预算继续由 `implementation-runtime-robustness-plan.md` 作为回归门槛。

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
