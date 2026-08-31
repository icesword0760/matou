# Matou Git 与 Worktree 控制设计

**日期：** 2026-08-31  
**状态：** 已批准执行  
**对标范围：** Codex 当前本地版本的分支、提交、推送交互；Worktree 采用 Matou 的持续保留原则。

## 1. 用户目标

用户点击底栏当前分支后，在不离开会话的情况下完成：

1. 查看当前仓库、分支、修改数量和增删行数。
2. 搜索、切换或创建本地分支。
3. 提交暂存区，或连同未暂存与未跟踪文件一起提交。
4. 提交并推送，或推送已有本地提交。
5. 查看本地 Git Worktree、进入对应工作目录、在访达中打开、新建或移除 Matou 管理的 Worktree。

底栏只显示当前分支和脏状态；详细信息按需加载，避免持续执行 Git 命令影响终端与卡片交互性能。

## 2. Codex 对标行为

### 2.1 分支

- 当前分支置顶并显示勾选，默认分支优先展示，其余分支按最近提交时间排列。
- 搜索只过滤本地分支，不修改仓库。
- 切换首先交给 Git 判断；本地修改可安全携带时直接完成切换。
- Git 报告修改会被覆盖时，展示冲突文件并提供“提交并切换”。提交成功后自动重试原切换目标。
- 创建分支以当前 HEAD 为基线；已存在或已在其他 Worktree 检出的分支展示真实 Git 错误。

### 2.2 提交与推送

- 提交信息由用户填写；本次不接入 AI 生成。
- “包含未暂存的更改”默认开启，并在本次应用生命周期内记忆。
- 关闭该选项时只提交暂存区；开启时先执行 `git add -A`。
- 有可提交修改时开放“提交”；存在远端能力时开放“提交并推送”。
- 当前分支领先 upstream 时开放“推送”；没有 upstream 时首次推送建立跟踪关系。
- 一个面板同一时间只执行一个 Git 操作；成功或失败只出现一条反馈。

### 2.3 Worktree

- 展示 `git worktree list --porcelain` 返回的全部本地 Worktree。
- 每项显示分支、路径、修改状态、关联 Matou 会话数量和当前标记。
- “进入”优先聚焦当前画布内已使用该 Worktree 的会话；不存在时在当前画布创建一个绑定该目录的 Shell。
- “访达”使用桌面进程打开路径。
- 新建 Worktree 存放于 Matou 数据目录 `worktrees/<workspaceId>/<worktreeId>`，并写入现有 Worktree/ExecutionContext 数据模型。
- 只有 Matou 管理目录内的 Worktree 展示“移除”。当前 Worktree、仍有关联会话或包含本地修改时保留目录并给出原因。
- 移出会话节点不连带删除 Worktree；不引入自动淘汰、自动快照或后台清理。

## 3. 状态模型

运行时返回 `GitRepositoryStatus`：

- 仓库：`repositoryRoot`、`cwd`、`currentBranch`、`defaultBranch`、`upstream`。
- 文件：`stagedCount`、`unstagedCount`、`untrackedCount`、`additions`、`deletions`、`dirty`。
- 同步：`ahead`、`behind`、`canPush`、`hasRemote`。
- 分支：名称、当前标记、提交时间、被其他 Worktree 棠出的路径。
- Worktree：路径、分支、HEAD、当前/主目录标记、脏状态、Matou 管理标记、关联会话数量。

`git.checkout` 返回 `switched` 或 `blocked-by-working-tree-changes`。后者携带目标分支与冲突路径，不用模糊错误替代产品状态。

## 4. 架构

### Runtime

- 新增 `GitWorkspaceService`，所有 Git 子进程使用参数数组调用 `git -C <cwd>`，不经过 Shell 拼接。
- 服务负责状态解析、分支、提交、推送以及 Worktree 发现；数据库只用于合并会话数量和 Matou 管理状态。
- 复用 `WorktreeService` 创建、登记和安全移除 Worktree。
- `SessionCanvasService.createShellSibling` 接受可选的 ExecutionContext/CWD 覆盖，用于在当前画布进入另一 Worktree。
- Git 变更完成后刷新已连接会话 HUD，底栏分支状态立即收敛。

### Renderer

- `TerminalHud` 的 Git 字段变成按钮，点击加载 `GitControlMenu`。
- 菜单使用 Portal 固定在触发按钮上方，包含“分支与提交”和“Worktree”两个页签。
- 所有操作串行；菜单内保留正在执行状态，关闭菜单不取消已发出的 Git 操作。
- 覆盖冲突通过居中的确认对话框进入“提交并切换”流程。

## 5. 错误与安全边界

- 非 Git 目录保持原底栏行为，不显示 Git 控件。
- detached HEAD 显示短提交号；分支创建仍可用，直接推送保持关闭。
- 没有远端时保留本地提交能力，并明确展示“尚未配置远端”。
- Worktree 被其他进程删除或分支状态变化时，下一次刷新以 Git 结果为准。
- 不执行 `git reset`、`git clean`、强制 checkout、强制 push 或强制 Worktree 删除。
- Git stderr 经过长度限制后展示；不吞掉真实失败原因。

## 6. 验收

1. 脏工作区可安全切换时直接成功；覆盖冲突进入提交并切换流程。
2. 提交范围与“包含未暂存的更改”一致。
3. 首次推送建立 upstream，随后推送使用 upstream。
4. Worktree 列表与 `git worktree list` 一致，当前项和关联会话数准确。
5. 进入 Worktree 后新建或聚焦的会话 cwd 与目标路径一致。
6. 当前、脏或仍有关联会话的 Worktree 得到保留。
7. Git 操作后底栏状态更新；全程单一反馈且终端输入保持可用。
