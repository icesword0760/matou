use std::{collections::HashMap, sync::mpsc::Sender, time::Duration};

use gpui::{prelude::*, *};
use matou_native_core::hierarchy::{HierarchyResult, Scene};
use serde_json::Value;

use crate::{
    runtime_bridge::{Command, Event},
    terminal_surface::{SurfaceEvent, TerminalSurface},
};

pub struct Workspace {
    commands: Sender<Command>,
    hierarchy: Option<HierarchyResult>,
    scenes: Vec<Scene>,
    tab_scroll: ScrollHandle,
    carousel_scroll: ScrollHandle,
    graph: Value,
    terminals: HashMap<String, Entity<TerminalSurface>>,
    order: Vec<String>,
    focused: Option<String>,
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
            carousel_scroll: ScrollHandle::new(),
            graph: Value::Null,
            terminals: HashMap::new(),
            order: Vec::new(),
            focused: None,
            resize_revisions: HashMap::new(),
            fault: None,
            subscriptions: Vec::new(),
        }
    }

    fn send(&mut self, command: Command, cx: &mut Context<Self>) {
        if self.commands.send(command).is_err() {
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
            }
            | Event::Navigation {
                hierarchy,
                scenes,
                graph,
            } => {
                self.scenes = scenes;
                self.focused = hierarchy.session.as_ref().map(|session| session.id.clone());
                self.hierarchy = Some(hierarchy);
                self.graph = graph;
                self.refresh_terminals(window, cx);
                if let Some(index) = self.scenes.iter().position(|scene| {
                    self.hierarchy
                        .as_ref()
                        .and_then(|h| h.scene.as_ref())
                        .is_some_and(|active| active.id == scene.id)
                }) {
                    self.tab_scroll.scroll_to_item(index);
                }
                if let Some(index) = self
                    .order
                    .iter()
                    .position(|id| Some(id) == self.focused.as_ref())
                {
                    self.carousel_scroll.scroll_to_item(index);
                }
                if let Some(terminal) = self.focused.as_ref().and_then(|id| self.terminals.get(id))
                {
                    window.focus(&terminal.read(cx).focus);
                }
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
        for id in &self.order {
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
            let scene_id = scene.id.clone();
            let mut title = scene.name.clone();
            if active && !scene.title_pinned && title.starts_with("Shell · ") {
                if let Some(cwd) = self.graph["nodes"]
                    .as_array()
                    .and_then(|nodes| {
                        nodes
                            .iter()
                            .find(|node| node["sessionId"].as_str() == self.focused.as_deref())
                    })
                    .and_then(|node| node["cwd"].as_str())
                {
                    title = format!("Shell · {cwd}");
                }
            }
            tabs = tabs.child(
                div()
                    .id(SharedString::from(format!("tab-{}", scene.id)))
                    .h(px(28.))
                    .min_w(px(100.))
                    .max_w(px(200.))
                    .flex_none()
                    .pl(px(12.))
                    .pr(px(10.))
                    .rounded(px(6.))
                    .bg(if active {
                        rgba(0xeceef3ff)
                    } else {
                        rgba(0x00000000)
                    })
                    .text_color(if active { rgb(0x2f3547) } else { rgb(0x999999) })
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(12.))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.send(
                            Command::SelectScene {
                                scene_id: scene_id.clone(),
                            },
                            cx,
                        )
                    }))
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(title),
                    )
                    .child(
                        div()
                            .opacity(if active { 1. } else { 0. })
                            .child(icon("x", 13., 0x606572)),
                    ),
            );
        }
        let task = self
            .hierarchy
            .as_ref()
            .and_then(|h| h.task.as_ref())
            .map(|t| t.id.clone());
        tabs = tabs.child(
            div()
                .id("create-canvas")
                .ml(px(4.))
                .size(px(24.))
                .flex_none()
                .rounded(px(6.))
                .bg(rgb(0xf1f1f4))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(task_id) = &task {
                        this.send(
                            Command::CreateCanvas {
                                task_id: task_id.clone(),
                            },
                            cx,
                        );
                    }
                }))
                .child(icon("plus", 15., 0x606572)),
        );
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
        let content_width = f32::from(window.viewport_size().width) - 260. - 20.;
        let columns = (((content_width + 20. + 12.) / 292.).floor()).clamp(1., 4.);
        let inactive_width = (content_width - (columns - 1.) * 12.) / columns;
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
            .bg(rgb(0xe9edf2));
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
            let scene_id = self
                .hierarchy
                .as_ref()
                .and_then(|h| h.scene.as_ref())
                .map(|s| s.id.clone());
            let selected = self.focused.as_ref() == Some(id);
            let width = if selected {
                (content_width * 0.48).min(620.)
            } else {
                inactive_width
            };
            let card = div()
                .id(SharedString::from(format!("card-{id}")))
                .w(px(width))
                .h_full()
                .flex_none()
                .relative()
                .top(px(if selected { -4. } else { 0. }))
                .rounded(px(12.))
                .overflow_hidden()
                .bg(rgb(0xfcfcfd))
                .flex()
                .flex_col()
                .shadow(vec![
                    BoxShadow {
                        color: rgba(0x32445c1c).into(),
                        offset: point(px(0.), px(7.)),
                        blur_radius: px(15.),
                        spread_radius: px(0.),
                    },
                    BoxShadow {
                        color: rgba(0x32445c30).into(),
                        offset: point(px(0.), px(24.)),
                        blur_radius: px(52.),
                        spread_radius: px(0.),
                    },
                ])
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        window.focus(&focus);
                        if this.focused.as_ref() != Some(&session_id) {
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
                )
                .child(
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
                );
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
        div()
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
    }
}

fn icon(name: &str, size: f32, color: u32) -> Svg {
    svg()
        .path(SharedString::from(format!("icons/{name}.svg")))
        .size(px(size))
        .flex_none()
        .text_color(rgb(color))
}
