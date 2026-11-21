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
        window.focus(&self.control_focus[menu.id()]);
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
        let locale = self.locale;
        let palette = self.theme_palette().workspace;
        let menu = popup.menu;
        let title = match menu {
            FooterMenu::Git => locale.text("Git 仓库", "Git repository"),
            FooterMenu::Environment => locale.text("运行环境", "Environment"),
            FooterMenu::Quota => locale.text("配额明细", "Quota details"),
        };
        let panel_id = match menu {
            FooterMenu::Git => "footer-git-menu",
            FooterMenu::Environment => "footer-environment-menu",
            FooterMenu::Quota => "footer-quota-menu",
        };
        let panel = div()
            .id("footer-popover-panel")
            .flex()
            .flex_col()
            .min_w(px(200.))
            .max_w(px(335.))
            .max_h((window.viewport_size().height - px(58.)).max(px(100.)))
            .p(px(6.))
            .border_1()
            .border_color(rgba(palette.line))
            .rounded(px(8.))
            .occlude()
            .bg(rgba(palette.panel))
            .text_color(rgba(palette.text))
            .text_size(px(12.))
            .shadow(vec![shadow(0x000000, 0.24, 8., 24.)])
            .child(
                div()
                    .px(px(8.))
                    .py(px(5.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(ui_text(title)),
            )
            .child(self.footer_popover_body(popup, window, cx));
        deferred(
            anchored()
                .anchor(if menu == FooterMenu::Quota {
                    Corner::BottomLeft
                } else {
                    Corner::BottomRight
                })
                .position_mode(AnchoredPositionMode::Local)
                .position(point(px(0.), px(0.)))
                .offset(point(px(0.), px(-PERMISSION_MENU_GAP)))
                .snap_to_window()
                .child(
                    div()
                        .relative()
                        .child(panel)
                        .children(footer_bounds_probe(panel_id)),
                ),
        )
        .with_priority(2)
    }

    fn footer_popover_body(
        &self,
        popup: &FooterPopover,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let palette = self.theme_palette().workspace;
        let locale = self.locale;
        let result = popup.result.as_ref().unwrap_or(&Value::Null);
        let menu = popup.menu;
        let mut body = div()
            .id("footer-popover-body")
            .flex()
            .flex_col()
            .min_h_0()
            .overflow_y_scroll();
        let live = self.live_agent_hud(&popup.session).unwrap_or(&Value::Null);
        if menu == FooterMenu::Quota {
            let mut usage = codex_hud::usage_windows(live, "allUsageWindows");
            if usage.is_empty() {
                usage = codex_hud::usage_windows(live, "usageWindows");
            }
            let dark = self.theme.effective() == AppTheme::Dark;
            return body.children(usage.iter().map(|usage| {
                div()
                    .flex_none()
                    .px(px(8.))
                    .py(px(5.))
                    .child(codex_hud::quota_row(usage, dark))
            }));
        }
        let fields = self.footer_fields_for(&popup.session);
        let inspected = popup
            .git_details
            .as_ref()
            .and_then(|details| diagnostic_git(details, locale));
        let git = inspected
            .as_ref()
            .or_else(|| fields.as_ref().and_then(|fields| fields.git.as_ref()));
        let branch = git.and_then(|git| git.branch.clone());
        let root = if result["isRepository"] == false {
            None
        } else {
            result["root"]
                .as_str()
                .map(PathBuf::from)
                .or_else(|| git.and_then(|git| git.root.clone()))
        };
        let status = match menu {
            FooterMenu::Git if result["isRepository"] == false => {
                locale.text("当前目录不是 Git 仓库", "No Git repository here")
            }
            FooterMenu::Git if git.is_some_and(|git| git.dirty) => {
                locale.text("有未提交的更改", "Uncommitted changes")
            }
            FooterMenu::Git => locale.text("工作区干净", "Working tree clean"),
            FooterMenu::Environment => match result["state"].as_str() {
                Some("ready") => locale.text("可用", "Ready"),
                Some("missing") => locale.text("目录不存在", "Directory missing"),
                Some("unavailable") => locale.text("环境不可用", "Environment unavailable"),
                _ => fields
                    .as_ref()
                    .and_then(|fields| fields.environment.as_ref())
                    .map_or("", |env| env.label),
            },
            FooterMenu::Quota => unreachable!(),
        };
        let focuses = window
            .use_keyed_state("footer-popover-focus", cx, |_, cx| {
                [(); 3].map(|_| cx.focus_handle().tab_stop(true))
            })
            .read(cx)
            .clone();
        body = body.child(
            div()
                .flex_none()
                .flex()
                .flex_col()
                .gap(px(3.))
                .px(px(8.))
                .pb(px(7.))
                .text_color(rgba(palette.muted))
                .children(
                    (menu == FooterMenu::Git)
                        .then_some(git)
                        .flatten()
                        .map(|git| ui_text(git.title.clone())),
                )
                .children(
                    (menu == FooterMenu::Environment)
                        .then_some(popup.cwd.as_ref())
                        .flatten()
                        .map(|path| ui_text(path.display().to_string())),
                )
                .child(ui_text(status))
                .children(
                    result["reason"]
                        .as_str()
                        .map(|reason| ui_text(reason.to_owned())),
                )
                .children(
                    popup
                        .request
                        .map(|_| ui_text(locale.text("正在检查…", "Checking…"))),
                )
                .children(popup.error.as_ref().map(|error| {
                    div()
                        .text_color(rgb(0xcf5856))
                        .child(ui_text(error.clone()))
                })),
        );
        let item = |id: &'static str, label| {
            div()
                .id(id)
                .flex_none()
                .px(px(8.))
                .py(px(6.))
                .rounded(px(4.))
                .cursor_pointer()
                .hover(move |style| style.bg(rgba(palette.panel_muted)))
                .children(footer_bounds_probe(id))
                .child(ui_text(label))
        };
        body = body.child(self.button(
            item(
                "hud-menu-refresh",
                locale.text(
                    if menu == FooterMenu::Git {
                        "刷新仓库状态"
                    } else {
                        "检查环境状态"
                    },
                    if menu == FooterMenu::Git {
                        "Refresh repository"
                    } else {
                        "Check environment"
                    },
                ),
            ),
            &focuses[0],
            Self::request_footer_diagnostics,
            cx,
        ));
        if menu == FooterMenu::Git {
            if let Some(branch) = branch {
                body = body.child(self.button(
                    item(
                        "hud-menu-copy-branch",
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
                body = body.child(self.button(
                    item(
                        "hud-menu-open-root",
                        locale.text("在 Finder 中打开仓库根目录", "Open repository in Finder"),
                    ),
                    &focuses[2],
                    move |this, window, cx| {
                        this.launch_footer_path(&root, PathAction::Finder, window, cx)
                    },
                    cx,
                ));
            }
        } else if let Some(path) = popup.cwd.clone() {
            let terminal_path = path.clone();
            body = body
                .child(self.button(
                    item(
                        "hud-menu-reveal",
                        locale.text("在 Finder 中显示", "Reveal in Finder"),
                    ),
                    &focuses[1],
                    move |this, window, cx| {
                        this.launch_footer_path(&path, PathAction::Reveal, window, cx)
                    },
                    cx,
                ))
                .child(self.button(
                    item(
                        "hud-menu-terminal",
                        locale.text("在系统终端中打开", "Open in Terminal"),
                    ),
                    &focuses[2],
                    move |this, window, cx| {
                        this.launch_footer_path(&terminal_path, PathAction::Terminal, window, cx)
                    },
                    cx,
                ));
        }
        body
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
    #[cfg(not(target_os = "macos"))]
    {
        let _ = action;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "This action requires macOS",
        ))
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
