//! Application appearance preference and semantic colors. Hosts can resolve
//! the same preference for every window, using its GPUI appearance for System.

use gpui::prelude::*;
use gpui::{
    App, BoxShadow, Div, ElementId, FontWeight, Stateful, Window, WindowAppearance, div, px, rgb,
    rgba,
};
use matou_native_core::sessions::InterfaceLocale;

use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use matou_native_terminal::Theme;
use serde_json::Value;

pub const THEME_PREFERENCE_FILE: &str = "theme.json";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemePreference {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemePreference {
    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    pub fn from_value(value: &str) -> Option<Self> {
        match value {
            "system" => Some(Self::System),
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    pub const fn resolve(self, system: WindowAppearance) -> AppTheme {
        match self {
            Self::Light => AppTheme::Light,
            Self::Dark => AppTheme::Dark,
            Self::System => match system {
                WindowAppearance::Light | WindowAppearance::VibrantLight => AppTheme::Light,
                WindowAppearance::Dark | WindowAppearance::VibrantDark => AppTheme::Dark,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppTheme {
    Light,
    Dark,
}

impl AppTheme {
    pub const fn palette(self) -> &'static ThemePalette {
        match self {
            Self::Light => &LIGHT,
            Self::Dark => &DARK,
        }
    }
}

/// Holds the selected preference separately from the current OS appearance.
/// Feed `window.appearance()` to `set_system_appearance` from GPUI's
/// `Context::observe_window_appearance` and redraw when it returns true.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThemeState {
    preference: ThemePreference,
    system: WindowAppearance,
}

impl ThemeState {
    pub const fn new(preference: ThemePreference, system: WindowAppearance) -> Self {
        Self { preference, system }
    }

    pub const fn preference(self) -> ThemePreference {
        self.preference
    }

    pub const fn effective(self) -> AppTheme {
        self.preference.resolve(self.system)
    }

    /// Returns whether the effective palette changed. Callers still update
    /// the selector when the saved preference itself changes.
    pub fn set_preference(&mut self, preference: ThemePreference) -> bool {
        let before = self.effective();
        self.preference = preference;
        self.effective() != before
    }

    pub fn set_system_appearance(&mut self, system: WindowAppearance) -> bool {
        let before = self.effective();
        self.system = system;
        self.effective() != before
    }
}

/// Uses System for missing files, malformed JSON or unknown values.
/// Other read errors remain available to the host for diagnostics.
pub fn read_theme_preference(data_root: &Path) -> io::Result<ThemePreference> {
    let bytes = match fs::read(data_root.join(THEME_PREFERENCE_FILE)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(ThemePreference::default());
        }
        Err(error) => return Err(error),
    };
    Ok(serde_json::from_slice::<Value>(&bytes)
        .ok()
        .and_then(|value| {
            value["preference"]
                .as_str()
                .and_then(ThemePreference::from_value)
        })
        .unwrap_or_default())
}

/// Replaces the dedicated theme file with a complete JSON document.
pub fn write_theme_preference(data_root: &Path, preference: ThemePreference) -> io::Result<()> {
    publish_preference(data_root, preference, |from, to| fs::rename(from, to))
}

fn publish_preference(
    data_root: &Path,
    preference: ThemePreference,
    replace: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> io::Result<()> {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let staged = data_root.join(format!(
        ".theme-{}-{now}-{sequence}.tmp",
        std::process::id()
    ));
    fs::create_dir_all(data_root)?;
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)?;
    let result = writeln!(file, "{{\"preference\":\"{}\"}}", preference.as_str())
        .and_then(|()| file.sync_all());
    drop(file);
    let result = result.and_then(|()| replace(&staged, &data_root.join(THEME_PREFERENCE_FILE)));
    if result.is_err() {
        let _ = fs::remove_file(staged);
    }
    result
}

/// GPUI colors are packed as `0xRRGGBBAA`, except fields marked RGB. Keeping
/// workspace tokens distinct from console chrome preserves the CSS cascade.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThemePalette {
    pub workspace: WorkspacePalette,
    pub chrome: ChromePalette,
    pub sidebar: SidebarPalette,
    pub tabs: TabPalette,
    pub cards: CardPalette,
    pub footer: FooterPalette,
    pub settings: SettingsPalette,
    pub recovery_water: RecoveryWaterPalette,
    pub recovery_page: DatabaseRecoveryPalette,
    pub terminal: Theme,
}

/// CSS workspace custom properties shared by the board and settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkspacePalette {
    pub canvas: u32,
    pub panel: u32,
    pub panel_muted: u32,
    pub line: u32,
    pub line_strong: u32,
    pub text: u32,
    pub muted: u32,
    pub faint: u32,
    pub accent: u32,
    pub accent_soft: u32,
    pub accent_line: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChromePalette {
    pub shell: u32,
    pub stage: u32,
    pub background: u32,
    pub text: u32,
    pub divider: u32,
    pub muted: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SidebarPalette {
    pub divider: u32,
    pub glass_stops: [u32; 3],
    pub glass_highlight: u32,
    pub heading: u32,
    pub new_workspace: u32,
    pub workspace_name: u32,
    pub task_text: u32,
    pub task_active_text: u32,
    pub task_active: u32,
    pub task_hover: u32,
    pub badge_text: u32,
    pub badge_background: u32,
    /// Sidebar surface using the CLI palette independently of AppKit vibrancy.
    pub background: u32,
    /// Shared translucent action hover for the new-workspace row and bell.
    pub action_hover: u32,
    /// Neutral hover used by workspace add/more rows.
    pub control_hover: u32,
    /// Default workspace/task glyph color.
    pub icon: u32,
    /// Dimmer add/more/pin glyph color.
    pub icon_muted: u32,
    pub chevron: u32,
    /// Folder glyph for the active workspace.
    pub accent_icon: u32,
    pub toolbar_text: u32,
    pub relink_text: u32,
    pub relink_background: u32,
    pub relink_line: u32,
    pub relink_hover: u32,
    pub invalid_text: u32,
    pub invalid_background: u32,
    pub bell: u32,
    /// Border of the notification count over the sidebar surface.
    pub bell_badge_border: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabPalette {
    pub bar: u32,
    pub inactive_text: u32,
    pub hover_text: u32,
    pub active_text: u32,
    pub active_background: u32,
    pub action_text: u32,
    pub action_hover_text: u32,
    pub action_hover: u32,
    pub add_background: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CardPalette {
    pub canvas: u32,
    pub background: u32,
    pub header: u32,
    pub title: u32,
    pub environment_text: u32,
    pub compact_background: u32,
    pub compact_text: u32,
    pub compact_title: u32,
    pub child_badge_text: u32,
    pub child_badge_background: u32,
    pub child_badge_line: u32,
    pub fork_text: u32,
    pub edge_intent: u32,
    pub edge_intent_progress: u32,
    pub action_text: u32,
    pub action_text_hover: u32,
    pub action_hover: u32,
    pub remove_hover: u32,
    pub stopped_background: u32,
    pub stopped_text: u32,
    pub stopped_header: u32,
    pub stopped_remove_text: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FooterPalette {
    pub background: u32,
    pub top_line: u32,
    pub text: u32,
    pub divider: u32,
    pub breadcrumb: u32,
    pub breadcrumb_count: u32,
    pub breadcrumb_hover: u32,
    pub breadcrumb_hover_text: u32,
    pub model: u32,
    pub session_name: u32,
    pub usage: u32,
    pub usage_accent: u32,
    pub account: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SettingsPalette {
    pub nav: u32,
    /// RGB, for GPUI's `rgb` helper.
    pub nav_text: u32,
    pub tabs: u32,
    pub selected_tab: u32,
    pub current_summary: u32,
    pub providers: u32,
    pub provider_hover: u32,
    pub provider_selected: u32,
    pub provider_text: u32,
    pub provider_action_text: u32,
    pub provider_action_background: u32,
    pub provider_disabled_text: u32,
    pub provider_disabled_background: u32,
    pub language_list: u32,
    pub language_text: u32,
    pub language_hover: u32,
    pub language_selected: u32,
    pub impact_title: u32,
    pub dialog: u32,
    pub dialog_label: u32,
    pub input_text: u32,
    pub input_background: u32,
    pub input_focus_line: u32,
    pub input_focus_ring: u32,
    /// Language radio outline when it is not selected.
    pub language_radio_border: u32,
    pub language_error: u32,
    pub language_button_hover: u32,
    pub language_primary: u32,
    pub language_primary_hover: u32,
    pub language_menu_text: u32,
    pub language_menu_background: u32,
    pub language_menu_active_background: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RecoveryWaterPalette {
    pub base: u32,
    /// Read-back base used by the existing native water rasterizer.
    pub native_base: u32,
    /// RGB plus the original gradient alpha, kept before 8-bit quantization.
    pub tint_rgb: u32,
    pub tint_alpha: f64,
    pub center: [f64; 2],
    pub fade_end: f64,
    pub inner_border: u32,
    pub inner_shadow: u32,
    pub overlay: u32,
    pub overlay_failed: u32,
    pub overlay_text: u32,
    pub panel: u32,
    pub panel_line: u32,
    pub panel_muted: u32,
}

/// The standalone database recovery page has a shared palette in
/// `recovery.css`; it declares no light or dark override.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DatabaseRecoveryPalette {
    pub page: u32,
    pub text: u32,
    pub card: u32,
    pub card_line: u32,
    pub muted: u32,
    pub accent: u32,
    pub error_text: u32,
    pub error_background: u32,
    pub success_text: u32,
    pub success_background: u32,
    pub progress_text: u32,
    pub progress_background: u32,
}

// hierarchy.css:1–19, 704–775, 864–869 and 1100–1214;
// session-canvas.css:70–93; recovery.css:1–28.
pub const LIGHT: ThemePalette = ThemePalette {
    workspace: WorkspacePalette {
        canvas: 0xf7f8faff,
        panel: 0xffffffff,
        panel_muted: 0xfbfcfdff,
        line: 0x424b581a,
        line_strong: 0x424b5829,
        text: 0x303744ff,
        muted: 0x7e8794ff,
        faint: 0xa3aab4ff,
        accent: 0x347fd6ff,
        accent_soft: 0x347fd61c,
        accent_line: 0x347fd647,
    },
    chrome: ChromePalette {
        shell: 0xf4f5f5ff,
        stage: 0xf7f8faff,
        background: 0xffffffff,
        text: 0x2f3547ff,
        divider: 0xe8e8e8ff,
        muted: 0x606572ff,
    },
    sidebar: SidebarPalette {
        divider: 0x58718b1f,
        glass_stops: [0xfbfdff17, 0xebf3f90b, 0xf6fafd12],
        glass_highlight: 0xffffff1a,
        heading: 0x2f3547ff,
        new_workspace: 0x2f3547ff,
        workspace_name: 0x656d76ff,
        // Keep the native task-row colors (workspace.rs, task row builder).
        task_text: 0x6f7683ff,
        task_active_text: 0x2f3547ff,
        task_active: 0x57657f18,
        task_hover: 0x57657f12,
        badge_text: 0x91949eff,
        badge_background: 0xeceef3ff,
        background: 0xf2f2f5ff,
        action_hover: 0x2f35470f,
        control_hover: 0x00000014,
        icon: 0x656d76ff,
        icon_muted: 0x81848dff,
        chevron: 0x777777ff,
        accent_icon: 0x4078f2ff,
        toolbar_text: 0x989da7ff,
        relink_text: 0x9a6818ff,
        relink_background: 0xfff8e8ff,
        relink_line: 0x9a681840,
        relink_hover: 0xd99d3c26,
        invalid_text: 0xd99d3cff,
        invalid_background: 0xd99d3c1a,
        bell: 0x8d96a3ff,
        bell_badge_border: 0xf4f5f7ff,
    },
    tabs: TabPalette {
        bar: 0xffffffff,
        inactive_text: 0x00000066,
        hover_text: 0x000000a6,
        active_text: 0x2f3547ff,
        active_background: 0xeceef3ff,
        action_text: 0x606572ff,
        action_hover_text: 0x2f3547ff,
        action_hover: 0x00000014,
        add_background: 0xeff0f4ff,
    },
    cards: CardPalette {
        canvas: 0xe9edf2ff,
        background: 0xfcfcfdff,
        header: 0xffffffff,
        title: 0x303744ff,
        environment_text: 0x8a919bff,
        compact_background: 0xf2f4f7ff,
        compact_text: 0x6b7482ff,
        compact_title: 0x364050ff,
        child_badge_text: 0x2666b1ff,
        child_badge_background: 0xe8f2ffff,
        child_badge_line: 0xb9d6ffff,
        fork_text: 0x5f7898ff,
        // The existing native edge indicator uses the shared blue.
        edge_intent: 0x6faaf024,
        edge_intent_progress: 0x6faaf0ff,
        action_text: 0x00000085,
        action_text_hover: 0x000000d1,
        action_hover: 0x0000000f,
        remove_hover: 0xfff0efff,
        stopped_background: 0xf8f9fbff,
        stopped_text: 0x687180ff,
        stopped_header: 0xffffffff,
        stopped_remove_text: 0x6a7380ff,
    },
    footer: FooterPalette {
        background: 0xffffffff,
        top_line: 0xe8e8e8ff,
        text: 0x606572ff,
        divider: 0xe8e8e8ff,
        breadcrumb: 0x000000ad,
        breadcrumb_count: 0x00000061,
        breadcrumb_hover: 0x00000014,
        breadcrumb_hover_text: 0x2f3547ff,
        model: 0x79c0ffff,
        session_name: 0x8b949eff,
        usage: 0x7f8a99ff,
        usage_accent: 0x58a6ffff,
        account: 0x647e9fff,
    },
    settings: SettingsPalette {
        nav: 0xfbfcfdff,
        nav_text: 0x5f6976,
        tabs: 0xf0f2f5ff,
        selected_tab: 0xfffffff0,
        current_summary: 0xf3f7fdff,
        providers: 0xfbfcfdff,
        provider_hover: 0xffffffff,
        provider_selected: 0xedf5ffff,
        provider_text: 0x414a57ff,
        provider_action_text: 0x525d6aff,
        provider_action_background: 0xffffffd1,
        provider_disabled_text: 0xa3aab4ff,
        provider_disabled_background: 0xdee2e894,
        language_list: 0xfbfcfdff,
        language_text: 0x414a57ff,
        language_hover: 0xffffffff,
        language_selected: 0xf5f8fcff,
        impact_title: 0x6a7481ff,
        dialog: 0xffffffff,
        // Existing native provider fields use the full setup dialog's colors
        // (provider-setup.css), including its opaque white input surface.
        dialog_label: 0x344257ff,
        input_text: 0x344257ff,
        input_background: 0xffffffff,
        input_focus_line: 0x5c9cedff,
        input_focus_ring: 0xe8f1ffff,
        language_radio_border: 0x9aa3aeff,
        language_error: 0xb34d47ff,
        language_button_hover: 0xf7faffff,
        language_primary: 0x347fd6ff,
        language_primary_hover: 0x2f79dfff,
        language_menu_text: 0x2679d8ff,
        language_menu_background: 0xe4efffff,
        language_menu_active_background: 0xeef5ffff,
    },
    recovery_water: RecoveryWaterPalette {
        base: 0xfafcffff,
        native_base: 0xfbfcffff,
        tint_rgb: 0x5699db,
        tint_alpha: 0.055,
        center: [0.5, 0.35],
        fade_end: 0.48,
        inner_border: 0xb5d8ff0e,
        inner_shadow: 0x135c9f0e,
        overlay: 0xf4f7fbeb,
        overlay_failed: 0xfff6f5f0,
        overlay_text: 0x29364aff,
        panel: 0xfffffffa,
        panel_line: 0xc8d4e3ff,
        panel_muted: 0x617086ff,
    },
    recovery_page: DatabaseRecoveryPalette {
        page: 0xf4f6f9ff,
        text: 0x20242dff,
        card: 0xffffffff,
        card_line: 0xdce1e8ff,
        muted: 0x606a78ff,
        accent: 0x3478f6ff,
        error_text: 0xa12525ff,
        error_background: 0xfff0efff,
        success_text: 0x17653aff,
        success_background: 0xedf9f1ff,
        progress_text: 0x275da8ff,
        progress_background: 0xedf4ffff,
    },
    terminal: Theme::LIGHT,
};

// Dark is the default CSS for the chrome and canvas; the workspace and
// settings variables have explicit dark overrides in hierarchy.css.
pub const DARK: ThemePalette = ThemePalette {
    workspace: WorkspacePalette {
        canvas: 0x181a1eff,
        panel: 0x2c2f35f5,
        panel_muted: 0x23262be0,
        line: 0xffffff16,
        line_strong: 0xffffff26,
        text: 0xe1e5ebff,
        muted: 0x8d96a2ff,
        faint: 0x626b77ff,
        accent: 0x67a6efff,
        accent_soft: 0x4a91e224,
        accent_line: 0x5b9eeb47,
    },
    chrome: ChromePalette {
        shell: 0xf4f5f5ff,
        stage: 0x1b1b1bff,
        background: 0x151515ff,
        text: 0xfafafaff,
        divider: 0x262626ff,
        muted: 0x7b7b7bff,
    },
    sidebar: SidebarPalette {
        divider: 0xffffff29,
        glass_stops: [0x1b1f2657, 0x14181e38, 0x191e2547],
        glass_highlight: 0xffffff17,
        heading: 0xdfdfdfff,
        new_workspace: 0xe8e8e8ff,
        workspace_name: 0xdededeff,
        task_text: 0x898989ff,
        task_active_text: 0xffffffff,
        task_active: 0xffffff17,
        task_hover: 0x1d1d1dff,
        badge_text: 0x858585ff,
        badge_background: 0xffffff0f,
        background: 0x1b1f26ff,
        action_hover: 0xffffff12,
        control_hover: 0xffffff1a,
        icon: 0x898989ff,
        icon_muted: 0x858585ff,
        chevron: 0x777777ff,
        accent_icon: 0x8db8e8ff,
        toolbar_text: 0x989da7ff,
        relink_text: 0xd99d3cff,
        relink_background: 0x2a1e17ff,
        relink_line: 0xd99d3c40,
        relink_hover: 0xd99d3c26,
        invalid_text: 0xd99d3cff,
        invalid_background: 0xd99d3c1a,
        bell: 0x8d96a3ff,
        bell_badge_border: 0x23262bff,
    },
    tabs: TabPalette {
        bar: 0x151515ff,
        inactive_text: 0x888888ff,
        hover_text: 0xffffffbf,
        active_text: 0xfffffff2,
        active_background: 0x2c2c2cff,
        action_text: 0xffffff4d,
        action_hover_text: 0xffffffff,
        action_hover: 0xffffff1a,
        add_background: 0xffffff0a,
    },
    cards: CardPalette {
        canvas: 0x111317ff,
        background: 0x191c21ff,
        header: 0x1d2025ff,
        title: 0xffffffdb,
        environment_text: 0xffffff61,
        compact_background: 0x151515ff,
        compact_text: 0xffffff8f,
        compact_title: 0xffffffd1,
        child_badge_text: 0x78b3f4ff,
        child_badge_background: 0x5b9de91a,
        child_badge_line: 0x5b9de96b,
        fork_text: 0x97b9e0b8,
        edge_intent: 0x6faaf024,
        edge_intent_progress: 0x6faaf0ff,
        action_text: 0xffffffbf,
        action_text_hover: 0xffffffff,
        action_hover: 0xffffff1f,
        remove_hover: 0x3a1f1fff,
        stopped_background: 0x15181dff,
        stopped_text: 0x8b93a1ff,
        stopped_header: 0x1d2025ff,
        stopped_remove_text: 0x9aa3aeff,
    },
    footer: FooterPalette {
        background: 0x151515ff,
        top_line: 0x262626ff,
        text: 0x6e7681ff,
        divider: 0x21262dff,
        breadcrumb: 0xffffffb8,
        breadcrumb_count: 0xffffff66,
        breadcrumb_hover: 0xffffff14,
        breadcrumb_hover_text: 0xffffffff,
        account: 0x8b949eff,
        ..LIGHT.footer
    },
    settings: SettingsPalette {
        nav: 0x1f2227b8,
        nav_text: 0x9aa3ae,
        tabs: 0xffffff0e,
        selected_tab: 0xffffff18,
        current_summary: 0x2c496b57,
        providers: 0x25282dc7,
        provider_hover: 0xffffff09,
        provider_selected: 0x2c496b38,
        provider_text: 0xdde2e8ff,
        provider_action_text: 0xcbd1d9ff,
        provider_action_background: 0xffffff0b,
        provider_disabled_text: 0x6e7782ff,
        provider_disabled_background: 0xffffff0a,
        language_list: 0x25282dc7,
        language_text: 0xdde2e8ff,
        language_hover: 0xffffff09,
        language_selected: 0x2c496b38,
        impact_title: 0xa3abb5ff,
        dialog: 0x25282dfa,
        dialog_label: 0xa8b0bbff,
        input_text: 0xe2e6ecff,
        input_background: 0xffffff0b,
        input_focus_line: 0x5b9eeb47,
        input_focus_ring: 0x4a91e224,
        language_radio_border: 0x6e7782ff,
        language_error: 0xbd8b87ff,
        language_button_hover: 0x4a91e224,
        language_primary: 0x2f79dfff,
        language_primary_hover: 0x2274dfff,
        language_menu_text: 0x67a6efff,
        language_menu_background: 0x2c496b38,
        language_menu_active_background: 0xffffff09,
    },
    recovery_water: RecoveryWaterPalette {
        base: 0x1b1b1bff,
        native_base: 0x1b1b1bff,
        tint_rgb: 0x4793de,
        tint_alpha: 0.05,
        overlay: 0x0d1117e0,
        overlay_failed: 0x190e0fe6,
        overlay_text: 0xdbe6f5ff,
        panel: 0x181f29f5,
        panel_line: 0x7da1d14d,
        panel_muted: 0x93a2b7ff,
        ..LIGHT.recovery_water
    },
    recovery_page: LIGHT.recovery_page,
    terminal: Theme::DARK,
};

/// Draws one System / Light / Dark choice list as a 42 pt settings row stack.
///
/// `preference` is the saved preference, so the control never owns selection
/// state. `on_change` runs only for a different choice, letting the host avoid
/// duplicate writes. Mouse clicks and Tab followed by Enter/Space both select.
pub fn theme_preference_picker(
    id: impl Into<ElementId>,
    preference: ThemePreference,
    palette: &ThemePalette,
    locale: InterfaceLocale,
    on_change: impl Fn(ThemePreference, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let id = id.into();
    let on_change: std::rc::Rc<dyn Fn(ThemePreference, &mut Window, &mut App)> =
        std::rc::Rc::new(on_change);

    let mut list = div()
        .id(id.clone())
        .flex()
        .flex_col()
        .w_full()
        .overflow_hidden()
        .rounded(px(10.))
        .border_1()
        .border_color(rgba(palette.workspace.line))
        .bg(rgba(palette.settings.language_list))
        // Frozen `.model-settings__language-options` shadow:
        // `0 1px 2px rgba(34,45,60,.02)`.
        .shadow(vec![BoxShadow {
            color: rgba(0x222d3c05).into(),
            offset: gpui::point(px(0.), px(1.)),
            blur_radius: px(2.),
            spread_radius: px(0.),
        }]);

    for (index, option) in ThemePreference::ALL.iter().copied().enumerate() {
        list = list.child(theme_preference_row(
            id.clone(),
            index,
            option,
            preference,
            palette,
            locale,
            on_change.clone(),
        ));
    }
    list
}

fn theme_preference_row(
    id: ElementId,
    index: usize,
    option: ThemePreference,
    preference: ThemePreference,
    palette: &ThemePalette,
    locale: InterfaceLocale,
    on_change: std::rc::Rc<dyn Fn(ThemePreference, &mut Window, &mut App)>,
) -> Stateful<Div> {
    let selected = option.as_str() == preference.as_str();
    let text_color = if selected {
        palette.workspace.accent
    } else {
        palette.settings.provider_text
    };
    let hover = palette.settings.language_hover;
    let focus_line = palette.workspace.accent_line;
    let key_change = on_change.clone();

    let mut row = div()
        .id((id, option.as_str()))
        .relative()
        .w_full()
        .h(px(42.))
        .flex()
        .items_center()
        .gap(px(9.))
        .px(px(13.))
        .text_size(px(10.))
        .font_weight(FontWeight(650.))
        .text_color(rgba(text_color))
        .cursor_pointer()
        // The forward auto-generated focus handle keeps this list stateless.
        // A 2 pt inner accent line is the focus-visible outline from
        // `hierarchy.css:1160`; the rounded list clips an outer ring.
        .focusable()
        .tab_stop(true)
        .focus(move |style| style.border_2().border_color(rgba(focus_line)))
        // Handle Enter/Space on key-down and stop propagation so a settings
        // scroll container cannot also act on the key. Mouse selection stays
        // on `on_click`.
        .on_key_down(move |event: &gpui::KeyDownEvent, window, cx| {
            if event.keystroke.modifiers.modified() {
                return;
            }
            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                cx.stop_propagation();
                if option.as_str() != preference.as_str() {
                    key_change(option, window, cx);
                }
            }
        })
        .on_click(move |event: &gpui::ClickEvent, window, cx| {
            if matches!(event, gpui::ClickEvent::Keyboard(_)) {
                return;
            }
            if option.as_str() != preference.as_str() {
                on_change(option, window, cx);
            }
        });

    if index + 1 < ThemePreference::ALL.len() {
        row = row.border_b_1().border_color(rgba(palette.workspace.line));
    }
    if selected {
        row = row.bg(rgba(palette.settings.language_selected)).child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left_0()
                .w(px(2.))
                .bg(rgba(palette.workspace.accent)),
        );
    } else {
        row = row.hover(move |style| style.bg(rgba(hover)));
    }

    row.child(theme_preference_radio(selected, palette))
        .child(crate::ui_text::ui_text(theme_preference_label(
            option, locale,
        )))
}

fn theme_preference_radio(selected: bool, palette: &ThemePalette) -> Div {
    let border = if selected {
        palette.workspace.accent
    } else {
        palette.workspace.muted
    };
    let mut radio = div()
        .flex_none()
        .size(px(13.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .border_1()
        .border_color(rgba(border))
        .bg(rgba(if selected {
            palette.workspace.accent
        } else {
            palette.settings.language_list
        }));
    if selected {
        radio = radio.child(div().size(px(5.)).rounded_full().bg(rgb(0xffffff)));
    }
    radio
}

fn theme_preference_label(option: ThemePreference, locale: InterfaceLocale) -> &'static str {
    match option {
        ThemePreference::System => locale.text("跟随系统", "Follow system"),
        ThemePreference::Light => locale.text("浅色", "Light"),
        ThemePreference::Dark => locale.text("深色", "Dark"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ThemePreference::{Dark, Light, System};

    const NATIVE_WORKSPACE: &str = include_str!("workspace.rs");

    #[test]
    fn reads_preferences_and_tracks_system_appearance_behind_overrides() {
        use WindowAppearance as OS;

        for (stored, preference) in [("system", System), ("light", Light), ("dark", Dark)] {
            assert_eq!(ThemePreference::from_value(stored), Some(preference));
            assert_eq!(preference.as_str(), stored);
        }
        assert_eq!(ThemePreference::from_value("Dark"), None);
        assert_eq!(ThemePreference::default(), System);

        let mut state = ThemeState::new(System, OS::Light);
        assert_eq!(state.effective(), AppTheme::Light);
        assert!(!state.set_system_appearance(OS::VibrantLight));
        assert!(state.set_system_appearance(OS::VibrantDark));
        assert_eq!(state.effective(), AppTheme::Dark);
        assert!(!state.set_preference(Dark));
        assert_eq!(state.preference(), Dark);
        assert!(!state.set_system_appearance(OS::Light));
        assert_eq!(state.effective(), AppTheme::Dark);
        assert!(state.set_preference(System));
        assert_eq!(state.effective(), AppTheme::Light);
        assert!(!state.set_preference(Light));
        assert!(!state.set_system_appearance(OS::Dark));
        assert_eq!(state.effective(), AppTheme::Light);
        assert!(state.set_preference(System));
        assert_eq!(state.effective(), AppTheme::Dark);
    }

    struct TestDir(std::path::PathBuf);

    impl TestDir {
        fn new() -> io::Result<Self> {
            // Keep fixture writes inside the isolated checkout.
            let parent = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target");
            let dir = parent.join(format!("theme-test-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&dir)?;
            Ok(Self(dir))
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn replaces_the_theme_preference_and_recovers_from_corrupt_files() -> io::Result<()> {
        let dir = TestDir::new()?;
        let root = &dir.0;
        let path = root.join(THEME_PREFERENCE_FILE);
        assert_eq!(read_theme_preference(root)?, System);
        for preference in [Light, Dark, System] {
            write_theme_preference(root, preference)?;
            assert_eq!(read_theme_preference(root)?, preference);
            assert_eq!(fs::read_dir(root)?.count(), 1);
        }
        for bad in [
            "{",
            "[]",
            r#"{"preference":false}"#,
            r#"{"preference":"other"}"#,
        ] {
            fs::write(&path, bad)?;
            assert_eq!(read_theme_preference(root)?, System);
        }
        write_theme_preference(root, Dark)?;
        assert_eq!(read_theme_preference(root)?, Dark);
        Ok(())
    }

    #[test]
    fn failed_atomic_replace_preserves_the_previous_file_and_cleans_the_stage() -> io::Result<()> {
        let dir = TestDir::new()?;
        let root = &dir.0;
        write_theme_preference(root, Light)?;
        let result = publish_preference(root, Dark, |staged, path| {
            assert_eq!(staged.parent(), path.parent());
            assert_eq!(fs::read_to_string(path)?, "{\"preference\":\"light\"}\n");
            assert_eq!(fs::read_to_string(staged)?, "{\"preference\":\"dark\"}\n");
            Err(io::Error::other("test failure during replacement"))
        });
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Other);
        assert_eq!(read_theme_preference(root)?, Light);
        assert_eq!(fs::read_dir(root)?.count(), 1);
        Ok(())
    }

    // Color expectations extracted from archive/electron-0.2-final (1c2cfd73).
    #[test]
    fn workspace_and_chrome_tokens_match_frozen_colors() {
        let palette = LIGHT;
        let w = palette.workspace;
        assert_eq!(w.canvas, 0xf7f8faff);
        assert_eq!(w.panel, 0xffffffff);
        assert_eq!(w.panel_muted, 0xfbfcfdff);
        assert_eq!(w.line, 0x424b581a);
        assert_eq!(w.line_strong, 0x424b5829);
        assert_eq!(w.text, 0x303744ff);
        assert_eq!(w.muted, 0x7e8794ff);
        assert_eq!(w.faint, 0xa3aab4ff);
        assert_eq!(w.accent, 0x347fd6ff);
        assert_eq!(w.accent_soft, 0x347fd61c);
        assert_eq!(w.accent_line, 0x347fd647);
        let c = palette.chrome;
        assert_eq!(c.shell, 0xf4f5f5ff);
        assert_eq!(c.stage, 0xf7f8faff);
        assert_eq!(c.background, 0xffffffff);
        assert_eq!(c.divider, 0xe8e8e8ff);
        assert_eq!(c.muted, 0x606572ff);
        assert_eq!(c.text, 0x2f3547ff);
        assert_eq!(palette.tabs.active_background, 0xeceef3ff);
        assert_eq!(palette.tabs.bar, c.background);
        assert_eq!(palette.footer.background, c.background);
        assert_eq!(palette.footer.top_line, c.divider);
        let f = palette.footer;
        assert_eq!(f.model, 0x79c0ffff);
        assert_eq!(f.session_name, 0x8b949eff);
        assert_eq!(f.usage, 0x7f8a99ff);
        assert_eq!(f.usage_accent, 0x58a6ffff);

        let palette = DARK;
        let w = palette.workspace;
        assert_eq!(w.canvas, 0x181a1eff);
        assert_eq!(w.panel, 0x2c2f35f5);
        assert_eq!(w.panel_muted, 0x23262be0);
        assert_eq!(w.line, 0xffffff16);
        assert_eq!(w.line_strong, 0xffffff26);
        assert_eq!(w.text, 0xe1e5ebff);
        assert_eq!(w.muted, 0x8d96a2ff);
        assert_eq!(w.faint, 0x626b77ff);
        assert_eq!(w.accent, 0x67a6efff);
        assert_eq!(w.accent_soft, 0x4a91e224);
        assert_eq!(w.accent_line, 0x5b9eeb47);
        let c = palette.chrome;
        assert_eq!(c.shell, 0xf4f5f5ff);
        assert_eq!(c.stage, 0x1b1b1bff);
        assert_eq!(c.background, 0x151515ff);
        assert_eq!(c.divider, 0x262626ff);
        assert_eq!(c.muted, 0x7b7b7bff);
        assert_eq!(c.text, 0xfafafaff);
        assert_eq!(palette.tabs.active_background, 0x2c2c2cff);
        assert_eq!(palette.tabs.bar, c.background);
        assert_eq!(palette.footer.background, c.background);
        assert_eq!(palette.footer.top_line, c.divider);
        let f = palette.footer;
        assert_eq!(f.model, 0x79c0ffff);
        assert_eq!(f.session_name, 0x8b949eff);
        assert_eq!(f.usage, 0x7f8a99ff);
        assert_eq!(f.usage_accent, 0x58a6ffff);
    }

    #[test]
    fn sidebar_tab_and_bottom_bar_states_match_frozen_colors() {
        let s = LIGHT.sidebar;
        assert_eq!(s.divider, 0x58718b1f);
        assert_eq!(s.heading, 0x2f3547ff);
        assert_eq!(
            LIGHT.sidebar.glass_stops,
            [0xfbfdff17, 0xebf3f90b, 0xf6fafd12]
        );
        assert_eq!(LIGHT.sidebar.glass_highlight, 0xffffff1a);
        assert_eq!(
            DARK.sidebar.glass_stops,
            [0x1b1f2657, 0x14181e38, 0x191e2547]
        );
        assert_eq!(DARK.sidebar.glass_highlight, 0xffffff17);
        assert_eq!(s.new_workspace, 0x2f3547ff);
        assert_eq!(s.workspace_name, 0x656d76ff);
        assert_eq!(s.task_active_text, 0x2f3547ff);
        assert_eq!(s.task_active, 0x57657f18);
        assert_eq!(s.badge_text, 0x91949eff);
        assert_eq!(s.badge_background, 0xeceef3ff);

        assert!(NATIVE_WORKSPACE.contains("sidebar_palette.task_text"));
        assert!(NATIVE_WORKSPACE.contains("rgba(sidebar_palette.task_hover)"));

        let s = DARK.sidebar;
        assert_eq!(s.divider, 0xffffff29);
        assert_eq!(s.heading, 0xdfdfdfff);
        assert_eq!(s.new_workspace, 0xe8e8e8ff);
        assert_eq!(s.workspace_name, 0xdededeff);
        assert_eq!(s.task_text, 0x898989ff);
        assert_eq!(s.task_active_text, 0xffffffff);
        assert_eq!(s.task_active, 0xffffff17);
        assert_eq!(s.task_hover, 0x1d1d1dff);
        assert_eq!(s.badge_text, 0x858585ff);
        assert_eq!(s.badge_background, 0xffffff0f);

        let t = LIGHT.tabs;
        assert_eq!(t.inactive_text, 0x00000066);
        assert_eq!(t.hover_text, 0x000000a6);
        assert_eq!(t.active_text, 0x2f3547ff);
        assert_eq!(t.action_text, 0x606572ff);
        assert_eq!(t.add_background, 0xeff0f4ff);
        assert_eq!(t.action_hover_text, 0x2f3547ff);
        assert_eq!(t.action_hover, 0x00000014);
        let f = LIGHT.footer;
        assert_eq!(f.text, 0x606572ff);
        assert_eq!(f.divider, 0xe8e8e8ff);
        assert_eq!(f.breadcrumb, 0x000000ad);
        assert_eq!(f.breadcrumb_count, 0x00000061);
        assert_eq!(f.breadcrumb_hover, 0x00000014);

        let t = DARK.tabs;
        assert_eq!(t.inactive_text, 0x888888ff);
        assert_eq!(t.hover_text, 0xffffffbf);
        assert_eq!(t.active_text, 0xfffffff2);
        assert_eq!(t.active_background, 0x2c2c2cff);
        assert_eq!(t.action_text, 0xffffff4d);
        assert_eq!(t.action_hover_text, 0xffffffff);
        assert_eq!(t.action_hover, 0xffffff1a);
        assert_eq!(t.add_background, 0xffffff0a);
        let f = DARK.footer;
        assert_eq!(f.text, 0x6e7681ff);
        assert_eq!(f.divider, 0x21262dff);
        assert_eq!(f.breadcrumb, 0xffffffb8);
        assert_eq!(f.breadcrumb_count, 0xffffff66);
        assert_eq!(f.breadcrumb_hover, 0xffffff14);
    }

    #[test]
    fn session_card_tokens_match_frozen_colors_and_native_light_edge() {
        let l = LIGHT.cards;
        assert_eq!(l.canvas, 0xe9edf2ff);
        assert_eq!(l.background, 0xfcfcfdff);
        assert_eq!(l.header, 0xffffffff);
        assert_eq!(l.title, 0x303744ff);
        assert_eq!(l.environment_text, 0x8a919bff);
        assert_eq!(l.compact_text, 0x6b7482ff);
        assert_eq!(l.compact_title, 0x364050ff);
        assert_eq!(l.child_badge_text, 0x2666b1ff);
        assert_eq!(l.child_badge_background, 0xe8f2ffff);
        assert_eq!(l.child_badge_line, 0xb9d6ffff);
        assert_eq!(l.fork_text, 0x5f7898ff);
        assert_eq!(l.compact_background, 0xf2f4f7ff);

        let d = DARK.cards;
        assert_eq!(d.canvas, 0x111317ff);
        assert_eq!(d.background, 0x191c21ff);
        assert_eq!(d.header, 0x1d2025ff);
        assert_eq!(d.title, 0xffffffdb);
        assert_eq!(d.environment_text, 0xffffff61);
        assert_eq!(d.compact_text, 0xffffff8f);
        assert_eq!(d.compact_title, 0xffffffd1);
        assert_eq!(d.child_badge_text, 0x78b3f4ff);
        assert_eq!(d.child_badge_background, 0x5b9de91a);
        assert_eq!(d.child_badge_line, 0x5b9de96b);
        assert_eq!(d.fork_text, 0x97b9e0b8);
        assert_eq!(d.edge_intent, 0x6faaf024);
        assert_eq!(d.edge_intent_progress, 0x6faaf0ff);
        assert_eq!(d.compact_background, DARK.chrome.background);
        assert!(NATIVE_WORKSPACE.contains(&format!("fill(track, rgba(0x{:08x}))", l.edge_intent)));
        assert!(NATIVE_WORKSPACE.contains(&format!(
            "let mut color: Hsla = rgb(0x{:06x}).into()",
            l.edge_intent_progress >> 8
        )));
    }

    #[test]
    fn settings_colors_match_the_frozen_cascade() {
        let s = LIGHT.settings;
        assert_eq!(s.tabs, 0xf0f2f5ff);
        assert_eq!(s.selected_tab, 0xfffffff0);
        assert_eq!(s.current_summary, 0xf3f7fdff);
        assert_eq!(s.provider_hover, 0xffffffff);
        assert_eq!(s.provider_text, 0x414a57ff);
        assert_eq!(s.provider_action_text, 0x525d6aff);
        assert_eq!(s.provider_action_background, 0xffffffd1);
        assert_eq!(s.provider_disabled_background, 0xdee2e894);
        assert_eq!(s.language_text, 0x414a57ff);
        assert_eq!(s.language_hover, 0xffffffff);
        assert_eq!(s.language_selected, 0xf5f8fcff);
        assert_eq!(s.impact_title, 0x6a7481ff);
        assert_eq!(s.nav_text, 0x5f6976);
        assert_eq!(s.provider_selected, 0xedf5ffff);
        assert_eq!(s.dialog, 0xffffffff);
        assert_eq!(s.dialog_label, 0x344257ff);
        assert_eq!(s.input_text, 0x344257ff);
        assert_eq!(s.input_background, 0xffffffff);
        assert_eq!(s.input_focus_line, 0x5c9cedff);
        assert_eq!(s.input_focus_ring, 0xe8f1ffff);
        assert_eq!(s.nav, LIGHT.workspace.panel_muted);
        assert_eq!(s.providers, LIGHT.workspace.panel_muted);
        assert_eq!(s.language_list, LIGHT.workspace.panel_muted);
        assert_eq!(s.provider_disabled_text, LIGHT.workspace.faint);

        let s = DARK.settings;
        assert_eq!(s.tabs, 0xffffff0e);
        assert_eq!(s.selected_tab, 0xffffff18);
        assert_eq!(s.current_summary, 0x2c496b57);
        assert_eq!(s.provider_hover, 0xffffff09);
        assert_eq!(s.provider_text, 0xdde2e8ff);
        assert_eq!(s.provider_action_text, 0xcbd1d9ff);
        assert_eq!(s.provider_action_background, 0xffffff0b);
        assert_eq!(s.provider_disabled_background, 0xffffff0a);
        assert_eq!(s.language_text, 0xdde2e8ff);
        assert_eq!(s.language_hover, 0xffffff09);
        assert_eq!(s.language_selected, 0x2c496b38);
        assert_eq!(s.impact_title, 0xa3abb5ff);
        assert_eq!(s.nav, 0x1f2227b8);
        assert_eq!(s.providers, 0x25282dc7);
        assert_eq!(s.provider_selected, 0x2c496b38);
        assert_eq!(s.provider_disabled_text, 0x6e7782ff);
        assert_eq!(s.language_list, 0x25282dc7);
        assert_eq!(s.dialog, 0x25282dfa);
        assert_eq!(s.dialog_label, 0xa8b0bbff);
        assert_eq!(s.input_text, 0xe2e6ecff);
        assert_eq!(s.input_background, 0xffffff0b);
        assert_eq!(s.nav_text, 0x9aa3ae);
        assert_eq!(s.input_focus_line, DARK.workspace.accent_line);
        assert_eq!(s.input_focus_ring, DARK.workspace.accent_soft);
    }

    #[test]
    fn recovery_water_and_database_recovery_match_frozen_colors() {
        let w = LIGHT.recovery_water;
        assert_eq!(w.tint_rgb, 0x5699db);
        assert_eq!(w.center, [0.5, 0.35]);
        assert_eq!(w.fade_end, 0.48);
        assert_eq!(w.tint_alpha, 0.055);
        assert_eq!(w.base, 0xfafcffff);
        assert_eq!(w.overlay, 0xf4f7fbeb);
        assert_eq!(w.overlay_text, 0x29364aff);
        assert_eq!(w.overlay_failed, 0xfff6f5f0);
        assert_eq!(w.panel, 0xfffffffa);
        assert_eq!(w.panel_muted, 0x617086ff);
        assert_eq!(w.panel_line, 0xc8d4e3ff);

        let w = DARK.recovery_water;
        assert_eq!(w.tint_rgb, 0x4793de);
        assert_eq!(w.center, [0.5, 0.35]);
        assert_eq!(w.fade_end, 0.48);
        assert_eq!(w.tint_alpha, 0.05);
        assert_eq!(w.base, DARK.chrome.stage);
        assert_eq!(w.overlay, 0x0d1117e0);
        assert_eq!(w.overlay_text, 0xdbe6f5ff);
        assert_eq!(w.overlay_failed, 0x190e0fe6);
        assert_eq!(w.panel, 0x181f29f5);
        assert_eq!(w.panel_muted, 0x93a2b7ff);
        assert_eq!(w.panel_line, 0x7da1d14d);

        for w in [LIGHT.recovery_water, DARK.recovery_water] {
            assert_eq!(w.inner_border, 0xb5d8ff0e);
            assert_eq!(w.inner_shadow, 0x135c9f0e);
        }
        assert_eq!(
            DARK.recovery_water.native_base,
            DARK.terminal.background << 8 | 0xff
        );
        #[cfg(target_os = "macos")]
        assert_eq!(
            LIGHT.recovery_water.native_base,
            crate::recovery_water_macos::water_base_color(Theme::LIGHT) << 8 | 0xff
        );

        let r = LIGHT.recovery_page;
        assert_eq!(r.page, 0xf4f6f9ff);
        assert_eq!(r.text, 0x20242dff);
        assert_eq!(r.card, 0xffffffff);
        assert_eq!(r.muted, 0x606a78ff);
        assert_eq!(r.accent, 0x3478f6ff);
        assert_eq!(r.error_text, 0xa12525ff);
        assert_eq!(r.error_background, 0xfff0efff);
        assert_eq!(r.success_text, 0x17653aff);
        assert_eq!(r.success_background, 0xedf9f1ff);
        assert_eq!(r.progress_text, 0x275da8ff);
        assert_eq!(r.progress_background, 0xedf4ffff);
        assert_eq!(r.card_line, 0xdce1e8ff);
        assert_eq!(DARK.recovery_page, r);
    }

    #[test]
    fn terminal_colors_reuse_the_native_palette_and_match_frozen_tokens() {
        let native = Theme::LIGHT;
        assert_eq!(LIGHT.terminal, native);
        assert_eq!(native.background, 0xFCFCFD);
        assert_eq!(native.foreground, 0x2F3547);
        assert_eq!(native.cursor, 0xFF7809);
        assert_eq!(native.cursor_accent, 0xFFFFFF);
        assert_eq!(native.selection_background, 0xbfceff);
        assert_eq!(
            native.ansi,
            [
                0x383a42, 0xe45649, 0x50a14f, 0xc18401, 0x4078f2, 0xa626a4, 0x0184bc, 0xa0a1a7,
                0x4f525e, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0x383a42,
            ]
        );

        let native = Theme::DARK;
        assert_eq!(DARK.terminal, native);
        assert_eq!(native.background, 0x1B1B1B);
        assert_eq!(native.foreground, 0xFAFAFA);
        assert_eq!(native.cursor, 0xFF7809);
        assert_eq!(native.cursor_accent, 0x0d1117);
        assert_eq!(native.selection_background, 0x264f78);
        assert_eq!(
            native.ansi,
            [
                0x484f58, 0xff7b72, 0x3fb950, 0xd29922, 0x58a6ff, 0xbc8cff, 0x39d353, 0xb1bac4,
                0x6e7681, 0xffa198, 0x56d364, 0xe3b341, 0x79c0ff, 0xd2a8ff, 0x56d364, 0xf0f6fc,
            ]
        );
    }
}
