//! Native database-recovery page.
//!
//! Frozen source: the light `database-recovery-page` / `database-recovery-card`
//! view from the reference desktop app. The page renders the reason, the
//! verified backups, the three ordinary actions and the separate danger entry
//! at the bottom. The empty-database confirmation is a focused, trapped modal
//! inside this page.
//!
//! The page owns no state and no runtime handle. It is a pure projection of
//! `DatabaseRecoveryDetails` plus the desktop operation state, so the caller can
//! swap recovery generations without leaving stale local state behind. Focus
//! handles and the confirmation visibility stay in the workspace; the page only
//! renders them and reports the requested action.

use std::path::Path;
use std::rc::Rc;

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
pub const RESTORING: &str = "正在恢复…";
pub const RESTORE_SELECTED: &str = "恢复所选备份";
pub const RECHECK_DATABASE: &str = "重新检查数据库";
pub const CHECKING: &str = "正在检查…";
pub const EXPORT_BUNDLE: &str = "导出恢复资料";
pub const START_EMPTY: &str = "创建全新空数据库";
pub const START_EMPTY_HINT: &str = "此入口只在你明确确认后执行；现有隔离文件和备份继续保留。";
pub const CONFIRM_EMPTY_LABEL: &str = "确认创建全新空数据库";
pub const CONFIRM_EMPTY_TITLE: &str = "确认创建全新空数据库？";
pub const CONFIRM_EMPTY_BODY: &str =
    "Matou 将显示一个全新的空工作区。当前损坏或异常文件和备份仍会保留，便于后续导出与排查。";
pub const BACK: &str = "返回";
pub const CONFIRM_EMPTY: &str = "确认创建空数据库";
pub const INVALID_MARKER_DESCRIPTION: &str = "Matou 已保留原数据库和恢复记录，会自动重新检查。";
pub const INVALID_MARKER_ERROR: &str =
    "数据库恢复记录无效。当前无法验证恢复状态，暂不能恢复备份、导出恢复资料或创建空数据库。";
pub const GENERIC_ERROR: &str = "数据库恢复操作失败";
const EXPORTED_TO: &str = "恢复资料已导出到 ";

/// Frozen headline of the read-only workspace banner. The workspace also uses
/// it verbatim as the disabled reason of blocked mutations.
pub const READ_ONLY_TITLE: &str = "数据库处于只读恢复模式";
const READ_ONLY_EXPLANATION: &str =
    "现有工作空间、事项和会话仍可浏览、搜索与复制；可能改动数据的操作已暂停。";
const SEARCH_TERMINAL: &str = "搜索当前终端";
const EXPORT_DATABASE: &str = "导出数据库资料";
const EXPORTING: &str = "正在导出…";
const EXPORTED: &str = "数据库资料已导出";
const EXPORT_FAILED: &str = "导出失败：";

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
const SUCCESS_TEXT: u32 = 0x17653a;
const SUCCESS_BACKGROUND: u32 = 0xedf9f1;
const ROW_BORDER: u32 = 0xe2e6ec;
const SELECTED_BORDER: u32 = 0x78a5ff;
const SELECTED_BACKGROUND: u32 = 0xf2f6ff;
const BUTTON_BORDER: u32 = 0xd4d9e1;
const DANGER_TEXT: u32 = 0xad3434;
const DANGER_FOCUS: u32 = 0x8f2626;
const DANGER_BUTTON: u32 = 0xb63838;
const FOOTER_BORDER: u32 = 0xeceff3;
const RADIO_BORDER: u32 = 0xc6ccd6;
const DIALOG_BACKDROP: u32 = 0x161b2361;

// Frozen roles from `read-only-recovery.css`. The background is an sRGB mix of
// the workspace terminal background with amber; the bottom rule is the darker
// amber at 42% over that mixed background. The title and error colors are
// fixed, exactly as in the stylesheet.
const BANNER_AMBER_COLOR: u32 = 0xf4b740;
const BANNER_HEADLINE_COLOR: u32 = 0x966300;
const BANNER_BORDER_COLOR: u32 = 0xc88708;
const BANNER_ERROR_COLOR: u32 = 0xb42318;
const BANNER_TINT: u32 = 17;
const BANNER_BORDER_TINT: u32 = 42;

const CARD_MAX_WIDTH: f32 = 720.;
const PAGE_PADDING_Y: f32 = 48.;
const PAGE_PADDING_X: f32 = 24.;
const CARD_RADIUS: f32 = 20.;
const CARD_PADDING: f32 = 30.;
const MAX_BACKUPS: usize = 7;

/// The frozen page renders in Chromium with `line-height: normal`; the fixed
/// Mac UI font cascade (SF Pro / PingFang SC) measures 1.4em for it. The
/// snapshot shows that leading in the title, empty state, dialog and footer.
/// Keeping it as a factor instead of a fixed card height means the card still
/// grows with its content and can scroll.
const NORMAL_LINE_HEIGHT: f32 = 1.4;
/// `h2 { margin: 0.83em 0 }` at the frozen 15pt section title. Chromium does
/// not collapse margins between the flex row and its `h2` item, so the row
/// grows by both margins. Applied as padding so the count label stays centered.
const SECTION_TITLE_MARGIN_Y: f32 = 15. * 0.83;
/// `.database-recovery-empty` inherits the UA paragraph `margin-block-start`.
const EMPTY_STATE_MARGIN_TOP: f32 = 16.;
/// `.database-recovery-card button:disabled { opacity: .48 }`.
const DISABLED_OPACITY: f32 = 0.48;
/// UA dialog button face and border on the frozen macOS build.
const DIALOG_BUTTON_BACKGROUND: u32 = 0xefefef;
const DIALOG_BUTTON_BORDER: u32 = 0x767676;
const DIALOG_BUTTON_TEXT: u32 = 0x000000;
const BUTTON_FONT_SIZE: f32 = 16.;

/// One of the four database-recovery actions exposed by the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageOperation {
    Restore,
    Retry,
    Export,
    StartEmpty,
}

/// The action requested by a click or a keyboard activation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PageAction {
    SelectBackup(String),
    Restore,
    Retry,
    Export,
    OpenConfirmation,
    CancelConfirmation,
    ConfirmEmpty,
}

/// Focus handles owned by the workspace for the empty-database confirmation.
pub struct ConfirmationFocus<'a> {
    pub back: &'a FocusHandle,
    pub confirm: &'a FocusHandle,
}

/// Everything the workspace projects into the recovery page.
pub struct PagePresentation<'a> {
    pub locale: matou_native_core::sessions::InterfaceLocale,
    pub available: bool,
    pub pending: Option<PageOperation>,
    pub selected_backup: Option<&'a str>,
    pub error: Option<&'a str>,
    pub exported_path: Option<&'a Path>,
    pub empty_trigger: Option<&'a FocusHandle>,
    pub confirmation: Option<ConfirmationFocus<'a>>,
    /// The actual window client height in points, from
    /// `Window::viewport_size().height`.
    ///
    /// The frozen page uses `min-height: 100vh`; a percentage minimum cannot
    /// resolve below GPUI's scroll container, so the caller must project the
    /// real viewport height here. Long cards still scroll because this is a
    /// minimum, not a fixed height.
    pub viewport_height: Pixels,
}

type PageActionHandler = Rc<dyn Fn(&PageAction, &mut Window, &mut App)>;

#[cfg(debug_assertions)]
#[derive(Default)]
pub(crate) struct PageProbe {
    pub(crate) root: Option<ElementId>,
    pub(crate) bounds: std::collections::BTreeMap<&'static str, Vec<Bounds<Pixels>>>,
    pub(crate) buttons: std::collections::BTreeMap<&'static str, (bool, ButtonSpec)>,
}

#[cfg(debug_assertions)]
pub(super) fn probe_root(root: &Stateful<Div>) {
    PAGE_PROBE.with_borrow_mut(|probe| {
        if let Some(probe) = probe {
            probe.root = Element::id(root);
        }
    });
}

#[cfg(debug_assertions)]
thread_local! {
    static PAGE_PROBE: std::cell::RefCell<Option<PageProbe>> = const { std::cell::RefCell::new(None) };
}

#[cfg(debug_assertions)]
#[allow(dead_code)]
pub(crate) fn probe_draw<T>(draw: impl FnOnce() -> T) -> (T, PageProbe) {
    PAGE_PROBE.set(Some(PageProbe::default()));
    let result = draw();
    (result, PAGE_PROBE.take().unwrap())
}

fn probe_children(parent: Div, name: &'static str) -> Div {
    #[cfg(debug_assertions)]
    if PAGE_PROBE.with_borrow(|probe| probe.is_some()) {
        return parent.on_children_prepainted(move |children, _, _| {
            PAGE_PROBE.with_borrow_mut(|probe| {
                if let Some(probe) = probe {
                    probe.bounds.insert(name, children);
                }
            });
        });
    }
    let _ = name;
    parent
}

/// Render the independent recovery page.
///
/// `pending` identifies the active action: only that button changes its label,
/// every other control becomes non-interactive and the global blue progress
/// notice appears. `error` is the latest action error; `details.error` is the
/// startup error and renders independently, because both can be present at the
/// same time. `exported_path` drives the frozen green success copy.
///
/// The workspace owns the recovery ID, the operation state, the focus handles
/// and whether `confirmation` is open. The page only reports the requested
/// action; it never confirms the empty-database choice by itself.
pub fn render(
    details: Option<&DatabaseRecoveryDetails>,
    presentation: PagePresentation<'_>,
    on_action: impl Fn(&PageAction, &mut Window, &mut App) + 'static,
) -> Stateful<Div> {
    let PagePresentation {
        locale,
        available,
        pending,
        selected_backup,
        error,
        exported_path,
        empty_trigger,
        confirmation,
        viewport_height,
    } = presentation;

    let on_action: PageActionHandler = Rc::new(on_action);
    let (title, description) = match details.map(|details| details.reason) {
        Some(DatabaseRecoveryReason::Ownership) => (locale.text(OWNERSHIP_TITLE, "The database ownership state needs attention"), locale.text(OWNERSHIP_DESCRIPTION, "The database ownership record or handover state is inconsistent. The original database is still in place and will not be moved aside as a damaged file before you act.")),
        Some(DatabaseRecoveryReason::Wal) => (locale.text(TITLE, "The database needs recovery"), locale.text(WAL_DESCRIPTION, "The database log is incomplete. Matou kept the original database and log files, so you can restore a backup or export your data.")),
        Some(DatabaseRecoveryReason::Integrity) => (locale.text(TITLE, "The database needs recovery"), locale.text(INTEGRITY_DESCRIPTION, "The database failed its integrity check. Matou kept the original files and will not start a blank workspace on its own.")),
        None => (locale.text(TITLE, "The database needs recovery"), INVALID_MARKER_DESCRIPTION),
    };
    let backups = details.map(|details| visible_backups(&details.backups));
    let startup_error = match details {
        Some(details) => details
            .error
            .as_ref()
            .map(recovery_error_text)
            .filter(|text| !text.trim().is_empty()),
        None => Some(INVALID_MARKER_ERROR.to_owned()),
    };
    let action_error = error.map(|text| {
        if text.trim().is_empty() {
            GENERIC_ERROR.to_owned()
        } else {
            text.to_owned()
        }
    });
    let success = exported_path.map(|path| {
        format!(
            "{}{}",
            locale.text(EXPORTED_TO, "Recovery bundle exported to "),
            path.display()
        )
    });
    let confirm_enabled = available && pending.is_none() && details.is_some();
    let can_act = confirm_enabled && confirmation.is_none();
    let can_open = can_act && details.is_some_and(|details| !details.recovery_id.is_empty());
    let selected_visible = selected_backup.is_some_and(|selected| {
        backups
            .as_ref()
            .is_some_and(|backups| backups.iter().any(|backup| backup.id == selected))
    });
    let can_restore = can_open && selected_visible;
    let has_notice =
        startup_error.is_some() || action_error.is_some() || success.is_some() || pending.is_some();

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
                .child(ui_text(locale.text(EYEBROW, "Matou data recovery")).normal_line_height()),
        )
        .child(
            div()
                .id("database-recovery-title")
                .debug_selector(|| "database-recovery-title".to_owned())
                .mt(px(4.))
                .text_color(rgb(PRIMARY_TEXT))
                .text_size(px(26.))
                .font_weight(FontWeight::BOLD)
                .line_height(relative(NORMAL_LINE_HEIGHT))
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

    let mut status = div()
        .id("database-recovery-status")
        .debug_selector(|| "database-recovery-status".to_owned())
        .w_full()
        .min_w_0();
    if has_notice {
        let mut notices = div()
            .id("database-recovery-notices")
            .debug_selector(|| "database-recovery-notices".to_owned())
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .gap(px(16.));
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
        if let Some(success) = success {
            notices = notices.child(notice(
                "database-recovery-success",
                success,
                SUCCESS_TEXT,
                SUCCESS_BACKGROUND,
            ));
        }
        if pending.is_some() {
            notices = notices.child(notice(
                "database-recovery-progress",
                locale
                    .text(WORKING, "Recovering the database. Keep Matou open…")
                    .to_owned(),
                PROGRESS_TEXT,
                PROGRESS_BACKGROUND,
            ));
        }
        status = status.mt(px(16.)).child(notices);
    }
    card = card.child(status);

    if let Some(backups) = backups {
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
                    .py(px(SECTION_TITLE_MARGIN_Y))
                    .child(
                        div()
                            .id("database-recovery-backups-title")
                            .debug_selector(|| "database-recovery-backups-title".to_owned())
                            .flex_none()
                            .text_color(rgb(PRIMARY_TEXT))
                            .text_size(px(15.))
                            .font_weight(FontWeight::BOLD)
                            .whitespace_nowrap()
                            .child(
                                ui_text(locale.text(BACKUPS, "Available backups"))
                                    .normal_line_height(),
                            ),
                    )
                    .child(
                        div()
                            .id("database-recovery-backup-count")
                            .debug_selector(|| "database-recovery-backup-count".to_owned())
                            .flex_none()
                            .text_color(rgb(COUNT_TEXT))
                            .text_size(px(13.))
                            .whitespace_nowrap()
                            .child(
                                ui_text(format!("{}{}", backups.len(), locale.text(" 份", "")))
                                    .normal_line_height(),
                            ),
                    ),
            );

        if backups.is_empty() {
            backups_section = backups_section.child(
                div()
                    .id("database-recovery-empty")
                    .debug_selector(|| "database-recovery-empty".to_owned())
                    .mt(px(EMPTY_STATE_MARGIN_TOP))
                    .min_w_0()
                    .text_color(rgb(SECONDARY_TEXT))
                    .text_size(px(16.))
                    .line_height(relative(NORMAL_LINE_HEIGHT))
                    .whitespace_normal()
                    .child(ui_text(locale.text(
                        NO_BACKUPS,
                        "No backup has passed the integrity check yet.",
                    ))),
            );
        } else {
            let mut list = div()
                .id("database-recovery-backup-list")
                .debug_selector(|| "database-recovery-backup-list".to_owned())
                .w_full()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(8.));
            for (index, backup) in backups.iter().enumerate() {
                list = list.child(backup_row(
                    index,
                    backup,
                    selected_backup == Some(backup.id.as_str()),
                    can_act,
                    locale,
                    on_action.clone(),
                ));
            }
            backups_section = backups_section.child(list);
        }
        card = card.child(backups_section);
    }

    let restore_label = if pending == Some(PageOperation::Restore) {
        locale.text(RESTORING, "Restoring…")
    } else {
        locale.text(RESTORE_SELECTED, "Restore backup")
    };
    let retry_label = if pending == Some(PageOperation::Retry) {
        locale.text(CHECKING, "Checking…")
    } else {
        locale.text(RECHECK_DATABASE, "Check database again")
    };
    let export_label = if pending == Some(PageOperation::Export) {
        locale.text(EXPORTING, "Exporting…")
    } else {
        locale.text(EXPORT_BUNDLE, "Export recovery bundle")
    };

    let restore = action_button(
        "database-recovery-restore",
        "database-recovery-restore",
        restore_label,
        can_restore,
        ButtonStyle::Primary,
        PageAction::Restore,
        None,
        on_action.clone(),
    );
    let retry = action_button(
        "database-recovery-retry",
        "database-recovery-retry",
        retry_label,
        can_open,
        ButtonStyle::Secondary,
        PageAction::Retry,
        None,
        on_action.clone(),
    );
    let export = action_button(
        "database-recovery-export",
        "database-recovery-export",
        export_label,
        can_act,
        ButtonStyle::Secondary,
        PageAction::Export,
        None,
        on_action.clone(),
    );
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
            .child(restore)
            .child(retry)
            .child(export),
    );

    let start_empty = action_button(
        "database-recovery-start-empty",
        "database-recovery-start-empty",
        locale.text(START_EMPTY, "Create new empty database"),
        can_open,
        ButtonStyle::DangerLink,
        PageAction::OpenConfirmation,
        empty_trigger,
        on_action.clone(),
    );
    card = card.child(
        div()
            .id("database-recovery-footer")
            .debug_selector(|| "database-recovery-footer".to_owned())
            .mt(px(24.))
            .pt(px(18.))
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .items_start()
            .border_t_1()
            .border_color(rgb(FOOTER_BORDER))
            .child(start_empty)
            .child(
                div()
                    .id("database-recovery-start-empty-hint")
                    .debug_selector(|| "database-recovery-start-empty-hint".to_owned())
                    .mt(px(7.))
                    .w_full()
                    .min_w_0()
                    .text_color(rgb(SECONDARY_TEXT))
                    .text_size(px(12.))
                    .line_height(relative(NORMAL_LINE_HEIGHT))
                    .whitespace_normal()
                    .child(ui_text(locale.text(START_EMPTY_HINT, "This runs only after you confirm it explicitly. The quarantined files and the backups are kept."))),
            ),
    );

    let mut page = div()
        .id("database-recovery-page")
        .debug_selector(|| "database-recovery-page".to_owned())
        .relative()
        .size_full()
        .bg(rgb(PAGE_BACKGROUND))
        .text_color(rgb(PRIMARY_TEXT))
        .child(
            div()
                .id("database-recovery-scroll")
                .debug_selector(|| "database-recovery-scroll".to_owned())
                .size_full()
                .overflow_y_scroll()
                .child(
                    probe_children(div(), "recovery-card")
                        .id("database-recovery-centering")
                        .debug_selector(|| "database-recovery-centering".to_owned())
                        .w_full()
                        .min_h(viewport_height)
                        .px(px(PAGE_PADDING_X))
                        .py(px(PAGE_PADDING_Y))
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .child(card),
                ),
        );
    if let Some(confirmation) = confirmation {
        page = page.child(empty_confirmation_dialog(
            confirmation.back,
            confirmation.confirm,
            confirm_enabled,
            locale,
            on_action,
        ));
    }
    page
}

/// Render the error and disabled actions without a trusted recovery generation.
pub fn render_invalid_marker(
    viewport_height: Pixels,
    locale: matou_native_core::sessions::InterfaceLocale,
) -> Stateful<Div> {
    render(
        None,
        PagePresentation {
            locale,
            available: false,
            pending: None,
            selected_backup: None,
            error: None,
            exported_path: None,
            empty_trigger: None,
            confirmation: None,
            viewport_height,
        },
        |_, _, _| {},
    )
}

/// The two button families in the frozen page. Card buttons are 38pt tall with
/// 9pt corners; the dialog lives outside the card, so its buttons use the
/// browser's own 28pt, square, 1pt-border controls. Keeping both here means the
/// page has one definition of every production button style.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ButtonStyle {
    Primary,
    Secondary,
    DangerLink,
    DialogBack,
    DialogDanger,
}

/// Resolved, testable production button parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ButtonSpec {
    height: Option<f32>,
    padding_x: f32,
    pub(crate) border_width: f32,
    pub(crate) radius: f32,
    pub(crate) background: Option<u32>,
    pub(crate) border: Option<u32>,
    pub(crate) text: u32,
    font_size: f32,
    pub(crate) font_weight: FontWeight,
    focus_border: Option<u32>,
    focus_text: Option<u32>,
    focus_underline: bool,
}

impl ButtonStyle {
    /// Resolve the frozen CSS for this button in its current state.
    ///
    /// Every disabled color is composited against the fixed white card/dialog
    /// surface with the same .48 factor as `button:disabled { opacity: .48 }`.
    /// Opaque white labels therefore stay white, and a same-colored fill and
    /// border stay identical instead of the border darkening over the fill.
    fn spec(self, enabled: bool) -> ButtonSpec {
        let spec = match self {
            ButtonStyle::Primary => ButtonSpec {
                height: Some(38.),
                padding_x: 14.,
                border_width: 1.,
                radius: 9.,
                background: Some(ACCENT),
                border: Some(ACCENT),
                text: 0xffffff,
                font_size: BUTTON_FONT_SIZE,
                font_weight: FontWeight::NORMAL,
                focus_border: Some(ACCENT_FOCUS),
                focus_text: None,
                focus_underline: false,
            },
            ButtonStyle::Secondary => ButtonSpec {
                height: Some(38.),
                padding_x: 14.,
                border_width: 1.,
                radius: 9.,
                background: Some(CARD_BACKGROUND),
                border: Some(BUTTON_BORDER),
                text: PRIMARY_TEXT,
                font_size: BUTTON_FONT_SIZE,
                font_weight: FontWeight::NORMAL,
                focus_border: Some(ACCENT),
                focus_text: None,
                focus_underline: false,
            },
            ButtonStyle::DangerLink => ButtonSpec {
                height: None,
                padding_x: 0.,
                border_width: 0.,
                radius: 0.,
                background: None,
                border: None,
                text: DANGER_TEXT,
                font_size: BUTTON_FONT_SIZE,
                font_weight: FontWeight::NORMAL,
                focus_border: None,
                focus_text: Some(DANGER_FOCUS),
                focus_underline: true,
            },
            ButtonStyle::DialogBack => ButtonSpec {
                height: Some(28.),
                padding_x: 7.,
                border_width: 1.,
                radius: 0.,
                background: Some(DIALOG_BUTTON_BACKGROUND),
                border: Some(DIALOG_BUTTON_BORDER),
                text: DIALOG_BUTTON_TEXT,
                font_size: BUTTON_FONT_SIZE,
                font_weight: FontWeight::NORMAL,
                focus_border: Some(ACCENT),
                focus_text: None,
                focus_underline: false,
            },
            ButtonStyle::DialogDanger => ButtonSpec {
                height: Some(28.),
                padding_x: 7.,
                border_width: 1.,
                radius: 0.,
                background: Some(DANGER_BUTTON),
                border: Some(DANGER_BUTTON),
                text: 0xffffff,
                font_size: BUTTON_FONT_SIZE,
                font_weight: FontWeight::NORMAL,
                focus_border: Some(DANGER_FOCUS),
                focus_text: None,
                focus_underline: false,
            },
        };
        if enabled {
            return spec;
        }
        // Render all three layers pre-composited against the white card. This
        // is the frozen `opacity: .48` group result without GPUI's
        // per-primitive fading, which would otherwise darken the border over
        // the already-faded fill and turn white labels light blue.
        let fade = |color: u32| {
            mix_color(
                color,
                CARD_BACKGROUND,
                ((1. - DISABLED_OPACITY) * 100.).round() as u32,
            )
        };
        ButtonSpec {
            background: spec.background.map(fade),
            border: spec.border.map(fade),
            text: fade(spec.text),
            ..spec
        }
    }
}

fn action_button(
    id: &'static str,
    selector: &'static str,
    label: &'static str,
    enabled: bool,
    style: ButtonStyle,
    action: PageAction,
    focus: Option<&FocusHandle>,
    on_action: PageActionHandler,
) -> Stateful<Div> {
    let label_selector = format!("{selector}-label");
    let spec = style.spec(enabled);
    #[cfg(debug_assertions)]
    PAGE_PROBE.with_borrow_mut(|probe| {
        if let Some(probe) = probe {
            probe.buttons.insert(id, (enabled, spec));
        }
    });
    let mut button = div()
        .id(id)
        .debug_selector(move || selector.to_owned())
        .relative()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(spec.font_size))
        .font_weight(spec.font_weight)
        .text_color(rgb(spec.text))
        .whitespace_nowrap()
        .px(px(spec.padding_x));
    if let Some(height) = spec.height {
        button = button.h(px(height));
    }
    if spec.border_width > 0. {
        button = button
            .border(px(spec.border_width))
            .rounded(px(spec.radius));
        if let Some(border) = spec.border {
            button = button.border_color(rgb(border));
        }
    }
    if let Some(background) = spec.background {
        button = button.bg(rgb(background));
    }
    button = button
        .child(
            div()
                .id(SharedString::from(label_selector.clone()))
                .debug_selector(move || label_selector)
                .child(ui_text(label).normal_line_height()),
        )
        .child(action_state_marker(selector, enabled));
    if !enabled {
        return button.cursor(CursorStyle::Arrow);
    }
    let button = match focus {
        Some(focus) => {
            let focus = focus.clone().tab_stop(true);
            button.track_focus(&focus)
        }
        None => button,
    };

    let key_action = action.clone();
    let click_action = action;
    let key_handler = on_action.clone();
    let click_handler = on_action;
    button
        .tab_index(0)
        .cursor_pointer()
        .focus(move |style| {
            let style = match spec.focus_border {
                Some(color) => style.border_color(rgb(color)),
                None => style,
            };
            let style = match spec.focus_text {
                Some(color) => style.text_color(rgb(color)),
                None => style,
            };
            if spec.focus_underline {
                style.underline()
            } else {
                style
            }
        })
        .on_key_down(move |event, window, cx| {
            if is_activation_key(&event.keystroke) {
                cx.stop_propagation();
                key_handler(&key_action, window, cx);
            }
        })
        .on_click(move |event, window, cx| {
            // GPUI also synthesizes a keyboard click on key-up. The key-down
            // handler already dispatched the action, so ignore that copy.
            if matches!(event, ClickEvent::Keyboard(_)) {
                return;
            }
            cx.stop_propagation();
            click_handler(&click_action, window, cx);
        })
}

/// A zero-size selector kept inside each button so a hidden-window probe can
/// tell enabled and disabled controls apart without reading the presentation.
fn action_state_marker(selector: &'static str, enabled: bool) -> AnyElement {
    let marker = format!(
        "{selector}-{}",
        if enabled { "enabled" } else { "disabled" }
    );
    div()
        .flex_none()
        .w(px(0.))
        .h(px(0.))
        .debug_selector(move || marker)
        .into_any_element()
}

fn is_activation_key(keystroke: &Keystroke) -> bool {
    matches!(keystroke.key.as_str(), "enter" | "space") && !keystroke.modifiers.modified()
}

impl ConfirmationFocus<'_> {
    pub(super) fn handle_key(
        &self,
        keystroke: &Keystroke,
        confirm_enabled: bool,
        window: &mut Window,
        cancel: impl FnOnce(&mut Window),
    ) -> bool {
        let key = keystroke.key.as_str();
        let modifiers = keystroke.modifiers;
        if key == "escape" {
            cancel(window);
            return true;
        }
        if key == "tab" && !modifiers.alt && !modifiers.control && !modifiers.platform {
            let next = if !confirm_enabled || self.confirm.is_focused(window) {
                self.back
            } else if self.back.is_focused(window) || modifiers.shift {
                self.confirm
            } else {
                self.back
            };
            window.focus(next);
            return true;
        }
        // Enter and Space reach the focused dialog button.
        key != "enter" && key != "space" && !modifiers.platform
    }
}

fn empty_confirmation_dialog(
    back: &FocusHandle,
    confirm: &FocusHandle,
    confirm_enabled: bool,
    locale: matou_native_core::sessions::InterfaceLocale,
    on_action: PageActionHandler,
) -> Stateful<Div> {
    let back_focus = back.clone().tab_stop(true);
    let confirm_focus = confirm.clone().tab_stop(true);
    let key_back = back_focus.clone();
    let key_confirm = confirm_focus.clone();
    let key_handler = on_action.clone();
    let back_button = action_button(
        "database-recovery-dialog-back",
        "database-recovery-dialog-back",
        locale.text(BACK, "Back"),
        true,
        ButtonStyle::DialogBack,
        PageAction::CancelConfirmation,
        Some(&back_focus),
        on_action.clone(),
    );
    let confirm_button = action_button(
        "database-recovery-dialog-confirm",
        "database-recovery-dialog-confirm",
        locale.text(CONFIRM_EMPTY, "Create empty database"),
        confirm_enabled,
        ButtonStyle::DialogDanger,
        PageAction::ConfirmEmpty,
        Some(&confirm_focus),
        on_action,
    );

    let title = div()
        .id("database-recovery-dialog-title")
        .debug_selector(|| "database-recovery-dialog-title".to_owned())
        .text_size(px(19.))
        .font_weight(FontWeight::BOLD)
        .line_height(relative(NORMAL_LINE_HEIGHT))
        .text_color(rgb(PRIMARY_TEXT))
        .whitespace_normal()
        .child(ui_text(
            locale.text(CONFIRM_EMPTY_TITLE, "Create a new empty database?"),
        ));
    let body = div()
        .id("database-recovery-dialog-body")
        .debug_selector(|| "database-recovery-dialog-body".to_owned())
        .mt(px(16.))
        .w_full()
        .min_w_0()
        .text_color(rgb(MUTED_TEXT))
        .text_size(px(16.))
        .line_height(relative(1.55))
        .whitespace_normal()
        .child(ui_text(locale.text(CONFIRM_EMPTY_BODY, "Matou will show a brand-new empty workspace. The damaged files and the backups are kept so you can export and investigate them later.")));
    let footer = probe_children(div(), "confirmation-buttons")
        .id("database-recovery-dialog-actions")
        .debug_selector(|| "database-recovery-dialog-actions".to_owned())
        .mt(px(20.))
        .flex()
        .justify_end()
        .gap(px(10.))
        .child(back_button)
        .child(confirm_button);

    let panel = div()
        .id("database-recovery-dialog")
        .debug_selector(|| "database-recovery-dialog".to_owned())
        .w_full()
        .max_w(px(430.))
        .flex()
        .flex_col()
        .p(px(22.))
        .rounded(px(16.))
        .bg(rgb(CARD_BACKGROUND))
        .text_color(rgb(PRIMARY_TEXT))
        .shadow(vec![shadow(0x000000, 0.24, 20., 80.)])
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .capture_key_down(move |event, window, cx| {
            let focus = ConfirmationFocus {
                back: &key_back,
                confirm: &key_confirm,
            };
            if focus.handle_key(&event.keystroke, confirm_enabled, window, |window| {
                key_handler(&PageAction::CancelConfirmation, window, cx)
            }) {
                cx.stop_propagation();
            }
        })
        .child(title)
        .child(body)
        .child(footer);

    div()
        .id("database-recovery-dialog-backdrop")
        .debug_selector(|| "database-recovery-dialog-backdrop".to_owned())
        .absolute()
        .inset_0()
        .occlude()
        .flex()
        .items_center()
        .justify_center()
        .p(px(20.))
        .bg(rgba(DIALOG_BACKDROP))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
        .child(panel)
}

/// Render the cross-workspace read-only banner.
///
/// `background` and `foreground` are the workspace terminal colors as
/// `0xRRGGBB`. The amber wash, bottom rule and muted explanation are derived
/// from them with the same sRGB mix as `read-only-recovery.css`; no extra
/// dependency is involved.
///
/// `pending` drives the in-progress export label and disables the export
/// button. `feedback` is the latest export result: `Ok(Some(path))` and
/// `Ok(None)` render the success copy on the feedback row, `Err(reason)` renders
/// the red failure copy. `search_enabled` and `export_enabled` only control the
/// click bindings; a disabled control never receives `on_search`/`on_export`.
pub fn render_read_only_banner(
    background: u32,
    foreground: u32,
    locale: matou_native_core::sessions::InterfaceLocale,
    pending: bool,
    feedback: Option<Result<Option<&str>, &str>>,
    search_enabled: bool,
    export_enabled: bool,
    on_search: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
    on_export: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> Stateful<Div> {
    let foreground = Hsla::from(rgb(foreground & 0x00ff_ffff));
    let banner_background = mix_color(background, BANNER_AMBER_COLOR, BANNER_TINT);
    let banner_border = mix_color(banner_background, BANNER_BORDER_COLOR, BANNER_BORDER_TINT);

    let title = div()
        .id("read-only-recovery-banner-title")
        .debug_selector(|| "read-only-recovery-banner-title".to_owned())
        .min_w_0()
        .text_color(rgb(BANNER_HEADLINE_COLOR))
        .font_weight(FontWeight::BOLD)
        .whitespace_normal()
        .child(
            ui_text(locale.text(
                READ_ONLY_TITLE,
                "The database is in read-only recovery mode",
            ))
            .normal_line_height(),
        );
    let explanation = div()
        .id("read-only-recovery-banner-explanation")
        .debug_selector(|| "read-only-recovery-banner-explanation".to_owned())
        .min_w_0()
        .text_color(foreground.opacity(0.78))
        .whitespace_normal()
        .child(ui_text(locale.text(READ_ONLY_EXPLANATION, "Existing workspaces, tasks and sessions can still be browsed, searched and copied. Anything that would change data is paused.")));
    let message = div()
        .id("read-only-recovery-banner-message")
        .debug_selector(|| "read-only-recovery-banner-message".to_owned())
        .flex_1()
        .min_w_0()
        .flex()
        .flex_wrap()
        .items_center()
        .gap_y(px(4.))
        .gap_x(px(10.))
        .child(title)
        .child(explanation);

    let search_label = div()
        .id("read-only-recovery-banner-search-label")
        .debug_selector(|| "read-only-recovery-banner-search-label".to_owned())
        .whitespace_nowrap()
        .child(ui_text(locale.text(SEARCH_TERMINAL, "Search this terminal")).normal_line_height());
    let search = div()
        .id("read-only-recovery-banner-search")
        .debug_selector(|| "read-only-recovery-banner-search".to_owned())
        .flex_none()
        .px(px(10.))
        .py(px(5.))
        .border_1()
        .border_color(foreground)
        .rounded(px(6.))
        .bg(rgba(0x00000000))
        .text_color(foreground)
        .whitespace_nowrap()
        .child(search_label);
    let search = if search_enabled {
        search
            .tab_index(0)
            .cursor_pointer()
            .on_click(move |event, window, cx| on_search(event, window, cx))
    } else {
        search.opacity(0.55).cursor(CursorStyle::Arrow)
    };

    let export_label = if pending {
        locale.text(EXPORTING, "Exporting…")
    } else {
        locale.text(EXPORT_DATABASE, "Export recovery bundle")
    };
    let export_label = div()
        .id("read-only-recovery-banner-export-label")
        .debug_selector(|| "read-only-recovery-banner-export-label".to_owned())
        .whitespace_nowrap()
        .child(ui_text(export_label).normal_line_height());
    let export = div()
        .id("read-only-recovery-banner-export")
        .debug_selector(|| "read-only-recovery-banner-export".to_owned())
        .flex_none()
        .px(px(10.))
        .py(px(5.))
        .border_1()
        .border_color(foreground)
        .rounded(px(6.))
        .bg(rgba(0x00000000))
        .text_color(foreground)
        .whitespace_nowrap()
        .child(export_label);
    let export = if export_enabled && !pending {
        export
            .tab_index(0)
            .cursor_pointer()
            .on_click(move |event, window, cx| on_export(event, window, cx))
    } else {
        export.opacity(0.55).cursor(CursorStyle::Arrow)
    };

    let actions = div()
        .id("read-only-recovery-banner-actions")
        .debug_selector(|| "read-only-recovery-banner-actions".to_owned())
        .flex_none()
        .flex()
        .flex_nowrap()
        .items_center()
        .justify_end()
        .gap(px(10.))
        .child(search)
        .child(export);
    let row = div()
        .id("read-only-recovery-banner-row")
        .debug_selector(|| "read-only-recovery-banner-row".to_owned())
        .w_full()
        .min_w_0()
        .flex()
        .items_center()
        .gap_x(px(14.))
        .child(message)
        .child(actions);

    let mut banner = div()
        .id("read-only-recovery-banner")
        .debug_selector(|| "read-only-recovery-banner".to_owned())
        .w_full()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(6.))
        .px(px(14.))
        .py(px(8.))
        .border_b(px(1.))
        .border_color(rgb(banner_border))
        .bg(rgb(banner_background))
        .text_color(foreground)
        .child(row);

    if let Some((text, error)) = read_only_feedback(feedback, locale) {
        let (id, selector, color) = if error {
            (
                "read-only-recovery-banner-error",
                "read-only-recovery-banner-error",
                Hsla::from(rgb(BANNER_ERROR_COLOR)).opacity(0.78),
            )
        } else {
            (
                "read-only-recovery-banner-feedback",
                "read-only-recovery-banner-feedback",
                foreground,
            )
        };
        banner = banner.child(
            div()
                .id(id)
                .debug_selector(move || selector.to_owned())
                .w_full()
                .min_w_0()
                .text_size(px(12.))
                .text_color(color)
                .whitespace_normal()
                .child(ui_text(text)),
        );
    }

    banner
}

fn read_only_feedback(
    feedback: Option<Result<Option<&str>, &str>>,
    locale: matou_native_core::sessions::InterfaceLocale,
) -> Option<(String, bool)> {
    feedback.map(|result| match result {
        Ok(Some(path)) => (
            format!(
                "{}{path}",
                locale.text("数据库资料已导出到 ", "Recovery bundle exported to ")
            ),
            false,
        ),
        Ok(None) => (
            locale.text(EXPORTED, "Recovery bundle exported").to_owned(),
            false,
        ),
        Err(reason) => (
            format!("{}{reason}", locale.text(EXPORT_FAILED, "Export failed: ")),
            true,
        ),
    })
}

/// Mix two opaque `0xRRGGBB` colors in sRGB. `percent` is the weight of
/// `tint`, matching CSS `color-mix(in srgb, …)`.
fn mix_color(base: u32, tint: u32, percent: u32) -> u32 {
    let tint_weight = percent.min(100);
    let base_weight = 100 - tint_weight;
    let channel = |shift: u32| {
        let base = (base >> shift) & 0xff;
        let tint = (tint >> shift) & 0xff;
        (base * base_weight + tint * tint_weight + 50) / 100
    };
    (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

/// The frozen page sorts by time descending, then ID descending, and keeps the
/// newest seven verified backups. Sorting lives here as well as in the backup
/// service so a reordered or reconnected projection cannot change the list.
pub(super) fn visible_backups(backups: &[DatabaseRecoveryBackup]) -> Vec<&DatabaseRecoveryBackup> {
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
        .line_height(relative(NORMAL_LINE_HEIGHT))
        .whitespace_normal()
        .child(ui_text(text))
}

fn backup_row(
    index: usize,
    backup: &DatabaseRecoveryBackup,
    selected: bool,
    enabled: bool,
    locale: matou_native_core::sessions::InterfaceLocale,
    on_action: PageActionHandler,
) -> Stateful<Div> {
    let metadata = format!(
        "{} · {} {} · {}",
        format_backup_time(backup.created_at),
        locale.text("数据版本", "Schema version"),
        backup.schema_version,
        format_backup_size(backup.size)
    );
    let marker = format!(
        "database-recovery-backup-{index}-{}",
        if selected { "selected" } else { "unselected" }
    );
    let radio = div()
        .flex_none()
        .size(px(14.))
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .border_1()
        .border_color(rgb(if selected { ACCENT } else { RADIO_BORDER }))
        .bg(rgb(if selected { ACCENT } else { CARD_BACKGROUND }))
        .when(selected, |radio| {
            radio.child(div().size(px(5.)).rounded_full().bg(rgb(0xffffff)))
        })
        .when(!enabled, |radio| radio.opacity(0.6));
    let text = div()
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
                .overflow_hidden()
                .text_color(rgb(PRIMARY_TEXT))
                .text_size(px(16.))
                .font_weight(FontWeight::BOLD)
                .line_height(relative(1.4))
                .whitespace_nowrap()
                .text_ellipsis()
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
                .overflow_hidden()
                .text_color(rgb(SECONDARY_TEXT))
                .text_size(px(13.))
                .line_height(relative(1.4))
                .whitespace_nowrap()
                .text_ellipsis()
                .child(ui_text(metadata)),
        );
    let row = div()
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
        .border_color(rgb(if selected {
            SELECTED_BORDER
        } else {
            ROW_BORDER
        }))
        .rounded(px(11.))
        .bg(rgb(if selected {
            SELECTED_BACKGROUND
        } else {
            CARD_BACKGROUND
        }))
        .child(radio)
        .child(text)
        .child(
            div()
                .flex_none()
                .w(px(0.))
                .h(px(0.))
                .debug_selector(move || marker)
                .into_any_element(),
        );
    if !enabled {
        return row.cursor(CursorStyle::Arrow);
    }

    let key_action = PageAction::SelectBackup(backup.id.clone());
    let click_action = key_action.clone();
    let key_handler = on_action.clone();
    let click_handler = on_action;
    row.tab_index(0)
        .cursor_pointer()
        .focus(|style| style.border_color(rgb(ACCENT_FOCUS)))
        .on_key_down(move |event, window, cx| {
            if is_activation_key(&event.keystroke) {
                cx.stop_propagation();
                key_handler(&key_action, window, cx);
            }
        })
        .on_click(move |event, window, cx| {
            if matches!(event, ClickEvent::Keyboard(_)) {
                return;
            }
            cx.stop_propagation();
            click_handler(&click_action, window, cx);
        })
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

#[cfg(test)]
mod tests {
    use gpui::FontWeight;

    use super::{
        ACCENT, ButtonStyle, CARD_BACKGROUND, DANGER_BUTTON, DIALOG_BUTTON_BACKGROUND,
        DIALOG_BUTTON_BORDER, DIALOG_BUTTON_TEXT, PRIMARY_TEXT, mix_color,
    };

    #[test]
    fn card_buttons_use_regular_38pt_chrome() {
        let primary = ButtonStyle::Primary.spec(true);
        assert_eq!(primary.height, Some(38.));
        assert_eq!(primary.padding_x, 14.);
        assert_eq!(primary.border_width, 1.);
        assert_eq!(primary.radius, 9.);
        assert_eq!(primary.background, Some(ACCENT));
        assert_eq!(primary.border, Some(ACCENT));
        assert_eq!(primary.text, 0xffffff);
        assert_eq!(primary.font_size, 16.);
        assert_eq!(primary.font_weight, FontWeight::NORMAL);

        let secondary = ButtonStyle::Secondary.spec(true);
        assert_eq!(secondary.height, Some(38.));
        assert_eq!(secondary.radius, 9.);
        assert_eq!(secondary.padding_x, 14.);
        assert_eq!(secondary.border_width, 1.);
        assert_eq!(secondary.font_weight, FontWeight::NORMAL);
    }

    #[test]
    fn disabled_primary_keeps_its_white_label() {
        let disabled = ButtonStyle::Primary.spec(false);
        assert_eq!(disabled.text, 0xffffff);
        assert_eq!(disabled.background, Some(0x9ebefb));
        assert_eq!(
            disabled.border, disabled.background,
            "fill and border must stay equal, as in the frozen group opacity"
        );

        let secondary = ButtonStyle::Secondary.spec(false);
        assert_eq!(
            secondary.text,
            mix_color(PRIMARY_TEXT, CARD_BACKGROUND, 52),
            "dark labels follow the .48 group fade"
        );
    }

    #[test]
    fn dialog_buttons_use_browser_28pt_geometry() {
        let back = ButtonStyle::DialogBack.spec(true);
        assert_eq!(back.height, Some(28.));
        assert_eq!(back.padding_x, 7.);
        assert_eq!(back.border_width, 1.);
        assert_eq!(back.radius, 0.);
        assert_eq!(back.background, Some(DIALOG_BUTTON_BACKGROUND));
        assert_eq!(back.border, Some(DIALOG_BUTTON_BORDER));
        assert_eq!(back.text, DIALOG_BUTTON_TEXT);
        assert_eq!(back.font_size, 16.);
        assert_eq!(back.font_weight, FontWeight::NORMAL);

        let confirm = ButtonStyle::DialogDanger.spec(true);
        assert_eq!(confirm.height, Some(28.));
        assert_eq!(confirm.padding_x, 7.);
        assert_eq!(confirm.border_width, 1.);
        assert_eq!(confirm.radius, 0.);
        assert_eq!(confirm.background, Some(DANGER_BUTTON));
        assert_eq!(confirm.text, 0xffffff);
    }
}
