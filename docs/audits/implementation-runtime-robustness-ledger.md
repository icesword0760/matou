# Terminal Runtime Robustness 实施台账

更新日期：2026-09-01

## 总表

| Task | 用户结果 | 状态 | 当前权威证据 |
|---|---|---|---|
| 1 大段 UTF-8 输入 | 大粘贴立即进入终端，透明分块且保持顺序 | **已闭合** | `runtime-task-1-utf8-input-chunking-report.md` |
| 2 拖入路径 | 普通/空格路径保持既有可见形式；复杂文件名只形成单个 argv，拖入不执行 | **已闭合** | `runtime-task-2-dropped-paths-report.md` |
| 3 Resize 合并 | 每帧最多一次且尺寸去重 | 待实施 | Task 3 |
| 4 App 回焦 | 恢复离开前的真实焦点 | 待实施 | Task 4 |
| 5 通知容量与 TTL | 工作空间通知有界且保留期可预测 | 待实施 | Task 5 |
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
