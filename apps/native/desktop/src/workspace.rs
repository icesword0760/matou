use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::history_view::{HistoryView, HistoryViewEvent, HistoryViewState};
#[cfg(target_os = "macos")]
use crate::recovery_water_view::RecoveryWater;
use gpui::{prelude::*, *};
use matou_native_core::geometry::GeometryUpdate;
use matou_native_core::hierarchy::canvas::{ForkPlacement, ForkWorktreeMode};
use matou_native_core::hierarchy::cards::RemoveScope;
use matou_native_core::hierarchy::{HierarchyResult, Scene, Task, Workspace as HierarchyWorkspace};
use matou_native_core::workspace_paths::WorkspacePathState;
use matou_native_runtime::history::{HistoryCursor, HistoryLine, HistorySearchOptions};
use matou_native_runtime::history_worker::{HistoryReply, HistoryRequest};
use matou_native_runtime::host::{RecentReplies, deferred_summary_text};
use matou_native_terminal::Theme;
use serde_json::Value;

mod deferred_recovery_card {
    include!("deferred_recovery_card.rs");
}

mod session_levels {
    include!("session_levels.rs");
}

use crate::{
    carousel_motion::{
        Curve, Transition, centered_scroll, focus_scroll_step, fully_visible_scroll,
    },
    rename_input::{RenameInput, RenameInputEvent, trim_name},
    runtime_bridge::{
        BridgeEvent, Command, ConnectionPhase, Event, RecoveryState, RuntimeCommands,
        StartupBlockKind,
    },
    sidebar_scrollbar::{self, SidebarScrollbarState},
    tab_dialog_style as tabs_ui,
    terminal_surface::{SurfaceEvent, SurfaceEventKind, TerminalSurface},
    ui_text::ui_text,
    workspace_drag::{self, WorkspaceDragSnapshot, WorkspaceDragTask},
};

#[cfg(debug_assertions)]
type MotionTrace = std::rc::Rc<std::cell::RefCell<std::fs::File>>;

#[derive(Clone)]
struct TaskDrag {
    task_id: String,
    workspace_id: String,
    title: String,
}
impl Render for TaskDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(230.))
            .h(px(34.))
            .px(px(7.))
            .rounded(px(8.))
            .bg(rgba(0xeceef3b8))
            .text_color(rgb(0x2f3547))
            .text_size(px(13.))
            .flex()
            .items_center()
            .gap(px(7.))
            .child(icon("task", 12., 0x2f3547))
            .child(self.title.clone())
    }
}

#[derive(Clone)]
struct WorkspaceDrag {
    workspace_id: String,
    snapshot: WorkspaceDragSnapshot,
    width: Pixels,
    offset: Point<Pixels>,
}
impl Render for WorkspaceDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().relative().w(self.width).child(
            workspace_drag::group(&self.snapshot, self.width, 1.)
                .relative()
                .left(self.offset.x)
                .top(self.offset.y),
        )
    }
}

struct CardMotion {
    width: Transition,
    lift: Transition,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LevelFocusRequest {
    OpenChildren,
    ReturnParent,
    Notification(u64),
}

struct WorkspaceDragMotion {
    scale: Transition,
    opacity: Transition,
    background: Transition,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PermissionMode {
    Default,
    Auto,
    AcceptEdits,
    Plan,
    Bypass,
}

impl PermissionMode {
    const ALL: [Self; 5] = [
        Self::Default,
        Self::Auto,
        Self::AcceptEdits,
        Self::Plan,
        Self::Bypass,
    ];

    fn from_node(node: &Value) -> Self {
        let value = node["permissionMode"].as_str();
        Self::ALL
            .into_iter()
            .find(|mode| Some(mode.value()) == value)
            .unwrap_or(Self::Default)
    }

    fn value(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Auto => "auto",
            Self::AcceptEdits => "acceptEdits",
            Self::Plan => "plan",
            Self::Bypass => "bypassPermissions",
        }
    }

    // Frozen i18n/messages/hud.ts: permission.
    fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Auto => "Auto",
            Self::AcceptEdits => "Accept Edits",
            Self::Plan => "Plan Mode",
            Self::Bypass => "Bypass Permissions",
        }
    }
}

struct PermissionChip {
    mode: PermissionMode,
    title: String,
    disabled: bool,
}

struct PermissionConfirmation {
    session_id: String,
    from: PermissionMode,
    target: PermissionMode,
    resumable: bool,
}

impl PermissionConfirmation {
    // Frozen i18n/messages/hud.ts: bypassEnter*, bypassExit*, bypassNotResumable.
    fn copy(&self) -> (&'static str, &'static str, &'static str) {
        let (title, body, confirm) = if self.target == PermissionMode::Bypass {
            (
                "切换到高权限模式",
                "切换到 Bypass Permissions 会打断当前 Claude 会话进程（正在执行的任务会被中断），重启后会自动 resume 恢复会话历史。\n\n是否确认？",
                "确认切换",
            )
        } else {
            (
                "退出高权限模式",
                "退出 Bypass Permissions 会打断当前 Claude 会话进程（正在执行的任务会被中断），重启后会自动 resume 恢复会话历史。\n\n是否确认？",
                "确认退出",
            )
        };
        let body = if self.resumable {
            body
        } else {
            "当前 Claude 会话还没有生成可恢复的 sessionId。继续切换会启动一个全新的 Claude 会话，当前内容将不会保留。\n\n是否确认？"
        };
        (title, body, confirm)
    }
}

#[derive(PartialEq, Eq)]
enum PermissionOutcome {
    Pending,
    Confirmed,
    Failed,
}

struct PermissionChange {
    from: PermissionMode,
    target: PermissionMode,
    outcome: PermissionOutcome,
}

enum TabDialog {
    RemoveWorkspace {
        workspace_id: String,
        name: String,
    },
    RenameTask {
        task_id: String,
        input: Entity<RenameInput>,
        empty_submitted: bool,
        failure: Option<(String, String)>,
    },
    DeleteTask {
        task_id: String,
        title: String,
    },
    Rename {
        scene_id: String,
        input: Entity<RenameInput>,
    },
    Close {
        scene_id: String,
        graph: Value,
        is_last: bool,
    },
    CloseSession {
        scene_id: String,
        session_id: String,
        body: String,
    },
    LastSession {
        scene_id: String,
        session_id: String,
        return_focus: Option<FocusHandle>,
    },
    RemoveSession {
        scene_id: String,
        session_id: String,
        scope: RemoveScope,
    },
    SessionLoader(SessionLoader),
    Branch(BranchDialog),
    PermissionMode(PermissionConfirmation),
}

impl TabDialog {
    fn is_session_card(&self) -> bool {
        matches!(
            self,
            Self::CloseSession { .. }
                | Self::LastSession { .. }
                | Self::RemoveSession { .. }
                | Self::SessionLoader(_)
                | Self::Branch(_)
        )
    }
}

struct BranchDialog {
    scene_id: String,
    session_id: String,
    source_title: String,
    placement: ForkPlacement,
    input: Entity<RenameInput>,
    submission_key: String,
    submitting: bool,
    error: Option<String>,
}

impl BranchDialog {
    fn copy(&self) -> (&'static str, String, &'static str) {
        let title = match self.placement {
            ForkPlacement::Child => "创建子会话分支",
            ForkPlacement::Sibling => "创建同级分支",
            ForkPlacement::Peer => "Fork 会话",
        };
        if self.placement == ForkPlacement::Peer {
            (
                title,
                format!("复制“{}”的当前对话并加入当前列表", self.source_title),
                "和当前会话使用同一目录，适合连续处理同一份改动",
            )
        } else {
            (
                title,
                format!("从“{}”继续一条独立工作路径", self.source_title),
                "和父会话使用同一目录，适合连续处理同一份改动",
            )
        }
    }
}

struct ForkAccess {
    blocked: Option<&'static str>,
    readiness: Option<&'static str>,
}

enum ForkRetry {
    Pending,
    Rejected(String),
}

fn fork_failure_copy(node: &Value) -> Option<(&'static str, Option<&str>)> {
    let progress = &node["forkProgress"];
    if progress["stage"] != "failed" && node["forkState"] != "failed" {
        return None;
    }
    let error = progress["error"]
        .as_str()
        .or_else(|| node["forkError"].as_str());
    if error.is_some_and(|error| {
        error
            .split(['\n', '\r', '\u{2028}', '\u{2029}'])
            .any(|line| {
                let line = line.to_ascii_lowercase();
                line.contains("provider session not found")
                    || line.contains("no conversation found")
                    || line
                        .split_once("conversation")
                        .is_some_and(|(_, rest)| rest.contains("not found"))
            })
    }) {
        Some((
            "父会话已失效",
            Some(
                "原 Claude Code 对话身份已失效，本次分支没有创建成功。请返回父会话继续，或移除此失败节点后新建空会话。",
            ),
        ))
    } else {
        Some(("分支创建失败", error))
    }
}

struct ForkProgressCopy {
    label: String,
    description: &'static str,
    counter: Option<String>,
    fraction: Option<f32>,
    interactive: bool,
}

fn fork_progress_copy(node: &Value) -> Option<ForkProgressCopy> {
    let progress = &node["forkProgress"];
    let stage_key = progress["stage"].as_str()?;
    let (stage, description) = match stage_key {
        "queued" => ("等待创建分支", "前面的分支任务完成后会自动继续"),
        "creating-worktree" => (
            "正在创建独立工作目录",
            "正在隔离文件修改，其他会话可继续工作",
        ),
        "applying-setup" => ("正在准备分支环境", "正在执行此工作空间需要的准备步骤"),
        "binding-session" => ("正在绑定分支会话", "正在把新会话连接到独立工作目录"),
        "restoring-provider" => ("正在恢复智能体会话", "正在续接原会话上下文并启动分支任务"),
        "starting-window" => ("正在打开分支窗口", "分支窗口即将可以输入和查看输出"),
        _ => return None,
    };
    let steps = progress["totalSteps"]
        .as_i64()
        .zip(progress["completedSteps"].as_i64());
    let steps = steps.map(|(total, completed)| (total.max(1), completed.clamp(0, total.max(0))));
    Some(ForkProgressCopy {
        label: format!("正在创建分支：{stage}"),
        description,
        counter: steps.map(|(total, completed)| {
            format!(
                "第 {} / {total} 阶段",
                completed + i64::from(completed < total)
            )
        }),
        fraction: steps.map(|(total, completed)| completed as f32 / total as f32),
        interactive: matches!(stage_key, "restoring-provider" | "starting-window"),
    })
}

struct LoaderOwner {
    session_id: String,
    title: String,
}

struct LoaderSession {
    id: String,
    title: String,
    model: Option<String>,
    permission_mode: String,
    message_count: usize,
    updated_at: i64,
    loaded_by: Option<LoaderOwner>,
}

impl LoaderSession {
    fn from_graph(value: &Value) -> Option<Self> {
        let loaded_by = &value["loadedBy"];
        Some(Self {
            id: value["providerSessionId"].as_str()?.into(),
            title: value["title"].as_str()?.into(),
            model: value["model"].as_str().map(str::to_owned),
            permission_mode: value["permissionMode"].as_str()?.into(),
            message_count: value["messageCount"].as_u64()?.try_into().ok()?,
            updated_at: value["updatedAt"].as_i64()?,
            loaded_by: if loaded_by.is_null() {
                None
            } else {
                Some(LoaderOwner {
                    session_id: loaded_by["sessionId"].as_str()?.into(),
                    title: loaded_by["title"].as_str()?.into(),
                })
            },
        })
    }
}

struct LoaderEvent {
    index: usize,
    kind: String,
    text: String,
    tool_name: Option<String>,
}

impl LoaderEvent {
    fn from_graph(value: &Value) -> Option<Self> {
        Some(Self {
            index: value["index"].as_u64()?.try_into().ok()?,
            kind: value["kind"].as_str()?.into(),
            text: value["text"].as_str()?.into(),
            tool_name: value["toolName"].as_str().map(str::to_owned),
        })
    }
}

struct LoaderDetailRequest {
    id: String,
    start: Option<i64>,
}

struct LoaderLoad {
    id: String,
    title: String,
    previous_revision: u64,
}

struct DeferredRecoveryCard {
    title: String,
    subtitle: Option<String>,
    cwd: Option<String>,
    agent: Option<&'static str>,
    updated_label: Option<String>,
    replies: Vec<DeferredReply>,
    loading: bool,
    can_open_history: bool,
    can_restore: bool,
}

struct DeferredReply {
    kind: String,
    label: String,
    text: String,
}

struct DeferredPreview {
    cwd: String,
    loading: bool,
    result: Option<RecentReplies>,
}

struct SessionLoader {
    history: bool,
    scene_id: String,
    session_id: String,
    cwd: String,
    input: Entity<RenameInput>,
    sessions: Vec<LoaderSession>,
    selected: Option<String>,
    events: Vec<LoaderEvent>,
    first_event: Option<i64>,
    total_events: usize,
    loading_list: bool,
    loading_detail: bool,
    loading_earlier: bool,
    pending_detail: Option<LoaderDetailRequest>,
    has_earlier: bool,
    error: Option<String>,
    list_scroll: ScrollHandle,
    preview_scroll: ScrollHandle,
    list_focus: FocusHandle,
}

impl SessionLoader {
    fn visible_sessions<'a>(
        &'a self,
        cx: &App,
    ) -> impl Iterator<Item = &'a LoaderSession> + use<'a> {
        let query = self.input.read(cx).text.trim().to_lowercase();
        let cwd_matches = query.is_empty() || self.cwd.to_lowercase().contains(&query);
        self.sessions.iter().filter(move |session| {
            cwd_matches
                || session.title.to_lowercase().contains(&query)
                || session.id.to_lowercase().contains(&query)
                || session
                    .model
                    .as_ref()
                    .is_some_and(|model| model.to_lowercase().contains(&query))
        })
    }
}

const SESSION_LOADER_PAGE_SIZE: i64 = 200;

const LAST_TAB_BODY: &str = "当前已是最后一个事项下的最后一个标签，这里点击关闭不会删除该事项。\n\n如需删除该工作区，请在左侧事项面板的下拉菜单中执行删除。";
const LAST_TAB_CONFIRM: &str = "我知道了";
const STOP_SESSION: &str = "停止会话";
const STOPPED_SESSION_STATUS: &str = "正在恢复会话…";
const REMOVE_NODE_ACTION: &str = "移除节点…";
const WORKSPACE_PATH_UNAVAILABLE: &str = "工作区目录不可用，请先在本地恢复原路径，或移出该工作区";
const SESSION_LOADER_TITLE: &str = "载入 Claude Code 会话";
const SESSION_LOADER_FILTER_PLACEHOLDER: &str = "筛选左侧：标题、路径、模型或会话 ID";
const SESSION_LOADER_RUNNING_WARNING: &str = "当前卡片正在运行，继续会结束当前进程。";
const SESSION_LOADER_NO_SESSIONS: &str = "当前工作空间内没有 Claude Code 会话";
const SESSION_LOADER_NO_MATCHES: &str = "左侧没有匹配的会话";
const SESSION_LOADER_LOADING_MORE: &str = "正在载入更多会话…";
const SESSION_LOADER_SELECT_SESSION: &str = "选择会话查看内容";
const SESSION_LOADER_LOADING_PREVIEW: &str = "正在载入预览…";
const SESSION_LOADER_NO_HISTORY: &str = "当前会话还没有可显示的历史内容";
const SESSION_LOADER_LOADING_EARLIER: &str = "正在载入更早内容…";
const SESSION_LOADER_LOADING: &str = "正在载入…";

struct StartFailureCard<'a> {
    title: &'static str,
    reason: &'a str,
    retry: &'static str,
    remove: &'static str,
    blocked: Option<&'static str>,
}

enum ProviderRestoreBanner<'a> {
    Restoring {
        title: &'static str,
    },
    Failed {
        title: &'static str,
        reason: &'a str,
        retry: &'static str,
        fresh: &'static str,
        pending: bool,
        retry_blocked: Option<&'static str>,
        fresh_blocked: Option<&'static str>,
    },
}

struct PendingProviderRestore {
    revision: Option<u64>,
    _watchdog: gpui::Task<()>,
}

struct CardTooltip(SharedString);

#[derive(Clone, Copy)]
struct RemoveNodeTooltip {
    blocked_reason: Option<&'static str>,
}

impl RemoveNodeTooltip {
    fn text(self) -> &'static str {
        self.blocked_reason.unwrap_or(REMOVE_NODE_ACTION)
    }
}

fn card_tooltip_box() -> Div {
    div()
        .px(px(9.))
        .py(px(7.))
        .border_1()
        .border_color(rgb(0xd6dae1))
        .rounded(px(7.))
        .bg(rgb(0xffffff))
        .text_color(rgb(0x333b47))
        .text_size(px(10.))
        .line_height(px(15.))
        .shadow(vec![shadow(0x1f2837, 0.16, 8., 24.)])
}

impl Render for RemoveNodeTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        card_tooltip_box().child(self.text())
    }
}

impl Render for CardTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        card_tooltip_box()
            .max_w(px(460.))
            .whitespace_normal()
            .child(ui_text(self.0.clone()).pre_line())
    }
}

struct RemovalDialogText {
    title: String,
    body: String,
    files_unchanged: Option<&'static str>,
    choices: Option<[RemovalChoiceText; 2]>,
    warning: Option<String>,
    confirm: String,
}

struct RemovalChoiceText {
    title: &'static str,
    impact: String,
    description: String,
}

#[derive(Default)]
struct RemovalImpact {
    sessions: usize,
    owned_worktrees: Option<usize>,
    running: usize,
    needs_input: usize,
}

impl RemovalImpact {
    fn add(&mut self, node: &Value) {
        let owned = node["hasOwnedWorktree"].as_bool().map(usize::from);
        self.owned_worktrees = if self.sessions == 0 {
            owned
        } else {
            self.owned_worktrees
                .zip(owned)
                .map(|(total, count)| total + count)
        };
        self.sessions += 1;
        match node["workStatus"].as_str() {
            Some("running" | "starting") => self.running += 1,
            Some("needs-input") => self.needs_input += 1,
            _ => {}
        }
    }

    fn activity(&self) -> String {
        [
            (self.running > 0).then(|| format!("{} 个运行中", self.running)),
            (self.needs_input > 0).then(|| format!("{} 个待输入", self.needs_input)),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("、")
    }

    fn label(&self) -> String {
        let mut label = format!("影响 {} 个会话", self.sessions);
        if let Some(count) = self.owned_worktrees {
            label.push_str(&format!("、{count} 个自有 Worktree"));
        }
        let activity = self.activity();
        if !activity.is_empty() {
            label.push_str(&format!("；其中 {activity}"));
        }
        label
    }
}

const CONTROLS: &[&str] = &[
    "new-workspace",
    "notifications",
    "task-menu",
    "board",
    "settings",
    "add-tab",
    "tab-overflow",
    "graph",
    "split",
    "update",
    "permission-mode",
    "session-return-parent",
];

struct ArchivedMatches {
    query: String,
    options: matou_native_terminal::SearchOptions,
    sequence: u64,
    index: usize,
    matches: Vec<HistoryLine>,
    gap_count: usize,
    has_more: bool,
    show_summary: bool,
}

struct TerminalSearchState {
    input: Entity<RenameInput>,
    controls: [FocusHandle; 6],
    query: String,
    open: bool,
    options: matou_native_terminal::SearchOptions,
    sequence: u64,
    session_id: Option<String>,
    result: Option<matou_native_runtime::search::SearchResult>,
    error: Option<String>,
    previous: bool,
    archive: Option<ArchivedMatches>,
    archive_pending: Option<u64>,
    archive_count: Option<(usize, usize)>,
    dismissed_sequence: Option<u64>,
}

struct HistoryContextRequest {
    session_id: String,
    sequence: u64,
    cursor: HistoryCursor,
    result_index: usize,
    result_count: usize,
}

const NOTIFICATION_COOLDOWN_MS: u64 = 5_000;
const NOTIFICATION_LIMIT: usize = 1_000;
const NOTIFICATION_READ_RETENTION_MS: u64 = 30 * 24 * 60 * 60 * 1_000;

struct NotificationEntry {
    id: u64,
    event_id: String,
    title: String,
    subtitle: String,
    body: String,
    workspace_id: Option<String>,
    task_id: Option<String>,
    scene_id: Option<String>,
    session_id: Option<String>,
    occurred_at: u64,
    read: bool,
    replacement_key: Option<String>,
    cooldown_key: Option<String>,
}

struct Notifications {
    entries: Vec<NotificationEntry>,
    // Last accepted receipt time and number of rows keeping each key alive.
    cooldowns: HashMap<String, (u64, usize)>,
    sequence: u64,
    sound_enabled: bool,
    sound_path: Option<PathBuf>,
}

impl Notifications {
    fn new(sound_path: Option<PathBuf>) -> Self {
        let sound_enabled = sound_path
            .as_ref()
            .and_then(|path| std::fs::read(path).ok())
            .is_none_or(|bytes| bytes != b"0");
        Self {
            entries: Vec::new(),
            cooldowns: HashMap::new(),
            sequence: 0,
            sound_enabled,
            sound_path,
        }
    }

    fn push(&mut self, value: &Value, focused: Option<&str>, now: u64) -> bool {
        let text = |name| {
            value[name]
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
        };
        let Some(event_id) = text("eventId") else {
            return false;
        };
        self.prune(now);
        let replacement_key = text("replacementKey");
        let replacement = replacement_key.and_then(|key| {
            self.entries
                .iter()
                .position(|entry| entry.replacement_key.as_deref() == Some(key))
        });
        if text("operation") == Some("dismiss") {
            if let Some(index) = replacement {
                self.remove_at(index);
            }
            return false;
        }
        if self.entries.iter().any(|entry| entry.event_id == event_id) {
            return false;
        }
        let session_id = text("sessionId");
        let scene_id = text("sceneId");
        let task_id = text("taskId");
        let workspace_id = text("workspaceId");
        let read = session_id.is_some() && session_id == focused;
        let (id, cooldown_key) = if let Some(index) = replacement {
            // A replacement takes precedence over cooldown and keeps the row's identity.
            let current = self.entries.remove(index);
            (current.id, current.cooldown_key)
        } else {
            let owner = session_id
                .or(scene_id)
                .or(task_id)
                .or(workspace_id)
                .unwrap_or("global");
            let key = text("cooldownKey")
                .or_else(|| text("eventType"))
                .unwrap_or("attention");
            let key = format!("{owner}:{key}");
            let cooldown = self.cooldowns.entry(key.clone()).or_insert((now, 0));
            if cooldown.1 > 0 && now.saturating_sub(cooldown.0) < NOTIFICATION_COOLDOWN_MS {
                return false;
            }
            *cooldown = (now, cooldown.1 + 1);
            if let Some(session_id) = session_id {
                self.mark_session_read(session_id);
            }
            self.sequence += 1;
            (self.sequence, Some(key))
        };
        let occurred_at = value["occurredAt"].as_u64().unwrap_or(now);
        let index = self
            .entries
            .partition_point(|entry| entry.occurred_at > occurred_at);
        self.entries.insert(
            index,
            NotificationEntry {
                id,
                event_id: event_id.into(),
                title: text("title").unwrap_or("Claude Code").into(),
                subtitle: text("subtitle").unwrap_or_default().into(),
                body: text("body").unwrap_or_default().into(),
                workspace_id: workspace_id.map(str::to_owned),
                task_id: task_id.map(str::to_owned),
                scene_id: scene_id.map(str::to_owned),
                session_id: session_id.map(str::to_owned),
                occurred_at,
                read,
                replacement_key: replacement_key.map(str::to_owned),
                cooldown_key,
            },
        );
        self.prune(now);
        !read && value["sound"].as_bool() != Some(false) && self.sound_enabled
    }

    fn forget_cooldown(cooldowns: &mut HashMap<String, (u64, usize)>, entry: &NotificationEntry) {
        if let Some(key) = &entry.cooldown_key
            && let std::collections::hash_map::Entry::Occupied(mut cooldown) =
                cooldowns.entry(key.clone())
        {
            cooldown.get_mut().1 -= 1;
            if cooldown.get().1 == 0 {
                cooldown.remove();
            }
        }
    }

    fn remove_at(&mut self, index: usize) {
        Self::forget_cooldown(&mut self.cooldowns, &self.entries.remove(index));
    }

    fn prune(&mut self, now: u64) {
        self.entries.retain(|entry| {
            let keep = !entry.read
                || now.saturating_sub(entry.occurred_at) <= NOTIFICATION_READ_RETENTION_MS;
            if !keep {
                Self::forget_cooldown(&mut self.cooldowns, entry);
            }
            keep
        });
        if self.entries.len() <= NOTIFICATION_LIMIT {
            return;
        }
        let mut counts = HashMap::new();
        self.entries.retain(|entry| {
            let count = counts.entry(entry.workspace_id.clone()).or_insert(0);
            *count += 1;
            let keep = *count <= NOTIFICATION_LIMIT;
            if !keep {
                Self::forget_cooldown(&mut self.cooldowns, entry);
            }
            keep
        });
    }

    fn mark_session_read(&mut self, id: &str) {
        for entry in &mut self.entries {
            if entry.session_id.as_deref() == Some(id) {
                entry.read = true;
            }
        }
    }

    fn set_sound(&mut self, enabled: bool) {
        self.sound_enabled = enabled;
        if let Some(path) = &self.sound_path {
            let temporary = path.with_extension("tmp");
            let bytes = if enabled { b"1" } else { b"0" };
            if let Err(error) =
                std::fs::write(&temporary, bytes).and_then(|_| std::fs::rename(&temporary, path))
            {
                eprintln!("Save notification sound preference: {error}");
            }
        }
    }
}

pub struct Workspace {
    history_view: Option<Entity<HistoryView>>,
    history_request: Option<HistoryContextRequest>,
    search: Option<TerminalSearchState>,
    commands: RuntimeCommands,
    runtime_generation: u64,
    transport_connected: bool,
    domain_ready: bool,
    session_frames: HashSet<String>,
    search_after_restore: bool,
    transport_fault: Option<(u64, String)>,
    hierarchy: Option<HierarchyResult>,
    scenes: Vec<Scene>,
    workspaces: Vec<HierarchyWorkspace>,
    workspace_paths: Vec<WorkspacePathState>,
    workspace_controls: HashMap<String, [FocusHandle; 4]>,
    workspace_drag_source: Option<String>,
    workspace_drag_press: Option<(String, Point<Pixels>)>,
    workspace_drag_gesture: bool,
    workspace_drag_target: Option<String>,
    workspace_drag_motion: HashMap<String, WorkspaceDragMotion>,
    workspace_bounds: HashMap<String, Bounds<Pixels>>,
    workspace_group_bounds: HashMap<String, Bounds<Pixels>>,
    workspace_hover: Option<String>,
    workspace_collapsed: HashSet<String>,
    workspace_menu: Option<(String, Point<Pixels>, Vec<FocusHandle>)>,
    creating_workspace: bool,
    tasks: Vec<Task>,
    task_controls: HashMap<String, [FocusHandle; 2]>,
    task_bounds: HashMap<String, Bounds<Pixels>>,
    task_hover: Option<String>,
    reveal_task: Option<String>,
    task_menu: Option<(String, Point<Pixels>, Vec<FocusHandle>)>,
    pub(crate) sidebar_scroll: ScrollHandle,
    sidebar_overflow: bool,
    pub(crate) sidebar_scrollbar: SidebarScrollbarState,
    notifications: Notifications,
    notification_open: bool,
    notification_scene_sessions: HashMap<String, (String, HashSet<String>)>,
    task_toast: Option<String>,
    toast_generation: u64,
    permission_menu: Option<String>,
    permission_changes: HashMap<String, PermissionChange>,
    permission_identities: HashSet<String>,
    permission_toast: Option<String>,
    permission_toast_timer: Option<gpui::Task<()>>,
    tab_scroll: ScrollHandle,
    tab_hover: Option<String>,
    tab_overflow: bool,
    hidden_tabs: Option<Vec<(String, FocusHandle)>>,
    tab_menu: Option<(String, Point<Pixels>, FocusHandle)>,
    control_focus: HashMap<&'static str, FocusHandle>,
    tab_controls: HashMap<String, [FocusHandle; 2]>,
    card_controls: HashMap<String, FocusHandle>,
    remove_card_controls: HashMap<String, FocusHandle>,
    fork_controls: HashMap<String, [FocusHandle; 4]>,
    fork_menu: Option<(String, Point<Pixels>, FocusHandle)>,
    fork_readiness: Option<&'static str>,
    fork_readiness_generation: u64,
    fork_retries: HashMap<String, ForkRetry>,
    keyboard_focus: bool,
    dialog: Option<TabDialog>,
    dialog_controls: Vec<FocusHandle>,
    dialog_subscription: Option<Subscription>,
    pending_close: Option<String>,
    carousel_scroll: ScrollHandle,
    motion_clock: Instant,
    #[cfg(debug_assertions)]
    motion_trace: Option<MotionTrace>,
    card_motion: HashMap<String, CardMotion>,
    focus_follow_until: Option<f64>,
    hovered: Option<String>,
    hover_follow_until: Option<f64>,
    hover_baseline: Option<f64>,
    hover_restore_at: Option<f64>,
    pointer_position: Option<Point<Pixels>>,
    edge_direction: i8,
    edge_blocked: bool,
    edge_next_at: Option<f64>,
    edge_target: Option<String>,
    edge_phase_start: f64,
    edge_cruising: bool,
    card_geometry: Vec<(String, f64, f64)>,
    geometry: HashMap<String, GeometryUpdate>,
    geometry_dirty: HashSet<String>,
    reconnect_geometry: HashSet<String>,
    restore_geometry: bool,
    column_capacity: Option<u32>,
    pending_geometry: Option<GeometryUpdate>,
    geometry_timer: u64,
    last_geometry_write: HashMap<String, f64>,
    geometry_sequence: u64,
    geometry_writes: HashMap<String, (u64, u8)>,
    graph: Value,
    level_parent_by_scene: HashMap<String, Option<String>>,
    carousel_owner: Option<String>,
    hovered_child_badge: Option<String>,
    terminals: HashMap<String, Entity<TerminalSurface>>,
    recovery: HashMap<String, RecoveryState>,
    recovery_controls: HashMap<String, FocusHandle>,
    deferred_previews: HashMap<String, DeferredPreview>,
    deferred_controls: HashMap<String, [FocusHandle; 2]>,
    start_failure_controls: HashMap<String, [FocusHandle; 2]>,
    provider_restore_controls: HashMap<String, [FocusHandle; 2]>,
    pending_provider_restores: HashMap<String, PendingProviderRestore>,
    pending_loader_loads: HashMap<String, LoaderLoad>,
    #[cfg(target_os = "macos")]
    recovery_water: HashMap<String, Entity<RecoveryWater>>,
    #[cfg(target_os = "macos")]
    recovery_water_visibility: Option<gpui::Task<()>>,
    storage_fault_sessions: HashSet<String>,
    order: Vec<String>,
    focused: Option<String>,
    pending_focus: Option<(String, String)>,
    pending_card_action: Option<(String, String)>,
    level_focus_request: Option<LevelFocusRequest>,
    copy_request_id: u64,
    resize_revisions: HashMap<String, u64>,
    fault: Option<String>,
    subscriptions: Vec<Subscription>,
}

impl Workspace {
    fn apply_search_results(
        &mut self,
        session_id: String,
        sequence: u64,
        result: Result<matou_native_runtime::search::SearchResult, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(search) = self.search.as_mut().filter(|search| {
            search.open
                && search.sequence == sequence
                && search.session_id.as_ref() == Some(&session_id)
        }) else {
            return;
        };
        if self
            .history_request
            .as_ref()
            .is_some_and(|request| request.session_id == session_id && request.sequence == sequence)
        {
            return;
        }
        search.archive_count = None;
        match result {
            Ok(result) => {
                if let Some(terminal) = self.terminals.get(&session_id) {
                    terminal.update(cx, |terminal, cx| {
                        terminal.set_search(Some(result.clone()), cx)
                    });
                }
                let live = !result.matches.is_empty() || search.query.is_empty();
                search.result = Some(result);
                search.error = None;
                if live {
                    search.archive = None;
                    search.archive_pending = None;
                    self.dismiss_history_context(cx);
                } else if search.archive.as_ref().is_some_and(|archive| {
                    archive.query == search.query
                        && archive.options == search.options
                        && !archive.matches.is_empty()
                }) {
                    let archive = search.archive.as_mut().unwrap();
                    if archive.sequence != sequence {
                        archive.index = if search.previous {
                            (archive.index + archive.matches.len() - 1) % archive.matches.len()
                        } else {
                            (archive.index + 1) % archive.matches.len()
                        };
                        archive.sequence = sequence;
                    }
                    self.open_history_context(window, cx);
                } else if search.archive_pending != Some(sequence) {
                    search.archive_pending = Some(sequence);
                    let options = HistorySearchOptions {
                        case_sensitive: search.options.case_sensitive,
                        regex: search.options.regex,
                        whole_word: search.options.whole_word,
                    };
                    let query = search.query.clone();
                    self.send(
                        Command::History(HistoryRequest::Search {
                            session_id,
                            sequence,
                            query,
                            options,
                        }),
                        cx,
                    );
                }
            }
            Err(error) => {
                search.result = None;
                search.archive = None;
                search.archive_pending = None;
                search.error = Some(
                    if error.contains("invalid terminal search regular expression") {
                        "正则表达式无效".into()
                    } else {
                        error
                    },
                );
                self.dismiss_history_context(cx);
                if let Some(terminal) = self.terminals.get(&session_id) {
                    terminal.update(cx, |terminal, cx| terminal.set_search(None, cx));
                }
            }
        }
        self.sync_terminal_input(window, cx);
    }

    fn open_history_context(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(search) = self.search.as_mut().filter(|search| search.open) else {
            return;
        };
        let Some(archive) = &mut search.archive else {
            return;
        };
        archive.show_summary = true;
        search.archive_count = Some((archive.index, archive.matches.len()));
        if search.dismissed_sequence == Some(search.sequence) {
            return;
        }
        let Some(matched) = archive.matches.get(archive.index) else {
            return;
        };
        let Some(session_id) = search.session_id.clone() else {
            return;
        };
        let context = HistoryContextRequest {
            session_id,
            sequence: search.sequence,
            cursor: matched.cursor,
            result_index: archive.index,
            result_count: archive.matches.len(),
        };
        let state = HistoryViewState::Loading {
            result_index: context.result_index,
            result_count: context.result_count,
            gap_count: archive.gap_count,
        };
        if self.history_view.is_none() {
            let view = cx.new(HistoryView::new);
            let generation = self.runtime_generation;
            self.subscriptions.push(cx.subscribe_in(
                &view,
                window,
                move |this, _, event, window, cx| {
                    if this.runtime_generation != generation
                        || this.commands.connection().0 != generation
                    {
                        return;
                    }
                    match event {
                        HistoryViewEvent::ReturnToLive => this.exit_history(window, cx),
                    }
                },
            ));
            self.history_view = Some(view);
        }
        self.history_view
            .as_ref()
            .unwrap()
            .update(cx, |view, cx| view.set_state(state, cx));
        let command = HistoryRequest::Context {
            session_id: context.session_id.clone(),
            sequence: context.sequence,
            cursor: context.cursor,
        };
        self.history_request = Some(context);
        self.sync_terminal_input(window, cx);
        self.send(Command::History(command), cx);
    }

    fn apply_history(&mut self, reply: HistoryReply, window: &mut Window, cx: &mut Context<Self>) {
        match reply {
            HistoryReply::Search {
                session_id,
                sequence,
                result,
            } => {
                let Some(search) = self.search.as_mut().filter(|search| {
                    search.open
                        && search.sequence == sequence
                        && search.session_id.as_ref() == Some(&session_id)
                        && search.archive_pending == Some(sequence)
                }) else {
                    return;
                };
                search.archive_pending = None;
                match result {
                    Ok(result) => {
                        let mut matches = result.matches;
                        matches.reverse();
                        let index = if search.previous {
                            matches.len().saturating_sub(1)
                        } else {
                            0
                        };
                        search.archive = Some(ArchivedMatches {
                            query: search.query.clone(),
                            options: search.options,
                            sequence,
                            index,
                            matches,
                            gap_count: result.gaps.len(),
                            has_more: result.has_more,
                            show_summary: true,
                        });
                        self.open_history_context(window, cx);
                    }
                    Err(_) => {
                        // Frozen archive RPC failures quietly return to the live view.
                        if let Some(archive) = &mut search.archive {
                            archive.show_summary = false;
                        }
                        self.dismiss_history_context(cx);
                    }
                }
            }
            HistoryReply::Context {
                session_id,
                sequence,
                cursor,
                result,
            } => {
                // A later query with zero matches leaves the current panel
                // open, including any pending read for that panel.
                let Some(context) = self.history_request.as_ref().filter(|request| {
                    request.session_id == session_id
                        && request.sequence == sequence
                        && request.cursor == cursor
                }) else {
                    return;
                };
                match result {
                    Ok(page)
                        if page
                            .anchor_index
                            .and_then(|index| page.lines.get(index))
                            .is_some_and(|line| line.cursor == cursor) =>
                    {
                        let state = HistoryViewState::Ready {
                            result_index: context.result_index,
                            result_count: context.result_count,
                            page,
                        };
                        if let Some(view) = &self.history_view {
                            view.update(cx, |view, cx| view.set_state(state, cx));
                        }
                    }
                    _ => {
                        self.dismiss_history_context(cx);
                    }
                }
            }
        }
        self.sync_terminal_input(window, cx);
        cx.notify();
    }

    fn dismiss_history_context(&mut self, cx: &mut Context<Self>) {
        self.history_request = None;
        if let Some(view) = &self.history_view {
            view.update(cx, |view, _| view.stop_drag());
        }
    }

    fn exit_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_history_context(cx);
        if let Some(search) = self.search.as_mut() {
            search.dismissed_sequence = Some(search.sequence);
            if let Some(archive) = &mut search.archive {
                archive.show_summary = false;
            }
        }
        self.sync_terminal_input(window, cx);
        if let Some(terminal) = self.focused.as_ref().and_then(|id| self.terminals.get(id)) {
            window.focus(&terminal.read(cx).focus);
        }
        cx.notify();
    }

    fn archive_summary(&self, session_id: &str) -> Option<Div> {
        let search = self
            .search
            .as_ref()
            .filter(|search| search.open && search.session_id.as_deref() == Some(session_id))?;
        let archive = search
            .archive
            .as_ref()
            .filter(|a| a.show_summary && (!a.matches.is_empty() || a.gap_count > 0))?;
        if self
            .history_request
            .as_ref()
            .is_some_and(|request| request.session_id == session_id)
        {
            return None;
        }
        let text = archive
            .matches
            .get(archive.index)
            .map_or("该范围存在不可读的历史片段", |line| {
                line.text.as_str()
            });
        let meta = format!(
            "{}/{}{}{}",
            archive.index + 1,
            archive.matches.len(),
            if archive.has_more { "+" } else { "" },
            if archive.gap_count > 0 {
                format!(" · {} 处历史缺口", archive.gap_count)
            } else {
                String::new()
            }
        );
        Some(
            div()
                .absolute()
                .right(px(12.))
                .bottom(px(10.))
                .max_w(px(720.))
                .left(px(12.))
                .px(px(10.))
                .py(px(7.))
                .border_1()
                .border_color(rgba(0x30384826))
                .rounded(px(7.))
                .bg(rgba(0xfffffff5))
                .text_color(rgb(0x303848))
                .flex()
                .items_center()
                .gap(px(8.))
                .font_family("SFMono-Regular")
                .text_size(px(11.))
                .child(
                    div()
                        .flex_none()
                        .text_color(rgb(0xff9a3d))
                        .child("归档历史"),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(text.to_owned()),
                )
                .child(div().flex_none().text_color(rgb(0x8d95a7)).child(meta)),
        )
    }

    fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.focused.is_none() {
            return;
        }
        if self.search.is_none() {
            let input = cx.new(|cx| {
                let mut input = RenameInput::new(String::new(), cx);
                input.placeholder = "搜索".into();
                input.search_style = true;
                input
            });
            // Focusing the reference search input replaces the document's
            // text selection. Window deactivation alone preserves that range.
            let input_focus = input.read(cx).focus.clone();
            self.subscriptions
                .push(cx.on_focus(&input_focus, window, |this, _, cx| {
                    if let Some(view) = &this.history_view {
                        // Focus listeners run after paint. Notify after that draw
                        // has ended so clearing the range schedules a new frame.
                        let view = view.clone();
                        cx.defer(move |cx| {
                            view.update(cx, |view, cx| view.clear_selection(cx));
                        });
                    }
                }));
            self.subscriptions.push(cx.subscribe_in(
                &input,
                window,
                |this, _, event, window, cx| match event {
                    RenameInputEvent::Changed => {
                        if let Some(search) = this.search.as_mut() {
                            search.query = search.input.read(cx).text.clone();
                        }
                        this.run_search(false, cx);
                    }
                    RenameInputEvent::Cancel => this.close_search(window, cx),
                    RenameInputEvent::Confirm => this.run_search(false, cx),
                },
            ));
            self.search = Some(TerminalSearchState {
                input,
                controls: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
                query: String::new(),
                open: false,
                options: Default::default(),
                sequence: 0,
                session_id: None,
                result: None,
                error: None,
                previous: false,
                archive: None,
                archive_pending: None,
                archive_count: None,
                dismissed_sequence: None,
            });
        }
        let search = self.search.as_mut().unwrap();
        search.open = true;
        window.focus(&search.input.read(cx).focus);
        self.run_search(false, cx);
    }

    fn run_search(&mut self, previous: bool, cx: &mut Context<Self>) {
        if self
            .focused
            .as_ref()
            .is_some_and(|id| self.recovery_blocked(id))
        {
            self.search_after_restore = self.search.as_ref().is_some_and(|search| search.open);
            return;
        }
        let Some(session_id) = self.focused.clone() else {
            if let Some(view) = &self.history_view {
                view.update(cx, |view, _| view.stop_drag());
            }
            return;
        };
        let Some(search) = self.search.as_mut().filter(|search| search.open) else {
            return;
        };
        let dismiss = search.session_id.as_ref() != Some(&session_id) || search.query.is_empty();
        if search.session_id.as_ref() != Some(&session_id) {
            if let Some(old) = search.session_id.take() {
                let _ = self.commands.send_at(
                    self.runtime_generation,
                    Command::ClearSearch {
                        session_id: old.clone(),
                    },
                );
                if let Some(terminal) = self.terminals.get(&old) {
                    terminal.update(cx, |terminal, cx| terminal.set_search(None, cx));
                }
            }
            search.result = None;
            search.archive = None;
            search.archive_pending = None;
        }
        search.session_id = Some(session_id.clone());
        search.sequence += 1;
        search.previous = previous;
        search.error = None;
        if let Some(archive) = &mut search.archive {
            archive.show_summary = false;
        }
        let request = matou_native_runtime::search::SearchRequest {
            sequence: search.sequence,
            query: search.query.clone(),
            options: search.options,
            previous,
        };
        if dismiss {
            self.dismiss_history_context(cx);
        }
        self.send(
            Command::Search {
                session_id,
                request,
            },
            cx,
        );
        cx.notify();
    }

    fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_history_context(cx);
        if let Some(search) = self.search.as_mut() {
            search.archive = None;
            search.archive_pending = None;
            search.archive_count = None;
            search.open = false;
            search.query.clear();
            search.sequence += 1;
            search.result = None;
            search.error = None;
            if let Some(session_id) = &search.session_id {
                let _ = self.commands.send_at(
                    self.runtime_generation,
                    Command::ClearSearch {
                        session_id: session_id.clone(),
                    },
                );
                if let Some(terminal) = self.terminals.get(session_id) {
                    terminal.update(cx, |terminal, cx| terminal.set_search(None, cx));
                }
            }
        }
        self.sync_terminal_input(window, cx);
        if let Some(terminal) = self.focused.as_ref().and_then(|id| self.terminals.get(id)) {
            window.focus(&terminal.read(cx).focus);
        }
        cx.notify();
    }

    fn search_key(
        &mut self,
        stroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.dialog.is_some() {
            return false;
        }
        if stroke.key == "escape" && self.history_request.is_some() {
            self.exit_history(window, cx);
            return true;
        }
        let m = stroke.modifiers;
        let command = if cfg!(target_os = "macos") {
            m.platform
        } else {
            m.control
        };
        if command && stroke.key == "f" && !m.alt {
            self.open_search(window, cx);
            return true;
        }
        let Some(search) = self
            .search
            .as_mut()
            .filter(|s| s.open && s.input.read(cx).focus.is_focused(window))
        else {
            return false;
        };
        if stroke.key == "escape" {
            self.close_search(window, cx);
            return true;
        }
        if stroke.key == "enter" {
            self.run_search(m.shift, cx);
            return true;
        }
        let modifier = if cfg!(target_os = "macos") {
            m.platform
        } else {
            m.alt
        };
        if modifier && matches!(stroke.key.as_str(), "c" | "r") {
            search.query = search.input.read(cx).text.clone();
            if stroke.key == "c" {
                search.options.case_sensitive = !search.options.case_sensitive;
            } else {
                search.options.regex = !search.options.regex;
            }
            self.run_search(false, cx);
            return true;
        }
        false
    }

    fn search_bar(&self, cx: &mut Context<Self>) -> Option<Div> {
        let search = self.search.as_ref().filter(|search| search.open)?;
        let count = if let Some(error) = &search.error {
            error.clone()
        } else if search.input.read(cx).text.is_empty() {
            String::new()
        } else if let Some((index, count)) = search.archive_count {
            if count == 0 {
                "0/0".into()
            } else {
                format!("{}/{}", index + 1, count)
            }
        } else {
            search.result.as_ref().map_or("0/0".into(), |r| {
                if r.matches.is_empty() {
                    "0/0".into()
                } else {
                    format!("{}/{}", r.result_index + 1, r.matches.len())
                }
            })
        };
        let mut bar = div()
            .flex_none()
            .w_full()
            .h(px(40.))
            .px(px(12.))
            .flex()
            .items_center()
            .border_b_1()
            .border_color(rgb(0xe8e8e8))
            .bg(rgb(0xffffff))
            .text_color(rgb(0x2f3547))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .font_family("SF Mono")
                    .text_size(px(12.))
                    .child(search.input.clone()),
            )
            .child(
                div()
                    .min_w(px(42.))
                    .px(px(8.))
                    .text_size(px(11.))
                    .font_family("SF Mono")
                    .text_color(rgb(if search.error.is_some() {
                        0xb14542
                    } else {
                        0x606572
                    }))
                    .child(count),
            );
        for (index, label, active) in [
            (0, "Aa", search.options.case_sensitive),
            (1, ".*", search.options.regex),
            (2, "ab", search.options.whole_word),
            (3, "↑", false),
            (4, "↓", false),
            (5, "×", false),
        ] {
            bar = bar.child(
                div()
                    .id(SharedString::from(format!("terminal-search-{index}")))
                    .track_focus(&search.controls[index])
                    .w(px(24.))
                    .h(px(24.))
                    .ml(px(4.))
                    .rounded(px(4.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(11.))
                    .text_color(rgb(if active { 0xff8c00 } else { 0x606572 }))
                    .cursor_pointer()
                    .hover(|style| {
                        style
                            .bg(rgba(0x00000014))
                            .text_color(rgb(0x2f3547))
                            .rounded(px(6.))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if index == 5 {
                            this.close_search(window, cx);
                            return;
                        }
                        if let Some(search) = this.search.as_mut() {
                            match index {
                                0 => search.options.case_sensitive = !search.options.case_sensitive,
                                1 => search.options.regex = !search.options.regex,
                                2 => search.options.whole_word = !search.options.whole_word,
                                _ => {}
                            }
                        }
                        this.run_search(index == 3, cx);
                    }))
                    .child(label),
            );
        }
        Some(bar)
    }

    pub fn new(
        commands: RuntimeCommands,
        events: async_channel::Receiver<BridgeEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.spawn_in(window, async move |this, cx| {
            while let Ok(event) = events.recv().await {
                let applied = cx
                    .update(|window, cx| this.update(cx, |this, cx| this.apply(event, window, cx)));
                if !matches!(applied, Ok(Ok(()))) {
                    break;
                }
            }
        })
        .detach();
        let view = cx.weak_entity();
        let workspace_window = window.window_handle();
        // Cover the document even when window.blur() leaves no focused GPUI node.
        let keyboard = cx.intercept_keystrokes(move |event, window, cx| {
            if window.window_handle() == workspace_window {
                let _ = view.update(cx, |this, cx| this.tab_key(&event.keystroke, window, cx));
            }
        });
        Self {
            history_view: None,
            history_request: None,
            search: None,
            runtime_generation: 0,
            transport_connected: false,
            domain_ready: false,
            session_frames: HashSet::new(),
            search_after_restore: false,
            transport_fault: None,
            commands,
            hierarchy: None,
            scenes: Vec::new(),
            workspaces: Vec::new(),
            workspace_paths: Vec::new(),
            workspace_controls: HashMap::new(),
            workspace_drag_source: None,
            workspace_drag_press: None,
            workspace_drag_gesture: false,
            workspace_drag_target: None,
            workspace_drag_motion: HashMap::new(),
            workspace_bounds: HashMap::new(),
            workspace_group_bounds: HashMap::new(),
            workspace_hover: None,
            workspace_collapsed: HashSet::new(),
            workspace_menu: None,
            creating_workspace: false,
            tasks: Vec::new(),
            task_controls: HashMap::new(),
            task_bounds: HashMap::new(),
            task_hover: None,
            reveal_task: None,
            task_menu: None,
            sidebar_scroll: ScrollHandle::new(),
            sidebar_overflow: false,
            sidebar_scrollbar: SidebarScrollbarState::default(),
            notifications: Notifications::new(
                crate::runtime_bridge::LaunchOptions::from_args(std::env::args_os().skip(1))
                    .ok()
                    .map(|options| options.data_root.join("notification-sound")),
            ),
            notification_open: false,
            notification_scene_sessions: HashMap::new(),
            task_toast: None,
            toast_generation: 0,
            permission_menu: None,
            permission_changes: HashMap::new(),
            permission_identities: HashSet::new(),
            permission_toast: None,
            permission_toast_timer: None,
            tab_scroll: ScrollHandle::new(),
            tab_hover: None,
            tab_overflow: false,
            hidden_tabs: None,
            tab_menu: None,
            control_focus: CONTROLS
                .iter()
                .map(|&id| (id, cx.focus_handle().tab_stop(true)))
                .collect(),
            tab_controls: HashMap::new(),
            card_controls: HashMap::new(),
            remove_card_controls: HashMap::new(),
            fork_controls: HashMap::new(),
            fork_menu: None,
            fork_readiness: None,
            fork_readiness_generation: 0,
            fork_retries: HashMap::new(),
            keyboard_focus: false,
            dialog: None,
            dialog_controls: Vec::new(),
            dialog_subscription: None,
            pending_close: None,
            carousel_scroll: ScrollHandle::new(),
            motion_clock: Instant::now(),
            #[cfg(debug_assertions)]
            motion_trace: open_motion_trace(),
            card_motion: HashMap::new(),
            focus_follow_until: None,
            hovered: None,
            hover_follow_until: None,
            hover_baseline: None,
            hover_restore_at: None,
            pointer_position: None,
            edge_direction: 0,
            edge_blocked: false,
            edge_next_at: None,
            edge_target: None,
            edge_phase_start: 0.,
            edge_cruising: false,
            card_geometry: Vec::new(),
            geometry: HashMap::new(),
            geometry_dirty: HashSet::new(),
            reconnect_geometry: HashSet::new(),
            restore_geometry: false,
            column_capacity: None,
            pending_geometry: None,
            geometry_timer: 0,
            last_geometry_write: HashMap::new(),
            geometry_sequence: 0,
            geometry_writes: HashMap::new(),
            graph: Value::Null,
            level_parent_by_scene: HashMap::new(),
            carousel_owner: None,
            hovered_child_badge: None,
            terminals: HashMap::new(),
            recovery: HashMap::new(),
            recovery_controls: HashMap::new(),
            deferred_previews: HashMap::new(),
            deferred_controls: HashMap::new(),
            start_failure_controls: HashMap::new(),
            provider_restore_controls: HashMap::new(),
            pending_provider_restores: HashMap::new(),
            pending_loader_loads: HashMap::new(),
            #[cfg(target_os = "macos")]
            recovery_water: HashMap::new(),
            #[cfg(target_os = "macos")]
            recovery_water_visibility: None,
            storage_fault_sessions: HashSet::new(),
            order: Vec::new(),
            focused: None,
            pending_focus: None,
            pending_card_action: None,
            level_focus_request: None,
            copy_request_id: 0,
            resize_revisions: HashMap::new(),
            fault: None,
            subscriptions: vec![keyboard],
        }
    }

    fn show_toast(&mut self, message: impl Into<String>, cx: &mut Context<Self>) {
        self.toast_generation += 1;
        let generation = self.toast_generation;
        self.task_toast = Some(message.into());
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            let _ = this.update(cx, |this, cx| {
                if this.toast_generation == generation {
                    this.task_toast = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn toggle_notifications(&mut self, cx: &mut Context<Self>) {
        if self.notification_open {
            self.close_notifications(cx);
        } else {
            self.notifications.prune(notification_now());
            self.notification_open = true;
            cx.notify();
        }
    }

    fn close_notifications(&mut self, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.notification_open) {
            cx.notify();
        }
    }

    fn clear_notifications(&mut self, cx: &mut Context<Self>) {
        self.notifications.entries.clear();
        self.notifications.cooldowns.clear();
        cx.notify();
    }

    fn dismiss_notification(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(index) = self
            .notifications
            .entries
            .iter()
            .position(|entry| entry.id == id)
        {
            self.notifications.remove_at(index);
            cx.notify();
        }
    }

    fn toggle_notification_sound(&mut self, cx: &mut Context<Self>) {
        self.notifications
            .set_sound(!self.notifications.sound_enabled);
        cx.notify();
    }

    fn update_notifications(&mut self, graph: &mut Value) {
        if let Some((scene, task)) = self
            .hierarchy
            .as_ref()
            .and_then(|hierarchy| Some((hierarchy.scene.as_ref()?, hierarchy.task.as_ref()?)))
            && graph["sceneId"] == scene.id
        {
            let sessions = session_levels::nodes(graph)
                .iter()
                .filter(|node| node["archivedAt"].is_null())
                .filter_map(|node| node["sessionId"].as_str().map(str::to_owned))
                .collect();
            self.notification_scene_sessions
                .insert(scene.id.clone(), (task.id.clone(), sessions));
        }
        if let Some(Value::Array(events)) = graph
            .as_object_mut()
            .and_then(|graph| graph.remove("agentNotifications"))
        {
            let now = notification_now();
            let mut play_sound = false;
            for event in &events {
                play_sound |= self.notifications.push(event, self.focused.as_deref(), now);
            }
            if play_sound {
                play_notification_sound();
            }
        }
    }

    fn notification_target<'a>(
        &'a self,
        entry: &'a NotificationEntry,
    ) -> Option<(&'a str, &'a str)> {
        let session_id = entry.session_id.as_deref()?;
        let task = self.tasks.iter().find(|task| {
            Some(task.id.as_str()) == entry.task_id.as_deref()
                && entry
                    .workspace_id
                    .as_deref()
                    .is_none_or(|id| id == task.workspace_id)
                && task.archived_at.is_none()
                && self.workspaces.iter().any(|workspace| {
                    workspace.id == task.workspace_id && workspace.archived_at.is_none()
                })
        })?;
        let current = self.hierarchy.as_ref()?;
        let scene_id = entry
            .scene_id
            .as_deref()
            .or_else(|| {
                self.notification_scene_sessions
                    .iter()
                    .find_map(|(scene, (task_id, sessions))| {
                        (task_id == &task.id && sessions.contains(session_id))
                            .then_some(scene.as_str())
                    })
            })
            .or_else(|| {
                // An inactive scene's last selected session is also recorded by the runtime.
                current
                    .navigation
                    .session_by_scene
                    .iter()
                    .find_map(|(scene, session)| (session == session_id).then_some(scene.as_str()))
            })?;
        if self
            .notification_scene_sessions
            .get(scene_id)
            .is_some_and(|(task_id, sessions)| {
                task_id != &task.id || !sessions.contains(session_id)
            })
            || (current
                .task
                .as_ref()
                .is_some_and(|active| active.id == task.id)
                && !self.scenes.iter().any(|scene| scene.id == scene_id))
        {
            return None;
        }
        Some((scene_id, session_id))
    }

    fn open_notification(&mut self, id: u64, _window: &mut Window, cx: &mut Context<Self>) {
        let Some((scene_id, session_id)) = self
            .notifications
            .entries
            .iter()
            .find(|entry| entry.id == id)
            .and_then(|entry| self.notification_target(entry))
            .map(|(scene, session)| (scene.to_owned(), session.to_owned()))
        else {
            return;
        };
        // FocusSession activates the target's scene, task and workspace in one
        // runtime transaction. A removed target leaves the current view intact.
        self.send(
            Command::FocusSession {
                scene_id,
                session_id,
            },
            cx,
        );
        if self.pending_focus.is_some() {
            self.level_focus_request = Some(LevelFocusRequest::Notification(id));
        }
    }

    fn send(&mut self, command: Command, cx: &mut Context<Self>) {
        if matches!(
            &command,
            Command::CreateWorkspace { .. }
                | Command::SelectWorkspace { .. }
                | Command::ReorderPinnedWorkspace { .. }
                | Command::RemoveWorkspace { .. }
                | Command::CreateTask { .. }
                | Command::SelectTask { .. }
                | Command::DeleteTask { .. }
                | Command::SelectScene { .. }
                | Command::CreateCanvas { .. }
                | Command::CreateShellSibling { .. }
                | Command::CloseScene { .. }
                | Command::ReorderScene { .. }
        ) {
            self.flush_geometry(cx);
        }
        if let Command::FocusSession {
            scene_id,
            session_id,
        } = &command
        {
            self.level_focus_request = None;
            self.pending_focus = Some((scene_id.clone(), session_id.clone()));
        }
        if self
            .commands
            .send_at(self.runtime_generation, command)
            .is_err()
        {
            self.pending_focus = None;
            self.level_focus_request = None;
            // Offline/reconnecting rejection is expected; do not stack a fault per key.
            cx.notify();
        }
    }

    fn retire_transport(&mut self, generation: u64, cx: &mut Context<Self>) {
        let reason = crate::runtime_bridge::SendError::NotReady.to_string();
        self.fail_pending_forks(&reason);
        self.fail_pending_permission_modes(&reason, cx);
        self.permission_identities.clear();
        self.runtime_generation = generation;
        self.transport_connected = false;
        self.domain_ready = false;
        self.session_frames.clear();
        self.recovery.clear();
        self.deferred_previews.clear();
        self.deferred_controls.clear();
        self.recovery_controls.clear();
        self.start_failure_controls.clear();
        self.pending_provider_restores.clear();
        self.copy_request_id += 1;
        self.resize_revisions.clear();
        self.reconnect_geometry = self.geometry_dirty.clone();
        self.geometry_timer += 1;
        self.pending_geometry = None;
        self.geometry_writes.clear();
        self.last_geometry_write.clear();
        self.pending_focus = None;
        self.pending_card_action = None;
        self.level_focus_request = None;
        self.level_parent_by_scene.clear();
        self.refresh_level(cx);
        self.pending_close = None;
        self.creating_workspace = false;
        self.search_after_restore = self.search.as_ref().is_some_and(|search| search.open);
        if let Some(search) = &mut self.search {
            search.sequence += 1;
            search.result = None;
            search.error = None;
            search.archive = None;
            search.archive_pending = None;
            search.archive_count = None;
            search.dismissed_sequence = None;
            search.session_id = None;
        }
        self.history_request = None;
        if let Some(view) = self.history_view.take() {
            view.update(cx, |view, cx| view.retire(cx));
        }
        for terminal in self.terminals.values() {
            terminal.update(cx, |terminal, cx| {
                terminal.begin_transport_generation(generation, cx)
            });
        }
        #[cfg(target_os = "macos")]
        for water in self.recovery_water.values() {
            water.update(cx, |water, cx| water.set_generation(generation, cx));
        }
        cx.notify();
    }
    fn resend_terminal_sizes(&self, cx: &Context<Self>) {
        // The new owner also needs sizes retained from previously visited tabs.
        // A later Navigation may reuse that surface without a new paint size change.
        for (id, terminal) in &self.terminals {
            if let Some((cols, rows, cell_width_px, cell_height_px)) =
                terminal.read(cx).target_dimensions()
            {
                let _ = self.commands.send_at(
                    self.runtime_generation,
                    Command::Resize {
                        session_id: id.clone(),
                        cols,
                        rows,
                        cell_width_px,
                        cell_height_px,
                    },
                );
            }
        }
    }
    fn resume_search_after_frame(&mut self, cx: &mut Context<Self>) {
        if self.search_after_restore
            && self
                .focused
                .as_ref()
                .is_some_and(|id| !self.recovery_blocked(id))
        {
            self.search_after_restore = false;
            self.run_search(false, cx);
        }
    }
    fn reconcile_transport_geometry(
        &mut self,
        authority: &[GeometryUpdate],
        cx: &mut Context<Self>,
    ) {
        let desired = self
            .reconnect_geometry
            .iter()
            .filter_map(|key| self.geometry.get(key).cloned())
            .collect::<Vec<_>>();
        for saved in authority {
            if !self.geometry_dirty.contains(&saved.owner_key) {
                self.geometry.insert(saved.owner_key.clone(), saved.clone());
            }
        }
        for mut update in desired {
            // Ready/Navigation supplies authority only for its active scene.
            // An absent background/task projection is not a deletion signal.
            let Some(scene) = self
                .hierarchy
                .as_ref()
                .and_then(|h| h.scene.as_ref())
                .filter(|scene| scene.id == update.scene_id)
            else {
                continue;
            };
            let prefix = format!("session-group:{}:", scene.id);
            if !update
                .owner_key
                .strip_prefix(&prefix)
                .is_some_and(|owner| owner == "root" || self.graph_node(owner).is_some())
            {
                self.geometry_dirty.remove(&update.owner_key);
                self.reconnect_geometry.remove(&update.owner_key);
                self.geometry.remove(&update.owner_key);
                continue;
            }
            if authority.iter().any(|saved| {
                saved.owner_key == update.owner_key && saved.geometry == update.geometry
            }) {
                self.geometry_dirty.remove(&update.owner_key);
                self.reconnect_geometry.remove(&update.owner_key);
                continue;
            }
            update.layout_revision = if self
                .hierarchy
                .as_ref()
                .and_then(|h| h.scene.as_ref())
                .is_some_and(|s| s.id == update.scene_id)
            {
                self.graph["layoutRevision"]
                    .as_f64()
                    .unwrap_or(scene.layout_revision as f64)
            } else {
                scene.layout_revision as f64
            };
            update.now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64;
            self.geometry
                .insert(update.owner_key.clone(), update.clone());
            self.reconnect_geometry.remove(&update.owner_key);
            self.write_geometry(update, 0, cx);
        }
    }
    fn close_invalid_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let invalid = self.dialog.as_ref().is_some_and(|dialog| match dialog {
            TabDialog::RemoveWorkspace { workspace_id, .. } => {
                !self.workspaces.iter().any(|w| &w.id == workspace_id)
            }
            TabDialog::RenameTask { task_id, .. } | TabDialog::DeleteTask { task_id, .. } => {
                !self.tasks.iter().any(|t| &t.id == task_id)
            }
            TabDialog::Rename { scene_id, .. } | TabDialog::Close { scene_id, .. } => {
                !self.scenes.iter().any(|s| &s.id == scene_id)
            }
            TabDialog::CloseSession {
                scene_id,
                session_id,
                ..
            }
            | TabDialog::LastSession {
                scene_id,
                session_id,
                ..
            } => self.active_scene_id() != Some(scene_id) || self.graph_node(session_id).is_none(),
            TabDialog::RemoveSession {
                scene_id,
                session_id,
                scope,
            } => {
                self.active_scene_id() != Some(scene_id)
                    || (self.is_stopped_session(session_id)
                        && !self.stopped_remove_available(session_id))
                    || self
                        .removal_dialog_text(session_id, *scope)
                        .is_none_or(|text| {
                            text.choices.is_some() != (self.dialog_controls.len() == 5)
                        })
            }
            TabDialog::SessionLoader(state) => {
                self.active_scene_id() != Some(state.scene_id.as_str())
                    || self.graph_node(&state.session_id).is_none_or(|node| {
                        !node["archivedAt"].is_null()
                            || node["cwd"] != state.cwd
                            || (state.history
                                && trimmed_graph_text(&node["historyProviderSessionId"])
                                    != state.selected.as_deref())
                    })
            }
            TabDialog::PermissionMode(state) => !self.permission_confirmation_valid(state),
            TabDialog::Branch(state) => {
                self.active_scene_id() != Some(state.scene_id.as_str())
                    || self
                        .graph_node(&state.session_id)
                        .is_none_or(|node| !node["archivedAt"].is_null())
            }
        });
        if invalid {
            self.close_dialog(window, cx);
        }
    }
    fn apply(&mut self, envelope: BridgeEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.commands.accepts(&envelope) {
            return;
        }
        let generation = envelope.generation();
        if generation != self.runtime_generation {
            self.retire_transport(generation, cx);
        }
        let event = match envelope {
            BridgeEvent::TransportDiagnostic { message, .. } => {
                self.fail_pending_forks(&message);
                self.fail_pending_permission_modes(&message, cx);
                self.reconcile_permission_overlay(window, cx);
                self.fail_pending_loader_commands(message.clone());
                self.transport_fault = Some((generation, message));
                cx.notify();
                return;
            }
            BridgeEvent::Connection { phase, .. } => {
                self.transport_connected = matches!(phase, ConnectionPhase::Ready);
                if !self.transport_connected {
                    let message = match &phase {
                        ConnectionPhase::Blocked(block) => block.message.clone(),
                        _ => crate::runtime_bridge::SendError::NotReady.to_string(),
                    };
                    self.fail_pending_forks(&message);
                    self.fail_pending_permission_modes(&message, cx);
                    self.fail_pending_loader_commands(message);
                    self.domain_ready = false;
                    self.pending_card_action = None;
                    self.pending_provider_restores.clear();
                    self.session_frames.clear();
                    if matches!(&self.dialog, Some(TabDialog::RemoveSession { session_id, .. })
                        if self.is_stopped_session(session_id))
                    {
                        self.close_invalid_dialog(window, cx);
                    }
                    self.sync_terminal_input(window, cx);
                }
                self.reconcile_permission_overlay(window, cx);
                #[cfg(target_os = "macos")]
                self.prune_recovery_water(window, cx);
                cx.notify();
                return;
            }
            BridgeEvent::Runtime { event, .. } => event,
        };
        let fresh_ready = matches!(&event, Event::Ready { .. });
        if fresh_ready {
            self.domain_ready = true;
            self.transport_connected = true;
            self.transport_fault = None;
        }
        match &event {
            Event::StorageFault { session_id, .. } => {
                self.storage_fault_sessions.insert(session_id.clone());
            }
            Event::StorageRecovered { session_id } => {
                self.storage_fault_sessions.remove(session_id);
            }
            _ => {}
        }
        match event {
            Event::Workspaces(workspaces) => {
                self.workspace_drag_motion
                    .retain(|id, _| workspaces.iter().any(|w| &w.id == id));
                self.workspace_controls
                    .retain(|id, _| workspaces.iter().any(|workspace| &workspace.id == id));
                self.workspace_group_bounds
                    .retain(|id, _| workspaces.iter().any(|workspace| &workspace.id == id));
                self.workspace_bounds
                    .retain(|id, _| workspaces.iter().any(|workspace| &workspace.id == id));
                self.workspace_collapsed
                    .retain(|id| workspaces.iter().any(|workspace| &workspace.id == id));
                if self
                    .workspace_menu
                    .as_ref()
                    .is_some_and(|(id, _, _)| !workspaces.iter().any(|w| &w.id == id))
                {
                    self.workspace_menu = None;
                }
                for workspace in &workspaces {
                    self.workspace_controls
                        .entry(workspace.id.clone())
                        .or_insert_with(|| {
                            [
                                cx.focus_handle().tab_stop(true),
                                cx.focus_handle().tab_stop(true),
                                cx.focus_handle().tab_stop(true),
                                cx.focus_handle().tab_stop(true),
                            ]
                        });
                }
                self.workspaces = workspaces;
            }
            Event::WorkspacePaths(paths) => {
                self.workspace_paths = paths;
                if matches!(&self.dialog, Some(TabDialog::RemoveSession { session_id, .. })
                    if self.is_stopped_session(session_id))
                {
                    self.close_invalid_dialog(window, cx);
                }
                self.sync_terminal_input(window, cx);
            }
            Event::WorkspaceActionResult {
                action,
                workspace_id,
                error,
            } => match action.as_str() {
                "create" => {
                    self.creating_workspace = false;
                    if error.is_some() {
                        self.show_toast("工作空间添加失败，请重试", cx);
                    } else if self.control_focus["new-workspace"].is_focused(window) {
                        // Let the following navigation focus the created Shell.
                        window.blur();
                    }
                }
                "relink" => {
                    if error.is_some() {
                        self.show_toast("工作目录恢复失败，请重新选择", cx);
                    } else {
                        let name = workspace_id
                            .as_deref()
                            .and_then(|id| self.workspaces.iter().find(|w| w.id == id))
                            .map(|w| w.name.as_str())
                            .unwrap_or("");
                        self.show_toast(format!("已恢复 {name} 的工作目录"), cx);
                    }
                }
                "pin" => {
                    if error.is_some() {
                        self.show_toast("工作空间置顶状态更新失败", cx);
                    } else {
                        let pinned = workspace_id
                            .as_deref()
                            .and_then(|id| self.workspaces.iter().find(|w| w.id == id))
                            .is_some_and(|w| w.is_pinned);
                        self.show_toast(
                            if pinned {
                                "工作空间已置顶"
                            } else {
                                "已取消工作空间置顶"
                            },
                            cx,
                        );
                    }
                }
                _ => {}
            },
            Event::Tasks(tasks) => {
                self.notification_scene_sessions
                    .retain(|_, (task_id, _)| tasks.iter().any(|task| &task.id == task_id));
                self.task_controls
                    .retain(|id, _| tasks.iter().any(|task| &task.id == id));
                self.task_bounds
                    .retain(|id, _| tasks.iter().any(|task| &task.id == id));
                for task in &tasks {
                    self.task_controls
                        .entry(task.id.clone())
                        .or_insert_with(|| {
                            [
                                cx.focus_handle().tab_stop(true),
                                cx.focus_handle().tab_stop(true),
                            ]
                        });
                }
                self.tasks = tasks;
            }
            Event::TaskRenameResult {
                task_id,
                title,
                error,
            } => {
                if let Some(TabDialog::RenameTask {
                    task_id: current,
                    failure,
                    ..
                }) = &mut self.dialog
                    && *current == task_id
                {
                    if let Some(_error) = error {
                        *failure = Some((title, "重命名失败：名称为空或已存在".into()));
                    } else {
                        self.close_dialog(window, cx);
                        self.show_toast("工作台已重命名", cx);
                    }
                }
            }
            Event::Ready {
                hierarchy,
                scenes,
                graph,
                geometry,
            }
            | Event::Navigation {
                hierarchy,
                scenes,
                graph,
                geometry,
            } => {
                let next_workspace = hierarchy.workspace.as_ref().map(|workspace| &workspace.id);
                let workspace_changed = self
                    .hierarchy
                    .as_ref()
                    .and_then(|h| h.workspace.as_ref())
                    .map(|w| &w.id)
                    != next_workspace;
                if workspace_changed && let Some(workspace_id) = next_workspace {
                    self.workspace_collapsed.remove(workspace_id);
                }
                let next_scene = hierarchy.scene.as_ref().map(|scene| &scene.id);
                let scene_changed = self
                    .hierarchy
                    .as_ref()
                    .and_then(|h| h.scene.as_ref())
                    .map(|s| &s.id)
                    != next_scene;
                let next_focus = hierarchy.session.as_ref().map(|session| session.id.clone());
                let focus_changed = self.focused != next_focus;
                let initializing = self.hierarchy.is_none();
                let focus_confirmed =
                    self.pending_focus.as_ref().is_some_and(|(scene, session)| {
                        Some(scene) == next_scene && Some(session) == next_focus.as_ref()
                    });
                let level_focus_request = if scene_changed || focus_confirmed {
                    self.pending_focus = None;
                    self.level_focus_request.take().filter(|_| focus_confirmed)
                } else {
                    None
                };
                let return_to_terminal =
                    level_focus_request == Some(LevelFocusRequest::ReturnParent);
                let notification_focus = match level_focus_request {
                    Some(LevelFocusRequest::Notification(id)) => Some(id),
                    _ => None,
                };
                if let Some((id, session_id)) = notification_focus.zip(next_focus.as_deref()) {
                    if let Some(entry) = self.notifications.entries.iter_mut().find(|entry| {
                        entry.id == id && entry.session_id.as_deref() == Some(session_id)
                    }) {
                        entry.read = true;
                    }
                    if let Some(scene_id) = next_scene {
                        let parent =
                            session_levels::node(session_levels::nodes(&graph), session_id)
                                .and_then(session_levels::parent_id)
                                .map(str::to_owned);
                        self.level_parent_by_scene.insert(scene_id.clone(), parent);
                    }
                    self.close_notifications(cx);
                    window.blur();
                }
                if scene_changed {
                    #[cfg(target_os = "macos")]
                    self.retain_recovery_water(window, cx, |_, _| false);
                    // A newly mounted source carousel starts at its current styles.
                    for saved in &geometry {
                        if !self.geometry_dirty.contains(&saved.owner_key) {
                            self.geometry.insert(saved.owner_key.clone(), saved.clone());
                        }
                    }
                }
                if !scene_changed && focus_changed && level_focus_request.is_none() {
                    self.restore_geometry = false;
                }
                if scene_changed || focus_changed {
                    let at = self.motion_clock.elapsed().as_secs_f64() * 1000.;
                    let from_preview = !scene_changed && self.hovered == next_focus;
                    self.focus_follow_until = if from_preview { None } else { Some(at + 440.) };
                    self.hovered = None;
                    self.hover_follow_until = None;
                    self.hover_baseline = None;
                    self.hover_restore_at = None;
                    #[cfg(debug_assertions)]
                    if let Some(trace) = &self.motion_trace {
                        record_motion(
                            trace,
                            &serde_json::json!({"kind":"focus","at":at,"from":self.focused,"to":next_focus,"sceneChanged":scene_changed}),
                        );
                    }
                }
                self.tab_controls
                    .retain(|id, _| scenes.iter().any(|scene| &scene.id == id));
                for scene in &scenes {
                    self.tab_controls
                        .entry(scene.id.clone())
                        .or_insert_with(|| {
                            [
                                cx.focus_handle().tab_stop(true),
                                cx.focus_handle().tab_stop(true),
                            ]
                        });
                }
                let task_changed = self
                    .hierarchy
                    .as_ref()
                    .and_then(|h| h.task.as_ref())
                    .map(|task| &task.id)
                    != hierarchy.task.as_ref().map(|task| &task.id);
                let active_collapsed = hierarchy
                    .workspace
                    .as_ref()
                    .is_some_and(|w| self.workspace_collapsed.contains(&w.id));
                if task_changed && !active_collapsed {
                    self.reveal_task = hierarchy.task.as_ref().map(|task| task.id.clone());
                }
                self.scenes = scenes;
                let search_focus_changed = self.focused != next_focus;
                self.focused = next_focus;
                if search_focus_changed {
                    self.run_search(false, cx);
                }
                self.hierarchy = Some(hierarchy);
                self.finish_card_action(&graph["sessionCardAction"]);
                self.replace_fork_graph(graph, window, cx);
                self.refresh_terminals(window, cx);
                if fresh_ready {
                    self.session_frames.clear();
                    self.recovery.clear();
                    self.deferred_previews.clear();
                    self.deferred_controls.clear();
                    self.start_failure_controls.clear();
                    for id in session_levels::nodes(&self.graph)
                        .iter()
                        .filter(|node| {
                            node["archivedAt"].is_null()
                                && Self::provider_restore_state(node) != Some("failed")
                                && fork_failure_copy(node).is_none()
                        })
                        .filter_map(|node| node["sessionId"].as_str())
                    {
                        self.recovery
                            .insert(id.to_owned(), RecoveryState::Restoring);
                    }
                    self.resend_terminal_sizes(cx);
                }
                if fresh_ready || self.dialog.as_ref().is_some_and(TabDialog::is_session_card) {
                    self.close_invalid_dialog(window, cx);
                }
                self.reconcile_transport_geometry(&geometry, cx);
                self.sync_terminal_input(window, cx);
                if scene_changed
                    && let Some(index) = self.scenes.iter().position(|scene| {
                        self.hierarchy
                            .as_ref()
                            .and_then(|h| h.scene.as_ref())
                            .is_some_and(|active| active.id == scene.id)
                    })
                {
                    self.tab_scroll.scroll_to_item(index);
                }
                if level_focus_request != Some(LevelFocusRequest::OpenChildren)
                    && (initializing
                        || scene_changed
                        || focus_changed
                        || return_to_terminal
                        || notification_focus.is_some())
                    && self.dialog.is_none()
                    && self.active_path_valid()
                    && (return_to_terminal
                        || self
                            .focused
                            .as_ref()
                            .is_none_or(|id| !self.recovery_blocked(id)))
                    && (return_to_terminal
                        || notification_focus.is_some()
                        || window.focused(cx).is_none()
                        || (workspace_changed
                            && self
                                .workspace_controls
                                .values()
                                .any(|controls| controls[0].is_focused(window)))
                        || (task_changed
                            && self
                                .task_controls
                                .values()
                                .any(|controls| controls[0].is_focused(window)))
                        || self
                            .terminals
                            .values()
                            .any(|terminal| terminal.read(cx).focus.is_focused(window)))
                    && let Some(terminal) = self
                        .focused
                        .as_ref()
                        .filter(|id| self.active_level_focus() == Some(id.as_str()))
                        .and_then(|id| self.terminals.get(id))
                {
                    window.focus(&terminal.read(cx).focus);
                }
            }
            Event::ScenesDisposed(ids) => {
                if self
                    .history_request
                    .as_ref()
                    .is_some_and(|request| ids.contains(&request.session_id))
                {
                    self.dismiss_history_context(cx);
                }
                for id in ids {
                    for (_, sessions) in self.notification_scene_sessions.values_mut() {
                        sessions.remove(&id);
                    }
                    self.clear_start_failure_focus(&id, window);
                    self.terminals.remove(&id);
                    self.recovery.remove(&id);
                    self.deferred_previews.remove(&id);
                    self.deferred_controls.remove(&id);
                    self.recovery_controls.remove(&id);
                    self.start_failure_controls.remove(&id);
                    self.provider_restore_controls.remove(&id);
                    self.pending_provider_restores.remove(&id);
                    self.pending_loader_loads.remove(&id);
                    self.permission_changes.remove(&id);
                    self.permission_identities.remove(&id);
                    self.storage_fault_sessions.remove(&id);
                    self.card_controls.remove(&id);
                    self.remove_card_controls.remove(&id);
                    self.card_motion.remove(&id);
                    self.resize_revisions.remove(&id);
                    self.session_frames.remove(&id);
                }
            }
            Event::SceneClosePreview {
                scene_id,
                graph,
                is_last,
            } => {
                if self.pending_close.as_ref() == Some(&scene_id) {
                    self.pending_close = None;
                    self.dialog_controls = (0..if is_last { 2 } else { 3 })
                        .map(|_| cx.focus_handle().tab_stop(true))
                        .collect();
                    window.focus(&self.dialog_controls[0]);
                    self.dialog = Some(TabDialog::Close {
                        scene_id,
                        graph,
                        is_last,
                    });
                }
            }
            Event::GeometryResult {
                sequence,
                update,
                error,
            } => {
                #[cfg(debug_assertions)]
                if let Some(trace) = &self.motion_trace {
                    record_motion(
                        trace,
                        &serde_json::json!({"kind":"geometry-write","at":self.motion_clock.elapsed().as_secs_f64()*1000.,"sequence":sequence,"owner":update.owner_key,"geometry":update.geometry,"error":error}),
                    );
                }
                let current_attempt = self
                    .geometry_writes
                    .get(&update.owner_key)
                    .filter(|(current, _)| *current == sequence)
                    .map(|(_, attempt)| *attempt);
                let desired = self
                    .geometry
                    .get(&update.owner_key)
                    .is_some_and(|desired| desired.geometry == update.geometry)
                    && !self
                        .pending_geometry
                        .as_ref()
                        .is_some_and(|pending| pending.owner_key == update.owner_key);
                if desired && current_attempt.is_some() && error.is_none() {
                    self.geometry_dirty.remove(&update.owner_key);
                } else if desired && let Some(attempt @ 0..=2) = current_attempt {
                    let generation = self.runtime_generation;
                    let timer = cx.background_executor().timer(Duration::from_millis(120));
                    cx.spawn(async move |this, cx| {
                        timer.await;
                        let _ = this.update(cx, |this, cx| {
                            if this.runtime_generation == generation
                                && this
                                    .geometry_writes
                                    .get(&update.owner_key)
                                    .is_some_and(|(current, _)| *current == sequence)
                                && this
                                    .geometry
                                    .get(&update.owner_key)
                                    .is_some_and(|desired| desired.geometry == update.geometry)
                                && !this
                                    .pending_geometry
                                    .as_ref()
                                    .is_some_and(|pending| pending.owner_key == update.owner_key)
                            {
                                let mut retry = update;
                                if this
                                    .hierarchy
                                    .as_ref()
                                    .and_then(|h| h.scene.as_ref())
                                    .is_some_and(|s| s.id == retry.scene_id)
                                {
                                    retry.layout_revision = this.graph["layoutRevision"]
                                        .as_f64()
                                        .unwrap_or(retry.layout_revision);
                                }
                                this.write_geometry(retry, attempt + 1, cx);
                            }
                        });
                    })
                    .detach();
                }
            }
            Event::NavigationFailed {
                message,
                attempted_focus,
            } => {
                if attempted_focus.is_none() {
                    self.pending_card_action = None;
                }
                if attempted_focus.is_some() && self.pending_focus == attempted_focus {
                    self.pending_focus = None;
                    if matches!(
                        self.level_focus_request.take(),
                        Some(LevelFocusRequest::Notification(_))
                    ) {
                        cx.notify();
                        return;
                    }
                }
                self.fault = Some(message);
            }
            Event::Graph(mut graph) => {
                let action = &graph["sessionCardAction"];
                if action["outcome"] == "hide-window" {
                    if self.finish_card_action(action)
                        && self.dialog.is_none()
                        && let (Some(scene_id), Some(session_id)) =
                            (action["sceneId"].as_str(), action["sessionId"].as_str())
                        && self.active_scene_id() == Some(scene_id)
                        && self.level_focus() == Some(session_id)
                    {
                        self.graph["workspaceSessionCount"] =
                            graph["workspaceSessionCount"].clone();
                        self.open_last_session(
                            scene_id.to_owned(),
                            session_id.to_owned(),
                            window,
                            cx,
                        );
                    }
                    self.update_notifications(&mut graph);
                    return;
                }
                let loader_results = graph.as_object_mut().map(|graph| {
                    [
                        graph.remove("providerSessionCatalogResults"),
                        graph.remove("providerSessionDetailResults"),
                        graph.remove("providerSessionLoadErrors"),
                    ]
                });
                self.replace_fork_graph(graph, window, cx);
                self.refresh_terminals(window, cx);
                if self.dialog.as_ref().is_some_and(TabDialog::is_session_card) {
                    self.close_invalid_dialog(window, cx);
                }
                if let Some([catalog, details, errors]) = loader_results {
                    let rows = |result: Option<Value>| match result {
                        Some(Value::Array(rows)) => rows,
                        _ => Vec::new(),
                    };
                    self.finish_session_loader(&rows(errors), window, cx);
                    self.loader_detail_result(&rows(details), cx);
                    self.loader_catalog_result(&rows(catalog), cx);
                }
                self.sync_terminal_input(window, cx);
            }
            Event::ClipboardWrite { session_id, text } => {
                // The active card belongs to the visible scene even while the
                // OS window is obscured or a dialog owns keyboard focus.
                if self.focused.as_ref() == Some(&session_id)
                    && self.graph_node(&session_id).is_some()
                {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            }
            Event::History(reply) => self.apply_history(reply, window, cx),
            Event::SearchResults {
                session_id,
                sequence,
                result,
            } => {
                self.apply_search_results(session_id, sequence, result, window, cx);
            }
            Event::Copied { request_id, text } => {
                if request_id == self.copy_request_id && !text.is_empty() {
                    cx.write_to_clipboard(ClipboardItem::new_string(text));
                }
            }
            Event::Frame {
                session_id,
                screen,
                selection,
                modes,
                sequence,
                scroll_request_id,
            } => {
                let was_usable = !self.recovery_blocked(&session_id);
                if let Some(terminal) = self.terminals.get(&session_id).cloned() {
                    let valid = self.domain_ready
                        && screen.cols > 0
                        && screen.rows > 0
                        && screen.cells.len() == screen.cols * screen.rows
                        && terminal.read(cx).target_dimensions().is_some_and(
                            |(cols, rows, _, _)| {
                                screen.cols == cols as usize && screen.rows == rows as usize
                            },
                        );
                    if valid {
                        self.session_frames.insert(session_id.clone());
                    } else if !was_usable {
                        self.session_frames.remove(&session_id);
                    }
                    terminal.update(cx, |terminal, cx| {
                        terminal.set_screen(screen, modes, selection, scroll_request_id, cx)
                    });
                    self.sync_terminal_input(window, cx);
                    self.resume_search_after_frame(cx);
                    // The reference live-output callback restores only an unclaimed
                    // active terminal; dialogs and focused controls retain ownership.
                    if self.focused.as_ref() == Some(&session_id)
                        && self.order.contains(&session_id)
                        && self.dialog.is_none()
                        && self.active_path_valid()
                        && !self.recovery_blocked(&session_id)
                        && window.focused(cx).is_none()
                    {
                        window.focus(&terminal.read(cx).focus);
                    }
                }
                let _ = self.commands.send_at(
                    generation,
                    Command::Applied {
                        session_id,
                        sequence,
                    },
                );
            }
            Event::Recovery { session_id, state } => {
                if self.is_stopped_session(&session_id) {
                    return;
                }
                if !matches!(&state, RecoveryState::Ready)
                    || (self.recovery_blocked(&session_id)
                        && !self
                            .terminals
                            .get(&session_id)
                            .is_some_and(|t| t.read(cx).frame_matches_target()))
                {
                    self.session_frames.remove(&session_id);
                }
                if matches!(state, RecoveryState::Failed(_)) {
                    self.recovery_controls
                        .entry(session_id.clone())
                        .or_insert_with(|| cx.focus_handle().tab_stop(true));
                } else if self
                    .recovery_controls
                    .get(&session_id)
                    .is_some_and(|focus| focus.is_focused(window))
                {
                    window.blur();
                }
                if matches!(state, RecoveryState::StartFailed(_)) {
                    self.start_failure_controls
                        .entry(session_id.clone())
                        .or_insert_with(|| {
                            [
                                cx.focus_handle().tab_stop(true),
                                cx.focus_handle().tab_stop(true),
                            ]
                        });
                } else {
                    self.clear_start_failure_focus(&session_id, window);
                    self.start_failure_controls.remove(&session_id);
                }
                if !matches!(state, RecoveryState::Ready)
                    && self
                        .terminals
                        .get(&session_id)
                        .is_some_and(|terminal| terminal.read(cx).focus.is_focused(window))
                {
                    window.blur();
                }
                if state == RecoveryState::Deferred {
                    self.deferred_controls
                        .entry(session_id.clone())
                        .or_insert_with(|| {
                            std::array::from_fn(|_| cx.focus_handle().tab_stop(true))
                        });
                } else {
                    self.deferred_previews.remove(&session_id);
                    if let Some(controls) = self.deferred_controls.remove(&session_id)
                        && controls.iter().any(|focus| focus.is_focused(window))
                    {
                        window.blur();
                    }
                }
                self.recovery.insert(session_id.clone(), state);
                self.sync_terminal_input(window, cx);
                // The replacement's first frame can precede Ready, including
                // when the CLI is waiting for its own bypass confirmation.
                if self
                    .permission_changes
                    .get(&session_id)
                    .is_some_and(|change| change.outcome == PermissionOutcome::Pending)
                    && self.active_level_focus() == Some(session_id.as_str())
                    && self.dialog.is_none()
                    && self.active_path_valid()
                    && !self.recovery_blocked(&session_id)
                    && window.focused(cx).is_none()
                    && let Some(terminal) = self.terminals.get(&session_id)
                {
                    window.focus(&terminal.read(cx).focus);
                }
                self.resume_search_after_frame(cx);
            }
            Event::StorageFault { message, .. } => self.fault = Some(message),
            Event::DeferredRecoveryPreview {
                session_id,
                cwd,
                preview,
            } => {
                if let Some(state) = self.deferred_previews.get_mut(&session_id)
                    && state.cwd == cwd
                {
                    state.loading = false;
                    state.result = preview;
                }
            }
            Event::Diagnostic(message) => {
                // Launch failures already carry this same diagnostic in their card.
                if !self.recovery.values().any(|state| {
                    matches!(state, RecoveryState::StartFailed(error) | RecoveryState::Failed(error) if error == &message)
                }) {
                    self.fault = Some(message);
                }
            }
            Event::StorageRecovered { .. } => self.fault = None,
            Event::StartupBlocked(_) | Event::Closed => {}
        }
        self.reconcile_permission_overlay(window, cx);
        #[cfg(target_os = "macos")]
        self.prune_recovery_water(window, cx);
        cx.notify();
    }

    fn refresh_terminals(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let nodes = session_levels::nodes(&self.graph);
        self.pending_provider_restores.retain(|id, pending| {
            session_levels::node(nodes, id).is_some_and(|node| {
                node["archivedAt"].is_null()
                    && Self::provider_restore_state(node) == Some("failed")
                    && node["providerSpawnRevision"].as_u64() == pending.revision
            })
        });
        for node in nodes {
            let Some(id) = node["sessionId"].as_str() else {
                continue;
            };
            self.remove_card_controls
                .entry(id.to_owned())
                .or_insert_with(|| cx.focus_handle().tab_stop(true));
            if node["archivedAt"].is_null()
                && (node["currentMode"] == "claude-code"
                    || session_levels::parent_id(node).is_some()
                    || fork_failure_copy(node).is_some())
            {
                self.fork_controls
                    .entry(id.to_owned())
                    .or_insert_with(|| std::array::from_fn(|_| cx.focus_handle().tab_stop(true)));
            }
            if node["archivedAt"].is_null() && Self::provider_restore_state(node) == Some("failed")
            {
                self.provider_restore_controls
                    .entry(id.to_owned())
                    .or_insert_with(|| {
                        [
                            cx.focus_handle().tab_stop(true),
                            cx.focus_handle().tab_stop(true),
                        ]
                    });
            } else if let Some(controls) = self.provider_restore_controls.remove(id)
                && controls.iter().any(|focus| focus.is_focused(window))
            {
                window.blur();
            }
            if !node["archivedAt"].is_null() {
                let terminal = self.terminals.remove(id);
                let retry = self.recovery_controls.remove(id);
                self.clear_start_failure_focus(id, window);
                self.start_failure_controls.remove(id);
                if terminal.is_some_and(|terminal| terminal.read(cx).focus.is_focused(window))
                    || retry.is_some_and(|focus| focus.is_focused(window))
                {
                    window.blur();
                }
                self.card_controls.remove(id);
                self.recovery.remove(id);
                self.deferred_previews.remove(id);
                self.deferred_controls.remove(id);
                self.session_frames.remove(id);
                self.resize_revisions.remove(id);
                continue;
            }
            if self.terminals.contains_key(id) {
                continue;
            }
            let disabled = !self.active_path_valid();
            let terminal = cx.new(|cx| {
                let mut surface = TerminalSurface::new(cx);
                surface.begin_transport_generation(self.runtime_generation, cx);
                surface.set_input_disabled(disabled || self.recovery_blocked(id), cx);
                surface
            });
            self.card_controls
                .insert(id.to_owned(), cx.focus_handle().tab_stop(true));
            let session_id = id.to_owned();
            cx.subscribe(&terminal, move |this, source, event: &SurfaceEvent, cx| {
                if this.terminals.get(&session_id) == Some(&source) {
                    this.handle_surface_event(&session_id, event, cx);
                }
            })
            .detach();
            self.terminals.insert(id.to_owned(), terminal);
        }
        if self
            .history_request
            .as_ref()
            .is_some_and(|request| self.is_stopped_session(&request.session_id))
        {
            self.dismiss_history_context(cx);
        }
        self.refresh_level(cx);
        self.fork_controls
            .retain(|id, _| self.terminals.contains_key(id));
    }

    fn handle_surface_event(
        &mut self,
        session_id: &str,
        event: &SurfaceEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(terminal) = self.terminals.get(session_id) else {
            return;
        };
        let terminal_id = terminal.entity_id();
        if event.generation != self.runtime_generation
            || !self.domain_ready
            || self.commands.connection() != (event.generation, ConnectionPhase::Ready)
        {
            return;
        }
        if !matches!(event.kind, SurfaceEventKind::Resize { .. })
            && self.recovery_blocked(session_id)
        {
            return;
        }
        let generation = event.generation;
        let session_id = session_id.to_owned();
        // Only an unqualified generation is protected by the first-frame gate.
        // An ordinary resize of an already usable terminal never starts recovery.
        if matches!(&event.kind, SurfaceEventKind::Resize { .. })
            && self.recovery_blocked(&session_id)
        {
            self.session_frames.remove(&session_id);
            if let Some(terminal) = self.terminals.get(&session_id) {
                terminal.update(cx, |terminal, cx| {
                    terminal.set_transport_available(false, cx);
                    terminal.set_input_disabled(true, cx);
                });
            }
        }
        match &event.kind {
            SurfaceEventKind::Select(action) => self.send(
                Command::Select {
                    session_id: session_id.clone(),
                    action: *action,
                },
                cx,
            ),
            SurfaceEventKind::Copy { interrupt_if_empty } => {
                self.copy_request_id += 1;
                self.send(
                    Command::CopySelection {
                        session_id: session_id.clone(),
                        request_id: self.copy_request_id,
                        interrupt_if_empty: *interrupt_if_empty,
                    },
                    cx,
                );
            }
            SurfaceEventKind::Input(bytes) => self.send(
                Command::Input {
                    session_id: session_id.clone(),
                    bytes: bytes.clone(),
                    defer_ordering: true,
                },
                cx,
            ),
            SurfaceEventKind::ScrollSelection(lines) => self.send(
                Command::ScrollSelection {
                    session_id: session_id.clone(),
                    lines: *lines,
                },
                cx,
            ),
            SurfaceEventKind::Scroll(lines) => self.send(
                Command::Scroll {
                    session_id: session_id.clone(),
                    lines: *lines,
                },
                cx,
            ),
            SurfaceEventKind::ScrollTo { row, request_id } => self.send(
                Command::ScrollTo {
                    session_id: session_id.clone(),
                    row: *row,
                    request_id: *request_id,
                },
                cx,
            ),
            SurfaceEventKind::Resize {
                cols,
                rows,
                cell_width_px,
                cell_height_px,
            } => {
                let revision = self.resize_revisions.entry(session_id.clone()).or_default();
                *revision += 1;
                let revision = *revision;
                let id = session_id.clone();
                let geometry = (*cols, *rows, *cell_width_px, *cell_height_px);
                let timer = cx.background_executor().timer(Duration::from_millis(80));
                cx.spawn(async move |this, cx| {
                    timer.await;
                    let _ = this.update(cx, |this, cx| {
                        if this.runtime_generation != generation {
                            return;
                        }
                        if this.resize_revisions.get(&id) != Some(&revision) {
                            return;
                        }
                        if this.terminals.get(&id).map(Entity::entity_id) != Some(terminal_id) {
                            return;
                        }
                        this.send(
                            Command::Resize {
                                session_id: id,
                                cols: geometry.0,
                                rows: geometry.1,
                                cell_width_px: geometry.2,
                                cell_height_px: geometry.3,
                            },
                            cx,
                        );
                    });
                })
                .detach();
            }
        }
    }

    pub fn flush_geometry(&mut self, cx: &mut Context<Self>) {
        self.geometry_timer += 1;
        if let Some(update) = self.pending_geometry.take() {
            self.last_geometry_write.insert(
                update.owner_key.clone(),
                self.motion_clock.elapsed().as_secs_f64() * 1000.,
            );
            self.write_geometry(update, 0, cx);
        }
    }

    fn write_geometry(&mut self, update: GeometryUpdate, attempt: u8, cx: &mut Context<Self>) {
        if self.active_scene_id() == Some(update.scene_id.as_str())
            && update.geometry["focusedSessionId"]
                .as_str()
                .is_some_and(|id| self.is_stopped_session(id))
        {
            self.geometry_dirty.remove(&update.owner_key);
            self.reconnect_geometry.remove(&update.owner_key);
            return;
        }
        self.geometry_sequence += 1;
        let sequence = self.geometry_sequence;
        self.geometry_writes
            .insert(update.owner_key.clone(), (sequence, attempt));
        self.send(Command::PutGeometry { sequence, update }, cx);
    }

    fn geometry_focus(&self) -> Option<&str> {
        self.pending_focus
            .as_ref()
            .filter(|(scene_id, _)| {
                self.hierarchy
                    .as_ref()
                    .and_then(|hierarchy| hierarchy.scene.as_ref())
                    .is_some_and(|scene| &scene.id == scene_id)
            })
            .map(|(_, session_id)| session_id.as_str())
            .or(self.focused.as_deref())
            .filter(|id| !self.is_stopped_session(id))
    }

    fn capture_geometry(
        &mut self,
        focused: Option<String>,
        continuous: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let viewport = f64::from(f32::from(window.viewport_size().width)) - 260.;
        let maximum = self
            .card_geometry
            .last()
            .map(|(_, left, width)| (left + width + 10. - viewport).max(0.))
            .unwrap_or(0.);
        let left = focused
            .as_ref()
            .and_then(|id| self.card_geometry.iter().find(|(card, _, _)| card == id))
            .map(|(_, left, _)| *left);
        self.save_geometry(
            f64::from(-f32::from(self.carousel_scroll.offset().x)).clamp(0., maximum),
            left,
            focused,
            continuous,
            cx,
        );
    }

    fn save_geometry(
        &mut self,
        scroll: f64,
        focused_left: Option<f64>,
        focused: Option<String>,
        continuous: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(scene) = self.hierarchy.as_ref().and_then(|h| h.scene.as_ref()) else {
            return;
        };
        let owner_key = session_levels::owner_key(&scene.id, self.level_parent());
        let mut geometry = serde_json::json!({"scrollLeft":scroll});
        if let Some(id) = &focused {
            geometry["focusedSessionId"] = id.clone().into();
            if let Some(left) = focused_left {
                geometry["anchorSessionId"] = id.clone().into();
                geometry["anchorViewportOffset"] = (left - scroll).into();
            }
        }
        if self
            .geometry
            .get(&owner_key)
            .is_some_and(|saved| saved.geometry == geometry)
        {
            return;
        }
        let update = GeometryUpdate {
            scene_id: scene.id.clone(),
            owner_key: owner_key.clone(),
            layout_revision: self.graph["layoutRevision"]
                .as_f64()
                .unwrap_or(scene.layout_revision as f64),
            geometry,
            now: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
        };
        let elapsed = self
            .last_geometry_write
            .get(&owner_key)
            .map(|last| self.motion_clock.elapsed().as_secs_f64() * 1000. - last)
            .unwrap_or(f64::INFINITY);
        self.geometry_dirty.insert(owner_key.clone());
        self.geometry.insert(owner_key, update.clone());
        self.pending_geometry = Some(update);
        // Match SessionCanvas: wheel checkpoints are immediate/throttled at
        // 50ms, while settled/click checkpoints debounce for 180ms.
        self.geometry_timer += 1;
        if continuous && elapsed >= 50. {
            self.flush_geometry(cx);
            return;
        }
        let generation = self.geometry_timer;
        let transport_generation = self.runtime_generation;
        let delay = if continuous {
            (50. - elapsed).max(1.)
        } else {
            180.
        };
        let timer = cx
            .background_executor()
            .timer(Duration::from_secs_f64(delay / 1000.));
        cx.spawn(async move |this, cx| {
            timer.await;
            let _ = this.update(cx, |this, cx| {
                if this.runtime_generation == transport_generation
                    && this.geometry_timer == generation
                {
                    this.flush_geometry(cx);
                }
            });
        })
        .detach();
    }

    fn scroll_carousel(
        &mut self,
        event: &ScrollWheelEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let delta = event.delta.pixel_delta(px(16.));
        let horizontal = event.modifiers.shift || delta.x.abs() >= delta.y.abs();
        let narrow = f32::from(window.viewport_size().width) - 260. < 720.;
        let over_terminal = self
            .order
            .iter()
            .filter(|id| !narrow || self.level_focus() == Some(id.as_str()))
            .any(|id| {
                self.terminals
                    .get(id)
                    .is_some_and(|t| t.read(cx).contains_position(event.position))
            });
        if !horizontal && over_terminal {
            return;
        }
        let amount = if horizontal && delta.x != px(0.) {
            delta.x
        } else {
            delta.y
        };
        if amount == px(0.) {
            return;
        }
        self.stop_edge_browse(true);
        self.restore_geometry = false;
        self.focus_follow_until = None;
        self.hover_follow_until = None;
        self.hover_baseline = None;
        self.hover_restore_at = None;
        let max_scroll = self
            .card_geometry
            .last()
            .map(|(_, left, width)| {
                left + width + 10. - (f64::from(f32::from(window.viewport_size().width)) - 260.)
            })
            .unwrap_or(0.)
            .max(0.);
        let next =
            (f64::from(f32::from(self.carousel_scroll.offset().x + amount))).clamp(-max_scroll, 0.);
        self.carousel_scroll
            .set_offset(point(px(next as f32), px(0.)));
        if let Some(focused) = self
            .geometry_focus()
            .filter(|id| self.order.iter().any(|card| card == id))
            .or_else(|| self.active_level_focus())
        {
            self.capture_geometry(Some(focused.to_owned()), true, window, cx);
        }
        // A horizontal gesture retargets visual expansion under the pointer,
        // while its own wheel deltas continue to own the viewport.
        if let Some(position) = self.pointer_position {
            let x = f64::from(f32::from(position.x)) - 260. - next;
            if let Some((id, _, _)) = self
                .card_geometry
                .iter()
                .find(|(_, left, width)| x >= *left && x <= *left + *width)
            {
                self.hovered = Some(id.clone());
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn stop_edge_browse(&mut self, block: bool) {
        self.edge_direction = 0;
        self.edge_blocked = block;
        self.edge_next_at = None;
        self.edge_target = None;
        self.edge_cruising = false;
    }

    fn nearest_hidden_card(&self, direction: i8, viewport_width: f64) -> Option<String> {
        let scroll = f64::from(-f32::from(self.carousel_scroll.offset().x));
        if direction < 0 {
            self.card_geometry
                .iter()
                .rev()
                .find(|(_, left, _)| *left < scroll + 10. - 0.5)
        } else {
            self.card_geometry
                .iter()
                .find(|(_, left, width)| *left + *width > scroll + viewport_width - 10. + 0.5)
        }
        .map(|(id, _, _)| id.clone())
    }

    fn edge_intent(&mut self, position: Point<Pixels>, window: &Window, cx: &mut Context<Self>) {
        let delta = self
            .pointer_position
            .map(|old| f32::from(position.x - old.x))
            .unwrap_or(0.);
        self.pointer_position = Some(position);
        let x = f32::from(position.x);
        let y = f32::from(position.y);
        let width = f32::from(window.viewport_size().width);
        let inside_y = y >= 48. && y <= f32::from(window.viewport_size().height) - 38.;
        let direction = if inside_y && x >= 260. && x <= 344. {
            -1
        } else if inside_y && x >= width - 84. && x <= width {
            1
        } else {
            0
        };
        if direction == 0 {
            let away = (self.edge_direction == 1 && delta < -2.)
                || (self.edge_direction == -1 && delta > 2.);
            self.stop_edge_browse(away);
            cx.notify();
            return;
        }
        if self.edge_direction != 0 && self.edge_direction != direction {
            self.stop_edge_browse(false);
        }
        if (direction == 1 && delta < -2.) || (direction == -1 && delta > 2.) {
            self.stop_edge_browse(true);
            cx.notify();
            return;
        }
        if self.edge_blocked || self.edge_next_at.is_some() {
            return;
        }
        let Some(target) = self.nearest_hidden_card(direction, (width - 260.) as f64) else {
            self.stop_edge_browse(false);
            cx.notify();
            return;
        };
        let now = self.motion_clock.elapsed().as_secs_f64() * 1000.;
        self.edge_direction = direction;
        self.edge_target = Some(target);
        self.edge_phase_start = now;
        self.edge_next_at = Some(now + 180.);
        cx.notify();
    }

    fn hover_card(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        let now = self.motion_clock.elapsed().as_secs_f64() * 1000.;
        self.hovered = id;
        #[cfg(debug_assertions)]
        if let Some(trace) = &self.motion_trace {
            record_motion(
                trace,
                &serde_json::json!({"kind":"hover","at":now,"session":self.hovered,"baseline":self.hover_baseline}),
            );
        }
        match &self.hovered {
            None => {
                self.hover_follow_until = None;
                if let Some(baseline) = self.hover_baseline {
                    self.carousel_scroll
                        .set_offset(point(px(-baseline as f32), px(0.)));
                    self.hover_restore_at = Some(now + 220.);
                }
            }
            Some(id) => {
                self.hover_restore_at = None;
                if self.focused.as_ref() == Some(id) {
                    self.hover_baseline = None;
                } else {
                    self.focus_follow_until = None;
                    self.hover_baseline
                        .get_or_insert(f64::from(-f32::from(self.carousel_scroll.offset().x)));
                }
                self.hover_follow_until = if self.edge_blocked {
                    None
                } else {
                    Some(now + 440.)
                };
            }
        }
        cx.notify();
    }

    fn active_path_valid(&self) -> bool {
        self.hierarchy
            .as_ref()
            .and_then(|h| h.workspace.as_ref())
            .is_none_or(|workspace| !self.workspace_invalid(&workspace.id))
    }

    fn sync_terminal_input(&self, window: &mut Window, cx: &mut Context<Self>) {
        let disabled = !self.active_path_valid();
        if self.history_request.as_ref().is_some_and(|request| {
            disabled
                || self.recovery_blocked(&request.session_id)
                || self.focused.as_ref() != Some(&request.session_id)
        }) {
            if let Some(view) = &self.history_view {
                view.update(cx, |view, _| view.stop_drag());
            }
        }
        // A removed recovery button no longer owns focus in the source DOM.
        if self
            .workspace_controls
            .iter()
            .any(|(id, controls)| !self.workspace_invalid(id) && controls[2].is_focused(window))
        {
            window.blur();
        }
        for (id, terminal) in &self.terminals {
            let disabled = disabled
                || self.recovery_blocked(id)
                || self
                    .history_request
                    .as_ref()
                    .is_some_and(|request| request.session_id == *id);
            terminal.update(cx, |terminal, cx| {
                terminal.set_transport_available(!self.recovery_blocked(id), cx);
                terminal.set_input_disabled(disabled, cx);
            });
        }
    }

    fn is_deferred(&self, id: &str) -> bool {
        self.recovery.get(id) == Some(&RecoveryState::Deferred)
    }

    fn request_deferred_preview(&mut self, id: &str) {
        let Some(cwd) = self
            .graph_node(id)
            .filter(|node| node["currentMode"] == "claude-code")
            .and_then(|node| node["cwd"].as_str())
            .map(str::to_owned)
        else {
            return;
        };
        if self
            .deferred_previews
            .get(id)
            .is_some_and(|preview| preview.cwd == cwd)
            || !self.domain_ready
            || !self.transport_connected
        {
            return;
        }
        let loading = self
            .commands
            .send_at(
                self.runtime_generation,
                Command::GetDeferredRecoveryPreview {
                    session_id: id.to_owned(),
                },
            )
            .is_ok();
        self.deferred_previews.insert(
            id.to_owned(),
            DeferredPreview {
                cwd,
                loading,
                result: None,
            },
        );
    }

    fn deferred_card(&self, id: &str) -> Option<DeferredRecoveryCard> {
        if !self.is_deferred(id) {
            return None;
        }
        let node = self.graph_node(id)?;
        let cwd = trimmed_graph_text(&node["cwd"]);
        let state = self
            .deferred_previews
            .get(id)
            .filter(|preview| Some(preview.cwd.as_str()) == cwd);
        let result = state.and_then(|state| state.result.as_ref());
        let agent = (node["currentMode"] == "claude-code").then_some("Claude Code");
        let agent_short = if agent.is_some() { "Claude" } else { "Shell" };
        let title = node["title"].as_str().unwrap_or(agent_short);
        let task = self
            .hierarchy
            .as_ref()
            .and_then(|h| h.task.as_ref())
            .map(|task| task.title.trim())
            .filter(|title| !title.is_empty());
        let provider = trimmed_graph_text(&node["providerTitle"]);
        let transcript = result
            .and_then(|preview| preview.title.as_deref())
            .filter(|title| !generic_agent_title(title, agent_short));
        let display = match node["titleSource"].as_str() {
            Some("default") => provider.or(transcript).or(task).unwrap_or(title),
            None if generic_agent_title(title, agent_short) => {
                provider.or(transcript).or(task).unwrap_or(title)
            }
            _ => title,
        };
        let subtitle = [provider, transcript, task]
            .into_iter()
            .flatten()
            .find(|candidate| *candidate != display);
        let mut replies: Vec<_> = result
            .into_iter()
            .flat_map(|preview| &preview.events)
            .map(|event| {
                let label = match event.kind.as_str() {
                    "user" => "你",
                    "assistant" => agent_short,
                    "tool" => event
                        .tool_name
                        .as_deref()
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .unwrap_or("工具"),
                    _ => "系统",
                };
                DeferredReply {
                    kind: event.kind.clone(),
                    label: label.into(),
                    text: event.text.clone(),
                }
            })
            .collect();
        if replies.is_empty() {
            replies.extend(
                node["latestLines"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .rev()
                    .filter_map(Value::as_str)
                    .map(deferred_summary_text)
                    .filter(|text| !text.is_empty())
                    .take(4)
                    .map(|text| DeferredReply {
                        kind: "assistant".into(),
                        label: agent_short.into(),
                        text,
                    }),
            );
            replies.reverse();
        }
        Some(DeferredRecoveryCard {
            title: display.to_owned(),
            subtitle: subtitle.map(str::to_owned),
            cwd: result
                .map(|detail| detail.cwd.as_str())
                .filter(|cwd| !cwd.is_empty())
                .or(cwd)
                .map(str::to_owned),
            agent,
            updated_label: result.and_then(|detail| detail.updated_label.clone()),
            replies,
            loading: state.is_some_and(|state| state.loading),
            can_open_history: agent.is_some(),
            can_restore: self.launch_action_blocked_reason(id, node).is_none(),
        })
    }

    fn restore_deferred_session(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.is_deferred(id)
            && self
                .graph_node(id)
                .is_some_and(|node| self.launch_action_blocked_reason(id, node).is_none())
        {
            self.send(
                Command::RetryRecovery {
                    session_id: id.to_owned(),
                },
                cx,
            );
        }
    }

    fn recovery_blocked(&self, session_id: &str) -> bool {
        !self.domain_ready
            || !self.transport_connected
            || !self.session_frames.contains(session_id)
            || !matches!(self.recovery.get(session_id), Some(RecoveryState::Ready))
    }

    fn clear_start_failure_focus(&self, id: &str, window: &mut Window) {
        if self
            .start_failure_controls
            .get(id)
            .is_some_and(|controls| controls.iter().any(|focus| focus.is_focused(window)))
        {
            window.blur();
        }
    }

    #[cfg(target_os = "macos")]
    fn recovery_water_needed(&self, id: &str, node: &Value) -> bool {
        node["archivedAt"].is_null()
            && fork_failure_copy(node).is_none()
            && fork_progress_copy(node).is_none()
            && Self::provider_restore_state(node) != Some("failed")
            && self.recovery_blocked(id)
            && !self.storage_fault_sessions.contains(id)
            && !matches!(
                self.recovery.get(id),
                Some(
                    RecoveryState::Deferred
                        | RecoveryState::Failed(_)
                        | RecoveryState::StartFailed(_)
                        | RecoveryState::Restarting(_)
                )
            )
            && !matches!(node["workStatus"].as_str(), Some("error" | "exited"))
    }

    #[cfg(target_os = "macos")]
    fn recovery_water_window_visible(window: &Window) -> bool {
        crate::macos::is_window_visible(window).unwrap_or_else(|error| {
            eprintln!("Recovery water window visibility: {error:#}");
            false
        })
    }

    #[cfg(target_os = "macos")]
    fn recovery_water_available(&self, window: &Window) -> bool {
        self.active_path_valid()
            && self.hierarchy.as_ref().is_some_and(|h| h.scene.is_some())
            && !matches!(
                self.commands.connection().1,
                ConnectionPhase::Stopped | ConnectionPhase::Blocked(_)
            )
            && Self::recovery_water_window_visible(window)
    }

    #[cfg(target_os = "macos")]
    fn prune_recovery_water(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.recovery_water.is_empty() {
            return;
        }
        let available = self.recovery_water_available(window);
        self.retain_recovery_water(window, cx, |this, id| {
            available
                && this.graph["nodes"].as_array().is_some_and(|nodes| {
                    nodes
                        .iter()
                        .find(|node| node["sessionId"].as_str() == Some(id))
                        .is_some_and(|node| this.recovery_water_needed(id, node))
                })
        });
    }

    #[cfg(target_os = "macos")]
    fn retain_recovery_water(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        mut keep: impl FnMut(&Self, &str) -> bool,
    ) {
        let mut water = std::mem::take(&mut self.recovery_water);
        water.retain(|id, entity| {
            if keep(self, id) {
                true
            } else {
                entity.update(cx, |water, cx| water.unmount(window, cx));
                false
            }
        });
        self.recovery_water = water;
        if self.recovery_water.is_empty() {
            self.recovery_water_visibility = None;
        }
    }

    #[cfg(target_os = "macos")]
    fn watch_recovery_water_visibility(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.recovery_water.is_empty() || self.recovery_water_visibility.is_some() {
            return;
        }
        // GPUI stops drawing hidden windows. An already inactive window can
        // also hide without an activation callback. Only watch while mounted.
        self.recovery_water_visibility = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                let keep = cx.update(|window, cx| {
                    let visible = Self::recovery_water_window_visible(window);
                    if !visible {
                        let _ = this.update(cx, |this, cx| {
                            this.retain_recovery_water(window, cx, |_, _| false);
                            cx.notify();
                        });
                    }
                    visible
                });
                if !matches!(keep, Ok(true)) {
                    break;
                }
            }
        }));
    }

    fn workspace_invalid(&self, workspace_id: &str) -> bool {
        self.workspace_paths
            .iter()
            .any(|state| state.workspace_id == workspace_id && state.status == "invalid")
    }

    fn finish_directory_choice(
        &mut self,
        generation: u64,
        root_directory: String,
        workspace_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.runtime_generation != generation || self.commands.connection().0 != generation {
            return;
        }
        if let Some(workspace_id) = workspace_id {
            self.send(
                Command::RelinkWorkspace {
                    workspace_id,
                    root_directory,
                },
                cx,
            );
        } else {
            self.creating_workspace = true;
            self.send(Command::CreateWorkspace { root_directory }, cx);
        }
        cx.notify();
    }
    fn choose_workspace_directory(&mut self, cx: &mut Context<Self>) {
        if self.creating_workspace {
            return;
        }
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        let generation = self.runtime_generation;
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = picker.await else {
                return;
            };
            let Some(path) = paths.pop() else {
                return;
            };
            let root_directory = path.to_string_lossy().into_owned();
            let _ = this.update(cx, |this, cx| {
                this.finish_directory_choice(generation, root_directory, None, cx)
            });
        })
        .detach();
    }

    fn relink_workspace_directory(&mut self, workspace_id: String, cx: &mut Context<Self>) {
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        let generation = self.runtime_generation;
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = picker.await else {
                return;
            };
            let Some(path) = paths.pop() else {
                return;
            };
            let root_directory = path.to_string_lossy().into_owned();
            let _ = this.update(cx, |this, cx| {
                this.finish_directory_choice(generation, root_directory, Some(workspace_id), cx)
            });
        })
        .detach();
    }

    fn open_workspace_menu(&mut self, workspace_id: &str, cx: &mut Context<Self>) {
        if self
            .workspace_menu
            .as_ref()
            .is_some_and(|menu| menu.0 == workspace_id)
        {
            self.workspace_menu = None;
        } else if let Some(bounds) = self.workspace_bounds.get(workspace_id) {
            self.workspace_menu = Some((
                workspace_id.into(),
                point(
                    bounds.right() + px(6.),
                    bounds.top() + bounds.size.height / 2.,
                ),
                (0..5).map(|_| cx.focus_handle().tab_stop(true)).collect(),
            ));
        }
        self.task_menu = None;
        self.tab_menu = None;
        self.hidden_tabs = None;
        cx.notify();
    }

    fn open_workspace_remove(
        &mut self,
        workspace_id: String,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dialog = Some(TabDialog::RemoveWorkspace { workspace_id, name });
        self.dialog_controls = (0..3).map(|_| cx.focus_handle().tab_stop(true)).collect();
        window.focus(&self.dialog_controls[0]);
        self.workspace_menu = None;
        cx.notify();
    }

    fn workspace_group(
        &self,
        workspace: &HierarchyWorkspace,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let active = self
            .hierarchy
            .as_ref()
            .and_then(|h| h.workspace.as_ref())
            .is_some_and(|w| w.id == workspace.id);
        let invalid = self.workspace_invalid(&workspace.id);
        let collapsed = self.workspace_collapsed.contains(&workspace.id);
        let menu_open = self
            .workspace_menu
            .as_ref()
            .is_some_and(|menu| menu.0 == workspace.id);
        let hovered = self.workspace_hover.as_ref() == Some(&workspace.id);
        let actions_visible = active || hovered || menu_open;
        let controls = &self.workspace_controls[&workspace.id];
        let id = workspace.id.clone();
        let hover_id = id.clone();
        let toggle_id = id.clone();
        let add_id = id.clone();
        let menu_id = id.clone();
        let bounds_id = id.clone();
        let view = cx.entity().downgrade();
        let now = self.motion_clock.elapsed().as_secs_f64() * 1000.;
        let scale = self
            .workspace_drag_motion
            .get(&id)
            .map(|motion| motion.scale.value(now) as f32)
            .unwrap_or(1.);
        let opacity = self
            .workspace_drag_motion
            .get(&id)
            .map(|motion| motion.opacity.value(now) as f32)
            .unwrap_or(1.);
        let group_bounds = self.workspace_group_bounds.get(&id).copied();
        let task_count = self
            .tasks
            .iter()
            .filter(|task| task.workspace_id == id)
            .count();
        let group_height = px(34.
            + if collapsed {
                0.
            } else {
                2. + task_count as f32 * 34. + task_count.saturating_sub(1) as f32
            });
        let group_measure_id = id.clone();
        let content_measure_id = id.clone();
        let content_measure_view = cx.entity().downgrade();
        let press_id = id.clone();
        let measure_view = cx.entity().downgrade();
        let task_active = self
            .hierarchy
            .as_ref()
            .and_then(|h| h.task.as_ref())
            .map(|t| &t.id);
        let snapshot = WorkspaceDragSnapshot {
            name: workspace.name.clone(),
            active,
            collapsed,
            is_default: workspace.is_default,
            is_pinned: workspace.is_pinned,
            invalid,
            actions_visible,
            header_hovered: group_bounds.is_some_and(|bounds| {
                bounds.contains(&window.mouse_position())
                    && window.mouse_position().y < bounds.top() + px(34.)
            }),
            tasks: self
                .tasks
                .iter()
                .filter(|task| task.workspace_id == workspace.id)
                .map(|task| WorkspaceDragTask {
                    title: task.title.clone(),
                    active: task_active == Some(&task.id),
                    is_pinned: task.is_pinned,
                    menu_visible: task_active == Some(&task.id)
                        || self.task_hover.as_ref() == Some(&task.id)
                        || self.task_menu.as_ref().is_some_and(|m| m.0 == task.id),
                    hovered: self.task_hover.as_ref() == Some(&task.id),
                })
                .collect(),
        };

        let toggle = self.button_inner(
            div()
                .id(SharedString::from(format!("workspace-toggle:{id}")))
                .h(px(34. * scale))
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .pl(px(7. * scale))
                .pr(px(5. * scale))
                .gap(px(7. * scale))
                .cursor_pointer()
                .child(icon(
                    if collapsed {
                        "chevron-right"
                    } else {
                        "chevron-down"
                    },
                    14. * scale,
                    0x777777,
                ))
                .child(icon(
                    "folder",
                    16. * scale,
                    if active { 0x4078f2 } else { 0x656d76 },
                ))
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .text_size(px(13. * scale))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(if active { 0x2f3547 } else { 0x656d76 }))
                        .child(ui_text(workspace.name.clone()).normal_line_height()),
                )
                .child(
                    div()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(6. * scale))
                        .when(workspace.is_default, |status| {
                            status.child(
                                div()
                                    .flex_none()
                                    .px(px(5. * scale))
                                    .py(px(1. * scale))
                                    .rounded(px(5. * scale))
                                    .bg(rgb(0xeceef3))
                                    .text_color(rgb(0x91949e))
                                    .text_size(px(9. * scale))
                                    .line_height(px(12.5 * scale))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(ui_text("默认")),
                            )
                        })
                        .when(workspace.is_pinned, |status| {
                            status.child(icon("pin", 13. * scale, 0x81848d))
                        })
                        .when(invalid, |status| {
                            status.child(
                                div()
                                    .flex_none()
                                    .px(px(4. * scale))
                                    .py(px(1. * scale))
                                    .rounded(px(4. * scale))
                                    .bg(rgba(0xd99d3c1a))
                                    .text_color(rgb(0xd99d3c))
                                    .text_size(px(9. * scale))
                                    .line_height(px(12.5 * scale))
                                    .child(ui_text("路径失效")),
                            )
                        }),
                ),
            &controls[0],
            move |this, window, cx| {
                if active {
                    if !this.workspace_collapsed.remove(&toggle_id) {
                        this.workspace_collapsed.insert(toggle_id.clone());
                        this.reveal_task = None;
                    }
                    cx.notify();
                } else {
                    this.workspace_collapsed.remove(&toggle_id);
                    this.send(
                        Command::SelectWorkspace {
                            workspace_id: toggle_id.clone(),
                        },
                        cx,
                    );
                    window.blur();
                }
            },
            false,
            cx,
        );

        let add_btn = div()
            .id(SharedString::from(format!("workspace-add:{id}")))
            .size(px(26. * scale))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6. * scale))
            .opacity(if invalid {
                0.28
            } else if actions_visible {
                1.
            } else {
                0.
            })
            .when(!invalid, |btn| {
                btn.cursor_pointer().hover(|s| s.bg(rgba(0x00000014)))
            })
            .when(invalid, |btn| btn.cursor(CursorStyle::OperationNotAllowed))
            .child(icon("plus", 14. * scale, 0x81848d));
        let add_el = if invalid {
            add_btn.into_any_element()
        } else {
            self.button(
                add_btn,
                &controls[1],
                move |this, window, cx| {
                    this.send(
                        Command::CreateTask {
                            workspace_id: add_id.clone(),
                        },
                        cx,
                    );
                    window.blur();
                },
                cx,
            )
            .into_any_element()
        };

        let more_el = self.button(
            div()
                .id(SharedString::from(format!("workspace-more:{id}")))
                .relative()
                .size(px(26. * scale))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6. * scale))
                .opacity(if actions_visible { 1. } else { 0. })
                .cursor_pointer()
                .hover(|s| s.bg(rgba(0x00000014)))
                .child(icon("ellipsis", 14. * scale, 0x81848d))
                .child(
                    canvas(
                        |_, _, _| (),
                        move |bounds, _, _, cx| {
                            let _ = view.update(cx, |this, _| {
                                this.workspace_bounds.insert(bounds_id.clone(), bounds);
                            });
                        },
                    )
                    .absolute()
                    .inset_0()
                    .size_full(),
                ),
            &controls[3],
            move |this, _, cx| {
                this.open_workspace_menu(&menu_id, cx);
            },
            cx,
        );

        let header = div()
            .min_h(px(34. * scale))
            .flex()
            .items_center()
            .rounded(px(8. * scale))
            .when(!active, |header| header.hover(|s| s.bg(rgba(0x2f35470f))))
            .child(toggle)
            .child(add_el)
            .when(invalid && !workspace.is_default, |header| {
                let relink_id = id.clone();
                header.child(
                    self.button(
                        div()
                            .id(SharedString::from(format!("workspace-relink:{id}")))
                            .h(px(24. * scale))
                            .flex_none()
                            .px(px(7. * scale))
                            .rounded(px(6. * scale))
                            .border(px(scale))
                            .border_color(rgba(0x9a681840))
                            .bg(rgb(0xfff8e8))
                            .text_color(rgb(0x9a6818))
                            .text_size(px(9. * scale))
                            .line_height(px(12.5 * scale))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgba(0xd99d3c26)))
                            .child(ui_text("恢复目录")),
                        &controls[2],
                        move |this, _, cx| {
                            this.relink_workspace_directory(relink_id.clone(), cx);
                        },
                        cx,
                    ),
                )
            })
            .child(more_el);

        let content = div()
            .relative()
            .when(active, |content| {
                content.child(workspace_drag::active_marker(scale))
            })
            .child(header)
            .child(canvas(|_, _, _| (), move |bounds, _, _, cx| {
                #[cfg(debug_assertions)]
                let _ = content_measure_view.update(cx, |this, _| {
                    if let Some(trace) = &this.motion_trace {
                        let now = this.motion_clock.elapsed().as_secs_f64() * 1000.;
                        record_motion(trace, &serde_json::json!({"kind":"workspace-drag-frame","at":now,"workspace":content_measure_id,"scale":scale,"opacity":opacity,"bounds":{"x":f32::from(bounds.origin.x),"y":f32::from(bounds.origin.y),"width":f32::from(bounds.size.width),"height":f32::from(bounds.size.height)}}));
                    }
                });
            }).absolute().inset_0().size_full())
            .when(!collapsed, |content| {
                content.child(
                    div()
                        .ml(px(17. * scale))
                        .pt(px(2. * scale))
                        .flex()
                        .flex_col()
                        .gap(px(scale))
                        .children(
                            self.tasks
                                .iter()
                                .filter(|task| task.workspace_id == workspace.id)
                                .map(|task| self.task_row(task, scale, cx).into_any_element()),
                        ),
                )
            });
        let group = div()
            .id(SharedString::from(format!("workspace:{id}")))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, _| {
                    this.workspace_drag_press = Some((press_id.clone(), event.position));
                }),
            )
            .relative()
            // Adjacent block margins collapse in the source: max(8px, 1px).
            // The native flex wrapper must account for that explicitly.
            .mt(px(
                if self.workspaces.first().is_some_and(|first| first.id == id) {
                    1.
                } else {
                    0.
                },
            ))
            .mb(px(8.))
            .when_some(
                self.workspace_drag_motion.get(id.as_str()),
                |group, motion| {
                    let mut background: Hsla = rgb(0x58a6ff).into();
                    background.a = if active {
                        0.
                    } else {
                        0.1 * motion.background.value(now) as f32
                    };
                    group.opacity(opacity).bg(background)
                },
            )
            .when(
                cx.has_active_drag() && self.workspace_drag_target.as_ref() == Some(&id),
                |group| {
                    group.child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .w_full()
                            .h(px(1.))
                            .bg(rgb(0x4697e8)),
                    )
                },
            )
            .on_hover(cx.listener(move |this, hovered, _, cx| {
                if *hovered {
                    this.workspace_hover = Some(hover_id.clone());
                } else if this.workspace_hover.as_ref() == Some(&hover_id) {
                    this.workspace_hover = None;
                }
                cx.notify();
            }))
            .child(if scale != 1. {
                if let Some(bounds) = group_bounds {
                    content
                        .absolute()
                        .left(bounds.size.width * ((1. - scale) * 0.5))
                        .top(group_height * ((1. - scale) * 0.5))
                        .w(bounds.size.width * scale)
                } else {
                    content
                }
            } else {
                content
            })
            .when(scale != 1., |group| {
                group.when_some(group_bounds, |group, _| group.h(group_height))
            })
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, _, cx| {
                        let _ = measure_view.update(cx, |this, _| {
                            this.workspace_group_bounds
                                .insert(group_measure_id.clone(), bounds);
                        });
                    },
                )
                .absolute()
                .inset_0()
                .size_full(),
            );

        if workspace.is_pinned {
            let drag_target = workspace.id.clone();
            let move_target = drag_target.clone();
            let view = cx.entity().downgrade();
            group
                .on_drag(
                    WorkspaceDrag {
                        workspace_id: workspace.id.clone(),
                        snapshot,
                        offset: Point::default(),
                        width: group_bounds
                            .map(|bounds| bounds.size.width)
                            .unwrap_or_else(|| self.sidebar_scroll.bounds().size.width - px(12.)),
                    },
                    move |info, cursor_offset, window, cx| {
                        let mut preview = info.clone();
                        let _ = view.update(cx, |this, cx| {
                            if let Some((id, press)) = this.workspace_drag_press.take() {
                                if id == info.workspace_id {
                                    // GPUI anchors at the threshold-crossing move; the source
                                    // browser anchors its drag image at the original press.
                                    let origin = window.mouse_position() - cursor_offset;
                                    preview.offset = cursor_offset - (press - origin);
                                }
                            }
                            this.workspace_drag_gesture = true;
                            this.workspace_drag_source = Some(info.workspace_id.clone());
                            this.workspace_drag_target = None;
                            cx.notify();
                        });
                        cx.new(|_| preview)
                    },
                )
                .on_drag_move(cx.listener(
                    move |this, event: &DragMoveEvent<WorkspaceDrag>, _, cx| {
                        if event.bounds.contains(&event.event.position)
                            && this.sidebar_scroll.bounds().contains(&event.event.position)
                            && event.drag(cx).workspace_id != move_target
                            && this.workspace_drag_target.as_ref() != Some(&move_target)
                        {
                            this.workspace_drag_target = Some(move_target.clone());
                            cx.notify();
                        }
                    },
                ))
                .on_drop(cx.listener(move |this, drag: &WorkspaceDrag, _, cx| {
                    this.workspace_drag_source = None;
                    this.workspace_drag_target = None;
                    if drag.workspace_id != drag_target {
                        this.send(
                            Command::ReorderPinnedWorkspace {
                                workspace_id: drag.workspace_id.clone(),
                                before_workspace_id: Some(drag_target.clone()),
                            },
                            cx,
                        );
                    }
                }))
                .into_any_element()
        } else {
            group.into_any_element()
        }
    }

    fn sidebar(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let new_workspace = div()
            .id("new-workspace-btn")
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(8.))
            .py(px(7.))
            .rounded(px(7.))
            .text_color(rgb(0x2f3547))
            .text_size(px(14.))
            .font_weight(FontWeight::SEMIBOLD)
            .when(!self.creating_workspace, |btn| {
                btn.cursor_pointer().hover(|s| s.bg(rgba(0x2f35470f)))
            })
            .when(self.creating_workspace, |btn| btn.opacity(0.6))
            .child(icon("square-pen", 17., 0x2f3547))
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(if self.creating_workspace {
                        "正在添加…"
                    } else {
                        "新增工作空间"
                    }),
            );
        let new_workspace = if self.creating_workspace {
            self.focus_control(new_workspace, &self.control_focus["new-workspace"])
        } else {
            self.button(
                new_workspace,
                &self.control_focus["new-workspace"],
                |this, _, cx| this.choose_workspace_directory(cx),
                cx,
            )
        };
        div()
            .w(px(NOTIFICATION_UI_SIDEBAR_WIDTH))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(rgba(0x58718b1f))
            .bg(rgba(0xfbfdff17))
            .child(
                div()
                    .h(px(NOTIFICATION_UI_SIDEBAR_HEADER_HEIGHT))
                    .flex_none()
                    .flex()
                    .items_center()
                    .pl(px(78.))
                    .pr(px(NOTIFICATION_UI_SIDEBAR_RIGHT_PADDING))
                    .gap(px(8.))
                    .child(new_workspace)
                    .child(div().flex_1())
                    .child(self.notification_bell(cx)),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_row()
                    .child(
                        div()
                            .id("sidebar-groups")
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.sidebar_scroll)
                            .px(px(6.))
                            .pt(px(8.))
                            .pb(px(20.))
                            .child(
                                // GPUI measures child border boxes for scroll extent. Keep the
                                // group's CSS margins inside a flex wrapper so they count too.
                                div().flex().flex_col().children(
                                    self.workspaces.iter().map(|workspace| {
                                        self.workspace_group(workspace, window, cx)
                                    }),
                                ),
                            ),
                    )
                    .when(self.sidebar_overflow, |row| {
                        row.child(sidebar_scrollbar::scrollbar(
                            self.sidebar_scroll.clone(),
                            self.sidebar_scrollbar,
                            self.dialog.is_none()
                                && self.hidden_tabs.is_none()
                                && self.workspace_menu.is_none()
                                && self.task_menu.is_none()
                                && self.tab_menu.is_none(),
                            cx.entity().downgrade(),
                        ))
                    }),
            )
            .child(
                div()
                    .h(px(38.))
                    .flex_none()
                    .px(px(18.))
                    .flex()
                    .items_center()
                    .text_size(px(12.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0x989da7))
                    .child(self.control(div().child(icon("columns-3", 16., 0x989da7)), "board"))
                    .child(div().ml(px(7.)).child("看板"))
                    .child(div().flex_1())
                    .child(self.control(div().child(icon("settings-2", 16., 0x989da7)), "settings"))
                    .child(div().ml(px(7.)).child("设置")),
            )
    }

    fn task_row(&self, task: &Task, scale: f32, cx: &mut Context<Self>) -> Stateful<Div> {
        let active = self
            .hierarchy
            .as_ref()
            .and_then(|h| h.task.as_ref())
            .is_some_and(|t| t.id == task.id);
        let focus = &self.task_controls[&task.id];
        let id = task.id.clone();
        let menu_id = id.clone();
        let right_id = id.clone();
        let hover_id = id.clone();
        let bounds_id = id.clone();
        let reveal_id = id.clone();
        let view = cx.entity().downgrade();
        let reveal_view = view.clone();
        let menu_visible = active
            || self.task_hover.as_ref() == Some(&id)
            || self.task_menu.as_ref().is_some_and(|m| m.0 == id);
        let color = if active { 0x2f3547 } else { 0x6f7683 };
        let row = self
            .focus_control(
                div().id(SharedString::from(format!("task:{id}"))),
                &focus[0],
            )
            .relative()
            .h(px(34. * scale))
            .flex_none()
            .rounded(px(8. * scale))
            .flex()
            .items_center()
            .px(px(7. * scale))
            .py(px(6. * scale))
            .gap(px(7. * scale))
            .text_size(px(13. * scale))
            .line_height(px(19.5 * scale))
            .text_color(rgb(color))
            .bg(if active {
                rgba(0x57657f18)
            } else {
                rgba(0x00000000)
            })
            .hover(|s| s.bg(rgba(0x57657f12)))
            .on_mouse_down(MouseButton::Left, {
                let focus = focus[0].clone();
                move |_, window, _| window.focus(&focus)
            })
            .on_click(cx.listener(move |this, event, _, cx| {
                if !matches!(event, ClickEvent::Keyboard(_)) && !this.workspace_drag_gesture {
                    this.send(
                        Command::SelectTask {
                            task_id: id.clone(),
                        },
                        cx,
                    );
                }
            }))
            .on_key_down(cx.listener({
                let id = task.id.clone();
                move |this, e: &KeyDownEvent, _, cx| {
                    if matches!(e.keystroke.key.as_str(), "enter" | "space")
                        && !e.keystroke.modifiers.modified()
                    {
                        cx.stop_propagation();
                        this.send(
                            Command::SelectTask {
                                task_id: id.clone(),
                            },
                            cx,
                        );
                    }
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _, window, cx| {
                    window.focus(&this.task_controls[&right_id][0]);
                    this.open_task_menu(&right_id, false, cx);
                    cx.stop_propagation();
                }),
            )
            .on_hover(cx.listener(move |this, hovered, _, cx| {
                this.task_hover = if *hovered {
                    Some(hover_id.clone())
                } else {
                    None
                };
                cx.notify();
            }))
            .child(icon("task", 12. * scale, color))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .line_height(px(19.5 * scale))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .font_weight(FontWeight::MEDIUM)
                    .child(ui_text(task.title.clone()))
                    .child(
                        canvas(
                            |_, _, _| (),
                            move |bounds, _, _, cx| {
                                let _ = reveal_view.update(cx, |this, cx| {
                                    if this.reveal_task.as_ref() == Some(&reveal_id) {
                                        this.reveal_task = None;
                                        let viewport = this.sidebar_scroll.bounds();
                                        let delta = if bounds.top() < viewport.top() {
                                            viewport.top() - bounds.top()
                                        } else if bounds.bottom() > viewport.bottom() {
                                            viewport.bottom() - bounds.bottom()
                                        } else {
                                            px(0. * scale)
                                        };
                                        if delta != px(0. * scale) {
                                            let offset = this.sidebar_scroll.offset();
                                            let max_y = this.sidebar_scroll.max_offset().height;
                                            let next_y =
                                                (offset.y + delta).clamp(-max_y, px(0. * scale));
                                            this.sidebar_scroll.set_offset(point(offset.x, next_y));
                                            cx.notify();
                                        }
                                    }
                                });
                            },
                        )
                        .absolute()
                        .inset_0()
                        .size_full(),
                    ),
            )
            .when(task.is_pinned, |row| {
                row.child(icon("pin", 13. * scale, 0x81848d))
            })
            .child(
                self.button(
                    div()
                        .id(SharedString::from(format!("task-more:{}", task.id)))
                        .size(px(22. * scale))
                        .flex_none()
                        .rounded(px(4. * scale))
                        .flex()
                        .items_center()
                        .justify_center()
                        .opacity(if menu_visible { 1. } else { 0. })
                        .cursor_pointer()
                        .hover(|s| s.bg(rgba(0x0000000f)))
                        .child(icon("ellipsis", 14. * scale, 0x656d76)),
                    &focus[1],
                    move |this, _, cx| this.open_task_menu(&menu_id, true, cx),
                    cx,
                ),
            )
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, _, _, cx| {
                        let _ = view.update(cx, |this, _| {
                            this.task_bounds.insert(bounds_id.clone(), bounds);
                        });
                    },
                )
                .absolute()
                .inset_0()
                .size_full(),
            );
        if task.is_pinned {
            let target = task.id.clone();
            let workspace_id = task.workspace_id.clone();
            row.on_drag(
                TaskDrag {
                    task_id: task.id.clone(),
                    workspace_id: workspace_id.clone(),
                    title: task.title.clone(),
                },
                |info, _, _, cx| cx.new(|_| info.clone()),
            )
            .drag_over::<TaskDrag>(|s, _, _, _| s.bg(rgba(0x58a6ff24)))
            .on_drop(cx.listener(move |this, drag: &TaskDrag, _, cx| {
                if drag.workspace_id == workspace_id && drag.task_id != target {
                    this.send(
                        Command::ReorderPinnedTask {
                            workspace_id: workspace_id.clone(),
                            task_id: drag.task_id.clone(),
                            before_task_id: Some(target.clone()),
                        },
                        cx,
                    );
                }
            }))
        } else {
            row
        }
    }

    fn open_task_menu(&mut self, task_id: &str, more_button: bool, cx: &mut Context<Self>) {
        if self
            .task_menu
            .as_ref()
            .is_some_and(|menu| menu.0 == task_id)
        {
            self.task_menu = None;
        } else if let Some(bounds) = self.task_bounds.get(task_id) {
            self.task_menu = Some((
                task_id.into(),
                point(
                    bounds.right() + px(if more_button { -1. } else { 6. }),
                    bounds.top() + bounds.size.height / 2.,
                ),
                (0..3).map(|_| cx.focus_handle().tab_stop(true)).collect(),
            ));
        }
        self.workspace_menu = None;
        self.tab_menu = None;
        self.hidden_tabs = None;
        cx.notify();
    }

    fn open_task_rename(&mut self, task_id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(task) = self.tasks.iter().find(|task| task.id == task_id) else {
            return;
        };
        let input = cx.new(|cx| {
            let mut input = RenameInput::new(task.title.clone(), cx);
            input.placeholder = "请输入事项名称".into();
            input
        });
        self.dialog_controls = (0..3).map(|_| cx.focus_handle().tab_stop(true)).collect();
        self.dialog_subscription =
            Some(
                cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
                    RenameInputEvent::Changed => {
                        if let Some(TabDialog::RenameTask {
                            empty_submitted, ..
                        }) = &mut this.dialog
                        {
                            *empty_submitted = false;
                        }
                        cx.notify();
                    }
                    RenameInputEvent::Cancel => this.close_dialog(window, cx),
                    RenameInputEvent::Confirm => this.confirm_dialog(window, cx),
                }),
            );
        window.focus(&input.read(cx).focus);
        self.dialog = Some(TabDialog::RenameTask {
            task_id,
            input,
            empty_submitted: false,
            failure: None,
        });
        self.task_menu = None;
        cx.notify();
    }

    fn task_rename_error(&self, task_id: &str, value: &str) -> Option<String> {
        let workspace_id = self
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .map(|task| task.workspace_id.as_str());
        self.tasks
            .iter()
            .any(|task| {
                task.id != task_id
                    && workspace_id.is_none_or(|ws| task.workspace_id == ws)
                    && task.title == value
            })
            .then(|| format!("当前工作区下已存在名为\"{value}\"的工作台"))
    }

    fn open_task_delete(&mut self, task_id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(task) = self.tasks.iter().find(|task| task.id == task_id) else {
            return;
        };
        self.dialog = Some(TabDialog::DeleteTask {
            task_id,
            title: task.title.clone(),
        });
        self.dialog_controls = (0..3).map(|_| cx.focus_handle().tab_stop(true)).collect();
        window.focus(&self.dialog_controls[0]);
        self.task_menu = None;
        cx.notify();
    }

    fn scene_title(&self, scene: &Scene) -> String {
        if !scene.title_pinned
            && scene.name.starts_with("Shell · ")
            && self
                .hierarchy
                .as_ref()
                .and_then(|h| h.scene.as_ref())
                .is_some_and(|s| s.id == scene.id)
        {
            if let Some(cwd) = self.graph["nodes"]
                .as_array()
                .and_then(|nodes| {
                    nodes
                        .iter()
                        .find(|n| n["sessionId"].as_str() == self.focused.as_deref())
                })
                .and_then(|n| n["cwd"].as_str())
            {
                return format!("Shell · {cwd}");
            }
        }
        scene.name.clone()
    }
    fn close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = self.dialog.take();
        self.dialog_subscription = None;
        self.dialog_controls.clear();
        let focus = match dialog {
            Some(TabDialog::LastSession { return_focus, .. }) => return_focus,
            Some(TabDialog::SessionLoader(state)) if state.history => self
                .deferred_controls
                .get(&state.session_id)
                .map(|buttons| buttons[0].clone()),
            Some(TabDialog::SessionLoader(state)) => {
                self.card_controls.get(&state.session_id).cloned()
            }
            Some(TabDialog::Branch(state)) => self.card_controls.get(&state.session_id).cloned(),
            Some(TabDialog::PermissionMode(_)) => self
                .permission_chip()
                .filter(|chip| !chip.disabled)
                .map(|_| self.control_focus["permission-mode"].clone()),
            _ => None,
        };
        if let Some(focus) = focus {
            window.focus(&focus);
        } else {
            window.blur();
        }
        cx.notify();
    }
    fn open_rename(&mut self, scene_id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(scene) = self.scenes.iter().find(|s| s.id == scene_id) else {
            return;
        };
        let text = self.scene_title(scene);
        let input = cx.new(|cx| RenameInput::new(text, cx));
        self.dialog_controls = (0..3).map(|_| cx.focus_handle().tab_stop(true)).collect();
        self.dialog_subscription =
            Some(
                cx.subscribe_in(&input, window, |this, _, event, window, cx| match event {
                    RenameInputEvent::Changed => cx.notify(),
                    RenameInputEvent::Cancel => this.close_dialog(window, cx),
                    RenameInputEvent::Confirm => this.confirm_dialog(window, cx),
                }),
            );
        window.focus(&input.read(cx).focus);
        self.dialog = Some(TabDialog::Rename { scene_id, input });
        self.workspace_menu = None;
        self.tab_menu = None;
        self.hidden_tabs = None;
        cx.notify();
    }
    fn rename_error(&self, scene_id: &str, value: &str) -> Option<String> {
        self.scenes
            .iter()
            .any(|s| s.id != scene_id && s.title_pinned && self.scene_title(s) == value)
            .then(|| format!("当前事项下已存在名为\"{value}\"的标签页"))
    }
    fn confirm_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(TabDialog::PermissionMode(state)) = &self.dialog {
            let request = self
                .permission_confirmation_valid(state)
                .then(|| (state.session_id.clone(), state.from, state.target));
            self.close_dialog(window, cx);
            if let Some((id, from, target)) = request {
                self.send_permission_mode(id, from, target, window, cx);
            }
            return;
        }
        if matches!(self.dialog, Some(TabDialog::Branch(_))) {
            self.submit_fork(window, cx);
            return;
        }
        if let Some(TabDialog::RenameTask { task_id, input, .. }) = &self.dialog {
            let task_id = task_id.clone();
            let text = input.read(cx);
            if text.composing() {
                return;
            }
            let title = trim_name(&text.text).to_owned();
            if title.is_empty() {
                if let Some(TabDialog::RenameTask {
                    empty_submitted, ..
                }) = &mut self.dialog
                {
                    *empty_submitted = true;
                }
                cx.notify();
                return;
            }
            if self.task_rename_error(&task_id, &title).is_none() {
                self.send(Command::RenameTask { task_id, title }, cx);
            }
            return;
        }
        let command = match &self.dialog {
            Some(TabDialog::RenameTask { .. }) => unreachable!("task rename handled above"),
            Some(TabDialog::RemoveWorkspace { workspace_id, .. }) => {
                Some(Command::RemoveWorkspace {
                    workspace_id: workspace_id.clone(),
                })
            }
            Some(TabDialog::DeleteTask { task_id, .. }) => Some(Command::DeleteTask {
                task_id: task_id.clone(),
            }),
            Some(TabDialog::Rename { scene_id, input }) => {
                let input = input.read(cx);
                let name = trim_name(&input.text);
                if input.composing()
                    || name.is_empty()
                    || self.rename_error(scene_id, name).is_some()
                {
                    return;
                }
                Some(Command::RenameScene {
                    scene_id: scene_id.clone(),
                    name: name.to_owned(),
                })
            }
            Some(TabDialog::Close {
                scene_id, is_last, ..
            }) => {
                if *is_last {
                    None
                } else {
                    Some(Command::CloseScene {
                        scene_id: scene_id.clone(),
                        confirmed: true,
                    })
                }
            }
            Some(TabDialog::LastSession { .. }) => None,
            Some(TabDialog::CloseSession {
                scene_id,
                session_id,
                ..
            }) => self
                .card_action_available(session_id)
                .then(|| Command::CloseSessionCard {
                    scene_id: scene_id.clone(),
                    session_id: session_id.clone(),
                    confirmed_intent: Some(format!("delete-session:{session_id}")),
                    preserve_scene_on_last_session: false,
                }),
            Some(TabDialog::RemoveSession {
                scene_id,
                session_id,
                scope,
            }) => self
                .remove_session_available(session_id)
                .then(|| Command::RemoveSessionBranch {
                    scene_id: scene_id.clone(),
                    session_id: session_id.clone(),
                    scope: *scope,
                }),
            Some(
                TabDialog::SessionLoader(_) | TabDialog::Branch(_) | TabDialog::PermissionMode(_),
            )
            | None => return,
        };
        self.close_dialog(window, cx);
        if let Some(command) = command {
            match command {
                Command::CloseSessionCard {
                    scene_id,
                    session_id,
                    ..
                } if self.graph["workspaceSessionCount"].as_u64() == Some(1) => {
                    self.open_last_session(scene_id, session_id, window, cx);
                }
                Command::CloseSessionCard { .. } | Command::RemoveSessionBranch { .. } => {
                    self.send_card_action(command, cx);
                }
                _ => self.send(command, cx),
            }
        }
    }
    fn create_canvas(&mut self, cx: &mut Context<Self>) {
        if !self.active_path_valid() {
            return;
        }
        if let Some(task) = self.hierarchy.as_ref().and_then(|h| h.task.as_ref()) {
            self.send(
                Command::CreateCanvas {
                    task_id: task.id.clone(),
                },
                cx,
            );
        }
    }

    fn split_horizontal(&mut self, cx: &mut Context<Self>) {
        if !self.active_path_valid()
            || (self.level_focus().is_some() && self.active_level_focus().is_none())
        {
            return;
        }
        if let (Some(scene_id), Some(source_session_id)) =
            (self.active_scene_id(), self.focused.as_ref())
        {
            let parent = self.level_parent();
            // The runtime derives the structural parent from the source when
            // parent_session_id is None. A root split needs an active root source.
            let source_session_id = if parent.is_none() && !self.order.contains(source_session_id) {
                let Some(source) = self.order.iter().find(|id| {
                    self.graph_node(id)
                        .is_some_and(|node| node["archivedAt"].is_null())
                }) else {
                    return;
                };
                source
            } else {
                source_session_id
            };
            if self.is_stopped_session(source_session_id) {
                return;
            }
            self.send(
                Command::CreateShellSibling {
                    scene_id: scene_id.to_owned(),
                    source_session_id: source_session_id.clone(),
                    parent_session_id: parent.map(str::to_owned),
                },
                cx,
            );
        }
    }

    fn active_scene_id(&self) -> Option<&str> {
        self.hierarchy
            .as_ref()?
            .scene
            .as_ref()
            .map(|scene| scene.id.as_str())
    }

    fn permission_blocked_reason(&self, id: &str, node: &Value) -> Option<&'static str> {
        let (generation, phase) = self.commands.connection();
        if matches!(&phase, ConnectionPhase::Blocked(block) if block.kind == StartupBlockKind::ReadOnly)
        {
            Some("数据库处于只读恢复模式")
        } else if self.storage_fault_sessions.contains(id) {
            Some("终端存储异常，请先恢复或结束当前会话")
        } else if self.recovery_blocked(id)
            || generation != self.runtime_generation
            || phase != ConnectionPhase::Ready
            || matches!(
                Self::provider_restore_state(node),
                Some("restoring" | "failed")
            )
        {
            Some("当前终端需要先完成恢复")
        } else {
            self.card_mutation_blocked_reason(id, node)
        }
    }

    fn permission_node(&self) -> Option<(&str, &Value)> {
        let id = self.level_focus()?;
        let node = self.graph_node(id)?;
        (node["currentMode"] == "claude-code").then_some((id, node))
    }

    fn permission_chip(&self) -> Option<PermissionChip> {
        let (id, node) = self.permission_node()?;
        let change = self.permission_changes.get(id);
        let mode = change.map_or_else(|| PermissionMode::from_node(node), |change| change.from);
        let blocked = self.permission_blocked_reason(id, node);
        Some(PermissionChip {
            mode,
            title: blocked.map_or_else(
                || format!("当前权限模式：{}，点击切换", mode.label()),
                str::to_owned,
            ),
            disabled: blocked.is_some() || change.is_some() || self.is_stopped_session(id),
        })
    }

    fn toggle_permission_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.permission_menu.is_some() {
            self.dismiss_permission_menu(window, cx);
        } else if self.dialog.is_none() && self.permission_chip().is_some_and(|chip| !chip.disabled)
        {
            self.permission_menu = self.permission_node().map(|(id, _)| id.to_owned());
            self.workspace_menu = None;
            self.task_menu = None;
            self.tab_menu = None;
            self.fork_menu = None;
            self.hidden_tabs = None;
            cx.notify();
        }
    }

    fn dismiss_permission_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.permission_menu.take() else {
            return;
        };
        if self.dialog.is_none()
            && self.level_focus() == Some(id.as_str())
            && self.permission_chip().is_some_and(|chip| !chip.disabled)
        {
            window.focus(&self.control_focus["permission-mode"]);
        }
        cx.notify();
    }

    fn permission_confirmation_valid(&self, state: &PermissionConfirmation) -> bool {
        self.permission_node().is_some_and(|(id, node)| {
            id == state.session_id
                && PermissionMode::from_node(node) == state.from
                && self.permission_chip().is_some_and(|chip| !chip.disabled)
        })
    }

    fn choose_permission_mode(
        &mut self,
        target: PermissionMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session_id) = self.permission_menu.take() else {
            return;
        };
        cx.notify();
        let Some((id, node)) = self.permission_node().filter(|(id, _)| *id == session_id) else {
            return;
        };
        let Some(chip) = self.permission_chip().filter(|chip| !chip.disabled) else {
            return;
        };
        let from = chip.mode;
        if from == target {
            window.focus(&self.control_focus["permission-mode"]);
            return;
        }
        if from == PermissionMode::Bypass || target == PermissionMode::Bypass {
            // Older graph nodes omit the provider session ID. A confirmed identity
            // event or binding-backed fields can still establish resumability.
            let resumable = node["canFork"] == true
                || self.permission_identities.contains(id)
                || ["providerSessionId", "model"].into_iter().any(|key| {
                    node[key]
                        .as_str()
                        .is_some_and(|value| !value.trim().is_empty())
                })
                || from != PermissionMode::Default;
            self.open_card_dialog(
                TabDialog::PermissionMode(PermissionConfirmation {
                    session_id,
                    from,
                    target,
                    resumable,
                }),
                3,
                window,
                cx,
            );
        } else {
            self.send_permission_mode(session_id, from, target, window, cx);
        }
    }

    fn send_permission_mode(
        &mut self,
        session_id: String,
        from: PermissionMode,
        target: PermissionMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let result = self.commands.send_at(
            self.runtime_generation,
            Command::SetProviderPermissionMode {
                session_id: session_id.clone(),
                mode: target.value().into(),
            },
        );
        match result {
            Ok(()) => {
                self.permission_changes.insert(
                    session_id,
                    PermissionChange {
                        from,
                        target,
                        outcome: PermissionOutcome::Pending,
                    },
                );
                // The terminal can reclaim focus when the replacement frame is ready.
                window.blur();
                cx.notify();
            }
            Err(error) => self.show_permission_failure(&error.to_string(), cx),
        }
    }

    fn reconcile_permission_modes(&mut self, results: &[Value], cx: &mut Context<Self>) {
        let nodes = session_levels::nodes(&self.graph);
        for identity in self.graph["providerIdentityEvents"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if let Some(id) = identity["sessionId"].as_str() {
                if identity["result"] == "confirmed"
                    && identity["providerSessionId"]
                        .as_str()
                        .is_some_and(|value| !value.trim().is_empty())
                {
                    self.permission_identities.insert(id.to_owned());
                } else if identity["result"] == "restore-failed" {
                    self.permission_identities.remove(id);
                }
            }
        }
        let mut failure = None;
        for result in results {
            let Some(id) = result["sessionId"].as_str() else {
                continue;
            };
            let Some(change) = self.permission_changes.get_mut(id) else {
                continue;
            };
            if result["permissionMode"] != change.target.value()
                || change.outcome != PermissionOutcome::Pending
            {
                continue;
            }
            change.outcome = if let Some(error) = result.get("error") {
                failure = Some(
                    error
                        .as_str()
                        .filter(|error| !error.is_empty())
                        .unwrap_or("未知错误"),
                );
                PermissionOutcome::Failed
            } else {
                PermissionOutcome::Confirmed
            };
        }
        self.permission_changes.retain(|id, change| {
            let node = session_levels::node(nodes, id);
            match change.outcome {
                PermissionOutcome::Pending => true,
                PermissionOutcome::Confirmed => {
                    node.is_none_or(|node| PermissionMode::from_node(node) != change.target)
                }
                // The existing runtime can park a failed restore and omit its
                // binding's mode. Keep the pre-switch label while it is parked.
                PermissionOutcome::Failed => {
                    node.is_some_and(|node| Self::provider_restore_state(node) == Some("failed"))
                }
            }
        });
        if let Some(reason) = failure {
            self.show_permission_failure(reason, cx);
        }
    }

    fn reconcile_permission_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.permission_menu.as_deref().is_some_and(|id| {
            self.dialog.is_some()
                || self
                    .permission_node()
                    .is_none_or(|(focused, _)| id != focused)
                || self.permission_chip().is_none_or(|chip| chip.disabled)
        }) {
            self.dismiss_permission_menu(window, cx);
        }
        if matches!(
            &self.dialog,
            Some(TabDialog::PermissionMode(state)) if !self.permission_confirmation_valid(state)
        ) {
            self.close_dialog(window, cx);
        }
    }

    fn fail_pending_permission_modes(&mut self, reason: &str, cx: &mut Context<Self>) {
        let was_pending = self
            .permission_changes
            .values()
            .any(|change| change.outcome != PermissionOutcome::Failed);
        self.permission_changes.clear();
        if was_pending {
            self.show_permission_failure(reason, cx);
        }
    }

    fn show_permission_failure(&mut self, reason: &str, cx: &mut Context<Self>) {
        self.permission_toast = Some(format!("切换失败：{reason}"));
        self.permission_toast_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(3)).await;
            let _ = this.update(cx, |this, cx| {
                this.permission_toast = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn graph_node(&self, id: &str) -> Option<&Value> {
        session_levels::node(session_levels::nodes(&self.graph), id)
    }

    fn is_stopped_session(&self, id: &str) -> bool {
        self.graph_node(id)
            .is_some_and(|node| !node["archivedAt"].is_null())
    }

    fn card_action_available(&self, id: &str) -> bool {
        self.graph_node(id).is_some_and(|node| {
            node["archivedAt"].is_null() && self.card_action_blocked_reason(id, node).is_none()
        })
    }

    fn stopped_remove_blocked_reason(&self) -> Option<&'static str> {
        if !self.active_path_valid() {
            return Some(WORKSPACE_PATH_UNAVAILABLE);
        }
        match self.commands.connection() {
            (generation, ConnectionPhase::Ready)
                if generation == self.runtime_generation && self.domain_ready =>
            {
                None
            }
            (_, ConnectionPhase::Blocked(block)) if block.kind == StartupBlockKind::ReadOnly => {
                Some("数据库处于只读恢复模式")
            }
            _ => Some("当前连接不可用"),
        }
    }

    fn stopped_remove_visible(&self, node: &Value) -> bool {
        let nodes = session_levels::nodes(&self.graph);
        nodes.iter().any(|other| {
            other["sessionId"] != node["sessionId"] && session_levels::parent_id(other).is_none()
        }) || self.removal_branch_impact(node).sessions < nodes.len()
    }

    fn stopped_remove_available(&self, id: &str) -> bool {
        self.stopped_remove_blocked_reason().is_none()
            && self
                .graph_node(id)
                .is_some_and(|node| self.stopped_remove_visible(node))
    }

    fn remove_session_available(&self, id: &str) -> bool {
        if let Some(node) = self
            .graph_node(id)
            .filter(|node| node["archivedAt"].is_null() && fork_failure_copy(node).is_some())
        {
            self.fork_repair_blocked_reason(id, node).is_none()
        } else if self.is_stopped_session(id) {
            self.stopped_remove_available(id)
        } else {
            self.card_action_available(id)
        }
    }

    fn card_action_blocked_reason(&self, id: &str, node: &Value) -> Option<&'static str> {
        self.card_mutation_blocked_reason(id, node)
            .or_else(|| self.recovery_blocked(id).then_some("当前终端仍在恢复"))
    }

    fn card_mutation_blocked_reason(&self, id: &str, node: &Value) -> Option<&'static str> {
        if !self.active_path_valid() {
            Some(WORKSPACE_PATH_UNAVAILABLE)
        } else if self.storage_fault_sessions.contains(id) {
            Some("终端存储异常，请先恢复或结束当前会话")
        } else if node["environment"]["state"]
            .as_str()
            .is_some_and(|state| state != "ready")
        {
            Some("当前运行环境需要先恢复或交接")
        } else {
            None
        }
    }

    fn remove_node_tooltip(&self, id: &str, node: &Value) -> RemoveNodeTooltip {
        RemoveNodeTooltip {
            blocked_reason: self.card_action_blocked_reason(id, node),
        }
    }

    fn session_loader_blocked_reason(&self, id: &str, node: &Value) -> Option<&'static str> {
        self.launch_action_blocked_reason(id, node)
            .or_else(|| self.recovery_blocked(id).then_some("当前终端仍在恢复"))
    }

    fn fork_connection_blocked_reason(&self) -> Option<&'static str> {
        match self.commands.connection() {
            (_, ConnectionPhase::Blocked(block)) if block.kind == StartupBlockKind::ReadOnly => {
                Some("数据库处于只读恢复模式")
            }
            (generation, ConnectionPhase::Ready)
                if generation == self.runtime_generation
                    && self.domain_ready
                    && self.transport_connected =>
            {
                None
            }
            _ => Some("当前终端仍在恢复"),
        }
    }

    fn fork_access(&self, id: &str, placement: ForkPlacement) -> Option<ForkAccess> {
        let node = self.graph_node(id)?;
        if !node["archivedAt"].is_null() || !node["detachedWindowId"].is_null() {
            return None;
        }
        let readiness = if placement == ForkPlacement::Sibling {
            let parent = self.graph_node(session_levels::parent_id(node)?)?;
            if parent["canFork"] != true || !parent["archivedAt"].is_null() {
                return None;
            }
            None
        } else {
            if node["currentMode"] != "claude-code" {
                return None;
            }
            (node["canFork"] != true).then(|| {
                if matches!(
                    node["workStatus"].as_str(),
                    Some("running" | "starting" | "needs-input")
                ) {
                    "当前回复完成后即可 Fork"
                } else {
                    "在当前会话输入一次，并等待 Claude Code 完成回复后，即可创建分支"
                }
            })
        };
        Some(ForkAccess {
            blocked: self
                .fork_connection_blocked_reason()
                .or_else(|| self.card_action_blocked_reason(id, node)),
            readiness,
        })
    }

    fn fork_repair_blocked_reason(&self, id: &str, node: &Value) -> Option<&'static str> {
        self.fork_connection_blocked_reason()
            .or_else(|| self.card_mutation_blocked_reason(id, node))
            .or_else(|| {
                // A failed fork has no usable terminal frame. Only actual recovery
                // in progress should block repairing the failed operation.
                matches!(
                    self.recovery.get(id),
                    Some(RecoveryState::Restoring | RecoveryState::Restarting(_))
                )
                .then_some("当前终端仍在恢复")
            })
    }

    fn show_fork_readiness(&mut self, message: &'static str, cx: &mut Context<Self>) {
        self.fork_readiness = Some(message);
        self.fork_readiness_generation += 1;
        let generation = self.fork_readiness_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(2800))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.fork_readiness_generation == generation {
                    this.fork_readiness = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn open_fork_menu(&mut self, id: &str, position: Point<Pixels>, cx: &mut Context<Self>) {
        if self.dialog.is_some()
            || self
                .fork_access(id, ForkPlacement::Peer)
                .is_none_or(|access| access.blocked.is_some())
        {
            return;
        }
        self.workspace_menu = None;
        self.task_menu = None;
        self.tab_menu = None;
        self.hidden_tabs = None;
        self.fork_menu = Some((id.to_owned(), position, cx.focus_handle().tab_stop(true)));
        cx.notify();
    }

    fn open_fork(
        &mut self,
        id: &str,
        placement: ForkPlacement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.fork_menu = None;
        cx.notify();
        if self.dialog.is_some() {
            return;
        }
        let Some(access) = self.fork_access(id, placement) else {
            return;
        };
        if access.blocked.is_some() {
            return;
        }
        if let Some(reason) = access.readiness {
            self.show_fork_readiness(reason, cx);
            return;
        }
        let Some((scene_id, node)) = self.active_scene_id().zip(self.graph_node(id)) else {
            return;
        };
        let source = if placement == ForkPlacement::Sibling {
            session_levels::parent_id(node)
                .and_then(|parent| self.graph_node(parent))
                .unwrap_or(node)
        } else {
            node
        };
        let state = BranchDialog {
            scene_id: scene_id.into(),
            session_id: id.into(),
            source_title: source["title"].as_str().unwrap_or_default().into(),
            placement,
            input: cx.new(|cx| RenameInput::session_loader_search("例如：修复登录流程", cx)),
            submission_key: fork_submission_key(),
            submitting: false,
            error: None,
        };
        self.workspace_menu = None;
        self.task_menu = None;
        self.tab_menu = None;
        self.hidden_tabs = None;
        self.fork_readiness = None;
        self.dialog_controls = (0..3).map(|_| cx.focus_handle().tab_stop(true)).collect();
        self.dialog_subscription = Some(cx.subscribe_in(
            &state.input,
            window,
            |this, _, event, window, cx| match event {
                RenameInputEvent::Changed => {
                    if let Some(TabDialog::Branch(state)) = &mut this.dialog {
                        state.error = None;
                    }
                    cx.notify();
                }
                RenameInputEvent::Cancel => this.cancel_fork(window, cx),
                RenameInputEvent::Confirm => this.submit_fork(window, cx),
            },
        ));
        window.focus(&state.input.read(cx).focus);
        self.dialog = Some(TabDialog::Branch(state));
        cx.notify();
    }

    fn cancel_fork(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(&self.dialog, Some(TabDialog::Branch(state)) if !state.submitting) {
            self.close_dialog(window, cx);
        }
    }

    fn submit_fork(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(TabDialog::Branch(state)) = &self.dialog else {
            return;
        };
        let input = state.input.read(cx);
        if state.submitting || input.composing() {
            return;
        }
        let name = trim_name(&input.text);
        let error = if name.is_empty() {
            Some("请输入分支名称")
        } else if name.chars().count() > FORK_NAME_LIMIT {
            Some("分支名称最多 64 个字符")
        } else {
            match self.fork_access(&state.session_id, state.placement) {
                Some(access) => access.blocked.or(access.readiness),
                None => Some(if state.placement == ForkPlacement::Sibling {
                    "完成首轮 AI 对话后可创建分支"
                } else {
                    "当前终端仍在恢复"
                }),
            }
        };
        let result = if let Some(error) = error {
            Err(error.to_owned())
        } else {
            self.commands
                .send_at(
                    self.runtime_generation,
                    Command::ForkSession {
                        scene_id: state.scene_id.clone(),
                        source_session_id: state.session_id.clone(),
                        placement: state.placement,
                        name: name.to_owned(),
                        worktree_mode: ForkWorktreeMode::Current,
                        submission_key: state.submission_key.clone(),
                    },
                )
                .map_err(|error| error.to_string())
        };
        let focus = input.focus.clone();
        let Some(TabDialog::Branch(state)) = &mut self.dialog else {
            return;
        };
        state.submitting = result.is_ok();
        state.error = result.err();
        if state.submitting {
            window.blur();
        } else {
            window.focus(&focus);
        }
        cx.notify();
    }

    fn retry_fork(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.graph_node(id).is_none_or(|node| {
            !node["archivedAt"].is_null()
                || fork_failure_copy(node).is_none()
                || self.fork_repair_blocked_reason(id, node).is_some()
        }) || matches!(self.fork_retries.get(id), Some(ForkRetry::Pending))
        {
            return;
        }
        let result = self.commands.send_at(
            self.runtime_generation,
            Command::RetryFork {
                session_id: id.to_owned(),
            },
        );
        self.fork_retries.insert(
            id.into(),
            match result {
                Ok(()) => ForkRetry::Pending,
                Err(error) => ForkRetry::Rejected(error.to_string()),
            },
        );
        cx.notify();
    }

    fn fail_pending_forks(&mut self, message: &str) {
        if let Some(TabDialog::Branch(state)) = &mut self.dialog
            && state.submitting
        {
            state.submitting = false;
            state.error = Some(message.into());
        }
        for retry in self.fork_retries.values_mut() {
            if matches!(retry, ForkRetry::Pending) {
                *retry = ForkRetry::Rejected(message.into());
            }
        }
    }

    fn replace_fork_graph(
        &mut self,
        mut graph: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let result = graph
            .as_object_mut()
            .and_then(|object| object.remove("forkSessionResult"));
        let permission_results = graph
            .as_object_mut()
            .and_then(|object| object.remove("providerPermissionModeResults"));
        self.update_notifications(&mut graph);
        self.graph = graph;
        self.reconcile_permission_modes(
            permission_results
                .as_ref()
                .and_then(Value::as_array)
                .map_or(&[], Vec::as_slice),
            cx,
        );
        if self
            .fork_menu
            .as_ref()
            .is_some_and(|(id, _, _)| self.fork_access(id, ForkPlacement::Peer).is_none())
        {
            self.fork_menu = None;
        }
        if let Some(result) = result {
            let error = result["error"].as_str();
            if let Some(TabDialog::Branch(state)) = &mut self.dialog
                && state.submitting
                && result["submissionKey"] == state.submission_key
            {
                if let Some(error) = error {
                    state.error = Some(error.into());
                    state.submitting = false;
                    window.focus(&state.input.read(cx).focus);
                } else if result["sessionId"].is_string() {
                    if state.placement == ForkPlacement::Child
                        && session_levels::node(
                            session_levels::nodes(&self.graph),
                            &state.session_id,
                        )
                        .is_some_and(|node| node["archivedAt"].is_null())
                    {
                        self.level_parent_by_scene
                            .insert(state.scene_id.clone(), Some(state.session_id.clone()));
                    }
                    self.close_dialog(window, cx);
                }
            } else if let Some(id) = result["sessionId"].as_str()
                && matches!(self.fork_retries.get(id), Some(ForkRetry::Pending))
            {
                if let Some(error) = error {
                    if self.graph_node(id).and_then(fork_failure_copy).is_some() {
                        self.fork_retries
                            .insert(id.into(), ForkRetry::Rejected(error.into()));
                    } else {
                        self.show_toast(error, cx);
                    }
                } else {
                    self.fork_retries.remove(id);
                }
            }
        }
        let nodes = session_levels::nodes(&self.graph);
        self.fork_retries.retain(|id, _| {
            session_levels::node(nodes, id).is_some_and(|node| {
                node["archivedAt"].is_null() && fork_failure_copy(node).is_some()
            })
        });
    }

    fn session_loader_running(&self, id: &str) -> bool {
        self.graph_node(id)
            .is_some_and(|node| matches!(node["workStatus"].as_str(), Some("running" | "starting")))
    }

    fn session_loader_can_submit(&self, state: &SessionLoader) -> bool {
        !state.history
            && state.selected.is_some()
            && !self.pending_loader_loads.contains_key(&state.session_id)
            && self.graph_node(&state.session_id).is_some_and(|node| {
                self.launch_action_blocked_reason(&state.session_id, node)
                    .is_none()
            })
    }

    fn open_session_loader(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open_session_loader_mode(id, false, window, cx);
    }

    fn open_deferred_history(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_deferred(id) {
            self.open_session_loader_mode(id, true, window, cx);
        }
    }

    fn open_session_loader_mode(
        &mut self,
        id: &str,
        history: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.dialog.is_some()
            || self.pending_close.is_some()
            || self.pending_card_action.is_some()
        {
            return;
        }
        let Some((scene_id, node)) = self.active_scene_id().zip(self.graph_node(id)) else {
            return;
        };
        if !self.order.iter().any(|card| card == id)
            || !node["archivedAt"].is_null()
            || if history {
                node["currentMode"] != "claude-code"
            } else {
                self.session_loader_blocked_reason(id, node).is_some()
            }
        {
            return;
        }
        let Some(cwd) = node["cwd"].as_str().filter(|path| !path.is_empty()) else {
            return;
        };
        let selected = history
            .then(|| trimmed_graph_text(&node["historyProviderSessionId"]))
            .flatten()
            .map(str::to_owned);
        let scene_id = scene_id.to_owned();
        let cwd = cwd.to_owned();
        let input =
            cx.new(|cx| RenameInput::session_loader_search(SESSION_LOADER_FILTER_PLACEHOLDER, cx));
        self.dialog_subscription =
            Some(cx.subscribe_in(&input, window, |this, _, event, _, cx| {
                if matches!(event, RenameInputEvent::Changed) {
                    this.refresh_loader_selection(true, cx);
                }
            }));
        self.dialog_controls = (0..4).map(|_| cx.focus_handle().tab_stop(true)).collect();
        let result = if history {
            window.focus(&self.dialog_controls[0]);
            None
        } else {
            window.focus(&input.read(cx).focus);
            Some(self.commands.send_at(
                self.runtime_generation,
                Command::ListProviderSessions { cwd: cwd.clone() },
            ))
        };
        self.dialog = Some(TabDialog::SessionLoader(SessionLoader {
            history,
            scene_id,
            session_id: id.to_owned(),
            cwd: cwd.to_owned(),
            input,
            sessions: Vec::new(),
            selected,
            events: Vec::new(),
            first_event: None,
            total_events: 0,
            loading_list: result.as_ref().is_some_and(Result::is_ok),
            loading_detail: false,
            loading_earlier: false,
            pending_detail: None,
            has_earlier: false,
            error: result.and_then(Result::err).map(|error| error.to_string()),
            list_scroll: ScrollHandle::new(),
            preview_scroll: ScrollHandle::new(),
            list_focus: cx.focus_handle().tab_stop(true),
        }));
        if history {
            self.request_loader_detail(None);
        }
        self.workspace_menu = None;
        self.task_menu = None;
        self.tab_menu = None;
        self.hidden_tabs = None;
        cx.notify();
    }

    fn clear_session_loader_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(TabDialog::SessionLoader(state)) = &self.dialog {
            let input = state.input.clone();
            window.focus(&input.read(cx).focus);
            input.update(cx, |input, cx| {
                let length = input.text.encode_utf16().count();
                input.replace_text_in_range(Some(0..length), "", window, cx);
            });
        }
    }

    fn refresh_loader_selection(&mut self, reset_scroll: bool, cx: &mut Context<Self>) {
        let Some(TabDialog::SessionLoader(state)) = &self.dialog else {
            return;
        };
        if state.history {
            return;
        }
        if reset_scroll {
            state.list_scroll.set_offset(Point::default());
        }
        let mut visible = state.visible_sessions(cx);
        let first = visible.next();
        let selected = state.selected.as_deref();
        let next = first
            .filter(|first| Some(first.id.as_str()) == selected)
            .or_else(|| visible.find(|session| Some(session.id.as_str()) == selected))
            .or(first)
            .map(|session| session.id.clone());
        drop(visible);
        self.set_loader_selection(next, cx);
        cx.notify();
    }

    fn select_loader_session(&mut self, id: &str, cx: &mut Context<Self>) {
        self.set_loader_selection(Some(id.to_owned()), cx);
    }

    fn step_loader_session(&mut self, delta: isize, cx: &mut Context<Self>) {
        let Some(TabDialog::SessionLoader(state)) = &self.dialog else {
            return;
        };
        let visible: Vec<_> = state.visible_sessions(cx).collect();
        let Some(current) = visible
            .iter()
            .position(|session| Some(&session.id) == state.selected.as_ref())
        else {
            return;
        };
        let next = current.saturating_add_signed(delta).min(visible.len() - 1);
        let id = visible[next].id.clone();
        state.list_scroll.scroll_to_item(next);
        self.select_loader_session(&id, cx);
    }

    fn set_loader_selection(&mut self, next: Option<String>, cx: &mut Context<Self>) {
        let Some(TabDialog::SessionLoader(state)) = &mut self.dialog else {
            return;
        };
        if state.selected == next {
            return;
        }
        state.selected = next;
        state.events.clear();
        state.first_event = None;
        state.total_events = 0;
        state.has_earlier = false;
        state.loading_earlier = false;
        state.loading_detail = state.selected.is_some();
        state.error = None;
        state.preview_scroll.set_offset(Point::default());
        if state.loading_detail {
            self.request_loader_detail(None);
        }
        cx.notify();
    }

    fn request_loader_detail(&mut self, page: Option<(i64, i64)>) {
        let Some(TabDialog::SessionLoader(state)) = &mut self.dialog else {
            return;
        };
        let Some(id) = state
            .selected
            .as_ref()
            .filter(|_| state.pending_detail.is_none())
        else {
            return;
        };
        let start = page.map(|(start, _)| start);
        match self.commands.send_at(
            self.runtime_generation,
            Command::GetProviderSession {
                cwd: state.cwd.clone(),
                provider_session_id: id.clone(),
                start_event_index: start,
                limit: page.map(|(_, limit)| limit),
            },
        ) {
            Ok(()) => {
                state.pending_detail = Some(LoaderDetailRequest {
                    id: id.clone(),
                    start,
                });
                state.loading_detail = page.is_none();
                state.loading_earlier = page.is_some();
            }
            Err(error) => {
                state.loading_detail = false;
                state.loading_earlier = false;
                state.error = Some(error.to_string());
            }
        }
    }

    fn load_earlier_session_events(&mut self, cx: &mut Context<Self>) {
        let Some(TabDialog::SessionLoader(state)) = &mut self.dialog else {
            return;
        };
        let Some(first) = state.first_event.filter(|first| *first > 1) else {
            return;
        };
        if !state.has_earlier || state.pending_detail.is_some() {
            return;
        }
        let count = (first - 1).min(SESSION_LOADER_PAGE_SIZE);
        let start = first - count;
        state.error = None;
        self.request_loader_detail(Some((start, count)));
        cx.notify();
    }

    fn submit_session_loader(&mut self, cx: &mut Context<Self>) {
        let Some(TabDialog::SessionLoader(state)) = &self.dialog else {
            return;
        };
        if !self.session_loader_can_submit(state) {
            return;
        }
        let Some(session) = state
            .sessions
            .iter()
            .find(|session| Some(&session.id) == state.selected.as_ref())
        else {
            return;
        };
        let session_id = state.session_id.clone();
        let previous_revision = self
            .graph_node(&session_id)
            .and_then(|node| node["providerSpawnRevision"].as_u64())
            .unwrap_or(0);
        let load = LoaderLoad {
            id: session.id.clone(),
            title: session.title.clone(),
            previous_revision,
        };
        let result = self.commands.send_at(
            self.runtime_generation,
            Command::LoadProviderSession {
                session_id: session_id.clone(),
                provider_session_id: load.id.clone(),
            },
        );
        if let Some(TabDialog::SessionLoader(state)) = &mut self.dialog {
            state.error = result.err().map(|error| error.to_string());
            if state.error.is_none() {
                self.pending_loader_loads.insert(session_id, load);
            }
        }
        cx.notify();
    }

    fn loader_catalog_result(&mut self, results: &[Value], cx: &mut Context<Self>) {
        let Some(TabDialog::SessionLoader(state)) = &mut self.dialog else {
            return;
        };
        let Some(result) = results
            .iter()
            .find(|result| state.loading_list && result["cwd"] == state.cwd)
        else {
            return;
        };
        state.loading_list = false;
        if let Some(error) = result["error"].as_str() {
            state.error = Some(error.to_owned());
        } else if let Some(sessions) = result["sessions"].as_array() {
            state.sessions = sessions
                .iter()
                .filter_map(LoaderSession::from_graph)
                .collect();
            self.refresh_loader_selection(true, cx);
        }
    }

    fn loader_detail_result(&mut self, results: &[Value], cx: &mut Context<Self>) {
        let Some(TabDialog::SessionLoader(state)) = &mut self.dialog else {
            return;
        };
        let Some(request) = &state.pending_detail else {
            return;
        };
        let Some(result) = results.iter().find(|result| {
            result["cwd"] == state.cwd
                && result["providerSessionId"] == request.id
                && (result["error"].is_string()
                    || match request.start {
                        Some(start) => {
                            result["page"]["startEventIndex"].as_i64() == Some(start)
                                || result["page"]["hasLater"] == false
                        }
                        None => result["page"]["hasLater"] == false,
                    })
        }) else {
            return;
        };
        let request = state.pending_detail.take().unwrap();
        state.loading_earlier = false;
        if state.selected.as_ref() != Some(&request.id)
            || request.start.is_some() && state.first_event.is_none()
        {
            self.request_loader_detail(None);
            cx.notify();
            return;
        }
        state.loading_detail = false;
        if let Some(error) = result["error"].as_str() {
            state.error = Some(error.to_owned());
        } else if let Some(rows) = result["events"].as_array() {
            let page = &result["page"];
            let previous_first = state.first_event;
            state.first_event = page["startEventIndex"].as_i64();
            state.total_events = page["total"].as_u64().unwrap_or(0) as usize;
            state.has_earlier = page["hasEarlier"] == true;
            let events = rows.iter().filter_map(LoaderEvent::from_graph);
            if request.start.is_some() && page["hasLater"] == true {
                let first = previous_first.unwrap_or_default() as usize;
                let mut previous: Vec<_> = events.filter(|event| event.index < first).collect();
                let anchor = previous.len() + usize::from(state.has_earlier);
                previous.append(&mut state.events);
                state.events = previous;
                state.preview_scroll.scroll_to_top_of_item(anchor);
            } else {
                state.events = events.collect();
                state.preview_scroll.scroll_to_bottom();
            }
        }
        cx.notify();
    }

    fn finish_session_loader(
        &mut self,
        errors: &[Value],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let nodes = session_levels::nodes(&self.graph);
        let identities = self.graph["providerIdentityEvents"].as_array();
        let visible_target = match &self.dialog {
            Some(TabDialog::SessionLoader(state)) => Some(state.session_id.as_str()),
            _ => None,
        };
        let mut outcome = None;
        // The runtime request can still finish after the user dismisses the modal.
        self.pending_loader_loads.retain(|session_id, load| {
            let error = errors.iter().find_map(|result| {
                (result["sessionId"] == *session_id
                    && (result["providerSessionId"].is_null()
                        || result["providerSessionId"] == load.id))
                    .then(|| result["error"].as_str())
                    .flatten()
            });
            let newer_revision = |value: &Value| {
                value
                    .as_u64()
                    .is_some_and(|revision| revision > load.previous_revision)
            };
            let identity = identities.and_then(|events| {
                events.iter().find(|result| {
                    result["sessionId"] == *session_id
                        && result["providerSessionId"] == load.id
                        && match result["result"].as_str() {
                            Some("confirmed") => {
                                result["reason"] == "already-loaded"
                                    || newer_revision(&result["spawnRevision"])
                            }
                            Some("restore-failed") => newer_revision(&result["spawnRevision"]),
                            _ => false,
                        }
                })
            });
            let error = error.or_else(|| {
                identity
                    .filter(|result| result["result"] == "restore-failed")
                    .and_then(|result| result["restoreError"].as_str())
            });
            // The committed binding hands off subsequent recovery to the card.
            let committed = session_levels::node(nodes, session_id).is_some_and(|node| {
                node["currentMode"] == "claude-code"
                    && newer_revision(&node["providerSpawnRevision"])
                    && node["title"] == load.title
            });
            if error.is_none()
                && !committed
                && !identity.is_some_and(|result| result["result"] == "confirmed")
            {
                return true;
            }
            if visible_target == Some(session_id.as_str()) {
                outcome = Some(error.map_or(Ok(()), |error| Err(error.to_owned())));
            }
            false
        });
        match outcome {
            Some(Ok(())) => self.close_dialog(window, cx),
            Some(Err(error)) => {
                if let Some(TabDialog::SessionLoader(state)) = &mut self.dialog {
                    state.error = Some(error);
                }
            }
            None => {}
        }
    }

    fn fail_pending_loader_commands(&mut self, message: String) {
        if let Some(TabDialog::SessionLoader(state)) = &mut self.dialog
            && (state.loading_list
                || state.pending_detail.is_some()
                || self.pending_loader_loads.contains_key(&state.session_id))
        {
            state.loading_list = false;
            state.loading_detail = false;
            state.loading_earlier = false;
            state.pending_detail = None;
            state.error = Some(message);
        }
        self.pending_loader_loads.clear();
    }

    fn launch_action_blocked_reason(&self, id: &str, node: &Value) -> Option<&'static str> {
        match self.commands.connection() {
            (_, ConnectionPhase::Blocked(block)) if block.kind == StartupBlockKind::ReadOnly => {
                Some("数据库处于只读恢复模式")
            }
            (generation, ConnectionPhase::Ready)
                if generation == self.runtime_generation
                    && self.domain_ready
                    && self.transport_connected =>
            {
                self.card_mutation_blocked_reason(id, node)
            }
            _ => Some("当前终端仍在恢复"),
        }
    }

    fn provider_restore_state(node: &Value) -> Option<&str> {
        if node["currentMode"] != "claude-code" {
            return None;
        }
        node["providerRestoreState"]
            .as_str()
            .filter(|state| matches!(*state, "restoring" | "failed"))
    }

    fn provider_restore_banner<'a>(
        &self,
        id: &str,
        node: &'a Value,
    ) -> Option<ProviderRestoreBanner<'a>> {
        if !node["archivedAt"].is_null() || node["forkState"] == "failed" {
            return None;
        }
        if Self::provider_restore_state(node)? == "restoring" {
            return Some(ProviderRestoreBanner::Restoring {
                title: "正在恢复 Claude Code 会话…",
            });
        }
        let reason = match node["providerRestoreError"].as_str() {
            Some("provider session running in background") => {
                "此会话仍在 Claude 后台运行。请先停止对应后台会话，再重试恢复；聊天记录会保留。"
            }
            None | Some("") => "终端进程未能启动",
            Some(error) => error,
        };
        let pending = self.pending_provider_restores.contains_key(id);
        let fresh_blocked = self.launch_action_blocked_reason(id, node);
        let retry_blocked = fresh_blocked.or_else(|| {
            matches!(
                self.recovery.get(id),
                Some(RecoveryState::Restoring | RecoveryState::Restarting(_))
            )
            .then_some("当前终端仍在恢复")
        });
        Some(ProviderRestoreBanner::Failed {
            title: "Claude Code 恢复失败",
            reason,
            retry: if pending {
                "正在恢复…"
            } else {
                "重试恢复"
            },
            fresh: "新开 Claude Code",
            pending,
            retry_blocked,
            fresh_blocked,
        })
    }

    fn run_provider_restore(
        &mut self,
        id: &str,
        fresh: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.order.iter().any(|session| session == id)
            || self.dialog.is_some()
            || self.pending_close.is_some()
            || self.pending_card_action.is_some()
        {
            return;
        }
        let Some(node) = self.graph_node(id) else {
            return;
        };
        let Some(ProviderRestoreBanner::Failed {
            pending: false,
            retry_blocked,
            fresh_blocked,
            ..
        }) = self.provider_restore_banner(id, node)
        else {
            return;
        };
        let blocked = if fresh { fresh_blocked } else { retry_blocked };
        if blocked.is_some() {
            return;
        }
        let revision = node["providerSpawnRevision"].as_u64();
        let session_id = id.to_owned();
        let command = if fresh {
            Command::StartFreshProviderSession { session_id }
        } else {
            Command::RetryProviderRestore { session_id }
        };
        match self.commands.send_at(self.runtime_generation, command) {
            Ok(()) => {
                let session_id = id.to_owned();
                let watchdog = cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(Duration::from_secs(8)).await;
                    let _ = this.update(cx, |this, cx| {
                        // The host can ignore a command while the old PTY retires.
                        // Allow another explicit click if no graph confirms it.
                        this.pending_provider_restores.remove(&session_id);
                        cx.notify();
                    });
                });
                self.pending_provider_restores.insert(
                    id.to_owned(),
                    PendingProviderRestore {
                        revision,
                        _watchdog: watchdog,
                    },
                );
                self.clear_start_failure_focus(id, window);
                self.start_failure_controls.remove(id);
                self.recovery_controls.remove(id);
                if matches!(
                    self.recovery.get(id),
                    Some(RecoveryState::Failed(_) | RecoveryState::StartFailed(_))
                ) {
                    self.recovery.remove(id);
                }
                window.blur();
            }
            Err(error) => self.show_toast(error.to_string(), cx),
        }
        cx.notify();
    }

    fn start_failure_card(&self, id: &str, node: &Value) -> Option<StartFailureCard<'_>> {
        if !node["archivedAt"].is_null() || Self::provider_restore_state(node).is_some() {
            return None;
        }
        let RecoveryState::StartFailed(error) = self.recovery.get(id)? else {
            return None;
        };
        Some(StartFailureCard {
            title: "会话启动失败",
            reason: if error.is_empty() {
                "终端进程未能启动"
            } else {
                error
            },
            retry: "重试启动",
            remove: "移除失败会话",
            blocked: self.launch_action_blocked_reason(id, node),
        })
    }

    fn start_failure_action_available(&self, id: &str) -> bool {
        self.active_scene_id().is_some()
            && self.dialog.is_none()
            && self.pending_close.is_none()
            && self.pending_card_action.is_none()
            && self.graph_node(id).is_some_and(|node| {
                self.start_failure_card(id, node)
                    .is_some_and(|card| card.blocked.is_none())
            })
    }

    fn retry_session_start(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !self.start_failure_action_available(id) {
            return;
        }
        let Some(RecoveryState::StartFailed(reason)) = self.recovery.get(id) else {
            return;
        };
        if self
            .commands
            .send_at(
                self.runtime_generation,
                Command::RestartSession {
                    session_id: id.to_owned(),
                },
            )
            .is_ok()
        {
            self.recovery
                .insert(id.to_owned(), RecoveryState::Restarting(reason.clone()));
            self.clear_start_failure_focus(id, window);
            self.start_failure_controls.remove(id);
            cx.notify();
        }
    }

    fn remove_failed_session(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.start_failure_action_available(id) {
            return;
        }
        let Some(scene_id) = self.active_scene_id() else {
            return;
        };
        self.send_card_action(
            Command::CloseSessionCard {
                scene_id: scene_id.to_owned(),
                session_id: id.to_owned(),
                confirmed_intent: Some(format!("delete-session:{id}")),
                preserve_scene_on_last_session: false,
            },
            cx,
        );
    }

    fn open_card_dialog(
        &mut self,
        dialog: TabDialog,
        controls: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dialog = Some(dialog);
        self.dialog_controls = (0..controls)
            .map(|_| cx.focus_handle().tab_stop(true))
            .collect();
        window.focus(&self.dialog_controls[0]);
        cx.notify();
    }

    fn open_last_session(
        &mut self,
        scene_id: String,
        session_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let return_focus = window.focused(cx);
        self.open_card_dialog(
            TabDialog::LastSession {
                scene_id,
                session_id,
                return_focus,
            },
            2,
            window,
            cx,
        );
    }

    fn request_close_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog.is_some()
            || self.pending_close.is_some()
            || self.pending_card_action.is_some()
        {
            return;
        }
        let (Some(scene_id), Some(id)) = (self.active_scene_id(), self.active_level_focus()) else {
            return;
        };
        if !self.card_action_available(id) {
            return;
        }
        let (scene_id, session_id) = (scene_id.to_owned(), id.to_owned());
        if self.graph["workspaceSessionCount"].as_u64() == Some(1) {
            self.open_last_session(scene_id, session_id, window, cx);
            return;
        }
        let node = self.graph_node(id).expect("available session");
        let active = matches!(node["workStatus"].as_str(), Some("running" | "needs-input"));
        let child_count =
            session_levels::children_of(session_levels::nodes(&self.graph), Some(id)).count();
        if !active && child_count == 0 {
            self.send_card_action(
                Command::CloseSessionCard {
                    scene_id,
                    session_id,
                    confirmed_intent: None,
                    preserve_scene_on_last_session: false,
                },
                cx,
            );
            return;
        }
        let title = node["title"].as_str().unwrap_or("当前会话");
        let activity = if active {
            "正在运行"
        } else {
            "当前空闲"
        };
        let descendants = if child_count > 0 {
            format!("，并有 {child_count} 个子会话")
        } else {
            String::new()
        };
        let body = format!(
            "“{title}”{activity}{descendants}。停止后，该节点会在会话列表和 DAG 中保持为“已停止”，子会话继续工作。"
        );
        self.open_card_dialog(
            TabDialog::CloseSession {
                scene_id,
                session_id,
                body,
            },
            3,
            window,
            cx,
        );
    }

    fn send_card_action(&mut self, command: Command, cx: &mut Context<Self>) {
        let (scene_id, session_id) = match &command {
            Command::CloseSessionCard {
                scene_id,
                session_id,
                ..
            }
            | Command::RemoveSessionBranch {
                scene_id,
                session_id,
                ..
            } => (scene_id, session_id),
            _ => unreachable!("expected a session card action"),
        };
        let target = (scene_id.clone(), session_id.clone());
        if self
            .commands
            .send_at(self.runtime_generation, command)
            .is_ok()
        {
            self.pending_card_action = Some(target);
        }
        cx.notify();
    }

    fn finish_card_action(&mut self, action: &Value) -> bool {
        let matches = self
            .pending_card_action
            .as_ref()
            .is_some_and(|(scene_id, session_id)| {
                action["sceneId"] == *scene_id && action["sessionId"] == *session_id
            });
        if matches {
            self.pending_card_action = None;
        }
        matches
    }

    fn open_remove_session(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog.is_some()
            || self.pending_close.is_some()
            || self.pending_card_action.is_some()
            || !self.remove_session_available(id)
        {
            return;
        }
        let (Some(scene_id), Some(text)) = (
            self.active_scene_id(),
            self.removal_dialog_text(id, RemoveScope::NodeOnly),
        ) else {
            return;
        };
        let dialog = TabDialog::RemoveSession {
            scene_id: scene_id.to_owned(),
            session_id: id.to_owned(),
            scope: RemoveScope::NodeOnly,
        };
        self.open_card_dialog(
            dialog,
            if text.choices.is_some() { 5 } else { 3 },
            window,
            cx,
        );
    }

    fn select_remove_scope(&mut self, scope: RemoveScope, cx: &mut Context<Self>) {
        if let Some(TabDialog::RemoveSession {
            scope: selected, ..
        }) = &mut self.dialog
        {
            *selected = scope;
            cx.notify();
        }
    }

    fn removal_branch_impact(&self, current: &Value) -> RemovalImpact {
        let mut children: HashMap<&str, Vec<&Value>> = HashMap::new();
        for node in session_levels::nodes(&self.graph) {
            if let Some(parent) = session_levels::parent_id(node) {
                children.entry(parent).or_default().push(node);
            }
        }
        let mut branch = RemovalImpact::default();
        let mut stack = vec![current];
        let mut seen = HashSet::new();
        while let Some(node) = stack.pop() {
            let Some(session_id) = node["sessionId"].as_str() else {
                continue;
            };
            if seen.insert(session_id) {
                branch.add(node);
                if let Some(children) = children.get(session_id) {
                    stack.extend(children);
                }
            }
        }
        branch
    }

    fn removal_dialog_text(&self, id: &str, scope: RemoveScope) -> Option<RemovalDialogText> {
        let current = self.graph_node(id)?;
        let title = format!("移除节点“{}”？", current["title"].as_str()?);
        let mut node_only = RemovalImpact::default();
        node_only.add(current);
        let branch = self.removal_branch_impact(current);
        if branch.sessions == 1 {
            return Some(RemovalDialogText {
                title,
                body: format!(
                    "{}。移除后，该会话会从会话列表和 DAG 中消失。",
                    node_only.label()
                ),
                files_unchanged: Some("项目文件和自有 Worktree 保持原样。"),
                choices: None,
                warning: None,
                confirm: "移除".into(),
            });
        }
        let description = if session_levels::parent_id(current).is_some() {
            "后代会话将重连到当前节点的父级。"
        } else {
            "直接后代会话将成为根节点。"
        };
        let (activity, confirm) = match scope {
            RemoveScope::NodeOnly => (node_only.activity(), "移除当前节点".into()),
            RemoveScope::NodeAndDescendants => (
                branch.activity(),
                format!("移除 {} 个会话", branch.sessions),
            ),
        };
        Some(RemovalDialogText {
            title,
            body: "请选择移除范围。项目文件和自有 Worktree 保持原样。".into(),
            files_unchanged: None,
            choices: Some([
                RemovalChoiceText {
                    title: "仅移除当前节点",
                    impact: node_only.label(),
                    description: description.into(),
                },
                RemovalChoiceText {
                    title: "移除当前节点及全部后代",
                    impact: branch.label(),
                    description: format!(
                        "当前节点与 {} 个后代会话会全部移除。",
                        branch.sessions - 1
                    ),
                },
            ]),
            warning: (!activity.is_empty()).then(|| format!("其中 {activity}的会话将先停止。")),
            confirm,
        })
    }

    fn level_parent(&self) -> Option<&str> {
        let explicit_parent = self
            .active_scene_id()
            .and_then(|scene_id| self.level_parent_by_scene.get(scene_id))
            .map(Option::as_deref);
        session_levels::level_parent(
            session_levels::nodes(&self.graph),
            explicit_parent,
            self.focused.as_deref(),
        )
    }

    fn level_focus(&self) -> Option<&str> {
        self.focused
            .as_ref()
            .filter(|id| self.order.contains(id))
            .or_else(|| self.order.first())
            .map(String::as_str)
    }

    fn active_level_focus(&self) -> Option<&str> {
        // Visual focus can expand a stopped card. Runtime actions require a live session.
        self.level_focus().filter(|id| !self.is_stopped_session(id))
    }

    fn breadcrumb_labels(&self) -> (String, String) {
        let label = self
            .level_parent()
            .and_then(|id| self.graph_node(id))
            .and_then(|node| node["title"].as_str())
            .filter(|title| !title.is_empty())
            .map_or_else(|| "根会话".into(), |title| format!("{title} 的子会话"));
        (label, format!("· {} 个会话", self.order.len()))
    }

    fn child_badge(&self, id: &str) -> Option<session_levels::ChildBadge> {
        session_levels::child_badge(session_levels::children_of(
            session_levels::nodes(&self.graph),
            Some(id),
        ))
    }

    fn refresh_level(&mut self, cx: &mut Context<Self>) {
        if let Some(scene) = self.active_scene_id()
            && let Some(Some(parent)) = self.level_parent_by_scene.get(scene)
            && self.graph_node(parent).is_none()
        {
            eprintln!(
                "Session level parent {parent} is missing in scene {scene}; returning to root"
            );
            self.level_parent_by_scene.insert(scene.to_owned(), None);
        }
        let parent = self.level_parent();
        let owner = self
            .active_scene_id()
            .map(|scene| session_levels::owner_key(scene, parent));
        self.order = session_levels::children_of(session_levels::nodes(&self.graph), parent)
            .filter_map(|node| node["sessionId"].as_str().map(str::to_owned))
            .collect();
        let now = self.motion_clock.elapsed().as_secs_f64() * 1000.;
        if self.carousel_owner != owner {
            self.flush_geometry(cx);
            self.carousel_owner = owner;
            self.restore_geometry = true;
            self.focus_follow_until = (!self.order.is_empty()).then_some(now + 440.);
            self.stop_edge_browse(false);
            self.pointer_position = None;
            self.card_motion.clear();
            self.card_geometry.clear();
            self.hovered = None;
            self.hovered_child_badge = None;
            self.hover_follow_until = None;
            self.hover_baseline = None;
            self.hover_restore_at = None;
            self.carousel_scroll.set_offset(point(px(0.), px(0.)));
        }
        let focus = self.level_focus().map(str::to_owned);
        for id in &self.order {
            let selected = if focus.as_ref() == Some(id) { 1. } else { 0. };
            let motion = self
                .card_motion
                .entry(id.clone())
                .or_insert_with(|| CardMotion {
                    width: Transition::new(selected),
                    lift: Transition::new(selected),
                });
            motion.lift.retarget(selected, now, 180., Curve::Lift);
        }
    }

    fn open_child_level(&mut self, id: &str, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(scene) = self.active_scene_id().map(str::to_owned) else {
            return;
        };
        let nodes = session_levels::nodes(&self.graph);
        if self
            .graph_node(id)
            .is_none_or(|node| !node["archivedAt"].is_null())
            || !self.order.iter().any(|card| card == id)
        {
            return;
        }
        let children = || session_levels::children_of(nodes, Some(id));
        if children().next().is_none() {
            return;
        }
        let preferred = session_levels::preferred_child(children()).map(str::to_owned);
        self.level_parent_by_scene
            .insert(scene.clone(), Some(id.to_owned()));
        self.refresh_level(cx);
        if let Some(session_id) = preferred {
            self.send(
                Command::FocusSession {
                    scene_id: scene,
                    session_id,
                },
                cx,
            );
            if self.pending_focus.is_some() {
                self.level_focus_request = Some(LevelFocusRequest::OpenChildren);
            }
        }
        cx.notify();
    }

    fn return_to_parent(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(scene) = self.active_scene_id().map(str::to_owned) else {
            return;
        };
        let Some(parent) = self.level_parent().map(str::to_owned) else {
            return;
        };
        let ancestor = self
            .graph_node(&parent)
            .and_then(session_levels::parent_id)
            .filter(|id| self.graph_node(id).is_some())
            .map(str::to_owned);
        self.level_parent_by_scene.insert(scene.clone(), ancestor);
        self.refresh_level(cx);
        self.send(
            Command::FocusSession {
                scene_id: scene,
                session_id: parent,
            },
            cx,
        );
        if self.pending_focus.is_some() {
            self.level_focus_request = Some(LevelFocusRequest::ReturnParent);
        }
        cx.notify();
    }

    fn jump_to_scene(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(scene) = self.scenes.get(index) {
            self.send(
                Command::SelectScene {
                    scene_id: scene.id.clone(),
                },
                cx,
            );
        }
    }

    fn step_scene(&mut self, offset: isize, cx: &mut Context<Self>) {
        let index = self
            .scenes
            .iter()
            .position(|scene| Some(scene.id.as_str()) == self.active_scene_id());
        let len = self.scenes.len().max(1) as isize;
        self.jump_to_scene(
            (index.map_or(-1, |i| i as isize) + offset).rem_euclid(len) as usize,
            cx,
        );
    }

    fn move_scene(&mut self, offset: isize, cx: &mut Context<Self>) {
        let Some(index) = self
            .scenes
            .iter()
            .position(|scene| Some(scene.id.as_str()) == self.active_scene_id())
        else {
            return;
        };
        let Some(target) = index
            .checked_add_signed(offset)
            .filter(|&i| i < self.scenes.len())
        else {
            return;
        };
        let before = if offset < 0 { target } else { target + 1 };
        self.send(
            Command::ReorderScene {
                scene_id: self.scenes[index].id.clone(),
                before_scene_id: self.scenes.get(before).map(|scene| scene.id.clone()),
            },
            cx,
        );
    }

    fn step_session(&mut self, offset: isize, cx: &mut Context<Self>) {
        let sessions = self
            .order
            .iter()
            .filter(|id| {
                self.graph_node(id)
                    .is_some_and(|node| node["archivedAt"].is_null())
            })
            .collect::<Vec<_>>();
        if sessions.len() <= 1 {
            return;
        }
        let Some(scene_id) = self.active_scene_id() else {
            return;
        };
        let current = sessions
            .iter()
            .position(|id| Some(*id) == self.focused.as_ref())
            .unwrap_or(0);
        let next = (current as isize + offset).rem_euclid(sessions.len() as isize) as usize;
        self.send(
            Command::FocusSession {
                scene_id: scene_id.to_owned(),
                session_id: sessions[next].clone(),
            },
            cx,
        );
    }

    fn workspace_shortcut(&mut self, stroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        let mods = stroke.modifiers;
        let mac = cfg!(target_os = "macos");
        if mods.alt || !(if mac { mods.platform } else { mods.control }) {
            return false;
        }
        match (stroke.key.as_str(), mods.shift) {
            ("d", false) => self.split_horizontal(cx),
            ("d", true) => {}
            ("t", false) => self.create_canvas(cx),
            ("]", false) => self.step_session(1, cx),
            ("[", false) => self.step_session(-1, cx),
            ("]" | "}", true) => self.step_scene(1, cx),
            ("[" | "{", true) => self.step_scene(-1, cx),
            ("tab", shift) if !mac => self.step_scene(if shift { -1 } else { 1 }, cx),
            ("left" | "pageup", true) => self.move_scene(-1, cx),
            ("right" | "pagedown", true) => self.move_scene(1, cx),
            (key, false) if matches!(key.as_bytes(), [b'1'..=b'9']) => {
                self.jump_to_scene(usize::from(key.as_bytes()[0] - b'1'), cx);
            }
            _ => return false,
        }
        true
    }

    fn tab_key(&mut self, stroke: &Keystroke, window: &mut Window, cx: &mut Context<Self>) {
        let mods = stroke.modifiers;
        if stroke.key == "escape" && self.notification_open {
            self.close_notifications(cx);
            cx.stop_propagation();
            return;
        }
        if let Some(TabDialog::Branch(state)) = &self.dialog {
            let input = state.input.read(cx);
            if stroke.key == "escape" && !input.composing() {
                self.cancel_fork(window, cx);
                cx.stop_propagation();
            } else if stroke.key == "tab" && !mods.alt && !mods.control && !mods.platform {
                if !state.submitting && !input.composing() {
                    let controls = [
                        &self.dialog_controls[0],
                        &input.focus,
                        &self.dialog_controls[1],
                        &self.dialog_controls[2],
                    ];
                    let current = controls.iter().position(|focus| focus.is_focused(window));
                    let next = match (current, mods.shift) {
                        (Some(index), true) => (index + controls.len() - 1) % controls.len(),
                        (Some(index), false) => (index + 1) % controls.len(),
                        (None, true) => controls.len() - 1,
                        (None, false) => 0,
                    };
                    window.focus(controls[next]);
                    self.keyboard_focus = true;
                    cx.notify();
                }
                cx.stop_propagation();
            }
            return;
        }
        if stroke.key == "escape" && self.fork_menu.take().is_some() {
            cx.notify();
            cx.stop_propagation();
            return;
        }
        if stroke.key == "escape" && self.permission_menu.is_some() {
            self.dismiss_permission_menu(window, cx);
            cx.stop_propagation();
            return;
        }
        if let Some(TabDialog::SessionLoader(state)) = &self.dialog {
            if stroke.key == "escape" {
                self.close_dialog(window, cx);
                cx.stop_propagation();
            } else if stroke.key == "tab" && !mods.alt && !mods.control && !mods.platform {
                let input = state.input.read(cx);
                let mut controls = vec![&self.dialog_controls[0], &input.focus];
                if !input.text.is_empty() {
                    controls.push(&self.dialog_controls[3]);
                }
                controls.push(&state.list_focus);
                if state.has_earlier && !state.loading_earlier {
                    controls.push(&self.dialog_controls[1]);
                }
                if self.session_loader_can_submit(state) {
                    controls.push(&self.dialog_controls[2]);
                }
                let current = controls.iter().position(|focus| focus.is_focused(window));
                let next = match (current, mods.shift) {
                    (Some(index), true) => (index + controls.len() - 1) % controls.len(),
                    (Some(index), false) => (index + 1) % controls.len(),
                    (None, true) => controls.len() - 1,
                    (None, false) => 0,
                };
                window.focus(controls[next]);
                self.keyboard_focus = true;
                cx.stop_propagation();
                cx.notify();
            }
            return;
        }
        if cfg!(target_os = "macos")
            && stroke.key == "w"
            && mods.platform
            && !mods.alt
            && !mods.control
            && !mods.shift
        {
            self.request_close_session(window, cx);
            cx.stop_propagation();
            return;
        }
        if self.search_key(stroke, window, cx) {
            cx.stop_propagation();
            return;
        }
        if self.search.as_ref().is_some_and(|search| {
            search.open
                && (search.input.read(cx).focus.is_focused(window)
                    || search.controls.iter().any(|focus| focus.is_focused(window)))
        }) {
            return;
        }
        if stroke.key == "escape" && self.workspace_drag_source.is_some() && cx.has_active_drag() {
            cx.stop_active_drag(window);
            self.workspace_drag_source = None;
            self.workspace_drag_target = None;
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if self.workspace_shortcut(stroke, cx) {
            cx.stop_propagation();
            return;
        }
        let confirmation = matches!(
            self.dialog,
            Some(
                TabDialog::Close { .. }
                    | TabDialog::DeleteTask { .. }
                    | TabDialog::RemoveWorkspace { .. }
                    | TabDialog::CloseSession { .. }
                    | TabDialog::LastSession { .. }
                    | TabDialog::RemoveSession { .. }
                    | TabDialog::PermissionMode(_)
            )
        );
        if stroke.key == "escape" {
            if confirmation {
                self.close_dialog(window, cx);
                cx.stop_propagation();
            } else if self.workspace_menu.take().is_some()
                || self.task_menu.take().is_some()
                || self.tab_menu.take().is_some()
            {
                // SceneTabBar dismisses this menu in the window bubble listener;
                // it neither moves focus nor consumes the underlying Escape.
                cx.notify();
            }
            return;
        }
        let m = stroke.modifiers;
        if stroke.key != "tab" || m.alt || m.control || m.platform {
            return;
        }
        if !confirmation
            && (self
                .terminals
                .values()
                .any(|terminal| terminal.read(cx).focus.is_focused(window))
                || matches!(&self.dialog, Some(TabDialog::Rename { input, .. } | TabDialog::RenameTask { input, .. }) if input.read(cx).focus.is_focused(window) && input.read(cx).composing()))
        {
            return;
        }
        self.keyboard_focus = true;
        if confirmation {
            let controls = &self.dialog_controls;
            let current = controls.iter().position(|focus| focus.is_focused(window));
            let next = match (current, m.shift) {
                (Some(i), true) => (i + controls.len() - 1) % controls.len(),
                (Some(i), false) => (i + 1) % controls.len(),
                (None, true) => controls.len() - 1,
                (None, false) => 0,
            };
            window.focus(&controls[next]);
        } else {
            if m.shift {
                window.focus_prev();
            } else {
                window.focus_next();
            }
            if let Some(index) = self.scenes.iter().position(|scene| {
                self.tab_controls[&scene.id]
                    .iter()
                    .any(|focus| focus.is_focused(window))
            }) {
                self.tab_scroll.scroll_to_item(index);
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn focus_control<E: InteractiveElement + ParentElement + Styled>(
        &self,
        mut element: E,
        focus: &FocusHandle,
    ) -> E {
        let style_radii = &element.style().corner_radii;
        let radii = Corners {
            top_left: style_radii.top_left.unwrap_or_default(),
            top_right: style_radii.top_right.unwrap_or_default(),
            bottom_right: style_radii.bottom_right.unwrap_or_default(),
            bottom_left: style_radii.bottom_left.unwrap_or_default(),
        };
        let element = element.track_focus(focus);
        if !self.keyboard_focus {
            return element;
        }
        let focus = focus.clone();
        // Paint an outline without changing control geometry. A GPUI shadow
        // fills transparent controls and cannot represent the reference ring.
        element.relative().child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    if focus.is_focused(window) {
                        let radii = radii.map(|radius| radius.to_pixels(window.rem_size()));
                        window.paint_quad(
                            outline(bounds, rgb(0xe3ab44), BorderStyle::default())
                                .corner_radii(radii)
                                .border_widths(px(2.)),
                        );
                    }
                },
            )
            .absolute()
            .inset_0()
            .size_full(),
        )
    }

    fn control(&self, element: Div, id: &str) -> Div {
        self.focus_control(element, &self.control_focus[id])
    }

    fn button<E: StatefulInteractiveElement + ParentElement + Styled>(
        &self,
        element: E,
        focus: &FocusHandle,
        activate: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> E {
        self.button_inner(element, focus, activate, true, cx)
    }

    // Workspace titles must let mouse-down reach their draggable group.
    fn button_inner<E: StatefulInteractiveElement + ParentElement + Styled>(
        &self,
        element: E,
        focus: &FocusHandle,
        activate: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
        stop_mouse_down: bool,
        cx: &mut Context<Self>,
    ) -> E {
        let activate = Rc::new(activate);
        let key_activate = activate.clone();
        let mouse_focus = focus.clone();
        self.focus_control(element, focus)
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                window.focus(&mouse_focus);
                if stop_mouse_down {
                    cx.stop_propagation();
                }
            })
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "enter" && !event.keystroke.modifiers.modified() {
                    cx.stop_propagation();
                    key_activate(this, window, cx);
                }
            }))
            .on_click(cx.listener(move |this, event, window, cx| {
                // GPUI synthesizes both clicks on keyup. Match button Enter on keydown.
                if !stop_mouse_down && matches!(event, ClickEvent::Mouse(_)) && this.workspace_drag_gesture {
                    return;
                }
                if !matches!(event, ClickEvent::Keyboard(event) if event.button == KeyboardButton::Enter) {
                    cx.stop_propagation();
                    activate(this, window, cx);
                }
            }))
    }

    fn topbar(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let active_id = self
            .hierarchy
            .as_ref()
            .and_then(|h| h.scene.as_ref())
            .map(|s| &s.id);
        let mut tabs = div()
            .id("scene-tabs")
            .flex_1()
            .min_w_0()
            .flex()
            .items_center()
            .overflow_x_scroll()
            .overflow_y_hidden()
            .track_scroll(&self.tab_scroll);
        for scene in &self.scenes {
            let active = active_id == Some(&scene.id);
            let select_id = scene.id.clone();
            let rename_id = scene.id.clone();
            let close_id = scene.id.clone();
            let menu_id = scene.id.clone();
            let hover_id = scene.id.clone();
            let title = self.scene_title(scene);
            let [title_focus, close_focus] = &self.tab_controls[&scene.id];
            let double_click_focus = title_focus.clone();
            tabs = tabs.child(
                tabs_ui::tab(SharedString::from(format!("tab-{}", scene.id)), active)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_hover(cx.listener(move |this, hover: &bool, _, cx| {
                        this.tab_hover = hover.then(|| hover_id.clone());
                        cx.notify();
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                            let size = window.viewport_size();
                            let focus = cx.focus_handle().tab_stop(true);
                            this.tab_menu = Some((
                                menu_id.clone(),
                                point(
                                    e.position.x.clamp(px(8.), size.width - px(136.)),
                                    e.position.y.clamp(px(8.), size.height - px(44.)),
                                ),
                                focus,
                            ));
                            this.workspace_menu = None;
                            this.task_menu = None;
                            this.hidden_tabs = None;
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                    .child(
                        self.button(
                            tabs_ui::tab_title(title)
                                .id(SharedString::from(format!("tab-title-{}", scene.id))),
                            title_focus,
                            move |this, _, cx| {
                                this.send(
                                    Command::SelectScene {
                                        scene_id: select_id.clone(),
                                    },
                                    cx,
                                );
                            },
                            cx,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                                window.focus(&double_click_focus);
                                if e.click_count == 2 {
                                    this.open_rename(rename_id.clone(), window, cx);
                                }
                                cx.stop_propagation();
                            }),
                        ),
                    )
                    .child(self.button(
                        tabs_ui::tab_close(
                            SharedString::from(format!("tab-close-{}", scene.id)),
                            active
                                || self.tab_hover.as_ref() == Some(&scene.id)
                                || close_focus.is_focused(window),
                        ),
                        close_focus,
                        move |this, _, cx| {
                            this.pending_close = Some(close_id.clone());
                            this.send(
                                Command::PreviewSceneClose {
                                    scene_id: close_id.clone(),
                                },
                                cx,
                            );
                        },
                        cx,
                    )),
            );
        }
        let path_valid = self.active_path_valid();
        let add_btn = tabs_ui::tab_add(
            if self.tab_overflow {
                "create-canvas-overflow"
            } else {
                "create-canvas"
            },
            path_valid,
        );
        let add = if path_valid {
            self.button(
                add_btn,
                &self.control_focus["add-tab"],
                |this, _, cx| this.create_canvas(cx),
                cx,
            )
            .into_any_element()
        } else {
            add_btn
                .opacity(0.5)
                .cursor(CursorStyle::OperationNotAllowed)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .into_any_element()
        };
        let mut overflow_actions = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.))
            .ml(px(8.));
        if !self.tab_overflow {
            tabs = tabs.child(add);
        } else {
            overflow_actions = overflow_actions
                .child(self.button(
                    tabs_ui::tab_overflow_btn("tab-overflow"),
                    &self.control_focus["tab-overflow"],
                    |this, _, cx| {
                        if this.hidden_tabs.is_some() {
                            this.hidden_tabs = None;
                        } else {
                            let viewport = this.tab_scroll.bounds();
                            let offset = this.tab_scroll.offset();
                            let hidden = this
                                .scenes
                                .iter()
                                .enumerate()
                                .filter(|(i, _)| {
                                    this.tab_scroll.bounds_for_item(*i).is_none_or(|b| {
                                        b.left() + offset.x < viewport.left() - px(1.)
                                            || b.right() + offset.x > viewport.right() + px(1.)
                                    })
                                })
                                .map(|(_, s)| (s.id.clone(), cx.focus_handle().tab_stop(true)))
                                .collect::<Vec<_>>();
                            this.hidden_tabs = Some(hidden);
                        }
                        this.workspace_menu = None;
                        this.task_menu = None;
                        this.tab_menu = None;
                        cx.notify();
                    },
                    cx,
                ))
                .child(add);
        }
        div()
            .h(px(48.))
            .flex_none()
            .px(px(8.))
            .flex()
            .items_center()
            .bg(rgb(0xffffff))
            .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
            .child(tabs)
            .when(self.tab_overflow, |bar| bar.child(overflow_actions))
            .child(
                self.control(
                    div()
                        .size(px(28.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon("graph-ring", 16., 0x606572)),
                    "graph",
                )
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()),
            )
            .child({
                let split_disabled =
                    !path_valid || self.active_scene_id().is_none() || self.focused.is_none();
                let split_btn = div()
                    .id("create-shell")
                    .size(px(28.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon("panel-right-open", 16., 0x606572));
                if split_disabled {
                    split_btn
                        .opacity(0.5)
                        .cursor(CursorStyle::OperationNotAllowed)
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .into_any_element()
                } else {
                    self.button(
                        split_btn.cursor_pointer(),
                        &self.control_focus["split"],
                        |this, _, cx| this.split_horizontal(cx),
                        cx,
                    )
                    .into_any_element()
                }
            })
            .child(
                self.control(
                    div()
                        .size(px(30.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon("upload", 16., 0x747b88).with_transformation(
                            Transformation::rotate(Radians(std::f32::consts::PI)),
                        )),
                    "update",
                )
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()),
            )
    }

    fn remove_scope_choice(
        &self,
        choice: RemovalChoiceText,
        scope: RemoveScope,
        selected: RemoveScope,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let (id, index, other) = match scope {
            RemoveScope::NodeOnly => ("remove-node-only", 1, RemoveScope::NodeAndDescendants),
            RemoveScope::NodeAndDescendants => {
                ("remove-node-and-descendants", 2, RemoveScope::NodeOnly)
            }
        };
        let selected = scope == selected;
        let choice = div()
            .id(id)
            .flex()
            .items_start()
            .gap(px(10.))
            .p(px(12.))
            .border_1()
            .border_color(if selected {
                rgb(0x458bd7)
            } else {
                rgba(0x0000001f)
            })
            .bg(if selected {
                rgb(0xeef6ff)
            } else {
                rgba(0x00000005)
            })
            .rounded(px(9.))
            .cursor_pointer()
            .child(
                div()
                    .size(px(18.))
                    .mt(px(3.))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .size(px(13.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .border_1()
                            .border_color(if selected {
                                rgb(0x5b9de9)
                            } else {
                                rgb(0x878787)
                            })
                            .bg(if selected {
                                rgb(0x5b9de9)
                            } else {
                                rgb(0xffffff)
                            })
                            .when(selected, |radio| {
                                radio.child(div().size(px(5.)).rounded_full().bg(rgb(0xffffff)))
                            }),
                    ),
            )
            .child(
                div()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .text_color(rgb(0x2f3547))
                            .child(choice.title),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(0x596273))
                            .child(ui_text(choice.impact)),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(0x77808e))
                            .child(ui_text(choice.description)),
                    ),
            )
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if matches!(
                    event.keystroke.key.as_str(),
                    "left" | "right" | "up" | "down"
                ) && !event.keystroke.modifiers.modified()
                {
                    window.focus(&this.dialog_controls[3 - index]);
                    this.select_remove_scope(other, cx);
                    cx.stop_propagation();
                }
            }));
        self.button(
            choice,
            &self.dialog_controls[index],
            move |this, _, cx| this.select_remove_scope(scope, cx),
            cx,
        )
    }

    fn compact_session_summary(node: Option<&Value>, title: &str) -> Div {
        let status = node
            .and_then(|node| node["workStatus"].as_str())
            .unwrap_or("idle");
        let active_child_count = node
            .and_then(|node| node["activeChildCount"].as_u64())
            .unwrap_or(0);
        let summary = node
            .and_then(|node| node["latestLines"].as_array())
            .map(|lines| {
                lines
                    .iter()
                    .skip(lines.len().saturating_sub(3))
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| {
                node.and_then(|node| node["cwd"].as_str())
                    .unwrap_or("")
                    .to_owned()
            });
        div()
            .size_full()
            .px(px(12.))
            .py(px(14.))
            .bg(rgb(0xf2f4f7))
            .rounded(px(12.))
            .text_color(rgb(0x6b7482))
            .flex()
            .flex_col()
            .gap(px(9.))
            .child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(0x364050))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(title.to_owned()),
            )
            .child(
                div()
                    .text_size(px(9.))
                    .px(px(6.))
                    .py(px(2.))
                    .rounded(px(8.))
                    .bg(rgba(0xffffff0f))
                    .map(|mut badge| {
                        badge.style().align_self = Some(AlignSelf::FlexStart);
                        badge
                    })
                    .child(match status {
                        "starting" | "running" => "运行中",
                        "needs-input" => "等待输入",
                        "error" => "异常",
                        "interrupted" => "中断",
                        "exited" => "已停止",
                        _ => "空闲",
                    }),
            )
            .children((active_child_count > 0).then(|| {
                div()
                    .text_size(px(9.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0xe8ae48))
                    .child(format!("子会话 {active_child_count}"))
            }))
            .child(
                div()
                    .text_size(px(9.))
                    .font_family("Menlo")
                    .line_height(relative(1.5))
                    .overflow_hidden()
                    .child(summary.to_owned()),
            )
    }

    fn stopped_session_status(id: &str) -> Stateful<Div> {
        div()
            .id(SharedString::from(format!("stopped-session-status-{id}")))
            .flex_none()
            .px(px(7.))
            .py(px(2.))
            .rounded(px(999.))
            // The frozen light theme inherits the white-on-white label.
            .bg(white().alpha(0.06))
            .text_color(white().alpha(0.52))
            .text_size(px(9.))
            .whitespace_nowrap()
            .child(ui_text(STOPPED_SESSION_STATUS).normal_line_height())
    }

    fn stopped_session_lines(node: &Value) -> Option<String> {
        let lines = node["latestLines"]
            .as_array()
            .filter(|lines| !lines.is_empty())?;
        let end = &lines[lines.len().saturating_sub(8)..];
        Some(
            end.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }

    fn stopped_session_card(
        &self,
        id: &str,
        node: &Value,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let title = node["title"].as_str().unwrap_or("Shell");
        let remove = self.stopped_remove_visible(node).then(|| {
            let blocked_reason = self.stopped_remove_blocked_reason();
            let tooltip = RemoveNodeTooltip { blocked_reason };
            let button_id = SharedString::from(format!("remove-session-{id}"));
            let session_id = id.to_owned();
            let glyph = svg()
                .path("icons/circle-minus.svg")
                .size(px(14.))
                .flex_none()
                .text_color(rgb(0x6a7380))
                .when(blocked_reason.is_none(), |glyph| {
                    glyph.group_hover(button_id.clone(), |style| style.text_color(rgb(0xc5443e)))
                });
            let button = div()
                .id(button_id.clone())
                .group(button_id)
                .w(px(28.))
                .h(px(27.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .child(glyph)
                .tooltip(move |_, cx| cx.new(|_| tooltip).into());
            if blocked_reason.is_none() {
                self.focus_control(button, &self.remove_card_controls[id])
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(0xfff0ef)))
                    .capture_any_mouse_down(|event, window, _| {
                        if event.button == MouseButton::Left {
                            window.prevent_default();
                        }
                    })
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.open_remove_session(&session_id, window, cx);
                    }))
            } else {
                button.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            }
        });
        let monospace = if cfg!(target_os = "macos") {
            "SF Mono"
        } else {
            "Consolas"
        };
        div()
            .id(SharedString::from(format!("stopped-session-card-{id}")))
            .size_full()
            .flex()
            .flex_col()
            .gap(px(14.))
            .pb(px(18.))
            .bg(rgb(0xf8f9fb))
            .text_color(rgb(0x687180))
            .child(
                div()
                    .id(SharedString::from(format!("stopped-session-header-{id}")))
                    .h(px(42.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(14.))
                    .bg(white())
                    .child(
                        div()
                            .id(SharedString::from(format!("stopped-session-title-{id}")))
                            .min_w_0()
                            .max_h(px(42.))
                            .overflow_hidden()
                            .text_color(rgb(0x303744))
                            .text_size(px(12.))
                            .font_weight(FontWeight::BOLD)
                            .child(ui_text(title.to_owned()).normal_line_height()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .child(Self::stopped_session_status(id))
                            .children(remove),
                    ),
            )
            .children(Self::stopped_session_lines(node).map(|lines| {
                div()
                    .id(SharedString::from(format!("stopped-session-lines-{id}")))
                    .mx(px(18.))
                    .max_h(px(180.))
                    .overflow_hidden()
                    .p(px(10.))
                    .rounded(px(7.))
                    .bg(black().alpha(0.16))
                    .font_family(monospace)
                    .text_size(px(10.))
                    .line_height(px(15.5))
                    .whitespace_normal()
                    .child(lines)
            }))
    }

    fn remove_card_button(
        &self,
        id: &str,
        node: &Value,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let tooltip = self.remove_node_tooltip(id, node);
        let button = div()
            .id(SharedString::from(format!("remove-session-{id}")))
            .size(px(27.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(5.))
            .text_color(rgb(0x5f7898))
            .child(
                svg()
                    .path("icons/circle-minus.svg")
                    .size(px(14.))
                    .flex_none(),
            )
            .tooltip(move |_, cx| cx.new(|_| tooltip).into());
        if tooltip.blocked_reason.is_none() {
            let session_id = id.to_owned();
            self.button(
                button
                    .cursor_pointer()
                    .hover(|style| style.bg(rgba(0x0000000f))),
                &self.remove_card_controls[id],
                move |this, window, cx| this.open_remove_session(&session_id, window, cx),
                cx,
            )
        } else {
            button
                .opacity(0.38)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        }
    }

    fn session_loader_button(
        &self,
        id: &str,
        node: &Value,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let blocked = self.session_loader_blocked_reason(id, node);
        let tooltip: SharedString = blocked.unwrap_or(SESSION_LOADER_TITLE).to_owned().into();
        let button = div()
            .id(SharedString::from(format!("session-loader-open-{id}")))
            .size(px(24.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(5.))
            .text_color(rgba(0x00000085))
            .child(
                svg()
                    .path("icons/folder-input.svg")
                    .size(px(16.))
                    .flex_none(),
            )
            .tooltip(move |_, cx| cx.new(|_| CardTooltip(tooltip.clone())).into());
        if blocked.is_none() {
            let session_id = id.to_owned();
            self.button(
                button
                    .cursor_pointer()
                    .hover(|style| style.text_color(rgba(0x000000d1)).bg(rgba(0x0000000f))),
                &self.card_controls[id],
                move |this, window, cx| {
                    this.open_session_loader(&session_id, window, cx);
                },
                cx,
            )
        } else {
            button
                .opacity(0.38)
                .cursor(CursorStyle::OperationNotAllowed)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        }
    }

    fn render_provider_restore_banner(
        &self,
        id: &str,
        banner: ProviderRestoreBanner<'_>,
        theme: Theme,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        // The card keeps its light surface and has no theme picker; follow the
        // terminal theme only where the native recovery water already does.
        let dark = theme.background != Theme::LIGHT.background;
        let solid = |color| Hsla::from(rgb(color));
        let alpha = |color, value| Hsla::from(rgb(color)).alpha(value);
        let restoring = matches!(banner, ProviderRestoreBanner::Restoring { .. });
        let (text, background, border) = match (dark, restoring) {
            (false, false) => (solid(0x8a4d10), solid(0xfff7eb), solid(0xefcfaa)),
            (true, false) => (solid(0xe4aa78), solid(0x2a1e17), alpha(0xeea858, 0.28)),
            (false, true) => (solid(0x315f9d), solid(0xedf5ff), solid(0xc5dcf8)),
            (true, true) => (solid(0x8fb8ed), solid(0x172333), alpha(0x679fe2, 0.28)),
        };
        let row = div()
            .id(SharedString::from(format!("provider-restore-banner-{id}")))
            .w_full()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(10.))
            .py(px(7.))
            .border_t_1()
            .border_b_1()
            .border_color(border)
            .bg(background)
            .text_color(text)
            .text_size(px(11.))
            .line_height(relative(1.2))
            .when(restoring, |row| row.justify_start())
            .when(!restoring, |row| row.justify_between());
        match banner {
            ProviderRestoreBanner::Restoring { title } => {
                row.child(div().font_weight(FontWeight::BOLD).child(title))
            }
            ProviderRestoreBanner::Failed {
                title,
                reason,
                retry,
                fresh,
                pending,
                retry_blocked,
                fresh_blocked,
            } => {
                let (reason_color, button_text, button_border, button_background) = if dark {
                    (
                        solid(0xc99a73),
                        solid(0xf1c08f),
                        alpha(0xeea858, 0.55),
                        alpha(0xeea858, 0.1),
                    )
                } else {
                    (
                        solid(0x966b43),
                        solid(0x7c440d),
                        solid(0xdaa86d),
                        solid(0xffffff),
                    )
                };
                let controls = &self.provider_restore_controls[id];
                let reason = SharedString::from(reason.to_owned());
                let reason_tooltip = reason.clone();
                let mut action = |name: &str, label, blocked: Option<&'static str>, fresh: bool| {
                    let button = div()
                        .id(SharedString::from(format!("{name}-{id}")))
                        .flex_none()
                        .px(px(8.))
                        .py(px(3.))
                        .border_1()
                        .border_color(button_border)
                        .rounded(px(5.))
                        .text_color(button_text)
                        .bg(button_background)
                        .whitespace_nowrap()
                        .cursor_pointer()
                        .child(label);
                    if let Some(blocked) = blocked {
                        return button
                            .tooltip(move |_, cx| cx.new(|_| CardTooltip(blocked.into())).into());
                    }
                    if pending {
                        return button;
                    }
                    let session_id = id.to_owned();
                    self.button(
                        button,
                        &controls[usize::from(fresh)],
                        move |this, window, cx| {
                            this.run_provider_restore(&session_id, fresh, window, cx)
                        },
                        cx,
                    )
                };
                let retry_button = action("retry-provider-restore", retry, retry_blocked, false);
                let fresh_button = action("start-fresh-provider", fresh, fresh_blocked, true);
                row.child(
                    div()
                        .flex()
                        .min_w_0()
                        .items_baseline()
                        .gap(px(8.))
                        .child(div().font_weight(FontWeight::BOLD).child(title))
                        .child(
                            div()
                                .id(SharedString::from(format!("provider-restore-reason-{id}")))
                                .min_w_0()
                                .text_color(reason_color)
                                .truncate()
                                .tooltip(move |_, cx| {
                                    cx.new(|_| CardTooltip(reason_tooltip.clone())).into()
                                })
                                .child(reason),
                        ),
                )
                .child(retry_button)
                .child(fresh_button)
            }
        }
    }

    fn render_start_failure_card(
        &self,
        id: &str,
        card: StartFailureCard<'_>,
        width: f32,
        cx: &mut Context<Self>,
    ) -> Div {
        let controls = &self.start_failure_controls[id];
        let narrow = width - 20. <= 520.;
        let reason = SharedString::from(card.reason.to_owned());
        let reason_tooltip = reason.clone();
        let action = |name: &str, label| {
            div()
                .id(SharedString::from(format!("{name}-{id}")))
                .flex_none()
                .px(px(8.))
                .py(px(3.))
                .border_1()
                .border_color(Hsla::from(rgb(0xe45d55)).alpha(0.45))
                .rounded(px(5.))
                .text_color(rgb(0xf2b7b3))
                .bg(rgba(0xe45d5514))
                .whitespace_nowrap()
                .cursor_pointer()
                .child(label)
        };
        let retry = action("retry-start", card.retry);
        let remove = action("remove-failed-session", card.remove);
        let (retry, remove) = if let Some(blocked) = card.blocked {
            let disable = |button: Stateful<Div>| {
                button.tooltip(move |_, cx| cx.new(|_| CardTooltip(blocked.into())).into())
            };
            (disable(retry), disable(remove))
        } else {
            let retry_id = id.to_owned();
            let remove_id = id.to_owned();
            (
                self.button(
                    retry,
                    &controls[0],
                    move |this, window, cx| this.retry_session_start(&retry_id, window, cx),
                    cx,
                ),
                self.button(
                    remove,
                    &controls[1],
                    move |this, _, cx| this.remove_failed_session(&remove_id, cx),
                    cx,
                ),
            )
        };
        div()
            .w_full()
            .flex_none()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .px(px(10.))
            .py(px(8.))
            .border_t_1()
            .border_b_1()
            .border_color(rgb(0xedc2bf))
            .bg(rgb(0xfff3f2))
            .text_color(rgb(0x9b2f2a))
            .text_size(px(11.))
            .line_height(relative(1.2))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .min_w_0()
                    .items_baseline()
                    .gap(px(8.))
                    .when(narrow, |summary| summary.w_full())
                    .when(!narrow, |summary| summary.flex_1())
                    .child(
                        div()
                            .flex_none()
                            .font_weight(FontWeight::BOLD)
                            .whitespace_nowrap()
                            .child(card.title),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("start-failure-reason-{id}")))
                            .flex_1()
                            .min_w_0()
                            .text_color(rgb(0xa95853))
                            .when(narrow, |label| label.whitespace_normal().line_clamp(2))
                            .when(!narrow, |label| label.truncate())
                            .tooltip(move |_, cx| {
                                cx.new(|_| CardTooltip(reason_tooltip.clone())).into()
                            })
                            .child(reason),
                    ),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .ml_auto()
                    .gap(px(8.))
                    .child(retry)
                    .child(remove),
            )
    }

    fn render_session_loader(
        &self,
        state: &SessionLoader,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let viewport = window.viewport_size();
        let viewport_width = f32::from(viewport.width);
        let viewport_height = f32::from(viewport.height);
        let backdrop_pad = (viewport_width * 0.03).clamp(14., 34.);
        let panel_width = 1040_f32.min((viewport_width - backdrop_pad * 2.).max(0.));
        let panel_height = 720_f32
            .min((viewport_height - backdrop_pad * 2.).max(0.))
            .max(430.);
        let list_width = (panel_width * 0.36).max(250.);
        let events_padding = (viewport_width * 0.03).clamp(14., 28.);

        let sessions: Vec<_> = state.visible_sessions(cx).collect();
        let input = state.input.read(cx);
        let filter_empty = input.text.is_empty();
        let filter_focused = input.focus.is_focused(window);
        let selected = state
            .selected
            .as_ref()
            .and_then(|id| state.sessions.iter().find(|session| &session.id == id));
        let target_title = self
            .graph_node(&state.session_id)
            .and_then(|node| node["title"].as_str())
            .unwrap_or("Shell")
            .to_owned();
        let running = !state.history && self.session_loader_running(&state.session_id);
        let can_submit = self.session_loader_can_submit(state);
        let mut focus_ring: Hsla = rgb(0x5b9de9).into();
        focus_ring.a = 0.13;

        let filter = div()
            .id("session-loader-filter")
            .h(px(34.))
            .w_full()
            .max_w(px(310.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(7.))
            .px(px(9.))
            .border_1()
            .border_color(if filter_focused {
                rgb(0x5b9de9)
            } else {
                rgb(0x414854)
            })
            .rounded(px(9.))
            .bg(rgb(0x15181c))
            .when(filter_focused, |filter| {
                filter.shadow(vec![BoxShadow {
                    color: focus_ring,
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(3.),
                }])
            })
            .child(icon("search", 15., 0x858e9b))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .child(state.input.clone()),
            )
            .children((!filter_empty).then(|| {
                self.button(
                    div()
                        .id("session-loader-filter-clear")
                        .size(px(20.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(5.))
                        .text_size(px(14.))
                        .text_color(rgb(0x7f8895))
                        .cursor_pointer()
                        .child("×"),
                    &self.dialog_controls[3],
                    |this, window, cx| this.clear_session_loader_filter(window, cx),
                    cx,
                )
            }));

        let mut rows = Vec::with_capacity(sessions.len());
        for session in &sessions {
            let is_selected = state.selected.as_deref() == Some(session.id.as_str());
            let row_session_id = session.id.clone();
            let model = session
                .model
                .as_deref()
                .filter(|model| !model.is_empty())
                .unwrap_or("Claude Code");
            let meta = format!("{} · {}", loader_relative_time(session.updated_at), model);
            let detail = format!(
                "{} · {} 条内容",
                loader_permission_label(&session.permission_mode),
                session.message_count
            );
            let loaded = session.loaded_by.as_ref().map(|owner| {
                if owner.session_id == state.session_id {
                    "已载入当前卡片".to_owned()
                } else {
                    format!("已载入{}", owner.title)
                }
            });
            rows.push(
                div()
                    .id(SharedString::from(format!(
                        "session-loader-row-{}",
                        session.id
                    )))
                    .w_full()
                    .flex_none()
                    .mb(px(6.))
                    .border_1()
                    .border_color(if is_selected {
                        rgba(0x5b9de999)
                    } else {
                        rgba(0x00000000)
                    })
                    .rounded(px(10.))
                    .bg(if is_selected {
                        rgba(0x5b9de91a)
                    } else {
                        rgba(0xffffff06)
                    })
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_loader_session(&row_session_id, cx);
                    }))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .px(px(11.))
                            .py(px(10.))
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_size(px(11.))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(rgb(0xe8ebf0))
                                    .child(ui_text(session.title.clone()).normal_line_height()),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_size(px(9.))
                                    .text_color(rgb(0x8a939f))
                                    .child(ui_text(meta).normal_line_height()),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_size(px(9.))
                                    .text_color(rgb(0x8a939f))
                                    .child(ui_text(detail).normal_line_height()),
                            )
                            .children(loaded.map(|loaded| {
                                div()
                                    .min_w_0()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis()
                                    .text_size(px(9.))
                                    .text_color(rgb(0x8a939f))
                                    .child(ui_text(loaded).normal_line_height())
                            })),
                    ),
            );
        }

        let list_focus = state.list_focus.clone();
        let list_results = div()
            .id("session-loader-results")
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .pb(px(12.))
            .overflow_y_scroll()
            .track_scroll(&state.list_scroll)
            .track_focus(&state.list_focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |_, _, window, _| {
                    window.focus(&list_focus);
                }),
            )
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                match event.keystroke.key.as_str() {
                    "up" => {
                        cx.stop_propagation();
                        this.step_loader_session(-1, cx);
                    }
                    "down" => {
                        cx.stop_propagation();
                        this.step_loader_session(1, cx);
                    }
                    _ => {}
                }
            }))
            .children(rows);
        let list_results = if state.loading_list {
            list_results.child(
                div()
                    .w_full()
                    .py(px(7.))
                    .text_center()
                    .text_size(px(9.))
                    .text_color(rgb(0x8490a0))
                    .child(ui_text(SESSION_LOADER_LOADING_MORE).normal_line_height()),
            )
        } else if sessions.is_empty() {
            list_results.child(
                div()
                    .min_h(px(120.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .p(px(18.))
                    .text_center()
                    .text_size(px(11.))
                    .text_color(rgb(0x77818e))
                    .child(ui_text(if filter_empty {
                        SESSION_LOADER_NO_SESSIONS
                    } else {
                        SESSION_LOADER_NO_MATCHES
                    })),
            )
        } else {
            list_results
        };

        let list = div()
            .id("session-loader-list")
            .w(px(list_width))
            .flex_none()
            .min_w_0()
            .flex()
            .flex_col()
            .pt(px(14.))
            .px(px(12.))
            .border_r_1()
            .border_color(rgb(0x333943))
            .bg(rgb(0x1d2025))
            .child(filter)
            .child(
                div()
                    .h(px(32.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .text_size(px(9.))
                    .text_color(rgb(0x747e8c))
                    .child(
                        ui_text(format!(
                            "{} / {} 个会话",
                            sessions.len(),
                            state.sessions.len()
                        ))
                        .normal_line_height(),
                    ),
            )
            .child(list_results);

        let preview_title = if state.history {
            "包含压缩前的消息，不影响当前会话".to_owned()
        } else {
            selected
                .map(|session| session.title.clone())
                .unwrap_or_else(|| SESSION_LOADER_SELECT_SESSION.to_owned())
        };
        let preview_header = div()
            .h(px(48.))
            .flex_none()
            .flex()
            .items_center()
            .px(px(16.))
            .border_b_1()
            .border_color(rgb(0x303640))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(11.))
                            .font_weight(FontWeight::BOLD)
                            .child(ui_text(preview_title).normal_line_height()),
                    )
                    .when(state.first_event.is_some(), |title| {
                        title.child(div().text_size(px(9.)).text_color(rgb(0x818b98)).child(
                            ui_text(format!("{} 条内容", state.total_events)).normal_line_height(),
                        ))
                    }),
            );

        let mut events = div()
            .id("session-loader-events")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&state.preview_scroll)
            .px(px(events_padding))
            .pt(px(16.))
            .pb(px(28.));
        if state.has_earlier || state.loading_earlier {
            let pill = div()
                .id("session-loader-earlier")
                .h(px(22.))
                .px(px(9.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(999.))
                .border_1()
                .border_color(rgba(0x5b9de926))
                .bg(rgba(0x181b20eb))
                .text_size(px(9.))
                .text_color(rgb(0x8490a0))
                .child(ui_text(SESSION_LOADER_LOADING_EARLIER).normal_line_height());
            let pill = if state.loading_earlier {
                pill.opacity(0.6)
            } else {
                self.button(
                    pill.cursor_pointer().hover(|style| {
                        style
                            .border_color(rgb(0x3c4653))
                            .text_color(rgb(0xb5c0cd))
                            .bg(rgba(0xffffff0a))
                    }),
                    &self.dialog_controls[1],
                    |this, _, cx| this.load_earlier_session_events(cx),
                    cx,
                )
            };
            events = events.child(
                div()
                    .w_full()
                    .flex()
                    .justify_center()
                    .mb(px(10.))
                    .child(pill),
            );
        }
        let mono = if cfg!(target_os = "macos") {
            "SF Mono"
        } else {
            "Consolas"
        };
        for event in &state.events {
            let (border, background) = match event.kind.as_str() {
                "tool" => (0x4c3d5a, 0x241f2a),
                "user" => (0x343b46, 0x252b32),
                _ => (0x343b46, 0x20242a),
            };
            let label = loader_event_label(&event.kind, event.tool_name.as_deref());
            let is_user = event.kind == "user";
            events = events.child(
                div()
                    .w_full()
                    .flex()
                    .mb(px(12.))
                    .when(is_user, |row| row.justify_end())
                    .when(!is_user, |row| row.justify_center())
                    .child(
                        div()
                            .w_full()
                            .max_w(px(760.))
                            .border_1()
                            .border_color(rgb(border))
                            .rounded(px(11.))
                            .bg(rgb(background))
                            .px(px(13.))
                            .py(px(11.))
                            .child(
                                div()
                                    .w_full()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .gap(px(8.))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .text_size(px(9.))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(0xb9c1cc))
                                            .child(ui_text(label.to_owned()).normal_line_height()),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_size(px(9.))
                                            .text_color(rgb(0x8e98a5))
                                            .child(format!("#{}", event.index)),
                                    ),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .min_w_0()
                                    .mt(px(7.))
                                    .font_family(mono)
                                    .text_size(px(10.))
                                    .line_height(px(16.))
                                    .text_color(rgb(0xd9dde3))
                                    .whitespace_normal()
                                    .child(ui_text(event.text.clone())),
                            ),
                    ),
            );
        }
        if let Some(message) = if state.loading_detail {
            Some(SESSION_LOADER_LOADING_PREVIEW)
        } else if state.history && state.error.is_none() && state.events.is_empty() {
            Some("当前会话还没有可显示的历史内容")
        } else {
            (state.first_event.is_some() && state.events.is_empty())
                .then_some(SESSION_LOADER_NO_HISTORY)
        } {
            events = events.child(
                div()
                    .min_h(px(120.))
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .p(px(18.))
                    .text_center()
                    .text_size(px(11.))
                    .text_color(rgb(0x77818e))
                    .child(ui_text(message)),
            );
        }

        let preview = div()
            .id("session-loader-preview")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(rgb(0x181b20))
            .child(preview_header)
            .child(events);

        let body = div()
            .flex_1()
            .min_h_0()
            .w_full()
            .min_w_0()
            .flex()
            .children((!state.history).then_some(list))
            .child(preview);

        let primary_label = if self.pending_loader_loads.contains_key(&state.session_id) {
            SESSION_LOADER_LOADING.to_owned()
        } else {
            format!("载入到“{target_title}”")
        };
        let primary = div()
            .id("session-loader-submit")
            .h(px(32.))
            .max_w(px(480.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_ellipsis()
            .px(px(14.))
            .border_1()
            .border_color(rgb(0x4a8fdc))
            .rounded(px(8.))
            .bg(rgb(0x347bc8))
            .text_color(rgb(0xffffff))
            .text_size(px(12.))
            .child(ui_text(primary_label).normal_line_height());
        let primary = if can_submit {
            self.button(
                primary.cursor_pointer(),
                &self.dialog_controls[2],
                |this, _, cx| this.submit_session_loader(cx),
                cx,
            )
        } else {
            primary
                .opacity(0.48)
                .cursor(CursorStyle::OperationNotAllowed)
        };

        let footer = div()
            .h(px(58.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(18.))
            .border_t_1()
            .border_color(rgb(0x343a44))
            .bg(rgb(0x20242a))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .children(state.error.as_ref().map(|error| {
                        div()
                            .id("session-loader-error")
                            .w_full()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(10.))
                            .text_color(rgb(0xff8276))
                            .child(ui_text(error.clone()).normal_line_height())
                    }))
                    .when(running, |left| {
                        left.child(
                            div()
                                .w_full()
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_size(px(10.))
                                .text_color(rgb(0xe5af52))
                                .child(
                                    ui_text(SESSION_LOADER_RUNNING_WARNING).normal_line_height(),
                                ),
                        )
                    }),
            )
            .children((!state.history).then_some(primary))
            .children(state.history.then(|| {
                let mut actions = div().flex().items_center().gap(px(10.));
                if state.error.is_some() && !state.loading_detail {
                    actions = actions.child(
                        self.button(
                            div()
                                .id("session-history-retry")
                                .px(px(8.))
                                .py(px(5.))
                                .cursor_pointer()
                                .child("重试读取会话历史"),
                            &self.dialog_controls[2],
                            |this, _, cx| {
                                if let Some(TabDialog::SessionLoader(state)) = &mut this.dialog {
                                    state.error = None;
                                }
                                this.request_loader_detail(None);
                                cx.notify();
                            },
                            cx,
                        ),
                    );
                }
                actions.child(
                    self.button(
                        div()
                            .id("session-history-close")
                            .px(px(8.))
                            .py(px(5.))
                            .cursor_pointer()
                            .child("关闭会话历史"),
                        &self.dialog_controls[3],
                        |this, window, cx| this.close_dialog(window, cx),
                        cx,
                    ),
                )
            }));

        let header = div()
            .h(px(58.))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(10.))
            .px(px(18.))
            .border_b_1()
            .border_color(rgb(0x343a44))
            .bg(rgb(0x20242a))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(15.))
                            .font_weight(FontWeight::BOLD)
                            .child(
                                ui_text(if state.history {
                                    "会话历史"
                                } else {
                                    SESSION_LOADER_TITLE
                                })
                                .normal_line_height(),
                            ),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(10.))
                            .text_color(rgb(0x8d96a3))
                            .child(
                                ui_text(if state.history {
                                    target_title
                                } else {
                                    state.cwd.clone()
                                })
                                .normal_line_height(),
                            ),
                    ),
            )
            .child(
                self.button(
                    div()
                        .id("session-loader-close")
                        .size(px(28.))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(7.))
                        .text_size(px(20.))
                        .line_height(relative(1.))
                        .text_color(rgb(0x9fa7b3))
                        .cursor_pointer()
                        .hover(|style| style.text_color(rgb(0xffffff)).bg(rgba(0xffffff12)))
                        .child("×"),
                    &self.dialog_controls[0],
                    |this, window, cx| this.close_dialog(window, cx),
                    cx,
                ),
            );

        let panel = div()
            .id("session-loader-dialog")
            .w(px(panel_width))
            .h(px(panel_height))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(16.))
            .border_1()
            .border_color(rgb(0x3b424d))
            .bg(rgb(0x1a1d22))
            .text_color(rgb(0xe8ebf0))
            .shadow(vec![shadow(0x000000, 0.48, 28., 90.)])
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(header)
            .child(body)
            .child(footer);

        div()
            .id("session-loader-backdrop")
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .p(px(backdrop_pad))
            .bg(rgba(0x0f12177a))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.close_dialog(window, cx);
                    cx.stop_propagation();
                }),
            )
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .child(panel)
            .into_any_element()
    }

    fn render_fork_header(
        &self,
        id: &str,
        placement: ForkPlacement,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        let access = self.fork_access(id, placement)?;
        let sibling = placement == ForkPlacement::Sibling;
        let (role, title) = if sibling {
            ("sibling", "从共同父会话 Fork 兄弟分支")
        } else {
            ("child", "创建子分支")
        };
        let tooltip = access.blocked.or(access.readiness).unwrap_or(title);
        let button = fork_header_button(
            format!("fork-{role}-{id}").into(),
            sibling,
            access.blocked.or(access.readiness).is_some(),
            access.blocked.is_some(),
            tooltip.into(),
        );
        Some(if access.blocked.is_some() {
            button.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        } else {
            let id = id.to_owned();
            self.button(
                button,
                &self.fork_controls[&id][usize::from(sibling)],
                {
                    let id = id.clone();
                    move |this, window, cx| this.open_fork(&id, placement, window, cx)
                },
                cx,
            )
        })
    }

    fn render_fork_failure(&self, id: &str, node: &Value, cx: &mut Context<Self>) -> Option<Div> {
        let (title, reason) = fork_failure_copy(node)?;
        let retry = self.fork_retries.get(id);
        let pending = matches!(retry, Some(ForkRetry::Pending));
        let error = match retry {
            Some(ForkRetry::Rejected(error)) => Some(error.as_str()),
            _ => None,
        };
        let blocked = self.fork_repair_blocked_reason(id, node);
        let action = |remove: bool, cx: &mut Context<Self>| {
            let (role, label) = if remove {
                ("remove", "移除节点…")
            } else {
                ("retry", "重试创建分支")
            };
            let retry_pending = pending && !remove;
            let button = fork_failure_button(
                format!("fork-{role}-{id}").into(),
                label,
                blocked,
                retry_pending,
            );
            if blocked.is_some() || retry_pending {
                return button;
            }
            let session_id = id.to_owned();
            self.button(
                button,
                &self.fork_controls[id][2 + usize::from(remove)],
                move |this, window, cx| {
                    if remove {
                        this.open_remove_session(&session_id, window, cx);
                    } else {
                        this.retry_fork(&session_id, cx);
                    }
                },
                cx,
            )
        };
        Some(fork_failure_body(
            title,
            reason,
            error,
            action(false, cx).into_any_element(),
            action(true, cx).into_any_element(),
        ))
    }

    fn render_branch_dialog(
        &self,
        state: &BranchDialog,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (title, description, hint) = state.copy();
        let input = state.input.read(cx);
        let focused = input.focus.is_focused(window);
        let count = trim_name(&input.text).chars().count();
        let input = div()
            .w_full()
            .h(px(38.))
            .flex()
            .items_center()
            .pl(px(11.))
            .pr(px(52.))
            .rounded(px(9.))
            .border_1()
            .border_color(rgb(if focused { 0x5b9de9 } else { 0x414752 }))
            .when(focused, |input| {
                input.shadow(vec![BoxShadow {
                    color: rgba(0x5b9de929).into(),
                    offset: point(px(0.), px(0.)),
                    blur_radius: px(0.),
                    spread_radius: px(3.),
                }])
            })
            .bg(rgb(0x15171a))
            .when_else(
                state.submitting,
                |field| {
                    field.child(
                        div()
                            .truncate()
                            .text_color(rgb(0xf2f3f5))
                            .child(input.text.clone()),
                    )
                },
                |field| field.child(state.input.clone()),
            );
        let close = fork_dialog_close();
        let cancel = fork_dialog_button("fork-dialog-cancel", "取消", false, state.submitting);
        let confirm = fork_dialog_button("fork-dialog-confirm", "创建分支", true, state.submitting);
        let (close, cancel, confirm) = if state.submitting {
            (close.opacity(0.55).cursor_default(), cancel, confirm)
        } else {
            (
                self.button(close, &self.dialog_controls[0], Self::cancel_fork, cx),
                self.button(cancel, &self.dialog_controls[1], Self::cancel_fork, cx),
                self.button(confirm, &self.dialog_controls[2], Self::submit_fork, cx),
            )
        };
        let panel = fork_dialog_panel(
            window.viewport_size().width,
            title,
            &description,
            hint,
            count,
            state.submitting,
            state.error.as_deref(),
            (
                close.into_any_element(),
                input.into_any_element(),
                cancel.into_any_element(),
                confirm.into_any_element(),
            ),
        );
        tabs_ui::dialog_overlay("fork-dialog-backdrop")
            .bg(rgba(0x00000070))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.cancel_fork(window, cx);
                    cx.stop_propagation();
                }),
            )
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .child(panel.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()))
            .into_any_element()
    }

    fn render_fork_menu(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (id, position, focus) = self.fork_menu.as_ref()?;
        let access = self.fork_access(id, ForkPlacement::Peer)?;
        if access.blocked.is_some() {
            return None;
        }
        let tooltip = access.readiness.unwrap_or("创建 Fork 会话");
        let item = tabs_ui::menu_item("fork-menu-peer", "⑂ Fork 会话")
            .when(access.readiness.is_some(), |item| item.opacity(0.38))
            .tooltip(move |_, cx| cx.new(|_| CardTooltip(tooltip.into())).into());
        let id = id.clone();
        let item = self.button(
            item,
            focus,
            move |this, window, cx| this.open_fork(&id, ForkPlacement::Peer, window, cx),
            cx,
        );
        let viewport = window.viewport_size();
        let position = point(
            position.x.min((viewport.width - px(132.)).max(px(0.))),
            position.y.min((viewport.height - px(36.)).max(px(0.))),
        );
        let menu = tabs_ui::context_menu("fork-card-menu")
            .w(px(132.))
            .left(position.x)
            .top(position.y)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(item);
        let dismiss =
            |this: &mut Self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>| {
                this.fork_menu = None;
                cx.stop_propagation();
                cx.notify();
            };
        Some(
            div()
                .id("fork-menu-overlay")
                .absolute()
                .inset_0()
                .occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(dismiss))
                .on_mouse_down(MouseButton::Right, cx.listener(dismiss))
                .child(menu)
                .into_any_element(),
        )
    }

    fn tab_overlay(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if let Some(TabDialog::Branch(state)) = &self.dialog {
            return Some(self.render_branch_dialog(state, window, cx));
        }
        if let Some(TabDialog::SessionLoader(state)) = &self.dialog {
            return Some(self.render_session_loader(state, window, cx));
        }
        if let Some(dialog) = &self.dialog {
            let (title, body, confirm, show_cancel, disabled): (SharedString, _, SharedString, _, _) = match dialog {
                TabDialog::SessionLoader(_) | TabDialog::Branch(_) => unreachable!("specialized dialog rendered above"),
                TabDialog::RenameTask { task_id, input, empty_submitted, failure } => {
                    let value = trim_name(&input.read(cx).text);
                    let error = if *empty_submitted && value.is_empty() { Some("工作台名称不能为空".into()) }
                        else { self.task_rename_error(task_id, value).or_else(|| failure.as_ref().filter(|(title, _)| title == value).map(|(_, message)| message.clone())) };
                    let mut body = tabs_ui::dialog_body().child(input.clone());
                    if let Some(error) = &error { body = body.child(tabs_ui::dialog_error(error.clone())); }
                    ("重命名".into(), body, "确定".into(), true, error.is_some() && !value.is_empty())
                }
                TabDialog::RemoveWorkspace { name, .. } => (
                    "移出工作空间".into(),
                    tabs_ui::dialog_body().child(ui_text(format!(
                        "移出 \"{name}\" 会关闭该空间下的事项和终端会话，本地文件保持原样。 是否继续？"
                    )).pre_line()),
                    "移出".into(),
                    true,
                    false,
                ),
                TabDialog::DeleteTask { title, .. } => (
                    "提示".into(), tabs_ui::dialog_body().child(ui_text(format!("删除 \"{title}\" 会丢失该事项下所有终端会话，但不会删除本地目录。 是否继续？")).pre_line()), "确定".into(), true, false,
                ),
                // Frozen permission confirmation reuses the native ConfirmDialog
                // shell verbatim. The generic footer below supplies close, cancel
                // and confirm, so only the copy is read here.
                TabDialog::PermissionMode(state) => {
                    let (title, body, confirm) = state.copy();
                    (
                        title.into(),
                        tabs_ui::dialog_body().child(ui_text(body).pre_line()),
                        confirm.into(),
                        true,
                        false,
                    )
                }
                TabDialog::Rename { scene_id, input } => {
                    let value = trim_name(&input.read(cx).text);
                    let error = self.rename_error(scene_id, value);
                    let mut body = tabs_ui::dialog_body().child(input.clone());
                    if let Some(error) = &error {
                        body = body.child(tabs_ui::dialog_error(error.clone()));
                    }
                    (
                        "重命名标签页".into(),
                        body,
                        "确定".into(),
                        true,
                        error.is_some() && !value.is_empty(),
                    )
                }
                TabDialog::Close {
                    scene_id,
                    graph,
                    is_last,
                } => {
                    let name = self
                        .scenes
                        .iter()
                        .find(|s| &s.id == scene_id)
                        .map(|s| self.scene_title(s))
                        .unwrap_or_else(|| "当前画布".into());
                    let nodes = graph["nodes"].as_array();
                    let count = nodes.map_or(0, |ns| {
                        ns.iter().filter(|n| n["archivedAt"].is_null()).count()
                    });
                    let running = nodes.map_or(0, |ns| {
                        ns.iter()
                            .filter(|n| {
                                matches!(n["workStatus"].as_str(), Some("running" | "starting"))
                            })
                            .count()
                    });
                    let needs = nodes.map_or(0, |ns| {
                        ns.iter()
                            .filter(|n| n["workStatus"] == "needs-input")
                            .count()
                    });
                    let mut affected = Vec::new();
                    if running > 0 {
                        affected.push(format!("{running} 个运行中会话"));
                    }
                    if needs > 0 {
                        affected.push(format!("{needs} 个待输入会话"));
                    }
                    let activity = if affected.is_empty() {
                        String::new()
                    } else {
                        format!("其中 {}将停止。", affected.join("和 "))
                    };
                    let text = if *is_last {
                        LAST_TAB_BODY.into()
                    } else {
                        format!(
                            "关闭后，“{name}”下的 {count} 个会话会全部从界面移除。{activity}项目文件和工作树保持原样。"
                        )
                    };
                    (
                        if *is_last { "提示" } else { "关闭画布" }.into(),
                        tabs_ui::dialog_body().child(ui_text(text).pre_line()),
                        if *is_last {
                            LAST_TAB_CONFIRM
                        } else {
                            "关闭画布"
                        }.into(),
                        !*is_last,
                        false,
                    )
                }
                TabDialog::CloseSession { body, .. } => (
                    STOP_SESSION.into(),
                    tabs_ui::dialog_body().child(ui_text(body.clone()).pre_line()),
                    STOP_SESSION.into(),
                    true,
                    false,
                ),
                TabDialog::LastSession { .. } => (
                    "提示".into(),
                    tabs_ui::dialog_body().child(ui_text(LAST_TAB_BODY).pre_line()),
                    LAST_TAB_CONFIRM.into(),
                    false,
                    false,
                ),
                TabDialog::RemoveSession { session_id, scope, .. } => {
                    let text = self.removal_dialog_text(session_id, *scope)?;
                    let body = div()
                        .px(px(24.))
                        .text_color(rgba(0x000000a6))
                        .text_size(px(13.))
                        .line_height(relative(1.5))
                        .flex()
                        .flex_col()
                        .when(text.files_unchanged.is_some(), |body| body.pt(px(4.)))
                        .child(div().mb(px(12.)).child(ui_text(text.body)))
                        .children(text.files_unchanged.map(|text| div().mb(px(12.)).child(ui_text(text))))
                        .children(text.choices.map(|[node, branch]| {
                            div()
                                .id("remove-node-scope")
                                .mt(px(2.))
                                .mb(px(12.))
                                .flex()
                                .flex_col()
                                .gap(px(8.))
                                .child(self.remove_scope_choice(node, RemoveScope::NodeOnly, *scope, cx))
                                .child(self.remove_scope_choice(branch, RemoveScope::NodeAndDescendants, *scope, cx))
                        }))
                        .children(text.warning.map(|warning| div().mb(px(12.)).text_color(rgb(0xe7aa65)).child(ui_text(warning))));
                    (text.title.into(), body, text.confirm.into(), true, false)
                }
            };
            let n = self.dialog_controls.len();
            let mut footer = tabs_ui::dialog_footer();
            if show_cancel {
                footer = footer.child(self.button(
                    tabs_ui::dialog_button("dialog-cancel", "取消", false, false),
                    &self.dialog_controls[n - 2],
                    |this, window, cx| this.close_dialog(window, cx),
                    cx,
                ));
            }
            let confirm_button = tabs_ui::dialog_button("dialog-confirm", confirm, true, disabled);
            footer = footer.child(if disabled {
                confirm_button
            } else {
                self.button(
                    confirm_button,
                    &self.dialog_controls[n - 1],
                    |this, window, cx| this.confirm_dialog(window, cx),
                    cx,
                )
            });
            return Some(
                tabs_ui::dialog_overlay("tab-dialog-overlay")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.close_dialog(window, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                    .child(
                        tabs_ui::dialog_panel(f32::from(window.viewport_size().width))
                            .id("tab-dialog-panel")
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(
                                tabs_ui::dialog_header()
                                    .child(tabs_ui::dialog_title(title).when(
                                        matches!(dialog, TabDialog::RemoveSession { .. }),
                                        |title| title.min_w_0().whitespace_normal(),
                                    ))
                                    .child(self.button(
                                        tabs_ui::dialog_close("dialog-close"),
                                        &self.dialog_controls[0],
                                        |this, window, cx| this.close_dialog(window, cx),
                                        cx,
                                    )),
                            )
                            .child(body)
                            .child(footer),
                    )
                    .into_any_element(),
            );
        }
        if let Some((id, position, controls)) = &self.workspace_menu {
            let workspace = self.workspaces.iter().find(|w| &w.id == id)?;
            let invalid = self.workspace_invalid(id);
            let show_relink = invalid && !workspace.is_default;
            let show_remove = !workspace.is_default;
            let pinned = workspace.is_pinned;
            let root_directory = workspace.root_directory.clone();
            let name = workspace.name.clone();
            let count = 3 + usize::from(show_relink) + usize::from(show_remove);
            let height = 14. + count as f32 * 34.5 + if show_remove { 5. } else { 0. };
            let top = position.y - px(height / 2.);
            let text_item = |item_id: &'static str, label: &'static str| {
                div()
                    .id(item_id)
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(12.))
                    .py(px(8.))
                    .rounded(px(6.))
                    .text_size(px(13.))
                    .line_height(px(18.5))
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(0xeceef3)))
                    .child(ui_text(label).normal_line_height())
            };
            let relink_id = id.clone();
            let pin_id = id.clone();
            let finder_path = root_directory.clone();
            let copy_path = root_directory;
            let remove_id = id.clone();
            let mut next_focus = 0;
            let mut menu = tabs_ui::context_menu("workspace-context-menu")
                .w_auto()
                .min_w(px(120.))
                .p(px(6.))
                .rounded(px(10.))
                .border_1()
                .border_color(rgba(0x00000000))
                .left(position.x)
                .top(top)
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.workspace_menu = None;
                    cx.notify();
                }))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
            if show_relink {
                menu = menu.child(self.button(
                    text_item("workspace-menu-relink", "重新关联工作空间目录"),
                    &controls[next_focus],
                    move |this, _, cx| {
                        this.workspace_menu = None;
                        this.relink_workspace_directory(relink_id.clone(), cx);
                    },
                    cx,
                ));
                next_focus += 1;
            }
            menu = menu
                .child(
                    self.button(
                        div()
                            .id("workspace-menu-pin")
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .px(px(12.))
                            .py(px(8.))
                            .rounded(px(6.))
                            .text_size(px(13.))
                            .line_height(px(18.5))
                            .whitespace_nowrap()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(0xeceef3)))
                            .child(icon("pin", 13., 0x2f3547))
                            .child(
                                ui_text(if pinned { "取消置顶" } else { "置顶" })
                                    .normal_line_height(),
                            ),
                        &controls[next_focus],
                        move |this, _, cx| {
                            this.workspace_menu = None;
                            this.send(
                                Command::SetWorkspacePinned {
                                    workspace_id: pin_id.clone(),
                                    pinned: !pinned,
                                },
                                cx,
                            );
                        },
                        cx,
                    ),
                )
                .child(self.button(
                    text_item("workspace-menu-finder", "在 Finder 中显示"),
                    &controls[next_focus + 1],
                    move |this, _, cx| {
                        this.workspace_menu = None;
                        cx.open_with_system(&PathBuf::from(&finder_path));
                        cx.notify();
                    },
                    cx,
                ))
                .child(self.button(
                    text_item("workspace-menu-copy", "复制路径"),
                    &controls[next_focus + 2],
                    move |this, _, cx| {
                        this.workspace_menu = None;
                        cx.write_to_clipboard(ClipboardItem::new_string(copy_path.clone()));
                        this.show_toast("路径已复制", cx);
                    },
                    cx,
                ));
            if show_remove {
                menu = menu.child(
                    self.button(
                        div()
                            .id("workspace-menu-remove")
                            .mt(px(4.))
                            .border_t_1()
                            .border_color(rgb(0xeceef2))
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .px(px(12.))
                            .py(px(8.))
                            .rounded_b(px(6.))
                            .text_size(px(13.))
                            .line_height(px(18.5))
                            .text_color(rgb(0xc5443e))
                            .whitespace_nowrap()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(0xfff1f0)).text_color(rgb(0xa8322e)))
                            .child(icon("trash-2", 14., 0xc5443e))
                            .child(ui_text("移出码头").normal_line_height()),
                        &controls[next_focus + 3],
                        move |this, window, cx| {
                            this.open_workspace_remove(remove_id.clone(), name.clone(), window, cx);
                        },
                        cx,
                    ),
                );
            }
            return Some(menu.into_any_element());
        }
        if let Some((id, position, controls)) = &self.task_menu {
            let task = self.tasks.iter().find(|task| &task.id == id)?;
            let pinned = task.is_pinned;
            let pin_id = id.clone();
            let rename_id = id.clone();
            let delete_id = id.clone();
            let menu_item = |id: &'static str, label: &'static str, symbol: &'static str| {
                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(12.))
                    .py(px(8.))
                    .rounded(px(6.))
                    .text_size(px(13.))
                    .line_height(px(19.5))
                    .cursor_pointer()
                    .hover(|s| s.bg(rgb(0xeceef3)))
                    .child(icon(
                        symbol,
                        if symbol == "pin" { 13. } else { 14. },
                        if symbol == "trash-2" {
                            0xc5443e
                        } else {
                            0x2f3547
                        },
                    ))
                    .child(ui_text(label))
            };
            return Some(
                tabs_ui::context_menu("task-context-menu")
                    .w(px(120.))
                    .p(px(6.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(rgba(0x00000000))
                    .left(position.x)
                    .top(position.y - px(62.25))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.task_menu = None;
                        cx.notify();
                    }))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(self.button(
                        menu_item("task-pin", if pinned { "取消置顶" } else { "置顶" }, "pin"),
                        &controls[0],
                        move |this, _, cx| {
                            this.task_menu = None;
                            this.send(
                                Command::SetTaskPinned {
                                    task_id: pin_id.clone(),
                                    pinned: !pinned,
                                },
                                cx,
                            );
                        },
                        cx,
                    ))
                    .child(self.button(
                        menu_item("task-rename", "重命名", "pencil"),
                        &controls[1],
                        move |this, window, cx| {
                            this.open_task_rename(rename_id.clone(), window, cx)
                        },
                        cx,
                    ))
                    .child(
                        self.button(
                            menu_item("task-delete", "删除", "trash-2")
                                .mt(px(4.))
                                .border_t_1()
                                .border_color(rgb(0xeceef2))
                                .text_color(rgb(0xc5443e)),
                            &controls[2],
                            move |this, window, cx| {
                                this.open_task_delete(delete_id.clone(), window, cx)
                            },
                            cx,
                        ),
                    )
                    .into_any_element(),
            );
        }
        if let Some((id, position, focus)) = &self.tab_menu {
            let id = id.clone();
            return Some(
                tabs_ui::context_menu("tab-context-menu")
                    .left(position.x)
                    .top(position.y)
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.tab_menu = None;
                        cx.notify();
                    }))
                    .child(self.button(
                        tabs_ui::menu_item("tab-rename-menu", "重命名页签"),
                        focus,
                        move |this, window, cx| this.open_rename(id.clone(), window, cx),
                        cx,
                    ))
                    .into_any_element(),
            );
        }
        if let Some(ids) = &self.hidden_tabs {
            let bounds = self.tab_scroll.bounds();
            let size = window.viewport_size();
            let top = bounds.bottom() + px(4.);
            // CSS shrink-to-fit width needs an explicit value here so GPUI's
            // percentage-width rows receive a definite containing block.
            let font = window.text_style().font();
            let menu_width = ids
                .iter()
                .filter_map(|(id, _)| self.scenes.iter().find(|s| &s.id == id))
                .map(|scene| {
                    let text: SharedString = self.scene_title(scene).into();
                    let run = TextRun {
                        len: text.len(),
                        font: font.clone(),
                        color: rgb(0x2f3547).into(),
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    let line = window
                        .text_system()
                        .shape_line(text.clone(), px(13.), &[run], None);
                    f32::from(line.x_for_index(text.len()))
                        + 24.
                        + if self
                            .hierarchy
                            .as_ref()
                            .and_then(|h| h.scene.as_ref())
                            .is_some_and(|s| s.id == scene.id)
                        {
                            24.
                        } else {
                            0.
                        }
                })
                .fold(200f32, f32::max)
                .min(360.);
            let mut panel = tabs_ui::overflow_panel("hidden-tab-panel")
                .w(px(menu_width))
                .right(size.width - bounds.right())
                .top(top)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
            for (id, focus) in ids {
                let Some(scene) = self.scenes.iter().find(|s| &s.id == id) else {
                    continue;
                };
                let active = self
                    .hierarchy
                    .as_ref()
                    .and_then(|h| h.scene.as_ref())
                    .is_some_and(|s| &s.id == id);
                let id = id.clone();
                panel = panel.child(
                    self.button(
                        tabs_ui::overflow_item(SharedString::from(format!("hidden-{id}")), active)
                            .child(tabs_ui::overflow_title(self.scene_title(scene), active))
                            .when(active, |item| item.child(tabs_ui::overflow_check())),
                        focus,
                        move |this, _, cx| {
                            this.hidden_tabs = None;
                            if let Some(index) = this.scenes.iter().position(|s| s.id == id) {
                                this.tab_scroll.scroll_to_item(index);
                            }
                            this.send(
                                Command::SelectScene {
                                    scene_id: id.clone(),
                                },
                                cx,
                            );
                        },
                        cx,
                    ),
                );
            }
            return Some(
                div()
                    .id("hidden-tab-overlay")
                    .occlude()
                    .absolute()
                    .inset_0()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.workspace_menu = None;
                            this.task_menu = None;
                            this.hidden_tabs = None;
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                    .child(panel)
                    .into_any_element(),
            );
        }
        None
    }

    fn child_badge_button(
        &self,
        id: &str,
        badge: session_levels::ChildBadge,
        badge_bounds: Entity<Option<Bounds<Pixels>>>,
        focus: &FocusHandle,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let click_id = id.to_owned();
        let hover_id = id.to_owned();
        let (text_color, border_color, background) = if badge.is_idle {
            (0x777f89, 0xd7dbe1, 0xf3f4f6)
        } else {
            (0x2666b1, 0xb9d6ff, 0xe8f2ff)
        };
        self.focus_control(
            div()
                .id(SharedString::from(format!("child-session-badge-{id}")))
                .relative()
                .h(px(24.))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(5.))
                .max_w(relative(1.))
                .px(px(8.))
                .border_1()
                .border_color(rgb(border_color))
                .rounded(px(999.))
                .bg(rgb(background))
                .text_color(rgb(text_color))
                .text_size(px(10.))
                .font_weight(FontWeight(650.))
                .whitespace_nowrap()
                .cursor_pointer()
                .on_hover(cx.listener(move |this, hovered, _, cx| {
                    if *hovered {
                        this.hovered_child_badge = Some(hover_id.clone());
                    } else if this.hovered_child_badge.as_deref() == Some(hover_id.as_str()) {
                        this.hovered_child_badge = None;
                    }
                    cx.notify();
                }))
                .on_mouse_down(MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.open_child_level(&click_id, window, cx);
                }))
                .child(icon("layers", 14., text_color))
                .child(
                    div()
                        .size(px(6.))
                        .flex_none()
                        .rounded_full()
                        .bg(rgb(badge.dot_color)),
                )
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(badge.summary),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(13.))
                        .line_height(px(13.))
                        .child("›"),
                )
                .child(
                    canvas(
                        |_, _, _| (),
                        move |bounds, _, _, cx| {
                            badge_bounds.update(cx, |slot, _| *slot = Some(bounds));
                        },
                    )
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full(),
                ),
            focus,
        )
    }

    fn footer(&self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        struct ReturnParentTooltip;
        impl Render for ReturnParentTooltip {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
                    .px(px(9.))
                    .py(px(7.))
                    .border_1()
                    .border_color(rgb(0xd6dae1))
                    .rounded(px(7.))
                    .bg(rgb(0xffffff))
                    .text_color(rgb(0x333b47))
                    .text_size(px(10.))
                    .line_height(px(15.))
                    .whitespace_nowrap()
                    .shadow(vec![shadow(0x1f2837, 0.16, 8., 24.)])
                    .child("返回父会话")
            }
        }

        let (label, count) = self.breadcrumb_labels();
        let content = div()
            .min_w_0()
            .flex_1()
            .flex()
            .items_center()
            .child(
                div()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(label),
            )
            .child(
                div()
                    .flex_none()
                    .ml(px(4.))
                    .text_color(rgba(0x00000061))
                    .child(count),
            );
        let breadcrumb = div()
            .h_full()
            .min_w_0()
            .max_w(px(
                (f32::from(window.viewport_size().width) * 0.44).min(520.)
            ))
            .flex()
            .items_center()
            .text_size(px(11.))
            .text_color(rgba(0x000000ad));
        let breadcrumb = if self.level_parent().is_some() {
            breadcrumb.child(
                self.button(
                    div()
                        .id("session-return-parent")
                        .h(px(26.))
                        .min_w_0()
                        .flex()
                        .items_center()
                        .px(px(8.))
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(|style| style.bg(rgba(0x00000014)).text_color(rgb(0x2f3547)))
                        .child(div().flex_none().mr(px(6.)).text_size(px(13.)).child("←"))
                        .child(content),
                    &self.control_focus["session-return-parent"],
                    |this, window, cx| this.return_to_parent(window, cx),
                    cx,
                )
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if matches!(event.keystroke.key.as_str(), "left" | "up") {
                        cx.stop_propagation();
                        this.return_to_parent(window, cx);
                    }
                }))
                .tooltip(|_, cx| cx.new(|_| ReturnParentTooltip).into()),
            )
        } else {
            breadcrumb.child(
                div()
                    .h(px(26.))
                    .min_w_0()
                    .flex()
                    .items_center()
                    .px(px(8.))
                    .child(content),
            )
        };
        let mut footer = div()
            .h(px(PERMISSION_FOOTER_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .px(px(PERMISSION_FOOTER_PADDING_X))
            .gap(px(8.))
            .border_t_1()
            .border_color(rgb(0xe8e8e8))
            .bg(rgb(0xffffff))
            .text_size(px(10.))
            .text_color(rgb(0x606572))
            .child(breadcrumb)
            .child(div().flex_1());
        if let Some(chip) = self.permission_chip() {
            let menu_open = self.permission_menu.is_some() && !chip.disabled;
            // The dismiss layer is mounted before the status area so the chip
            // and the anchored menu paint (and receive input) above it.
            if menu_open {
                footer = footer.child(self.permission_menu_overlay(window, cx));
            }
            footer = footer.child(self.permission_status(&chip, menu_open, window, cx));
        }
        footer
    }

    /// The right-hand footer cell reserved for the permission control. The
    /// 180px width is the frozen menu's minimum width so an open menu can sit
    /// flush above the chip without overlapping the breadcrumb.
    fn permission_status(
        &self,
        chip: &PermissionChip,
        menu_open: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        div()
            .id("permission-status")
            .w(px(PERMISSION_STATUS_WIDTH))
            .h_full()
            .flex_none()
            .flex()
            .items_center()
            .child(self.permission_chip_view(chip, cx))
            .when(menu_open, |status| {
                status.child(self.permission_menu_view(chip.mode, window, cx))
            })
    }

    /// `.status-perm-badge` + `.is-clickable` + `.status-field:disabled`.
    /// The label is uppercased and tracked here because GPUI has no
    /// `text-transform` / `letter-spacing`; the tooltip keeps the whole title.
    fn permission_chip_view(&self, chip: &PermissionChip, cx: &mut Context<Self>) -> Stateful<Div> {
        let tooltip: SharedString = chip.title.clone().into();
        let cell = div()
            .id("permission-chip")
            .h(px(PERMISSION_CHIP_HEIGHT))
            .flex()
            .items_center()
            .px(px(6.))
            .py(px(1.))
            .rounded(px(4.))
            .text_size(px(10.))
            .line_height(px(PERMISSION_CHIP_LINE_HEIGHT))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(permission_mode_color(chip.mode)))
            .whitespace_nowrap()
            .hover(|style| {
                style.bg(Rgba {
                    a: 0.08,
                    ..rgb(0xffffff)
                })
            })
            .tooltip(move |_, cx| cx.new(|_| CardTooltip(tooltip.clone())).into())
            .child(permission_tracked_text(
                permission_chip_text(chip.mode),
                PERMISSION_CHIP_TRACKING,
            ));
        if chip.disabled {
            cell.opacity(PERMISSION_CHIP_DISABLED_OPACITY)
                .cursor(CursorStyle::Arrow)
        } else {
            self.button(
                cell.cursor_pointer(),
                &self.control_focus["permission-mode"],
                |this, window, cx| this.toggle_permission_menu(window, cx),
                cx,
            )
        }
    }

    /// Full-window dismiss layer shown only while the menu is open. It is a
    /// sibling mounted before the status cell, so the chip and the deferred
    /// menu stay hittable above it.
    fn permission_menu_overlay(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let viewport = window.viewport_size();
        anchored()
            .anchor(Corner::TopLeft)
            .position(point(px(0.), px(0.)))
            .child(
                div()
                    .id("permission-menu-overlay")
                    .w(viewport.width)
                    .h(viewport.height)
                    .occlude()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.dismiss_permission_menu(window, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(|this, _, window, cx| {
                            this.dismiss_permission_menu(window, cx);
                            cx.stop_propagation();
                        }),
                    ),
            )
    }

    /// `.perm-menu` in a deferred anchored layer. The window-space anchor is
    /// the chip's top-left minus the frozen 8px gap; `BottomLeft` grows the
    /// menu upward and left-aligns it with the chip.
    fn permission_menu_view(
        &self,
        active: PermissionMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let focuses = window
            .use_keyed_state(
                SharedString::from("permission-menu-item-focus"),
                cx,
                |_, cx| {
                    (0..PermissionMode::ALL.len())
                        .map(|_| cx.focus_handle().tab_stop(true))
                        .collect::<Vec<_>>()
                },
            )
            .read(cx)
            .clone();
        let items = PermissionMode::ALL
            .into_iter()
            .enumerate()
            .map(|(index, mode)| {
                let item = permission_menu_item(mode, active);
                self.button(
                    item,
                    &focuses[index],
                    move |this, window, cx| this.choose_permission_mode(mode, window, cx),
                    cx,
                )
            });
        let panel = div()
            .id("permission-menu")
            .flex()
            .flex_col()
            .min_w(px(PERMISSION_MENU_MIN_WIDTH))
            .p(px(PERMISSION_MENU_PADDING))
            .border_1()
            .border_color(rgb(0x30363d))
            .rounded(px(8.))
            .occlude()
            .bg(rgb(0x1c2128))
            .text_color(rgb(0xc9d1d9))
            .text_size(px(12.))
            .shadow(vec![shadow(0x000000, 0.5, 8., 24.)])
            .child(permission_menu_title())
            .children(items);
        deferred(
            anchored()
                .anchor(Corner::BottomLeft)
                .position(permission_menu_anchor(window.viewport_size()))
                .snap_to_window()
                .child(panel),
        )
        .with_priority(2)
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        const CARD_RADIUS: Pixels = px(12.);
        let level_focus = self.level_focus().map(str::to_owned);
        let active_level_focus = self.active_level_focus().map(str::to_owned);
        let viewport_width = f64::from(f32::from(window.viewport_size().width)) - 260.;
        let content_width = viewport_width - 20.;
        let columns = (((viewport_width + 12.) / 292.).floor()).clamp(1., 4.);
        let narrow = viewport_width < 720.;
        let inactive_width = if narrow {
            (content_width * 0.28).clamp(112., 176.)
        } else {
            (content_width - (columns - 1.) * 12.) / columns
        };
        let active_width = if narrow {
            (content_width * 0.68).clamp(260., 560.)
        } else {
            (content_width * 0.48).min(620.)
        };
        let now = self.motion_clock.elapsed().as_secs_f64() * 1000.;
        let dragging = cx.has_active_drag();
        if !dragging {
            self.workspace_drag_source = None;
            self.workspace_drag_target = None;
        }
        for workspace in &self.workspaces {
            let motion = self
                .workspace_drag_motion
                .entry(workspace.id.clone())
                .or_insert_with(|| WorkspaceDragMotion {
                    scale: Transition::new(1.),
                    opacity: Transition::new(1.),
                    background: Transition::new(0.),
                });
            motion.scale.retarget(
                if dragging && self.workspace_drag_source.as_ref() == Some(&workspace.id) {
                    0.99
                } else {
                    1.
                },
                now,
                140.,
                Curve::Ease,
            );
            motion.opacity.retarget(
                if dragging && self.workspace_drag_source.as_ref() == Some(&workspace.id) {
                    0.6
                } else {
                    1.
                },
                now,
                140.,
                Curve::Ease,
            );
            motion.background.retarget(
                if dragging && self.workspace_drag_target.as_ref() == Some(&workspace.id) {
                    1.
                } else {
                    0.
                },
                now,
                140.,
                Curve::Ease,
            );
            if motion.scale.is_running(now)
                || motion.opacity.is_running(now)
                || motion.background.is_running(now)
            {
                window.request_animation_frame();
            }
        }

        let capacity = columns as u32;
        if self.column_capacity != Some(capacity) {
            self.column_capacity = Some(capacity);
            self.restore_geometry = true;
        }
        let mut widths = HashMap::new();
        let mut positions = HashMap::new();
        let mut left = 10.;
        self.card_geometry.clear();
        let mut active_geometry = None;
        let mut preview_geometry = None;
        let mut moving = false;
        let width_duration = if reduced_motion() { 1. } else { 380. };
        for id in &self.order {
            let expanded =
                level_focus.as_ref() == Some(id) || (!narrow && self.hovered.as_ref() == Some(id));
            let motion = self.card_motion.get_mut(id).expect("mounted card motion");
            motion.width.retarget(
                if expanded { 1. } else { 0. },
                now,
                width_duration,
                Curve::Width,
            );
            let width = inactive_width + (active_width - inactive_width) * motion.width.value(now);
            widths.insert(id.clone(), width);
            positions.insert(id.clone(), left);
            self.card_geometry.push((id.clone(), left, width));
            if level_focus.as_ref() == Some(id) {
                active_geometry = Some((left, width));
            }
            if self.hovered.as_ref() == Some(id) {
                preview_geometry = Some((left, width));
            }
            left += width + 12.;
            moving |= motion.width.is_running(now) || motion.lift.is_running(now);
        }
        let total =
            20. + widths.values().sum::<f64>() + self.order.len().saturating_sub(1) as f64 * 12.;
        let max_scroll = (total - viewport_width).max(0.);
        if let Some(deadline) = self.edge_next_at {
            if now >= deadline {
                let target = self
                    .edge_target
                    .take()
                    .filter(|id| self.order.contains(id))
                    .or_else(|| self.nearest_hidden_card(self.edge_direction, viewport_width));
                if let Some(id) = target {
                    // Dwelling at an edge is navigation intent: unlike ordinary
                    // preview, this new position remains after pointer exit.
                    self.hover_baseline = None;
                    self.hover_restore_at = None;
                    self.focus_follow_until = None;
                    self.hovered = Some(id);
                    self.hover_follow_until = Some(now + 440.);
                    if !self.edge_cruising {
                        self.edge_phase_start = now;
                    }
                    self.edge_cruising = true;
                    self.edge_next_at = Some(now + 900.);
                    cx.notify();
                } else {
                    self.stop_edge_browse(false);
                }
            }
            moving = true;
        }
        if self.restore_geometry {
            self.restore_geometry = moving;
            if let Some(saved) = self
                .carousel_owner
                .as_ref()
                .and_then(|owner| self.geometry.get(owner))
            {
                let value = &saved.geometry;
                let anchor = value["anchorSessionId"]
                    .as_str()
                    .and_then(|id| positions.get(id));
                let offset = value["anchorViewportOffset"].as_f64();
                let scroll = match (anchor, offset) {
                    (Some(left), Some(offset)) => left - offset,
                    _ => value["scrollLeft"].as_f64().unwrap_or(0.),
                };
                self.carousel_scroll
                    .set_offset(point(px(-scroll.clamp(0., max_scroll) as f32), px(0.)));
                if scroll > 0. || anchor.is_some() {
                    self.focus_follow_until = None;
                }
            }
        }
        if let (Some(until), Some((left, width))) = (self.hover_follow_until, preview_geometry) {
            let current =
                f64::from(-f32::from(self.carousel_scroll.offset().x)).clamp(0., max_scroll);
            let target = fully_visible_scroll(left, width, current, viewport_width, max_scroll);
            let next = focus_scroll_step(current, target);
            self.carousel_scroll
                .set_offset(point(px(-next as f32), px(0.)));
            if now < until || (target - next).abs() > 0.5 {
                moving = true;
            } else {
                self.hover_follow_until = None;
            }
        }
        if let Some(at) = self.hover_restore_at {
            if now >= at || !moving {
                if let Some(baseline) = self.hover_baseline.take() {
                    self.carousel_scroll
                        .set_offset(point(px(-baseline as f32), px(0.)));
                }
                self.hover_restore_at = None;
            } else {
                moving = true;
            }
        }
        if let (Some(until), Some((left, width))) = (self.focus_follow_until, active_geometry) {
            let target = centered_scroll(left, width, viewport_width, max_scroll);
            let current =
                f64::from(-f32::from(self.carousel_scroll.offset().x)).clamp(0., max_scroll);
            let next = focus_scroll_step(current, target);
            self.carousel_scroll
                .set_offset(point(px(-next as f32), px(0.)));
            if now < until || (target - next).abs() > 0.5 {
                moving = true;
            } else {
                self.focus_follow_until = None;
            }
        }
        if self.hover_baseline.is_none()
            && self.hover_follow_until.is_none()
            && self.focus_follow_until.is_none()
            && !self.restore_geometry
            && self.pending_focus.is_none()
            && active_level_focus.is_some()
        {
            let scroll =
                f64::from(-f32::from(self.carousel_scroll.offset().x)).clamp(0., max_scroll);
            self.save_geometry(
                scroll,
                active_geometry.map(|(left, _)| left),
                active_level_focus,
                false,
                cx,
            );
        }
        if moving {
            window.request_animation_frame();
        }
        let mut carousel = div()
            .id("session-carousel")
            .flex_1()
            .min_h_0()
            .flex()
            .gap(px(12.))
            .p(px(10.))
            .border_t_1()
            .border_color(rgb(0xe8e8e8))
            .overflow_x_scroll()
            .track_scroll(&self.carousel_scroll)
            .overflow_y_hidden()
            .bg(rgb(0xe9edf2))
            .capture_any_mouse_down(cx.listener(|this, _, _, cx| {
                this.stop_edge_browse(true);
                cx.notify();
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, window, cx| {
                this.edge_intent(event.position, window, cx);
            }))
            .on_hover(cx.listener(|this, hovered, window, cx| {
                if !hovered {
                    this.pointer_position = None;
                    this.hovered_child_badge = None;
                    this.stop_edge_browse(false);
                    this.hover_card(None, cx);
                } else {
                    this.edge_blocked = false;
                    this.edge_intent(window.mouse_position(), window, cx);
                }
            }));
        let path_valid = self.active_path_valid();
        #[cfg(target_os = "macos")]
        let recovery_water_available = self.recovery_water_available(window);
        let carousel_left =
            f64::from(-f32::from(self.carousel_scroll.offset().x)).clamp(0., max_scroll);
        #[cfg(target_os = "macos")]
        let mut mounted_water = HashSet::new();
        let visible_deferred: Vec<_> = self
            .order
            .iter()
            .filter(|&id| {
                self.is_deferred(id)
                    && (!narrow || level_focus.as_ref() == Some(id))
                    && positions[id] < carousel_left + viewport_width
                    && positions[id] + widths[id] > carousel_left
            })
            .cloned()
            .collect();
        for id in visible_deferred {
            self.request_deferred_preview(&id);
        }
        let mut badge_tooltip = None;
        for id in &self.order {
            let node = session_levels::node(session_levels::nodes(&self.graph), id);
            let selected = level_focus.as_ref() == Some(id);
            let width = widths[id] as f32;
            let lift = -4. * self.card_motion[id].lift.value(now) as f32;
            let hover_id = id.clone();
            let card = div()
                .id(SharedString::from(format!("card-{id}")))
                .w(px(width))
                .h_full()
                .flex_none()
                .relative()
                .top(px(lift))
                .rounded(CARD_RADIUS)
                .overflow_hidden()
                .bg(rgb(0xfcfcfd))
                .flex()
                .flex_col()
                .shadow(if selected {
                    vec![
                        shadow(0x32445c, 0.11, 7., 15.),
                        shadow(0x32445c, 0.19, 24., 52.),
                        shadow(0x3f82d4, 0.12, 15., 42.),
                    ]
                } else if self.hovered.as_ref() == Some(id) {
                    vec![
                        shadow(0x2a3748, 0.1, 4., 12.),
                        shadow(0x2a3748, 0.12, 17., 36.),
                    ]
                } else {
                    vec![
                        shadow(0x2a3748, 0.075, 2., 7.),
                        shadow(0x2a3748, 0.07, 13., 30.),
                    ]
                })
                .on_hover(cx.listener(move |this, hovered, _, cx| {
                    if *hovered {
                        this.hover_card(Some(hover_id.clone()), cx);
                    }
                }));
            let compact = narrow && !selected;
            let title = node
                .and_then(|node| node["title"].as_str())
                .unwrap_or("Shell");
            if let Some(node) = node.filter(|node| !node["archivedAt"].is_null()) {
                let card = if compact {
                    card.child(Self::compact_session_summary(Some(node), title))
                } else {
                    card.child(self.stopped_session_card(id, node, cx))
                };
                carousel = carousel.child(card);
                continue;
            }
            let Some(terminal) = self.terminals.get(id) else {
                continue;
            };
            // The badge measures its painted bounds so pointer-down can be
            // excluded from the card's capture-phase activation even before
            // the first hover event reaches the freshly rendered badge.
            let badge_bounds = if compact {
                None
            } else {
                Some(window.use_keyed_state(
                    SharedString::from(format!("child-session-badge-bounds-{id}")),
                    cx,
                    |_, _| None::<Bounds<Pixels>>,
                ))
            };
            let badge_bounds_for_capture = badge_bounds.clone();
            let badge = badge_bounds.as_ref().and_then(|badge_bounds| {
                let badge = self.child_badge(id)?;
                let focus = window.use_keyed_state(
                    SharedString::from(format!("child-session-badge-focus-{id}")),
                    cx,
                    |_, cx| cx.focus_handle().tab_stop(true),
                );
                let focus = focus.read(cx).clone();
                if self.dialog.is_none()
                    && let Some(bounds) = *badge_bounds.read(cx)
                    && ((self.hovered_child_badge.as_ref() == Some(id)
                        && bounds.contains(&window.mouse_position()))
                        || (self.keyboard_focus && focus.is_focused(window)))
                {
                    badge_tooltip = Some(
                        anchored()
                            .anchor(Corner::TopRight)
                            .position(bounds.bottom_right() + point(px(0.), px(7.)))
                            .snap_to_window()
                            .child(
                                div()
                                    .max_w((window.viewport_size().width - px(24.)).min(px(460.)))
                                    .px(px(9.))
                                    .py(px(7.))
                                    .border_1()
                                    .border_color(rgb(0xd6dae1))
                                    .rounded(px(7.))
                                    .bg(rgb(0xffffff))
                                    .text_color(rgb(0x333b47))
                                    .text_size(px(10.))
                                    .line_height(px(15.))
                                    .whitespace_nowrap()
                                    .shadow(vec![shadow(0x1f2837, 0.16, 8., 24.)])
                                    .child(badge.detail.clone()),
                            ),
                    );
                }
                Some(self.child_badge_button(id, badge, badge_bounds.clone(), &focus, cx))
            });
            let focus = terminal.read(cx).focus.clone();
            let scrollbar_terminal = terminal.clone();
            let scrollbar_session = id.clone();
            let scrollbar_scene = self
                .hierarchy
                .as_ref()
                .and_then(|h| h.scene.as_ref())
                .map(|s| s.id.clone());
            let session_id = id.clone();
            let scene_id = self
                .hierarchy
                .as_ref()
                .and_then(|h| h.scene.as_ref())
                .map(|s| s.id.clone());
            #[cfg(target_os = "macos")]
            let water = if recovery_water_available
                && (!narrow || selected)
                && positions[id] < carousel_left + viewport_width
                && positions[id] + widths[id] > carousel_left
                && node.is_some_and(|node| self.recovery_water_needed(id, node))
            {
                mounted_water.insert(id.clone());
                // Match the native terminal's current default workspace theme.
                let theme = Theme::default();
                let entity = self.recovery_water.entry(id.clone()).or_insert_with(|| {
                    cx.new(|cx| {
                        RecoveryWater::new(self.runtime_generation, theme, reduced_motion(), cx)
                            .with_bottom_radius(CARD_RADIUS)
                    })
                });
                Some(RecoveryWater::overlay(entity.clone()))
            } else {
                None
            };
            let card = card
                .capture_any_mouse_down(cx.listener(
                    move |this, event: &MouseDownEvent, window, cx| {
                        if this.is_stopped_session(&scrollbar_session) {
                            return;
                        }
                        // Compact cards mount no badge bounds, so a stale hit
                        // area can never swallow their ordinary activation.
                        if let Some(badge_bounds) = &badge_bounds_for_capture
                            && badge_bounds
                                .read(cx)
                                .as_ref()
                                .is_some_and(|bounds| bounds.contains(&event.position))
                        {
                            // The badge handles its own pointer-down and stops
                            // propagation in the bubble phase.
                            return;
                        }
                        #[cfg(target_os = "macos")]
                        if this.recovery_water.contains_key(&scrollbar_session)
                            && let Some(index) =
                                this.order.iter().position(|id| id == &scrollbar_session)
                            && let Some(bounds) = this.carousel_scroll.bounds_for_item(index)
                            && event.position.y
                                >= bounds.top()
                                    + this.carousel_scroll.offset().y
                                    + RecoveryWater::TOP
                        {
                            // The terminal and the card's normal click handler are both below the water.
                            cx.stop_propagation();
                            return;
                        }
                        // The original card activates in pointer capture, before
                        // the scrollbar consumes its drag and preserves focus.
                        if scrollbar_terminal.read(cx).scrollbar_hit(event.position)
                            && this.geometry_focus() != Some(scrollbar_session.as_str())
                        {
                            this.hover_baseline = None;
                            this.hover_restore_at = None;
                            this.hover_follow_until = None;
                            this.capture_geometry(
                                Some(scrollbar_session.clone()),
                                false,
                                window,
                                cx,
                            );
                            if let Some(scene_id) = &scrollbar_scene {
                                this.send(
                                    Command::FocusSession {
                                        scene_id: scene_id.clone(),
                                        session_id: scrollbar_session.clone(),
                                    },
                                    cx,
                                );
                            }
                        }
                    },
                ))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        if this.is_stopped_session(&session_id) {
                            return;
                        }
                        if this.active_path_valid() && !this.recovery_blocked(&session_id) {
                            window.focus(&focus);
                        }
                        this.hover_baseline = None;
                        this.hover_restore_at = None;
                        this.hover_follow_until = None;
                        this.capture_geometry(Some(session_id.clone()), false, window, cx);
                        if this.geometry_focus() != Some(session_id.as_str()) {
                            if let Some(scene_id) = &scene_id {
                                this.send(
                                    Command::FocusSession {
                                        scene_id: scene_id.clone(),
                                        session_id: session_id.clone(),
                                    },
                                    cx,
                                );
                            }
                        }
                    }),
                );
            let fork_failure = node
                .filter(|_| !compact)
                .and_then(|node| self.render_fork_failure(id, node, cx));
            let fork_progress = node.and_then(fork_progress_copy);
            let provider_banner = node
                .filter(|_| !compact && fork_failure.is_none() && fork_progress.is_none())
                .and_then(|node| self.provider_restore_banner(id, node));
            let card = if !compact
                && fork_failure.is_none()
                && fork_progress.is_none()
                && provider_banner.is_none()
                && let Some(RecoveryState::Failed(error)) = self.recovery.get(id)
            {
                let retry_id = id.clone();
                card.child(
                    div()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .p(px(24.))
                        .rounded(px(12.))
                        .bg(rgb(0xfff7f6))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .w(px((width - 72.).min(360.).max(0.)))
                                .flex()
                                .flex_col()
                                .items_center()
                                .gap(px(9.))
                                .p(px(22.))
                                .border_1()
                                .border_color(rgb(0xc8d4e3))
                                .rounded(px(12.))
                                .bg(rgb(0xffffff))
                                .shadow(vec![shadow(0x1f2d3d, 0.14, 16., 44.)])
                                .text_center()
                                .text_color(rgb(0x29364a))
                                .text_size(px(11.))
                                .line_height(px(16.5))
                                .child(div().font_weight(FontWeight::BOLD).child("终端恢复失败"))
                                .child(
                                    div()
                                        .w_full()
                                        .text_color(rgb(0x617086))
                                        .child(error.clone()),
                                )
                                .child(
                                    self.button(
                                        div()
                                            .id(SharedString::from(format!("retry-recovery-{id}")))
                                            .px(px(12.))
                                            .py(px(6.))
                                            .border_1()
                                            .border_color(rgb(0x516b8d))
                                            .rounded(px(7.))
                                            .text_color(rgb(0xe0ebfa))
                                            .bg(rgb(0x2a3b51))
                                            .text_size(px(11.))
                                            .line_height(px(15.))
                                            .cursor_pointer()
                                            .child("重试"),
                                        &self.recovery_controls[id],
                                        move |this, _, cx| {
                                            this.send(
                                                Command::RetryRecovery {
                                                    session_id: retry_id.clone(),
                                                },
                                                cx,
                                            )
                                        },
                                        cx,
                                    ),
                                ),
                        ),
                )
                .children(badge.map(|badge| badge.absolute().top(px(8.)).right(px(8.))))
            } else if compact {
                card.child(Self::compact_session_summary(node, title))
            } else {
                card.child(
                    div()
                        .id(SharedString::from(format!("session-header-{id}")))
                        .h(px(42.))
                        .flex_none()
                        .rounded_t(px(12.))
                        .px(px(12.))
                        .flex()
                        .items_center()
                        .bg(rgb(0xffffff))
                        .on_mouse_down(
                            MouseButton::Right,
                            cx.listener({
                                let id = id.clone();
                                move |this, event: &MouseDownEvent, _, cx| {
                                    this.open_fork_menu(&id, event.position, cx);
                                    cx.stop_propagation();
                                }
                            }),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::BOLD)
                                .child(title.to_owned()),
                        )
                        .child(div().flex_1())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .children(badge)
                                .children(node.map(|node| self.session_loader_button(id, node, cx)))
                                .children(self.render_fork_header(id, ForkPlacement::Child, cx))
                                .children(self.render_fork_header(id, ForkPlacement::Sibling, cx))
                                .children(
                                    node.filter(|node| node["archivedAt"].is_null())
                                        .map(|node| self.remove_card_button(id, node, cx)),
                                ),
                        ),
                )
                .when(!path_valid, |card| {
                    card.child(
                        div()
                            .flex_none()
                            .w_full()
                            .px(px(10.))
                            .py(px(7.))
                            .bg(rgb(0x2a1e17))
                            .text_color(rgb(0xe4aa78))
                            .text_size(px(11.))
                            .line_height(px(15.4))
                            .child(WORKSPACE_PATH_UNAVAILABLE),
                    )
                })
                .children(provider_banner.map(|banner| {
                    self.render_provider_restore_banner(id, banner, Theme::default(), cx)
                }))
                .children(
                    (fork_failure.is_none() && fork_progress.is_none())
                        .then(|| {
                            node.and_then(|node| self.start_failure_card(id, node))
                                .map(|card| self.render_start_failure_card(id, card, width, cx))
                        })
                        .flatten(),
                )
                .child(if let Some(failure) = fork_failure {
                    failure.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                } else {
                    let body = div()
                        .flex_1()
                        .min_h_0()
                        .relative()
                        .child(terminal.clone())
                        .when(
                            self.history_request
                                .as_ref()
                                .is_some_and(|request| request.session_id == *id),
                            |body| {
                                body.child(
                                    div()
                                        .absolute()
                                        .inset_0()
                                        .child(self.history_view.as_ref().unwrap().clone()),
                                )
                            },
                        )
                        .children(self.archive_summary(id))
                        .children(fork_progress.map(|progress| fork_progress_body(progress, now)))
                        .children(self.deferred_card(id).map(|card| {
                            self.render_deferred_recovery_card(id, card, width, window, cx)
                        }));
                    #[cfg(target_os = "macos")]
                    let body = body.when(water.is_some(), |body| body.opacity(0.));
                    body
                })
            };
            #[cfg(target_os = "macos")]
            let card = card.children(water);
            #[cfg(debug_assertions)]
            let card = if let Some(trace) = self
                .motion_trace
                .clone()
                .filter(|_| self.motion_clock.elapsed() < Duration::from_secs(90))
            {
                let clock = self.motion_clock;
                let session = id.clone();
                card.child(canvas(|_,_,_|(), move |bounds,_,_,_| {
                    record_motion(&trace,&serde_json::json!({"kind":"paint","renderAt":now,"paintAt":clock.elapsed().as_secs_f64()*1000.,
                        "session":session,"focused":selected,"x":f32::from(bounds.origin.x),"y":f32::from(bounds.origin.y),
                        "width":f32::from(bounds.size.width),"height":f32::from(bounds.size.height),"moving":moving}));
                }).absolute().top_0().left_0().size_full())
            } else {
                card
            };
            carousel = carousel.child(card);
        }
        if self.order.is_empty() {
            carousel = carousel.child(
                div()
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(12.))
                    .text_color(rgb(0x8a919b))
                    .child("当前画布没有活跃会话"),
            );
        }
        #[cfg(target_os = "macos")]
        {
            self.retain_recovery_water(window, cx, |_, id| mounted_water.contains(id));
            self.watch_recovery_water_visibility(window, cx);
        }
        if let Some(fault) = &self.fault {
            carousel = carousel.child(
                div()
                    .w(px(360.))
                    .p(px(16.))
                    .text_size(px(12.))
                    .text_color(rgb(0xb14542))
                    .child(fault.clone()),
            );
        }
        let view = cx.entity().downgrade();
        let direction = self.edge_direction;
        let cruising = self.edge_cruising;
        let elapsed = now - self.edge_phase_start;
        let indicator = canvas(
            |_, _, _| (),
            move |bounds, _, window, _| {
                let view = view.clone();
                window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
                    #[cfg(debug_assertions)]
                    if phase == DispatchPhase::Capture {
                        let _ = view.update(cx, |this, _| {
                            if let Some(trace) = &this.motion_trace { record_motion(trace, &serde_json::json!({"kind":"wheel","position":format!("{:?}",event.position),"delta":format!("{:?}",event.delta),"bounds":format!("{:?}",bounds)})); }
                        });
                    }
                    if phase != DispatchPhase::Capture
                        || !bounds.contains(&event.position)
                        || event.modifiers.control
                        || event.modifiers.platform
                    {
                        return;
                    }
                    let _ = view.update(cx, |this, cx| { if this.dialog.is_none() && this.hidden_tabs.is_none() { this.scroll_carousel(event, window, cx); } });
                });
                if direction == 0 {
                    return;
                }
                let x = if direction < 0 {
                    bounds.left() + px(6.)
                } else {
                    bounds.right() - px(9.)
                };
                let track = Bounds::new(
                    point(x, bounds.top() + px(18.)),
                    size(px(3.), bounds.size.height - px(36.)),
                );
                window.paint_quad(fill(track, rgba(0x6faaf024)).corner_radii(px(2.)));
                let (opacity, scale) = if cruising {
                    let p = (elapsed % 900.) / 900.;
                    (
                        if p < 0.55 {
                            0.22 + 0.63 * Curve::Edge.ease(p / 0.55)
                        } else {
                            0.85 - 0.53 * Curve::Edge.ease((p - 0.55) / 0.45)
                        },
                        1.,
                    )
                } else {
                    let p = (elapsed / 180.).clamp(0., 1.);
                    (0.2 + 0.62 * p, 0.08 + 0.92 * p)
                };
                let mut color: Hsla = rgb(0x6faaf0).into();
                color.a = opacity as f32;
                let height = track.size.height * scale as f32;
                window.paint_quad(
                    fill(
                        Bounds::new(point(x, track.bottom() - height), size(px(3.), height)),
                        color,
                    )
                    .corner_radii(px(2.)),
                );
            },
        )
        .absolute()
        .size_full();
        let canvas_dialog = matches!(self.dialog, Some(TabDialog::RemoveSession { .. }));
        let overlay = self
            .tab_overlay(window, cx)
            .or_else(|| self.render_fork_menu(window, cx));
        let (carousel_overlay, window_overlay) = if canvas_dialog {
            (overlay, None)
        } else {
            (None, overlay)
        };
        let carousel = div()
            .flex_1()
            .min_h_0()
            .relative()
            .flex()
            .child(carousel)
            .child(indicator)
            .children(carousel_overlay);
        let toast = self.task_toast.as_ref().map(|text| {
            div()
                .absolute()
                .top(px(20.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .px(px(15.))
                        .py(px(9.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(rgba(0x67c23a59))
                        .bg(rgb(0x1c2518))
                        .text_color(rgb(0xb3e19d))
                        .text_size(px(13.))
                        .child(text.clone()),
                )
        });
        // `.terminal-toast.is-error`: independent 3s permission failure feedback.
        let permission_toast = self.permission_toast.as_ref().map(|text| {
            div()
                .absolute()
                .top(px(20.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .id("permission-error-toast")
                        .px(px(15.))
                        .py(px(9.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(rgba(0xf8514959))
                        .bg(rgb(0x2b1718))
                        .text_color(rgb(0xffa198))
                        .text_size(px(13.))
                        .shadow(vec![shadow(0x000000, 0.3, 6., 18.)])
                        .child(text.clone()),
                )
        });
        let geometry_view = cx.entity().downgrade();
        div()
            .id("workspace-root")
            .relative()
            .capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                if event.button == MouseButton::Left {
                    this.workspace_drag_gesture = false;
                }
                if this.keyboard_focus {
                    this.keyboard_focus = false;
                    cx.notify();
                }
            }))
            .size_full()
            .flex()
            .font_family(".AppleSystemUIFont")
            .text_color(rgb(0x303744))
            .child(self.sidebar(window, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(self.topbar(window, cx))
                    .child(carousel)
                    .children(self.search_bar(cx))
                    .child(self.footer(window, cx)),
            )
            .child(
                canvas(
                    |_, _, _| (),
                    move |_, _, window, cx| {
                        #[cfg(all(target_os = "macos", debug_assertions))]
                        crate::macos::window_diag::workspace_painted(window, cx);
                        let _ = geometry_view.update(cx, |this, cx| {
                            let overflow = this.tab_scroll.max_offset().width > px(4.);
                            if overflow != this.tab_overflow {
                                this.tab_overflow = overflow;
                                if !overflow {
                                    this.hidden_tabs = None;
                                }
                                cx.notify();
                            }
                            let sidebar_overflow =
                                this.sidebar_scroll.max_offset().height > px(0.5);
                            if sidebar_overflow != this.sidebar_overflow {
                                this.sidebar_overflow = sidebar_overflow;
                                if !sidebar_overflow {
                                    this.sidebar_scrollbar.reset();
                                }
                                cx.notify();
                            }
                            // Native scroll bounds are ready only after this frame's layout.
                            let _ = window;
                        });
                    },
                )
                .absolute()
                .size(px(0.)),
            )
            .children(self.notification_panel(window, cx))
            .children(badge_tooltip.filter(|_| !canvas_dialog))
            .children(toast)
            .children(
                self.fork_readiness
                    .filter(|_| self.dialog.is_none())
                    .map(fork_readiness_toast),
            )
            .children(window_overlay)
            .children(permission_toast)
    }
}

fn fork_submission_key() -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("desktop-fork-{}-{time}-{sequence}", std::process::id())
}

// NotificationCenter.tsx, hierarchy.css:449–500 / 870–875, and i18n/messages/notifications.ts.
// Panel geometry from `.notification-center`:
// position fixed; top 49; bottom 5; left 0; width 382; radius 12.
const NOTIFICATION_UI_PANEL_WIDTH: f32 = 382.;
const NOTIFICATION_UI_PANEL_TOP: f32 = 49.;
const NOTIFICATION_UI_PANEL_BOTTOM_GAP: f32 = 5.;
const NOTIFICATION_UI_PANEL_RADIUS: f32 = 12.;
const NOTIFICATION_UI_HEADER_HEIGHT: f32 = 44.;

// `.flat-sidebar__notify` is 30x30. The real rect at the fixed sidebar
// header is x=218..248, y=9..39 (260px sidebar, 48px header); the parent's
// window-level outside-click capture should use that rect to keep the bell's
// second click working.
const NOTIFICATION_UI_BELL_SIZE: f32 = 30.;
const NOTIFICATION_UI_SIDEBAR_WIDTH: f32 = 260.;
const NOTIFICATION_UI_SIDEBAR_HEADER_HEIGHT: f32 = 48.;
const NOTIFICATION_UI_SIDEBAR_RIGHT_PADDING: f32 = 12.;
const NOTIFICATION_UI_BELL_LEFT: f32 = NOTIFICATION_UI_SIDEBAR_WIDTH
    - 1.
    - NOTIFICATION_UI_SIDEBAR_RIGHT_PADDING
    - NOTIFICATION_UI_BELL_SIZE;
const NOTIFICATION_UI_BELL_TOP: f32 =
    (NOTIFICATION_UI_SIDEBAR_HEADER_HEIGHT - NOTIFICATION_UI_BELL_SIZE) / 2.;

// `.notification-item__dismiss`.
const NOTIFICATION_UI_DISMISS_TOP: f32 = 10.;
const NOTIFICATION_UI_DISMISS_RIGHT: f32 = 10.;
const NOTIFICATION_UI_DISMISS_SIZE: f32 = 20.;

// `.notification-center` animation: notification-panel-drop-in .22s
// cubic-bezier(.16,1,.3,1), from translateY(-8px) + opacity 0.
const NOTIFICATION_UI_ANIMATION_MS: u64 = 220;
const NOTIFICATION_UI_ANIMATION_OFFSET: f32 = 8.;

#[derive(Clone, Copy)]
struct NotificationUiPalette {
    panel_bg: u32,
    text: u32,
    muted: u32,
    count: u32,
    item_bg: Rgba,
    item_hover_bg: Rgba,
    action: u32,
    action_hover: u32,
    action_hover_bg: Rgba,
    close: u32,
    close_hover: u32,
    dismiss: u32,
    dismiss_hover_bg: Rgba,
    empty: u32,
    shadow_alpha: f32,
    shadow_y: f32,
    shadow_blur: f32,
}

impl NotificationUiPalette {
    /// The panel is the only themed surface in this file. The workspace
    /// around it keeps its existing light treatment.
    fn for_appearance(appearance: WindowAppearance) -> Self {
        let light = matches!(
            appearance,
            WindowAppearance::Light | WindowAppearance::VibrantLight
        );
        let dark = Self {
            panel_bg: 0x212121,
            text: 0xdfdfdf,
            muted: 0x7b7b7b,
            count: 0x898989,
            item_bg: rgba(0xffffff0a),
            item_hover_bg: rgba(0xffffff0f),
            action: 0x797979,
            action_hover: 0xffffff,
            action_hover_bg: rgba(0xffffff0f),
            close: 0xdfdfdf,
            close_hover: 0xffffff,
            dismiss: 0xffffff,
            dismiss_hover_bg: rgba(0xffffff1a),
            empty: 0x8b949e,
            shadow_alpha: 0.38,
            shadow_y: 8.,
            shadow_blur: 24.,
        };
        if light {
            Self {
                panel_bg: 0xffffff,
                text: 0x2f3547,
                item_bg: rgba(0xf7f8faff),
                item_hover_bg: rgba(0xeceef3ff),
                shadow_alpha: 0.1,
                shadow_y: 4.,
                shadow_blur: 20.,
                ..dark
            }
        } else {
            dark
        }
    }
}

fn notification_ui_icon(name: &str, size: f32, color: u32) -> Svg {
    // GPUI's Svg paint reads the computed `text.color` directly (it does not
    // fall back to the parent's text style), so pass it explicitly.
    svg()
        .path(SharedString::from(format!("icons/{name}.svg")))
        .size(px(size))
        .flex_none()
        .text_color(rgb(color))
}

/// Svg paints from its own computed `text.color`, so a hover colour on the
/// parent button would not reach it. Bind the icon to the button's group
/// instead. Used by the two header actions whose frozen hover turns #fff.
fn notification_ui_icon_with_hover(
    name: &str,
    size: f32,
    color: u32,
    group: SharedString,
    hover: u32,
) -> Svg {
    notification_ui_icon(name, size, color)
        .group_hover(group, move |style| style.text_color(rgb(hover)))
}

/// One axis of a cubic Bézier with endpoints 0/1.
fn notification_ui_bezier_axis(t: f32, control_1: f32, control_2: f32) -> f32 {
    let inverse = 1. - t;
    3. * inverse * inverse * t * control_1 + 3. * inverse * t * t * control_2 + t * t * t
}

/// `cubic-bezier(.16, 1, .3, 1)` from the frozen drop-in animation.
/// GPUI 0.2 ships named easings only, so solve x -> u with a small bisection
/// and evaluate y(u).
fn notification_ui_easing(progress: f32) -> f32 {
    let target = progress.clamp(0., 1.);
    let mut low = 0.;
    let mut high = 1.;
    let mut u = target;
    for _ in 0..12 {
        let x = notification_ui_bezier_axis(u, 0.16, 0.3);
        if x < target {
            low = u;
        } else {
            high = u;
        }
        u = (low + high) * 0.5;
    }
    notification_ui_bezier_axis(u, 1., 1.).clamp(0., 1.)
}

/// `.notification-item__dismiss` is `occlude`d so the body underneath does not
/// also receive the click. Its own `hover` keeps it visible while the pointer
/// is on it (the parent item's group hover no longer contributes there).
fn notification_ui_item_dismiss(
    id: u64,
    group: SharedString,
    palette: NotificationUiPalette,
    cx: &mut Context<Workspace>,
) -> Stateful<Div> {
    div()
        .id(SharedString::from(format!("notification-dismiss-{id}")))
        .absolute()
        .top(px(NOTIFICATION_UI_DISMISS_TOP))
        .right(px(NOTIFICATION_UI_DISMISS_RIGHT))
        .size(px(NOTIFICATION_UI_DISMISS_SIZE))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .text_color(rgb(palette.dismiss))
        .opacity(0.)
        .occlude()
        .cursor_pointer()
        .focusable()
        .tab_stop(true)
        .group_hover(group, |style| style.opacity(1.))
        .hover(|style| style.opacity(1.).bg(palette.dismiss_hover_bg))
        .focus(|style| style.opacity(1.))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |this, _, _, cx| {
            cx.stop_propagation();
            this.dismiss_notification(id, cx);
        }))
        .child(notification_ui_icon("x", 16., palette.dismiss))
}

fn notification_clear_icon() -> impl IntoElement {
    // Frozen 24×24 ClearIcon, at the header's 14 px icon size.
    canvas(
        |_, _, _| (),
        |bounds, (), window, _| {
            let at = |x: f32, y: f32| bounds.origin + point(px(x * 14. / 24.), px(y * 14. / 24.));
            let mut path = PathBuilder::stroke(px(2. * 14. / 24.));
            path.move_to(at(3., 6.));
            path.line_to(at(21., 6.));
            path.move_to(at(8., 6.));
            path.line_to(at(8., 4.));
            path.line_to(at(16., 4.));
            path.line_to(at(16., 6.));
            path.move_to(at(19., 6.));
            path.line_to(at(18., 20.));
            path.line_to(at(6., 20.));
            path.line_to(at(5., 6.));
            if let Ok(path) = path.build() {
                window.paint_path(path, window.text_style().color);
            }
        },
    )
    .size(px(14.))
    .flex_none()
}

fn notification_item_title(
    entry: &NotificationEntry,
    palette: NotificationUiPalette,
) -> Stateful<Div> {
    div()
        .id(SharedString::from(format!(
            "notification-item-title-{}",
            entry.id
        )))
        .text_color(rgb(palette.text))
        .text_size(px(14.))
        .line_height(relative(1.4))
        .font_weight(if entry.read {
            FontWeight::MEDIUM
        } else {
            FontWeight::BOLD
        })
        .child(entry.title.clone())
}

impl Workspace {
    /// `.flat-sidebar__notify` + the frozen red unread indicator, upgraded from
    /// the frozen 6px dot to the numeric badge required by this round.
    fn notification_bell(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let unread = self
            .notifications
            .entries
            .iter()
            .filter(|entry| !entry.read)
            .count();
        let badge = (unread > 0).then(|| {
            div()
                .id("notification-bell-badge")
                .absolute()
                .top(px(1.))
                .right(px(1.))
                .h(px(16.))
                .min_w(px(16.))
                .px(px(4.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(8.))
                .border(px(1.5))
                // The sidebar is the existing light surface regardless of
                // window appearance, so the frozen light dot border applies.
                .border_color(rgb(0xf4f5f7))
                .bg(rgb(0xf24d49))
                .text_size(px(10.))
                .line_height(px(12.))
                .font_weight(FontWeight::BOLD)
                .text_color(rgb(0xffffff))
                .child(unread.to_string())
        });
        let bell = div()
            .id("notification-bell")
            .relative()
            .size(px(NOTIFICATION_UI_BELL_SIZE))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(7.))
            .text_color(rgb(0x8d96a3))
            .cursor_pointer()
            .hover(|style| style.bg(rgba(0x2f35470f)))
            .child(notification_ui_icon("bell", 16., 0x8d96a3).opacity(0.9))
            .children(badge);
        self.button(
            bell,
            &self.control_focus["notifications"],
            |this, _, cx| this.toggle_notifications(cx),
            cx,
        )
        .tooltip(|_, cx| cx.new(|_| CardTooltip("通知中心".into())).into())
    }

    /// The panel is mounted by `Render for Workspace` as an absolute root
    /// child after the main sidebar/content children.
    fn notification_panel(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        if !self.notification_open {
            return None;
        }

        let palette = NotificationUiPalette::for_appearance(window.appearance());
        let viewport = window.viewport_size();
        let viewport_height = f32::from(viewport.height);
        let panel_height =
            (viewport_height - NOTIFICATION_UI_PANEL_TOP - NOTIFICATION_UI_PANEL_BOTTOM_GAP)
                .max(0.);

        let entries = &self.notifications.entries;

        let close_focus = window
            .use_keyed_state("notification-close-focus", cx, |_, cx| {
                cx.focus_handle().tab_stop(true)
            })
            .read(cx)
            .clone();
        let sound_focus = window
            .use_keyed_state("notification-sound-focus", cx, |_, cx| {
                cx.focus_handle().tab_stop(true)
            })
            .read(cx)
            .clone();
        let clear_focus = (!entries.is_empty()).then(|| {
            window
                .use_keyed_state("notification-clear-focus", cx, |_, cx| {
                    cx.focus_handle().tab_stop(true)
                })
                .read(cx)
                .clone()
        });

        // `.notification-center__title` + `__title-count`.
        let title = div()
            .id("notification-panel-title")
            .flex()
            .items_center()
            .gap(px(4.))
            .text_size(px(13.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(rgb(palette.text))
            .child("通知")
            .child(
                div()
                    .id("notification-panel-count")
                    .text_color(rgb(palette.count))
                    .child(format!("({})", entries.len())),
            );

        let mut header_actions = div().flex().items_center().gap(px(8.));
        if let Some(clear_focus) = clear_focus {
            let clear = div()
                .id("notification-clear")
                .flex()
                .items_center()
                .justify_center()
                .p(px(5.))
                .rounded(px(6.))
                .text_color(rgb(palette.action))
                .cursor_pointer()
                .hover(|style| {
                    style
                        .bg(palette.action_hover_bg)
                        .text_color(rgb(palette.action_hover))
                })
                .child(notification_clear_icon());
            header_actions = header_actions.child(
                self.button(
                    clear,
                    &clear_focus,
                    |this, _, cx| this.clear_notifications(cx),
                    cx,
                )
                .tooltip(|_, cx| cx.new(|_| CardTooltip("清空通知".into())).into()),
            );
        }
        // `.notification-center__close-btn`.
        let close_group = SharedString::from("notification-close-group");
        let close = div()
            .id("notification-close")
            .group(close_group.clone())
            .flex()
            .items_center()
            .justify_center()
            .p(px(4.))
            .rounded(px(6.))
            .text_color(rgb(palette.close))
            .cursor_pointer()
            .hover(|style| {
                style
                    .bg(palette.action_hover_bg)
                    .text_color(rgb(palette.close_hover))
            })
            .child(notification_ui_icon_with_hover(
                "x",
                16.,
                palette.close,
                close_group,
                palette.close_hover,
            ));
        header_actions = header_actions.child(
            self.button(
                close,
                &close_focus,
                |this, _, cx| this.close_notifications(cx),
                cx,
            )
            .tooltip(|_, cx| cx.new(|_| CardTooltip("关闭通知中心".into())).into()),
        );

        let header = div()
            .id("notification-panel-header")
            .flex_none()
            .h(px(NOTIFICATION_UI_HEADER_HEIGHT))
            .flex()
            .items_center()
            .justify_between()
            .px(px(12.))
            .child(title)
            .child(header_actions);

        let empty = entries.is_empty();
        let list_content = if empty {
            // `.notification-center__empty` / `__empty-text`.
            div()
                .id("notification-empty")
                .h_full()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .id("notification-empty-text")
                        .text_size(px(13.))
                        .text_color(rgb(palette.empty))
                        .child("暂无通知"),
                )
                .into_any_element()
        } else {
            let items = entries
                .iter()
                .map(|entry| self.notification_item(entry, palette, cx))
                .collect::<Vec<_>>();
            // `.notification-center__group { padding: 8px 16px 16px }` and
            // `.notification-center__items { gap: 8px }`.
            div()
                .id("notification-group")
                .px(px(16.))
                .pt(px(8.))
                .pb(px(16.))
                .child(
                    div()
                        .id("notification-items")
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .children(items),
                )
                .into_any_element()
        };

        // `.notification-center__list` is the only scroll container; the panel
        // is absolute and the list is `min-height: 0` so it cannot push the
        // workspace (or the footer) around.
        let scroll = window
            .use_keyed_state("notification-scroll", cx, |_, _| ScrollHandle::new())
            .read(cx)
            .clone();
        let list = div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                div()
                    .id("notification-list")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&scroll)
                    .scrollbar_width(px(4.))
                    .child(list_content),
            )
            .child(
                canvas(
                    |_, _, _| (),
                    move |bounds, (), window, _| {
                        let overflow = scroll.max_offset().height;
                        if overflow <= px(0.) {
                            return;
                        }
                        let height = bounds.size.height;
                        let thumb = (height * (height / (height + overflow)))
                            .max(px(10.))
                            .min(height);
                        let offset =
                            (height - thumb) * (-scroll.offset().y / overflow).clamp(0., 1.);
                        window.paint_quad(
                            fill(
                                Bounds::new(
                                    bounds.origin + point(px(0.), offset),
                                    size(px(4.), thumb),
                                ),
                                rgb(0x30363d),
                            )
                            .corner_radii(px(10.)),
                        );
                    },
                )
                .absolute()
                .right_0()
                .top_0()
                .h_full()
                .w(px(4.)),
            );

        // `.notification-center__footer` / `__sound-toggle` / `__switch`.
        let sound_enabled = self.notifications.sound_enabled;
        let sound_track = div()
            .id("notification-sound-track")
            .relative()
            .w(px(32.))
            .h(px(16.))
            .rounded(px(8.))
            .bg(if sound_enabled {
                rgb(0xff684e)
            } else {
                rgb(0x30363d)
            })
            .child(
                div()
                    .id("notification-sound-thumb")
                    .absolute()
                    .top(px(2.))
                    .left(px(if sound_enabled { 18. } else { 2. }))
                    .size(px(12.))
                    .rounded_full()
                    .bg(rgb(0xffffff)),
            );
        let sound_toggle = div()
            .id("notification-sound-toggle")
            .w_full()
            .flex()
            .items_center()
            .gap(px(8.))
            .cursor_pointer()
            .child(sound_track)
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(palette.muted))
                    .child("通知声音"),
            );
        let footer = div()
            .id("notification-footer")
            .flex_none()
            .px(px(14.))
            .py(px(12.))
            .child(self.button(
                sound_toggle,
                &sound_focus,
                |this, _, cx| this.toggle_notification_sound(cx),
                cx,
            ));

        let panel = div()
            .id("notification-panel")
            .absolute()
            .left(px(0.))
            .top(px(NOTIFICATION_UI_PANEL_TOP))
            .w(px(NOTIFICATION_UI_PANEL_WIDTH))
            .h(px(panel_height))
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded(px(NOTIFICATION_UI_PANEL_RADIUS))
            .bg(rgb(palette.panel_bg))
            .text_color(rgb(palette.text))
            .shadow(vec![shadow(
                0x000000,
                palette.shadow_alpha,
                palette.shadow_y,
                palette.shadow_blur,
            )])
            .occlude()
            .on_mouse_down_out(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                let bell = Bounds::new(
                    point(px(NOTIFICATION_UI_BELL_LEFT), px(NOTIFICATION_UI_BELL_TOP)),
                    size(px(NOTIFICATION_UI_BELL_SIZE), px(NOTIFICATION_UI_BELL_SIZE)),
                );
                if !bell.contains(&event.position) {
                    this.close_notifications(cx);
                }
            }))
            .child(header)
            .child(list)
            .child(footer);

        let panel: AnyElement = if reduced_motion() {
            panel.into_any_element()
        } else {
            panel
                .with_animation(
                    "notification-panel-drop-in",
                    Animation::new(Duration::from_millis(NOTIFICATION_UI_ANIMATION_MS))
                        .with_easing(notification_ui_easing),
                    |panel, delta| {
                        panel.opacity(delta).top(px(NOTIFICATION_UI_PANEL_TOP
                            - NOTIFICATION_UI_ANIMATION_OFFSET * (1. - delta)))
                    },
                )
                .into_any_element()
        };

        Some(panel)
    }

    /// `.notification-item` + `.notification-item__body`.
    fn notification_item(
        &self,
        entry: &NotificationEntry,
        palette: NotificationUiPalette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = entry.id;
        let group = SharedString::from(format!("notification-item-group-{id}"));

        let workspace_name = entry
            .workspace_id
            .as_deref()
            .and_then(|id| {
                self.workspaces
                    .iter()
                    .find(|workspace| workspace.id.as_str() == id)
            })
            .map_or_else(
                || "未知工作区".to_string(),
                |workspace| workspace.name.clone(),
            );
        let task_name = entry
            .task_id
            .as_deref()
            .and_then(|id| self.tasks.iter().find(|task| task.id.as_str() == id))
            .map_or_else(|| "未知工作台".to_string(), |task| task.title.clone());

        let breadcrumb_part = |text: String| {
            div()
                .min_w_0()
                .flex_shrink()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(text)
        };
        let breadcrumb = div()
            .id(SharedString::from(format!(
                "notification-item-breadcrumb-{id}"
            )))
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(4.))
            .text_size(px(12.))
            .text_color(rgb(palette.muted))
            .child(breadcrumb_part(workspace_name))
            .child(div().flex_none().opacity(0.5).child("/"))
            .child(breadcrumb_part(task_name));

        let title_row = div()
            .flex()
            .items_center()
            .my(px(4.))
            .child(notification_item_title(entry, palette));

        let body = div()
            .id(SharedString::from(format!("notification-item-body-{id}")))
            .w_full()
            .flex()
            .flex_col()
            .focusable()
            .tab_stop(true)
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.open_notification(id, window, cx);
            }))
            .child(breadcrumb)
            .child(title_row)
            .when(!entry.subtitle.is_empty(), |body| {
                // `.notification-item__subtitle`.
                body.child(
                    div()
                        .id(SharedString::from(format!(
                            "notification-item-subtitle-{id}"
                        )))
                        .mb(px(6.))
                        .text_color(rgb(palette.muted))
                        .text_size(px(12.))
                        .line_height(relative(1.4))
                        .child(entry.subtitle.clone()),
                )
            })
            .when(!entry.body.is_empty(), |body| {
                // `.notification-item__content`: pre-wrap + four-line clamp.
                body.child(
                    div()
                        .id(SharedString::from(format!(
                            "notification-item-content-{id}"
                        )))
                        .mb(px(4.))
                        .text_color(rgb(palette.text))
                        .text_size(px(12.))
                        .line_height(relative(1.6))
                        .line_clamp(4)
                        .child(entry.body.clone()),
                )
            })
            .child(
                // `.notification-item__time`; the string is formatted by the
                // parent's `notification_time`.
                div()
                    .id(SharedString::from(format!("notification-item-time-{id}")))
                    .mt(px(8.))
                    .text_color(rgb(palette.muted))
                    .text_size(px(12.))
                    .child(notification_time(entry.occurred_at)),
            );

        let dismiss = notification_ui_item_dismiss(id, group.clone(), palette, cx);
        div()
            .id(SharedString::from(format!("notification-item-{id}")))
            .group(group)
            .relative()
            .px(px(12.))
            .py(px(8.))
            .rounded(px(10.))
            .bg(palette.item_bg)
            .hover(|style| style.bg(palette.item_hover_bg))
            .cursor_pointer()
            .child(body)
            .child(dismiss)
            .into_any_element()
    }
}

fn notification_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

fn notification_time(timestamp: u64) -> String {
    // Use the system's local zone, including its offset at the event time.
    #[repr(C)]
    #[derive(Default)]
    struct LocalTime {
        second: i32,
        minute: i32,
        hour: i32,
        day: i32,
        month: i32,
        year: i32,
        weekday: i32,
        year_day: i32,
        is_dst: i32,
        #[cfg(target_os = "macos")]
        utc_offset: std::ffi::c_long,
        #[cfg(target_os = "macos")]
        timezone: *const std::ffi::c_char,
    }
    #[cfg(target_os = "macos")]
    unsafe extern "C" {
        fn localtime_r(seconds: *const i64, output: *mut LocalTime) -> *mut LocalTime;
    }
    #[cfg(target_os = "windows")]
    unsafe extern "C" {
        fn _localtime64_s(output: *mut LocalTime, seconds: *const i64) -> i32;
    }
    let seconds = (timestamp / 1_000) as i64;
    let mut local = LocalTime::default();
    // Each function fills the caller-owned C tm for this platform.
    #[cfg(target_os = "macos")]
    let valid = unsafe { !localtime_r(&seconds, &mut local).is_null() };
    #[cfg(target_os = "windows")]
    let valid = unsafe { _localtime64_s(&mut local, &seconds) == 0 };
    if valid {
        format!("{:02}:{:02}", local.hour, local.minute)
    } else {
        format!("{:02}:{:02}", seconds / 3_600 % 24, seconds / 60 % 60)
    }
}

fn play_notification_sound() {
    #[cfg(target_os = "macos")]
    {
        #[link(name = "AppKit", kind = "framework")]
        unsafe extern "C" {
            fn NSBeep();
        }
        // Workspace handles graph events on the GPUI main thread.
        unsafe { NSBeep() };
    }
    #[cfg(target_os = "windows")]
    {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn MessageBeep(kind: u32) -> i32;
        }
        // Zero selects the standard system notification sound.
        unsafe { MessageBeep(0) };
    }
}

fn trimmed_graph_text(value: &Value) -> Option<&str> {
    value
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

fn generic_agent_title(title: &str, short: &str) -> bool {
    let title = title.trim().to_lowercase();
    title == short.to_lowercase()
        || matches!(
            title.as_str(),
            "" | "claude"
                | "claude code"
                | "antigravity"
                | "codex"
                | "shell"
                | "未命名会话"
                | "未命名 claude 会话"
        )
}

fn loader_relative_time(updated_at: i64) -> String {
    if updated_at == 0 {
        return "时间未知".into();
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64);
    let hours = (now - updated_at).max(0) / 3_600_000;
    if hours < 24 {
        if hours <= 0 {
            "刚刚".into()
        } else {
            format!("{hours} 小时前")
        }
    } else {
        format!("{} 天前", hours / 24)
    }
}

// ---------------------------------------------------------------------------
// Permission chip / menu visuals.
//
// Render-only. Frozen sources:
// - apps/desktop/src/renderer/src/hud/TerminalHud.tsx:137-145,205-260;
// - apps/desktop/src/renderer/src/i18n/messages/hud.ts:7-18,44-66;
// - apps/desktop/src/renderer/src/hierarchy/hierarchy.css:317-323,347-362;
// - apps/desktop/src/renderer/src/hierarchy/hierarchy.css:446-447
//   (`.terminal-toast.is-error`).
//
// State and commands stay with the Workspace wiring; these helpers only
// encode the frozen geometry, colors and copy so probes can assert them.
// ---------------------------------------------------------------------------

const PERMISSION_FOOTER_HEIGHT: f32 = 38.;
const PERMISSION_FOOTER_PADDING_X: f32 = 20.;
/// The frozen `.perm-menu { min-width: 180px }`; the footer keeps this much
/// room on the right so the left-aligned menu never clips at the window edge.
const PERMISSION_STATUS_WIDTH: f32 = 180.;
const PERMISSION_MENU_MIN_WIDTH: f32 = 180.;
const PERMISSION_MENU_PADDING: f32 = 6.;
/// Frozen item padding (6px) plus a 12px/14px line box: the browser's normal
/// line-height at 12px. Fixed so probe row centres stay stable.
const PERMISSION_MENU_ITEM_HEIGHT: f32 = 26.;
const PERMISSION_CHIP_HEIGHT: f32 = 14.;
const PERMISSION_CHIP_LINE_HEIGHT: f32 = 12.;
const PERMISSION_CHIP_TRACKING: f32 = 0.2;
const PERMISSION_CHIP_DISABLED_OPACITY: f32 = 0.7;
const PERMISSION_MENU_GAP: f32 = 8.;
const PERMISSION_MENU_TITLE: &str = "权限模式";
const PERMISSION_MENU_TITLE_TRACKING: f32 = 0.5;
const PERMISSION_MENU_TITLE_LINE_HEIGHT: f32 = 13.;
const PERMISSION_MENU_CHECK: &str = "✓";

/// `.status-perm-badge.perm-<mode>`. `Auto` keeps the frozen green text.
fn permission_mode_color(mode: PermissionMode) -> u32 {
    match mode {
        PermissionMode::Default => 0x8b949e,
        PermissionMode::Auto => 0x7ee787,
        PermissionMode::AcceptEdits => 0xd29922,
        PermissionMode::Plan => 0x58a6ff,
        PermissionMode::Bypass => 0xff6b35,
    }
}

/// `.perm-menu__dot.perm-<mode>`. The frozen CSS has no `.perm-auto` dot rule;
/// use the same neutral as Default instead of the chip's green.
fn permission_menu_dot_color(mode: PermissionMode) -> u32 {
    match mode {
        PermissionMode::Auto => 0x8b949e,
        other => permission_mode_color(other),
    }
}

/// Uppercased chip label. CSS `text-transform: uppercase`; the tooltip keeps
/// the original mixed-case label supplied in `PermissionChip::title`.
fn permission_chip_text(mode: PermissionMode) -> SharedString {
    mode.label().to_uppercase().into()
}

fn permission_mode_slug(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::Default => "default",
        PermissionMode::Auto => "auto",
        PermissionMode::AcceptEdits => "accept-edits",
        PermissionMode::Plan => "plan",
        PermissionMode::Bypass => "bypass-permissions",
    }
}

fn permission_menu_item_id(mode: PermissionMode) -> SharedString {
    format!("permission-menu-item-{}", permission_mode_slug(mode)).into()
}

fn permission_menu_dot_id(mode: PermissionMode) -> SharedString {
    format!("permission-menu-dot-{}", permission_mode_slug(mode)).into()
}

fn permission_menu_label_id(mode: PermissionMode) -> SharedString {
    format!("permission-menu-label-{}", permission_mode_slug(mode)).into()
}

fn permission_menu_check_id(mode: PermissionMode) -> SharedString {
    format!("permission-menu-check-{}", permission_mode_slug(mode)).into()
}

/// CSS `letter-spacing` substitute: one text cell per glyph with a fractional
/// flex gap. The trailing zero-width cell contributes the spacing after the
/// final glyph, matching the browser's included advance.
fn permission_tracked_text(text: impl Into<SharedString>, tracking: f32) -> Div {
    let text = text.into();
    div()
        .flex()
        .items_center()
        .gap(px(tracking))
        .children(text.chars().map(|ch| ui_text(ch.to_string())))
        .child(div().w(px(0.)))
}

/// `.perm-menu__title`.
fn permission_menu_title() -> Stateful<Div> {
    div()
        .id("permission-menu-title")
        .mb(px(4.))
        .px(px(10.))
        .pt(px(6.))
        .pb(px(8.))
        .border_b_1()
        .border_color(rgb(0x30363d))
        .text_color(rgb(0x6e7681))
        .text_size(px(11.))
        .line_height(px(PERMISSION_MENU_TITLE_LINE_HEIGHT))
        .child(permission_tracked_text(
            PERMISSION_MENU_TITLE.to_uppercase(),
            PERMISSION_MENU_TITLE_TRACKING,
        ))
}

/// `.perm-menu button.perm-menu__item` (+ `.is-active` / `:hover`).
fn permission_menu_item(mode: PermissionMode, active: PermissionMode) -> Stateful<Div> {
    let is_active = mode == active;
    let item = div()
        .id(permission_menu_item_id(mode))
        .flex()
        .items_center()
        .w_full()
        .h(px(PERMISSION_MENU_ITEM_HEIGHT))
        .gap(px(8.))
        .px(px(10.))
        .py(px(6.))
        .rounded(px(4.))
        .text_size(px(12.))
        .line_height(px(14.))
        .text_color(rgb(if is_active { 0x58a6ff } else { 0xc9d1d9 }))
        .bg(if is_active {
            Rgba {
                a: 0.1,
                ..rgb(0x58a6ff)
            }
        } else {
            rgba(0x00000000)
        })
        .cursor_pointer()
        .child(
            div()
                .id(permission_menu_dot_id(mode))
                .flex_none()
                .size(px(8.))
                .rounded_full()
                .bg(rgb(permission_menu_dot_color(mode))),
        )
        .child(
            div()
                .id(permission_menu_label_id(mode))
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(ui_text(mode.label())),
        )
        .when(is_active, |item| {
            item.child(
                div()
                    .id(permission_menu_check_id(mode))
                    .flex_none()
                    .font_weight(FontWeight::BOLD)
                    .text_color(rgb(0x58a6ff))
                    .child(ui_text(PERMISSION_MENU_CHECK)),
            )
        });
    if is_active {
        // The frozen `.is-active` rule follows `:hover`, so the active row
        // keeps its blue tint instead of switching to the neutral hover gray.
        item
    } else {
        item.hover(|style| style.bg(rgb(0x21262d)))
    }
}

/// Window-space bottom-left anchor for the menu: chip's left/top edge, 8px up.
/// The chip is vertically centred in the 38px footer and left-aligned in the
/// 180px status cell, so this is exact without per-frame measurement.
fn permission_menu_anchor(viewport: Size<Pixels>) -> Point<Pixels> {
    let chip_top_from_bottom = (PERMISSION_FOOTER_HEIGHT - 1. + PERMISSION_CHIP_HEIGHT) / 2.;
    point(
        viewport.width - px(PERMISSION_FOOTER_PADDING_X + PERMISSION_STATUS_WIDTH),
        viewport.height - px(chip_top_from_bottom + PERMISSION_MENU_GAP),
    )
}

fn loader_permission_label(mode: &str) -> &str {
    match mode {
        "" | "default" => "默认权限",
        "auto" => "自动模式",
        "acceptEdits" => "自动接受编辑",
        "plan" => "计划模式",
        "bypassPermissions" => "开放所有权限",
        "ask" => "逐项确认",
        other => other,
    }
}

fn loader_event_label(kind: &str, tool_name: Option<&str>) -> SharedString {
    match kind {
        "user" => "你".into(),
        "assistant" => "Claude".into(),
        "tool" => tool_name
            .filter(|name| !name.is_empty())
            .map_or_else(|| "工具".into(), |name| format!("工具 · {name}").into()),
        _ => "系统".into(),
    }
}

fn icon(name: &str, size: f32, color: u32) -> Svg {
    svg()
        .path(SharedString::from(format!("icons/{name}.svg")))
        .size(px(size))
        .flex_none()
        .text_color(rgb(color))
}

fn shadow(color: u32, alpha: f32, y: f32, blur: f32) -> BoxShadow {
    let mut color: Hsla = rgb(color).into();
    color.a = alpha;
    BoxShadow {
        color,
        offset: point(px(0.), px(y)),
        blur_radius: px(blur),
        spread_radius: px(0.),
    }
}

fn reduced_motion() -> bool {
    #[cfg(target_os = "macos")]
    {
        crate::macos::reduced_motion()
    }
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW,
        };
        let mut enabled: i32 = 1;
        // Query only: never changes the user's accessibility preference.
        unsafe {
            SystemParametersInfoW(
                SPI_GETCLIENTAREAANIMATION,
                0,
                (&mut enabled as *mut i32).cast(),
                0,
            );
        }
        enabled == 0
    }
}

// Debug-only, opt-in acceptance probe. Records actual GPUI paint bounds; it is
// not physical input-to-photon latency and is absent from release builds.
#[cfg(debug_assertions)]
fn open_motion_trace() -> Option<MotionTrace> {
    let path = std::env::var_os("MATOU_NATIVE_MOTION_TRACE")?;
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(file) => Some(std::rc::Rc::new(std::cell::RefCell::new(file))),
        Err(error) => {
            eprintln!("Open motion acceptance trace: {error}");
            None
        }
    }
}
#[cfg(debug_assertions)]
fn record_motion(trace: &MotionTrace, record: &Value) {
    use std::io::Write;
    if let Err(error) = writeln!(trace.borrow_mut(), "{record}") {
        eprintln!("Write motion acceptance trace: {error}");
    }
}

// ---------------------------------------------------------------------------
// Fork entry points, dialog, progress, failure and readiness views.
//
// Render-only: commands, submission keys, graph state and the Workspace
// callbacks stay in the main view. Frozen sources:
// - hierarchy/hierarchy.css `.pane-fork` (221-224) and its light-surface
//   variant (759-761), `.fork-readiness-toast` (225 / 762),
//   `.fork-failure-card` (767-771);
// - hierarchy/hierarchy.css `.branch-dialog*` (787-813);
// - session-canvas/ForkProgressOverlay.tsx and fork-progress-overlay.css;
// - i18n sessionCanvas.ts `branchDialog`, hierarchyTerminal.ts `pane`.
//
// The native card head is a light surface, so the entry icon uses the frozen
// light-theme `.pane-fork` colors; the dialog, progress and failure surfaces
// keep their frozen dark colors.
// ---------------------------------------------------------------------------

/// `sessionCanvas.branchDialog.worktreeSection`.
const FORK_WORKTREE_SECTION: &str = "工作目录";
/// `sessionCanvas.branchDialog.useCurrentWorktree`.
const FORK_USE_CURRENT_WORKTREE: &str = "使用当前工作树";
/// `sessionCanvas.branchDialog.useNewWorktree`.
const FORK_USE_NEW_WORKTREE: &str = "从新工作树创建";
/// `sessionCanvas.branchDialog.newWorktreeHint`.
const FORK_NEW_WORKTREE_HINT: &str = "创建隔离的 Git worktree，适合多个功能并行开发";
/// `sessionCanvas.branchDialog.needsGitRepository`.
const FORK_NEEDS_GIT_REPOSITORY: &str = "需要 Git 仓库";
/// `sessionCanvas.branchDialog.name`.
const FORK_DIALOG_NAME: &str = "分支名称";
/// `sessionCanvas.branchDialog.creating`.
const FORK_DIALOG_CREATING: &str = "正在创建分支…";
/// `sessionCanvas.branchDialog.nameTooLong` / the frozen counter denominator.
const FORK_NAME_LIMIT: usize = 64;

/// `hierarchyTerminal.pane.forkChild` uses lucide `layers-plus`;
/// `forkSibling` uses `copy-plus`.
fn fork_header_icon(sibling: bool) -> &'static str {
    if sibling { "copy-plus" } else { "layers-plus" }
}

/// `.pane-fork` at 24×24 with a 16px frozen AppIcon (the React default).
/// `dimmed` means `canFork == false`, `blocked` means the shared action gate is
/// closed. Both share the frozen `[aria-disabled="true"]` opacity; a blocked
/// entry shows the not-allowed cursor in place of the browser's `help`.
fn fork_header_button(
    id: SharedString,
    sibling: bool,
    dimmed: bool,
    blocked: bool,
    tooltip: SharedString,
) -> Stateful<Div> {
    let enabled = !dimmed && !blocked;
    let glyph = svg()
        .path(SharedString::from(format!(
            "icons/{}.svg",
            fork_header_icon(sibling)
        )))
        .size(px(16.))
        .flex_none()
        .text_color(rgba(0x00000085))
        .when(enabled, |glyph| {
            glyph.group_hover(id.clone(), |style| style.text_color(rgba(0x000000d1)))
        });
    let button = div()
        .id(id.clone())
        .group(id)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .size(px(24.))
        .rounded(px(5.))
        .bg(rgba(0x00000000))
        .text_color(rgba(0x00000085))
        .child(glyph)
        .tooltip(move |_, cx| cx.new(|_| CardTooltip(tooltip.clone())).into());
    if enabled {
        button
            .cursor_pointer()
            .hover(|style| style.text_color(rgba(0x000000d1)).bg(rgba(0x0000000f)))
    } else {
        button
            .opacity(0.38)
            .cursor(CursorStyle::OperationNotAllowed)
    }
}

/// `.branch-dialog__field small`: `n/64`, bottom-right of the input wrapper.
fn fork_dialog_name_counter(count: usize) -> Div {
    div()
        .absolute()
        .right(px(10.))
        .bottom(px(12.))
        .text_color(rgb(0x747b86))
        .text_size(px(12.))
        .child(format!("{count}/{FORK_NAME_LIMIT}"))
}

/// `.branch-worktree-card input` (frozen accent `#5b9de9`).
fn fork_worktree_radio(selected: bool) -> Div {
    div()
        .flex_none()
        .mt(px(2.))
        .flex()
        .items_center()
        .justify_center()
        .size(px(14.))
        .rounded_full()
        .border_1()
        .border_color(if selected {
            rgb(0x5b9de9)
        } else {
            rgb(0x5a616c)
        })
        .when(selected, |radio| {
            radio.child(div().size(px(6.)).rounded_full().bg(rgb(0x5b9de9)))
        })
}

/// `.branch-worktree-card` (+ `.is-selected` / `.is-disabled`).
fn fork_worktree_card(
    selected: bool,
    disabled: bool,
    title: &'static str,
    hint: &str,
    note: Option<&'static str>,
) -> Div {
    let card = div()
        .flex()
        .gap(px(10.))
        .p(px(12.))
        .border_1()
        .border_color(if selected {
            rgb(0x5b9de9)
        } else {
            rgb(0x343942)
        })
        .rounded(px(10.))
        .bg(if selected {
            rgba(0x5b9de917)
        } else {
            rgb(0x181a1e)
        })
        .child(fork_worktree_radio(selected))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .child(div().text_size(px(13.)).child(title))
                .child(
                    div()
                        .text_color(rgb(0x8f96a1))
                        .text_size(px(11.))
                        .child(ui_text(hint.to_owned())),
                )
                .children(note.map(|note| {
                    div()
                        .text_color(rgb(0xe6a44a))
                        .text_size(px(11.))
                        .child(note)
                })),
        );
    if disabled {
        card.opacity(0.55)
    } else {
        card.cursor_pointer()
    }
}

/// `.branch-dialog` panel, excluding the overlay/backdrop owned by the host.
/// `so` is `(close, input, cancel, confirm)`; the input must be the existing
/// `RenameInput` entity. The two worktree cards are frozen presentation: the
/// current worktree is selected and the new-worktree card is always disabled
/// this round, with its "需要 Git 仓库" note.
#[allow(clippy::too_many_arguments)]
fn fork_dialog_panel(
    viewport_width: Pixels,
    title: &str,
    description: &str,
    current_hint: &str,
    count: usize,
    submitting: bool,
    error: Option<&str>,
    so: (AnyElement, AnyElement, AnyElement, AnyElement),
) -> Div {
    let (close, input, cancel, confirm) = so;
    let panel_width = 520.0_f32.min((f32::from(viewport_width) - 48.0).max(0.0));
    let header = div()
        .flex()
        .items_start()
        .justify_between()
        .gap(px(16.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(px(18.))
                        .font_weight(FontWeight::BOLD)
                        .child(ui_text(title.to_owned())),
                )
                .child(
                    div()
                        .mt(px(6.))
                        .text_color(rgb(0x9298a3))
                        .text_size(px(12.))
                        .line_height(relative(1.45))
                        .child(ui_text(description.to_owned())),
                ),
        )
        .child(close);
    let field = div()
        .relative()
        .flex()
        .flex_col()
        .gap(px(7.))
        .mt(px(20.))
        .text_color(rgb(0xc9cdd4))
        .text_size(px(12.))
        .child(div().child(FORK_DIALOG_NAME))
        .child(
            div()
                .relative()
                .w_full()
                .child(input)
                .child(fork_dialog_name_counter(count)),
        );
    let worktrees = div()
        .flex()
        .flex_col()
        .gap(px(9.))
        .mt(px(18.))
        .child(
            div()
                .mb(px(8.))
                .text_color(rgb(0xc9cdd4))
                .text_size(px(12.))
                .child(FORK_WORKTREE_SECTION),
        )
        .child(fork_worktree_card(
            true,
            false,
            FORK_USE_CURRENT_WORKTREE,
            current_hint,
            None,
        ))
        .child(fork_worktree_card(
            false,
            true,
            FORK_USE_NEW_WORKTREE,
            FORK_NEW_WORKTREE_HINT,
            Some(FORK_NEEDS_GIT_REPOSITORY),
        ));
    let mut panel = div()
        .flex()
        .flex_col()
        .w(px(panel_width))
        .p(px(20.))
        .border_1()
        .border_color(rgb(0x343942))
        .rounded(px(16.))
        .text_color(rgb(0xe8eaed))
        .bg(rgb(0x1d1f23))
        .shadow(vec![shadow(0x000000, 0.46, 24., 80.)])
        .child(header)
        .child(field)
        .child(worktrees);
    if let Some(error) = error {
        panel = panel.child(
            div()
                .mt(px(12.))
                .text_color(rgb(0xff8276))
                .text_size(px(12.))
                .child(ui_text(error.to_owned())),
        );
    }
    if submitting {
        panel = panel.child(
            div()
                .mt(px(12.))
                .text_color(rgb(0x8ebcf4))
                .text_size(px(12.))
                .child(FORK_DIALOG_CREATING),
        );
    }
    panel.child(
        div()
            .flex()
            .justify_end()
            .gap(px(8.))
            .mt(px(20.))
            .child(cancel)
            .child(confirm),
    )
}

/// `.branch-dialog__header button`: the frozen 22px `×` close affordance.
fn fork_dialog_close() -> Stateful<Div> {
    div()
        .id("fork-dialog-close")
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .px(px(6.))
        .py(px(1.))
        .text_color(rgb(0xa5abb4))
        .text_size(px(22.))
        .line_height(relative(1.))
        .cursor_pointer()
        .child("×")
}

/// `.branch-dialog__footer button` (+ `.primary` / `:disabled`).
fn fork_dialog_button(
    id: &'static str,
    label: &'static str,
    primary: bool,
    disabled: bool,
) -> Stateful<Div> {
    let (border, text, background) = if primary {
        (rgb(0x4b8edc), rgb(0xffffff), rgb(0x347bc8))
    } else {
        (rgb(0x3b414a), rgb(0xd9dce1), rgb(0x26292e))
    };
    let button = div()
        .id(id)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .h(px(34.))
        .px(px(16.))
        .border_1()
        .border_color(border)
        .rounded(px(8.))
        .text_color(text)
        .bg(background)
        .text_size(px(16.))
        .whitespace_nowrap()
        .child(ui_text(label).normal_line_height());
    if disabled {
        button.opacity(0.55).cursor(CursorStyle::Arrow)
    } else {
        button.cursor_pointer()
    }
}

/// `.fork-failure-card`: fills the card below the retained head, over an opaque
/// `#181a1e` surface so the terminal cannot show through. The latest
/// `retry_error` replaces the stale reason on the single frozen reason line.
fn fork_failure_body(
    title: &'static str,
    reason: Option<&str>,
    retry_error: Option<&str>,
    retry: AnyElement,
    remove: AnyElement,
) -> Div {
    div()
        .flex_1()
        .min_h_0()
        .w_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(14.))
        .p(px(28.))
        .bg(rgb(0x181a1e))
        .text_color(rgb(0xd9dce2))
        .text_center()
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .child(
                    div()
                        .text_size(px(14.))
                        .font_weight(FontWeight::BOLD)
                        .child(title),
                )
                .children(
                    reason
                        .into_iter()
                        .chain(retry_error.filter(|error| Some(*error) != reason))
                        .map(|error| {
                            div()
                                .max_w(px(520.))
                                .mt(px(8.))
                                .text_color(rgb(0xf08a80))
                                .text_size(px(12.))
                                .line_height(relative(1.5))
                                .child(ui_text(error.to_owned()))
                        }),
                ),
        )
        .child(
            div()
                .flex()
                .justify_center()
                .gap(px(8.))
                .child(retry)
                .child(remove),
        )
}

/// `.fork-failure-actions button`; `blocked_reason` disables with a frozen
/// `CardTooltip`, `pending` only dims while the retry is in flight.
fn fork_failure_button(
    id: SharedString,
    label: &'static str,
    blocked_reason: Option<&'static str>,
    pending: bool,
) -> Stateful<Div> {
    let button = div()
        .id(id)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .h(px(30.))
        .px(px(13.))
        .border_1()
        .border_color(rgb(0x434955))
        .rounded(px(7.))
        .text_color(rgb(0xd9dce2))
        .bg(rgb(0x282b31))
        .text_size(px(16.))
        .whitespace_nowrap()
        .child(ui_text(label).normal_line_height());
    if let Some(reason) = blocked_reason {
        button
            .opacity(0.55)
            .cursor(CursorStyle::OperationNotAllowed)
            .tooltip(move |_, cx| cx.new(|_| CardTooltip(reason.into())).into())
    } else if pending {
        button.opacity(0.55).cursor(CursorStyle::Arrow)
    } else {
        button.cursor_pointer()
    }
}

/// `.fork-progress-overlay` and its interactive variant for provider prompts.
fn fork_progress_body(progress: ForkProgressCopy, now: f64) -> Stateful<Div> {
    let fill = progress.fraction.unwrap_or(0.);
    let animate = !reduced_motion();
    let spinner_size = if progress.interactive { 12. } else { 20. };
    let spinner = div()
        .flex_none()
        .size(px(spinner_size))
        .rounded_full()
        .border_2()
        .border_color(rgba(0x7eb1f247))
        .relative()
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    if !bounds.intersects(&window.content_mask().bounds) {
                        return;
                    }
                    let rotation = if animate {
                        (now as f32 / 800.).fract() * std::f32::consts::TAU
                    } else {
                        0.
                    };
                    let radius = px(spinner_size / 2. - 1.);
                    let at = |angle: f32| {
                        bounds.center() + point(radius * angle.cos(), radius * angle.sin())
                    };
                    let start = rotation - 3. * std::f32::consts::FRAC_PI_4;
                    let mut arc = PathBuilder::stroke(px(2.));
                    arc.move_to(at(start));
                    arc.arc_to(
                        point(radius, radius),
                        px(0.),
                        false,
                        true,
                        at(start + std::f32::consts::FRAC_PI_2),
                    );
                    if let Ok(arc) = arc.build() {
                        window.paint_path(arc, rgb(0x7eb1f2));
                    }
                    if animate {
                        window.request_animation_frame();
                    }
                },
            )
            .absolute()
            .top(px(-2.))
            .left(px(-2.))
            .size(px(spinner_size)),
        );
    let content = div()
        .flex()
        .items_center()
        .gap(px(9.))
        .border_1()
        .border_color(rgba(0x78abef57))
        .rounded(px(12.))
        .text_color(rgb(0xe7eef9))
        .bg(rgba(0x1d222bf5))
        .shadow(vec![shadow(0x000000, 0.34, 16., 44.)])
        .text_center()
        .child(spinner)
        .child(
            div()
                .text_size(px(if progress.interactive { 11. } else { 16. }))
                .font_weight(FontWeight::BOLD)
                .child(progress.label),
        );
    if progress.interactive {
        return div()
            .id("fork-progress")
            .absolute()
            .right(px(12.))
            .bottom(px(12.))
            .child(content.max_w(px(260.)).px(px(12.)).py(px(8.)));
    }
    let detail = |label: SharedString| {
        div()
            .text_color(rgb(0xaebdd1))
            .text_size(px(11.))
            .line_height(relative(1.5))
            .child(ui_text(label))
    };
    let content = content
        .flex_col()
        .w_full()
        .max_w(px(420.))
        .p(px(22.))
        .child(detail(progress.description.into()))
        .children(progress.fraction.map(|_| {
            div()
                .w_full()
                .max_w(px(280.))
                .h(px(4.))
                .overflow_hidden()
                .rounded_full()
                .bg(rgba(0x7eb1f229))
                .child(
                    div()
                        .h_full()
                        .w(relative(fill))
                        .rounded_full()
                        .bg(rgb(0x7eb1f2)),
                )
        }))
        .children(progress.counter.map(|counter| detail(counter.into())));
    div()
        .id("fork-progress")
        .absolute()
        .inset_0()
        .occlude()
        .flex()
        .items_center()
        .justify_center()
        .p(px(24.))
        .bg(rgba(0x111214c7))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(content)
}

/// `.fork-readiness-toast` (frozen light-surface variant): fixed below the card
/// head, horizontally centered, max 420px. The host owns the 2.8s dismissal.
fn fork_readiness_toast(message: &'static str) -> Div {
    div()
        .absolute()
        .top(px(72.))
        .left_0()
        .right_0()
        .flex()
        .justify_center()
        .child(
            div()
                .max_w(px(420.))
                .mx(px(16.))
                .px(px(14.))
                .py(px(9.))
                .border_1()
                .border_color(rgb(0xbdd7f6))
                .rounded(px(8.))
                .text_color(rgb(0x365a83))
                .bg(rgb(0xf3f8ff))
                .shadow(vec![shadow(0x314660, 0.18, 10., 28.)])
                .text_size(px(12.))
                .line_height(relative(1.45))
                .child(ui_text(message)),
        )
}
