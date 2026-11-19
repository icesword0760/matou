# Native parity matrix

Initial source inventory, NOT a closed functional or acceptance matrix. Baseline and active DAG: STATUS.md. Full contract: OBJECTIVE.md.

Group inventory below is not an acceptance result. Concrete observed scenarios follow it; unlisted states remain pending. Native storage/runtime contracts and the first Mac Shell window are implemented; complete Mac/Windows product parity remains pending. D01 is the approved Shell history restoration difference. Test names refer to tests/e2e/<name>.spec.ts; component names locate existing renderer/main/runtime code. As each group is observed, add concrete scenario IDs and preconditions, action, expected state/persistence/error, code owner, paired evidence, discrepancy. Do not generate duplicate reports for each row.

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

Evidence base: artifacts/native-parity/reference/evidence/. Native results for the first Shell slice are recorded below; other groups remain pending. Source baseline and path-only harness documented in STATUS.md. Screenshots are actual frozen product, not mockups.

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
| A16 storage primitives |35 source migrations,87 schema objects, transaction/replay/conflict/rollback/disk reopen passed|core/tests/storage_contract.rs,5 tests; startup/backup integrated in later A16 rows; recovery UX still pending|
| A01/A08 hierarchy structure |6 source operations match result/error,15 tables,events,receipts; real SQLite close/reopen preserves identity/navigation|core/tests/bootstrap_contract.rs,1 sequential contract; no native window or PTY, D01 visible Shell history still pending|
| A07 parser and display-state boundary |6 frozen source fixture groups plus theme/OSC/resize/scroll/sync checks pass; Unicode 6 widths preserved; Mac 11 tests pass|terminal/tests/terminal_contract.rs; Windows library and test EXEs compile, not executed; GPUI glyphs/IME/selection/full input modes still pending|
| A08 completed Shell history |7 source collector cases and real DB105-block insert/100-block retention/close-reopen, disabled restoration, limits, purge and CRLF text pass|core/tests/shell_history_contract.rs,4 tests; root source review completed; D01 still needs native window quit/reopen proof|
| A16 data ownership |9 captured source cases, five real competing processes for empty/stale paths, winner crash/next-generation takeover and malformed SQLite fence pass|core/tests/ownership_contract.rs,4 tests including worker entry; Mac only; Windows compiled but directory synchronization still needs target-filesystem execution|

| A08/A16 journal boundary |8 captured records encode identically; streaming raw/gzip, torn-tail repair, CRC corruption, segment rotation/reopen, protected compression and cache-write-fault semantics pass|core/tests/journal_codec_contract.rs (5), journal_segments_contract.rs (6), segments unit (1); live session covered below; Windows filesystem execution pending|
| A16 per-session durability |6 frozen scenarios/22 steps plus error-event ordering match; bounded admission, fault retention, retry publication and healthy watermarks|core/tests/durability_contract.rs (2); live disk-fault/session integration covered below; visible error/retry UI pending|
| A07 real Shell process boundary |real isolated zsh verifies login/config environment, direct/absent-SHELL parity, OSC command/output/completion order, resize97x33, normal exit7/signal0 and SIGKILL exit0/signal9|core/tests/pty_contract.rs (2); frozen node-pty probe baseline/pty-shell-environment.json; session-owner integration covered below; no native GUI, Windows compile only|
| A07/A16 live session |real zsh verifies Unicode output,97x33 resize, exact DSR response, exit7, journal-before-publication; a real directory-blocked journal rotation pauses only its session and retries the same sequence; 2.2MiB slow-consumer replay is complete without duplicate completed blocks|runtime/tests/session_contract.rs (4); flow_contract.rs (3) compares original decoder/batcher/credit fixtures; storage-fault screen retains last displayed healthy frame; no visible error UI yet|
| A07 independent workers |frozen A does not prevent B's command or persisted output; asynchronous command forwarding, owned screen return, natural exit with later screen/ack and explicit join/cleanup pass|runtime/tests/worker_contract.rs (1), two actual isolated Mac zsh processes; not a slow-disk timing measurement or UI responsiveness result|
| A08/D01 runtime restore |completed command reaches SQLite before forwarded output; actual worker shutdown and DB reopen restore completed marker into new terminal state before live prompt; unfinished command is absent; disabled restoration preserves saved rows; restoration text is not written into new journal frames|runtime/tests/owner_contract.rs (1), real SQLite and PTYs; proves internal screen/DB integration, not visible GPUI quit/reopen acceptance|

| A16 backup/restore |online backup includes committed WAL; frozen source backup restores; old committed data remains in rollback; retention7 and rename interruptions preserve authoritative data|core/tests/backup_contract.rs (4), backup unit (3 failure stages); source fixture/exporter retained; no recovery UI or clean-exit integration yet|
| A01/A07 Shell repository |22 frozen operations match session/run rows, domain events, command receipts, results and errors across real reopen|core/tests/session_repository_contract.rs (1 sequential contract); worker lifecycle wiring remains open|
| A16 recovery records |9 frozen marker/claim/resolution cases, concurrent generation publication and interrupted publication/resolution pass|core/tests/recovery_marker_contract.rs (2), resolution fault unit; required generation stable across reopen; recovery action UI not implemented|
| A16 startup |fresh pre-migration backup, newer/readonly preservation, live-owner precedence, corruption quarantine, backup-list failure, malformed owner and actual killed-process WAL/SHM behavior pass|core/tests/startup_contract.rs (9 + explicitly invoked child helper); actual Mac files/SQLite; Windows test executable compiled only; not native app startup or end-user recovery acceptance|

| A03/A05 graph and work state |16 frozen projection/status steps preserve graph values, rows, events and replay; same-status call retains source event behavior|core/tests/session_graph_contract.rs; provider/fork/worktree/detached branches awaiting respective capability-stage fixtures and UI|
| A07 ordinary-Shell derivation |22 work streams/7 cwd streams/7 summaries/UTF16 boundary match frozen source|runtime/tests/derived_contract.rs; actual Shell success/error/read prompt/cwd results additionally exercised by owner tests|
| A07/A08 live run and graph persistence |real startup, failed launch without run, PID/run start, success/error/needs-input/idle, delayed OS cwd capture, summary, natural exit7, new PID/ordinal; shutdown recovery interrupts old active work and disabled history does not collect|runtime/tests/owner_contract.rs (2); runtime Saved callbacks verified against journal independent of credit/replay; no visible GPUI acceptance yet|


## Initial native window results — N02r/s

This does not close a whole capability group. Images under `artifacts/native-parity/native/evidence/` bind to the isolated build record; source/automated results are detailed in STATUS.md.

| Scenario | Native actual result | Evidence / differences still open |
| --- | --- | --- |
| A01.01 | Real GPUI initial Shell window, native runtime and zsh prompt; no browser runtime | Native screenshots and desktop bridge contract. Source-derived dimensions present; layered sidebar, header shadow, title truncation, icons, menus and AX remain incomplete. Same-state pixel comparison not passed. |
| A07.01 | Chinese paste executes; selected Chinese text copied and pasted back into real prompt | CUA observed `COPY_中文_123`; A07-native-before-quit.png includes later completed marker. Basic text only; IME candidate composition probe inconclusive, full key/mouse protocol coverage pending. |
| A03 resize | Actual native window resized to 1080×730; card and terminal reflow; input remains usable | CUA observation and before-quit screenshot. Animation/anchoring/timing comparison and geometry persistence remain open. |
| A08.01 / D01 | Normal Cmd+Q exits process; same data reopens with completed Chinese output, restoration divider and new prompt | A08-native-after-reopen.png; real bridge test additionally proves further input after restore. This closes visible completed-history restoration for this simple Mac Shell case, not full crash/Agent/large-history recovery. |
| Output confirmation | Initial pre-output frame releases UI slot without sending invalid credit ACK; later frames arrive without further input | bridge_contract exercises split real zsh output, continued command and shutdown/reopen. No mock. |
| Windows desktop | Native desktop, core and runtime branches type-check for x86_64 MSVC | No Windows desktop executable/link/install/real UI result in this batch. |


## Canvas and multi-Shell results — N02t/u/v

| Scenario | Reference baseline | Native actual result / evidence | Remaining difference |
| --- | --- | --- | --- |
| A02/A03 create canvas | Frozen SessionCanvasService createCanvas; numbered default titles, pinned-name conflict, receipts and navigation | 16-operation creation contract (includes sibling cases), 19-table source snapshots; B02-native-create-canvas.png proves immediate Chinese input after top plus | Full loading/error/disabled treatment and complete tab controls open |
| A03 horizontal Shell | Frozen SceneTabBar control → createShellSibling; source-created Shell receives focus | B01-reference-create-shell.png vs B03-native-create-shell.png; source transaction contract, real three-PTY bridge | Remove-node control, source motion/shadows and pixel alignment open |
| A03/A08 select/focus | Frozen activateScene/setFocusedSession and 12-operation source fixture | B04–B06 native card/old-tab/return evidence; independent marker input; saved focus survives navigation | Scrolling motion/anchoring, narrow layout, full keyboard navigation open |
| A07 background Shell | Existing canvas hides views while owner retains the sessions | canvas_bridge_contract releases old command only while new canvas is selected; observes its real output, preserved env and no cross-session markers | Full background restart scheduling and UI performance evidence open |
| A08 multi-Shell reopen | Source persistent navigation plus D01 completed-command restoration | B07-native-reopen-canvas.png shows active canvas, first-card focus, both histories and a fresh executed marker; actual app quit/relaunch | Full crash, Agent, large-history and window geometry cases open |

The screenshots are functional evidence, not a passed same-state pixel comparison. All referenced images and canvas-build.json are under artifacts/native-parity/native/evidence; B01 is a reference-app image captured there for this paired sequence.


## Carousel motion and geometry — N02w/x

| Scenario | Frozen reference | Native result / evidence | Remaining difference |
| --- | --- | --- | --- |
| A03 card selection motion | Actual CSS browser samples: width380ms/lift180ms, two interrupted reversals | `carousel_motion_contract`; actual paint traces, C01/C02 screenshots and real post-switch input | Full matched-state frame/pixel comparison not complete |
| A03 ordinary hover | D15/D16 isolate preview/return without edge intent | D01–D07 show expansion, unchanged keyboard input target and restoration | Horizontal gestures and all interruption combinations not accepted |
| A03 edge browse | D08–D10/source: dwell180ms, browse900ms, keep position on leave | D17/D18 native dwell/leave demonstrated | Indicator timing/pixel comparison still open |
| A03 narrow card | Frozen source compact summary styles and widths | Native compact summary renders/activates; ordinary terminal input works after selection | Full narrow state/resize pixel matrix open |
| A03/A16 canvas anchor | Frozen geometry repository fixture and source saved root-group geometry | `geometry_contract`, SQLite `geometry-before-reopen.json`, D07 and D11/D12 tab/quit/reopen | Nested groups/DAG/window geometry not yet connected |
| A16 geometry write failure | Source latest-write retry120ms, up to3 retries | Real SQLite trigger failure then same-value success129.1ms, D22/`geometry-transient-failure.json` | Latest build exact-value retry127.4ms passed; delayed-focus/tab switch verified with a real DB lock; sub120ms failure/reentry ordering source-reviewed only |


## Canvas tab operations — N02y (in progress)

| Scenario | Frozen reference | Native result / evidence | Remaining difference |
| --- | --- | --- | --- |
| A02/A03 rename/reorder/close storage | 18 actual TypeScript service operations and SQLite snapshots | `scene_tabs_contract`; same results, errors, rows and events | Delayed-exit bridge and real UI disposal passed; complete Agent disposal remains open |
| A03 rename UI | E01/E02, live HTML input undo/word editing | E04–E07/E17–E19: Chinese names, duplicate/FEFF trim, 20-unit input, selected-word undo and redo | Genuine IME, complete focus traversal and pixel comparison open |
| A02.05/A02.08 close UI | E03 and earlier original close/last-tab captures | E08–E10; background tab removed, retained Shell takes input; sole tab preserved | Slow-exit concurrency/run persistence pass in bridge; E20/scene-tabs-final-exit.json confirm real UI result; multi-task UI open |
| A03 overflow tabs | E13/E14 hidden list and selection | E11/E12/E15/E16 visible menu, selection, width and long-title ellipsis | Full keyboard/unread/disabled/pixel states open |
| A03 modal interaction | Original input Tab → cancel → confirm → underlying new-workspace | Native scroll/right-click occluded; disabled confirm skipped | N02z corrects rename trap and preserves separate confirmation cycle; full app focus parity remains open |

| A03/A08 slow closed Shell | Original private runtime disposal retains normal run-exit recording and does not serialize other session input | Real HUP-ignoring zsh bridge: retained input completes before closed PID exits; run exited + code/signal persisted unchanged across reopen. E20 + scene-tabs-final-exit.json show same user route in native app | UI pixel/timing equivalence of structural removal versus event/RPC projection still open; no Agent/multiwindow claim |


N02z continuation (A03/A08/A16, evidence `native/evidence/tab-focus-build.json`):

| Requirement | Frozen reference | Native observation | Remaining |
| --- | --- | --- | --- |
| Rename Tab/Escape | Input → cancel → confirm → background; Escape on cancel keeps modal | F03/F05 plus real traversal; background terminal executes with modal open | Missing full-app controls and genuine IME |
| Confirmation focus | Close → cancel → confirm cycle, Escape cancels | Real forward/reverse traversal and Escape retain canvas | Full pixel comparison |
| Canvas keyboard | Modifier+T/D/[ / ], modifier+Shift+arrows/brackets, numeric tabs | F03/F06; reorder persisted across native relaunch | Full platform key/repeat and read-only states |
| Button focus after navigation | Existing button retains focus; output only restores unclaimed active terminal | F08/F09 current/different tab, F10 delayed-output recovery | Full app/window/AX behavior |
| Context and overflow focus | Context does not autofocus; Escape dismisses it. Overflow keeps trigger focus and ignores Escape | F11 reference AX; F12/F13 final native | No complete menu pixel claim |
| Focus indicator | F07 frozen Mac cancel outline | F14 separately painted outline, transparent interior retained | Exact geometry/color and other platforms remain unverified |


## Sidebar task operations — N02aa (in progress)

Evidence base: artifacts/native-parity/native/evidence. Frozen logic: task-lifecycle-reference.json (33 steps), session-interaction-reference.json (9 steps).

| Requirement | Frozen reference | Native observation | Remaining |
| --- | --- | --- | --- |
| A02.02 create/select | New task owns independent canvas and Shell; selection preserves prior Shell | G04 creation; G05 reference vs G09 native direct terminal input; actual multi-PTY bridge | Full multi-workspace/read-only/invalid-path UI |
| A02.03/A02.04 menu/rename | G01/G02; 20-unit input, duplicate/empty rejection and toast | G06/G07/G13/G14 real Chinese rename, duplicate and empty state | Full modal focus/IME/pixel matrix |
| A02 pin and order | Frozen listTasks/pin/reorder and interaction transactions | G08/G11/G12 pin, recency and drag end state; G20/G21 final create/switch/MRU | Full drag trajectory/timing open; rapid-input/focus/shutdown real-zsh regression passes |
| A02.07 delete | G03 confirms sessions removed but local files kept | G15/G16 other task remains usable; real bridge retires all child sessions | Agent/worktree/window-specific disposal when those features are connected |
| A02 last-task deletion | Frozen deleteTask creates default replacement | G18 executes cat in replacement Shell; exact fixture and bridge | Full pixel/loading/error feedback |
| A02/A08 quit/reopen | Persist task/focus; approved D01 restores completed commands | G19 plus task-reopen.json, old process exit and fresh live output | Full app lifetime/recovery/Windows |
| A02 sidebar visibility | H01 active title span nearest; original 4px overflow scrollbar | H10/H19 corrected title reveal; H18 collapse removes scrollbar | Full sidebar visual/drag/gesture states remain open |


N02ab continuation (A02; evidence `native/evidence/sidebar-build.json`):

| Requirement | Frozen reference | Native observation | Remaining |
| --- | --- | --- | --- |
| Long-list active-title reveal | H01, source activeRef span scrollIntoView(nearest) | H10/H19 title aligns to viewport; row may remain partly clipped | Full multi-workspace states |
| Thumb size / content extent | H03/H04 28 tasks, 4px track, real top/click extents | H14/H15 after including group margins | Full pixel/color comparison and minimum-thumb/system preference variants |
| Hold paging and focus | H08 hold800ms reaches bottom | H16 same settled extent; HOLD_RELEASE_OK without terminal refocus | Intermediate timing, leave/reenter and outside-window release not demonstrated |
| Option track placement | H09 and 29-item H20; pointer at screen y600 | H21 final 29-item thumb bounds [455,744) match H20; H22 real input | Option document dragging, alternate system preferences |
| Thumb/wheel input | Reference scrollbar real track paths plus Chromium source | H13 initial thumb drag to top and wheel over track | Exact wheel/gesture distance and continuous trajectory remain open |
| Pinned-task drag | Source draggable state and real settled original ordering | Existing G12 native end-state only | H06/H07 capture failed; no intermediate animation acceptance |


## N02ac workspace/path integration (partial acceptance)

| Scenario | Frozen source/reference evidence | Native evidence and current result | Still open |
| --- | --- | --- | --- |
| A01 workspace domain lifecycle | Original service 31-step, 23-table fixture; I01–I03 original UI | workspace_contract; create/activate/pin/reorder/remove/relink/replay and reopen match | Entire hierarchy scope not complete |
| A01 directory validity | Original path service 7-step real-filesystem fixture | workspace_path_contract; generations/events/receipts match | Windows actual filesystem behavior |
| Create and immediate input | I01 original creation/default Shell | I05 failed focus attempt retained; I06 direct input succeeds | Exact picker/window/keyboard parity |
| Switch and keep Shell | Source workspace focus/MRU rules | I07 old variable preserved; real multi-PTY bridge | All window combinations |
| Pin and remove | I02/I03 original menu/confirmation | I08–I11, DB and file preservation; source fallback navigation retained | Pixel comparison, pinned drag trajectory, cancellation/focus details |
| Live path loss | I04 original warning and disabled creation | I16 final build retains content and blocks typed input; initial hover panic fixed | Exact warning pixels and disabled IME; clipboard now verified in J02/J07 |
| Relink and restart | I05 original relink/new Shell | I13/I14 initial relink; I17 final relink clears old env, retains history and accepts direct input | Feedback timing measurement and all failure/cancel states |
| Hidden scene restoration | Source TerminalPane revision + foreground-only surface | Real bridge proves old background run survives until selection, then replacement | Actual multi-tab UI side-by-side comparison |
| Invalid switch-away/return | Source projected state transitions | Real bridge test verifies same-session/new run on return | UI sequence timing |
| Quit/reopen workspace | Source persistence contract | I15 final build retains pinned workspace, relinked root and history | Invalid-startup failure/retry now verified in J03-J05/J08-J13; full recovery scheduler and loading animation open |
| Finder opening | Frozen main revealDirectory uses shell.openPath | Final native action opens 项目甲-final containing KEEP.txt, observed via Finder AX | Windows Explorer equivalent |

I01–I17 refer to artifacts/native-parity/native/evidence, whose original-reference and native filenames are distinct. workspace-build.json records staged versus final screenshots. These captures do not constitute full pixel comparison. Overall node remains in progress.


N02ac invalid-startup follow-up: J02 original AX and J07 native visible rename field both read `COPY_WHILE_INVALID` after selecting/copying a disabled terminal. J06 was an unsuccessful initial attempt and is not acceptance. Frozen original J03 fails startup on unavailable root; J04 preserves failure after root returns; J05 succeeds only after retry. Native J08–J11 establish the same sequence, and final J12/J13 include corrected card rounding/footer plus direct input after retry. Real-zsh workspace_bridge now covers failed retry, no auto-start even after navigating away/back, same-session new run and dropping blocked inputs. This closes these specific behavior gaps; exact failure surface pixels, complete loading water animation, disabled IME and Windows behavior remain open. Source/binary binding: manual-recovery-build.json.


#### A01/A02 workspace drag follow-up (N02ac; still in progress)

| Scene | Frozen baseline | Native evidence | Remaining |
| --- | --- | --- | --- |
| Pinned workspace title drag | TaskSidebar parent section draggable, source .6 opacity/.99 scale, 140ms ease, retained last target; K01/K07 attempts | Final K15 + workspace-drag-order.json persist title-initiated reorder; previous order survives restart | Reference real drop did not complete reliably; ghost, scale, intermediate trajectory/timing and full pixel comparison open |
| Invalid drop / cancellation | Source dragend clears state | Final K12 shows terminal input after outside-sidebar drop; source IDs reset when GPUI has no active drag | Esc-mid-drag, subsequent pinned-task drag and clipped-target path are code-reviewed only; K13 used unpinned tasks |
| Ordinary buttons after title-event change | Existing button focus/activation; source active header transparent | K14 menu opens; normal buttons focus then stop, only title bubbles | Full focus matrix and menu pixels open |
| Disabled-path IME | J02 copy accepted separately; OS IME behavior required | Input-method shortcuts did not expose candidate window | No IME acceptance; continue real candidate-window verification |


Workspace ghost follow-up: full-group snapshot and scaled source rows/controls are implemented. `workspace-drag-reference-frames.json` captures actual reference drag transition with fixed layout (247x70, scale .99, opacity .6); its system drop landed outside the sidebar, so original completed reorder remains open. Final native L04 plus `workspace-ghost-build.json` prove persistent second-to-first order; L05 proves subsequent ordinary collapse. L03 is an endpoint-only source-section attempt. Native passive traces have no scaled frames, so full-group ghost, pointer anchor, .99 scaling, trajectory/duration, Esc/keyboard-during-drag and pixel parity are still NOT accepted. Source mouse-up activation suppression and current logical placeholder height are code-reviewed only for the unobserved paths. Mac/Windows compile, format and identifier gates pass; no Windows live evidence.


N02ac header/menu geometry follow-up: `workspace-menu-reference-style.json` and final `workspace-header-build.json` match first/second group bounds exactly (x6,y57/99,w247,h34/70) after adjacent-margin collapse correction. M06–M09 show final fold/menu/escape paths and rest-state headers. Default badge line height and active gradient/glow are implemented, but final pixel equality is NOT claimed. Two reference menu crops are pixel-identical; M07 native residual diff still shows text/edges. M04/M05 confirms dialog panel/button placement and wrapping only; typography remains different. Measured browser fonts and standalone CoreText probes identify the default CJK fallback/advance mismatch for next investigation. No font exemption, screenshot mask or relaxed threshold was introduced. Final Mac/Windows compile passes; drag process, invalid-label visual states, IME and Windows live acceptance remain open.


N02ac UI font follow-up: actual GPUI shape_text with explicit fallback runs now matches reference CJK advance (13px title/menu, 14px body); Latin retains primary system font. Final N03/N04 compare menus/dialogs; ui-font-pixel-diagnostic.json retains glyph/baseline/edge residuals, so pixel parity remains open. A real GPUI hidden-window layout probe verifies non-ASCII ellipsis at whole grapheme boundaries (Chinese/combining accent/ZWJ emoji), while final N08-native-long-title-final.jpg and N07-native-long-title-verified.jpg versus N05/N08-reference-long-title-verified.jpg establish the same visible 18-character-title truncation in actual windows. N06 is not acceptance. No tolerance/mask or reference logic changed. Final sources/bundle bind to ui-font-build.json; Mac build and Windows compile pass. Task-title weight, all remaining font raster/baseline differences, full UI font coverage, held drag, IME and Windows live behavior remain unaccepted.


N02ac Medium follow-up: actual reference platform font for CSS500 is PingFangSC-Medium. Native font-kit selection incorrectly resolved500 toRegular because Apple Medium is exposed as530. UiText now corrects that request for both primary/fallback faces and uses actual shaped widths for ASCII Medium ellipsis. O04 is the final real window; hidden GPUI probe and Mac/Windows compile pass, bound in ui-medium-build.json. Full text pixels remain open. Pinned Chromium source and live range/paint measurements distinguish normal from explicit line-height. O03 device-pixel baseline snapping improved the task title but regressed the workspace heading, so that entire experiment and instrumentation were removed; it is NOT final or accepted evidence. No masks/tolerances/reference product logic were changed.


N02ac device-scale typography follow-up: actual LayerTree drawTextBlob evidence plus pinned Chromium zoom/font sources establish that metric/leading/origin rounding occurs after device-scale zoom. Mac single-line UiText now matches that rule, with normal height for workspace headings/menus and explicit height for task titles. Final P08/P09 and ui-device-baseline-build.json bind actual native results. Five local glyph-edge diagnostics have best vertical shift0; CUA JPEG captures and remaining material/raster differences do NOT prove pixel equality. Hidden GPUI layout also verifies that a hidden Chinese suffix still contributes to normal height after ASCII ellipsis, and retains whole-grapheme fitting. Mac build, Windows cross-check and gates pass. Multiline text, other font coverage, full raster/material equality, held drag/IME and Windows execution remain open; no tolerance/mask/reference logic changed.


N02ac confirmation paragraph follow-up: Mac confirmation text now uses per-visual-line device-scale baseline positioning and pre-line whitespace, with normal metrics for shared dialog headings/buttons. Actual hidden GPUI shape/layout verifies reference wrap and baselines743/788; existing single-line regressions pass. Final real Q05/Q06 and ui-dialog-build.json bind the same displayed workspace-removal confirmation. Five local glyph-edge comparisons prefer shift0, but JPEG/raster differences prevent pixel equality acceptance. Repeated center-menu clicks failed to activate reliably, while a right-side click succeeded; this is a new unresolved interaction issue, not waived as tooling noise. Q02/Q03/Q04 are failed menu attempts. Mac build, Windows compile and gates pass; task/canvas confirmations share implementation but have no fresh visible acceptance. Full interaction, raster/material, IME/drag and Windows runtime acceptance remain open.


N02ac menu diagnosis correction: actual center-click handler/open_remove/render telemetry and R03 prove activation works at the previously suspect location. Earlier old-screen observations are now a presentation/capture-timing issue, still unresolved, not accepted. Actual frame callbacks and Visible occlusion state reject a blanket hidden-window explanation. See menu-presentation-probe.log/patch and R01–R03; white partial captures are not parity evidence. Temporary probes fully removed, product source unchanged from e3e5741. Next inspect actual Metal submission versus capture freshness; no refresh workaround added.


## A07 terminal selection — N02ad

Evidence base: `artifacts/native-parity/native/evidence/`. Final source/bundle binding: `terminal-selection-build.json`.

| Scenario | Reference operation/result | Native actual result | Evidence and remaining boundary |
| --- | --- | --- | --- |
| A07.03 | Double-click `beta-gamma/path` selects full token including slash/hyphen | Same range; Cmd+C then Cmd+V pastes the exact token | S01 reference, S03 pre-fix failure, S04/S05 final; no pixel/latency claim |
| A07.04 | Triple-click mixed ASCII/Chinese output selects entire line | Same line selection; copy/paste retains Chinese and all text | S02 reference, S06/S07 final; pasted text cleared without execution |
| A07.05 | Double-click on continuation of a two-row word selects both rows | Same start/end and row break boundary | S09 native / S10 reference; screen-external continuation remains incomplete |
| A07.06 | Character, word and line selection retain their distinct drag boundaries | Character drag real-window regression passes; 46 actual-source probes cover word/line ranges and copied text | S08, terminal-selection-tests.log; held multi-click drag trajectories and automatic screen-edge scroll remain unverified/unimplemented respectively |

Mac build and Windows compile pass. The IME shortcut attempt did not open candidates in either product; it is not composition acceptance. No threshold, mask, source expectation or reference product behavior was changed. A07 as a whole remains open.


| Scenario | Frozen baseline / operation | Native evidence | Remaining gap |
| --- | --- | --- | --- |
| A08.04 / N02ae — saved history during storage fault | pty-session.ts parses protocol before write and publishes renderer data only after successful persistence. With saved multi-screen output, force a real journal-index write error, scroll and copy old text. | Old T01/T02 stuck; final T03/T04 scroll to earlier history during the error; T05 copies SAVED_HISTORY_001 into an independent Shell. Real PTY contract covers all accessible viewports, hidden uncommitted state and exactly-once retry. T07 verifies normal output after restored files and restart. Source/build/launch: committed-state-build.json. | Reference live fault UI not newly captured; native storage retry UI missing (contract only). No screen-external selection/edge scrolling, full timing, pixel or Windows runtime acceptance. Extra parser/history cost recorded separately, not an end-to-end pass. |

N02ad history access prerequisite: native absolute-row reads now preserve viewport and match rendered cells for Unicode, wraps and spaces (history-row-tests.log); Windows terminal compiles. Desktop cross-screen selection and edge auto-scroll remain unimplemented. This internal API evidence does not close A07/A08 interaction acceptance.

N02ad history-backed selection core: 46 unchanged source probes now pass against real terminal parsing, plus a multi-screen scroll/copy contract (history-selection-tests.log). Windows terminal compiles. No GUI switch yet; old desktop algorithm is transitional until shared-core wiring, stable anchors and edge scrolling are implemented. No additional A07/A08 UI acceptance claimed.

N02ad anchor experiment: direct libghostty tracked references rejected. Under actual2–4-region scrolling, reference selects C at row1 while native ref moves to A at row0. Logs and reproducible patch: selection-anchor-{reference,native}.log and selection-anchor-probe.patch. No production change; tiny-history native trim remains unobserved at bounded probe limit. Next implementation must preserve row semantics and obtain actual trim events separately.

N02ad trim accounting prerequisite: real engine now reports actual front-history removal without confusing active-region row edits. Contract covers automatic retention, continued output, CSI3J and unchanged counter for local scrolling/CSI L/M; existing11 terminal contracts also pass. See history-trim-tests.log/build.log. Buffer/reset lifecycle and actual GUI selection wiring remain open.

N02ad selection-state prerequisite: original46 pointer probes and6 groups/20 model operations match, including clipped starts, initial wrapped lengths and reverse endpoints. Real trim/local-scroll contracts pass. Same-batch alternate round trip and RIS are now observable lifecycle signals, verified on Mac; Windows rebuild/compile passes. Automatic lifecycle application and GUI wiring still pending, so A07/A08 remain partial.

N02ad owned selection: actual trim/epoch changes now reconcile automatically; real saved-buffer copy remains available during storage faults and rejects unpersisted content. Terminal owner3/state4/session8 contracts pass; Windows runtime compiles. Evidence selection-owner-tests.log / selection-owner-windows.log. Worker/UI frame integration, offscreen pointer selection and edge-scroll acceptance remain open; no additional A07/A08 UI acceptance.

| A07 / N02ad offscreen selection copy | Select a saved word; scroll it out of view; copy and paste in another Shell | Native CUA PID76297 copies exact SAVED_HISTORY_100 after viewport shows001–017. Shared selection now drives frame highlighting and async copy; terminal-only duplicate algorithm removed. Build binding selection-ui-build.json; CUA screenshots in task transcript | Partial: capture/presentation lag remains, edge auto-scroll absent, held-drag/timing/sync-output/Windows UI not accepted. |

| A07 / N02ad edge auto-scroll | Original50ms timer,1–15 lines by distance; update raw end after scrolling, stop at mouseup | Implemented window-capture drag and FIFO scroll+endpoint. Direct source13 velocities and real Worker100-line range pass. selection-edge-build.json and selection-edge-*.log | NOT interaction-accepted: CUA outside-move→release0.438ms prevents held-dwell validation; source speed/timer logic and worker results do not prove real trajectory. |

| A07 Shift incremental click | Retain start/initial word or line extent/mode, set raw pointer end; no start means no selection | Native core+surface implemented;8 source model groups/27 operations and46 prior pointer probes pass; Mac/Windows compile | Real comparison unaccepted: unrelated mousemoves occurred inside reference synthetic clicks. Passive observer removed; native launched build remains previous1c1da2e. |

| A07 Alt rectangular selection | Alt single drag in four directions, physical row copy, wide boundaries, zero width and edge scrolling | Shared column range and surface dispatch implemented; actual source12 probes/14 actions, terminal+worker9 tests and Mac/Windows compilation pass. selection-column-{tests,build,windows}.log | Partial: real Alt drag/highlight/copy, held scrolling and Windows runtime unverified; new binary not launched. |

| A07 / N02af terminal mouse reporting | Program enables ordered tracking/encoding; click/release/drag/wheel vs text selection | Core61 byte+15 wheel probes,20 mode steps; real native1000/1006 click/Shift-click/down-wheel produces exact SGR bytes in PTY. mouse-ui-build.json and mouse-pty-native-click.json | Partial: reference GUI comparison, drag/hover/pixels/other buttons/alternate wheel/Windows runtime and presentation timing open. DEFAULT remains internally encoded but not forwarded, matching frozen product onData-only subscription. |

| A07 / N02ag stale screenshot diagnosis | Continuous real PTY markers; expose, fully cover, expose independent native window |84 frame receptions while AppKit visible bit absent, zero paints; exposing resumes even when inactive.244 visible receive-to-paint samples median8.744ms/p9515.402ms. display-probe-build.json / display-probe-summary.json | Controlled cause identified, no runtime patch needed. Old uninstrumented captures and full reference/input-to-photon/drag acceptance remain open. Expose target and confirm fresh state before future GUI sampling. |

| A07 / N02ah live OSC52 clipboard | Active visible session emits write; inactive sender emits delayed write; replay/query/fault cases |32 frozen handler +8 actual parser streams; real PTY persistence/replay contract; both reference and native GUI paste Chinese text, background sender cannot replace it. clipboard-ui-build.json and clipboard-{reference-background,native-active,native-background}.png | Mac demonstrated active/background path passes. Replay/fault proven at real PTY level only; Windows runtime and multiwindow unverified. No clipboard-read capability added. |

| A08 / N02ai invalid regex (D02) | Frozen reference Cmd+F, `[` query, Cmd+R to enable regex | Entire workspace disappears; ordinary reload restores it. search-reference-invalid-regex.png | User explicitly approved native difference: preserve terminal/search UI, show invalid regex, allow correction. Implementation pending; valid regex/search behavior remains 1:1. |

| A08 / N02ai search foundations | Frozen retained-buffer engine and archive reader with source-created journals | 39 retained-buffer +51 archive probes pass, including UTF16/regex/whole-word/navigation and raw/gzip/middle-gap/paging/context; search-engine-tests.log, history-search-contract.log | Core implemented only. No search UI/worker integration, visible highlights/navigation/archive behavior or Windows execution acceptance. D02 error UI pending; signed selection integration and archive malformed-input differences remain open. |

| A08 / N02aj live search UI | Cmd+F, retained history query, Enter/Shift+Enter, regex toggle/correction, Escape then Shell input | Native GUI1/121→2/121; regex '[' field error preserves workspace, corrected regex1/2→2/2; final input-scroll fix displays SEARCH_SCROLL_OK without manual scrolling. search-native-{invalid,recovered}.png; search-ui-{observed-build,build}.json.13 terminal contracts+real PTY search pass, Mac build/Windows check pass | Demonstrated native behavior only. Full reference pixel/interaction comparison, archive fallback/view, overview ruler and Windows runtime pending. D02 recovery implemented; invalid-query output tested at PTY layer. |


| A08 / N02ak archived search | Query beyond the retained buffer, cycle archived matches, inspect context, Esc and live input | Frozen reference and native isolated windows show two results and anchored context; native archive-search-{first,next,return,return-input}.png; actual raw/gzip/gap history worker contract passes | Partial. Production-handler timing probe verifies an in-flight old page after a newer retained miss, Esc rejection and same-cursor replacement. Final Mac build/Windows check pass; full pixel/focus/scroll and Windows execution open. |

| A08 / N02ak history selection and copy | Double-click word, triple-click line, drag across two Unicode/space-rich lines, copy and paste into a real PTY | PID74351 visible selection, real paste captures; reference/native archive-copy-multiline.json bytes equal; native full line matches reference first row. archive-copy-observed-build.json | Demonstrated Mac word/line/multiline path passes. Held edge-scroll, complete boundary semantics, selection appearance and Windows remain open. |

| A08 / D03 changed query with no matches | Old archived context is open, change to a nonempty term absent from all logs | Frozen reference shows 0/0 and retains previous history text (archive-search-stale-after-empty.png); user explicitly chose 保持原样 on 2026-11-16 | Preserve the frozen behavior, including completion of an already loading old context. This is not permission to change empty-result behavior or broaden D02. Final native PID29169 shows the same retained history and 0/0 (archive-search-d03-empty.png); Esc then displays D03_RETURN_OK in the live Shell. archive-final-ui-build.json. |


| A08 / N02al archive drag scrolling | Hold a text selection just inside a history scrollport edge; scroll and extend the copied range; release/Esc/replace context | Frozen Chromium150 source geometry matches42 cases at1x/2x. Hidden actual GPUI layout + production frame step scrolls38px over4 frames, selected/copied rows1–9→1–10,159 bytes without line numbers; stop/replacement hold. Native PID50792 real ordinary drag visibly selects rows, Esc allows HISTORY_DRAG_RETURN_OK. history-drag-{source-contract,probe,context-regression,build,windows}.log; history-drag-build.json; history-drag-return.png | Implementation and automatic contracts pass. Held dwell/physical trajectory, asynchronous OS focus stop, broader header/footer/boundary selection and complete appearance are NOT accepted. CUA endpoint release cannot validate sustained scrolling. Windows compile only. |


| A08 / N02am history panel selection | Drag body→heading/footer, preserve intervening hidden records and labels, copy, then return to live | Reference DOM anchors/text captured; nativePID56544 real heading/footer highlight, exact49-character/87-byte footer paste (LF→CR), Esc and return button pass. history-panel-build.json / history-panel-heading.png / history-footer-paste.json; extended hidden GPUI probe passes | Partial: initial inactive drags and reference clipboard attempt rejected. Newline-width highlight, gap hit testing, empty-line live copy, italic/text/material pixels, held-dwell/focus timing and Windows live remain open. |


| A08 / N02an history selection appearance | Select the last archived row through the boundary hint and footer | Frozen block-end space extension + Chromium0.25 synthetic italic; pinned GPUI macOS patch replay/probe pass; nativePID58655 displays slant and extension, copies exact49 chars/87bytes matching reference DOM selection after LF→CR. history-appearance-build.json, comparison.json, paired JPGs and italic-comparison.png | These visible corrections and native clipboard pass. Exact pixel parity remains open: lossy captures, fractional card origins and remaining selection-color/material differences. Reference clipboard, empty rows, precise gap endpoints, held dwell/focus and Windows runtime remain open. |


| A08 / N02ao history color and focus | Select last record through footer; deactivate the window; focus the search field | Frozen Chromium/AppKit color path and current-palette probe match; nativePID27618 retains gray range while inactive, finalPID59447 shows blue range then clears it on search focus without changing context/query/count. history-color-build.json, history-color-final-active.jpg, history-color-inactive.jpg, history-color-search-focus.jpg; paired reference evidence | Demonstrated Mac paths only. Final-build inactive attempt did not switch OS activation and is rejected; earlier unchanged-color implementation evidence is retained. JPEG/fractional origins prevent exact pixel acceptance; held-drag focus/timing and Windows execution open. |
| A08 / N02ao reference footer clipboard | Actual Cmd+C from matching selected record, boundary hint and footer; capture the subsequent terminal paste | Reference history-color-reference-paste.json exactly equals native history-appearance-paste.json:49 characters,87UTF8 bytes, including standard paste CR separators | Demonstrated reference/native clipboard range closes the earlier reference-copy gap. Empty-record live copy and precise gap endpoints remain open. |


| A08 / N02ap overview-ruler audit | Search the actual retained terminal and inspect the configured renderer | Frozen source never sets overviewRuler.width; default is empty; current live DOM has 3 search decorations and no ruler canvas. search-ruler-live.json | Earlier ruler TODO was an inventory error. No feature is added; actual scrollbar still required. |
| A07 / N02ap terminal history scrollbar | Hover, click track, drag thumb, then wheel/input/search | Native geometry and absolute worker command implemented; 13 source geometry cases/182 source positions, real-PTY FIFO/selection, request-tagged bridge and seven hidden GPUI handler checks pass. Mac build/Windows cross-check pass; terminal-scrollbar-build.json | In progress. Built binary not yet launched; Pointer-interfered reference click rejected; real drag/fade/focus and exact pixel acceptance still open. Empty-row copy remains inconclusive despite equal files. |

| A08 / N02ap late-frame and hide follow-up | Drag then release in bottom padding before the final view reply; independently wait after a reveal without another draw | Same frozen terminal scroll behavior. Reply suppression and one-shot deadline; production-handler probe covers release→reply and actual650ms wait. Final Mac build and Windows cross-check pass. NativePID41737 track click visibly changes retained history; terminal-scrollbar-final-build.json binds sources, bundle, logs and screenshots | Automatic ordering/timer contracts pass; only live positioning response established. Hover/thumb visibility, held drag, timing/pixels and clean immediate input remain open. Preedit/paste-control anomaly recovered with Ctrl+C; next diagnosis required. Windows runtime not tested. |

| A07 / N02aq IME live position | Start marked text while viewing historical output, cancel or commit | Source requires return to live output. Native handler implementation and hidden probe cover marking, cancel/no bytes, commit once and pending scroll. Mac build and Windows cross-check pass; ime-scroll-radius-build.json | Real input returned to bottom, but current attempt does not prove OS marked-text callbacks. Full actual cancel/commit path remains open; earlier escape/paste anomaly not declared fixed. |
| A07/A16 / N02ax–az runtime recovery and card feedback | Frozen automatic child restart/backoff, old-port rejection and card-local recovery before restored terminal visual readiness | Supervisor/tempchild contracts and hiddenGUI source-generation reset pass, source water16frame numerical contract passes. Separate native diagnostic window shows candidate water/input/pause/removal. `lifecycle-10` through `lifecycle-16` evidence | In progress: no actual product crash/recovery acceptance yet. Frozen150 Canvas10samples obtained; all5reference repeats exact. Readback stage isolated (`gate36`): control colors 127→128 under both frozen CPU roundings, alpha unchanged, so the LSB gap is readback, not raster; remaining fluid alpha differences stay 50,054/8,953/30,400. Root measured the CG candidate distribution (657,789/1,344,000 pixels exact; 95% within 1–3/255; alpha>3 only 1,859). **User decision 2026-11-19:** accept "not visually distinguishable" — geometry/color/timing match, single channel ≤3, alpha ≤3, edge coverage ≤1px, plus paired screenshots for the user; full data retained. Atlas key retirement passed; physical presentation/first-frame tails, mounting and real crash recovery remain open. CurrentMatou package remains1cb0c3c. |
| A07/A08/A14/A16 / N02aw real desktop-child boundary | Frozen desktop manages independent runtime; retained-window sessions survive hiding, actual quit ends runtime, D01 restores completed Shell history | Real GUI61461→runtime61467→PTYs, unique data owner; hidden window99837 persists while7,059,097 output bytes committed; reopen samewindow/child/Shell, Chinese paste works; normalQuit cleans all processes/lease, same-data newGUI93161/runtime93165 restores completed markers. `lifecycle-08-review.md` / result; root reviewed source, restored image and final PID/hash/signature | Demonstrated Mac cross-process slice accepted. No automatic runtime supervision/reconnect, full backup/quit semantics, complete pixels/performance, multiwindow, externalCLI or Windows live acceptance yet. |
| A14.02 / N02as L01 Mac main-window hide/reopen | Frozen main close prevents destruction and hides only that window; A14.02 same Shell PID survives reopening | Real native window99141 hidden/reopened via LaunchServices with sameapp70411/Shell70502, fresh input works; hidden30s workload completed with5,903,253 new journal bytes. Second hidden normalCmd+Q ended app and bothShells; final normal relaunchPID30059. `lifecycle-03-review.md` / binding / output/quit results; root reviewed code and visible result | Demonstrated single-window Mac behavior accepted.1s Quit samples do not exclude subsecond activation; Dock click/menuQuit, multiple windows, standaloneIPC, clean-exit backup, all visuals and Windows live remain open. |
| A08 / N02ar archive empty-row clipboard freshness | Select original BEGIN→empty record→END in archive search; seed different clipboard sentinels before each real Cmd+C | Both real apps overwrite their distinct sentinels with identical raw 55 UTF-8 bytes: `BEGIN\n \nEND` (full markers in evidence), two LF, one middle space, no row numbers/trailing LF. Root inspected paired selected ranges and independently compared binaries. `ime-04-review.md`, `ime-04-comparison.json`, `ime-04-binding.json` | This demonstrated Mac copy scenario accepted; prior stale-clipboard ambiguity closed. Not whole archive selection, pixel, motion or Windows acceptance. |
| A07 / N02ar input during held scrollbar | Frozen pointer monitor and scrollbar callback retain capture and original drag base across programmatic return-to-bottom; stop on release/button change | Removed only input-path capture clearing. New production-handler regression failed on old code, passed after fix; root independently reran probe and identifier/whitespace gates. `native/evidence/ime-02-review.md`, `ime-02-result.json` | Mac build/Windows compile pass; new isolated nativePID16035 is bound by `ime-03-build.json`. Ordinary native drag changes history; reference clicks selected text rather than the track, so paired drag remains unverified. Genuine OS composition/held gesture, hide/reveal, full trajectory and pixel/timing acceptance remain open. Current CUA lacks independent hover/move and held-button typing; no waived scenarios. |
| A07 / N02aq visible scrollbar | Click track and drag the thumb in the real native window | Corrected GPUI radius from999px to1.5px for3px thumb. Visible thumb observed; track shows empty-record-fill-07774 and drag to1200px shows empty-record-fill-10288. scrollbar-radius-track-visible.jpg and scrollbar-radius-drag-attempt.jpg | Visibility and positioning demonstrated. Interrupted attempts rejected; held motion, exact fade duration and matched reference pixel/interaction acceptance remain open. |
| A07 / N02bc recovery water mounted | While a session is recovering (no current-generation frame yet) in a visible card, the card shows the frozen water overlay from 32px below its top (over the 42px header by 10px), input is unavailable, presses in the water area do not activate/focus the card, and the first real frame removes it | Per-session entity mount with the existing recovery gate; 12px bottom radius, light `fafcff` background, radial/edge layers, generation reuse, image release on unmount. Hidden production-path probe: 560×600@2 at (270,87), inset 32, mouse/keyboard assertions, 26 images created/dropped, frame cadence 16.5–18.3 ms after a 45 ms first static frame. Real process: child killed → new child 156 ms later, GUI and window alive, new lease/generation, no backup. `recovery-water-mount01` evidence | Mounting and the approved pointer rule accepted at probe level. Real-window screenshots, on-screen first-frame timing, physical presentation and physical input remain open (CUA/screen capture unavailable). |
| A14.01 / N02bb macOS delayed quit | Cmd+Q/menu/Dock quit hides the window immediately, keeps the main thread responsive, waits for the runtime to finish (including clean-exit backup) and only then terminates; repeated quit does not double-run cleanup | Delegate hook added at runtime (no vendor patch); `NSTerminateLater` + one-shot reply through a shared gate with GCD and main-run-loop entries; final `PutGeometry` queued before `Shutdown`. Isolated real runs: window offscreen ~82 ms, main thread `CFRunLoop`, exit ~26 ms after child; GPUI `cx.quit()` nested path verified; nonzero child exits app with status 0 and logs; 1 Shutdown/1 reply; geometry read back from live DB and backup (`quick_check=ok`). `terminate-01/REPORT.md` | This isolated macOS entry is accepted. Physical Cmd+Q/menu/Dock key events, supervisor panic, Windows runtime, multiwindow quit and full parity acceptance remain open. |
| A16 / N02ba clean-exit backup | On real product quit the runtime backs up before closing the database, with one 5s budget covering create+rotate and seven-copy retention; abnormal parent loss does not create a backup | Core stage plus runtime wiring: explicit child `Shutdown` marks clean before enqueue; sessions stop, then backup runs on the open DB, then owner drop closes SQLite and releases the lease; errors aggregate. Root reran runtime-targeted17/17 and core7/7. Real child+PTY tests cover explicit Shutdown (one snapshot, newest seven, committed rename+summary inside, standalone integrity, lease released), double Shutdown, EOF, decode failures, post-ready domain error, read-only backup dir. `clean-exit-core-review.md`, `process_contract.rs` | Runtime + real child/PTY contracts accepted. No GUI Cmd+Q/menu/Dock acceptance, no Windows execution; blocking FS calls and one backup step are not preemptible; two pre-existing runtime test targets do not compile and are excluded. |
