# 2026-11-25 待办：会话状态误报与派活任务被外部拉起

两起问题都发生在同一次收口里：另一个会话看到本会话是 `needs-input`，于是改用 `mt-foreman send` 催办；催办没有到达监工会话，本会话也没意识到任务被重新拉起。

## 一、`needs-input` 卡住不回收

### 现象

2026-11-25 20:09–20:27 之间，本会话（`d9b37d11`、run `12272d88`）一直在跑工具和回复，但另一个会话读到的 `workStatus` 是 `needs-input`。

### 原因（已用数据库核实）

- `apps/native/runtime/src/provider_hook.rs:290` `NotificationKind::work_status`：`completed` → `idle`，`permission` / `waiting` / `attention` → `needs-input`，`error` 保持。
- 状态只在这几处被改写：通知到达、终端输入提交（`owner.rs:6552` `InputSubmitted` → `running`）、进程退出/出错。
- **持续工作本身不会把 `needs-input` 改回 `running`**：assistant 消息、工具调用、transcript 增长都只喂 HUD 和上下文保护（`owner.rs:5447` `observe_claude_transcript`），不碰 `work_status`。
- 本会话在 20:07:48 收到 `permission`、20:09:57 收到 `waiting`（AskUserQuestion 等待回答），状态变成 `needs-input`；用户用鼠标点选项回答，没有产生终端提交事件；之后继续跑工具，状态一直停在 `needs-input`，直到 20:27 的 `completed` 通知才变成 `idle`。
- 数据库证据：`~/.matou/matou.sqlite` `domain_events` 中该会话的 `agent.notification` 序列（seq 12986229 permission、12986281 waiting、12986566/12986598 completed）。

### 影响

- 卡片状态灯、画布状态计数、别的 agent / 会话读取到的状态全部失真。
- 别的会话据此判断「它空了、在等人」，做出错误协调动作（本次就是改用 foreman 通道催办的直接原因）。

### 修复方向（建议，未实施）

1. 任何 provider 活动事件（assistant 消息 / 工具开始结束 / `UserPromptSubmit` / `PostToolUse`）到达时，若当前是 `needs-input` 且不是待确认的权限弹窗，则改回 `running`。
2. 权限确认、问题回答这类非终端提交的交互完成后，显式写入 `running`（AskUserQuestion 的回答链路要单独接线）。
3. 保留 `needs-input` 的真实语义：没有新活动时不清除。

## 二、`mt-foreman send` 静默拉起任务，监工收不到通知

### 现象

另一个会话执行 `mt-foreman send matou-history-seam-01 <<'EOF' … EOF` 催办；执行器收到并开始了新一轮，监工（本会话）没有任何推送，直到用户截图才知道。

### 原因

- `send` 走的是执行器 inbox（`foreman.mjs` 的 `send`/`resume` 分支），不是会话间消息；监工只有在 `watch` 事件流时才会看到。
- `start` 没有记录发起者会话；`send` 也不区分调用者是谁，因此无法在「别人拉起了你的任务」时通知所有者。
- 结论：不是消息丢失的 bug，是通道语义与通知缺失。`mt send <会话> "…" --enter` 才是给会话发话。

### 修复方向（建议，未实施）

1. `start` 时用 `mt identify --json` 记录发起会话（标题与 ref）到任务状态文件。
2. `send` 时再取一次当前调用者；与记录的所有者不一致时，启动后向所有者会话发一条 `mt send … --enter`：「任务 X 被 <调用者> 重新拉起一轮」。
3. 同一所有者的 `send` 不通知，避免自我噪音；同时在 feed 里打一行 `✉ 外部 send by …`，`status` 里显示最近一次外部调用者。

## 三、暂不处理

- `observed started` 长期停在 `starting` 的会话（本次 `mt list` 里 13 个）：现象独立，未定位，另开。
- 用户消息按鼠标回答后没有终端提交事件：与问题一第 2 条同源，一并修。
