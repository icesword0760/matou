# Codex 第三关真实交互探针

2026-11-15，`codex-cli 0.162.1`；真实 HOME 和默认 CODEX_HOME；临时 Git 仓库 `/private/tmp/matou-stage3-probe`。使用 `--no-alt-screen --no-daemon -C /private/tmp/matou-stage3-probe`、一次性 `-c 'projects."/private/tmp/matou-stage3-probe".trust_level="trusted"'` 和 `/tmp` 别名对应覆盖、`--dangerously-bypass-hook-trust` 以及一次性 `-c hooks=…`（仅七种已确认事件）。未指定模型、档位或审批策略。完整非敏感参数见 `/tmp/matou-stage3-probe/launch.sh`。在此次交互实际 `/status` 显示 GPT-6-Astra、reasoning medium；权限显示 `Custom (danger-full-access, Ask for approval)`。未启动其他代理。

先前单次信任参数未消除目录信任提示。用户裁决允许备份后仅对此目录接受一次信任，并在结束后核对与恢复。已执行，详见末节；原阻塞已处理。

会话 ID：`01a1ddd6-4064-70e1-a006-79d310600776`。实际 rollout：`/Users/icesword/.codex/sessions/2026/11/15/rollout-2026-11-15T14-12-48-01a1ddd6-4064-70e1-a006-79d310600776.jsonl`。原始终端录制 `/tmp/matou-stage3-probe/terminal2.log` 含账户等私有信息，不作为公开证据；下面只摘录与验收相关的终端原文，去掉 ANSI 控制符。最后 `/exit` 退出码 0。

## 自动标题

第一轮指示模型以三步读取临时样例、打印 2 并汇报；没有执行 `/rename` 或手工命名。三轮完成后分别查询真实 `~/.codex/session_index.jsonl`，均匹配到同一记录，没有观察到第二、第三轮更新时间变化：

```json
{"id":"01a1ddd6-4064-70e1-a006-79d310600776","thread_name":"Process sample.txt and summarize","updated_at":"2026-11-15T06:13:09.179593Z"}
```

TUI `/status` 的 `Thread name: Process sample.txt and summarize` 与索引一致。这证明本次会话自动生成并写入了标题；不证明所有会话或其他版本的生成时机。

## 计划工具

第一轮明确要求调用 `update_plan` 创建三项并随进度更新；模型以文本列出三项，最后称“当前会话没有可用的 update_plan 工具，因此以文字更新状态”。第二轮再次明确要求调用工具并将第一项标记完成；模型答复“update_plan 工具当前不可用”。真实 rollout 中 `payload.name == update_plan` 的调用数为 0，因此没有可交付的三项计划原始结构样本；文本列表不能当作工具载荷。两次同类尝试后停止这一部分。以上只说明此会话中模型报告不可用且未观察到调用，无法证明工具在全部版本或配置中不可用。

## 真实上下文对照

每轮完成后分别读取 rollout 的最近 `token_count.payload.info`，并在 TUI 执行 `/status`。下表 TUI 数字为 **left（剩余）**；估算列按 `100 × (1 − last_token_usage.total_tokens / model_context_window)` 计算。差值是 TUI 百分比减去此估算值，单位为百分点。

| 轮次 | 最近 total_tokens | model_context_window | 直接除法估算 left | TUI /status left | 差值 |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 | 34,069 | 258,400 | 86.815% | 91% | +4.185 |
| 2 | 34,130 | 258,400 | 86.792% | 91% | +4.208 |
| 3 | 34,175 | 258,400 | 86.774% | 91% | +4.226 |

与表对应的终端摘录：

```text
Model:               GPT-6-Astra (reasoning medium, summaries auto)
Context window:      91% left (34.1K used / 258K)  # 第 1 轮
Context window:      91% left (34.1K used / 258K)  # 第 2 轮
Context window:      91% left (34.2K used / 258K)  # 第 3 轮
```

每轮最近 `token_count` 的原始关键结构如下，其他累计计费字段省略：

```json
{"last_token_usage":{"input_tokens":33960,"cached_input_tokens":33664,"cache_write_input_tokens":0,"output_tokens":109,"reasoning_output_tokens":0,"total_tokens":34069},"model_context_window":258400}
{"last_token_usage":{"input_tokens":34116,"cached_input_tokens":33920,"cache_write_input_tokens":0,"output_tokens":14,"reasoning_output_tokens":0,"total_tokens":34130},"model_context_window":258400}
{"last_token_usage":{"input_tokens":34147,"cached_input_tokens":33920,"cache_write_input_tokens":0,"output_tokens":28,"reasoning_output_tokens":0,"total_tokens":34175},"model_context_window":258400}
```

三个样本中直接除法与 TUI 的剩余百分比不一致；TUI 百分比的内部计算规则未确认，不能把直接除法的结果描述为与 TUI 一致。模型与推理档位为 TUI 显示，不能由本次样本推断 rollout 始终提供档位。

## 信任和配置恢复

测试开始前将 `~/.codex/config.toml` 以保留权限方式备份到 `/tmp/matou-stage3-probe/config.toml.backup`，两者 SHA-256 均为 `f261c9af1f2e09e71276d97adb784f5df5ec2acfd9e06ae9ace8a4b1710b95af`、权限均为 `600`。用户批准后仅选择一次该临时目录的信任，未选择全局持续允许。退出后、恢复前 diff 只显示以下新增条目，没有其他同期差异：

```toml
[projects."/private/tmp/matou-stage3-probe"]
trust_level = "trusted"
```

从备份恢复后，`diff -u` 输出为空；备份与恢复文件的 SHA-256 都为 `f261c9af1f2e09e71276d97adb784f5df5ec2acfd9e06ae9ace8a4b1710b95af`，恢复权限为 `600`。`~/.codex/hooks.json` 测试前后 SHA-256 都为 `bf46565e936c07ce36c4f4beb5df09fbc11607ff8af3b1079458c517c1c8c0a1`。按授权保留正常 sessions/index 新增。

自主决定：跳过本次升级提示；用 `/status` 获取带文字标识的上下文数值；两次无计划工具调用后停止重试；保留差异而不猜测百分比算法。未完成项：真实 `update_plan` 三项载荷及状态更新、百分比内部算法和真实 Matou HUD 界面验收。
