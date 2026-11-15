# Native parity matrix

Initial source inventory, NOT a closed functional or acceptance matrix. Baseline and active DAG: STATUS.md. Full contract: OBJECTIVE.md.

All groups below: baseline UI evidence pending; native implementation pending; Mac/Windows verification pending; no differences accepted. Test names refer to tests/e2e/<name>.spec.ts; component names locate existing renderer/main/runtime code. As each group is observed, replace group summaries with concrete scenario IDs and preconditions, action, expected state/persistence/error, code owner, paired evidence, discrepancy. Do not generate duplicate reports for each row.

| Group | Scope | Source entry | Existing scenario seed |
| --- | --- | --- | --- |
| A01 | 外壳/启动/最近会话/空态 | App.tsx; HierarchyShell; EmptyWorkspaceState | startup-recent-sessions; prd-03-workbench-information |
| A02 | 层级创建/切换/重命名/关闭/路径修复 | WorkspaceSwitcher; WorkspaceKanbanBoard; TaskSidebar; SceneTabBar | prd-05-hierarchy; prd-05-lifecycle; prd-05-path-recovery |
| A03 | 标签/分屏/调宽/后台保活 | SplitTree; SplitDivider; TerminalPane; scene-layout-service | terminal-card-width; terminal-resize-settle; terminal-offscreen-continuity |
| A04 | 会话画布/轮播/面包屑/导航/恢复 | SessionCanvas; SessionCarousel; ParentProjection | session-canvas-basics; session-canvas-navigation; session-canvas-recovery |
| A05 | DAG窗口/搜索/定位/大图 | DagWindowApp; DagCanvas; DagSearch; dag-window-manager | session-dag-window; scale/dag-10000-electron |
| A06 | 分支/批量分支/节点移除/worktree/回滚 | BranchDialog; RemoveNodeDialog; fork-workflow-service | prd-06-session-fork; session-node-removal; fork-runtime-crash-recovery |
| A07 | PTY/键鼠/IME/剪贴板/字体/启动失败 | TerminalSurface; useTerminalShortcuts; pty-session | terminal-channel; session-shell-start-failure; terminal-text-sharpness |
| A08 | 历史/搜索/滚动/恢复 | TerminalSearchBar; journal-history-reader; terminal-screen-recovery | terminal-long-history; terminal-corrupt-history-live; prd-04-session-recovery |
| A09 | Agent启动/载入/续会话 | SessionLoaderDialog; provider-launch-plan; provider-resume-monitor | session-load-existing-claude; antigravity-acceptance; real-claude-storage-resume |
| A10 | 账户/供应商/登录/切模/认证 | AgentModelSettings; ProviderSetupDialog; claude-accounts; provider-config | agent-model-settings; provider-onboarding; provider-activation-response |
| A11 | Git/分支/提交/推送/执行环境 | GitControlMenu; EnvironmentControlMenu; git-workspace-service | session-canvas-fork-worktree; recovery-worktree |
| A12 | 通知/HUD/Agent团队状态 | NotificationProvider; NotificationCenter; TerminalHud; provider-hook-server | prd-01-agent-notifications; prd-02-bottom-hud; context-usage-null |
| A13 | 语言/主题/时区/出口保护 | locale-store; useTerminalShortcuts Mod+I; ClaudeTimeZoneSettings; egress | locale-english; claude-time-zone; network-egress |
| A14 | 多窗/独立终端/外接屏/Finder/系统集成 | window-manager; detached-terminal-focus-coordinator; finder-quick-action | prd-05-multi-window; native-window-focus-restoration; secondary-display-placement |
| A15 | mt CLI/外部控制/导航 | mt-cli; host-control-server; runtime-host-action-facade | ai-host-control-cli; ai-host-navigation; ai-host-structure-control |
| A16 | 存储/迁移/只读/备份/故障恢复 | runtime-host; DatabaseRecoveryPage; storage; recovery | database-recovery; runtime-crash-recovery; runtime-journal-fault-isolation |
| A17 | 安装/更新/活跃会话保护 | AppUpdateControl; app-update-manager; update-feed | app-updates; packaged-runtime |


## Observed reference scenarios — 2026-11-15

Evidence base: artifacts/native-parity/reference/evidence/. All native result/evidence columns remain pending. Source baseline and path-only harness documented in STATUS.md. Screenshots are actual frozen product, not mockups.

| Scenario | Preconditions and action | Actual reference result | Evidence / remaining question |
| --- | --- | --- | --- |
| A01.01 | Empty isolated data; valid default workspace; ordinary startup | Creates default pinned workspace, 默认 task and Shell canvas, renders real zsh prompt | A01-first-launch.png; repeat identical pixels |
| A07.01 | Focus shell; paste then Enter printf with Chinese/emoji | Displays command and single Unicode output | A07-shell-input.png; IME candidate interaction still pending |
| A02.01 | Click new tab + | Creates and selects 新画布 immediately with real Shell, prior tab remains | A02-new-tab.png |
| A02.02 | Click workspace + task | Adds/selects 新事项 and its initial Shell scene immediately | A02-new-task.png |
| A02.03 | Open task menu | 置顶 / 重命名 / 删除 menu adjacent to task | A02-task-menu.png |
| A02.04 | Rename task → enter 原生对照 → 确定 | Centered modal, 20-character limit shown, closes and updates task, toast 工作台已重命名 | A02-rename-dialog.png; retained after quit/reopen |
| A10.01 | Open settings from bottom sidebar | Claude Code provider page, AI 服务 and 通用 navigation, one default provider, compaction defaults follow session | A10-settings.png; repeat identical pixels; no credentials used |
| A02.05 | Two canvases in current task; close new running Shell canvas | 关闭画布 confirmation says 1 session removed from UI, files/worktrees kept; confirm returns to prior tab and terminal focus | A02-close-tab-confirm.png; A02-after-close-tab.png |
| A08.01 | Execute completed marker then Cmd+Q; process gone; reopen same data | Structure/task/name restored; screen only shows new prompt, completed text not visible | A08-before-quit.png / A08-after-quit-reopen.png; DB has completed block. Investigate discrepancy; NOT accepted as history-restoration pass |

## First slice source contracts (read-only advisor, verify against UI where pending)

- main/index.ts:184: main-window-1, default root override or system home, name basename. HierarchyShell.tsx:186 bootstrap then refresh.
- hierarchy-application-service.ts:1741 creates workspaces/execution_contexts/tasks/scenes/scene_nodes/sessions/session_mounts/window placements and focus in one transaction; default pinned workspace, task 默认/planned, scene Shell · root/tile/title_pinned=0, shell created/title Shell. Five ordered creation events at :2010.
- SceneTabBar.tsx:105 new tab invokes hierarchy.create-canvas (not create-scene); session-canvas-service.ts:174 creates tile/root node/Shell/mount and focus, names 新画布 then unused suffix (:1022).
- TerminalSurface.tsx:701 attaches actual cols/rows; runtime-server.ts:2039 chooses persisted cwd, workspace root, execution cwd, home; pty-session.ts:359 starts real shell, TERM=xterm-256color, shell integration.
- Ordinary Shell actual quit should create new PID. shell.restoreHistoryEnabled defaults true; completed OSC133 blocks persisted in shell_history_blocks, limited to 100 blocks, newest output 5000 lines/1MiB each per existing source. Runtime publishes terminal.restored-history; renderer observation presently disagrees (A08.01). Non-shell checkpoint restore is a separate path.
- SceneTabBar.tsx:257 last task/last tab only dismisses notice, does not actually close scene; actual Mac close button hides existing process. Both still require isolated actual capture. Do not implement solely from misleading action label hide-window.
- Closing normal eligible scene archives sessions/scene and disposes PTY; selects right neighbor or last left neighbor. Running/starting/needs-input requires confirmation, observed A02.05.
