# Matou 终端协作规则

当用户要求查看或操作另一个 Matou 会话时，使用当前终端内置的 `mt`。这是只在当前会话生效的宿主控制能力。

## 固定流程

1. 先运行 `mt identify --json` 确认当前窗口、工作空间、事项、画布、会话及 DAG 层级。
2. 再运行 `mt list --json` 建立当前 DAG level 的候选；只有用户给出明确名称或跨窗口条件时才用 `mt list --all --json`。
3. 按元信息解析唯一目标；信息不足时可用 `mt read <target> --lines 20` 短读少量候选。
4. 读取或输入前确认实际目标。输入完成后，用一句自然语言确认结果。

## 目标解析

- `left`、`right`、`sibling:N` 只表示调用方当前画布、当前 DAG level 的同级卡片顺序。
- `parent`、`child:N` 表示 DAG 父子关系，不与左右顺序混用。
- 稳定 `ref` 和明确 session ref 优先于名称推断。
- 明确名称才允许跨窗口搜索。名称命中多个目标时，列出最多 5 个候选并请用户选择；超过 5 个时，请用户补充工作空间、事项、画布、标题或路径等条件。
- 连续对话中的“它”“那个”复用上一轮已经确认的目标；“另一个”“左边”“右边”“换成”表示重新解析。

## 命令边界

首期只有：

- `mt identify`
- `mt list`
- `mt read TARGET`
- `mt history TARGET`
- `mt commands TARGET`
- `mt send TARGET TEXT [--enter]`
- `mt key TARGET KEY`

不要暗示或拼造创建、Fork、移出、关闭、聚焦、切换画布等命令。所有读取和输入都保持用户当前界面焦点、滚动位置与通知状态不变。

## 输入与确认

- `mt send` 前复述目标和将发送的文本；只有用户已明确表达目标与内容时才直接执行。
- 需要提交命令时显式加 `--enter`；仅填入文本时省略。
- 控制键仅使用：Enter、Tab、Escape、Backspace、Delete、方向键、Home、End、PageUp、PageDown、CtrlC、CtrlD、CtrlL、CtrlU、CtrlZ。
- 成功后说明已向哪个会话发送了什么；不要宣称界面已经切换或目标已经聚焦。

## 错误处理

- `TARGET_NOT_FOUND`：重新 `mt list`；若仍有多候选，展示候选并请用户补充条件。
- `CONFLICT`：列表顺序已变化，重新 `mt list --json` 后解析。
- `TARGET_NOT_READY`：目标进程当前未准备好，说明状态并建议稍后再试或选择其他会话。
- `PERMISSION_DENIED`：说明当前会话缺少对应权限。
- 连接或超时错误：重新 `mt identify`；若仍失败，说明当前 Matou 控制连接状态。
