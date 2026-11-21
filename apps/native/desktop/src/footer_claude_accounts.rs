use super::*;
use matou_native_runtime::claude_accounts::{
    ClaudeAccount, ClaudeAccountCommand as Action, ClaudeAccountsState, UsageWindow,
};

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Page {
    #[default]
    List,
    Manage,
    Edit,
    Remove,
    Scope,
    Add,
    Reauth,
    Login,
    Draft,
    Detected,
    Renewed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Reply {
    List,
    Refresh,
    Switch,
    Rename,
    Remove,
    Login,
    Code,
    Save,
    Cancel,
}

#[derive(Default)]
pub(super) struct AccountUi {
    pub(super) requested: bool,
    pub(super) input: Option<Entity<RenameInput>>,
    request: Option<(u64, Reply)>,
    login: Option<u64>,
    state: ClaudeAccountsState,
    page: Page,
    selected: Option<String>,
    remember: bool,
    notice: bool,
}

#[derive(Clone, Copy)]
struct Colors {
    bg: Rgba,
    text: Rgba,
    muted: Rgba,
    border: Rgba,
    line: Rgba,
    field: Rgba,
    blue: Rgba,
    active: Rgba,
    active_border: Rgba,
}

impl Colors {
    fn new(dark: bool) -> Self {
        if dark {
            Self {
                bg: rgba(0x232a34ff),
                text: rgba(0xe1e6edff),
                muted: rgba(0xa4b3c8ff),
                border: rgba(0x3c4858ff),
                line: rgba(0x3c4858ff),
                field: rgba(0x1f2835ff),
                blue: rgba(0x83b7f4ff),
                active: rgba(0x293549ff),
                active_border: rgba(0x374c68ff),
            }
        } else {
            Self {
                bg: rgba(0xffffffff),
                text: rgba(0x34445bff),
                muted: rgba(0x7186a3ff),
                border: rgba(0xd9e3f1ff),
                line: rgba(0xe3ecf9ff),
                field: rgba(0xf7f9fcff),
                blue: rgba(0x287cddff),
                active: rgba(0xf4f8feff),
                active_border: rgba(0xd3e3f8ff),
            }
        }
    }
}

impl Workspace {
    pub(super) fn account_footer_visible(&self, window: &Window) -> bool {
        self.footer_session(window)
            .and_then(|id| self.hud_node(id))
            .and_then(|node| node["currentMode"].as_str())
            .is_some_and(footer_shows_account)
    }

    pub(super) fn dismiss_ineligible_account_overlay(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.wiring.overlay == Some(Overlay::Accounts)
            && self.wiring.overlay_window == Some(window.window_handle())
            && !self.account_footer_visible(window)
        {
            self.dismiss_wiring_overlay(window, cx);
        }
    }

    pub(super) fn load_claude_accounts(&mut self, cx: &mut Context<Self>) {
        self.request_claude_account(Action::List, cx);
    }

    fn request_claude_account(&mut self, action: Action, cx: &mut Context<Self>) {
        let ui = &self.wiring.accounts;
        if ui.request.is_some()
            || (ui.login.is_some()
                && !matches!(action, Action::SubmitLoginCode { .. } | Action::CancelLogin))
        {
            return;
        }
        let request_id = self.next_wiring_request();
        let reply = match action {
            Action::List => Reply::List,
            Action::RefreshUsage => Reply::Refresh,
            Action::Switch { .. } | Action::Reauth { .. } => Reply::Switch,
            Action::Rename { .. } => Reply::Rename,
            Action::Remove { .. } => Reply::Remove,
            Action::Login { .. } => Reply::Login,
            Action::SubmitLoginCode { .. } => Reply::Code,
            Action::SaveDraft { .. } | Action::SaveDetected { .. } => Reply::Save,
            Action::CancelLogin => Reply::Cancel,
        };
        let command = if reply == Reply::List {
            Command::GetClaudeAccounts { request_id }
        } else {
            Command::ClaudeAccountCommand { request_id, action }
        };
        let sent = self
            .commands
            .send_at(self.runtime_generation, command)
            .is_ok();
        let ui = &mut self.wiring.accounts;
        ui.requested = true;
        ui.state.error = (!sent).then(|| "runtime".into());
        ui.request = sent.then_some((request_id, reply));
        cx.notify();
    }

    pub(super) fn receive_claude_accounts(
        &mut self,
        request_id: u64,
        reply: &Value,
        cx: &mut Context<Self>,
    ) {
        let ui = &mut self.wiring.accounts;
        let request = if ui.request.is_some_and(|(id, _)| id == request_id) {
            ui.request.take().map(|(_, reply)| reply)
        } else if ui.login == Some(request_id) {
            ui.login = None;
            Some(Reply::Login)
        } else {
            None
        };
        let Some(request) = request else { return };
        let Ok(state) = serde_json::from_value::<ClaudeAccountsState>(reply["result"].clone())
        else {
            ui.state.error = Some("runtime".into());
            cx.notify();
            return;
        };
        if request == Reply::Login && state.login_attempt_id.is_some() {
            ui.login = Some(request_id);
        }
        if state.error.is_none() {
            let page = match request {
                Reply::Switch | Reply::Cancel => Some(Page::List),
                Reply::Rename | Reply::Remove => Some(Page::Manage),
                Reply::Login if state.draft.is_some() => Some(Page::Draft),
                Reply::Login if state.login_attempt_id.is_some() => Some(Page::Login),
                Reply::Save if ui.page == Page::Draft => {
                    if let Some(id) = ui.state.reauth_id.as_ref() {
                        ui.selected = Some(id.clone());
                        Some(Page::Renewed)
                    } else {
                        Some(Page::List)
                    }
                }
                Reply::Save => Some(Page::List),
                _ => None,
            };
            if let Some(page) = page {
                let text = if page == Page::Draft {
                    state.draft.as_ref().map(|a| a.name.as_str())
                } else {
                    None
                };
                ui.page = page;
                ui.input = match page {
                    Page::Draft => Some(account_input(
                        text.unwrap_or_default(),
                        "",
                        self.theme.effective() == AppTheme::Dark,
                        cx,
                    )),
                    Page::Login => Some(account_input(
                        "",
                        self.locale.text("粘贴授权码", "Paste authorization code"),
                        self.theme.effective() == AppTheme::Dark,
                        cx,
                    )),
                    _ => None,
                };
            }
            ui.notice |= state.switched;
        } else if request == Reply::Login
            && ui.page == Page::Login
            && state.login_attempt_id.is_none()
        {
            ui.page = if ui.selected.is_some() {
                Page::Reauth
            } else {
                Page::Add
            };
            ui.input = None;
        }
        ui.state = state;
        cx.notify();
    }

    fn toggle_accounts(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.wiring.overlay == Some(Overlay::Accounts)
            && self.wiring.overlay_window == Some(window.window_handle())
        {
            self.dismiss_wiring_overlay(window, cx);
        } else if self.hud_wiring_available(window) && self.account_footer_visible(window) {
            self.open_wiring_overlay(Overlay::Accounts, window, cx);
            self.wiring.restore_focus = Some(self.control_focus["accounts"].clone());
            self.wiring.accounts.notice = false;
            if let Some(draft) = self.wiring.accounts.state.draft.as_ref() {
                let name = draft.name.clone();
                self.account_page(Page::Draft, None, &name, cx);
            } else if self.wiring.accounts.state.login_attempt_id.is_some() {
                self.account_page(Page::Login, None, "", cx);
            } else {
                self.account_page(Page::List, None, "", cx);
                self.load_claude_accounts(cx);
            }
        }
    }

    fn account_page(
        &mut self,
        page: Page,
        selected: Option<String>,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        let dark = self.theme.effective() == AppTheme::Dark;
        let input = match page {
            Page::Edit | Page::Draft | Page::Detected => Some(account_input(text, "", dark, cx)),
            Page::Login => Some(account_input(
                "",
                self.locale.text("粘贴授权码", "Paste authorization code"),
                dark,
                cx,
            )),
            _ => None,
        };
        let ui = &mut self.wiring.accounts;
        ui.page = page;
        ui.input = input;
        ui.selected = selected;
        ui.remember = ui.state.remember_scope;
        ui.state.error = None;
        cx.notify();
    }

    pub(super) fn claude_accounts_key(
        &mut self,
        key: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if key.key == "tab"
            && !key.modifiers.alt
            && !key.modifiers.platform
            && !key.modifiers.control
        {
            if key.modifiers.shift {
                window.focus_prev();
            } else {
                window.focus_next();
            }
            return true;
        }
        if self
            .wiring
            .accounts
            .input
            .as_ref()
            .is_some_and(|input| input.read(cx).focus.is_focused(window))
        {
            if key.key == "enter" && !key.modifiers.modified() {
                self.submit_account_input(cx);
                return true;
            }
            return false;
        }
        !matches!(key.key.as_str(), "enter" | "space")
    }

    fn submit_account_input(&mut self, cx: &mut Context<Self>) {
        let ui = &self.wiring.accounts;
        let Some(input) = &ui.input else { return };
        if input.read(cx).composing() {
            return;
        }
        let text = input.read(cx).text.clone();
        let action = match ui.page {
            Page::Login if !ui.state.login_code_submitted => {
                let Some(attempt_id) = ui.state.login_attempt_id.clone() else {
                    return;
                };
                input.update(cx, |input, cx| input.clear_sensitive(cx));
                Action::SubmitLoginCode {
                    attempt_id,
                    code: text,
                }
            }
            Page::Draft => Action::SaveDraft { name: text },
            Page::Detected => Action::SaveDetected { name: text },
            Page::Edit => {
                let Some(id) = ui.selected.clone() else {
                    return;
                };
                Action::Rename { id, name: text }
            }
            _ => return,
        };
        self.request_claude_account(action, cx);
    }

    pub(in crate::workspace) fn footer_account_trigger(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let palette = self.theme_palette().footer;
        let state = &self.wiring.accounts.state;
        let current = state
            .accounts
            .iter()
            .find(|account| Some(&account.id) == state.current_id.as_ref());
        let locale = self.locale;
        let label = current
            .map(|account| account.name.clone())
            .unwrap_or_else(|| locale.text(FOOTER_ACCOUNT_LABEL, "Claude accounts").into());
        let open = self.wiring.overlay == Some(Overlay::Accounts)
            && self.wiring.overlay_window == Some(window.window_handle());
        let color = rgba(if open { 0x317bdbff } else { palette.account });
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
                .text_color(color)
                .whitespace_nowrap()
                .cursor_pointer()
                .when(open, |trigger| trigger.bg(rgba(0x518ef020)))
                .hover(|style| style.bg(rgba(0x518ef019)).text_color(rgba(0x317bdbff)))
                .tooltip(move |_, cx| {
                    cx.new(|_| {
                        CardTooltip(
                            locale
                                .text(
                                    "切换本机默认 Claude 账户",
                                    "Switch the device-default Claude account",
                                )
                                .into(),
                        )
                    })
                    .into()
                })
                .child(footer_user_icon(14., color))
                .child(
                    div()
                        .max_w(px(125.))
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(footer_text("account", label)),
                )
                .children(current.filter(|a| a.needs_login).map(|_| {
                    div()
                        .flex_none()
                        .size(px(5.))
                        .rounded_full()
                        .bg(rgba(0xd79842ff))
                }))
                .child(ui_text("⌃"))
                .children(footer_bounds_probe("account"))
                .children(wiring_probe_bounds("footer-account".into())),
            &self.control_focus["accounts"],
            Self::toggle_accounts,
            cx,
        )
    }

    pub(super) fn render_account_panel(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let locale = self.locale;
        let dark = self.theme.effective() == AppTheme::Dark;
        let colors = Colors::new(dark);
        let ui = &self.wiring.accounts;
        let viewport = window.viewport_size();
        let title = match ui.page {
            Page::Manage => locale.text("管理账户", "Manage accounts"),
            Page::Edit => locale.text("编辑账户", "Edit account"),
            Page::Scope => locale.text("切换本机默认账户", "Switch device default"),
            _ => locale.text(FOOTER_ACCOUNT_LABEL, "Claude accounts"),
        };
        let close = self
            .ca_button(
                "footer-accounts-close",
                "×",
                colors.muted,
                window,
                cx,
                Self::dismiss_wiring_overlay,
            )
            .text_size(px(21.))
            .size(px(26.))
            .justify_center()
            .p_0();
        let mut header = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(18.))
            .py(px(15.));
        if ui.page != Page::List && ui.page != Page::Login {
            header = header.child(self.ca_button(
                "claude-account-back",
                "‹",
                colors.muted,
                window,
                cx,
                |this, _, cx| this.account_page(Page::List, None, "", cx),
            ));
        }
        header = header.child(
            div()
                .flex_1()
                .text_size(px(14.))
                .font_weight(FontWeight(650.))
                .child(ui_text(title)),
        );
        if ui.page == Page::List {
            let refreshing = ui.request.is_some_and(|(_, reply)| reply == Reply::Refresh);
            let label = if refreshing {
                locale.text("刷新中", "Refreshing")
            } else {
                locale.text("刷新", "Refresh")
            };
            header = header.child(
                self.ca_button(
                    "claude-account-refresh",
                    label,
                    colors.blue,
                    window,
                    cx,
                    |this, _, cx| this.request_claude_account(Action::RefreshUsage, cx),
                )
                .px(px(4.)),
            );
        }
        let mut body = div()
            .id("footer-account-list")
            .flex()
            .flex_col()
            .min_h_0()
            .overflow_y_scroll()
            .px(px(12.))
            .pb(px(8.));
        if let Some(error) = ui.state.error.as_deref() {
            body = body.child(ca_notice(
                colors,
                locale.text("操作尚未完成", "Action not completed"),
                account_error(locale, error),
            ));
        }
        if ui.page == Page::List {
            body = body.child(self.account_list(colors, dark, window, cx));
        } else {
            body = body.child(self.account_form(colors, window, cx));
        }
        let panel = div()
            .id("footer-account-panel")
            .track_focus(&self.wiring.overlay_focus)
            .relative()
            .w((viewport.width - px(24.)).max(px(0.)).min(px(410.)))
            .max_h((viewport.height - px(70.)).max(px(0.)))
            .flex()
            .flex_col()
            .rounded(px(12.))
            .overflow_hidden()
            .border_1()
            .border_color(colors.border)
            .bg(colors.bg)
            .text_color(colors.text)
            .text_size(px(12.))
            .line_height(px(18.6))
            .shadow(if dark {
                vec![shadow(0, 0.33, 16., 42.)]
            } else {
                vec![
                    shadow(0x263d61, 0.15, 16., 42.),
                    shadow(0x263d61, 0.047, 3., 10.),
                ]
            })
            .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
            .child(header.child(close))
            .child(body)
            .when(
                ui.page == Page::List && !ui.state.accounts.is_empty(),
                |panel| {
                    panel.child(
                        div()
                            .flex_none()
                            .mx(px(12.))
                            .px(px(4.))
                            .py(px(14.))
                            .border_t_1()
                            .border_color(colors.line)
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(16.))
                            .child(
                                self.ca_button(
                                    "claude-account-add",
                                    locale.text("＋ 添加账户", "＋ Add account"),
                                    rgba(0xffffffff),
                                    window,
                                    cx,
                                    |this, _, cx| this.account_page(Page::Add, None, "", cx),
                                )
                                .min_w(px(142.))
                                .h(px(35.))
                                .py_0()
                                .justify_center()
                                .font_weight(FontWeight(550.))
                                .bg(rgba(0x3284edff)),
                            )
                            .child(
                                self.ca_button(
                                    "footer-manage-accounts",
                                    locale.text("管理账户", "Manage accounts"),
                                    colors.blue,
                                    window,
                                    cx,
                                    |this, _, cx| this.account_page(Page::Manage, None, "", cx),
                                )
                                .font_weight(FontWeight(550.)),
                            ),
                    )
                },
            );
        div()
            .absolute()
            .inset_0()
            .occlude()
            .on_any_mouse_down(cx.listener(|this, _, window, cx| {
                this.dismiss_wiring_overlay(window, cx);
                cx.stop_propagation();
            }))
            .child(
                anchored()
                    .anchor(Corner::BottomRight)
                    .position(point(viewport.width - px(12.), viewport.height - px(46.)))
                    .snap_to_window()
                    .child(
                        div()
                            .relative()
                            .children(wiring_probe_bounds("footer-account-panel".into()))
                            .child(panel),
                    ),
            )
            .into_any_element()
    }

    fn ca_button(
        &self,
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        color: Rgba,
        window: &mut Window,
        cx: &mut Context<Self>,
        activate: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> Stateful<Div> {
        let id = id.into();
        let focus = window
            .use_keyed_state(id.clone(), cx, |_, cx| cx.focus_handle().tab_stop(true))
            .read(cx)
            .clone();
        let disabled = self.wiring.accounts.request.is_some()
            && !matches!(
                id.as_ref(),
                "footer-accounts-close" | "claude-account-refresh"
            );
        let button = div()
            .id(id.clone())
            .relative()
            .flex_none()
            .flex()
            .items_center()
            .px(px(12.))
            .py(px(6.))
            .rounded(px(7.))
            .text_color(color)
            .children(wiring_probe_bounds(id.to_string()))
            .when(id.as_ref() == "claude-account-refresh", |button| {
                button.gap(px(6.)).child(account_refresh_icon(color))
            })
            .child(ui_text(label.into()));
        if disabled {
            button.opacity(0.55)
        } else {
            self.button(
                button
                    .cursor_pointer()
                    .hover(|style| style.bg(rgba(0x518ef01a))),
                &focus,
                activate,
                cx,
            )
        }
    }

    fn account_list(
        &self,
        colors: Colors,
        dark: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let locale = self.locale;
        let ui = &self.wiring.accounts;
        let state = &ui.state;
        let mut list = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .when(!state.accounts.is_empty(), |list| list.pb(px(10.)));
        if ui.notice {
            list = list.child(div().text_color(colors.text).px(px(4.)).child(ui_text(locale.text(
                "✓ 已切换账户，现有会话通常会在约 30 秒内使用新登录。", "✓ Account switched. Existing sessions usually pick it up within about 30 seconds."))));
        }
        if let Some(account) = &state.detected {
            let name = account.name.clone();
            list = list.child(
                ca_notice(
                    colors,
                    locale.text("发现本机已有 Claude 登录", "Existing Claude sign-in found"),
                    account.email.clone(),
                )
                .child(self.ca_button(
                    "claude-account-detected",
                    locale.text("保存当前账户", "Save current account"),
                    colors.blue,
                    window,
                    cx,
                    move |this, _, cx| this.account_page(Page::Detected, None, &name, cx),
                )),
            );
        } else if state.accounts.is_empty() {
            if ui.request.is_some() {
                list = list.child(
                    div()
                        .p(px(18.))
                        .text_center()
                        .text_color(colors.muted)
                        .child(ui_text(
                            locale.text("正在读取本机账户…", "Reading device accounts…"),
                        )),
                );
            } else {
                list =
                    list.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .p(px(18.))
                            .text_center()
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(ui_text(locale.text(
                                        "多个账户，随手切换",
                                        "Your accounts, one click away",
                                    ))),
                            )
                            .child(div().text_color(colors.muted).child(ui_text(locale.text(
                                "每个账户只需登录一次。以后在这里查看用量、选择账户。",
                                "Sign in once per account. Check usage and choose an account here.",
                            ))))
                            .child(
                                self.ca_button(
                                    "claude-account-first",
                                    locale.text("添加第一个账户", "Add your first account"),
                                    rgba(0xffffffff),
                                    window,
                                    cx,
                                    |this, _, cx| this.account_page(Page::Add, None, "", cx),
                                )
                                .justify_center()
                                .bg(rgba(0x3284edff)),
                            ),
                    );
            }
        }
        list.children(state.accounts.iter().enumerate().map(|(index, account)| {
            let current = state.current_id.as_deref() == Some(&account.id);
            self.account_card(account, index, current, colors, dark, window, cx)
        }))
        .children(state.renewed_ids.iter().filter_map(|id| {
            let account = state.accounts.iter().find(|account| account.id == *id)?;
            let id = id.clone();
            let current = state.current_id.as_ref() == Some(&id);
            Some(
                ca_notice(
                    colors,
                    locale.text("新登录已保存", "Renewed sign-in saved"),
                    format!(
                        "{} · {}",
                        account.name,
                        if current {
                            locale.text(
                                "还需应用到本机默认登录。",
                                "Apply it to the device default next.",
                            )
                        } else {
                            locale.text("需要时再切换。", "Switch when needed.")
                        }
                    ),
                )
                .child(self.ca_button(
                    format!("claude-renewed-{id}"),
                    locale.text("继续处理", "Continue"),
                    colors.blue,
                    window,
                    cx,
                    move |this, _, cx| {
                        this.account_page(Page::Renewed, Some(id.clone()), "", cx);
                    },
                )),
            )
        }))
    }

    fn account_card(
        &self,
        account: &ClaudeAccount,
        index: usize,
        current: bool,
        colors: Colors,
        dark: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let locale = self.locale;
        let a = account;
        let id = a.id.clone();
        let default = div()
            .px(px(9.))
            .py(px(3.))
            .rounded(px(7.))
            .text_size(px(10.))
            .bg(if dark {
                rgba(0x344c6aff)
            } else {
                rgba(0xe5efffff)
            })
            .text_color(colors.blue)
            .child(ui_text(locale.text("本机默认", "Default")));
        let renewed = self.wiring.accounts.state.renewed_ids.contains(&a.id);
        let identity = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(account_avatar(index, format!("{:02}", index + 1)))
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(
                        div()
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(ui_text(a.name.clone())),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(colors.muted)
                            .whitespace_nowrap()
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(ui_text(a.email.clone())),
                    ),
            )
            .children(current.then_some(default))
            .children((!current && !a.needs_login).then(|| {
                let id = id.clone();
                self.ca_button(
                    format!("claude-account-switch-{id}"),
                    locale.text("切换", "Switch"),
                    colors.blue,
                    window,
                    cx,
                    move |this, _, cx| {
                        if renewed {
                            this.account_page(Page::Renewed, Some(id.clone()), "", cx);
                        } else if this.wiring.accounts.state.remember_scope {
                            this.request_claude_account(
                                Action::Switch {
                                    id: id.clone(),
                                    remember_scope: true,
                                },
                                cx,
                            );
                        } else {
                            this.account_page(Page::Scope, Some(id.clone()), "", cx);
                        }
                    },
                )
                .border_1()
                .border_color(rgba(0x70a6f0ff))
                .bg(colors.bg)
            }));
        let quota = if a.needs_login {
            let (bg, text) = if dark {
                (rgba(0x382c22ff), rgba(0xf0b779ff))
            } else {
                (rgba(0xfff6e9ff), rgba(0x9c581eff))
            };
            div()
                .flex()
                .items_center()
                .p(px(11.))
                .gap(px(10.))
                .rounded(px(8.))
                .bg(bg)
                .text_color(text)
                .child(
                    div()
                        .size(px(17.))
                        .flex_none()
                        .rounded_full()
                        .bg(rgba(0xec902fff))
                        .text_color(rgba(0xffffffff))
                        .font_weight(FontWeight::BOLD)
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(ui_text("!")),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(ui_text(locale.text("登录已失效", "Sign-in expired"))),
                        )
                        .child(div().text_size(px(10.)).child(ui_text(
                            locale.text("重新登录后可刷新用量", "Sign in again to refresh usage"),
                        ))),
                )
                .child(
                    self.ca_button(
                        format!("claude-account-reauth-{id}"),
                        if renewed {
                            locale.text("继续处理", "Continue")
                        } else {
                            locale.text("重新登录", "Sign in again")
                        },
                        text,
                        window,
                        cx,
                        move |this, _, cx| {
                            this.account_page(
                                if renewed { Page::Renewed } else { Page::Reauth },
                                Some(id.clone()),
                                "",
                                cx,
                            )
                        },
                    )
                    .px(px(10.))
                    .border_1()
                    .border_color(rgba(0xdda56bff))
                    .bg(colors.bg),
                )
        } else {
            let usage = a.usage.as_ref();
            let status = usage.map_or_else(
                || {
                    locale
                        .text(
                            "使用此账户后自动记录用量",
                            "Usage is recorded after using this account",
                        )
                        .to_owned()
                },
                |usage| {
                    format!(
                        "{} {}",
                        if current && !usage.stale {
                            locale.text("更新于", "Updated")
                        } else {
                            locale.text("上次数据", "Last known")
                        },
                        account_date(usage.checked_at)
                    )
                },
            );
            div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .child(
                    div()
                        .flex()
                        .gap(px(16.))
                        .child(account_quota(
                            locale.text("5 小时已用", "5-hour used"),
                            usage.and_then(|u| u.five_hour.as_ref()),
                            locale,
                            colors,
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .pl(px(16.))
                                .border_l_1()
                                .border_color(colors.line)
                                .child(account_quota(
                                    locale.text("每周已用", "Weekly used"),
                                    usage.and_then(|u| u.weekly.as_ref()),
                                    locale,
                                    colors,
                                )),
                        ),
                )
                .child(
                    div()
                        .text_size(px(9.))
                        .line_height(px(14.))
                        .text_color(colors.muted)
                        .child(ui_text(status)),
                )
        };
        div()
            .relative()
            .flex()
            .flex_col()
            .gap(px(18.))
            .p(px(16.))
            .rounded(px(11.))
            .border_1()
            .border_color(if current {
                colors.active_border
            } else {
                colors.line
            })
            .bg(if current { colors.active } else { colors.bg })
            .children(wiring_probe_bounds(
                format!("claude-account-card-{}", a.id).into(),
            ))
            .child(identity)
            .child(quota)
    }

    fn account_form(&self, colors: Colors, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let locale = self.locale;
        let t = |zh, en| locale.text(zh, en);
        let ui = &self.wiring.accounts;
        let state = &ui.state;
        let selected = state
            .accounts
            .iter()
            .find(|a| Some(&a.id) == ui.selected.as_ref());
        let id = ui.selected.clone().unwrap_or_default();
        let mut form = div().flex().flex_col().gap(px(10.)).p(px(6.));
        let heading = |label: String| {
            div()
                .text_size(px(15.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(ui_text(label))
        };
        let paragraph = |label: &'static str| div().text_color(colors.muted).child(ui_text(label));
        let primary = |btn: Stateful<Div>| {
            btn.justify_center()
                .bg(rgba(0x3284edff))
                .text_color(rgba(0xffffffff))
        };
        let back = |this: &mut Self, _: &mut Window, cx: &mut Context<Self>| {
            this.account_page(Page::List, None, "", cx)
        };
        let input = ui.input.as_ref().map(|input| {
            div()
                .rounded(px(8.))
                .border_1()
                .border_color(colors.line)
                .bg(colors.field)
                .px(px(10.))
                .child(input.clone())
        });
        match ui.page {
            Page::Manage => {
                form = form
                    .child(paragraph(t(
                        "这里的名称只影响 Matou 显示。",
                        "Names here only affect Matou.",
                    )))
                    .children(state.accounts.iter().enumerate().map(|(index, account)| {
                        let id = account.id.clone();
                        let name = account.name.clone();
                        let email = if state.current_id.as_ref() == Some(&id) {
                            format!("{} · {}", account.email, t("本机默认", "Default"))
                        } else {
                            account.email.clone()
                        };
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .py(px(13.))
                            .border_b_1()
                            .border_color(colors.line)
                            .child(account_avatar(
                                index,
                                account.name.chars().take(1).collect(),
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(ui_text(account.name.clone())),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(colors.muted)
                                            .child(ui_text(email)),
                                    ),
                            )
                            .child(self.ca_button(
                                format!("claude-account-edit-{id}"),
                                t("编辑", "Edit"),
                                colors.blue,
                                window,
                                cx,
                                move |this, _, cx| {
                                    this.account_page(Page::Edit, Some(id.clone()), &name, cx)
                                },
                            ))
                    }))
                    .child(paragraph(t(
                        "移除会删除已保存的账户副本。",
                        "Removing deletes the saved sign-in copy.",
                    )))
                    .child(self.ca_button(
                        "claude-manage-add",
                        t("＋ 添加账户", "＋ Add account"),
                        colors.blue,
                        window,
                        cx,
                        |this, _, cx| this.account_page(Page::Add, None, "", cx),
                    ));
            }
            Page::Add | Page::Reauth => {
                let reauth = ui.page == Page::Reauth;
                form = form.child(heading(if let Some(a) = selected {
                    format!("{}「{}」", t("重新登录", "Sign in again "), a.name)
                } else { t("添加 Claude 账户", "Add a Claude account").into() }))
                    .child(paragraph(if reauth { t("请使用原账户重新授权。", "Authorize the original account.") }
                        else { t("将打开独立的临时隐私窗口。请在 Claude 官方页面登录要添加的账户。",
                            "Sign in to the account you want to add on the official Claude page in a separate private window.") }))
                    .child(primary(self.ca_button("claude-start-login", if reauth {
                        t("打开隐私窗口重新登录", "Open private window to sign in again")
                    } else { t("打开独立隐私窗口", "Open private sign-in window") }, colors.blue, window, cx,
                        move |this, _, cx| this.request_claude_account(Action::Login { replace_id: reauth.then(|| id.clone()) }, cx))));
            }
            Page::Login => {
                form = form
                    .child(heading(
                        t("在独立窗口登录另一个账户", "Sign in in the separate window").into(),
                    ))
                    .child(paragraph(t(
                        "请在浏览器中完成授权，然后在这里粘贴完整授权码。",
                        "Authorize in your browser, then paste the complete code here.",
                    )))
                    .children((!state.login_code_submitted).then_some(input).flatten());
                if state.login_code_submitted {
                    form = form.child(paragraph(t(
                        "授权码已提交，正在核对账户…",
                        "Code submitted. Checking the account…",
                    )));
                } else {
                    form = form.child(primary(self.ca_button(
                        "claude-submit-code",
                        t("提交授权码", "Submit code"),
                        colors.blue,
                        window,
                        cx,
                        |this, _, cx| this.submit_account_input(cx),
                    )));
                }
                form = form.child(self.ca_button(
                    "claude-cancel-login",
                    t("取消本次登录", "Cancel sign-in"),
                    colors.blue,
                    window,
                    cx,
                    |this, _, cx| this.request_claude_account(Action::CancelLogin, cx),
                ));
            }
            Page::Edit | Page::Draft | Page::Detected => {
                let account = match ui.page {
                    Page::Draft => state.draft.as_ref(),
                    Page::Detected => state.detected.as_ref(),
                    _ => selected,
                };
                let repairing = ui.page == Page::Draft && state.reauth_id.is_some();
                form = form
                    .child(heading(t("账户名称", "Account name").into()))
                    .children(account.map(|a| {
                        div()
                            .text_color(colors.muted)
                            .child(ui_text(a.email.clone()))
                    }))
                    .children((!repairing).then_some(input).flatten())
                    .child(primary(self.ca_button(
                        "claude-save-name",
                        if repairing {
                            t("保存新登录", "Save renewed sign-in")
                        } else {
                            t("保存账户", "Save account")
                        },
                        colors.blue,
                        window,
                        cx,
                        |this, _, cx| this.submit_account_input(cx),
                    )));
                if ui.page == Page::Draft {
                    form = form.child(self.ca_button(
                        "claude-discard-login",
                        t("放弃本次登录", "Discard sign-in"),
                        colors.blue,
                        window,
                        cx,
                        |this, _, cx| this.request_claude_account(Action::CancelLogin, cx),
                    ));
                }
                if ui.page == Page::Edit {
                    let id = id.clone();
                    form = form.child(
                        div()
                            .flex()
                            .justify_between()
                            .child(self.ca_button(
                                "claude-edit-reauth",
                                t("重新登录", "Sign in again"),
                                colors.blue,
                                window,
                                cx,
                                move |this, _, cx| {
                                    this.account_page(Page::Reauth, Some(id.clone()), "", cx)
                                },
                            ))
                            .child(self.ca_button(
                                "claude-remove-account",
                                t("移除账户…", "Remove account…"),
                                rgba(0xc05a58ff),
                                window,
                                cx,
                                |this, _, cx| {
                                    this.account_page(
                                        Page::Remove,
                                        this.wiring.accounts.selected.clone(),
                                        "",
                                        cx,
                                    )
                                },
                            )),
                    );
                }
            }
            Page::Remove | Page::Scope | Page::Renewed => {
                let name = selected.map_or("", |a| a.name.as_str());
                let is_remove = ui.page == Page::Remove;
                let apply =
                    ui.page == Page::Renewed && state.current_id.as_deref() == Some(id.as_str());
                form = form.child(heading(if is_remove { format!("{}「{name}」？", t("移除", "Remove ")) }
                    else if apply { t("新登录已保存", "Renewed sign-in saved").into() }
                    else { format!("{}「{name}」", t("切换到", "Switch to ")) }))
                    .child(paragraph(if is_remove {
                        t("将删除保存的账户登录。下次添加需要重新登录。", "This deletes the saved sign-in. You will need to sign in to add it again.")
                    } else { t("共享默认登录的 Claude 实例都会受影响，包括应用外的终端和编辑器。现有任务继续运行。",
                        "Claude instances sharing the device default are affected, including external terminals and editors. Existing tasks keep running.") }));
                if !is_remove && !apply {
                    let check = if ui.remember { "☑" } else { "☐" };
                    form = form.child(self.ca_button(
                        "claude-remember-scope",
                        format!(
                            "{check} {}",
                            t(
                                "已了解范围，下次直接切换",
                                "Remember this scope and switch directly next time"
                            )
                        ),
                        colors.text,
                        window,
                        cx,
                        |this, _, cx| {
                            this.wiring.accounts.remember = !this.wiring.accounts.remember;
                            cx.notify();
                        },
                    ));
                }
                form = form.child(
                    div()
                        .flex()
                        .justify_end()
                        .gap(px(7.))
                        .child(self.ca_button(
                            "claude-action-back",
                            t("暂不处理", "Not now"),
                            colors.blue,
                            window,
                            cx,
                            back,
                        ))
                        .child(
                            primary(self.ca_button(
                                "claude-confirm-action",
                                if is_remove {
                                    t("确认移除", "Remove account")
                                } else if apply {
                                    t("应用到本机默认登录", "Apply to device default")
                                } else {
                                    t("立即切换", "Switch now")
                                },
                                colors.blue,
                                window,
                                cx,
                                move |this, _, cx| {
                                    let id = id.clone();
                                    let action = if is_remove {
                                        Action::Remove { id }
                                    } else if apply {
                                        Action::Reauth { id }
                                    } else {
                                        Action::Switch {
                                            id,
                                            remember_scope: this.wiring.accounts.remember,
                                        }
                                    };
                                    this.request_claude_account(action, cx);
                                },
                            ))
                            .when(is_remove, |button| button.bg(rgba(0xc05a58ff))),
                        ),
                );
            }
            Page::List => {}
        }
        form
    }
}

fn account_input(
    text: &str,
    placeholder: &str,
    dark: bool,
    cx: &mut Context<Workspace>,
) -> Entity<RenameInput> {
    cx.new(|cx| {
        let colors = Colors::new(dark);
        let mut input = RenameInput::new(text.into(), cx);
        input.search_style = true;
        input.focus = input.focus.clone().tab_stop(true);
        input.placeholder = placeholder.into();
        input.placeholder_color = Some(colors.muted);
        input.text_color = Some(colors.text);
        input
    })
}

fn account_refresh_icon(color: Rgba) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let at = |x: f32, y: f32| bounds.origin + point(px(x * 0.6), px(y * 0.6));
            let mut path = PathBuilder::stroke(px(1.2));
            path.move_to(at(20., 7.));
            path.line_to(at(20., 12.));
            path.line_to(at(15., 12.));
            path.move_to(at(4., 17.));
            path.line_to(at(4., 12.));
            path.line_to(at(9., 12.));
            path.move_to(at(6.1, 6.1));
            path.arc_to(point(px(4.8), px(4.8)), px(0.), false, true, at(20., 12.));
            path.move_to(at(4., 12.));
            path.arc_to(
                point(px(4.8), px(4.8)),
                px(0.),
                false,
                false,
                at(17.9, 17.9),
            );
            if let Ok(path) = path.build() {
                window.paint_path(path, color);
            }
        },
    )
    .size(px(14.4))
    .flex_none()
}

fn account_avatar(index: usize, text: String) -> Div {
    let (bg, fg) = [
        (rgba(0xe9f1fcff), rgba(0x658fbeff)),
        (rgba(0xedf4edff), rgba(0x75917aff)),
        (rgba(0xf4eee6ff), rgba(0xa48660ff)),
    ][index % 3];
    div()
        .size(px(30.))
        .flex_none()
        .rounded(px(8.))
        .bg(bg)
        .text_color(fg)
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(13.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(ui_text(text))
}

fn account_quota(
    label: &'static str,
    window: Option<&UsageWindow>,
    locale: InterfaceLocale,
    colors: Colors,
) -> Div {
    let percent = window.map(|usage| usage.used.clamp(0., 100.));
    let tone = match percent {
        Some(value) if value >= 100. => rgba(0xcf5856ff),
        Some(value) if value >= 90. => rgba(0xbe822aff),
        _ => rgba(0x526d91ff),
    };
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .text_color(colors.muted)
        .text_size(px(11.))
        .line_height(px(17.))
        .child(
            div()
                .flex()
                .justify_between()
                .gap(px(5.))
                .child(ui_text(label))
                .child(
                    div()
                        .text_color(tone)
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(ui_text(
                            percent.map_or_else(|| "—".into(), |value| format!("{value:.0}%")),
                        )),
                ),
        )
        .children(percent.map(|value| {
            let reset = window.and_then(UsageWindow::reset_timestamp);
            let reset_label = if reset.is_some_and(|reset| reset <= notification_now()) {
                locale.text("已到重置时间，待更新", "reset time passed; awaiting update")
            } else {
                locale.text("重置", "reset")
            };
            div()
                .flex()
                .flex_col()
                .gap(px(7.))
                .pt(px(7.))
                .child(
                    div()
                        .h(px(5.))
                        .rounded(px(4.))
                        .bg(colors.line)
                        .overflow_hidden()
                        .child(
                            div()
                                .h_full()
                                .w(relative(value as f32 / 100.))
                                .rounded(px(4.))
                                .bg(if value >= 90. { tone } else { rgba(0x3284edff) }),
                        ),
                )
                .child(
                    div()
                        .text_size(px(9.))
                        .line_height(px(14.))
                        .child(ui_text(format!(
                            "{} {reset_label}",
                            reset.map_or_else(|| "—".into(), account_date)
                        ))),
                )
        }))
}

fn account_date(timestamp: u64) -> String {
    let (date, time) = local_date_time(timestamp);
    format!("{date} {time}")
}

fn ca_notice(colors: Colors, title: &'static str, detail: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .p(px(10.))
        .rounded(px(8.))
        .bg(colors.active)
        .border_1()
        .border_color(colors.active_border)
        .child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(ui_text(title)),
        )
        .child(div().text_color(colors.muted).child(ui_text(detail.into())))
}

fn account_error(locale: InterfaceLocale, error: &str) -> &'static str {
    let (zh, en) = match error {
        "engine-missing" => (
            "账户切换组件尚未就绪。开发版请运行 pnpm accounts:setup；正式版请重新安装完整应用。",
            "The account component is missing. In development, run pnpm accounts:setup; otherwise reinstall the full app.",
        ),
        "engine-version" => (
            "账户组件版本不匹配，请更新完整应用。",
            "The account engine version does not match. Update the full app.",
        ),
        "network" => (
            "网络暂时没有响应，请稍后重试。",
            "The network did not respond. Try again shortly.",
        ),
        "login-required" => (
            "登录已失效，请重新登录这个账户。",
            "Sign in to this account again.",
        ),
        "login-timeout" => (
            "登录等待已结束。你可以重新打开登录流程。",
            "Sign-in timed out. Start again when ready.",
        ),
        "login-code-invalid" => (
            "请粘贴浏览器提供的完整授权码（包含 # 后面的部分）。",
            "Paste the complete authorization code, including the part after #.",
        ),
        "login-ended" => (
            "这次登录已结束，请重新打开登录流程并获取新的授权码。",
            "This sign-in has ended. Start again to get a new authorization code.",
        ),
        "login-failed" => (
            "登录尚未完成，请重试并在浏览器中完成授权。",
            "Sign-in did not finish. Try again and complete it in your browser.",
        ),
        "browser-closed" => (
            "登录窗口已关闭。可以重新开始本次登录。",
            "The sign-in window was closed. Try signing in again.",
        ),
        "browser-missing" => (
            "账户登录需要 Chrome、Edge 或 Brave。安装其中一个后重试。",
            "Install Chrome, Edge or Brave to add an account in an isolated window.",
        ),
        "cli-missing" => (
            "请先安装 Claude Code，再登录账户。",
            "Install Claude Code before adding an account.",
        ),
        "storage" => (
            "账户偏好保存失败，请检查磁盘空间后重试。",
            "Preferences were not saved. Check disk space and retry.",
        ),
        "keychain" => (
            "请解锁系统钥匙串，并允许账户组件访问后重试。",
            "Unlock the system keychain and allow access, then retry.",
        ),
        "busy" => (
            "登录或会话配置正在更新，请稍后重试。",
            "Sign-in or session configuration is being updated. Retry shortly.",
        ),
        "identity" => (
            "登录的账户与选择的账户不一致，请核对后重新登录。",
            "The signed-in identity differs from the selected account. Check it and sign in again.",
        ),
        "conflict" => (
            "账户状态已在别处更新，请刷新后重新选择。",
            "Accounts changed elsewhere. Refresh and select again.",
        ),
        "invalid" => (
            "请检查名称和所选账户。",
            "Check the name and selected account.",
        ),
        "unsupported" => (
            "当前使用自定义 Claude 配置。请使用默认配置启动 Matou 后切换本机账户。",
            "Launch Matou with the default Claude profile to switch the device account.",
        ),
        "switch-failed" => (
            "默认账户尚未确认切换成功，请刷新后重试。",
            "The default-account change is not confirmed. Refresh and retry.",
        ),
        "recovery-required" => (
            "上次操作需要检查。请刷新账户状态后继续。",
            "The previous operation needs checking. Refresh before continuing.",
        ),
        "runtime" => (
            "正在连接会话服务，请稍后重试。",
            "Connecting to the session service. Retry shortly.",
        ),
        _ => (
            "账户组件未完成本次操作。请刷新状态；若仍提示登录失效，请重新登录。",
            "Refresh the account status and try again. Sign in again if the account requires it.",
        ),
    };
    locale.text(zh, en)
}
