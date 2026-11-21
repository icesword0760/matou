// Anchored HUD controls shared by the workspace and detached terminal windows.
// Frozen references: hud/GitControlMenu.tsx, hud/EnvironmentControlMenu.tsx,
// and TerminalHud.tsx (QuotaDetailWindows).
use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum FooterMenu {
    Git,
    Environment,
    Quota,
}

impl FooterMenu {
    fn id(self) -> &'static str {
        match self {
            Self::Git => "footer-git",
            Self::Environment => "footer-environment",
            Self::Quota => "footer-quota",
        }
    }
}

pub(super) struct FooterPopover {
    pub(super) session: String,
    menu: FooterMenu,
    cwd: Option<PathBuf>,
    request: Option<u64>,
    result: Option<Value>,
    git_details: Option<Value>,
    error: Option<String>,
    search: Option<Entity<RenameInput>>,
    _search_subscription: Option<Subscription>,
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
                    .child(self.footer_popover(popup, window, cx)),
            )
        } else {
            button
        }
    }

    pub(super) fn footer_quota(
        &self,
        mode: &str,
        hud: &Value,
        popup: Option<&FooterPopover>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        if !matches!(mode, "claude-code" | "antigravity") {
            return None;
        }
        let usages = codex_hud::usage_windows(hud, "usageWindows");
        if usages.is_empty() {
            return None;
        }
        let dark = self.theme.effective() == AppTheme::Dark;
        let tooltip: SharedString = usages
            .iter()
            .map(|usage| usage.summary())
            .collect::<Vec<_>>()
            .join("\n")
            .into();
        let quota = div()
            .id("footer-quota")
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.))
            .children(footer_bounds_probe("quota"))
            .children(usages.iter().map(|usage| codex_hud::quota_row(usage, dark)))
            .tooltip(move |_, cx| cx.new(|_| CardTooltip(tooltip.clone())).into());
        Some(if mode == "antigravity" {
            self.footer_menu_trigger(quota, FooterMenu::Quota, popup, window, cx)
        } else {
            quota
        })
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
        let search = (menu == FooterMenu::Git).then(|| {
            let placeholder = self.locale.text("搜索 matou 分支", "Search Matou branches");
            let dark = self.theme.effective() == AppTheme::Dark;
            cx.new(|cx| {
                let mut input = RenameInput::new(String::new(), cx);
                input.search_style = true;
                input.placeholder = placeholder.into();
                input.placeholder_color = Some(rgba(if dark { 0x9da1a7ff } else { 0x92969cff }));
                input.text_color = Some(rgba(if dark { 0xf2f3f4ff } else { 0x303338ff }));
                input
            })
        });
        let search_subscription = search
            .as_ref()
            .map(|input| cx.subscribe(input, |_, _, _: &RenameInputEvent, cx| cx.notify()));
        if let Some(input) = &search {
            window.focus(&input.read(cx).focus);
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
                git_details: None,
                error: None,
                search,
                _search_subscription: search_subscription,
            },
        );
        if menu != FooterMenu::Quota {
            self.request_footer_diagnostics(window, cx);
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
        self.request_footer_check(window.window_handle(), false, cx);
    }

    fn request_footer_check(
        &mut self,
        handle: AnyWindowHandle,
        inspect_git: bool,
        cx: &mut Context<Self>,
    ) {
        let request_id = self.next_wiring_request();
        let Some(popup) = self.footer_popovers.get_mut(&handle) else {
            return;
        };
        let Some(cwd) = popup.cwd.clone() else {
            return;
        };
        let command = match popup.menu {
            FooterMenu::Git if !inspect_git => Command::CheckGitRepo { request_id, cwd },
            FooterMenu::Git | FooterMenu::Environment => Command::GetEnvironmentDiagnostics {
                request_id,
                cwd,
                repair: None,
            },
            FooterMenu::Quota => return,
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
        if !matches!(kind, "gitRepoResult" | "environmentDiagnosticsResult") {
            return;
        }
        let Some((&handle, popup)) = self
            .footer_popovers
            .iter_mut()
            .find(|(_, popup)| popup.request == Some(request_id))
        else {
            return;
        };
        popup.request = None;
        popup.error = reply["error"].as_str().map(str::to_owned);
        let result = reply.get("result").cloned();
        if kind == "environmentDiagnosticsResult" && popup.menu == FooterMenu::Git {
            popup.git_details = result;
            cx.notify();
            return;
        }
        let inspect_git = popup.menu == FooterMenu::Git
            && result
                .as_ref()
                .is_some_and(|result| result["isRepository"] == true);
        if popup.menu == FooterMenu::Git {
            popup.git_details = None;
        }
        popup.result = result;
        if inspect_git {
            self.request_footer_check(handle, true, cx);
        }
        cx.notify();
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
            self.dismiss_footer_popover(window, cx);
            return true;
        }
        if stroke.key == "tab"
            && popup.menu != FooterMenu::Quota
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
        if popup
            .search
            .as_ref()
            .is_some_and(|input| input.read(cx).focus.is_focused(window))
        {
            return false;
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
            FooterMenu::Quota => {
                let live = self.live_agent_hud(&popup.session).unwrap_or(&Value::Null);
                let mut usage = codex_hud::usage_windows(live, "allUsageWindows");
                if usage.is_empty() {
                    usage = codex_hud::usage_windows(live, "usageWindows");
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
        let panel = div()
            .relative()
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
            anchored()
                .anchor(Corner::BottomRight)
                .position(point(viewport.width - px(16.), viewport.height - px(44.)))
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
                [(); 3].map(|_| cx.focus_handle().tab_stop(true))
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
        let is_worktree = environment["kind"] == "worktree";
        let owns_worktree = node["hasOwnedWorktree"] == true || node["worktree"].is_object();
        let needs_recovery = matches!(environment["state"].as_str(), Some("missing" | "failed"))
            || matches!(result["state"].as_str(), Some("missing" | "unavailable"));
        let mut recovery_labels = Vec::new();
        if is_worktree && needs_recovery {
            recovery_labels.extend([
                locale.text("恢复原 Worktree", "Restore the original worktree"),
                locale.text("定位已移动的 Worktree", "Locate the moved worktree"),
            ]);
        }
        if is_worktree && (ready || needs_recovery) {
            recovery_labels.push(locale.text("切换到 Local", "Switch to Local"));
        } else if owns_worktree && (ready || needs_recovery) {
            recovery_labels
                .push(locale.text("交接到自有 Worktree", "Hand off to the owned worktree"));
        }
        let recovery = (!recovery_labels.is_empty()).then(|| {
            div()
                .flex()
                .flex_col()
                .gap(px(7.))
                .children(recovery_labels.into_iter().map(|label| {
                    let explanation = locale.text(
                        "当前版本暂不支持在这里切换会话环境。",
                        "Session environment changes are unavailable here.",
                    );
                    div()
                        .id(label)
                        .px(px(10.))
                        .py(px(7.))
                        .rounded(px(7.))
                        .border_1()
                        .border_color(rgba(border))
                        .bg(rgba(button_bg))
                        .text_center()
                        .opacity(0.45)
                        .child(ui_text(label))
                        .tooltip(move |_, cx| cx.new(|_| CardTooltip(explanation.into())).into())
                }))
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
            .children(popup.error.as_ref().map(|error| {
                div()
                    .min_h(px(34.))
                    .px(px(12.))
                    .py(px(8.))
                    .border_t_1()
                    .border_color(rgba(divider))
                    .bg(rgba(path_bg))
                    .text_color(rgba(if dark { 0xf0aaa5ff } else { 0xab3d37ff }))
                    .child(ui_text(error.clone()))
            }))
    }

    fn git_panel(
        &self,
        popup: &FooterPopover,
        window: &mut Window,
        cx: &mut Context<Self>,
        dark: bool,
    ) -> Div {
        let locale = self.locale;
        let viewport = window.viewport_size();
        let (bg, text, muted, line, hover, field) = if dark {
            (
                0x202124ff, 0xf2f3f4ff, 0x9da1a7ff, 0xffffff17, 0x2b2d30ff, 0x27292cff,
            )
        } else {
            (
                0xfffffffb, 0x303338ff, 0x92969cff, 0x14181d1c, 0xf1f1f2ff, 0xf7f7f8ff,
            )
        };
        let result = popup.result.as_ref().unwrap_or(&Value::Null);
        let details = popup.git_details.as_ref().unwrap_or(&Value::Null);
        let fields = self.footer_fields_for(&popup.session);
        let inspected = diagnostic_git(details, locale);
        let git = inspected
            .as_ref()
            .or_else(|| fields.as_ref().and_then(|fields| fields.git.as_ref()))
            .filter(|_| result["isRepository"] != false);
        let root = result["root"]
            .as_str()
            .map(PathBuf::from)
            .or_else(|| git.and_then(|git| git.root.clone()));
        let branch = git.and_then(|git| git.branch.clone());
        let branches = result["branches"]
            .as_array()
            .or_else(|| details["git"]["branches"].as_array());
        let query = popup
            .search
            .as_ref()
            .map(|input| input.read(cx).text.trim().to_lowercase())
            .unwrap_or_default();
        let pending = ["staged", "unstaged", "untracked"]
            .into_iter()
            .map(|key| details["git"]["changes"][key].as_u64().unwrap_or_default())
            .sum::<u64>();
        let status = if pending > 0 {
            if locale == InterfaceLocale::Chinese {
                format!("未提交：{pending} 个文件")
            } else {
                format!(
                    "{pending} uncommitted {}",
                    if pending == 1 { "file" } else { "files" }
                )
            }
        } else if git.is_some_and(|git| git.dirty) {
            locale.text("有未提交的更改", "Uncommitted changes").into()
        } else {
            locale.text("工作区干净", "Working tree clean").into()
        };
        let branch_rows = if let Some(branches) = branches.filter(|values| !values.is_empty()) {
            branches
                .iter()
                .filter_map(|entry| {
                    let name = entry["name"].as_str()?;
                    let current = entry["current"] == true;
                    let summary = if current {
                        Some(status.clone())
                    } else {
                        entry["checkedOutPath"].as_str().map(|_| {
                            locale
                                .text("已在 Worktree 中", "Already in a worktree")
                                .to_owned()
                        })
                    };
                    Some((name.to_owned(), current, summary))
                })
                .collect::<Vec<_>>()
        } else {
            git.filter(|git| git.available)
                .map(|git| {
                    vec![(
                        branch.clone().unwrap_or_else(|| git.label.clone()),
                        true,
                        Some(status),
                    )]
                })
                .unwrap_or_default()
        };
        let visible = branch_rows
            .into_iter()
            .filter(|(name, _, _)| name.to_lowercase().contains(&query))
            .collect::<Vec<_>>();
        let search = popup.search.as_ref().map(|input| {
            let focused = input.read(cx).focus.is_focused(window);
            div()
                .id("hud-menu-git-search")
                .relative()
                .flex()
                .items_center()
                .h(px(37.))
                .rounded(px(9.))
                .border_2()
                .border_color(rgba(if focused { 0x5b91e8b8 } else { 0 }))
                .bg(rgba(if focused { field } else { 0 }))
                .pl(px(9.))
                .pr(px(10.))
                .gap(px(8.))
                .text_size(px(13.))
                .text_color(rgba(muted))
                .children(footer_bounds_probe("hud-menu-git-search"))
                .child(git_search_icon(rgba(muted)))
                .child(div().flex_1().min_w_0().child(input.clone()))
                .children((!query.is_empty()).then(|| {
                    let input = input.clone();
                    div()
                        .id("hud-menu-git-clear")
                        .size(px(25.))
                        .rounded(px(6.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(move |style| style.bg(rgba(hover)))
                        .child(ui_text("×"))
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            input.update(cx, |input, cx| input.clear(cx));
                            window.focus(&input.read(cx).focus);
                        })
                }))
        });
        let focuses = window
            .use_keyed_state("git-menu-focus", cx, |_, cx| {
                [(); 3].map(|_| cx.focus_handle().tab_stop(true))
            })
            .read(cx)
            .clone();
        let action_row = |id, icon: &'static str, label| {
            div()
                .id(id)
                .relative()
                .flex()
                .items_center()
                .gap(px(9.))
                .h(px(36.))
                .px(px(10.))
                .rounded(px(9.))
                .cursor_pointer()
                .hover(move |style| style.bg(rgba(hover)))
                .children(footer_bounds_probe(id))
                .child(
                    div()
                        .w(px(15.))
                        .text_color(rgba(muted))
                        .child(ui_text(icon)),
                )
                .child(ui_text(label))
        };
        let mut actions = div()
            .flex()
            .flex_col()
            .pt(px(5.))
            .border_t_1()
            .border_color(rgba(line))
            .child(self.button(
                action_row("hud-menu-refresh", "↻", locale.text("刷新", "Refresh")),
                &focuses[0],
                Self::request_footer_diagnostics,
                cx,
            ));
        if let Some(branch) = branch {
            actions = actions.child(self.button(
                action_row(
                    "hud-menu-copy-branch",
                    "⧉",
                    locale.text("复制分支名", "Copy branch name"),
                ),
                &focuses[1],
                move |this, window, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(branch.clone()));
                    this.dismiss_footer_popover(window, cx);
                },
                cx,
            ));
        }
        if let Some(root) = root.filter(|path| path.is_absolute()) {
            let label = if cfg!(target_os = "macos") {
                locale.text("在 Finder 中打开", "Open in Finder")
            } else {
                locale.text("在文件管理器中打开", "Open in File Manager")
            };
            actions = actions.child(self.button(
                action_row("hud-menu-open-root", "↗", label),
                &focuses[2],
                move |this, window, cx| {
                    this.launch_footer_path(&root, PathAction::Finder, window, cx)
                },
                cx,
            ));
        }
        div()
            .w((viewport.width - px(28.)).min(px(316.)).max(px(0.)))
            .max_h((viewport.height - px(66.)).min(px(390.)).max(px(0.)))
            .p_0()
            .pt(px(7.))
            .px(px(8.))
            .pb(px(8.))
            .flex()
            .flex_col()
            .border_1()
            .border_color(rgba(line))
            .rounded(px(17.))
            .bg(rgba(bg))
            .text_color(rgba(text))
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
            .children(search)
            .child(
                div()
                    .px(px(10.))
                    .pt(px(5.))
                    .pb(px(6.))
                    .text_color(rgba(muted))
                    .child(ui_text(locale.text("分支", "Branches"))),
            )
            .child(
                div()
                    .id("hud-menu-git-branches")
                    .min_h_0()
                    .max_h(px(190.))
                    .overflow_y_scroll()
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
                                .text_color(rgba(muted))
                                .child(ui_text(if result["isRepository"] == false {
                                    locale.text("当前目录不是 Git 仓库", "No Git repository here")
                                } else if popup.request.is_some() && query.is_empty() {
                                    locale.text("正在读取仓库…", "Reading repository…")
                                } else {
                                    locale.text("没有匹配的分支", "No matching branches")
                                })),
                        )
                    })
                    .children(visible.into_iter().enumerate().map(
                        |(index, (name, current, summary))| {
                            div()
                                .min_h(px(45.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .px(px(8.))
                                .py(px(5.))
                                .rounded(px(10.))
                                .when(index == 0, |row| row.bg(rgba(hover)))
                                .child(
                                    div()
                                        .w(px(20.))
                                        .flex_none()
                                        .child(git_branch_glyph(rgba(muted))),
                                )
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
                                                    if current {
                                                        "git-menu-current"
                                                    } else {
                                                        "git-menu-other"
                                                    },
                                                    name,
                                                )),
                                        )
                                        .children(summary.map(|summary| {
                                            div()
                                                .text_size(px(11.))
                                                .text_color(rgba(muted))
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
                                        .text_color(rgba(muted))
                                        .children(current.then(|| ui_text("✓"))),
                                )
                        },
                    )),
            )
            .child(actions)
            .children(popup.error.as_ref().map(|error| {
                div()
                    .px(px(10.))
                    .py(px(6.))
                    .bg(rgba(field))
                    .text_color(rgba(0xf0877fff))
                    .child(ui_text(error.clone()))
            }))
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

fn environment_warning(reason: &str, locale: InterfaceLocale) -> &str {
    let reason = reason
        .strip_prefix("restore-failed:")
        .or_else(|| reason.strip_prefix("locate-failed:"))
        .unwrap_or(reason);
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
        _ => reason,
    }
}

fn git_search_icon(color: Rgba) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let at = |x, y| bounds.origin + point(px(x), px(y));
            let mut path = PathBuilder::stroke(px(1.5));
            path.move_to(at(11., 6.));
            path.arc_to(point(px(4.5), px(4.5)), px(0.), false, true, at(2., 6.));
            path.arc_to(point(px(4.5), px(4.5)), px(0.), false, true, at(11., 6.));
            path.move_to(at(10., 10.));
            path.line_to(at(14., 14.));
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        },
    )
    .size(px(15.))
    .flex_none()
}

fn git_branch_glyph(color: Rgba) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let at = |x, y| bounds.origin + point(px(x), px(y));
            let mut path = PathBuilder::stroke(px(1.25));
            for (x, y) in [(4., 3.), (4., 12.), (11., 3.)] {
                path.move_to(at(x + 1.5, y));
                path.arc_to(point(px(1.5), px(1.5)), px(0.), false, true, at(x - 1.5, y));
                path.arc_to(point(px(1.5), px(1.5)), px(0.), false, true, at(x + 1.5, y));
            }
            path.move_to(at(4., 4.5));
            path.line_to(at(4., 10.5));
            path.move_to(at(11., 4.5));
            path.cubic_bezier_to(at(4., 8.5), at(11., 7.5), at(8., 8.5));
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        },
    )
    .size(px(15.))
    .flex_none()
}

fn diagnostic_git(result: &Value, locale: InterfaceLocale) -> Option<FooterGit> {
    let git = &result["git"];
    if result["state"] != "ready" || !git.is_object() {
        return None;
    }
    let dirty = ["staged", "unstaged", "untracked"]
        .into_iter()
        .any(|key| git["changes"][key].as_u64().is_some_and(|count| count > 0));
    let node = serde_json::json!({
        "git": {
            "state": "ready", "branch": git["head"]["branch"], "detachedHead": git["head"]["revision"],
            "dirty": dirty, "root": git["root"],
        }
    });
    FooterGit::projected(&node, None, locale)
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

    #[::core::prelude::v1::test]
    fn git_projection_uses_runtime_branch_and_false_dirty_values() {
        let node = serde_json::json!({
            "git": {"state":"ready","branch":"old","dirty":true,"root":"/work/tree"}
        });
        let live = serde_json::json!({"gitBranch":"review/feature","gitDirty":false});
        let clean = FooterGit::projected(&node, Some(&live), InterfaceLocale::English).unwrap();
        assert_eq!(clean.label, "review/feature");
        assert_eq!(clean.branch.as_deref(), Some("review/feature"));
        assert_eq!(
            clean.root.as_deref(),
            Some(std::path::Path::new("/work/tree"))
        );
        let legacy = FooterGit::projected(&node, None, InterfaceLocale::English).unwrap();
        assert_eq!(legacy.label, "old*");
    }

    #[::core::prelude::v1::test]
    fn hidden_finder_and_terminal_actions_do_not_launch_apps() {
        let path = std::path::Path::new("/fixture path/with 'quotes'");
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
