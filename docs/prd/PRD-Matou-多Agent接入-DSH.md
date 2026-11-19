# PRD：多 Agent 接入（第三期：DSH）

| 字段 | 值 |
|---|---|
| 日期 | 2026-11-15 |
| 状态 | 调研中（DSH 与外置 agent 架构差异大，先交方案再定规格） |
| 范围 | 让 DSH 卡片具备与 Claude Code / Antigravity / Codex 卡片同等的状态、通知、HUD、自动标题、载入会话、重启恢复、Fork、`mt` 跨卡片控制 |
| 依赖 | 多 Agent 底座（Agent 清单 + v35 迁移，已合入 main）；第二期 Codex 接入 |
| 不在范围 | 账户切换、服务供应商面板、Agent Team |

## 1. 背景

DSH 不是单个 agent 进程，而是一个启动器（`dsh`）加可组合的 profile。用户当前使用的 `dsh-tui` profile 是第三方终端界面（`@deepseek-harness-tui/dsh-tui`，0.19.0），底层可选 DSH / Claude / Codex 三种后端。因此接入方式和前三期不同：Matou 面对的是一个自带完整界面的终端应用，状态、会话、标题、Fork 都在它内部或 DSH 插件层。

## 2. 产品结论（目标）

用户在 Matou 里开一张 DSH 卡片，和在其它终端里直接运行 dsh-TUI 的体验一致；同时 Matou 能显示它的工作状态、在需要审批或完成时通知、显示 HUD、重启后接回原对话、按目录载入已有会话，并允许 Fork。

## 3. 功能规格（与前三期对齐）

| # | 功能 | 期望行为 |
|---|---|---|
| 1 | 新建 | 与其它 Agent 一致：Shell 输入启动命令升级，或 `mt create session --profile dsh` |
| 2 | 工作状态 | 启动中 / 空闲 / 工作中 / 需要输入 / 完成 / 出错 |
| 3 | 通知 | 完成、等待审批、出错，标题 `DSH`；不重复 dsh-TUI 自带的终端通知 |
| 4 | HUD | 模型与推理档位、上下文占用、执行模式、工具活动、目录、Git；项目指令文件（以 dsh-TUI 实际读取的文件为准）可编辑 |
| 5 | 执行模式 | 以 dsh-TUI 实际提供的模式为准，对齐现有菜单 |
| 6 | 自动标题 | 跟随 dsh-TUI / DSH 会话标题；手动锁定与恢复 |
| 7 | 载入会话 | 按工作目录列出 DSH 历史会话，可预览、搜索、载入 |
| 8 | 重启恢复 | App 重启后接回原对话；失败有重试 / 新开 |
| 9 | Fork | 完成一轮后可 Fork，子会话独立 |
| 10 | `mt` 跨卡片 | DSH 卡片内可读取、操作其它卡片 |
| 11 | 不提供 | 账户切换、服务供应商面板、Agent Team；dsh-TUI 已有能力不重复实现 |

## 4. 待调研（第一关交付）

- 启动方式：`dsh dsh-tui` 与 `dsh --profile dsh-tui` 的区别；能否在不改用户 profile 的前提下，为单次会话注入插件 / 补丁层（`--patch`）、开发者指令或 `mt` 说明。
- 身份与会话：会话 ID 如何获得（界面、文件、参数）；`--resume <session>` 的确切形式；会话目录与 `session.v4.jsonl.zstd` 的结构、标题来源。
- 事件：DSH / dsh-tui 是否提供钩子（`/hooks` 目前为占位）、外部通知插件接口、或可监听的日志/文件；能否拿到工具开始/结束、等待审批、轮次完成与错误。
- Fork：界面内 fork 还是命令行参数；新会话 ID 如何确认。
- HUD 数据：模型、上下文、token、费用、Git 在会话文件或插件事件中的来源。
- 与 Matou 现有页签、会话目录、`mt` 注入的复用边界。

## 5. 验收

真实打包 App + 本机真实 dsh-TUI，逐项验收并交付交互对照矩阵；Claude / Antigravity / Codex 无新失败；`pnpm typecheck`、`pnpm check:identifiers` 通过。调研阶段若某项（如钩子）被证实不存在，如实记录并给出替代方案或标为不提供。
