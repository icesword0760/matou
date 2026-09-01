# Recovery Task 7 — Worktree 健康检查与启动对账

日期：2026-09-02

## 用户结果

- App 启动时会核对每个受管 Worktree 的真实目录、Git 登记、仓库身份、分支/HEAD 与 dirty 状态。
- Worktree 目录被删除、移动到未登记位置、指向其他仓库或切到错误分支时，原会话、历史与 DAG 关系继续保留；对应会话进入待处理环境状态。
- 受管 Worktree 不健康时，Shell、Claude Code、Codex 的新 PTY/provider 进程不会在工作空间根目录或 HOME 中替代启动，避免用户误在错误目录继续修改文件。
- 普通 Local 会话仍保留原有目录降级策略：保存目录消失时回到当前工作空间根目录。
- 创建过程在 branch-only、目录已创建但 DB 未完成、branch 与目录均未创建三种中断点都可继续；删除过程在目录已消失时清理 Git 登记，在 dirty 时保留用户改动。
- 单个 Worktree setup 失败只降级对应环境，其他 Worktree 继续完成启动对账。

## 真实场景覆盖

全部 Worktree 场景均使用临时真实 Git 仓库与 `git worktree` 命令，不使用虚构 Git 返回值：

1. ready / clean
2. 目录删除
3. 目录移动但 Git 未登记新路径
4. 错误仓库
5. 错误 branch
6. detached HEAD 与 detached HEAD identity mismatch
7. dirty worktree
8. creating: branch-only
9. creating: directory-ready
10. creating: fully-missing
11. removing: directory已删并 `git worktree prune`
12. removing: dirty retained
13. setup 失败隔离并继续其他 Worktree
14. Runtime spawn 前删除受管 Worktree，确认无 PTY、无 cwd fallback、Session cwd 不变
15. Local Session 保存目录删除，确认仍回到 Workspace 根目录

## 验证证据

- Task 7 定向矩阵：4 files，22 passed，57 skipped。
- Runtime typecheck：通过。
- Runtime 全量：542 项中 541 passed；1 项既有 5 秒真实 Shell 时序用例超时，随后精确单测复跑通过。
- 全量失败用例：`tracks real Shell commands as running, idle, error, and interrupted work`。
- 精确复跑：1 passed，58 skipped。

## 变更边界

- 新增 `WorktreeHealthService` 与 `WorktreeReconciler`。
- Worktree 创建开始时持久化 `base_ref`，让完全缺失的中断操作可用原 identity 续作。
- Worktree 删除目录已消失时先清理 Git worktree 登记，再把 DB 与 execution context 收敛到 removed。
- Runtime 在创建新 PTY/provider 前读取权威 SessionEnvironment 并实时复核受管 Worktree。
- 对账只修改 Worktree 和 SessionEnvironment 状态；不改 Session、provider binding、历史或关系图。

## 后续衔接

- Task 8 在这些权威状态上增加“恢复、重新定位、Handoff 到 Local/Worktree”操作。
- Task 9 将环境状态与 Git 状态拆成右下角两个独立入口，并在卡片上展示恢复层。
- Task 15 将 Worktree 对账接入完整的分层恢复进度协调器。
