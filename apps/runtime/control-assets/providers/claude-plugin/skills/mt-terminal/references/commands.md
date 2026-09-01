# MT 命令参考

```text
mt identify [--json]
mt list [--all] [--json]
mt read TARGET [--lines N] [--bytes N] [--json]
mt history TARGET [--lines N] [--bytes N] [--json]
mt commands TARGET [--limit N] [--json]
mt send TARGET TEXT [--enter] [--json]
mt key TARGET KEY [--json]
```

目标支持 `self`、`left`、`right`、`parent`、`child:N`、`sibling:N`、稳定 ref、明确 session ref。

## 读取选择

- “现在屏幕上是什么” → `mt read`
- “之前输出/更早日志” → `mt history`
- “执行过哪些 Shell 命令” → `mt commands`

`history` 是 Runtime Journal 中仍可见的历史；`commands` 来自 Shell Integration 的命令边界，不把启动脚本当作用户命令。

## 输入选择

- 只填入文本：`mt send TARGET "text"`
- 填入并提交：`mt send TARGET "command" --enter`
- 单键：`mt key TARGET Enter`

允许键：Enter、Tab、Escape、Backspace、Delete、ArrowUp、ArrowDown、ArrowLeft、ArrowRight、Home、End、PageUp、PageDown、CtrlC、CtrlD、CtrlL、CtrlU、CtrlZ。

发送前确认目标与内容。操作只写入目标 PTY，不切换焦点、不滚动画布、不激活窗口、不改变通知状态。

## 错误后的下一步

- 目标缺失：重新 list，并补充条件。
- revision 冲突：重新 list 后使用新 ref/序号。
- 目标未准备：稍后重试或选择其他会话。
- 权限问题：说明当前会话没有该动作的权限。
- 连接超时：重新 identify 检查当前会话的控制连接。
