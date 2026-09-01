# Terminal Runtime Robustness 实施台账

更新日期：2026-09-01

## 总表

| Task | 用户结果 | 状态 | 当前权威证据 |
|---|---|---|---|
| 1 大段 UTF-8 输入 | 大粘贴立即进入终端，透明分块且保持顺序 | **已闭合** | `runtime-task-1-utf8-input-chunking-report.md` |
| 2 拖入路径 | 普通/空格路径保持既有可见形式；复杂文件名只形成单个 argv，拖入不执行 | **已闭合** | `runtime-task-2-dropped-paths-report.md` |
| 3 Resize 合并 | 卡片即时适配；Runtime 更新不超过 60Hz，最终 PTY 尺寸准确 | **已闭合** | `runtime-task-3-resize-coalescing-report.md` |
| 4 App 回焦 | 恢复离开前的真实焦点 | 待实施 | Task 4 |
| 5 通知容量与 TTL | 每个工作空间独立保留最新 1,000 条；未读持续保留，已读 30 天后清理 | **已闭合** | `runtime-task-5-notification-capacity-report.md` |
| 6 Journal 策略 | 16 MiB segment / 256 MiB raw 热窗口 | 待实施 | Task 6 |
| 7 Checkpoint / tail index | 最近 10,000 行即时恢复 | 待实施 | Task 7 |
| 8 压缩历史读取 | raw 与压缩历史统一读取 | 待实施 | Task 8 |
| 9 Journal 压缩 | 旧 segment 异步压缩与原子替换 | 待实施 | Task 9 |
| 10 前台 xterm 绑定 | 后台 PTY 保活，非前台 xterm 解绑 | 待实施 | Task 10 |
| 11 每卡恢复遮罩 | 单卡恢复不阻塞其它卡片 | 待实施 | Task 11 |
| 12 压缩历史搜索 | 现有搜索入口可查询更旧输出 | 待实施 | Task 12 |
| 13 终端恢复边界 | Unicode、长行、alternate-screen、缺口和损坏可恢复 | 待实施 | Task 13 |
| 14 全系统门禁 | 真实规模与交互回归统一收口 | 待实施 | Task 14 |

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
| GREEN：真实 zsh + component | 定向 Vitest 24 tests | 通过 |
| GREEN：真实 Electron / PTY | `terminal-channel.spec.ts` drop 场景 | 通过；8 个真实文件名，副作用文件不存在 |

Task 2 只改变路径拖入解析和引用。普通键入与 paste 仍走原输入链路，未加入确认、自动提交或错误提示。

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
