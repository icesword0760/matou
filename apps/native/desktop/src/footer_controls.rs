// Anchored HUD controls shared by the workspace and detached terminal windows.
// Frozen references: hud/GitControlMenu.tsx, hud/EnvironmentControlMenu.tsx,
// and TerminalHud.tsx (QuotaDetailWindows).
use super::*;
use matou_native_core::git_repo::{GitStatusDetail, LocalBranch};
use matou_native_runtime::claude_accounts::ClaudeAccountsState;
use matou_native_runtime::host::{GitFailure, GitReply};
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FooterMenu {
    Git,
    Environment,
    Quota,
    Stats,
}

impl FooterMenu {
    fn id(self) -> &'static str {
        match self {
            Self::Git => "footer-git",
            Self::Environment => "footer-environment",
            Self::Quota => "footer-quota",
            Self::Stats => "footer-stats",
        }
    }
}

struct QuotaSlot {
    usage: codex_hud::UsageWindow,
    tooltip: String,
}

fn claude_quota_slots(
    hud: &Value,
    default_account: bool,
    accounts: Option<&ClaudeAccountsState>,
    locale: InterfaceLocale,
    now: u64,
) -> Vec<QuotaSlot> {
    let unavailable = locale.text("暂无用量数据", "Usage unavailable");
    if !default_account {
        return codex_hud::usage_windows(hud, "usageWindows")
            .into_iter()
            .map(|usage| {
                let reset = usage.reset.as_ref().map_or_else(
                    || locale.text("暂无重置时间", "Reset time unavailable").into(),
                    |text| format!("{} {text}", locale.text("距离重置", "resets in")),
                );
                let tooltip = format!("{} usage: {}% · {reset}", usage.label, usage.percent);
                QuotaSlot { usage, tooltip }
            })
            .collect();
    }

    let Some(accounts) = accounts else {
        return Vec::new();
    };
    let Some(account) = accounts
        .accounts
        .iter()
        .find(|account| Some(&account.id) == accounts.current_id.as_ref())
        .or(accounts.detected.as_ref())
        .filter(|account| !account.needs_login)
    else {
        return Vec::new();
    };
    let Some(usage) = account.usage.as_ref() else {
        return Vec::new();
    };
    let prefix = format!(
        "{}: {}",
        locale.text("默认账户", "Default account"),
        account.name
    );
    let problem = match accounts.error.as_deref() {
        Some("rate-limited") => Some(locale.text("限流，稍后重试", "Rate limited; retrying later")),
        Some("unsupported") => Some(unavailable),
        Some(_) => Some(locale.text("读取失败，保留上次数据", "Read failed; last known usage")),
        None if usage.stale => {
            Some(locale.text("上次记录，使用后更新", "Last known; updates after use"))
        }
        None => None,
    };
    [("5h", &usage.five_hour), ("Weekly", &usage.weekly)]
        .into_iter()
        .filter_map(|(label, window)| {
            let window = window.as_ref().filter(|window| window.used.is_finite())?;
            let percent = window.used.round();
            let (reset, detail) = claude_quota_reset(window.reset_timestamp(), now, locale);
            let mut tooltip = format!("{prefix} · {label} usage: {percent}% · {detail}");
            if let Some(problem) = problem {
                tooltip.push_str(" · ");
                tooltip.push_str(problem);
            }
            Some(QuotaSlot {
                usage: codex_hud::UsageWindow {
                    label: label.into(),
                    percent,
                    reset: reset.map(Into::into),
                },
                tooltip,
            })
        })
        .collect()
}

fn claude_quota_reset(
    reset: Option<u64>,
    now: u64,
    locale: InterfaceLocale,
) -> (Option<String>, String) {
    let Some(reset) = reset.filter(|reset| *reset != 0) else {
        return (
            None,
            locale.text("暂无重置时间", "Reset time unavailable").into(),
        );
    };
    if reset <= now {
        let (year, date, time) = local_date_time_with_year(reset);
        let text = match year {
            Some(year) if Some(year) != local_date_time_with_year(now).0 => {
                format!("{year}/{date} {time}")
            }
            _ => format!("{date} {time}"),
        };
        let detail = format!(
            "{}: {text} · {}",
            locale.text("上次记录的重置时间", "Last recorded reset"),
            locale.text(
                "已到重置时间，使用后更新",
                "Reset time reached; updates after use"
            ),
        );
        return (Some(text), detail);
    }
    let minutes = (reset - now).div_ceil(60_000);
    let hours = minutes / 60;
    let (mut text, remainder, unit) = match minutes {
        0..=59 => (format!("{minutes}m"), 0, 'm'),
        60..=1439 => (format!("{hours}h"), minutes % 60, 'm'),
        _ => (format!("{}d", hours / 24), hours % 24, 'h'),
    };
    if remainder != 0 {
        text.push_str(&format!("{remainder}{unit}"));
    }
    let detail = format!("{} {text}", locale.text("距离重置", "resets in"));
    (Some(text), detail)
}

pub(super) struct FooterPopover {
    pub(super) session: String,
    pub(super) menu: FooterMenu,
    cwd: Option<PathBuf>,
    request: Option<u64>,
    result: Option<Value>,
    error: Option<String>,
    git: Option<GitMenu>,
    _activation: Option<Subscription>,
}

use matou_native_runtime::host::{
    EnvironmentChangeResult, SessionEnvironmentReply, SessionEnvironmentTarget,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum EnvironmentAction {
    Restore,
    Locate,
    Local,
    Worktree,
}

pub(super) enum EnvironmentActionState {
    Picking,
    Pending {
        request: u64,
        action: EnvironmentAction,
    },
    Failed(String),
}

impl EnvironmentAction {
    fn id(self) -> &'static str {
        match self {
            Self::Restore => "environment-restore",
            Self::Locate => "environment-locate",
            Self::Local => "environment-handoff-local",
            Self::Worktree => "environment-handoff-worktree",
        }
    }

    fn label(self, locale: InterfaceLocale, overlay: bool) -> &'static str {
        match (self, overlay) {
            (Self::Restore, false) => {
                locale.text("恢复原 Worktree", "Restore the original worktree")
            }
            (Self::Restore, true) => locale.text("恢复 Worktree", "Restore worktree"),
            (Self::Locate, false) => {
                locale.text("定位已移动的 Worktree", "Locate the moved worktree")
            }
            (Self::Locate, true) => locale.text("定位目录", "Locate directory"),
            (Self::Local, _) => locale.text("交接到 Local", "Hand off to local"),
            (Self::Worktree, false) => {
                locale.text("交接到自有 Worktree", "Hand off to the owned worktree")
            }
            (Self::Worktree, true) => locale.text("交接到 Worktree", "Hand off to worktree"),
        }
    }

    fn busy(self, locale: InterfaceLocale) -> &'static str {
        match self {
            Self::Restore => {
                locale.text("正在恢复原 Worktree…", "Restoring the original worktree…")
            }
            Self::Locate => locale.text("正在定位 Worktree…", "Locating the worktree…"),
            Self::Local => locale.text("正在交接到 Local…", "Handing off to local…"),
            Self::Worktree => locale.text("正在交接到 Worktree…", "Handing off to the worktree…"),
        }
    }

    fn command(
        self,
        session_id: String,
        request_id: u64,
        path: Option<PathBuf>,
    ) -> Option<Command> {
        Some(match self {
            Self::Restore => Command::RestoreSessionEnvironment {
                session_id,
                request_id,
            },
            Self::Locate => Command::LocateSessionEnvironment {
                session_id,
                request_id,
                path: path?,
            },
            Self::Local | Self::Worktree => Command::HandoffSessionEnvironment {
                session_id,
                request_id,
                target: if self == Self::Local {
                    SessionEnvironmentTarget::Local
                } else {
                    SessionEnvironmentTarget::Worktree
                },
            },
        })
    }
}

fn environment_actions(
    environment: &Value,
    owns_worktree: bool,
    needs_recovery: bool,
    ready: bool,
) -> &'static [EnvironmentAction] {
    use EnvironmentAction::*;
    match (
        environment["kind"].as_str(),
        needs_recovery,
        ready,
        owns_worktree,
    ) {
        (Some("worktree"), true, _, true) => &[Restore, Locate, Local],
        (Some("worktree"), _, true, true) => &[Local],
        (Some("local"), _, true, true) | (Some("local"), true, _, true) => &[Worktree],
        _ => &[],
    }
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum GitView {
    #[default]
    Branches,
    CreateBranch,
    Worktrees,
    CreateWorktree,
    Commit,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GitAction {
    Status,
    Checkout,
    CreateBranch,
    Commit { push: bool },
    Push { after_commit: bool },
    CreateWorktree,
    RemoveWorktree,
}

struct GitMenu {
    status: Option<GitStatusDetail>,
    session_counts: BTreeMap<PathBuf, usize>,
    view: GitView,
    search: Entity<RenameInput>,
    back: FocusHandle,
    input: Option<Entity<RenameInput>>,
    base: Option<Entity<RenameInput>>,
    include_unstaged: bool,
    selected_branch: usize,
    branch_scroll: gpui::ScrollHandle,
    worktree_menu: Option<(PathBuf, Bounds<Pixels>)>,
    busy: Option<(u64, GitAction)>,
    notice: Option<&'static str>,
    _search_subscription: Subscription,
    input_subscription: Option<Subscription>,
    base_subscription: Option<Subscription>,
}

pub(super) struct FooterGitRequest {
    handle: AnyWindowHandle,
    session: String,
    cwd: PathBuf,
    action: GitAction,
}

pub(super) struct FooterGitProjection {
    cwd: PathBuf,
    request_id: u64,
    received: Instant,
    git: FooterGit,
}

impl GitMenu {
    fn branches<'a>(&'a self, query: &'a str) -> impl Iterator<Item = &'a LocalBranch> {
        self.status
            .iter()
            .flat_map(|status| &status.branches)
            .filter(move |branch| branch.name.to_lowercase().contains(query))
    }
}

impl Workspace {
    pub(super) fn footer_menu_trigger(
        &self,
        button: Stateful<Div>,
        menu: FooterMenu,
        popup: Option<&FooterPopover>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        self.footer_menu_trigger_with_stats(button, menu, popup, None, window, cx)
    }

    fn footer_menu_trigger_with_stats(
        &self,
        button: Stateful<Div>,
        menu: FooterMenu,
        popup: Option<&FooterPopover>,
        stats: Option<&codex_hud::StatsSummary>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let button = self.button(
            button.relative().cursor_pointer(),
            &self.control_focus[menu.id()],
            move |this, window, cx| this.toggle_footer_popover(menu, window, cx),
            cx,
        );
        if let Some(popup) = popup.filter(|popup| popup.menu == menu) {
            button.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .child(self.footer_popover(popup, stats, window, cx)),
            )
        } else {
            button
        }
    }

    pub(super) fn footer_stats(
        &self,
        stats: &codex_hud::StatsSummary,
        popup: Option<&FooterPopover>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        if stats.is_empty() {
            return None;
        }
        let title = codex_hud::stats_title(self.locale);
        let palette = self.theme_palette().footer;
        let button = div()
            .id("footer-stats")
            .flex_none()
            .size(px(20.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(5.))
            .line_height(px(16.8))
            .hover(move |style| {
                style
                    .bg(rgba(palette.breadcrumb_hover))
                    .text_color(rgba(palette.breadcrumb_hover_text))
            })
            .child(footer_text("stats", "⋯"))
            .children(footer_bounds_probe("stats"))
            .tooltip(move |_, cx| cx.new(|_| CardTooltip(title.into())).into());
        Some(self.footer_menu_trigger_with_stats(
            button,
            FooterMenu::Stats,
            popup,
            Some(stats),
            window,
            cx,
        ))
    }

    pub(super) fn footer_quota(
        &self,
        mode: &str,
        hud: &Value,
        popup: Option<&FooterPopover>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        let slots = match mode {
            "claude-code" => claude_quota_slots(
                hud,
                hud["defaultAccountUsage"] == true,
                self.claude_quota_state(),
                self.locale,
                notification_now(),
            ),
            "codex" | "antigravity" => codex_hud::usage_windows(hud, "usageWindows")
                .into_iter()
                .map(|usage| QuotaSlot {
                    tooltip: usage.summary(),
                    usage,
                })
                .collect(),
            _ => return None,
        };
        let usages: Vec<_> = slots.iter().map(|slot| &slot.usage).collect();
        let selected = codex_hud::footer_usage_windows(&usages);
        let first = usages[*selected.first()?];
        let palette = self.theme_palette().footer;
        let tooltip: SharedString = selected
            .iter()
            .map(|&index| slots[index].tooltip.as_str())
            .collect::<Vec<_>>()
            .join("\n")
            .into();
        let quota = div()
            .id("footer-quota")
            .flex_none()
            .flex()
            .items_center()
            .gap(px(4.))
            .whitespace_nowrap()
            .text_color(rgba(palette.usage))
            .children(footer_bounds_probe("quota"))
            .child(codex_hud::footer_quota_row(
                first,
                palette,
                "quota-first",
                "quota-first-percent",
            ))
            .children(selected.get(1).map(|&second| {
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .child(ui_text("·"))
                    .child(codex_hud::footer_quota_row(
                        usages[second],
                        palette,
                        "quota-second",
                        "quota-second-percent",
                    ))
            }))
            .tooltip(move |_, cx| cx.new(|_| CardTooltip(tooltip.clone())).into());
        Some(self.footer_menu_trigger(quota, FooterMenu::Quota, popup, window, cx))
    }

    fn toggle_footer_popover(
        &mut self,
        menu: FooterMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let handle = window.window_handle();
        if self
            .footer_popovers
            .get(&handle)
            .is_some_and(|popup| popup.menu == menu)
        {
            self.dismiss_footer_popover(window, cx);
            return;
        }
        let Some(session) = self.footer_session(window).map(str::to_owned) else {
            return;
        };
        let cwd = self.footer_cwd(&session);
        self.dismiss_permission_menu(window, cx);
        self.dismiss_hud_wiring_overlay(window, cx);
        let git = (menu == FooterMenu::Git).then(|| {
            let placeholder = self.locale.text("搜索 matou 分支", "Search Matou branches");
            let search = self.new_git_input(placeholder, false, cx);
            let subscription = cx.subscribe_in(
                &search,
                window,
                move |this, _, event, window, cx| match event {
                    RenameInputEvent::Changed => {
                        if let Some(git) = this.git_menu_mut(handle) {
                            git.selected_branch = 0;
                            git.branch_scroll.scroll_to_item(0);
                        }
                        cx.notify();
                    }
                    RenameInputEvent::Confirm => this.confirm_git_branch(window, cx),
                    _ => {}
                },
            );
            GitMenu {
                status: None,
                session_counts: BTreeMap::new(),
                view: GitView::Branches,
                search,
                back: cx.focus_handle().tab_stop(true),
                input: None,
                base: None,
                include_unstaged: true,
                selected_branch: 0,
                branch_scroll: gpui::ScrollHandle::new(),
                worktree_menu: None,
                busy: None,
                notice: None,
                _search_subscription: subscription,
                input_subscription: None,
                base_subscription: None,
            }
        });
        if let Some(git) = &git {
            window.focus(&git.search.read(cx).focus);
        } else {
            window.focus(&self.control_focus[menu.id()]);
        }
        self.footer_popovers.insert(
            handle,
            FooterPopover {
                session,
                menu,
                cwd,
                request: None,
                result: None,
                error: None,
                git,
                _activation: matches!(menu, FooterMenu::Quota | FooterMenu::Stats).then(|| {
                    cx.observe_window_activation(window, |this, window, cx| {
                        if !window.is_window_active() {
                            this.dismiss_footer_popover(window, cx);
                        }
                    })
                }),
            },
        );
        match menu {
            FooterMenu::Git => self.refresh_footer_git(window, cx),
            FooterMenu::Environment => self.request_footer_diagnostics(window, cx),
            FooterMenu::Quota | FooterMenu::Stats => {}
        }
        cx.notify();
    }

    pub(super) fn footer_cwd(&self, id: &str) -> Option<PathBuf> {
        let node = self.hud_node(id)?;
        ["environment", "worktree"]
            .into_iter()
            .find_map(|key| trimmed_graph_text(&node[key]["path"]))
            .or_else(|| trimmed_graph_text(&node["cwd"]))
            .or_else(|| {
                self.live_agent_hud(id)
                    .and_then(|hud| trimmed_graph_text(&hud["cwd"]))
            })
            .or_else(|| {
                (self.level_focus() == Some(id))
                    .then(|| {
                        self.hierarchy
                            .as_ref()
                            .and_then(|hierarchy| hierarchy.execution_context.as_ref())
                            .map(|context| context.cwd.as_str())
                    })
                    .flatten()
            })
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    }

    fn request_footer_diagnostics(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = window.window_handle();
        let request_id = self.next_wiring_request();
        let Some(popup) = self.footer_popovers.get_mut(&handle) else {
            return;
        };
        let Some(cwd) = popup.cwd.clone() else {
            return;
        };
        let command = match popup.menu {
            FooterMenu::Environment => Command::GetEnvironmentDiagnostics {
                request_id,
                cwd,
                repair: None,
            },
            _ => return,
        };
        popup.error = None;
        match self.commands.send_at(self.runtime_generation, command) {
            Ok(()) => popup.request = Some(request_id),
            Err(error) => {
                popup.request = None;
                popup.error = Some(error.to_string());
            }
        }
        cx.notify();
    }

    pub(super) fn receive_footer_diagnostics(
        &mut self,
        kind: &str,
        request_id: u64,
        reply: &Value,
        cx: &mut Context<Self>,
    ) {
        if kind != "environmentDiagnosticsResult" {
            return;
        }
        let Some(popup) = self
            .footer_popovers
            .values_mut()
            .find(|popup| popup.request == Some(request_id))
        else {
            return;
        };
        popup.request = None;
        popup.error = reply["error"].as_str().map(str::to_owned);
        popup.result = reply.get("result").cloned();
        cx.notify();
    }

    pub(super) fn environment_unavailable(&self, session: &str) -> bool {
        matches!(
            self.environment_actions.get(session),
            Some(EnvironmentActionState::Pending { .. })
        ) || self.hud_node(session).is_some_and(|node| {
            node["environment"]["state"]
                .as_str()
                .is_some_and(|state| state != "ready")
        })
    }

    fn environment_action_disabled(&self, session: &str) -> bool {
        !matches!(self.commands.connection(), (generation, ConnectionPhase::Ready)
            if generation == self.runtime_generation && self.domain_ready)
            || self.read_only.is_some()
            || self.storage_fault_sessions.contains(session)
            || !self
                .hud_node(session)
                .is_some_and(|node| node["archivedAt"].is_null())
            || matches!(
                self.environment_actions.get(session),
                Some(EnvironmentActionState::Picking | EnvironmentActionState::Pending { .. })
            )
            || self.hud_node(session).is_some_and(|node| {
                matches!(
                    node["environment"]["state"].as_str(),
                    Some("handoff" | "recovering")
                )
            })
    }

    fn run_environment_action(
        &mut self,
        session: &str,
        action: EnvironmentAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.environment_action_disabled(session) {
            return;
        }
        if action != EnvironmentAction::Locate {
            self.send_environment_action(session, action, None, cx);
            self.sync_terminal_input(window, cx);
            return;
        }
        #[cfg(debug_assertions)]
        if let Some(path) = self.environment_picker_path.take() {
            self.send_environment_action(session, action, Some(path), cx);
            self.sync_terminal_input(window, cx);
            return;
        }
        self.environment_actions
            .insert(session.to_owned(), EnvironmentActionState::Picking);
        let picker = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: None,
        });
        let generation = self.runtime_generation;
        let session = session.to_owned();
        cx.spawn_in(window, async move |this, cx| {
            let selected = picker.await;
            let _ = cx.update(|window, cx| {
                this.update(cx, |this, cx| {
                    if this.runtime_generation != generation
                        || !matches!(
                            this.environment_actions.get(&session),
                            Some(EnvironmentActionState::Picking)
                        )
                    {
                        return;
                    }
                    this.environment_actions.remove(&session);
                    if let Ok(Ok(Some(mut paths))) = selected
                        && let Some(path) = paths.pop()
                        && !this.environment_action_disabled(&session)
                    {
                        this.send_environment_action(&session, action, Some(path), cx);
                        this.sync_terminal_input(window, cx);
                    }
                    cx.notify();
                })
            });
        })
        .detach();
        cx.notify();
    }

    fn send_environment_action(
        &mut self,
        session: &str,
        action: EnvironmentAction,
        path: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let request = self.next_wiring_request();
        let Some(command) = action.command(session.to_owned(), request, path) else {
            return;
        };
        let state = match self.commands.send_at(self.runtime_generation, command) {
            Ok(()) => EnvironmentActionState::Pending { request, action },
            Err(error) => EnvironmentActionState::Failed(error.to_string()),
        };
        self.environment_actions.insert(session.to_owned(), state);
        cx.notify();
    }

    pub(super) fn receive_environment_change(
        &mut self,
        reply: SessionEnvironmentReply,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let session = &reply.session_id;
        if !matches!(self.environment_actions.get(session),
            Some(EnvironmentActionState::Pending { request, .. }) if *request == reply.request_id)
        {
            return;
        }
        self.environment_actions.remove(session);
        let close = |this: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
            if this
                .footer_popovers
                .get(&window.window_handle())
                .is_some_and(|popup| {
                    popup.menu == FooterMenu::Environment && &popup.session == session
                })
            {
                this.dismiss_footer_popover(window, cx);
            }
            this.footer_popovers.retain(|_, popup| {
                popup.menu != FooterMenu::Environment || &popup.session != session
            });
        };
        match reply.result {
            EnvironmentChangeResult::Ok { .. } => close(self, window, cx),
            EnvironmentChangeResult::Rejected { reason } => {
                self.environment_actions
                    .insert(session.clone(), EnvironmentActionState::Failed(reason));
            }
            EnvironmentChangeResult::SwitchSession {
                session_id,
                scene_id: Some(scene_id),
            } => {
                close(self, window, cx);
                self.send(
                    Command::FocusSession {
                        scene_id,
                        session_id,
                    },
                    cx,
                );
            }
            EnvironmentChangeResult::SwitchSession { scene_id: None, .. } => {
                self.environment_actions.insert(
                    session.clone(),
                    EnvironmentActionState::Failed("path-owned-by-another-session".into()),
                );
            }
        }
        self.sync_terminal_input(window, cx);
        self.refresh_live_wiring(cx);
        cx.notify();
    }

    fn environment_action_feedback(&self, session: &str) -> Option<(&str, bool)> {
        match self.environment_actions.get(session)? {
            EnvironmentActionState::Picking => None,
            EnvironmentActionState::Pending { action, .. } => {
                Some((action.busy(self.locale), false))
            }
            EnvironmentActionState::Failed(reason) => {
                Some((environment_warning(reason, self.locale), true))
            }
        }
    }

    pub(super) fn environment_card_overlay(
        &self,
        session: &str,
        node: &Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        if !node["archivedAt"].is_null() || !self.environment_unavailable(session) {
            return None;
        }
        let environment = &node["environment"];
        let locale = self.locale;
        let worktree = environment["kind"] == "worktree";
        let state = match self.environment_actions.get(session) {
            Some(EnvironmentActionState::Pending {
                action: EnvironmentAction::Local | EnvironmentAction::Worktree,
                ..
            }) => "handoff",
            Some(EnvironmentActionState::Pending { .. }) => "recovering",
            _ => environment["state"].as_str()?,
        };
        let (title, description) = match state {
            "recovering" => (locale.text("正在恢复运行环境", "Recovering the environment"), locale.text(
                "会话历史仍然保留，恢复完成后将自动重新进入终端。",
                "The session history is kept. The terminal reopens automatically once recovery finishes.")),
            "handoff" => (locale.text("正在交接运行环境", "Handing off the environment"), locale.text(
                "正在停止旧进程并进入目标目录，请稍候。",
                "Stopping the old process and moving into the target directory. This takes a moment.")),
            _ => (if state == "missing" {
                    locale.text("Worktree 需要恢复", "The worktree needs recovery")
                } else {
                    locale.text("运行环境需要处理", "The environment needs attention")
                },
                if worktree { locale.text(
                    "会话和历史仍然保留。恢复、定位原 Worktree，或交接到 Local 后可继续输入。",
                    "The session and its history are kept. Restore or locate the original worktree, or hand off to local, to keep typing.")
                } else { locale.text(
                    "会话和历史仍然保留。请先交接到可用环境后继续输入。",
                    "The session and its history are kept. Hand off to an available environment to keep typing.") }),
        };
        let dark = self.wiring_dark();
        let disabled = self.environment_action_disabled(session);
        let actions = environment_actions(
            environment,
            node["hasOwnedWorktree"] == true,
            matches!(state, "missing" | "failed"),
            false,
        );
        let focuses = window
            .use_keyed_state(
                SharedString::from(format!("environment-card-focus-{session}")),
                cx,
                |_, cx| [(); 3].map(|_| cx.focus_handle().tab_stop(true)),
            )
            .read(cx)
            .clone();
        Some(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .p(px(24.))
                .bg(rgba(if dark { 0x0d1117c7 } else { 0xf4f7fbcc }))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .w_full()
                        .max_w(px(420.))
                        .flex()
                        .flex_col()
                        .items_center()
                        .p(px(22.))
                        .gap(px(9.))
                        .rounded(px(12.))
                        .border_1()
                        .border_color(rgba(if dark { 0x7da1d14d } else { 0xc8d4e3ff }))
                        .bg(rgba(if dark { 0x181f29f0 } else { 0xfffffff5 }))
                        .text_color(rgba(if dark { 0xdbe6f5ff } else { 0x29364aff }))
                        .shadow(vec![shadow(
                            if dark { 0 } else { 0x1f2d3d },
                            if dark { 0.34 } else { 0.14 },
                            16.,
                            44.,
                        )])
                        .text_size(px(12.))
                        .text_center()
                        .child(
                            div()
                                .text_size(px(18.))
                                .text_color(rgb(if actions.is_empty() {
                                    0x79b8ff
                                } else {
                                    0xe4b269
                                }))
                                .child(if actions.is_empty() { "◌" } else { "◇" }),
                        )
                        .child(div().font_weight(FontWeight::BOLD).child(title))
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(rgb(if dark { 0x93a2b7 } else { 0x617086 }))
                                .child(description),
                        )
                        .when(!actions.is_empty(), |content| {
                            content.child(
                                div()
                                    .flex()
                                    .flex_wrap()
                                    .justify_center()
                                    .gap(px(7.))
                                    .children(actions.iter().zip(focuses.iter()).map(
                                        |(&action, focus)| {
                                            let id = match action {
                                                EnvironmentAction::Restore => {
                                                    "card-environment-restore"
                                                }
                                                EnvironmentAction::Locate => {
                                                    "card-environment-locate"
                                                }
                                                EnvironmentAction::Local => {
                                                    "card-environment-handoff-local"
                                                }
                                                EnvironmentAction::Worktree => {
                                                    "card-environment-handoff-worktree"
                                                }
                                            };
                                            let button = div()
                                                .relative()
                                                .id(id)
                                                .px(px(10.))
                                                .py(px(6.))
                                                .rounded(px(7.))
                                                .border_1()
                                                .border_color(rgb(if dark {
                                                    0x415269
                                                } else {
                                                    0xc5d1df
                                                }))
                                                .bg(rgb(if dark { 0x273447 } else { 0xf0f4f8 }))
                                                .text_size(px(11.))
                                                .child(action.label(locale, true));
                                            if disabled {
                                                button.opacity(0.5)
                                            } else {
                                                let session = session.to_owned();
                                                self.button(
                                                    button
                                                        .cursor_pointer()
                                                        .hover(|button| {
                                                            button.border_color(rgb(0x79b8ff))
                                                        })
                                                        .children(footer_bounds_probe(id)),
                                                    focus,
                                                    move |this, window, cx| {
                                                        this.run_environment_action(
                                                            &session, action, window, cx,
                                                        )
                                                    },
                                                    cx,
                                                )
                                            }
                                        },
                                    )),
                            )
                        })
                        .children(self.environment_action_feedback(session).map(
                            |(message, error)| {
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgb(if error { 0xf0aaa5 } else { 0x8fb8ed }))
                                    .child(message.to_owned())
                            },
                        )),
                ),
        )
    }

    fn git_menu_mut(&mut self, handle: AnyWindowHandle) -> Option<&mut GitMenu> {
        self.footer_popovers.get_mut(&handle)?.git.as_mut()
    }

    fn new_git_input(
        &self,
        placeholder: &str,
        multiline: bool,
        cx: &mut Context<Self>,
    ) -> Entity<RenameInput> {
        let colors = GitColors::new(self.theme.effective() == AppTheme::Dark);
        cx.new(|cx| {
            let mut input = if multiline {
                let focus = cx.focus_handle().tab_stop(true);
                RenameInput::instructions(String::new(), focus, cx)
            } else {
                RenameInput::new(String::new(), cx)
            };
            if !multiline {
                input.placeholder = placeholder.into();
                input.search_style = true;
                input.text_color = Some(rgba(colors.text));
                input.placeholder_color = Some(rgba(colors.muted));
            }
            input
        })
    }

    fn show_git_view(&mut self, view: GitView, window: &mut Window, cx: &mut Context<Self>) {
        let handle = window.window_handle();
        let placeholder = match view {
            GitView::CreateBranch => "feature/improve-git-menu",
            GitView::CreateWorktree => "feature/new-worktree",
            _ => "",
        };
        let input = matches!(
            view,
            GitView::Commit | GitView::CreateBranch | GitView::CreateWorktree
        )
        .then(|| self.new_git_input(placeholder, view == GitView::Commit, cx));
        let subscription = input.as_ref().map(|input| {
            cx.subscribe_in(
                input,
                window,
                move |this, _, event, window, cx| match event {
                    RenameInputEvent::Confirm => this.submit_git_form(false, window, cx),
                    RenameInputEvent::Changed => cx.notify(),
                    _ => {}
                },
            )
        });
        let Some(popup) = self.footer_popovers.get_mut(&handle) else {
            return;
        };
        let Some(git) = popup.git.as_mut() else {
            return;
        };
        popup.error = None;
        git.notice = None;
        git.view = view;
        git.worktree_menu = None;
        git.input = input;
        git.input_subscription = subscription;
        git.base = None;
        git.base_subscription = None;
        if let Some(input) = &git.input {
            window.focus(&input.read(cx).focus);
        } else if view == GitView::Branches {
            window.focus(&git.search.read(cx).focus);
        } else {
            window.focus(&git.back);
        }
        cx.notify();
    }

    fn edit_git_base(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let handle = window.window_handle();
        let placeholder = self
            .locale
            .text("分支、标签或提交", "Branch, tag, or commit");
        let input = self.new_git_input(placeholder, false, cx);
        let subscription = cx.subscribe_in(&input, window, |this, _, event, window, cx| {
            if matches!(event, RenameInputEvent::Confirm) {
                this.submit_git_form(false, window, cx);
            }
        });
        if let Some(git) = self.git_menu_mut(handle) {
            if git.base.is_none() && git.busy.is_none() {
                window.focus(&input.read(cx).focus);
                git.base = Some(input);
                git.base_subscription = Some(subscription);
                cx.notify();
            }
        }
    }

    fn move_git_branch_selection(
        &mut self,
        down: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(git) = self.git_menu_mut(window.window_handle()) {
            let query = git.search.read(cx).text.trim().to_lowercase();
            let last = git.branches(&query).count().saturating_sub(1);
            git.selected_branch = if down {
                (git.selected_branch + 1).min(last)
            } else {
                git.selected_branch.saturating_sub(1)
            };
            git.branch_scroll.scroll_to_item(git.selected_branch);
            cx.notify();
        }
    }

    fn confirm_git_branch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(git) = self.git_menu_mut(window.window_handle()) else {
            return;
        };
        if git.view != GitView::Branches {
            return;
        }
        let query = git.search.read(cx).text.trim().to_lowercase();
        let name = git
            .branches(&query)
            .nth(git.selected_branch)
            .filter(|branch| !branch.current && branch.checked_out_path.is_none())
            .map(|branch| branch.name.clone());
        if let Some(name) = name {
            self.checkout_git_branch(name, window, cx);
        }
    }

    fn checkout_git_branch(&mut self, branch: String, window: &mut Window, cx: &mut Context<Self>) {
        self.submit_footer_git(window, cx, GitAction::Checkout, |request_id, cwd| {
            Command::GitCheckout {
                request_id,
                cwd,
                branch,
            }
        });
    }

    fn refresh_footer_git(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.submit_footer_git(window, cx, GitAction::Status, |request_id, cwd| {
            Command::GitStatusDetail { request_id, cwd }
        });
    }

    fn submit_git_form(&mut self, push: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(git) = self.git_menu_mut(window.window_handle()) else {
            return;
        };
        let Some(input) = &git.input else { return };
        let text = input.read(cx).text.trim().to_owned();
        match git.view {
            GitView::CreateBranch if !text.is_empty() => {
                let start_point = git
                    .base
                    .as_ref()
                    .map(|input| input.read(cx).text.trim().to_owned())
                    .filter(|value| !value.is_empty());
                self.submit_footer_git(window, cx, GitAction::CreateBranch, |request_id, cwd| {
                    Command::GitCreateBranch {
                        request_id,
                        cwd,
                        name: text,
                        start_point,
                    }
                });
            }
            GitView::CreateWorktree if !text.is_empty() => {
                self.submit_footer_git(window, cx, GitAction::CreateWorktree, |request_id, cwd| {
                    Command::GitWorktreeCreate {
                        request_id,
                        cwd,
                        branch: text,
                        path: None,
                    }
                });
            }
            GitView::Commit => {
                let Some(status) = &git.status else { return };
                let stage_all = git.include_unstaged;
                if !can_footer_commit(status, stage_all) || (push && !status.has_remote) {
                    return;
                }
                let message = if text.is_empty() {
                    let count = status.dirty_file_count;
                    format!(
                        "chore: update {count} {}",
                        if count == 1 { "file" } else { "files" }
                    )
                } else {
                    text
                };
                self.submit_footer_git(
                    window,
                    cx,
                    GitAction::Commit { push },
                    |request_id, cwd| Command::GitCommit {
                        request_id,
                        cwd,
                        message,
                        stage_all,
                    },
                );
            }
            _ => {}
        }
    }

    fn submit_footer_git(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
        action: GitAction,
        command: impl FnOnce(u64, PathBuf) -> Command,
    ) {
        let handle = window.window_handle();
        let Some(popup) = self.footer_popovers.get(&handle) else {
            return;
        };
        if popup.git.as_ref().is_none_or(|git| git.busy.is_some())
            || self.footer_session(window) != Some(popup.session.as_str())
            || (action != GitAction::Status && self.read_only.is_some())
        {
            return;
        }
        let Some(cwd) = popup.cwd.clone() else {
            if let Some(popup) = self.footer_popovers.get_mut(&handle) {
                popup.error = Some(
                    self.locale
                        .text("当前目录不可用", "The current directory is unavailable")
                        .into(),
                );
            }
            cx.notify();
            return;
        };
        let request = FooterGitRequest {
            handle,
            session: popup.session.clone(),
            cwd,
            action,
        };
        self.send_footer_git(request, command, true, cx);
    }

    fn send_footer_git(
        &mut self,
        request: FooterGitRequest,
        command: impl FnOnce(u64, PathBuf) -> Command,
        show_in_popup: bool,
        cx: &mut Context<Self>,
    ) {
        let request_id = self.next_wiring_request();
        let result = self.commands.send_at(
            self.runtime_generation,
            command(request_id, request.cwd.clone()),
        );
        if show_in_popup
            && let Some(popup) = self.footer_popovers.get_mut(&request.handle)
            && let Some(git) = &mut popup.git
        {
            popup.error = result.as_ref().err().map(ToString::to_string);
            git.notice = None;
            git.worktree_menu = None;
            git.busy = result.as_ref().ok().map(|_| (request_id, request.action));
            if git.view == GitView::Commit
                && let Some(input) = &git.input
            {
                input.update(cx, |input, cx| {
                    input.set_document_read_only(git.busy.is_some(), cx)
                });
            }
        }
        if result.is_ok() {
            self.pending_footer_git.insert(request_id, request);
        }
        cx.notify();
    }

    pub(super) fn receive_footer_git(
        &mut self,
        reply: GitReply,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(mut request) = self.pending_footer_git.remove(&reply.request_id) else {
            return;
        };
        let visible = self
            .git_menu_mut(request.handle)
            .is_some_and(|git| git.busy == Some((reply.request_id, request.action)));
        let completed = reply.result.is_ok();
        if let Ok(status) = &reply.result {
            self.remember_footer_git(&request, reply.request_id, status);
        }
        if visible
            && let Some(popup) = self.footer_popovers.get_mut(&request.handle)
            && let Some(git) = &mut popup.git
        {
            git.busy = None;
            if git.view == GitView::Commit
                && let Some(input) = &git.input
            {
                input.update(cx, |input, cx| {
                    input.set_document_read_only(false, cx);
                    if completed && matches!(request.action, GitAction::Commit { .. }) {
                        input.clear(cx);
                    }
                });
            }
            match reply.result {
                Ok(status) => {
                    git.status = Some(status);
                    git.session_counts = reply.worktree_session_counts;
                    git.notice = request.action.notice(self.locale);
                }
                Err(error) => {
                    popup.error = Some(localized_git_error(self.locale, request.action, &error))
                }
            }
        }
        if completed {
            match request.action {
                GitAction::Commit { push: true } => {
                    request.action = GitAction::Push { after_commit: true };
                    self.send_footer_git(
                        request,
                        |request_id, cwd| Command::GitPush { request_id, cwd },
                        visible,
                        cx,
                    );
                }
                GitAction::Checkout | GitAction::CreateBranch if visible => {
                    if window.window_handle() == request.handle {
                        self.dismiss_footer_popover(window, cx);
                    } else {
                        self.footer_popovers.remove(&request.handle);
                        let focus = self.control_focus[FooterMenu::Git.id()].clone();
                        cx.defer(move |cx| {
                            let _ = request
                                .handle
                                .update(cx, |_, window, _| window.focus(&focus));
                        });
                    }
                }
                GitAction::CreateWorktree if visible => {
                    if let Some(git) = self.git_menu_mut(request.handle)
                        && git.view == GitView::CreateWorktree
                    {
                        git.view = GitView::Worktrees;
                        git.input = None;
                        git.input_subscription = None;
                        let focus = git.back.clone();
                        if window.window_handle() == request.handle {
                            window.focus(&focus);
                        } else {
                            cx.defer(move |cx| {
                                let _ = request
                                    .handle
                                    .update(cx, |_, window, _| window.focus(&focus));
                            });
                        }
                    }
                }
                _ => {}
            }
        }
        cx.notify();
    }

    fn remember_footer_git(
        &mut self,
        request: &FooterGitRequest,
        request_id: u64,
        status: &GitStatusDetail,
    ) {
        let node = serde_json::json!({"git": {
            "state": "ready", "branch": status.current_branch, "detachedHead": status.detached_head,
            "root": status.repository_root, "dirty": status.dirty
        }});
        let Some(git) = FooterGit::projected(&node, None, self.locale) else {
            return;
        };
        let entry = self
            .footer_git_projections
            .entry(request.session.clone())
            .or_insert_with(|| FooterGitProjection {
                cwd: request.cwd.clone(),
                request_id: 0,
                received: Instant::now(),
                git: git.clone(),
            });
        if entry.request_id <= request_id {
            *entry = FooterGitProjection {
                cwd: request.cwd.clone(),
                request_id,
                received: Instant::now(),
                git,
            };
        }
    }

    pub(super) fn footer_git_projection(&self, session: &str) -> Option<FooterGit> {
        let projection = self.footer_git_projections.get(session)?;
        (self.footer_cwd(session).as_ref() == Some(&projection.cwd)).then(|| projection.git.clone())
    }

    pub(super) fn release_footer_git_projection(
        &mut self,
        session: &str,
        request_id: u64,
        reply: &Value,
    ) {
        if reply["result"]["hud"]["gitDirty"].is_boolean()
            && self
                .footer_git_projections
                .get(session)
                .is_some_and(|entry| {
                    request_id > entry.request_id
                        && entry.received.elapsed() >= Duration::from_secs(2)
                })
        {
            self.footer_git_projections.remove(session);
        }
    }

    pub(super) fn retire_footer_git(&mut self, cx: &mut Context<Self>) {
        self.pending_footer_git.clear();
        self.footer_git_projections.clear();
        for popup in self.footer_popovers.values_mut() {
            if let Some(git) = &mut popup.git
                && git.busy.take().is_some()
            {
                popup.error = Some(
                    self.locale
                        .text(
                            "连接已断开，请刷新仓库状态",
                            "Connection lost. Refresh the repository status.",
                        )
                        .into(),
                );
                if git.view == GitView::Commit
                    && let Some(input) = &git.input
                {
                    input.update(cx, |input, cx| input.set_document_read_only(false, cx));
                }
            }
        }
    }

    pub(super) fn dismiss_footer_popover(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(popup) = self.footer_popovers.remove(&window.window_handle()) {
            if self.footer_session(window) == Some(popup.session.as_str()) {
                window.focus(&self.control_focus[popup.menu.id()]);
            }
            cx.notify();
        }
    }

    pub(super) fn footer_popover_key(
        &mut self,
        stroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(popup) = self.footer_popovers.get(&window.window_handle()) else {
            return false;
        };
        if self.footer_session(window) != Some(popup.session.as_str()) {
            self.dismiss_footer_popover(window, cx);
            return false;
        }
        if stroke.key == "escape" {
            if let Some(git) = &popup.git {
                let view = git.view;
                if self
                    .git_menu_mut(window.window_handle())
                    .is_some_and(|git| git.worktree_menu.take().is_some())
                {
                    cx.notify();
                    return true;
                }
                if view != GitView::Branches {
                    self.show_git_view(
                        if view == GitView::CreateWorktree {
                            GitView::Worktrees
                        } else {
                            GitView::Branches
                        },
                        window,
                        cx,
                    );
                    return true;
                }
            }
            self.dismiss_footer_popover(window, cx);
            return true;
        }
        if stroke.key == "tab"
            && matches!(popup.menu, FooterMenu::Git | FooterMenu::Environment)
            && !stroke.modifiers.alt
            && !stroke.modifiers.control
            && !stroke.modifiers.platform
        {
            if stroke.modifiers.shift {
                window.focus_prev();
            } else {
                window.focus_next();
            }
            return true;
        }
        if let Some(git) = &popup.git {
            if git.view == GitView::Branches && git.search.read(cx).focus.is_focused(window) {
                if matches!(stroke.key.as_str(), "up" | "down") {
                    self.move_git_branch_selection(stroke.key == "down", window, cx);
                    return true;
                }
                return false;
            }
            if [git.input.as_ref(), git.base.as_ref()]
                .into_iter()
                .flatten()
                .any(|input| input.read(cx).focus.is_focused(window))
            {
                return git.busy.is_some();
            }
        }
        !matches!(stroke.key.as_str(), "enter" | "space") && stroke.key != "tab"
    }

    pub(super) fn footer_popover_overlay(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let viewport = window.viewport_size();
        anchored()
            .anchor(Corner::TopLeft)
            .position(point(px(0.), px(0.)))
            .child(
                div()
                    .id("footer-popover-backdrop")
                    .w(viewport.width)
                    .h(viewport.height)
                    .occlude()
                    .on_any_mouse_down(cx.listener(|this, _, window, cx| {
                        this.dismiss_footer_popover(window, cx);
                        cx.stop_propagation();
                    })),
            )
    }

    fn footer_popover(
        &self,
        popup: &FooterPopover,
        stats: Option<&codex_hud::StatsSummary>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let dark = self.theme.effective() == AppTheme::Dark;
        let viewport = window.viewport_size();
        let (id, panel) = match popup.menu {
            FooterMenu::Environment => (
                "footer-environment-menu",
                self.environment_panel(popup, window, cx, dark),
            ),
            FooterMenu::Git => ("footer-git-menu", self.git_panel(popup, window, cx, dark)),
            FooterMenu::Stats => (
                "footer-stats-menu",
                codex_hud::stats_panel(
                    stats.expect("stats trigger supplies the summary"),
                    self.locale,
                    dark,
                ),
            ),
            FooterMenu::Quota => {
                let node = self.hud_node(&popup.session).unwrap_or(&Value::Null);
                let live = self
                    .live_agent_hud(&popup.session)
                    .unwrap_or(&node["codexHud"]);
                let mut usage = codex_hud::usage_windows(live, "allUsageWindows");
                if usage.is_empty() {
                    usage = codex_hud::usage_windows(live, "usageWindows");
                }
                if usage.is_empty() && node["currentMode"] == "claude-code" {
                    usage = claude_quota_slots(
                        live,
                        live["defaultAccountUsage"] == true,
                        self.claude_quota_state(),
                        self.locale,
                        notification_now(),
                    )
                    .into_iter()
                    .map(|slot| slot.usage)
                    .collect();
                }
                let palette = self.theme_palette().workspace;
                (
                    "footer-quota-menu",
                    div()
                        .flex()
                        .flex_col()
                        .min_w(px(200.))
                        .max_w(px(335.))
                        .p(px(6.))
                        .border_1()
                        .border_color(rgba(palette.line))
                        .rounded(px(8.))
                        .bg(rgba(palette.panel))
                        .text_color(rgba(palette.text))
                        .text_size(px(12.))
                        .shadow(vec![shadow(0x000000, 0.24, 8., 24.)])
                        .child(
                            div()
                                .px(px(8.))
                                .py(px(5.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(ui_text(self.locale.text("配额明细", "Quota details"))),
                        )
                        .children(usage.iter().map(|usage| {
                            div()
                                .px(px(8.))
                                .py(px(5.))
                                .child(codex_hud::quota_row(usage, dark))
                        })),
                )
            }
        };
        let stats_menu = popup.menu == FooterMenu::Stats;
        let panel = div()
            .relative()
            .when(stats_menu, |panel| {
                panel.max_w((viewport.width - px(32.)).max(px(0.)))
            })
            .occlude()
            .child(panel)
            .children(footer_bounds_probe(id));
        let quota = popup.menu == FooterMenu::Quota;
        let anchor = if quota {
            anchored()
                .anchor(Corner::BottomLeft)
                .position_mode(AnchoredPositionMode::Local)
                .position(point(px(0.), px(0.)))
                .offset(point(px(0.), px(-PERMISSION_MENU_GAP)))
        } else {
            anchored().anchor(Corner::BottomRight).position(point(
                viewport.width - px(16.),
                viewport.height - px(if stats_menu { 46. } else { 44. }),
            ))
        };
        deferred(anchor.snap_to_window().child(panel)).with_priority(2)
    }

    fn environment_panel(
        &self,
        popup: &FooterPopover,
        window: &mut Window,
        cx: &mut Context<Self>,
        dark: bool,
    ) -> Div {
        let locale = self.locale;
        let viewport = window.viewport_size();
        let result = popup.result.as_ref().unwrap_or(&Value::Null);
        let node = self.hud_node(&popup.session).unwrap_or(&Value::Null);
        let environment = node
            .get("environment")
            .filter(|environment| environment.is_object())
            .or_else(|| {
                self.live_agent_hud(&popup.session)
                    .and_then(|hud| hud.get("environment"))
            })
            .unwrap_or(&Value::Null);
        let fields = self.footer_fields_for(&popup.session);
        let title = environment["state"]
            .as_str()
            .and_then(|state| match state {
                "missing" => Some(locale.text("待恢复", "Needs recovery")),
                "recovering" => Some(locale.text("恢复中", "Recovering")),
                "handoff" => Some(locale.text("交接中", "Handing off")),
                "failed" => Some(locale.text("待处理", "Needs attention")),
                _ => None,
            })
            .or_else(|| {
                fields
                    .as_ref()
                    .and_then(|fields| fields.environment.as_ref())
                    .map(|env| env.label)
            })
            .unwrap_or("Local");
        let ready = popup.cwd.is_some()
            && !matches!(
                self.environment_actions.get(&popup.session),
                Some(EnvironmentActionState::Pending { .. })
            )
            && [environment["state"].as_str(), result["state"].as_str()]
                .into_iter()
                .flatten()
                .all(|state| state == "ready");
        let warning = environment["error"]
            .as_str()
            .map(|reason| environment_warning(reason, locale))
            .or_else(|| match result["state"].as_str() {
                Some("missing") => Some(locale.text("目录不存在", "Directory missing")),
                Some("unavailable") => Some(locale.text("环境不可用", "Environment unavailable")),
                _ => None,
            });
        let (bg, border, divider, text, path_bg, button_bg) = if dark {
            (
                0x161b22ff, 0x30363dff, 0x30363dff, 0xc9d1d9ff, 0x0d1117ff, 0x21262dff,
            )
        } else {
            (
                0xffffffff, 0xd0d7deff, 0xd8dee4ff, 0x24292fff, 0xf6f8faff, 0xf6f8faff,
            )
        };
        let focuses = window
            .use_keyed_state("environment-menu-focus", cx, |_, cx| {
                [(); 6].map(|_| cx.focus_handle().tab_stop(true))
            })
            .read(cx)
            .clone();
        let path = popup
            .cwd
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "—".into());
        let close = self.button(
            div()
                .id("hud-menu-environment-close")
                .size(px(28.))
                .rounded(px(6.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .text_size(px(18.))
                .text_color(rgba(0x8b949eff))
                .child(ui_text("×")),
            &focuses[0],
            Self::dismiss_footer_popover,
            cx,
        );
        let mut actions = div().flex().gap(px(7.));
        let finder = if cfg!(target_os = "macos") {
            locale.text("在 Finder 中显示", "Reveal in Finder")
        } else {
            locale.text("在文件管理器中显示", "Reveal in File Manager")
        };
        for (index, (id, label, action)) in [
            ("hud-menu-reveal", finder, PathAction::Reveal),
            (
                "hud-menu-terminal",
                locale.text("在系统终端中打开", "Open in the system terminal"),
                PathAction::Terminal,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            let button = div()
                .id(id)
                .flex_1()
                .min_w_0()
                .relative()
                .flex()
                .items_center()
                .justify_center()
                .px(px(10.))
                .py(px(7.))
                .rounded(px(7.))
                .border_1()
                .border_color(rgba(border))
                .bg(rgba(button_bg))
                .child(ui_text(label))
                .children(footer_bounds_probe(id));
            actions = actions.child(if let Some(path) = popup.cwd.clone().filter(|_| ready) {
                self.button(
                    button
                        .cursor_pointer()
                        .hover(|style| style.border_color(rgba(0x58a6ffff))),
                    &focuses[index + 1],
                    move |this, window, cx| this.launch_footer_path(&path, action, window, cx),
                    cx,
                )
            } else {
                button.opacity(0.45)
            });
        }
        let needs_recovery = matches!(environment["state"].as_str(), Some("missing" | "failed"))
            || matches!(result["state"].as_str(), Some("missing" | "unavailable"));
        let actions_for_environment = environment_actions(
            environment,
            node["hasOwnedWorktree"] == true,
            needs_recovery,
            ready,
        );
        let disabled = self.environment_action_disabled(&popup.session);
        let recovery = (!actions_for_environment.is_empty()).then(|| {
            div().flex().flex_col().gap(px(7.)).children(
                actions_for_environment
                    .iter()
                    .zip(&focuses[3..])
                    .map(|(&action, focus)| {
                        let button = div()
                            .relative()
                            .id(action.id())
                            .px(px(10.))
                            .py(px(7.))
                            .rounded(px(7.))
                            .border_1()
                            .border_color(rgba(border))
                            .bg(rgba(button_bg))
                            .text_center()
                            .child(ui_text(action.label(locale, false)));
                        if disabled {
                            button.opacity(0.45)
                        } else {
                            let session = popup.session.clone();
                            self.button(
                                button
                                    .cursor_pointer()
                                    .hover(|style| style.border_color(rgba(0x58a6ffff)))
                                    .children(footer_bounds_probe(action.id())),
                                focus,
                                move |this, window, cx| {
                                    this.run_environment_action(&session, action, window, cx)
                                },
                                cx,
                            )
                        }
                    }),
            )
        });
        div()
            .w((viewport.width - px(32.)).min(px(360.)).max(px(0.)))
            .flex()
            .flex_col()
            .rounded(px(12.))
            .border_1()
            .border_color(rgba(border))
            .bg(rgba(bg))
            .text_color(rgba(text))
            .text_size(px(12.))
            .line_height(px(16.8))
            .shadow(vec![shadow(
                if dark { 0 } else { 0x1f2328 },
                if dark { 0.48 } else { 0.2 },
                14.,
                40.,
            )])
            .child(
                div()
                    .min_h(px(44.))
                    .px(px(12.))
                    .border_b_1()
                    .border_color(rgba(divider))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .relative()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .children(footer_bounds_probe("hud-menu-environment-title"))
                            .child(div().text_color(rgba(0x79c0ffff)).child(ui_text("▣")))
                            .child(
                                div()
                                    .font_weight(FontWeight::BOLD)
                                    .child(footer_text("environment-menu-title", title)),
                            ),
                    )
                    .child(close),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .p(px(12.))
                    .gap(px(10.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .line_height(px(14.))
                                    .text_color(rgba(0x8b949eff))
                                    .child(ui_text(locale.text("当前路径", "Current path"))),
                            )
                            .child(
                                div()
                                    .relative()
                                    .id("environment-path-code")
                                    .p(px(8.))
                                    .rounded(px(7.))
                                    .bg(rgba(path_bg))
                                    .text_size(px(10.))
                                    .line_height(px(14.))
                                    .font_family("Menlo")
                                    .whitespace_nowrap()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .children(footer_bounds_probe("hud-menu-environment-path"))
                                    .child(footer_text("environment-menu-path", path.clone()))
                                    .tooltip(move |_, cx| {
                                        cx.new(|_| CardTooltip(path.clone().into())).into()
                                    }),
                            ),
                    )
                    .children(warning.map(|warning| {
                        div()
                            .p(px(8.))
                            .border_1()
                            .rounded(px(7.))
                            .border_color(rgba(0xd299224d))
                            .bg(rgba(0xd2992214))
                            .text_color(rgba(if dark { 0xe3b866ff } else { 0x8b5a12ff }))
                            .child(ui_text(warning.to_owned()))
                    }))
                    .child(actions)
                    .children(recovery),
            )
            .children(
                self.environment_action_feedback(&popup.session)
                    .or_else(|| popup.error.as_deref().map(|error| (error, true)))
                    .map(|(feedback, error)| {
                        div()
                            .min_h(px(34.))
                            .px(px(12.))
                            .py(px(8.))
                            .border_t_1()
                            .border_color(rgba(divider))
                            .bg(rgba(path_bg))
                            .text_color(rgba(match (error, dark) {
                                (true, true) => 0xf0aaa5ff,
                                (true, false) => 0xab3d37ff,
                                (false, true) => 0x8fb8edff,
                                (false, false) => 0x234f86ff,
                            }))
                            .child(footer_text(
                                "environment-menu-feedback",
                                feedback.to_owned(),
                            ))
                    }),
            )
    }

    fn git_panel(
        &self,
        popup: &FooterPopover,
        window: &mut Window,
        cx: &mut Context<Self>,
        dark: bool,
    ) -> Div {
        let git = popup.git.as_ref().expect("Git popover state");
        let colors = GitColors::new(dark);
        let viewport = window.viewport_size();
        let view = match (git.view, &git.status) {
            (GitView::Branches, _) | (_, None) => self.git_branches(git, colors, window, cx),
            (GitView::CreateBranch | GitView::CreateWorktree, Some(status)) => {
                self.git_create(git, status, colors, window, cx)
            }
            (GitView::Worktrees, Some(status)) => {
                self.git_worktrees(git, status, colors, window, cx)
            }
            (GitView::Commit, Some(status)) => {
                self.git_commit_panel(git, status, colors, window, cx)
            }
        };
        let feedback = popup
            .error
            .as_deref()
            .or_else(|| git.busy.map(|(_, action)| action.pending(self.locale)))
            .or(git.notice);
        div()
            .w((viewport.width - px(28.)).min(px(316.)).max(px(0.)))
            .max_h((viewport.height - px(66.)).min(px(390.)).max(px(0.)))
            .overflow_hidden()
            .flex()
            .flex_col()
            .border_1()
            .border_color(rgba(colors.line))
            .rounded(px(17.))
            .bg(rgba(colors.panel))
            .text_color(rgba(colors.text))
            .text_size(px(12.))
            .line_height(px(16.8))
            .shadow(if dark {
                vec![shadow(0, 0.38, 16., 44.), shadow(0, 0.22, 2., 8.)]
            } else {
                vec![
                    shadow(0x171c23, 0.20, 16., 44.),
                    shadow(0x171c23, 0.10, 2., 8.),
                ]
            })
            .child(div().min_h_0().pt(px(7.)).px(px(8.)).pb(px(8.)).child(view))
            .children(feedback.map(|feedback| {
                let refresh = self.git_button(
                    "hud-menu-git-refresh",
                    git_label(self.locale.text("刷新", "Refresh")),
                    true,
                    colors,
                    window,
                    cx,
                    Self::refresh_footer_git,
                );
                div()
                    .flex_none()
                    .min_h(px(34.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .border_t_1()
                    .border_color(rgba(colors.line))
                    .px(px(10.))
                    .py(px(6.))
                    .bg(rgba(colors.field))
                    .text_color(rgba(if popup.error.is_some() {
                        if dark { 0xf0877fff } else { 0xa9322aff }
                    } else if dark {
                        0x83ade6ff
                    } else {
                        0x3973bdff
                    }))
                    .children(footer_bounds_probe("hud-menu-git-feedback"))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .max_h(px(68.))
                            .overflow_hidden()
                            .child(footer_text("git-menu-feedback", feedback.to_owned())),
                    )
                    .children(git.busy.is_none().then_some(refresh.h(px(22.)).px(px(7.))))
            }))
    }

    fn git_branches(
        &self,
        git: &GitMenu,
        colors: GitColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let locale = self.locale;
        let query = git.search.read(cx).text.trim().to_lowercase();
        let focused = git.search.read(cx).focus.is_focused(window);
        let search = div()
            .id("hud-menu-git-search")
            .flex_none()
            .flex()
            .items_center()
            .h(px(37.))
            .rounded(px(9.))
            .border_2()
            .border_color(rgba(if focused { 0x5b91e8b8 } else { 0 }))
            .bg(rgba(if focused { colors.field } else { 0 }))
            .pl(px(9.))
            .pr(px(6.))
            .gap(px(8.))
            .text_size(px(13.))
            .children(footer_bounds_probe("hud-menu-git-search"))
            .child(git_icon("icons/search.svg", colors.muted))
            .child(div().flex_1().min_w_0().child(git.search.clone()))
            .children((!git.search.read(cx).text.is_empty()).then(|| {
                let input = git.search.clone();
                div()
                    .id("hud-menu-git-clear")
                    .size(px(25.))
                    .rounded(px(6.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(move |style| style.bg(rgba(colors.hover)))
                    .child(git_icon("icons/x.svg", colors.muted))
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        input.update(cx, |input, cx| input.clear(cx));
                        window.focus(&input.read(cx).focus);
                    })
            }));
        let mut view = div().min_h_0().flex().flex_col().child(search);
        let Some(status) = &git.status else {
            return view;
        };
        let pending = status.dirty_file_count;
        let visible: Vec<_> = git.branches(&query).collect();
        let mut list = div()
            .id("hud-menu-git-branches")
            .min_h_0()
            .max_h(px(190.))
            .overflow_y_scroll()
            .track_scroll(&git.branch_scroll)
            .px(px(1.))
            .pb(px(5.))
            .flex()
            .flex_col()
            .when(visible.is_empty(), |list| {
                list.child(
                    div()
                        .pt(px(25.))
                        .pb(px(30.))
                        .px(px(12.))
                        .text_center()
                        .text_color(rgba(colors.muted))
                        .child(ui_text(
                            locale.text("没有匹配的分支", "No matching branches"),
                        )),
                )
            });
        for (index, branch) in visible.into_iter().enumerate() {
            let summary = if branch.current && pending > 0 {
                Some(uncommitted_files(locale, pending))
            } else if !branch.current && branch.checked_out_path.is_some() {
                Some(
                    locale
                        .text("已在 Worktree 中", "Already in a worktree")
                        .into(),
                )
            } else {
                None
            };
            let id: SharedString = format!("hud-git-branch-{}", branch.name).into();
            let name = branch.name.clone();
            let row = div()
                .id(id.clone())
                .min_h(px(45.))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(5.))
                .px(px(8.))
                .py(px(5.))
                .rounded(px(10.))
                .when(index == git.selected_branch, |row| {
                    row.bg(rgba(colors.hover))
                })
                .children(footer_bounds_probe(if branch.current {
                    "hud-menu-git-branch-current"
                } else {
                    "hud-menu-git-branch-other"
                }))
                .child(git_icon("icons/git-branch.svg", colors.muted))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(1.))
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_weight(FontWeight::MEDIUM)
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(footer_text(
                                    if branch.current {
                                        "git-menu-current"
                                    } else {
                                        "git-menu-other"
                                    },
                                    name.clone(),
                                )),
                        )
                        .children(summary.map(|summary| {
                            div()
                                .text_size(px(11.))
                                .text_color(rgba(colors.muted))
                                .whitespace_nowrap()
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(ui_text(summary))
                        })),
                )
                .child(
                    div()
                        .w(px(22.))
                        .text_right()
                        .text_color(rgba(colors.muted))
                        .children(branch.current.then(|| ui_text("✓"))),
                )
                .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                    if *hovered && let Some(git) = this.git_menu_mut(window.window_handle()) {
                        git.selected_branch = index;
                        cx.notify();
                    }
                }));
            let enabled = self.read_only.is_none()
                && git.busy.is_none()
                && (branch.current || branch.checked_out_path.is_none());
            let focus = git_focus(window, cx, id);
            let row = if enabled {
                self.button(
                    row.cursor_pointer()
                        .hover(move |style| style.bg(rgba(colors.hover))),
                    &focus,
                    move |this, window, cx| {
                        if this
                            .git_menu_mut(window.window_handle())
                            .is_some_and(|git| {
                                git.status.as_ref().is_some_and(|status| {
                                    status.current_branch.as_deref() != Some(&name)
                                })
                            })
                        {
                            this.checkout_git_branch(name.clone(), window, cx);
                        }
                    },
                    cx,
                )
            } else {
                row.opacity(0.4)
            };
            list = list.child(row);
        }
        let worktrees = status
            .worktrees
            .iter()
            .filter(|worktree| !worktree.main)
            .count();
        let actions = [
            (
                "hud-menu-git-create-branch",
                "icons/plus.svg",
                locale.text("创建并检出新分支…", "Create and check out a branch…"),
                0,
                GitView::CreateBranch,
            ),
            (
                "hud-menu-git-worktrees",
                "icons/network.svg",
                locale.text("管理 Worktree…", "Manage Worktrees…"),
                worktrees,
                GitView::Worktrees,
            ),
            (
                "hud-menu-git-commit-view",
                "icons/git-commit-horizontal.svg",
                locale.text("提交与推送…", "Commit and push…"),
                pending,
                GitView::Commit,
            ),
        ];
        let mut action_rows = div()
            .flex_none()
            .pt(px(5.))
            .border_t_1()
            .border_color(rgba(colors.line));
        for (id, icon, label, count, target) in actions {
            action_rows = action_rows.child(self.git_button(
                id,
                git_action_copy(icon, label, colors).children((count > 0).then(|| {
                    div()
                        .text_size(px(11.))
                        .text_color(rgba(colors.muted))
                        .child(ui_text(count.to_string()))
                })),
                git.busy.is_none(),
                colors,
                window,
                cx,
                move |this, window, cx| this.show_git_view(target, window, cx),
            ));
        }
        view = view.child(
            div()
                .flex_none()
                .px(px(10.))
                .pt(px(5.))
                .pb(px(6.))
                .text_color(rgba(colors.muted))
                .child(ui_text(locale.text("分支", "Branches"))),
        );
        view.child(list).child(action_rows)
    }

    fn git_button(
        &self,
        id: &'static str,
        copy: Div,
        enabled: bool,
        colors: GitColors,
        window: &mut Window,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        let row = div()
            .id(id)
            .relative()
            .flex_none()
            .flex()
            .items_center()
            .h(px(36.))
            .px(px(10.))
            .rounded(px(9.))
            .children(footer_bounds_probe(id))
            .child(copy);
        if enabled {
            let focus = git_focus(window, cx, id);
            self.button(
                row.cursor_pointer()
                    .hover(move |style| style.bg(rgba(colors.hover))),
                &focus,
                action,
                cx,
            )
        } else {
            row.opacity(0.4).on_click(|_, _, cx| cx.stop_propagation())
        }
    }

    fn git_subview_header(
        &self,
        title: &'static str,
        focus: &FocusHandle,
        back: GitView,
        colors: GitColors,
        cx: &mut Context<Self>,
    ) -> Div {
        let back_label = if back == GitView::Worktrees {
            self.locale.text("返回 Worktree 列表", "Back to Worktrees")
        } else {
            self.locale.text("返回分支列表", "Back to branches")
        };
        div()
            .h(px(36.))
            .flex_none()
            .flex()
            .items_center()
            .child(
                self.button(
                    div()
                        .id("hud-menu-git-back")
                        .size(px(30.))
                        .relative()
                        .rounded(px(8.))
                        .cursor_pointer()
                        .hover(move |style| style.bg(rgba(colors.hover)))
                        .children(footer_bounds_probe("hud-menu-git-back"))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(22.))
                        .text_color(rgba(colors.muted))
                        .child(ui_text("‹")),
                    focus,
                    move |this, window, cx| this.show_git_view(back, window, cx),
                    cx,
                )
                .tooltip(move |_, cx| cx.new(|_| CardTooltip(back_label.into())).into()),
            )
            .child(
                div()
                    .flex_1()
                    .text_center()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(footer_text("git-menu-title", title)),
            )
            .child(div().w(px(30.)))
    }

    fn git_create(
        &self,
        git: &GitMenu,
        status: &GitStatusDetail,
        colors: GitColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let locale = self.locale;
        let worktree = git.view == GitView::CreateWorktree;
        let (title, label, submit, back) = if worktree {
            (
                locale.text("创建 Worktree", "Create Worktree"),
                locale.text("新 Worktree 分支", "New Worktree branch"),
                locale.text("创建", "Create"),
                GitView::Worktrees,
            )
        } else {
            (
                locale.text("创建新分支", "Create a branch"),
                locale.text("分支名称", "Branch name"),
                locale.text("创建并检出", "Create and check out"),
                GitView::Branches,
            )
        };
        let input = git.input.as_ref().expect("Git create form input");
        let writable = self.read_only.is_none() && git.busy.is_none();
        let base_row = if worktree {
            div()
                .flex()
                .items_center()
                .gap(px(7.))
                .px(px(10.))
                .py(px(9.))
                .rounded(px(8.))
                .bg(rgba(colors.field))
                .child(git_icon("icons/network.svg", colors.muted))
                .child(ui_text(locale.text(
                    "创建在 Matou Worktree 目录",
                    "Creates inside the Matou Worktree directory",
                )))
                .into_any_element()
        } else if let Some(input) = &git.base {
            div()
                .flex()
                .flex_col()
                .gap(px(5.))
                .child(ui_text(
                    locale.text("基分支（可选）", "Start point (optional)"),
                ))
                .child(git_form_field(
                    "hud-menu-git-base-input",
                    input,
                    colors,
                    window,
                    cx,
                ))
                .into_any_element()
        } else {
            let current = status.current_branch.as_deref().unwrap_or("HEAD");
            self.git_button(
                "hud-menu-git-base",
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .w_full()
                    .child(git_icon("icons/git-branch.svg", colors.muted))
                    .child(ui_text(
                        locale.text("基于当前分支", "Based on current branch"),
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(rgba(colors.text))
                            .font_weight(FontWeight::SEMIBOLD)
                            .overflow_hidden()
                            .text_ellipsis()
                            .whitespace_nowrap()
                            .child(ui_text(current.to_owned())),
                    )
                    .child(git_icon("icons/chevron-down.svg", colors.muted)),
                writable,
                colors,
                window,
                cx,
                Self::edit_git_base,
            )
            .h(px(34.))
            .rounded(px(8.))
            .bg(rgba(colors.field))
            .into_any_element()
        };
        let cancel = self
            .git_button(
                "hud-menu-git-cancel",
                git_label(locale.text("取消", "Cancel")),
                git.busy.is_none(),
                colors,
                window,
                cx,
                move |this, window, cx| this.show_git_view(back, window, cx),
            )
            .bg(rgba(colors.field))
            .border_color(rgba(colors.line));
        let create = self
            .git_button(
                "hud-menu-git-create",
                git_label(submit),
                writable && !input.read(cx).text.trim().is_empty(),
                colors,
                window,
                cx,
                |this, window, cx| this.submit_git_form(false, window, cx),
            )
            .bg(rgba(colors.primary))
            .text_color(rgba(colors.primary_text))
            .border_color(rgba(colors.primary));
        div()
            .flex()
            .flex_col()
            .child(self.git_subview_header(title, &git.back, back, colors, cx))
            .child(
                div()
                    .px(px(6.))
                    .pt(px(10.))
                    .pb(px(6.))
                    .child(
                        div()
                            .pl(px(2.))
                            .pb(px(7.))
                            .text_size(px(11.))
                            .text_color(rgba(colors.muted))
                            .child(ui_text(label)),
                    )
                    .child(git_form_field(
                        "hud-menu-git-name",
                        input,
                        colors,
                        window,
                        cx,
                    ))
                    .child(
                        div()
                            .mt(px(9.))
                            .text_size(px(11.))
                            .text_color(rgba(colors.muted))
                            .child(base_row),
                    )
                    .child(div().flex().justify_end().gap(px(7.)).mt(px(14.)).children(
                        [cancel, create].map(|button| {
                            button
                                .h(px(32.))
                                .px(px(12.))
                                .min_w(px(65.))
                                .border_1()
                                .rounded(px(8.))
                        }),
                    )),
            )
    }

    fn git_worktrees(
        &self,
        git: &GitMenu,
        status: &GitStatusDetail,
        colors: GitColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let locale = self.locale;
        let writable = self.read_only.is_none() && git.busy.is_none();
        let mut list = div()
            .id("hud-menu-git-worktree-list")
            .min_h_0()
            .max_h(px(260.))
            .overflow_y_scroll()
            .pt(px(3.))
            .pb(px(5.))
            .flex()
            .flex_col();
        let handle = window.window_handle();
        list = list.on_scroll_wheel(cx.listener(move |this, _, _, cx| {
            if let Some(git) = this.git_menu_mut(handle)
                && git.worktree_menu.take().is_some()
            {
                cx.notify();
            }
        }));
        let mut worktree_menu = None;
        for (index, tree) in status.worktrees.iter().enumerate() {
            let branch = tree.branch.as_deref().unwrap_or("(detached)");
            let path = tree.path.clone();
            let row_id: SharedString = format!("hud-menu-git-worktree-more-{index}").into();
            let sessions = git.session_counts.get(&path).copied().unwrap_or_default();
            let anchor = git
                .worktree_menu
                .as_ref()
                .filter(|(open_path, _)| open_path == &path)
                .map(|(_, bounds)| *bounds);
            let bounds = Rc::new(Cell::new(None));
            let measure = bounds.clone();
            let mut more = div()
                .id(row_id.clone())
                .relative()
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .size(px(27.))
                .rounded(px(7.))
                .children(footer_bounds_probe(if tree.current {
                    "hud-menu-git-worktree-more-current"
                } else {
                    "hud-menu-git-worktree-more-other"
                }))
                .child(
                    canvas(
                        move |bounds, _, _| measure.set(Some(bounds)),
                        |_, _, _, _| (),
                    )
                    .absolute()
                    .inset_0()
                    .size_full(),
                )
                .child(git_icon("icons/ellipsis.svg", colors.muted));
            if git.busy.is_none() {
                let focus = git_focus(window, cx, row_id);
                let menu_path = path.clone();
                let tooltip: SharedString =
                    format!("{branch} {}", locale.text("更多操作", "more actions")).into();
                more = self
                    .button(
                        more.cursor_pointer()
                            .hover(move |style| style.bg(rgba(colors.hover))),
                        &focus,
                        move |this, _, cx| {
                            if let Some(git) = this.git_menu_mut(handle) {
                                git.worktree_menu = if git
                                    .worktree_menu
                                    .as_ref()
                                    .is_some_and(|(path, _)| path == &menu_path)
                                {
                                    None
                                } else {
                                    bounds.get().map(|bounds| (menu_path.clone(), bounds))
                                };
                                cx.notify();
                            }
                        },
                        cx,
                    )
                    .tooltip(move |_, cx| cx.new(|_| CardTooltip(tooltip.clone())).into());
            }
            let path_label = compact_git_path(&path);
            let mut tags = div().flex().items_center().gap(px(4.)).mt(px(2.));
            for (show, label) in [
                (tree.current, locale.text("当前", "Current").into()),
                (tree.dirty, locale.text("有更改", "Has changes").into()),
                (
                    sessions > 0,
                    if locale == InterfaceLocale::Chinese {
                        format!("{sessions} 会话")
                    } else {
                        format!(
                            "{sessions} {}",
                            if sessions == 1 { "session" } else { "sessions" }
                        )
                    },
                ),
            ] {
                if show {
                    let current = label == locale.text("当前", "Current");
                    let (bg, text) = if current {
                        (colors.current_tag, colors.current_text)
                    } else {
                        (colors.field, colors.muted)
                    };
                    tags = tags.child(
                        div()
                            .px(px(5.))
                            .py(px(1.))
                            .rounded(px(999.))
                            .bg(rgba(bg))
                            .text_color(rgba(text))
                            .text_size(px(9.))
                            .child(ui_text(label)),
                    );
                }
            }
            if let Some(anchor) = anchor {
                let reveal = path.clone();
                let mut menu = div()
                    .w(px(148.))
                    .p(px(5.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(rgba(colors.line))
                    .bg(rgba(colors.panel))
                    .shadow(vec![shadow(0, 0.26, 10., 26.)])
                    .occlude()
                    .child(
                        self.git_button(
                            "hud-menu-git-reveal-worktree",
                            div().child(ui_text(if cfg!(target_os = "macos") {
                                locale.text("在 Finder 中显示", "Show in Finder")
                            } else {
                                locale.text("在文件管理器中显示", "Reveal in File Manager")
                            })),
                            true,
                            colors,
                            window,
                            cx,
                            move |this, window, cx| this.reveal_git_worktree(&reveal, window, cx),
                        )
                        .h(px(31.))
                        .px(px(8.)),
                    );
                if !tree.current && !tree.main && !tree.locked {
                    let target = path.clone();
                    let remove = self
                        .git_button(
                            "hud-menu-git-remove-worktree",
                            div().child(ui_text(locale.text("移除 Worktree", "Remove Worktree"))),
                            writable && sessions == 0,
                            colors,
                            window,
                            cx,
                            move |this, window, cx| {
                                this.submit_footer_git(
                                    window,
                                    cx,
                                    GitAction::RemoveWorktree,
                                    |request_id, cwd| Command::GitWorktreeRemove {
                                        request_id,
                                        cwd,
                                        worktree_path: target.clone(),
                                    },
                                )
                            },
                        )
                        .h(px(31.))
                        .px(px(8.))
                        .text_color(rgba(0xed756cff));
                    menu = menu.child(if sessions == 0 {
                        remove
                    } else {
                        let label =
                            locale.text("先移出关联会话", "Move attached sessions out first");
                        remove.tooltip(move |_, cx| cx.new(|_| CardTooltip(label.into())).into())
                    });
                }
                let above = index + 2 >= status.worktrees.len();
                worktree_menu = Some(
                    anchored()
                        .anchor(if above {
                            Corner::BottomRight
                        } else {
                            Corner::TopRight
                        })
                        .position(point(
                            anchor.right(),
                            if above { anchor.top() } else { anchor.bottom() },
                        ))
                        .snap_to_window()
                        .child(menu),
                );
            }
            list = list.child(
                div()
                    .relative()
                    .flex_none()
                    .flex()
                    .items_center()
                    .min_h(px(55.))
                    .px(px(7.))
                    .py(px(6.))
                    .gap(px(8.))
                    .rounded(px(10.))
                    .child(git_icon("icons/network.svg", colors.muted))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(1.))
                            .child(
                                div()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(ui_text(branch.to_owned())),
                            )
                            .child(
                                div()
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .whitespace_nowrap()
                                    .text_size(px(10.))
                                    .text_color(rgba(colors.muted))
                                    .child(ui_text(path_label)),
                            )
                            .child(tags),
                    )
                    .child(more),
            );
        }
        div()
            .relative()
            .min_h_0()
            .flex()
            .flex_col()
            .child(self.git_subview_header("Worktree", &git.back, GitView::Branches, colors, cx))
            .child(list)
            .child(
                div()
                    .flex_none()
                    .pt(px(5.))
                    .border_t_1()
                    .border_color(rgba(colors.line))
                    .child(self.git_button(
                        "hud-menu-git-create-worktree",
                        git_action_copy(
                            "icons/plus.svg",
                            locale.text("创建新 Worktree…", "Create a Worktree…"),
                            colors,
                        ),
                        writable,
                        colors,
                        window,
                        cx,
                        |this, window, cx| this.show_git_view(GitView::CreateWorktree, window, cx),
                    )),
            )
            .children(worktree_menu)
    }

    fn reveal_git_worktree(
        &mut self,
        path: &std::path::Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(git) = self.git_menu_mut(window.window_handle()) {
            git.worktree_menu = None;
        }
        match open_footer_path(path, PathAction::Reveal, self.wiring_visible()) {
            Ok(Some(mut child)) => cx
                .background_executor()
                .spawn(async move {
                    let _ = child.wait();
                })
                .detach(),
            Ok(None) => {}
            Err(error) => {
                if let Some(popup) = self.footer_popovers.get_mut(&window.window_handle()) {
                    popup.error = Some(format!(
                        "{} {error}",
                        self.locale.text("打开失败。", "Could not open.")
                    ));
                }
            }
        }
        cx.notify();
    }

    fn git_commit_panel(
        &self,
        git: &GitMenu,
        status: &GitStatusDetail,
        colors: GitColors,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let locale = self.locale;
        let input = git.input.as_ref().expect("Git commit message input");
        let editable = git.busy.is_none() && self.read_only.is_none();
        let can_commit = can_footer_commit(status, git.include_unstaged);
        let shareable = status.has_remote && status.current_branch.is_some();
        let branch = status
            .current_branch
            .as_deref()
            .or_else(|| {
                status
                    .detached_head
                    .as_deref()
                    .and_then(|head| head.get(..12))
            })
            .unwrap_or("HEAD");
        let count = status.dirty_file_count;
        let commit = self.git_button(
            "hud-menu-git-commit",
            git_action_copy(
                "icons/git-commit-horizontal.svg",
                locale.text("提交", "Commit"),
                colors,
            )
            .child(
                div()
                    .px(px(6.))
                    .py(px(2.))
                    .rounded(px(6.))
                    .bg(rgba(colors.field))
                    .text_color(rgba(colors.muted))
                    .text_size(px(9.))
                    .child(ui_text(if cfg!(target_os = "macos") {
                        "⌘↵"
                    } else {
                        "Ctrl+↵"
                    })),
            ),
            can_commit && editable,
            colors,
            window,
            cx,
            |this, window, cx| this.submit_git_form(false, window, cx),
        );
        let shortcut_hint = locale.text("当前没有可提交的更改", "No changes to commit");
        let commit = if can_commit {
            commit
        } else {
            commit.tooltip(move |_, cx| cx.new(|_| CardTooltip(shortcut_hint.into())).into())
        };
        let include = self
            .git_button(
                "hud-menu-git-stage-all",
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .text_size(px(11.))
                    .text_color(rgba(colors.muted))
                    .child(
                        div()
                            .size(px(13.))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.))
                            .border_1()
                            .border_color(rgba(if git.include_unstaged {
                                0x6f94c8ff
                            } else {
                                colors.muted
                            }))
                            .bg(rgba(if git.include_unstaged {
                                0x6f94c8ff
                            } else {
                                colors.field
                            }))
                            .text_color(rgba(0xffffffff))
                            .children(git.include_unstaged.then(|| ui_text("✓"))),
                    )
                    .child(div().flex_1().min_w_0().child(ui_text(
                        locale.text("包含未暂存的更改", "Include unstaged changes"),
                    )))
                    .child(
                        div()
                            .flex()
                            .gap(px(5.))
                            .child(
                                div()
                                    .text_color(rgba(0x3fb950ff))
                                    .child(ui_text(format!("+{}", status.additions))),
                            )
                            .child(
                                div()
                                    .text_color(rgba(0xf47067ff))
                                    .child(ui_text(format!("-{}", status.deletions))),
                            ),
                    ),
                editable,
                colors,
                window,
                cx,
                |this, window, cx| {
                    if let Some(git) = this.git_menu_mut(window.window_handle()) {
                        git.include_unstaged = !git.include_unstaged;
                        cx.notify();
                    }
                },
            )
            .px(px(8.))
            .rounded_none()
            .border_b_1()
            .border_color(rgba(colors.line));
        let push_tooltip = if !status.has_remote {
            Some(locale.text(
                "仓库尚未配置远端",
                "The repository has no remote configured yet",
            ))
        } else if status.current_branch.is_none() {
            Some(locale.text(
                "当前处于 detached HEAD，请先创建分支",
                "HEAD is detached; create a branch first",
            ))
        } else {
            None
        };
        let commit_push = self.git_button(
            "hud-menu-git-commit-push",
            git_action_copy(
                "icons/upload.svg",
                locale.text("提交并推送", "Commit and push"),
                colors,
            ),
            can_commit && shareable && editable,
            colors,
            window,
            cx,
            |this, window, cx| this.submit_git_form(true, window, cx),
        );
        let commit_push = if let Some(hint) = push_tooltip {
            commit_push.tooltip(move |_, cx| cx.new(|_| CardTooltip(hint.into())).into())
        } else {
            commit_push
        };
        let push = self.git_button(
            "hud-menu-git-push",
            git_action_copy("icons/upload.svg", locale.text("推送", "Push"), colors),
            shareable && status.can_push && editable,
            colors,
            window,
            cx,
            |this, window, cx| {
                this.submit_footer_git(
                    window,
                    cx,
                    GitAction::Push {
                        after_commit: false,
                    },
                    |request_id, cwd| Command::GitPush { request_id, cwd },
                )
            },
        );
        let push = if let Some(hint) = push_tooltip.or_else(|| {
            (!status.can_push).then(|| locale.text("当前没有待推送的提交", "No commits to push"))
        }) {
            push.tooltip(move |_, cx| cx.new(|_| CardTooltip(hint.into())).into())
        } else {
            push
        };
        div()
            .flex()
            .flex_col()
            .child(self.git_subview_header(
                locale.text("提交与推送", "Commit and push"),
                &git.back,
                GitView::Branches,
                colors,
                cx,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(8.))
                    .pt(px(3.))
                    .pb(px(7.))
                    .child(git_icon("icons/git-branch.svg", colors.muted))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_weight(FontWeight::SEMIBOLD)
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(ui_text(branch.to_owned())),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgba(colors.muted))
                            .child(footer_text(
                                "git-menu-commit-count",
                                uncommitted_files(locale, count),
                            )),
                    ),
            )
            .child(
                div()
                    .id("hud-menu-git-message")
                    .relative()
                    .h(px(86.))
                    .flex()
                    .flex_col()
                    .children(footer_bounds_probe("hud-menu-git-message"))
                    .child(input.clone())
                    .children(input.read(cx).text.is_empty().then(|| {
                        div()
                            .absolute()
                            .top(px(18.))
                            .left(px(18.))
                            .text_size(px(13.))
                            .text_color(rgba(colors.muted))
                            .child(ui_text(locale.text(
                                "提交信息（留空将自动生成）…",
                                "Commit message (generated when left empty)…",
                            )))
                    })),
            )
            .child(include)
            .child(
                div()
                    .pt(px(5.))
                    .child(commit)
                    .child(commit_push)
                    .child(push),
            )
    }

    fn launch_footer_path(
        &mut self,
        path: &std::path::Path,
        action: PathAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let result = open_footer_path(path, action, self.wiring_visible());
        match result {
            Ok(child) => {
                if let Some(mut child) = child {
                    cx.background_executor()
                        .spawn(async move {
                            let _ = child.wait();
                        })
                        .detach();
                }
                self.dismiss_footer_popover(window, cx);
            }
            Err(error) => {
                if let Some(popup) = self.footer_popovers.get_mut(&window.window_handle()) {
                    popup.error = Some(format!(
                        "{} {error}",
                        self.locale.text("打开失败。", "Could not open.")
                    ));
                    cx.notify();
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
struct GitColors {
    panel: u32,
    text: u32,
    muted: u32,
    line: u32,
    hover: u32,
    field: u32,
    primary: u32,
    primary_text: u32,
    current_tag: u32,
    current_text: u32,
}

impl GitColors {
    fn new(dark: bool) -> Self {
        if dark {
            Self {
                panel: 0x202124ff,
                text: 0xf2f3f4ff,
                muted: 0x9da1a7ff,
                line: 0xffffff17,
                hover: 0x2b2d30ff,
                field: 0x27292cff,
                primary: 0xf2f3f4ff,
                primary_text: 0x202124ff,
                current_tag: 0x3478f626,
                current_text: 0x75a8edff,
            }
        } else {
            Self {
                panel: 0xfffffffb,
                text: 0x303338ff,
                muted: 0x92969cff,
                line: 0x14181d1c,
                hover: 0xf1f1f2ff,
                field: 0xf7f7f8ff,
                primary: 0x22262bff,
                primary_text: 0xffffffff,
                current_tag: 0xe7f0fcff,
                current_text: 0x3973bdff,
            }
        }
    }
}

impl GitAction {
    fn pending(self, locale: InterfaceLocale) -> &'static str {
        match self {
            Self::Status => locale.text("正在读取仓库状态…", "Reading repository status…"),
            Self::Checkout => locale.text("正在切换分支…", "Switching branches…"),
            Self::CreateBranch => locale.text("正在创建分支…", "Creating branch…"),
            Self::Commit { push: false } => locale.text("正在提交…", "Committing…"),
            Self::Commit { push: true } | Self::Push { after_commit: true } => {
                locale.text("正在提交并推送…", "Committing and pushing…")
            }
            Self::Push {
                after_commit: false,
            } => locale.text("正在推送…", "Pushing…"),
            Self::CreateWorktree => locale.text("正在创建 Worktree…", "Creating Worktree…"),
            Self::RemoveWorktree => locale.text("正在移除 Worktree…", "Removing Worktree…"),
        }
    }

    fn notice(self, locale: InterfaceLocale) -> Option<&'static str> {
        Some(match self {
            Self::Commit { push: false } => locale.text("提交已完成", "Commit complete"),
            Self::Push {
                after_commit: false,
            } => locale.text("推送已完成", "Push complete"),
            Self::Push { after_commit: true } => {
                locale.text("提交与推送已完成", "Commit and push complete")
            }
            Self::CreateWorktree => locale.text("Worktree 已创建", "Worktree created"),
            Self::RemoveWorktree => locale.text("Worktree 已移除", "Worktree removed"),
            _ => return None,
        })
    }
}

fn localized_git_error(locale: InterfaceLocale, action: GitAction, error: &GitFailure) -> String {
    let summary = match error.code.as_str() {
        "STORAGE_READ_ONLY" => locale.text(
            STORAGE_READ_ONLY_TITLE,
            "The database is in read-only recovery mode",
        ),
        "GIT_NOT_REPOSITORY" => locale.text(
            "当前目录不是 Git 仓库",
            "The current directory is not a Git repository",
        ),
        "GIT_BRANCH_REQUIRED" => locale.text("请输入分支名称", "Enter a branch name"),
        "GIT_INVALID_BRANCH" => {
            locale.text("分支名称格式不正确", "The branch name format is invalid")
        }
        "GIT_BRANCH_EXISTS" => locale.text("该分支已存在", "This branch already exists"),
        "GIT_BRANCH_MISSING" => {
            locale.text("目标分支已不存在", "The target branch no longer exists")
        }
        "GIT_INVALID_START_POINT" => locale.text(
            "找不到基分支、标签或提交",
            "The start branch, tag, or commit could not be found",
        ),
        "GIT_BRANCH_IN_WORKTREE" => locale.text(
            "目标分支已在另一个 Worktree 中检出",
            "The target branch is already checked out in another worktree",
        ),
        "GIT_CHECKOUT_CONFLICT" => locale.text(
            "切换会覆盖当前更改。请先提交或暂存这些更改。",
            "Switching would overwrite local changes. Commit or stash them first.",
        ),
        "GIT_UNMERGED" => locale.text(
            "请先解决仓库中的合并冲突",
            "Resolve the repository’s merge conflicts first",
        ),
        "GIT_COMMIT_MESSAGE_REQUIRED" => locale.text("请输入提交信息", "Enter a commit message"),
        "GIT_NOTHING_TO_COMMIT" => locale.text("当前没有可提交的更改", "No changes to commit"),
        "GIT_DETACHED_HEAD" => locale.text(
            "当前处于 detached HEAD，请先创建分支",
            "HEAD is detached; create a branch first",
        ),
        "GIT_NO_REMOTE" => locale.text(
            "仓库尚未配置远端",
            "The repository has no remote configured yet",
        ),
        "GIT_NOTHING_TO_PUSH" => locale.text("当前没有待推送的提交", "No commits to push"),
        "GIT_UPSTREAM_UNAVAILABLE" => locale.text(
            "上游分支不可用。请检查远端和上游设置。",
            "The upstream branch is unavailable. Check the remote and tracking settings.",
        ),
        "GIT_PUSH_REJECTED" => locale.text(
            "远端包含不同的提交。请先同步远端更改再重试。",
            "The remote has different commits. Sync the remote changes and try again.",
        ),
        "GIT_WORKTREE_UNKNOWN" => {
            locale.text("Git 未登记这个 Worktree", "Git does not list this worktree")
        }
        "GIT_WORKTREE_PROTECTED" => locale.text(
            "当前、主仓库或已锁定的 Worktree 无法移除",
            "The current, main, or locked worktree cannot be removed",
        ),
        "GIT_WORKTREE_DIRTY" => locale.text(
            "本地更改已保留，Worktree 未移除",
            "Local changes kept; worktree was not removed",
        ),
        "GIT_WORKTREE_IN_USE" => locale.text(
            "该 Worktree 仍有关联会话",
            "This worktree still has sessions attached",
        ),
        _ => locale.text("Git 操作失败", "The Git operation failed"),
    };
    let detail = (error.code == "GIT_OPERATION_FAILED")
        .then(|| format!(" {0}", error.message.lines().next().unwrap_or_default()))
        .unwrap_or_default();
    let prefix = if action == (GitAction::Push { after_commit: true }) {
        locale.text("已提交。推送失败。", "Commit complete. Push failed.")
    } else {
        ""
    };
    [prefix, summary, detail.as_str()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn can_footer_commit(status: &GitStatusDetail, stage_all: bool) -> bool {
    if stage_all {
        status.dirty_file_count > 0
    } else {
        status.staged_count > 0
    }
}

fn uncommitted_files(locale: InterfaceLocale, count: usize) -> String {
    match locale {
        InterfaceLocale::Chinese => format!("未提交：{count} 个文件"),
        InterfaceLocale::English => format!(
            "{count} uncommitted {}",
            if count == 1 { "file" } else { "files" }
        ),
    }
}

fn compact_git_path(path: &std::path::Path) -> String {
    let path = path.to_string_lossy();
    let parts: Vec<_> = path
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .collect();
    if parts.len() > 3 {
        format!("…/{}", parts[parts.len() - 3..].join("/"))
    } else {
        path.into_owned()
    }
}

fn git_focus(window: &mut Window, cx: &mut App, id: impl Into<ElementId>) -> FocusHandle {
    window
        .use_keyed_state(id, cx, |_, cx| cx.focus_handle().tab_stop(true))
        .read(cx)
        .clone()
}

fn git_icon(path: &'static str, color: u32) -> impl IntoElement {
    svg()
        .path(path)
        .size(px(15.))
        .flex_none()
        .text_color(rgba(color))
}

fn git_label(label: &'static str) -> Div {
    div().w_full().text_center().child(ui_text(label))
}

fn git_action_copy(icon: &'static str, label: &'static str, colors: GitColors) -> Div {
    div()
        .w_full()
        .flex()
        .items_center()
        .gap(px(9.))
        .child(git_icon(icon, colors.muted))
        .child(div().flex_1().min_w_0().child(ui_text(label)))
}

fn git_form_field(
    id: &'static str,
    input: &Entity<RenameInput>,
    colors: GitColors,
    window: &Window,
    cx: &App,
) -> Div {
    let focused = input.read(cx).focus.is_focused(window);
    div()
        .h(px(38.))
        .flex()
        .items_center()
        .px(px(11.))
        .rounded(px(9.))
        .bg(rgba(colors.field))
        .border_1()
        .border_color(rgba(if focused { 0x6697dcff } else { colors.line }))
        .children(footer_bounds_probe(id))
        .child(input.clone())
}

fn environment_warning(reason: &str, locale: InterfaceLocale) -> &str {
    if reason.starts_with("restore-failed:") {
        return locale.text("恢复 Worktree 失败", "Could not restore the worktree");
    }
    if reason.starts_with("locate-failed:") {
        return locale.text("定位 Worktree 失败", "Could not locate the worktree");
    }
    if reason.starts_with("handoff-failed:") {
        return locale.text("切换运行环境失败", "Could not switch environments");
    }
    match reason {
        "path-missing" => locale.text("所选目录不存在", "The selected directory does not exist"),
        "not-worktree" => locale.text(
            "所选目录不是 Git Worktree",
            "The selected directory is not a Git worktree",
        ),
        "wrong-repository" => locale.text(
            "所选目录不属于当前仓库",
            "The selected directory does not belong to this repository",
        ),
        "not-listed-by-git" => {
            locale.text("Git 未登记这个 Worktree", "Git does not list this worktree")
        }
        "wrong-branch" => locale.text(
            "所选 Worktree 的分支与原会话不一致",
            "The selected worktree is on a different branch",
        ),
        "wrong-head" => locale.text(
            "所选 Worktree 的提交与原会话不一致",
            "The selected worktree is on a different commit",
        ),
        "path-owned-by-another-session" => locale.text(
            "此 Worktree 已由其他会话拥有",
            "This worktree belongs to another session",
        ),
        "environment-transition-in-progress" => locale.text(
            "运行环境正在切换，请稍候",
            "The environment is changing. Try again shortly.",
        ),
        "environment-transition-interrupted" => locale.text(
            "上次运行环境切换已中断",
            "The previous environment change was interrupted",
        ),
        "no-owned-worktree" => locale.text(
            "此会话没有自有 Worktree",
            "This session does not own a worktree",
        ),
        "session-unavailable" => locale.text("此会话已不可用", "This session is unavailable"),
        "invalid-path" => locale.text(
            "无法读取所选目录的路径",
            "Could not read the selected directory path",
        ),
        "STORAGE_READ_ONLY" => locale.text(
            STORAGE_READ_ONLY_TITLE,
            "The database is in read-only recovery mode",
        ),
        _ => reason,
    }
}

#[derive(Clone, Copy)]
enum PathAction {
    Finder,
    Reveal,
    Terminal,
}

fn open_footer_path(
    path: &std::path::Path,
    action: PathAction,
    visible: bool,
) -> std::io::Result<Option<std::process::Child>> {
    use std::io;
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Expected an absolute path",
        ));
    }
    if !visible {
        return Ok(None);
    }
    #[cfg(target_os = "macos")]
    {
        let mut command = std::process::Command::new("/usr/bin/open");
        match action {
            PathAction::Finder => {}
            PathAction::Reveal => {
                command.arg("-R");
            }
            PathAction::Terminal => {
                command.args(["-a", "Terminal"]);
            }
        }
        command.arg(path).spawn().map(Some)
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let mut command = match action {
            PathAction::Terminal => {
                let mut terminal = std::process::Command::new("cmd.exe");
                terminal
                    .arg("/d")
                    .current_dir(path)
                    .creation_flags(0x00000010);
                terminal
            }
            _ => {
                let mut explorer = std::process::Command::new("explorer.exe");
                explorer.arg(path);
                explorer
            }
        };
        command.spawn().map(Some)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let mut command = match action {
            PathAction::Terminal => {
                let mut terminal = std::process::Command::new("x-terminal-emulator");
                terminal.current_dir(path);
                terminal
            }
            _ => {
                let mut opener = std::process::Command::new("xdg-open");
                opener.arg(path);
                opener
            }
        };
        command.spawn().map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use matou_native_runtime::claude_accounts::{AccountUsage, ClaudeAccount, UsageWindow};
    use serde_json::json;

    #[::core::prelude::v1::test]
    fn claude_quota_has_no_slot_without_usage() {
        let empty = ClaudeAccountsState::default();
        let hud = json!({"usageWindows": []});
        for (default_account, state) in [(false, None), (true, None), (true, Some(&empty))] {
            assert!(
                claude_quota_slots(&hud, default_account, state, InterfaceLocale::Chinese, 0)
                    .is_empty()
            );
        }
    }

    #[::core::prelude::v1::test]
    fn default_claude_quota_follows_the_current_account_and_explains_cached_data() {
        let mut accounts = ClaudeAccountsState {
            current_id: Some("work".into()),
            accounts: vec![ClaudeAccount {
                id: "work".into(),
                name: "Work".into(),
                email: String::new(),
                needs_login: false,
                usage: Some(AccountUsage {
                    checked_at: 0,
                    stale: true,
                    five_hour: Some(UsageWindow {
                        used: 37.6,
                        resets_at: Some("2026-01-02T03:04:00Z".into()),
                    }),
                    weekly: Some(UsageWindow {
                        used: 62.,
                        resets_at: None,
                    }),
                }),
            }],
            ..Default::default()
        };
        let hud = json!({"usageWindows":[{"label":"5h","usedPercent":99}]});
        let now = 1_767_319_440_000; // 2026-01-02 02:04 UTC
        let slots = claude_quota_slots(&hud, true, Some(&accounts), InterfaceLocale::English, now);
        assert_eq!(
            slots
                .iter()
                .map(|slot| slot.usage.summary())
                .collect::<Vec<_>>(),
            ["5h 38% · 1h", "Weekly 62%"]
        );
        assert_eq!(
            slots[0].tooltip,
            "Default account: Work · 5h usage: 38% · resets in 1h · Last known; updates after use"
        );
        assert_eq!(
            slots[1].tooltip,
            "Default account: Work · Weekly usage: 62% · Reset time unavailable · Last known; updates after use"
        );

        let mut personal = accounts.accounts[0].clone();
        personal.id = "personal".into();
        personal.name = "Personal".into();
        personal.needs_login = true;
        accounts.current_id = Some(personal.id.clone());
        accounts.accounts.push(personal);
        let slots = claude_quota_slots(&hud, true, Some(&accounts), InterfaceLocale::Chinese, now);
        assert!(slots.is_empty());

        let slots = claude_quota_slots(&hud, false, Some(&accounts), InterfaceLocale::English, now);
        assert_eq!(slots[0].usage.summary(), "5h 99%");
        assert_eq!(slots[0].tooltip, "5h usage: 99% · Reset time unavailable");
    }

    #[::core::prelude::v1::test]
    fn git_projection_uses_runtime_branch_and_false_dirty_values() {
        let path = if cfg!(windows) {
            r"C:\work\tree"
        } else {
            "/work/tree"
        };
        let node = serde_json::json!({
            "git": {"state":"ready","branch":"old","dirty":true,"root":path}
        });
        let live = serde_json::json!({"gitBranch":"review/feature","gitDirty":false});
        let clean = FooterGit::projected(&node, Some(&live), InterfaceLocale::English).unwrap();
        assert_eq!(clean.label, "review/feature");
        assert_eq!(clean.branch.as_deref(), Some("review/feature"));
        assert_eq!(clean.root.as_deref(), Some(std::path::Path::new(path)));
        let legacy = FooterGit::projected(&node, None, InterfaceLocale::English).unwrap();
        assert_eq!(legacy.label, "old*");
    }

    #[::core::prelude::v1::test]
    fn hidden_finder_and_terminal_actions_do_not_launch_apps() {
        let path = std::path::Path::new(if cfg!(windows) {
            r"C:\fixture path\with 'quotes'"
        } else {
            "/fixture path/with 'quotes'"
        });
        for action in [PathAction::Finder, PathAction::Reveal, PathAction::Terminal] {
            assert!(open_footer_path(path, action, false).is_ok());
        }
        assert!(open_footer_path(std::path::Path::new("-a"), PathAction::Finder, false).is_err());
    }

    #[::core::prelude::v1::test]
    fn account_button_and_permission_are_scoped_to_the_provider() {
        assert!(footer_shows_account("claude-code"));
        assert!(footer_shows_account("shell"));
        for mode in ["antigravity", "codex", "dsh", "agent-team-member"] {
            assert!(!footer_shows_account(mode));
        }
        let node = serde_json::json!({"currentMode":"antigravity", "permissionMode":"ask"});
        assert_eq!(PermissionMode::from_node(&node), PermissionMode::Ask);
        assert_eq!(permission_chip_text(PermissionMode::Ask).as_ref(), "AK");
        assert_eq!(permission_mode_color(PermissionMode::Ask), 0xa371f7);
        assert!(
            PermissionMode::supported(&node)
                .unwrap()
                .contains(&PermissionMode::Bypass)
        );
    }
}

#[cfg(test)]
mod environment_tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn controls_require_ownership_and_a_recoverable_state() {
        use EnvironmentAction::*;
        let local = serde_json::json!({"kind":"local"});
        let worktree = serde_json::json!({"kind":"worktree"});
        assert!(environment_actions(&local, false, false, true).is_empty());
        assert_eq!(environment_actions(&local, true, false, true), &[Worktree]);
        assert_eq!(environment_actions(&worktree, true, false, true), &[Local]);
        assert_eq!(
            environment_actions(&worktree, true, true, false),
            &[Restore, Locate, Local]
        );
        assert!(environment_actions(&worktree, true, false, false).is_empty());
        assert!(Locate.command("session".into(), 3, None).is_none());
        let path = PathBuf::from("/selected/worktree");
        assert!(
            matches!(Locate.command("session".into(), 3, Some(path.clone())),
            Some(Command::LocateSessionEnvironment { request_id: 3, session_id, path: sent })
            if session_id == "session" && sent == path)
        );
    }
}
