use std::collections::BTreeMap;

use matou_native_core::sessions::InterfaceLocale;

/// The card stays in this mount while the detached window presents its session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetachedMount {
    pub main_window_id: String,
    pub scene_id: String,
    pub mount_id: String,
    pub session_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DetachedSession {
    pub mount: DetachedMount,
    pub window_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DetachOutcome {
    Open(DetachedSession),
    FocusExisting(DetachedSession),
    WindowIdInUse,
}

/// Tracks the presentation of a session for the lifetime of the desktop process.
#[derive(Default)]
pub struct DetachedWindows {
    by_window: BTreeMap<String, DetachedSession>,
}

impl DetachedWindows {
    /// Reserve the presentation before opening a child. Use a fresh window ID
    /// for each attempt so a late close cannot affect a subsequent detach.
    /// If opening fails, pass its ID to `window_closed` to restore the mount.
    pub fn detach(&mut self, mount: DetachedMount, window_id: String) -> DetachOutcome {
        if let Some(existing) = self.for_session(&mount.session_id) {
            return DetachOutcome::FocusExisting(existing.clone());
        }
        if self.by_window.contains_key(&window_id) {
            return DetachOutcome::WindowIdInUse;
        }
        let detached = DetachedSession { mount, window_id };
        self.by_window
            .insert(detached.window_id.clone(), detached.clone());
        DetachOutcome::Open(detached)
    }

    pub fn for_session(&self, session_id: &str) -> Option<&DetachedSession> {
        self.by_window
            .values()
            .find(|detached| detached.mount.session_id == session_id)
    }

    /// Route focus through the owning main window to the requested session.
    pub fn focus_target(&self, main_window_id: &str, session_id: &str) -> Option<&str> {
        let detached = self.for_session(session_id)?;
        (detached.mount.main_window_id == main_window_id).then_some(detached.window_id.as_str())
    }

    /// The terminal and the recovery-water overlay share this presentation owner.
    pub fn presentation_window<'a>(
        &'a self,
        session_id: &str,
        attached_window_id: &'a str,
    ) -> &'a str {
        self.for_session(session_id)
            .map_or(attached_window_id, |detached| detached.window_id.as_str())
    }

    /// Restore the captured mount and close the child identified by the result.
    pub fn return_to_main(&mut self, window_id: &str) -> Option<DetachedSession> {
        self.by_window.remove(window_id)
    }

    /// Restore the captured mount when the OS closes the child. A duplicate
    /// close following an explicit return or discard is a no-op.
    pub fn window_closed(&mut self, window_id: &str) -> Option<DetachedSession> {
        self.return_to_main(window_id)
    }

    /// Call on archive or removal. The result identifies the child to close.
    /// The caller leaves the card archived or removed.
    pub fn discard_session(&mut self, session_id: &str) -> Option<DetachedSession> {
        let window_id = self.for_session(session_id)?.window_id.clone();
        self.by_window.remove(&window_id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetachedWindowAction {
    Focus,
    ReturnToMain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DetachedPlaceholderCopy {
    pub badge: &'static str,
    pub body: &'static str,
    pub return_to_tab: &'static str,
}

pub fn detached_placeholder_copy(locale: InterfaceLocale) -> DetachedPlaceholderCopy {
    DetachedPlaceholderCopy {
        badge: locale.text("已脱出", "Detached"),
        body: locale.text(
            "终端正在独立窗口中运行，进程与会话保持不变。",
            "The terminal is running in a separate window. Its process and session are unchanged.",
        ),
        return_to_tab: locale.text("归还到当前页签", "Return to this tab"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetachedHeaderStatus {
    Running,
    ReadOnly,
    TeamMember,
}

pub fn detached_header_status(
    locale: InterfaceLocale,
    status: DetachedHeaderStatus,
) -> &'static str {
    match status {
        DetachedHeaderStatus::Running => locale.text(
            "独立窗口 · 会话保持运行",
            "Detached window · Session keeps running",
        ),
        DetachedHeaderStatus::ReadOnly => {
            locale.text("独立窗口 · 只读历史", "Detached window · Read-only history")
        }
        DetachedHeaderStatus::TeamMember => {
            locale.text("独立窗口 · 队友摘要", "Detached window · Teammate summary")
        }
    }
}

mod view {
    use std::rc::Rc;

    use gpui::{prelude::*, *};

    use super::*;
    use crate::ui_text::ui_text;

    type DetachedActionHandler = Rc<dyn Fn(DetachedWindowAction, &mut Window, &mut App)>;

    /// `.detached-placeholder`: centered title/badge, one body paragraph and the
    /// return button. The body focuses the child window; the return button
    /// stops the click so ReturnToMain cannot also emit Focus.
    pub fn detached_placeholder(
        title: impl Into<SharedString>,
        locale: InterfaceLocale,
        on_action: impl Fn(DetachedWindowAction, &mut Window, &mut App) + 'static,
    ) -> gpui::AnyElement {
        let title: SharedString = title.into();
        let copy = detached_placeholder_copy(locale);
        let handler: DetachedActionHandler = Rc::new(on_action);
        let focus_handler = handler.clone();
        let return_handler = handler;

        div()
            .id("detached-placeholder")
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .bg(rgb(0x10141b))
            .text_color(rgb(0x8792a6))
            .text_size(px(16.))
            .text_center()
            .cursor_pointer()
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                focus_handler(DetachedWindowAction::Focus, window, cx);
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .child(div().font_weight(FontWeight::BOLD).child(ui_text(title)))
                    .child(
                        div()
                            .px(px(6.))
                            .py(px(3.))
                            .rounded(px(999.))
                            .bg(rgb(0x202b42))
                            .text_color(rgb(0xaac0ed))
                            .text_size(px(10.))
                            .child(ui_text(copy.badge)),
                    ),
            )
            .child(
                div()
                    .mt(px(10.))
                    .mb(px(14.))
                    .text_size(px(11.))
                    .child(ui_text(copy.body)),
            )
            .child(
                div()
                    .id("detached-placeholder-return")
                    .tab_index(0)
                    .px(px(10.))
                    .py(px(6.))
                    .border_1()
                    .border_color(rgb(0x35415a))
                    .rounded(px(6.))
                    .bg(rgb(0x1a2333))
                    .text_color(rgb(0xcbd8f1))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        return_handler(DetachedWindowAction::ReturnToMain, window, cx);
                    })
                    .child(ui_text(copy.return_to_tab)),
            )
            .into_any_element()
    }

    /// `.detached-terminal-app > header`: the frozen 42px title bar with a
    /// dark/light palette. The title/status drag region and the return control
    /// are separate elements so dragging never starts on the button.
    pub fn detached_window_header(
        title: impl Into<SharedString>,
        locale: InterfaceLocale,
        status: DetachedHeaderStatus,
        light: bool,
        on_action: impl Fn(DetachedWindowAction, &mut Window, &mut App) + 'static,
    ) -> gpui::AnyElement {
        let title: SharedString = title.into();
        let copy = detached_placeholder_copy(locale);
        let status = detached_header_status(locale, status);
        let border = if light { rgb(0xe8e8e8) } else { rgb(0x252a33) };
        let background = if light { rgb(0xffffff) } else { rgb(0x11151c) };
        let foreground = if light { rgb(0x2f3547) } else { rgb(0xdce3ef) };
        let status_color = if light { rgb(0x606572) } else { rgb(0x748096) };
        let button_border = if light { rgb(0xe8e8e8) } else { rgb(0x35415a) };
        let button_background = if light { rgb(0xf7f8fa) } else { rgb(0x1a2333) };
        let button_text = if light { rgb(0x2f3547) } else { rgb(0xcbd8f1) };
        let button_hover = if light { rgb(0xf0f2f5) } else { rgb(0x202b42) };

        div()
            .id("detached-window-header")
            .flex()
            .flex_none()
            .items_center()
            .w_full()
            .h(px(42.))
            .px(px(12.))
            .gap(px(8.))
            .border_b_1()
            .border_color(border)
            .bg(background)
            .text_color(foreground)
            .child(
                div()
                    .id("detached-window-drag")
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .items_center()
                    .gap(px(8.))
                    .window_control_area(WindowControlArea::Drag)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .text_size(px(16.))
                            .font_weight(FontWeight::BOLD)
                            .child(ui_text(title)),
                    )
                    .child(
                        div()
                            .flex_none()
                            .whitespace_nowrap()
                            .text_size(px(10.))
                            .text_color(status_color)
                            .child(ui_text(status)),
                    ),
            )
            .child(
                div()
                    .id("detached-window-return")
                    .tab_index(0)
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(px(24.))
                    .px(px(10.))
                    .border_1()
                    .border_color(button_border)
                    .rounded(px(6.))
                    .bg(button_background)
                    .text_color(button_text)
                    .text_size(px(11.))
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .hover(move |style| style.bg(button_hover))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        on_action(DetachedWindowAction::ReturnToMain, window, cx);
                    })
                    .child(ui_text(copy.return_to_tab)),
            )
            .into_any_element()
    }
}

pub use view::{detached_placeholder, detached_window_header};

#[cfg(test)]
mod tests {
    use super::*;

    fn mount(session_id: &str) -> DetachedMount {
        DetachedMount {
            main_window_id: "main".into(),
            scene_id: "scene".into(),
            mount_id: format!("mount-{session_id}"),
            session_id: session_id.into(),
        }
    }

    #[test]
    fn detaches_multiple_cards_and_resolves_their_presentation_and_focus() {
        let mut windows = DetachedWindows::default();
        let first = DetachedSession {
            mount: mount("a"),
            window_id: "a-child".into(),
        };
        assert_eq!(
            windows.detach(first.mount.clone(), first.window_id.clone()),
            DetachOutcome::Open(first.clone()),
        );
        assert!(matches!(
            windows.detach(mount("b"), "b-child".into()),
            DetachOutcome::Open(_)
        ));
        assert_eq!(windows.for_session("a"), Some(&first));
        assert_eq!(windows.focus_target("main", "a"), Some("a-child"));
        assert_eq!(windows.focus_target("other-main", "a"), None);
        assert_eq!(windows.presentation_window("a", "main"), "a-child");
        assert_eq!(windows.presentation_window("b", "main"), "b-child");
        assert_eq!(windows.presentation_window("c", "main"), "main");
    }

    #[test]
    fn repeated_detach_focuses_the_existing_window_without_replacing_its_origin() {
        let mut windows = DetachedWindows::default();
        let DetachOutcome::Open(original) = windows.detach(mount("a"), "a-child".into()) else {
            panic!("expected a new child");
        };
        let mut duplicate = mount("a");
        duplicate.main_window_id = "other-main".into();
        duplicate.mount_id = "other-mount".into();
        assert_eq!(
            windows.detach(duplicate, "unused-child".into()),
            DetachOutcome::FocusExisting(original.clone()),
        );
        assert_eq!(windows.window_closed("unused-child"), None);
        assert_eq!(
            windows.detach(mount("b"), "a-child".into()),
            DetachOutcome::WindowIdInUse
        );
        assert_eq!(windows.for_session("a"), Some(&original));
    }

    #[test]
    fn close_and_explicit_return_restore_the_original_mount_once() {
        let mut windows = DetachedWindows::default();
        windows.detach(mount("a"), "a-child".into());
        windows.detach(mount("b"), "b-child".into());

        assert_eq!(windows.window_closed("a-child").unwrap().mount, mount("a"));
        assert_eq!(windows.return_to_main("b-child").unwrap().mount, mount("b"));
        windows.detach(mount("b"), "b-new-child".into());
        assert_eq!(windows.window_closed("b-child"), None);
        assert_eq!(windows.return_to_main("a-child"), None);
        assert_eq!(windows.presentation_window("a", "main"), "main");
        assert_eq!(windows.presentation_window("b", "main"), "b-new-child");
    }

    #[test]
    fn failed_window_creation_releases_the_reservation_for_a_retry() {
        let mut windows = DetachedWindows::default();
        windows.detach(mount("a"), "failed-child".into());
        assert_eq!(
            windows.window_closed("failed-child").unwrap().mount,
            mount("a"),
        );
        assert!(matches!(
            windows.detach(mount("a"), "retry-child".into()),
            DetachOutcome::Open(_)
        ));
    }

    #[test]
    fn archive_and_removal_discard_their_child_without_a_late_restore() {
        let mut windows = DetachedWindows::default();
        for session in ["archived", "removed", "retained"] {
            windows.detach(mount(session), format!("{session}-child"));
        }
        for session in ["archived", "removed"] {
            let detached = windows.discard_session(session).unwrap();
            assert_eq!(detached.mount.session_id, session);
            assert_eq!(windows.for_session(session), None);
            assert_eq!(windows.window_closed(&detached.window_id), None);
        }
        assert_eq!(
            windows.focus_target("main", "retained"),
            Some("retained-child")
        );
    }

    #[test]
    fn placeholder_and_header_use_the_frozen_copy_in_both_locales() {
        let zh = detached_placeholder_copy(InterfaceLocale::Chinese);
        assert_eq!(zh.badge, "已脱出");
        assert_eq!(zh.body, "终端正在独立窗口中运行，进程与会话保持不变。");
        assert_eq!(zh.return_to_tab, "归还到当前页签");

        let en = detached_placeholder_copy(InterfaceLocale::English);
        assert_eq!(en.badge, "Detached");
        assert_eq!(
            en.body,
            "The terminal is running in a separate window. Its process and session are unchanged."
        );
        assert_eq!(en.return_to_tab, "Return to this tab");
        assert_eq!(
            detached_header_status(InterfaceLocale::Chinese, DetachedHeaderStatus::Running),
            "独立窗口 · 会话保持运行",
        );
        assert_eq!(
            detached_header_status(InterfaceLocale::English, DetachedHeaderStatus::ReadOnly),
            "Detached window · Read-only history",
        );
        assert_eq!(
            detached_header_status(InterfaceLocale::English, DetachedHeaderStatus::TeamMember),
            "Detached window · Teammate summary",
        );
    }
}
