use super::*;

pub(super) struct ContextDetailsPopover {
    pub(super) session: String,
    pub(super) request: u64,
    pub(super) result: Option<Value>,
    pub(super) failed: bool,
    pub(super) expanded: Option<ContextLogPreview>,
}

pub(super) struct ContextLogPreview {
    pub(super) id: String,
    pub(super) request: u64,
    pub(super) text: Option<String>,
}

impl Workspace {
    pub(super) fn close_context_details(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let closed = self
            .context_details
            .remove(&window.window_handle())
            .is_some();
        if closed {
            cx.notify();
        }
        closed
    }

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

    pub(super) fn context_details_trigger(
        &self,
        id: &'static str,
        session: &str,
        label: String,
        detail: Option<&'static str>,
        fg: u32,
        bg: u32,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let session = session.to_owned();
        self.context_hud_pill(label, fg, bg)
            .id(id)
            .cursor_pointer()
            .when_some(detail, |pill, detail| {
                pill.tooltip(move |_, cx| cx.new(|_| CardTooltip(detail.into())).into())
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.toggle_context_details(&session, window, cx);
                cx.stop_propagation();
            }))
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
        let reason = node["contextCardAlert"].as_str()?;
        if self.read_only.is_some() {
            return None;
        }
        let stalled = reason == "stalled";
        let (fg, bg, label) = if stalled {
            (
                0xb34d47,
                0xb34d4722,
                self.locale.text("会话疑似卡死", "Session appears stalled"),
            )
        } else {
            (
                0xb45309,
                0xd9770622,
                self.locale.text("上下文占用 ≥85%", "Context usage ≥85%"),
            )
        };
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
                .bg(rgba(bg))
                .text_color(rgb(fg))
                .text_size(px(11.))
                .child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(label),
                )
                .child(self.context_handoff_actions(session, "card", stalled, cx)),
        )
    }

    pub(super) fn render_context_details(
        &self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let window_id = window.window_handle();
        let state = self.context_details.get(&window_id)?;
        let session = self.footer_session(window)?;
        if state.session != session {
            return None;
        }
        let viewport = window.viewport_size();
        let palette = self.theme_palette().workspace;
        let locale = self.locale;
        let result = state.result.as_ref().unwrap_or(&Value::Null);
        let live = self
            .live_agent_hud(session)
            .map(|hud| &hud["contextGuard"])
            .unwrap_or(result);
        let cache = &live["cache"];
        let stall = &live["stall"];
        let hit = cache["hitRatioPercent"]
            .as_f64()
            .map(|hit| format!("{hit:.0}%"))
            .unwrap_or_else(|| locale.text("暂无数据", "Unavailable").to_owned());
        let cache_status = if !cache.is_object() {
            locale.text("未开启", "Off").to_owned()
        } else {
            let remaining = cache["remainingSec"].as_u64().unwrap_or(0);
            let countdown =
                matou_native_desktop::context_optimizer_settings::format_cache_countdown(
                    remaining,
                    cache["warm"] == true,
                    locale,
                );
            match cache["timing"].as_str() {
                Some("precise") => countdown,
                Some("estimate") => format!("{countdown} · {}", locale.text("估算", "est.")),
                _ => locale.text("到期时间未知", "Expiry unknown").to_owned(),
            }
        };
        let stage = match stall["severity"].as_str() {
            Some("stalled") => locale.text("疑似卡死", "Stalled"),
            Some("warning") => locale.text("静默观察中", "Watching silence"),
            Some(_) => locale.text("正常", "Normal"),
            None => locale.text("未开启", "Off"),
        };
        let silent = stall["silentSec"]
            .as_u64()
            .map_or_else(String::new, |seconds| {
                format!(" · {seconds} {}", locale.text("秒", "s"))
            });
        let mut body = div()
            .id("footer-context-details-scroll")
            .flex()
            .flex_col()
            .gap(px(8.))
            .overflow_y_scroll()
            .p(px(12.))
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(locale.text("上下文详情", "Context details")),
            )
            .child(format!(
                "{} {hit} · {cache_status}",
                locale.text("缓存命中率", "Cache hit rate")
            ))
            .child(format!(
                "{} {stage}{silent}",
                locale.text("看门狗", "Watchdog")
            ));
        body = body.child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(locale.text("本会话截断日志", "Truncated logs")),
        );
        match result["logs"].as_array() {
            Some(logs) if !logs.is_empty() => {
                for log in logs {
                    let Some(id) = log["id"].as_str() else {
                        continue;
                    };
                    let open = state.expanded.as_ref().filter(|preview| preview.id == id);
                    let copy_id = id.to_owned();
                    let expand_id = id.to_owned();
                    let controls = div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(6.))
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(id.to_owned()))
                        .child(
                            div()
                                .flex()
                                .gap(px(4.))
                                .child(
                                    self.context_hud_pill(
                                        locale.text("复制 ID", "Copy ID").to_owned(),
                                        0x2679d8,
                                        0x2679d81f,
                                    )
                                    .id(SharedString::from(format!("context-copy-{id}")))
                                    .cursor_pointer()
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            cx.write_to_clipboard(ClipboardItem::new_string(
                                                copy_id.clone(),
                                            ));
                                            this.show_toast(
                                                this.locale.text("已复制日志 ID", "Log ID copied"),
                                                cx,
                                            );
                                        },
                                    )),
                                )
                                .child(
                                    self.context_hud_pill(
                                        if open.is_some() {
                                            locale.text("收起原文", "Collapse")
                                        } else {
                                            locale.text("查看原文", "View log")
                                        }
                                        .to_owned(),
                                        0x2679d8,
                                        0x2679d81f,
                                    )
                                    .id(SharedString::from(format!("context-expand-{id}")))
                                    .cursor_pointer()
                                    .on_click(cx.listener(
                                        move |this, _, window, cx| {
                                            this.toggle_context_log(&expand_id, window, cx);
                                        },
                                    )),
                                ),
                        );
                    let command = log["command"].as_str().unwrap_or_default();
                    let preview = match open {
                        Some(preview) => preview
                            .text
                            .as_deref()
                            .unwrap_or(locale.text("加载中…", "Loading…")),
                        None => log["preview"].as_str().unwrap_or_default(),
                    };
                    body = body.child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_none()
                            .gap(px(4.))
                            .p(px(7.))
                            .border_1()
                            .border_color(rgba(palette.line))
                            .rounded(px(5.))
                            .child(controls)
                            .child(format!(
                                "{command} · {} B",
                                log["rawBytes"].as_u64().unwrap_or(0)
                            ))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .whitespace_normal()
                                    .overflow_hidden()
                                    .child(preview.to_owned()),
                            ),
                    );
                }
            }
            Some(_) => body = body.child(locale.text("暂无截断日志", "No truncated logs")),
            None if state.failed => {
                body = body.child(locale.text("无法读取日志", "Could not read logs"))
            }
            None => body = body.child(locale.text("加载中…", "Loading…")),
        }
        let panel = div()
            .id("footer-context-details")
            .flex()
            .flex_col()
            .occlude()
            .w(px(390.).min(viewport.width - px(16.)))
            .max_h(px(435.).min(viewport.height - px(64.)))
            .overflow_hidden()
            .border_1()
            .border_color(rgba(palette.line))
            .rounded(px(8.))
            .bg(rgba(palette.panel))
            .text_color(rgba(palette.text))
            .text_size(px(12.))
            .shadow(vec![shadow(0x000000, 0.24, 8., 24.)])
            .child(body);
        Some(
            div()
                .child(
                    anchored()
                        .anchor(Corner::TopLeft)
                        .position(point(px(0.), px(0.)))
                        .child(
                            div()
                                .id("footer-context-details-backdrop")
                                .w(viewport.width)
                                .h(viewport.height)
                                .occlude()
                                .on_any_mouse_down(cx.listener(|this, _, window, cx| {
                                    this.close_context_details(window, cx);
                                    cx.stop_propagation();
                                })),
                        ),
                )
                .child(
                    deferred(
                        anchored()
                            .anchor(Corner::BottomRight)
                            .position(point(viewport.width - px(14.), viewport.height - px(37.)))
                            .snap_to_window()
                            .child(panel),
                    )
                    .with_priority(2),
                )
                .into_any_element(),
        )
    }
}
