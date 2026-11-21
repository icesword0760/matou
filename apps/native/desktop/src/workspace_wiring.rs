use super::*;
use matou_native_desktop::{
    card_context_menu::{
        CardMenu, CardMenuAction, CardMenuContext, CardMenuEvent, CardPresentation, ForkReadiness,
        card_context_menu_with_selection, resolve_card_menu,
    },
    detach_window::{
        DetachOutcome, DetachedHeaderStatus, DetachedMount, DetachedWindowAction, DetachedWindows,
        detached_placeholder, detached_window_header,
    },
    egress_panel::{
        EgressAction, EgressBannerState, EgressPanelState, EgressState, EgressStatus,
        render_egress_banner, render_egress_chip_with_label, render_egress_panel,
    },
    shortcut_panel::{
        ShortcutLocale, ShortcutPanelAction, ShortcutPanelClose, ShortcutPanelEffect,
        ShortcutPanelModel, ShortcutPanelState, ShortcutPlatform, ShortcutTheme, shortcut_panel,
    },
    team_member_card::{
        TeamMemberActions, TeamMemberCardModel, TeamMemberWorkStatus, team_member_card,
    },
    update_popover::{
        UpdateAction, UpdateControl, UpdateEffect, UpdateErrorStage, UpdateState, UpdateStatus,
        render_update_popover, render_update_trigger,
    },
    workspace_board::{WorkspaceBoard, render_workspace_board_with_sessions},
};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Overlay {
    Shortcuts,
    CardMenu,
    Update,
    Egress,
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
}

type TriggerBounds = Rc<Cell<Option<Bounds<Pixels>>>>;

pub(super) struct WorkspaceWiring {
    overlay: Option<Overlay>,
    overlay_focus: FocusHandle,
    restore_focus: Option<FocusHandle>,
    shortcuts: ShortcutPanelState,
    card_menu: Option<(String, Point<Pixels>)>,
    card_menu_index: Option<usize>,
    pub(super) board_open: bool,
    board_scenes: HashMap<String, BoardScene>,
    detached: DetachedWindows,
    children: HashMap<String, DetachedCard>,
    next_child: u64,
    visible: bool,
    update: UpdateControl,
    update_ticket: u64,
    update_bounds: TriggerBounds,
    egress: EgressPanelState,
    egress_status: EgressStatus,
    egress_bounds: TriggerBounds,
    banners: HashMap<String, EgressBannerState>,
    confirmation_focus: FocusHandle,
    _window_closed: Subscription,
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
            overlay_focus: cx.focus_handle().tab_stop(true),
            restore_focus: None,
            shortcuts: ShortcutPanelState::default(),
            card_menu: None,
            card_menu_index: None,
            board_open: false,
            board_scenes: HashMap::new(),
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
            update_ticket: 0,
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
            egress_bounds: Rc::default(),
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
    fn wiring_dark(&self) -> bool {
        self.theme.effective() == matou_native_desktop::theme_tokens::AppTheme::Dark
    }

    fn wiring_available(&self) -> bool {
        self.settings.is_none()
            && self.dialog.is_none()
            && self.project_instructions_preview.is_none()
            && self.database_recovery.is_none()
            && !self.invalid_recovery_marker
            && (self.hierarchy.is_some() || self.read_only.is_some())
    }

    fn open_wiring_overlay(&mut self, next: Overlay, window: &mut Window, cx: &mut Context<Self>) {
        if self.wiring.overlay != Some(next) {
            self.dismiss_wiring_overlay(window, cx);
            self.close_notifications(cx);
            self.workspace_menu = None;
            self.task_menu = None;
            self.tab_menu = None;
            self.hidden_tabs = None;
            self.permission_menu = None;
            self.wiring.restore_focus = window.focused(cx);
            self.wiring.overlay = Some(next);
            window.focus(&self.wiring.overlay_focus);
        }
        cx.notify();
    }

    pub(super) fn dismiss_wiring_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
            None => return,
        }
        let previous = self.wiring.restore_focus.take();
        if self.wiring.overlay_focus.is_focused(window)
            || self.wiring.overlay_focus.contains_focused(window, cx)
        {
            if let Some(focus) = previous {
                window.focus(&focus);
            } else {
                window.blur();
            }
        }
        cx.notify();
    }

    pub(super) fn wiring_key(
        &mut self,
        stroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let mods = stroke.modifiers;
        let shortcut =
            stroke.key == "/" && (mods.platform ^ mods.control) && !mods.shift && !mods.alt;
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
        if shortcut
            && self.wiring_available()
            && !self
                .terminals
                .values()
                .any(|terminal| terminal.read(cx).focus.is_focused(window))
        {
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
            window.focus(&self.control_focus["board"]);
        }
        if !self.wiring.board_open {
            self.restore_geometry = true;
        }
        cx.notify();
    }

    pub(super) fn sync_wiring_graph(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let graph_scene = self.graph["sceneId"].as_str();
        let active_scene = self.hierarchy.as_ref().and_then(|h| h.scene.as_ref());
        if let Some(scene) = active_scene.filter(|scene| graph_scene == Some(scene.id.as_str())) {
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
                && let Some(title) = node["title"].as_str()
            {
                child.title = title.into();
            }
        }
        if self.wiring.card_menu.as_ref().is_some_and(|(id, _)| {
            !self.order.contains(id) || !self.wiring_card_menu(id).can_open()
        }) {
            self.dismiss_wiring_overlay(window, cx);
        }
    }

    pub(super) fn forget_board_scene(&mut self, scene_id: &str) {
        self.wiring.board_scenes.remove(scene_id);
    }

    pub(super) fn render_board(&self, cx: &Context<Self>) -> Option<AnyElement> {
        if !self.wiring.board_open {
            return None;
        }
        let hierarchy = self.hierarchy.as_ref()?;
        let workspace = hierarchy.workspace.as_ref()?;
        let active_task = hierarchy.task.as_ref().map(|task| task.id.as_str());
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
        let view = WorkspaceBoard::from_session_tasks(
            &workspace.id,
            &workspace.name,
            active_task,
            &self.tasks,
            groups
                .iter()
                .flat_map(|(task, sessions)| sessions.iter().map(|_| task.as_str())),
        )
        .view(self.locale, None);
        let owner = cx.weak_entity();
        let palette = self.theme_palette().workspace;
        Some(
            render_workspace_board_with_sessions(
                &view,
                self.wiring_dark(),
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
        window.focus(&self.control_focus["board"]);
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
                git_available: !node["git"].is_null(),
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
                recovering: self.recovery_blocked(id),
            },
            self.locale,
        );
        // The current Runtime explicitly rejects creating a fork in a new worktree.
        if let Some(item) = menu
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
                    CardMenuAction::ForkWorktree => {}
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

    pub(super) fn card_more_button(&self, id: &str, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.wiring_card_menu(id).can_open() {
            return None;
        }
        let target = id.to_owned();
        let label = self.locale.text("会话操作", "Session actions");
        Some(
            div()
                .id(SharedString::from(format!("card-more-{id}")))
                .focusable()
                .tab_index(0)
                .size(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(5.))
                .text_color(rgba(self.theme_palette().workspace.muted))
                .cursor_pointer()
                .tooltip(move |_, cx| cx.new(|_| CardTooltip(label.into())).into())
                .child("···")
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                    this.open_wiring_card_menu(&target, event.position(), window, cx);
                }))
                .into_any_element(),
        )
    }

    pub(super) fn render_wiring_overlay(
        &self,
        window: &Window,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        if !self.wiring_available() {
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
                            &self
                                .wiring
                                .update
                                .view(self.locale, self.motion_clock.elapsed()),
                            dark,
                            bounds,
                            focus,
                            self.update_handler(cx),
                        ))
                        .into_any_element(),
                )
            }
            Overlay::Egress => {
                let bounds = self.wiring.egress_bounds.get();
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
                if let Some(view) = &mut view
                    && self.wiring.egress_status.state == EgressState::Unknown
                {
                    view.checked = self
                        .locale
                        .text("暂无出口检测结果", "No exit check results")
                        .into();
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
                    window.focus(&terminal.read(cx).focus);
                    DetachedCardView {
                        owner,
                        main,
                        session_id,
                        terminal,
                        presentation,
                        _owner_updates: owner_updates,
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
                    },
                );
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
        cx.notify();
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
        self.wiring.detached.discard_session(id);
        if let Some(child) = self.wiring.children.remove(id) {
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
        if let Some(child) = self.wiring.children.remove(id)
            && explicit
        {
            let _ = child
                .window
                .update(cx, |_, window, _| window.remove_window());
        }
        if self.active_scene_id() == Some(scene_id.as_str())
            && self.level_focus() == Some(id)
            && self.wiring_available()
            && !self.wiring.board_open
            && !self.recovery_blocked(id)
            && let Some((card, terminal)) = self.card_controls.get(id).zip(self.terminals.get(id))
            && (card.is_focused(window) || window.focused(cx).is_none())
        {
            window.focus(&terminal.read(cx).focus);
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
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(render_update_trigger(
                &self
                    .wiring
                    .update
                    .view(self.locale, self.motion_clock.elapsed()),
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
        }
        let stage = match self.wiring.update.dispatch(action) {
            Some(UpdateEffect::Check) => {
                self.accept_update_status(
                    UpdateStatus::Checking {
                        retry_attempt: 0,
                        max_retry_attempts: None,
                    },
                    cx,
                );
                UpdateErrorStage::Check
            }
            Some(UpdateEffect::Download) => UpdateErrorStage::Download,
            Some(UpdateEffect::Install) => UpdateErrorStage::Install,
            None => return,
        };
        let ticket = self.wiring.update_ticket;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(300))
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.wiring.update_ticket == ticket {
                    this.accept_update_status(
                        UpdateStatus::Error {
                            stage,
                            message: "offline".into(),
                            manual_download_available: false,
                        },
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    pub fn accept_update_status(&mut self, status: UpdateStatus, cx: &mut Context<Self>) {
        self.wiring.update_ticket += 1;
        self.wiring.update.accept(
            UpdateState {
                current_version: self.wiring.update.current_version().to_owned(),
                status,
            },
            self.motion_clock.elapsed(),
        );
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
        self.wiring
            .egress_status
            .families
            .iter()
            .any(|family| family.frozen_session_ids.iter().any(|frozen| frozen == id))
    }

    pub(super) fn egress_trigger(&self, cx: &Context<Self>) -> Stateful<Div> {
        let mut view = self.wiring.egress.chip(
            true,
            Some(&self.wiring.egress_status),
            self.active_level_focus().unwrap_or_default(),
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
            .children(footer_bounds_probe("egress"))
            .child(render_egress_chip_with_label(
                view.as_ref(),
                self.wiring_dark(),
                label,
                self.egress_handler(None, cx),
            ))
            .child(measure_trigger(self.wiring.egress_bounds.clone()))
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
                    EgressAction::Recheck => banner.recheck_finished(),
                    EgressAction::ConfirmOverride(_) => banner.override_finished(),
                    _ => {}
                }
                self.egress_offline_feedback(cx);
            }
            cx.notify();
            return;
        }
        match action {
            EgressAction::TogglePanel if self.wiring.overlay == Some(Overlay::Egress) => {
                self.dismiss_wiring_overlay(window, cx)
            }
            EgressAction::TogglePanel if self.wiring_available() => {
                self.open_wiring_overlay(Overlay::Egress, window, cx);
                self.wiring.egress.open = true;
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
            EgressAction::OpenReference(reference) => cx.open_url(reference.url()),
            EgressAction::Recheck
            | EgressAction::SetRule(_)
            | EgressAction::SetProtection(_)
            | EgressAction::AcceptCurrent(_) => self.egress_offline_feedback(cx),
            _ => {}
        }
        cx.notify();
    }

    fn egress_offline_feedback(&mut self, cx: &mut Context<Self>) {
        self.show_toast(
            self.locale.text(
                "出口检测服务当前不可用",
                "Exit checking is currently unavailable",
            ),
            cx,
        );
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

fn wiring_probe_bounds(_id: String) -> Option<Canvas<()>> {
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
        })
    }

    pub(super) fn update_view_for_probe(&self) -> matou_native_desktop::update_popover::UpdateView {
        self.wiring
            .update
            .view(self.locale, self.motion_clock.elapsed())
    }

    pub(super) fn trigger_for_probe(&self, egress: bool) -> Bounds<Pixels> {
        if egress {
            self.wiring.egress_bounds.get()
        } else {
            self.wiring.update_bounds.get()
        }
        .expect("trigger painted")
    }

    pub(super) fn request_offline_update_check_for_probe(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.update_action(UpdateAction::Check, window, cx);
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
    _owner_updates: Option<Subscription>,
}

struct DetachedPresentation {
    title: String,
    locale: InterfaceLocale,
    light: bool,
    status: DetachedHeaderStatus,
    bg: u32,
}

impl Render for DetachedCardView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let presentation = &self.presentation;
        let target = self.session_id.clone();
        let owner = self.owner.clone();
        let main = self.main;
        div()
            .id("detached-card-window")
            .size_full()
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
                                    if this.wiring.visible {
                                        window.activate_window();
                                    }
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
            )
    }
}
