//! Native non-client hit regions for the Windows caption buttons.

/// Whether GPUI should fuse the title bar into the content area on this platform.
pub fn appears_transparent() -> bool {
    appears_transparent_on(std::env::consts::OS)
}

fn appears_transparent_on(os: &str) -> bool {
    matches!(os, "macos" | "windows")
}

#[cfg(any(target_os = "windows", test))]
mod caption {
    use gpui::{Div, Stateful, WindowControlArea, div, prelude::*, px, rgb, rgba, svg};

    pub const WINDOWS_CAPTION_CONTROLS_WIDTH: f32 = 138.;
    const BUTTON_WIDTH: f32 = 46.;
    const ICON_SIZE: f32 = 10.;
    const CLOSE_GROUP: &str = "windows-caption-close";
    const ICON_MINIMIZE: &str = "icons/window-minimize.svg";
    const ICON_MAXIMIZE: &str = "icons/window-maximize.svg";
    const ICON_RESTORE: &str = "icons/window-restore.svg";
    const ICON_CLOSE: &str = "icons/window-close.svg";

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    struct CaptionPalette {
        icon: u32,
        hover: u32,
        close_hover: u32,
        close_hover_icon: u32,
    }

    fn caption_palette(dark: bool) -> CaptionPalette {
        CaptionPalette {
            icon: if dark { 0xdce3ef } else { 0x2f3547 },
            hover: if dark { 0xffffff14 } else { 0x00000010 },
            close_hover: 0xc42b1c,
            close_hover_icon: 0xffffff,
        }
    }

    fn maximize_icon(maximized: bool) -> &'static str {
        if maximized {
            ICON_RESTORE
        } else {
            ICON_MAXIMIZE
        }
    }

    fn caption_button(
        id: &'static str,
        height: f32,
        area: WindowControlArea,
        icon: &'static str,
        palette: CaptionPalette,
    ) -> Stateful<Div> {
        let icon = svg()
            .path(icon)
            .size(px(ICON_SIZE))
            .flex_none()
            .text_color(rgb(palette.icon));
        let button = div()
            .id(id)
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .w(px(BUTTON_WIDTH))
            .h(px(height))
            // GPUI owns the non-client events, including Snap Layout and WM_CLOSE.
            .window_control_area(area)
            .occlude();
        if area == WindowControlArea::Close {
            button
                .group(CLOSE_GROUP)
                .hover(move |style| style.bg(rgb(palette.close_hover)))
                .child(icon.group_hover(CLOSE_GROUP, move |style| {
                    style.text_color(rgb(palette.close_hover_icon))
                }))
        } else {
            button
                .hover(move |style| style.bg(rgba(palette.hover)))
                .child(icon)
        }
    }

    /// Build the native caption controls at the height of their host title bar.
    pub fn windows_caption_controls(height: f32, dark: bool, maximized: bool) -> Stateful<Div> {
        let palette = caption_palette(dark);
        div()
            .id("windows-caption-controls")
            .debug_selector(|| "windows-caption-controls".to_owned())
            .flex()
            .flex_none()
            .w(px(WINDOWS_CAPTION_CONTROLS_WIDTH))
            .h(px(height))
            .children([
                caption_button(
                    "windows-caption-min",
                    height,
                    WindowControlArea::Min,
                    ICON_MINIMIZE,
                    palette,
                ),
                caption_button(
                    "windows-caption-max",
                    height,
                    WindowControlArea::Max,
                    maximize_icon(maximized),
                    palette,
                ),
                caption_button(
                    "windows-caption-close",
                    height,
                    WindowControlArea::Close,
                    ICON_CLOSE,
                    palette,
                ),
            ])
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::assets::NativeAssets;
        use gpui::AssetSource;

        #[test]
        fn caption_controls_match_the_three_title_bar_sizes() {
            assert_eq!(BUTTON_WIDTH, 46.);
            assert_eq!(WINDOWS_CAPTION_CONTROLS_WIDTH, 3. * BUTTON_WIDTH);
            for height in [48., 42., 40.] {
                let mut controls = windows_caption_controls(height, false, false);
                assert_eq!(controls.style().size.width, Some(px(138.).into()));
                assert_eq!(controls.style().size.height, Some(px(height).into()));

                let mut button = caption_button(
                    "size-test",
                    height,
                    WindowControlArea::Min,
                    ICON_MINIMIZE,
                    caption_palette(false),
                );
                assert_eq!(button.style().size.width, Some(px(46.).into()));
                assert_eq!(button.style().size.height, Some(px(height).into()));
            }
        }

        #[test]
        fn caption_hover_colors_match_light_and_dark_themes() {
            assert_eq!(
                caption_palette(false),
                CaptionPalette {
                    icon: 0x2f3547,
                    hover: 0x00000010,
                    close_hover: 0xc42b1c,
                    close_hover_icon: 0xffffff
                },
            );
            assert_eq!(
                caption_palette(true),
                CaptionPalette {
                    icon: 0xdce3ef,
                    hover: 0xffffff14,
                    close_hover: 0xc42b1c,
                    close_hover_icon: 0xffffff
                },
            );
        }

        #[test]
        fn maximize_state_selects_the_bundled_restore_icon() {
            assert_eq!(maximize_icon(false), "icons/window-maximize.svg");
            assert_eq!(maximize_icon(true), "icons/window-restore.svg");
            for path in [
                ICON_MINIMIZE,
                maximize_icon(false),
                maximize_icon(true),
                ICON_CLOSE,
            ] {
                let bytes = NativeAssets
                    .load(path)
                    .unwrap_or_else(|err| panic!("{path}: {err}"));
                let bytes = bytes.unwrap_or_else(|| panic!("Missing caption asset {path}"));
                let svg = std::str::from_utf8(&bytes).unwrap();
                assert!(svg.contains("viewBox=\"0 0 10 10\""), "{path}");
                assert!(svg.contains("stroke-width=\"1\""), "{path}");
            }
        }
    }
}

#[cfg(any(target_os = "windows", test))]
pub use caption::WINDOWS_CAPTION_CONTROLS_WIDTH;
#[cfg(target_os = "windows")]
pub use caption::windows_caption_controls;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_windows_use_fused_title_bars_on_macos_and_windows() {
        assert!(appears_transparent_on("macos"));
        assert!(appears_transparent_on("windows"));
        assert!(!appears_transparent_on("linux"));
        assert_eq!(
            appears_transparent(),
            cfg!(any(target_os = "macos", target_os = "windows"))
        );
    }
}
