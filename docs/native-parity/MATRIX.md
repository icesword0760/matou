# Native parity matrix

Initial source inventory, NOT a closed functional or acceptance matrix. Baseline and active DAG: STATUS.md. Full contract: OBJECTIVE.md.

Group inventory below is not an acceptance result. Concrete observed scenarios follow it; unlisted states remain pending. Native storage and bootstrap source contracts are implemented; all Mac/Windows product parity verification remains pending. D01 is the approved Shell history restoration difference. Test names refer to tests/e2e/<name>.spec.ts; component names locate existing renderer/main/runtime code. As each group is observed, add concrete scenario IDs and preconditions, action, expected state/persistence/error, code owner, paired evidence, discrepancy. Do not generate duplicate reports for each row.

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
| A08.01 | Execute completed marker then Cmd+Q; process gone; reopen same data | Structure/task/name restored; screen only shows new prompt, completed text not visible | A08-before-quit.png / A08-after-quit-reopen.png; DB has completed block. Approved deviation D01: user chose native visible completed-history restoration per existing tests; reference blank result is NOT a successful restoration |
| A02.06 | Two tasks; current task has one running Shell canvas; close and confirm | Removes that canvas, retains the task with an empty stage and new-tab button; horizontal new Shell disabled | A02-last-canvas-with-other-task.png; A02-empty-task-after-last-canvas.png |
| A02.07 | Open menu for the empty test task → 删除 → confirm | Warns that sessions are lost but local directory is retained; removes test task, other task remains selected | A02-delete-empty-task-confirm.png; subsequent A02-last-tab-protection.png shows sole remaining task |
| A02.08 | Only one task with one canvas; click tab close | Informational modal explains the last tab cannot delete the task; only 我知道了, leaves canvas/process intact | A02-last-tab-protection.png |
| A02.09 | One task, one active canvas with two Shell nodes; open Board | Shows 就绪/运行中/阻塞/完成 columns; existing default task is 就绪. Badge reports 3 sessions, while active canvas has 2: count semantics must be checked before implementing | A02-kanban.png; dragging and count semantics pending |
| A03.01 | One Shell; click 横向新增 Shell | Second real Shell appears horizontally in same canvas and receives keyboard focus, root-session count becomes 2; first Shell retained | A03-two-shells.png; A14-after-window-reopen.png confirms both later remain |
| A04.01 | Right-click Shell card header | Menu 重命名… / ↗ 独立窗口 / 移除节点…; plain Shell does not expose Agent fork action | A04-shell-context-menu.png |
| A05.01 | Two Shell root nodes; click DAG | Opens separate graph window, two root cards with output previews, search, scale controls, relationship legend | A05-dag-two-roots.png |
| A05.02 | Search DAG for unique second-Shell output marker; press Enter | One result matches output; Enter returns to main window and focuses corresponding terminal | A05-dag-output-search.png; AX confirmed main window and second terminal focus; graph animation not timed |
| A08.02 | Print PARITY_SECOND_SHELL; Cmd+F; search marker; Esc | Search bar has case/regex/whole-word and prev/next; reports 1/2 for command+output; Esc restores second terminal focus | A08-search-two-shells.png; historical archive search remains pending |
| A09.01 | Open session loader from Shell; switch source tabs | Two source tabs; both are empty in isolated home/workspace, disabled load button, left filter and disabled preview search; cancel restores terminal focus | A09-session-loader-empty.png; A09-session-loader-alternate-empty.png; actual authenticated Agent resume pending |
| A11.01 | Click Local environment in bottom bar | Popover shows current path and Finder/system-terminal actions | A11-environment-menu.png; launching external apps not exercised |
| A12.01 | Open notification center with no events | 通知 (0), 暂无通知, sound checked by default | A12-notification-empty.png; real Agent event notification/HUD pending |
| A13.01 | Cmd+I repeatedly in terminal | Alternates light/dark and returns focus to terminal; four toggles return to initial light | A13-theme-cycle-first.png (dark), second (light), third (dark); restart persistence not inferred: source uses component state |
| A13.02 | Open language/timezone under initial MATOU_LOCALE=zh-CN harness; select English | Preference changes but UI remains Chinese with explicit environment-override notice | A13-language-timezone.png; A13-language-env-override.png; this is override behavior, not failed ordinary locale behavior |
| A13.03 | Relaunch same reference data using --locale system, no MATOU_LOCALE; select English, then Follow system | Visible labels switch immediately to English, user task name remains 默认; Follow system restores Chinese; closes settings into same terminal | A13-language-english.png; A13-language-return.png; runtime language requires restart per UI, not yet checked |
| A14.01 | Open second Shell as independent window; execute DETACHED_PARITY_OK; close independent window | Independent window states session stays running; closing returns to same main card and focus, marker visible there | A14-detached-shell.png; A14-detached-output.png; A14-detached-return.png |
| A14.02 | Print Shell PID, click main red close button, reacquire app, print PID again | Main app PID 87615 survives; Shell PID is 35789 before/after; output and both cards remain. AX reacquisition raises the app, so no duration/animation claim for hidden state | A14-before-window-hide.png; A14-after-window-reopen.png |
| A17.01 | Open application update control in isolated disabled-update environment | Popover identifies version 0.2.19 and offers Check updates | A17-update-popover.png; update download/apply/failure and active-session protection not exercised |

## First slice source contracts (read-only advisor, verify against UI where pending)

- main/index.ts:184: main-window-1, default root override or system home, name basename. HierarchyShell.tsx:186 bootstrap then refresh.
- hierarchy-application-service.ts:1741 creates workspaces/execution_contexts/tasks/scenes/scene_nodes/sessions/session_mounts/window placements and focus in one transaction; default pinned workspace, task 默认/planned, scene Shell · root/tile/title_pinned=0, shell created/title Shell. Five ordered creation events at :2010.
- SceneTabBar.tsx:105 new tab invokes hierarchy.create-canvas (not create-scene); session-canvas-service.ts:174 creates tile/root node/Shell/mount and focus, names 新画布 then unused suffix (:1022).
- TerminalSurface.tsx:701 attaches actual cols/rows; runtime-server.ts:2039 chooses persisted cwd, workspace root, execution cwd, home; pty-session.ts:359 starts real shell, TERM=xterm-256color, shell integration.
- Ordinary Shell actual quit should create new PID. shell.restoreHistoryEnabled defaults true; completed OSC133 blocks persisted in shell_history_blocks, limited to 100 blocks, newest output 5000 lines/1MiB each per existing source. Runtime publishes terminal.restored-history; renderer observation presently disagrees (A08.01); user approved D01 to restore history correctly in native implementation. Non-shell checkpoint restore is a separate path.
- SceneTabBar.tsx:257 last task/last tab only dismisses notice, does not actually close scene, now observed A02.08. Mac red close retains app/Shell process, observed A14.02; hidden transition duration not measured. Do not implement solely from misleading action label hide-window.
- Closing normal eligible scene archives sessions/scene and disposes PTY; selects right neighbor or last left neighbor. Running/starting/needs-input requires confirmation, observed A02.05.

## Performance evidence scope

`trace-1794714678641/` records one 45-second diagnostic run of the actual reference package using optional loopback CDP, no E2E or substitute runtime. CUA supplied real Shell paste/Enter, terminal search open/query/close, settings navigation and language switching. `metadata.json` binds launch PID 56861, renderer 56871, path isolation and no locale override. `summary.json` is generated by `tooling/native-parity/summarize-trace.py`; `trace.json` retains raw events.

The 9,509 recorded renderer main-thread tasks have max duration 11.713 ms, with zero over 50 ms. This only describes the sampled mixed run. Engine EventLatency intervals are retained as diagnostics and MUST NOT be relabeled terminal output latency, scene-ready time, or physical input-to-display latency. The recording has no app-start interval and does not cover sustained output, long history, resize/drag or repeated per-scenario samples. Performance budgets are not frozen yet.


## Additional observed reference scenarios — first-slice performance and history

| Scenario | Operation and result | Evidence / boundary |
| --- | --- | --- |
| A03.02 | Add third Shell; click between terminals, focused card expands and others compact; real input focus follows clicked terminal | A03-three-shells.png; trace-1794716367473 covers initial clicks, native motion comparison pending |
| A04.02 | Drag card header horizontally and horizontally scroll back; strip changes visible portion while all three sessions remain | A04-carousel-drag.png; A04-carousel-scroll-return.png; trace-1794716681206 adds actions under load |
| A03.03 | Drag bottom-right corner from1200x780 to1050x680 then back; terminals reflow and focus remains in same session | A03-resized-1050x680.png; A03-resize-return-1200x780.png; P02-load-small.png adds pressure state |
| A07.02 | Two shells each output8MiB/30s; third executes unique printf; marker visible while both output streams continue, both eventually finish | P02-input-during-two-output.png; P02-both-output-completed.png; trace-1794716681206; do not equate task durations with visible feedback latency |
| A08.03 | Search early START_B after it leaves live scrollback; archive opens1/1, read-only with±250-line context; Esc returns live, X closes search and focus returns | A08-long-history-search.png; both completion markers visible after return; compression path not forced, large cold archive/restart remains pending |
| P01.01 | Two real process restarts, OS caches retained; first UI690.1/693.7ms, both prompts754.85/798.14ms in inspected frame7 | startup-observed-summary.json and both frame directories; not disk-cold or physical monitor latency |
| P01.02 | Ten pasted digits through real raw-mode PTY; next actual compositor frame changes known color patch | trace-1794715435260, median30.48ms/max34.87ms; renderer dispatch-to-captured-frame, not keydown-to-photon |

## Native source-contract progress (not native product acceptance)

| Slice | Result | Evidence and remaining integration |
| --- | --- | --- |
| A16 storage primitives |35 source migrations,87 schema objects, transaction/replay/conflict/rollback/disk reopen passed|core/tests/storage_contract.rs,5 tests; caller-owned DB only, lease/backup/recovery UX still pending|
| A01/A08 hierarchy structure |6 source operations match result/error,15 tables,events,receipts; real SQLite close/reopen preserves identity/navigation|core/tests/bootstrap_contract.rs,1 sequential contract; no native window or PTY, D01 visible Shell history still pending|
| A07 parser and display-state boundary |6 frozen source fixture groups plus theme/OSC/resize/scroll/sync checks pass; Unicode 6 widths preserved; Mac 11 tests pass|terminal/tests/terminal_contract.rs; Windows library and test EXEs compile, not executed; GPUI glyphs/IME/selection/full input modes still pending|
| A08 completed Shell history |7 source collector cases and real DB105-block insert/100-block retention/close-reopen, disabled restoration, limits, purge and CRLF text pass|core/tests/shell_history_contract.rs,4 tests; root source review completed; D01 still needs native window quit/reopen proof|
| A16 data ownership |9 captured source cases, five real competing processes for empty/stale paths, winner crash/next-generation takeover and malformed SQLite fence pass|core/tests/ownership_contract.rs,4 tests including worker entry; Mac only; Windows compiled but directory synchronization still needs target-filesystem execution|

| A08/A16 journal boundary |8 captured records encode identically; streaming raw/gzip, torn-tail repair, CRC corruption, segment rotation/reopen, protected compression and cache-write-fault semantics pass|core/tests/journal_codec_contract.rs (5), journal_segments_contract.rs (6), segments unit (1); live session and Windows filesystem still pending|
| A16 per-session durability |6 frozen scenarios/22 steps plus error-event ordering match; bounded admission, fault retention, retry publication and healthy watermarks|core/tests/durability_contract.rs (2); synchronous owner seam only, real disk fault/session integration not yet exercised|
| A07 real Shell process boundary |real isolated zsh verifies login/config environment, direct/absent-SHELL parity, OSC command/output/completion order, resize97x33, normal exit7/signal0 and SIGKILL exit0/signal9|core/tests/pty_contract.rs (2); frozen node-pty probe baseline/pty-shell-environment.json; no native GUI or full session-owner integration, Windows compile only|
| A07/A16 live session |real zsh verifies Unicode output,97x33 resize, exact DSR response, exit7, journal-before-publication; a real directory-blocked journal rotation pauses only its session and retries the same sequence; 2.2MiB slow-consumer replay is complete without duplicate completed blocks|runtime/tests/session_contract.rs (4); flow_contract.rs (3) compares original decoder/batcher/credit fixtures; storage-fault screen retains last displayed healthy frame; no visible error UI yet|
| A07 independent workers |frozen A does not prevent B's command or persisted output; asynchronous command forwarding, owned screen return, natural exit with later screen/ack and explicit join/cleanup pass|runtime/tests/worker_contract.rs (1), two actual isolated Mac zsh processes; not a slow-disk timing measurement or UI responsiveness result|
| A08/D01 runtime restore |completed command reaches SQLite before forwarded output; actual worker shutdown and DB reopen restore completed marker into new terminal state before live prompt; unfinished command is absent; disabled restoration preserves saved rows; restoration text is not written into new journal frames|runtime/tests/owner_contract.rs (1), real SQLite and PTYs; proves internal screen/DB integration, not visible GPUI quit/reopen acceptance|
