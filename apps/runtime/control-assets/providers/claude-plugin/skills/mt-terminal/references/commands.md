# MT 命令参考

所有写操作和导航都使用 `--json`；从 JSON 结果读取下一步所需的稳定引用，但面向用户只汇报标题、路径、环境、状态和影响。

```text
mt identify [--json]
mt list [--all] [--json]
mt read TARGET [--lines N] [--bytes N] [--json]
mt history TARGET [--lines N] [--bytes N] [--json]
mt commands TARGET [--limit N] [--json]
mt send TARGET TEXT [--enter] [--replace | --append] [--json]
mt key TARGET KEY [--json]

mt create workspace --path PATH [--title TITLE] [--submission-key KEY] [--enter] [--json]
mt create task --workspace TARGET [--title TITLE] [--submission-key KEY] [--enter] [--json]
mt create canvas --task TARGET [--title TITLE] [--submission-key KEY] [--enter] [--json]
mt create session --canvas TARGET [--profile shell|claude-code|codex] [--title TITLE] [--submission-key KEY] [--enter] [--json]

mt fork child SOURCE --title TITLE --environment-json JSON [--prompt TEXT] [--start] [--submission-key KEY] [--json]
mt fork sibling SOURCE --title TITLE --environment-json JSON [--prompt TEXT] [--start] [--submission-key KEY] [--json]
mt fork children SOURCE --items-json JSON|- [--batch-key KEY] [--retry-item-key KEY] [--json]

mt remove preview TARGET --scope node|subtree [--json]
mt remove commit CONFIRMATION_REF [--json]
mt close canvas-preview TARGET [--json]
mt close canvas-commit CONFIRMATION_REF [--json]

mt focus TARGET [--json]
mt switch workspace TARGET [--json]
mt switch task TARGET [--json]
mt switch canvas TARGET [--json]
```

目标支持 `current`（命令支持时）、`self`、`left`、`right`、`parent`、`child:N`、`sibling:N`，以及 `mt list --json` 返回的稳定目标引用。

分支与 Worktree 环境选择前统一运行 `mt list --all --json`，按 `executionContextRef`、`worktreeRef` 去重。父节点当前环境已承载目标分支时使用 `current`，否则使用去重后的 `existing-worktree`；多个真实候选才询问。

## 三方案批量示例

先向用户总结“轻量适配方案”“服务层重构方案”“完整架构升级”三个节点标题，再集中确认仍缺失的环境。确认后提交：

仅在至少一个方案缺环境时集中询问全部缺失项；用户已逐项给出环境或说“全部 main”时直接解析并提交。

```bash
cat <<'JSON' | mt fork children self --items-json - --batch-key three-options-v1 --json
[
  {"itemKey":"light","title":"轻量适配方案","environment":{"mode":"current"}},
  {"itemKey":"service","title":"服务层重构方案","environment":{"mode":"new-worktree","branch":"feature/service-refactor"}},
  {"itemKey":"architecture","title":"完整架构升级","environment":{"mode":"existing-worktree","branch":"main","worktreeRef":"worktree:main"}}
]
JSON
```

“创建”时条目不带 `start`，节点保持待命。“创建并执行”时每个条目带自己的 `prompt` 和 `"start":true`。

稳定 key 由对话意图生成并在同一逻辑操作中保持不变。部分成功时只重试失败项，仍提交原始条目定义，并复用原 batch key、原 item key：

```bash
cat <<'JSON' | mt fork children self --items-json - --batch-key three-options-v1 --retry-item-key service --json
[
  {"itemKey":"light","title":"轻量适配方案","environment":{"mode":"current"}},
  {"itemKey":"service","title":"服务层重构方案","environment":{"mode":"new-worktree","branch":"feature/service-refactor"}},
  {"itemKey":"architecture","title":"完整架构升级","environment":{"mode":"existing-worktree","branch":"main","worktreeRef":"worktree:main"}}
]
JSON
```

成功项保持原状态，不为重试生成新节点。多个失败项可重复使用 `--retry-item-key`，或传 `--retry-item-keys-json`。

## 创建、Fork 与导航

- 用户没有要求进入新对象时，不传 `--enter`，创建后保持当前焦点。
- 单节点“创建并执行”使用 `--prompt` 与 `--start`；只创建则不传 `--start`。
- `mt focus` 聚焦会话；`mt switch workspace|task|canvas` 切换对应层级。唯一目标直接执行，多候选先展示 CLI 提供的人类可读详情。

## 移除与关闭画布

先生成预览并向用户展示影响：

```bash
mt remove preview self --scope node --json
mt close canvas-preview current --json
```

叶子节点用 `node`。存在子节点时，先让用户选择 `node` 或 `subtree`。用户看到影响并明确确认后，从内部预览 JSON 读取确认引用并立即提交：

```bash
mt remove commit CONFIRMATION_REF --json
mt close canvas-commit CONFIRMATION_REF --json
```

明确确认后才执行 commit；确认过期或结构变化时重新预览，不复用旧引用。项目文件、Git 分支和 Worktree 保持不变。

不向用户展示确认引用、内部引用或控制凭据；这些值只在当前 `--json` 流程中使用。

## 读取与输入

- “现在屏幕上是什么” → `mt read`
- “之前输出/更早日志” → `mt history`
- “执行过哪些 Shell 命令” → `mt commands`
- 只填入文本 → `mt send TARGET "text"`
- 填入并提交 → `mt send TARGET "command" --enter`
- 强制清掉已有草稿再提交 → `mt send TARGET "text" --enter --replace`
- 保留已有草稿并在后面追加提交 → `mt send TARGET "text" --enter --append`
- 给另一个 AI 会话传话、布置任务或请它做完后回报 → 一律 `--enter`
- 单键 → `mt key TARGET Enter`

### 输入框里的未发送内容

`mt read --json` 在存在未提交输入时返回：

```json
"pendingInput": {"text": "已写入的预览", "origin": "user", "reliable": true}
```

- 没有 `pendingInput`：输入框里的字是模型建议（例如 Claude Code 的灰色占位建议），不是用户或 agent 写的。`mt send --enter` 会直接键入并提交，建议会被替换，不会拼接；不需要清空，也不需要向用户确认。
- `origin = "user"`：用户真打的草稿。`--enter`（默认 `auto`）返回 `PENDING_USER_INPUT`，零字节写入；只有 `--replace` 会清掉重发，`--append` 会保留并追加。
- `origin = "agent"`：别的 agent 预填、尚未提交的内容。`auto` 会按完整字符数退格清除，再键入并提交新内容。
- `reliable = false`：输入经过方向键、Tab、无效 UTF-8 等无法可靠重建的编辑。`auto` 和 `--replace` 都返回 `PENDING_INPUT_UNRELIABLE`，不会冒险清除；请先手动清空，或用 `--append` 明确保留。
- 发送正文和回车之间如果目标输入发生变化，返回 `PENDING_INPUT_CHANGED`，自动发送停止。
- `truncated: true` 只表示预览被截断（超过 4 KB），清除仍按完整字符数执行。

判断输入归属只看 `pendingInput`，不要根据截屏文字猜测。`mt key Enter` 不参与这套检查，它就是按回车。

允许键：Enter、Tab、Escape、Backspace、Delete、ArrowUp、ArrowDown、ArrowLeft、ArrowRight、Home、End、PageUp、PageDown、CtrlC、CtrlD、CtrlL、CtrlU、CtrlZ。

给另一个 AI 会话传话或布置任务（包括“做完后通知我”）时一律加 `--enter`，否则消息只停在对方输入框；需要对方回报时，先用 `mt identify --json` 取得自己的标题，在消息里写明回报命令 `mt send "<我的标题>" "<回报内容>" --enter`。

发送前确认目标与内容。操作只写入目标 PTY，不切换焦点、不滚动画布、不激活窗口、不改变通知状态。

## 错误后的下一步

- 目标缺失或 revision 冲突：重新 list 并解析。
- 多个目标候选：按 CLI details 展示候选，让用户选择。
- 目标未准备：说明当前状态，稍后重试。
- 确认过期或影响变化：重新生成预览并再次展示影响。
- 分支或 Worktree 冲突：集中询问受影响节点的新环境选择。
- 批量部分成功：保留成功项，只重试失败项并复用原 key。
- 导航超时：汇报目标标题和路径，说明本次导航未完成且可重试。
- `PENDING_USER_INPUT`：目标输入框里有用户草稿；不要重复重试。确认要覆盖时用 `--replace`，要保留时用 `--append`，否则等用户自己处理。
- `PENDING_INPUT_UNRELIABLE`：输入无法可靠重建；请用户手动清空，或用 `--append` 明确追加。
- `PENDING_INPUT_CHANGED`：等待回车期间输入发生变化；先用 `mt read --json` 查看当前 `pendingInput`，再决定。
- 权限问题：说明当前会话没有该动作的权限。
- 连接超时：重新 identify 检查当前会话的控制连接。
