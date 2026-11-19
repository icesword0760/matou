# Matou 多 Agent 第二期：Codex 完整接入方案（第一关）

日期：2026-11-15。工作区：`/Users/icesword/Documents/AIProjects/matou-multi-agent-codex`；分支：`feat/multi-agent-codex`。

状态：**方案待审，未实施产品改动**。本文件是本关唯一新增的项目文件；开始时已存在未跟踪的 `docs/prd/PRD-Matou-多Agent接入-Codex.md`，保留不动。未提交、未推送、未改版本、未新增依赖。

## 1. 产品结果与范围

Codex 卡片接入现有看板、通知、HUD 和会话管理：用户能知道它在工作、等审批、完成还是失败；看到真实模型、推理档位和上下文；载入与重启后接回原对话；完成一轮正常回复后 Fork；编辑项目 `AGENTS.md`。

沿用已定方案：启动参数注入 hooks，不改 `~/.codex/config.toml` / `~/.codex/hooks.json`；同形钩子进入 Claude 已有解析入口，保留 Antigravity 专属路径；没有状态栏回调不伪造 statusline；增加 rollout 格式读取分支，不再建平行 HUD / 通知系统。仅 Codex 的清单、字段适配及必要入口扩展发生变化。Claude / Antigravity 的参数、默认状态、菜单、通知、HUD 和权限切换均保持原样。供应商面板、账户、Agent Team 不在范围。

### 1.1 本轮已经获得的裁决

- 监督回答 #1 允许在**项目外临时 Git 仓库**做无害探针，真实 HOME 保留；仅启动参数注入钩子；正常新增的 Codex 会话记录可保留。没有修改全局配置，也没有把用户主仓库或其他 worktree 用作探针。
- 监督回答 #2 确认：Codex 菜单中的「计划模式」采用只读沙箱，**不追求原生 Plan 协作模式，不注入 `/plan`**；菜单文案和 HUD 不另加说明。default 不传审批或沙箱参数；acceptEdits 用 workspace-write；plan 用 read-only；bypass 沿用危险绕过参数。审批参数取舍依据见第 5 节。

## 2. 真实 Codex 实测与证据边界

### 2.1 环境、方法和结果

本机 `codex --version` 返回 **`codex-cli 0.162.1`**；已只读核对 `codex exec --help`、`codex resume --help`、`codex fork --help`。npm 安装包版本也为 0.162.1。

临时仓库：`/var/folders/pz/y1l2jfbn2k78f6__84v18jhc0000gn/T/matou-codex-probe-vvej5zuq`。命令形态：

```text
codex exec -C <临时仓库> --dangerously-bypass-hook-trust -c 'hooks=<TOML 内联表>' --json <无害探针提示>
```

内联 hooks 每项形态为 `{ hooks = [{ type = "command", command = "python3 <临时仓库>/capture.py", timeout = 5 }] }`；采集脚本只将 stdin JSON 追加到临时 `hooks.jsonl`，无 stdout，退出 0。为观测缺口一并配置了 PRD 和现有 Claude 注册表涉及的事件名；**配置未报错不代表每个事件受支持或已触发**。SessionEnd 超时被 CLI 自动限制为 3 秒，后续生成配置直接使用 3 秒。

提示仅要求读临时 `probe.txt`、执行 `python3 -c 'print(1)'`、回复 `PROBE_OK`。探针确实只执行了这两条本地命令，输出 1 和 `PROBE_OK`，CLI **exit 0**。未指定模型或推理档位，沿用用户当前默认；实际报告 `gpt-6-astra` / `medium`。启动伴有本机已有 skill/MCP 配置告警，但不影响本次完成；告警不能当作会话失败通知。

原始证据：临时仓库下 `hooks.jsonl`、`exec-output.jsonl`；持久化 rollout：

```text
~/.codex/sessions/2026/11/15/rollout-2026-11-15T13-38-02-01a1ddb6-6cd5-7dc0-a3ae-59fd9b584d14.jsonl
```

下列样例摘自真实记录；省略无关字段，将长路径写为 `<probe>` / `<rollout>`，不收录账户标识、指令全文或其他对话正文。

### 2.2 钩子载荷：已观察到 8 条、6 类事件

事件顺序：SessionStart → UserPromptSubmit → PreToolUse / PostToolUse 两对 → Stop → SessionEnd。

```json
{"hook_event_name":"SessionStart","session_id":"01a1ddb6-6cd5-7dc0-a3ae-59fd9b584d14","transcript_path":"<rollout>","cwd":"<probe>","model":"gpt-6-astra","permission_mode":"bypassPermissions","source":"startup"}
{"hook_event_name":"UserPromptSubmit","session_id":"01a1ddb6-6cd5-7dc0-a3ae-59fd9b584d14","turn_id":"01a1ddb6-6e2f-73f2-906e-1932e0620748","transcript_path":"<rollout>","cwd":"<probe>","model":"gpt-6-astra","permission_mode":"bypassPermissions","prompt":"<probe prompt>"}
{"hook_event_name":"PreToolUse","session_id":"01a1ddb6-6cd5-7dc0-a3ae-59fd9b584d14","transcript_path":"<rollout>","cwd":"<probe>","model":"gpt-6-astra","permission_mode":"bypassPermissions","tool_name":"Bash","tool_input":{"command":"python3 -c 'print(1)'"},"tool_use_id":"exec-e8440da5-95e0-47f7-a487-d3a42893ae3f"}
{"hook_event_name":"PostToolUse","session_id":"01a1ddb6-6cd5-7dc0-a3ae-59fd9b584d14","transcript_path":"<rollout>","cwd":"<probe>","model":"gpt-6-astra","permission_mode":"bypassPermissions","tool_name":"Bash","tool_input":{"command":"python3 -c 'print(1)'"},"tool_response":"1\n","tool_use_id":"exec-e8440da5-95e0-47f7-a487-d3a42893ae3f"}
{"hook_event_name":"Stop","session_id":"01a1ddb6-6cd5-7dc0-a3ae-59fd9b584d14","transcript_path":"<rollout>","cwd":"<probe>","model":"gpt-6-astra","permission_mode":"bypassPermissions","stop_hook_active":false,"last_assistant_message":"PROBE_OK"}
{"hook_event_name":"SessionEnd","session_id":"01a1ddb6-6cd5-7dc0-a3ae-59fd9b584d14","transcript_path":"<rollout>","cwd":"<probe>","reason":"other"}
```

结论：`providerSessionIdentity`、`providerTranscriptPath`、`providerCurrentModel` 已识别上述身份/路径/模型；`toProviderNotificationEvent(payload, 'codex')` 的正常 Stop 和 PermissionRequest 分支可直接复用，标题自动得到 `Codex`。HUD 目前只认对象型 model，须补 Codex 字符串型模型，不能把身份解析成功误当 HUD 解析成功。

本探针没有审批、工具失败、压缩、取消或模型失败，所以 **PermissionRequest、PreCompact、Notification、PostToolUseFailure 的实际触发及负载均未验证**。本机二进制能检索到 PermissionRequest / PreCompact 等名字，但检索不到 PostToolUseFailure / StopFailure；这只是线索，不能据此宣布支持或不支持。后续只注册已确认可用的事件，不照抄 Claude 名单；错误通知也不能假设存在 StopFailure。

### 2.3 rollout 关键字段：已读取真实落盘记录

```json
{"type":"session_meta","payload":{"id":"01a1ddb6-6cd5-7dc0-a3ae-59fd9b584d14","session_id":"01a1ddb6-6cd5-7dc0-a3ae-59fd9b584d14","cwd":"<probe>","cli_version":"0.162.1","originator":"codex_exec","source":"exec","thread_source":"user","history_mode":"paginated","timestamp":"2026-11-15T05:38:02.608Z"}}
{"type":"turn_context","payload":{"turn_id":"01a1ddb6-6e2f-73f2-906e-1932e0620748","cwd":"<probe>","model":"gpt-6-astra","effort":"medium","approval_policy":"never","sandbox_policy":{"type":"danger-full-access"},"permission_profile":{"type":"disabled"},"collaboration_mode":{"mode":"default","settings":{"model":"gpt-6-astra","reasoning_effort":"medium"}}}}
{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":94277,"cached_input_tokens":62464,"output_tokens":94,"reasoning_output_tokens":0,"total_tokens":94371},"last_token_usage":{"input_tokens":31526,"cached_input_tokens":31232,"output_tokens":7,"reasoning_output_tokens":0,"total_tokens":31533},"model_context_window":258400},"rate_limits":{"limit_id":"codex","primary":{"used_percent":23.0,"window_minutes":10080}}}}
{"type":"event_msg","payload":{"type":"item_completed","item":{"type":"CommandExecution","id":"exec-e8440da5-95e0-47f7-a487-d3a42893ae3f","command":["/bin/zsh","-lc","python3 -c 'print(1)'"],"status":"completed","exit_code":0,"aggregated_output":"1\n"}}}
{"type":"response_item","payload":{"type":"custom_tool_call","name":"exec","call_id":"call_f39034e361f34d6b8ffcfaf46c610ad3","input":"text(await tools.exec_command({cmd:\"python3 -c 'print(1)'\",max_output_tokens:2000}));\n"}}
{"type":"event_msg","payload":{"type":"task_complete","turn_id":"01a1ddb6-6e2f-73f2-906e-1932e0620748","last_agent_message":"PROBE_OK","duration_ms":16406}}
```

- `session_meta` 是行的 `type`，cwd 真正路径是 **`row.payload.cwd`**，ID 是 `row.payload.id`，不能读成顶层 cwd 或用文件名推断项目归属。
- `token_count` 是 **`row.type === 'event_msg' && row.payload.type === 'token_count'`**；模型窗口在 `payload.info.model_context_window`。`total_token_usage` 是跨请求累计，不能当当前上下文。
- 当前用量按最近有效 `last_token_usage.total_tokens / model_context_window` 展示估计比例，本样例为 `31533 / 258400 ≈ 12.2%`，复用现有四舍五入显示 12%。缓存 tokens 已包含在 input 中，不再加一次；reasoning 也不重复加。该值是最近请求的占用估计，并非精确测量当下全部本地上下文；压缩后等待新有效样本，不继续展示旧比例。后续需和真实 TUI 同场景核对。
- `turn_context.payload.effort` 是本版本实测推理档位；`collaboration_mode.settings.reasoning_effort` 可作为相同记录的回退字段。没有值不补 medium。
- 工具 hooks 报告 `Bash`，rollout 外层调用却叫 `exec`；**不能把两者相加，不能执行或解析任意 JavaScript 以反推调用**。本样本的 `CommandExecution.id` 与 hook 的 `tool_use_id` 完全相同，可统一按该 ID 去重并归为 Bash；通用工具按已验证类型适配，未知条目不编造次数。
- 实际 token_count 中有 rate_limits。因此 PRD 中“Codex 不提供额度”不能作为事实依据；**仍照已定产品决定隐藏全部 5h/Weekly 额度及空占位**，不扩展本期范围。
- 本轮没有让 Codex 写计划，未观察到计划工具。`update_plan` / plan 字段在编码阶段取得真实样本再接入；没有样本或字段时隐藏待办，不能把普通助手正文当计划。

### 2.4 标题索引与 exec 的差别

只读检查真实 `~/.codex/session_index.jsonl`，存在如下字段结构（标题值脱敏）：

```json
{"id":"01a1b34f-d304-7a40-9759-77d35c9e9160","thread_name":"<existing real title redacted>","updated_at":"2026-11-14T15:23:02.93076Z"}
```

本探针 ID **没有**出现在 session_index 中。已证实索引字段结构，未证实本次 exec 会自动生成标题；不能承诺每条 rollout 都有名字。会话目录必须来自 rollout，索引只补名字/更新时间，不能从索引独立造出可载入会话。无 thread_name 时保留卡片原名；列表复用现有无标题展示并以时间/ID 区分，不把首条提示写成 Codex 自动标题。真实 TUI 自动命名、改名后的索引更新时间仍需后续实测。

## 3. 已确认的代码差距与最小实现路径

1. `provider-hook-server.ts` 的 codexHookSettings / codexInlineHookConfig 只生成 SessionStart、SessionEnd；身份写入后又直接返回，HUD、标题、通知被丢弃。扩展**实际用于启动的内联配置**及对应 JSON 描述；只移除 Codex 的副作用短路，保留身份拒绝、run 所有权、retirement 和 Fork lease 校验。
2. 清单中 Codex fork/sessionLoader/autoTitle/autoRestore 为 false，instructionFiles 错为 CLAUDE.md。改为已定值，forkRequiresCompletedTurn=true，permissionSwitchMode 保持 respawn-only。statusLineSource 保持 none，transcriptPathSource 精确设为 hook。
3. 不能仅开开关：`ProviderTranscriptHudReader` 目前识别 Claude 的 message/tool_use，`SessionHudRegistry` 会把 Codex 未知模型回退成 opusplan；需在现有读取器和 registry 内增加 Codex 行类型与字段适配，避免错误模型、空字段和历史回刷覆盖实时状态。
4. 会话 RPC 和前端 provider 联合类型/页签只含 Claude、Antigravity；新增 `CodexSessionCatalog` 并扩展现有路由和列表，复用现有返回结构、分页、搜索、预览及重复载入确认。**不新增 RPC 命名空间，不重命名历史 ClaudeSession 类型。**
5. 现有 Codex 启动计划只处理 bypass；acceptEdits/plan 实际不加参数。此处已按第 1.1 节获得产品裁决，补 Codex 参数映射，其他两种 Agent 不变。
6. 现有正常 Stop 门禁可复用，但“每个 Stop 都算正常完成”对错误/取消须验证；需要 Codex 专属限定，不能改变 Claude 的现有判定。

### 3.1 HUD 接入方式与字段来源

仍走 `ProviderHookServer → createProviderHudPayloadHandler → SessionHudRegistry → terminal.hud → TerminalHud`。rollout 在同一个 ProviderTranscriptHudReader 的增量读取器里解释，不建立第二个 HUD。补充内部快照的可选 model/effort/context 字段，现有 `SessionHudWire` 已有对应字段，无需协议版本迁移。

| HUD 项 | 确切来源 / 合并方式 | 缺失与边界 |
| --- | --- | --- |
| 模型 | hook.model 字符串；rollout 最新 turn_context.payload.model 校正，统一转入已有 model 字段 | 不显示 opusplan 或猜测的 Claude 模型；未观察值仅可用真实已选启动模型 |
| 推理档位 | 最新 turn_context.payload.effort，回退其 collaboration_mode.settings.reasoning_effort | 未取得时隐藏；TerminalHud.modelLabel 当前不消费独立 effort，需只为 Codex 拼接已观察档位；真实 `/model` 改动后新 turn_context 能覆盖旧值 |
| 上下文比例 | 最新有效 event_msg/token_count 的 info.last_token_usage.total_tokens 与 info.model_context_window | 分母须有限正数，分子须有效；缺失、null、未知版本隐藏（Codex 同时抑制现有“等待真实用量”占位）；明确真实 0 才能显示 0，不用累计 total_token_usage；压缩使旧样本失效 |
| 权限 | hook.permission_mode 为主，最新 turn_context 的 approval_policy / sandbox_policy / permission_profile 用于校验和回退 | 显示已确认有效模式；切换中沿用现有 pending 状态，不因启动请求就宣称已生效；default 可被用户配置实际解析为其他策略 |
| 运行中工具 | PreToolUse.tool_name / tool_input / tool_use_id；PostToolUse 同 ID 移除，Stop 清空 | 不等 rollout IO 才显示；晚到的历史快照不能重新挂起已完成工具 |
| 工具总数 | hooks 与 rollout CommandExecution 等条目以稳定执行 ID 合并；已完成 ID 只计一次 | 当前样本两次 Bash，恢复后仍两次；外层 exec 不另加；没有可靠 ID 的历史工具信息隐藏 |
| 待办 | 真实计划工具的结构化结果，映射到已有 todos | 实测尚缺，不从正文猜测；后续取得样本再启用可见数据 |
| 目录 | hook.cwd；会话列表从 session_meta.payload.cwd，运行目录依现有 cwd tracker | Worktree 继续显示卡片实际执行目录，不能被继承历史中的父 cwd 回写 |
| Git 分支/脏标 | 现有 RuntimeServer.refreshSessionHud / Git 状态缓存 | 不从历史 session_meta.git 当实时结果 |
| AGENTS.md | manifest.instructionFiles.candidates + inspectProviderConfig / 现有指令文件 RPC | 只在当前项目文件存在时显示入口；编辑保存仍走现有编辑器 |
| 额度/账户/MCP/子 Agent/hook 数 | Codex 清单关闭额度与账户控件、空额度占位；registry 不转发 Codex rate_limits，TerminalHud 按开关过滤已有窗口数据（supportsQuotaWindows 当前尚无消费方，不能只改清单）；未有可靠 Codex 数据的计数关闭 | 不读取 Claude 配置填 Codex 数据，不改供应商面板；待办 supportsTodos 可保留 true，但数据缺失不可见 |

增量读取复用 offset/remainder/inFlight/截断重读机制；仅解析完整 JSON 行，未知 type 跳过。恢复时先读取历史再接实时钩子；已有 CommandExecution 的执行 ID 与实时 ID 合并，避免 `refreshTranscript` 用空历史覆盖活动工具。Codex 的实时模型不能被“首次观察后不再更新”的旧标记永久锁住。

hooks 到达时先发布实时状态，再异步读 rollout。为补齐 Stop 先到、最终 token_count 后落盘的竞态，Codex 分支利用现有可见 HUD 刷新周期重新增量读取 registry 内保存的 transcriptPath；后台卡片在其 hook 触发时刷新，用户切回时立即刷新。无需全盘持续扫描、无独立轮询服务；每次读后复核 runId/对象所有权，SessionEnd 后不再让旧路径恢复 HUD。目录/文件变化不阻塞终端输入。

### 3.2 状态、通知和错误的复用边界

- SessionStart 确认身份和空闲；UserPromptSubmit / PreToolUse 为工作中；PermissionRequest 为 needs-input；正常 Stop 为空闲并发完成通知。沿用既有 `providerModes.observeHook`、HUD taskStatus、通知状态转换与语义投影，完成继续用已有状态表达，不新增第六种看板状态。
- `toProviderNotificationEvent` 的标题由 manifest 取到 Codex，完成/审批共用现有 cooldownKey、声音、聚焦抑制和通知中心发布。接收器只观察，绝不自动代批 PermissionRequest。
- **错误不能只删短路就算完成**：当前非 Antigravity Stop 总被当作成功，Notification 文本分支也不会产生 error。后续在同一个转换器添加仅 Codex 生效的、基于真实失败样本的 error 判定，复用既有 `eventType:'error'` 和发布路径；不修改 Claude / Antigravity 分支。
- 非零进程退出/启动失败沿用 RuntimeServer 的失败处理；Codex 进程仍存活的请求失败须补实际 TUI/rollout 错误样本，优先读取结构化终止错误；若只能由明确终端错误判定，则只在既有 TerminalWorkStatusTracker 的 Codex 分支增加已实测特征，同时经既有错误通知转换器发布。普通日志中的 ERROR、可恢复工具失败、启动 skill/MCP 告警不能算整轮失败。
- 未知错误字段不猜成成功：不发虚假完成、不解锁 Fork；保留真实终端输出供用户处理。取消不发完成通知、不计正常回复。上述失败/取消样本是第二关状态验收的前置缺口，未经验证不能宣称 #2/#3/#9 闭合。

## 4. PRD 第 3 节：11 项能力逐项落点

以下路径均相对本工作区。

| # | 用户能完成什么 | 实现落点与复用机制 | 该项验收要点 |
| --- | --- | --- | --- |
| 1 新建卡片 | 原入口输入 codex 或 mt create session --profile codex | runtime-server.ts 现有命令升级、provider-launch-plan.ts；不改入口 | Shell 升级/mt 创建、焦点、重复按键、未安装错误 |
| 2 工作状态 | 同看板识别空闲/工作/等审批/完成/出错 | provider-hook-server.ts、session-hud-registry.ts、session-canvas/provider-mode-service.ts、index.ts 现有状态投影；Codex 启用 terminal-work-status-tracker.ts 处理登录/信任提示 | 真实六类 hook 顺序；审批、取消、模型失败；旧 run 晚到不改新卡状态 |
| 3 通知 | Codex 完成、审批、失败通知 | provider-notification-event.ts → index.ts → notifications/agent-notification-repository.ts；仓库现有声音/冷却/聚焦规则 | 标题 Codex；后台/聚焦两种；同事件不重复；错误不伪装完成 |
| 4 HUD | 看到真实模型/档位/上下文/工具/目录/Git | provider-hud-payload-handler.ts、provider-transcript-hud.ts、session-hud-registry.ts、runtime-server.ts；前端 hud/TerminalHud.tsx 复用布局 | 第 3.1 节字段逐一核对；空值隐藏、无额度/账户、两次 Bash 不变四次 |
| 5 执行模式 | 四项菜单，切换重启接回 | agent-manifest.ts、provider-launch-plan.ts、runtime-server.ts 的 respawn 路径 | 四种模式实际策略/参数/新 PID/同会话 ID；切换失败保留恢复入口 |
| 6 自动标题 | 跟随 thread_name，手动标题锁保护，可恢复自动标题 | codex-session-catalog.ts 提供索引读取；provider-hook-server.ts 扩展 #observeTitle 来源，仍调用 onTitleObserved；domain/session-repository.ts 的 observeProviderTitle / restoreProviderTitle 不另写规则 | 手动锁、恢复自动、索引缺失/更新/原子替换；不从 rollout 首条正文起自动标题 |
| 7 载入已有会话 | 增加 Codex 页签，列表/预览/搜索/载入一致 | codex-session-catalog.ts；rpc/runtime-rpc-router.ts 现有 claude-sessions.* 路由；session-canvas/SessionLoaderDialog.tsx 与 hierarchy 调用链扩展 provider | 按真实 cwd 过滤、分页、全文命中跳转、重复载入确认、接回指定 UUID |
| 8 重启恢复 | App 重启后接回；失败有重试/新开 | manifest.autoRestore=true；runtime-server.ts、provider-resume-monitor.ts、session-canvas/provider-mode-service.ts 现有 binding/恢复握手；TerminalPane 现有横幅 | 原 UUID 与记忆保持；ID 不符/会话不存在/未登录/启动失败；失败不静默新开 |
| 9 Fork | 正常一轮后当前目录/新 Worktree 分出独立对话 | 清单开关和 forkRequiresCompletedTurn；provider-launch-plan.ts 已有 fork 子命令；session-canvas/fork-workflow-service.ts、fork-operation-coordinator.ts、provider-hook-server.ts 现有意图/租约/新身份确认 | 子 UUID 必须不同于父；父运行时 Fork、双击/重试/崩溃恢复；父绑定/文件不变 |
| 10 指令文件 | HUD 编辑保存当前项目 AGENTS.md | 清单 candidates=['AGENTS.md'] / buttonLabel='AGENTS.md'；rpc/runtime-rpc-router.ts 的 #sessionInstructionsTarget 与已有编辑器、TerminalHud | 不显示 CLAUDE.md；当前/Worktree 指向正确文件；缺失隐藏；保存立即读回 |
| 11 mt 跨卡控制 | 保持现有跨卡读取/控制 | provider-launch-plan.ts 的 developer_instructions；apps/runtime/control-assets/providers/codex-developer-instructions.md 现有资产 | 打包资产存在；Codex 自然语言场景实际读兄弟卡；不修改 mt 协议或放宽其他工具审批 |

自动标题监听只读 session_index，按 UUID 精确取最新有效 thread_name（重复行按 updated_at，再按追加顺序取最新），不按标题匹配身份。Codex 活跃注册期间监听索引所在目录、去抖，并处理原子替换；继续复用 registration 的清理和所有权保护。Codex 监听保留到本 run retire/dispose，避免现有 Claude 的 10 秒窗口漏掉较晚索引更新；**不更改 Claude/Antigravity 的窗口策略**。没有索引时保留原名，不让空标题覆盖手动名。

## 5. 恢复、载入与 Fork：参数和身份

### 5.1 启动参数

```text
新会话： codex [本次 hooks / mt / 原有模型与档位参数] [权限参数]
恢复：   codex [本次注入参数] [权限参数] resume <provider_session_id>
Fork：   codex [本次注入参数] [权限参数] fork <parent_provider_session_id>
```

实际产品继续由 PTY cwd / 新 Worktree 的执行上下文设置工作目录；不使用 Codex 自己的 --worktree 创建第二套 Worktree。保留当前 `-c hooks=…`、`--dangerously-bypass-hook-trust`、developer_instructions 和已选模型/档位，不写全局文件；用 argv 数组传 ID，避免 shell 拼接。加载/恢复按 UUID，绝不使用 --last 或标题名。

| Matou 模式 | Codex 拟定参数 | 依据与验证状态 |
| --- | --- | --- |
| default | 不传 --ask-for-approval / --sandbox | 监督明确要求沿用用户 config；本轮探针未指定权限却实际报告 bypassPermissions，正好证明不能声称 default 必等于某个固定沙箱 |
| acceptEdits | --ask-for-approval on-request --sandbox workspace-write | 本机 help 确认两参数及枚举可用；显式 approval 避免从用户 never 继承成意外不审批；实际交互审批尚待第二关验证 |
| plan | --ask-for-approval on-request --sandbox read-only | 已裁决的只读语义；help 确认；原生 Plan 协作状态不作为验收条件，不能自动输入 /plan |
| bypassPermissions | --dangerously-bypass-approvals-and-sandbox | 当前已有实现，不额外叠加冲突参数；本轮真实 hook / turn_context 观察到 bypassPermissions / never / danger-full-access，但未单独测试切换过程 |

权限切换始终 respawn-only，走现有会话绑定和重启握手；重启前记录目标模式，实际报告不一致不能在 HUD 假称成功。default 恢复用户默认配置的具体 CLI 行为（包括 resume 是否继承历史策略）须真实验证；若不符合上述产品决定，先报告再调整，不私自覆盖用户配置。

### 5.2 载入与目录

新 CodexSessionCatalog 使用默认 `~/.codex`（进程已有 CODEX_HOME 覆盖时读取同一个实际目录，不新增设置页面），扫描 sessions 日期目录的 rollout 元信息，使用 canonicalPath 对比 `session_meta.payload.cwd` 与卡片当前工作目录。不只按文件名，不把子目录误算成同项目；目录不存在/没有会话返回空列表，拒绝权限与损坏文件有可读反馈。

同 UUID 的记录去重，列表标题以索引 thread_name 为准，更新时间取有效索引更新时间和 rollout 实际变化时间的最新值；已有 Matou 手动载入标题优先关系保持。列表先完成元信息过滤，预览/全文搜索按需增量索引，复用 session-catalog-utils 的 JSONL 流式扫描、偏移分页和有界缓存，不将所有历史正文一次性读入内存。

消息预览只取用户/助手消息及工具事件，过滤 system/developer 指令与内部元信息。rollout 同时存在 response_item.message 和 event_msg.item_completed 的消息镜像：选一个稳定表示（本版本优先结构化 item_completed；缺少时才用 response_item）并按消息 ID 去重；搜索与预览必须共用相同 eventIndex。detail/search/load 均再次检查 cwd 与 UUID；UI 不提交任意文件路径。目标会话的 transcriptPath 从已校验目录结果/当前 hook 获得，文件名 UUID 回退仅作兼容，不能替代 authoritative identity。

### 5.3 身份与恢复握手

Matou sessionId、runId 和 Codex providerSessionId 是不同身份。hook 的 session_id 是 Codex UUID；`ProviderHookServer` 的独立随机 URL 将其归属到特定 Matou 卡和 run。恢复/载入设置 expectedProviderSessionId，身份一致后才写 binding、HUD、标题和通知；不一致沿用恢复失败路径，拒绝后续副作用，不悄悄绑定一个新会话。保持现有 run fence、retire grace、恢复失败横幅和重试/新开行为。

载入列表保留 loaded-here / loaded-elsewhere 与「仍然载入」现有确认，不改变其他 provider 行为。该确认只提示同一对话并开风险，不声称防止用户在 Matou 外同时 resume。

### 5.4 Fork 门禁与父子安全

使用现有 fork intent / 工作目录准备 / lease / operation coordinator，不复制 rollout 文件。源需有可恢复 binding 且 `canFork=true`，来自正常 UserPromptSubmit + 正常 Stop；仅启动或工具返回不能解锁，取消/失败不能记为正常 Stop。载入有历史的会话须确认至少有一轮有效正常回复，不能因为“来自 catalog”就对空/失败会话设置 canFork=true；现有 loadClaudeSession 无条件设置 true 的路径只为 Codex 增加验证结果限制。

子启动**直接** `codex fork <父UUID>`，不先 resume 父会话，不走 Antigravity 的 /fork 两段式。子第一个权威 hook 必须携带与父不同的新 UUID，再确认 lease 并完成 intent。当前 immediate-new-id 语义不足以自动拒绝“错误返回父 ID”，须在 Codex 注册中携带父 UUID，并在**首次写 binding 前**做不同性验证；不能仅在写入以后检查，也不能误触 Antigravity 的 slash-ready 回调。无需新增持久表，复用 intent 已保存的 sourceProviderSessionId 和注册记录。

若子返回父 ID、没有 ID、旧 run 上报或 Fork 失败，子 intent 留在既有失败/可重试状态，父 binding、父标题、父 JSONL 均不动，不能回退成 resume 父。父运行时允许从已完成一轮的历史 Fork，但复制边界以 Codex 原生快照为准，不能承诺包含父尚未完成的回复；要实测父子继续对话互不污染。同一会话被“仍然载入”到两个父卡时仍按 providerSessionId 区分会话源，所有子均必须取得独立 UUID，不重写原 binding。

## 6. 改动文件清单（后续实现，本关不改）

下列现有文件已按源码定位；标为新增的路径是后续计划。不为了统一命名重构无关文件。

| 文件 | 一句话改动 |
| --- | --- |
| packages/domain/src/agent-manifest.ts | 仅更新 Codex 能力、指令文件、HUD 支持项和准确事件来源，保留 respawn-only。 |
| apps/runtime/src/session/provider-hook-server.ts | 扩展已确认事件的 JSON/内联注入，移除副作用短路，接入 Codex 标题来源和 Fork 新 ID 校验，保留现有解析入口。 |
| apps/runtime/src/session/provider-notification-event.ts | 共用正常完成/审批解析，仅补实测支持的 Codex 错误/取消分流，复用现有通知结构。 |
| apps/runtime/src/session/provider-hud-payload-handler.ts | 保持唯一 HUD 入口及 run 校验，接入 Codex 增量快照刷新与实时优先合并。 |
| apps/runtime/src/session/provider-transcript-hud.ts | 在已有流式读取器内解析 session_meta/turn_context/token_count/工具与计划，保留 Claude 行解析原样。 |
| apps/runtime/src/session/session-hud-registry.ts | 消除 Codex 的 opusplan 默认，处理字符串模型/档位/上下文及工具 ID 去重、刷新和字段缺失。 |
| apps/runtime/src/session/provider-launch-plan.ts | Codex 增加已裁决权限组合；保留现有 resume/fork 子命令与单次注入。 |
| apps/runtime/src/session/codex-session-catalog.ts（新增） | 只读 rollout 元信息/分页预览/搜索和 session_index 标题，接口沿用已有 catalog 形态。 |
| apps/runtime/src/rpc/runtime-rpc-router.ts | 现有 list/detail/search/load 加 Codex 分派，复用指令编辑、会话占用和载入确认。 |
| apps/runtime/src/session-canvas/provider-mode-service.ts | Codex 载入时据真实历史决定 Fork 门禁，错误/取消不计正常回复，其他 provider 不变。 |
| apps/runtime/src/runtime-server.ts | Codex 接入登录/提示状态跟踪、可见 HUD 增量刷新、Fork 父 ID 注册和失败通知衔接。 |
| apps/runtime/src/session/terminal-work-status-tracker.ts | 只在 Codex 分支补有真实样本的登录/信任/终止错误提示，避免全局正则改变其他卡片。 |
| apps/runtime/src/session/provider-resume-monitor.ts（按样本需要） | 仅在 Codex 实测错误文案未被现有匹配覆盖时补匹配，绝不自动确认未知提示。 |
| apps/desktop/src/renderer/src/session-canvas/SessionLoaderDialog.tsx | 沿用原布局/状态/确认交互，加 Codex provider 和页签。 |
| apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx | loader/history 的 provider 透传增加 Codex，去除将 Codex 错当 Claude 的回退。 |
| apps/desktop/src/renderer/src/hierarchy/hierarchy-types.ts | 扩展现有载入/预览/搜索命令 provider 联合类型，无新增 RPC。 |
| apps/desktop/src/renderer/src/hierarchy/TerminalPane.tsx | 现有历史/自动标题入口按 Codex 能力开放，复用菜单和恢复横幅。 |
| apps/desktop/src/renderer/src/hud/TerminalHud.tsx | 仅补 Codex 推理档位展示与缺失字段/配额隐藏，不重排布局。 |
| tests/e2e/codex-acceptance.spec.ts（新增） | 真实打包 App 的 11 项与并发、失败、延迟验收，沿用隔离数据目录策略。 |
| docs/acceptance/codex-reference-matrix.md（新增） | 按 11 项记录 Claude 同场景基线、Codex 实际、证据、差异、未闭合项。 |

定点测试放在已有 `provider-hook-server.test.ts`、`provider-notification-event.test.ts`、`provider-transcript-hud.test.ts`、`session-hud-registry.test.ts`、`provider-launch-plan.test.ts`、`rpc/runtime-rpc-router.test.ts`、`session-canvas/provider-mode-service.test.ts`、`session-canvas/fork-workflow-service.test.ts`、`terminal-work-status-tracker.test.ts`，以及新增 `codex-session-catalog.test.ts`；前端在已有 `SessionLoaderDialog.test.tsx`、`TerminalPane.test.tsx`、`hud/TerminalHud.test.tsx` 增加必要覆盖。若恢复匹配需改才更新其已有测试。

明确复用而不计划改动：`apps/runtime/src/index.ts` 的身份/通知/HUD/标题回调、`notifications/agent-notification-repository.ts`、`domain/session-repository.ts` 的标题存储、`session-catalog-utils.ts`、`session-canvas/fork-operation-coordinator.ts`、现有 `SessionHudWire`。若实施时发现必须改这些边界，应先说明缺口，不能借此扩大为底座重写。`apps/runtime/src/agents/antigravity-plugin-installer.ts` 只作参考，**不改，也不为 Codex 新建全局插件安装器**。

现有 `hierarchy-commands` 的 JS 参数透传逻辑可复用（实际文件 `apps/desktop/src/renderer/src/hierarchy/hierarchy-commands.ts`）；编码时以类型检查定位剩余 provider 窄类型，若只是同一调用链补 Codex literal 不改其他 Agent 行为。指令文件编辑和 mt 资产均复用既有实现。

## 7. 后续关卡与验收

| 关卡 | 交付 | 通过条件 / 不合格条件 |
| --- | --- | --- |
| 第一关（本关） | 本方案、字段实测、差距与裁决 | 仅文档；11 项有落点；实测与未知分开；审过再继续。 |
| 第二关：钩子接管线 + 状态通知 | Codex 事件集、状态、完成/审批/失败通知、身份隔离 | 编码前补真实审批/取消/错误样本，确认实际事件支持；正常循环、聚焦冷却、断报和旧 run 均通过；没有错误证据不能报整关完成。 |
| 第三关：HUD / 标题 / 指令文件 | 模型/档位/用量/工具/真实计划、索引标题、AGENTS.md | 补 TUI 自动标题和计划样本；缺失隐藏、实时工具不闪回、历史不重计、手工标题锁；验证字段和真实页面一致。 |
| 第四关：Fork / 载入 / 恢复 | Codex 页签、完整搜索预览、UUID 恢复、两种目录 Fork、权限重启 | 参数实际生效；同 UUID 接回；子新 UUID；空/失败会话不解锁；父运行中 Fork 与重试/崩溃、错误身份都不污染父。 |
| 第五关：打包实测与矩阵 | 打包 App、Codex 对照矩阵、回归证据 | 11 项逐一闭合；真实 App 重启记忆恢复；Claude/Antigravity 无新失败；异常用受控 fixture 的证据须单独标注。 |

开发中只跑相关定点检查，稳定后统一 `pnpm typecheck`、`pnpm check:identifiers` 和必要包内测试。最终一次打包复用给 Codex 与回归验收，源码变化后再重打；不为每个文件重构建。

真实验收以原始 reference product 可运行 CLI 黑色工作区及现存逻辑为边界，同时以 Claude 卡片同场景作为本 PRD 直接基线，沿用 `docs/acceptance/antigravity-reference-matrix.md` 的证据分级。第 11 项还必须用自然语言真实调用 mt，不以 developer_instructions 参数存在代替通过。历史矩阵记载过 Claude Fork 的既有失败，只能作为准备信息，不能冒充本分支本轮回归结果；新失败必须排查。

性能记录输入反馈、running/idle 各三次事件到 UI 延迟（沿用一期 ≤1000ms）、工具开始/结束、切焦、滚动/菜单/焦点，以及多张真实终端并行时响应。大 rollout 以真实长文件验证增量读/分页，不做全文件反复解析；HUD IO 不能成为 hook 进入状态/通知的前置等待。UI 差异、跳动、误触、超时、未测试项均写入矩阵，不能用 typecheck 代替。

## 8. 风险、异常和需要拍板的点

| 情形 | 用户结果与处理 | 验收边界 |
| --- | --- | --- |
| Codex 未安装 | 复用启动失败提示，保留 Shell/重试入口，不生成假身份 | 用缺失二进制 fixture；本轮只确认当前机器已安装 |
| 未登录/首次信任 | 保留真实终端登录/信任交互，卡片 needs-input；不代登录、不自动批准 | 本轮使用已有登录态，未验证无登录 TUI；第二关采真实提示，fixture 仅补失败分支 |
| hook 端点不可达/超时/响应损坏 | 现有脚本 curl --max-time 2、丢弃响应、`|| true`；不影响 Codex 自身执行、不打印 URL/token | 沿用当前失败开放脚本；第二关实测断报及多次工具调用下的等待开销，不能仅证明 exit 0 而忽略每次 2 秒 |
| rollout 缺字段/半行/版本变化 | 跳过未知行、仅隐藏不可靠字段；旧 run 的缓存不能冒充新值，核心终端仍可用 | 字段缺失、截断、替换、null、模型变更、压缩后重建样本 |
| 索引缺失或标题晚到 | 卡片保留原名、列表可辨认；索引可用后再更新，不写全局索引 | exec 已观察到无索引；TUI 命名更新留到第三关实测 |
| 两卡并开同 UUID | 保持现有重复载入确认；不承诺外部并开的写锁保护 | Fork 必须新 UUID，错误父 ID 上报在落 binding 前拒绝 |
| 父忙时 Fork | 只继承 Codex 原生已落盘快照，不操纵父卡片或父 rollout | 实测父子继续对话、文件合法、绑定不变；不保证复制未完成消息 |
| 外部直接运行 codex | 没有 Matou 启动参数就没有 Matou 注入和上报；不修改其行为 | 对照无注入启动；注意用户自身已有 hooks/plugins 仍属于其 Codex 环境，Matou 不删改 |
| 新依赖/存储/核心抽象 | 本方案不需要；使用 Node 内建 fs/path、已有 JSONL 流读取、现有 binding/intent | 若实际必须引入，先带理由与更简单替代提问；不提前加表/通用框架 |

**当前没有尚未裁决的产品取舍**：隔离实测边界和计划模式语义已分别由回答 #1/#2 确认。本关提交后仍需用户审核放行后续关卡。

**仍有实测缺口，不能算已验收**：PermissionRequest 与失败/取消的真实载荷、可用失败事件名、计划工具字段、TUI 的标题生成/更新、四种权限策略与 resume 实际继承方式、真实 Fork/恢复、打包 UI、hook 断报性能。这些是后续对应关卡的前置检查；拿不到字段隐藏，拿不到错误/身份依据则该能力不闭合，不编造默认 0 或成功。

本方案自行作出的实施取舍：复用原 RPC 名称及返回类型；当前上下文取最近 token_count 而非累计值；工具按结构化执行 ID 去重并忽略外层 exec 重复包装；无索引标题不生成伪自动标题；Codex 缺证据的 HUD 计数不显示；acceptEdits/plan 显式 on-request，default 完全沿用用户配置。均未落入产品代码。
