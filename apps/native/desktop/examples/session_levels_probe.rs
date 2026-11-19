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
        pub(super) fn assert_permission_chip(&self, expected: Option<(&str, &str, bool)>) {
            let chip = self.permission_chip();
            assert_eq!(
                chip.as_ref()
                    .map(|chip| (chip.mode.label(), chip.title.as_str(), chip.disabled)),
                expected
            );
        }

        pub(super) fn assert_permission_menu(&self, open: bool) {
            assert_eq!(self.permission_menu.is_some(), open);
        }

        pub(super) fn assert_permission_confirmation(&self, expected: (&str, &str, &str)) {
            let Some(TabDialog::PermissionMode(state)) = &self.dialog else {
                panic!("expected permission confirmation");
            };
            assert_eq!(state.copy(), expected);
        }

        pub(super) fn assert_permission_toast(&self, expected: Option<&str>) {
            assert_eq!(self.permission_toast.as_deref(), expected);
        }

        pub(super) fn permission_chip_point(&self, window: &Window) -> Point<Pixels> {
            let viewport = window.viewport_size();
            point(viewport.width - px(182.), viewport.height - px(18.5))
        }

        pub(super) fn permission_row_point(&self, index: usize, window: &Window) -> Point<Pixels> {
            let viewport = window.viewport_size();
            let row_center_from_bottom = 7. + 26. * (4 - index) as f32 + 13.;
            point(
                viewport.width - px(140.),
                viewport.height - px(34.5 + row_center_from_bottom),
            )
        }

        pub(super) fn assert_permission_colors(
            &self,
            index: usize,
            color: u32,
            cx: &mut Context<Self>,
        ) {
            let mode = PermissionMode::ALL[index];
            let chip = self.permission_chip().unwrap();
            assert_eq!(chip.mode, mode);
            let mut view = self.permission_chip_view(&chip, cx);
            let style = view.style().text.as_ref().unwrap();
            assert_eq!(style.color, Some(rgb(color).into()));
            assert_eq!(style.font_size, Some(px(10.).into()));
            assert_eq!(style.font_weight, Some(FontWeight::SEMIBOLD));
            assert_eq!(
                permission_menu_dot_color(mode),
                if index == 1 { 0x8b949e } else { color }
            );
            assert_eq!(
                permission_chip_text(mode).as_ref(),
                mode.label().to_ascii_uppercase()
            );
            assert_eq!(PERMISSION_CHIP_TRACKING, 0.2);
            assert_eq!(PERMISSION_MENU_MIN_WIDTH, 180.);
            assert_eq!(PERMISSION_MENU_GAP, 8.);
            assert_eq!(PERMISSION_MENU_TITLE, "权限模式");
            assert_eq!(PERMISSION_MENU_CHECK, "✓");
            let mut active = permission_menu_item(mode, mode);
            let mut inactive = permission_menu_item(PermissionMode::ALL[(index + 1) % 5], mode);
            assert_eq!(
                active.style().text.as_ref().unwrap().color,
                Some(rgb(0x58a6ff).into())
            );
            assert_eq!(
                inactive.style().text.as_ref().unwrap().color,
                Some(rgb(0xc9d1d9).into())
            );
            assert_ne!(active.style().background, inactive.style().background);
        }

        pub(super) fn fork_input(&self) -> Entity<RenameInput> {
            let Some(TabDialog::Branch(state)) = &self.dialog else {
                panic!("expected branch dialog");
            };
            state.input.clone()
        }

        pub(super) fn assert_fork_dialog(
            &self,
            placement: ForkPlacement,
            title: &str,
            description: &str,
            hint: &str,
            error: Option<&str>,
            submitting: bool,
            cx: &App,
        ) -> String {
            let Some(TabDialog::Branch(state)) = &self.dialog else {
                panic!("expected branch dialog");
            };
            assert_eq!(state.placement, placement);
            assert_eq!(state.copy(), (title, description.to_owned(), hint));
            assert_eq!(state.input.read(cx).placeholder, "例如：修复登录流程");
            assert_eq!(state.error.as_deref(), error);
            assert_eq!(state.submitting, submitting);
            state.submission_key.clone()
        }

        pub(super) fn assert_fork_access(
            &self,
            id: &str,
            placement: ForkPlacement,
            expected: Option<(Option<&str>, Option<&str>)>,
        ) {
            assert_eq!(
                self.fork_access(id, placement)
                    .map(|access| (access.blocked, access.readiness)),
                expected
            );
        }

        pub(super) fn focus_fork_control(&self, id: &str, control: usize, window: &mut Window) {
            window.focus(&self.fork_controls[id][control]);
        }

        pub(super) fn focus_fork_menu(&self, window: &mut Window) {
            window.focus(&self.fork_menu.as_ref().expect("card context menu").2);
        }

        pub(super) fn assert_fork_menu_closed(&self) {
            assert!(self.fork_menu.is_none());
        }

        pub(super) fn focus_fork_dialog_control(&self, control: usize, window: &mut Window) {
            window.focus(&self.dialog_controls[control]);
        }

        pub(super) fn assert_fork_remove_dialog(&self, id: &str) {
            assert!(
                matches!(&self.dialog, Some(TabDialog::RemoveSession { session_id, .. }) if session_id == id)
            );
        }

        pub(super) fn assert_fork_level(&self, parent: &str, new: &str) {
            assert_eq!(self.level_parent(), Some(parent));
            assert!(self.order.iter().any(|id| id == new));
        }

        pub(super) fn return_from_fork_level(
            &mut self,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            self.return_to_parent(window, cx);
        }

        pub(super) fn assert_fork_readiness(&self, message: Option<&str>) {
            assert_eq!(self.fork_readiness, message);
        }

        pub(super) fn fork_readiness_visible(&self) -> bool {
            self.fork_readiness.is_some()
        }

        pub(super) fn assert_fork_failure(
            &self,
            id: &str,
            failure: (&str, Option<&str>),
            blocked: Option<&str>,
            error: Option<&str>,
        ) {
            let node = self.graph_node(id).unwrap();
            assert_eq!(fork_failure_copy(node), Some(failure));
            assert_eq!(self.fork_repair_blocked_reason(id, node), blocked);
            assert_eq!(
                self.fork_retries.get(id).and_then(|retry| match retry {
                    ForkRetry::Pending => None,
                    ForkRetry::Rejected(error) => Some(error.as_str()),
                }),
                error
            );
        }

        pub(super) fn assert_fork_stage(&self, id: &str, stage: &str) {
            let node = self.graph_node(id).unwrap();
            assert_eq!(node["forkProgress"]["stage"], stage);
            let progress = fork_progress_copy(node).unwrap();
            if stage == "queued" {
                assert_eq!(progress.label, "正在创建分支：等待创建分支");
                assert_eq!(progress.counter.as_deref(), Some("第 1 / 2 阶段"));
                assert!(!progress.interactive);
            } else {
                assert_eq!(progress.label, "正在创建分支：正在恢复智能体会话");
                assert!(progress.interactive);
            }
        }

        pub(super) fn assert_fork_worktree_copy(&self) {
            assert_eq!(FORK_WORKTREE_SECTION, "工作目录");
            assert_eq!(FORK_USE_CURRENT_WORKTREE, "使用当前工作树");
            assert_eq!(FORK_USE_NEW_WORKTREE, "从新工作树创建");
            assert_eq!(
                FORK_NEW_WORKTREE_HINT,
                "创建隔离的 Git worktree，适合多个功能并行开发"
            );
            assert_eq!(FORK_NEEDS_GIT_REPOSITORY, "需要 Git 仓库");
            assert_eq!(FORK_DIALOG_CREATING, "正在创建分支…");
        }

        pub(super) fn fork_card_header(&self, id: &str) -> Point<Pixels> {
            let index = self
                .order
                .iter()
                .position(|candidate| candidate == id)
                .unwrap();
            let bounds = self.carousel_scroll.bounds_for_item(index).unwrap();
            bounds.origin + self.carousel_scroll.offset() + point(px(32.), px(20.))
        }

        pub(super) fn set_fork_input(
            &self,
            text: &str,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            let input = self.fork_input();
            input.update(cx, |input, cx| {
                let len = input.text.encode_utf16().count();
                input.replace_text_in_range(Some(0..len), text, window, cx);
            });
        }

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

        pub(super) fn loader_cwd(&self) -> String {
            self.graph_node(super::probe::PEER).unwrap()["cwd"]
                .as_str()
                .unwrap()
                .into()
        }

        pub(super) fn loader_blocked(&self) -> Option<&'static str> {
            let id = super::probe::PEER;
            self.session_loader_blocked_reason(id, self.graph_node(id).unwrap())
        }

        pub(super) fn loader_button_position(&self, id: &str) -> Point<Pixels> {
            let fork_count = [ForkPlacement::Child, ForkPlacement::Sibling]
                .into_iter()
                .filter(|placement| self.fork_access(id, *placement).is_some())
                .count();
            self.remove_button_position(id) - point(px(31.5 + 30. * fork_count as f32), px(0.))
        }

        fn loader(&self) -> &SessionLoader {
            let Some(TabDialog::SessionLoader(state)) = &self.dialog else {
                panic!("expected the session loader");
            };
            state
        }

        pub(super) fn assert_loader_open(&self, window: &Window, cx: &Context<Self>) {
            let state = self.loader();
            let input = state.input.read(cx);
            assert_eq!(input.placeholder, "筛选左侧：标题、路径、模型或会话 ID");
            assert!(input.focus.is_focused(window));
            assert!(input.search_style);
            assert!(state.loading_list);
            assert!(state.list_scroll.bounds().size.width >= px(220.));
            assert!(state.preview_scroll.bounds().size.width > px(250.));
            assert_eq!(SESSION_LOADER_TITLE, "载入 Claude Code 会话");
            assert_eq!(SESSION_LOADER_LOADING_MORE, "正在载入更多会话…");
        }

        pub(super) fn assert_loader_list(
            &self,
            visible: &[&str],
            selected: Option<&str>,
            cx: &Context<Self>,
        ) {
            let state = self.loader();
            assert_eq!(state.cwd, self.loader_cwd());
            assert_eq!(state.selected.as_deref(), selected);
            let rows: Vec<_> = state.visible_sessions(cx).collect();
            assert_eq!(
                rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
                visible
            );
            if let Some(row) = rows.iter().find(|row| row.id == "history-a") {
                assert_eq!(row.title, "Alpha planning");
                assert_eq!(row.message_count, 205);
                assert_eq!(row.model.as_deref(), Some("Sonnet"));
                assert_eq!(row.permission_mode, "plan");
                let owner = row.loaded_by.as_ref().unwrap();
                assert_eq!(
                    (owner.session_id.as_str(), owner.title.as_str()),
                    (PARENT, "分支主会话")
                );
            }
        }

        pub(super) fn assert_loader_preview(&self, indices: &[usize], total: usize, earlier: bool) {
            let state = self.loader();
            assert_eq!(
                state
                    .events
                    .iter()
                    .map(|event| event.index)
                    .collect::<Vec<_>>(),
                indices
            );
            assert_eq!(state.total_events, total);
            assert_eq!(state.has_earlier, earlier);
            assert_eq!(SESSION_LOADER_LOADING_EARLIER, "正在载入更早内容…");
            if !indices.is_empty() {
                assert_eq!(
                    state.preview_scroll.children_count(),
                    indices.len() + usize::from(earlier)
                );
            }
            if indices.contains(&8) {
                let latest = &state.events[state.events.len() - 4..];
                assert_eq!(
                    latest
                        .iter()
                        .map(|event| event.kind.as_str())
                        .collect::<Vec<_>>(),
                    ["user", "assistant", "tool", "system"]
                );
                assert_eq!(latest[0].text, "第一行\n第二行");
                assert_eq!(latest[2].tool_name.as_deref(), Some("Read"));
                assert_eq!(
                    latest
                        .iter()
                        .map(|event| loader_event_label(&event.kind, event.tool_name.as_deref()))
                        .collect::<Vec<_>>(),
                    ["你", "Claude", "工具 · Read", "系统"]
                );
            }
        }

        pub(super) fn loader_filter(
            &self,
            query: &str,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            self.loader().input.update(cx, |input, cx| {
                let length = input.text.encode_utf16().count();
                input.replace_text_in_range(Some(0..length), query, window, cx);
            });
        }

        pub(super) fn focus_loader_control(&self, index: usize, window: &mut Window) {
            window.focus(&self.dialog_controls[index]);
        }

        pub(super) fn focus_loader_list(&self, window: &mut Window) {
            window.focus(&self.loader().list_focus);
        }

        pub(super) fn assert_loader_submit(
            &self,
            running: bool,
            enabled: bool,
            error: Option<&str>,
        ) {
            let state = self.loader();
            assert_eq!(self.session_loader_running(&state.session_id), running);
            assert_eq!(self.session_loader_can_submit(state), enabled);
            assert_eq!(state.error.as_deref(), error);
            if running {
                assert_eq!(
                    SESSION_LOADER_RUNNING_WARNING,
                    "当前卡片正在运行，继续会结束当前进程。"
                );
            }
        }

        pub(super) fn deliver_loader_bridge_error(
            &mut self,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            self.apply(
                BridgeEvent::TransportDiagnostic {
                    generation: self.commands.connection().0,
                    message: "probe: runtime connection lost".into(),
                },
                window,
                cx,
            );
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
        AnyWindowHandle, AppContext, Application, AsyncApp, Bounds, Keystroke, MouseButton, Pixels,
        Point, WindowBounds, WindowHandle, WindowOptions, point, px, size,
    };
    use matou_native_core::{
        geometry,
        hierarchy::{
            self, BootstrapWindowInput, HierarchyIds,
            canvas::{
                self, CanvasIds, CanvasMessages, CreateCanvasInput, CreateShellSiblingInput,
                ForkPlacement, ForkSessionInput, ForkWorktreeMode,
            },
            cards::{RemoveScope, scene_workspace_session_count},
        },
        session_graph::project_scene_graph,
        sessions::fail_claude_fork,
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
        fn fork_source(&self, ready: bool, work_status: &str) {
            let Some(BootstrapResult::Writable { connection, .. }) = &self.db else {
                unreachable!()
            };
            connection
                .execute(
                    "UPDATE sessions SET kind = 'claude-code', work_status = ? WHERE id = ?",
                    (work_status, PARENT),
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO provider_bindings (
                         id, session_id, provider, provider_session_id, resume_state,
                         metadata_json, created_at, updated_at, validated_at
                     ) VALUES ('probe-fork-parent-binding', ?, 'claude-code', 'probe-provider',
                         'resumed', ?, 20, 20, 20)
                     ON CONFLICT(id) DO UPDATE SET metadata_json = excluded.metadata_json",
                    (PARENT, serde_json::json!({ "canFork": ready }).to_string()),
                )
                .unwrap();
        }

        fn create_fork(&mut self, id: &str, request: Command) -> serde_json::Value {
            let Command::ForkSession {
                scene_id,
                source_session_id,
                placement,
                name,
                worktree_mode,
                submission_key,
            } = request
            else {
                panic!("expected ForkSession");
            };
            let Some(BootstrapResult::Writable { connection, .. }) = &mut self.db else {
                unreachable!()
            };
            canvas::create_fork_child(
                connection,
                &command(id),
                &ForkSessionInput {
                    window_id: WINDOW.into(),
                    scene_id,
                    source_session_id,
                    placement,
                    name,
                    worktree_mode,
                    submission_key,
                    now: 30,
                },
                &canvas_ids(SCENE, id),
                &MESSAGES,
                30,
            )
            .unwrap()
        }

        fn fail_fork(&mut self, id: &str, reason: &str, sequence: i64) {
            let Some(BootstrapResult::Writable { connection, .. }) = &mut self.db else {
                unreachable!()
            };
            fail_claude_fork(
                connection,
                &command(&format!("fork-failure-{sequence}")),
                id,
                reason,
                sequence,
            )
            .unwrap();
        }

        fn retry_fork(&mut self, id: &str) -> serde_json::Value {
            let Some(BootstrapResult::Writable { connection, .. }) = &mut self.db else {
                unreachable!()
            };
            canvas::retry_fork(connection, &command("probe-retry"), WINDOW, id, 33).unwrap()
        }

        fn start_fork(&self, id: &str) {
            let Some(BootstrapResult::Writable { connection, .. }) = &self.db else {
                unreachable!()
            };
            connection
                .execute(
                    "UPDATE session_fork_intents SET state = 'starting',
                   stage = 'restoring-provider', completed_steps = 1 WHERE session_id = ?",
                    [id],
                )
                .unwrap();
        }

        fn fork_reply(&self, result: serde_json::Value, navigation: bool) -> Event {
            let Event::Navigation {
                hierarchy,
                scenes,
                mut graph,
                geometry,
            } = self.event(false)
            else {
                unreachable!()
            };
            graph["forkSessionResult"] = result;
            if navigation {
                Event::Navigation {
                    hierarchy,
                    scenes,
                    graph,
                    geometry,
                }
            } else {
                Event::Graph(graph)
            }
        }

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

        fn commit_catalog_load(&mut self, cwd: &str) {
            use matou_native_core::sessions::{ClaudeCatalogLoad, load_claude_catalog_binding};

            let Some(BootstrapResult::Writable { connection, .. }) = &mut self.db else {
                unreachable!()
            };
            load_claude_catalog_binding(
                connection,
                &command("probe-load-history"),
                &ClaudeCatalogLoad {
                    binding_id: "probe-loaded-binding",
                    session_id: PEER,
                    scene_id: SCENE,
                    cwd,
                    provider_session_id: "history-b",
                    title: "Beta draft",
                    model: Some("Haiku"),
                    permission_mode: "default",
                },
                24,
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

        fn permission_mode(&self, mode: Option<&str>) {
            let Some(BootstrapResult::Writable { connection, .. }) = &self.db else {
                unreachable!()
            };
            if let Some(mode) = mode {
                connection
                    .execute(
                        "UPDATE provider_bindings SET metadata_json =
                         json_set(metadata_json, '$.permissionMode', ?) WHERE session_id = ?",
                        (mode, PARENT),
                    )
                    .unwrap();
            } else {
                connection
                    .execute(
                        "DELETE FROM provider_bindings WHERE session_id = ?",
                        [PARENT],
                    )
                    .unwrap();
            }
        }

        fn permission_result(&self, mode: &str, error: Option<&str>) -> Event {
            let Event::Graph(mut graph) = self.graph_event() else {
                unreachable!()
            };
            let mut result = serde_json::json!({"sessionId": PARENT, "permissionMode": mode});
            if let Some(error) = error {
                result["error"] = error.into();
            }
            graph["providerPermissionModeResults"] = serde_json::json!([result]);
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
                    | Command::SetProviderPermissionMode { .. }
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
        use core_foundation::base::CFRelease;
        use core_graphics::{
            display::CGDisplay,
            event::{CGEventType, CGMouseButton},
            geometry::CGPoint,
        };
        use objc::{msg_send, sel, sel_impl};
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use std::ffi::c_void;

        unsafe extern "C" {
            fn CGEventCreateMouseEvent(
                source: *const c_void,
                kind: CGEventType,
                location: CGPoint,
                button: CGMouseButton,
            ) -> *mut c_void;
        }

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
            if matches!(
                kind,
                NSEventType::NSRightMouseDown | NSEventType::NSRightMouseUp
            ) {
                // The AppKit mouse-event constructor above always sets buttonNumber to 0.
                let down = kind == NSEventType::NSRightMouseDown;
                let event = CGEventCreateMouseEvent(
                    std::ptr::null(),
                    if down {
                        CGEventType::RightMouseDown
                    } else {
                        CGEventType::RightMouseUp
                    },
                    CGPoint::new(
                        location.x,
                        CGDisplay::main().bounds().size.height - location.y,
                    ),
                    CGMouseButton::Right,
                );
                assert!(!event.is_null());
                let native = NSEvent::eventWithCGEvent_(nil, event);
                CFRelease(event);
                assert_eq!(native.buttonNumber(), 1);
                assert_eq!(native.locationInWindow().x, location.x);
                assert_eq!(native.locationInWindow().y, location.y);
                if down {
                    let _: () = msg_send![view, rightMouseDown: native];
                } else {
                    let _: () = msg_send![view, rightMouseUp: native];
                }
                return;
            }
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

    fn fork_click(
        window: WindowHandle<Workspace>,
        cx: &mut AsyncApp,
        button: MouseButton,
        position: Point<Pixels>,
    ) {
        match button {
            MouseButton::Left => click(window, cx, position),
            MouseButton::Right => {
                mouse(window, cx, position, NSEventType::NSRightMouseDown);
                mouse(window, cx, position, NSEventType::NSRightMouseUp);
            }
            _ => unreachable!(),
        }
    }

    async fn verify_fork(cx: &mut AsyncApp) {
        use ForkPlacement::{Child, Peer, Sibling};
        use serde_json::json;

        const BRANCH_HINT: &str = "和父会话使用同一目录，适合连续处理同一份改动";
        const PEER_HINT: &str = "和当前会话使用同一目录，适合连续处理同一份改动";
        const EXPIRED_REASON: &str = "原 Claude Code 对话身份已失效，本次分支没有创建成功。请返回父会话继续，或移除此失败节点后新建空会话。";
        const FAILED: &str = "probe-created-peer";
        let mut fixture = Fixture::new();
        fixture.fork_source(false, "needs-input");
        fixture.focus(SCENE, PARENT);
        let (window, sink) = open_hidden(cx, 1250.);
        deliver(window, cx, fixture.event(true));
        draw(window, cx);
        let assert_closed = |cx: &mut AsyncApp| {
            window
                .update(cx, |view, _, _| view.assert_no_dialog())
                .unwrap();
        };
        let qualify = |cx: &mut AsyncApp, id: &str| {
            draw(window, cx);
            window
                .update(cx, |view, window, cx| view.qualify(id, window, cx))
                .unwrap();
        };
        let enter = |cx: &mut AsyncApp| {
            AnyWindowHandle::from(window)
                .update(cx, |_, window, cx| {
                    window.dispatch_keystroke(Keystroke::parse("enter").unwrap(), cx);
                })
                .unwrap();
        };
        let control = |cx: &mut AsyncApp, id: &str, index: usize| {
            draw(window, cx);
            window
                .update(cx, |view, window, _| {
                    view.focus_fork_control(id, index, window)
                })
                .unwrap();
            enter(cx);
            draw(window, cx);
        };
        let menu_item = |cx: &mut AsyncApp, id: &str| {
            draw(window, cx);
            let position = window
                .update(cx, |view, _, _| view.fork_card_header(id))
                .unwrap();
            fork_click(window, cx, MouseButton::Right, position);
            draw(window, cx);
            window
                .update(cx, |view, window, _| view.focus_fork_menu(window))
                .unwrap();
            press(window, cx, "enter");
            draw(window, cx);
        };
        let name = |cx: &mut AsyncApp, text: &str| {
            window
                .update(cx, |view, window, cx| view.set_fork_input(text, window, cx))
                .unwrap();
        };
        let dialog_control = |cx: &mut AsyncApp, index: usize| {
            draw(window, cx);
            window
                .update(cx, |view, window, _| {
                    view.focus_fork_dialog_control(index, window)
                })
                .unwrap();
            enter(cx);
            draw(window, cx);
        };
        let commands = || {
            std::iter::from_fn(|| sink.try_recv().ok())
                .filter(|command| {
                    matches!(
                        command,
                        Command::ForkSession { .. }
                            | Command::RetryFork { .. }
                            | Command::RemoveSessionBranch { .. }
                    )
                })
                .collect::<Vec<_>>()
        };
        window
            .update(cx, |view, _, _| {
                view.assert_fork_access(PARENT, Sibling, None);
                view.assert_fork_access(CHILD, Sibling, None);
                view.assert_fork_access(PEER, Child, None);
                view.assert_fork_access(
                    PARENT,
                    Child,
                    Some((Some("当前终端仍在恢复"), Some("当前回复完成后即可 Fork"))),
                );
            })
            .unwrap();
        control(cx, PARENT, 0);
        assert_closed(cx);
        let header = window
            .update(cx, |view, _, _| view.fork_card_header(PARENT))
            .unwrap();
        fork_click(window, cx, MouseButton::Right, header);
        window
            .update(cx, |view, _, _| {
                view.assert_fork_readiness(None);
                view.assert_fork_menu_closed();
            })
            .unwrap();
        assert!(commands().is_empty());
        qualify(cx, PARENT);
        control(cx, PARENT, 0);
        assert_closed(cx);
        window
            .update(cx, |view, _, _| {
                view.assert_fork_readiness(Some("当前回复完成后即可 Fork"))
            })
            .unwrap();
        let until = Instant::now() + Duration::from_secs(5);
        while window
            .update(cx, |view, _, _| view.fork_readiness_visible())
            .unwrap()
        {
            assert!(Instant::now() < until, "fork readiness did not expire");
            cx.background_executor()
                .timer(Duration::from_millis(100))
                .await;
        }
        fixture.fork_source(false, "idle");
        deliver(window, cx, fixture.graph_event());
        menu_item(cx, PARENT);
        assert_closed(cx);
        window
            .update(cx, |view, _, _| {
                view.assert_fork_readiness(Some(
                    "在当前会话输入一次，并等待 Claude Code 完成回复后，即可创建分支",
                ));
            })
            .unwrap();
        assert!(commands().is_empty());
        println!(
            "Fork entries | root hides sibling, unsupported profile hides child, recovering is blocked, child and peer show timed readiness"
        );

        fixture.fork_source(true, "idle");
        deliver(window, cx, fixture.graph_event());
        let mut keys = Vec::new();
        for (placement, source, new, title, description, hint) in [
            (
                Child,
                PARENT,
                "probe-created-child",
                "创建子会话分支",
                "从“分支主会话”继续一条独立工作路径",
                BRANCH_HINT,
            ),
            (
                Sibling,
                CHILD,
                "probe-created-sibling",
                "创建同级分支",
                "从“分支主会话”继续一条独立工作路径",
                BRANCH_HINT,
            ),
            (
                Peer,
                PARENT,
                FAILED,
                "Fork 会话",
                "复制“分支主会话”的当前对话并加入当前列表",
                PEER_HINT,
            ),
        ] {
            if placement == Peer {
                window
                    .update(cx, |view, window, cx| {
                        view.return_from_fork_level(window, cx)
                    })
                    .unwrap();
            }
            fixture.focus(SCENE, source);
            deliver(window, cx, fixture.event(false));
            qualify(cx, source);
            match placement {
                Child => control(cx, source, 0),
                Sibling => control(cx, source, 1),
                Peer => menu_item(cx, source),
            }
            let assert_dialog = |cx: &mut AsyncApp, error, submitting| {
                window
                    .update(cx, |view, _, cx| {
                        view.assert_fork_dialog(
                            placement,
                            title,
                            description,
                            hint,
                            error,
                            submitting,
                            cx,
                        )
                    })
                    .unwrap()
            };
            let key = assert_dialog(cx, None, false);
            assert!(!key.is_empty() && !keys.contains(&key));
            if placement == Child {
                let disabled_option = window
                    .update(cx, |view, window, _| {
                        view.assert_fork_worktree_copy();
                        let viewport = window.viewport_size();
                        point(viewport.width / 2., viewport.height / 2. + px(80.))
                    })
                    .unwrap();
                click(window, cx, disabled_option);
                assert_dialog(cx, None, false);
                dialog_control(cx, 2);
                assert_dialog(cx, Some("请输入分支名称"), false);
                name(cx, &"中".repeat(65));
                dialog_control(cx, 2);
                assert_dialog(cx, Some("分支名称最多 64 个字符"), false);
                assert!(commands().is_empty());
            }
            name(cx, &format!("  {new}  "));
            dialog_control(cx, 2);
            assert_eq!(assert_dialog(cx, None, true), key);
            press(window, cx, "escape");
            fork_click(window, cx, MouseButton::Left, point(px(8.), px(85.)));
            dialog_control(cx, 2);
            assert_eq!(assert_dialog(cx, None, true), key);
            let take_submission = || {
                let mut sent = commands();
                assert_eq!(sent.len(), 1);
                let request = sent.pop().unwrap();
                assert!(matches!(&request, Command::ForkSession {
                    scene_id, source_session_id, placement: actual, name, worktree_mode, submission_key
                } if scene_id == SCENE && source_session_id == source && actual == &placement
                    && name == new && *worktree_mode == ForkWorktreeMode::Current && submission_key == &key));
                request
            };
            let mut request = take_submission();
            if placement == Child {
                deliver(
                    window,
                    cx,
                    fixture.fork_reply(
                        json!({
                            "submissionKey": "unrelated", "error": "ignored"
                        }),
                        false,
                    ),
                );
                assert_dialog(cx, None, true);
                deliver(
                    window,
                    cx,
                    fixture.fork_reply(
                        json!({
                            "submissionKey": key, "error": "原始 Fork 错误"
                        }),
                        false,
                    ),
                );
                assert_eq!(assert_dialog(cx, Some("原始 Fork 错误"), false), key);
                dialog_control(cx, 2);
                request = take_submission();
            }
            let mut result = fixture.create_fork(new, request);
            result["submissionKey"] = json!(key);
            keys.push(key);
            deliver(window, cx, fixture.fork_reply(result, placement != Sibling));
            assert_closed(cx);
            draw(window, cx);
            window
                .update(cx, |view, _, _| view.assert_fork_stage(new, "queued"))
                .unwrap();
            if placement != Peer {
                window
                    .update(cx, |view, _, _| view.assert_fork_level(PARENT, new))
                    .unwrap();
            }
        }
        println!(
            "Fork dialog | three placements, frozen copy, Unicode validation, current worktree payload, duplicate suppression, error, same key, Graph and Navigation completion"
        );

        fixture.start_fork(FAILED);
        deliver(window, cx, fixture.graph_event());
        draw(window, cx);
        window
            .update(cx, |view, _, _| {
                view.assert_fork_stage(FAILED, "restoring-provider")
            })
            .unwrap();
        fixture.fail_fork(FAILED, "No Conversation Found", 32);
        deliver(window, cx, fixture.graph_event());
        let failed = |cx: &mut AsyncApp, title, reason, blocked, error| {
            window
                .update(cx, |view, _, _| {
                    view.assert_fork_failure(FAILED, (title, Some(reason)), blocked, error);
                })
                .unwrap();
        };
        failed(cx, "父会话已失效", EXPIRED_REASON, None, None);
        deliver(
            window,
            cx,
            Event::Recovery {
                session_id: FAILED.into(),
                state: RecoveryState::Restoring,
            },
        );
        failed(
            cx,
            "父会话已失效",
            EXPIRED_REASON,
            Some("当前终端仍在恢复"),
            None,
        );
        deliver(
            window,
            cx,
            Event::Recovery {
                session_id: FAILED.into(),
                state: RecoveryState::Failed("No Conversation Found".into()),
            },
        );
        deliver(
            window,
            cx,
            Event::StorageFault {
                session_id: FAILED.into(),
                message: "probe storage".into(),
            },
        );
        failed(
            cx,
            "父会话已失效",
            EXPIRED_REASON,
            Some("终端存储异常，请先恢复或结束当前会话"),
            None,
        );
        deliver(
            window,
            cx,
            Event::StorageRecovered {
                session_id: FAILED.into(),
            },
        );
        sink.read_only();
        failed(
            cx,
            "父会话已失效",
            EXPIRED_REASON,
            Some("数据库处于只读恢复模式"),
            None,
        );
        sink.generation(1);
        control(cx, FAILED, 2);
        control(cx, FAILED, 2);
        assert!(
            matches!(commands().as_slice(), [Command::RetryFork { session_id }] if session_id == FAILED)
        );
        deliver(
            window,
            cx,
            fixture.fork_reply(
                json!({
                    "sessionId": FAILED, "error": "当前分支无需重试"
                }),
                false,
            ),
        );
        failed(
            cx,
            "父会话已失效",
            EXPIRED_REASON,
            None,
            Some("当前分支无需重试"),
        );
        control(cx, FAILED, 3);
        window
            .update(cx, |view, _, _| view.assert_fork_remove_dialog(FAILED))
            .unwrap();
        assert!(commands().is_empty());
        press(window, cx, "escape");
        assert_closed(cx);
        control(cx, FAILED, 2);
        assert!(
            matches!(commands().as_slice(), [Command::RetryFork { session_id }] if session_id == FAILED)
        );
        let result = fixture.retry_fork(FAILED);
        deliver(window, cx, fixture.fork_reply(result, true));
        draw(window, cx);
        window
            .update(cx, |view, _, _| view.assert_fork_stage(FAILED, "queued"))
            .unwrap();
        fixture.fail_fork(FAILED, "probe launch error", 34);
        deliver(window, cx, fixture.graph_event());
        failed(cx, "分支创建失败", "probe launch error", None, None);
        control(cx, FAILED, 3);
        window
            .update(cx, |view, _, _| view.assert_fork_remove_dialog(FAILED))
            .unwrap();
        dialog_control(cx, 2);
        assert_remove_command(commands(), FAILED, RemoveScope::NodeOnly);
        println!(
            "Fork failure | expired parent mapping, recovery/storage/read-only reasons, deduplicated RetryFork, original rejection and remove entry"
        );

        fixture.focus(SCENE, PARENT);
        deliver(window, cx, fixture.event(false));
        qualify(cx, PARENT);
        control(cx, PARENT, 0);
        press(window, cx, "escape");
        assert_closed(cx);
        control(cx, PARENT, 0);
        fork_click(window, cx, MouseButton::Left, point(px(8.), px(85.)));
        assert_closed(cx);
        control(cx, PARENT, 0);
        dialog_control(cx, 0);
        assert_closed(cx);
        control(cx, PARENT, 0);
        let Event::Graph(mut removed) = fixture.graph_event() else {
            unreachable!()
        };
        removed["nodes"]
            .as_array_mut()
            .unwrap()
            .retain(|node| node["sessionId"] != PARENT);
        deliver(window, cx, Event::Graph(removed));
        assert_closed(cx);
        assert!(commands().is_empty());
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        println!("Fork UI hidden GPUI probe passed");
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

    fn verify_session_loader(cx: &mut AsyncApp) {
        use serde_json::{Value, json};

        let mut fixture = Fixture::new();
        let Some(BootstrapResult::Writable { connection, .. }) = &fixture.db else {
            unreachable!()
        };
        connection
            .execute("UPDATE sessions SET title = '目标卡' WHERE id = ?1", [PEER])
            .unwrap();
        let (window, sink) = open_hidden(cx, 1200.);
        deliver(window, cx, fixture.event(true));
        draw(window, cx);
        window
            .update(cx, |view, window, cx| view.qualify(PEER, window, cx))
            .unwrap();
        draw(window, cx);
        let cwd = window.update(cx, |view, _, _| view.loader_cwd()).unwrap();
        let assert_blocked = |cx: &mut AsyncApp, expected| {
            window
                .update(cx, |view, _, _| assert_eq!(view.loader_blocked(), expected))
                .unwrap();
        };
        let click_entry = |cx: &mut AsyncApp| {
            draw(window, cx);
            let at = window
                .update(cx, |view, _, _| view.loader_button_position(PEER))
                .unwrap();
            click(window, cx, at);
            draw(window, cx);
        };
        let commands = |sink: &ProbeReceiver| {
            std::iter::from_fn(|| sink.try_recv().ok())
                .filter(|command| {
                    matches!(
                        command,
                        Command::ListProviderSessions { .. }
                            | Command::GetProviderSession { .. }
                            | Command::LoadProviderSession { .. }
                    )
                })
                .collect::<Vec<_>>()
        };
        let assert_closed = |cx: &mut AsyncApp| {
            window
                .update(cx, |view, _, _| view.assert_no_dialog())
                .unwrap();
        };
        for (event, reason) in [
            (
                Event::WorkspacePaths(vec![WorkspacePathState {
                    workspace_id: "levels-workspace".into(),
                    status: "invalid".into(),
                    reason: "probe path".into(),
                    checked_at: 1,
                    validation_generation: 1,
                }]),
                "工作区目录不可用，请先在本地恢复原路径，或移出该工作区",
            ),
            (
                Event::StorageFault {
                    session_id: PEER.into(),
                    message: "probe storage".into(),
                },
                "终端存储异常，请先恢复或结束当前会话",
            ),
        ] {
            deliver(window, cx, event);
            assert_blocked(cx, Some(reason));
            click_entry(cx);
            assert_closed(cx);
            assert!(commands(&sink).is_empty());
            deliver(window, cx, Event::WorkspacePaths(Vec::new()));
        }
        deliver(
            window,
            cx,
            Event::StorageRecovered {
                session_id: PEER.into(),
            },
        );
        sink.read_only();
        assert_blocked(cx, Some("数据库处于只读恢复模式"));
        click_entry(cx);
        assert_closed(cx);
        assert!(commands(&sink).is_empty());
        sink.generation(1);
        assert_blocked(cx, None);

        let open = |cx: &mut AsyncApp| {
            click_entry(cx);
            window
                .update(cx, |view, window, cx| view.assert_loader_open(window, cx))
                .unwrap();
            assert!(
                matches!(commands(&sink).as_slice(), [Command::ListProviderSessions { cwd: path }]
                if path == &cwd)
            );
        };
        let list = |cx: &mut AsyncApp, visible: &[&str], selected: Option<&str>| {
            window
                .update(cx, |view, _, cx| {
                    view.assert_loader_list(visible, selected, cx)
                })
                .unwrap();
        };
        let detail = |id: &str, start: Option<i64>, limit: Option<i64>| {
            assert!(
                matches!(commands(&sink).as_slice(), [Command::GetProviderSession {
                cwd: path, provider_session_id, start_event_index, limit: count,
            }] if path == &cwd && provider_session_id == id && *start_event_index == start && *count == limit)
            );
        };
        let graph =
            |fixture: &Fixture, cx: &mut AsyncApp, key: &str, result: Value, running: bool| {
                let Event::Graph(mut graph) = fixture.graph_event() else {
                    unreachable!()
                };
                if running {
                    let card = graph["nodes"]
                        .as_array_mut()
                        .unwrap()
                        .iter_mut()
                        .find(|node| node["sessionId"] == PEER)
                        .unwrap();
                    card["workStatus"] = json!("running");
                }
                graph[key] = json!([result]);
                deliver(window, cx, Event::Graph(graph));
                draw(window, cx);
            };
        let preview = |cx: &mut AsyncApp, indices: &[usize], total, earlier| {
            window
                .update(cx, |view, _, _| {
                    view.assert_loader_preview(indices, total, earlier)
                })
                .unwrap();
        };
        let control = |cx: &mut AsyncApp, index| {
            draw(window, cx);
            window
                .update(cx, |view, window, _| {
                    view.focus_loader_control(index, window)
                })
                .unwrap();
            draw(window, cx);
            press(window, cx, "enter");
            draw(window, cx);
        };
        let assert_submit = |cx: &mut AsyncApp, running, enabled, error| {
            window
                .update(cx, |view, _, _| {
                    view.assert_loader_submit(running, enabled, error)
                })
                .unwrap();
        };
        open(cx);
        list(cx, &[], None);
        assert_submit(cx, false, false, None);
        graph(
            &fixture,
            cx,
            "providerSessionCatalogResults",
            json!({"cwd":cwd,"sessions":[]}),
            false,
        );
        list(cx, &[], None);
        press(window, cx, "escape");
        assert_closed(cx);

        open(cx);
        graph(
            &fixture,
            cx,
            "providerSessionCatalogResults",
            json!({"cwd":cwd,"error":"probe list error"}),
            false,
        );
        assert_submit(cx, false, false, Some("probe list error"));
        press(window, cx, "escape");
        assert_closed(cx);

        open(cx);
        graph(
            &fixture,
            cx,
            "providerSessionCatalogResults",
            json!({"cwd":cwd,"sessions":[
                {"providerSessionId":"history-a","title":"Alpha planning","updatedAt":1730000000000_i64,
                 "messageCount":205,"model":"Sonnet","permissionMode":"plan",
                 "loadedBy":{"sessionId":PARENT,"title":"分支主会话"}},
                {"providerSessionId":"history-b","title":"Beta draft","updatedAt":1720000000000_i64,
                 "messageCount":0,"model":"Haiku","permissionMode":"default",
                 "loadedBy":{"sessionId":PEER,"title":"目标卡"}}
            ]}),
            false,
        );
        list(cx, &["history-a", "history-b"], Some("history-a"));
        detail("history-a", None, None);
        graph(
            &fixture,
            cx,
            "providerSessionDetailResults",
            json!({
                "cwd":cwd, "providerSessionId":"history-a",
                "events":[
                    {"index":5,"kind":"user","role":"user","text":"第一行\n第二行","matched":false},
                    {"index":6,"kind":"assistant","role":"assistant","text":"回答","matched":false},
                    {"index":7,"kind":"tool","toolName":"Read","text":"工具结果","matched":false},
                    {"index":8,"kind":"system","text":"系统消息","matched":false}
                ],
                "page":{"startEventIndex":5,"endEventIndex":9,"total":8,"hasEarlier":true,"hasLater":false}
            }),
            false,
        );
        preview(cx, &[5, 6, 7, 8], 8, true);
        control(cx, 1);
        detail("history-a", Some(1), Some(4));
        graph(
            &fixture,
            cx,
            "providerSessionDetailResults",
            json!({
                "cwd":cwd,"providerSessionId":"history-a",
                "events":(1..=4).map(|index| json!({"index":index,"kind":"user","text":"更早","matched":false})).collect::<Vec<_>>(),
                "page":{"startEventIndex":1,"endEventIndex":5,"total":8,"hasEarlier":false,"hasLater":true}
            }),
            false,
        );
        preview(cx, &[1, 2, 3, 4, 5, 6, 7, 8], 8, false);

        let filter = |cx: &mut AsyncApp, query: &str| {
            window
                .update(cx, |view, window, cx| view.loader_filter(query, window, cx))
                .unwrap();
            draw(window, cx);
        };
        filter(cx, "sOnNeT");
        list(cx, &["history-a"], Some("history-a"));
        filter(cx, "ALPHA");
        list(cx, &["history-a"], Some("history-a"));
        filter(cx, "HISTORY-A");
        list(cx, &["history-a"], Some("history-a"));
        filter(cx, &cwd.to_uppercase());
        list(cx, &["history-a", "history-b"], Some("history-a"));
        filter(cx, "");
        window
            .update(cx, |view, window, _| view.focus_loader_list(window))
            .unwrap();
        draw(window, cx);
        press(window, cx, "down");
        list(cx, &["history-a", "history-b"], Some("history-b"));
        detail("history-b", None, None);
        graph(
            &fixture,
            cx,
            "providerSessionDetailResults",
            json!({
                "cwd":cwd,"providerSessionId":"history-b","error":"probe preview error"
            }),
            false,
        );
        assert_submit(cx, false, true, Some("probe preview error"));
        filter(cx, "no such conversation");
        list(cx, &[], None);
        assert_submit(cx, false, false, None);
        filter(cx, "beta");
        list(cx, &["history-b"], Some("history-b"));
        detail("history-b", None, None);
        graph(
            &fixture,
            cx,
            "providerSessionDetailResults",
            json!({
                "cwd":cwd,"providerSessionId":"history-b","events":[],
                "page":{"startEventIndex":0,"endEventIndex":0,"total":0,"hasEarlier":false,"hasLater":false}
            }),
            true,
        );
        preview(cx, &[], 0, false);
        assert_submit(cx, true, true, None);
        let assert_load = || {
            assert!(
                matches!(commands(&sink).as_slice(), [Command::LoadProviderSession {
                session_id, provider_session_id,
            }] if session_id == PEER && provider_session_id == "history-b")
            );
        };
        control(cx, 2);
        assert_load();
        assert_submit(cx, true, false, None);
        AnyWindowHandle::from(window)
            .update(cx, |_, window, cx| {
                window.dispatch_keystroke(Keystroke::parse("enter").unwrap(), cx);
            })
            .unwrap();
        assert!(commands(&sink).is_empty());
        let busy = "Card already has a running or starting process";
        graph(
            &fixture,
            cx,
            "providerSessionLoadErrors",
            json!({
                "sessionId":PEER,"providerSessionId":"history-b","error":busy
            }),
            true,
        );
        assert_submit(cx, true, true, Some(busy));
        control(cx, 2);
        assert_load();
        press(window, cx, "escape");
        assert_closed(cx);
        open(cx);
        graph(
            &fixture,
            cx,
            "providerSessionCatalogResults",
            json!({"cwd":cwd,"sessions":[
                {"providerSessionId":"history-b","title":"Beta draft","updatedAt":0,
                 "messageCount":1,"model":"Haiku","permissionMode":"default","loadedBy":null}
            ]}),
            true,
        );
        detail("history-b", None, None);
        assert_submit(cx, true, false, None);
        assert!(commands(&sink).is_empty());
        fixture.commit_catalog_load(&cwd);
        let Event::Graph(bound_graph) = fixture.graph_event() else {
            unreachable!()
        };
        let revision = bound_graph["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["sessionId"] == PEER)
            .unwrap()["providerSpawnRevision"]
            .as_u64()
            .unwrap();
        deliver(window, cx, Event::Graph(bound_graph));
        assert_closed(cx);

        open(cx);
        graph(
            &fixture,
            cx,
            "providerSessionCatalogResults",
            json!({"cwd":cwd,"sessions":[
                {"providerSessionId":"history-b","title":"Beta draft","updatedAt":0,
                 "messageCount":1,"model":"Haiku","permissionMode":"default",
                 "loadedBy":{"sessionId":PEER,"title":"Beta draft"}}
            ]}),
            true,
        );
        detail("history-b", None, None);
        control(cx, 2);
        assert_load();
        let mut confirmed = json!({
            "sessionId":PEER,"providerSessionId":"history-b",
            "result":"confirmed","spawnRevision":revision
        });
        graph(
            &fixture,
            cx,
            "providerIdentityEvents",
            confirmed.clone(),
            true,
        );
        assert_submit(cx, true, false, None);
        confirmed["reason"] = json!("already-loaded");
        graph(&fixture, cx, "providerIdentityEvents", confirmed, true);
        assert_closed(cx);
        assert!(commands(&sink).is_empty());

        open(cx);
        let large_list: Vec<_> = (0..200)
            .map(|index| {
                json!({
                    "providerSessionId":format!("fixture-list-{index}"),
                    "title":format!("历史会话 {index}"),
                    "updatedAt":0,"permissionMode":"default","messageCount":1,"loadedBy":null
                })
            })
            .collect();
        let started = Instant::now();
        graph(
            &fixture,
            cx,
            "providerSessionCatalogResults",
            json!({"cwd":cwd,"sessions":large_list}),
            true,
        );
        println!(
            "Session loader 200-row graph + hidden GPUI draw | {:?}",
            started.elapsed()
        );
        detail("fixture-list-0", None, None);
        filter(cx, "fixture-list-199");
        list(cx, &["fixture-list-199"], Some("fixture-list-199"));
        graph(
            &fixture,
            cx,
            "providerSessionDetailResults",
            json!({
                "cwd":cwd,"providerSessionId":"fixture-list-0","error":"stale detail"
            }),
            true,
        );
        assert_submit(cx, true, true, None);
        detail("fixture-list-199", None, None);
        click(window, cx, point(px(300.), px(12.)));
        assert_closed(cx);
        open(cx);
        let close = window
            .update(cx, |_, window, _| {
                let viewport = window.viewport_size();
                let inset = (f32::from(viewport.width) * 0.03).clamp(14., 34.);
                let width = px(1040.).min(viewport.width - px(inset * 2.));
                let height = px(720.).min(viewport.height - px(inset * 2.)).max(px(430.));
                point(
                    (viewport.width + width) / 2. - px(33.),
                    (viewport.height - height) / 2. + px(30.),
                )
            })
            .unwrap();
        click(window, cx, close);
        assert_closed(cx);
        open(cx);
        window
            .update(cx, |view, window, cx| {
                view.deliver_loader_bridge_error(window, cx)
            })
            .unwrap();
        assert_submit(cx, true, false, Some("probe: runtime connection lost"));
        let Event::Graph(mut removed) = fixture.graph_event() else {
            unreachable!()
        };
        removed["nodes"]
            .as_array_mut()
            .unwrap()
            .retain(|node| node["sessionId"] != PEER);
        deliver(window, cx, Event::Graph(removed));
        assert_closed(cx);
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        println!(
            "Session loader | GPUI entry, filter, selection, preview pages, command payloads, runtime outcomes and dismissal"
        );
    }

    async fn verify_permission_mode(cx: &mut AsyncApp) {
        const LABELS: [(&str, &str, u32); 5] = [
            ("default", "Default", 0x8b949e),
            ("auto", "Auto", 0x7ee787),
            ("acceptEdits", "Accept Edits", 0xd29922),
            ("plan", "Plan Mode", 0x58a6ff),
            ("bypassPermissions", "Bypass Permissions", 0xff6b35),
        ];
        const DEFAULT: &str = "当前权限模式：Default，点击切换";
        const PLAN: &str = "当前权限模式：Plan Mode，点击切换";
        const BYPASS: &str = "当前权限模式：Bypass Permissions，点击切换";
        const RECOVERING: &str = "当前终端需要先完成恢复";
        const STORAGE: &str = "终端存储异常，请先恢复或结束当前会话";
        const READ_ONLY: &str = "数据库处于只读恢复模式";
        const ENTER_BYPASS: (&str, &str, &str) = (
            "切换到高权限模式",
            "切换到 Bypass Permissions 会打断当前 Claude 会话进程（正在执行的任务会被中断），重启后会自动 resume 恢复会话历史。\n\n是否确认？",
            "确认切换",
        );
        const EXIT_BYPASS: (&str, &str, &str) = (
            "退出高权限模式",
            "退出 Bypass Permissions 会打断当前 Claude 会话进程（正在执行的任务会被中断），重启后会自动 resume 恢复会话历史。\n\n是否确认？",
            "确认退出",
        );
        const UNBOUND: (&str, &str, &str) = (
            "切换到高权限模式",
            "当前 Claude 会话还没有生成可恢复的 sessionId。继续切换会启动一个全新的 Claude 会话，当前内容将不会保留。\n\n是否确认？",
            "确认切换",
        );
        let mut fixture = Fixture::new();
        fixture.fork_source(true, "idle");
        let (window, sink) = open_hidden(cx, 1250.);
        let assert_chip = |cx: &mut AsyncApp, expected| {
            window
                .update(cx, |view, _, _| view.assert_permission_chip(expected))
                .unwrap();
        };
        let assert_menu = |cx: &mut AsyncApp, open| {
            window
                .update(cx, |view, _, _| view.assert_permission_menu(open))
                .unwrap();
        };
        let click_chip = |cx: &mut AsyncApp| {
            draw(window, cx);
            let at = window
                .update(cx, |view, window, _| view.permission_chip_point(window))
                .unwrap();
            click(window, cx, at);
            draw(window, cx);
        };
        let select = |cx: &mut AsyncApp, index| {
            click_chip(cx);
            assert_menu(cx, true);
            let at = window
                .update(cx, |view, window, _| {
                    view.permission_row_point(index, window)
                })
                .unwrap();
            click(window, cx, at);
            draw(window, cx);
            assert_menu(cx, false);
        };
        let qualify = |cx: &mut AsyncApp| {
            draw(window, cx);
            window
                .update(cx, |view, window, cx| view.qualify(PARENT, window, cx))
                .unwrap();
        };
        let assert_dialog = |cx: &mut AsyncApp, copy| {
            window
                .update(cx, |view, _, _| view.assert_permission_confirmation(copy))
                .unwrap();
        };
        let assert_closed = |cx: &mut AsyncApp| {
            window
                .update(cx, |view, _, _| view.assert_no_dialog())
                .unwrap();
        };
        let dialog_action = |cx: &mut AsyncApp, index| {
            window
                .update(cx, |view, window, _| {
                    view.focus_fork_dialog_control(index, window)
                })
                .unwrap();
            press(window, cx, "enter");
            draw(window, cx);
            assert_closed(cx);
        };
        let assert_toast = |cx: &mut AsyncApp, expected| {
            window
                .update(cx, |view, _, _| view.assert_permission_toast(expected))
                .unwrap();
        };
        let assert_command = |actions: Vec<Command>, mode: &str| {
            assert!(
                matches!(
                    actions.as_slice(),
                    [Command::SetProviderPermissionMode { session_id, mode: sent }]
                        if session_id == PARENT && sent == mode
                ),
                "unexpected permission command: {}",
                serde_json::to_string(&actions).unwrap(),
            );
        };

        deliver(window, cx, fixture.event(true));
        draw(window, cx);
        assert_chip(cx, None);
        click_chip(cx);
        assert_menu(cx, false);
        fixture.focus(SCENE, PARENT);
        deliver(window, cx, fixture.event(false));
        assert_chip(cx, Some(("Default", RECOVERING, true)));
        qualify(cx);
        for (index, (mode, label, color)) in LABELS.iter().copied().enumerate() {
            fixture.permission_mode(Some(mode));
            deliver(window, cx, fixture.graph_event());
            let title = format!("当前权限模式：{label}，点击切换");
            window
                .update(cx, |view, _, cx| {
                    view.assert_permission_chip(Some((label, &title, false)));
                    view.assert_permission_colors(index, color, cx);
                })
                .unwrap();
            select(cx, index);
            assert!(fixture.process(window, cx, &sink).is_empty());
        }
        fixture.permission_mode(Some("default"));
        deliver(window, cx, fixture.graph_event());
        assert_chip(cx, Some(("Default", DEFAULT, false)));
        click_chip(cx);
        assert_menu(cx, true);
        click_chip(cx);
        assert_menu(cx, false);
        click_chip(cx);
        click(window, cx, point(px(1050.), px(450.)));
        draw(window, cx);
        assert_menu(cx, false);
        click_chip(cx);
        press(window, cx, "escape");
        assert_menu(cx, false);
        assert!(fixture.process(window, cx, &sink).is_empty());

        select(cx, 3);
        assert_closed(cx);
        assert_command(fixture.process(window, cx, &sink), "plan");
        assert_chip(cx, Some(("Default", DEFAULT, true)));
        click_chip(cx);
        assert_menu(cx, false);
        assert!(fixture.process(window, cx, &sink).is_empty());
        deliver(window, cx, fixture.permission_result("plan", None));
        assert_chip(cx, Some(("Default", DEFAULT, true)));
        fixture.permission_mode(Some("plan"));
        deliver(window, cx, fixture.graph_event());
        assert_chip(cx, Some(("Plan Mode", PLAN, false)));

        select(cx, 1);
        assert_closed(cx);
        assert_command(fixture.process(window, cx, &sink), "auto");
        deliver(
            window,
            cx,
            fixture.permission_result("acceptEdits", Some("stale result")),
        );
        assert_chip(cx, Some(("Plan Mode", PLAN, true)));
        deliver(
            window,
            cx,
            fixture.permission_result("auto", Some("probe failure")),
        );
        assert_toast(cx, Some("切换失败：probe failure"));
        assert_chip(cx, Some(("Plan Mode", PLAN, false)));

        select(cx, 2);
        assert_closed(cx);
        assert_command(fixture.process(window, cx, &sink), "acceptEdits");
        fixture.permission_mode(Some("acceptEdits"));
        deliver(window, cx, fixture.permission_result("acceptEdits", None));
        assert_chip(
            cx,
            Some((
                "Accept Edits",
                "当前权限模式：Accept Edits，点击切换",
                false,
            )),
        );
        fixture.permission_mode(Some("plan"));
        deliver(window, cx, fixture.graph_event());

        select(cx, 4);
        assert_dialog(cx, ENTER_BYPASS);
        assert!(fixture.process(window, cx, &sink).is_empty());
        dialog_action(cx, 1);
        assert!(fixture.process(window, cx, &sink).is_empty());
        select(cx, 4);
        dialog_action(cx, 2);
        assert_command(fixture.process(window, cx, &sink), "bypassPermissions");
        deliver(
            window,
            cx,
            Event::Recovery {
                session_id: PARENT.into(),
                state: RecoveryState::Restoring,
            },
        );
        fixture.permission_mode(Some("bypassPermissions"));
        deliver(window, cx, fixture.graph_event());
        qualify(cx); // CLI consent can render and accept ordinary terminal input in Ready.
        assert_closed(cx);
        assert_chip(cx, Some(("Plan Mode", PLAN, true)));
        window
            .update(cx, |view, window, cx| {
                view.assert_terminal_focus(PARENT, true, window, cx)
            })
            .unwrap();
        click_chip(cx);
        assert_menu(cx, false);
        assert!(fixture.process(window, cx, &sink).is_empty());
        deliver(
            window,
            cx,
            fixture.permission_result("bypassPermissions", None),
        );
        assert_chip(cx, Some(("Bypass Permissions", BYPASS, false)));

        select(cx, 0);
        assert_dialog(cx, EXIT_BYPASS);
        press(window, cx, "escape");
        assert_closed(cx);
        assert!(fixture.process(window, cx, &sink).is_empty());
        select(cx, 0);
        dialog_action(cx, 2);
        assert_command(fixture.process(window, cx, &sink), "default");
        deliver(
            window,
            cx,
            fixture.permission_result("default", Some("probe runtime refusal")),
        );
        assert_chip(cx, Some(("Bypass Permissions", BYPASS, false)));
        assert_toast(cx, Some("切换失败：probe runtime refusal"));

        fixture.permission_mode(None);
        deliver(window, cx, fixture.graph_event());
        select(cx, 4);
        assert_dialog(cx, UNBOUND);
        dialog_action(cx, 2);
        assert_command(fixture.process(window, cx, &sink), "bypassPermissions");
        deliver(
            window,
            cx,
            fixture.permission_result(
                "bypassPermissions",
                Some("active AI Session is required for permission respawn"),
            ),
        );
        assert_toast(
            cx,
            Some("切换失败：active AI Session is required for permission respawn"),
        );
        assert_chip(cx, Some(("Default", DEFAULT, false)));

        fixture.fork_source(false, "idle");
        let Event::Graph(mut graph) = fixture.graph_event() else {
            unreachable!()
        };
        graph["providerIdentityEvents"] = serde_json::json!([{
            "sessionId": PARENT,
            "providerSessionId": "probe-provider",
            "result": "confirmed",
        }]);
        deliver(window, cx, Event::Graph(graph));
        deliver(window, cx, fixture.graph_event());
        select(cx, 4);
        assert_dialog(cx, ENTER_BYPASS);
        dialog_action(cx, 1);
        assert!(fixture.process(window, cx, &sink).is_empty());

        deliver(
            window,
            cx,
            Event::StorageFault {
                session_id: PARENT.into(),
                message: "probe storage fault".into(),
            },
        );
        assert_chip(cx, Some(("Default", STORAGE, true)));
        click_chip(cx);
        assert_menu(cx, false);
        deliver(
            window,
            cx,
            Event::StorageRecovered {
                session_id: PARENT.into(),
            },
        );
        sink.read_only();
        assert_chip(cx, Some(("Default", READ_ONLY, true)));
        click_chip(cx);
        assert_menu(cx, false);
        assert!(fixture.process(window, cx, &sink).is_empty());
        sink.generation(1);

        cx.background_executor()
            .timer(Duration::from_millis(2500))
            .await;
        assert_toast(
            cx,
            Some("切换失败：active AI Session is required for permission respawn"),
        );
        cx.background_executor()
            .timer(Duration::from_millis(700))
            .await;
        assert_toast(cx, None);
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        println!(
            "Permission mode | GPUI/SQLite chip, menu, confirmation, command, pending, block and toast assertions passed"
        );
    }

    pub(super) fn run() {
        let loader_only = std::env::args().any(|arg| arg == "--session-loader");
        let fork_only = std::env::args().any(|arg| arg == "--fork");
        let permission_only = std::env::args().any(|arg| arg == "--permission");
        Application::new().with_assets(NativeAssets).run(move |cx| {
            if permission_only {
                cx.spawn(async |cx| {
                    verify_permission_mode(cx).await;
                    cx.update(|cx| cx.quit()).unwrap();
                })
                .detach();
                return;
            }
            if fork_only {
                cx.spawn(async |cx| {
                    verify_fork(cx).await;
                    cx.update(|cx| cx.quit()).unwrap();
                })
                .detach();
                return;
            }
            if loader_only {
                cx.spawn(async |cx| {
                    verify_session_loader(cx);
                    cx.update(|cx| cx.quit()).unwrap();
                })
                .detach();
                return;
            }
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
                verify_session_loader(cx);
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
