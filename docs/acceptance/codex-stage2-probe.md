# Codex 第二关交互钩子探针（2026-11-15）

## 环境和边界

- 本机 `codex-cli 0.162.1`，真实交互 TUI，临时 Git 仓库 `/tmp/matou-stage2-probe`。不指定模型或推理强度，TUI 显示 GPT-6-Astra medium。`HOME` 保持原值；为避免信任目录时写入全局配置，复制配置到临时 `CODEX_HOME`，通过其 auth.json 符号链接沿用登录态；只在临时目录接受仓库信任。
- 同一次实际测试启动使用 `--no-alt-screen --no-daemon -C /tmp/matou-stage2-probe -a on-request -s read-only --dangerously-bypass-hook-trust -c 'hooks=…'`；临时启动脚本为 `/tmp/matou-stage2-probe/launch.sh`，注入 SessionStart、UserPromptSubmit、PreToolUse、PermissionRequest、PostToolUse、Stop、SessionEnd，hook 将原始 stdin JSON 逐行追加。另有一次性 `projects` 信任覆盖未消除首次目录信任提示，最终只在临时 CODEX_HOME 接受信任。前两次启动在信任提示选择退出，未提交探针提示。
- 这是显式审批配置下的实测，**不是 Matou 默认无覆盖启动的证明**。三个 turn 的载荷 `permission_mode` 实际均为 `default`，不能仅据此反推完整 sandbox/approval 配置。
- 原始采集文件中混有其他 session，因此按本探针 session ID 筛出的未修改原始行保存为 `/tmp/matou-stage2-probe/probe-session-hooks.jsonl`；不要将原采集文件中其他 session 的内容用于本报告。捕获脚本 `/tmp/matou-stage2-probe/capture.py`。路径及 transcript 路径在下方样例脱敏，其他字段保留原样。

## 结果

1. **审批并成功**：要求运行 `python3 -c 'print(2)'` 并申请 escalated 权限。界面确实显示审批提示；人工仅批准本次，命令输出 `2`，模型正常回复。顺序：SessionStart → UserPromptSubmit → PreToolUse → PermissionRequest → PostToolUse → Stop。
2. **命令失败**：要求运行一次 `matou_probe_missing_command`、不重试；终端报告退出码 127，模型如实回复。顺序：UserPromptSubmit → PreToolUse → PostToolUse → Stop。PostToolUse 的 `tool_response` 为 `zsh:1: command not found: matou_probe_missing_command\n`，没有独立退出码字段；Stop 仍是正常回复结束。这证明一次工具命令失败后模型仍可正常完成，不能因此断言整轮模型失败，也不能依赖 Stop 字段判别命令失败。
3. **取消**：请求解释彩虹形成，在 TUI 显示 `Working … esc to interrupt` 且生成未完成时实际发送 Esc；界面显示 `Conversation interrupted`，生成停止。此 turn 采到 UserPromptSubmit，直到后续 `/exit` 进程退出为止，**没有该 turn 的 Stop**。本次没有工具执行，不能推广到工具运行中取消的全部情况。
4. `/exit` 后 CLI 退出码 0；本次捕获中没有 SessionEnd，无法据此推论该事件在其他退出方式中是否触发。两次较早的启动均在信任提示退出，进程退出码 0，无有效业务回合。

### PermissionRequest 原始载荷（路径脱敏）

```json
{"session_id":"01a1ddc1-88fd-7fc1-8c66-ab8d38719da8","turn_id":"01a1ddc1-e364-7770-96b1-4b249153a4dd","transcript_path":"<transcript>","cwd":"<probe>","hook_event_name":"PermissionRequest","model":"gpt-6-astra","permission_mode":"default","tool_name":"Bash","tool_input":{"command":"python3 -c 'print(2)'","description":"Do you approve running exactly python3 -c 'print(2)' with escalated permissions for this approval test?"}}
```

此 PermissionRequest 无 `tool_use_id`；不应假设该字段必有。

### 成功后的完整 Stop（路径脱敏）

```json
{"session_id":"01a1ddc1-88fd-7fc1-8c66-ab8d38719da8","turn_id":"01a1ddc1-e364-7770-96b1-4b249153a4dd","transcript_path":"<transcript>","cwd":"<probe>","hook_event_name":"Stop","model":"gpt-6-astra","permission_mode":"default","stop_hook_active":false,"last_assistant_message":"The command ran with escalated permissions and exited successfully (code 0).\n\nOutput:\n```text\n2\n```"}
```

### 失败命令后的完整 Stop（路径脱敏）

```json
{"session_id":"01a1ddc1-88fd-7fc1-8c66-ab8d38719da8","turn_id":"01a1ddc2-8f37-7210-898a-25f61e127666","transcript_path":"<transcript>","cwd":"<probe>","hook_event_name":"Stop","model":"gpt-6-astra","permission_mode":"default","stop_hook_active":false,"last_assistant_message":"Ran once. Exit code: `127`.\n\n```text\nzsh:1: command not found: matou_probe_missing_command\n```"}
```

两种 Stop 字段集合相同，仅 turn ID 和自然语言内容不同；该样本没有结构化 error/cancel 字段。真正的整轮模型失败未测试，不推断其事件或字段。

## 配置核对及自主决定

测试前后 `~/.codex/config.toml` SHA-256 均为 `9a9a311e24f379c1e0d0bed5c562bd3a31355eecc6820512df2ca6d4fad66bb6`；`~/.codex/hooks.json` 均为 `bf46565e936c07ce36c4f4beb5df09fbc11607ff8af3b1079458c517c1c8c0a1`。未修改这两个全局配置。

自主决定：显式使用 on-request/read-only 触发审批；临时 CODEX_HOME 隔离信任写入；取消选择生成中按 Esc，避免增加额外命令；过滤按 session ID 混入的无关事件；不通过自然语言猜测结构化错误。未验证真实 Matou UI、工具执行中取消、模型级失败和其他审批策略；以上结果限定于当前版本、参数与单次场景。

## 混入会话的归属补查

只读复核发现另一个 session `01a1ddc1-e832-7cf2-83b7-5adb2f12224b` 的 SessionStart 为 `source: startup`、`cwd: /private/tmp/matou-stage2-probe/codex-home/memories`、`transcript_path: null`、`model: gpt-5.6-terra`、`permission_mode: bypassPermissions`，随后有独立 turn 和 SessionEnd；它不同于交互探针的主 session、工作目录和模型。未找到该 ID 的 session_meta 文件，现有事件也没有 originator 或父 session 字段。其工作目录提示它可能与临时 CODEX_HOME 的记忆处理有关，但**具体创建者及用途未证实**，不能断言是审批审查器或内部子代理。操作者没有手动启动此 session，也未指示被测 Codex 启动子代理；现有元数据不足以证明被测 Codex 是否自行启动。此结果说明接收同一 hook 配置时可能出现其他 session 的事件，不能仅凭接收到事件就认定它属于卡片主会话。

## 第二关实施结果与范围（主代理整合）

实现提交：`c411c4c`（钩子/状态/通知入口与身份隔离）、`0a77d19`（实测中断提示、恢复和通知回归）；先行方案文档提交为 `ea0cb92`。没有改账户、供应商面板、版本、依赖或持久化结构，没有推送。

| 文件 | 本关改动 |
| --- | --- |
| apps/runtime/src/session/provider-hook-server.ts | 七类已确认事件共用 JSON/内联配置表，移除 Codex 副作用短路，复用身份/HUD/通知回调，发送 Codex 状态；过滤显式空 transcript 的实测噪声、固定本 run 已确认的主 ID。 |
| apps/runtime/src/session/session-hud-registry.ts | 仅补 Codex SessionStart 的空闲状态，以及审批后 PostToolUse 恢复工作中；没有实现 HUD 字段解析。 |
| apps/runtime/src/runtime-server.ts | 让 Codex 使用既有终端工作状态跟踪器，其他 provider 分支不变。 |
| apps/runtime/src/session/terminal-work-status-tracker.ts | Codex 只识别实测的英文 Conversation interrupted，按既有 interrupted 状态上报一次并清空已消费文本，不推断错误/审批/未实测中文。 |
| apps/runtime/src/session/provider-hook-server.test.ts | 配置、状态、HUD 管线入口、通知、旧 run、retirement、lease、身份不符、噪声会话回归；只按监督回答 #3 更新一条旧 Codex 配置全等断言。 |
| apps/runtime/src/session/provider-notification-event.test.ts | 用实测 Codex 审批、成功回复和命令失败后的正常回复，验证既有转换器；不增加无证据错误判定。 |
| apps/runtime/src/session/terminal-work-status-tracker.test.ts | 分片中断文本、只消费一次、新尝试清理，以及不从未知输出推断状态。 |
| apps/runtime/src/runtime-server.test.ts | 受控真实 PTY 验证 Codex 卡片中断后，下一次提交回到工作中。 |
| apps/desktop/src/renderer/src/notifications/agent-event-ingestion.test.ts | 新增 Codex 事件的冷却、声音、聚焦免打扰测试；没有修改前端产品代码。 |
| docs/acceptance/codex-stage2-probe.md | 交互探针、已确认裁决、实现、校验与剩余缺口。 |

状态映射：SessionStart → idle；UserPromptSubmit / PreToolUse → running；PermissionRequest → needs-input；PostToolUse → running（审批已完成）；普通 Stop → idle 并发送回复完成通知；SessionEnd → exited；观察到的英文 Esc 中断提示 → interrupted。既有 `terminal.input` 新提交和新 PreToolUse 会恢复 running。中断文本跟踪不发布通知，因此没有伪完成或错误提醒。

本关完成/审批通知仍由 `toProviderNotificationEvent`、`AgentNotificationRepository` 和现有前端通知 store 处理，标题 Codex，沿用声音、冷却、聚焦免打扰。不存在的命令退出 127 后，模型正常给出失败说明，这条 **Stop 通知表示回复已结束，正文保留失败说明，不代表命令成功**。没有整轮模型错误样本，所以没有新增 Codex error 分类/通知；也没有注册 Notification、PostToolUseFailure、PreCompact、StopFailure 或 Interrupt。

身份护栏在写 binding 前生效：显式 `transcript_path:null` 且无其他有效 transcript 路径的 Codex 事件不参与身份和任何副作用；本 run 一旦确认主 providerSessionId，其他 ID（或缺失 ID）的后续事件不更新卡片。Fork 初次尚未确认身份时保留现有认领入口，没有提前实现 Fork 功能；不增加 cwd 严格校验。旧 run、退休 suppression、恢复 ID 不符、过期 lease 的保护原样保留，并有 Codex 定点覆盖。

上述范围扩充分别经监督确认：回答 #3 允许唯一旧 Codex 配置断言更新；#4 接受明确英文中断提示映射；#5 接受空 transcript 过滤并要求 run 身份固定。Claude / Antigravity 原断言未改。其清单、启动参数、插件安装器、通知转换器、标题/指令文件和 rollout 读取实现均未改。

### 自动校验

以下共覆盖 **292 个不同用例、12 个文件**；单项定位时的重复执行不重复计数。

| 检查 | 结果 |
| --- | --- |
| provider-hook-server.test.ts + session-hud-registry.test.ts | 63/63 通过。 |
| terminal-work-status-tracker.test.ts + provider-notification-event.test.ts | 22/22 通过。 |
| 前端 agent-event-ingestion.test.ts + AgentNotificationStore.test.ts | 29/29 通过。 |
| runtime-server.test.ts 全文件 | 151/151 通过，包括新增受控 PTY 中断恢复测试；未更改已有断言。 |
| antigravity-runtime.test.ts、antigravity-plugin-installer.test.ts、provider-launch-plan.test.ts、provider-work-status.test.ts、agent-notification-repository.test.ts | 初次 26 通过、1 项因缺少 dist/control-assets 在进入断言前失败；执行现有 pnpm build:runtime 后仅重跑这 1 项通过（同文件其余 6 项此前已通过，本次过滤跳过）。合计 27 个用例均已通过，不将过滤跳过算作新执行。 |
| pnpm typecheck | exit 0，全 workspace 通过。 |
| pnpm check:identifiers | 每次提交前通过，最终文档提交前再执行。 |

新工作区最初缺少 packages 的 dist，第一次定点测试未能装载模块（0 个用例），按既有 `pnpm build:packages` 补齐后执行；Antigravity 跨卡测试需要运行时构建资源，按现有 `pnpm build:runtime` 补齐，没有改断言或新增依赖。新 lease 测试夹具最初误设源卡等于目标卡触发既有数据库约束，修正为独立源卡后通过；未修改产品约束。

完整长命令输出保存在监督目录：

- `/Users/icesword/.claude/agent-foreman/runs/matou-多Agent-codex-01/logs/stage2-typecheck.log`
- `/Users/icesword/.claude/agent-foreman/runs/matou-多Agent-codex-01/logs/stage2-runtime-regression.log`
- `/Users/icesword/.claude/agent-foreman/runs/matou-多Agent-codex-01/logs/stage2-agent-regression.log`
- `/Users/icesword/.claude/agent-foreman/runs/matou-多Agent-codex-01/logs/stage2-runtime-build.log`
- `/Users/icesword/.claude/agent-foreman/runs/matou-多Agent-codex-01/logs/stage2-antigravity-mt-after-build.log`

### 尚未验证与本关停止点

- **真实 CLI 已验证**：交互审批、单次批准后执行、失败命令的普通 Stop、生成中 Esc 无 Stop、全局两配置文件未改。交互审批只在显式 on-request/read-only 和临时 CODEX_HOME 下验证，不能声称 Matou 无参数 default 必然出现审批。
- **受控测试已验证**：Matou 既有钩子管线与状态/通知规则、中断恢复、运行隔离；不是完整打包 App 的真实 Codex 验收。
- **未验证**：模型级失败的结构化载荷与 error 通知、工具执行中取消、中文中断提示、真实 Matou 页面与打包 App、hook 断报的真实 Codex 性能。本次交互 `/exit` 未收到 SessionEnd；第一关 exec 曾收到，该事件保留注册，PTY 退出仍走既有生命周期。
- 未实现后续关卡：HUD 字段与空值/额度处理、标题索引、指令文件入口调整、权限模式参数补齐、载入、Fork、重启恢复能力开关。清单、rollout 读取器和标题逻辑保持原样，不能据本关测试宣称这些能力完成。
- 原有未跟踪 PRD 保留，不纳入本次提交。第二关交付后停下，后续关卡等待放行。

主代理自行取舍：PostToolUse 解除审批等待并恢复 running；JSON 与内联配置复用一个七事件表防止漂移；不以助手正文、工具退出 127、通用 Error 文本推断整轮失败；中断文本消费后清空缓存，避免同一提示随输出反复触发。Terra 探针取舍见前文「配置核对及自主决定」，由主代理逐项复核后整合。没有新增错误字段假设或全局配置修改。
