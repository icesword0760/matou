//! Hidden GPUI layout and input probe backed by a disposable session graph.

#![allow(unexpected_cfgs)]

#[cfg(target_os = "macos")]
pub use matou_native_desktop::{
    assets, recovery_water_view, rename_input, tab_dialog_style, workspace_drag,
};

#[cfg(target_os = "macos")]
mod macos {
    pub use matou_native_desktop::macos::*;

    std::thread_local! {
        pub(super) static MOUNT_HIDDEN_WATER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    pub fn is_window_visible(window: &gpui::Window) -> anyhow::Result<bool> {
        if MOUNT_HIDDEN_WATER.get() {
            return Ok(true);
        }
        matou_native_desktop::macos::is_window_visible(window)
    }
}

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
#[path = "../src/provider_settings.rs"]
mod provider_settings;
#[cfg(target_os = "macos")]
#[path = "../src/settings_shell.rs"]
mod settings_shell;

#[cfg(target_os = "macos")]
#[allow(dead_code)]
mod workspace {
    include!("../src/workspace.rs");

    use super::probe::{PARENT, PEER, SCENE};
    use crate::ui_text::TextGeometryHandle;
    use matou_native_terminal::{Terminal, TerminalOptions};
    use std::cell::RefCell;

    #[derive(Default)]
    pub(super) struct HeaderReadings {
        pub title: TextGeometryHandle,
        pub actions: RefCell<Vec<Bounds<Pixels>>>,
        pub loader_hovered: Cell<bool>,
        pub remove_hovered: Cell<bool>,
    }

    pub(super) struct HeaderProbe {
        pub workspace: Entity<Workspace>,
        pub width: Pixels,
        pub title: Option<&'static str>,
        pub readings: Rc<HeaderReadings>,
    }

    impl Render for HeaderProbe {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let (title, actions) = self.workspace.update(cx, |view, cx| {
                let node = view.graph_node(PEER).unwrap();
                let title = self
                    .title
                    .or_else(|| node["title"].as_str())
                    .unwrap_or("Shell");
                let loader = self.readings.clone();
                let remove = self.readings.clone();
                let readings = self.readings.clone();
                let actions = Workspace::session_card_actions()
                    .on_children_prepainted(move |bounds, _, _| {
                        *readings.actions.borrow_mut() = bounds;
                    })
                    .child(
                        view.session_loader_button(PEER, node, cx)
                            .on_hover(move |hovered, _, _| loader.loader_hovered.set(*hovered)),
                    )
                    .children(view.render_fork_header(PEER, ForkPlacement::Child, cx))
                    .children(view.render_fork_header(PEER, ForkPlacement::Sibling, cx))
                    .child(
                        view.remove_card_button(PEER, node, cx)
                            .on_hover(move |hovered, _, _| remove.remove_hovered.set(*hovered)),
                    );
                (title.to_owned(), actions)
            });
            Workspace::session_card_header(
                ui_text(title).track_geometry(self.readings.title.clone()),
                actions,
            )
            .font_family(".AppleSystemUIFont")
            .w(self.width)
        }
    }

    impl Workspace {
        pub(super) fn title_probe_projection(&self) -> (String, Bounds<Pixels>, String) {
            let scene = self.hierarchy.as_ref().unwrap().scene.as_ref().unwrap();
            let tab_index = self
                .scenes
                .iter()
                .position(|item| item.id == scene.id)
                .unwrap();
            let label = self.scene_title(&self.scenes[tab_index]);
            let mut element = tabs_ui::tab_title(label.clone());
            let text = element.style().text.as_ref().unwrap();
            assert_eq!(text.line_clamp, Some(1));
            assert!(
                matches!(&text.text_overflow, Some(TextOverflow::Truncate(mark)) if mark == "…")
            );
            (
                label,
                self.tab_scroll.bounds_for_item(tab_index).unwrap(),
                self.graph_node(PEER).unwrap()["title"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            )
        }

        pub(super) fn focus_title_menu_item(&self, restore: bool, window: &mut Window) {
            let (_, _, rename, restore_auto) = self.fork_menu.as_ref().expect("session title menu");
            window.focus(if restore { restore_auto } else { rename });
        }

        pub(super) fn replace_title_input(&self, window: &mut Window, cx: &mut Context<Self>) {
            let Some(TabDialog::RenameSession { input, .. }) = &self.dialog else {
                panic!("expected the session rename dialog");
            };
            input.update(cx, |input, cx| {
                let len = input.text.encode_utf16().count();
                input.replace_text_in_range(Some(0..len), "手工标题", window, cx);
            });
        }

        pub(super) fn assert_title_dialog_state(&self, failed: bool) {
            if failed {
                assert!(
                    matches!(&self.dialog, Some(TabDialog::RenameSession { failure: Some(title), .. }) if title == "手工标题")
                );
            } else {
                assert!(self.dialog.is_none());
            }
        }

        pub(super) fn card_header_probe_width(&self, id: &str) -> Pixels {
            let index = self.order.iter().position(|card| card == id).unwrap();
            self.carousel_scroll
                .bounds_for_item(index)
                .unwrap()
                .size
                .width
        }

        pub(super) fn assert_compact_status_color() {
            let summary_color = Self::compact_session_summary(None, "Shell")
                .style()
                .text
                .as_ref()
                .unwrap()
                .color;
            assert_eq!(summary_color, Some(rgb(0x6b7482).into()));
            for status in [
                "needs-input",
                "starting",
                "running",
                "error",
                "interrupted",
                "exited",
                "idle",
            ] {
                let color = Self::compact_session_status(status)
                    .style()
                    .text
                    .as_ref()
                    .unwrap()
                    .color;
                assert_eq!(
                    color,
                    (status == "needs-input").then(|| rgb(0xe8ae48).into())
                );
            }
            println!("Compact summary | needs-input #e8ae48 | other states inherit #6b7482");
        }

        pub(super) fn assert_card_header_dialog(&self, remove: bool) {
            if remove {
                assert!(
                    matches!(&self.dialog, Some(TabDialog::RemoveSession { session_id, .. }) if session_id == PEER)
                );
            } else {
                assert!(
                    matches!(&self.dialog, Some(TabDialog::SessionLoader(state)) if state.session_id == PEER)
                );
            }
        }

        pub(super) fn configure_footer_shell(&mut self, shell: &str) {
            self.footer_shell = footer_shell_name(shell).map(str::to_owned);
        }

        pub(super) fn assert_footer_fields(
            &self,
            expected_name: Option<&str>,
            expected_cwd: Option<(&str, &str)>,
            expected_git: Option<(&str, bool)>,
            expected_environment: Option<&str>,
            account: bool,
        ) {
            let fields = self.footer_fields().expect("focused footer fields");
            assert_eq!(fields.name.as_deref(), expected_name);
            assert_eq!(
                fields
                    .cwd
                    .as_ref()
                    .map(|(full, short)| (*full, short.as_str())),
                expected_cwd,
            );
            assert_eq!(
                fields
                    .git
                    .as_ref()
                    .map(|git| (git.label.as_str(), git.available)),
                expected_git,
            );
            assert_eq!(
                fields
                    .environment
                    .as_ref()
                    .map(|environment| environment.label),
                expected_environment,
            );
            assert_eq!(fields.account, account);
        }

        pub(super) fn assert_footer_empty(&self) {
            assert!(self.footer_fields().is_none());
            assert_eq!(
                self.breadcrumb_labels(),
                ("根会话".into(), "· 0 个会话".into())
            );
        }

        pub(super) fn footer_card_point(&self, id: &str, window: &Window) -> Point<Pixels> {
            let index = self.order.iter().position(|entry| entry == id).unwrap();
            let bounds = self.carousel_scroll.bounds_for_item(index).unwrap();
            let left = (bounds.left() + self.carousel_scroll.offset().x).max(px(260.));
            let right = (bounds.right() + self.carousel_scroll.offset().x)
                .min(window.viewport_size().width);
            assert!(right - left > px(20.));
            point((left + right) / 2., bounds.top() + px(31.))
        }

        pub(super) fn open_footer_child_level(
            &mut self,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            self.open_child_level(PARENT, window, cx);
        }

        pub(super) fn select_footer_parent(&mut self, window: &Window, cx: &mut Context<Self>) {
            self.activate_session_card(PARENT, window, cx);
        }

        pub(super) fn clear_footer_readings() {
            FOOTER_READINGS.with_borrow_mut(|readings| *readings = Some(FooterReadings::default()));
        }

        pub(super) fn footer_bounds(key: &'static str) -> Option<Bounds<Pixels>> {
            FOOTER_READINGS
                .with_borrow(|readings| readings.as_ref()?.bounds.borrow().get(key).copied())
        }

        pub(super) fn instructions_focus(&self, control: &str, window: &Window) -> bool {
            self.control_focus[control].is_focused(window)
        }

        pub(super) fn instructions_ready(&self) -> bool {
            self.project_instructions_request.is_none()
        }

        pub(super) fn assert_instructions_preview(
            &self,
            path: &std::path::Path,
            expected: Result<Option<&str>, &str>,
        ) -> ScrollHandle {
            let (_, target) = self.project_instructions_target.as_ref().unwrap();
            assert_eq!(target, path);
            let preview = self.project_instructions_preview.as_ref().unwrap();
            match (preview.result.as_ref(), expected) {
                (Some(ProjectInstructionsRead::Missing), Ok(None)) => {}
                (Some(ProjectInstructionsRead::Content(text)), Ok(Some(expected))) => {
                    assert_eq!(text.as_ref(), expected)
                }
                (Some(ProjectInstructionsRead::Failed(error)), Err(expected)) => {
                    assert!(error.starts_with(expected), "{error}")
                }
                _ => panic!("unexpected instruction preview state"),
            }
            preview.scroll.clone()
        }

        pub(super) fn assert_painted_footer_text(
            key: &'static str,
            expected: &str,
            ellipsis: bool,
        ) -> String {
            FOOTER_READINGS.with_borrow(|readings| {
                let readings = readings.as_ref().unwrap();
                let labels = readings.text.borrow();
                let (source, text) = labels
                    .get(key)
                    .unwrap_or_else(|| panic!("missing footer label {key}"));
                assert_eq!(source.as_ref(), expected);
                let text = text.borrow();
                let geometry = text.as_ref().unwrap_or_else(|| {
                    panic!(
                        "no painted footer text for {key} in {:?}",
                        Self::footer_bounds(key)
                    )
                });
                assert!(geometry.layout.width <= geometry.bounds.size.width + px(0.5));
                assert!(geometry.bounds.size.height <= px(18.));
                if ellipsis {
                    let length = geometry.layout.len.checked_sub("…".len()).unwrap();
                    let prefix = expected
                        .get(..length)
                        .expect("ellipsis followed a text character");
                    assert!(!prefix.is_empty() && geometry.layout.len < expected.len());
                    format!("{prefix}…")
                } else {
                    assert_eq!(geometry.layout.len, expected.len());
                    expected.into()
                }
            })
        }

        pub(super) fn assert_fitted_footer_text(key: &'static str, expected: &str) -> String {
            let ellipsis = FOOTER_READINGS.with_borrow(|readings| {
                let labels = readings.as_ref().unwrap().text.borrow();
                let (_, geometry) = labels.get(key).expect("painted HUD field");
                geometry.borrow().as_ref().unwrap().layout.len < expected.len()
            });
            Self::assert_painted_footer_text(key, expected, ellipsis)
        }

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
            let (title, body, confirm) = state.copy();
            assert_eq!((title, body.as_str(), confirm), expected);
        }

        pub(super) fn toggle_codex_permission_menu(
            &mut self,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            assert!(!matou_native_desktop::macos::is_window_visible(window).unwrap());
            self.toggle_permission_menu(window, cx);
        }

        pub(super) fn assert_codex_permission_rows(&self) {
            let chip = self.permission_chip().unwrap();
            let rows: Vec<_> = chip.modes.iter().map(|mode| mode.label()).collect();
            assert_eq!(
                rows,
                ["Default", "Accept Edits", "Plan Mode", "Bypass Permissions"]
            );
            assert_eq!(PERMISSION_MENU_TITLE, "权限模式");
        }

        pub(super) fn choose_codex_permission_mode(
            &mut self,
            mode: &str,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            let mode = PermissionMode::ALL
                .into_iter()
                .find(|entry| entry.value() == mode)
                .unwrap();
            self.choose_permission_mode(mode, window, cx);
        }

        pub(super) fn complete_codex_permission_confirmation(
            &mut self,
            confirm: bool,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            if confirm {
                self.confirm_dialog(window, cx);
            } else {
                self.close_dialog(window, cx);
            }
        }

        pub(super) fn assert_permission_toast(&self, expected: Option<&str>) {
            assert_eq!(self.permission_toast.as_deref(), expected);
        }

        pub(super) fn permission_chip_point(&self, window: &Window) -> Point<Pixels> {
            let bounds = Self::footer_bounds("permission").expect("permission chip was prepainted");
            assert!(bounds.right() < window.viewport_size().width);
            bounds.center()
        }

        pub(super) fn permission_row_point(&self, index: usize, window: &Window) -> Point<Pixels> {
            let menu =
                Self::footer_bounds("permission-menu").expect("permission menu was prepainted");
            assert!(menu.right() <= window.viewport_size().width + px(0.5));
            let mode = self.permission_chip().unwrap().modes[index];
            let row = Self::footer_bounds(mode.value()).expect("permission row was prepainted");
            assert!((row.size.width - menu.size.width + px(14.)).abs() <= px(0.5));
            assert!(menu.contains(&row.center()));
            row.center()
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
                ["D", "A", "AE", "PM", "BP"][index]
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

        pub(super) fn verify_deferred_card(
            &self,
            title: &str,
            cwd: &str,
            time: Option<&str>,
            replies: &[&str],
            loading: bool,
        ) {
            let card = self
                .deferred_card(crate::probe::PEER)
                .expect("deferred recovery card");
            assert_eq!(card.title, title);
            assert_eq!(card.agent, Some("Claude Code"));
            assert_eq!(card.cwd.as_deref(), Some(cwd));
            assert_eq!(card.updated_label.as_deref(), time);
            assert_eq!(
                card.replies
                    .iter()
                    .map(|reply| reply.text.as_str())
                    .collect::<Vec<_>>(),
                replies
            );
            assert_eq!(card.loading, loading);
            assert!(card.can_open_history);
            assert!(card.can_restore);
            assert!(!self.session_frames.contains(crate::probe::PEER));
        }

        pub(super) fn focus_deferred_action(&self, index: usize, window: &mut Window) {
            window.focus(&self.deferred_controls[crate::probe::PEER][index]);
        }

        pub(super) fn assert_deferred_history(&self, open: bool) {
            if open {
                let state = self.loader();
                assert!(state.history);
                assert_eq!(state.selected.as_deref(), Some("probe-conversation"));
                assert!(!self.session_loader_can_submit(state));
                assert!(self.is_deferred(crate::probe::PEER));
            } else {
                assert!(self.dialog.is_none());
            }
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

        pub(super) fn open_codex_loader_direct(
            &mut self,
            id: &str,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            self.open_session_loader(id, window, cx);
        }

        pub(super) fn verify_codex_loader_direct(
            &mut self,
            selected: &str,
            expect_focus: bool,
            enabled: bool,
            window: &Window,
        ) {
            let state = self.loader();
            assert_eq!(state.provider, "codex");
            assert_eq!(state.selected.as_deref(), Some(selected));
            assert_eq!(state.list_focus.is_focused(window), expect_focus);
            assert_eq!(self.session_loader_can_submit(state), enabled);
        }

        pub(super) fn codex_loader_select_direct(
            &mut self,
            id: &str,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            self.select_loader_session(id, cx);
            window.focus(&self.loader().list_focus);
        }

        pub(super) fn codex_loader_close_direct(
            &mut self,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            self.close_dialog(window, cx);
        }

        pub(super) fn codex_loader_source_direct(
            &mut self,
            provider: &'static str,
            cx: &mut Context<Self>,
        ) {
            self.switch_loader_provider(provider, cx);
        }

        pub(super) fn codex_loader_submit_direct(&mut self, cx: &mut Context<Self>) {
            self.submit_session_loader(cx);
            assert!(self.loader().confirm_running);
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
                        .map(|event| loader_event_label(
                            &event.kind,
                            event.tool_name.as_deref(),
                            "Claude"
                        ))
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

        pub(super) fn open_codex_history_direct(
            &mut self,
            id: &str,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            assert!(!matou_native_desktop::macos::is_window_visible(window).unwrap());
            let card = self.deferred_card(id).unwrap();
            assert_eq!(card.agent, Some("Codex"));
            assert!(card.can_open_history);
            self.open_deferred_history(id, window, cx);
            assert!(self.loader().history);
            assert_eq!(self.loader().provider, "codex");
            assert!(!self.session_loader_can_submit(self.loader()));
        }

        pub(super) fn codex_history_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
            self.loader().content_input.update(cx, |input, cx| {
                input.replace_text_in_range(Some(0..0), "祖先", window, cx);
            });
        }

        pub(super) fn codex_history_earlier(&mut self, cx: &mut Context<Self>) {
            self.load_earlier_session_events(cx);
            assert!(self.loader().loading_earlier);
        }

        pub(super) fn codex_history_open_hit(&mut self, cx: &mut Context<Self>) {
            let state = self.loader();
            assert_eq!(state.search_hits[0].excerpt, "祖先线索");
            self.select_loader_hit(state.search_hits[0].index, cx);
        }

        pub(super) fn codex_history_more_hits(&mut self, cx: &mut Context<Self>) {
            assert!(self.loader().has_more_hits);
            self.load_more_loader_hits(cx);
        }

        pub(super) fn assert_codex_history_jump(&self) {
            let state = self.loader();
            assert_eq!(state.events[1].text, "祖先线索");
            let scroll = &state.preview_scroll;
            let hit = scroll.bounds_for_item(1).unwrap();
            assert_eq!(hit.top() + scroll.offset().y, scroll.bounds().top());
        }

        pub(super) fn close_codex_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
            self.close_dialog(window, cx);
            assert!(self.dialog.is_none());
        }

        pub(super) fn assert_codex_history_empty(&self) {
            let state = self.loader();
            assert!(!state.loading_detail);
            assert!(state.events.is_empty() && state.error.is_none());
            assert_eq!(state.preview_scroll.children_count(), 1);
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

        pub(super) fn recovery_pointer_bounds(&self, id: &str) -> Bounds<Pixels> {
            let index = self.order.iter().position(|session| session == id).unwrap();
            let mut bounds = self.carousel_scroll.bounds_for_item(index).unwrap();
            bounds.origin += self.carousel_scroll.offset();
            bounds
        }

        pub(super) fn recovery_pointer_geometry(&self) -> Option<Value> {
            self.geometry
                .get(self.carousel_owner.as_ref().unwrap())
                .map(|update| update.geometry.clone())
        }

        pub(super) fn assert_recovery_pointer_state(
            &self,
            active: &str,
            authoritative: &str,
            covered: bool,
        ) {
            assert_eq!(self.level_focus(), Some(active));
            assert_eq!(self.focused.as_deref(), Some(authoritative));
            assert_eq!(self.recovery_water_covers_terminal(PARENT), covered);
        }

        pub(super) fn begin_pointer_reconnect(
            &mut self,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            self.apply(
                BridgeEvent::Connection {
                    generation: self.runtime_generation,
                    phase: ConnectionPhase::Reconnecting,
                },
                window,
                cx,
            );
            assert!(!self.domain_ready);
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
            assert!(matches!(
                self.card_motion[id].width.as_ref().unwrap().target_rule(),
                crate::carousel_motion::WidthRule::WideExpanded
                    | crate::carousel_motion::WidthRule::NarrowExpanded
            ));
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

        pub(super) fn provider_restore_pending(&self, session_id: &str) -> bool {
            self.pending_provider_restores.contains_key(session_id)
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
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use cocoa::appkit::NSEventType;

    use gpui::{
        AnyWindowHandle, AppContext, Application, AsyncApp, Bounds, Keystroke, MouseButton, Pixels,
        Point, Render, WindowBounds, WindowHandle, WindowOptions, point, px, size,
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
        workspace::{HeaderProbe, HeaderReadings, Workspace},
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

        fn codex_permission_state(&self, work: &str, restore: &str) {
            let Some(BootstrapResult::Writable { connection, .. }) = &self.db else {
                unreachable!()
            };
            connection
                .execute(
                    "UPDATE sessions SET kind = 'codex', work_status = ? WHERE id = ?",
                    (work, PARENT),
                )
                .unwrap();
            connection.execute(
                "UPDATE provider_bindings SET provider = 'codex', restore_state = ? WHERE session_id = ?",
                (restore, PARENT),
            ).unwrap();
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
                Workspace::clear_footer_readings();
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

    fn mouse<V: Render>(
        window: WindowHandle<V>,
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
            event::{CGEventType, CGMouseButton, CGScrollEventUnit, ScrollEventUnit},
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
            fn CGEventCreateScrollWheelEvent2(
                source: *const c_void,
                units: CGScrollEventUnit,
                wheel_count: u32,
                wheel1: i32,
                wheel2: i32,
                wheel3: i32,
            ) -> *mut c_void;
            fn CGEventSetLocation(event: *mut c_void, location: CGPoint);
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
            if kind == NSEventType::NSScrollWheel {
                let event = CGEventCreateScrollWheelEvent2(
                    std::ptr::null(),
                    ScrollEventUnit::PIXEL,
                    2,
                    -130,
                    -100,
                    0,
                );
                assert!(!event.is_null());
                CGEventSetLocation(
                    event,
                    CGPoint::new(
                        location.x,
                        CGDisplay::main().bounds().size.height - location.y,
                    ),
                );
                let native = NSEvent::eventWithCGEvent_(nil, event);
                CFRelease(event);
                assert_eq!(native.locationInWindow().x, location.x);
                assert_eq!(native.locationInWindow().y, location.y);
                let _: () = msg_send![view, scrollWheel: native];
                return;
            }
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
        open_probe_window(cx, width, false)
    }

    fn open_probe_window(
        cx: &mut AsyncApp,
        width: f32,
        visible: bool,
    ) -> (WindowHandle<Workspace>, ProbeReceiver) {
        let (commands, sink) = runtime_bridge::probe_channel();
        let (_events, incoming) = async_channel::unbounded();
        let window = cx
            .open_window(
                WindowOptions {
                    show: visible,
                    focus: visible,
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(width), px(780.)),
                    ))),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| Workspace::new(commands, incoming, window, cx)),
            )
            .unwrap();
        if visible {
            window
                .update(cx, |_, window, _| {
                    window.set_window_title("Matou · 延后恢复验收")
                })
                .unwrap();
        }
        (window, sink)
    }

    fn verify_recovery_water_pointer(cx: &mut AsyncApp) {
        for (covered, y, reconnect) in [
            (false, 96., false),
            (true, 36., false),
            (true, 96., false),
            (true, 96., true),
        ] {
            crate::macos::MOUNT_HIDDEN_WATER.set(covered);
            let mut fixture = Fixture::new();
            let (window, sink) = open_hidden(cx, 1200.);
            let drain = || std::iter::from_fn(|| sink.try_recv().ok()).collect::<Vec<_>>();
            let focus_commands = |commands: Vec<Command>| {
                commands
                    .into_iter()
                    .filter(|command| matches!(command, Command::FocusSession { .. }))
                    .collect::<Vec<_>>()
            };

            deliver(window, cx, fixture.event(true));
            draw(window, cx);
            window
                .update(cx, |view, window, cx| {
                    view.qualify(PARENT, window, cx);
                    view.settle_stopped_probe_geometry(cx);
                    view.assert_recovery_pointer_state(PEER, PEER, covered);
                    if reconnect {
                        view.begin_pointer_reconnect(window, cx);
                    }
                })
                .unwrap();
            if reconnect {
                sink.generation(2);
            }
            drain();
            let (bounds, geometry_before) = window
                .update(cx, |view, _, _| {
                    (
                        view.recovery_pointer_bounds(PARENT),
                        view.recovery_pointer_geometry(),
                    )
                })
                .unwrap();
            assert_ne!(
                geometry_before
                    .as_ref()
                    .and_then(|value| value["anchorSessionId"].as_str()),
                Some(PARENT)
            );
            let center_x = bounds.center().x;
            for (label, position) in [
                ("right-exact", point(bounds.right(), bounds.top() + px(96.))),
                ("bottom-exact", point(center_x, bounds.bottom())),
                (
                    "top-left-corner",
                    point(bounds.left() + px(1.), bounds.top() + px(1.)),
                ),
                (
                    "top-right-corner",
                    point(bounds.right() - px(1.), bounds.top() + px(1.)),
                ),
                (
                    "bottom-corner",
                    point(bounds.left() + px(1.), bounds.bottom() - px(1.)),
                ),
            ] {
                click(window, cx, position);
                window
                    .update(cx, |view, _, _| {
                        view.assert_recovery_pointer_state(PEER, PEER, covered);
                        assert_eq!(view.recovery_pointer_geometry(), geometry_before, "{label}");
                    })
                    .unwrap();
                assert!(
                    drain().iter().all(|command| !matches!(
                        command,
                        Command::FocusSession { .. } | Command::PutGeometry { .. }
                    )),
                    "{label}"
                );
            }

            click(window, cx, point(center_x, bounds.top() + px(y)));
            let check_optimistic = |cx: &mut AsyncApp| {
                window
                    .update(cx, |view, _, _| {
                        view.assert_recovery_pointer_state(PARENT, PEER, covered);
                        let geometry = view.recovery_pointer_geometry().unwrap();
                        assert_eq!(geometry["focusedSessionId"], PARENT);
                        assert_eq!(geometry["anchorSessionId"], PARENT);
                        assert!(geometry["anchorViewportOffset"].is_number());
                    })
                    .unwrap();
            };
            check_optimistic(cx);
            let commands = focus_commands(drain());
            if reconnect {
                assert!(commands.is_empty());
                deliver(window, cx, fixture.event(true));
                check_optimistic(cx);
                let ready_commands = drain();
                assert!(ready_commands.iter().any(|command| matches!(command,
                    Command::PutGeometry { update, .. } if update.geometry["anchorSessionId"] == PARENT
                )));
                assert_focus(focus_commands(ready_commands), PARENT);
                deliver(window, cx, fixture.event(true));
                assert!(
                    focus_commands(drain()).is_empty(),
                    "the replacement ready graph must not replay focus twice"
                );
            } else {
                assert_focus(commands, PARENT);
                draw(window, cx);
                window
                    .update(cx, |view, window, cx| {
                        view.assert_visual_focus(PARENT);
                        view.assert_terminal_focus(PARENT, true, window, cx);
                    })
                    .unwrap();
                AnyWindowHandle::from(window)
                    .update(cx, |_, window, cx| {
                        window.dispatch_keystroke(Keystroke::parse("a").unwrap(), cx);
                    })
                    .unwrap();
                let commands = drain();
                let input = commands
                    .iter()
                    .filter(|command| matches!(command, Command::Input { .. }))
                    .collect::<Vec<_>>();
                if covered {
                    assert!(input.is_empty(), "input reached the runtime under water");
                } else {
                    assert!(
                        matches!(input.as_slice(), [Command::Input { session_id, bytes, .. }]
                        if session_id == PARENT && bytes == b"a"),
                        "the uncovered terminal should emit one input command"
                    );
                }
            }
            fixture.focus(SCENE, PARENT);
            deliver(window, cx, fixture.event(false));
            window
                .update(cx, |view, _, _| {
                    view.assert_recovery_pointer_state(PARENT, PARENT, covered);
                })
                .unwrap();
            assert!(focus_commands(drain()).is_empty());
            window
                .update(cx, |_, window, _| window.remove_window())
                .unwrap();
            println!(
                "Card pointer | covered={covered} y={y} reconnect={reconnect} | activation, geometry, boundaries and transport passed"
            );
        }
        crate::macos::MOUNT_HIDDEN_WATER.set(false);
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
        let deadline = Instant::now() + Duration::from_secs(12);
        while window
            .update(cx, |view, _, _| view.provider_restore_pending(PEER))
            .unwrap()
            && Instant::now() < deadline
        {
            cx.background_executor()
                .timer(Duration::from_millis(100))
                .await;
        }
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

    fn verify_codex_title_layout(cx: &mut AsyncApp) {
        const LONG: &str =
            "这是一次较晚到达的 Codex 自动标题，用于检查标题更新后卡头是否在原有宽度内省略";
        let fixture = Fixture::new();
        let Event::Ready { mut graph, .. } = fixture.event(true) else {
            unreachable!()
        };
        let update =
            |graph: &mut serde_json::Value, title: &str, source: &str, provider: Option<&str>| {
                let node = graph["nodes"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|node| node["sessionId"] == PEER)
                    .unwrap();
                node["currentMode"] = "codex".into();
                node["title"] = title.into();
                node["titleSource"] = source.into();
                node["providerTitle"] = serde_json::json!(provider);
            };
        update(&mut graph, "Codex", "default", None);
        for width in [720., 900., 1200.] {
            let (window, sink) = open_hidden(cx, width);
            let Event::Ready {
                hierarchy,
                mut scenes,
                geometry,
                ..
            } = fixture.event(true)
            else {
                unreachable!()
            };
            // Keep a long pinned tab on screen while the card's title changes.
            let scene = scenes.iter_mut().find(|scene| scene.id == SCENE).unwrap();
            scene.title_pinned = true;
            scene.name = LONG.into();
            let mut graph = graph.clone();
            deliver(
                window,
                cx,
                Event::Ready {
                    hierarchy,
                    scenes,
                    graph: graph.clone(),
                    geometry,
                },
            );
            draw(window, cx);
            window
                .update(cx, |view, window, cx| {
                    assert_eq!(window.viewport_size().width, px(width));
                    assert!(!crate::macos::is_window_visible(window).unwrap());
                    view.qualify(PEER, window, cx);
                })
                .unwrap();
            draw(window, cx);
            let (card_width, workspace, initial) = window
                .update(cx, |view, _, cx| {
                    (
                        view.card_header_probe_width(PEER),
                        cx.entity(),
                        view.title_probe_projection(),
                    )
                })
                .unwrap();
            assert_eq!(initial.2, "Codex");
            assert_eq!(initial.0, LONG);
            assert!((px(198.)..=px(200.)).contains(&initial.1.size.width));
            let readings = Rc::new(HeaderReadings::default());
            let probe = cx
                .open_window(
                    WindowOptions {
                        show: false,
                        focus: false,
                        ..Default::default()
                    },
                    |_, cx| {
                        cx.new(|_| HeaderProbe {
                            workspace,
                            width: card_width,
                            title: Some("…"),
                            readings: readings.clone(),
                        })
                    },
                )
                .unwrap();
            let painted = |cx: &mut AsyncApp| {
                AnyWindowHandle::from(probe)
                    .update(cx, |_, window, cx| {
                        assert!(!crate::macos::is_window_visible(window).unwrap());
                        window.refresh();
                        window.draw(cx).clear();
                        let geometry = readings.title.borrow();
                        let geometry = geometry.as_ref().unwrap();
                        let run = geometry.layout.runs.last().unwrap();
                        assert!(geometry.layout.width <= geometry.bounds.size.width);
                        (
                            (run.font_id, run.glyphs.last().unwrap().id),
                            geometry.layout.len,
                            geometry.bounds,
                            readings.actions.borrow().clone(),
                        )
                    })
                    .unwrap()
            };
            let (ellipsis, _, _, actions) = painted(cx);
            probe.update(cx, |view, _, _| view.title = None).unwrap();
            for (case, (title, source, provider)) in [
                (LONG, "auto", LONG),
                ("手工标题", "manual", LONG),
                (LONG, "auto", LONG),
            ]
            .into_iter()
            .enumerate()
            {
                update(&mut graph, title, source, Some(provider));
                deliver(window, cx, Event::Graph(graph.clone()));
                draw(window, cx);
                let (tab_title, tab, card) = window
                    .update(cx, |view, _, _| view.title_probe_projection())
                    .unwrap();
                assert_eq!(
                    (tab_title, tab, card.as_str()),
                    (initial.0.clone(), initial.1, title)
                );
                let (last_glyph, len, bounds, buttons) = painted(cx);
                assert_eq!(buttons, actions);
                assert!(bounds.right() <= buttons[0].left());
                if title == LONG {
                    assert_eq!(last_glyph, ellipsis);
                    assert!(len < LONG.len());
                    assert!(
                        (bounds.size.width - (buttons[0].left() - px(12.)) * 0.46).abs() <= px(1.)
                    );
                } else {
                    assert_ne!(last_glyph, ellipsis);
                    assert_eq!(len, title.len());
                }
                println!(
                    "Codex title {width} pt | source={source} | card={bounds:?} | clipped={} | tab={tab:?} | actions unchanged",
                    len < title.len()
                );
                if width == 1200. && case < 2 {
                    let restore = case == 1;
                    let position = window
                        .update(cx, |view, _, _| view.fork_card_header(PEER))
                        .unwrap();
                    fork_click(window, cx, MouseButton::Right, position);
                    draw(window, cx);
                    window
                        .update(cx, |view, window, _| {
                            view.focus_title_menu_item(restore, window)
                        })
                        .unwrap();
                    press(window, cx, "enter");
                    if !restore {
                        window
                            .update(cx, |view, window, cx| view.replace_title_input(window, cx))
                            .unwrap();
                        draw(window, cx);
                        press(window, cx, "enter");
                    }
                    let submitted: Vec<_> = std::iter::from_fn(|| sink.try_recv().ok())
                        .filter_map(|command| match command {
                            Command::SetSessionTitle { session_id, title } => {
                                Some((session_id, title))
                            }
                            _ => None,
                        })
                        .collect();
                    let requested = (!restore).then(|| "手工标题".to_owned());
                    assert_eq!(submitted, [(PEER.to_owned(), requested.clone())]);
                    if !restore {
                        deliver(
                            window,
                            cx,
                            Event::SessionTitleResult {
                                session_id: PEER.into(),
                                title: requested.clone(),
                                error: Some("synthetic failure".into()),
                            },
                        );
                        window
                            .update(cx, |view, _, _| view.assert_title_dialog_state(true))
                            .unwrap();
                        draw(window, cx);
                        press(window, cx, "enter");
                        assert!(std::iter::from_fn(|| sink.try_recv().ok()).any(|command| matches!(command, Command::SetSessionTitle { session_id, title } if session_id == PEER && title == requested)));
                    }
                    deliver(
                        window,
                        cx,
                        Event::SessionTitleResult {
                            session_id: PEER.into(),
                            title: requested,
                            error: None,
                        },
                    );
                    window
                        .update(cx, |view, _, _| view.assert_title_dialog_state(false))
                        .unwrap();
                    println!(
                        "Codex title menu | {}",
                        if restore {
                            "restore sent from manual card"
                        } else {
                            "rename sent, failure kept input, retry succeeded"
                        }
                    );
                }
            }
            probe
                .update(cx, |_, window, _| window.remove_window())
                .unwrap();
            window
                .update(cx, |_, window, _| window.remove_window())
                .unwrap();
        }
    }

    fn verify_card_header_layout(cx: &mut AsyncApp) {
        const LONG: &str = "Claude 占位会话，这是一段有意加长用于检查卡头截断行为的验收标题";
        Workspace::assert_compact_status_color();
        let fixture = Fixture::new();
        let Some(BootstrapResult::Writable { connection, .. }) = &fixture.db else {
            unreachable!()
        };
        connection
            .execute(
                "UPDATE sessions SET title = ?, kind = 'claude-code' WHERE id = ?",
                (LONG, PEER),
            )
            .unwrap();

        for width in [720., 900., 1200.] {
            let (window, _sink) = open_hidden(cx, width);
            deliver(window, cx, fixture.event(true));
            draw(window, cx);
            window
                .update(cx, |view, window, cx| {
                    assert_eq!(window.viewport_size().width, px(width));
                    view.qualify(PEER, window, cx);
                })
                .unwrap();
            draw(window, cx);
            let card_width = window
                .update(cx, |view, _, _| view.card_header_probe_width(PEER))
                .unwrap();
            let workspace = window.update(cx, |_, _, cx| cx.entity()).unwrap();
            let readings = Rc::new(HeaderReadings::default());
            let probe = cx
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
                    |_, cx| {
                        cx.new(|_| HeaderProbe {
                            workspace,
                            width: card_width,
                            title: Some("…"),
                            readings: readings.clone(),
                        })
                    },
                )
                .unwrap();
            let mut ellipsis_glyph = None;
            let mut baseline_actions = Vec::new();
            for title in ["…", "Shell", LONG] {
                probe
                    .update(cx, |view, _, _| view.title = Some(title))
                    .unwrap();
                let buttons = AnyWindowHandle::from(probe).update(cx, |_, window, cx| {
                    assert!(!crate::macos::is_window_visible(window).unwrap());
                    window.refresh();
                    window.draw(cx).clear();
                    let geometry = readings.title.borrow();
                    let geometry = geometry.as_ref().unwrap();
                    let run = geometry.layout.runs.last().unwrap();
                    let last_glyph = (run.font_id, run.glyphs.last().unwrap().id);
                    let actions = readings.actions.borrow();
                    let loader = actions[0];
                    let remove = *actions.last().unwrap();
                    let title_budget = (loader.left() - px(12.)) * 0.46;
                    assert_eq!(remove.right(), card_width - px(12.));
                    assert_eq!(loader.size, size(px(24.), px(24.)));
                    assert_eq!(remove.size, size(px(27.), px(27.)));
                    assert_eq!(loader.center().y, px(21.));
                    assert!((remove.center().y - px(21.)).abs() <= px(0.5));
                    assert!(geometry.layout.width <= geometry.bounds.size.width);
                    assert!(geometry.bounds.right() <= loader.left());
                    if title == "…" {
                        ellipsis_glyph = Some(last_glyph);
                        baseline_actions.clone_from(&actions);
                        return None;
                    }
                    assert_eq!(*actions, baseline_actions);
                    let painted = if title == LONG {
                        let prefix = LONG.get(..geometry.layout.len - "…".len()).unwrap();
                        assert!(!prefix.is_empty() && prefix.len() < LONG.len());
                        assert_eq!(Some(last_glyph), ellipsis_glyph);
                        assert!((geometry.bounds.size.width - title_budget).abs() <= px(1.));
                        format!("{prefix}…")
                    } else {
                        assert_eq!(geometry.layout.len, title.len());
                        assert_ne!(Some(last_glyph), ellipsis_glyph);
                        assert!(geometry.bounds.size.width < title_budget);
                        title.to_owned()
                    };
                    println!(
                        "{width} pt | card={card_width:?} | title={painted:?} | budget={title_budget:?} | title bounds={:?} | actions={actions:?}",
                        geometry.bounds,
                    );
                    Some((loader, remove))
                }).unwrap();
                let Some((loader, remove)) = buttons else {
                    continue;
                };
                for (position, expected) in [
                    (loader.center(), (true, false)),
                    (remove.center(), (false, true)),
                    (point(px(1.), px(50.)), (false, false)),
                ] {
                    mouse(probe, cx, position, NSEventType::NSMouseMoved);
                    assert_eq!(
                        (readings.loader_hovered.get(), readings.remove_hovered.get()),
                        expected
                    );
                }
                println!("{width} pt | loader/remove hover passed");
            }
            probe
                .update(cx, |_, window, _| window.remove_window())
                .unwrap();

            for remove in [false, true] {
                let position = window
                    .update(cx, |view, _, _| {
                        if remove {
                            view.remove_button_position(PEER)
                        } else {
                            view.loader_button_position(PEER)
                        }
                    })
                    .unwrap();
                click(window, cx, position);
                window
                    .update(cx, |view, _, _| view.assert_card_header_dialog(remove))
                    .unwrap();
                draw(window, cx);
                press(window, cx, "escape");
                draw(window, cx);
                println!(
                    "{width} pt | {} opened and dismissed | {position:?}",
                    if remove { "remove" } else { "loader" }
                );
            }
            window
                .update(cx, |_, window, _| window.remove_window())
                .unwrap();
        }
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

    fn verify_deferred_recovery(cx: &mut AsyncApp, visible: bool) {
        use matou_native_runtime::host::{RecentReplies, RecentReply};

        const TIME: &str = "11/19 19:30";
        let fixture = Fixture::new();
        let set_title =
            |fixture: &Fixture, source: &str, provider: Option<&str>| {
                let Some(BootstrapResult::Writable { connection, .. }) = &fixture.db else {
                    unreachable!()
                };
                let title = if source == "manual" {
                    "手工卡名"
                } else {
                    "Claude Code"
                };
                connection.execute(
                "UPDATE sessions SET title_source=?1, provider_title=?2, title=?3 WHERE id=?4",
                (source, provider, title, PEER),
            ).unwrap();
            };
        let deferred_graph = |fixture: &Fixture, ready| {
            use matou_native_core::sessions::{claude_binding, get_session};
            let Some(BootstrapResult::Writable { connection, .. }) = &fixture.db else {
                unreachable!()
            };
            let mut event = fixture.event(ready);
            let (Event::Ready { graph, .. } | Event::Navigation { graph, .. }) = &mut event else {
                unreachable!()
            };
            let node = graph["nodes"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|node| node["sessionId"] == PEER)
                .unwrap();
            let session = get_session(connection, PEER).unwrap().unwrap();
            node["titleSource"] = serde_json::json!(session.title_source);
            node["providerTitle"] = serde_json::json!(session.provider_title);
            node["historyProviderSessionId"] = serde_json::json!(
                claude_binding(connection, PEER)
                    .unwrap()
                    .map(|binding| binding.provider_session_id)
            );
            if ready {
                event
            } else {
                Event::Graph(std::mem::take(graph))
            }
        };
        fixture.set_provider_restore("none", None, 3);
        set_title(&fixture, "default", Some("保存的会话名"));
        let (window, sink) = open_probe_window(cx, 1200., visible);
        deliver(window, cx, deferred_graph(&fixture, true));
        deliver(
            window,
            cx,
            Event::Recovery {
                session_id: PEER.into(),
                state: RecoveryState::Deferred,
            },
        );
        draw(window, cx);
        let cwd = fixture.root.to_str().unwrap();
        let verify = |cx: &mut AsyncApp, title: &str, time, replies: &[&str], loading| {
            window
                .update(cx, |view, _, _| {
                    view.verify_deferred_card(title, cwd, time, replies, loading)
                })
                .unwrap();
        };
        let send_preview = |cx: &mut AsyncApp, preview| {
            deliver(
                window,
                cx,
                Event::DeferredRecoveryPreview {
                    session_id: PEER.into(),
                    cwd: cwd.into(),
                    preview,
                },
            );
            draw(window, cx);
        };
        let detail = |has_reply: bool| RecentReplies {
            title: has_reply.then(|| "对话标题".into()),
            cwd: cwd.into(),
            updated_label: Some(TIME.into()),
            events: if has_reply {
                vec![RecentReply {
                    kind: "assistant".into(),
                    role: Some("assistant".into()),
                    timestamp: Some(serde_json::json!(1234)),
                    text: "最近的一条回复".into(),
                    tool_name: None,
                }]
            } else {
                Vec::new()
            },
        };
        verify(cx, "保存的会话名", None, &[], true);
        let mut sent = std::iter::from_fn(|| sink.try_recv().ok());
        assert!(sent.any(|command| matches!(command, Command::GetDeferredRecoveryPreview { session_id } if session_id == PEER)));
        send_preview(cx, Some(detail(true)));
        verify(cx, "保存的会话名", Some(TIME), &["最近的一条回复"], false);
        if visible {
            println!("Deferred recovery | visual review fixture ready");
            return;
        }
        set_title(&fixture, "default", None);
        deliver(window, cx, deferred_graph(&fixture, false));
        verify(cx, "对话标题", Some(TIME), &["最近的一条回复"], false);
        set_title(&fixture, "manual", Some("其他会话名"));
        deliver(window, cx, deferred_graph(&fixture, false));
        verify(cx, "手工卡名", Some(TIME), &["最近的一条回复"], false);
        set_title(&fixture, "default", None);
        deliver(window, cx, deferred_graph(&fixture, false));
        send_preview(cx, Some(detail(false)));
        let Event::Navigation { hierarchy, .. } = fixture.event(false) else {
            unreachable!()
        };
        let task_title = hierarchy.task.unwrap().title;
        verify(cx, &task_title, Some(TIME), &[], false);
        send_preview(cx, None);
        verify(cx, &task_title, None, &[], false);
        {
            let Some(BootstrapResult::Writable { connection, .. }) = &fixture.db else {
                unreachable!()
            };
            connection
                .execute(
                    "INSERT INTO session_graph_summaries VALUES (?1, ?2, 22)",
                    (PEER, r#"["No response requested.","  终端摘要  "]"#),
                )
                .unwrap();
        }
        deliver(window, cx, deferred_graph(&fixture, false));
        draw(window, cx);
        verify(cx, &task_title, None, &["终端摘要"], false);

        window
            .update(cx, |view, window, _| view.focus_deferred_action(0, window))
            .unwrap();
        press(window, cx, "enter");
        draw(window, cx);
        window
            .update(cx, |view, _, _| view.assert_deferred_history(true))
            .unwrap();
        assert!(sent.any(|command| matches!(command,
            Command::GetProviderSession { provider_session_id, cwd: directory, start_event_index: None, .. }
                if provider_session_id == "probe-conversation" && directory == cwd)));
        assert!(!sent.any(|command| matches!(
            command,
            Command::RetryRecovery { .. } | Command::LoadProviderSession { .. }
        )));
        press(window, cx, "escape");
        draw(window, cx);
        window
            .update(cx, |view, _, _| view.assert_deferred_history(false))
            .unwrap();
        window
            .update(cx, |view, window, _| view.focus_deferred_action(1, window))
            .unwrap();
        press(window, cx, "enter");
        assert!(sent.any(
            |command| matches!(command, Command::RetryRecovery { session_id } if session_id == PEER)
        ));
        println!(
            "Deferred recovery | SQLite + hidden GPUI verified loading, title precedence, Agent, timestamp, directory, replies, empty/error, terminal fallback, history and restore"
        );
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

    fn footer_bounds(
        window: WindowHandle<Workspace>,
        cx: &mut AsyncApp,
        key: &'static str,
    ) -> Option<Bounds<Pixels>> {
        window
            .update(cx, |_, _, _| Workspace::footer_bounds(key))
            .unwrap()
    }

    fn assert_footer_text(
        window: WindowHandle<Workspace>,
        cx: &mut AsyncApp,
        key: &'static str,
        expected: &str,
        ellipsis: bool,
    ) -> String {
        window
            .update(cx, |_, _, _| {
                Workspace::assert_painted_footer_text(key, expected, ellipsis)
            })
            .unwrap()
    }

    fn assert_shell_footer_layout(
        window: WindowHandle<Workspace>,
        cx: &mut AsyncApp,
        width: f32,
        shell: &str,
        short_cwd: &str,
        long: bool,
    ) {
        let status = footer_bounds(window, cx, "status").unwrap();
        let egress = footer_bounds(window, cx, "egress").unwrap();
        let account = footer_bounds(window, cx, "account").unwrap();
        assert_eq!(account.right(), px(width - 20.));
        assert!(egress.right() < account.left());
        assert!(egress.left() >= status.left());
        assert_eq!(account.center().y, egress.center().y);
        for (label, expected) in [
            ("label", "根会话"),
            ("count", "· 2 个会话"),
            ("name", shell),
            ("egress", "出口"),
            ("account", "Claude 账户"),
        ] {
            assert_footer_text(window, cx, label, expected, false);
        }
        let cwd = if width == 720. {
            assert!(footer_bounds(window, cx, "cwd").is_none());
            "省略".to_owned()
        } else {
            let bounds = footer_bounds(window, cx, "cwd").unwrap();
            assert!(bounds.size.width <= px(130.) && bounds.right() < egress.left());
            assert_footer_text(window, cx, "cwd", short_cwd, long)
        };
        println!(
            "footer {width} | root count 2 | shell={shell} | cwd={short_cwd} -> {cwd} | status={:?}, egress={egress:?}, account={account:?}",
            status.size.width
        );
    }

    fn verify_footer_base(cx: &mut AsyncApp) {
        const TITLE: &str = "父会话的超长标题，用于在不同窗口宽度检查面包屑保留会话数量并在同一行截断，标题末尾不应覆盖底栏中的其他字段";
        const TAIL: &str = "current-directory-with-a-name-long-enough-to-require-ellipsis";

        for width in [720., 900., 1200.] {
            let mut fixture = Fixture::new();
            let long_cwd = format!("/fixture/{TAIL}");
            let short_cwd = format!("~/{TAIL}");
            let Some(BootstrapResult::Writable { connection, .. }) = &fixture.db else {
                unreachable!()
            };
            connection
                .execute(
                    "UPDATE sessions SET cwd = ? WHERE id = ?",
                    (&long_cwd, PEER),
                )
                .unwrap();
            connection
                .execute(
                    "UPDATE sessions SET cwd = '/fixture/alpha', title = ? WHERE id = ?",
                    (TITLE, PARENT),
                )
                .unwrap();
            connection
                .execute("UPDATE sessions SET cwd = '' WHERE id = ?", [CHILD])
                .unwrap();
            connection
                .execute(
                    "UPDATE session_environment_bindings SET state = 'failed' WHERE session_id = ?",
                    [CHILD],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO provider_bindings (
                        id, session_id, provider, provider_session_id, resume_state,
                        metadata_json, created_at, updated_at, validated_at
                    ) VALUES ('footer-model-fixture', ?, 'claude-code', 'footer-model',
                        'resumed', '{\"model\":\"Claude Sonnet\",\"permissionMode\":\"plan\"}', 20, 20, 20)",
                    [CHILD],
                )
                .unwrap();

            let (window, sink) = open_hidden(cx, width);
            window
                .update(cx, |view, window, _| {
                    assert!(!crate::macos::is_window_visible(window).unwrap());
                    assert_eq!(window.viewport_size().width, px(width));
                    view.configure_footer_shell(if width == 900. {
                        "/bin/bash"
                    } else {
                        "/bin/zsh"
                    });
                })
                .unwrap();
            let shell = if width == 900. { "bash" } else { "zsh" };
            deliver(window, cx, fixture.event(true));
            draw(window, cx);
            window
                .update(cx, |view, _, _| {
                    view.assert_level(None, ROOTS, "根会话", "· 2 个会话", PEER);
                    view.assert_footer_fields(
                        Some(shell),
                        Some((&long_cwd, &short_cwd)),
                        None,
                        Some("Local"),
                        true,
                    );
                    view.assert_permission_chip(None);
                })
                .unwrap();
            assert_shell_footer_layout(window, cx, width, shell, &short_cwd, true);

            if width < 1000. {
                let activate_parent = window
                    .update(cx, |view, window, _| view.footer_card_point(PARENT, window))
                    .unwrap();
                click(window, cx, activate_parent);
            } else {
                window
                    .update(cx, |view, window, cx| view.select_footer_parent(window, cx))
                    .unwrap();
            }
            assert_focus(fixture.process(window, cx, &sink), PARENT);
            draw(window, cx);
            window
                .update(cx, |view, _, _| {
                    view.assert_level(None, ROOTS, "根会话", "· 2 个会话", PARENT);
                    view.assert_footer_fields(
                        Some(shell),
                        Some(("/fixture/alpha", "~/alpha")),
                        None,
                        Some("Local"),
                        true,
                    );
                })
                .unwrap();
            assert_shell_footer_layout(window, cx, width, shell, "~/alpha", false);
            let egress = footer_bounds(window, cx, "egress").unwrap().center();
            let account = footer_bounds(window, cx, "account").unwrap().center();
            click(window, cx, egress);
            click(window, cx, account);
            assert!(fixture.process(window, cx, &sink).is_empty());
            println!("footer {width} | inactive exit & account slots did not dispatch commands");

            window
                .update(cx, |view, window, cx| {
                    view.open_footer_child_level(window, cx)
                })
                .unwrap();
            assert_focus(fixture.process(window, cx, &sink), CHILD);
            draw(window, cx);
            window
                .update(cx, |view, window, cx| view.qualify(CHILD, window, cx))
                .unwrap();
            draw(window, cx);
            window
                .update(cx, |view, _, _| {
                    view.assert_level(
                        Some(PARENT),
                        CHILDREN,
                        &format!("{TITLE} 的子会话"),
                        "· 4 个会话",
                        CHILD,
                    );
                    view.assert_footer_fields(Some("Sonnet"), None, None, Some("待处理"), true);
                })
                .unwrap();
            for key in [
                "label",
                "count",
                "status",
                "permission",
                "name",
                "environment",
                "egress",
                "account",
            ] {
                println!(
                    "footer {width} | child {key}={:?}",
                    footer_bounds(window, cx, key)
                );
            }
            let title = assert_footer_text(window, cx, "label", &format!("{TITLE} 的子会话"), true);
            println!("footer {width} | painted child title={title}");
            assert_footer_text(window, cx, "count", "· 4 个会话", false);
            let name = assert_footer_text(window, cx, "name", "Sonnet", width == 720.);
            let environment =
                assert_footer_text(window, cx, "environment", "待处理", width == 720.);
            let secondary = footer_bounds(window, cx, "secondary").unwrap();
            let environment_bounds = footer_bounds(window, cx, "environment").unwrap();
            assert!(environment_bounds.left() >= secondary.left() - px(0.5));
            assert!(environment_bounds.right() <= secondary.right() + px(0.5));
            assert!(footer_bounds(window, cx, "cwd").is_none());
            window
                .update(cx, |view, _, _| {
                    view.assert_permission_chip(Some((
                        "Plan Mode",
                        "当前运行环境需要先恢复或交接",
                        true,
                    )))
                })
                .unwrap();
            let permission = footer_bounds(window, cx, "permission").unwrap();
            click(window, cx, permission.center());
            draw(window, cx);
            window
                .update(cx, |view, _, _| view.assert_permission_menu(false))
                .unwrap();
            println!(
                "footer {width} | child count 4 (includes stopped card), missing cwd omitted, model={name}, environment={environment}, disabled permission"
            );

            let Event::Graph(mut recovered) = fixture.graph_event() else {
                unreachable!()
            };
            let node = recovered["nodes"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|node| node["sessionId"] == CHILD)
                .unwrap();
            node["environment"]["state"] = "ready".into();
            node["cwd"] = "/fixture/recovered".into();
            deliver(window, cx, Event::Graph(recovered));
            draw(window, cx);
            window
                .update(cx, |view, _, _| {
                    view.assert_permission_chip(Some((
                        "Plan Mode",
                        "当前权限模式：Plan Mode，点击切换",
                        false,
                    )))
                })
                .unwrap();
            let chip = footer_bounds(window, cx, "permission").unwrap();
            click(window, cx, chip.center());
            draw(window, cx);
            window
                .update(cx, |view, _, _| view.assert_permission_menu(true))
                .unwrap();
            let menu = footer_bounds(window, cx, "permission-menu").unwrap();
            let anchor = chip.left().min(px(width) - menu.size.width);
            assert!((menu.left() - anchor).abs() <= px(0.5));
            assert!(menu.right() <= px(width));
            assert!((chip.top() - menu.bottom() - px(8.)).abs() <= px(0.5));
            let plan = window
                .update(cx, |view, window, _| view.permission_row_point(3, window))
                .unwrap();
            click(window, cx, plan);
            draw(window, cx);
            window
                .update(cx, |view, _, _| view.assert_permission_menu(false))
                .unwrap();
            assert!(fixture.process(window, cx, &sink).is_empty());
            println!(
                "footer {width} | measured permission chip opened anchored menu, selecting current Plan closed it without a command"
            );

            if width == 1200. {
                fixture.focus(SCENE, "child-running");
                deliver(window, cx, fixture.event(false));
                let Event::Graph(mut graph) = fixture.graph_event() else {
                    unreachable!()
                };
                let node = graph["nodes"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|node| node["sessionId"] == "child-running")
                    .unwrap();
                node["cwd"] = "/".into();
                node["git"] = serde_json::json!({"state": "unavailable", "dirty": false});
                deliver(window, cx, Event::Graph(graph));
                draw(window, cx);
                window
                    .update(cx, |view, _, _| {
                        view.assert_footer_fields(
                            None,
                            None,
                            Some(("Git 不可用", false)),
                            Some("Local"),
                            false,
                        );
                    })
                    .unwrap();
                assert!(footer_bounds(window, cx, "name").is_none());
                assert!(footer_bounds(window, cx, "cwd").is_none());
                assert!(footer_bounds(window, cx, "account").is_none());
                assert_footer_text(window, cx, "git", "Git 不可用", false);
                let git = footer_bounds(window, cx, "git").unwrap().center();
                click(window, cx, git);
                assert!(fixture.process(window, cx, &sink).is_empty());
                println!(
                    "footer {width} | Codex with no model and root-only cwd: both omitted, Git 不可用 inert, account omitted"
                );
            }

            let back = footer_bounds(window, cx, "return").unwrap();
            click(window, cx, back.center());
            assert_focus(fixture.process(window, cx, &sink), PARENT);
            draw(window, cx);
            window
                .update(cx, |view, _, _| {
                    view.assert_level(None, ROOTS, "根会话", "· 2 个会话", PARENT);
                })
                .unwrap();
            println!("footer {width} | mouse on measured breadcrumb returned to parent Shell");

            let Event::Graph(mut empty) = fixture.graph_event() else {
                unreachable!()
            };
            empty["nodes"].as_array_mut().unwrap().clear();
            deliver(window, cx, Event::Graph(empty));
            draw(window, cx);
            window
                .update(cx, |view, _, _| view.assert_footer_empty())
                .unwrap();
            assert_footer_text(window, cx, "count", "· 0 个会话", false);
            assert!(footer_bounds(window, cx, "status").is_none());
            println!("footer {width} | empty graph: root count zero, no stale HUD");
            window
                .update(cx, |_, window, _| window.remove_window())
                .unwrap();
        }
        println!("Footer base hidden GPUI probe passed");
    }

    fn verify_codex_hud(cx: &mut AsyncApp) {
        use serde_json::{Value, json};

        const LONG_MODEL: &str =
            "fixture-long-model-name-for-testing-single-line-ellipsis-in-the-current-codex-run";
        let long_label = format!("{LONG_MODEL} (high) (1.2M context)");
        let root_hud = json!({
            "model": LONG_MODEL, "effort": "high", "contextPercent": 86.0,
            "contextWindowSize": 1_200_000.0,
            "runningTools": [{"name": "Read", "target": "fixture.txt"}],
            "toolCounts": [{"name": "Bash", "count": 2}],
            "usageWindows": [{"label":"quota","percent":7}], "agents": [{"label":"hidden"}],
            "todos":[{"content":"hidden"}], "configCounts":{"mcpServers":2,"hooks":4},
            "teamRole":"hidden",
        });
        let parent_hud = json!({
            "model": "fixture-parent", "effort": "low", "contextPercent": 0.0,
            "contextWindowSize": 999.0, "toolCounts":[{"name":"Bash","count":1}]
        });
        let child_hud = json!({
            "model": "fixture-child", "effort": "medium", "contextPercent": 70.0,
            "contextWindowSize": 200_000.0, "toolCounts":[{"name":"Bash","count":4}]
        });
        let fitted = |window: WindowHandle<Workspace>, cx: &mut AsyncApp, model: &str| {
            let status = footer_bounds(window, cx, "status").expect("HUD status");
            let width = f32::from(status.size.width) - 13.;
            if width > 210. {
                window
                    .update(cx, |_, _, _| {
                        Workspace::assert_fitted_footer_text("name", model)
                    })
                    .unwrap()
            } else {
                assert!(footer_bounds(window, cx, "name").is_none());
                "模型按宽度折叠".into()
            }
        };
        for (width, show_model, show_context) in [
            (610., false, false),
            (650., true, false),
            (720., true, true),
            (900., true, true),
            (1200., true, true),
        ] {
            let mut fixture = Fixture::new();
            let Some(BootstrapResult::Writable { connection, .. }) = &fixture.db else {
                unreachable!()
            };
            connection
                .execute(
                    "UPDATE sessions SET kind = 'codex' WHERE id IN (?, ?, ?)",
                    [PARENT, PEER, CHILD],
                )
                .unwrap();

            let update = |fixture: &Fixture, child_payload: Option<&Value>| {
                let Event::Graph(mut graph) = fixture.graph_event() else {
                    unreachable!()
                };
                for node in graph["nodes"].as_array_mut().unwrap() {
                    let payload = match node["sessionId"].as_str() {
                        Some(PEER) => Some(&root_hud),
                        Some(PARENT) => Some(&parent_hud),
                        Some(CHILD) => child_payload,
                        _ => None,
                    };
                    if let Some(payload) = payload {
                        node["codexHud"] = payload.clone();
                    }
                }
                Event::Graph(graph)
            };

            let (window, sink) = open_hidden(cx, width);
            window
                .update(cx, |_, window, _| {
                    assert!(!crate::macos::is_window_visible(window).unwrap());
                    assert_eq!(window.viewport_size().width, px(width));
                })
                .unwrap();
            deliver(window, cx, fixture.event(true));
            deliver(window, cx, update(&fixture, Some(&child_hud)));
            draw(window, cx);
            draw(window, cx);
            let displayed = if show_model {
                assert_footer_text(window, cx, "name", &long_label, true)
            } else {
                assert!(footer_bounds(window, cx, "name").is_none());
                "模型折叠".into()
            };
            let context = if show_context {
                assert_footer_text(window, cx, "context", "86%", false)
            } else {
                assert!(footer_bounds(window, cx, "context").is_none());
                "上下文折叠".into()
            };
            assert_footer_text(window, cx, "tools", "3 Tools", false);
            let mut previous = footer_bounds(window, cx, "permission").unwrap();
            for (field, active) in [
                ("name", show_model),
                ("context", show_context),
                ("tools", true),
                ("egress", true),
            ] {
                if !active {
                    continue;
                }
                let bounds = footer_bounds(window, cx, field).unwrap();
                assert!(
                    previous.right() <= bounds.left() + px(0.5),
                    "{field} overlaps another field"
                );
                assert!((previous.center().y - bounds.center().y).abs() <= px(1.));
                previous = bounds;
            }
            assert_eq!(previous.right(), px(width - 20.));
            for hidden in [
                "account",
                "quota",
                "todos",
                "team",
                "agents",
                "mcp",
                "hooks",
                "instructions",
            ] {
                assert!(
                    footer_bounds(window, cx, hidden).is_none(),
                    "{hidden} rendered"
                );
            }
            let status_width =
                f32::from(footer_bounds(window, cx, "status").unwrap().size.width) - 13.;
            println!(
                "codex HUD GPUI | {width} pt | status content {status_width} pt | {displayed} / {context} / 3 Tools on one line, exit at {} pt, hidden fields omitted",
                width - 20.
            );
            if width < 720. {
                window
                    .update(cx, |_, window, _| window.remove_window())
                    .unwrap();
                continue;
            }

            if width < 1000. {
                let point = window
                    .update(cx, |view, window, _| view.footer_card_point(PARENT, window))
                    .unwrap();
                click(window, cx, point);
            } else {
                window
                    .update(cx, |view, window, cx| view.select_footer_parent(window, cx))
                    .unwrap();
            }
            assert_focus(fixture.process(window, cx, &sink), PARENT);
            deliver(window, cx, update(&fixture, Some(&child_hud)));
            draw(window, cx);
            let parent = fitted(window, cx, "fixture-parent (low) (999 context)");
            assert_footer_text(window, cx, "context", "0%", false);
            assert_footer_text(window, cx, "tools", "1 Tools", false);
            println!(
                "codex HUD GPUI | {width} pt | activated root card shows {parent}, 0% and 1 Tools"
            );

            window
                .update(cx, |view, window, cx| {
                    view.open_footer_child_level(window, cx)
                })
                .unwrap();
            assert_focus(fixture.process(window, cx, &sink), CHILD);
            deliver(window, cx, update(&fixture, Some(&child_hud)));
            draw(window, cx);
            draw(window, cx);
            let model = fitted(window, cx, "fixture-child (medium) (200K context)");
            let status = footer_bounds(window, cx, "status").unwrap();
            let context = if f32::from(status.size.width) - 13. > 250. {
                assert_footer_text(window, cx, "context", "70%", false)
            } else {
                assert!(footer_bounds(window, cx, "context").is_none());
                "上下文按宽度折叠".into()
            };
            assert_footer_text(window, cx, "tools", "4 Tools", false);
            println!(
                "codex HUD GPUI | {width} pt | child level shows {model}, {context} and 4 Tools"
            );

            deliver(window, cx, update(&fixture, None));
            draw(window, cx);
            for field in ["name", "context", "tools", "account"] {
                assert!(
                    footer_bounds(window, cx, field).is_none(),
                    "{field} leaked from former run"
                );
            }
            assert!(footer_bounds(window, cx, "permission").is_some());
            assert_footer_text(window, cx, "egress", "出口", false);
            let resumed = json!({"model":"fixture-resumed"});
            deliver(window, cx, update(&fixture, Some(&resumed)));
            draw(window, cx);
            fitted(window, cx, "fixture-resumed");
            assert!(footer_bounds(window, cx, "context").is_none());
            assert!(footer_bounds(window, cx, "tools").is_none());
            println!(
                "codex HUD GPUI | {width} pt | replacement graph clears old fields and shows the new run model when it arrives"
            );

            let back = footer_bounds(window, cx, "return").unwrap().center();
            click(window, cx, back);
            assert_focus(fixture.process(window, cx, &sink), PARENT);
            deliver(window, cx, update(&fixture, Some(&resumed)));
            draw(window, cx);
            fitted(window, cx, "fixture-parent (low) (999 context)");
            println!("codex HUD GPUI | {width} pt | back arrow restored parent HUD");
            window
                .update(cx, |_, window, _| window.remove_window())
                .unwrap();
        }
        println!("Codex HUD hidden GPUI probe passed");
    }

    async fn wait_for_instructions(window: WindowHandle<Workspace>, cx: &mut AsyncApp) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            draw(window, cx);
            let ready = window
                .update(cx, |view, _, _| view.instructions_ready())
                .unwrap();
            if ready {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "instruction file check did not finish"
            );
            cx.background_executor()
                .timer(Duration::from_millis(10))
                .await;
        }
    }

    async fn verify_project_instructions(cx: &mut AsyncApp) {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        for width in [720., 900., 1200.] {
            let mut fixture = Fixture::new();
            let repository = fixture.root.join("project");
            let root_cwd = repository.join("当前项目");
            let absent_cwd = root_cwd.join("nested");
            let child_cwd = absent_cwd.join("child");
            let unreadable_cwd = repository.join("unreadable");
            let changed_cwd = repository.join("changed");
            for dir in [&child_cwd, &unreadable_cwd, &changed_cwd] {
                fs::create_dir_all(dir).unwrap();
            }
            let root_file = root_cwd.join("AGENTS.md");
            let child_file = child_cwd.join("AGENTS.md");
            let denied_file = unreadable_cwd.join("AGENTS.md");
            let changed_file = changed_cwd.join("AGENTS.md");
            let missing_file = absent_cwd.join("AGENTS.md");
            let mut contents = (1..=250)
                .map(|line| format!("{line:03}  测试指令\n"))
                .collect::<String>();
            contents.push_str(&"一行很长的指令 ".repeat(100));
            fs::write(&root_file, &contents).unwrap();
            fs::write(&child_file, "# Child instructions\n").unwrap();
            fs::write(&changed_file, "# Updated cwd\n").unwrap();
            fs::write(&denied_file, "# Unreadable\n").unwrap();
            fs::set_permissions(&denied_file, fs::Permissions::from_mode(0o000)).unwrap();
            assert_eq!(
                fs::read(&denied_file).unwrap_err().kind(),
                std::io::ErrorKind::PermissionDenied
            );

            let set_card = |fixture: &Fixture, id: &str, mode: &str, cwd: &std::path::Path| {
                let Some(BootstrapResult::Writable { connection, .. }) = &fixture.db else {
                    unreachable!()
                };
                assert_eq!(
                    connection
                        .execute(
                            "UPDATE sessions SET kind = ?, cwd = ? WHERE id = ?",
                            (mode, cwd.to_str().unwrap(), id),
                        )
                        .unwrap(),
                    1
                );
            };
            set_card(&fixture, PEER, "codex", &root_cwd);
            set_card(&fixture, PARENT, "codex", &absent_cwd);
            set_card(&fixture, CHILD, "codex", &child_cwd);
            let (window, sink) = open_hidden(cx, width);
            let viewport = window
                .update(cx, |_, window, _| {
                    assert!(!crate::macos::is_window_visible(window).unwrap());
                    window.viewport_size()
                })
                .unwrap();
            assert_eq!(viewport.width, px(width));
            deliver(window, cx, fixture.event(true));
            let Event::Graph(mut graph) = fixture.graph_event() else {
                unreachable!()
            };
            let peer = graph["nodes"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|node| node["sessionId"] == PEER)
                .unwrap();
            peer["codexHud"] = serde_json::json!({
                "model": "test-model-with-a-long-name-to-exercise-the-hud",
                "effort": "high",
                "contextPercent": 80.0,
                "toolCounts": [{"name": "Bash", "count": 4}]
            });
            deliver(window, cx, Event::Graph(graph));
            wait_for_instructions(window, cx).await;
            let entry = footer_bounds(window, cx, "instructions").unwrap();
            let permission = footer_bounds(window, cx, "permission").unwrap();
            let status = footer_bounds(window, cx, "status").unwrap();
            let egress = footer_bounds(window, cx, "egress").unwrap();
            assert!(entry.left() >= status.left() && entry.right() < permission.left());
            assert_eq!(entry.center().y, permission.center().y);
            assert_eq!(egress.right(), px(width - 20.));
            assert_footer_text(window, cx, "instructions", "AGENTS.md", false);
            let tools = footer_bounds(window, cx, "tools").unwrap();
            assert!(tools.left() > permission.right() && tools.right() < egress.left());
            assert_footer_text(window, cx, "tools", "4 Tools", false);

            click(window, cx, entry.center());
            wait_for_instructions(window, cx).await;
            let panel = footer_bounds(window, cx, "instructions-dialog").unwrap();
            let panel_width = px((width - 64.).min(760.));
            let panel_height = (viewport.height - px(80.)).min(px(640.));
            // The bounds canvas fills the content box inside the 1 pt border.
            assert_eq!(
                panel.size,
                size(panel_width - px(2.), panel_height - px(2.))
            );
            assert_eq!(
                panel.center(),
                point(viewport.width / 2., viewport.height / 2.)
            );
            let scroll = window
                .update(cx, |view, window, _| {
                    assert!(view.instructions_focus("project-instructions-preview", window));
                    view.assert_instructions_preview(&root_file, Ok(Some(&contents)))
                })
                .unwrap();
            println!(
                "instructions | {width} pt | present: AGENTS.md before permission, tools and exit aligned, {:.0}×{:.0} panel, cwd path and exact UTF-8, preview autofocused",
                f32::from(panel_width),
                f32::from(panel_height),
            );

            assert!(scroll.max_offset().height > px(500.));
            assert_eq!(scroll.max_offset().width, px(0.));
            assert_eq!(scroll.offset().y, px(0.));
            let carousel_before = window.update(cx, |view, _, _| view.scroll_left()).unwrap();
            mouse(
                window,
                cx,
                scroll.bounds().center(),
                NSEventType::NSScrollWheel,
            );
            draw(window, cx);
            assert_eq!(scroll.offset().y, px(-130.));
            assert_eq!(
                window.update(cx, |view, _, _| view.scroll_left()).unwrap(),
                carousel_before
            );
            press(window, cx, "end");
            draw(window, cx);
            assert_eq!(scroll.offset().y, -scroll.max_offset().height);
            println!(
                "instructions | {width} pt | long preview: wheel moved 130 pt without moving cards, End reached the final line"
            );

            let assert_control_focus = |cx: &mut AsyncApp, control: &str| {
                assert!(
                    window
                        .update(cx, |view, window, _| view
                            .instructions_focus(control, window))
                        .unwrap()
                );
            };
            press(window, cx, "tab");
            draw(window, cx);
            assert_control_focus(cx, "project-instructions-cancel");
            press(window, cx, "tab");
            draw(window, cx);
            assert_control_focus(cx, "project-instructions-close");
            press(window, cx, "shift-tab");
            draw(window, cx);
            assert_control_focus(cx, "project-instructions-cancel");
            press(window, cx, "escape");
            draw(window, cx);
            assert!(footer_bounds(window, cx, "instructions-dialog").is_none());
            assert_control_focus(cx, "project-instructions");
            press(window, cx, "enter");
            wait_for_instructions(window, cx).await;
            assert_control_focus(cx, "project-instructions-preview");
            let cancel = footer_bounds(window, cx, "instructions-cancel")
                .unwrap()
                .center();
            click(window, cx, cancel);
            draw(window, cx);
            assert!(footer_bounds(window, cx, "instructions-dialog").is_none());
            assert_control_focus(cx, "project-instructions");
            println!(
                "instructions | {width} pt | keyboard: Tab/Shift-Tab cycle, Esc returns to entry, Enter reopens, Cancel dismisses"
            );

            if width == 900. {
                click(window, cx, entry.center());
                wait_for_instructions(window, cx).await;
                let close = footer_bounds(window, cx, "instructions-close")
                    .unwrap()
                    .center();
                click(window, cx, close);
                draw(window, cx);
                assert!(footer_bounds(window, cx, "instructions-dialog").is_none());
                click(window, cx, entry.center());
                wait_for_instructions(window, cx).await;
                click(window, cx, point(px(10.), px(10.)));
                draw(window, cx);
                assert!(footer_bounds(window, cx, "instructions-dialog").is_none());
                println!(
                    "instructions | {width} pt | close icon and backdrop dismiss; preview wheel stays inside"
                );

                // Deliver the same projection the runtime sends after an effective
                // cwd change, while the read-only preview is still open.
                click(window, cx, entry.center());
                wait_for_instructions(window, cx).await;
                set_card(&fixture, PEER, "codex", &changed_cwd);
                deliver(window, cx, fixture.graph_event());
                wait_for_instructions(window, cx).await;
                let new_scroll = window
                    .update(cx, |view, _, _| {
                        view.assert_instructions_preview(&changed_file, Ok(Some("# Updated cwd\n")))
                    })
                    .unwrap();
                assert_eq!(new_scroll.offset().y, px(0.));
                println!(
                    "instructions | {width} pt | same card cwd change: new path, new content, reset scroll"
                );

                set_card(&fixture, PEER, "codex", &unreadable_cwd);
                deliver(window, cx, fixture.graph_event());
                wait_for_instructions(window, cx).await;
                window
                    .update(cx, |view, _, _| {
                        view.assert_instructions_preview(&denied_file, Err("读取失败："));
                    })
                    .unwrap();
                assert!(footer_bounds(window, cx, "instructions").is_some());
                println!(
                    "instructions | {width} pt | unreadable mode 000: existing entry retained, 读取失败, old text cleared"
                );

                set_card(&fixture, PEER, "codex", &absent_cwd);
                deliver(window, cx, fixture.graph_event());
                wait_for_instructions(window, cx).await;
                window
                    .update(cx, |view, _, _| {
                        view.assert_instructions_preview(&missing_file, Ok(None));
                    })
                    .unwrap();
                assert!(footer_bounds(window, cx, "instructions").is_none());
                assert!(footer_bounds(window, cx, "instructions-dialog").is_some());
                press(window, cx, "escape");
                draw(window, cx);
                assert!(footer_bounds(window, cx, "instructions-dialog").is_none());
                println!(
                    "instructions | {width} pt | missing after cwd change: no entry, expected path and blank preview; ancestor file ignored"
                );

                set_card(&fixture, PEER, "codex", &root_cwd);
                deliver(window, cx, fixture.graph_event());
                wait_for_instructions(window, cx).await;
            }

            if width < 1000. {
                let card = window
                    .update(cx, |view, window, _| view.footer_card_point(PARENT, window))
                    .unwrap();
                click(window, cx, card);
            } else {
                window
                    .update(cx, |view, window, cx| view.select_footer_parent(window, cx))
                    .unwrap();
            }
            assert_focus(fixture.process(window, cx, &sink), PARENT);
            wait_for_instructions(window, cx).await;
            assert!(footer_bounds(window, cx, "instructions").is_none());
            println!(
                "instructions | {width} pt | active parent card: no current-file entry even with AGENTS.md one directory up"
            );
            window
                .update(cx, |view, window, cx| {
                    view.open_footer_child_level(window, cx)
                })
                .unwrap();
            assert_focus(fixture.process(window, cx, &sink), CHILD);
            wait_for_instructions(window, cx).await;
            let child_entry = footer_bounds(window, cx, "instructions").unwrap().center();
            click(window, cx, child_entry);
            wait_for_instructions(window, cx).await;
            window
                .update(cx, |view, _, _| {
                    view.assert_instructions_preview(
                        &child_file,
                        Ok(Some("# Child instructions\n")),
                    );
                })
                .unwrap();
            press(window, cx, "escape");
            draw(window, cx);
            let back = footer_bounds(window, cx, "return").unwrap().center();
            click(window, cx, back);
            assert_focus(fixture.process(window, cx, &sink), PARENT);
            wait_for_instructions(window, cx).await;
            assert!(footer_bounds(window, cx, "instructions").is_none());
            println!(
                "instructions | {width} pt | child level: shows child AGENTS.md only; return restores the parent with no entry"
            );

            fs::write(&missing_file, "# same path, shell mode\n").unwrap();
            set_card(&fixture, PARENT, "shell", &absent_cwd);
            deliver(window, cx, fixture.graph_event());
            wait_for_instructions(window, cx).await;
            assert!(footer_bounds(window, cx, "instructions").is_none());
            println!("instructions | {width} pt | Shell with physical AGENTS.md: entry omitted");
            window
                .update(cx, |_, window, _| window.remove_window())
                .unwrap();
        }
        println!("Project instructions hidden GPUI probe passed");
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
            println!("Permission | click chip at {at:?}");
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
            println!("Permission | click row {index} at {at:?}");
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
        assert!(footer_bounds(window, cx, "permission").is_none());
        let shell_hud = footer_bounds(window, cx, "status").unwrap();
        click(window, cx, shell_hud.center());
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

    fn verify_codex_history(cx: &mut AsyncApp) {
        use serde_json::{Value, json};

        const CHILD: &str = "00000000-0000-4000-8000-000000000001";
        let mut fixture = Fixture::new();
        fixture.fork_source(false, "idle");
        fixture.codex_permission_state("idle", "none");
        fixture.focus(SCENE, PARENT);
        let with_binding = |mut event: Event, detail: Option<Value>| {
            let (Event::Ready { graph, .. } | Event::Graph(graph)) = &mut event else {
                unreachable!()
            };
            graph["nodes"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|node| node["sessionId"] == PARENT)
                .unwrap()["historyProviderSessionId"] = json!(CHILD);
            if let Some(detail) = detail {
                graph["providerSessionDetailResults"] = json!([detail]);
            }
            event
        };
        let (window, sink) = open_hidden(cx, 1250.);
        deliver(window, cx, with_binding(fixture.event(true), None));
        deliver(
            window,
            cx,
            Event::Recovery {
                session_id: PARENT.into(),
                state: RecoveryState::Deferred,
            },
        );
        draw(window, cx);
        let open = |cx: &mut AsyncApp| {
            window
                .update(cx, |view, window, cx| {
                    view.open_codex_history_direct(PARENT, window, cx)
                })
                .unwrap();
        };
        let request_detail = |start, around, page_limit| {
            std::iter::from_fn(|| sink.try_recv().ok())
                .find_map(|command| match command {
                    Command::GetCodexSession {
                        request_id,
                        cwd,
                        provider_session_id,
                        start_event_index,
                        around_event_index,
                        limit,
                    } => {
                        assert_eq!(provider_session_id, CHILD);
                        assert_eq!(cwd, fixture.root.to_str().unwrap());
                        assert_eq!(
                            (start_event_index, around_event_index, limit),
                            (start, around, page_limit)
                        );
                        Some(request_id)
                    }
                    Command::LoadCodexSession { .. } | Command::RetryRecovery { .. } => {
                        panic!("history changed the session")
                    }
                    _ => None,
                })
                .expect("Codex history detail")
        };
        let reply = |cx: &mut AsyncApp, request, events: Value, start, end, total| {
            deliver(
                window,
                cx,
                with_binding(
                    fixture.graph_event(),
                    Some(json!({
                        "provider":"codex", "requestId":request, "providerSessionId":CHILD,
                        "events":events, "page":{"startEventIndex":start,"endEventIndex":end,
                            "total":total,"hasEarlier":start > 1,"hasLater":end < total}
                    })),
                ),
            );
            draw(window, cx);
        };
        open(cx);
        let request = request_detail(None, None, None);
        reply(
            cx,
            request,
            json!([{"index":3,"kind":"assistant","text":"本会话答案"}]),
            3,
            3,
            3,
        );
        window
            .update(cx, |view, _, cx| {
                view.assert_loader_preview(&[3], 3, true);
                view.codex_history_earlier(cx);
            })
            .unwrap();
        let tall = "样例\n".repeat(120);
        let older = json!([
            {"index":1,"kind":"assistant","text":tall},
            {"index":2,"kind":"user","text":"祖先线索"}
        ]);
        let request = request_detail(Some(1), None, Some(2));
        reply(cx, request, older.clone(), 1, 2, 3);
        window
            .update(cx, |view, window, cx| {
                view.assert_loader_preview(&[1, 2, 3], 3, false);
                view.codex_history_search(window, cx);
            })
            .unwrap();
        println!(
            "hidden Codex history | existing deferred panel opened child UUID, loaded latest and prepended earlier events"
        );

        let request_search = |expected_offset| {
            std::iter::from_fn(|| sink.try_recv().ok())
                .find_map(|command| {
                    if let Command::SearchCodexSession {
                        request_id,
                        provider_session_id,
                        query,
                        offset,
                        ..
                    } = command
                    {
                        assert_eq!(provider_session_id, CHILD);
                        assert_eq!(query, "祖先");
                        assert_eq!(offset, Some(expected_offset));
                        Some(request_id)
                    } else {
                        None
                    }
                })
                .expect("history content search")
        };
        let search_reply = |cx: &mut AsyncApp, request, hits, has_more, next_offset| {
            deliver(
                window,
                cx,
                with_binding(
                    fixture.graph_event(),
                    Some(json!({
                        "provider":"codex", "requestId":request, "providerSessionId":CHILD, "search":true,
                        "hits":hits,"hasMore":has_more,"nextOffset":next_offset
                    })),
                ),
            );
            draw(window, cx);
        };
        search_reply(
            cx,
            request_search(0),
            json!([{"eventIndex":2,"excerpt":"祖先线索"}]),
            true,
            1,
        );
        window
            .update(cx, |view, _, cx| view.codex_history_more_hits(cx))
            .unwrap();
        search_reply(
            cx,
            request_search(1),
            json!([{"eventIndex":3,"excerpt":"本会话也有祖先字样"}]),
            false,
            2,
        );
        window
            .update(cx, |view, _, cx| view.codex_history_open_hit(cx))
            .unwrap();
        let request = request_detail(None, Some(2), None);
        let mut events = older;
        events
            .as_array_mut()
            .unwrap()
            .push(json!({"index":3,"kind":"assistant","text":tall}));
        reply(cx, request, events, 1, 3, 3);
        window
            .update(cx, |view, window, cx| {
                view.assert_codex_history_jump();
                view.close_codex_history(window, cx);
            })
            .unwrap();
        println!(
            "hidden Codex history | paged search hit requested around index 2 and scrolled to its ancestor event"
        );

        open(cx);
        let request = request_detail(None, None, None);
        reply(cx, request, json!([]), 0, 0, 0);
        window
            .update(cx, |view, window, _| {
                view.assert_codex_history_empty();
                window.remove_window();
            })
            .unwrap();
        println!("hidden Codex history | empty event page rendered the existing empty state");
    }

    fn verify_codex_loader(cx: &mut AsyncApp) {
        use serde_json::{Value, json};
        let mut fixture = Fixture::new();
        fixture.fork_source(false, "idle");
        fixture.codex_permission_state("idle", "none");
        fixture.focus(SCENE, PARENT);
        let (window, sink) = open_hidden(cx, 1250.);
        deliver(window, cx, fixture.event(true));
        draw(window, cx);
        window
            .update(cx, |view, window, cx| {
                view.qualify(PARENT, window, cx);
                view.open_codex_loader_direct(PARENT, window, cx);
            })
            .unwrap();
        draw(window, cx);
        let list_request = || {
            std::iter::from_fn(|| sink.try_recv().ok())
                .find_map(|command| {
                    if let Command::ListCodexSessions {
                        request_id,
                        cwd,
                        offset,
                        limit,
                        ..
                    } = command
                    {
                        assert_eq!(offset, Some(0));
                        assert_eq!(limit, Some(50));
                        Some((request_id, cwd))
                    } else {
                        None
                    }
                })
                .expect("Codex list request")
        };
        let send_list = |cx: &mut AsyncApp, request_id, cwd: &str, second_owner: Value| {
            let Event::Graph(mut graph) = fixture.graph_event() else {
                unreachable!()
            };
            graph["providerSessionCatalogResults"] = json!([{
                "provider": "codex", "cwd": cwd, "requestId": request_id,
                "total": 2, "offset": 0, "limit": 50, "nextOffset": 2, "hasMore": false,
                "sessions": [
                    {"providerSessionId":"codex-first","title":"第一个会话","updatedAt":2,"permissionMode":"default","messageCount":0,"loadedBy":{"sessionId":PARENT,"title":"主卡"}},
                    {"providerSessionId":"codex-second","title":"第二个会话","updatedAt":1,"permissionMode":"default","messageCount":0,"loadedBy":second_owner}
                ]
            }]);
            deliver(window, cx, Event::Graph(graph));
            draw(window, cx);
        };
        let assert_state = |cx: &mut AsyncApp, selected: &str, focus, enabled| {
            window
                .update(cx, |view, window, _| {
                    view.verify_codex_loader_direct(selected, focus, enabled, window);
                })
                .unwrap();
        };
        let (request, cwd) = list_request();
        send_list(cx, request, &cwd, Value::Null);
        assert_state(cx, "codex-first", false, false);
        window
            .update(cx, |view, window, cx| {
                view.codex_loader_select_direct("codex-second", window, cx);
            })
            .unwrap();
        assert_state(cx, "codex-second", true, true);
        let preview = std::iter::from_fn(|| sink.try_recv().ok())
            .filter_map(|command| {
                if let Command::GetCodexSession {
                    request_id,
                    provider_session_id,
                    ..
                } = command
                    && provider_session_id == "codex-second"
                {
                    Some(request_id)
                } else {
                    None
                }
            })
            .last()
            .expect("selected Codex preview request");
        let Event::Graph(mut graph) = fixture.graph_event() else {
            unreachable!()
        };
        graph["providerSessionDetailResults"] = json!([{
            "provider": "codex", "cwd": cwd, "requestId": preview, "providerSessionId":"codex-second",
            "events": [{"index":1,"kind":"user","text":"合成首条提示"}],
            "page":{"startEventIndex":1,"endEventIndex":1,"total":1,"hasEarlier":false,"hasLater":false}
        }]);
        deliver(window, cx, Event::Graph(graph));
        draw(window, cx);
        window
            .update(cx, |view, _, _| view.assert_loader_preview(&[1], 1, false))
            .unwrap();
        println!(
            "Codex shared panel rendered preview; current-card occupation disabled the first selection"
        );

        // A title change refreshes the list without moving focus or clearing the preview.
        let Event::Graph(mut graph) = fixture.graph_event() else {
            unreachable!()
        };
        graph["nodes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|node| node["sessionId"] == PARENT)
            .unwrap()["title"] = json!("新卡名");
        deliver(window, cx, Event::Graph(graph));
        let (refresh, _) = list_request();
        let Event::Graph(mut graph) = fixture.graph_event() else {
            unreachable!()
        };
        graph["nodes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|node| node["sessionId"] == PARENT)
            .unwrap()["title"] = json!("新卡名");
        graph["providerSessionCatalogResults"] = json!([{
            "provider":"codex","cwd":cwd,"requestId":refresh,"total":2,"nextOffset":2,"hasMore":false,
            "sessions":[
                {"providerSessionId":"codex-first","title":"第一个会话","updatedAt":2,"permissionMode":"default","messageCount":0},
                {"providerSessionId":"codex-second","title":"已刷新会话","updatedAt":1,"permissionMode":"default","messageCount":0,"loadedBy":{"sessionId":PEER,"title":"其他卡"}}
            ]
        }]);
        deliver(window, cx, Event::Graph(graph));
        draw(window, cx);
        assert_state(cx, "codex-second", true, false);
        window
            .update(cx, |view, _, _| view.assert_loader_preview(&[1], 1, false))
            .unwrap();
        println!(
            "silent refresh kept selected row, preview and list focus; elsewhere occupation disabled Load"
        );

        window
            .update(cx, |view, window, cx| {
                view.codex_loader_close_direct(window, cx);
                view.open_codex_loader_direct(PARENT, window, cx);
            })
            .unwrap();
        draw(window, cx);
        let (reopen, _) = list_request();
        send_list(cx, reopen, &cwd, Value::Null);
        assert_state(cx, "codex-second", true, true);
        println!("reopen kept Codex source, selected UUID and list focus");
        // Isolated direct handlers drive the warning/confirm path without keyboard or mouse delivery.
        let Event::Graph(mut graph) = fixture.graph_event() else {
            unreachable!()
        };
        graph["nodes"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|node| node["sessionId"] == PARENT)
            .unwrap()["workStatus"] = json!("running");
        deliver(window, cx, Event::Graph(graph));
        window
            .update(cx, |view, _, cx| view.codex_loader_submit_direct(cx))
            .unwrap();
        assert!(
            !std::iter::from_fn(|| sink.try_recv().ok())
                .any(|command| matches!(command, Command::LoadCodexSession { .. }))
        );
        window
            .update(cx, |view, _, cx| view.codex_loader_submit_direct(cx))
            .unwrap();
        assert!(std::iter::from_fn(|| sink.try_recv().ok()).any(|command| matches!(command,
            Command::LoadCodexSession { session_id, provider_session_id } if session_id == PARENT && provider_session_id == "codex-second"
        )));
        println!(
            "running card required confirmation, then submitted exact target and selected UUID"
        );
        let Event::Graph(mut graph) = fixture.graph_event() else {
            unreachable!()
        };
        graph["providerSessionLoadErrors"] = json!([{"sessionId": PARENT,"providerSessionId":"codex-second","error":"fixture error"}]);
        deliver(window, cx, Event::Graph(graph));
        window
            .update(cx, |view, _, cx| {
                view.assert_loader_submit(false, true, Some("fixture error"));
                view.codex_loader_source_direct("claude-code", cx);
            })
            .unwrap();
        assert!(
            std::iter::from_fn(|| sink.try_recv().ok())
                .any(|command| matches!(command, Command::ListProviderSessions { .. }))
        );
        draw(window, cx);
        window
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        println!(
            "provider switch reused the same panel and Claude list command; no key or mouse event delivered"
        );
    }

    fn verify_codex_permission_mode(cx: &mut AsyncApp) {
        const RECOVERING: &str = "当前终端需要先完成恢复";
        const LABELS: [(&str, &str); 4] = [
            ("default", "Default"),
            ("acceptEdits", "Accept Edits"),
            ("plan", "Plan Mode"),
            ("bypassPermissions", "Bypass Permissions"),
        ];
        const ENTER: (&str, &str, &str) = (
            "切换到高权限模式",
            "切换到 Bypass Permissions 会打断当前 Codex 会话进程（正在执行的任务会被中断），重启后会自动 resume 恢复会话历史。\n\n是否确认？",
            "确认切换",
        );
        const EXIT: (&str, &str, &str) = (
            "退出高权限模式",
            "退出 Bypass Permissions 会打断当前 Codex 会话进程（正在执行的任务会被中断），重启后会自动 resume 恢复会话历史。\n\n是否确认？",
            "确认退出",
        );
        let mut fixture = Fixture::new();
        fixture.fork_source(false, "starting");
        fixture.codex_permission_state("starting", "none");
        fixture.focus(SCENE, PARENT);
        let (window, sink) = open_hidden(cx, 1250.);
        deliver(window, cx, fixture.event(true));
        draw(window, cx);
        window
            .update(cx, |view, window, cx| {
                view.qualify(PARENT, window, cx);
                view.assert_permission_chip(Some(("Default", RECOVERING, true)));
                view.toggle_codex_permission_menu(window, cx);
                view.assert_permission_menu(false);
            })
            .unwrap();
        println!("Codex starting | permission disabled even with a ready terminal frame");

        fixture.codex_permission_state("idle", "none");
        for (mode, label) in LABELS {
            fixture.permission_mode(Some(mode));
            deliver(window, cx, fixture.graph_event());
            let title = format!("当前权限模式：{label}，点击切换");
            window
                .update(cx, |view, window, cx| {
                    view.assert_permission_chip(Some((label, &title, false)));
                    view.toggle_codex_permission_menu(window, cx);
                    view.assert_permission_menu(true);
                    view.assert_codex_permission_rows();
                })
                .unwrap();
            draw(window, cx);
            window
                .update(cx, |view, window, cx| {
                    view.choose_codex_permission_mode(mode, window, cx);
                    view.assert_permission_menu(false);
                    view.assert_no_dialog();
                })
                .unwrap();
            assert!(fixture.process(window, cx, &sink).is_empty());
            println!(
                "Codex {label} | exact chip, tooltip and four drawn menu rows; selecting current mode sent no command"
            );
        }
        let choose = |cx: &mut AsyncApp, mode| {
            window
                .update(cx, |view, window, cx| {
                    view.toggle_codex_permission_menu(window, cx);
                    view.choose_codex_permission_mode(mode, window, cx);
                })
                .unwrap();
        };
        choose(cx, "auto");
        assert!(fixture.process(window, cx, &sink).is_empty());
        choose(cx, "plan");
        window
            .update(cx, |view, window, cx| {
                view.assert_permission_confirmation(EXIT);
                view.complete_codex_permission_confirmation(false, window, cx);
            })
            .unwrap();
        assert!(fixture.process(window, cx, &sink).is_empty());
        println!("Codex exit Bypass | frozen confirmation copy; cancel sent no command");

        fixture.permission_mode(Some("default"));
        deliver(window, cx, fixture.graph_event());
        choose(cx, "bypassPermissions");
        window
            .update(cx, |view, window, cx| {
                view.assert_permission_confirmation(ENTER);
                view.complete_codex_permission_confirmation(true, window, cx);
                view.assert_permission_chip(Some((
                    "Default",
                    "当前权限模式：Default，点击切换",
                    true,
                )));
                view.toggle_codex_permission_menu(window, cx);
                view.assert_permission_menu(false);
            })
            .unwrap();
        assert!(matches!(
            fixture.process(window, cx, &sink).as_slice(),
            [Command::SetProviderPermissionMode { session_id, mode }]
                if session_id == PARENT && mode == "bypassPermissions"
        ));
        println!(
            "Codex enter Bypass | frozen confirmation copy; confirm sent one command, pending kept previous mode disabled"
        );

        fixture.codex_permission_state("error", "failed");
        deliver(
            window,
            cx,
            fixture.permission_result("bypassPermissions", Some("复起夹具错误")),
        );
        window
            .update(cx, |view, window, cx| {
                view.assert_permission_chip(Some(("Default", RECOVERING, true)));
                view.assert_permission_toast(Some("切换失败：复起夹具错误"));
                view.toggle_codex_permission_menu(window, cx);
                view.assert_permission_menu(false);
            })
            .unwrap();
        fixture.codex_permission_state("starting", "restoring");
        deliver(window, cx, fixture.graph_event());
        window
            .update(cx, |view, _, _| {
                view.assert_permission_chip(Some(("Default", RECOVERING, true)));
            })
            .unwrap();
        fixture.codex_permission_state("idle", "none");
        fixture.permission_mode(Some("bypassPermissions"));
        deliver(window, cx, fixture.graph_event());
        window
            .update(cx, |view, window, _| {
                view.assert_permission_chip(Some((
                    "Bypass Permissions",
                    "当前权限模式：Bypass Permissions，点击切换",
                    false,
                )));
                window.remove_window();
            })
            .unwrap();
        println!(
            "Codex failure and recovery | toast copy, disabled failed/restoring states, reopened with confirmed mode"
        );
    }

    pub(super) fn run() {
        let footer_only = std::env::args().any(|arg| arg == "--footer-base");
        let instructions_only = std::env::args().any(|arg| arg == "--project-instructions");
        let codex_hud_only = std::env::args().any(|arg| arg == "--codex-hud");
        let header_only = std::env::args().any(|arg| arg == "--card-header");
        let codex_title_only = std::env::args().any(|arg| arg == "--codex-title");
        let water_pointer_only = std::env::args().any(|arg| arg == "--recovery-water-pointer");
        let deferred_only = std::env::args().any(|arg| arg == "--deferred-recovery");
        let show_deferred = std::env::args().any(|arg| arg == "--show-deferred-recovery");
        let loader_only = std::env::args().any(|arg| arg == "--session-loader");
        let fork_only = std::env::args().any(|arg| arg == "--fork");
        let permission_only = std::env::args().any(|arg| arg == "--permission");
        let codex_permission_only = std::env::args().any(|arg| arg == "--codex-permission");
        let codex_loader_only = std::env::args().any(|arg| arg == "--codex-loader");
        let codex_history_only = std::env::args().any(|arg| arg == "--codex-history");
        Application::new().with_assets(NativeAssets).run(move |cx| {
            if instructions_only {
                cx.spawn(async |cx| {
                    verify_project_instructions(cx).await;
                    cx.update(|cx| cx.quit()).unwrap();
                }).detach();
                return;
            }
            if codex_hud_only {
                cx.spawn(async |cx| {
                    verify_codex_hud(cx);
                    cx.update(|cx| cx.quit()).unwrap();
                }).detach();
                return;
            }
            if footer_only {
                cx.spawn(async |cx| {
                    verify_footer_base(cx);
                    cx.update(|cx| cx.quit()).unwrap();
                }).detach();
                return;
            }
            if codex_title_only {
                cx.spawn(async |cx| {
                    verify_codex_title_layout(cx);
                    cx.update(|cx| cx.quit()).unwrap();
                }).detach();
                return;
            }
            if header_only {
                cx.spawn(async |cx| {
                    verify_card_header_layout(cx);
                    verify_compact_card(cx);
                    verify_session_loader(cx);
                    verify_card_actions(cx);
                    verify_stopped_cards(cx);
                    verify_narrow_stopped_cards(cx);
                    cx.update(|cx| cx.quit()).unwrap();
                }).detach();
                return;
            }
            if water_pointer_only {
                cx.spawn(async |cx| {
                    verify_recovery_water_pointer(cx);
                    cx.update(|cx| cx.quit()).unwrap();
                }).detach();
                return;
            }
            if deferred_only || show_deferred {
                cx.spawn(async move |cx| {
                    verify_deferred_recovery(cx, show_deferred);
                    if !show_deferred {
                        cx.update(|cx| cx.quit()).unwrap();
                    }
                }).detach();
                return;
            }
            if codex_loader_only {
                cx.spawn(async |cx| {
                    verify_codex_loader(cx);
                    cx.update(|cx| cx.quit()).unwrap();
                }).detach();
                return;
            }
            if codex_history_only {
                cx.spawn(async |cx| {
                    verify_codex_history(cx);
                    cx.update(|cx| cx.quit()).unwrap();
                }).detach();
                return;
            }
            if codex_permission_only {
                cx.spawn(async |cx| {
                    verify_codex_permission_mode(cx);
                    cx.update(|cx| cx.quit()).unwrap();
                })
                .detach();
                return;
            }
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
                verify_card_header_layout(cx);
                verify_recovery_water_pointer(cx);
                verify_compact_card(cx);
                verify_start_failure_card(cx);
                verify_provider_restore_banner(cx).await;
                verify_session_loader(cx);
                verify_deferred_recovery(cx, false);
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
