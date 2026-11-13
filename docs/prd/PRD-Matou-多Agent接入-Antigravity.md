# PRD：多 Agent 接入（第一期：Antigravity CLI）

| 字段 | 值 |
|---|---|
| 日期 | 2026-11-13 |
| 状态 | 待确认 |
| 范围 | 把 Matou 从「只认 Claude Code」改成「按 Agent 清单接入」，并完整接入 Antigravity CLI（`agy`） |
| 后续期次 | 第二期 Codex 补齐 → 第三期 DSH → 第四期 Kimi Code CLI，均复用本期底座 |
| 不在范围 | 账户切换与多账户；服务供应商面板；Agent Team；「订阅账号接入 Claude Code」PRD 中的 Antigravity 账号（那是给 Claude Code 当模型来源，与本期的 Antigravity 终端 Agent 是两件事） |

## 1. 产品结论

用户可以在 Matou 里像使用 Claude Code 一样使用 Antigravity：同样的入口新建 Antigravity 卡片，卡片上看得到它在工作、在等你确认、已完成还是出错；HUD 显示模型、上下文、额度和工具活动；App 重启后接回原对话；可以 Fork 出子会话、载入本机已有的 Antigravity 会话；Antigravity 也能用 `mt` 读取和操作隔壁卡片。

验收基线：reference product 没有 Antigravity，本期场景按「与 Matou 现有 Claude Code 卡片同入口、同反馈时机、同结果」验收；Claude 独有、Antigravity 不具备的能力明确列为不提供，不做占位入口。

## 2. 用户场景

- 有 Google AI Pro / Ultra 订阅的用户，想把一部分任务交给 Antigravity，同时继续用 Matou 管理多个并行会话。
- 同一个事项里 Claude 和 Antigravity 各开几张卡片做不同方案，用户在一个看板里看谁在等自己。
- App 重启或电脑重启后，Antigravity 卡片回到原位并接回原对话。

## 3. 功能规格（逐项对照 Claude Code 卡片）

| # | 功能 | Antigravity 卡片的行为 | 与 Claude 卡片的差异 |
|---|---|---|---|
| 1 | 新建卡片 | 与 Claude 相同：在 Shell 卡片里输入 `agy`（可带参数）回车，卡片自动升级为 Antigravity 卡片；`mt create session --profile`、Host Control、载入会话对话框都可选 `Antigravity` | 无（界面新建按钮仍默认建 Shell，与 Claude 一致） |
| 2 | 标签与卡片标识 | 标签、DAG 节点、父会话投影、子会话角标显示 `Antigravity` | 无 |
| 3 | 首次进入目录 | agy 会先询问「是否信任此文件夹」。卡片状态显示「需要输入」，用户在终端里选择即可 | 与 Claude 的工作区信任提示同一处理方式 |
| 4 | 工作状态 | 卡片、标签、看板实时显示：启动中 → 空闲 → 工作中 → 需要输入 / 完成 / 出错 | 无 |
| 5 | 通知 | 完成、等待审批、出错时发 Matou 通知，规则（只在需要你时打断、冷却、声音）与 Claude 相同；不额外打开 agy 自带的系统通知，避免同一件事通知两次 | 无 |
| 6 | HUD | 模型名与推理档位、上下文已用比例、额度（agy 的 5 小时 / 每周额度，按模型组显示）、当前执行模式、工具次数与正在运行的工具、目录、Git 分支与脏状态；项目存在 `AGENTS.md` / `GEMINI.md` 时显示其编辑入口（对应 Claude 的 `CLAUDE.md` 入口） | 不显示待办列表（agy 不提供结构化待办）；不显示 Claude 账户入口；拿不到的字段直接隐藏，不显示 0 或空值 |
| 7 | 自动标题 | 卡片标题跟随 agy 为对话生成的标题；用户手动改过的标题不被覆盖，可「恢复自动标题」 | 无 |
| 8 | 载入已有会话 | 「载入会话」对话框可切换到 Antigravity，列出当前工作空间目录下的 agy 历史会话（标题、更新时间、步数），载入后接回对话 | 对话框文案按 Agent 显示 |
| 9 | 重启恢复 | App 重启后，Antigravity 卡片按原会话 ID 恢复，失败时显示与 Claude 相同的恢复失败状态和「重试 / 新开」操作 | 无 |
| 10 | Fork | 卡片在完成过至少一轮回复后可 Fork（规则同 Claude）。子会话继承父对话上下文，可选当前目录或新 Worktree | agy 没有启动即分叉的参数：子卡片先接回父对话，再自动发送一次 `/fork`。用户会在子卡片里短暂看到这条命令，随后子卡片获得独立会话 ID；父卡片不受影响 |
| 11 | 执行模式 | HUD 权限菜单四项：默认（即 agy 的自动模式）、自动接受编辑、计划、跳过所有审批。切换方式与 Claude 相同：前三项之间切换不重启，由 Matou 向终端发送 Shift+Tab 并以状态栏回报的模式确认到位；进出「跳过所有审批」需确认并重启接回对话。用户在 agy 里按 Shift+Tab 切换后，HUD 同步显示 | 菜单第一项文案沿用 Claude 的「默认」 |
| 12 | 模型 | HUD 显示当前模型与推理档位；与 Claude 卡片一致，HUD 不提供模型切换入口，用户在 agy 内用它自己的 `/model` 切换，HUD 随状态栏同步 | 不进服务供应商面板 |
| 13 | `mt` 跨卡片控制 | Antigravity 卡片内可用自然语言让 agy 读取 / 操作隔壁卡片，流程与 Claude 相同 | 无 |
| 14 | 未安装 / 未登录 | 未找到 `agy`：卡片显示启动失败原因和安装指引。未登录：agy 自己的登录流程在终端内完成，卡片状态为「需要输入」 | 无 |

明确不提供：账户切换与用量面板（agy 的额度只在 HUD 展示）、Agent Team、服务供应商切换。

## 4. 实现要点（2026-11-13 在本机 agy 1.5.7 / 1.5.8 上实测）

### 4.1 事件来源
- **生命周期 hook**：agy 支持 `SessionStart / PreToolUse / PostToolUse / PreInvocation / PostInvocation / Stop` 六个事件，只支持命令型 hook（不支持 HTTP）。每个事件的 stdin 都带 `conversationId`、`transcriptPath`、`modelName`、`cwd`、`hookEventName`；`Stop` 另带 `terminationReason`（`NO_TOOL_CALL` 正常完成、`ERROR`、`USER_CANCELED` 等）、`error`、`fullyIdle`。
- **状态栏回调**：通过启动参数 `--settings '{"statusLine":{...}}'` 可单次注入，不改用户的全局设置。回调 JSON 带 `agent_state`（实测：`authenticating / initializing / idle / working / tool_use`）、`conversation_id`（首次开始工作后出现）、`model`、`context_window.used_percentage`、`quota`（各模型组 5 小时 / 每周剩余比例与重置时间）、`cycle_mode`（当前执行模式）。
- **等待审批**：hook 里没有审批事件。实测弹出审批框时 `agent_state` 变为 `tool_use`；自动放行的工具执行期间保持 `working`。实现阶段需要对几类工具（命令、编辑、浏览器）补测，确认 `tool_use` 只在等待审批时出现。

### 4.2 必须遵守的坑
- **PreToolUse 的 hook 必须什么都不输出**。实测输出 `{}` 会导致 agy 直接拒绝这次工具调用，等于把用户的 agent 卡死；不输出任何内容则不干预原有审批流程。
- **hook 会同步阻塞 agy**：脚本必须立即返回，上报放到后台进行，不能拖慢每次工具调用。
- **状态栏回调不能有输出**，并设置 `stack_with_default: true`，否则会替换掉 agy 自带的状态栏。用户自定义过 `statusLine` 时，Matou 的脚本要把输入转交给用户原来的命令，保留其显示。
- **状态栏给的 `transcript_path` 是错的**：它指向 `~/.gemini/antigravity/…`，实际文件在 `~/.gemini/antigravity-cli/brain/<id>/.system_generated/logs/transcript.jsonl`。以 hook 给出的路径为准。
- **agy 会自动升级**：实测期间从 1.5.7 升到了 1.5.8。字段缺失时一律降级隐藏，不能导致卡片状态错乱。

### 4.3 hook 的安装位置
- agy 只能从项目里的 `.agents/` 或全局的 `~/.gemini/config/` 发现插件。写进项目会污染用户仓库，所以 Matou 在 `~/.gemini/config/plugins/matou/` 放一个插件（`plugin.json` + 根目录 `hooks.json`）。
- 插件内的脚本只在 Matou 启动的终端里生效（靠 Matou 注入的会话环境变量），在 Matou 外运行 agy 时直接退出、不做任何事。已实测：没有环境变量时 hook 完全不起作用，有环境变量时全部触发。
- 用户执行 `agy plugin list` 会看到 `matou` 插件，这是可接受的代价。插件随 Matou 版本更新覆盖写入。
- `mt` 使用说明通过 `SessionStart` 的 `injectSteps.systemMessage` 注入（已实测 agy 能读到），不需要单独装技能。

### 4.4 会话、恢复与分叉
- 恢复：`agy --conversation <id>`，已实测上下文完整。
- 分叉：交互内的 `/fork`，已实测会生成新 ID 并带着上下文继续。子卡片流程：以 `--conversation <父 id>` 启动 → 状态栏报告空闲且 ID 等于父 ID → 发送 `/fork` → 状态栏 ID 变为新值，记为子会话身份。超时或 ID 未变化则按 Fork 失败处理，与 Claude 的失败状态一致。
- 实现阶段需要验证：父卡片运行中时，子卡片接回同一对话再 `/fork`，不会写坏父对话。
- 历史会话列表：`~/.gemini/antigravity-cli/cache/conversation_metadata.json` 的 `conversations.<id>.summary` 含 `Title`、`UpdatedAt`、`WorkspaceURIs`、`NumSteps`、`AppDataDir`、`Internal`；按 `AppDataDir = antigravity-cli`、非 `Internal`、工作目录匹配来筛选。自动标题同样取自 `Title`。
- 启动参数：`--model`、`--mode accept-edits|auto|plan`、`--dangerously-skip-permissions`、`--settings`。

## 5. 底座改造（本期一并完成，后续三期复用）

- 现在约 30 个文件把会话类型写死为 `shell | claude-code | codex`，涉及数据模型、数据库、RPC、`mt` 参数、主控指令格式、前端类型、标签名称、终端配置。本期改成一份 Agent 清单，每个 Agent 声明：显示名称、可执行文件、启动 / 恢复 / 分叉方式、事件来源、HUD 可提供的字段、`mt` 注入方式，以及是否支持 Fork、载入会话、自动标题等能力。
- 界面只根据清单声明的能力显示入口：不支持的能力不显示入口，不提供点了才报错的按钮。
- Claude Code 与 Codex 的现有行为保持不变；对应的回归测试必须全部通过。
- 通知标题、恢复失败文案、载入对话框等写死 "Claude Code" 的文案改为按 Agent 显示。

## 6. 验收

- 每个场景以 Matou 现有 Claude Code 卡片的同场景表现为基线，在真实打包 App 中使用真实 agy 验收，交付「交互对照矩阵」：第 3 节 14 项逐项给出截图或运行证据与差异结论。
- 交互性能：hook 不得让 agy 每次工具调用的等待增加到可感知的程度；HUD 和状态变化在事件发生后 1 秒内反映到卡片上。
- 回归：Claude Code 与 Codex 现有的端到端测试全部通过；`pnpm check:identifiers` 通过。
