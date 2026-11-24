use matou_native_core::sessions::InterfaceLocale;

const MENU_PADDING: f32 = 6.;
const MENU_BORDER: f32 = 1.;
const MENU_ITEM_HEIGHT: f32 = 28.;
const DANGER_TOP_GAP: f32 = 4.;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardMenuAction {
    Rename,
    Fork,
    ForkWorktree,
    OpenDetached,
    ReturnToMain,
    ClosePane,
    RemoveNode,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CardMenuEvent {
    Dismiss,
    Activate(CardMenuAction),
    ReadinessHint(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CardPresentation {
    #[default]
    Attached,
    Archived,
    Detached,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForkReadiness {
    Ready,
    WaitingForReply,
    NeedsFirstReply,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CardMenuContext<'a> {
    /// Session kind from the hierarchy, such as `shell` or `claude-code`.
    pub kind: &'a str,
    pub presentation: CardPresentation,
    pub can_rename: bool,
    /// Readiness is present when the caller supplies the peer Fork action.
    pub fork: Option<ForkReadiness>,
    pub git_available: bool,
    pub can_detach: bool,
    /// The hierarchy has approved removal for this node and its descendants.
    pub can_remove_node: bool,
    pub read_only: bool,
    pub storage_fault: bool,
    pub environment_unavailable: bool,
    pub recovering: bool,
}

impl CardMenuContext<'_> {
    fn blocked_reason(self, locale: InterfaceLocale) -> Option<&'static str> {
        if self.read_only {
            Some(locale.text(
                "数据库处于只读恢复模式",
                "The database is in read-only recovery mode",
            ))
        } else if self.storage_fault {
            Some(locale.text(
                "终端存储异常，请先恢复或结束当前会话",
                "Terminal storage failed. Recover or end the current session first.",
            ))
        } else if self.environment_unavailable {
            Some(locale.text(
                "当前运行环境需要先恢复或交接",
                "The current environment must be recovered or handed off first.",
            ))
        } else if self.recovering {
            Some(locale.text("当前终端仍在恢复", "This terminal is still recovering."))
        } else {
            None
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardMenuItem {
    pub action: CardMenuAction,
    pub label: String,
    pub disabled_reason: Option<String>,
    /// A click on a temporarily unavailable Fork can surface its explanation.
    pub show_readiness_hint: bool,
    pub danger: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardMenu {
    pub items: Vec<CardMenuItem>,
    opens_on_header: bool,
}

impl CardMenu {
    pub fn can_open(&self) -> bool {
        self.opens_on_header && !self.items.is_empty()
    }
}

pub fn resolve_card_menu(context: CardMenuContext<'_>, locale: InterfaceLocale) -> CardMenu {
    use CardMenuAction::*;

    // The detached placeholder can close a child even in read-only recovery.
    let blocked = if context.presentation == CardPresentation::Detached {
        None
    } else {
        context.blocked_reason(locale)
    };
    let mut items = Vec::new();
    let mut add = |action, label: String, reason: Option<String>, show_readiness_hint| {
        items.push(CardMenuItem {
            action,
            label,
            disabled_reason: blocked.map(str::to_owned).or(reason),
            show_readiness_hint: blocked.is_none() && show_readiness_hint,
            danger: action == RemoveNode,
        });
    };
    match context.presentation {
        CardPresentation::Detached => {
            add(
                ReturnToMain,
                locale.text("归还到当前页签", "Return to this tab").into(),
                None,
                false,
            );
        }
        CardPresentation::Archived => {
            if context.can_remove_node {
                add(
                    RemoveNode,
                    locale.text("移除节点…", "Remove node…").into(),
                    None,
                    false,
                );
            }
        }
        CardPresentation::Attached => {
            let fork = context.fork.and_then(|readiness| {
                let agent = match context.kind {
                    "claude-code" => "Claude Code",
                    "codex" => "Codex",
                    "antigravity" => "Antigravity",
                    _ => return None,
                };
                Some(fork_disabled_reason(readiness, agent, locale))
            });
            if let Some(reason) = &fork {
                add(
                    Fork,
                    locale.text("⑂ Fork 会话", "⑂ Fork session").into(),
                    reason.clone(),
                    reason.is_some(),
                );
            }
            if context.can_rename {
                add(
                    Rename,
                    locale.text("重命名…", "Rename…").into(),
                    None,
                    false,
                );
            }
            if context.can_detach {
                add(
                    OpenDetached,
                    locale.text("↗ 独立窗口", "↗ Detached window").into(),
                    None,
                    false,
                );
            }
            if let Some(reason) = fork {
                let hint = reason.is_some();
                let worktree_reason = reason.or_else(|| {
                    (!context.git_available).then(|| {
                        locale
                            .text("需要 Git 仓库", "Requires a Git repository")
                            .into()
                    })
                });
                add(
                    ForkWorktree,
                    locale
                        .text("从新工作树创建", "Create from a new worktree")
                        .into(),
                    worktree_reason,
                    hint,
                );
            }
        }
    }
    CardMenu {
        items,
        // Stopped cards expose removal on their own header button.
        opens_on_header: context.presentation != CardPresentation::Archived && blocked.is_none(),
    }
}

fn fork_disabled_reason(
    readiness: ForkReadiness,
    agent: &str,
    locale: InterfaceLocale,
) -> Option<String> {
    match readiness {
        ForkReadiness::Ready => None,
        ForkReadiness::WaitingForReply => Some(
            locale
                .text(
                    "当前回复完成后即可 Fork",
                    "You can fork once the current reply finishes",
                )
                .into(),
        ),
        ForkReadiness::NeedsFirstReply => Some(match locale {
            InterfaceLocale::Chinese => {
                format!("在当前会话输入一次，并等待 {agent} 完成回复后，即可创建分支")
            }
            InterfaceLocale::English => format!(
                "Send one message in this session and wait for {agent} to finish replying, then you can fork"
            ),
        }),
    }
}

fn card_menu_height(items: &[CardMenuItem]) -> f32 {
    let separators = items.iter().filter(|item| item.danger).count() as f32;
    2. * (MENU_PADDING + MENU_BORDER)
        + items.len() as f32 * MENU_ITEM_HEIGHT
        + separators * (DANGER_TOP_GAP + MENU_BORDER)
}

/// Clamp the menu's top-left corner to the window's viewport.
pub fn clamp_menu_origin(
    anchor: (f32, f32),
    menu_size: (f32, f32),
    viewport: (f32, f32),
) -> (f32, f32) {
    (
        anchor.0.clamp(0., (viewport.0 - menu_size.0).max(0.)),
        anchor.1.clamp(0., (viewport.1 - menu_size.1).max(0.)),
    )
}

mod view {
    use std::rc::Rc;

    use gpui::{prelude::*, *};

    use super::*;
    use crate::ui_text::ui_text;
    use crate::workspace::{PopoverColors, card_tooltip_box, popover_colors};

    const MENU_WIDTH: f32 = 190.;

    type CardMenuHandler = Rc<dyn Fn(CardMenuEvent, &mut Window, &mut App)>;

    /// Render at the window root above the canvas. The caller assigns `focus`
    /// when opening and restores the previous focus after handling an event.
    pub fn card_context_menu(
        menu: &CardMenu,
        position: gpui::Point<gpui::Pixels>,
        viewport: gpui::Size<gpui::Pixels>,
        focus: &gpui::FocusHandle,
        light: bool,
        handler: impl Fn(CardMenuEvent, &mut gpui::Window, &mut gpui::App) + 'static,
    ) -> Option<gpui::AnyElement> {
        card_context_menu_with_selection(menu, position, viewport, focus, light, None, handler)
    }

    pub fn card_context_menu_with_selection(
        menu: &CardMenu,
        position: gpui::Point<gpui::Pixels>,
        viewport: gpui::Size<gpui::Pixels>,
        focus: &gpui::FocusHandle,
        light: bool,
        selected: Option<usize>,
        handler: impl Fn(CardMenuEvent, &mut gpui::Window, &mut gpui::App) + 'static,
    ) -> Option<gpui::AnyElement> {
        if !menu.can_open() {
            return None;
        }

        let handler: CardMenuHandler = Rc::new(handler);
        let viewport_width = f32::from(viewport.width);
        let viewport_height = f32::from(viewport.height);
        let width = MENU_WIDTH.min(viewport_width).max(0.);
        let menu_height = card_menu_height(&menu.items);
        let rendered_height = menu_height.min(viewport_height).max(0.);
        let scrollable = menu_height > rendered_height;
        let origin = clamp_menu_origin(
            (f32::from(position.x), f32::from(position.y)),
            (width, rendered_height),
            (viewport_width, viewport_height),
        );

        let dark = !light;
        let colors = popover_colors(dark);
        let menu_shadow = colors.card_menu_shadow();

        let escape_handler = handler.clone();
        let mut menu_panel = div()
            .id("card-context-menu")
            .absolute()
            .flex()
            .flex_col()
            .left(px(origin.0))
            .top(px(origin.1))
            .w(px(width))
            .p(px(MENU_PADDING))
            .rounded(px(10.))
            .border_1()
            .border_color(colors.card_menu_border)
            .bg(colors.card_menu_panel)
            .text_color(colors.card_menu_text)
            .shadow(vec![menu_shadow])
            .track_focus(focus)
            .on_key_down(move |event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    cx.stop_propagation();
                    escape_handler(CardMenuEvent::Dismiss, window, cx);
                }
            })
            .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
            .on_click(|_, _, cx| cx.stop_propagation());
        if scrollable {
            menu_panel = menu_panel.h(px(rendered_height)).overflow_y_scroll();
        }
        let menu_panel = menu_panel.children(menu.items.iter().enumerate().map(|(index, item)| {
            menu_row(index, item, colors, dark, handler.clone())
                .when(selected == Some(index), |row| {
                    row.bg(colors.card_menu_hover)
                })
        }));

        Some(
            div()
                .id("card-context-menu-overlay")
                .absolute()
                .inset_0()
                .occlude()
                .on_any_mouse_down(move |_, window, cx| {
                    cx.stop_propagation();
                    handler(CardMenuEvent::Dismiss, window, cx);
                })
                .child(menu_panel)
                .into_any_element(),
        )
    }

    fn menu_row(
        index: usize,
        item: &CardMenuItem,
        colors: PopoverColors,
        dark: bool,
        handler: CardMenuHandler,
    ) -> Stateful<Div> {
        let readiness_hint = item.show_readiness_hint && item.disabled_reason.is_some();
        let permanently_disabled = item.disabled_reason.is_some() && !readiness_hint;
        let text_color = if item.danger {
            colors.card_menu_danger_text
        } else {
            colors.card_menu_text
        };

        let mut row = div()
            .id(SharedString::from(format!("card-menu-item-{index}")))
            .flex()
            .flex_none()
            .items_center()
            .w_full()
            .h(px(MENU_ITEM_HEIGHT))
            .px(px(10.))
            .rounded(px(6.))
            .bg(rgba(0x00000000))
            .text_size(px(12.))
            .text_color(text_color)
            .whitespace_nowrap()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(ui_text(item.label.clone())),
            );
        if item.danger {
            row = row
                .h(px(MENU_ITEM_HEIGHT + MENU_BORDER))
                .mt(px(DANGER_TOP_GAP))
                .border_t_1()
                .border_color(colors.card_menu_danger_border)
                .rounded(px(6.))
                .rounded_t(px(0.));
        }
        if let Some(reason) = &item.disabled_reason {
            let reason = reason.clone();
            row = row.tooltip(move |_, cx| {
                cx.new(|_| CardMenuTooltip(reason.clone().into(), dark))
                    .into()
            });
        }

        if permanently_disabled {
            // A permanent gate never activates and never closes the menu; the
            // tooltip is its only explanation.
            row.opacity(0.38).cursor(CursorStyle::OperationNotAllowed)
        } else if readiness_hint {
            let reason = item.disabled_reason.clone().unwrap_or_default();
            let activate_hint = handler;
            row.opacity(0.38)
                .cursor_pointer()
                .hover(move |style| style.bg(colors.card_menu_hover))
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    activate_hint(CardMenuEvent::ReadinessHint(reason.clone()), window, cx);
                })
        } else {
            let action = item.action;
            let activate = handler;
            row.cursor_pointer()
                .hover(move |style| style.bg(colors.card_menu_hover))
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    activate(CardMenuEvent::Activate(action), window, cx);
                })
        }
    }

    struct CardMenuTooltip(SharedString, bool);

    impl Render for CardMenuTooltip {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            card_tooltip_box(self.1)
                .max_w(px(360.))
                .whitespace_normal()
                .child(ui_text(self.0.clone()))
        }
    }
}

pub use view::{card_context_menu, card_context_menu_with_selection};

#[cfg(test)]
mod tests {
    use super::*;
    use CardMenuAction::*;

    fn context(kind: &str) -> CardMenuContext<'_> {
        CardMenuContext {
            kind,
            can_rename: true,
            fork: Some(ForkReadiness::Ready),
            git_available: true,
            can_detach: true,
            can_remove_node: true,
            ..Default::default()
        }
    }

    fn actions(menu: &CardMenu) -> Vec<CardMenuAction> {
        menu.items.iter().map(|item| item.action).collect()
    }

    #[test]
    fn shell_menu_has_no_provider_actions() {
        let menu = resolve_card_menu(context("shell"), InterfaceLocale::Chinese);
        assert!(menu.can_open());
        assert_eq!(actions(&menu), [Rename, OpenDetached]);
        assert!(menu.items.iter().all(|item| item.disabled_reason.is_none()));
    }

    #[test]
    fn attached_agent_menu_has_four_items_in_order() {
        for kind in ["claude-code", "codex", "antigravity"] {
            let menu = resolve_card_menu(context(kind), InterfaceLocale::Chinese);
            assert_eq!(actions(&menu), [Fork, Rename, OpenDetached, ForkWorktree]);
            assert!(menu.items.iter().all(|item| !item.danger));
            assert_eq!(card_menu_height(&menu.items), 126.);
            assert_eq!(menu.items[1].label, "重命名…");
            assert_eq!(menu.items[3].label, "从新工作树创建");
        }
    }

    #[test]
    fn codex_forks_use_provider_readiness() {
        let menu = resolve_card_menu(
            CardMenuContext {
                fork: Some(ForkReadiness::NeedsFirstReply),
                ..context("codex")
            },
            InterfaceLocale::English,
        );
        assert_eq!(actions(&menu), [Fork, Rename, OpenDetached, ForkWorktree]);
        for item in [&menu.items[0], &menu.items[3]] {
            assert!(item.show_readiness_hint);
            assert_eq!(
                item.disabled_reason.as_deref(),
                Some(
                    "Send one message in this session and wait for Codex to finish replying, then you can fork"
                ),
            );
        }
    }

    #[test]
    fn dsh_does_not_offer_forks() {
        let menu = resolve_card_menu(context("dsh"), InterfaceLocale::English);
        assert_eq!(actions(&menu), [Rename, OpenDetached]);
    }

    #[test]
    fn an_attached_card_without_supported_actions_has_no_menu() {
        let menu = resolve_card_menu(CardMenuContext::default(), InterfaceLocale::English);
        assert!(!menu.can_open());
        assert!(menu.items.is_empty());
    }

    #[test]
    fn an_archived_card_keeps_approved_removal_on_its_header_button() {
        let archived = CardMenuContext {
            presentation: CardPresentation::Archived,
            ..context("claude-code")
        };
        let menu = resolve_card_menu(archived, InterfaceLocale::English);
        assert!(!menu.can_open());
        assert_eq!(actions(&menu), [RemoveNode]);
        assert!(menu.items[0].danger);
        let menu = resolve_card_menu(
            CardMenuContext {
                can_remove_node: false,
                ..archived
            },
            InterfaceLocale::English,
        );
        assert!(!menu.can_open());
        assert!(menu.items.is_empty());
    }

    #[test]
    fn read_only_prevents_the_attached_menu_from_opening() {
        let menu = resolve_card_menu(
            CardMenuContext {
                read_only: true,
                storage_fault: true,
                ..context("claude-code")
            },
            InterfaceLocale::English,
        );
        assert!(!menu.can_open());
        assert_eq!(actions(&menu), [Fork, Rename, OpenDetached, ForkWorktree]);
        for item in menu.items {
            assert_eq!(
                item.disabled_reason.as_deref(),
                Some("The database is in read-only recovery mode")
            );
            assert!(!item.show_readiness_hint);
        }
    }

    #[test]
    fn recovering_card_uses_the_terminal_blocker_before_the_fork_hint() {
        let menu = resolve_card_menu(
            CardMenuContext {
                recovering: true,
                fork: Some(ForkReadiness::WaitingForReply),
                ..context("claude-code")
            },
            InterfaceLocale::Chinese,
        );
        assert!(!menu.can_open());
        assert_eq!(actions(&menu), [Fork, Rename, OpenDetached, ForkWorktree]);
        for item in menu.items {
            assert_eq!(item.disabled_reason.as_deref(), Some("当前终端仍在恢复"));
            assert!(!item.show_readiness_hint);
        }
    }

    #[test]
    fn already_detached_card_only_offers_return_in_both_locales() {
        let detached = CardMenuContext {
            presentation: CardPresentation::Detached,
            read_only: true,
            ..context("claude-code")
        };
        for (locale, label) in [
            (InterfaceLocale::Chinese, "归还到当前页签"),
            (InterfaceLocale::English, "Return to this tab"),
        ] {
            let menu = resolve_card_menu(detached, locale);
            assert!(menu.can_open());
            assert_eq!(actions(&menu), [ReturnToMain]);
            assert_eq!(menu.items[0].label, label);
            assert_eq!(menu.items[0].disabled_reason, None);
        }
    }

    #[test]
    fn worktree_capability_and_current_reply_have_distinct_disabled_hints() {
        let no_git = CardMenuContext {
            git_available: false,
            ..context("codex")
        };
        let menu = resolve_card_menu(no_git, InterfaceLocale::English);
        assert_eq!(menu.items[0].disabled_reason, None);
        assert_eq!(
            menu.items[3].disabled_reason.as_deref(),
            Some("Requires a Git repository")
        );
        assert!(!menu.items[3].show_readiness_hint);
        let menu = resolve_card_menu(
            CardMenuContext {
                fork: Some(ForkReadiness::WaitingForReply),
                ..no_git
            },
            InterfaceLocale::Chinese,
        );
        assert_eq!(
            menu.items[3].disabled_reason.as_deref(),
            Some("当前回复完成后即可 Fork")
        );
        assert!(menu.items[3].show_readiness_hint);
    }

    #[test]
    fn menu_omits_actions_missing_from_its_caller_and_uses_localized_labels() {
        let menu = resolve_card_menu(
            CardMenuContext {
                can_rename: true,
                can_detach: true,
                ..Default::default()
            },
            InterfaceLocale::English,
        );
        assert_eq!(actions(&menu), [Rename, OpenDetached]);
        assert_eq!(menu.items[0].label, "Rename…");
        assert_eq!(menu.items[1].label, "↗ Detached window");
        let menu = resolve_card_menu(context("shell"), InterfaceLocale::Chinese);
        assert_eq!(menu.items[0].label, "重命名…");
        assert_eq!(menu.items[1].label, "↗ 独立窗口");
    }

    #[test]
    fn menu_position_stays_in_bounds_at_edges_and_in_small_windows() {
        let menu = (170., 100.);
        let viewport = (800., 600.);
        assert_eq!(clamp_menu_origin((12., 30.), menu, viewport), (12., 30.));
        assert_eq!(
            clamp_menu_origin((799., 599.), menu, viewport),
            (630., 500.)
        );
        assert_eq!(clamp_menu_origin((-6., -8.), menu, viewport), (0., 0.));
        assert_eq!(clamp_menu_origin((40., 35.), menu, (80., 70.)), (0., 0.));
    }
}
