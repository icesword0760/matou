use super::*;

#[path = "footer_claude_accounts.rs"]
mod footer_claude_accounts;
use footer_claude_accounts::AccountUi;
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
        detached_placeholder, detached_window_header_with_notification,
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
    windows_titlebar,
    workspace_board::{
        BOARD_FEEDBACK_DURATION, BoardFeedback, BoardMoveRequest, BoardView, WorkspaceBoard,
        render_workspace_board,
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
            let opened = matou_native_runtime::browser_integration::open_controlled_url(&url);
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
    split: Option<SplitTree>,
    pending_split: Option<PendingSplit>,
    pending_shell_focus: Option<PendingShellFocus>,
    link_opener: Rc<LinkOpener>,
    copy_link: Rc<LinkCopier>,
    link_in_progress: bool,
    pub(super) board_open: bool,
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
    accounts: AccountUi,
    #[cfg(debug_assertions)]
    egress_probe_bounds: TriggerBounds,
    hud: HashMap<String, LiveHud>,
    canvas_team_members: HashMap<String, Vec<CanvasTeamMember>>,
    banners: HashMap<String, EgressBannerState>,
    confirmation_focus: FocusHandle,
    _window_closed: Subscription,
}

#[derive(Default)]
struct LiveHud {
    mode: Option<&'static str>,
    request: Option<u64>,
    canvas: Option<String>,
    result: Value,
    refresh_at: Option<Duration>,
}

#[derive(Clone)]
pub(super) struct CanvasTeamMember {
    pub id: String,
    lead: String,
    teammate_id: String,
    pub summary: Value,
}

fn observe_canvas_teammates(cards: &mut Vec<CanvasTeamMember>, lead: &str, members: &[Value]) {
    for summary in members {
        let Some(teammate_id) = trimmed_graph_text(&summary["teammateId"]) else {
            continue;
        };
        if let Some(card) = cards
            .iter_mut()
            .find(|card| card.lead == lead && card.teammate_id == teammate_id)
        {
            card.summary = summary.clone();
        } else if summary["workStatus"] != TeamMemberWorkStatus::Exited.wire() {
            cards.push(CanvasTeamMember {
                id: format!("team:{}:{lead}:{teammate_id}", lead.len()),
                lead: lead.to_owned(),
                teammate_id: teammate_id.to_owned(),
                summary: summary.clone(),
            });
        }
    }
}

#[cfg(test)]
#[::core::prelude::v1::test]
fn canvas_teammates_keep_one_summary_per_identity_and_skip_new_ended_members() {
    let mut cards = Vec::new();
    let running = serde_json::json!({"teammateId":"member@team","workStatus":"running"});
    observe_canvas_teammates(&mut cards, "lead", &[running.clone(), running.clone()]);
    assert_eq!(cards.len(), 1);
    let id = cards[0].id.clone();
    observe_canvas_teammates(&mut cards, "other-lead", &[running]);
    assert_eq!(cards.len(), 2);
    let exited = serde_json::json!({"teammateId":"member@team","workStatus":"exited","latestLines":["done"]});
    observe_canvas_teammates(
        &mut cards,
        "lead",
        &[
            exited.clone(),
            serde_json::json!({"teammateId":"unseen@team","workStatus":"exited"}),
        ],
    );
    observe_canvas_teammates(&mut cards, "lead", &[]);
    assert_eq!(cards.len(), 2);
    assert_eq!(cards[0].id, id);
    assert_eq!(cards[0].summary, exited);
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

struct PendingShellFocus {
    owner: String,
    existing: Vec<String>,
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
            split: None,
            pending_split: None,
            pending_shell_focus: None,
            link_opener: Rc::new(launch_link),
            copy_link: Rc::new(|url, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(url.to_owned()))
            }),
            link_in_progress: false,
            board_open: false,
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
            accounts: AccountUi::default(),
            #[cfg(debug_assertions)]
            egress_probe_bounds: TriggerBounds::default(),
            hud: HashMap::new(),
            canvas_team_members: HashMap::new(),
            banners: HashMap::new(),
            confirmation_focus: cx.focus_handle().tab_stop(true),
            _window_closed: window_closed,
        }
    }

    pub(super) fn is_detached(&self, id: &str) -> bool {
        self.detached.for_session(id).is_some()
    }
}

impl Workspace {
    pub(super) fn wiring_overlay_open(&self, window: &Window) -> bool {
        self.wiring.overlay.is_some()
            && self
                .wiring
                .overlay_window
                .is_none_or(|handle| handle == window.window_handle())
    }

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
                            && let Some(tree) = this.wiring.split.as_mut()
                            && tree.apply_ratio(change, 0).is_some()
                        {
                            this.save_split_layout(true, cx);
                            cx.notify();
                        }
                    });
                },
            ))
    }

    pub(super) fn sync_split_layout(&mut self) -> bool {
        let restored = self
            .carousel_owner
            .as_ref()
            .and_then(|owner| self.geometry.get(owner))
            .filter(|saved| self.graph["sceneId"].as_str() == Some(saved.scene_id.as_str()))
            .and_then(|saved| {
                SplitTree::from_canvas_layout(
                    &saved.scene_id,
                    &saved.geometry["splitLayout"],
                    |id| self.order.iter().any(|card| card == id) && !self.is_stopped_session(id),
                )
            });
        let pruned = self
            .wiring
            .split
            .as_ref()
            .zip(restored.as_ref())
            .is_some_and(|(previous, restored)| {
                previous.ordered_sessions().len() > restored.ordered_sessions().len()
            });
        self.wiring.split = restored;
        pruned
    }

    fn save_split_layout(&mut self, continuous: bool, cx: &mut Context<Self>) {
        let Some(owner) = self.carousel_owner.clone() else {
            return;
        };
        let mut geometry = self
            .geometry
            .get(&owner)
            .map_or_else(serde_json::Map::new, |saved| {
                saved.geometry.as_object().cloned().unwrap_or_default()
            });
        if let Some(tree) = &self.wiring.split {
            geometry.insert("splitLayout".into(), tree.canvas_layout());
        } else {
            geometry.insert("splitLayout".into(), Value::Null);
        }
        self.queue_canvas_geometry(owner, Value::Object(geometry), continuous, cx);
        if !continuous {
            self.flush_geometry(cx);
        }
    }

    pub(super) fn restore_split_focus(&mut self, cx: &mut Context<Self>) {
        if self.is_read_only() || !self.domain_ready || !self.active_path_valid() {
            return;
        }
        if let Some(tree) = &self.wiring.split {
            let panes = tree.ordered_sessions();
            if self
                .active_level_focus()
                .is_none_or(|id| !panes.contains(&id))
                && let (Some(scene), Some(id)) = (self.active_scene_id(), panes.first())
            {
                let (scene, id) = (scene.to_owned(), (*id).to_owned());
                self.send_card_focus(&scene, &id, cx);
            }
        }
    }

    pub(super) fn active_split_tree(&self) -> Option<&SplitTree> {
        let tree = self.wiring.split.as_ref()?;
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
            .split
            .get_or_insert_with(|| SplitTree::single(scene_id.clone(), &request.source));
        if !tree.ordered_sessions().contains(&request.source.as_str()) {
            *tree = SplitTree::single(scene_id, &request.source);
        }
        if tree.split_session(&request.source, &new, request.direction) {
            self.save_split_layout(false, cx);
            self.stop_edge_browse(true);
            if self.wiring_available()
                && !self.wiring.board_open
                && !self.provider_restore_hides_terminal(&new)
                && let Some(terminal) = self.terminals.get(&new)
            {
                terminal.read(cx).focus.focus(window);
            }
            cx.notify();
        }
    }

    pub(super) fn record_pending_shell_focus(&mut self) {
        self.wiring.pending_shell_focus =
            self.carousel_owner.clone().map(|owner| PendingShellFocus {
                owner,
                existing: self.order.clone(),
            });
    }

    pub(super) fn clear_pending_shell_focus(&mut self) {
        self.wiring.pending_shell_focus = None;
    }

    pub(super) fn finish_pending_shell_focus(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(request) = &self.wiring.pending_shell_focus else {
            return;
        };
        if self.carousel_owner.as_ref() != Some(&request.owner) {
            self.clear_pending_shell_focus();
            return;
        }
        let Some(new) = self.focused.as_ref().filter(|id| {
            !request.existing.contains(id)
                && self.order.contains(id)
                && self
                    .graph_node(id)
                    .is_some_and(|node| node["currentMode"] == "shell")
        }) else {
            return;
        };
        self.wiring.pending_shell_focus = None;
        // Mounting the new card can leave focus on the previous UI. Reassert
        // the create intent after the authoritative graph update so either
        // entry point leaves the new shell ready for keyboard input.
        if self.wiring_available()
            && !self.wiring.board_open
            && !self.provider_restore_hides_terminal(new)
            && let Some(terminal) = self.terminals.get(new)
        {
            terminal.read(cx).focus.focus(window);
        }
        cx.notify();
    }

    fn focus_split_pane(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.wiring.is_detached(id) {
            self.focus_detached_card(id, cx);
        } else if !self.provider_restore_hides_terminal(id)
            && let Some(terminal) = self.terminals.get(id)
        {
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
        let Some(tree) = self.wiring.split.as_mut() else {
            return;
        };
        if tree.close_session(id) {
            if tree.ordered_sessions().len() == 1 {
                self.wiring.split = None;
                self.restore_geometry = false;
            }
            self.save_split_layout(false, cx);
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

    pub(super) fn wiring_available(&self) -> bool {
        self.settings.is_none()
            && self.wiring_unblocked()
            && (self.hierarchy.is_some() || self.read_only.is_some())
    }

    fn wiring_unblocked(&self) -> bool {
        self.dialog.is_none()
            && self
                .project_instructions
                .values()
                .all(|state| state.preview.is_none())
            && self.database_recovery.is_none()
            && !self.invalid_recovery_marker
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

    pub(super) fn active_terminal_focus(&self, cx: &App) -> Option<FocusHandle> {
        let id = self.active_level_focus()?;
        if !self.wiring_available()
            || self.wiring.board_open
            || self.read_only.is_some()
            || self.wiring.is_detached(id)
            || !self.active_path_valid()
            || self.recovery_blocked(id)
            || self.provider_restore_hides_terminal(id)
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
                let focus = self.visible_wiring_focus(focus, cx);
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
            let accounts = self.wiring.overlay == Some(Overlay::Accounts);
            let previous = self.reset_wiring_overlay();
            let previous = self.visible_wiring_focus(previous, cx);
            if accounts && self.account_footer_visible(window) {
                window.focus(&self.control_focus["accounts"]);
            } else {
                restore_wiring_focus(window, cx, &self.wiring.overlay_focus, previous);
            }
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
            Some(Overlay::Accounts) => self.wiring.accounts.input = None,
            None => {}
        }
        self.wiring.restore_focus.take()
    }

    fn visible_wiring_focus(&self, previous: Option<FocusHandle>, cx: &App) -> Option<FocusHandle> {
        previous.filter(|focus| {
            !self.terminals.iter().any(|(id, terminal)| {
                terminal.read(cx).focus == *focus && self.provider_restore_hides_terminal(id)
            })
        })
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
        if self.wiring.overlay == Some(Overlay::Accounts) {
            return self.claude_accounts_key(stroke, window, cx);
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
        if self.read_only.is_some()
            || !self.wiring_unblocked()
            || self
                .hierarchy
                .as_ref()
                .and_then(|h| h.workspace.as_ref())
                .is_none()
        {
            return;
        }
        if !self.close_settings_for_navigation(window, cx) {
            return;
        }
        self.dismiss_wiring_overlay(window, cx);
        self.stop_edge_browse(false);
        if self.wiring.board_open {
            self.close_board_to_canvas(true, window, cx);
        } else {
            self.wiring.board_open = true;
            window.focus(&self.control_focus["sidebar-board"]);
            cx.notify();
        }
    }

    pub(super) fn close_board_to_canvas(
        &mut self,
        focus_terminal: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !std::mem::take(&mut self.wiring.board_open) {
            return;
        }
        self.restore_geometry = true;
        if focus_terminal && let Some(focus) = self.active_terminal_focus(cx) {
            window.focus(&focus);
        }
        cx.notify();
    }

    pub(super) fn sync_wiring_graph(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_ineligible_account_overlay(window, cx);
        let graph_scene = self.graph["sceneId"].as_str();
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

    fn board_projection(&self) -> Option<WorkspaceBoard> {
        let hierarchy = self.hierarchy.as_ref()?;
        let workspace = hierarchy.workspace.as_ref()?;
        let active_task = hierarchy.task.as_ref().map(|task| task.id.as_str());
        let counts = self.graph["workspaceSessionCountByTask"].as_object();
        let mut board = WorkspaceBoard::from_session_counts(
            &workspace.id,
            &workspace.name,
            active_task,
            &self.tasks,
            |task| {
                counts
                    .and_then(|counts| counts.get(task))
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize
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
            .board_projection()
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

    fn board_view(&self) -> Option<BoardView> {
        let workspace = self.hierarchy.as_ref()?.workspace.as_ref()?;
        let feedback = self
            .wiring
            .board_feedback
            .as_ref()
            .filter(|(_, owner, _)| *owner == workspace.id)
            .map(|(_, _, feedback)| *feedback);
        Some(self.board_projection()?.view(self.locale, feedback))
    }

    pub(super) fn render_board(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if !self.wiring.board_open {
            return None;
        }
        let view = self.board_view()?;
        let owner = cx.weak_entity();
        Some(
            render_workspace_board(
                &view,
                self.wiring_dark(),
                Rc::new(move |request, _, cx| {
                    let _ = owner.update(cx, |this, cx| this.move_task_on_board(request, cx));
                }),
            )
            .into_any_element(),
        )
    }

    pub(super) fn wiring_card_menu(&self, id: &str) -> CardMenu {
        let Some(node) = self.graph_node(id) else {
            return resolve_card_menu(CardMenuContext::default(), self.locale);
        };
        let kind = node["currentMode"].as_str().unwrap_or_default();
        let archived = !node["archivedAt"].is_null();
        let has_terminal = self.terminals.contains_key(id);
        let can_mutate = self.card_action_available(id);
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
        let presentation = if self.wiring.is_detached(id) {
            CardPresentation::Detached
        } else if archived {
            CardPresentation::Archived
        } else {
            CardPresentation::Attached
        };
        let mut menu = resolve_card_menu(
            CardMenuContext {
                kind,
                presentation,
                can_rename: can_mutate,
                fork,
                // For Codex, Runtime validates both the repository and the
                // trusted isolation root when the New worktree action is sent.
                git_available: kind == "codex" || !node["git"].is_null(),
                can_detach: can_mutate && self.terminals.contains_key(id),
                can_remove_node: self.remove_session_available(id),
                read_only: self.read_only.is_some(),
                storage_fault: self.storage_fault_sessions.contains(id),
                environment_unavailable: !self.active_path_valid()
                    || self.environment_unavailable(id),
                recovering: has_terminal && self.recovery_blocked(id),
            },
            self.locale,
        );
        if let Some(index) = menu
            .items
            .iter()
            .position(|item| item.action == CardMenuAction::Rename)
        {
            let disabled_reason = menu.items[index].disabled_reason.clone().or_else(|| {
                (node["titleSource"] != "manual").then(|| {
                    self.locale
                        .text(
                            "当前会话已使用自动标题",
                            "This session already uses an automatic title",
                        )
                        .into()
                })
            });
            menu.items.insert(
                index + 1,
                CardMenuItem {
                    action: CardMenuAction::RestoreAutomaticTitle,
                    label: self
                        .locale
                        .text("恢复自动标题", "Restore automatic title")
                        .into(),
                    disabled_reason,
                    show_readiness_hint: false,
                    danger: false,
                },
            );
        }
        if presentation == CardPresentation::Attached && !has_terminal && can_mutate {
            menu.items.push(CardMenuItem {
                action: CardMenuAction::RemoveNode,
                label: self.locale.text("移除节点…", "Remove node…").into(),
                disabled_reason: None,
                show_readiness_hint: false,
                danger: true,
            });
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
        if presentation == CardPresentation::Detached
            && self
                .active_split_tree()
                .is_some_and(|tree| tree.ordered_sessions().contains(&id))
        {
            menu.items.push(CardMenuItem {
                action: CardMenuAction::ClosePane,
                label: self.locale.text("关闭分屏", "Close pane").into(),
                disabled_reason: None,
                show_readiness_hint: false,
                danger: false,
            });
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
                    CardMenuAction::RestoreAutomaticTitle => self.restore_session_title(id, cx),
                    CardMenuAction::Fork => self.open_fork(id, ForkPlacement::Peer, window, cx),
                    CardMenuAction::ForkWorktree => self.open_fork_with_worktree(
                        id,
                        ForkPlacement::Peer,
                        ForkWorktreeMode::New,
                        window,
                        cx,
                    ),
                    CardMenuAction::OpenDetached => self.detach_card(id, window, cx),
                    CardMenuAction::ReturnToMain => self.return_detached_card(id, window, cx),
                    CardMenuAction::ClosePane => self.close_split_pane(id, window, cx),
                    CardMenuAction::RemoveNode => self.open_remove_session(id, window, cx),
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
                    appears_transparent: windows_titlebar::appears_transparent(),
                    traffic_light_position: cfg!(target_os = "macos")
                        .then_some(point(px(12.), px(13.))),
                }),
                ..Default::default()
            },
            |window, cx| {
                #[cfg(target_os = "windows")]
                matou_native_desktop::windows_theme::set_dark(window, !presentation.light);
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
                                    let presentation =
                                        owner.detached_presentation(card.title.clone());
                                    #[cfg(target_os = "windows")]
                                    if child.presentation.light != presentation.light {
                                        matou_native_desktop::windows_theme::set_dark(
                                            window,
                                            !presentation.light,
                                        );
                                    }
                                    child.presentation = presentation;
                                    cx.notify();
                                }
                            },
                        )
                    });
                    let notification_activation = cx.observe_window_activation(
                        window,
                        |child: &mut DetachedCardView, window, cx| {
                            child.finish_notification_focus(window, cx);
                        },
                    );
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
                        notification_focus: None,
                        _owner_updates: owner_updates,
                        _notification_activation: notification_activation,
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

    pub(super) fn active_detached_notification_session(&self, cx: &mut App) -> Option<String> {
        self.wiring.children.iter().find_map(|(session, child)| {
            child
                .window
                .update(cx, |_, window, _| notification_window_active(window))
                .ok()
                .filter(|active| *active)
                .map(|_| session.clone())
        })
    }

    pub(super) fn open_detached_notification(&self, id: u64, session: &str, cx: &mut App) {
        if let Some(child) = self.wiring.children.get(session) {
            let _ = child.window.update(cx, |child, window, cx| {
                child.notification_focus = Some(id);
                child.terminal.read(cx).focus.focus(window);
                if notification_window_active(window) {
                    child.finish_notification_focus(window, cx);
                } else if self.wiring.visible {
                    window.activate_window();
                }
            });
        }
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
        self.wiring.detached.discard_session(id);
        if let Some(child) = self.wiring.children.remove(id) {
            self.clear_detached_hud(child.window.into());
            let _ = child
                .window
                .update(cx, |_, window, _| window.remove_window());
        }
        self.wiring.banners.remove(id);
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
                    DetachedWindowAction::Focus => {
                        this.dismiss_session_notification(&target, cx);
                        this.focus_detached_card(&target, cx);
                    }
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
                self.request_update("updateInstallResult", UpdateErrorStage::Install, cx);
                cx.notify();
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
            "updateInstallResult" => Command::InstallUpdate { request_id },
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
        if stage == UpdateErrorStage::Install {
            self.wiring.update.finish_install(false);
        }
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

    fn egress_anchor(&self, window: &mut Window, cx: &mut Context<Self>) -> TriggerBounds {
        window
            .use_keyed_state("footer-egress-anchor", cx, |_, _| TriggerBounds::default())
            .read(cx)
            .clone()
    }

    pub(super) fn egress_trigger_for(
        &self,
        session: Option<&str>,
        compact: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        use matou_native_desktop::egress_panel::EgressTone;

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
            view.tone = EgressTone::Neutral;
        }
        let label = view.as_ref().map(|view| {
            let region = if view.tone == EgressTone::Ok {
                self.wiring
                    .egress_status
                    .footer_region(session.unwrap_or_default(), self.locale)
            } else {
                None
            };
            footer_text("egress", region.unwrap_or_else(|| view.label.clone()))
        });
        let tooltip = view
            .as_ref()
            .map(|view| SharedString::from(view.accessible_label.clone()));
        div()
            .id("footer-egress")
            .flex_none()
            .when(compact, |element| element.max_w(px(76.)))
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
            .when_some(tooltip, |element, tooltip| {
                element.tooltip(move |_, cx| cx.new(|_| CardTooltip(tooltip.clone())).into())
            })
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
        self.retire_footer_git(cx);
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
            if stage == UpdateErrorStage::Install {
                self.wiring.update.finish_install(false);
            }
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
        self.wiring.accounts = AccountUi::default();
        self.wiring.hud.clear();
        self.context_rescue_requests.clear();
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

    pub(super) fn refresh_egress_for_graph_change(
        &mut self,
        graph: &Value,
        cx: &mut Context<Self>,
    ) {
        let (generation, phase) = self.commands.connection();
        if generation != self.runtime_generation
            || phase != ConnectionPhase::Ready
            || !self.domain_ready
        {
            return;
        }
        let modes: HashMap<_, _> = session_levels::nodes(&self.graph)
            .iter()
            .filter_map(|node| Some((node["sessionId"].as_str()?, node["currentMode"].as_str()?)))
            .collect();
        let nodes = session_levels::nodes(graph);
        if modes.len() != nodes.len()
            || nodes.iter().any(|node| {
                node["sessionId"]
                    .as_str()
                    .is_some_and(|id| modes.get(id).copied() != node["currentMode"].as_str())
            })
        {
            self.refresh_egress_status(cx);
        }
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
        if !self.wiring.accounts.requested
            && targets.iter().any(|id| {
                self.hud_node(id)
                    .is_some_and(|node| node["currentMode"] == "claude-code")
            })
        {
            self.load_claude_accounts(cx);
        }
        let now = self.motion_clock.elapsed();
        for session_id in targets {
            let mode = self.hud_node(&session_id).and_then(agent_hud_mode);
            let canvas = self
                .active_scene_id()
                .filter(|_| {
                    self.level_focus() == Some(session_id.as_str())
                        && !self.wiring.is_detached(&session_id)
                })
                .map(str::to_owned);
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
                let state = self.wiring.hud.entry(session_id).or_default();
                state.request = Some(request_id);
                state.canvas = canvas;
            }
        }
    }

    pub(super) fn request_context_rescue(&mut self, session: &str, cx: &mut Context<Self>) {
        if self.context_rescue_requests.contains_key(session) {
            return;
        }
        let request_id = self.next_wiring_request();
        if self
            .commands
            .send_at(
                self.runtime_generation,
                Command::RescueSessionWithHandoff {
                    request_id,
                    session_id: session.to_owned(),
                },
            )
            .is_ok()
        {
            self.context_rescue_requests
                .insert(session.to_owned(), request_id);
            cx.notify();
        } else {
            self.show_error_toast(
                self.locale
                    .text("无法启动救援会话", "Could not start rescue session"),
                cx,
            );
        }
    }

    pub(super) fn request_session_handoff(&mut self, session_id: String, cx: &mut Context<Self>) {
        let request_id = self.next_wiring_request();
        if self
            .commands
            .send_at(
                self.runtime_generation,
                Command::GetSessionHandoffBrief {
                    request_id,
                    session_id,
                },
            )
            .is_err()
        {
            self.show_error_toast(
                self.locale
                    .text("无法生成交接摘要：运行时未就绪", "Runtime unavailable for handoff brief"),
                cx,
            );
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
        } else if kind == "egressStatusChanged"
            && request_id == 0
            && let Some(status) = project_egress(&reply["result"])
        {
            self.set_egress_status(status, window, cx);
            self.refresh_frozen_egress(cx);
        }
        if let Some((pending, expected, stage)) = self.wiring.update_request
            && pending == request_id
            && expected == kind
        {
            self.wiring.update_request = None;
            match self.wiring_reply_error(reply) {
                Some(error) => self.update_failed(stage, error, cx),
                None if kind == "updateInstallResult" => {
                    if reply["result"]["ok"] == true {
                        self.wiring.update.finish_install(true);
                        cx.notify();
                    } else {
                        let message = self.locale.text(
                            "运行时未确认打开安装包。",
                            "The runtime did not confirm that the installer opened.",
                        );
                        self.update_failed(stage, message.into(), cx);
                    }
                }
                None => match serde_json::from_value::<update_download::UpdateState>(
                    reply["result"].clone(),
                ) {
                    Ok(status) => self.accept_update_state(project_update(status), cx),
                    Err(error) => self.update_failed(stage, error.to_string(), cx),
                },
            }
        }
        if kind == "claudeAccountsResult" {
            self.receive_claude_accounts(request_id, reply, cx);
        }
        self.receive_footer_diagnostics(kind, request_id, reply, cx);
        if kind == "rescueSessionWithHandoffResult" {
            let count = self.context_rescue_requests.len();
            self.context_rescue_requests
                .retain(|_, request| *request != request_id);
            if count == self.context_rescue_requests.len() {
                return;
            }
            let result = &reply["result"];
            if let Some(error) = self.wiring_reply_error(reply) {
                self.show_error_toast(error, cx);
            } else if let (Some(scene_id), Some(session_id)) =
                (result["sceneId"].as_str(), result["sessionId"].as_str())
            {
                if result["delivery"] == "clipboard_fallback"
                    && let Some(markdown) = result["markdown"].as_str()
                {
                    cx.write_to_clipboard(ClipboardItem::new_string(markdown.to_owned()));
                    self.show_toast(
                        self.locale.text(
                            "新会话已创建，交接摘要已复制到剪贴板，可直接粘贴",
                            "New session created. Handoff brief copied and ready to paste.",
                        ),
                        cx,
                    );
                }
                self.send(
                    Command::FocusSession {
                        scene_id: scene_id.to_owned(),
                        session_id: session_id.to_owned(),
                    },
                    cx,
                );
            } else {
                self.show_toast(
                    self.locale.text(
                        "当前会话暂无可交接进度",
                        "No handoff progress recorded for this session yet",
                    ),
                    cx,
                );
            }
            cx.notify();
            return;
        }
        if kind == "sessionHandoffBriefResult" {
            if let Some(markdown) = reply["result"]["markdown"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
            {
                cx.write_to_clipboard(ClipboardItem::new_string(markdown.to_owned()));
                self.show_toast(
                    self.locale.text(
                        "已复制交接摘要，可直接粘贴到新会话继续",
                        "Handoff brief copied — paste into a new session to continue",
                    ),
                    cx,
                );
            } else {
                self.show_toast(
                    self.locale.text(
                        "当前会话暂无可交接进度",
                        "No handoff progress recorded for this session yet",
                    ),
                    cx,
                );
            }
            return;
        }
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
            let canvas = state.canvas.take();
            let interval = Duration::from_secs(2);
            let refresh_at = self.motion_clock.elapsed() + interval;
            state.refresh_at = Some(refresh_at);
            if let Some(canvas) = canvas.filter(|id| self.active_scene_id() == Some(id.as_str()))
                && self.graph_node(&target).is_some()
                && !self.wiring.is_detached(&target)
                && let Some(members) = reply["result"]["teammates"].as_array()
                && !members.is_empty()
            {
                observe_canvas_teammates(
                    self.wiring.canvas_team_members.entry(canvas).or_default(),
                    &target,
                    members,
                );
            }
            self.release_footer_git_projection(&target, request_id, reply);
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

    pub(super) fn canvas_team_members(&self) -> Vec<CanvasTeamMember> {
        if self.read_only.is_some() {
            return Vec::new();
        }
        let Some(cards) = self
            .active_scene_id()
            .and_then(|scene| self.wiring.canvas_team_members.get(scene))
        else {
            return Vec::new();
        };
        let nodes = session_levels::nodes(&self.graph);
        cards
            .iter()
            .filter(|card| {
                self.order.contains(&card.lead)
                    && !nodes.iter().any(|node| {
                        Self::team_member_node(node)
                            && session_levels::parent_id(node) == Some(card.lead.as_str())
                            && node["teamMemberSummary"]["teammateId"] == card.teammate_id
                    })
            })
            .cloned()
            .collect()
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

    pub(super) fn detach_card_for_probe(
        &mut self,
        session: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.detach_card(session, window, cx);
    }

    pub(super) fn board_view_for_probe(&self) -> Option<BoardView> {
        self.board_view()
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

#[cfg(test)]
mod egress_tests {
    use super::{EgressPanelState, InterfaceLocale, project_egress};

    #[test]
    fn runtime_session_families_supply_the_flag_and_full_egress_copy() {
        let families = ["anthropic", "openai", "google"].map(|family| {
            serde_json::json!({
                "family": family, "state": "ok", "exit": {"ip": "203.0.113.1", "country": "US"},
            })
        });
        let status = project_egress(&serde_json::json!({
        "state": "ok",
        "checkedAt": 1_700_000_000_000u64,
        "checks": [],
            "families": families,
        "sessionFamilies": {"claude-card": ["anthropic"], "codex-card": ["openai"], "agy-card": ["google"]},
    }))
        .unwrap();
        let panel = EgressPanelState::default();
        for session in ["claude-card", "codex-card", "agy-card"] {
            let chip = panel
                .chip(true, Some(&status), session, InterfaceLocale::Chinese)
                .unwrap();
            assert_eq!(chip.label, "🇺🇸 美国 · 出口正常");
            assert_eq!(chip.accessible_label, chip.label);
            assert_eq!(
                status
                    .footer_region(session, InterfaceLocale::Chinese)
                    .as_deref(),
                Some("🇺🇸")
            );
        }
    }
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
    notification_focus: Option<u64>,
    _owner_updates: Option<Subscription>,
    _notification_activation: Subscription,
    _keyboard: Subscription,
}

impl DetachedCardView {
    fn dismiss_notification_on_press(
        &mut self,
        event: &MouseDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.terminal.read(cx).scrollbar_hit(event.position) {
            let _ = self.owner.update(cx, |owner, cx| {
                owner.dismiss_session_notification(&self.session_id, cx);
            });
        }
    }

    fn finish_notification_focus(&mut self, window: &Window, cx: &mut Context<Self>) {
        if notification_window_active(window)
            && self.terminal.read(cx).focus.is_focused(window)
            && let Some(id) = self.notification_focus.take()
        {
            let owner = self.owner.clone();
            let session = self.session_id.clone();
            cx.defer(move |cx| {
                let _ = owner.update(cx, |owner, cx| {
                    owner.finish_notification_jump(id, &session, cx)
                });
            });
        }
    }
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

                        let environment = owner.hud_node(&self.session_id).and_then(|node| {
                            owner.environment_card_overlay(&self.session_id, node, window, cx)
                        });
                        (hud, permissions, instructions, wiring, environment)
                    })
                    .ok()
            })
            .flatten();
        let (hud, environment) = match hud {
            Some((hud, permissions, instructions, wiring, environment)) => {
                (Some((hud, permissions, instructions, wiring)), environment)
            }
            None => (None, None),
        };
        let target = self.session_id.clone();
        let owner = self.owner.clone();
        let main = self.main;
        let unread = self.hud_ready
            && self.owner.upgrade().is_some_and(|owner| {
                owner
                    .read(cx)
                    .notifications
                    .session_has_unread(&self.session_id)
            });
        let animate = unread && !reduced_motion();
        let mut root = div()
            .id("detached-card-window")
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(rgba(presentation.bg))
            .child(
                div()
                    .flex_none()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(Self::dismiss_notification_on_press),
                    )
                    .child(detached_window_header_with_notification(
                        presentation.title.clone(),
                        presentation.locale,
                        presentation.status,
                        presentation.light,
                        #[cfg(target_os = "windows")]
                        Some(window.is_maximized()),
                        #[cfg(not(target_os = "windows"))]
                        None,
                        unread.then(|| notification_card_badge(presentation.locale, animate)),
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
                    )),
            )
            .child(
                div()
                    .id("detached-terminal-body")
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(Self::dismiss_notification_on_press),
                    )
                    .child(self.terminal.clone())
                    .children(environment),
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
        root.when(unread, |root| {
            root.child(notification_card_frame(px(0.), animate))
        })
    }
}
