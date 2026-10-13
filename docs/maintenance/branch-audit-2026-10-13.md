# Matou 分支检查与清理（2026-10-13）

> 这是第一轮检查快照。后续逐分支整合已完成，最终状态见 `branch-integration-2026-10-13.md`。

本地分支：55 → 50。仅删除以下 5 个已合入 main、没有 worktree 占用的本地分支。未删除任何工作目录、未提交文件或远程分支。

检查时 main 与 origin/main 一致：`1cf3d521f65a9f76e616f0ad170ed88d2a523a66`。已执行 git fetch origin。

## 已清理及恢复方式

- `codex/ai-structure-control`：`6e771bf312431514d2f21f203f8b5827ec4340b0`。恢复：`git branch codex/ai-structure-control refs/archive/branch-cleanup/20261013-174203/codex/ai-structure-control`。
- `codex/brand-neutral-cleanup`：`c1f2e4fa1639ccf1ba09a9de09716e8143c91d3f`。恢复：`git branch codex/brand-neutral-cleanup refs/archive/branch-cleanup/20261013-174203/codex/brand-neutral-cleanup`。
- `docs/english-screenshots`：`416bbeb7effe45a71143ab29bdf567621c3d3769`。恢复：`git branch docs/english-screenshots refs/archive/branch-cleanup/20261013-174203/docs/english-screenshots`。
- `launch/english-first`：`bd8507ffbdc3d983983116dec762aa3a1244bb9d`。恢复：`git branch launch/english-first refs/archive/branch-cleanup/20261013-174203/launch/english-first`。
- `video/bilibili-launch`：`9a514a986161e2f89b70b1a8944db92eab5c6e6f`。恢复：`git branch video/bilibili-launch refs/archive/branch-cleanup/20261013-174203/video/bilibili-launch`。

## 优先保护的未提交工作

- `codex/claude-accounts-acceptance`：正在验收的 Claude 账户切换，基于 0.2.4。虽然提交历史与 main 相同，实际功能仍在未提交文件中，完整保留。
- `feat/settings-language`：当前原始目录，旧基础上的账户功能及文档仍未提交，完整保留。
- `codex/cloud-release-test`：旧 0.1.1 更新测试，另有未跟踪 output 目录，保留。

## 尚未确认全部纳入主线的 21 个历史分支

这里表示提交内容仍有差异，不等于产品缺少这些功能。多数主线存在同标题、但补丁不同的提交；同标题不作为删除依据。下表按实际提交及改动文件归纳。

| 分支 | 工作内容 | 主线证据 |
| --- | --- | --- |
| `codex/brand-gate-final` | 品牌标识清理与命名检查 | `956d113` → `fa829de`, `c1f2e4f` |
| `codex/claude-provider-recovery-e2e` | Claude 恢复失败时保留会话、恢复流程及端到端测试 | `e5f5ae5` → `38a6cfd` |
| `codex/cloud-release-test` | 旧 0.1.1 云更新测试版本；存在未跟踪 output | `835fd6f` → 未找到同标题提交 |
| `codex/d01-foreground-terminal-lifecycle` | 卡片虚拟化时保留前台终端、连接和缓存 | `0245cde` → `72a5f9c` |
| `codex/durability-gate-overflow` | 终端持久化压力测试；另两项日志背压与压缩补丁已等价纳入 | `f7f8c1d` → `ba18d9b` |
| `codex/final-ui-e2e-fixes` | DAG、卡片、状态投影与底层运行交互稳定性 | `8e60147` → `ea59fc7` |
| `codex/final-window-recovery-e2e-fixes` | 独立窗口、Shell 恢复和终端回放 | `88c10a1` → `0ba451b` |
| `codex/journal-range-replay` | 终端日志按范围读取与流式回放 | `1fbebce` → `bc9d7fb` |
| `codex/layered-recovery` | 分层恢复调度、单卡片恢复进度、旧命名检查脚本 | `5f3173a` → `ce68c81`; `d6f24e6` → `10415c4`; `b41c084` → 未找到同标题提交 |
| `codex/layered-recovery-merge-fixes` | 恢复状态与权限的并发竞态修复 | `c9167c3` → `310e40e` |
| `codex/long-terminal-history-gate` | 长终端历史、日志故障与恢复测试 | `553d445` → `0ee3eb6` |
| `codex/main-wip-backup-20260901-154452` | 9 月 1 日主线未完成工作备份，包含工作区看板、侧栏原型、模型配置、DAG 等混合改动 | `76419fe` → 未找到同标题提交 |
| `codex/migration-interruption-e2e` | 数据库迁移中断、崩溃后的恢复测试 | `7435499` → `e0091bb` |
| `codex/native-window-focus-e2e` | 原生窗口焦点恢复测试 | `82ef8c5` → `6dbac18` |
| `codex/recovery-notification-fix` | 恢复通知、画布关闭及删除确认；8 项中 7 项已有等价补丁 | `2c50e37` → `d822b62`, `57a9888` |
| `codex/runtime-crash-e2e` | 后台运行进程崩溃与连接、界面恢复测试 | `e36af44` → `a66715f` |
| `codex/runtime-pty-stress-matrix` | 真实终端进程的压力测试；与 durability 分支共享一项提交 | `f7f8c1d` → `ba18d9b` |
| `codex/scale-dag` | 大量会话下的层级、DAG 与恢复性能 | `31b6539` → `2bea7b7` |
| `codex/task10-offscreen-terminal-e2e` | 离屏卡片的前台终端连续性测试 | `2a827ab` → `e9f07f2` |
| `codex/task6-checkpoint-segment-protection` | 日志压缩时保护恢复检查点与关联分段 | `7c344a0` → `8716d0b` |
| `codex/terminal-history` | 持久终端历史归档、压缩、搜索及回放 | `ee0e947` → `437ea1f` |

## 20 个已找到全部等价补丁的历史分支

Git 提交编号不同，但 git cherry 显示分支新增提交均在 main 中有等价补丁。均有 worktree 占用，本轮保留目录和分支；未将目录占用直接断言为仍有人开发。

- `codex/archive-history-context` — 2026-09-02 feat(terminal): open cold search context safely
- `codex/d08-dag-aggregation` — 2026-09-02 feat(dag): aggregate far session branches
- `codex/d09-delete-scope-ui` — 2026-09-02 feat: unify session node removal scopes
- `codex/dag-10k-electron-e2e` — 2026-09-02 test(dag): gate real 10000-node Electron flow
- `codex/detached-return-retry` — 2026-09-02 fix(desktop): retry detached session return
- `codex/final-fork-e2e-fixes` — 2026-09-02 fix(fork): project failures and preserve restored focus
- `codex/fork-background-e2e` — 2026-09-02 test(recovery): gate background Fork setup
- `codex/hud-e2e-fix` — 2026-09-02 fix: keep bottom HUD controls responsive
- `codex/ledger-update` — 2026-09-02 docs(audit): reconcile hardening implementation ledgers
- `codex/projection-10k-incremental` — 2026-09-02 perf(runtime): keep 10k projections within frame budget
- `codex/provider-identity-gate` — 2026-09-02 fix(runtime): verify restored provider identity
- `codex/real-claude-storage-resume` — 2026-09-02 fix(recovery): wait for real Claude readiness
- `codex/recovery-readonly-gates` — 2026-09-02 test: harden recovery acceptance gates
- `codex/recovery-scale-gate` — 2026-09-02 test(recovery): add real PTY scale gate
- `codex/recovery-scale-worktrees` — 2026-09-02 test(recovery): cover managed worktrees at scale
- `codex/runtime-enospc-card-isolation` — 2026-09-02 test(desktop): verify background Scene PTY continuity
- `codex/task2-graph-layout-index` — 2026-09-02 perf: make session graph traversal iterative
- `codex/terminal-history-fixes` — 2026-09-02 fix(terminal): recover rotated history safely
- `codex/unit-gate-audit` — 2026-09-02 fix: stabilize release hardening gates
- `codex/worktree-recovery-e2e` — 2026-09-02 fix(recovery): close Worktree relocation flow

## 其余保留的主线及已合并分支

以下分支都有工作目录；没有强制删除、切换或拆除这些工作目录。

- `codex/claude-accounts-acceptance`：有未提交工作，优先保护。
- `codex/git-workflow`：保留已占用的工作目录。
- `codex/internal-hardening-audit`：保留已占用的工作目录。
- `codex/loaded-session-fork`：保留已占用的工作目录。
- `codex/session-history-pagination`：保留已占用的工作目录。
- `feat/settings-language`：有未提交工作，优先保护。
- `feature/prd-05-hierarchy`：保留已占用的工作目录。
- `feature/session-dag-canvas`：保留已占用的工作目录。
- `main`：保留已占用的工作目录。

## 检查边界

- 对 55 个本地分支检查提交祖先关系、所有已注册工作目录的已跟踪/未跟踪修改；对历史未合并提交检查补丁等价性和主线同标题提交。
- 未对所有历史功能重新做产品验收；没有把同名提交当成完全等价，也没有把未找到等价补丁当成主线缺少功能。
- 本轮仅清理无 worktree 占用的明确已合并分支，因此无需判断或中断其他聊天/进程。
- 一个 detached HEAD 的历史工作目录也完整保留。
- 未删除忽略文件、构建产物和目录。归档 refs 保留被清理分支的提交，可用上述命令恢复。
