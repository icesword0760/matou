use std::{
    collections::{HashMap, HashSet},
    sync::mpsc::Sender,
    time::{Duration, Instant},
};

use gpui::{prelude::*, *};
use matou_native_core::geometry::GeometryUpdate;
use matou_native_core::hierarchy::{HierarchyResult, Scene};
use serde_json::Value;

use crate::{
    carousel_motion::{
        Curve, Transition, centered_scroll, focus_scroll_step, fully_visible_scroll,
    },
    rename_input::{RenameInput, RenameInputEvent, trim_name},
    runtime_bridge::{Command, Event},
    tab_dialog_style as tabs_ui,
    terminal_surface::{SurfaceEvent, TerminalSurface},
};

#[cfg(debug_assertions)]
type MotionTrace = std::rc::Rc<std::cell::RefCell<std::fs::File>>;

struct CardMotion {
    width: Transition,
    lift: Transition,
}

enum TabDialog {
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

pub struct Workspace {
    commands: Sender<Command>,
    hierarchy: Option<HierarchyResult>,
    scenes: Vec<Scene>,
    tab_scroll: ScrollHandle,
    tab_hover: Option<String>,
    tab_overflow: bool,
    hidden_tabs: Option<Vec<String>>,
    tab_menu: Option<(String, Point<Pixels>)>,
    dialog: Option<TabDialog>,
    dialog_controls: Vec<FocusHandle>,
    dialog_subscription: Option<Subscription>,
    dialog_return_focus: Option<FocusHandle>,
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
        Self {
            commands,
            hierarchy: None,
            scenes: Vec::new(),
            tab_scroll: ScrollHandle::new(),
            tab_hover: None,
            tab_overflow: false,
            hidden_tabs: None,
            tab_menu: None,
            dialog: None,
            dialog_controls: Vec::new(),
            dialog_subscription: None,
            dialog_return_focus: None,
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
            order: Vec::new(),
            focused: None,
            pending_focus: None,
            resize_revisions: HashMap::new(),
            fault: None,
            subscriptions: Vec::new(),
        }
    }

    fn send(&mut self, command: Command, cx: &mut Context<Self>) {
        if matches!(
            &command,
            Command::SelectScene { .. }
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
                let next_scene = hierarchy.scene.as_ref().map(|scene| &scene.id);
                let scene_changed = self
                    .hierarchy
                    .as_ref()
                    .and_then(|h| h.scene.as_ref())
                    .map(|s| &s.id)
                    != next_scene;
                let next_focus = hierarchy.session.as_ref().map(|session| session.id.clone());
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
                if !scene_changed && self.focused != next_focus {
                    self.restore_geometry = false;
                }
                if scene_changed || self.focused != next_focus {
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
                self.scenes = scenes;
                self.focused = next_focus;
                self.hierarchy = Some(hierarchy);
                self.graph = graph;
                self.refresh_terminals(window, cx);
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
                if self.dialog.is_none()
                    && let Some(terminal) =
                        self.focused.as_ref().and_then(|id| self.terminals.get(id))
                {
                    window.focus(&terminal.read(cx).focus);
                }
            }
            Event::ScenesDisposed(ids) => {
                for id in ids {
                    self.terminals.remove(&id);
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
                    self.dialog_return_focus = window.focused(cx);
                    self.dialog_controls = (0..if is_last { 2 } else { 3 })
                        .map(|_| cx.focus_handle())
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
                self.refresh_terminals(window, cx);
            }
            Event::Frame {
                session_id,
                screen,
                modes,
                sequence,
            } => {
                if let Some(terminal) = self.terminals.get(&session_id) {
                    terminal.update(cx, |terminal, cx| terminal.set_screen(screen, modes, cx));
                }
                self.send(
                    Command::Applied {
                        session_id,
                        sequence,
                    },
                    cx,
                );
            }
            Event::StorageFault { message, .. } | Event::Diagnostic(message) => {
                self.fault = Some(message)
            }
            Event::StorageRecovered { .. } => self.fault = None,
            Event::Closed => {}
        }
        cx.notify();
    }

    fn refresh_terminals(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
            let terminal = cx.new(TerminalSurface::new);
            let session_id = id.clone();
            self.subscriptions.push(cx.subscribe(
                &terminal,
                move |this, _, event: &SurfaceEvent, cx| match event {
                    SurfaceEvent::Input(bytes) => this.send(
                        Command::Input {
                            session_id: session_id.clone(),
                            bytes: bytes.clone(),
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
            if self.focused.as_ref() == Some(id) {
                window.focus(&terminal.read(cx).focus);
            }
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

    fn sidebar(&self) -> Div {
        let workspace = self
            .hierarchy
            .as_ref()
            .and_then(|h| h.workspace.as_ref())
            .map(|w| w.name.as_str())
            .unwrap_or("");
        let task = self
            .hierarchy
            .as_ref()
            .and_then(|h| h.task.as_ref())
            .map(|t| t.title.as_str())
            .unwrap_or("");
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
                    .pl(px(86.))
                    .pr(px(12.))
                    .gap(px(10.))
                    .child(icon("square-pen", 17., 0x2f3547))
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("新增工作空间"),
                    )
                    .child(div().flex_1())
                    .child(icon("bell", 16., 0x8d96a3)),
            )
            .child(
                div().flex_1().min_h_0().px(px(6.)).pt(px(8.)).child(
                    div()
                        .relative()
                        .mt(px(1.))
                        .mb(px(8.))
                        .child(
                            div()
                                .absolute()
                                .top(px(5.))
                                .bottom(px(5.))
                                .left_0()
                                .w(px(2.))
                                .rounded(px(1.))
                                .bg(rgb(0x67a6f4)),
                        )
                        .child(
                            div()
                                .h(px(34.))
                                .flex()
                                .items_center()
                                .pl(px(7.))
                                .gap(px(7.))
                                .child(icon("chevron-down", 14., 0x777777))
                                .child(icon("folder", 16., 0x4078f2))
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(workspace.to_owned()),
                                )
                                .child(
                                    div()
                                        .px(px(5.))
                                        .py(px(1.))
                                        .rounded(px(5.))
                                        .bg(rgb(0xeceef3))
                                        .text_color(rgb(0x91949e))
                                        .text_size(px(9.))
                                        .child("默认"),
                                )
                                .child(icon("pin", 13., 0x81848d))
                                .child(div().flex_1())
                                .child(icon("plus", 14., 0x81848d))
                                .child(
                                    div()
                                        .w(px(26.))
                                        .flex()
                                        .justify_center()
                                        .child(icon("ellipsis", 14., 0x81848d)),
                                ),
                        )
                        .child(
                            div()
                                .ml(px(17.))
                                .mr(px(1.))
                                .mt(px(2.))
                                .h(px(34.))
                                .rounded(px(8.))
                                .bg(rgba(0x57657f18))
                                .flex()
                                .items_center()
                                .px(px(8.))
                                .gap(px(8.))
                                .child(icon("task", 12., 0x2f3547))
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(task.to_owned()),
                                )
                                .child(div().flex_1())
                                .child(icon("ellipsis", 14., 0x656d76)),
                        ),
                ),
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
                    .child(icon("columns-3", 16., 0x989da7))
                    .child(div().ml(px(7.)).child("看板"))
                    .child(div().flex_1())
                    .child(icon("settings-2", 16., 0x989da7))
                    .child(div().ml(px(7.)).child("设置")),
            )
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
        if let Some(focus) = self.dialog_return_focus.take() {
            window.focus(&focus);
        }
        cx.notify();
    }
    fn open_rename(&mut self, scene_id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(scene) = self.scenes.iter().find(|s| s.id == scene_id) else {
            return;
        };
        let text = self.scene_title(scene);
        let input = cx.new(|cx| RenameInput::new(text, cx));
        self.dialog_return_focus = window.focused(cx);
        self.dialog_controls = vec![
            cx.focus_handle(),
            input.read(cx).focus.clone(),
            cx.focus_handle(),
            cx.focus_handle(),
        ];
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
        let command = match &self.dialog {
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
    fn tab_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.dialog.is_some() {
            let composing = matches!(&self.dialog,Some(TabDialog::Rename{input,..}) if input.read(cx).composing());
            if e.keystroke.key == "escape" && !composing {
                self.close_dialog(window, cx);
                cx.stop_propagation();
                return;
            }
            if e.keystroke.key == "tab" && !composing {
                let disabled = matches!(&self.dialog,Some(TabDialog::Rename{scene_id,input}) if !trim_name(&input.read(cx).text).is_empty() && self.rename_error(scene_id,trim_name(&input.read(cx).text)).is_some());
                let n = self.dialog_controls.len() - usize::from(disabled);
                let i = self
                    .dialog_controls
                    .iter()
                    .position(|f| f.is_focused(window))
                    .unwrap_or(0);
                let next = if e.keystroke.modifiers.shift {
                    (i + n - 1) % n
                } else {
                    (i + 1) % n
                };
                window.focus(&self.dialog_controls[next]);
                cx.stop_propagation();
                return;
            }
            if matches!(e.keystroke.key.as_str(), "enter" | "space") && !composing {
                let i = self
                    .dialog_controls
                    .iter()
                    .position(|f| f.is_focused(window))
                    .unwrap_or(0);
                if matches!(&self.dialog, Some(TabDialog::Rename { .. })) && i == 1 {
                    return;
                }
                if i + 1 == self.dialog_controls.len() {
                    self.confirm_dialog(window, cx);
                } else {
                    self.close_dialog(window, cx);
                }
                cx.stop_propagation();
            }
            return;
        }
        if e.keystroke.key == "escape" && (self.tab_menu.is_some() || self.hidden_tabs.is_some()) {
            self.tab_menu = None;
            self.hidden_tabs = None;
            cx.stop_propagation();
            cx.notify();
        }
    }
    fn topbar(&self, cx: &mut Context<Self>) -> Div {
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
                            this.tab_menu = Some((
                                menu_id.clone(),
                                point(
                                    e.position.x.clamp(px(8.), size.width - px(136.)),
                                    e.position.y.clamp(px(8.), size.height - px(44.)),
                                ),
                            ));
                            this.hidden_tabs = None;
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
                    .child(
                        tabs_ui::tab_title(title)
                            .id(SharedString::from(format!("tab-title-{}", scene.id)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                                    if e.click_count == 2 {
                                        this.open_rename(rename_id.clone(), window, cx);
                                    }
                                    cx.stop_propagation();
                                }),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.send(
                                    Command::SelectScene {
                                        scene_id: select_id.clone(),
                                    },
                                    cx,
                                );
                            })),
                    )
                    .child(
                        tabs_ui::tab_close(
                            SharedString::from(format!("tab-close-{}", scene.id)),
                            active || self.tab_hover.as_ref() == Some(&scene.id),
                        )
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.pending_close = Some(close_id.clone());
                            this.send(
                                Command::PreviewSceneClose {
                                    scene_id: close_id.clone(),
                                },
                                cx,
                            );
                            cx.stop_propagation();
                        })),
                    ),
            );
        }
        let task = self
            .hierarchy
            .as_ref()
            .and_then(|h| h.task.as_ref())
            .map(|t| t.id.clone());
        let add = |id: &'static str| {
            tabs_ui::tab_add(id)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener({
                    let task = task.clone();
                    move |this, _, _, cx| {
                        if let Some(task_id) = &task {
                            this.send(
                                Command::CreateCanvas {
                                    task_id: task_id.clone(),
                                },
                                cx,
                            );
                        }
                    }
                }))
        };
        if !self.tab_overflow {
            tabs = tabs.child(add("create-canvas"));
        }
        let overflow_actions = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(px(4.))
            .ml(px(8.))
            .child(
                tabs_ui::tab_overflow_btn("tab-overflow")
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.hidden_tabs.is_some() {
                            this.hidden_tabs = None;
                        } else {
                            let viewport = this.tab_scroll.bounds();
                            let offset = this.tab_scroll.offset();
                            this.hidden_tabs = Some(
                                this.scenes
                                    .iter()
                                    .enumerate()
                                    .filter(|(i, _)| {
                                        this.tab_scroll.bounds_for_item(*i).is_none_or(|b| {
                                            b.left() + offset.x < viewport.left() - px(1.)
                                                || b.right() + offset.x > viewport.right() + px(1.)
                                        })
                                    })
                                    .map(|(_, s)| s.id.clone())
                                    .collect(),
                            );
                        }
                        this.tab_menu = None;
                        cx.notify();
                    })),
            )
            .child(add("create-canvas-overflow"));
        let scene = active_id.cloned();
        let session = self.focused.clone();
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
                div()
                    .size(px(28.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon("graph-ring", 16., 0x606572)),
            )
            .child(
                div()
                    .id("create-shell")
                    .size(px(28.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let (Some(scene_id), Some(source_session_id)) = (&scene, &session) {
                            this.send(
                                Command::CreateShellSibling {
                                    scene_id: scene_id.clone(),
                                    source_session_id: source_session_id.clone(),
                                    parent_session_id: None,
                                },
                                cx,
                            );
                        }
                    }))
                    .child(icon("panel-right-open", 16., 0x606572)),
            )
            .child(
                div()
                    .size(px(30.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon("upload", 16., 0x747b88).with_transformation(
                        Transformation::rotate(Radians(std::f32::consts::PI)),
                    )),
            )
    }

    fn tab_overlay(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if let Some(dialog) = &self.dialog {
            let (title, body, confirm, show_cancel, disabled) = match dialog {
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
                        tabs_ui::dialog_body().child(text),
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
                footer = footer.child(
                    tabs_ui::dialog_button("dialog-cancel", "取消", false, false)
                        .track_focus(&self.dialog_controls[n - 2])
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, window, _| {
                                window.focus(&this.dialog_controls[n - 2])
                            }),
                        )
                        .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx))),
                );
            }
            footer = footer.child(
                tabs_ui::dialog_button("dialog-confirm", confirm, true, disabled)
                    .track_focus(&self.dialog_controls[n - 1])
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, _| {
                            if !disabled {
                                window.focus(&this.dialog_controls[n - 1]);
                            }
                        }),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if !disabled {
                            this.confirm_dialog(window, cx);
                        }
                    })),
            );
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
                                    .child(
                                        tabs_ui::dialog_close("dialog-close")
                                            .track_focus(&self.dialog_controls[0])
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(|this, _, window, _| {
                                                    window.focus(&this.dialog_controls[0])
                                                }),
                                            )
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.close_dialog(window, cx)
                                            })),
                                    ),
                            )
                            .child(body)
                            .child(footer),
                    )
                    .into_any_element(),
            );
        }
        if let Some((id, position)) = &self.tab_menu {
            let id = id.clone();
            return Some(
                tabs_ui::context_menu("tab-context-menu")
                    .left(position.x)
                    .top(position.y)
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.tab_menu = None;
                        cx.notify();
                    }))
                    .child(
                        tabs_ui::menu_item("tab-rename-menu", "重命名页签").on_click(cx.listener(
                            move |this, _, window, cx| this.open_rename(id.clone(), window, cx),
                        )),
                    )
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
                .filter_map(|id| self.scenes.iter().find(|s| &s.id == id))
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
            for id in ids {
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
                    tabs_ui::overflow_item(SharedString::from(format!("hidden-{id}")), active)
                        .child(tabs_ui::overflow_title(self.scene_title(scene), active))
                        .when(active, |item| item.child(tabs_ui::overflow_check()))
                        .on_click(cx.listener(move |this, _, _, cx| {
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
                        })),
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
            .child(
                div()
                    .ml(px(8.))
                    .pl(px(12.))
                    .border_l_1()
                    .border_color(rgb(0xe8e8e8))
                    .child("zsh"),
            )
            .child(div().flex_1())
            .child(
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
            .child(
                div()
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
                        window.focus(&focus);
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
            let card = if narrow && !selected {
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
                        .child(icon("folder-input", 16., 0x687d92)),
                )
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
        let geometry_view = cx.entity().downgrade();
        div()
            .id("workspace-root")
            .relative()
            .capture_key_down(cx.listener(Self::tab_key))
            .size_full()
            .flex()
            .font_family(".AppleSystemUIFont")
            .text_color(rgb(0x303744))
            .child(self.sidebar())
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(self.topbar(cx))
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
                            // Native scroll bounds are ready only after this frame's layout.
                            let _ = window;
                        });
                    },
                )
                .absolute()
                .size(px(0.)),
            )
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
