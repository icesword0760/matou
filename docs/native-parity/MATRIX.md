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
