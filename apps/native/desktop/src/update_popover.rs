//! Presentation and interaction state for the desktop update control.

use std::time::Duration;

use matou_native_core::sessions::InterfaceLocale;

pub const UPDATED_TOAST_DURATION: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateInstallMode {
    Automatic,
    Manual,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateErrorStage {
    Check,
    Download,
    Verify,
    Install,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateRelease {
    pub version: String,
    pub install_mode: UpdateInstallMode,
    pub size_bytes: Option<u64>,
    /// Release date in the user's local calendar, as (month, day).
    pub release_day: Option<(u8, u8)>,
    pub notes: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UpdateProgress {
    pub percent: f64,
    pub transferred_bytes: u64,
    pub total_bytes: u64,
    pub bytes_per_second: u64,
    pub remaining_seconds: Option<u64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum UpdateStatus {
    Idle,
    NotAvailable,
    Checking {
        retry_attempt: u32,
        max_retry_attempts: Option<u32>,
    },
    Available(UpdateRelease),
    Downloading(UpdateRelease, UpdateProgress),
    Downloaded(UpdateRelease),
    Error {
        stage: UpdateErrorStage,
        message: String,
        manual_download_available: bool,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct UpdateState {
    pub current_version: String,
    pub status: UpdateStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateAction {
    TogglePopover,
    Dismiss,
    Check,
    Download,
    Install,
    SetWaitingForIdle(bool),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateEffect {
    Check,
    Download,
    Install,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateButtonStyle {
    Primary,
    Normal,
    Quiet,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateButton {
    pub label: &'static str,
    pub action: UpdateAction,
    pub enabled: bool,
    pub style: UpdateButtonStyle,
    pub in_row: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UpdateProgressView {
    pub fraction: f32,
    pub rounded_percent: i64,
    pub sizes: String,
    pub percent_label: String,
    pub rate: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UpdateView {
    pub open: bool,
    pub label: String,
    pub popover_label: &'static str,
    pub close_label: &'static str,
    pub highlight: bool,
    pub dot: bool,
    pub title: String,
    pub subtitle: String,
    pub message: Option<String>,
    pub error: bool,
    pub notice: Option<&'static str>,
    pub notes: Vec<String>,
    pub progress: Option<UpdateProgressView>,
    pub active_sessions: Option<(String, &'static str)>,
    pub scheduled: Option<&'static str>,
    pub buttons: Vec<UpdateButton>,
    pub toast: Option<(String, &'static str)>,
}

/// Effects are handed back to the updater owner. Time values use the same caller-provided clock.
#[derive(Debug)]
pub struct UpdateControl {
    state: UpdateState,
    open: bool,
    active_session_count: usize,
    waiting_for_idle: bool,
    installing: Option<UpdateInstallMode>,
    toast_until: Option<Duration>,
    last_seen_version: Option<String>,
}

impl UpdateControl {
    pub fn new(state: UpdateState, last_seen_version: Option<&str>, now: Duration) -> Self {
        let toast_until = last_seen_version
            .filter(|version| !version.is_empty() && *version != state.current_version)
            .filter(|_| !state.current_version.is_empty())
            .map(|_| now.saturating_add(UPDATED_TOAST_DURATION));
        let last_seen_version = if state.current_version.is_empty() {
            last_seen_version
                .filter(|version| !version.is_empty())
                .map(str::to_owned)
        } else {
            Some(state.current_version.clone())
        };
        Self {
            open: opens_on_arrival(&state.status),
            state,
            active_session_count: 0,
            waiting_for_idle: false,
            installing: None,
            toast_until,
            last_seen_version,
        }
    }

    /// The owner may persist this version after accepting a snapshot.
    pub fn current_version(&self) -> &str {
        &self.state.current_version
    }

    pub fn accept(&mut self, state: UpdateState, now: Duration) -> Option<UpdateEffect> {
        if self.state.current_version != state.current_version {
            if state.current_version.is_empty() {
                self.toast_until = None;
            } else {
                self.toast_until = self
                    .last_seen_version
                    .as_ref()
                    .filter(|version| *version != &state.current_version)
                    .map(|_| now.saturating_add(UPDATED_TOAST_DURATION));
                self.last_seen_version = Some(state.current_version.clone());
            }
        }
        self.open |= opens_on_arrival(&state.status);
        self.state = state;
        self.install_when_idle()
    }

    pub fn set_active_sessions(&mut self, count: usize) -> Option<UpdateEffect> {
        self.active_session_count = count;
        self.install_when_idle()
    }

    pub fn finish_install(&mut self, ok: bool) {
        if !ok || self.installing == Some(UpdateInstallMode::Manual) {
            self.installing = None;
            self.waiting_for_idle = false;
        }
    }

    pub fn dispatch(&mut self, action: UpdateAction) -> Option<UpdateEffect> {
        match action {
            UpdateAction::TogglePopover => self.open = !self.open,
            UpdateAction::Dismiss => self.open = false,
            _ if !self.open
                || !self
                    .buttons(InterfaceLocale::English, cfg!(target_os = "windows"))
                    .iter()
                    .any(|button| button.action == action && button.enabled) =>
            {
                return None;
            }
            UpdateAction::Check => return Some(UpdateEffect::Check),
            UpdateAction::Download => return Some(UpdateEffect::Download),
            UpdateAction::Install => return self.request_install(),
            UpdateAction::SetWaitingForIdle(waiting) => self.waiting_for_idle = waiting,
        }
        None
    }

    fn install_when_idle(&mut self) -> Option<UpdateEffect> {
        if self.waiting_for_idle
            && self.active_session_count == 0
            && matches!(&self.state.status, UpdateStatus::Downloaded(release)
                if release.install_mode == UpdateInstallMode::Automatic)
        {
            return self.request_install();
        }
        None
    }

    fn request_install(&mut self) -> Option<UpdateEffect> {
        if self.installing.is_some() {
            return None;
        }
        let UpdateStatus::Downloaded(release) = &self.state.status else {
            return None;
        };
        self.installing = Some(release.install_mode);
        Some(UpdateEffect::Install)
    }

    fn buttons(&self, locale: InterfaceLocale, windows: bool) -> Vec<UpdateButton> {
        use UpdateAction::*;
        use UpdateButtonStyle::*;
        let installing = self.installing.is_some();
        let button = |zh, en, action, style, enabled, in_row| UpdateButton {
            label: locale.text(zh, en),
            action,
            style,
            enabled,
            in_row,
        };
        let primary = |zh, en, action| button(zh, en, action, Primary, true, false);
        let dismiss = |zh, en| button(zh, en, Dismiss, Quiet, true, false);
        match &self.state.status {
            UpdateStatus::Idle | UpdateStatus::NotAvailable => {
                vec![primary("检查更新", "Check for updates", Check)]
            }
            UpdateStatus::Checking { .. } => vec![],
            UpdateStatus::Error {
                manual_download_available: true,
                ..
            } => vec![
                if windows {
                    primary("下载安装包更新", "Download installer update", Download)
                } else {
                    primary("下载 DMG 更新", "Download DMG update", Download)
                },
                button("重新检查", "Check again", Check, Quiet, true, false),
            ],
            UpdateStatus::Error { .. } => vec![primary("重新检查", "Check again", Check)],
            UpdateStatus::Available(release) => vec![
                if release.install_mode == UpdateInstallMode::Manual {
                    if windows {
                        primary("下载安装包更新", "Download installer update", Download)
                    } else {
                        primary("下载更新", "Download update", Download)
                    }
                } else {
                    primary("后台下载", "Download in background", Download)
                },
                dismiss("稍后提醒", "Remind me later"),
            ],
            UpdateStatus::Downloading(..) => {
                vec![dismiss("继续在后台下载", "Keep downloading in background")]
            }
            UpdateStatus::Downloaded(release)
                if release.install_mode == UpdateInstallMode::Manual =>
            {
                vec![
                    if installing {
                        button("正在打开…", "Opening…", Install, Primary, false, false)
                    } else if windows {
                        primary("打开安装包安装", "Open installer", Install)
                    } else {
                        primary("打开 DMG 安装", "Open DMG installer", Install)
                    },
                    dismiss("稍后安装", "Install later"),
                ]
            }
            UpdateStatus::Downloaded(_) if self.active_session_count > 0 => vec![
                if self.waiting_for_idle {
                    button(
                        "取消空闲更新",
                        "Cancel idle update",
                        SetWaitingForIdle(false),
                        Normal,
                        !installing,
                        false,
                    )
                } else {
                    button(
                        "空闲后自动更新",
                        "Update once idle",
                        SetWaitingForIdle(true),
                        Primary,
                        !installing,
                        false,
                    )
                },
                button(
                    "立即重启并更新",
                    "Install and restart now",
                    Install,
                    Normal,
                    !installing,
                    true,
                ),
                button("退出时安装", "Install on quit", Dismiss, Normal, true, true),
            ],
            UpdateStatus::Downloaded(_) => vec![
                if installing {
                    button(
                        "正在准备更新…",
                        "Preparing update…",
                        Install,
                        Primary,
                        false,
                        false,
                    )
                } else {
                    primary("重启并更新", "Install and restart", Install)
                },
                dismiss("退出时安装", "Install on quit"),
            ],
        }
    }

    pub fn view(&self, locale: InterfaceLocale, now: Duration) -> UpdateView {
        self.view_for_platform(locale, now, cfg!(target_os = "windows"))
    }

    fn view_for_platform(
        &self,
        locale: InterfaceLocale,
        now: Duration,
        windows: bool,
    ) -> UpdateView {
        let popover_label = locale.text("Matou 应用更新", "Matou app update");
        let fallback_version = if self.state.current_version.is_empty() {
            "—"
        } else {
            &self.state.current_version
        };
        let current = format!(
            "{} {fallback_version}",
            locale.text("当前版本", "Current version")
        );
        let mut view = UpdateView {
            open: self.open,
            label: locale.text("应用更新", "App update").into(),
            popover_label,
            close_label: locale.text("关闭更新浮层", "Close update popover"),
            highlight: false,
            dot: opens_on_arrival(&self.state.status),
            title: popover_label.into(),
            subtitle: current.clone(),
            message: None,
            error: false,
            notice: None,
            notes: vec![],
            progress: None,
            active_sessions: None,
            scheduled: None,
            buttons: self.buttons(locale, windows),
            toast: self.toast_until.filter(|until| now < *until).map(|_| {
                (
                    format!(
                        "{} {}",
                        locale.text("Matou 已更新至", "Matou updated to"),
                        self.state.current_version
                    ),
                    locale.text("工作空间与会话已恢复", "Workspaces and sessions restored"),
                )
            }),
        };
        match &self.state.status {
            // The subtitle already shows `当前版本 …`; Idle must not repeat it.
            UpdateStatus::Idle => view.message = None,
            UpdateStatus::NotAvailable => {
                view.title = locale
                    .text("Matou 已是最新版本", "Matou is up to date")
                    .into();
                view.message = Some(match locale {
                    InterfaceLocale::Chinese => {
                        format!("当前已是最新版本（{}）", self.state.current_version)
                    }
                    InterfaceLocale::English => format!(
                        "Already on the latest version ({})",
                        self.state.current_version
                    ),
                });
            }
            UpdateStatus::Checking {
                retry_attempt,
                max_retry_attempts,
            } => {
                view.title = locale.text("正在检查更新", "Checking for updates").into();
                view.label = locale
                    .text("应用更新：正在检查", "App update: checking")
                    .into();
                view.message = Some(if *retry_attempt == 0 {
                    locale
                        .text("正在检查云端是否有新版本…", "Checking for a newer version…")
                        .into()
                } else {
                    let max = max_retry_attempts.unwrap_or(*retry_attempt);
                    match locale {
                        InterfaceLocale::Chinese => {
                            format!("连接波动，正在自动重试（{retry_attempt}/{max}）…")
                        }
                        InterfaceLocale::English => format!(
                            "Connection is unstable, retrying automatically ({retry_attempt}/{max})…"
                        ),
                    }
                });
            }
            UpdateStatus::Error { stage, message, .. } => {
                view.title = error_title(*stage, locale).into();
                view.label = format!(
                    "{}{}",
                    locale.text("应用更新：", "App update: "),
                    view.title
                );
                view.error = true;
                view.message = Some(friendly_error(*stage, message, locale, windows));
            }
            UpdateStatus::Available(release) => {
                let version = &release.version;
                view.title = match locale {
                    InterfaceLocale::Chinese => format!("Matou {version} 可用"),
                    InterfaceLocale::English => format!("Matou {version} is available"),
                };
                view.label = match locale {
                    InterfaceLocale::Chinese => format!("应用更新：发现 {version}"),
                    InterfaceLocale::English => format!("App update: {version} available"),
                };
                view.subtitle = release_subtitle(locale.text("稳定版", "Stable").into(), release);
                view.highlight = true;
                view.notes = release.notes.iter().take(3).cloned().collect();
                if release.install_mode == UpdateInstallMode::Manual {
                    view.notice = Some(if windows {
                        locale.text(
                            "当前体验包将通过应用内下载安装包更新。",
                            "This build updates by downloading an installer in the app.",
                        )
                    } else {
                        locale.text(
                            "当前体验包将通过应用内下载 DMG 更新。",
                            "This build updates through an in-app DMG download.",
                        )
                    });
                }
            }
            UpdateStatus::Downloading(release, progress) => {
                let progress = progress_view(*progress, locale);
                view.title = locale
                    .text("正在后台下载", "Downloading in background")
                    .into();
                view.label = format!(
                    "{} {}%",
                    locale.text("应用更新：下载中", "App update: downloading"),
                    progress.rounded_percent
                );
                view.subtitle = release_subtitle(format!("Matou {}", release.version), release);
                view.highlight = true;
                view.progress = Some(progress);
            }
            UpdateStatus::Downloaded(release) => {
                let manual = release.install_mode == UpdateInstallMode::Manual;
                view.title = if manual && windows {
                    locale.text("安装包已下载完成", "Installer downloaded")
                } else if manual {
                    locale.text("DMG 已下载完成", "DMG downloaded")
                } else {
                    locale.text("更新已准备好", "Update ready")
                }
                .into();
                view.label = if manual {
                    locale.text(
                        "应用更新：等待打开安装包",
                        "App update: waiting to open installer",
                    )
                } else {
                    locale.text("应用更新：等待安装", "App update: waiting to install")
                }
                .into();
                view.subtitle = release_subtitle(format!("Matou {}", release.version), release);
                view.highlight = true;
                view.notes = release.notes.iter().take(3).cloned().collect();
                if !manual && self.active_session_count > 0 {
                    let count = self.active_session_count;
                    view.active_sessions = Some((match locale {
                        InterfaceLocale::Chinese => format!("当前有 {count} 个活动会话"),
                        InterfaceLocale::English if count == 1 => "You have 1 active session".into(),
                        InterfaceLocale::English => format!("You have {count} active sessions"),
                    }, locale.text("空闲后更新会保留工作区、画布位置及会话恢复信息。",
                        "Updating once idle keeps your workspaces, canvas positions and session recovery data.")));
                }
                if !manual && self.waiting_for_idle {
                    view.scheduled =
                        Some(locale.text("已安排：空闲后自动更新", "Scheduled: update once idle"));
                }
            }
        }
        view
    }
}

fn opens_on_arrival(status: &UpdateStatus) -> bool {
    matches!(
        status,
        UpdateStatus::Available(_) | UpdateStatus::Downloaded(_)
    )
}

fn release_subtitle(channel_or_version: String, release: &UpdateRelease) -> String {
    let mut parts = vec![channel_or_version];
    if let Some(bytes) = release.size_bytes.filter(|bytes| *bytes != 0) {
        parts.push(format_bytes(bytes));
    }
    if let Some((month, day)) = release.release_day {
        parts.push(format!("{month}/{day}"));
    }
    parts.join(" · ")
}

fn format_bytes(bytes: u64) -> String {
    match bytes {
        0 => "0 MB".into(),
        // JS toFixed rounds exact positive halfway values up.
        bytes if bytes % 500_000 == 250_000 => {
            let tenths = bytes / 100_000 + 1;
            format!("{}.{} MB", tenths / 10, tenths % 10)
        }
        bytes => format!("{:.1} MB", bytes as f64 / 1_000_000.0),
    }
}

fn progress_view(progress: UpdateProgress, locale: InterfaceLocale) -> UpdateProgressView {
    let percent = if progress.percent.is_finite() {
        progress.percent
    } else {
        0.0
    };
    let rounded_percent = (percent + 0.5).floor() as i64;
    let mut percent_label = format!("{rounded_percent}%");
    if let Some(seconds) = progress.remaining_seconds {
        let remaining = match locale {
            InterfaceLocale::Chinese => format!("约 {seconds} 秒"),
            InterfaceLocale::English => format!("about {seconds}s left"),
        };
        percent_label.push_str(&format!(" · {remaining}"));
    }
    UpdateProgressView {
        fraction: (percent.clamp(0.0, 100.0) / 100.0) as f32,
        rounded_percent,
        sizes: format!(
            "{} / {}",
            format_bytes(progress.transferred_bytes),
            format_bytes(progress.total_bytes)
        ),
        percent_label,
        rate: format!(
            "{}{}",
            format_bytes(progress.bytes_per_second),
            locale.text(
                "/秒 · 可继续使用当前会话",
                "/s · you can keep using your sessions"
            )
        ),
    }
}

fn error_title(stage: UpdateErrorStage, locale: InterfaceLocale) -> &'static str {
    match stage {
        UpdateErrorStage::Check => locale.text("更新检查失败", "Update check failed"),
        UpdateErrorStage::Download => locale.text("更新下载失败", "Update download failed"),
        UpdateErrorStage::Verify => {
            locale.text("安装包校验未通过", "Installer verification failed")
        }
        UpdateErrorStage::Install => locale.text("更新安装失败", "Update installation failed"),
    }
}

fn friendly_error(
    stage: UpdateErrorStage,
    message: &str,
    locale: InterfaceLocale,
    windows: bool,
) -> String {
    let message = message.to_ascii_lowercase();
    let contains = |patterns: &[&str]| patterns.iter().any(|pattern| message.contains(pattern));
    let dns = || {
        message
            .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
            .any(|word| word == "dns")
    };
    let timed_out = || {
        message.match_indices("time").any(|(index, _)| {
            let tail = &message[index + 4..];
            tail.strip_prefix('d')
                .unwrap_or(tail)
                .trim_start()
                .starts_with("out")
        })
    };
    let manifest = || {
        message.split_whitespace().any(|word| {
            word.find("latest")
                .is_some_and(|index| word[index + 6..].contains(".yml"))
        })
    };
    let text = if stage == UpdateErrorStage::Verify && windows {
        locale.text(
            "更新安装包未通过完整性校验，请重新检查更新。",
            "The update installer failed its integrity check. Please check for updates again.",
        )
    } else if stage == UpdateErrorStage::Verify {
        locale.text("当前安装包缺少 Apple 发布签名，改用应用内 DMG 下载继续更新。",
            "This installer has no Apple release signature. Matou will continue with the in-app DMG download.")
    } else if stage == UpdateErrorStage::Install {
        locale.text(
            "更新安装前的会话保存或应用退出过程出现异常，请重新尝试。",
            "Saving sessions or quitting the app before the install went wrong. Please try again.",
        )
    } else if contains(&[
        "enotfound",
        "eai_again",
        "err_name_not_resolved",
        "getaddrinfo",
    ]) || dns()
    {
        locale.text(
            "更新服务器域名解析失败，请检查网络或 DNS 设置。",
            "The update server address could not be resolved. Check your network or DNS settings.",
        )
    } else if contains(&["etimedout", "err_timed_out"]) || timed_out() {
        locale.text(
            "连接更新服务器超时，请检查网络后重试。",
            "The connection to the update server timed out. Check your network and try again.",
        )
    } else if contains(&[
        "enetunreach",
        "err_internet_disconnected",
        "network is unreachable",
    ]) {
        locale.text(
            "当前设备尚未接入网络，请恢复网络后重试。",
            "This device is not connected to a network. Reconnect and try again.",
        )
    } else if contains(&["econnrefused", "err_connection_refused"]) {
        locale.text(
            "更新服务器暂时拒绝连接，请稍后重试。",
            "The update server refused the connection. Please try again shortly.",
        )
    } else if contains(&["econnreset", "err_connection_reset", "socket hang up"]) {
        locale.text("下载连接被中途断开，应用已保留当前版本，请重新尝试。",
            "The download connection was cut off. Matou kept the current version, please try again.")
    } else if contains(&["cert_", "ssl", "tls", "certificate"]) {
        locale.text("更新服务器的安全连接校验异常，请检查系统时间后重试。",
            "The secure connection to the update server failed verification. Check the system clock and try again.")
    } else if let Some(code) = http_status(&message) {
        return match locale {
            InterfaceLocale::Chinese => format!("更新服务器返回 HTTP {code}，请稍后重试。"),
            InterfaceLocale::English => {
                format!("The update server returned HTTP {code}. Please try again later.")
            }
        };
    } else if contains(&["stable-mac.yml", "yaml", "parse"]) || manifest() {
        locale.text("更新信息格式异常，当前版本保持不变，请稍后重试。",
            "The update manifest is malformed. The current version is unchanged, please try again later.")
    } else if contains(&["network", "server", "fetch", "econn", "timeout"]) {
        locale.text(
            "暂时没有连接到更新服务器，请检查网络后重试。",
            "Matou could not reach the update server. Check your network and try again.",
        )
    } else if stage == UpdateErrorStage::Download {
        locale.text(
            "更新文件下载中断，请检查网络后重试。",
            "The update download was interrupted. Check your network and try again.",
        )
    } else {
        locale.text(
            "更新服务出现异常，请稍后重新检查。",
            "The update service ran into a problem. Please check again later.",
        )
    };
    text.into()
}

fn http_status(message: &str) -> Option<&str> {
    message
        .match_indices("http")
        .chain(message.match_indices("status"))
        .filter_map(|(index, keyword)| {
            let remainder = message[index + keyword.len()..]
                .trim_start_matches(|ch: char| !ch.is_ascii_digit());
            remainder
                .get(..3)
                .filter(|code| code.bytes().all(|ch| ch.is_ascii_digit()))
                .map(|code| (index, code))
        })
        .min_by_key(|(index, _)| *index)
        .map(|(_, code)| code)
}

// App update status point, popover and toast, appended to the native update module.
// Frozen source: updates/AppUpdateControl.tsx + hierarchy.css 114-161, 776-796.

mod update_ui {
    use super::*;
    use gpui::{prelude::*, *};
    use std::rc::Rc;

    #[derive(Clone, Copy)]
    struct UpdatePalette {
        trigger: Rgba,
        trigger_hover: Rgba,
        trigger_hover_bg: Rgba,
        trigger_active: Rgba,
        trigger_active_bg: Rgba,
        dot: Rgba,
        dot_border: Rgba,
        popover_bg: Rgba,
        popover_border: Rgba,
        text: Rgba,
        subtitle: Rgba,
        body: Rgba,
        divider: Rgba,
        close: Rgba,
        close_hover: Rgba,
        close_hover_bg: Rgba,
        button_text: Rgba,
        button_bg: Rgba,
        button_hover: Rgba,
        quiet_text: Rgba,
        primary_from: Rgba,
        primary_to: Rgba,
        note_dot: Rgba,
        download_text: Rgba,
        download_rate: Rgba,
        track: Rgba,
        bar_from: Rgba,
        bar_to: Rgba,
        session_text: Rgba,
        session_hint: Rgba,
        session_bg: Rgba,
        session_dot: Rgba,
        error_hint: Rgba,
        error_bg: Rgba,
        waiting: Rgba,
        toast_bg: Rgba,
        toast_border: Rgba,
        toast_muted: Rgba,
        toast_check: Rgba,
    }

    impl UpdatePalette {
        fn new(dark: bool) -> Self {
            if dark {
                Self {
                    trigger: rgb(0x747b88),
                    trigger_hover: rgb(0xe9eef5),
                    trigger_hover_bg: rgba(0xffffff14),
                    trigger_active: rgb(0x69adff),
                    trigger_active_bg: rgba(0x3a8ef11c),
                    dot: rgb(0x3d98ff),
                    dot_border: rgb(0x151515),
                    popover_bg: rgb(0x1f232a),
                    popover_border: rgba(0xffffff1a),
                    text: rgb(0xe3e7ee),
                    subtitle: rgb(0x878f9e),
                    body: rgb(0x9ba3b0),
                    divider: rgba(0xffffff12),
                    close: rgb(0x7f8795),
                    close_hover: rgb(0xe9eef5),
                    close_hover_bg: rgba(0xffffff12),
                    button_text: rgb(0xd8dde6),
                    button_bg: rgba(0xffffff12),
                    button_hover: rgba(0xffffff1a),
                    quiet_text: rgb(0x8992a0),
                    primary_from: rgb(0x3c97fa),
                    primary_to: rgb(0x287fe9),
                    note_dot: rgb(0x62aaff),
                    download_text: rgb(0xa7afbc),
                    download_rate: rgb(0x7f8794),
                    track: rgba(0x7889a026),
                    bar_from: rgb(0x378df4),
                    bar_to: rgb(0x6bbaff),
                    session_text: rgb(0xd6b16d),
                    session_hint: rgb(0xa98d5c),
                    session_bg: rgba(0xce96331f),
                    session_dot: rgb(0xc4943e),
                    error_hint: rgb(0xbd8b87),
                    error_bg: rgba(0xe34b3f1a),
                    waiting: rgb(0x69adff),
                    toast_bg: rgb(0x1f232a),
                    toast_border: rgba(0xffffff1a),
                    toast_muted: rgb(0x8d96a5),
                    toast_check: rgb(0x2ca56d),
                }
            } else {
                Self {
                    trigger: rgb(0x687387),
                    trigger_hover: rgb(0x354156),
                    trigger_hover_bg: rgba(0x3e4c6514),
                    trigger_active: rgb(0x2686f6),
                    trigger_active_bg: rgba(0x2b89f71a),
                    dot: rgb(0x3d98ff),
                    dot_border: rgb(0xffffff),
                    popover_bg: rgb(0xfcfdff),
                    popover_border: rgba(0xffffffe6),
                    text: rgb(0x283142),
                    subtitle: rgb(0x778196),
                    body: rgb(0x778196),
                    divider: rgba(0x3e4c6514),
                    close: rgb(0x8790a0),
                    close_hover: rgb(0x354156),
                    close_hover_bg: rgba(0x3e4c6512),
                    button_text: rgb(0x4e5a6e),
                    button_bg: rgba(0x5c6c8614),
                    button_hover: rgba(0x5c6c8624),
                    quiet_text: rgb(0x718096),
                    primary_from: rgb(0x3994fa),
                    primary_to: rgb(0x257eea),
                    note_dot: rgb(0x62aaff),
                    download_text: rgb(0x6b7688),
                    download_rate: rgb(0x8790a0),
                    track: rgba(0x7889a026),
                    bar_from: rgb(0x378df4),
                    bar_to: rgb(0x6bbaff),
                    session_text: rgb(0x73572f),
                    session_hint: rgb(0x8c7149),
                    session_bg: rgba(0xffb8441f),
                    session_dot: rgb(0xc4943e),
                    error_hint: rgb(0x9d6d68),
                    error_bg: rgba(0xdf4b3f17),
                    waiting: rgb(0x2686f6),
                    toast_bg: rgb(0xfcfdff),
                    toast_border: rgba(0xffffffe6),
                    toast_muted: rgb(0x8790a0),
                    toast_check: rgb(0x2ca56d),
                }
            }
        }
    }

    /// Test-only mirror of the two opaque surfaces used by the popover/toast.
    #[cfg(test)]
    pub(super) fn background_alpha_for_test(dark: bool) -> (f32, f32) {
        let palette = UpdatePalette::new(dark);
        (palette.popover_bg.a, palette.toast_bg.a)
    }

    fn update_shadow(color: u32, alpha: f32, y: f32, blur: f32) -> BoxShadow {
        let mut color: Hsla = rgb(color).into();
        color.a = alpha;
        BoxShadow {
            color,
            offset: point(px(0.), px(y)),
            blur_radius: px(blur),
            spread_radius: px(0.),
        }
    }

    fn update_download_icon(size: f32, color: Rgba) -> Svg {
        svg()
            .path("icons/download.svg")
            .size(px(size))
            .flex_none()
            .text_color(color)
    }

    /// The HUD status point. `label` is the frozen accessible label and is kept on
    /// the model; GPUI has no accessibility tree, so the visible surface stays the icon.
    pub fn render_update_trigger(
        view: &UpdateView,
        dark: bool,
        on_action: Rc<dyn Fn(UpdateAction, &mut Window, &mut App)>,
    ) -> impl IntoElement {
        let palette = UpdatePalette::new(dark);
        let highlighted = view.highlight;
        let expanded = view.open;
        let icon_color = if highlighted {
            palette.trigger_active
        } else {
            palette.trigger
        };
        let on_action = on_action.clone();

        let mut trigger = div()
            .id("app-update-trigger")
            .size(px(30.))
            .rounded(px(8.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .focusable()
            .bg(if highlighted {
                palette.trigger_active_bg
            } else if expanded {
                palette.trigger_hover_bg
            } else {
                rgba(0x00000000)
            })
            .text_color(if highlighted {
                palette.trigger_active
            } else if expanded {
                palette.trigger_hover
            } else {
                palette.trigger
            })
            .on_click(move |_, window, cx| {
                on_action(UpdateAction::TogglePopover, window, cx);
            });

        if !highlighted {
            trigger = trigger.hover(move |style| {
                style
                    .bg(palette.trigger_hover_bg)
                    .text_color(palette.trigger_hover)
            });
        }

        let icon = if let Some(progress) = view.progress.as_ref() {
            div()
                .size(px(24.))
                .relative()
                .flex()
                .items_center()
                .justify_center()
                .child(update_progress_ring(progress.fraction))
                .child(update_download_icon(16., icon_color))
                .into_any_element()
        } else {
            update_download_icon(17., icon_color).into_any_element()
        };

        trigger.child(icon).when(view.dot, |trigger| {
            trigger.child(
                div()
                    .absolute()
                    .top(px(4.))
                    .right(px(4.))
                    .size(px(7.))
                    .rounded_full()
                    .bg(palette.dot)
                    .border_2()
                    .border_color(palette.dot_border),
            )
        })
    }

    /// Pass the trigger bounds to preserve its toggle when the panel is already open.
    /// The owner can focus the panel for Escape handling and restore the previous focus on close.
    pub fn render_update_popover(
        view: &UpdateView,
        dark: bool,
        trigger_bounds: Option<Bounds<Pixels>>,
        focus: &FocusHandle,
        on_action: Rc<dyn Fn(UpdateAction, &mut Window, &mut App)>,
    ) -> AnyElement {
        if !view.open {
            return Empty.into_any_element();
        }

        let palette = UpdatePalette::new(dark);
        let close_action = on_action.clone();

        let header = div()
            .min_h(px(66.))
            .flex()
            .items_start()
            .gap(px(12.))
            .px(px(16.))
            .pt(px(14.))
            .pb(px(11.))
            .child(
                div()
                    .size(px(38.))
                    .flex_none()
                    .rounded(px(12.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(linear_gradient(
                        145.,
                        linear_color_stop(rgb(0x68b7ff), 0.),
                        linear_color_stop(rgb(0x2b82f0), 1.),
                    ))
                    .shadow(vec![update_shadow(0x2881ef, 0.25, 7., 18.)])
                    .child(update_download_icon(17., rgb(0xffffff))),
            )
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.))
                    .pt(px(1.))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight(700.))
                            .line_height(px(20.))
                            .text_color(palette.text)
                            .child(view.title.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(10.))
                            .line_height(px(17.))
                            .text_color(palette.subtitle)
                            .child(view.subtitle.clone()),
                    ),
            )
            .child(
                div()
                    .id("app-update-close")
                    .focusable()
                    .size(px(24.))
                    .flex_none()
                    .rounded(px(7.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .text_size(px(18.))
                    .line_height(px(18.))
                    .text_color(palette.close)
                    .hover(move |style| {
                        style
                            .bg(palette.close_hover_bg)
                            .text_color(palette.close_hover)
                    })
                    .on_click({
                        let on_action = close_action.clone();
                        move |_, window, cx| {
                            on_action(UpdateAction::Dismiss, window, cx);
                        }
                    })
                    .child("×"),
            );

        let body = update_popover_body(view, palette);
        let actions = update_actions(view, palette, on_action.clone());

        let escape_action = on_action.clone();

        div()
            .id("app-update-popover")
            .track_focus(focus)
            .on_mouse_down_out(move |event, window, cx| {
                if !trigger_bounds.is_some_and(|bounds| bounds.contains(&event.position)) {
                    on_action(UpdateAction::Dismiss, window, cx);
                }
            })
            .on_key_down(move |event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "escape" {
                    escape_action(UpdateAction::Dismiss, window, cx);
                    cx.stop_propagation();
                }
            })
            .relative()
            .occlude()
            .w(px(350.))
            .overflow_hidden()
            .rounded(px(16.))
            .border_1()
            .border_color(palette.popover_border)
            .bg(palette.popover_bg)
            .shadow(vec![
                update_shadow(0x000000, if dark { 0.32 } else { 0.18 }, 24., 60.),
                update_shadow(0x000000, if dark { 0.16 } else { 0.08 }, 3., 10.),
            ])
            .flex()
            .flex_col()
            .text_color(palette.text)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .child(header)
            .child(body)
            .child(actions)
            .with_animation(
                "app-update-popover-in",
                appearance_animation(210),
                |popover, progress| popover.opacity(progress).top(px(-7. * (1. - progress))),
            )
            .into_any_element()
    }

    fn update_popover_body(view: &UpdateView, palette: UpdatePalette) -> Div {
        let mut body = div()
            .px(px(16.))
            .pb(px(14.))
            .text_size(px(11.))
            .line_height(px(16.5))
            .text_color(palette.body);

        if let Some(message) = view.message.clone() {
            if view.error {
                body = body.child(
                    div()
                        .mt(px(5.))
                        .p(px(10.))
                        .rounded(px(10.))
                        .bg(palette.error_bg)
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .text_size(px(9.))
                                .text_color(palette.error_hint)
                                .child(message),
                        ),
                );
            } else {
                body = body.child(div().mt(px(5.)).child(message));
            }
        }

        if let Some(notice) = view.notice {
            body = body.child(
                div()
                    .mt(px(8.))
                    .text_center()
                    .text_color(palette.waiting)
                    .child(notice),
            );
        }

        if !view.notes.is_empty() {
            body = body.child(div().mt(px(3.)).flex().flex_col().gap(px(4.)).children(
                view.notes.iter().map(|note| {
                    div()
                        .relative()
                        .pl(px(13.))
                        .child(
                            div()
                                .absolute()
                                .top(px(7.))
                                .left(px(0.))
                                .size(px(4.))
                                .rounded_full()
                                .bg(palette.note_dot),
                        )
                        .child(note.clone())
                }),
            ));
        }

        if let Some(progress) = view.progress.as_ref() {
            let fraction = progress.fraction.clamp(0., 1.);
            body = body.child(
                div()
                    .mt(px(3.))
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .text_size(px(10.))
                            .text_color(palette.download_text)
                            .child(progress.sizes.clone())
                            .child(progress.percent_label.clone()),
                    )
                    .child(
                        div()
                            .mt(px(8.))
                            .h(px(6.))
                            .overflow_hidden()
                            .rounded(px(99.))
                            .bg(palette.track)
                            .child(div().h_full().w(relative(fraction)).rounded(px(99.)).bg(
                                linear_gradient(
                                    90.,
                                    linear_color_stop(palette.bar_from, 0.),
                                    linear_color_stop(palette.bar_to, 1.),
                                ),
                            )),
                    )
                    .child(
                        div()
                            .mt(px(6.))
                            .text_size(px(9.))
                            .text_color(palette.download_rate)
                            .child(progress.rate.clone()),
                    ),
            );
        }

        if let Some((message, hint)) = view.active_sessions.as_ref() {
            body = body.child(
                div()
                    .mt(px(8.))
                    .p(px(10.))
                    .rounded(px(10.))
                    .bg(palette.session_bg)
                    .flex()
                    .gap(px(9.))
                    .child(
                        div()
                            .mt(px(5.))
                            .size(px(7.))
                            .flex_none()
                            .rounded_full()
                            .bg(palette.session_dot),
                    )
                    .child(
                        div()
                            .min_w(px(0.))
                            .flex_1()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(palette.session_text)
                                    .child(message.clone()),
                            )
                            .child(
                                div()
                                    .mt(px(2.))
                                    .text_size(px(9.))
                                    .line_height(px(13.05))
                                    .text_color(palette.session_hint)
                                    .child(*hint),
                            ),
                    ),
            );
        }

        if let Some(scheduled) = view.scheduled {
            body = body.child(
                div()
                    .mt(px(8.))
                    .text_center()
                    .text_color(palette.waiting)
                    .child(scheduled),
            );
        }

        body
    }

    fn update_actions(
        view: &UpdateView,
        palette: UpdatePalette,
        on_action: Rc<dyn Fn(UpdateAction, &mut Window, &mut App)>,
    ) -> Div {
        let mut rows = Vec::<AnyElement>::new();
        let mut pending_row = Vec::<AnyElement>::new();

        for (index, button) in view.buttons.iter().enumerate() {
            let element = update_action_button(index, button, palette, on_action.clone());
            if button.in_row {
                pending_row.push(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .child(element)
                        .into_any_element(),
                );
            } else {
                if !pending_row.is_empty() {
                    rows.push(
                        div()
                            .flex()
                            .gap(px(7.))
                            .children(pending_row.drain(..))
                            .into_any_element(),
                    );
                }
                rows.push(element.into_any_element());
            }
        }
        if !pending_row.is_empty() {
            rows.push(
                div()
                    .flex()
                    .gap(px(7.))
                    .children(pending_row)
                    .into_any_element(),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap(px(7.))
            .px(px(16.))
            .pt(px(12.))
            .pb(px(15.))
            .border_t(px(1.))
            .border_color(palette.divider)
            .children(rows)
    }

    fn update_action_button(
        index: usize,
        button: &UpdateButton,
        palette: UpdatePalette,
        on_action: Rc<dyn Fn(UpdateAction, &mut Window, &mut App)>,
    ) -> impl IntoElement {
        let quiet = matches!(button.style, UpdateButtonStyle::Quiet);
        let primary = matches!(button.style, UpdateButtonStyle::Primary);
        let action = button.action;
        let enabled = button.enabled;

        let mut element = div()
            .id(SharedString::from(format!("app-update-action-{index}")))
            .h(px(if quiet { 26. } else { 34. }))
            .px(px(12.))
            .rounded(px(9.))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(11.))
            .font_weight(FontWeight(if quiet { 550. } else { 650. }))
            .child(button.label)
            .when(!enabled, |element| element.opacity(0.55).cursor_default());

        if primary {
            element = element
                .text_color(rgb(0xffffff))
                .bg(linear_gradient(
                    180.,
                    linear_color_stop(palette.primary_from, 0.),
                    linear_color_stop(palette.primary_to, 1.),
                ))
                .shadow(vec![update_shadow(0x277eea, 0.2, 6., 16.)])
                .when(enabled, |element| {
                    element.hover(|style| style.opacity(0.96))
                });
        } else if quiet {
            element = element
                .text_color(palette.quiet_text)
                .when(enabled, |element| {
                    element.hover(move |style| style.bg(palette.button_bg))
                });
        } else {
            element = element
                .text_color(palette.button_text)
                .bg(palette.button_bg)
                .when(enabled, |element| {
                    element
                        .cursor_pointer()
                        .hover(move |style| style.bg(palette.button_hover))
                });
        }

        element.when(enabled, |element| {
            let on_action = on_action.clone();
            element
                .focusable()
                .cursor_pointer()
                .on_click(move |_, window, cx| on_action(action, window, cx))
        })
    }

    /// Completion toast. The caller chooses the placement; the frozen card is 280px
    /// minimum with the check badge, title and hint from the update model.
    pub fn render_update_toast(view: &UpdateView, dark: bool) -> impl IntoElement {
        let Some((message, hint)) = view.toast.as_ref() else {
            return Empty.into_any_element();
        };
        let palette = UpdatePalette::new(dark);

        div()
            .id("app-update-toast")
            .relative()
            .min_w(px(280.))
            .p(px(12.))
            .px(px(14.))
            .rounded(px(13.))
            .border_1()
            .border_color(palette.toast_border)
            .bg(palette.toast_bg)
            .shadow(vec![update_shadow(
                0x000000,
                if dark { 0.3 } else { 0.18 },
                20.,
                50.,
            )])
            .flex()
            .items_center()
            .gap(px(10.))
            .text_color(palette.text)
            .child(
                div()
                    .size(px(25.))
                    .flex_none()
                    .rounded_full()
                    .bg(palette.toast_check)
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(12.))
                    .font_weight(FontWeight(700.))
                    .text_color(rgb(0xffffff))
                    .child("✓"),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .text_size(px(11.))
                            .font_weight(FontWeight(650.))
                            .child(message.clone()),
                    )
                    .child(
                        div()
                            .mt(px(2.))
                            .text_size(px(9.))
                            .text_color(palette.toast_muted)
                            .child(*hint),
                    ),
            )
            .with_animation(
                "app-update-toast-in",
                appearance_animation(220),
                |toast, progress| toast.opacity(progress).top(px(8. * (1. - progress))),
            )
            .into_any_element()
    }

    fn appearance_animation(millis: u64) -> Animation {
        #[cfg(target_os = "macos")]
        let millis = if crate::macos::reduced_motion() {
            1
        } else {
            millis
        };
        Animation::new(Duration::from_millis(millis)).with_easing(ease_out_quint())
    }

    fn update_progress_ring(fraction: f32) -> impl IntoElement {
        let fraction = fraction.clamp(0., 1.);
        canvas(
            move |bounds, _, _| bounds,
            move |bounds, _, window, _| {
                let width = f32::from(bounds.size.width);
                let height = f32::from(bounds.size.height);
                let radius = (width.min(height) * 0.5) - 1.;
                if radius <= 0. {
                    return;
                }
                let center = bounds.center();
                update_paint_circle(window, center, px(radius), rgba(0x64789629));
                if fraction > 0. {
                    update_paint_progress(window, center, px(radius), fraction);
                }
            },
        )
        .absolute()
        .inset_0()
        .size_full()
    }

    fn update_paint_circle(
        window: &mut Window,
        center: Point<Pixels>,
        radius: Pixels,
        color: Rgba,
    ) {
        let mut path = PathBuilder::stroke(px(2.));
        path.move_to(point(center.x, center.y - radius));
        path.arc_to(
            point(radius, radius),
            px(0.),
            false,
            true,
            point(center.x + radius, center.y),
        );
        path.arc_to(
            point(radius, radius),
            px(0.),
            false,
            true,
            point(center.x, center.y + radius),
        );
        path.arc_to(
            point(radius, radius),
            px(0.),
            false,
            true,
            point(center.x - radius, center.y),
        );
        path.arc_to(
            point(radius, radius),
            px(0.),
            false,
            true,
            point(center.x, center.y - radius),
        );
        if let Ok(path) = path.build() {
            window.paint_path(path, color);
        }
    }

    fn update_paint_progress(
        window: &mut Window,
        center: Point<Pixels>,
        radius: Pixels,
        fraction: f32,
    ) {
        if fraction >= 0.999 {
            update_paint_circle(window, center, radius, rgb(0x4aa1ff));
            return;
        }
        let mut path = PathBuilder::stroke(px(2.));
        path.move_to(point(center.x, center.y - radius));
        let angle = -std::f32::consts::FRAC_PI_2 + fraction * std::f32::consts::TAU;
        let end = point(
            center.x + radius * angle.cos(),
            center.y + radius * angle.sin(),
        );
        path.arc_to(point(radius, radius), px(0.), fraction > 0.5, true, end);
        if let Ok(path) = path.build() {
            window.paint_path(path, rgb(0x4aa1ff));
        }
    }
}

pub use update_ui::{render_update_popover, render_update_toast, render_update_trigger};

#[cfg(test)]
mod tests {
    use super::*;
    use InterfaceLocale::{Chinese, English};
    use UpdateInstallMode::{Automatic, Manual};

    const NOW: Duration = Duration::from_secs(10);

    fn release(install_mode: UpdateInstallMode) -> UpdateRelease {
        UpdateRelease {
            version: "1.2.3".into(),
            install_mode,
            size_bytes: Some(9_600_000),
            release_day: Some((11, 21)),
            notes: ["One", "Two", "Three", "Four"].map(str::to_owned).to_vec(),
        }
    }

    fn state(status: UpdateStatus) -> UpdateState {
        UpdateState {
            current_version: "1.0.0".into(),
            status,
        }
    }

    fn control(status: UpdateStatus) -> UpdateControl {
        UpdateControl::new(state(status), None, NOW)
    }

    fn labels(view: &UpdateView) -> Vec<&'static str> {
        view.buttons.iter().map(|button| button.label).collect()
    }

    #[test]
    fn popover_and_toast_backgrounds_are_opaque_in_both_themes() {
        for dark in [false, true] {
            let (popover, toast) = super::update_ui::background_alpha_for_test(dark);
            assert_eq!(popover, 1.0, "popover dark={dark}");
            assert_eq!(toast, 1.0, "toast dark={dark}");
        }
    }

    #[test]
    fn idle_up_to_date_and_checking_use_frozen_copy_and_actions() {
        let mut idle = control(UpdateStatus::Idle);
        for (locale, label, title, version, button, close) in [
            (
                Chinese,
                "应用更新",
                "Matou 应用更新",
                "当前版本 1.0.0",
                "检查更新",
                "关闭更新浮层",
            ),
            (
                English,
                "App update",
                "Matou app update",
                "Current version 1.0.0",
                "Check for updates",
                "Close update popover",
            ),
        ] {
            let view = idle.view(locale, NOW);
            assert_eq!(
                (
                    &*view.label,
                    &*view.title,
                    &*view.subtitle,
                    view.close_label
                ),
                (label, title, version, close)
            );
            // Idle keeps the version only on the subtitle (single occurrence).
            assert_eq!(view.message.as_deref(), None);
            assert_eq!(labels(&view), [button]);
            assert!(!view.open && !view.highlight && !view.dot && view.buttons[0].enabled);
        }
        assert_eq!(idle.dispatch(UpdateAction::Check), None);
        idle.dispatch(UpdateAction::TogglePopover);
        assert_eq!(
            idle.dispatch(UpdateAction::Check),
            Some(UpdateEffect::Check)
        );

        let latest = control(UpdateStatus::NotAvailable);
        for (locale, title, message, button) in [
            (
                Chinese,
                "Matou 已是最新版本",
                "当前已是最新版本（1.0.0）",
                "检查更新",
            ),
            (
                English,
                "Matou is up to date",
                "Already on the latest version (1.0.0)",
                "Check for updates",
            ),
        ] {
            let view = latest.view(locale, NOW);
            assert_eq!(view.title, title);
            assert_eq!(view.message.as_deref(), Some(message));
            assert_eq!(labels(&view), [button]);
            assert!(!view.dot && !view.highlight && view.buttons[0].enabled);
        }

        let checking = control(UpdateStatus::Checking {
            retry_attempt: 0,
            max_retry_attempts: None,
        });
        let retrying = control(UpdateStatus::Checking {
            retry_attempt: 2,
            max_retry_attempts: Some(4),
        });
        for (locale, label, title, message, retry) in [
            (
                Chinese,
                "应用更新：正在检查",
                "正在检查更新",
                "正在检查云端是否有新版本…",
                "连接波动，正在自动重试（2/4）…",
            ),
            (
                English,
                "App update: checking",
                "Checking for updates",
                "Checking for a newer version…",
                "Connection is unstable, retrying automatically (2/4)…",
            ),
        ] {
            let view = checking.view(locale, NOW);
            assert_eq!((&*view.label, &*view.title), (label, title));
            assert_eq!(view.message.as_deref(), Some(message));
            assert_eq!(retrying.view(locale, NOW).message.as_deref(), Some(retry));
            assert!(view.buttons.is_empty() && !view.highlight && !view.dot);
        }
    }

    #[test]
    fn available_and_downloading_distinguish_modes_and_progress() {
        let automatic = control(UpdateStatus::Available(release(Automatic)));
        let manual = control(UpdateStatus::Available(release(Manual)));
        for (locale, label, title, subtitle, background, download, later, notice) in [
            (
                Chinese,
                "应用更新：发现 1.2.3",
                "Matou 1.2.3 可用",
                "稳定版 · 9.6 MB · 11/21",
                "后台下载",
                "下载更新",
                "稍后提醒",
                "当前体验包将通过应用内下载 DMG 更新。",
            ),
            (
                English,
                "App update: 1.2.3 available",
                "Matou 1.2.3 is available",
                "Stable · 9.6 MB · 11/21",
                "Download in background",
                "Download update",
                "Remind me later",
                "This build updates through an in-app DMG download.",
            ),
        ] {
            let view = automatic.view(locale, NOW);
            assert_eq!(
                (&*view.label, &*view.title, &*view.subtitle),
                (label, title, subtitle)
            );
            assert_eq!(labels(&view), [background, later]);
            assert!(view.open && view.highlight && view.dot && view.notice.is_none());
            assert_eq!(view.notes, ["One", "Two", "Three"]);
            assert!(view.buttons.iter().all(|button| button.enabled));
            let manual_view = manual.view_for_platform(locale, NOW, false);
            assert_eq!(labels(&manual_view), [download, later]);
            assert_eq!(manual_view.notice, Some(notice));
            let windows_view = manual.view_for_platform(locale, NOW, true);
            assert_eq!(
                labels(&windows_view),
                [
                    locale.text("下载安装包更新", "Download installer update"),
                    later
                ]
            );
            assert_eq!(
                windows_view.notice,
                Some(locale.text(
                    "当前体验包将通过应用内下载安装包更新。",
                    "This build updates by downloading an installer in the app.",
                ))
            );
        }

        let mut downloading = control(UpdateStatus::Downloading(
            release(Automatic),
            UpdateProgress {
                percent: 52.5,
                transferred_bytes: 5_000_000,
                total_bytes: 9_600_000,
                bytes_per_second: 1_250_000,
                remaining_seconds: Some(7),
            },
        ));
        downloading.dispatch(UpdateAction::TogglePopover);
        for (locale, label, title, percent, rate, background) in [
            (
                Chinese,
                "应用更新：下载中 53%",
                "正在后台下载",
                "53% · 约 7 秒",
                "1.3 MB/秒 · 可继续使用当前会话",
                "继续在后台下载",
            ),
            (
                English,
                "App update: downloading 53%",
                "Downloading in background",
                "53% · about 7s left",
                "1.3 MB/s · you can keep using your sessions",
                "Keep downloading in background",
            ),
        ] {
            let view = downloading.view(locale, NOW);
            assert_eq!(
                (&*view.label, &*view.title, &*view.subtitle),
                (label, title, "Matou 1.2.3 · 9.6 MB · 11/21")
            );
            assert!(view.highlight && !view.dot && view.notes.is_empty());
            assert_eq!(labels(&view), [background]);
            assert!(view.buttons[0].enabled);
            let progress = view.progress.unwrap();
            assert_eq!((progress.rounded_percent, progress.fraction), (53, 0.525));
            assert_eq!(
                (&*progress.sizes, &*progress.percent_label, &*progress.rate),
                ("5.0 MB / 9.6 MB", percent, rate)
            );
        }
        assert_eq!(downloading.dispatch(UpdateAction::Download), None);
    }

    #[test]
    fn downloaded_manual_opening_is_disabled_until_request_finishes() {
        let mut control = control(UpdateStatus::Downloaded(release(Manual)));
        control.set_active_sessions(3);
        for (locale, label, title, open, later) in [
            (
                Chinese,
                "应用更新：等待打开安装包",
                "DMG 已下载完成",
                "打开 DMG 安装",
                "稍后安装",
            ),
            (
                English,
                "App update: waiting to open installer",
                "DMG downloaded",
                "Open DMG installer",
                "Install later",
            ),
        ] {
            let view = control.view_for_platform(locale, NOW, false);
            assert_eq!((&*view.label, &*view.title), (label, title));
            assert!(view.dot && view.highlight && view.open && view.active_sessions.is_none());
            assert_eq!(labels(&view), [open, later]);
            assert!(view.buttons.iter().all(|button| button.enabled));
            let windows_view = control.view_for_platform(locale, NOW, true);
            assert_eq!(
                windows_view.title,
                locale.text("安装包已下载完成", "Installer downloaded")
            );
            assert_eq!(
                labels(&windows_view),
                [locale.text("打开安装包安装", "Open installer"), later]
            );
        }
        assert_eq!(
            control.dispatch(UpdateAction::Install),
            Some(UpdateEffect::Install)
        );
        assert_eq!(control.dispatch(UpdateAction::Install), None);
        for (locale, opening) in [(Chinese, "正在打开…"), (English, "Opening…")] {
            let buttons = control.view(locale, NOW).buttons;
            assert_eq!(buttons[0].label, opening);
            assert!(!buttons[0].enabled && buttons[1].enabled);
        }
        control.finish_install(true);
        assert_eq!(
            control.dispatch(UpdateAction::Install),
            Some(UpdateEffect::Install)
        );
        control.finish_install(false);
        control.accept(
            UpdateState {
                current_version: "1.2.2".into(),
                status: UpdateStatus::Error {
                    stage: UpdateErrorStage::Install,
                    message: "Could not open installer".into(),
                    manual_download_available: false,
                },
            },
            NOW,
        );
        assert!(control.view(English, NOW).error);
        assert_eq!(control.dispatch(UpdateAction::Install), None);
        assert_eq!(
            control.dispatch(UpdateAction::Check),
            Some(UpdateEffect::Check)
        );
    }

    #[test]
    fn automatic_wait_can_be_cancelled_and_installs_once_when_idle() {
        let mut control = control(UpdateStatus::Downloaded(release(Automatic)));
        for (locale, label, title, install, quit) in [
            (
                Chinese,
                "应用更新：等待安装",
                "更新已准备好",
                "重启并更新",
                "退出时安装",
            ),
            (
                English,
                "App update: waiting to install",
                "Update ready",
                "Install and restart",
                "Install on quit",
            ),
        ] {
            let view = control.view(locale, NOW);
            assert_eq!((&*view.label, &*view.title), (label, title));
            assert_eq!(labels(&view), [install, quit]);
            assert!(view.dot && view.highlight && view.active_sessions.is_none());
            assert!(view.buttons.iter().all(|button| button.enabled));
        }
        control.set_active_sessions(1);
        for (locale, active, keeps, schedule, restart, quit) in [
            (
                Chinese,
                "当前有 1 个活动会话",
                "空闲后更新会保留工作区、画布位置及会话恢复信息。",
                "空闲后自动更新",
                "立即重启并更新",
                "退出时安装",
            ),
            (
                English,
                "You have 1 active session",
                "Updating once idle keeps your workspaces, canvas positions and session recovery data.",
                "Update once idle",
                "Install and restart now",
                "Install on quit",
            ),
        ] {
            let view = control.view(locale, NOW);
            assert_eq!(
                view.active_sessions
                    .as_ref()
                    .map(|(label, note)| (label.as_str(), *note)),
                Some((active, keeps))
            );
            assert_eq!(labels(&view), [schedule, restart, quit]);
            assert!(!view.buttons[0].in_row && view.buttons[1].in_row && view.buttons[2].in_row);
        }

        control.dispatch(UpdateAction::SetWaitingForIdle(true));
        for (locale, scheduled, cancel) in [
            (Chinese, "已安排：空闲后自动更新", "取消空闲更新"),
            (English, "Scheduled: update once idle", "Cancel idle update"),
        ] {
            let view = control.view(locale, NOW);
            assert_eq!(view.scheduled, Some(scheduled));
            assert_eq!(view.buttons[0].label, cancel);
            assert_eq!(view.buttons[0].style, UpdateButtonStyle::Normal);
        }
        control.dispatch(UpdateAction::SetWaitingForIdle(false));
        assert_eq!(control.set_active_sessions(0), None);
        control.set_active_sessions(2);
        assert_eq!(
            control.view(English, NOW).active_sessions.unwrap().0,
            "You have 2 active sessions"
        );
        control.dispatch(UpdateAction::SetWaitingForIdle(true));
        control.dispatch(UpdateAction::Dismiss);
        assert_eq!(control.set_active_sessions(0), Some(UpdateEffect::Install));
        assert_eq!(control.set_active_sessions(0), None);
        for (locale, preparing) in [(Chinese, "正在准备更新…"), (English, "Preparing update…")]
        {
            let buttons = control.view(locale, NOW).buttons;
            assert_eq!(buttons[0].label, preparing);
            assert!(!buttons[0].enabled && buttons[1].enabled);
        }
        control.set_active_sessions(2);
        let view = control.view(English, NOW);
        assert!(!view.buttons[0].enabled && !view.buttons[1].enabled && view.buttons[2].enabled);
    }

    #[test]
    fn all_four_error_stages_and_manual_fallback_use_frozen_labels() {
        use UpdateErrorStage::*;
        for (stage, zh_title, en_title, zh_detail, en_detail) in [
            (
                Check,
                "更新检查失败",
                "Update check failed",
                "更新服务出现异常，请稍后重新检查。",
                "The update service ran into a problem. Please check again later.",
            ),
            (
                Download,
                "更新下载失败",
                "Update download failed",
                "更新文件下载中断，请检查网络后重试。",
                "The update download was interrupted. Check your network and try again.",
            ),
            (
                Verify,
                "安装包校验未通过",
                "Installer verification failed",
                "当前安装包缺少 Apple 发布签名，改用应用内 DMG 下载继续更新。",
                "This installer has no Apple release signature. Matou will continue with the in-app DMG download.",
            ),
            (
                Install,
                "更新安装失败",
                "Update installation failed",
                "更新安装前的会话保存或应用退出过程出现异常，请重新尝试。",
                "Saving sessions or quitting the app before the install went wrong. Please try again.",
            ),
        ] {
            for fallback in [false, true] {
                let control = control(UpdateStatus::Error {
                    stage,
                    message: "unclassified".into(),
                    manual_download_available: fallback,
                });
                for (locale, title, prefix, detail, dmg, recheck) in [
                    (
                        Chinese,
                        zh_title,
                        "应用更新：",
                        zh_detail,
                        "下载 DMG 更新",
                        "重新检查",
                    ),
                    (
                        English,
                        en_title,
                        "App update: ",
                        en_detail,
                        "Download DMG update",
                        "Check again",
                    ),
                ] {
                    let view = control.view_for_platform(locale, NOW, false);
                    assert_eq!(view.label, format!("{prefix}{title}"));
                    assert_eq!(view.title, title);
                    assert_eq!(view.message.as_deref(), Some(detail));
                    assert!(view.error && !view.dot && !view.highlight);
                    assert_eq!(
                        labels(&view),
                        if fallback {
                            vec![dmg, recheck]
                        } else {
                            vec![recheck]
                        }
                    );
                    let windows_view = control.view_for_platform(locale, NOW, true);
                    assert!(!windows_view.message.as_deref().unwrap().contains("DMG"));
                    if fallback {
                        assert_eq!(
                            labels(&windows_view),
                            [
                                locale.text("下载安装包更新", "Download installer update"),
                                recheck
                            ]
                        );
                    }
                    assert!(view.buttons.iter().all(|button| button.enabled));
                }
            }
        }
    }

    #[test]
    fn diagnostics_classify_specific_failures_before_generic_copy() {
        use UpdateErrorStage::Check;
        for (message, zh, en) in [
            (
                "getaddrinfo ENOTFOUND",
                "更新服务器域名解析失败，请检查网络或 DNS 设置。",
                "The update server address could not be resolved. Check your network or DNS settings.",
            ),
            (
                "fetch timed out",
                "连接更新服务器超时，请检查网络后重试。",
                "The connection to the update server timed out. Check your network and try again.",
            ),
            (
                "network is unreachable",
                "当前设备尚未接入网络，请恢复网络后重试。",
                "This device is not connected to a network. Reconnect and try again.",
            ),
            (
                "ECONNREFUSED",
                "更新服务器暂时拒绝连接，请稍后重试。",
                "The update server refused the connection. Please try again shortly.",
            ),
            (
                "socket hang up",
                "下载连接被中途断开，应用已保留当前版本，请重新尝试。",
                "The download connection was cut off. Matou kept the current version, please try again.",
            ),
            (
                "TLS alert",
                "更新服务器的安全连接校验异常，请检查系统时间后重试。",
                "The secure connection to the update server failed verification. Check the system clock and try again.",
            ),
            (
                "Bad status code: 503",
                "更新服务器返回 HTTP 503，请稍后重试。",
                "The update server returned HTTP 503. Please try again later.",
            ),
            (
                "unable to read latest-linux.yml",
                "更新信息格式异常，当前版本保持不变，请稍后重试。",
                "The update manifest is malformed. The current version is unchanged, please try again later.",
            ),
            (
                "fetch didn't complete",
                "暂时没有连接到更新服务器，请检查网络后重试。",
                "Matou could not reach the update server. Check your network and try again.",
            ),
        ] {
            assert_eq!(friendly_error(Check, message, Chinese, false), zh);
            assert_eq!(friendly_error(Check, message, English, false), en);
        }
    }

    #[test]
    fn toast_prefers_current_version_and_expires_after_five_seconds() {
        let mut control = UpdateControl::new(state(UpdateStatus::Idle), Some("0.9.0"), NOW);
        assert_eq!(
            control.view(Chinese, NOW).toast,
            Some(("Matou 已更新至 1.0.0".into(), "工作空间与会话已恢复"))
        );
        assert_eq!(
            control.view(English, NOW).toast,
            Some((
                "Matou updated to 1.0.0".into(),
                "Workspaces and sessions restored"
            ))
        );
        control.accept(
            state(UpdateStatus::Available(release(Automatic))),
            NOW + Duration::from_secs(1),
        );
        assert!(
            control
                .view(English, NOW + Duration::from_secs(4))
                .toast
                .is_some()
        );
        assert!(
            control
                .view(English, NOW + UPDATED_TOAST_DURATION)
                .toast
                .is_none()
        );
        assert!(control.view(English, NOW).open);
        assert_eq!(control.current_version(), "1.0.0");

        let first_launch = UpdateControl::new(state(UpdateStatus::Idle), None, NOW);
        assert!(first_launch.view(English, NOW).toast.is_none());
        let same_version = UpdateControl::new(state(UpdateStatus::Idle), Some("1.0.0"), NOW);
        assert!(same_version.view(English, NOW).toast.is_none());
    }
}
