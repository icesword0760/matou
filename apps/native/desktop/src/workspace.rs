use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
    sync::mpsc::Sender,
    time::{Duration, Instant},
};

use gpui::{prelude::*, *};
use matou_native_core::geometry::GeometryUpdate;
use matou_native_core::hierarchy::{HierarchyResult, Scene, Task, Workspace as HierarchyWorkspace};
use matou_native_core::workspace_paths::WorkspacePathState;
use serde_json::Value;

use crate::{
    carousel_motion::{
        Curve, Transition, centered_scroll, focus_scroll_step, fully_visible_scroll,
    },
    rename_input::{RenameInput, RenameInputEvent, trim_name},
    runtime_bridge::{Command, Event, RecoveryState},
    sidebar_scrollbar::{self, SidebarScrollbarState},
    tab_dialog_style as tabs_ui,
    terminal_surface::{SurfaceEvent, TerminalSurface},
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

struct WorkspaceDragMotion {
    scale: Transition,
    opacity: Transition,
    background: Transition,
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
    "account",
];

pub struct Workspace {
    commands: Sender<Command>,
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
    task_toast: Option<String>,
    toast_generation: u64,
    tab_scroll: ScrollHandle,
    tab_hover: Option<String>,
    tab_overflow: bool,
    hidden_tabs: Option<Vec<(String, FocusHandle)>>,
    tab_menu: Option<(String, Point<Pixels>, FocusHandle)>,
    control_focus: HashMap<&'static str, FocusHandle>,
    tab_controls: HashMap<String, [FocusHandle; 2]>,
    card_controls: HashMap<String, FocusHandle>,
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
    restore_geometry: bool,
    column_capacity: Option<u32>,
    pending_geometry: Option<GeometryUpdate>,
    geometry_timer: u64,
    last_geometry_write: HashMap<String, f64>,
    geometry_sequence: u64,
    geometry_writes: HashMap<String, (u64, u8)>,
    graph: Value,
    terminals: HashMap<String, Entity<TerminalSurface>>,
    recovery: HashMap<String, RecoveryState>,
    recovery_controls: HashMap<String, FocusHandle>,
    order: Vec<String>,
    focused: Option<String>,
    pending_focus: Option<(String, String)>,
    resize_revisions: HashMap<String, u64>,
    fault: Option<String>,
    subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(
        commands: Sender<Command>,
        events: async_channel::Receiver<Event>,
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
            task_toast: None,
            toast_generation: 0,
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
            restore_geometry: false,
            column_capacity: None,
            pending_geometry: None,
            geometry_timer: 0,
            last_geometry_write: HashMap::new(),
            geometry_sequence: 0,
            geometry_writes: HashMap::new(),
            graph: Value::Null,
            terminals: HashMap::new(),
            recovery: HashMap::new(),
            recovery_controls: HashMap::new(),
            order: Vec::new(),
            focused: None,
            pending_focus: None,
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
            self.pending_focus = Some((scene_id.clone(), session_id.clone()));
        }
        if self.commands.send(command).is_err() {
            self.pending_focus = None;
            self.fault = Some("原生运行时已停止".into());
            cx.notify();
        }
    }

    fn apply(&mut self, event: Event, window: &mut Window, cx: &mut Context<Self>) {
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
                if scene_changed
                    || self.pending_focus.as_ref().is_some_and(|(scene, session)| {
                        Some(scene) == next_scene && Some(session) == next_focus.as_ref()
                    })
                {
                    self.pending_focus = None;
                }
                if scene_changed {
                    // A newly mounted source carousel starts at its current styles.
                    for saved in geometry {
                        if !self.geometry_dirty.contains(&saved.owner_key) {
                            self.geometry.insert(saved.owner_key.clone(), saved);
                        }
                    }
                    self.restore_geometry = true;
                    self.column_capacity = None;
                    self.stop_edge_browse(false);
                    self.pointer_position = None;
                    self.card_motion.clear();
                    self.hovered = None;
                    self.hover_follow_until = None;
                    self.hover_baseline = None;
                    self.hover_restore_at = None;
                    self.carousel_scroll.set_offset(point(px(0.), px(0.)));
                }
                if !scene_changed && focus_changed {
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
                self.focused = next_focus;
                self.hierarchy = Some(hierarchy);
                self.graph = graph;
                self.refresh_terminals(cx);
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
                if (initializing || scene_changed || focus_changed)
                    && self.dialog.is_none()
                    && self.active_path_valid()
                    && self
                        .focused
                        .as_ref()
                        .is_none_or(|id| !self.recovery_blocked(id))
                    && (window.focused(cx).is_none()
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
                    && let Some(terminal) =
                        self.focused.as_ref().and_then(|id| self.terminals.get(id))
                {
                    window.focus(&terminal.read(cx).focus);
                }
            }
            Event::ScenesDisposed(ids) => {
                for id in ids {
                    self.terminals.remove(&id);
                    self.recovery.remove(&id);
                    self.recovery_controls.remove(&id);
                    self.card_controls.remove(&id);
                    self.card_motion.remove(&id);
                    self.resize_revisions.remove(&id);
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
                    let timer = cx.background_executor().timer(Duration::from_millis(120));
                    cx.spawn(async move |this, cx| {
                        timer.await;
                        let _ = this.update(cx, |this, cx| {
                            if this
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
                if attempted_focus.is_some() && self.pending_focus == attempted_focus {
                    self.pending_focus = None;
                }
                self.fault = Some(message);
            }
            Event::Graph(graph) => {
                self.graph = graph;
                self.refresh_terminals(cx);
                self.sync_terminal_input(window, cx);
            }
            Event::Frame {
                session_id,
                screen,
                modes,
                sequence,
            } => {
                if let Some(terminal) = self.terminals.get(&session_id) {
                    terminal.update(cx, |terminal, cx| terminal.set_screen(screen, modes, cx));
                    // The reference live-output callback restores only an unclaimed
                    // active terminal; dialogs and focused controls retain ownership.
                    if self.focused.as_ref() == Some(&session_id)
                        && self.dialog.is_none()
                        && self.active_path_valid()
                        && !self.recovery_blocked(&session_id)
                        && window.focused(cx).is_none()
                    {
                        window.focus(&terminal.read(cx).focus);
                    }
                }
                self.send(
                    Command::Applied {
                        session_id,
                        sequence,
                    },
                    cx,
                );
            }
            Event::Recovery { session_id, state } => {
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
                if !matches!(state, RecoveryState::Ready)
                    && self
                        .terminals
                        .get(&session_id)
                        .is_some_and(|terminal| terminal.read(cx).focus.is_focused(window))
                {
                    window.blur();
                }
                self.recovery.insert(session_id, state);
                self.sync_terminal_input(window, cx);
            }
            Event::StorageFault { message, .. } | Event::Diagnostic(message) => {
                self.fault = Some(message)
            }
            Event::StorageRecovered { .. } => self.fault = None,
            Event::Closed => {}
        }
        cx.notify();
    }

    fn refresh_terminals(&mut self, cx: &mut Context<Self>) {
        let Some(nodes) = self.graph["nodes"].as_array() else {
            return;
        };
        self.order = nodes
            .iter()
            .filter_map(|node| node["sessionId"].as_str().map(str::to_owned))
            .collect();
        let now = self.motion_clock.elapsed().as_secs_f64() * 1000.;
        for id in &self.order {
            let selected = if self.focused.as_ref() == Some(id) {
                1.
            } else {
                0.
            };
            let motion = self
                .card_motion
                .entry(id.clone())
                .or_insert_with(|| CardMotion {
                    width: Transition::new(selected),
                    lift: Transition::new(selected),
                });
            motion.lift.retarget(selected, now, 180., Curve::Lift);
            if self.terminals.contains_key(id) {
                continue;
            }
            let disabled = !self.active_path_valid();
            let terminal = cx.new(|cx| {
                let mut surface = TerminalSurface::new(cx);
                surface.set_input_disabled(disabled, cx);
                surface
            });
            self.card_controls
                .insert(id.clone(), cx.focus_handle().tab_stop(true));
            let session_id = id.clone();
            self.subscriptions.push(cx.subscribe(
                &terminal,
                move |this, _, event: &SurfaceEvent, cx| match event {
                    SurfaceEvent::Input(bytes) => this.send(
                        Command::Input {
                            session_id: session_id.clone(),
                            bytes: bytes.clone(),
                            defer_ordering: true,
                        },
                        cx,
                    ),
                    SurfaceEvent::Scroll(lines) => this.send(
                        Command::Scroll {
                            session_id: session_id.clone(),
                            lines: *lines,
                        },
                        cx,
                    ),
                    SurfaceEvent::Resize {
                        cols,
                        rows,
                        cell_width_px,
                        cell_height_px,
                    } => {
                        let revision = this.resize_revisions.entry(session_id.clone()).or_default();
                        *revision += 1;
                        let revision = *revision;
                        let id = session_id.clone();
                        let geometry = (*cols, *rows, *cell_width_px, *cell_height_px);
                        let timer = cx.background_executor().timer(Duration::from_millis(80));
                        cx.spawn(async move |this, cx| {
                            timer.await;
                            let _ = this.update(cx, |this, cx| {
                                if this.resize_revisions.get(&id) != Some(&revision) {
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
                },
            ));
            self.terminals.insert(id.clone(), terminal);
        }
    }

    pub fn flush_geometry(&mut self, cx: &mut Context<Self>) {
        self.geometry_timer += 1;
        if let Some(update) = self.pending_geometry.take() {
            self.last_geometry_write.insert(
                update.scene_id.clone(),
                self.motion_clock.elapsed().as_secs_f64() * 1000.,
            );
            self.write_geometry(update, 0, cx);
        }
    }

    fn write_geometry(&mut self, update: GeometryUpdate, attempt: u8, cx: &mut Context<Self>) {
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
        let owner_key = format!("session-group:{}:root", scene.id);
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
            .get(&scene.id)
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
                if this.geometry_timer == generation {
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
            .filter(|id| !narrow || self.focused.as_ref() == Some(id))
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
        self.capture_geometry(self.geometry_focus().map(str::to_owned), true, window, cx);
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
        // A removed recovery button no longer owns focus in the source DOM.
        if self
            .workspace_controls
            .iter()
            .any(|(id, controls)| !self.workspace_invalid(id) && controls[2].is_focused(window))
        {
            window.blur();
        }
        for (id, terminal) in &self.terminals {
            let disabled = disabled || self.recovery_blocked(id);
            terminal.update(cx, |terminal, cx| {
                terminal.set_input_disabled(disabled, cx);
            });
        }
    }

    fn recovery_blocked(&self, session_id: &str) -> bool {
        self.recovery
            .get(session_id)
            .is_some_and(|state| !matches!(state, RecoveryState::Ready))
    }

    fn workspace_invalid(&self, workspace_id: &str) -> bool {
        self.workspace_paths
            .iter()
            .any(|state| state.workspace_id == workspace_id && state.status == "invalid")
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
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = picker.await else {
                return;
            };
            let Some(path) = paths.pop() else {
                return;
            };
            let root_directory = path.to_string_lossy().into_owned();
            let _ = this.update(cx, |this, cx| {
                this.creating_workspace = true;
                this.send(Command::CreateWorkspace { root_directory }, cx);
                cx.notify();
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
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = picker.await else {
                return;
            };
            let Some(path) = paths.pop() else {
                return;
            };
            let root_directory = path.to_string_lossy().into_owned();
            let _ = this.update(cx, |this, cx| {
                this.send(
                    Command::RelinkWorkspace {
                        workspace_id,
                        root_directory,
                    },
                    cx,
                );
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
            .w(px(260.))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(rgba(0x58718b1f))
            .bg(rgba(0xfbfdff17))
            .child(
                div()
                    .h(px(48.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .pl(px(78.))
                    .pr(px(12.))
                    .gap(px(8.))
                    .child(new_workspace)
                    .child(div().flex_1())
                    .child(
                        self.control(div().child(icon("bell", 16., 0x8d96a3)), "notifications")
                            .flex_none(),
                    ),
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
        self.dialog = None;
        self.dialog_subscription = None;
        self.dialog_controls.clear();
        window.blur();
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
            None => return,
        };
        self.close_dialog(window, cx);
        if let Some(command) = command {
            self.send(command, cx);
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
        if !self.active_path_valid() {
            return;
        }
        if let (Some(scene_id), Some(source_session_id)) =
            (self.active_scene_id(), self.focused.as_ref())
        {
            self.send(
                Command::CreateShellSibling {
                    scene_id: scene_id.to_owned(),
                    source_session_id: source_session_id.clone(),
                    parent_session_id: None,
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
        if self.order.len() <= 1 {
            return;
        }
        let Some(scene_id) = self.active_scene_id() else {
            return;
        };
        let current = self
            .order
            .iter()
            .position(|id| Some(id) == self.focused.as_ref())
            .unwrap_or(0);
        let next = (current as isize + offset).rem_euclid(self.order.len() as isize) as usize;
        self.send(
            Command::FocusSession {
                scene_id: scene_id.to_owned(),
                session_id: self.order[next].clone(),
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

    fn tab_overlay(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if let Some(dialog) = &self.dialog {
            let (title, body, confirm, show_cancel, disabled) = match dialog {
                TabDialog::RenameTask { task_id, input, empty_submitted, failure } => {
                    let value = trim_name(&input.read(cx).text);
                    let error = if *empty_submitted && value.is_empty() { Some("工作台名称不能为空".into()) }
                        else { self.task_rename_error(task_id, value).or_else(|| failure.as_ref().filter(|(title, _)| title == value).map(|(_, message)| message.clone())) };
                    let mut body = tabs_ui::dialog_body().child(input.clone());
                    if let Some(error) = &error { body = body.child(tabs_ui::dialog_error(error.clone())); }
                    ("重命名", body, "确定", true, error.is_some() && !value.is_empty())
                }
                TabDialog::RemoveWorkspace { name, .. } => (
                    "移出工作空间",
                    tabs_ui::dialog_body().child(ui_text(format!(
                        "移出 \"{name}\" 会关闭该空间下的事项和终端会话，本地文件保持原样。 是否继续？"
                    )).pre_line()),
                    "移出",
                    true,
                    false,
                ),
                TabDialog::DeleteTask { title, .. } => (
                    "提示", tabs_ui::dialog_body().child(ui_text(format!("删除 \"{title}\" 会丢失该事项下所有终端会话，但不会删除本地目录。 是否继续？")).pre_line()), "确定", true, false,
                ),
                TabDialog::Rename { scene_id, input } => {
                    let value = trim_name(&input.read(cx).text);
                    let error = self.rename_error(scene_id, value);
                    let mut body = tabs_ui::dialog_body().child(input.clone());
                    if let Some(error) = &error {
                        body = body.child(tabs_ui::dialog_error(error.clone()));
                    }
                    (
                        "重命名标签页",
                        body,
                        "确定",
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
                        "当前已是最后一个事项下的最后一个标签，这里点击关闭不会删除该事项。\n\n如需删除该工作区，请在左侧事项面板的下拉菜单中执行删除。".into()
                    } else {
                        format!(
                            "关闭后，“{name}”下的 {count} 个会话会全部从界面移除。{activity}项目文件和工作树保持原样。"
                        )
                    };
                    (
                        if *is_last { "提示" } else { "关闭画布" },
                        tabs_ui::dialog_body().child(ui_text(text).pre_line()),
                        if *is_last {
                            "我知道了"
                        } else {
                            "关闭画布"
                        },
                        !*is_last,
                        false,
                    )
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
                                    .child(tabs_ui::dialog_title(title))
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

    fn footer(&self) -> Div {
        let recovering = self
            .focused
            .as_ref()
            .is_some_and(|id| self.recovery_blocked(id));
        let cwd = self
            .focused
            .as_ref()
            .and_then(|id| {
                self.graph["nodes"]
                    .as_array()?
                    .iter()
                    .find(|n| n["sessionId"] == *id)
            })
            .and_then(|n| n["cwd"].as_str())
            .unwrap_or("");
        div()
            .h(px(38.))
            .flex_none()
            .flex()
            .items_center()
            .px(px(20.))
            .gap(px(8.))
            .border_t_1()
            .border_color(rgb(0xe8e8e8))
            .bg(rgb(0xffffff))
            .text_size(px(10.))
            .text_color(rgb(0x606572))
            .child(
                div()
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("根会话"),
            )
            .child(
                div()
                    .text_color(rgb(0x999999))
                    .child(format!("· {} 个会话", self.order.len())),
            )
            .when(!recovering, |footer| {
                footer.child(
                    div()
                        .ml(px(8.))
                        .pl(px(12.))
                        .border_l_1()
                        .border_color(rgb(0xe8e8e8))
                        .child("zsh"),
                )
            })
            .child(div().flex_1())
            .when(!recovering, |footer| {
                footer.child(
                    div()
                        .max_w(px(160.))
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(
                            cwd.rsplit(['/', '\\'])
                                .find(|s| !s.is_empty())
                                .map(|s| format!("~/{s}"))
                                .unwrap_or_default(),
                        ),
                )
            })
            .child(
                self.control(div(), "account")
                    .ml(px(14.))
                    .text_color(rgb(0x60758b))
                    .child("Claude 账户"),
            )
            .child(icon("chevron-down", 13., 0x60758b))
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                self.focused.as_ref() == Some(id) || (!narrow && self.hovered.as_ref() == Some(id));
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
            if self.focused.as_ref() == Some(id) {
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
                .hierarchy
                .as_ref()
                .and_then(|h| h.scene.as_ref())
                .and_then(|s| self.geometry.get(&format!("session-group:{}:root", s.id)))
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
        {
            let scroll =
                f64::from(-f32::from(self.carousel_scroll.offset().x)).clamp(0., max_scroll);
            self.save_geometry(
                scroll,
                active_geometry.map(|(left, _)| left),
                self.focused.clone(),
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
                    this.stop_edge_browse(false);
                    this.hover_card(None, cx);
                } else {
                    this.edge_blocked = false;
                    this.edge_intent(window.mouse_position(), window, cx);
                }
            }));
        let path_valid = self.active_path_valid();
        for id in &self.order {
            let Some(terminal) = self.terminals.get(id) else {
                continue;
            };
            let title = self.graph["nodes"]
                .as_array()
                .and_then(|nodes| nodes.iter().find(|node| node["sessionId"] == *id))
                .and_then(|node| node["title"].as_str())
                .unwrap_or("Shell");
            let focus = terminal.read(cx).focus.clone();
            let session_id = id.clone();
            let hover_id = id.clone();
            let scene_id = self
                .hierarchy
                .as_ref()
                .and_then(|h| h.scene.as_ref())
                .map(|s| s.id.clone());
            let selected = self.focused.as_ref() == Some(id);
            let width = widths[id] as f32;
            let lift = -4. * self.card_motion[id].lift.value(now) as f32;
            let card = div()
                .id(SharedString::from(format!("card-{id}")))
                .w(px(width))
                .h_full()
                .flex_none()
                .relative()
                .top(px(lift))
                .rounded(px(12.))
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
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
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
            let card = if let Some(RecoveryState::Failed(error)) = self.recovery.get(id) {
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
            } else if narrow && !selected {
                let node = self.graph["nodes"]
                    .as_array()
                    .and_then(|nodes| nodes.iter().find(|node| node["sessionId"] == *id));
                let status = node
                    .and_then(|node| node["workStatus"].as_str())
                    .unwrap_or("idle");
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
                card.child(
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
                        .child(
                            div()
                                .text_size(px(9.))
                                .font_family("Menlo")
                                .line_height(relative(1.5))
                                .overflow_hidden()
                                .child(summary.to_owned()),
                        ),
                )
            } else {
                card.child(
                    div()
                        .h(px(42.))
                        .flex_none()
                        .rounded_t(px(12.))
                        .px(px(12.))
                        .flex()
                        .items_center()
                        .bg(rgb(0xffffff))
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::BOLD)
                                .child(title.to_owned()),
                        )
                        .child(div().flex_1())
                        .child(self.focus_control(
                            div().child(icon("folder-input", 16., 0x687d92)),
                            &self.card_controls[id],
                        )),
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
                            .child("工作区目录不可用，请先在本地恢复原路径，或移出该工作区"),
                    )
                })
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .pl(px(14.))
                        .pr(px(28.))
                        .py(px(12.))
                        .child(terminal.clone()),
                )
            };
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
        let carousel = div()
            .flex_1()
            .min_h_0()
            .relative()
            .flex()
            .child(carousel)
            .child(indicator);
        let overlay = self.tab_overlay(window, cx);
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
                    .child(self.footer()),
            )
            .child(
                canvas(
                    |_, _, _| (),
                    move |_, _, window, cx| {
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
            .children(toast)
            .children(overlay)
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
