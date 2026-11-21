use super::*;
use matou_native_core::link_routing::{
    BROWSER_RELEASES_URL, ConfirmationAction, ControlledBrowserFailure, LinkSource, OpenFailure,
    route_link, system_browser_confirmation,
};
use matou_native_desktop::{
    card_context_menu::{
        CardMenu, CardMenuAction, CardMenuContext, CardMenuEvent, CardMenuItem, CardPresentation,
        ForkReadiness, card_context_menu_with_selection, resolve_card_menu,
    },
    detach_window::{
        DetachOutcome, DetachedHeaderStatus, DetachedMount, DetachedWindowAction, DetachedWindows,
        detached_placeholder, detached_window_header,
    },
    egress_panel::{
        EgressAction, EgressBannerState, EgressCheck, EgressCheckKind, EgressCheckResult,
        EgressExit, EgressFamily, EgressFamilyStatus, EgressPanelState, EgressRule, EgressState,
        EgressStatus, EgressTransition, EgressViolation, render_egress_banner,
        render_egress_chip_with_label, render_egress_panel,
    },
    shortcut_panel::{
        ShortcutLocale, ShortcutPanelAction, ShortcutPanelClose, ShortcutPanelEffect,
        ShortcutPanelModel, ShortcutPanelState, ShortcutPlatform, ShortcutTheme, shortcut_panel,
    },
    split_tree::{PaneDirection, PaneStep, SplitDirection, SplitTree, split_tree_view},
    team_member_card::{
        TeamMemberActions, TeamMemberCardModel, TeamMemberWorkStatus, team_member_card,
    },
    update_popover::{
        UpdateAction, UpdateControl, UpdateEffect, UpdateErrorStage, UpdateInstallMode,
        UpdateProgress, UpdateRelease, UpdateState, UpdateStatus, UpdateView,
        render_update_popover, render_update_trigger,
    },
    workspace_board::{
        BOARD_FEEDBACK_DURATION, BoardFeedback, BoardMoveRequest, BoardView, WorkspaceBoard,
        render_workspace_board_with_sessions,
    },
};
use matou_native_runtime::{
    egress_probe::{Family, RuleMode},
    update_download::{self, UpdatePhase},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Overlay {
    Shortcuts,
    CardMenu,
    Update,
    Egress,
    Accounts,
}

#[derive(Clone)]
struct BoardSession {
    scene: String,
    id: String,
    title: String,
}

struct BoardScene {
    task: String,
    sessions: Vec<BoardSession>,
}

struct DetachedCard {
    window: WindowHandle<DetachedCardView>,
    title: String,
    workspace_id: String,
    node: Value,
}

type TriggerBounds = Rc<Cell<Option<Bounds<Pixels>>>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LinkOpenTarget {
    Controlled,
    System,
}

#[derive(Clone, Copy)]
enum LinkFallbackAction {
    Install,
    OpenSystem,
    Copy,
    Dismiss,
}

type LinkOpener =
    dyn Fn(&str, LinkOpenTarget, &mut App) -> gpui::Task<Result<(), ControlledBrowserFailure>>;
type LinkCopier = dyn Fn(&str, &mut App);

fn launch_link(
    url: &str,
    target: LinkOpenTarget,
    cx: &mut App,
) -> gpui::Task<Result<(), ControlledBrowserFailure>> {
    if target == LinkOpenTarget::System {
        cx.open_url(url);
        return gpui::Task::ready(Ok(()));
    }
    #[cfg(target_os = "macos")]
    {
        let url = url.to_owned();
        cx.background_executor().spawn(async move {
            let app = "/Applications/Mantle.app";
            if !std::path::Path::new(app)
                .join("Contents/MacOS/Chromium")
                .is_file()
            {
                return Err(ControlledBrowserFailure::Missing);
            }
            let opened = std::process::Command::new("/usr/bin/open")
                .args(["-a", app, &url])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success());
            opened
                .then_some(())
                .ok_or(ControlledBrowserFailure::LaunchFailed)
        })
    }
    #[cfg(not(target_os = "macos"))]
    gpui::Task::ready(Err(ControlledBrowserFailure::Missing))
}

pub(super) struct WorkspaceWiring {
    overlay: Option<Overlay>,
    overlay_window: Option<AnyWindowHandle>,
    overlay_focus: FocusHandle,
    restore_focus: Option<FocusHandle>,
    terminal_return: Option<(String, String)>,
    shortcuts: ShortcutPanelState,
    card_menu: Option<(String, Point<Pixels>)>,
    card_menu_index: Option<usize>,
    splits: HashMap<String, SplitTree>,
    pending_split: Option<PendingSplit>,
    link_opener: Rc<LinkOpener>,
    copy_link: Rc<LinkCopier>,
    link_in_progress: bool,
    pub(super) board_open: bool,
    board_scenes: HashMap<String, BoardScene>,
    board_request_id: u64,
    board_moves: Vec<(u64, BoardMoveRequest)>,
    board_feedback: Option<(u64, String, BoardFeedback)>,
    detached: DetachedWindows,
    children: HashMap<String, DetachedCard>,
    next_child: u64,
    visible: bool,
    update: UpdateControl,
    request_sequence: u64,
    update_request: Option<(u64, &'static str, UpdateErrorStage)>,
    update_release: Option<UpdateRelease>,
    update_error: Option<String>,
    update_bounds: TriggerBounds,
    egress: EgressPanelState,
    egress_status: EgressStatus,
    egress_request: Option<(u64, &'static str)>,
    egress_refresh_scheduled: bool,
    egress_error: Option<String>,
    account_request: Option<u64>,
    account_summary: Option<Value>,
    #[cfg(debug_assertions)]
    egress_probe_bounds: TriggerBounds,
    hud: HashMap<String, LiveHud>,
    banners: HashMap<String, EgressBannerState>,
    confirmation_focus: FocusHandle,
    _window_closed: Subscription,
}

#[derive(Default)]
struct LiveHud {
    mode: Option<&'static str>,
    request: Option<u64>,
    result: Value,
    refresh_at: Option<Duration>,
}

#[derive(Clone, Copy)]
enum EgressRequest {
    Status,
    Recheck,
    Exempt(EgressFamily),
    SetRule(EgressRule),
    SetProtection(bool),
    AcceptBaseline(EgressFamily),
}

struct PendingSplit {
    owner: String,
    source: String,
    direction: SplitDirection,
    existing: HashSet<String>,
}

impl WorkspaceWiring {
    pub(super) fn new(window: &Window, cx: &mut Context<Workspace>) -> Self {
        let main = window.window_handle();
        let owner = cx.weak_entity();
        let window_closed = cx.on_window_closed(move |cx| {
            let owner = owner.clone();
            cx.defer(move |cx| {
                let _ = main.update(cx, |_, window, cx| {
                    let _ = owner.update(cx, |this, cx| this.reap_detached(window, cx));
                });
            });
        });
        cx.on_release(|this, cx| {
            for child in this.wiring.children.values() {
                let _ = child
                    .window
                    .update(cx, |_, window, _| window.remove_window());
            }
        })
        .detach();

        Self {
            overlay: None,
            overlay_window: None,
            overlay_focus: cx.focus_handle().tab_stop(true),
            restore_focus: None,
            terminal_return: None,
            shortcuts: ShortcutPanelState::default(),
            card_menu: None,
            card_menu_index: None,
            splits: HashMap::new(),
            pending_split: None,
            link_opener: Rc::new(launch_link),
            copy_link: Rc::new(|url, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(url.to_owned()))
            }),
            link_in_progress: false,
            board_open: false,
            board_scenes: HashMap::new(),
            board_request_id: 0,
            board_moves: Vec::new(),
            board_feedback: None,
            detached: DetachedWindows::default(),
            children: HashMap::new(),
            next_child: 0,
            visible: true,
            update: UpdateControl::new(
                UpdateState {
                    current_version: env!("CARGO_PKG_VERSION").into(),
                    status: UpdateStatus::Idle,
                },
                None,
                Duration::ZERO,
            ),
            request_sequence: 0,
            update_request: None,
            update_release: None,
            update_error: None,
            update_bounds: Rc::default(),
            egress: EgressPanelState::default(),
            egress_status: EgressStatus {
                state: EgressState::Unknown,
                families: Vec::new(),
                checks: Vec::new(),
                rule_mode: Default::default(),
                protection_enabled: false,
                session_families: HashMap::new(),
                session_family_sets: HashMap::new(),
                checked_at: None,
                last_transition: None,
            },
            egress_request: None,
            egress_refresh_scheduled: false,
            egress_error: None,
            account_request: None,
            account_summary: None,
            #[cfg(debug_assertions)]
            egress_probe_bounds: TriggerBounds::default(),
            hud: HashMap::new(),
            banners: HashMap::new(),
            confirmation_focus: cx.focus_handle().tab_stop(true),
            _window_closed: window_closed,
        }
    }

    pub(super) fn is_detached(&self, id: &str) -> bool {
        self.detached.for_session(id).is_some()
    }
}

fn account_label(account: &Value) -> String {
    if let Some(email) = trimmed_graph_text(&account["email"])
        && let Some((prefix, _)) = email.split_once('@')
        && !prefix.is_empty()
    {
        return if prefix.contains('•') {
            prefix.chars().take(24).collect()
        } else {
            format!("{}•••", prefix.chars().take(2).collect::<String>())
        };
    }
    let name = trimmed_graph_text(&account["name"]).or_else(|| trimmed_graph_text(&account["id"]));
    name.map_or_else(
        || "•••".into(),
        |name| {
            let prefix: String = name.chars().take(4).collect();
            format!("{prefix}•••")
        },
    )
}

fn account_dot(account: &Value) -> Div {
    let color = match account["status"].as_str() {
        Some("active") => 0x3da66b,
        Some("expired") => 0xcf5856,
        Some("rate-limited") => 0xd79842,
        _ => 0x8d99a8,
    };
    div().flex_none().size(px(5.)).rounded_full().bg(rgb(color))
}

impl Workspace {
    pub(super) fn open_external_link(
        &mut self,
        url: &str,
        source: LinkSource<'static>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let link = match route_link(Some(url), source) {
            Ok(link) => link,
            Err(rejection) => {
                self.show_toast(rejection.message(self.locale), cx);
                return;
            }
        };
        if self.wiring.link_in_progress {
            return;
        }
        self.wiring.link_in_progress = true;
        let url = link.url().to_owned();
        let open = (self.wiring.link_opener)(&url, LinkOpenTarget::Controlled, cx);
        cx.spawn_in(window, async move |owner, cx| {
            if let Err(failure) = open.await {
                let prompt = owner.update_in(cx, |this, window, cx| {
                    this.link_failure_prompt(&url, source, failure, window, cx)
                });
                if let Ok(prompt) = prompt {
                    let action = prompt.await;
                    let _ = owner.update(cx, |this, cx| match action {
                        LinkFallbackAction::Install => {
                            if let Ok(releases) =
                                route_link(Some(BROWSER_RELEASES_URL), LinkSource::External)
                            {
                                (this.wiring.link_opener)(
                                    releases.url(),
                                    LinkOpenTarget::System,
                                    cx,
                                )
                                .detach();
                            }
                        }
                        LinkFallbackAction::OpenSystem => {
                            (this.wiring.link_opener)(&url, LinkOpenTarget::System, cx).detach();
                        }
                        LinkFallbackAction::Copy => {
                            (this.wiring.copy_link)(&url, cx);
                        }
                        LinkFallbackAction::Dismiss => {}
                    });
                }
            }
            let _ = owner.update(cx, |this, _| this.wiring.link_in_progress = false);
        })
        .detach();
    }

    fn link_failure_prompt(
        &self,
        url: &str,
        source: LinkSource<'static>,
        failure: ControlledBrowserFailure,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Task<LinkFallbackAction> {
        let cancel = PromptButton::cancel(self.locale.text("取消", "Cancel"));
        let mut actions = Vec::new();
        let answer = if matches!(source, LinkSource::ProviderKeyPage { .. }) {
            let fallback = route_link(Some(url), source)
                .ok()
                .and_then(|link| {
                    link.copy_fallback(
                        match failure {
                            ControlledBrowserFailure::Missing => OpenFailure::BridgeUnavailable,
                            ControlledBrowserFailure::LaunchFailed => {
                                OpenFailure::BrowserDidNotOpen
                            }
                        },
                        self.locale,
                    )
                })
                .expect("a routed preset URL has a copy fallback");
            actions.push(LinkFallbackAction::Copy);
            window.prompt(
                PromptLevel::Warning,
                &fallback.message(),
                None,
                &[
                    PromptButton::ok(self.locale.text("复制链接", "Copy link")),
                    cancel,
                ],
                cx,
            )
        } else {
            let confirmation = system_browser_confirmation(failure, self.locale);
            let buttons: Vec<_> = confirmation
                .buttons
                .iter()
                .enumerate()
                .map(|(index, button)| {
                    actions.push(match confirmation.selected_action(Some(index)) {
                        ConfirmationAction::InstallBrowser => LinkFallbackAction::Install,
                        ConfirmationAction::OpenSystemBrowser => LinkFallbackAction::OpenSystem,
                        ConfirmationAction::Cancel => LinkFallbackAction::Dismiss,
                    });
                    if button.action == ConfirmationAction::Cancel {
                        PromptButton::cancel(button.label)
                    } else if index == confirmation.default_index {
                        PromptButton::ok(button.label)
                    } else {
                        PromptButton::new(button.label)
                    }
                })
                .collect();
            window.prompt(
                PromptLevel::Warning,
                confirmation.title,
                Some(&confirmation.detail.map_or_else(
                    || confirmation.message.to_owned(),
                    |detail| format!("{}\n\n{detail}", confirmation.message),
                )),
                &buttons,
                cx,
            )
        };
        cx.spawn(async move |_, _| {
            answer
                .await
                .ok()
                .and_then(|index| actions.get(index))
                .copied()
                .unwrap_or(LinkFallbackAction::Dismiss)
        })
    }

    pub(super) fn render_split_canvas(
        &self,
        tree: &SplitTree,
        mut cards: HashMap<String, AnyElement>,
        cx: &Context<Self>,
    ) -> Div {
        let owner = self.carousel_owner.clone().unwrap();
        let view = cx.weak_entity();
        let palette = self.theme_palette();
        div()
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .border_t_1()
            .border_color(rgba(palette.chrome.divider))
            .bg(rgba(palette.cards.canvas))
            .children(wiring_probe_bounds("split-canvas".into()))
            .child(split_tree_view(
                tree,
                owner.clone().into(),
                rgba(palette.chrome.divider).into(),
                |_, id| {
                    div()
                        .relative()
                        .size_full()
                        .min_h_0()
                        .min_w_0()
                        .children(wiring_probe_bounds(format!("split-pane-{id}")))
                        .children(cards.remove(id))
                        .into_any_element()
                },
                move |change, _, cx| {
                    let _ = view.update(cx, |this, cx| {
                        if this.carousel_owner.as_ref() == Some(&owner)
                            && let Some(tree) = this.wiring.splits.get_mut(&owner)
                            && tree.apply_ratio(change, 0).is_some()
                        {
                            cx.notify();
                        }
                    });
                },
            ))
    }

    pub(super) fn active_split_tree(&self) -> Option<&SplitTree> {
        let tree = self.wiring.splits.get(self.carousel_owner.as_ref()?)?;
        let panes = tree.ordered_sessions();
        (self.read_only.is_none()
            && self
                .active_level_focus()
                .is_some_and(|id| panes.contains(&id))
            && panes
                .iter()
                .all(|id| self.order.iter().any(|card| card == id)))
        .then_some(tree)
    }

    fn can_split_pane(&self, id: &str) -> bool {
        self.active_path_valid()
            && self.carousel_owner.is_some()
            && self.wiring.pending_split.is_none()
            && !self.wiring.is_detached(id)
            && self.card_action_available(id)
            && self.terminals.contains_key(id)
            && self.order.iter().any(|card| card == id)
    }

    pub(super) fn split_pane(
        &mut self,
        id: &str,
        direction: SplitDirection,
        cx: &mut Context<Self>,
    ) {
        if !self.can_split_pane(id) || !self.wiring_available() || self.wiring.board_open {
            return;
        }
        let (Some(scene), Some(owner)) = (self.active_scene_id(), self.carousel_owner.clone())
        else {
            return;
        };
        let command = Command::CreateShellSibling {
            scene_id: scene.to_owned(),
            source_session_id: id.to_owned(),
            parent_session_id: self.level_parent().map(str::to_owned),
        };
        self.flush_geometry(cx);
        if self
            .commands
            .send_at(self.runtime_generation, command)
            .is_err()
        {
            return;
        }
        self.wiring.pending_split = Some(PendingSplit {
            owner,
            source: id.to_owned(),
            direction,
            existing: self.order.iter().cloned().collect(),
        });
        cx.notify();
    }

    pub(super) fn clear_pending_split(&mut self) {
        self.wiring.pending_split = None;
    }

    pub(super) fn finish_pane_split(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(request) = &self.wiring.pending_split else {
            return;
        };
        if self.carousel_owner.as_ref() != Some(&request.owner) {
            self.clear_pending_split();
            return;
        }
        let Some(new) = self
            .focused
            .as_deref()
            .filter(|id| {
                !request.existing.contains(*id)
                    && self.order.iter().any(|card| card == *id)
                    && self
                        .graph_node(id)
                        .is_some_and(|node| node["currentMode"] == "shell")
            })
            .map(str::to_owned)
        else {
            return;
        };
        let Some(scene_id) = self.active_scene_id().map(str::to_owned) else {
            return;
        };
        let request = self.wiring.pending_split.take().unwrap();
        if !self.order.contains(&request.source)
            || self.is_stopped_session(&request.source)
            || self.wiring.is_detached(&request.source)
        {
            return;
        }
        let tree = self
            .wiring
            .splits
            .entry(request.owner)
            .or_insert_with(|| SplitTree::single(scene_id.clone(), &request.source));
        if !tree.ordered_sessions().contains(&request.source.as_str()) {
            *tree = SplitTree::single(scene_id, &request.source);
        }
        if tree.split_session(&request.source, &new, request.direction) {
            self.stop_edge_browse(true);
            if self.wiring_available()
                && !self.wiring.board_open
                && let Some(terminal) = self.terminals.get(&new)
            {
                terminal.read(cx).focus.focus(window);
            }
            cx.notify();
        }
    }

    fn focus_split_pane(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.wiring.is_detached(id) {
            self.focus_detached_card(id, cx);
        } else if let Some(terminal) = self.terminals.get(id) {
            terminal.read(cx).focus.focus(window);
        }
        if self.geometry_focus() != Some(id)
            && let Some(scene) = self.active_scene_id().map(str::to_owned)
        {
            self.send_card_focus(&scene, id, cx);
        }
        cx.notify();
    }

    fn close_split_pane(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.geometry_focus();
        let Some(tree) = self.active_split_tree() else {
            return;
        };
        let next = (current == Some(id))
            .then(|| tree.adjacent_session(Some(id), PaneStep::Next))
            .flatten()
            .map(str::to_owned);
        let Some(owner) = &self.carousel_owner else {
            return;
        };
        let Some(tree) = self.wiring.splits.get_mut(owner) else {
            return;
        };
        if tree.close_session(id) && tree.ordered_sessions().len() == 1 {
            self.wiring.splits.remove(owner);
            self.restore_geometry = false;
        }
        if let Some(next) = next {
            self.focus_split_pane(&next, window, cx);
        }
        cx.notify();
    }

    pub(super) fn split_shortcut(
        &mut self,
        stroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let mods = stroke.modifiers;
        if !self.wiring_available() || self.wiring.board_open || self.wiring.overlay.is_some() {
            return false;
        }
        let Some(current) = self.active_level_focus().map(str::to_owned) else {
            return false;
        };
        if stroke.key == "d"
            && !mods.alt
            && (mods.shift || self.active_split_tree().is_some())
            && self.can_split_pane(&current)
        {
            let direction = if mods.shift {
                SplitDirection::Vertical
            } else {
                SplitDirection::Horizontal
            };
            self.split_pane(&current, direction, cx);
            return true;
        }
        let Some(tree) = self.active_split_tree() else {
            return false;
        };
        if mods.shift {
            return false;
        }
        let target = match (mods.alt, stroke.key.as_str()) {
            (true, "up" | "arrowup") => tree.directional_session(&current, PaneDirection::Up),
            (true, "down" | "arrowdown") => tree.directional_session(&current, PaneDirection::Down),
            (true, "left" | "arrowleft") => tree.directional_session(&current, PaneDirection::Left),
            (true, "right" | "arrowright") => {
                tree.directional_session(&current, PaneDirection::Right)
            }
            (false, "[") => tree.adjacent_session(Some(&current), PaneStep::Previous),
            (false, "]") => tree.adjacent_session(Some(&current), PaneStep::Next),
            (false, "w") => {
                self.close_split_pane(&current, window, cx);
                return true;
            }
            _ => return false,
        };
        if let Some(target) = target.map(str::to_owned) {
            self.focus_split_pane(&target, window, cx);
        }
        true
    }

    pub(super) fn wiring_dark(&self) -> bool {
        self.theme.effective() == matou_native_desktop::theme_tokens::AppTheme::Dark
    }

    fn wiring_available(&self) -> bool {
        self.settings.is_none()
            && self.dialog.is_none()
            && self
                .project_instructions
                .values()
                .all(|state| state.preview.is_none())
            && self.database_recovery.is_none()
            && !self.invalid_recovery_marker
            && (self.hierarchy.is_some() || self.read_only.is_some())
    }

    fn hud_wiring_available(&self, window: &Window) -> bool {
        if self
            .detached_session_for_window(window.window_handle())
            .is_some()
        {
            self.database_recovery.is_none()
                && !self.invalid_recovery_marker
                && !self.project_instructions_open(window)
                && !self
                    .detached_permission_confirmations
                    .contains_key(&window.window_handle())
        } else {
            self.wiring_available()
        }
    }

    fn active_terminal_focus(&self, cx: &App) -> Option<FocusHandle> {
        let id = self.active_level_focus()?;
        if !self.wiring_available()
            || self.wiring.board_open
            || self.read_only.is_some()
            || self.wiring.is_detached(id)
            || !self.active_path_valid()
            || self.recovery_blocked(id)
            || self
                .history_request
                .as_ref()
                .is_some_and(|request| request.session_id == id)
        {
            return None;
        }
        self.terminals
            .get(id)
            .map(|terminal| terminal.read(cx).focus.clone())
    }

    pub(super) fn return_terminal_focus(
        &mut self,
        scene: String,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.wiring_available() || self.read_only.is_some() {
            return;
        }
        self.dismiss_wiring_overlay(window, cx);
        if self.wiring.board_open {
            self.toggle_board(window, cx);
        }
        self.wiring.terminal_return = Some((scene, id));
        self.finish_terminal_return(window, cx);
        if self.control_focus["graph"].is_focused(window) {
            // Return must not reopen the graph while navigation or the terminal
            // frame is still being applied.
            window.blur();
        }
        cx.notify();
    }

    pub(super) fn finish_terminal_return(&mut self, window: &mut Window, cx: &App) {
        let Some(target @ (scene, id)) = self.wiring.terminal_return.as_ref() else {
            return;
        };
        if !self.wiring_available() || self.wiring.overlay.is_some() {
            self.wiring.terminal_return = None;
            return;
        }
        if self.active_scene_id() != Some(scene.as_str())
            || self.active_level_focus() != Some(id.as_str())
        {
            // The navigation request may still be projecting a different scene or level.
            if self.pending_focus.as_ref() != Some(target) {
                self.wiring.terminal_return = None;
            }
            return;
        }
        if let Some(focus) = self.active_terminal_focus(cx) {
            window.focus(&focus);
            self.wiring.terminal_return = None;
        }
    }

    fn open_wiring_overlay(&mut self, next: Overlay, window: &mut Window, cx: &mut Context<Self>) {
        let handle = window.window_handle();
        if self.wiring.overlay != Some(next) || self.wiring.overlay_window != Some(handle) {
            if let Some(previous) = self
                .wiring
                .overlay_window
                .filter(|previous| *previous != handle)
            {
                let focus = self.reset_wiring_overlay();
                let overlay_focus = self.wiring.overlay_focus.clone();
                let _ = previous.update(cx, |_, previous_window, cx| {
                    restore_wiring_focus(previous_window, cx, &overlay_focus, focus);
                });
            }
            self.dismiss_wiring_overlay(window, cx);
            self.dismiss_footer_popover(window, cx);
            self.close_notifications(cx);
            self.workspace_menu = None;
            self.task_menu = None;
            self.tab_menu = None;
            self.hidden_tabs = None;
            self.dismiss_permission_menu(window, cx);
            let previous = window.focused(cx);
            self.wiring.restore_focus = if next != Overlay::CardMenu && previous.is_none() {
                self.active_terminal_focus(cx).or(previous)
            } else {
                previous
            };
            self.wiring.overlay = Some(next);
            self.wiring.overlay_window = Some(handle);
            window.focus(&self.wiring.overlay_focus);
        }
        cx.notify();
    }

    pub(super) fn dismiss_wiring_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .wiring
            .overlay_window
            .is_some_and(|handle| handle != window.window_handle())
        {
            return;
        }
        if self.wiring.overlay.is_some() {
            let previous = self.reset_wiring_overlay();
            restore_wiring_focus(window, cx, &self.wiring.overlay_focus, previous);
            cx.notify();
        }
    }

    fn reset_wiring_overlay(&mut self) -> Option<FocusHandle> {
        self.wiring.overlay_window = None;
        match self.wiring.overlay.take() {
            Some(Overlay::Shortcuts) => {
                self.wiring
                    .shortcuts
                    .dispatch(ShortcutPanelAction::Close(ShortcutPanelClose::Toggle));
            }
            Some(Overlay::CardMenu) => {
                self.wiring.card_menu = None;
                self.wiring.card_menu_index = None;
            }
            Some(Overlay::Update) => {
                self.wiring.update.dispatch(UpdateAction::Dismiss);
            }
            Some(Overlay::Egress) => self.wiring.egress.open = false,
            Some(Overlay::Accounts) => {}
            None => {}
        }
        self.wiring.restore_focus.take()
    }

    pub(super) fn wiring_key(
        &mut self,
        stroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self
            .wiring
            .overlay_window
            .is_some_and(|handle| handle != window.window_handle())
        {
            return false;
        }
        let mods = stroke.modifiers;
        let mod_key = terminal_surface::terminal_mod(mods, cfg!(target_os = "macos"));
        let shortcut = mod_key
            && !mods.alt
            && (matches!(stroke.key.as_str(), "/" | "?") && !mods.shift
                || stroke.key == "?"
                || stroke.key_char.as_deref() == Some("?"));
        if self.wiring.overlay == Some(Overlay::Shortcuts) {
            if shortcut || stroke.key == "escape" {
                let reason = if shortcut {
                    ShortcutPanelClose::Toggle
                } else {
                    ShortcutPanelClose::Escape
                };
                self.close_shortcuts(reason, window, cx);
            }
            // Leave Enter/Space for the panel's close button. Keep Tab inside it.
            return !matches!(stroke.key.as_str(), "enter" | "space");
        }
        if stroke.key == "escape" && self.wiring.overlay.is_some() {
            self.dismiss_wiring_overlay(window, cx);
            return true;
        }
        if let Some((id, _)) = self.wiring.card_menu.clone() {
            let menu = self.wiring_card_menu(&id);
            match stroke.key.as_str() {
                "enter" | "space" => {
                    if let Some(item) = self
                        .wiring
                        .card_menu_index
                        .and_then(|index| menu.items.get(index))
                    {
                        if let Some(reason) = item
                            .disabled_reason
                            .as_ref()
                            .filter(|_| item.show_readiness_hint)
                        {
                            self.wiring_card_action(
                                &id,
                                CardMenuEvent::ReadinessHint(reason.clone()),
                                window,
                                cx,
                            );
                        } else if item.disabled_reason.is_none() {
                            self.wiring_card_action(
                                &id,
                                CardMenuEvent::Activate(item.action),
                                window,
                                cx,
                            );
                        }
                    }
                }
                "up" | "down" | "tab" => {
                    let eligible: Vec<_> = menu
                        .items
                        .iter()
                        .enumerate()
                        .filter_map(|(index, item)| {
                            (item.disabled_reason.is_none() || item.show_readiness_hint)
                                .then_some(index)
                        })
                        .collect();
                    let backward = stroke.key == "up" || stroke.key == "tab" && mods.shift;
                    if !eligible.is_empty() {
                        let current = eligible
                            .iter()
                            .position(|index| Some(*index) == self.wiring.card_menu_index);
                        let next = match (current, backward) {
                            (Some(index), true) => (index + eligible.len() - 1) % eligible.len(),
                            (Some(index), false) => (index + 1) % eligible.len(),
                            (None, true) => eligible.len() - 1,
                            (None, false) => 0,
                        };
                        self.wiring.card_menu_index = Some(eligible[next]);
                        cx.notify();
                    }
                }
                _ => {}
            }
            return true;
        }
        if shortcut && self.wiring_available() {
            self.toggle_shortcuts(window, cx);
            return true;
        }
        false
    }

    fn close_shortcuts(
        &mut self,
        reason: ShortcutPanelClose,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(ShortcutPanelEffect::RestoreTerminalFocus(_)) = self
            .wiring
            .shortcuts
            .dispatch(ShortcutPanelAction::Close(reason))
        {
            self.dismiss_wiring_overlay(window, cx);
        }
    }

    pub(super) fn toggle_shortcuts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.wiring_available() {
            return;
        }
        if self.wiring.shortcuts.is_open() {
            self.close_shortcuts(ShortcutPanelClose::Toggle, window, cx);
        } else {
            self.open_wiring_overlay(Overlay::Shortcuts, window, cx);
            self.wiring.shortcuts.dispatch(ShortcutPanelAction::Open);
        }
    }

    pub(super) fn toggle_board(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.wiring_available()
            || self
                .hierarchy
                .as_ref()
                .and_then(|h| h.workspace.as_ref())
                .is_none()
        {
            return;
        }
        self.dismiss_wiring_overlay(window, cx);
        self.stop_edge_browse(false);
        self.wiring.board_open = !self.wiring.board_open;
        if self.wiring.board_open
            && self
                .terminals
                .values()
                .any(|terminal| terminal.read(cx).focus.is_focused(window))
        {
            window.focus(&self.control_focus["sidebar-board"]);
        }
        if !self.wiring.board_open {
            self.restore_geometry = true;
        }
        cx.notify();
    }

    pub(super) fn sync_wiring_graph(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_ineligible_account_overlay(window, cx);
        let graph_scene = self.graph["sceneId"].as_str();
        let active_scene = self.hierarchy.as_ref().and_then(|h| h.scene.as_ref());
        if let Some(scene) = active_scene.filter(|scene| graph_scene == Some(scene.id.as_str())) {
            if let Some(owner) = &self.carousel_owner
                && let Some(tree) = self.wiring.splits.get_mut(owner)
            {
                let nodes = session_levels::nodes(&self.graph);
                let removed: Vec<_> = tree
                    .ordered_sessions()
                    .into_iter()
                    .filter(|id| {
                        session_levels::node(nodes, id)
                            .is_none_or(|node| !node["archivedAt"].is_null())
                    })
                    .map(str::to_owned)
                    .collect();
                for id in removed {
                    tree.close_session(&id);
                }
                if tree.ordered_sessions().len() < 2 {
                    self.wiring.splits.remove(owner);
                }
            }
            let sessions = session_levels::nodes(&self.graph)
                .iter()
                .filter(|node| {
                    node["archivedAt"].is_null() && node["currentMode"] != "agent-team-member"
                })
                .filter_map(|node| {
                    Some(BoardSession {
                        scene: scene.id.clone(),
                        id: node["sessionId"].as_str()?.into(),
                        title: node["title"].as_str().unwrap_or_default().into(),
                    })
                })
                .collect();
            self.wiring.board_scenes.insert(
                scene.id.clone(),
                BoardScene {
                    task: scene.task_id.clone(),
                    sessions,
                },
            );
            self.wiring.board_scenes.retain(|id, snapshot| {
                snapshot.task != scene.task_id || self.scenes.iter().any(|scene| scene.id == *id)
            });
        }
        if self
            .hierarchy
            .as_ref()
            .and_then(|h| h.workspace.as_ref())
            .is_none()
        {
            self.wiring.board_open = false;
        }

        let removed: Vec<_> = self
            .wiring
            .children
            .keys()
            .filter_map(|id| {
                let detached = self.wiring.detached.for_session(id)?;
                if graph_scene != Some(detached.mount.scene_id.as_str()) {
                    return None;
                }
                self.graph_node(id)
                    .is_none_or(|node| !node["archivedAt"].is_null())
                    .then(|| id.clone())
            })
            .collect();
        for id in removed {
            self.discard_wiring_session(&id, cx);
        }
        for node in session_levels::nodes(&self.graph) {
            if let Some(child) = node["sessionId"]
                .as_str()
                .and_then(|id| self.wiring.children.get_mut(id))
            {
                if let Some(title) = node["title"].as_str() {
                    child.title = title.into();
                }
                child.node = node.clone();
            }
        }
        if self.wiring.card_menu.as_ref().is_some_and(|(id, _)| {
            !self.order.contains(id) || !self.wiring_card_menu(id).can_open()
        }) {
            self.dismiss_wiring_overlay(window, cx);
        }
        self.refresh_live_wiring(cx);
    }

    pub(super) fn forget_board_scene(&mut self, scene_id: &str) {
        self.wiring.board_scenes.remove(scene_id);
    }

    fn board_sessions(&self) -> HashMap<String, Vec<BoardSession>> {
        let active_scene = self.active_scene_id();
        let mut scenes: Vec<_> = self.wiring.board_scenes.iter().collect();
        scenes.sort_by_key(|(id, _)| (Some(id.as_str()) != active_scene, id.as_str()));
        let mut groups = HashMap::<String, Vec<BoardSession>>::new();
        let mut seen = HashSet::new();
        for (_, scene) in scenes {
            for session in &scene.sessions {
                if seen.insert(&session.id) {
                    groups
                        .entry(scene.task.clone())
                        .or_default()
                        .push(session.clone());
                }
            }
        }
        groups
    }

    fn board_projection(
        &self,
        groups: &HashMap<String, Vec<BoardSession>>,
    ) -> Option<WorkspaceBoard> {
        let hierarchy = self.hierarchy.as_ref()?;
        let workspace = hierarchy.workspace.as_ref()?;
        let active_task = hierarchy.task.as_ref().map(|task| task.id.as_str());
        let counts = self.graph["workspaceSessionCountByTask"].as_object();
        let mut board = WorkspaceBoard::from_session_counts(
            &workspace.id,
            &workspace.name,
            active_task,
            &self.tasks,
            |task| match counts {
                Some(counts) => counts.get(task).and_then(Value::as_u64).unwrap_or(0) as usize,
                None => groups.get(task).map_or(0, Vec::len),
            },
        );
        // Replay pending moves on each fresh projection so unrelated updates do
        // not erase previews and a failed move never restores an old snapshot.
        for (_, request) in &self.wiring.board_moves {
            if let Ok(next) = board.after_drop(request) {
                board = next;
            }
        }
        Some(board)
    }

    fn move_task_on_board(&mut self, request: BoardMoveRequest, cx: &mut Context<Self>) {
        if !self.wiring.board_open {
            return;
        }
        self.wiring.board_request_id += 1;
        let request_id = self.wiring.board_request_id;
        self.wiring.board_feedback = None;
        let valid_drop = self
            .board_projection(&self.board_sessions())
            .is_some_and(|board| board.after_drop(&request).is_ok());
        if valid_drop
            && self
                .commands
                .send_at(
                    self.runtime_generation,
                    Command::SetTaskStatus {
                        request_id,
                        workspace_id: request.workspace_id.clone(),
                        task_id: request.task_id.clone(),
                        status: request.status.as_str().into(),
                        before_task_id: request.before_task_id.clone(),
                    },
                )
                .is_ok()
        {
            self.wiring.board_moves.push((request_id, request));
            cx.notify();
        } else {
            self.show_board_feedback(
                request_id,
                request.workspace_id,
                BoardFeedback::SaveFailed,
                cx,
            );
        }
    }

    fn show_board_feedback(
        &mut self,
        request_id: u64,
        workspace_id: String,
        feedback: BoardFeedback,
        cx: &mut Context<Self>,
    ) {
        if request_id != self.wiring.board_request_id {
            return;
        }
        self.wiring.board_feedback = Some((request_id, workspace_id, feedback));
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(BOARD_FEEDBACK_DURATION)
                .await;
            let _ = this.update(cx, |this, cx| {
                if this
                    .wiring
                    .board_feedback
                    .as_ref()
                    .is_some_and(|(id, _, _)| *id == request_id)
                {
                    this.wiring.board_feedback = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn board_view(&self, groups: &HashMap<String, Vec<BoardSession>>) -> Option<BoardView> {
        let workspace = self.hierarchy.as_ref()?.workspace.as_ref()?;
        let feedback = self
            .wiring
            .board_feedback
            .as_ref()
            .filter(|(_, owner, _)| *owner == workspace.id)
            .map(|(_, _, feedback)| *feedback);
        Some(self.board_projection(groups)?.view(self.locale, feedback))
    }

    pub(super) fn render_board(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if !self.wiring.board_open {
            return None;
        }
        let groups = self.board_sessions();
        let view = self.board_view(&groups)?;
        let owner = cx.weak_entity();
        let move_owner = owner.clone();
        let palette = self.theme_palette().workspace;
        Some(
            render_workspace_board_with_sessions(
                &view,
                self.wiring_dark(),
                Rc::new(move |request, _, cx| {
                    let _ = move_owner.update(cx, |this, cx| this.move_task_on_board(request, cx));
                }),
                Rc::new(move |task| {
                    groups
                        .get(task)
                        .into_iter()
                        .flatten()
                        .map(|session| {
                            let target = session.clone();
                            let owner = owner.clone();
                            div()
                                .id(SharedString::from(format!("board-session-{}", session.id)))
                                .focusable()
                                .tab_index(0)
                                .mt(px(5.))
                                .px(px(6.))
                                .py(px(3.))
                                .rounded(px(4.))
                                .text_color(rgba(palette.text))
                                .hover(move |style| style.bg(rgba(palette.accent_soft)))
                                .truncate()
                                .cursor_pointer()
                                .relative()
                                .children(wiring_probe_bounds(format!(
                                    "board-session-{}",
                                    session.id
                                )))
                                .child(session.title.clone())
                                .on_click(move |_, window, cx| {
                                    let _ = owner.update(cx, |this, cx| {
                                        this.select_board_session(&target, window, cx)
                                    });
                                })
                                .into_any_element()
                        })
                        .collect()
                }),
            )
            .into_any_element(),
        )
    }

    fn select_board_session(
        &mut self,
        target: &BoardSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.wiring.board_open {
            return;
        }
        self.toggle_board(window, cx);
        window.focus(&self.control_focus["sidebar-board"]);
        if self.active_scene_id() == Some(target.scene.as_str())
            && self.graph_node(&target.id).is_some()
        {
            let parent = self
                .graph_node(&target.id)
                .and_then(session_levels::parent_id)
                .map(str::to_owned);
            self.level_parent_by_scene
                .insert(target.scene.clone(), parent);
            self.refresh_level(cx);
            self.restore_geometry = false;
            self.focus_follow_until =
                Some(self.motion_clock.elapsed().as_secs_f64() * 1000. + 440.);
            self.activate_session_card(&target.id, window, cx);
            if let Some(focus) = self.card_controls.get(&target.id) {
                window.focus(focus);
            }
        } else {
            self.send_card_focus(&target.scene, &target.id, cx);
            self.level_parent_by_scene.remove(&target.scene);
        }
    }

    pub(super) fn wiring_card_menu(&self, id: &str) -> CardMenu {
        let Some(node) = self.graph_node(id) else {
            return resolve_card_menu(CardMenuContext::default(), self.locale);
        };
        let kind = node["currentMode"].as_str().unwrap_or_default();
        let archived = !node["archivedAt"].is_null();
        let can_mutate = self.card_action_available(id);
        let retry_fork = kind == "antigravity"
            && !archived
            && !self.wiring.is_detached(id)
            && fork_failure_copy(node).is_some()
            && self.fork_repair_blocked_reason(id, node).is_none();
        let fork = self
            .fork_access(id, ForkPlacement::Peer)
            .filter(|access| access.blocked.is_none())
            .map(|access| {
                if access.readiness.is_none() {
                    ForkReadiness::Ready
                } else if matches!(
                    node["workStatus"].as_str(),
                    Some("running" | "starting" | "needs-input")
                ) {
                    ForkReadiness::WaitingForReply
                } else {
                    ForkReadiness::NeedsFirstReply
                }
            });
        let mut menu = resolve_card_menu(
            CardMenuContext {
                kind,
                presentation: if self.wiring.is_detached(id) {
                    CardPresentation::Detached
                } else if archived {
                    CardPresentation::Archived
                } else {
                    CardPresentation::Attached
                },
                can_rename: can_mutate,
                can_restore_auto_title: can_mutate
                    && kind == "codex"
                    && node["titleSource"] == "manual"
                    && trimmed_graph_text(&node["providerTitle"]).is_some(),
                fork,
                // For Codex, Runtime validates both the repository and the
                // trusted isolation root when the New worktree action is sent.
                git_available: kind == "codex" || !node["git"].is_null(),
                can_detach: can_mutate && self.terminals.contains_key(id),
                can_view_history: can_mutate
                    && kind == "claude-code"
                    && trimmed_graph_text(&node["cwd"]).is_some(),
                can_close_session: can_mutate,
                can_remove_node: self.remove_session_available(id),
                read_only: self.read_only.is_some(),
                storage_fault: self.storage_fault_sessions.contains(id),
                environment_unavailable: !self.active_path_valid()
                    || node["environment"]["state"]
                        .as_str()
                        .is_some_and(|state| state != "ready"),
                recovering: !retry_fork && self.recovery_blocked(id),
            },
            self.locale,
        );
        if retry_fork {
            let pending = matches!(self.fork_retries.get(id), Some(ForkRetry::Pending));
            let label = if pending {
                self.locale.text("正在重试分叉…", "Retrying fork…")
            } else {
                self.locale.text("重试创建分支", "Retry fork")
            };
            menu.items.insert(
                0,
                CardMenuItem {
                    action: CardMenuAction::RetryFork,
                    label: label.into(),
                    disabled_reason: pending.then(|| label.into()),
                    show_readiness_hint: false,
                    danger: false,
                },
            );
        }
        if kind != "codex"
            && let Some(item) = menu
                .items
                .iter_mut()
                .find(|item| item.action == CardMenuAction::ForkWorktree)
            && item.disabled_reason.is_none()
        {
            item.disabled_reason = Some(
                self.locale
                    .text(
                        "当前仅支持在原工作树 Fork",
                        "Forking currently supports the original worktree only",
                    )
                    .into(),
            );
        }
        if self.can_split_pane(id)
            || self
                .active_split_tree()
                .is_some_and(|tree| tree.ordered_sessions().contains(&id))
        {
            let mut actions = Vec::new();
            if self.can_split_pane(id) {
                actions.extend([
                    (
                        CardMenuAction::SplitHorizontal,
                        self.locale.text("左右分屏", "Split left and right"),
                    ),
                    (
                        CardMenuAction::SplitVertical,
                        self.locale.text("上下分屏", "Split top and bottom"),
                    ),
                ]);
            }
            if self
                .active_split_tree()
                .is_some_and(|tree| tree.ordered_sessions().contains(&id))
            {
                actions.push((
                    CardMenuAction::ClosePane,
                    self.locale.text("关闭分屏", "Close pane"),
                ));
            }
            let index = menu
                .items
                .iter()
                .position(|item| item.danger)
                .unwrap_or(menu.items.len());
            menu.items.splice(
                index..index,
                actions.into_iter().map(|(action, label)| CardMenuItem {
                    action,
                    label: label.into(),
                    disabled_reason: None,
                    show_readiness_hint: false,
                    danger: false,
                }),
            );
        }
        menu
    }

    pub(super) fn open_wiring_card_menu(
        &mut self,
        id: &str,
        origin: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.wiring_available()
            || !self.order.iter().any(|item| item == id)
            || !self.wiring_card_menu(id).can_open()
        {
            return;
        }
        self.open_wiring_overlay(Overlay::CardMenu, window, cx);
        self.wiring.card_menu = Some((id.into(), origin));
        self.wiring.card_menu_index = None;
    }

    #[cfg(debug_assertions)]
    pub(super) fn wiring_card_menu_target(&self) -> Option<&str> {
        self.wiring.card_menu.as_ref().map(|(id, _)| id.as_str())
    }

    #[cfg(debug_assertions)]
    pub(super) fn select_card_menu_action_for_probe(
        &mut self,
        action: CardMenuAction,
        cx: &mut Context<Self>,
    ) {
        let (id, _) = self.wiring.card_menu.as_ref().expect("open card menu");
        self.wiring.card_menu_index = Some(
            self.wiring_card_menu(id)
                .items
                .iter()
                .position(|item| item.action == action)
                .expect("menu action present"),
        );
        cx.notify();
    }

    fn wiring_card_action(
        &mut self,
        id: &str,
        event: CardMenuEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_wiring_overlay(window, cx);
        match event {
            CardMenuEvent::Dismiss => {}
            CardMenuEvent::ReadinessHint(hint) => self.show_fork_readiness(hint, cx),
            CardMenuEvent::Activate(action) => {
                if !self
                    .wiring_card_menu(id)
                    .items
                    .iter()
                    .any(|item| item.action == action && item.disabled_reason.is_none())
                {
                    return;
                }
                match action {
                    CardMenuAction::Rename => self.open_session_rename(id.into(), window, cx),
                    CardMenuAction::RestoreAutoTitle => self.send(
                        Command::SetSessionTitle {
                            session_id: id.into(),
                            title: None,
                        },
                        cx,
                    ),
                    CardMenuAction::Fork => self.open_fork(id, ForkPlacement::Peer, window, cx),
                    CardMenuAction::ForkWorktree => self.open_fork_with_worktree(
                        id,
                        ForkPlacement::Peer,
                        ForkWorktreeMode::New,
                        window,
                        cx,
                    ),
                    CardMenuAction::RetryFork => self.retry_fork(id, cx),
                    CardMenuAction::SplitHorizontal => {
                        self.split_pane(id, SplitDirection::Horizontal, cx)
                    }
                    CardMenuAction::SplitVertical => {
                        self.split_pane(id, SplitDirection::Vertical, cx)
                    }
                    CardMenuAction::ClosePane => self.close_split_pane(id, window, cx),
                    CardMenuAction::OpenDetached => self.detach_card(id, window, cx),
                    CardMenuAction::ReturnToMain => self.return_detached_card(id, window, cx),
                    CardMenuAction::ViewHistory => {
                        self.open_session_loader_mode(id, true, window, cx)
                    }
                    CardMenuAction::RemoveNode => self.open_remove_session(id, window, cx),
                    CardMenuAction::CloseSession => {
                        self.request_close_session_by_id(id, window, cx)
                    }
                }
            }
        }
    }

    pub(super) fn render_wiring_overlay(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.hud_wiring_available(window)
            || self
                .wiring
                .overlay_window
                .is_some_and(|handle| handle != window.window_handle())
        {
            return None;
        }
        let owner = cx.weak_entity();
        let dark = self.wiring_dark();
        let focus = &self.wiring.overlay_focus;
        match self.wiring.overlay? {
            Overlay::Shortcuts => shortcut_panel(
                &self.wiring.shortcuts,
                ShortcutPanelModel {
                    platform: if cfg!(target_os = "macos") {
                        ShortcutPlatform::Mac
                    } else {
                        ShortcutPlatform::Windows
                    },
                    locale: match self.locale {
                        InterfaceLocale::Chinese => ShortcutLocale::ZhCn,
                        InterfaceLocale::English => ShortcutLocale::En,
                    },
                    theme: if dark {
                        ShortcutTheme::Dark
                    } else {
                        ShortcutTheme::Light
                    },
                },
                focus,
                move |reason, window, cx| {
                    let _ = owner.update(cx, |this, cx| this.close_shortcuts(reason, window, cx));
                },
            )
            .map(IntoElement::into_any_element),
            Overlay::CardMenu => {
                let (id, origin) = self.wiring.card_menu.as_ref()?;
                let target = id.clone();
                card_context_menu_with_selection(
                    &self.wiring_card_menu(id),
                    *origin,
                    window.viewport_size(),
                    focus,
                    !dark,
                    self.wiring.card_menu_index,
                    move |event, window, cx| {
                        let _ = owner.update(cx, |this, cx| {
                            this.wiring_card_action(&target, event, window, cx)
                        });
                    },
                )
            }
            Overlay::Update => {
                let bounds = self.wiring.update_bounds.get();
                let position = bounds.map_or(
                    point(window.viewport_size().width - px(8.), px(48.)),
                    |bounds| bounds.bottom_right() + point(px(0.), px(6.)),
                );
                Some(
                    anchored()
                        .anchor(Corner::TopRight)
                        .position(position)
                        .snap_to_window()
                        .child(render_update_popover(
                            &self.runtime_update_view(),
                            dark,
                            bounds,
                            focus,
                            self.update_handler(cx),
                        ))
                        .into_any_element(),
                )
            }
            Overlay::Accounts => Some(self.render_account_panel(window, cx)),
            Overlay::Egress => {
                let bounds = self.egress_anchor(window, cx).get();
                let position = bounds.map_or(
                    point(
                        window.viewport_size().width - px(8.),
                        window.viewport_size().height - px(32.),
                    ),
                    |bounds| bounds.top_right() - point(px(0.), px(6.)),
                );
                let mut view = self.wiring.egress.view(
                    Some(&self.wiring.egress_status),
                    self.locale,
                    self.motion_clock.elapsed(),
                );
                if let Some(view) = &mut view {
                    if let Some(error) = &self.wiring.egress_error {
                        view.checked = error.clone();
                    } else if self.wiring.egress_status.state == EgressState::Unknown
                        && self.wiring.egress_request.is_none()
                    {
                        view.checked = self
                            .locale
                            .text("暂无出口检测结果", "No exit check results")
                            .into();
                    }
                }
                Some(
                    anchored()
                        .anchor(Corner::BottomRight)
                        .position(position)
                        .snap_to_window()
                        .child(render_egress_panel(
                            view.as_ref(),
                            dark,
                            window,
                            bounds,
                            focus,
                            self.egress_handler(None, cx),
                        ))
                        .into_any_element(),
                )
            }
        }
    }

    fn detach_card(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(hierarchy) = &self.hierarchy else {
            return;
        };
        let Some(scene) = &hierarchy.scene else {
            return;
        };
        let Some(workspace_id) = hierarchy
            .workspace
            .as_ref()
            .map(|workspace| workspace.id.clone())
        else {
            return;
        };
        let Some(node) = self.graph_node(id) else {
            return;
        };
        let Some(terminal) = self.terminals.get(id).cloned() else {
            return;
        };
        let title = node["title"].as_str().unwrap_or_default().to_owned();
        let detached_node = node.clone();
        let mount = DetachedMount {
            main_window_id: hierarchy.navigation.window_id.clone(),
            scene_id: scene.id.clone(),
            mount_id: node["mountId"].as_str().unwrap_or(id).into(),
            session_id: id.into(),
        };
        self.wiring.next_child += 1;
        let child_id = format!(
            "{}:detached:{}",
            mount.main_window_id, self.wiring.next_child
        );
        match self.wiring.detached.detach(mount, child_id.clone()) {
            DetachOutcome::Open(_) => {}
            DetachOutcome::FocusExisting(_) => return self.focus_detached_card(id, cx),
            DetachOutcome::WindowIdInUse => return,
        }

        let owner = cx.weak_entity();
        let main = window.window_handle();
        let visible = self.wiring.visible;
        let presentation = self.detached_presentation(title.clone());
        let session_id = id.to_owned();
        let display_id = window.display(cx).map(|display| display.id());
        let bounds = Bounds::centered(display_id, size(px(770.), px(500.)), cx);
        let opened = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                display_id,
                window_min_size: Some(size(px(360.), px(240.))),
                show: visible,
                focus: visible,
                titlebar: Some(TitlebarOptions {
                    title: Some(title.clone().into()),
                    appears_transparent: cfg!(target_os = "macos"),
                    traffic_light_position: cfg!(target_os = "macos")
                        .then_some(point(px(12.), px(13.))),
                }),
                ..Default::default()
            },
            |window, cx| {
                cx.new(|cx| {
                    let owner_updates = owner.upgrade().map(|owner| {
                        cx.observe_in(
                            &owner,
                            window,
                            |child: &mut DetachedCardView, owner, window, cx| {
                                let owner = owner.read(cx);
                                if let Some(card) = owner.wiring.children.get(&child.session_id) {
                                    if child.presentation.title != card.title {
                                        window.set_window_title(&card.title);
                                    }
                                    child.presentation =
                                        owner.detached_presentation(card.title.clone());
                                    cx.notify();
                                }
                            },
                        )
                    });
                    let child_window = window.window_handle();
                    let shortcut_owner = owner.clone();
                    let keyboard = cx.intercept_keystrokes(move |event, window, cx| {
                        if window.window_handle() != child_window {
                            return;
                        }
                        if is_child_window_close_shortcut(&event.keystroke) {
                            cx.stop_propagation();
                            window.remove_window();
                        } else if shortcut_owner
                            .update(cx, |owner, cx| {
                                owner.detached_hud_key(&event.keystroke, window, cx)
                            })
                            .unwrap_or(false)
                        {
                            cx.stop_propagation();
                        } else if Workspace::is_theme_shortcut(&event.keystroke) {
                            cx.stop_propagation();
                            let _ = shortcut_owner.update(cx, Workspace::toggle_theme_shortcut);
                        } else if let Some(zoom) = terminal_surface::font_zoom_for_key(
                            &event.keystroke,
                            cfg!(target_os = "macos"),
                        ) {
                            cx.stop_propagation();
                            let _ = shortcut_owner
                                .update(cx, |owner, cx| owner.zoom_terminal_font(zoom, cx));
                        }
                    });
                    window.focus(&terminal.read(cx).focus);
                    DetachedCardView {
                        owner,
                        main,
                        session_id,
                        terminal,
                        presentation,
                        hud_ready: false,
                        _owner_updates: owner_updates,
                        _keyboard: keyboard,
                    }
                })
            },
        );
        match opened {
            Ok(child) => {
                if self.terminals[id].read(cx).focus.is_focused(window) {
                    window.focus(&self.card_controls[id]);
                }
                self.wiring.children.insert(
                    id.into(),
                    DetachedCard {
                        window: child,
                        title,
                        workspace_id,
                        node: detached_node,
                    },
                );
                let _ = child.update(cx, |view, _, cx| {
                    view.hud_ready = true;
                    cx.notify();
                });
                if self
                    .history_request
                    .as_ref()
                    .is_some_and(|request| request.session_id == id)
                {
                    self.dismiss_history_context(cx);
                    self.sync_terminal_input(window, cx);
                }
            }
            Err(error) => {
                self.wiring.detached.window_closed(&child_id);
                eprintln!("Open detached terminal: {error:#}");
                self.show_toast(
                    self.locale
                        .text("无法打开独立窗口", "Could not open detached window"),
                    cx,
                );
            }
        }
        self.refresh_live_wiring(cx);
        cx.notify();
    }

    pub(super) fn hud_node(&self, id: &str) -> Option<&Value> {
        self.graph_node(id)
            .or_else(|| self.wiring.children.get(id).map(|child| &child.node))
    }

    pub(super) fn detached_session_for_window(&self, window: AnyWindowHandle) -> Option<&str> {
        self.wiring
            .children
            .iter()
            .find(|(_, child)| AnyWindowHandle::from(child.window) == window)
            .map(|(id, _)| id.as_str())
    }

    pub(super) fn wiring_visible(&self) -> bool {
        self.wiring.visible
    }

    pub(super) fn dismiss_hud_wiring_overlay(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_wiring_overlay(window, cx);
    }

    fn detached_hud_key(
        &mut self,
        key: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.project_instructions_open(window) {
            return self.project_instructions_key(key, window, cx);
        }
        if self.footer_popover_key(key, window, cx) {
            return true;
        }
        if self.wiring.overlay_window == Some(window.window_handle())
            && self.wiring_key(key, window, cx)
        {
            return true;
        }
        let confirming = self
            .detached_permission_confirmations
            .contains_key(&window.window_handle());
        let menu_open = self.permission_menu_open(window);
        if key.key == "escape" {
            if confirming {
                self.close_detached_permission(window, cx);
            } else if menu_open {
                self.dismiss_permission_menu(window, cx);
            }
            return confirming || menu_open;
        }
        if (confirming || menu_open) && key.key == "tab" && !key.modifiers.modified() {
            if key.modifiers.shift {
                window.focus_prev();
            } else {
                window.focus_next();
            }
            return true;
        }
        false
    }

    fn clear_detached_hud(&mut self, window: AnyWindowHandle) {
        self.footer_popovers.remove(&window);
        self.detached_permission_menus.remove(&window);
        self.detached_permission_confirmations.remove(&window);
        self.project_instructions.remove(&window);
        if self.wiring.overlay_window == Some(window) {
            self.reset_wiring_overlay();
        }
    }

    pub(super) fn detached_path_invalid(&self, id: &str) -> Option<bool> {
        self.wiring
            .children
            .get(id)
            .map(|card| self.workspace_invalid(&card.workspace_id))
    }

    fn detached_presentation(&self, title: String) -> DetachedPresentation {
        DetachedPresentation {
            title,
            locale: self.locale,
            light: !self.wiring_dark(),
            status: if self.read_only.is_some() {
                DetachedHeaderStatus::ReadOnly
            } else {
                DetachedHeaderStatus::Running
            },
            bg: self.theme_palette().workspace.canvas,
            footer: self.theme_palette().footer,
        }
    }

    fn focus_detached_card(&self, id: &str, cx: &mut Context<Self>) {
        let Some((hierarchy, child)) = self.hierarchy.as_ref().zip(self.wiring.children.get(id))
        else {
            return;
        };
        if self
            .wiring
            .detached
            .focus_target(&hierarchy.navigation.window_id, id)
            .is_none()
        {
            return;
        }
        let _ = child.window.update(cx, |view, window, cx| {
            view.terminal.read(cx).focus.focus(window);
            if self.wiring.visible {
                window.activate_window();
            }
        });
    }

    pub(super) fn discard_wiring_session(&mut self, id: &str, cx: &mut Context<Self>) {
        if self
            .wiring
            .terminal_return
            .as_ref()
            .is_some_and(|(_, target)| target == id)
        {
            self.wiring.terminal_return = None;
        }
        self.wiring
            .splits
            .retain(|_, tree| !tree.close_session(id) || tree.ordered_sessions().len() > 1);
        self.wiring.detached.discard_session(id);
        if let Some(child) = self.wiring.children.remove(id) {
            self.clear_detached_hud(child.window.into());
            let _ = child
                .window
                .update(cx, |_, window, _| window.remove_window());
        }
        self.wiring.banners.remove(id);
        for snapshot in self.wiring.board_scenes.values_mut() {
            snapshot.sessions.retain(|session| session.id != id);
        }
    }

    fn reap_detached(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let windows = cx.windows();
        let closed: Vec<_> = self
            .wiring
            .children
            .iter()
            .filter(|(_, child)| {
                !windows
                    .iter()
                    .any(|handle| handle.window_id() == child.window.window_id())
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in closed {
            self.finish_detached(&id, false, window, cx);
        }
    }

    fn return_detached_card(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_detached(id, true, window, cx);
    }

    fn finish_detached(
        &mut self,
        id: &str,
        explicit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(detached) = self.wiring.detached.for_session(id) else {
            return;
        };
        let child_id = detached.window_id.clone();
        let scene_id = detached.mount.scene_id.clone();
        if explicit {
            self.wiring.detached.return_to_main(&child_id);
        } else {
            self.wiring.detached.window_closed(&child_id);
        }
        if let Some(child) = self.wiring.children.remove(id) {
            self.clear_detached_hud(child.window.into());
            if explicit {
                let _ = child
                    .window
                    .update(cx, |_, window, _| window.remove_window());
            }
        }
        if self.wiring_available() && self.read_only.is_none() {
            if self.wiring.visible {
                window.activate_window();
            }
            let active = self.active_scene_id() == Some(scene_id.as_str());
            if !active || self.active_level_focus() != Some(id) {
                if active && let Some(node) = self.graph_node(id) {
                    let parent = session_levels::parent_id(node).map(str::to_owned);
                    self.level_parent_by_scene.insert(scene_id.clone(), parent);
                } else if !active {
                    // Resolve the returned card's level from the incoming scene
                    // projection instead of reusing that scene's old breadcrumb.
                    self.level_parent_by_scene.remove(&scene_id);
                }
                self.send_card_focus(&scene_id, id, cx);
                if active {
                    self.refresh_level(cx);
                }
            }
            self.return_terminal_focus(scene_id, id.to_owned(), window, cx);
        }
        self.sync_terminal_input(window, cx);
        cx.notify();
    }

    pub(super) fn render_detached_placeholder(
        &self,
        id: &str,
        title: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let owner = cx.weak_entity();
        let target = id.to_owned();
        let menu_target = target.clone();
        let body = detached_placeholder(
            title.to_owned(),
            self.locale,
            !self.wiring_dark(),
            move |action, window, cx| {
                let _ = owner.update(cx, |this, cx| match action {
                    DetachedWindowAction::Focus => this.focus_detached_card(&target, cx),
                    DetachedWindowAction::ReturnToMain => {
                        this.return_detached_card(&target, window, cx)
                    }
                });
            },
        );
        div()
            .id(SharedString::from(format!("detached-mount-{id}")))
            .size_full()
            .relative()
            .when_some(self.card_controls.get(id), |card, focus| {
                card.track_focus(focus)
            })
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.open_wiring_card_menu(&menu_target, event.position, window, cx);
                    cx.stop_propagation();
                }),
            )
            .child(body)
            .children(wiring_probe_bounds(format!("detached-mount-{id}")))
            .into_any_element()
    }

    pub(super) fn update_trigger(&self, cx: &Context<Self>) -> impl IntoElement {
        div()
            .relative()
            .flex_none()
            .capture_any_mouse_down(preserve_overlay_mouse_focus)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(render_update_trigger(
                &self.runtime_update_view(),
                self.wiring_dark(),
                self.update_handler(cx),
            ))
            .child(measure_trigger(self.wiring.update_bounds.clone()))
    }

    fn update_handler(
        &self,
        cx: &Context<Self>,
    ) -> Rc<dyn Fn(UpdateAction, &mut Window, &mut App)> {
        let owner = cx.weak_entity();
        Rc::new(move |action, window, cx| {
            let _ = owner.update(cx, |this, cx| this.update_action(action, window, cx));
        })
    }

    fn update_action(&mut self, action: UpdateAction, window: &mut Window, cx: &mut Context<Self>) {
        if !self.wiring_available()
            || (self.wiring.overlay != Some(Overlay::Update) && action == UpdateAction::Dismiss)
        {
            return;
        }
        if action == UpdateAction::Dismiss
            || (action == UpdateAction::TogglePopover
                && self.wiring.overlay == Some(Overlay::Update))
        {
            self.dismiss_wiring_overlay(window, cx);
            return;
        }
        if action == UpdateAction::TogglePopover {
            self.open_wiring_overlay(Overlay::Update, window, cx);
            self.wiring.update.dispatch(action);
            if self.wiring.update_request.is_none() {
                self.request_update("updateStatusResult", UpdateErrorStage::Check, cx);
            }
            return;
        }
        match self.wiring.update.dispatch(action) {
            Some(UpdateEffect::Check) => {
                self.accept_update_status(
                    UpdateStatus::Checking {
                        retry_attempt: 0,
                        max_retry_attempts: None,
                    },
                    cx,
                );
                self.request_update("updateCheckResult", UpdateErrorStage::Check, cx);
            }
            Some(UpdateEffect::Download) => {
                if let Some(release) = self.wiring.update_release.clone() {
                    self.accept_update_status(
                        UpdateStatus::Downloading(
                            release,
                            UpdateProgress {
                                percent: 0.,
                                transferred_bytes: 0,
                                total_bytes: 0,
                                bytes_per_second: 0,
                                remaining_seconds: None,
                            },
                        ),
                        cx,
                    );
                }
                self.request_update("updateDownloadResult", UpdateErrorStage::Download, cx);
            }
            Some(UpdateEffect::Install) => {
                let message = self.locale.text(
                    "当前运行时尚未提供安装操作。",
                    "Installation is unavailable in the current runtime.",
                );
                self.update_failed(UpdateErrorStage::Install, message.into(), cx);
            }
            None => {}
        }
    }

    fn request_update(
        &mut self,
        kind: &'static str,
        stage: UpdateErrorStage,
        cx: &mut Context<Self>,
    ) {
        let request_id = self.next_wiring_request();
        let command = match kind {
            "updateCheckResult" => Command::CheckForUpdate { request_id },
            "updateDownloadResult" => Command::DownloadUpdate { request_id },
            _ => Command::GetUpdateStatus { request_id },
        };
        self.wiring.update_request = None;
        match self.commands.send_at(self.runtime_generation, command) {
            Ok(()) => self.wiring.update_request = Some((request_id, kind, stage)),
            Err(error) => self.update_failed(stage, self.wiring_send_error(&error), cx),
        }
    }

    fn runtime_update_view(&self) -> UpdateView {
        let mut view = self
            .wiring
            .update
            .view(self.locale, self.motion_clock.elapsed());
        if let Some(error) = &self.wiring.update_error {
            view.message = Some(error.clone());
        }
        view
    }

    fn update_failed(&mut self, stage: UpdateErrorStage, message: String, cx: &mut Context<Self>) {
        self.accept_update_status(
            UpdateStatus::Error {
                stage,
                message: message.clone(),
                manual_download_available: false,
            },
            cx,
        );
        self.wiring.update_error = Some(message);
    }

    pub fn accept_update_status(&mut self, status: UpdateStatus, cx: &mut Context<Self>) {
        self.accept_update_state(
            UpdateState {
                current_version: self.wiring.update.current_version().to_owned(),
                status,
            },
            cx,
        );
    }

    fn accept_update_state(&mut self, state: UpdateState, cx: &mut Context<Self>) {
        self.wiring.update_error = None;
        match &state.status {
            UpdateStatus::Available(release)
            | UpdateStatus::Downloading(release, _)
            | UpdateStatus::Downloaded(release) => {
                self.wiring.update_release = Some(release.clone())
            }
            UpdateStatus::Idle | UpdateStatus::NotAvailable => self.wiring.update_release = None,
            _ => {}
        }
        self.wiring
            .update
            .accept(state, self.motion_clock.elapsed());
        if self.wiring.overlay != Some(Overlay::Update) {
            self.wiring.update.dispatch(UpdateAction::Dismiss);
        }
        self.wiring.update.manual_install_finished();
        cx.notify();
    }

    pub fn set_egress_status(
        &mut self,
        status: EgressStatus,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let now = self.motion_clock.elapsed();
        for session in status
            .session_families
            .keys()
            .chain(status.session_family_sets.keys())
        {
            self.wiring.banners.entry(session.clone()).or_default();
        }
        self.wiring.banners.retain(|session, banner| {
            banner.sync(Some(&status), session, now);
            banner.view(self.locale, now).is_some()
        });
        self.wiring.egress_status = status;
        self.sync_terminal_input(window, cx);
        cx.notify();
    }

    pub(super) fn egress_frozen(&self, id: &str) -> bool {
        self.wiring.egress_status.families.iter().any(|family| {
            !family.overridden && family.frozen_session_ids.iter().any(|frozen| frozen == id)
        })
    }

    pub(super) fn refresh_accounts_after_provider_change(&mut self, cx: &mut Context<Self>) {
        if self.domain_ready && self.hierarchy.is_some() {
            self.wiring.account_request = None;
            self.refresh_accounts(cx);
        }
    }

    fn refresh_accounts(&mut self, cx: &mut Context<Self>) {
        if self.wiring.account_request.is_some() {
            return;
        }
        let request_id = self.next_wiring_request();
        if self
            .commands
            .send_at(
                self.runtime_generation,
                Command::GetAccountSummary { request_id },
            )
            .is_ok()
        {
            self.wiring.account_request = Some(request_id);
            cx.notify();
        }
    }

    fn account_footer_visible(&self, window: &Window) -> bool {
        self.footer_session(window)
            .and_then(|id| self.hud_node(id))
            .and_then(|node| node["currentMode"].as_str())
            .is_some_and(footer_shows_account)
    }

    fn dismiss_ineligible_account_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.wiring.overlay == Some(Overlay::Accounts)
            && self.wiring.overlay_window == Some(window.window_handle())
            && !self.account_footer_visible(window)
        {
            self.dismiss_wiring_overlay(window, cx);
        }
    }

    fn toggle_accounts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.wiring.overlay == Some(Overlay::Accounts)
            && self.wiring.overlay_window == Some(window.window_handle())
        {
            self.dismiss_wiring_overlay(window, cx);
        } else if self.hud_wiring_available(window) && self.account_footer_visible(window) {
            self.open_wiring_overlay(Overlay::Accounts, window, cx);
            let restore = if let Some(id) = self.detached_session_for_window(window.window_handle())
            {
                self.terminals
                    .get(id)
                    .map(|terminal| terminal.read(cx).focus.clone())
            } else {
                self.active_terminal_focus(cx)
            };
            if let Some(focus) = restore {
                self.wiring.restore_focus = Some(focus);
            }
            self.refresh_accounts(cx);
        }
    }

    fn accounts(&self) -> &[Value] {
        self.wiring
            .account_summary
            .as_ref()
            .and_then(|summary| summary["accounts"].as_array())
            .map_or(&[], Vec::as_slice)
    }

    fn current_account(&self) -> Option<&Value> {
        let id = self.wiring.account_summary.as_ref()?["currentId"].as_str()?;
        self.accounts().iter().find(|account| account["id"] == id)
    }

    pub(super) fn footer_account_trigger(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let palette = self.theme_palette().footer;
        let bounds = self.account_anchor(window, cx);
        let account = self.current_account();
        let locale = self.locale;
        let label = account.map(account_label).unwrap_or_else(|| {
            self.locale
                .text(FOOTER_ACCOUNT_LABEL, "Claude accounts")
                .into()
        });
        self.button(
            div()
                .id("footer-account")
                .relative()
                .flex_none()
                .ml(px(8.))
                .flex()
                .items_center()
                .gap(px(7.))
                .px(px(8.))
                .py(px(5.))
                .rounded(px(5.))
                .text_color(rgba(palette.account))
                .whitespace_nowrap()
                .cursor_pointer()
                .hover(|style| style.bg(rgba(0x518ef019)))
                .tooltip(move |_, cx| {
                    cx.new(|_| {
                        CardTooltip(
                            locale
                                .text("本机默认 Claude 账户", "Device default Claude account")
                                .into(),
                        )
                    })
                    .into()
                })
                .child(footer_user_icon(14., rgba(palette.account)))
                .child(
                    div()
                        .max_w(px(116.))
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(footer_text("account", label)),
                )
                .children(account.map(account_dot))
                .child(ui_text("⌃"))
                .children(footer_bounds_probe("account"))
                .children(wiring_probe_bounds("footer-account".into()))
                .child(measure_trigger(bounds)),
            &self.control_focus["accounts"],
            Self::toggle_accounts,
            cx,
        )
    }

    fn account_anchor(&self, window: &mut Window, cx: &mut Context<Self>) -> TriggerBounds {
        window
            .use_keyed_state("footer-account-anchor", cx, |_, _| TriggerBounds::default())
            .read(cx)
            .clone()
    }

    fn open_account_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.detached_session_for_window(window.window_handle())
            && let Some(child) = self.wiring.children.get(id)
            && let Ok(view) = child.window.read(cx)
        {
            let main = view.main;
            let owner = cx.weak_entity();
            self.dismiss_wiring_overlay(window, cx);
            cx.defer(move |cx| {
                let _ = main.update(cx, |_, window, cx| {
                    let _ = owner.update(cx, |this, cx| {
                        if this.settings.is_none() {
                            this.toggle_settings(window, cx);
                        }
                        if this.wiring.visible {
                            window.activate_window();
                        }
                    });
                });
            });
        } else {
            self.toggle_settings(window, cx);
        }
    }

    fn render_account_panel(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let palette = self.theme_palette();
        let locale = self.locale;
        let width = (window.viewport_size().width - px(24.))
            .max(px(0.))
            .min(px(410.));
        let position = self
            .account_anchor(window, cx)
            .get()
            .map(|bounds| bounds.top_right() - point(px(0.), px(6.)))
            .unwrap_or_else(|| {
                point(
                    window.viewport_size().width - px(12.),
                    window.viewport_size().height - px(46.),
                )
            });
        let owner = cx.weak_entity();
        let close = owner.clone();
        let manage = owner.clone();
        let active = self.current_account();
        let rows = div()
            .id("footer-account-list")
            .flex()
            .flex_col()
            .max_h((window.viewport_size().height - px(160.)).max(px(44.)))
            .overflow_y_scroll()
            .px(px(18.))
            .when(self.accounts().is_empty(), |rows| {
                rows.child(
                    div()
                        .py(px(14.))
                        .text_color(rgba(palette.workspace.muted))
                        .child(if self.wiring.account_request.is_some() {
                            locale.text("正在读取账户…", "Loading accounts…")
                        } else {
                            locale.text(
                                "暂无 Claude 账户，请前往设置管理。",
                                "No Claude accounts. Manage accounts in Settings.",
                            )
                        }),
                )
            })
            .children(self.accounts().iter().map(|account| {
                let status = match account["status"].as_str() {
                    Some("active") => locale.text("使用中", "Active"),
                    Some("expired") => locale.text("登录已失效", "Expired"),
                    Some("rate-limited") => locale.text("限流", "Rate limited"),
                    _ => locale.text("待启用", "Needs activation"),
                };
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .py(px(9.))
                    .border_t_1()
                    .border_color(rgba(palette.workspace.line))
                    .when(active == Some(account), |row| {
                        row.font_weight(FontWeight::SEMIBOLD)
                    })
                    .child(account_dot(account))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(account_label(account)),
                    )
                    .children(trimmed_graph_text(&account["plan"]).map(str::to_owned))
                    .children((active == Some(account)).then(|| {
                        div()
                            .text_color(rgba(palette.workspace.accent))
                            .child(locale.text("本机默认", "Default"))
                    }))
                    .child(
                        div()
                            .text_color(rgba(palette.workspace.muted))
                            .child(status),
                    )
            }));
        let panel = div()
            .id("footer-account-panel")
            .track_focus(&self.wiring.overlay_focus)
            .w(width)
            .flex()
            .flex_col()
            .rounded(px(12.))
            .overflow_hidden()
            .bg(rgba(palette.workspace.panel))
            .text_color(rgba(palette.workspace.text))
            .text_size(px(12.))
            .border_1()
            .border_color(rgba(palette.workspace.line))
            .shadow(vec![shadow(0x263d61, 0.15, 16., 42.)])
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .px(px(18.))
                    .py(px(15.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(locale.text(FOOTER_ACCOUNT_LABEL, "Claude accounts")),
                    )
                    .child(
                        div()
                            .id("footer-accounts-close")
                            .cursor_pointer()
                            .child("×")
                            .on_click(move |_, window, cx| {
                                let _ = close
                                    .update(cx, |this, cx| this.dismiss_wiring_overlay(window, cx));
                            }),
                    ),
            )
            .child(rows)
            .child(
                div()
                    .id("footer-manage-accounts")
                    .px(px(18.))
                    .py(px(12.))
                    .border_t_1()
                    .border_color(rgba(palette.workspace.line))
                    .text_color(rgba(palette.workspace.accent))
                    .cursor_pointer()
                    .focusable()
                    .tab_index(0)
                    .child(locale.text("前往设置管理", "Manage in Settings"))
                    .on_click(move |_, window, cx| {
                        let _ =
                            manage.update(cx, |this, cx| this.open_account_settings(window, cx));
                    }),
            );
        div()
            .absolute()
            .inset_0()
            .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                let _ = owner.update(cx, |this, cx| this.dismiss_wiring_overlay(window, cx));
                cx.stop_propagation();
            })
            .child(
                anchored()
                    .anchor(Corner::BottomRight)
                    .position(position)
                    .snap_to_window()
                    .child(panel),
            )
            .into_any_element()
    }

    fn egress_anchor(&self, window: &mut Window, cx: &mut Context<Self>) -> TriggerBounds {
        window
            .use_keyed_state("footer-egress-anchor", cx, |_, _| TriggerBounds::default())
            .read(cx)
            .clone()
    }

    pub(super) fn egress_trigger_for(
        &self,
        session: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let bounds = self.egress_anchor(window, cx);
        let mut view = self.wiring.egress.chip(
            true,
            Some(&self.wiring.egress_status),
            session.unwrap_or_default(),
            self.locale,
        );
        if let Some(view) = &mut view
            && self.wiring.egress_status.state == EgressState::Unknown
        {
            view.label = self.locale.text("出口", "Exit").into();
            view.tone = matou_native_desktop::egress_panel::EgressTone::Neutral;
        }
        let label = view
            .as_ref()
            .map(|view| footer_text("egress", view.label.clone()));
        div()
            .id("footer-egress")
            .flex_none()
            .relative()
            .capture_any_mouse_down(preserve_overlay_mouse_focus)
            .children(footer_bounds_probe("egress"))
            .child(render_egress_chip_with_label(
                view.as_ref(),
                self.wiring_dark(),
                label,
                self.egress_handler(None, cx),
            ))
            .child(measure_trigger(bounds))
            .children(self.egress_probe())
    }

    fn egress_probe(&self) -> Option<Canvas<()>> {
        #[cfg(debug_assertions)]
        {
            return Some(measure_trigger(self.wiring.egress_probe_bounds.clone()));
        }
        #[cfg(not(debug_assertions))]
        None
    }

    pub(super) fn render_card_egress(&self, id: &str, cx: &Context<Self>) -> Option<AnyElement> {
        let view = self
            .wiring
            .banners
            .get(id)?
            .view(self.locale, self.motion_clock.elapsed())?;
        Some(
            div()
                .relative()
                .flex_none()
                .child(render_egress_banner(
                    Some(&view),
                    self.wiring_dark(),
                    &self.wiring.confirmation_focus,
                    self.egress_handler(Some(id.to_owned()), cx),
                ))
                .children(wiring_probe_bounds(format!("egress-banner-{id}")))
                .into_any_element(),
        )
    }

    fn egress_handler(
        &self,
        id: Option<String>,
        cx: &Context<Self>,
    ) -> Rc<dyn Fn(EgressAction, &mut Window, &mut App)> {
        let owner = cx.weak_entity();
        Rc::new(move |action, window, cx| {
            let _ = owner.update(cx, |this, cx| {
                this.egress_action(id.as_deref(), action, window, cx)
            });
        })
    }

    fn egress_action(
        &mut self,
        id: Option<&str>,
        action: EgressAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = id {
            let Some(banner) = self.wiring.banners.get_mut(id) else {
                return;
            };
            let confirms = matches!(action, EgressAction::RequestOverride(_));
            let effect = banner.dispatch(action);
            if confirms {
                window.focus(&self.wiring.confirmation_focus);
            } else if self.wiring.confirmation_focus.is_focused(window) {
                if let Some(focus) = self.card_controls.get(id) {
                    window.focus(focus);
                }
            }
            if let Some(effect) = effect {
                match effect {
                    EgressAction::Recheck => self.request_egress(EgressRequest::Recheck, cx),
                    EgressAction::ConfirmOverride(family) => {
                        self.request_egress(EgressRequest::Exempt(family), cx)
                    }
                    _ => {}
                }
            }
            cx.notify();
            return;
        }
        match action {
            EgressAction::TogglePanel
                if self.wiring.overlay == Some(Overlay::Egress)
                    && self.wiring.overlay_window == Some(window.window_handle()) =>
            {
                self.dismiss_wiring_overlay(window, cx)
            }
            EgressAction::TogglePanel if self.hud_wiring_available(window) => {
                self.open_wiring_overlay(Overlay::Egress, window, cx);
                self.wiring.egress.open = true;
                self.request_egress(EgressRequest::Status, cx);
            }
            EgressAction::DismissPanel if self.wiring.overlay == Some(Overlay::Egress) => {
                self.dismiss_wiring_overlay(window, cx)
            }
            EgressAction::CopyReport(report) => {
                cx.write_to_clipboard(ClipboardItem::new_string(report));
                self.wiring
                    .egress
                    .copy_succeeded(self.motion_clock.elapsed());
                cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(matou_native_desktop::egress_panel::EGRESS_COPIED_DURATION)
                        .await;
                    let _ = this.update(cx, |_, cx| cx.notify());
                })
                .detach();
            }
            EgressAction::OpenReference(reference) => {
                self.open_external_link(reference.url(), LinkSource::External, window, cx);
            }
            EgressAction::Recheck => self.request_egress(EgressRequest::Recheck, cx),
            EgressAction::SetRule(rule) if rule != self.wiring.egress_status.rule_mode => {
                self.request_egress(EgressRequest::SetRule(rule), cx)
            }
            EgressAction::SetProtection(enabled) => {
                self.request_egress(EgressRequest::SetProtection(enabled), cx)
            }
            EgressAction::AcceptCurrent(family) => {
                self.request_egress(EgressRequest::AcceptBaseline(family), cx)
            }
            _ => {}
        }
        cx.notify();
    }

    pub(super) fn next_wiring_request(&mut self) -> u64 {
        self.wiring.request_sequence += 1;
        self.wiring.request_sequence
    }

    fn wiring_send_error(&self, error: &crate::runtime_bridge::SendError) -> String {
        if matches!(error, crate::runtime_bridge::SendError::StorageReadOnly) {
            self.locale
                .text(
                    STORAGE_READ_ONLY_TITLE,
                    "The database is in read-only recovery mode",
                )
                .into()
        } else {
            error.to_string()
        }
    }

    fn wiring_reply_error(&self, reply: &Value) -> Option<String> {
        reply["error"].as_str().map(|error| {
            if reply["code"] == "STORAGE_READ_ONLY" {
                self.locale
                    .text(
                        STORAGE_READ_ONLY_TITLE,
                        "The database is in read-only recovery mode",
                    )
                    .into()
            } else {
                error.to_owned()
            }
        })
    }

    fn request_egress(&mut self, action: EgressRequest, cx: &mut Context<Self>) {
        if self.wiring.egress_request.is_some_and(|(_, kind)| {
            matches!(action, EgressRequest::Status) || kind != "egressStatusResult"
        }) {
            return;
        }
        let request_id = self.next_wiring_request();
        let (kind, command) = match action {
            EgressRequest::Status => (
                "egressStatusResult",
                Command::GetEgressStatus { request_id },
            ),
            EgressRequest::Recheck => {
                ("egressRecheckResult", Command::RecheckEgress { request_id })
            }
            EgressRequest::Exempt(family) => (
                "egressExemptionResult",
                Command::SetEgressExemption {
                    request_id,
                    family: runtime_family(family),
                },
            ),
            EgressRequest::SetRule(rule) => (
                "egressRuleModeResult",
                Command::SetEgressRuleMode {
                    request_id,
                    mode: match rule {
                        EgressRule::Country => RuleMode::Country,
                        EgressRule::Ip => RuleMode::Ip,
                    },
                },
            ),
            EgressRequest::SetProtection(enabled) => (
                "egressProtectionEnabledResult",
                Command::SetEgressProtectionEnabled {
                    request_id,
                    enabled,
                },
            ),
            EgressRequest::AcceptBaseline(family) => (
                "egressBaselineResult",
                Command::AcceptEgressBaseline {
                    request_id,
                    family: runtime_family(family),
                },
            ),
        };
        self.wiring.egress_request = None;
        self.wiring.egress_error = None;
        match self.commands.send_at(self.runtime_generation, command) {
            Ok(()) => {
                self.wiring.egress_request = Some((request_id, kind));
                self.wiring.egress.rechecking = !matches!(action, EgressRequest::Status);
            }
            Err(error) => self.finish_egress_request(Some(self.wiring_send_error(&error)), cx),
        }
    }

    fn finish_egress_request(&mut self, error: Option<String>, cx: &mut Context<Self>) {
        self.wiring.egress_request = None;
        self.wiring.egress.rechecking = false;
        for banner in self.wiring.banners.values_mut() {
            banner.recheck_finished();
            banner.override_finished();
        }
        if let Some(error) = &error {
            self.show_toast(error.clone(), cx);
        }
        self.wiring.egress_error = error;
        cx.notify();
    }

    pub(super) fn retire_wiring_commands(&mut self, cx: &mut Context<Self>) {
        if let Some((request_id, request)) = self.wiring.board_moves.pop() {
            self.wiring.board_moves.clear();
            self.show_board_feedback(
                request_id,
                request.workspace_id,
                BoardFeedback::SaveFailed,
                cx,
            );
        }
        self.wiring.egress_request = None;
        self.wiring.egress_refresh_scheduled = false;
        self.wiring.egress.rechecking = false;
        for banner in self.wiring.banners.values_mut() {
            banner.recheck_finished();
            banner.override_finished();
        }
        if let Some((_, _, stage)) = self.wiring.update_request.take() {
            self.wiring.update.accept(
                UpdateState {
                    current_version: self.wiring.update.current_version().to_owned(),
                    status: UpdateStatus::Error {
                        stage,
                        message: crate::runtime_bridge::SendError::NotReady.to_string(),
                        manual_download_available: false,
                    },
                },
                self.motion_clock.elapsed(),
            );
        }
        self.wiring.account_request = None;
        self.wiring.account_summary = None;
        self.wiring.hud.clear();
    }

    fn refresh_frozen_egress(&mut self, cx: &mut Context<Self>) {
        if self.wiring.egress_refresh_scheduled
            || !self
                .wiring
                .egress_status
                .families
                .iter()
                .any(|family| !family.overridden && !family.frozen_session_ids.is_empty())
        {
            return;
        }
        self.wiring.egress_refresh_scheduled = true;
        let generation = self.runtime_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            let _ =
                this.update(cx, |this, cx| {
                    if this.runtime_generation == generation && this.wiring.egress_refresh_scheduled
                    {
                        this.wiring.egress_refresh_scheduled = false;
                        if this.wiring.egress_status.families.iter().any(|family| {
                            !family.overridden && !family.frozen_session_ids.is_empty()
                        }) {
                            this.refresh_egress_status(cx);
                        }
                    }
                });
        })
        .detach();
    }

    pub(super) fn refresh_egress_status(&mut self, cx: &mut Context<Self>) {
        self.request_egress(EgressRequest::Status, cx);
    }

    pub(super) fn refresh_live_wiring(&mut self, cx: &mut Context<Self>) {
        let (generation, phase) = self.commands.connection();
        if generation != self.runtime_generation
            || phase != ConnectionPhase::Ready
            || !self.domain_ready
        {
            return;
        }
        let mut targets = Vec::new();
        if let Some(id) = self.level_focus().and_then(|id| {
            let node = self.graph_node(id)?;
            if Self::team_member_node(node) {
                session_levels::parent_id(node)
            } else {
                agent_hud_mode(node).map(|_| id)
            }
        }) {
            targets.push(id.to_owned());
        }
        targets.extend(
            self.wiring
                .children
                .iter()
                .filter(|(id, _)| !targets.contains(id))
                .filter_map(|(id, child)| agent_hud_mode(&child.node).map(|_| id.clone()))
                .collect::<Vec<_>>(),
        );
        self.wiring.hud.retain(|id, _| targets.contains(id));
        if self.wiring.account_summary.is_none()
            && targets.iter().any(|id| {
                self.hud_node(id)
                    .is_some_and(|node| node["currentMode"] == "claude-code")
            })
        {
            self.refresh_accounts(cx);
        }
        let now = self.motion_clock.elapsed();
        for session_id in targets {
            let mode = self.hud_node(&session_id).and_then(agent_hud_mode);
            let state = self.wiring.hud.entry(session_id.clone()).or_default();
            if state.mode != mode {
                *state = LiveHud {
                    mode,
                    ..LiveHud::default()
                };
            }
            if state.request.is_some() || state.refresh_at.is_some_and(|at| now < at) {
                continue;
            }
            let request_id = self.next_wiring_request();
            if self
                .commands
                .send_at(
                    self.runtime_generation,
                    Command::GetAgentHud {
                        request_id,
                        session_id: session_id.clone(),
                    },
                )
                .is_ok()
            {
                self.wiring.hud.entry(session_id).or_default().request = Some(request_id);
            }
        }
    }

    pub(super) fn live_agent_hud(&self, id: &str) -> Option<&Value> {
        self.wiring
            .hud
            .get(id)
            .filter(|state| state.mode == self.hud_node(id).and_then(agent_hud_mode))
            .map(|state| &state.result["hud"])
            .filter(|hud| hud.is_object())
    }

    pub(super) fn confirm_live_hud_permission(&mut self, id: &str, mode: &str) {
        if let Some(state) = self.wiring.hud.get_mut(id) {
            if state.result["hud"].is_object() {
                state.result["hud"]["permissionMode"] = mode.into();
            }
            // A poll sent before the confirmation can still carry the old mode.
            state.request = None;
            state.refresh_at = None;
        }
        if let Some(child) = self.wiring.children.get_mut(id) {
            child.node["permissionMode"] = mode.into();
        }
    }

    pub(super) fn receive_wiring_reply(
        &mut self,
        kind: &str,
        request_id: u64,
        reply: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if kind == "taskStatusResult"
            && let Some(index) = self
                .wiring
                .board_moves
                .iter()
                .position(|(id, _)| *id == request_id)
        {
            let (_, request) = self.wiring.board_moves.remove(index);
            let feedback = if reply["result"].is_array() {
                BoardFeedback::Moved(request.status)
            } else {
                BoardFeedback::SaveFailed
            };
            self.show_board_feedback(request_id, request.workspace_id, feedback, cx);
            cx.notify();
        }
        if self.wiring.egress_request == Some((request_id, kind)) {
            let error = self.wiring_reply_error(reply);
            self.finish_egress_request(error.clone(), cx);
            if error.is_none()
                && let Some(status) = project_egress(&reply["result"])
            {
                self.set_egress_status(status, window, cx);
                self.refresh_frozen_egress(cx);
            }
        }
        if let Some((pending, expected, stage)) = self.wiring.update_request
            && pending == request_id
            && expected == kind
        {
            self.wiring.update_request = None;
            match self.wiring_reply_error(reply) {
                Some(error) => self.update_failed(stage, error, cx),
                None => match serde_json::from_value::<update_download::UpdateState>(
                    reply["result"].clone(),
                ) {
                    Ok(status) => self.accept_update_state(project_update(status), cx),
                    Err(error) => self.update_failed(stage, error.to_string(), cx),
                },
            }
        }
        if kind == "accountSummaryResult" && self.wiring.account_request == Some(request_id) {
            self.wiring.account_request = None;
            self.wiring.account_summary = Some(reply["result"]["accountSummary"].clone());
            cx.notify();
        }
        self.receive_footer_diagnostics(kind, request_id, reply, cx);
        if kind == "agentHudResult"
            && let Some((id, state)) = self
                .wiring
                .hud
                .iter_mut()
                .find(|(_, state)| state.request == Some(request_id))
        {
            state.request = None;
            state.result = reply["result"].clone();
            let target = id.clone();
            let interval = Duration::from_secs(2);
            let refresh_at = self.motion_clock.elapsed() + interval;
            state.refresh_at = Some(refresh_at);
            let generation = self.runtime_generation;
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(interval).await;
                let _ = this.update(cx, |this, cx| {
                    if this.runtime_generation == generation
                        && let Some(state) = this.wiring.hud.get_mut(&target)
                        && state.refresh_at == Some(refresh_at)
                    {
                        state.refresh_at = None;
                        this.refresh_live_wiring(cx);
                    }
                });
            })
            .detach();
            cx.notify();
        }
    }

    pub(super) fn render_live_teammates(&self, id: &str) -> Option<Stateful<Div>> {
        let result = &self.wiring.hud.get(id)?.result;
        let teammates = result["teammates"]
            .as_array()
            .filter(|members| !members.is_empty())?;
        Some(
            div()
                .id(SharedString::from(format!("team-members-{id}")))
                .w_full()
                .flex_none()
                .flex()
                .gap(px(6.))
                .overflow_x_scroll()
                .children(teammates.iter().filter_map(|member| {
                    let member_id = member["teammateId"].as_str()?;
                    Some(
                        div()
                            .flex_none()
                            .w(px(280.))
                            .child(self.render_team_member(&format!("{id}-{member_id}"), member)),
                    )
                })),
        )
    }

    pub(super) fn render_team_member(&self, id: &str, node: &Value) -> AnyElement {
        let summary = node
            .get("teamMemberSummary")
            .filter(|summary| summary.is_object())
            .unwrap_or(node);
        let status = summary["workStatus"]
            .as_str()
            .and_then(TeamMemberWorkStatus::from_wire)
            .or_else(|| {
                node["workStatus"]
                    .as_str()
                    .and_then(TeamMemberWorkStatus::from_wire)
            })
            .unwrap_or(TeamMemberWorkStatus::Idle);
        let lines = summary["latestLines"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str);
        let title = summary["title"]
            .as_str()
            .or_else(|| summary["name"].as_str())
            .or_else(|| node["title"].as_str())
            .unwrap_or_default();
        team_member_card(
            SharedString::from(format!("team-member-{id}")),
            &TeamMemberCardModel::new(title, status, lines),
            self.locale,
            self.theme_palette().terminal,
            &TeamMemberActions::none(),
        )
        .relative()
        .children(wiring_probe_bounds(format!("team-member-{id}")))
        .into_any_element()
    }

    pub(super) fn team_member_node(node: &Value) -> bool {
        node["currentMode"] == "agent-team-member"
    }
}

fn preserve_overlay_mouse_focus(event: &MouseDownEvent, window: &mut Window, _: &mut App) {
    if event.button == MouseButton::Left {
        window.prevent_default();
    }
}

#[cfg(debug_assertions)]
thread_local! {
    static WIRING_PROBE: std::cell::RefCell<Option<HashMap<String, Bounds<Pixels>>>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(debug_assertions)]
pub(super) fn capture_wiring_bounds(draw: impl FnOnce()) -> HashMap<String, Bounds<Pixels>> {
    WIRING_PROBE.with_borrow_mut(|capture| *capture = Some(HashMap::new()));
    draw();
    WIRING_PROBE.with_borrow_mut(|capture| capture.take().unwrap_or_default())
}

pub(super) fn wiring_probe_bounds(_id: String) -> Option<Canvas<()>> {
    #[cfg(debug_assertions)]
    if WIRING_PROBE.with_borrow(Option::is_some) {
        return Some(
            canvas(
                |_, _, _| (),
                move |bounds, _, _, _| {
                    WIRING_PROBE.with_borrow_mut(|capture| {
                        if let Some(capture) = capture {
                            capture.insert(_id.clone(), bounds);
                        }
                    });
                },
            )
            .absolute()
            .inset_0()
            .size_full(),
        );
    }
    None
}

#[cfg(debug_assertions)]
impl Workspace {
    pub(super) fn hide_wiring_children_for_probe(&mut self) {
        self.wiring.visible = false;
    }

    pub(super) fn board_view_for_probe(&self) -> Option<BoardView> {
        self.board_view(&self.board_sessions())
    }

    pub(super) fn mock_external_links_for_probe(
        &mut self,
        open: impl Fn(
            &str,
            LinkOpenTarget,
            &mut App,
        ) -> gpui::Task<Result<(), ControlledBrowserFailure>>
        + 'static,
        copy: impl Fn(&str, &mut App) + 'static,
    ) {
        self.wiring.link_opener = Rc::new(open);
        self.wiring.copy_link = Rc::new(copy);
    }

    pub(super) fn link_in_progress_for_probe(&self) -> bool {
        self.wiring.link_in_progress
    }

    pub(super) fn wiring_child_for_probe(&self, id: &str) -> Option<AnyWindowHandle> {
        self.wiring
            .children
            .get(id)
            .map(|child| child.window.into())
    }

    pub(super) fn wiring_overlay_for_probe(&self) -> Option<&'static str> {
        self.wiring.overlay.map(|overlay| match overlay {
            Overlay::Shortcuts => "shortcuts",
            Overlay::CardMenu => "card menu",
            Overlay::Update => "update",
            Overlay::Egress => "egress",
            Overlay::Accounts => "accounts",
        })
    }

    pub(super) fn update_view_for_probe(&self) -> matou_native_desktop::update_popover::UpdateView {
        self.runtime_update_view()
    }

    pub(super) fn trigger_for_probe(&self, egress: bool) -> Bounds<Pixels> {
        if egress {
            self.wiring.egress_probe_bounds.get()
        } else {
            self.wiring.update_bounds.get()
        }
        .expect("trigger painted")
    }

    pub(super) fn egress_for_probe(&self) -> &EgressStatus {
        &self.wiring.egress_status
    }

    pub(super) fn egress_action_for_probe(
        &mut self,
        id: Option<&str>,
        action: EgressAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.egress_action(id, action, window, cx);
    }

    pub(super) fn request_update_action_for_probe(
        &mut self,
        action: UpdateAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_action(action, window, cx);
    }
}

fn runtime_family(family: EgressFamily) -> Family {
    match family {
        EgressFamily::Anthropic => Family::Anthropic,
        EgressFamily::OpenAi => Family::OpenAi,
        EgressFamily::ChatGptSubscription => Family::ChatgptSubscription,
        EgressFamily::Google => Family::Google,
    }
}

fn desktop_family(value: &Value) -> Option<EgressFamily> {
    EgressFamily::ALL
        .into_iter()
        .find(|family| value == runtime_family(*family).name())
}

fn project_egress(value: &Value) -> Option<EgressStatus> {
    fn state(value: &Value) -> EgressState {
        match value.as_str() {
            Some("checking") => EgressState::Checking,
            Some("ok") => EgressState::Ok,
            Some("warning") => EgressState::Warning,
            Some("abnormal") => EgressState::Abnormal,
            _ => EgressState::Unknown,
        }
    }
    fn exit(value: &Value) -> Option<EgressExit> {
        Some(EgressExit {
            ip: value["ip"].as_str()?.to_owned(),
            country: value["country"].as_str()?.to_owned(),
        })
    }
    let families = value["families"]
        .as_array()?
        .iter()
        .filter_map(|row| {
            Some(EgressFamilyStatus {
                family: desktop_family(&row["family"])?,
                state: state(&row["state"]),
                exit: exit(&row["exit"]),
                baseline: exit(&row["baseline"]),
                violation: match row["violation"].as_str() {
                    Some("mainland") => Some(EgressViolation::Mainland),
                    Some("rule") => Some(EgressViolation::Rule),
                    _ => None,
                },
                overridden: row["overridden"] == true,
                frozen_session_ids: row["frozenSessionIds"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
            })
        })
        .collect();
    let checks = value["checks"]
        .as_array()?
        .iter()
        .filter_map(|row| {
            Some(EgressCheck {
                kind: match row["id"].as_str()? {
                    "mainland" => EgressCheckKind::Mainland,
                    "exit-rule" => EgressCheckKind::ExitRule,
                    "login-api-match" => EgressCheckKind::LoginApiMatch,
                    "agent-proxy-path" => EgressCheckKind::AgentProxyPath,
                    "browser-path" => EgressCheckKind::BrowserPath,
                    "time-zone" => EgressCheckKind::TimeZone,
                    "ipv6" => EgressCheckKind::Ipv6,
                    _ => return None,
                },
                family: desktop_family(&row["family"]),
                result: match row["result"].as_str() {
                    Some("pass") => EgressCheckResult::Pass,
                    Some("warn") => EgressCheckResult::Warn,
                    Some("fail") => EgressCheckResult::Fail,
                    _ => EgressCheckResult::Unknown,
                },
                facts: row["facts"]
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter_map(|(key, value)| Some((key.clone(), value.as_str()?.to_owned())))
                    .collect(),
            })
        })
        .collect();
    let transition = &value["lastTransition"];
    Some(EgressStatus {
        state: state(&value["state"]),
        families,
        checks,
        rule_mode: if value["ruleMode"] == "ip" {
            EgressRule::Ip
        } else {
            EgressRule::Country
        },
        protection_enabled: value["protectionEnabled"] == true,
        session_families: HashMap::new(),
        session_family_sets: value["sessionFamilies"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(session, families)| {
                Some((
                    session.clone(),
                    families
                        .as_array()?
                        .iter()
                        .filter_map(desktop_family)
                        .collect(),
                ))
            })
            .collect(),
        checked_at: value["checkedAt"].as_u64().and_then(|at| {
            let clock = notification_time(at);
            let (hour, minute) = clock.split_once(':')?;
            Some((hour.parse().ok()?, minute.parse().ok()?))
        }),
        last_transition: (|| {
            let id = transition["id"].as_str()?.to_owned();
            let family = desktop_family(&transition["family"])?;
            match transition["kind"].as_str()? {
                "frozen" => Some(EgressTransition::Frozen),
                "resumed" => Some(EgressTransition::Resumed { id, family }),
                _ => None,
            }
        })(),
    })
}

fn project_update(value: update_download::UpdateState) -> UpdateState {
    use matou_native_core::update_policy::{ErrorStage, InstallMode};
    fn release(value: update_download::Release) -> UpdateRelease {
        let release_day = value.release_date.as_deref().and_then(|date| {
            let (_, day) = date.split_once('-')?;
            let (month, day) = day.get(..5)?.split_once('-')?;
            Some((month.parse().ok()?, day.parse().ok()?))
        });
        UpdateRelease {
            version: value.version,
            install_mode: match value.install_mode {
                InstallMode::Automatic => UpdateInstallMode::Automatic,
                InstallMode::Manual => UpdateInstallMode::Manual,
            },
            size_bytes: value.size_bytes,
            release_day,
            notes: value.release_notes,
        }
    }
    let status = match value.phase {
        UpdatePhase::Idle => UpdateStatus::Idle,
        UpdatePhase::NotAvailable => UpdateStatus::NotAvailable,
        UpdatePhase::Checking {
            retry_attempt,
            max_retry_attempts,
        } => UpdateStatus::Checking {
            retry_attempt: retry_attempt.unwrap_or_default() as u32,
            max_retry_attempts: max_retry_attempts.map(|attempts| attempts as u32),
        },
        UpdatePhase::Available { release: item } => UpdateStatus::Available(release(item)),
        UpdatePhase::Downloading {
            release: item,
            progress,
        } => UpdateStatus::Downloading(
            release(item),
            UpdateProgress {
                percent: progress.percent,
                transferred_bytes: progress.transferred_bytes,
                total_bytes: progress.total_bytes,
                bytes_per_second: progress.bytes_per_second,
                remaining_seconds: progress.remaining_seconds,
            },
        ),
        UpdatePhase::Downloaded { release: item } => UpdateStatus::Downloaded(release(item)),
        UpdatePhase::Error {
            error_message,
            error_stage,
            manual_download_url,
            ..
        } => UpdateStatus::Error {
            stage: match error_stage {
                ErrorStage::Check => UpdateErrorStage::Check,
                ErrorStage::Download => UpdateErrorStage::Download,
                ErrorStage::Verify => UpdateErrorStage::Verify,
                ErrorStage::Install => UpdateErrorStage::Install,
            },
            message: error_message,
            manual_download_available: manual_download_url.is_some(),
        },
    };
    UpdateState {
        current_version: value.current_version,
        status,
    }
}

fn restore_wiring_focus(
    window: &mut Window,
    cx: &App,
    overlay: &FocusHandle,
    previous: Option<FocusHandle>,
) {
    if overlay.is_focused(window) || overlay.contains_focused(window, cx) {
        if let Some(focus) = previous {
            window.focus(&focus);
        } else {
            window.blur();
        }
    }
}

fn agent_hud_mode(node: &Value) -> Option<&'static str> {
    match node["currentMode"].as_str()? {
        "claude-code" => Some("claude-code"),
        "antigravity" => Some("antigravity"),
        "codex" => Some("codex"),
        "dsh" => Some("dsh"),
        _ => None,
    }
}

fn measure_trigger(bounds: TriggerBounds) -> Canvas<()> {
    canvas(
        move |position, _, _| bounds.set(Some(position)),
        |_, _, _, _| {},
    )
    .absolute()
    .inset_0()
    .size_full()
}

struct DetachedCardView {
    owner: WeakEntity<Workspace>,
    main: AnyWindowHandle,
    session_id: String,
    terminal: Entity<TerminalSurface>,
    presentation: DetachedPresentation,
    hud_ready: bool,
    _owner_updates: Option<Subscription>,
    _keyboard: Subscription,
}

struct DetachedPresentation {
    title: String,
    locale: InterfaceLocale,
    light: bool,
    status: DetachedHeaderStatus,
    bg: u32,
    footer: matou_native_desktop::theme_tokens::FooterPalette,
}

impl Render for DetachedCardView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let presentation = &self.presentation;
        // open_window paints immediately while its owner is still handling the
        // detach action. Enable the HUD once the child has been registered.
        let hud = self
            .hud_ready
            .then(|| {
                self.owner
                    .update(cx, |owner, cx| {
                        owner.refresh_project_instructions(window, cx);
                        owner.dismiss_ineligible_account_overlay(window, cx);
                        let hud = owner.terminal_hud(Some(&self.session_id), true, window, cx);
                        let permissions = owner.detached_permission_overlay(window, cx);
                        let instructions = owner.project_instructions_overlay(window, cx);
                        let wiring = owner.render_wiring_overlay(window, cx);

                        (hud, permissions, instructions, wiring)
                    })
                    .ok()
            })
            .flatten();
        let target = self.session_id.clone();
        let owner = self.owner.clone();
        let main = self.main;
        let mut root = div()
            .id("detached-card-window")
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(rgba(presentation.bg))
            .child(detached_window_header(
                presentation.title.clone(),
                presentation.locale,
                presentation.status,
                presentation.light,
                move |action, _, cx| {
                    if action == DetachedWindowAction::ReturnToMain {
                        let owner = owner.clone();
                        let target = target.clone();
                        cx.defer(move |cx| {
                            let _ = main.update(cx, |_, window, cx| {
                                let _ = owner.update(cx, |this, cx| {
                                    this.return_detached_card(&target, window, cx);
                                });
                            });
                        });
                    }
                },
            ))
            .child(
                div()
                    .id("detached-terminal-body")
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(self.terminal.clone()),
            );
        if let Some((hud, permissions, instructions, wiring)) = hud {
            root = root
                .child(hud)
                .children(permissions)
                .children(instructions)
                .children(wiring);
        } else {
            root = root.child(
                div()
                    .flex_none()
                    .h(px(PERMISSION_FOOTER_HEIGHT))
                    .border_t_1()
                    .border_color(rgba(presentation.footer.top_line))
                    .bg(rgba(presentation.footer.background)),
            );
        }
        root
    }
}
