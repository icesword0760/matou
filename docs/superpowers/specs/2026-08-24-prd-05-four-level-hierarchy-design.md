# PRD 05 Four-Level Hierarchy Design

**Date:** 2026-08-24  
**Status:** Approved in chat; written-spec review pending  
**Scope:** PRD 05 only  
**Product stack:** Electron + React + xterm.js + Runtime utility process + SQLite + node-pty

## 1. Goal

Implement the complete user-visible hierarchy defined by PRD 05:

```text
Workspace -> Task -> Scene -> Session + SessionMount
工作区       事项      页签      终端
```

The hierarchy must remain predictable under switching, destructive actions,
path failure, process failure, application restart, terminal detachment, and
multi-window task migration. Runtime remains the authority. Renderer stores are
rebuildable projections and never export full authoritative snapshots.

## 2. Authoritative Sources

The implementation uses the following precedence:

1. This approved design and the already approved infrastructure design.
2. `05-四级层级管理.md`, especially sections 4.2 through 4.14 and section 9.
3. Kooky behavior and data flow where the PRD is silent or internally inconsistent.
4. Existing Matou infrastructure constraints.

Primary Kooky evidence:

- `src/modules/terminal/stores/project.js`
- `src/modules/terminal/stores/workbench.js`
- `src/modules/terminal/stores/tab.js`
- `src/modules/terminal/stores/panel.js`
- `src/modules/terminal/components/ProjectDropdown.vue`
- `src/modules/terminal/components/ProjectSidebar.vue`
- `src/modules/terminal/components/WorkspaceTabBar.vue`
- `src/modules/terminal/components/ClaudeCodeView.vue`
- `src/modules/terminal/utils/projectSessionLifecycle.js`
- `src/modules/terminal/utils/projectPathStatus.js`
- `electron/project-defaults.js`

Existing Matou mapping, already fixed by the infrastructure design:

| Kooky | Matou authority | User-facing term |
|---|---|---|
| Project | Workspace | 工作区 |
| Workbench | Task | 事项 |
| Tab | Scene | 页签 |
| Panel | Session + SessionMount | 终端 |
| layoutRoot | SceneNode tree + SceneGeometry | 分屏布局 |
| terminalId | SessionRun | 终端进程 |
| claudeSessionId | ProviderBinding | AI 会话身份 |

## 3. Scope

### 3.1 Included

1. Workspace create, switch, rename, remove, default bootstrap, path validation,
   path recovery, and empty-workspace-list state.
2. Task create, switch, rename, reorder, delete, duplicate-name validation,
   default creation, active-item visibility, and per-window placement.
3. Scene create, switch, rename, close, keyboard reorder, overflow list, active
   Scene visibility, and title fallback.
4. Session creation, mixed Shell/Agent Sessions, terminal deletion, focus, and
   strict cwd inheritance from the Workspace execution context.
5. Arbitrarily nested horizontal and vertical split trees, draggable dividers,
   independent Scene layouts, and geometry persistence outside Outbox.
6. Per-window hierarchy focus memory and restart restoration.
7. Hide-to-tray protection for the last Scene of the last Task.
8. Terminal detachment to a native BrowserWindow, placeholder rendering,
   process-preserving return, and restart normalization.
9. Script-triggered whole-Task migration between main windows with rollback.
10. Exact destructive-action semantics and product copy selected in section 5.
11. Runtime-side enforcement of path-invalid read-only behavior.

### 3.2 Excluded

- Notification generation and notification visual language beyond stable hierarchy
  attachment points; PRD 01 owns the feature.
- HUD field definitions; PRD 02 owns the feature.
- Task progress/status/log presentation; PRD 03 owns the feature.
- Provider-specific restoration policy; PRD 04 owns the feature.
- Session forking and DAG creation; PRD 06 owns the feature.
- Agent Team orchestration.
- Browser/editor panes.

PRD 05 still exposes stable active Workspace, Task, Scene, Session, cwd, and
window-placement projections for those later modules.

## 4. Architecture Decision

Use a Runtime-authoritative vertical slice.

```text
React UI
  -> RuntimeClient RPC command
  -> Runtime hierarchy application service
  -> one SQLite transaction + domain events
  -> RPC result and ordered Outbox events
  -> Renderer projection

xterm input/output
  <-> direct Renderer/Runtime MessagePort
  <-> node-pty + Journal
```

The application service owns multi-aggregate workflows. Repository methods remain
useful for single aggregates, but UI code does not orchestrate sequences such as
"create Workspace, then Task, then Scene, then Session". This prevents partially
created hierarchies after crashes or rejected commands.

## 5. Resolved PRD/Kooky Semantics

The source PRD contains a few statements that describe different revisions of
the same behavior. The approved product behavior is fixed here.

### 5.1 Default names

- User-created Task names follow Kooky and PRD acceptance item 28:
  `新事项`, `新事项 2`, `新事项 3`, selecting the lowest available suffix.
- System-created Task name is `默认`.
- The initial Scene has an automatic title. A manually pinned Scene title is
  unique among pinned titles in the same Task.
- The release-controlled default Workspace directory is `~/matou_workspace` and
  its display name comes from the same `matou_workspace` constant.

### 5.2 Three destructive entry points

They are separate commands and separate confirmation flows.

#### Delete Task from the sidebar

- UI term, tooltip, and aria-label: `删除事项`.
- If the Task contains at most one Session, show two dialogs:
  1. title `删除事项`; body `删除最后一个终端将连带删除对应事项，是否继续？`;
     buttons `继续 / 取消`.
  2. title `删除事项`; body
     `删除“{name}”会丢失该事项下所有终端会话，但不会删除本地目录。是否继续？`;
     buttons `确认删除 / 取消`.
- Otherwise show only the second dialog.
- Any cancellation leaves the hierarchy and processes unchanged.
- Successful deletion archives its Scenes and Sessions and terminates their live
  PTYs. If the Workspace path is valid and no Task remains, atomically create a
  `默认` Task, one Scene, one Shell Session, and one mount.
- This entry point does not hide the window.

#### Close Scene from the tab strip

- Last Scene of the last Task: hide only the current main window, preserve every
  entity and process, and show no dialog.
- Last Scene of a Task when another Task exists: show title `关闭标签`, body
  `关闭此标签会丢失“{task_name}”中的所有终端会话，但不会删除本地目录。是否继续？`,
  buttons `确认关闭 / 取消`. Confirmation archives the empty Task as well.
- A non-last Scene closes silently and focus moves to the next Scene, or the
  preceding Scene when the closed Scene was last in order.
- A Session-deletion cascade calls this workflow with `skipConfirm=true`.

#### Delete Session from a terminal header

- If it is the Workspace's final Session, show one dialog only: title `删除终端`,
  body `删除最后一个终端将连带销毁事项“{task_name}”，所有终端会话会丢失（本地目录不受影响）。是否继续？`,
  buttons `确认删除 / 取消`.
- Other Session deletions are silent.
- Deleting a Scene's final Session cascades through Scene/Task cleanup with
  `skipConfirm=true` and applies the default-Task rule.
- Programmatic lifecycle cleanup is explicitly marked `skipConfirm=true`.

### 5.3 Path-invalid presentation

Follow PRD 4.3.5 and current Kooky behavior:

- Workspace dropdown shows `路径失效` and a reason tooltip.
- Existing hierarchy and terminal history remain visible.
- The terminal area writes a clear `工作区目录不可用` state message.
- This release does not add a separate top-of-content warning banner.
- Runtime rejects new Task/Scene/Session creation, terminal input, split creation,
  Agent resume, and shortcut-command input for that Workspace.
- Error copy is
  `工作区目录不可用，请先在本地恢复原路径，或移出该工作区`.
- Runtime never substitutes another cwd.

### 5.4 Scene title

- Manual pinned title wins.
- Otherwise use `普通终端短名 + 当前工作目录`.
- Process/Agent title is confined to the terminal header and does not rewrite the
  Scene title.

### 5.5 Detached Session return

- The original Scene remains the logical owner and displays an `已脱出` placeholder.
- Closing the detached window returns the same live Session to the original Scene,
  placed to the right of its current active mount.
- Exact pre-detach slot restoration is not promised.
- If the original Scene/Task/Workspace was archived, return to the current main
  window's active Scene.
- Restart does not reopen detached windows; detached mounts normalize to attached.

## 6. Domain and Storage Changes

Add one schema migration after the current schema version.

### 6.1 Workspace path state

```sql
CREATE TABLE workspace_path_state (
  workspace_id TEXT PRIMARY KEY REFERENCES workspaces(id),
  status TEXT NOT NULL CHECK (status IN ('valid', 'invalid')),
  reason TEXT NOT NULL CHECK (reason IN ('', 'missing', 'not-directory', 'no-access', 'unknown')),
  checked_at INTEGER NOT NULL,
  validation_generation INTEGER NOT NULL
) STRICT;
```

This is a Runtime-derived cache. Changes emit `workspace.path-status-changed` so
connected projections update immediately. The authoritative path remains
`workspaces.root_directory`.

### 6.2 Scene title/order/revision

Add to `scenes`:

- `title_pinned INTEGER NOT NULL DEFAULT 0`
- `sort_key TEXT NOT NULL DEFAULT ''`
- `layout_revision INTEGER NOT NULL DEFAULT 1`

Create a partial uniqueness index for active pinned Scene titles within a Task.
Task title uniqueness is enforced for active Tasks within a Workspace. Values are
trimmed before reaching SQLite and comparison matches Kooky's case-sensitive rule.

### 6.3 Per-window navigation state

```sql
CREATE TABLE app_windows (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL CHECK (kind IN ('main', 'detached-terminal')),
  state TEXT NOT NULL CHECK (state IN ('visible', 'hidden', 'closed')),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
) STRICT;

CREATE TABLE window_navigation (
  window_id TEXT PRIMARY KEY REFERENCES app_windows(id),
  active_workspace_id TEXT REFERENCES workspaces(id),
  updated_at INTEGER NOT NULL
) STRICT;

CREATE TABLE window_workspace_focus (
  window_id TEXT NOT NULL REFERENCES app_windows(id),
  workspace_id TEXT NOT NULL REFERENCES workspaces(id),
  active_task_id TEXT REFERENCES tasks(id),
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(window_id, workspace_id)
) STRICT;

CREATE TABLE window_task_focus (
  window_id TEXT NOT NULL REFERENCES app_windows(id),
  task_id TEXT NOT NULL REFERENCES tasks(id),
  active_scene_id TEXT REFERENCES scenes(id),
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(window_id, task_id)
) STRICT;

CREATE TABLE window_scene_focus (
  window_id TEXT NOT NULL REFERENCES app_windows(id),
  scene_id TEXT NOT NULL REFERENCES scenes(id),
  active_session_id TEXT REFERENCES sessions(id),
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(window_id, scene_id)
) STRICT;
```

Navigation writes are serialized by Runtime and persisted without Outbox geometry
noise. The command response returns the canonical navigation projection. Initial
snapshot includes the requesting window's navigation state.

### 6.4 Task window placement

```sql
CREATE TABLE window_task_placements (
  window_id TEXT NOT NULL REFERENCES app_windows(id),
  task_id TEXT NOT NULL UNIQUE REFERENCES tasks(id),
  ordinal INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(window_id, task_id)
) STRICT;
```

A Task appears in one main window at a time. Workspace ownership never changes
during migration. A new Task is placed in the creating window.

### 6.5 Default Workspace tombstone

```sql
CREATE TABLE bootstrap_state (
  key TEXT PRIMARY KEY,
  value_json TEXT NOT NULL,
  updated_at INTEGER NOT NULL
) STRICT;
```

`default-workspace-created` and `default-workspace-removed` prevent the default
Workspace from silently returning after explicit removal.

## 7. Hierarchy Invariants

1. Each Task belongs to exactly one Workspace.
2. Each Scene belongs to exactly one Task.
3. Each Session belongs to exactly one Task and each visible terminal placement is
   represented by a SessionMount in a Scene belonging to that Task.
4. A non-archived Scene always has at least one logical SessionMount. A detached
   mount remains logical and carries a detached SceneWindow reference.
5. Session cwd is resolved from its ExecutionContext by Runtime. The Renderer never
   submits an arbitrary cwd for hierarchy-created Sessions.
6. Active focus references must point to non-archived descendants. Cleanup selects
   the nearest valid successor deterministically.
7. Task and pinned Scene names are unique in their documented parent scope.
8. Scene trees are replaced atomically with a compare-and-swap layout revision.
9. Geometry writes do not enter Outbox. Structural changes do.
10. Closing a Renderer or hiding a BrowserWindow does not imply terminal disposal.
11. Explicit hierarchy deletion terminates PTYs only after the metadata transaction
    commits; retry is idempotent and stale PTYs are recovered on Runtime restart.
12. A path-invalid Workspace preserves historical entities and rejects execution.

## 8. Runtime Application Service

Create `HierarchyApplicationService`. Commands use the existing command metadata
and deduplication mechanism.

### 8.1 Workflow commands

- `hierarchy.bootstrap-window`
- `hierarchy.create-workspace`
- `hierarchy.rename-workspace`
- `hierarchy.remove-workspace`
- `hierarchy.activate-workspace`
- `hierarchy.validate-workspace-path`
- `hierarchy.create-task`
- `hierarchy.rename-task`
- `hierarchy.reorder-task`
- `hierarchy.delete-task`
- `hierarchy.activate-task`
- `hierarchy.create-scene`
- `hierarchy.rename-scene`
- `hierarchy.reorder-scene`
- `hierarchy.close-scene`
- `hierarchy.activate-scene`
- `hierarchy.split-session`
- `hierarchy.activate-session`
- `hierarchy.delete-session`
- `hierarchy.replace-layout`
- `hierarchy.detach-session`
- `hierarchy.return-session`
- `hierarchy.move-task-to-window`

Each destructive command receives a confirmed intent token produced by the UI's
specific confirmation flow. The Runtime validates current state again so a dialog
opened against stale state never authorizes a broader deletion.

### 8.2 Bootstrap result

`hierarchy.bootstrap-window` registers the `windowId`, normalizes stale detached
mounts, creates the first default hierarchy when eligible, repairs focus, validates
the active Workspace path, and returns:

```ts
interface HierarchyProjection {
  windowId: string
  workspaces: WorkspaceView[]
  tasks: TaskView[]
  scenes: SceneView[]
  sceneSnapshots: SceneSnapshot[]
  sessions: SessionView[]
  pathStates: WorkspacePathState[]
  navigation: WindowNavigationView
  taskPlacements: TaskPlacementView[]
  eventSequence: number
}
```

### 8.3 Path validation

- Validate on Workspace activation and immediately before execution-producing
  commands.
- Poll active Workspace every 30 seconds while the application is not suspended.
- Poll invalid Workspaces every 30 seconds so restoration appears automatically.
- Validation checks existence, directory type, and read/execute access.
- Every request carries a validation generation; older completions are discarded.
- A valid-to-invalid transition does not terminate running PTYs, but freezes new
  input at the Runtime boundary.

### 8.4 PTY lifecycle integration

Hierarchy-created Sessions are persisted first. Renderer then attaches by sending
`terminal.spawn` with the persisted Session identity and ExecutionContext.
Runtime marks startup failure on that Session without affecting siblings.

Deletion returns a list of committed Session IDs to the RuntimeServer. It disposes
matching PTYs idempotently after commit. Recovery scans archived Sessions with live
registry entries and completes cleanup after a crash window.

## 9. Scene Layout Model

The Renderer edits an immutable tree:

```ts
type LayoutNode =
  | { id: string; kind: 'mount'; mountId: string }
  | {
      id: string
      kind: 'split'
      direction: 'horizontal' | 'vertical'
      children: LayoutNode[]
    }
```

- Horizontal means children appear left-to-right.
- Vertical means children appear top-to-bottom.
- Splitting the active mount replaces it with a split containing the existing mount
  first and the new mount second.
- Nested same-direction splits may be flattened during normalization.
- Removing a child collapses a one-child split.
- Minimum pane size is 160 by 100 CSS pixels.
- Divider ratios are saved through debounced `geometry.put` with `layoutRevision`.
- Structural replacement uses `hierarchy.replace-layout(expectedRevision, tree)`.
- Revision conflict triggers a fresh Scene snapshot and reapplies only the user's
  still-valid intent.

## 10. Renderer Structure

```text
renderer/src/
  runtime/
    RuntimeClient.ts
    RuntimeProvider.tsx
  hierarchy/
    hierarchy-types.ts
    hierarchy-projection.ts
    hierarchy-commands.ts
    HierarchyShell.tsx
    WorkspaceSwitcher.tsx
    TaskSidebar.tsx
    SceneTabBar.tsx
    SceneOverflowMenu.tsx
    SplitTree.tsx
    SplitDivider.tsx
    TerminalPane.tsx
    DetachedPlaceholder.tsx
    RenameDialog.tsx
    ConfirmDialog.tsx
    EmptyWorkspaceState.tsx
    hierarchy.css
  terminal/
    TerminalSurface.tsx
    terminal-session-controller.ts
```

`RuntimeClient` owns the one MessagePort, handshake, RPC correlation, ordered event
subscription, reconnect, and terminal stream fan-out. Individual TerminalSurface
components subscribe by Session ID. Unmounting a surface detaches its consumer;
only an explicit hierarchy deletion sends terminal disposal.

### 10.1 Workspace switcher

- Shows current Workspace and full path tooltip.
- Path renders with a left-side ellipsis and preserved tail.
- Dropdown supports create, switch, rename, and remove.
- Selecting an already registered normalized path activates the existing Workspace.
- Invalid entries show `路径失效` with reason tooltip.

### 10.2 Task sidebar

- Button label: `+ 新事项`.
- Active, hover, dragging, and drop-target states are visually distinct.
- HTML drag data includes source Workspace and Task. A foreign Workspace target
  rejects the drop and leaves order unchanged.
- Active Task uses `scrollIntoView({ block: 'nearest' })` after activation, restore,
  and reorder.
- Context menu contains `重命名` and `删除事项`.

### 10.3 Scene tab bar

- `+`, horizontal split, and vertical split actions.
- Active Scene remains visible.
- Overflow ends in `…`; menu selection centers the target tab.
- `Cmd/Ctrl+Shift+PageUp/PageDown` changes order.
- Context menu supports rename; close button follows section 5.2.

### 10.4 Terminal panes

- Active pane has the strong selection treatment and receives focus after hierarchy
  navigation unless focus currently belongs to search, a dialog, or another window.
- Header shows terminal title/type and delete control.
- Mixed Shell and Agent Sessions remain independent.
- Invalid Workspace blocks key input before posting and Runtime enforces the same
  rule after posting.

## 11. Electron Main and Preload

Expose a narrow typed API through `contextBridge`:

- `selectWorkspaceDirectory()`
- `registerWindow(windowId, kind)`
- `hideCurrentWindow(windowId)`
- `createDetachedTerminalWindow({ windowId, sessionId, sceneId, mountId })`
- `closeDetachedTerminalWindow(windowId)`
- `onDetachedWindowClosed(listener)`

Main owns:

- a stable per-window ID;
- BrowserWindow registry;
- tray creation and `显示 Matou` / `退出` actions;
- hide-on-protected-close without destroying WebContents;
- detached-window query/bootstrap data;
- relaying native window closure to Runtime/Renderer;
- creating additional main windows for scripted Task migration targets.

Main does not open SQLite and does not proxy terminal bytes.

## 12. Detach and Task Migration Protocols

### 12.1 Detach

1. Renderer sends `hierarchy.detach-session`.
2. Runtime marks the SceneWindow detached and keeps the mount.
3. Main creates the detached BrowserWindow.
4. Detached Renderer attaches to the existing Session; Runtime transfers live
   delivery to it without restarting PTY.
5. Main Scene renders a placeholder.
6. On native close, Runtime resolves the return target, updates layout, and the main
   Renderer reattaches the existing Session.
7. Any failure before native-window readiness compensates by returning the mount.

### 12.2 Whole-Task migration

Expose `task.move-to-window` through the Host Control Plane with a dedicated scope.

1. Validate source placement, target main window, Workspace, Task, Scenes, and live
   Session registry.
2. Insert target placement in a transaction with migration state `preparing`.
3. Target Renderer acknowledges the Task projection and all Scene snapshots.
4. Runtime marks target placement committed and removes source placement.
5. Terminal streams transfer Renderer attachment while PTYs stay in Runtime.
6. If target acknowledgment times out or target closes, Runtime restores source
   placement and emits one failure result.

Workspace ownership and ExecutionContext never change.

## 13. Error Handling

- Duplicate name: `CONFLICT`; dialog shows real-time red text and disables confirm.
- Missing path: `WORKSPACE_PATH_INVALID`; UI shows the fixed product message.
- Stale confirmation: `CONFLICT`; UI closes stale dialog and refreshes projection.
- Stale layout revision: `CONFLICT`; fetch latest Scene and preserve valid intent.
- PTY spawn failure: Session enters failed/exited presentation; siblings remain live.
- Corrupt Scene: preserve other hierarchy nodes and synthesize a single-mount
  placeholder Scene for the damaged node.
- Detached BrowserWindow creation failure: compensate to attached state.
- Task migration failure: restore source placement before reporting failure.
- Runtime reconnect: refresh projection, then each visible terminal reattaches and
  requests Journal replay from its acknowledged sequence.

## 14. Performance and Accessibility

- Hierarchy switch target: under 200 ms user-perceived latency.
- Path-status update target: under 500 ms after validation completes.
- Geometry writes debounce at 100 ms and stay outside Outbox.
- Terminal components for inactive Scenes remain mounted when live so switching does
  not recreate PTYs; xterm rendering is paused/resized when hidden.
- Buttons have Chinese aria-labels matching visible product terms.
- All lists support keyboard activation.
- Dialog focus is trapped, Escape cancels, and destructive confirmation never
  triggers while IME composition is active.
- Dragging always has a keyboard equivalent for Scene reorder; Task reorder exposes
  move-up/move-down menu actions in addition to pointer drag.

## 15. Test Strategy

All production behavior follows red-green-refactor.

### 15.1 Unit and repository tests

- Migration forward application and schema constraints.
- Atomic default hierarchy creation and command deduplication.
- Default Workspace tombstone.
- Task/pinned-Scene duplicate names.
- Lowest-available default names.
- Focus repair and deterministic successor selection.
- Task and Scene reorder constraints.
- Split-tree normalization, insertion, removal, and revision conflicts.
- Three destructive workflows and stale confirmation rejection.
- Path validation generations and Runtime input enforcement.
- Detach return-target resolution.
- Whole-Task migration commit and rollback.

### 15.2 Renderer tests

- Projection replacement/event application.
- Workspace path-tail rendering and invalid badge.
- Rename validation and disabled confirmation.
- Distinct confirmation sequences and exact copy.
- Active Task/Scene visibility.
- Overflow selection centers the Scene.
- Split direction and independent layout state.
- Focus behavior and invalid-path input guard.
- Detached placeholder.

### 15.3 Electron E2E

The E2E suite maps one-to-one to PRD 05 section 9.1:

1. First launch creates Workspace, Task, Scene, Shell, and correct cwd.
2. New Workspace activates immediately with a complete default hierarchy.
3. Workspace switch restores Task, Scene, active terminal, and geometry.
4. Task switch restores Scene and terminal focus.
5. Scene switch restores split tree and focus.
6. Task reorder persists across restart.
7. Cross-Workspace Task drop is rejected.
8. Last Scene/Task close hides current window and tray restore preserves state.
9. Non-last Scene closes only itself and selects deterministic successor.
10. Last Task sidebar deletion recreates `默认` when path is valid.
11. Horizontal/vertical split inserts in the correct direction.
12. Scene layouts remain independent.
13. Detach keeps PID and renders placeholder.
14. Detached close returns the same PID.
15. Restart normalizes detached state.
16. Removed directory shows invalid state while history remains visible.
17. Invalid Workspace rejects every execution-producing action with fixed copy.
18. Restored path returns to valid state with hierarchy unchanged.
19. Removed default Workspace does not return after restart.
20. Default Workspace name and directory use one constant.
21. Active terminal switch updates the shared active-context projection.
22. Mixed Shell/Agent Sessions close independently.
23. Agent mode changes panel title only; Scene title remains stable.
24. 3 Workspaces x 2 Tasks x 3 Scenes x 4 terminals remains operable.
25. Runtime crash and app restart restore the last stable hierarchy.
26. Workspace deletion dialog copy and disk-directory preservation.
27. Task deletion two-step/one-step confirmation matrix.
28. Scene-close static confirmation matrix.
29. Session-delete single-dialog matrix.
30. Duplicate Task/Scene rename behavior.
31. Default Task naming sequence.
32. Path left ellipsis and tooltip.
33. Scene overflow menu and centering.
34. One main window hiding does not affect another.
35. Whole-Task migration preserves PID, Workspace ownership, and rollback.

The packaged Electron suite repeats bootstrap, split, restart, path-invalid,
detach/return, and tray restoration against `Matou.app`.

## 16. Acceptance Deliverable

PRD 05 reaches its user acceptance point only when all of the following exist:

1. The complete Runtime/Main/Renderer implementation in the repository.
2. Full unit, integration, typecheck, build, Electron E2E, and packaged E2E evidence.
3. A generated `Matou.app` under the desktop release directory.
4. A PRD 05 acceptance matrix linking every product requirement to a test or an
   explicit manual verification step.
5. No authority regression: Renderer exports no authoritative hierarchy snapshot,
   Main opens no SQLite connection, terminal bytes bypass Main.
6. A dedicated Git commit for PRD 05.

After this gate, work pauses for user acceptance. PRD 03 starts only after explicit
approval of the PRD 05 deliverable.
