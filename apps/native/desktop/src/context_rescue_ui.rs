use super::*;

impl Workspace {
    pub(super) fn context_hud_pill(&self, label: String, fg: u32, bg: u32) -> Div {
        div()
            .flex_none()
            .px(px(6.))
            .py(px(1.5))
            .rounded(px(5.))
            .bg(rgba(bg))
            .text_size(px(10.))
            .font_weight(FontWeight(600.))
            .text_color(rgb(fg))
            .whitespace_nowrap()
            .child(label)
    }

    pub(super) fn context_handoff_actions(
        &self,
        session: &str,
        placement: &'static str,
        urgent: bool,
        cx: &mut Context<Self>,
    ) -> Div {
        let locale = self.locale;
        let footer = placement == "footer";
        let (copy_label, copy_fg, copy_bg) = if footer {
            let dark = self.theme.effective() == AppTheme::Dark;
            (
                locale.text("交接摘要", "Handoff brief"),
                if dark { 0xff9a73 } else { 0x9a3412 },
                0xff6b351f,
            )
        } else {
            (locale.text("复制摘要", "Copy brief"), 0x2679d8, 0x2679d81f)
        };
        let copy_session = session.to_owned();
        div()
            .flex()
            .flex_none()
            .gap(px(4.))
            .children((!footer).then(|| {
                let busy = self.context_rescue_requests.contains_key(session);
                let rescue_session = session.to_owned();
                let (fg, bg) = if urgent {
                    (0xffffff, 0xb34d47ff)
                } else {
                    (0x2679d8, 0x2679d81f)
                };
                self.context_hud_pill(
                    if busy {
                        locale.text("正在交接…", "Starting…")
                    } else {
                        locale.text("带摘要开新会话", "Start with handoff")
                    }
                    .to_owned(),
                    fg,
                    bg,
                )
                .id(SharedString::from(format!("{placement}-rescue-{session}")))
                .when(!busy, |button| button.cursor_pointer())
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.request_context_rescue(&rescue_session, cx);
                    cx.stop_propagation();
                }))
            }))
            .child(
                self.context_hud_pill(copy_label.to_owned(), copy_fg, copy_bg)
                    .id(SharedString::from(format!(
                        "{placement}-copy-handoff-{session}"
                    )))
                    .when(footer, |pill| pill.children(footer_bounds_probe("handoff")))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.request_session_handoff(copy_session.clone(), cx);
                        cx.stop_propagation();
                    })),
            )
    }

    pub(super) fn render_context_card_alert(
        &self,
        session: &str,
        node: &Value,
        cx: &mut Context<Self>,
    ) -> Option<Div> {
        if node["contextCardAlert"].as_str() != Some("critical") || self.read_only.is_some() {
            return None;
        }
        Some(
            div()
                .flex()
                .flex_none()
                .w_full()
                .items_center()
                .justify_between()
                .gap(px(6.))
                .px(px(10.))
                .py(px(4.))
                .bg(rgba(0xd9770622))
                .text_color(rgb(0xb45309))
                .text_size(px(11.))
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(self.locale.text("上下文占用 ≥85%", "Context usage ≥85%")),
                )
                .child(self.context_handoff_actions(session, "card", true, cx)),
        )
    }
}
