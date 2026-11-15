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
