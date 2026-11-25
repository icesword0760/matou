// Frozen `DeferredRecoveryCard.tsx` (main@924de28d) and
// `.session-recovery-overlay.state-deferred` / `.session-deferred-card*`
// (hierarchy.css) ported to GPUI. Every string below is the frozen zh-CN
// `hierarchyTerminal.pane` copy; the overlay fills the terminal body and does
// not read or mutate any session data.

use super::AppTheme;
use crate::ui_text::ui_text;
use gpui::{prelude::*, *};

const DEFERRED_BADGE: &str = "会话尚未启动";
const DEFERRED_BODY: &str = "启动时仅自动恢复最近使用的 16 条会话。此会话的记录已保留。";
const DEFERRED_HISTORY: &str = "会话历史";
const DEFERRED_RESTORE: &str = "点击恢复";
const DEFERRED_REPLIES: &str = "最后几条回复";
const DEFERRED_LOADING: &str = "正在读取最近对话…";

/// Frozen dark rules plus the `[data-theme="light"]` overrides from
/// hierarchy.css. The deferred card has no light overrides for `__note`,
/// `__hint`, or the primary button, so those inherit the overlay `p` color or
/// keep their dark base colors.
#[derive(Clone, Copy)]
struct DeferredColors {
    overlay_background: u32,
    overlay_text: u32,
    card_border: u32,
    card_background: u32,
    card_shadow: u32,
    card_shadow_alpha: f32,
    note: u32,
    badge_text: u32,
    badge_background: u32,
    agent: u32,
    time_cwd: u32,
    title: u32,
    subtitle: u32,
    replies_border: u32,
    replies_background: u32,
    replies_label: u32,
    role: u32,
    role_background: u32,
    role_user: u32,
    role_user_background: u32,
    role_muted: u32,
    role_muted_background: u32,
    reply_text: u32,
    secondary_border: u32,
    secondary_text: u32,
    secondary_background: u32,
}

impl DeferredColors {
    const DARK: Self = Self {
        overlay_background: 0x0d1117e0,
        overlay_text: 0xdbe6f5,
        card_border: 0x7da1d14d,
        card_background: 0x181f29f5,
        card_shadow: 0x000000,
        card_shadow_alpha: 0.34,
        note: 0x93a2b7,
        badge_text: 0xb7d2f7,
        badge_background: 0x7da1d12e,
        agent: 0x9ab0cc,
        time_cwd: 0x7f91a8,
        title: 0xf0f6fc,
        subtitle: 0x9ab0cc,
        replies_border: 0x7da1d133,
        replies_background: 0x0d11177a,
        replies_label: 0x8da2be,
        role: 0x9fc4f6,
        role_background: 0x79b8ff24,
        role_user: 0xd5e6ff,
        role_user_background: 0x5c9ced38,
        role_muted: 0x9eb0c6,
        role_muted_background: 0x8c9eb529,
        reply_text: 0xcbd7e6,
        secondary_border: 0x7da1d14d,
        secondary_text: 0xb7c8dd,
        secondary_background: 0x00000000,
    };

    const LIGHT: Self = Self {
        overlay_background: 0xf4f7fbeb,
        overlay_text: 0x29364a,
        card_border: 0xc8d4e3ff,
        card_background: 0xfffffffa,
        card_shadow: 0x1f2d3d,
        card_shadow_alpha: 0.14,
        note: 0x617086,
        badge_text: 0x365070,
        badge_background: 0xeaf0f7ff,
        agent: 0x4c5d75,
        time_cwd: 0x6e7f96,
        title: 0x1b2533,
        subtitle: 0x54647a,
        replies_border: 0xdce4eeff,
        replies_background: 0xf6f8fbff,
        replies_label: 0x66778d,
        role: 0x24578a,
        role_background: 0xe4effbff,
        role_user: 0x204d80,
        role_user_background: 0xdceaffff,
        role_muted: 0x55667c,
        role_muted_background: 0xe8edf3ff,
        reply_text: 0x2b3748,
        secondary_border: 0xc7d3e2ff,
        secondary_text: 0x3d4d63,
        secondary_background: 0xffffffff,
    };
}

impl super::Workspace {
    /// `.session-recovery-overlay.state-deferred` mounted by workspace.rs as an
    /// absolute overlay inside the terminal body. The caller owns the card data
    /// and the two actions; this method only draws and routes input.
    pub(super) fn render_deferred_recovery_card(
        &self,
        id: &str,
        card: super::DeferredRecoveryCard,
        width: f32,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let super::DeferredRecoveryCard {
            title,
            subtitle,
            cwd,
            agent,
            updated_label,
            replies,
            loading,
            can_open_history,
            can_restore,
        } = card;

        // The workspace owns these handles so focus can return to the same
        // button after the session-history dialog closes.
        let [history_focus, restore_focus] = &self.deferred_controls[id];

        let colors = match self.theme.effective() {
            AppTheme::Light => DeferredColors::LIGHT,
            AppTheme::Dark => DeferredColors::DARK,
        };

        // `.session-deferred-card { width: min(460px, calc(100% - 16px)) }`
        // inside a 16px overlay padding: the measured terminal body width loses
        // both paddings plus that 16px inset.
        let card_width = (width - 48.).clamp(0., 460.);

        // `.session-deferred-card__meta`, `__badge`, `__agent`, `__time`.
        let mut meta = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(6.))
            .text_size(px(10.))
            .line_height(relative(1.2))
            .child(
                div()
                    .flex_none()
                    .px(px(7.))
                    .py(px(2.))
                    .rounded_full()
                    .bg(rgba(colors.badge_background))
                    .text_color(rgb(colors.badge_text))
                    .font_weight(FontWeight::SEMIBOLD)
                    .whitespace_nowrap()
                    .child(
                        ui_text(self.locale.text(DEFERRED_BADGE, "Session not started"))
                            .normal_line_height(),
                    ),
            );
        if let Some(agent) = agent {
            meta = meta.child(
                div()
                    .flex_none()
                    .text_color(rgb(colors.agent))
                    .font_weight(FontWeight::SEMIBOLD)
                    .whitespace_nowrap()
                    .child(ui_text(agent).normal_line_height()),
            );
        }
        // `__time { margin-left: auto }`: GPUI has no auto margins, so an empty
        // flex spacer pushes the time to the end of the wrapped row.
        meta = meta.child(div().flex_1());
        if let Some(updated_label) = updated_label.filter(|label| !label.trim().is_empty()) {
            meta = meta.child(
                div()
                    .flex_none()
                    .text_color(rgb(colors.time_cwd))
                    .whitespace_nowrap()
                    .child(ui_text(updated_label).normal_line_height()),
            );
        }

        let title_tooltip = SharedString::from(title.clone());
        let title = div()
            .id(SharedString::from(format!("deferred-title-{id}")))
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .text_color(rgb(colors.title))
            .text_size(px(14.))
            .font_weight(FontWeight(650.))
            .line_height(relative(1.4))
            .line_clamp(2)
            .child(ui_text(title))
            .tooltip(move |_, cx| cx.new(|_| super::CardTooltip(title_tooltip.clone())).into());

        let mut content = div()
            .flex_none()
            .w(px(card_width))
            .flex()
            .flex_col()
            .gap(px(8.))
            .px(px(20.))
            .py(px(18.))
            .border_1()
            .border_color(rgba(colors.card_border))
            .rounded(px(12.))
            .bg(rgba(colors.card_background))
            .shadow(vec![super::shadow(
                colors.card_shadow,
                colors.card_shadow_alpha,
                16.,
                44.,
            )])
            .child(meta)
            .child(title);

        if let Some(subtitle) = subtitle.filter(|subtitle| !subtitle.trim().is_empty()) {
            let subtitle_tooltip = SharedString::from(subtitle.clone());
            content = content.child(
                div()
                    .id(SharedString::from(format!("deferred-subtitle-{id}")))
                    .truncate()
                    .text_color(rgb(colors.subtitle))
                    .text_size(px(11.))
                    .line_height(relative(1.2))
                    .child(ui_text(subtitle).normal_line_height())
                    .tooltip(move |_, cx| {
                        cx.new(|_| super::CardTooltip(subtitle_tooltip.clone()))
                            .into()
                    }),
            );
        }

        if let Some(cwd) = cwd.filter(|cwd| !cwd.trim().is_empty()) {
            let cwd_tooltip = SharedString::from(cwd.clone());
            content = content.child(
                div()
                    .id(SharedString::from(format!("deferred-cwd-{id}")))
                    .truncate()
                    .text_color(rgb(colors.time_cwd))
                    .text_size(px(10.))
                    .line_height(relative(1.2))
                    .child(ui_text(cwd).normal_line_height())
                    .tooltip(move |_, cx| {
                        cx.new(|_| super::CardTooltip(cwd_tooltip.clone())).into()
                    }),
            );
        }

        if !replies.is_empty() {
            // `.session-deferred-card__replies` / `__replies-label` / `__list`
            // / `__item` / `__role` / `p.__text`.
            let mut list = div().flex().flex_col().gap(px(7.));
            for reply in replies {
                let super::DeferredReply { kind, label, text } = reply;
                let (role_color, role_background) = match kind.as_str() {
                    "user" => (colors.role_user, colors.role_user_background),
                    "tool" | "system" => (colors.role_muted, colors.role_muted_background),
                    _ => (colors.role, colors.role_background),
                };
                list = list.child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_none()
                                .px(px(6.))
                                .py(px(1.))
                                .rounded(px(5.))
                                .bg(rgba(role_background))
                                .text_color(rgb(role_color))
                                .text_size(px(10.))
                                .line_height(relative(1.2))
                                .font_weight(FontWeight::SEMIBOLD)
                                .whitespace_nowrap()
                                .child(ui_text(label).normal_line_height()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_color(rgb(colors.reply_text))
                                .text_size(px(11.))
                                .line_height(relative(1.45))
                                .line_clamp(2)
                                .child(ui_text(text)),
                        ),
                );
            }
            content = content.child(
                div()
                    .id(SharedString::from(format!("deferred-replies-{id}")))
                    .flex_none()
                    .mt(px(2.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .max_h(px(220.))
                    .overflow_y_scroll()
                    .px(px(12.))
                    .py(px(10.))
                    .border_1()
                    .border_color(rgba(colors.replies_border))
                    .rounded(px(8.))
                    .bg(rgba(colors.replies_background))
                    .child(
                        div()
                            .text_color(rgb(colors.replies_label))
                            .text_size(px(10.))
                            .line_height(relative(1.2))
                            .font_weight(FontWeight::SEMIBOLD)
                            .whitespace_nowrap()
                            .child(
                                ui_text(self.locale.text(DEFERRED_REPLIES, "Recent messages"))
                                    .normal_line_height(),
                            ),
                    )
                    .child(list),
            );
        } else if loading {
            // Frozen: the loading hint only replaces an empty reply list.
            content = content.child(
                div()
                    .text_color(rgb(colors.note))
                    .text_size(px(11.))
                    .line_height(relative(1.5))
                    .child(
                        ui_text(
                            self.locale
                                .text(DEFERRED_LOADING, "Loading recent messages…"),
                        )
                        .normal_line_height(),
                    ),
            );
        }

        // `.session-deferred-card__note` keeps the generic overlay `p` style.
        content = content.child(
            div()
                .mt(px(2.))
                .text_color(rgb(colors.note))
                .text_size(px(11.))
                .line_height(relative(1.5))
                .child(ui_text(self.locale.text(
                    DEFERRED_BODY,
                    "Only the 16 most recently used sessions restore automatically at startup. This session’s history is saved.",
                ))),
        );

        // `__actions` + `button.__secondary` / `button.__primary`; all input
        // goes through `Workspace::button`, which keeps the buttons tab stops,
        // draws the shared keyboard focus ring, and handles Enter/Space.
        let history = can_open_history.then(|| {
            let button = div()
                .id(SharedString::from(format!("deferred-history-{id}")))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .px(px(12.))
                .py(px(6.))
                .border_1()
                .border_color(rgba(colors.secondary_border))
                .rounded(px(7.))
                .text_color(rgb(colors.secondary_text))
                .bg(rgba(colors.secondary_background))
                .text_size(px(11.))
                .line_height(relative(1.2))
                .whitespace_nowrap()
                .cursor_pointer()
                .child(
                    ui_text(self.locale.text(DEFERRED_HISTORY, "Session history"))
                        .normal_line_height(),
                );
            let history_id = id.to_owned();
            self.button(
                button,
                history_focus,
                move |this, window, cx| this.open_deferred_history(&history_id, window, cx),
                cx,
            )
        });

        let restore = div()
            .id(SharedString::from(format!("deferred-restore-{id}")))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .px(px(12.))
            .py(px(6.))
            .border_1()
            .border_color(rgb(0x516b8d))
            .rounded(px(7.))
            .text_color(rgb(0xe0ebfa))
            .bg(rgb(0x2a3b51))
            .text_size(px(12.))
            .font_weight(FontWeight::SEMIBOLD)
            .line_height(relative(1.2))
            .whitespace_nowrap()
            .child(
                ui_text(self.locale.text(DEFERRED_RESTORE, "Restore session")).normal_line_height(),
            );
        let restore = if can_restore {
            let restore_id = id.to_owned();
            self.button(
                restore.cursor_pointer(),
                restore_focus,
                move |this, _, cx| this.restore_deferred_session(&restore_id, cx),
                cx,
            )
        } else {
            // The frozen `disabled` attribute has no deferred-card rule, so use
            // the native card convention: dim and block the pointer.
            restore
                .opacity(0.55)
                .cursor(CursorStyle::OperationNotAllowed)
        };

        content = content.child(
            div()
                .flex()
                .flex_wrap()
                .justify_end()
                .gap(px(8.))
                .mt(px(4.))
                .children(history)
                .child(restore),
        );

        // `state-deferred` starts at the 32px pane header, which is exactly the
        // top of the native terminal body: the overlay fills that body.
        div()
            .id(SharedString::from(format!("deferred-recovery-{id}")))
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .p(px(16.))
            .overflow_y_scroll()
            .bg(rgba(colors.overlay_background))
            .text_color(rgb(colors.overlay_text))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
            .child(content)
    }
}
