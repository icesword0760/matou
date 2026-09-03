---
name: mt-terminal
description: Use when the user asks Claude Code running inside Matou to inspect or interact with managed terminals, or to create, fork, remove, close, focus, or switch Matou workspaces, tasks, canvases, and sessions.
---

# MT Terminal

通过自然语言使用 Matou 当前会话内置的 `mt` CLI 管理和协作终端。

## 固定决策顺序

每次结构或导航操作都按同一顺序推进：

1. 运行 `mt identify --json`，确认当前窗口、工作空间、事项、画布、会话和 DAG level。
2. 运行 `mt list --json`，按标题、路径、profile、cwd、层级和序号解析目标；用户明确给出跨窗口名称或路径时才运行 `mt list --all --json`。
3. 批量 Fork 时先从最近对话提取方案并向用户总结所有节点标题。
4. 若分支或 Worktree 仍缺用户决策，只集中询问一次；其他参数完整且目标唯一时直接执行，不重复确认。
5. 写操作使用稳定的 submission key；批量操作使用稳定的 batch key 和 item key，并带 `--json` 执行。
6. 仅以人类可读的标题、路径、环境和结果汇报。内部引用、协议字段、控制凭据和确认引用只在结构化结果中流转，不展示给用户。

连续追问中的“它/那个”只沿用上一轮唯一且已确认的目标；“另一个/左边/右边/换成”重新列举和解析。

## 创建与批量 Fork

- 目标唯一、参数完整的工作空间、事项、画布、会话创建直接执行。默认保持当前焦点；只有用户明确要求进入时才传 `--enter`。
- “创建三个子节点”只完成 Fork、标题与环境设置，节点保持待命。
- “创建并分别实现三个方案”还要为每项传入对应方案说明并开始执行；不得把一个方案的任务发送给其他节点。
- 三方案场景先总结三个简短、可区分的节点标题，再统一询问每个节点使用 `current`、`existing-worktree` 或 `new-worktree`。
- 用户说“都在 main”时，先解析承载 `main` 的现有执行环境并让各节点共享；候选不唯一时只就这些候选询问一次。
- 使用 `mt fork children ... --batch-key ... --json` 提交一个批次。部分成功时保留成功节点，只重试失败项，并复用原 batch key、原 item key 和原始条目定义。

## 移除与关闭画布

- 叶子会话使用“移除节点”，先运行 `mt remove preview TARGET --scope node --json`。
- 目标有子节点时，先询问“仅移除当前节点”还是“移除当前节点及全部子节点”，再以 `node` 或 `subtree` 生成预览。
- 关闭画布先运行 `mt close canvas-preview TARGET --json`。
- 将预览中的标题、完整路径、影响数量、运行中或等待中的会话、终端进程及项目文件、分支和 Worktree 保留情况展示给用户。用户明确确认后，才使用预览 JSON 中的确认引用执行对应 commit。
- 用户确认前不提交；预览过期或影响变化时重新预览。面向用户统一说“移除”和“关闭画布”。

## 导航与终端交互

- 聚焦或切换目标唯一时直接执行；多个候选时按 CLI 返回的标题、工作空间、事项、画布、profile、cwd 和序号展示，让用户选择。
- 输入前确认目标和内容，除非用户已经明确给出两者。成功后用一句自然语言说明最终标题或路径。

## 渐进式参考

- 目标、环境或序号复杂：读取 [references/target-resolution.md](references/target-resolution.md)
- 完整命令、批量示例、确认提交或失败恢复：读取 [references/commands.md](references/commands.md)

## 能力边界

`mt` 可识别、列举、读取、查看历史和 Shell 命令记录、发送文本或单键，也可创建、Fork、移除、关闭画布、聚焦和切换。它只操作 Matou 托管对象；未明确要求导航时，结构写入不改变用户当前焦点。
