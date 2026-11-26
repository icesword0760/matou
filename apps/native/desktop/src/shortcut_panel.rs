//! Frozen shortcut artwork, its readable contents, and panel lifecycle.

const DARK_MAC: &[u8] = include_bytes!("../assets/shortcuts/dark_mac.png");
const DARK_WIN: &[u8] = include_bytes!("../assets/shortcuts/dark_win.png");
const LIGHT_MAC: &[u8] = include_bytes!("../assets/shortcuts/white_mac.png");
const LIGHT_WIN: &[u8] = include_bytes!("../assets/shortcuts/white_win.png");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutPlatform {
    Mac,
    Windows,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutTheme {
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutLocale {
    ZhCn,
    En,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShortcutPanelModel {
    pub platform: ShortcutPlatform,
    pub theme: ShortcutTheme,
    pub locale: ShortcutLocale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShortcutPanelMessages {
    pub title: &'static str,
    pub close: &'static str,
    pub image_alt: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShortcutGroup {
    pub title: &'static str,
    pub entries: &'static [ShortcutEntry],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShortcutEntry {
    pub label: &'static str,
    mac_keys: &'static [&'static str],
    win_keys: &'static [&'static str],
}

impl ShortcutEntry {
    const fn new(
        label: &'static str,
        mac_keys: &'static [&'static str],
        win_keys: &'static [&'static str],
    ) -> Self {
        Self {
            label,
            mac_keys,
            win_keys,
        }
    }

    pub fn keys(self, model: ShortcutPanelModel) -> &'static [&'static str] {
        match model.platform {
            ShortcutPlatform::Mac => self.mac_keys,
            ShortcutPlatform::Windows => self.win_keys,
        }
    }
}

const SPLITS: &[ShortcutEntry] = &[
    ShortcutEntry::new("竖向分屏（垂直分割）", &["⌘", "D"], &["Ctrl", "D"]),
    ShortcutEntry::new(
        "横向分屏（水平分割）",
        &["⌘", "Shift", "D"],
        &["Ctrl", "Shift", "D"],
    ),
    ShortcutEntry::new("切换到下一个 pane", &["⌘", "]"], &["Ctrl", "]"]),
    ShortcutEntry::new("切换到上一个pane", &["⌘", "["], &["Ctrl", "["]),
    ShortcutEntry::new(
        "切换到指定方向的 pane",
        &["⌘", "Option", "↑/↓/←/→"],
        &["Ctrl", "Alt", "↑/↓/←/→"],
    ),
    ShortcutEntry::new("关闭当前 pane", &["⌘", "W"], &["Ctrl", "W"]),
];

const TABS: &[ShortcutEntry] = &[
    ShortcutEntry::new("新建标签页", &["⌘", "T"], &["Ctrl", "T"]),
    ShortcutEntry::new("关闭标签页", &["⌘", "W"], &["Ctrl", "W"]),
    ShortcutEntry::new("切换到下一个标签页", &["⌘", "Shift", "]"], &["Ctrl", "Tab"]),
    ShortcutEntry::new(
        "切换到上一个标签页",
        &["⌘", "Shift", "["],
        &["Ctrl", "Shift", "Tab"],
    ),
    ShortcutEntry::new("跳转到指定标签页", &["⌘", "1~9"], &["Ctrl", "1~9"]),
    ShortcutEntry::new(
        "移动标签页位置",
        &["⌘", "Shift", "←/→"],
        &["Ctrl", "Shift", "PgUp/PgDn"],
    ),
];

const FONT_SIZE: &[ShortcutEntry] = &[
    ShortcutEntry::new("放大字号", &["⌘", "+"], &["Ctrl", "+"]),
    ShortcutEntry::new("缩小字号", &["⌘", "−"], &["Ctrl", "−"]),
    ShortcutEntry::new("重置字号", &["⌘", "0"], &["Ctrl", "0"]),
    ShortcutEntry::new("滚轮缩放", &["⌘", "滚轮"], &["Ctrl", "滚轮"]),
];

const OTHER: &[ShortcutEntry] = &[
    ShortcutEntry::new("换肤", &["⌘", "I"], &["Ctrl", "I"]),
    ShortcutEntry::new("搜索", &["⌘", "F"], &["Ctrl", "F"]),
];

impl ShortcutPanelModel {
    pub fn messages(self) -> ShortcutPanelMessages {
        match self.locale {
            ShortcutLocale::ZhCn => ShortcutPanelMessages {
                title: "快捷键列表",
                close: "关闭快捷键列表",
                image_alt: "快捷键说明",
            },
            ShortcutLocale::En => ShortcutPanelMessages {
                title: "Keyboard shortcuts",
                close: "Close keyboard shortcuts",
                image_alt: "Keyboard shortcut reference",
            },
        }
    }

    /// Both frozen locales use the same platform/theme artwork and its Chinese entries.
    pub fn groups(self) -> [ShortcutGroup; 4] {
        [
            ShortcutGroup {
                title: "分屏管理",
                entries: SPLITS,
            },
            ShortcutGroup {
                title: "标签页管理",
                entries: TABS,
            },
            ShortcutGroup {
                title: "终端字号管理",
                entries: FONT_SIZE,
            },
            ShortcutGroup {
                title: match self.platform {
                    ShortcutPlatform::Mac => "其他",
                    ShortcutPlatform::Windows => "换肤",
                },
                entries: OTHER,
            },
        ]
    }

    pub fn artwork(self) -> &'static [u8] {
        match (self.platform, self.theme) {
            (ShortcutPlatform::Mac, ShortcutTheme::Dark) => DARK_MAC,
            (ShortcutPlatform::Windows, ShortcutTheme::Dark) => DARK_WIN,
            (ShortcutPlatform::Mac, ShortcutTheme::Light) => LIGHT_MAC,
            (ShortcutPlatform::Windows, ShortcutTheme::Light) => LIGHT_WIN,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutPanelClose {
    Button,
    Backdrop,
    Escape,
    Toggle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutPanelAction {
    Open,
    Close(ShortcutPanelClose),
    Toggle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutPanelEffect {
    FocusPanel,
    RestoreTerminalFocus(ShortcutPanelClose),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ShortcutPanelState {
    open: bool,
}

impl ShortcutPanelState {
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The host applies the focus effect after changing the modal state.
    pub fn dispatch(&mut self, action: ShortcutPanelAction) -> Option<ShortcutPanelEffect> {
        let effect = match (self.open, action) {
            (false, ShortcutPanelAction::Open | ShortcutPanelAction::Toggle) => {
                ShortcutPanelEffect::FocusPanel
            }
            (true, ShortcutPanelAction::Close(reason)) => {
                ShortcutPanelEffect::RestoreTerminalFocus(reason)
            }
            (true, ShortcutPanelAction::Toggle) => {
                ShortcutPanelEffect::RestoreTerminalFocus(ShortcutPanelClose::Toggle)
            }
            _ => return None,
        };
        self.open = !self.open;
        Some(effect)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Transcribed from the frozen artwork in display order, reading rows left to right.
    const MAC_ENTRIES: &str = "分屏管理
竖向分屏（垂直分割）|⌘ D
横向分屏（水平分割）|⌘ Shift D
切换到下一个 pane|⌘ ]
切换到上一个pane|⌘ [
切换到指定方向的 pane|⌘ Option ↑/↓/←/→
关闭当前 pane|⌘ W
标签页管理
新建标签页|⌘ T
关闭标签页|⌘ W
切换到下一个标签页|⌘ Shift ]
切换到上一个标签页|⌘ Shift [
跳转到指定标签页|⌘ 1~9
移动标签页位置|⌘ Shift ←/→
终端字号管理
放大字号|⌘ +
缩小字号|⌘ −
重置字号|⌘ 0
滚轮缩放|⌘ 滚轮
其他
换肤|⌘ I
搜索|⌘ F";

    const WIN_ENTRIES: &str = "分屏管理
竖向分屏（垂直分割）|Ctrl D
横向分屏（水平分割）|Ctrl Shift D
切换到下一个 pane|Ctrl ]
切换到上一个pane|Ctrl [
切换到指定方向的 pane|Ctrl Alt ↑/↓/←/→
关闭当前 pane|Ctrl W
标签页管理
新建标签页|Ctrl T
关闭标签页|Ctrl W
切换到下一个标签页|Ctrl Tab
切换到上一个标签页|Ctrl Shift Tab
跳转到指定标签页|Ctrl 1~9
移动标签页位置|Ctrl Shift PgUp/PgDn
终端字号管理
放大字号|Ctrl +
缩小字号|Ctrl −
重置字号|Ctrl 0
滚轮缩放|Ctrl 滚轮
换肤
换肤|Ctrl I
搜索|Ctrl F";

    #[test]
    fn all_locale_platform_and_theme_combinations_match_the_frozen_panel() {
        for (locale, chrome) in [
            (
                ShortcutLocale::ZhCn,
                ["快捷键列表", "关闭快捷键列表", "快捷键说明"],
            ),
            (
                ShortcutLocale::En,
                [
                    "Keyboard shortcuts",
                    "Close keyboard shortcuts",
                    "Keyboard shortcut reference",
                ],
            ),
        ] {
            for (platform, theme, artwork_fingerprint) in [
                (
                    ShortcutPlatform::Mac,
                    ShortcutTheme::Dark,
                    0x91cb41da4a415487,
                ),
                (
                    ShortcutPlatform::Mac,
                    ShortcutTheme::Light,
                    0xf0fe923be57aaa91,
                ),
                (
                    ShortcutPlatform::Windows,
                    ShortcutTheme::Dark,
                    0x327876b01ec3352c,
                ),
                (
                    ShortcutPlatform::Windows,
                    ShortcutTheme::Light,
                    0x50d8149aa3c3e275,
                ),
            ] {
                let model = ShortcutPanelModel {
                    platform,
                    theme,
                    locale,
                };
                let messages = model.messages();
                assert_eq!([messages.title, messages.close, messages.image_alt], chrome);

                let mut lines = Vec::new();
                for group in model.groups() {
                    lines.push(group.title.to_owned());
                    lines.extend(
                        group.entries.iter().map(|entry| {
                            format!("{}|{}", entry.label, entry.keys(model).join(" "))
                        }),
                    );
                }
                let expected = match platform {
                    ShortcutPlatform::Mac => MAC_ENTRIES,
                    ShortcutPlatform::Windows => WIN_ENTRIES,
                };
                assert_eq!(lines.join("\n"), expected, "{model:?}");
                let artwork_hash = model
                    .artwork()
                    .iter()
                    .fold(0xcbf29ce484222325_u64, |hash, byte| {
                        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
                    });
                assert_eq!(artwork_hash, artwork_fingerprint, "{model:?}");
            }
        }
    }

    #[test]
    fn button_backdrop_and_escape_close_once_and_restore_terminal_focus() {
        let mut state = ShortcutPanelState::default();
        for reason in [
            ShortcutPanelClose::Button,
            ShortcutPanelClose::Backdrop,
            ShortcutPanelClose::Escape,
        ] {
            assert!(!state.is_open());
            assert_eq!(state.dispatch(ShortcutPanelAction::Close(reason)), None);
            assert_eq!(
                state.dispatch(ShortcutPanelAction::Open),
                Some(ShortcutPanelEffect::FocusPanel)
            );
            assert!(state.is_open());
            assert_eq!(state.dispatch(ShortcutPanelAction::Open), None);
            assert_eq!(
                state.dispatch(ShortcutPanelAction::Close(reason)),
                Some(ShortcutPanelEffect::RestoreTerminalFocus(reason))
            );
            assert!(!state.is_open());
            assert_eq!(state.dispatch(ShortcutPanelAction::Close(reason)), None);
        }
    }

    #[test]
    fn shortcut_toggles_panel_focus_and_then_terminal_focus() {
        let mut state = ShortcutPanelState::default();
        assert_eq!(
            state.dispatch(ShortcutPanelAction::Toggle),
            Some(ShortcutPanelEffect::FocusPanel)
        );
        assert!(state.is_open());
        assert_eq!(
            state.dispatch(ShortcutPanelAction::Toggle),
            Some(ShortcutPanelEffect::RestoreTerminalFocus(
                ShortcutPanelClose::Toggle
            ))
        );
        assert!(!state.is_open());
    }
}

mod ui {
    use std::{
        rc::Rc,
        sync::{Arc, OnceLock},
        time::Duration,
    };

    use gpui::{prelude::*, *};

    use super::*;
    use crate::ui_text::ui_text;

    type CloseHandler = dyn Fn(ShortcutPanelClose, &mut Window, &mut App);

    fn cached_artwork(model: ShortcutPanelModel) -> Arc<Image> {
        static DARK_MAC: OnceLock<Arc<Image>> = OnceLock::new();
        static DARK_WIN: OnceLock<Arc<Image>> = OnceLock::new();
        static LIGHT_MAC: OnceLock<Arc<Image>> = OnceLock::new();
        static LIGHT_WIN: OnceLock<Arc<Image>> = OnceLock::new();
        let cache = match (model.platform, model.theme) {
            (ShortcutPlatform::Mac, ShortcutTheme::Dark) => &DARK_MAC,
            (ShortcutPlatform::Windows, ShortcutTheme::Dark) => &DARK_WIN,
            (ShortcutPlatform::Mac, ShortcutTheme::Light) => &LIGHT_MAC,
            (ShortcutPlatform::Windows, ShortcutTheme::Light) => &LIGHT_WIN,
        };
        cache
            .get_or_init(|| {
                Arc::new(Image::from_bytes(
                    ImageFormat::Png,
                    model.artwork().to_vec(),
                ))
            })
            .clone()
    }

    fn opening_animation() -> Animation {
        Animation::new(Duration::from_millis(200)).with_easing(|progress| {
            // CSS ease, cubic-bezier(.25,.1,.25,1). Invert the x axis first.
            let (mut lo, mut hi) = (0.0, 1.0);
            for _ in 0..12 {
                let t = (lo + hi) * 0.5;
                let x = 0.75 * t * (1.0 - t) + t * t * t;
                if x < progress {
                    lo = t;
                } else {
                    hi = t;
                }
            }
            let t = (lo + hi) * 0.5;
            let u = 1.0 - t;
            3.0 * u * t * (0.1 * u + t) + t * t * t
        })
    }

    #[derive(IntoElement)]
    struct ShortcutDialog {
        model: ShortcutPanelModel,
        focus: FocusHandle,
        on_close: Rc<CloseHandler>,
        scale: f32,
    }

    impl RenderOnce for ShortcutDialog {
        fn render(self, window: &mut Window, _: &mut App) -> impl IntoElement {
            let messages = self.model.messages();
            let (background, title_color) = match self.model.theme {
                ShortcutTheme::Dark => (rgb(0x1e1e1e), rgb(0xd1d1d1)),
                ShortcutTheme::Light => (rgb(0xffffff), rgb(0x2f3547)),
            };
            let scaled = |value| px(value * self.scale);
            let viewport = window.viewport_size();
            let width = (viewport.width * 0.9).min(px(800.)) * self.scale;
            let artwork_width = (width - scaled(40.)).max(px(0.));

            let close = div()
                .id("shortcut-panel-close")
                .track_focus(&self.focus)
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(scaled(24.))
                .rounded(scaled(6.))
                .text_color(rgb(0x7b7b7b))
                .text_size(scaled(24.))
                .font_weight(FontWeight(300.))
                .line_height(scaled(24.))
                .cursor_pointer()
                .hover(|style| style.bg(rgba(0x80808026)))
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    (self.on_close)(ShortcutPanelClose::Button, window, cx);
                })
                .child("×");

            let header = div()
                .flex()
                .flex_none()
                .items_center()
                .justify_between()
                .pt(scaled(16.))
                .px(scaled(20.))
                .pb(scaled(12.))
                .child(
                    div()
                        .text_color(title_color)
                        .text_size(scaled(16.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .line_height(scaled(22.))
                        .child(ui_text(messages.title)),
                )
                .child(close);

            let content = div()
                .id("shortcut-panel-content")
                .flex()
                .flex_col()
                .flex_shrink()
                .min_h_0()
                .overflow_y_scroll()
                .px(scaled(20.))
                .child(
                    img(cached_artwork(self.model))
                        .id("shortcut-panel-artwork")
                        .flex_none()
                        .w(artwork_width)
                        .h(artwork_width * (1216. / 1520.))
                        .with_fallback(move || ui_text(messages.image_alt).into_any_element()),
                );

            div()
                .flex()
                .flex_none()
                .flex_col()
                .w(width)
                .max_h(viewport.height * 0.85 * self.scale)
                .pb(scaled(32.))
                .rounded(scaled(12.))
                .overflow_hidden()
                .occlude()
                .bg(background)
                .child(header)
                .child(content)
        }
    }

    /// Render as the last child of the window overlay. The host applies the
    /// state machine's focus effects, using this focus handle for the close button.
    pub fn shortcut_panel(
        state: &ShortcutPanelState,
        model: ShortcutPanelModel,
        focus: &FocusHandle,
        on_close: impl Fn(ShortcutPanelClose, &mut Window, &mut App) + 'static,
    ) -> Option<Div> {
        if !state.is_open() {
            return None;
        }

        let on_close: Rc<CloseHandler> = Rc::new(on_close);
        let backdrop_close = on_close.clone();
        let backdrop = div()
            .absolute()
            .inset_0()
            .size_full()
            .bg(rgba(0x00000099))
            .on_any_mouse_down(move |_, window, cx| {
                cx.stop_propagation();
                (backdrop_close)(ShortcutPanelClose::Backdrop, window, cx);
            });

        let panel = ShortcutDialog {
            model,
            focus: focus.clone(),
            on_close: on_close.clone(),
            scale: 1.0,
        }
        .with_animation(
            "shortcut-panel-scale",
            opening_animation(),
            |mut panel, progress| {
                panel.scale = 0.95 + progress * 0.05;
                panel
            },
        );
        let presentation = div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(backdrop)
            .child(panel)
            .with_animation(
                "shortcut-panel-fade",
                opening_animation(),
                |overlay, progress| overlay.opacity(progress),
            );

        Some(
            div()
                .absolute()
                .inset_0()
                .size_full()
                .occlude()
                .on_key_down(move |event, window, cx| {
                    if event.keystroke.key == "escape" {
                        window.prevent_default();
                        cx.stop_propagation();
                        (on_close)(ShortcutPanelClose::Escape, window, cx);
                    }
                })
                .child(presentation),
        )
    }
}

pub use ui::shortcut_panel;
