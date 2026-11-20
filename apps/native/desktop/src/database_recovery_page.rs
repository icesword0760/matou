//! Native database-recovery page.
//!
//! Frozen source: the light `database-recovery-page` / `database-recovery-card`
//! view from the reference desktop app. Only the copy that this stage can act
//! on is rendered: the page header, the reason, the verified backups and the
//! single "重新检查数据库" action. The other frozen actions (restore, export,
//! start empty) stay out of this view until their runtime paths exist.
//!
//! The page owns no state and no runtime handle. It is a pure projection of
//! `DatabaseRecoveryDetails` plus the desktop operation state, so the caller can
//! swap recovery generations without leaving stale local state behind.

use gpui::{prelude::*, *};
use matou_native_core::startup::RecoveryError;
use matou_native_runtime::host::{
    DatabaseRecoveryBackup, DatabaseRecoveryDetails, DatabaseRecoveryReason,
};

use crate::ui_text::ui_text;

// Frozen zh-CN copy. Keep these strings verbatim; probes and product review
// compare them against the reference recovery page.
pub const EYEBROW: &str = "Matou 数据恢复";
pub const TITLE: &str = "数据库需要恢复";
pub const OWNERSHIP_TITLE: &str = "数据库占用状态需要处理";
pub const OWNERSHIP_DESCRIPTION: &str =
    "数据库占用记录或接管状态异常，原数据库仍保留在原位置。处理前不会将其当作损坏文件移动。";
pub const WAL_DESCRIPTION: &str =
    "数据库日志状态不完整。Matou 已保留原数据库和日志文件，请选择备份恢复或导出资料。";
pub const INTEGRITY_DESCRIPTION: &str =
    "数据库完整性检查未通过。Matou 已保留原文件，不会直接进入全新空工作区。";
pub const WORKING: &str = "正在处理数据库恢复，请保持 Matou 开启…";
pub const BACKUPS: &str = "可用备份";
pub const NO_BACKUPS: &str = "暂未找到通过完整性校验的备份。";
pub const RECHECK_DATABASE: &str = "重新检查数据库";
pub const CHECKING: &str = "正在检查…";
pub const GENERIC_ERROR: &str = "数据库恢复操作失败";

// Frozen light palette. The recovery page is deliberately light even when the
// normal workspace follows a dark terminal theme.
const PAGE_BACKGROUND: u32 = 0xf4f6f9;
const CARD_BACKGROUND: u32 = 0xffffff;
const CARD_BORDER: u32 = 0xdce1e8;
const PRIMARY_TEXT: u32 = 0x20242d;
const MUTED_TEXT: u32 = 0x606a78;
const SECONDARY_TEXT: u32 = 0x737d8b;
const COUNT_TEXT: u32 = 0x7a8492;
const ACCENT: u32 = 0x3478f6;
const ACCENT_FOCUS: u32 = 0x1d5fd0;
const ERROR_TEXT: u32 = 0xa12525;
const ERROR_BACKGROUND: u32 = 0xfff0ef;
const PROGRESS_TEXT: u32 = 0x275da8;
const PROGRESS_BACKGROUND: u32 = 0xedf4ff;
const ROW_BORDER: u32 = 0xe2e6ec;

const CARD_MAX_WIDTH: f32 = 720.;
const PAGE_PADDING_Y: f32 = 48.;
const PAGE_PADDING_X: f32 = 24.;
const CARD_RADIUS: f32 = 20.;
const CARD_PADDING: f32 = 30.;
const MAX_BACKUPS: usize = 7;

/// Render the independent recovery page.
///
/// `pending` is the shared database-recovery operation state: while it is set,
/// the only action becomes "正在检查…" and the global progress line appears.
/// `error` is the latest action error. `details.error` is the startup error and
/// intentionally renders independently, because both errors can be present at
/// the same time. The page only shows the action this stage implements.
pub fn render(
    details: &DatabaseRecoveryDetails,
    pending: bool,
    error: Option<&str>,
    enabled: bool,
    on_retry: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> Stateful<Div> {
    let (title, description) = match details.reason {
        DatabaseRecoveryReason::Ownership => (OWNERSHIP_TITLE, OWNERSHIP_DESCRIPTION),
        DatabaseRecoveryReason::Wal => (TITLE, WAL_DESCRIPTION),
        DatabaseRecoveryReason::Integrity => (TITLE, INTEGRITY_DESCRIPTION),
    };

    let backups = visible_backups(&details.backups);
    let startup_error = details
        .error
        .as_ref()
        .map(recovery_error_text)
        .filter(|text| !text.trim().is_empty());
    let action_error = error.map(|text| {
        if text.trim().is_empty() {
            GENERIC_ERROR.to_owned()
        } else {
            text.to_owned()
        }
    });
    let has_notice = startup_error.is_some() || action_error.is_some() || pending;

    let header = div()
        .id("database-recovery-header")
        .debug_selector(|| "database-recovery-header".to_owned())
        .w_full()
        .flex()
        .flex_col()
        .min_w_0()
        .child(
            div()
                .id("database-recovery-eyebrow")
                .debug_selector(|| "database-recovery-eyebrow".to_owned())
                .text_color(rgb(ACCENT))
                .text_size(px(13.))
                .font_weight(FontWeight::BOLD)
                .whitespace_nowrap()
                .child(ui_text(EYEBROW).normal_line_height()),
        )
        .child(
            div()
                .id("database-recovery-title")
                .debug_selector(|| "database-recovery-title".to_owned())
                .mt(px(4.))
                .text_color(rgb(PRIMARY_TEXT))
                .text_size(px(26.))
                .font_weight(FontWeight::BOLD)
                .line_height(relative(1.2))
                .whitespace_normal()
                .child(ui_text(title)),
        )
        .child(
            div()
                .id("database-recovery-description")
                .debug_selector(|| "database-recovery-description".to_owned())
                .mt(px(10.))
                .min_w_0()
                .text_color(rgb(MUTED_TEXT))
                .text_size(px(16.))
                .line_height(relative(1.6))
                .whitespace_normal()
                .child(ui_text(description)),
        );

    let mut card = div()
        .id("database-recovery-card")
        .debug_selector(|| "database-recovery-card".to_owned())
        .w_full()
        .max_w(px(CARD_MAX_WIDTH))
        .min_w_0()
        .flex()
        .flex_col()
        .p(px(CARD_PADDING))
        .border_1()
        .border_color(rgb(CARD_BORDER))
        .rounded(px(CARD_RADIUS))
        .bg(rgb(CARD_BACKGROUND))
        .shadow(vec![shadow(0x192231, 0.10, 24., 70.)])
        .text_color(rgb(PRIMARY_TEXT))
        .child(header);

    if has_notice {
        let mut notices = div()
            .id("database-recovery-notices")
            .debug_selector(|| "database-recovery-notices".to_owned())
            .mt(px(16.))
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(8.));
        if let Some(startup_error) = startup_error {
            notices = notices.child(notice(
                "database-recovery-startup-error",
                startup_error,
                ERROR_TEXT,
                ERROR_BACKGROUND,
            ));
        }
        if let Some(action_error) = action_error {
            notices = notices.child(notice(
                "database-recovery-action-error",
                action_error,
                ERROR_TEXT,
                ERROR_BACKGROUND,
            ));
        }
        if pending {
            notices = notices.child(notice(
                "database-recovery-progress",
                WORKING.to_owned(),
                PROGRESS_TEXT,
                PROGRESS_BACKGROUND,
            ));
        }
        card = card.child(notices);
    }

    let mut backups_section = div()
        .id("database-recovery-backups")
        .debug_selector(|| "database-recovery-backups".to_owned())
        .mt(px(26.))
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .child(
            div()
                .id("database-recovery-backups-heading")
                .debug_selector(|| "database-recovery-backups-heading".to_owned())
                .w_full()
                .min_w_0()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .child(
                    div()
                        .id("database-recovery-backups-title")
                        .debug_selector(|| "database-recovery-backups-title".to_owned())
                        .flex_none()
                        .text_color(rgb(PRIMARY_TEXT))
                        .text_size(px(15.))
                        .font_weight(FontWeight::BOLD)
                        .whitespace_nowrap()
                        .child(ui_text(BACKUPS).normal_line_height()),
                )
                .child(
                    div()
                        .id("database-recovery-backup-count")
                        .debug_selector(|| "database-recovery-backup-count".to_owned())
                        .flex_none()
                        .text_color(rgb(COUNT_TEXT))
                        .text_size(px(13.))
                        .whitespace_nowrap()
                        .child(ui_text(format!("{} 份", backups.len())).normal_line_height()),
                ),
        );

    if backups.is_empty() {
        backups_section = backups_section.child(
            div()
                .id("database-recovery-empty")
                .debug_selector(|| "database-recovery-empty".to_owned())
                .mt(px(10.))
                .min_w_0()
                .text_color(rgb(SECONDARY_TEXT))
                .text_size(px(16.))
                .line_height(relative(1.5))
                .whitespace_normal()
                .child(ui_text(NO_BACKUPS)),
        );
    } else {
        let mut list = div()
            .id("database-recovery-backup-list")
            .debug_selector(|| "database-recovery-backup-list".to_owned())
            .mt(px(10.))
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(8.));
        for (index, backup) in backups.iter().enumerate() {
            list = list.child(backup_row(index, backup));
        }
        backups_section = backups_section.child(list);
    }
    card = card.child(backups_section);

    let action_enabled = enabled && !pending;
    let retry_label = if pending { CHECKING } else { RECHECK_DATABASE };
    let retry = div()
        .id("database-recovery-retry")
        .debug_selector(|| "database-recovery-retry".to_owned())
        .flex_none()
        .h(px(38.))
        .px(px(14.))
        .flex()
        .items_center()
        .justify_center()
        .border_1()
        .border_color(rgb(ACCENT))
        .rounded(px(9.))
        .bg(rgb(ACCENT))
        .text_color(rgb(0xffffff))
        .text_size(px(16.))
        .font_weight(FontWeight::SEMIBOLD)
        .whitespace_nowrap()
        .child(
            div()
                .id("database-recovery-retry-label")
                .debug_selector(|| "database-recovery-retry-label".to_owned())
                .child(ui_text(retry_label).normal_line_height()),
        );
    let retry = if action_enabled {
        retry
            .tab_index(0)
            .cursor_pointer()
            .focus(|style| style.border_color(rgb(ACCENT_FOCUS)))
            .on_click(move |event, window, cx| on_retry(event, window, cx))
    } else {
        retry.opacity(0.48).cursor(CursorStyle::Arrow)
    };
    card = card.child(
        div()
            .id("database-recovery-primary-actions")
            .debug_selector(|| "database-recovery-primary-actions".to_owned())
            .mt(px(22.))
            .w_full()
            .min_w_0()
            .flex()
            .flex_wrap()
            .gap(px(10.))
            .child(retry),
    );

    div()
        .id("database-recovery-page")
        .size_full()
        .overflow_y_scroll()
        .bg(rgb(PAGE_BACKGROUND))
        .text_color(rgb(PRIMARY_TEXT))
        .child(
            div()
                .id("database-recovery-centering")
                .debug_selector(|| "database-recovery-centering".to_owned())
                .w_full()
                .min_h(relative(1.))
                .px(px(PAGE_PADDING_X))
                .py(px(PAGE_PADDING_Y))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .child(card),
        )
}

/// The frozen page sorts by time descending, then ID descending, and keeps the
/// newest seven verified backups. Sorting lives here as well as in the backup
/// service so a reordered or reconnected projection cannot change the list.
pub fn visible_backups(backups: &[DatabaseRecoveryBackup]) -> Vec<&DatabaseRecoveryBackup> {
    let mut visible: Vec<_> = backups.iter().collect();
    visible.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| right.id.cmp(&left.id))
    });
    visible.truncate(MAX_BACKUPS);
    visible
}

/// Locale-style `YYYY/MM/DD HH:mm` in the system's local zone.
pub fn format_backup_time(created_at_ms: i64) -> String {
    let seconds = created_at_ms.div_euclid(1_000);
    if let Some((year, month, day, hour, minute)) = local_parts(seconds) {
        return format!("{year:04}/{month:02}/{day:02} {hour:02}:{minute:02}");
    }
    let days = seconds.div_euclid(86_400);
    let day_seconds = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}/{month:02}/{day:02} {:02}:{:02}",
        day_seconds / 3_600,
        day_seconds % 3_600 / 60
    )
}

/// Frozen size format: integer KB below 1 MiB (minimum 1), otherwise one
/// decimal MB.
pub fn format_backup_size(size: u64) -> String {
    if size < 1024 * 1024 {
        format!("{} KB", (size.saturating_add(512) / 1024).max(1))
    } else {
        format!("{:.1} MB", size as f64 / (1024. * 1024.))
    }
}

fn notice(id: &'static str, text: String, text_color: u32, background: u32) -> Stateful<Div> {
    div()
        .id(id)
        .debug_selector(move || id.to_owned())
        .w_full()
        .min_w_0()
        .px(px(12.))
        .py(px(10.))
        .rounded(px(10.))
        .bg(rgb(background))
        .text_color(rgb(text_color))
        .text_size(px(16.))
        .line_height(relative(1.5))
        .whitespace_normal()
        .child(ui_text(text))
}

fn backup_row(index: usize, backup: &DatabaseRecoveryBackup) -> Stateful<Div> {
    let metadata = format!(
        "{} · 数据版本 {} · {}",
        format_backup_time(backup.created_at),
        backup.schema_version,
        format_backup_size(backup.size)
    );
    div()
        .id(SharedString::from(format!(
            "database-recovery-backup-{index}"
        )))
        .debug_selector(move || format!("database-recovery-backup-{index}"))
        .w_full()
        .min_w_0()
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(12.))
        .py(px(12.))
        .border_1()
        .border_color(rgb(ROW_BORDER))
        .rounded(px(11.))
        .bg(rgb(CARD_BACKGROUND))
        .child(
            div()
                .min_w_0()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .id(SharedString::from(format!(
                            "database-recovery-backup-id-{index}"
                        )))
                        .debug_selector(move || format!("database-recovery-backup-id-{index}"))
                        .w_full()
                        .min_w_0()
                        .text_color(rgb(PRIMARY_TEXT))
                        .text_size(px(16.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .line_height(relative(1.4))
                        .whitespace_normal()
                        .child(ui_text(backup.id.clone())),
                )
                .child(
                    div()
                        .id(SharedString::from(format!(
                            "database-recovery-backup-meta-{index}"
                        )))
                        .debug_selector(move || format!("database-recovery-backup-meta-{index}"))
                        .w_full()
                        .min_w_0()
                        .text_color(rgb(SECONDARY_TEXT))
                        .text_size(px(13.))
                        .line_height(relative(1.4))
                        .whitespace_normal()
                        .child(ui_text(metadata)),
                ),
        )
}

fn recovery_error_text(error: &RecoveryError) -> String {
    if error.message.trim().is_empty() {
        GENERIC_ERROR.to_owned()
    } else {
        error.message.clone()
    }
}

fn shadow(color: u32, alpha: f32, y: f32, blur: f32) -> BoxShadow {
    let mut color: Hsla = rgb(color).into();
    color.a = alpha;
    BoxShadow {
        color,
        offset: point(px(0.), px(y)),
        blur_radius: px(blur),
        spread_radius: px(0.),
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn local_parts(seconds: i64) -> Option<(i32, u32, u32, u32, u32)> {
    // The system's local zone, including the offset at the backup's time.
    // Mirrors the notification clock already used by the native workspace.
    #[repr(C)]
    #[derive(Default)]
    struct LocalTime {
        second: i32,
        minute: i32,
        hour: i32,
        day: i32,
        month: i32,
        year: i32,
        weekday: i32,
        year_day: i32,
        is_dst: i32,
        #[cfg(target_os = "macos")]
        utc_offset: std::ffi::c_long,
        #[cfg(target_os = "macos")]
        timezone: *const std::ffi::c_char,
    }
    #[cfg(target_os = "macos")]
    unsafe extern "C" {
        fn localtime_r(seconds: *const i64, output: *mut LocalTime) -> *mut LocalTime;
    }
    #[cfg(target_os = "windows")]
    unsafe extern "C" {
        fn _localtime64_s(output: *mut LocalTime, seconds: *const i64) -> i32;
    }

    let mut local = LocalTime::default();
    // Each function fills the caller-owned C tm for this platform.
    #[cfg(target_os = "macos")]
    let valid = unsafe { !localtime_r(&seconds, &mut local).is_null() };
    #[cfg(target_os = "windows")]
    let valid = unsafe { _localtime64_s(&mut local, &seconds) == 0 };
    valid.then(|| {
        (
            local.year + 1900,
            (local.month + 1) as u32,
            local.day as u32,
            local.hour as u32,
            local.minute as u32,
        )
    })
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn local_parts(_seconds: i64) -> Option<(i32, u32, u32, u32, u32)> {
    None
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    // Howard Hinnant's civil-from-days algorithm. Used only when the platform
    // local clock is unavailable, so the timestamp is still readable.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month as u32, day as u32)
}
