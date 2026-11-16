# PRD：多 Agent 接入（第二期：Codex 补齐）

| 字段 | 值 |
|---|---|
| 日期 | 2026-11-15 |
| 状态 | 待确认（已按多 Agent 底座既有口径实现） |
| 范围 | 让 Codex 卡片具备与 Claude Code / Antigravity 卡片同等的状态、通知、HUD、自动标题、载入会话、重启恢复、Fork |
| 依赖 | 第一期已合入 main：Agent 清单（`packages/domain/src/agent-manifest.ts`）、v35 迁移、Antigravity 完整接入 |
| 不在范围 | 账户切换（Codex 用官方登录）；Agent Team；服务供应商面板改动；Codex 本身的模型切换（沿用现有） |

## 1. 产品结论

Codex 卡片现在只能启动和恢复，用户看不到它在忙什么。本期补齐后，Codex 成为第三个「完整接入」的 Agent：卡片上能看工作状态、需要审批时通知你、HUD 显示模型与上下文、项目有 `AGENTS.md` 可以直接编辑，重启 App 能自动接回原对话，能 Fork 出子会话比较方案，也能载入本机已有的 Codex 会话。

## 2. 用户场景

- 同一个事项里 Claude 和 Codex 各跑几张卡片，用户在一个看板里看谁在等自己。
- Codex 有订阅、Claude 额度用完了，把任务交给 Codex，行为与管理方式不变。
- 想比较 Codex 和 Claude 的两种实现，用 Fork 各开一条，不污染当前目录。

## 3. 功能规格（对照现有 Claude / Antigravity 卡片）

| # | 功能 | Codex 卡片行为 | 与 Claude 的差异 |
|---|---|---|---|
| 1 | 新建卡片 | 与现在相同（Shell 输入 `codex` 升级、`mt create session --profile codex`）；不改入口 | 无 |
| 2 | 工作状态 | 启动中 → 空闲 → 工作中 → 需要输入（等待审批）/ 完成 / 出错，规则与 Claude 一致 | 无 |
| 3 | 通知 | 完成、等待审批、出错发 Matou 通知，标题 `Codex`；冷却、声音、聚焦免打扰与 Claude 相同 | 无 |
| 4 | HUD | 模型名与推理档位、上下文已用比例、当前权限模式、工具次数与正在运行的工具、目录、Git 分支与脏状态；项目存在 `AGENTS.md` 时显示编辑入口 | 不显示 5h/Weekly 额度（Codex 不提供）；待办仅在 Codex 自己写了计划时显示。拿不到的字段直接隐藏 |
| 5 | 执行模式 | 菜单四项与 Claude 一致；切换方式为重启并接回对话（Codex 的启动参数只能决定初始审批策略，运行中不能像 Claude 那样模拟切换） | 每次切换都会重启，和现在的 Codex 行为一致 |
| 6 | 自动标题 | 卡片标题跟随 Codex 会话名；手动改过的不被覆盖，可「恢复 Codex 自动标题」 | 无 |
| 7 | 载入已有会话 | 「载入会话」对话框增加 Codex 页签，列出本机某工作目录下的 Codex 会话（标题、更新时间），载入后接回对话 | 与 Antigravity 页签相同的交互 |
| 8 | 重启恢复 | App 重启后按原会话 ID 自动接回，失败时显示恢复失败横幅与「重试 / 新开」 | 无 |
| 9 | Fork | 完成过至少一轮回复后可 Fork；子卡片继承父对话上下文，可选当前目录或新 Worktree | 使用 Codex 自带的 `fork` 子命令，一步到位，没有 Antigravity 的两段式 |
| 10 | 指令文件入口 | HUD 显示 `AGENTS.md`（Codex 的项目指令文件），可编辑保存 | Claude 是 `CLAUDE.md` |
| 11 | `mt` 跨卡片控制 | 与现在相同（已通过开发者指令注入），不重新设计 | 无 |

## 4. 实现要点（2026-11-15 在本机 Codex CLI 0.162.1 上核实）

- **钩子**：Codex 支持 `SessionStart / UserPromptSubmit / PreToolUse / PostToolUse / PermissionRequest / Stop / SessionEnd / PreCompact` 等命令型钩子，载荷字段与 Claude 同形（`hook_event_name`、`session_id`、`transcript_path`、`tool_name`、`tool_input`、`tool_response`、`permission_mode`、`model`）。现有实现只注册了 `SessionStart` / `SessionEnd`，并在运行时入口对 Codex 直接返回、丢弃其余事件——这是本期要改的核心。
- **不给用户全局配置添东西**：沿用现有的单次启动注入（`-c hooks=…` + `--dangerously-bypass-hook-trust`），不写 `~/.codex/config.toml`。
- **HUD 数据来源**：Codex 没有状态栏回调。`transcript_path` 指向会话 rollout 文件，其中有 `token_count`（上下文用量）、`turn_context`（模型与权限）、工具调用与计划；HUD 从这些事件与文件读取，读不到就隐藏。
- **会话与标题**：会话在 `~/.codex/sessions/<年>/<月>/<日>/rollout-*.jsonl`，`session_meta.cwd` 用于按目录过滤；`~/.codex/session_index.jsonl` 的 `thread_name` 是会话名，用作自动标题。
- **恢复与 Fork**：`codex resume <session_id>`、`codex fork <session_id>`。
- **权限模式映射**：`default` / `acceptEdits`（`--ask-for-approval on-request` 下由 sandbox 控制）/ `plan` / `bypassPermissions`（`--dangerously-bypass-approvals-and-sandbox`）。

## 5. 验收

- 以现有 Claude 卡片的同场景行为为基线，在真实打包 App 中用真实 Codex 逐项验收，交付「交互对照矩阵」（第 3 节 11 项逐项：截图或运行证据、差异结论）。
- Claude 与 Antigravity 现有单元测试、端到端测试不出现新失败。
- `pnpm typecheck`、`pnpm check:identifiers` 通过。
