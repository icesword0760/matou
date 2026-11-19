//! Hidden GPUI layout and input probe backed by a disposable session graph.

#![allow(unexpected_cfgs)]

#[cfg(target_os = "macos")]
pub use matou_native_desktop::{
    assets, macos, recovery_water_view, rename_input, tab_dialog_style, workspace_drag,
};

#[cfg(target_os = "macos")]
#[path = "../src/carousel_motion.rs"]
mod carousel_motion;
#[cfg(target_os = "macos")]
#[path = "../src/history_view.rs"]
mod history_view;
#[cfg(target_os = "macos")]
#[path = "support/runtime_probe.rs"]
mod runtime_bridge;
#[cfg(target_os = "macos")]
#[path = "../src/sidebar_scrollbar.rs"]
mod sidebar_scrollbar;
#[cfg(target_os = "macos")]
#[path = "../src/terminal_input.rs"]
mod terminal_input;
#[cfg(target_os = "macos")]
#[path = "../src/terminal_scrollbar.rs"]
mod terminal_scrollbar;
#[cfg(target_os = "macos")]
#[path = "../src/terminal_surface.rs"]
#[allow(dead_code)]
mod terminal_surface;
#[cfg(target_os = "macos")]
#[path = "../src/ui_text.rs"]
mod ui_text;

#[cfg(target_os = "macos")]
#[allow(dead_code)]
mod workspace {
    include!("../src/workspace.rs");

    use super::probe::{PARENT, SCENE};
    use matou_native_terminal::{Terminal, TerminalOptions};

    impl Workspace {
        pub(super) fn deliver(
            &mut self,
            event: Event,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            self.apply(
                BridgeEvent::Runtime {
                    generation: self.commands.connection().0,
                    event,
                },
                window,
                cx,
            );
        }

        pub(super) fn assert_level(
            &self,
            parent: Option<&str>,
            cards: &[&str],
            label: &str,
            count: &str,
            focused: &str,
        ) {
            assert_eq!(self.level_parent(), parent);
            assert_eq!(self.order, cards);
            for index in 0..cards.len() {
                let bounds = self.carousel_scroll.bounds_for_item(index).unwrap();
                assert!(bounds.size.width > px(0.) && bounds.size.height > px(0.));
            }
            assert_eq!(self.card_geometry.len(), cards.len());
            if !cards.is_empty() {
                assert!(self.carousel_scroll.bounds_for_item(cards.len()).is_none());
            }
            let (actual_label, actual_count) = self.breadcrumb_labels();
            assert_eq!(
                (actual_label.as_str(), actual_count.as_str()),
                (label, count)
            );
            assert_eq!(self.focused.as_deref(), Some(focused));
            println!(
                "{label} {count} | cards={cards:?} | focus={focused} | scene={}",
                self.active_scene_id().unwrap()
            );
        }

        pub(super) fn badge_position(&self) -> Point<Pixels> {
            let index = self.order.iter().position(|id| id == PARENT).unwrap();
            let bounds = self.carousel_scroll.bounds_for_item(index).unwrap();
            let inset = if matches!(self.recovery.get(PARENT), Some(RecoveryState::Failed(_))) {
                50.
            } else {
                83.
            };
            point(
                bounds.right() + self.carousel_scroll.offset().x - px(inset),
                bounds.top() + px(20.),
            )
        }

        pub(super) fn compact_parent_position(
            &self,
            window: &Window,
            inset: Pixels,
        ) -> Point<Pixels> {
            assert!(window.viewport_size().width - px(260.) < px(720.));
            assert_eq!(self.level_focus(), Some(super::probe::PEER));
            let index = self.order.iter().position(|id| id == PARENT).unwrap();
            let bounds = self.carousel_scroll.bounds_for_item(index).unwrap();
            assert!(bounds.size.width <= px(176.));
            point(
                bounds.center().x + self.carousel_scroll.offset().x,
                bounds.top() + inset,
            )
        }

        pub(super) fn assert_root_badge(&self) {
            let badge = self.child_badge(PARENT).unwrap();
            assert_eq!(badge.summary, "4 分支 · 1 异常");
            assert_eq!(
                badge.detail,
                "Claude 1 · Codex 1 · Shell 2；运行中 1 · 待输入 1 · 错误 1"
            );
            assert!(self.child_badge(super::probe::PEER).is_none());
            println!("badge={} | hover={}", badge.summary, badge.detail);
        }

        pub(super) fn scroll_left(&self) -> Pixels {
            -self.carousel_scroll.offset().x
        }

        pub(super) fn assert_badge_hover(&self, hovered: bool) {
            assert_eq!(
                self.hovered_child_badge.as_deref(),
                hovered.then_some(PARENT)
            );
        }

        pub(super) fn assert_terminal_focus(
            &self,
            id: &str,
            expected: bool,
            window: &Window,
            cx: &Context<Self>,
        ) {
            assert_eq!(
                self.terminals[id].read(cx).focus.is_focused(window),
                expected
            );
        }

        pub(super) fn focus_return_button(&self, window: &mut Window) {
            window.focus(&self.control_focus["session-return-parent"]);
        }

        pub(super) fn scroll_for_geometry(&mut self, window: &Window, cx: &mut Context<Self>) {
            self.scroll_carousel(
                &ScrollWheelEvent {
                    position: point(px(700.), px(250.)),
                    delta: ScrollDelta::Pixels(point(px(-120.), px(0.))),
                    ..Default::default()
                },
                window,
                cx,
            );
        }

        pub(super) fn qualify(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
            let (cols, rows, cell_width, cell_height) = self.terminals[id]
                .read(cx)
                .target_dimensions()
                .expect("terminal painted in hidden GPUI layout");
            let mut terminal =
                Terminal::new(TerminalOptions::new(cols, rows, cell_width, cell_height)).unwrap();
            self.deliver(
                Event::Recovery {
                    session_id: id.into(),
                    state: RecoveryState::Ready,
                },
                window,
                cx,
            );
            self.deliver(
                Event::Frame {
                    session_id: id.into(),
                    screen: terminal.screen().unwrap(),
                    modes: terminal.input_modes().unwrap(),
                    selection: None,
                    sequence: 1,
                    scroll_request_id: 0,
                },
                window,
                cx,
            );
            assert!(!self.recovery_blocked(id));
        }

        pub(super) fn assert_sibling_command(
            &self,
            command: Command,
            parent: Option<&str>,
            source: &str,
        ) {
            assert!(matches!(command, Command::CreateShellSibling {
                scene_id, source_session_id, parent_session_id,
            } if scene_id == SCENE && source_session_id == source && parent_session_id.as_deref() == parent));
            println!("CreateShellSibling | source={source} | parent={parent:?}");
        }

        pub(super) fn remove_button_position(&self, id: &str) -> Point<Pixels> {
            let index = self.order.iter().position(|card| card == id).unwrap();
            let bounds = self.carousel_scroll.bounds_for_item(index).unwrap();
            point(
                bounds.right() + self.carousel_scroll.offset().x - px(25.5),
                bounds.top() + px(21.),
            )
        }

        pub(super) fn stopped_card_position(&self, id: &str, inset: Pixels) -> Point<Pixels> {
            self.remove_button_position(id) - point(inset, px(0.))
        }

        pub(super) fn assert_stopped_state(&self, id: &str, focus: Option<&str>) {
            assert!(self.order.iter().any(|card| card == id));
            let node = self
                .graph_node(id)
                .expect("stopped card stays in the graph");
            assert!(!node["archivedAt"].is_null());
            assert_eq!(self.active_level_focus(), focus);
            assert!(!self.terminals.contains_key(id));
            assert!(!self.card_controls.contains_key(id));
            assert!(!self.recovery.contains_key(id));
            assert!(!self.recovery_controls.contains_key(id));
            assert!(!self.recovery_water.contains_key(id));
            assert!(!self.recovery_water_needed(id, node));
        }

        pub(super) fn assert_stopped_content(&self, id: &str, title: &str, lines: &[&str]) {
            let node = self.graph_node(id).unwrap();
            assert_eq!(node["title"], title);
            assert_eq!(STOPPED_SESSION_STATUS, "正在恢复会话…");
            assert_eq!(
                Workspace::stopped_session_lines(node),
                (!lines.is_empty()).then(|| lines.join("\n")),
            );
            let mut status = Workspace::stopped_session_status(id);
            let style = status.style();
            assert_eq!(
                style.text.as_ref().unwrap().color,
                Some(white().alpha(0.52))
            );
            assert_eq!(style.background, Some(white().alpha(0.06).into()));
            assert_eq!(
                RemoveNodeTooltip {
                    blocked_reason: None
                }
                .text(),
                "移除节点…"
            );
            println!(
                "{title} | {STOPPED_SESSION_STATUS} | {} summary lines | white label alpha .52/.06",
                lines.len()
            );
        }

        pub(super) fn seed_recovery_water(&mut self, id: &str, cx: &mut Context<Self>) {
            assert!(self.terminals.contains_key(id));
            assert!(self.recovery_water_needed(id, self.graph_node(id).unwrap()));
            self.recovery_water.insert(
                id.to_owned(),
                cx.new(|cx| {
                    RecoveryWater::new(self.runtime_generation, Theme::default(), true, cx)
                }),
            );
        }

        pub(super) fn assert_stopped_remove_dialog(
            &self,
            id: &str,
            title: &str,
            scope: RemoveScope,
            branch: bool,
        ) {
            let Some(TabDialog::RemoveSession {
                session_id,
                scope: selected,
                ..
            }) = &self.dialog
            else {
                panic!("expected stopped-card removal confirmation");
            };
            let text = self.removal_dialog_text(id, scope).unwrap();
            assert_eq!(session_id, id);
            assert_eq!(*selected, scope);
            assert_eq!(text.title, title);
            assert_eq!(text.choices.is_some(), branch);
            assert_eq!(self.dialog_controls.len(), if branch { 5 } else { 3 });
            println!("{title} | scope={scope:?}");
        }

        pub(super) fn settle_stopped_probe_geometry(&mut self, cx: &mut Context<Self>) {
            self.hover_follow_until = None;
            self.focus_follow_until = None;
            self.restore_geometry = false;
            self.flush_geometry(cx);
        }

        pub(super) fn stopped_remove_status(&self, id: &str) -> (bool, Option<&'static str>) {
            let node = self.graph_node(id).unwrap();
            (
                self.stopped_remove_visible(node),
                self.stopped_remove_blocked_reason(),
            )
        }

        pub(super) fn assert_visual_focus(&self, id: &str) {
            assert_eq!(self.level_focus(), Some(id));
            assert_eq!(self.card_motion[id].width.value(f64::INFINITY), 1.);
        }

        pub(super) fn assert_live_card(&self, id: &str, has_children: bool, cx: &Context<Self>) {
            assert!(self.terminals[id].read(cx).target_dimensions().is_some());
            assert!(!self.is_stopped_session(id));
            assert_eq!(self.child_badge(id).is_some(), has_children);
        }

        pub(super) fn assert_root_without_badge_hover(&self, id: &str) {
            assert_eq!(self.level_parent(), None);
            assert!(self.child_badge(id).is_some(), "fixture needs children");
            assert!(self.hovered_child_badge.is_none());
        }

        pub(super) fn assert_no_dialog(&self) {
            assert!(self.dialog.is_none());
        }

        pub(super) fn assert_card_error(&self) {
            self.assert_no_dialog();
            assert_eq!(self.fault.as_deref(), Some("probe: navigation error"));
        }

        pub(super) fn assert_remove_tooltip(&self, id: &str, expected: &str, enabled: bool) {
            let tooltip = self.remove_node_tooltip(id, self.graph_node(id).unwrap());
            assert_eq!(tooltip.text(), expected);
            assert_eq!(tooltip.blocked_reason.is_none(), enabled);
            println!("Remove tooltip | enabled={enabled} | {expected}");
        }

        pub(super) fn assert_start_failure_card(
            &self,
            id: &str,
            reason: &str,
            blocked: Option<&str>,
        ) {
            let node = self.graph_node(id).unwrap();
            let card = self
                .start_failure_card(id, node)
                .expect("startup failure card");
            assert_eq!(card.title, "会话启动失败");
            assert_eq!(card.reason, reason);
            assert_eq!(card.retry, "重试启动");
            assert_eq!(card.remove, "移除失败会话");
            assert_eq!(card.blocked, blocked);
            assert_eq!(self.start_failure_action_available(id), blocked.is_none());
            assert!(!self.recovery_water_needed(id, node));
            assert!(!self.recovery_controls.contains_key(id));
            assert_eq!(self.fault, None, "the launch error belongs on its card");
            println!(
                "{} | {} | {} | {} | blocked={blocked:?}",
                card.title, card.reason, card.retry, card.remove
            );
        }

        pub(super) fn retry_start_button_position(&self, id: &str) -> Point<Pixels> {
            let index = self.order.iter().position(|card| card == id).unwrap();
            let bounds = self.carousel_scroll.bounds_for_item(index).unwrap();
            assert!(
                bounds.size.width > px(540.),
                "probe needs a single-row failure card"
            );
            point(
                bounds.right() + self.carousel_scroll.offset().x - px(133.),
                bounds.top() + px(61.),
            )
        }

        pub(super) fn assert_restarting_has_no_failure_controls(&self, id: &str) {
            let node = self.graph_node(id).unwrap();
            assert!(matches!(
                self.recovery.get(id),
                Some(RecoveryState::Restarting(_))
            ));
            assert!(self.start_failure_card(id, node).is_none());
            assert!(!self.start_failure_controls.contains_key(id));
            assert!(!self.recovery_controls.contains_key(id));
            assert!(!self.recovery_water_needed(id, node));
        }

        pub(super) fn focus_start_failure_button(
            &self,
            id: &str,
            button: usize,
            window: &mut Window,
        ) {
            window.focus(&self.start_failure_controls[id][button]);
        }

        pub(super) fn assert_provider_restore(
            &self,
            id: &str,
            reason: Option<&str>,
            pending: bool,
            blocked: [Option<&str>; 2],
        ) {
            let node = self.graph_node(id).expect("projected Claude card");
            let banner = self
                .provider_restore_banner(id, node)
                .expect("visible provider restore banner");
            assert!(self.start_failure_card(id, node).is_none());
            match (banner, reason) {
                (ProviderRestoreBanner::Restoring { title }, None) => {
                    assert_eq!(title, "正在恢复 Claude Code 会话…");
                    println!("{title} | generic startup failure hidden");
                }
                (
                    ProviderRestoreBanner::Failed {
                        title,
                        reason,
                        retry,
                        fresh,
                        pending: busy,
                        retry_blocked,
                        fresh_blocked,
                    },
                    Some(expected_reason),
                ) => {
                    assert_eq!(title, "Claude Code 恢复失败");
                    assert_eq!(reason, expected_reason);
                    assert_eq!(
                        retry,
                        if pending {
                            "正在恢复…"
                        } else {
                            "重试恢复"
                        }
                    );
                    assert_eq!(fresh, "新开 Claude Code");
                    assert_eq!(busy, pending);
                    assert_eq!([retry_blocked, fresh_blocked], blocked);
                    assert!(!self.recovery_water_needed(id, node));
                    println!("{title} | {reason} | {retry} | {fresh} | busy={busy} | {blocked:?}");
                }
                _ => panic!("unexpected provider restore banner"),
            }
        }

        pub(super) fn assert_no_provider_restore(&self, id: &str) {
            assert!(
                self.graph_node(id)
                    .and_then(|node| self.provider_restore_banner(id, node))
                    .is_none()
            );
        }

        pub(super) fn assert_background_provider_restore_hidden(&self, id: &str) {
            assert!(!self.order.iter().any(|card| card == id));
            assert!(self.graph_node(id).is_none());
            self.assert_no_provider_restore(id);
        }

        pub(super) fn focus_provider_restore_button(
            &self,
            id: &str,
            button: usize,
            window: &mut Window,
        ) {
            window.focus(&self.provider_restore_controls[id][button]);
        }

        pub(super) fn provider_restore_button_positions(&self, id: &str) -> [Point<Pixels>; 2] {
            let index = self.order.iter().position(|card| card == id).unwrap();
            let bounds = self.carousel_scroll.bounds_for_item(index).unwrap();
            let right = bounds.right() + self.carousel_scroll.offset().x;
            [
                point(right - px(136.), bounds.top() + px(58.)),
                point(right - px(45.), bounds.top() + px(58.)),
            ]
        }

        pub(super) fn assert_provider_bridge_error(&self) {
            assert_eq!(
                self.task_toast.as_deref(),
                Some("native runtime command rejected: NotReady")
            );
        }

        pub(super) fn focus_recovery_failure_button(&self, id: &str, window: &mut Window) {
            assert!(
                self.start_failure_card(id, self.graph_node(id).unwrap())
                    .is_none()
            );
            assert!(!self.start_failure_controls.contains_key(id));
            window.focus(&self.recovery_controls[id]);
        }

        pub(super) fn focus_remove_button(&self, id: &str, window: &mut Window) {
            window.focus(&self.remove_card_controls[id]);
        }

        pub(super) fn assert_card_focus(&self, id: &str, window: &Window, cx: &Context<Self>) {
            assert_eq!(self.level_parent(), None);
            assert_eq!(self.level_focus(), Some(id));
            self.assert_terminal_focus(id, true, window, cx);
        }

        pub(super) fn assert_stop_dialog(&self, expected: &str, window: &Window) {
            let Some(TabDialog::CloseSession { body, .. }) = &self.dialog else {
                panic!("expected stop confirmation");
            };
            assert_eq!(body, expected);
            assert_eq!(STOP_SESSION, "停止会话");
            assert_eq!(self.dialog_controls.len(), 3);
            assert!(self.dialog_controls[0].is_focused(window));
            println!("停止会话 | {body} | 取消 / 停止会话");
        }

        pub(super) fn assert_last_dialog(&self, id: &str, window: &Window) {
            assert!(
                matches!(&self.dialog, Some(TabDialog::LastSession { session_id, .. }) if session_id == id)
            );
            assert_eq!(self.dialog_controls.len(), 2);
            assert!(self.dialog_controls[0].is_focused(window));
            assert_eq!(
                LAST_TAB_BODY,
                "当前已是最后一个事项下的最后一个标签，这里点击关闭不会删除该事项。\n\n如需删除该工作区，请在左侧事项面板的下拉菜单中执行删除。"
            );
            assert_eq!(LAST_TAB_CONFIRM, "我知道了");
            println!("提示 | {LAST_TAB_BODY} | 我知道了");
        }

        pub(super) fn assert_leaf_dialog(&self) {
            let Some(TabDialog::RemoveSession {
                session_id, scope, ..
            }) = &self.dialog
            else {
                panic!("expected leaf removal confirmation");
            };
            assert_eq!(*scope, RemoveScope::NodeOnly);
            let text = self.removal_dialog_text(session_id, *scope).unwrap();
            assert_eq!(text.title, "移除节点“level-peer”？");
            assert_eq!(
                text.body,
                "影响 1 个会话、0 个自有 Worktree。移除后，该会话会从会话列表和 DAG 中消失。"
            );
            assert_eq!(
                text.files_unchanged,
                Some("项目文件和自有 Worktree 保持原样。")
            );
            assert!(text.choices.is_none() && text.warning.is_none());
            assert_eq!(text.confirm, "移除");
            assert_eq!(self.dialog_controls.len(), 3);
            println!(
                "{} | {} | {} | 取消 / {}",
                text.title,
                text.body,
                text.files_unchanged.unwrap(),
                text.confirm
            );
        }

        pub(super) fn assert_branch_dialog(&self, selected: RemoveScope) {
            let Some(TabDialog::RemoveSession {
                session_id, scope, ..
            }) = &self.dialog
            else {
                panic!("expected branch removal confirmation");
            };
            assert_eq!(*scope, selected);
            let text = self.removal_dialog_text(session_id, *scope).unwrap();
            assert_eq!(
                text.body,
                "请选择移除范围。项目文件和自有 Worktree 保持原样。"
            );
            let [node, branch] = text.choices.unwrap();
            assert_eq!(node.title, "仅移除当前节点");
            assert_eq!(branch.title, "移除当前节点及全部后代");
            assert_eq!(self.dialog_controls.len(), 5);
            match session_id.as_str() {
                PARENT => {
                    assert_eq!(text.title, "移除节点“分支主会话”？");
                    assert_eq!(node.impact, "影响 1 个会话、0 个自有 Worktree");
                    assert_eq!(node.description, "直接后代会话将成为根节点。");
                    assert_eq!(
                        branch.impact,
                        "影响 6 个会话、1 个自有 Worktree；其中 2 个运行中、1 个待输入"
                    );
                    assert_eq!(branch.description, "当前节点与 5 个后代会话会全部移除。");
                    match scope {
                        RemoveScope::NodeOnly => {
                            assert_eq!(text.confirm, "移除当前节点");
                            assert_eq!(text.warning, None);
                        }
                        RemoveScope::NodeAndDescendants => {
                            assert_eq!(text.confirm, "移除 6 个会话");
                            assert_eq!(
                                text.warning.as_deref(),
                                Some("其中 2 个运行中、1 个待输入的会话将先停止。")
                            );
                        }
                    }
                }
                "child-first" => {
                    assert_eq!(text.title, "移除节点“child-first”？");
                    assert_eq!(node.description, "后代会话将重连到当前节点的父级。");
                    assert_eq!(
                        node.impact,
                        "影响 1 个会话、1 个自有 Worktree；其中 1 个运行中"
                    );
                    assert_eq!(
                        branch.impact,
                        "影响 2 个会话、1 个自有 Worktree；其中 1 个运行中"
                    );
                    assert_eq!(branch.description, "当前节点与 1 个后代会话会全部移除。");
                    assert_eq!(text.confirm, "移除当前节点");
                    assert_eq!(
                        text.warning.as_deref(),
                        Some("其中 1 个运行中的会话将先停止。")
                    );
                }
                _ => unreachable!(),
            }
            println!(
                "{} | {} | {} | {:?} | 取消 / {}",
                text.title, node.impact, branch.impact, text.warning, text.confirm
            );
        }
    }
}

#[cfg(target_os = "macos")]
mod probe {
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use cocoa::appkit::NSEventType;

    use gpui::{
        AnyWindowHandle, AppContext, Application, AsyncApp, Bounds, Keystroke, Pixels, Point,
        WindowBounds, WindowHandle, WindowOptions, point, px, size,
    };
    use matou_native_core::{
        geometry,
        hierarchy::{
            self, BootstrapWindowInput, HierarchyIds,
            canvas::{self, CanvasIds, CanvasMessages, CreateCanvasInput, CreateShellSiblingInput},
            cards::{RemoveScope, scene_workspace_session_count},
        },
        session_graph::project_scene_graph,
        startup::{self, BootstrapResult, open_read_only},
        storage::CommandMetadata,
        workspace_paths::WorkspacePathState,
    };
    use matou_native_runtime::host::{Command, Event, LaunchOptions, RecoveryState, RuntimeHost};

    use crate::{
        assets::NativeAssets,
        runtime_bridge::{self, ProbeReceiver},
        workspace::Workspace,
    };

    pub(super) const SCENE: &str = "levels-scene";
    pub(super) const PARENT: &str = "level-parent";
    pub(super) const PEER: &str = "level-peer";
    const CHILD: &str = "child-urgent";
    const WINDOW: &str = "levels-window";
    const SECOND_SCENE: &str = "other-scene";
    const SECOND_ROOT: &str = "other-root";
    const ROOTS: &[&str] = &[PARENT, PEER];
    const CHILDREN: &[&str] = &["child-first", "child-stopped", CHILD, "child-running"];
    const MESSAGES: CanvasMessages<'static> = CanvasMessages {
        new_canvas: "新画布",
        duplicate_scene_name: "画布名称重复",
        workspace_path_invalid: "工作区不可用",
        detached_session_must_return: "会话须返回画布",
    };

    struct Fixture {
        db: Option<BootstrapResult>,
        root: PathBuf,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            drop(self.db.take());
            std::fs::remove_dir_all(&self.root).unwrap();
        }
    }

    fn command(id: &str) -> CommandMetadata {
        CommandMetadata {
            command_id: id.into(),
            command_type: id.into(),
            request_hash: id.into(),
            causation_id: None,
            correlation_id: None,
        }
    }

    fn canvas_ids(scene_id: &str, session_id: &str) -> CanvasIds {
        CanvasIds {
            scene_id: scene_id.into(),
            session_id: session_id.into(),
            root_node_id: format!("{session_id}-node"),
            secondary_node_id: format!("{session_id}-split"),
            mount_id: format!("{session_id}-mount"),
            relation_id: format!("{session_id}-relation"),
        }
    }

    impl Fixture {
        fn new() -> Self {
            let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../target")
                .join(format!("session-levels-probe-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&root).unwrap();
            let mut fixture = Self { db: None, root };
            fixture.db = Some(startup::open(&fixture.root, "session-levels-probe").unwrap());
            let Some(BootstrapResult::Writable { connection, .. }) = fixture.db.as_mut() else {
                panic!("expected writable isolated fixture")
            };
            hierarchy::bootstrap_window(
                connection,
                &command("bootstrap"),
                &BootstrapWindowInput {
                    window_id: WINDOW.into(),
                    default_root_directory: fixture.root.to_string_lossy().into_owned(),
                    default_name: "会话层级探针".into(),
                    now: 1,
                },
                &HierarchyIds {
                    workspace_id: "levels-workspace".into(),
                    execution_context_id: "levels-context".into(),
                    task_id: "levels-task".into(),
                    scene_id: SCENE.into(),
                    root_node_id: "levels-root-node".into(),
                    session_id: PARENT.into(),
                    mount_id: "levels-root-mount".into(),
                },
                "工作台",
                1,
            )
            .unwrap();
            for (index, id) in [PEER]
                .into_iter()
                .chain(CHILDREN.iter().copied())
                .enumerate()
            {
                let now = index as i64 + 2;
                canvas::create_shell_sibling(
                    connection,
                    &command(id),
                    &CreateShellSiblingInput {
                        window_id: WINDOW.into(),
                        scene_id: SCENE.into(),
                        source_session_id: PARENT.into(),
                        parent_session_id: (id != PEER).then(|| PARENT.into()),
                        execution_context_id: None,
                        title: Some(id.into()),
                        navigation: None,
                        now,
                    },
                    &canvas_ids(SCENE, id),
                    &MESSAGES,
                    now,
                )
                .unwrap();
            }
            connection.execute_batch(
                "UPDATE sessions SET title = '分支主会话' WHERE id = 'level-parent';
                 UPDATE sessions SET kind = 'claude-code', work_status = 'needs-input' WHERE id = 'child-urgent';
                 UPDATE sessions SET kind = 'codex', work_status = 'running' WHERE id = 'child-running';
                 UPDATE sessions SET archived_at = 8, work_status = 'error' WHERE id = 'child-stopped';",
            ).unwrap();
            canvas::create_canvas(
                connection,
                &command("other-canvas"),
                &CreateCanvasInput {
                    window_id: WINDOW.into(),
                    task_id: "levels-task".into(),
                    title: Some("另一画布".into()),
                    navigation: None,
                    now: 9,
                },
                &canvas_ids(SECOND_SCENE, SECOND_ROOT),
                &MESSAGES,
                9,
            )
            .unwrap();
            fixture.focus(SCENE, PEER);
            fixture
        }

        fn prepare_card_actions(&mut self) {
            let Some(BootstrapResult::Writable { connection, .. }) = self.db.as_mut() else {
                unreachable!()
            };
            canvas::create_shell_sibling(
                connection,
                &command("grandchild"),
                &CreateShellSiblingInput {
                    window_id: WINDOW.into(),
                    scene_id: SCENE.into(),
                    source_session_id: "child-first".into(),
                    parent_session_id: Some("child-first".into()),
                    execution_context_id: None,
                    title: Some("grandchild".into()),
                    navigation: None,
                    now: 20,
                },
                &canvas_ids(SCENE, "grandchild"),
                &MESSAGES,
                20,
            )
            .unwrap();
            connection.execute_batch(
                "UPDATE sessions SET work_status = 'starting' WHERE id = 'child-first';
                 INSERT INTO execution_contexts(id, workspace_id, kind, cwd, created_at)
                 VALUES ('probe-worktree-context', 'levels-workspace', 'git-worktree', '/probe-worktree', 20);
                 INSERT INTO worktrees (id, execution_context_id, repository_root, worktree_path, branch_name, state, created_at, updated_at)
                 VALUES ('probe-worktree', 'probe-worktree-context', '/probe-repository', '/probe-worktree', 'probe', 'ready', 20, 20);
                 UPDATE session_environment_bindings SET managed_worktree_id = 'probe-worktree' WHERE session_id = 'child-first';",
            ).unwrap();
            self.focus(SCENE, PEER);
        }

        fn focus(&mut self, scene: &str, session: &str) {
            let Some(BootstrapResult::Writable { connection, .. }) = self.db.as_mut() else {
                unreachable!()
            };
            hierarchy::focus_session(connection, WINDOW, scene, session, 10, "会话不在当前画布中")
                .unwrap();
        }

        fn set_peer_environment(&self, state: &str) {
            let Some(BootstrapResult::Writable { connection, .. }) = &self.db else {
                unreachable!()
            };
            assert_eq!(
                connection
                    .execute(
                        "UPDATE session_environment_bindings SET state = ?1 WHERE session_id = ?2",
                        [state, PEER],
                    )
                    .unwrap(),
                1
            );
        }

        fn set_provider_restore(&self, state: &str, error: Option<&str>, revision: i64) {
            let Some(BootstrapResult::Writable { connection, .. }) = &self.db else {
                unreachable!()
            };
            let resume = match state {
                "failed" => "failed",
                "restoring" => "available",
                _ => "expired",
            };
            connection
                .execute(
                    "UPDATE sessions SET kind = 'claude-code' WHERE id = ?1",
                    [PEER],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO provider_bindings (
                        id, session_id, provider, provider_session_id, resume_state, metadata_json,
                        created_at, updated_at, validated_at, restore_state, restore_error
                    ) VALUES ('provider-probe', ?1, 'claude-code', 'probe-conversation', ?2,
                        json_object('spawnRevision', ?3), 2, 3, 3, ?4, ?5)
                    ON CONFLICT(id) DO UPDATE SET resume_state = excluded.resume_state,
                        metadata_json = excluded.metadata_json, restore_state = excluded.restore_state,
                        restore_error = excluded.restore_error",
                    (PEER, resume, revision, state, error),
                )
                .unwrap();
        }

        fn event(&self, ready: bool) -> Event {
            let Some(BootstrapResult::Writable { connection, .. }) = &self.db else {
                unreachable!()
            };
            let hierarchy = hierarchy::read_hierarchy(connection, WINDOW).unwrap();
            let scene_id = &hierarchy.scene.as_ref().unwrap().id;
            let scenes = hierarchy::list_scenes(connection, "levels-task").unwrap();
            let mut graph = project_scene_graph(connection, scene_id, Some(WINDOW)).unwrap();
            graph["workspaceSessionCount"] = scene_workspace_session_count(connection, scene_id)
                .unwrap()
                .into();
            let geometry = geometry::list(connection, scene_id).unwrap();
            if ready {
                Event::Ready {
                    hierarchy,
                    scenes,
                    graph,
                    geometry,
                }
            } else {
                Event::Navigation {
                    hierarchy,
                    scenes,
                    graph,
                    geometry,
                }
            }
        }

        fn graph_event(&self) -> Event {
            let Event::Navigation { graph, .. } = self.event(false) else {
                unreachable!()
            };
            Event::Graph(graph)
        }

        fn process(
            &mut self,
            window: WindowHandle<Workspace>,
            cx: &mut AsyncApp,
            sink: &ProbeReceiver,
        ) -> Vec<Command> {
            let mut actions = Vec::new();
            while let Ok(command) = sink.try_recv() {
                match command {
                    Command::FocusSession {
                        ref scene_id,
                        ref session_id,
                    } => {
                        self.focus(scene_id, session_id);
                        deliver(window, cx, self.event(false));
                        actions.push(command);
                    }
                    Command::PutGeometry { sequence, update } => {
                        let Some(BootstrapResult::Writable { connection, .. }) = &self.db else {
                            unreachable!()
                        };
                        let update = geometry::put(connection, &update).unwrap();
                        println!(
                            "PutGeometry | owner={} | {}",
                            update.owner_key, update.geometry
                        );
                        deliver(
                            window,
                            cx,
                            Event::GeometryResult {
                                sequence,
                                update,
                                error: None,
                            },
                        );
                    }
                    Command::CreateShellSibling { .. }
                    | Command::RestartSession { .. }
                    | Command::RetryRecovery { .. }
                    | Command::RetryProviderRestore { .. }
                    | Command::StartFreshProviderSession { .. }
                    | Command::CloseSessionCard { .. }
                    | Command::RemoveSessionBranch { .. } => actions.push(command),
                    _ => {}
                }
            }
            actions
        }

        fn assert_geometry(&self) {
            let Some(BootstrapResult::Writable { connection, .. }) = &self.db else {
                unreachable!()
            };
            let saved = geometry::list(connection, SCENE).unwrap();
            assert_eq!(
                saved
                    .iter()
                    .map(|update| update.owner_key.as_str())
                    .collect::<Vec<_>>(),
                [
                    "session-group:levels-scene:level-parent",
                    "session-group:levels-scene:root"
                ],
            );
            assert!(saved[0].geometry["scrollLeft"].as_f64().unwrap() > 0.);
            assert_eq!(saved[0].geometry["anchorSessionId"], CHILD);
            assert_eq!(saved[1].geometry["scrollLeft"].as_f64(), Some(0.));
            println!("Persisted independent root and child geometry in SQLite");
        }

        fn archived_children_graph(&self) -> serde_json::Value {
            let Some(BootstrapResult::Writable { connection, .. }) = &self.db else {
                unreachable!()
            };
            connection
                .execute_batch("UPDATE sessions SET archived_at = 11 WHERE id LIKE 'child-%'")
                .unwrap();
            project_scene_graph(connection, SCENE, Some(WINDOW)).unwrap()
        }

        fn set_archived(&self, id: &str, archived: bool) {
            let Some(BootstrapResult::Writable { connection, .. }) = &self.db else {
                unreachable!()
            };
            assert_eq!(
                connection
                    .execute(
                        "UPDATE sessions SET archived_at = ? WHERE id = ?",
                        (archived.then_some(21), id),
                    )
                    .unwrap(),
                1
            );
        }

        fn stopped_graph(&self) -> serde_json::Value {
            let Some(BootstrapResult::Writable { connection, .. }) = &self.db else {
                unreachable!()
            };
            project_scene_graph(connection, SCENE, Some(WINDOW)).unwrap()
        }

        fn seed_stopped_summary(&self, lines: &[&str]) {
            let Some(BootstrapResult::Writable { connection, .. }) = &self.db else {
                unreachable!()
            };
            connection
                .execute(
                    "INSERT INTO session_graph_summaries VALUES (?, ?, 22)",
                    (PARENT, serde_json::to_string(lines).unwrap()),
                )
                .unwrap();
        }
    }

    fn draw(window: WindowHandle<Workspace>, cx: &mut AsyncApp) {
        AnyWindowHandle::from(window)
            .update(cx, |_, window, cx| {
                window.refresh();
                window.draw(cx).clear();
            })
            .unwrap();
    }

    fn deliver(window: WindowHandle<Workspace>, cx: &mut AsyncApp, event: Event) {
        window
            .update(cx, |view, window, cx| view.deliver(event, window, cx))
            .unwrap();
    }

    fn assert_focus(actions: Vec<Command>, focus: &str) {
        assert!(
            matches!(actions.as_slice(), [Command::FocusSession { scene_id, session_id }]
            if scene_id == SCENE && session_id == focus),
            "expected a single focus command for {focus}"
        );
    }

    fn mouse(
        window: WindowHandle<Workspace>,
        cx: &mut AsyncApp,
        position: Point<Pixels>,
        kind: NSEventType,
    ) {
        use cocoa::{
            appkit::{NSEvent, NSEventModifierFlags},
            base::{id, nil},
            foundation::{NSInteger, NSPoint},
        };
        use objc::{msg_send, sel, sel_impl};
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};

        let (view, location, number) = window
            .update(cx, |_, window, _| {
                let RawWindowHandle::AppKit(handle) =
                    HasWindowHandle::window_handle(window).unwrap().as_raw()
                else {
                    unreachable!();
                };
                let view = handle.ns_view.as_ptr() as id;
                let number: NSInteger = unsafe {
                    let native: id = msg_send![view, window];
                    msg_send![native, windowNumber]
                };
                let location = NSPoint::new(
                    f64::from(f32::from(position.x)),
                    f64::from(f32::from(window.viewport_size().height - position.y)),
                );
                (view, location, number)
            })
            .unwrap();
        // Target only the probe NSView. The event never enters the desktop queue.
        unsafe {
            let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure_(
                nil, kind, location, NSEventModifierFlags::empty(), 0., number, nil, 0, 1, 1.,
            );
            match kind {
                NSEventType::NSMouseMoved => {
                    let _: () = msg_send![view, mouseMoved: event];
                }
                NSEventType::NSLeftMouseDown => {
                    let _: () = msg_send![view, mouseDown: event];
                }
                NSEventType::NSLeftMouseUp => {
                    let _: () = msg_send![view, mouseUp: event];
                }
                _ => unreachable!(),
            }
        }
    }

    fn press(window: WindowHandle<Workspace>, cx: &mut AsyncApp, key: &str) {
        AnyWindowHandle::from(window)
            .update(cx, |_, window, cx| {
                assert!(window.dispatch_keystroke(Keystroke::parse(key).unwrap(), cx));
            })
            .unwrap();
    }

    fn scroll(window: WindowHandle<Workspace>, cx: &mut AsyncApp) {
        window
            .update(cx, |view, window, cx| view.scroll_for_geometry(window, cx))
            .unwrap();
    }

    fn click(window: WindowHandle<Workspace>, cx: &mut AsyncApp, position: Point<Pixels>) {
        mouse(window, cx, position, NSEventType::NSLeftMouseDown);
        mouse(window, cx, position, NSEventType::NSLeftMouseUp);
    }

    fn click_remove(window: WindowHandle<Workspace>, cx: &mut AsyncApp, id: &str) {
        draw(window, cx);
        let position = window
            .update(cx, |view, _, _| view.remove_button_position(id))
            .unwrap();
        click(window, cx, position);
        draw(window, cx);
    }

    fn open_hidden(cx: &mut AsyncApp, width: f32) -> (WindowHandle<Workspace>, ProbeReceiver) {
        let (commands, sink) = runtime_bridge::probe_channel();
        let (_events, incoming) = async_channel::unbounded();
        let window = cx
            .open_window(
                WindowOptions {
                    show: false,
                    focus: false,
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(width), px(780.)),
                    ))),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| Workspace::new(commands, incoming, window, cx)),
            )
            .unwrap();
        (window, sink)
    }

    fn assert_remove_command(commands: Vec<Command>, id: &str, expected: RemoveScope) {
        assert!(
            matches!(commands.as_slice(), [Command::RemoveSessionBranch {
            scene_id, session_id, scope,
        }] if scene_id == SCENE && session_id == id && *scope == expected)
        );
        println!("RemoveSessionBranch | session={id} | scope={expected:?}");
    }

    fn assert_close_command(commands: Vec<Command>, id: &str, intent: Option<&str>) {
        assert!(matches!(commands.as_slice(), [Command::CloseSessionCard {
            scene_id, session_id, confirmed_intent, preserve_scene_on_last_session: false,
        }] if scene_id == SCENE && session_id == id && confirmed_intent.as_deref() == intent));
        println!("CloseSessionCard | session={id} | confirmed_intent={intent:?}");
    }

    fn assert_stopped_interaction_writes(sink: &ProbeReceiver, stopped: &[&str]) {
        while let Ok(command) = sink.try_recv() {
            match command {
                Command::Applied { .. } => {}
                Command::Resize { session_id, .. } => {
                    assert!(!stopped.contains(&session_id.as_str()))
                }
                _ => panic!("stopped-card interaction sent an unexpected command"),
            }
        }
    }

    fn acknowledge_stopped_removal(
        fixture: &Fixture,
        window: WindowHandle<Workspace>,
        cx: &mut AsyncApp,
        id: &str,
    ) {
        // Release the pending action in the command-sink probe. Keep the fixture
        // nodes so the next case can exercise the same graph without a new owner.
        let mut event = fixture.event(false);
        let Event::Navigation { graph, .. } = &mut event else {
            unreachable!()
        };
        graph["sessionCardAction"] = serde_json::json!({
            "action": "remove-session-branch",
            "sceneId": SCENE,
            "sessionId": id,
        });
        deliver(window, cx, event);
    }

    fn verify_stopped_cards(cx: &mut AsyncApp) {
        const LINES: &[&str] = &[
            "drop one",
            "drop two",
            "step 03",
            "step 04",
            "",
            "  step 06  ",
            "step 07",
            "step 08",
            "step 09",
            "step 10",
        ];
        let fixture = Fixture::new();
        let (window, sink) = open_hidden(cx, 1200.);
        deliver(window, cx, fixture.event(true));
        draw(window, cx);
        window
            .update(cx, |view, window, cx| {
                view.qualify(PEER, window, cx);
                view.assert_terminal_focus(PEER, true, window, cx);
                view.seed_recovery_water(PARENT, cx);
            })
            .unwrap();

        fixture.seed_stopped_summary(LINES);
        fixture.set_archived(PARENT, true);
        let graph = fixture.stopped_graph();
        deliver(window, cx, Event::Graph(graph.clone()));
        window
            .update(cx, |view, _, _| {
                view.assert_stopped_state(PARENT, Some(PEER));
                view.assert_stopped_content(PARENT, "分支主会话", &LINES[6..]);
                assert_eq!(view.stopped_remove_status(PARENT), (true, None));
            })
            .unwrap();

        let mut expanded_graph = graph.clone();
        expanded_graph["nodes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|node| node["sessionId"] == PARENT)
            .unwrap()["latestLines"] = serde_json::json!(LINES);
        deliver(window, cx, Event::Graph(expanded_graph));
        deliver(
            window,
            cx,
            Event::Recovery {
                session_id: PARENT.into(),
                state: RecoveryState::Failed("stale recovery callback".into()),
            },
        );
        window
            .update(cx, |view, _, cx| view.settle_stopped_probe_geometry(cx))
            .unwrap();
        draw(window, cx);
        window
            .update(cx, |view, window, cx| {
                view.assert_level(None, ROOTS, "根会话", "· 2 个会话", PEER);
                view.assert_stopped_content(PARENT, "分支主会话", &LINES[2..]);
                view.assert_stopped_state(PARENT, Some(PEER));
                view.assert_terminal_focus(PEER, true, window, cx);
                view.assert_live_card(PEER, false, cx);
                view.assert_root_without_badge_hover(PARENT);
                view.flush_geometry(cx);
            })
            .unwrap();
        while sink.try_recv().is_ok() {}

        let (body, badge_slot, remove) = window
            .update(cx, |view, _, _| {
                (
                    view.stopped_card_position(PARENT, px(90.)) + point(px(0.), px(60.)),
                    view.stopped_card_position(PARENT, px(58.)),
                    view.remove_button_position(PARENT),
                )
            })
            .unwrap();
        click(window, cx, body);
        mouse(window, cx, badge_slot, NSEventType::NSMouseMoved);
        click(window, cx, badge_slot);
        window
            .update(cx, |view, window, cx| {
                view.assert_no_dialog();
                view.assert_root_without_badge_hover(PARENT);
                view.assert_terminal_focus(PEER, true, window, cx);
                view.flush_geometry(cx);
            })
            .unwrap();
        assert_stopped_interaction_writes(&sink, &[PARENT]);

        mouse(window, cx, remove, NSEventType::NSLeftMouseDown);
        window
            .update(cx, |view, window, cx| {
                view.assert_no_dialog();
                view.assert_stopped_state(PARENT, Some(PEER));
                view.assert_terminal_focus(PEER, true, window, cx);
                view.flush_geometry(cx);
            })
            .unwrap();
        assert_stopped_interaction_writes(&sink, &[PARENT]);
        mouse(window, cx, remove, NSEventType::NSLeftMouseUp);
        draw(window, cx);
        window
            .update(cx, |view, _, _| {
                view.assert_stopped_remove_dialog(
                    PARENT,
                    "移除节点“分支主会话”？",
                    RemoveScope::NodeOnly,
                    true,
                );
            })
            .unwrap();
        press(window, cx, "tab"); // Node only
        press(window, cx, "tab"); // Node and descendants
        press(window, cx, "enter");
        draw(window, cx);
        window
            .update(cx, |view, _, _| {
                view.assert_stopped_remove_dialog(
                    PARENT,
                    "移除节点“分支主会话”？",
                    RemoveScope::NodeAndDescendants,
                    true,
                );
            })
            .unwrap();
        press(window, cx, "tab"); // Cancel
        press(window, cx, "tab"); // Confirm
        press(window, cx, "enter");
        assert_remove_command(
            std::iter::from_fn(|| sink.try_recv().ok()).collect(),
            PARENT,
            RemoveScope::NodeAndDescendants,
        );

        acknowledge_stopped_removal(&fixture, window, cx, PARENT);
        fixture.set_archived(PEER, true);
        deliver(window, cx, Event::Graph(fixture.stopped_graph()));
        draw(window, cx);
        window
            .update(cx, |view, _, _| {
                view.assert_level(None, ROOTS, "根会话", "· 2 个会话", PEER);
                view.assert_visual_focus(PEER);
                view.assert_stopped_state(PEER, None);
                view.assert_stopped_content(PEER, "level-peer", &[]);
            })
            .unwrap();
        press(window, cx, "cmd-w");
        assert_stopped_interaction_writes(&sink, &[PARENT, PEER]);
        click_remove(window, cx, PEER);
        window
            .update(cx, |view, _, _| {
                view.assert_stopped_remove_dialog(
                    PEER,
                    "移除节点“level-peer”？",
                    RemoveScope::NodeOnly,
                    false,
                );
            })
            .unwrap();
        deliver(
            window,
            cx,
            Event::WorkspacePaths(vec![WorkspacePathState {
                workspace_id: "levels-workspace".into(),
                status: "invalid".into(),
                reason: "probe path".into(),
                checked_at: 1,
                validation_generation: 1,
            }]),
        );
        click_remove(window, cx, PEER);
        window
            .update(cx, |view, _, _| {
                view.assert_no_dialog();
                assert_eq!(
                    view.stopped_remove_status(PEER),
                    (
                        true,
                        Some("工作区目录不可用，请先在本地恢复原路径，或移出该工作区")
                    )
                );
            })
            .unwrap();
        assert_stopped_interaction_writes(&sink, &[PARENT, PEER]);
        deliver(window, cx, Event::WorkspacePaths(Vec::new()));
        sink.read_only();
        click_remove(window, cx, PEER);
        window
            .update(cx, |view, _, _| {
                view.assert_no_dialog();
                assert_eq!(
                    view.stopped_remove_status(PEER),
                    (true, Some("数据库处于只读恢复模式"))
                );
            })
            .unwrap();
        assert_stopped_interaction_writes(&sink, &[PARENT, PEER]);
        sink.generation(1);
        click_remove(window, cx, PEER);
        press(window, cx, "shift-tab");
        press(window, cx, "enter");
        assert_remove_command(
            std::iter::from_fn(|| sink.try_recv().ok()).collect(),
            PEER,
            RemoveScope::NodeOnly,
        );
        acknowledge_stopped_removal(&fixture, window, cx, PEER);

        let mut only_branch = fixture.stopped_graph();
        only_branch["nodes"]
            .as_array_mut()
            .unwrap()
            .retain(|node| node["sessionId"] != PEER);
        deliver(window, cx, Event::Graph(only_branch));
        click_remove(window, cx, PARENT);
        window
            .update(cx, |view, _, _| {
                view.assert_no_dialog();
                assert_eq!(view.stopped_remove_status(PARENT), (false, None));
                view.assert_visual_focus(PARENT);
            })
            .unwrap();
        assert_stopped_interaction_writes(&sink, &[PARENT, PEER]);
        fixture.set_archived(PARENT, false);
        deliver(window, cx, Event::Graph(fixture.stopped_graph()));
        draw(window, cx);
        window
            .update(cx, |view, window, cx| {
                view.assert_live_card(PARENT, true, cx);
                window.remove_window();
            })
            .unwrap();
        println!("Stopped cards preserve the visual level and never activate a terminal");
    }

    fn verify_narrow_stopped_cards(cx: &mut AsyncApp) {
        let fixture = Fixture::new();
        let (window, sink) = open_hidden(cx, 900.);
        fixture.set_archived(PARENT, true);
        deliver(window, cx, fixture.event(true));
        draw(window, cx);
        window
            .update(cx, |view, _, cx| view.settle_stopped_probe_geometry(cx))
            .unwrap();
        draw(window, cx);
        window
            .update(cx, |view, _, cx| view.flush_geometry(cx))
            .unwrap();
        while sink.try_recv().is_ok() {}
        let compact_header = window
            .update(cx, |view, window, _| {
                view.compact_parent_position(window, px(21.))
            })
            .unwrap();
        click(window, cx, compact_header);
        click_remove(window, cx, PARENT);
        window
            .update(cx, |view, _, cx| {
                view.assert_stopped_state(PARENT, Some(PEER));
                view.assert_no_dialog();
                view.flush_geometry(cx);
            })
            .unwrap();
        assert_stopped_interaction_writes(&sink, &[PARENT]);

        fixture.set_archived(PEER, true);
        deliver(window, cx, Event::Graph(fixture.stopped_graph()));
        click_remove(window, cx, PEER);
        window
            .update(cx, |view, window, _| {
                view.assert_visual_focus(PEER);
                view.assert_stopped_remove_dialog(
                    PEER,
                    "移除节点“level-peer”？",
                    RemoveScope::NodeOnly,
                    false,
                );
                window.remove_window();
            })
            .unwrap();
        println!("Narrow stopped peer uses the shared summary until visually selected");
    }

    fn verify_start_failure_card(cx: &mut AsyncApp) {
        let mut fixture = Fixture::new();
        fixture.prepare_card_actions();
        let (window, sink) = open_hidden(cx, 1500.);
        let failure = |state| Event::Recovery {
            session_id: PEER.into(),
            state,
        };
        let inspect = |cx: &mut AsyncApp, reason, blocked| {
            window
                .update(cx, |view, _, _| {
                    view.assert_start_failure_card(PEER, reason, blocked);
                })
                .unwrap();
        };
        let activate = |cx: &mut AsyncApp, button| {
            window
                .update(cx, |view, window, _| {
                    view.focus_start_failure_button(PEER, button, window);
                })
                .unwrap();
            draw(window, cx);
            AnyWindowHandle::from(window)
                .update(cx, |_, window, cx| {
                    window.dispatch_keystroke(Keystroke::parse("enter").unwrap(), cx);
                })
                .unwrap();
        };

        deliver(window, cx, fixture.event(true));
        let error = "start Shell /probe/missing-zsh: No such file or directory";
        deliver(
            window,
            cx,
            failure(RecoveryState::StartFailed(error.into())),
        );
        deliver(window, cx, Event::Diagnostic(error.into()));
        draw(window, cx);
        inspect(cx, error, None);
        assert!(fixture.process(window, cx, &sink).is_empty());

        fixture.set_peer_environment("recovering");
        deliver(window, cx, fixture.event(false));
        draw(window, cx);
        inspect(cx, error, Some("当前运行环境需要先恢复或交接"));
        activate(cx, 0);
        activate(cx, 1);
        assert!(fixture.process(window, cx, &sink).is_empty());
        sink.read_only();
        inspect(cx, error, Some("数据库处于只读恢复模式"));
        sink.generation(1);

        fixture.set_peer_environment("ready");
        deliver(window, cx, fixture.event(false));
        draw(window, cx);
        let retry_position = window
            .update(cx, |view, _, _| view.retry_start_button_position(PEER))
            .unwrap();
        click(window, cx, retry_position);
        assert!(matches!(fixture.process(window, cx, &sink).as_slice(), [
            Command::RestartSession { session_id }
        ] if session_id == PEER));
        draw(window, cx);
        window
            .update(cx, |view, _, _| {
                view.assert_restarting_has_no_failure_controls(PEER);
            })
            .unwrap();
        deliver(window, cx, failure(RecoveryState::Restarting(error.into())));
        draw(window, cx);
        window
            .update(cx, |view, _, _| {
                view.assert_restarting_has_no_failure_controls(PEER);
            })
            .unwrap();
        click(window, cx, retry_position);
        assert!(fixture.process(window, cx, &sink).is_empty());
        println!("重试中隐藏启动失败卡、恢复失败控件和恢复水效；原按钮位置再次点击无会话命令");

        deliver(
            window,
            cx,
            failure(RecoveryState::StartFailed(String::new())),
        );
        draw(window, cx);
        inspect(cx, "终端进程未能启动", None);
        deliver(
            window,
            cx,
            failure(RecoveryState::StartFailed("updated Shell failure".into())),
        );
        draw(window, cx);
        inspect(cx, "updated Shell failure", None);
        activate(cx, 1);
        assert_close_command(
            fixture.process(window, cx, &sink),
            PEER,
            Some(&format!("delete-session:{PEER}")),
        );

        deliver(
            window,
            cx,
            failure(RecoveryState::Failed("probe recovery failure".into())),
        );
        window
            .update(cx, |view, window, _| {
                view.focus_recovery_failure_button(PEER, window);
            })
            .unwrap();
        draw(window, cx);
        press(window, cx, "enter");
        assert!(matches!(fixture.process(window, cx, &sink).as_slice(), [
            Command::RetryRecovery { session_id }
        ] if session_id == PEER));
        println!("恢复失败仍发送 RetryRecovery | session={PEER}");
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
    }

    async fn verify_provider_restore_banner(cx: &mut AsyncApp) {
        const BACKGROUND: &str = "provider session running in background";
        const BACKGROUND_ZH: &str =
            "此会话仍在 Claude 后台运行。请先停止对应后台会话，再重试恢复；聊天记录会保留。";
        const NO_BLOCK: [Option<&str>; 2] = [None, None];
        let mut fixture = Fixture::new();
        let (window, sink) = open_hidden(cx, 900.);
        let inspect = |cx: &mut AsyncApp, reason, pending, blocked| {
            draw(window, cx);
            window
                .update(cx, |view, _, _| {
                    view.assert_provider_restore(PEER, reason, pending, blocked);
                    view.assert_no_provider_restore(PARENT);
                })
                .unwrap();
        };
        let activate = |cx: &mut AsyncApp, button| {
            window
                .update(cx, |view, window, _| {
                    view.focus_provider_restore_button(PEER, button, window);
                })
                .unwrap();
            draw(window, cx);
            press(window, cx, "enter");
        };

        fixture.set_provider_restore("failed", Some(BACKGROUND), 0);
        deliver(window, cx, fixture.event(true));
        inspect(cx, Some(BACKGROUND_ZH), false, NO_BLOCK);

        fixture.set_provider_restore("restoring", None, 0);
        deliver(window, cx, fixture.graph_event());
        deliver(
            window,
            cx,
            Event::Recovery {
                session_id: PEER.into(),
                state: RecoveryState::StartFailed("obsolete startup failure".into()),
            },
        );
        inspect(cx, None, false, NO_BLOCK);

        fixture.set_provider_restore("failed", None, 0);
        deliver(window, cx, fixture.graph_event());
        inspect(cx, Some("终端进程未能启动"), false, NO_BLOCK);
        let reason = "provider process could not start: ".to_owned() + &"probe detail ".repeat(30);
        fixture.set_provider_restore("failed", Some(&reason), 0);
        deliver(window, cx, fixture.graph_event());
        inspect(cx, Some(&reason), false, NO_BLOCK);

        let [retry_position, fresh_position] = window
            .update(cx, |view, _, _| {
                view.provider_restore_button_positions(PEER)
            })
            .unwrap();
        deliver(
            window,
            cx,
            Event::Recovery {
                session_id: PEER.into(),
                state: RecoveryState::Restoring,
            },
        );
        inspect(cx, Some(&reason), false, [Some("当前终端仍在恢复"), None]);
        click(window, cx, retry_position);
        assert!(fixture.process(window, cx, &sink).is_empty());
        deliver(
            window,
            cx,
            Event::Recovery {
                session_id: PEER.into(),
                state: RecoveryState::Failed(reason.clone()),
            },
        );
        fixture.set_peer_environment("recovering");
        deliver(window, cx, fixture.graph_event());
        inspect(
            cx,
            Some(&reason),
            false,
            [Some("当前运行环境需要先恢复或交接"); 2],
        );
        click(window, cx, retry_position);
        click(window, cx, fresh_position);
        assert!(fixture.process(window, cx, &sink).is_empty());
        sink.read_only();
        inspect(
            cx,
            Some(&reason),
            false,
            [Some("数据库处于只读恢复模式"); 2],
        );
        sink.generation(1);
        fixture.set_peer_environment("ready");
        deliver(window, cx, fixture.graph_event());
        inspect(cx, Some(&reason), false, NO_BLOCK);

        click(window, cx, retry_position);
        // Dispatch both follow-up clicks before drawing so stale callbacks
        // also exercise the action handler's in-flight guard.
        click(window, cx, retry_position);
        click(window, cx, fresh_position);
        assert!(matches!(
            fixture.process(window, cx, &sink).as_slice(),
            [Command::RetryProviderRestore { session_id }] if session_id == PEER
        ));
        println!(
            "RetryProviderRestore | session_id={PEER} | duplicate and competing clicks suppressed"
        );
        inspect(cx, Some(&reason), true, NO_BLOCK);
        click(window, cx, fresh_position);
        assert!(fixture.process(window, cx, &sink).is_empty());
        deliver(window, cx, fixture.graph_event());
        inspect(cx, Some(&reason), true, NO_BLOCK);

        // The graph can publish a new failed revision without an intermediate
        // restoring frame. The next explicit action must still become available.
        fixture.set_provider_restore("failed", Some(&reason), 1);
        deliver(window, cx, fixture.graph_event());
        inspect(cx, Some(&reason), false, NO_BLOCK);
        activate(cx, 1);
        click(window, cx, retry_position);
        assert!(matches!(
            fixture.process(window, cx, &sink).as_slice(),
            [Command::StartFreshProviderSession { session_id }] if session_id == PEER
        ));
        println!("StartFreshProviderSession | session_id={PEER} | competing click suppressed");
        inspect(cx, Some(&reason), true, NO_BLOCK);

        fixture.set_provider_restore("none", None, 2);
        deliver(window, cx, fixture.graph_event());
        draw(window, cx);
        window
            .update(cx, |view, _, _| view.assert_no_provider_restore(PEER))
            .unwrap();

        fixture.set_provider_restore("failed", Some(BACKGROUND), 2);
        fixture.focus(SECOND_SCENE, SECOND_ROOT);
        deliver(window, cx, fixture.event(false));
        draw(window, cx);
        window
            .update(cx, |view, _, _| {
                view.assert_background_provider_restore_hidden(PEER)
            })
            .unwrap();
        fixture.focus(SCENE, PEER);
        deliver(window, cx, fixture.event(false));
        inspect(cx, Some(BACKGROUND_ZH), false, NO_BLOCK);
        println!("Background card stays out of the active graph until its canvas opens");

        activate(cx, 0);
        assert!(matches!(
            fixture.process(window, cx, &sink).as_slice(),
            [Command::RetryProviderRestore { session_id }] if session_id == PEER
        ));
        inspect(cx, Some(BACKGROUND_ZH), true, NO_BLOCK);
        cx.background_executor()
            .timer(Duration::from_millis(8200))
            .await;
        inspect(cx, Some(BACKGROUND_ZH), false, NO_BLOCK);
        assert!(fixture.process(window, cx, &sink).is_empty());
        println!("Missing runtime acknowledgement unlocks both controls without another command");

        drop(sink);
        activate(cx, 0);
        inspect(cx, Some(BACKGROUND_ZH), false, NO_BLOCK);
        window
            .update(cx, |view, window, _| {
                view.assert_provider_bridge_error();
                window.remove_window();
            })
            .unwrap();
    }

    fn verify_card_actions(cx: &mut AsyncApp) {
        let mut fixture = Fixture::new();
        fixture.prepare_card_actions();
        let (window, sink) = open_hidden(cx, 1200.);
        let assert_tooltip = |cx: &mut AsyncApp, text, enabled| {
            window
                .update(cx, |view, _, _| {
                    view.assert_remove_tooltip(PEER, text, enabled)
                })
                .unwrap();
        };
        deliver(window, cx, fixture.event(true));
        draw(window, cx);

        assert_tooltip(cx, "当前终端仍在恢复", false);
        press(window, cx, "cmd-w");
        assert!(
            fixture.process(window, cx, &sink).is_empty(),
            "recovery blocks close"
        );
        window
            .update(cx, |view, window, cx| view.qualify(PEER, window, cx))
            .unwrap();
        deliver(
            window,
            cx,
            Event::WorkspacePaths(vec![WorkspacePathState {
                workspace_id: "levels-workspace".into(),
                status: "invalid".into(),
                reason: "probe path".into(),
                checked_at: 1,
                validation_generation: 1,
            }]),
        );
        assert_tooltip(
            cx,
            "工作区目录不可用，请先在本地恢复原路径，或移出该工作区",
            false,
        );
        press(window, cx, "cmd-w");
        assert!(
            fixture.process(window, cx, &sink).is_empty(),
            "invalid path blocks close"
        );
        deliver(window, cx, Event::WorkspacePaths(Vec::new()));
        deliver(
            window,
            cx,
            Event::StorageFault {
                session_id: PEER.into(),
                message: "probe storage".into(),
            },
        );
        assert_tooltip(cx, "终端存储异常，请先恢复或结束当前会话", false);
        press(window, cx, "cmd-w");
        assert!(
            fixture.process(window, cx, &sink).is_empty(),
            "storage fault blocks close"
        );
        deliver(
            window,
            cx,
            Event::StorageRecovered {
                session_id: PEER.into(),
            },
        );
        fixture.set_peer_environment("recovering");
        deliver(window, cx, fixture.event(false));
        assert_tooltip(cx, "当前运行环境需要先恢复或交接", false);
        fixture.set_peer_environment("ready");
        deliver(window, cx, fixture.event(false));
        assert_tooltip(cx, "移除节点…", true);

        window
            .update(cx, |view, window, _| view.focus_remove_button(PEER, window))
            .unwrap();
        draw(window, cx);
        press(window, cx, "enter");
        draw(window, cx);
        window
            .update(cx, |view, _, _| view.assert_leaf_dialog())
            .unwrap();
        press(window, cx, "cmd-w");
        press(window, cx, "tab");
        press(window, cx, "enter"); // Cancel after the header close control.
        window
            .update(cx, |view, _, _| view.assert_no_dialog())
            .unwrap();
        assert!(
            fixture.process(window, cx, &sink).is_empty(),
            "leaf cancel sends nothing"
        );

        click_remove(window, cx, PEER);
        press(window, cx, "shift-tab");
        press(window, cx, "enter");
        assert_remove_command(
            fixture.process(window, cx, &sink),
            PEER,
            RemoveScope::NodeOnly,
        );
        send_card_failure(window, cx);

        press(window, cx, "cmd-w");
        press(window, cx, "cmd-w");
        window
            .update(cx, |view, _, _| view.assert_no_dialog())
            .unwrap();
        assert_close_command(fixture.process(window, cx, &sink), PEER, None);
        press(window, cx, "cmd-w");
        assert!(
            fixture.process(window, cx, &sink).is_empty(),
            "in-flight close sends nothing"
        );
        send_card_failure(window, cx);

        fixture.focus(SCENE, CHILD);
        deliver(window, cx, fixture.event(false));
        draw(window, cx);
        window
            .update(cx, |view, window, cx| view.qualify(CHILD, window, cx))
            .unwrap();
        press(window, cx, "cmd-w");
        press(window, cx, "cmd-w");
        draw(window, cx);
        window.update(cx, |view, window, _| view.assert_stop_dialog(
            "“child-urgent”正在运行。停止后，该节点会在会话列表和 DAG 中保持为“已停止”，子会话继续工作。",
            window,
        )).unwrap();
        assert!(
            fixture.process(window, cx, &sink).is_empty(),
            "confirmation is required"
        );
        press(window, cx, "shift-tab");
        press(window, cx, "enter");
        press(window, cx, "cmd-w");
        assert_close_command(
            fixture.process(window, cx, &sink),
            CHILD,
            Some("delete-session:child-urgent"),
        );
        send_card_failure(window, cx);

        fixture.focus(SCENE, "child-first");
        deliver(window, cx, fixture.event(false));
        draw(window, cx);
        window
            .update(cx, |view, window, cx| {
                view.qualify("child-first", window, cx)
            })
            .unwrap();
        click_remove(window, cx, "child-first");
        window
            .update(cx, |view, _, _| {
                view.assert_branch_dialog(RemoveScope::NodeOnly)
            })
            .unwrap();
        press(window, cx, "shift-tab");
        press(window, cx, "enter");
        assert_remove_command(
            fixture.process(window, cx, &sink),
            "child-first",
            RemoveScope::NodeOnly,
        );
        send_card_failure(window, cx);

        fixture.focus(SCENE, PARENT);
        deliver(window, cx, fixture.event(false));
        draw(window, cx);
        window
            .update(cx, |view, window, cx| view.qualify(PARENT, window, cx))
            .unwrap();
        press(window, cx, "cmd-w");
        draw(window, cx);
        window.update(cx, |view, window, _| view.assert_stop_dialog(
            "“分支主会话”当前空闲，并有 4 个子会话。停止后，该节点会在会话列表和 DAG 中保持为“已停止”，子会话继续工作。",
            window,
        )).unwrap();
        press(window, cx, "escape");
        assert!(
            fixture.process(window, cx, &sink).is_empty(),
            "stop Escape sends nothing"
        );
        press(window, cx, "cmd-w");
        draw(window, cx);
        press(window, cx, "shift-tab");
        press(window, cx, "enter");
        assert_close_command(
            fixture.process(window, cx, &sink),
            PARENT,
            Some("delete-session:level-parent"),
        );
        send_card_failure(window, cx);

        click_remove(window, cx, PARENT);
        window
            .update(cx, |view, _, _| {
                view.assert_branch_dialog(RemoveScope::NodeOnly)
            })
            .unwrap();
        press(window, cx, "escape");
        assert!(
            fixture.process(window, cx, &sink).is_empty(),
            "remove Escape sends nothing"
        );
        click_remove(window, cx, PARENT);
        click(window, cx, point(px(300.), px(190.))); // Canvas shade outside the centered panel.
        window
            .update(cx, |view, _, _| view.assert_no_dialog())
            .unwrap();
        assert!(
            fixture.process(window, cx, &sink).is_empty(),
            "shade sends nothing"
        );
        click_remove(window, cx, PARENT);
        press(window, cx, "tab");
        press(window, cx, "down");
        draw(window, cx);
        window
            .update(cx, |view, _, _| {
                view.assert_branch_dialog(RemoveScope::NodeAndDescendants)
            })
            .unwrap();
        press(window, cx, "tab");
        press(window, cx, "tab");
        press(window, cx, "enter");
        assert_remove_command(
            fixture.process(window, cx, &sink),
            PARENT,
            RemoveScope::NodeAndDescendants,
        );
        send_card_failure(window, cx);
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
    }

    fn send_card_failure(window: WindowHandle<Workspace>, cx: &mut AsyncApp) {
        deliver(
            window,
            cx,
            Event::NavigationFailed {
                message: "probe: navigation error".into(),
                attempted_focus: None,
            },
        );
        window
            .update(cx, |view, _, _| view.assert_card_error())
            .unwrap();
    }

    fn runtime_event(host: &RuntimeHost, deadline: Instant) -> Event {
        loop {
            match host.events.try_recv() {
                Ok(Event::Diagnostic(message) | Event::NavigationFailed { message, .. }) => {
                    panic!("probe runtime: {message}");
                }
                Ok(event) => return event,
                Err(async_channel::TryRecvError::Empty) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("timed out or disconnected from probe runtime: {error}"),
            }
        }
    }

    fn runtime_navigation(host: &RuntimeHost, command: Command, count: u64) -> serde_json::Value {
        host.commands.send(command).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Event::Navigation { graph, .. } = runtime_event(host, deadline) {
                assert_eq!(graph["workspaceSessionCount"], count);
                return graph;
            }
        }
    }

    fn verify_last_card(cx: &mut AsyncApp) {
        let root = Fixture {
            db: None,
            root: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../target")
                .join(format!("last-card-probe-{}", uuid::Uuid::new_v4())),
        };
        let options = LaunchOptions {
            data_root: root.root.join("data"),
            workspace: root.root.join("workspace"),
            shell_home: root.root.join("shell-home"),
            shell: "/bin/zsh".into(),
        };
        let db_path = options.data_root.join("matou.sqlite");
        let mut host = RuntimeHost::start(options).unwrap();
        let (window, sink) = open_hidden(cx, 1000.);
        let deadline = Instant::now() + Duration::from_secs(10);
        let ready = loop {
            if let event @ Event::Ready { .. } = runtime_event(&host, deadline) {
                break event;
            }
        };
        let Event::Ready {
            hierarchy, graph, ..
        } = &ready
        else {
            unreachable!()
        };
        let id = hierarchy.session.as_ref().unwrap().id.clone();
        let scene = hierarchy.scene.as_ref().unwrap().id.clone();
        let task_id = hierarchy.task.as_ref().unwrap().id.clone();
        let workspace_id = hierarchy.workspace.as_ref().unwrap().id.clone();
        assert_eq!(graph["workspaceSessionCount"], 1);
        let mut missing_count = graph.clone();
        missing_count
            .as_object_mut()
            .unwrap()
            .remove("workspaceSessionCount");
        deliver(window, cx, ready);
        draw(window, cx);
        window
            .update(cx, |view, window, cx| view.qualify(&id, window, cx))
            .unwrap();
        while sink.try_recv().is_ok() {}
        let db = open_read_only(&db_path).unwrap();
        let card_state = || {
            db.query_row(
                "SELECT archived_at,
                   (SELECT json_group_array(json_array(id, scene_id, scene_window_id))
                    FROM (SELECT * FROM session_mounts WHERE session_id = ?1 ORDER BY id)),
                   (SELECT COUNT(*) FROM domain_events WHERE event_type = 'session.archived'),
                   (SELECT COUNT(*) FROM command_deduplication WHERE command_type = 'hierarchy.delete-session')
                 FROM sessions WHERE id = ?1",
                [&id],
                |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, String>(1)?,
                          row.get::<_, i64>(2)?, row.get::<_, i64>(3)?)),
            ).unwrap()
        };
        let before = card_state();
        assert!(before.0.is_none() && before.1 != "[]");

        press(window, cx, "cmd-w");
        press(window, cx, "cmd-w");
        window
            .update(cx, |view, window, _| view.assert_last_dialog(&id, window))
            .unwrap();
        assert!(
            sink.try_recv().is_err(),
            "known final card sends zero commands"
        );
        draw(window, cx);
        press(window, cx, "tab");
        press(window, cx, "enter");
        window
            .update(cx, |view, window, cx| {
                view.assert_no_dialog();
                view.assert_card_focus(&id, window, cx);
            })
            .unwrap();
        assert!(
            sink.try_recv().is_err(),
            "dismissing final-card notice sends zero commands"
        );
        assert_eq!(card_state(), before);

        deliver(window, cx, Event::Graph(missing_count));
        press(window, cx, "cmd-w");
        press(window, cx, "cmd-w");
        let command = sink
            .try_recv()
            .expect("one card close request with missing count");
        assert!(matches!(&command, Command::CloseSessionCard {
            scene_id, session_id, confirmed_intent: None, preserve_scene_on_last_session: false,
        } if scene_id == &scene && session_id == &id));
        assert!(sink.try_recv().is_err());
        host.commands.send(command).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        let reply = loop {
            match runtime_event(&host, deadline) {
                Event::Graph(graph) if graph["sessionCardAction"]["outcome"] == "hide-window" => {
                    let action = &graph["sessionCardAction"];
                    assert_eq!(action["sessionId"], id);
                    assert_eq!(action["disposedSessionIds"], serde_json::json!([]));
                    break Event::Graph(graph);
                }
                Event::ScenesDisposed(ids) => panic!("last card disposed: {ids:?}"),
                _ => {}
            }
        };
        deliver(window, cx, reply);
        draw(window, cx);
        window
            .update(cx, |view, window, _| view.assert_last_dialog(&id, window))
            .unwrap();
        press(window, cx, "escape");
        window
            .update(cx, |view, window, cx| {
                view.assert_no_dialog();
                view.assert_card_focus(&id, window, cx);
                window.remove_window();
            })
            .unwrap();
        assert!(sink.try_recv().is_err());
        assert_eq!(card_state(), before);
        println!(
            "Actual runtime: hide-window, unchanged mounts, no archive events or close transaction"
        );
        drop(db);
        let same_canvas = runtime_navigation(
            &host,
            Command::CreateShellSibling {
                scene_id: scene.clone(),
                source_session_id: id.clone(),
                parent_session_id: None,
            },
            2,
        );
        assert_eq!(same_canvas["sceneId"], scene);
        assert_eq!(same_canvas["nodes"].as_array().unwrap().len(), 2);
        let across_canvas = runtime_navigation(
            &host,
            Command::CreateCanvas {
                task_id: task_id.clone(),
            },
            3,
        );
        assert_ne!(across_canvas["sceneId"], scene);
        assert_eq!(across_canvas["nodes"].as_array().unwrap().len(), 1);
        let across_task = runtime_navigation(&host, Command::CreateTask { workspace_id }, 4);
        assert_eq!(across_task["nodes"].as_array().unwrap().len(), 1);
        runtime_navigation(&host, Command::SelectTask { task_id }, 4);
        runtime_navigation(
            &host,
            Command::SelectScene {
                scene_id: scene.clone(),
            },
            4,
        );
        let after_archive = runtime_navigation(
            &host,
            Command::CloseSessionCard {
                scene_id: scene,
                session_id: id.clone(),
                confirmed_intent: Some(format!("delete-session:{id}")),
                preserve_scene_on_last_session: false,
            },
            3,
        );
        assert!(
            after_archive["nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|node| { node["sessionId"] == id && node["archivedAt"].is_number() })
        );
        println!(
            "Actual Navigation: workspace count 2 in one canvas, 3 across canvases, 4 across tasks, then 3 after archival"
        );
        host.shutdown().unwrap();
    }

    fn verify_compact_card(cx: &mut AsyncApp) {
        let mut fixture = Fixture::new();
        let (commands, sink) = runtime_bridge::probe_channel();
        let (_events, incoming) = async_channel::unbounded();
        let window = cx
            .open_window(
                WindowOptions {
                    show: false,
                    focus: false,
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(900.), px(780.)),
                    ))),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| Workspace::new(commands, incoming, window, cx)),
            )
            .unwrap();
        deliver(window, cx, fixture.event(true));
        draw(window, cx);
        window
            .update(cx, |view, _, _| {
                view.assert_level(None, ROOTS, "根会话", "· 2 个会话", PEER);
            })
            .unwrap();
        // Sample the compact header and summary rows where the obsolete
        // branch button would take hover, then click that same area.
        for inset in [20., 34., 42., 50., 58., 66.] {
            let position = window
                .update(cx, |view, window, _| {
                    view.compact_parent_position(window, px(inset))
                })
                .unwrap();
            mouse(window, cx, position, NSEventType::NSMouseMoved);
            window
                .update(cx, |view, _, _| view.assert_badge_hover(false))
                .unwrap();
        }
        let summary = window
            .update(cx, |view, window, _| {
                view.compact_parent_position(window, px(50.))
            })
            .unwrap();
        mouse(window, cx, summary, NSEventType::NSLeftMouseDown);
        mouse(window, cx, summary, NSEventType::NSLeftMouseUp);
        assert_focus(fixture.process(window, cx, &sink), PARENT);
        draw(window, cx);
        window
            .update(cx, |view, _, _| {
                view.assert_level(None, ROOTS, "根会话", "· 2 个会话", PARENT);
            })
            .unwrap();
        println!(
            "Compact parent has no branch badge hit target; click selected it in the root level"
        );
        fixture.focus(SCENE, PEER);
        deliver(window, cx, fixture.event(false));
        draw(window, cx);
        let compact_header = window
            .update(cx, |view, _, _| view.remove_button_position(PARENT))
            .unwrap();
        click(window, cx, compact_header);
        assert_focus(fixture.process(window, cx, &sink), PARENT);
        window
            .update(cx, |view, _, _| view.assert_no_dialog())
            .unwrap();
        println!("Compact card header activates the card without a remove control");

        deliver(
            window,
            cx,
            Event::Recovery {
                session_id: PARENT.into(),
                state: RecoveryState::Failed("probe recovery failure".into()),
            },
        );
        draw(window, cx);
        let badge = window
            .update(cx, |view, _, _| view.badge_position())
            .unwrap();
        mouse(window, cx, badge, NSEventType::NSMouseMoved);
        window
            .update(cx, |view, _, _| view.assert_badge_hover(true))
            .unwrap();
        mouse(window, cx, badge, NSEventType::NSLeftMouseDown);
        mouse(window, cx, badge, NSEventType::NSLeftMouseUp);
        assert_focus(fixture.process(window, cx, &sink), CHILD);
        draw(window, cx);
        window
            .update(cx, |view, _, _| {
                view.assert_level(
                    Some(PARENT),
                    CHILDREN,
                    "分支主会话 的子会话",
                    "· 4 个会话",
                    CHILD,
                );
            })
            .unwrap();
        println!("The expanded recovery-failed card still opens its child level");
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
    }

    pub(super) fn run() {
        Application::new().with_assets(NativeAssets).run(|cx| {
            let mut fixture = Fixture::new();
            let (commands, sink) = runtime_bridge::probe_channel();
            let (_events, incoming) = async_channel::unbounded();
            let window = cx
                .open_window(
                    WindowOptions {
                        show: false,
                        focus: false,
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            point(px(0.), px(0.)),
                            size(px(1200.), px(780.)),
                        ))),
                        ..Default::default()
                    },
                    |window, cx| cx.new(|cx| Workspace::new(commands, incoming, window, cx)),
                )
                .unwrap();
            cx.spawn(async move |cx| {
                verify_compact_card(cx);
                verify_start_failure_card(cx);
                verify_provider_restore_banner(cx).await;
                verify_card_actions(cx);
                verify_stopped_cards(cx);
                verify_narrow_stopped_cards(cx);
                verify_last_card(cx);
                deliver(window, cx, fixture.event(true));
                draw(window, cx);
                window
                    .update(cx, |view, window, cx| {
                        view.assert_level(None, ROOTS, "根会话", "· 2 个会话", PEER);
                        view.assert_root_badge();
                        view.qualify(PARENT, window, cx);
                        view.qualify(PEER, window, cx);
                        view.assert_terminal_focus(PEER, true, window, cx);
                    })
                    .unwrap();
                draw(window, cx);
                scroll(window, cx);
                assert!(fixture.process(window, cx, &sink).is_empty());

                let badge = window
                    .update(cx, |view, _, _| view.badge_position())
                    .unwrap();
                mouse(window, cx, badge, NSEventType::NSMouseMoved);
                draw(window, cx);
                window
                    .update(cx, |view, _, _| view.assert_badge_hover(true))
                    .unwrap();
                mouse(
                    window,
                    cx,
                    point(px(100.), px(100.)),
                    NSEventType::NSMouseMoved,
                );
                draw(window, cx);
                window
                    .update(cx, |view, _, _| view.assert_badge_hover(false))
                    .unwrap();
                println!("Badge hover entered and left on GPUI mouse movement");

                for return_key in [None, Some("left"), Some("up")] {
                    let root_focus = if return_key.is_none() { PEER } else { PARENT };
                    let position = window
                        .update(cx, |view, _, _| view.badge_position())
                        .unwrap();
                    mouse(window, cx, position, NSEventType::NSLeftMouseDown);
                    window
                        .update(cx, |view, window, cx| {
                            view.assert_terminal_focus(root_focus, true, window, cx);
                        })
                        .unwrap();
                    assert!(
                        fixture.process(window, cx, &sink).is_empty(),
                        "badge mouse-down focused the card"
                    );
                    mouse(window, cx, position, NSEventType::NSLeftMouseUp);
                    draw(window, cx);
                    window
                        .update(cx, |view, _, _| {
                            view.assert_level(
                                Some(PARENT),
                                CHILDREN,
                                "分支主会话 的子会话",
                                "· 4 个会话",
                                root_focus,
                            );
                            if return_key.is_some() {
                                assert!(
                                    view.scroll_left() > px(0.),
                                    "child scroll was not restored"
                                );
                            }
                        })
                        .unwrap();
                    if return_key.is_none() {
                        window
                            .update(cx, |view, window, cx| view.qualify(CHILD, window, cx))
                            .unwrap();
                    }
                    assert_focus(fixture.process(window, cx, &sink), CHILD);
                    draw(window, cx);
                    window
                        .update(cx, |view, window, cx| {
                            view.assert_level(
                                Some(PARENT),
                                CHILDREN,
                                "分支主会话 的子会话",
                                "· 4 个会话",
                                CHILD,
                            );
                            view.assert_terminal_focus(CHILD, false, window, cx);
                        })
                        .unwrap();

                    if return_key.is_none() {
                        scroll(window, cx);
                        press(window, cx, "cmd-d");
                        let mut actions = fixture.process(window, cx, &sink);
                        assert_eq!(actions.len(), 1);
                        window
                            .update(cx, |view, _, _| {
                                view.assert_sibling_command(
                                    actions.pop().unwrap(),
                                    Some(PARENT),
                                    CHILD,
                                );
                            })
                            .unwrap();
                    }

                    if let Some(key) = return_key {
                        window
                            .update(cx, |view, window, _| view.focus_return_button(window))
                            .unwrap();
                        // Key listeners use the dispatch path from the most recent frame.
                        draw(window, cx);
                        press(window, cx, key);
                    } else {
                        let position = window
                            .update(cx, |_, window, _| {
                                point(px(300.), window.viewport_size().height - px(19.))
                            })
                            .unwrap();
                        mouse(window, cx, position, NSEventType::NSLeftMouseDown);
                        mouse(window, cx, position, NSEventType::NSLeftMouseUp);
                    }
                    assert_focus(fixture.process(window, cx, &sink), PARENT);
                    draw(window, cx);
                    window
                        .update(cx, |view, window, cx| {
                            view.assert_level(None, ROOTS, "根会话", "· 2 个会话", PARENT);
                            assert_eq!(view.scroll_left(), px(0.));
                            view.assert_terminal_focus(PARENT, true, window, cx);
                        })
                        .unwrap();
                    println!(
                        "Return through {} focused the parent terminal",
                        return_key.unwrap_or("click")
                    );
                }
                fixture.assert_geometry();

                press(window, cx, "cmd-d");
                let mut actions = fixture.process(window, cx, &sink);
                assert_eq!(actions.len(), 1);
                window
                    .update(cx, |view, _, _| {
                        view.assert_sibling_command(actions.pop().unwrap(), None, PARENT);
                    })
                    .unwrap();

                let badge = window
                    .update(cx, |view, _, _| view.badge_position())
                    .unwrap();
                mouse(window, cx, badge, NSEventType::NSLeftMouseDown);
                mouse(window, cx, badge, NSEventType::NSLeftMouseUp);
                assert_focus(fixture.process(window, cx, &sink), CHILD);
                fixture.focus(SECOND_SCENE, SECOND_ROOT);
                deliver(window, cx, fixture.event(false));
                draw(window, cx);
                window
                    .update(cx, |view, _, _| {
                        view.assert_level(
                            None,
                            &[SECOND_ROOT],
                            "根会话",
                            "· 1 个会话",
                            SECOND_ROOT,
                        );
                    })
                    .unwrap();
                fixture.focus(SCENE, PEER);
                deliver(window, cx, fixture.event(false));
                draw(window, cx);
                window
                    .update(cx, |view, _, _| {
                        view.assert_level(
                            Some(PARENT),
                            CHILDREN,
                            "分支主会话 的子会话",
                            "· 4 个会话",
                            PEER,
                        );
                    })
                    .unwrap();
                sink.generation(2);
                deliver(window, cx, fixture.event(true));
                draw(window, cx);
                window
                    .update(cx, |view, _, _| {
                        view.assert_level(None, ROOTS, "根会话", "· 2 个会话", PEER);
                    })
                    .unwrap();
                println!("Scene overrides survived scene switch and were cleared on reconnect");

                let mut graph = fixture.archived_children_graph();
                deliver(window, cx, Event::Graph(graph.clone()));
                draw(window, cx);
                let badge = window
                    .update(cx, |view, _, _| view.badge_position())
                    .unwrap();
                mouse(window, cx, badge, NSEventType::NSLeftMouseDown);
                mouse(window, cx, badge, NSEventType::NSLeftMouseUp);
                assert!(fixture.process(window, cx, &sink).is_empty());
                draw(window, cx);
                window
                    .update(cx, |view, _, _| {
                        view.assert_level(
                            Some(PARENT),
                            CHILDREN,
                            "分支主会话 的子会话",
                            "· 4 个会话",
                            PEER,
                        );
                    })
                    .unwrap();
                println!("An archived-only level opened without changing focus");

                fixture.set_archived(PARENT, true);
                deliver(window, cx, Event::Graph(fixture.stopped_graph()));
                draw(window, cx);
                let return_button = window
                    .update(cx, |view, window, _| {
                        view.assert_level(
                            Some(PARENT),
                            CHILDREN,
                            "分支主会话 的子会话",
                            "· 4 个会话",
                            PEER,
                        );
                        point(px(300.), window.viewport_size().height - px(19.))
                    })
                    .unwrap();
                click(window, cx, return_button);
                draw(window, cx);
                window
                    .update(cx, |view, _, _| {
                        view.assert_level(None, ROOTS, "根会话", "· 2 个会话", PEER);
                        view.assert_stopped_state(PARENT, Some(PEER));
                    })
                    .unwrap();
                assert_focus(
                    std::iter::from_fn(|| sink.try_recv().ok())
                        .filter(|command| matches!(command, Command::FocusSession { .. }))
                        .collect(),
                    PARENT,
                );
                println!("Return from an archived parent preserves the level change and sends the parent focus request");
                deliver(
                    window,
                    cx,
                    Event::NavigationFailed {
                        message: "probe: archived parent focus rejected".into(),
                        attempted_focus: Some((SCENE.into(), PARENT.into())),
                    },
                );
                deliver(window, cx, Event::StorageRecovered { session_id: PARENT.into() });
                fixture.set_archived(PARENT, false);
                deliver(window, cx, Event::Graph(graph.clone()));
                draw(window, cx);
                let badge = window.update(cx, |view, _, _| view.badge_position()).unwrap();
                click(window, cx, badge);
                graph["nodes"]
                    .as_array_mut()
                    .unwrap()
                    .retain(|node| node["sessionId"] != PARENT);
                deliver(window, cx, Event::Graph(graph.clone()));
                draw(window, cx);
                window
                    .update(cx, |view, _, _| {
                        view.assert_level(None, &[PEER], "根会话", "· 1 个会话", PEER);
                    })
                    .unwrap();
                graph["nodes"].as_array_mut().unwrap().clear();
                deliver(window, cx, Event::Graph(graph));
                draw(window, cx);
                window
                    .update(cx, |view, _, _| {
                        view.assert_level(None, &[], "根会话", "· 0 个会话", PEER);
                    })
                    .unwrap();
                drop(fixture);

                println!("Session levels GPUI probe passed");
                cx.update(|cx| cx.quit()).unwrap();
            })
            .detach();
        });
    }
}

#[cfg(target_os = "macos")]
fn main() {
    probe::run();
}

#[cfg(not(target_os = "macos"))]
fn main() {}
