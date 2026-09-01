---
name: mt-terminal
description: Use when the user asks Claude Code running inside Matou to identify its card, inspect another Matou terminal, read current or historical output, review executed commands, or send text and allowed keys to another managed session.
---

# MT Terminal

通过自然语言使用 Matou 当前会话内置的 `mt` CLI 协作终端。

## 开始前

先执行：

```bash
mt identify --json
```

成功后再按 `identify → list → resolve → act` 推进。不要从环境变量或界面截图猜测当前卡片。

## 高频流程

1. `mt identify --json`：确认调用方所处窗口、工作空间、事项、画布和 DAG level。
2. `mt list --json`：列当前 level；明确跨窗口名字时使用 `mt list --all --json`。
3. 先按标题、profile、cwd、层级和序号解析；必要时短读少量候选。
4. 多候选先确认；唯一目标再执行 `read/history/commands/send/key`。
5. 输入前确认目标和内容，成功后用一句自然语言反馈。

连续追问中的“它/那个”沿用上一轮确认的目标；“另一个/左边/右边/换成”重新解析。

## 渐进式参考

- 目标或序号复杂：读取 [references/target-resolution.md](references/target-resolution.md)
- 命令参数、键名或失败恢复：读取 [references/commands.md](references/commands.md)

## 能力边界

本 Skill 仅覆盖识别、列举、读当前屏幕、读历史、读 Shell 命令记录、发送文本和发送允许的单个控制键。它不包含创建、Fork、移出、关闭、聚焦或切换画布。
