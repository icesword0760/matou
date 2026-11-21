// The desktop probes also include the Workspace source and use the same parser.
extern crate self as matou_native_desktop;

pub mod assets;
pub mod card_context_menu;
pub mod carousel_motion;
pub mod dag_canvas;
pub mod detach_window;
pub mod dispatch_settings;
pub mod instance_open;
#[cfg(target_os = "macos")]
pub mod macos;
pub mod provider_context_editor;
mod provider_settings;
pub mod runtime_bridge;
mod settings_shell;
pub use settings_shell::claude_role_assignments;
pub mod team_member_card;
pub mod terminal_input;
mod terminal_scrollbar;
pub mod terminal_surface;
pub mod theme_tokens;
pub mod window_registry;
pub mod workspace;
pub use workspace::database_recovery_page;

pub mod tab_dialog_style;

pub mod rename_input;
pub mod sidebar_scrollbar;

pub mod workspace_drag;

pub mod ui_text;

pub mod history_view;

pub mod recovery_water;

#[cfg(target_os = "macos")]
pub mod recovery_water_macos;

pub mod egress_panel;
pub mod recovery_water_view;
pub mod shortcut_panel;
pub mod split_tree;
pub mod update_popover;
pub mod workspace_board;

#[cfg(target_os = "windows")]
pub mod windows_theme;
