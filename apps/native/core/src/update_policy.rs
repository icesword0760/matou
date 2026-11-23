use std::cmp::Ordering;
use std::collections::HashSet;
use std::fmt;

use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256, Sha512};
use url::Url;

pub const DEFAULT_UPDATE_BASE_URL: &str =
    "https://github.com/icesword0760/matou/releases/latest/download";

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Darwin,
    Win32,
    Linux,
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Architecture {
    #[default]
    X64,
    Arm64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PolicyError {
    pub code: &'static str,
    pub message: String,
}

impl fmt::Display for PolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PolicyError {}

fn invalid_url() -> PolicyError {
    PolicyError {
        code: "ERR_INVALID_URL",
        message: "Invalid URL".into(),
    }
}

pub fn parse_packaged_base_url(config: &str) -> Option<&str> {
    let generic = config.lines().any(|line| {
        line.strip_prefix("provider:")
            .is_some_and(|value| value.trim() == "generic")
    });
    if !generic {
        return None;
    }
    config
        .lines()
        .filter_map(|line| line.strip_prefix("url:"))
        .map(str::trim)
        .find(|value| !value.is_empty())
}

#[derive(Clone, Debug, Serialize)]
pub struct FeedConfiguration {
    pub provider: &'static str,
    pub url: String,
    pub channel: String,
}

impl FeedConfiguration {
    pub fn resolve(
        override_url: Option<&str>,
        packaged_config: Option<&str>,
        channel: Option<&str>,
    ) -> Self {
        let url = override_url
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .or_else(|| packaged_config.and_then(parse_packaged_base_url))
            .unwrap_or(DEFAULT_UPDATE_BASE_URL);
        Self {
            provider: "generic",
            url: url.into(),
            channel: channel.unwrap_or("stable").into(),
        }
    }

    pub fn validate(&self) -> Result<(), PolicyError> {
        self.base_url().map(|_| ())
    }

    fn base_url(&self) -> Result<Url, PolicyError> {
        let url = Url::parse(&self.url).map_err(|_| invalid_url())?;
        if url.cannot_be_a_base() {
            return Err(invalid_url());
        }
        Ok(url)
    }

    pub fn channel_filename(&self, platform: Platform, arch: Architecture) -> String {
        let suffix = match (platform, arch) {
            (Platform::Darwin, _) => "-mac",
            (Platform::Win32, _) => "",
            (Platform::Linux, Architecture::X64) => "-linux",
            (Platform::Linux, Architecture::Arm64) => "-linux-arm64",
        };
        format!("{}{suffix}.yml", self.channel)
    }

    pub fn manifest_url(
        &self,
        platform: Platform,
        arch: Architecture,
        unix_ms: u64,
    ) -> Result<Url, PolicyError> {
        let mut base = self.base_url()?;
        if !base.path().ends_with('/') {
            base.path_segments_mut()
                .map_err(|_| invalid_url())?
                .push("");
        }
        let mut url = base
            .join(&self.channel_filename(platform, arch))
            .map_err(|_| invalid_url())?;
        if let Some(query) = base.query().filter(|query| !query.is_empty()) {
            url.set_query(Some(query));
        } else {
            url.query_pairs_mut()
                .append_pair("noCache", &base32(unix_ms));
        }
        Ok(url)
    }
}

fn base32(mut value: u64) -> String {
    const DIGITS: &[u8; 32] = b"0123456789abcdefghijklmnopqrstuv";
    let mut digits = Vec::new();
    loop {
        digits.push(char::from(DIGITS[(value % 32) as usize]));
        value /= 32;
        if value == 0 {
            return digits.into_iter().rev().collect();
        }
    }
}

pub fn update_checks_enabled(is_packaged: bool, disable_auto_update: Option<&str>) -> bool {
    is_packaged && disable_auto_update != Some("1")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UpdatePreferences {
    pub allow_downgrade: bool,
    pub allow_prerelease: bool,
    pub auto_download: bool,
    pub auto_install_on_app_quit: bool,
}

impl UpdatePreferences {
    pub fn new(current: &CurrentVersion) -> Self {
        Self {
            allow_downgrade: false,
            allow_prerelease: current.is_prerelease(),
            auto_download: true,
            auto_install_on_app_quit: true,
        }
    }

    pub fn start(&mut self) {
        self.auto_download = false;
    }
}

#[derive(Debug)]
struct Version<'a> {
    core: [u64; 3],
    prerelease: Option<&'a str>,
}

fn numeric(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn no_leading_zero(value: &str) -> bool {
    value.len() == 1 || !value.starts_with('0')
}

fn valid_identifiers(value: &str, prerelease: bool) -> bool {
    value.split('.').all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
            && (!prerelease || !numeric(part) || no_leading_zero(part))
    })
}

impl<'a> Version<'a> {
    fn parse(value: &'a str) -> Option<Self> {
        let value = value.trim();
        if value.len() > 256 {
            return None;
        }
        let value = value.strip_prefix('v').unwrap_or(value);
        let (without_build, build) = value
            .split_once('+')
            .map_or((value, None), |(left, right)| (left, Some(right)));
        if build.is_some_and(|build| !valid_identifiers(build, false)) {
            return None;
        }
        let (core, prerelease) = without_build
            .split_once('-')
            .map_or((without_build, None), |(left, right)| (left, Some(right)));
        if prerelease.is_some_and(|value| !valid_identifiers(value, true)) {
            return None;
        }
        let mut parts = core.split('.');
        let mut version = [0; 3];
        for item in &mut version {
            let part = parts.next()?;
            if !numeric(part) || !no_leading_zero(part) {
                return None;
            }
            *item = part.parse().ok()?;
            if *item > 9_007_199_254_740_991 {
                return None;
            }
        }
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            core: version,
            prerelease,
        })
    }

    fn precedence(&self, other: &Self) -> Ordering {
        self.core
            .cmp(&other.core)
            .then_with(|| match (self.prerelease, other.prerelease) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(left), Some(right)) => {
                    let left = left.split('.');
                    let right = right.split('.');
                    for (a, b) in left.clone().zip(right.clone()) {
                        let order = match (numeric(a), numeric(b)) {
                            (true, true) => a.len().cmp(&b.len()).then_with(|| a.cmp(b)),
                            (true, false) => Ordering::Less,
                            (false, true) => Ordering::Greater,
                            (false, false) => a.cmp(b),
                        };
                        if order != Ordering::Equal {
                            return order;
                        }
                    }
                    left.count().cmp(&right.count())
                }
            })
    }
}

#[derive(Debug)]
pub struct CurrentVersion(String);

impl CurrentVersion {
    pub fn parse(value: impl Into<String>) -> Result<Self, PolicyError> {
        let value = value.into();
        Version::parse(&value).ok_or_else(|| PolicyError {
            code: "ERR_UPDATER_INVALID_VERSION",
            message: format!("App version is not a valid semver version: \"{value}\""),
        })?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_prerelease(&self) -> bool {
        self.parsed().prerelease.is_some()
    }

    pub fn offers(&self, candidate: &str) -> Result<bool, PolicyError> {
        let candidate = Version::parse(candidate).ok_or_else(|| PolicyError {
            code: "ERR_UPDATER_INVALID_VERSION",
            message: format!(
                "This file could not be downloaded, or the latest version (from update server) does not have a valid semver version: \"{candidate}\""
            ),
        })?;
        Ok(candidate.precedence(&self.parsed()) == Ordering::Greater)
    }

    fn parsed(&self) -> Version<'_> {
        Version::parse(self.as_str()).expect("current version was validated at construction")
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InstallMode {
    #[default]
    Automatic,
    Manual,
}

#[derive(Deserialize)]
pub struct SignatureInspection {
    pub status: Option<i32>,
    pub output: String,
}

pub fn install_mode(
    platform: Platform,
    is_packaged: bool,
    inspect: impl FnOnce() -> SignatureInspection,
) -> InstallMode {
    if !matches!(platform, Platform::Darwin) || !is_packaged {
        return InstallMode::Automatic;
    }
    let signature = inspect();
    let has_authority = signature.output.lines().any(|line| {
        line.strip_prefix("Authority=")
            .is_some_and(|value| !value.is_empty())
    });
    let team = signature.output.lines().find_map(|line| {
        line.strip_prefix("TeamIdentifier=")
            .filter(|value| !value.is_empty())
            .map(str::trim)
    });
    if signature.status == Some(0)
        && has_authority
        && team.is_some_and(|team| !team.is_empty() && team != "not set")
    {
        InstallMode::Automatic
    } else {
        InstallMode::Manual
    }
}

pub fn active_session_count<'a>(
    sessions: impl IntoIterator<Item = (&'a str, &'a str, bool)>,
) -> usize {
    sessions
        .into_iter()
        .filter(|&(_, status, archived)| {
            !archived && matches!(status, "starting" | "running" | "needs-input")
        })
        .map(|(id, _, _)| id)
        .collect::<HashSet<_>>()
        .len()
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
pub enum UpdateLocale {
    #[default]
    #[serde(rename = "zh-CN")]
    Chinese,
    #[serde(rename = "en")]
    English,
}

impl UpdateLocale {
    pub fn text(self, chinese: &'static str, english: &'static str) -> &'static str {
        match self {
            Self::Chinese => chinese,
            Self::English => english,
        }
    }

    pub fn active_sessions(self, count: usize) -> String {
        match self {
            Self::Chinese => format!("当前有 {count} 个活动会话"),
            Self::English if count == 1 => "You have 1 active session".into(),
            Self::English => format!("You have {count} active sessions"),
        }
    }

    pub fn idle_update_keeps_state(self) -> &'static str {
        self.text(
            "空闲后更新会保留工作区、画布位置及会话恢复信息。",
            "Updating once idle keeps your workspaces, canvas positions and session recovery data.",
        )
    }

    pub fn idle_update_scheduled(self) -> &'static str {
        self.text("已安排：空闲后自动更新", "Scheduled: update once idle")
    }

    pub fn updated_to(self, version: &str) -> String {
        match self {
            Self::Chinese => format!("Matou 已更新至 {version}"),
            Self::English => format!("Matou updated to {version}"),
        }
    }

    pub fn download_http(self, status: u16) -> String {
        match self {
            Self::Chinese => format!("更新文件下载失败（HTTP {status}）"),
            Self::English => format!("Update download failed (HTTP {status})"),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ErrorStage {
    Check,
    Download,
    Verify,
    Install,
}

#[derive(Debug, Serialize)]
pub struct ErrorDialog {
    pub title: &'static str,
    pub error: String,
    pub actions: Vec<&'static str>,
}

impl ErrorStage {
    pub fn title(self, locale: UpdateLocale) -> &'static str {
        match self {
            Self::Check => locale.text("更新检查失败", "Update check failed"),
            Self::Download => locale.text("更新下载失败", "Update download failed"),
            Self::Verify => locale.text("安装包校验未通过", "Installer verification failed"),
            Self::Install => locale.text("更新安装失败", "Update installation failed"),
        }
    }

    pub fn dialog(
        self,
        message: &str,
        has_manual_fallback: bool,
        locale: UpdateLocale,
    ) -> ErrorDialog {
        let mut actions = Vec::new();
        if has_manual_fallback {
            actions.push(locale.text("下载 DMG 更新", "Download DMG update"));
        }
        actions.push(locale.text("重新检查", "Check again"));
        ErrorDialog {
            title: self.title(locale),
            error: self.friendly_message(message, locale),
            actions,
        }
    }

    fn friendly_message(self, message: &str, locale: UpdateLocale) -> String {
        if self == Self::Verify {
            return locale.text(
                "当前安装包缺少 Apple 发布签名，改用应用内 DMG 下载继续更新。",
                "This installer has no Apple release signature. Matou will continue with the in-app DMG download.",
            ).into();
        }
        if self == Self::Install {
            return locale.text(
                "更新安装前的会话保存或应用退出过程出现异常，请重新尝试。",
                "Saving sessions or quitting the app before the install went wrong. Please try again.",
            ).into();
        }
        let message = message.to_ascii_lowercase();
        let has = |terms: &[&str]| terms.iter().any(|term| message.contains(term));
        let words = || message.split(|c: char| !c.is_ascii_alphanumeric() && c != '_');
        let text = if has(&[
            "enotfound",
            "eai_again",
            "err_name_not_resolved",
            "getaddrinfo",
        ]) || words().any(|word| word == "dns")
        {
            locale.text("更新服务器域名解析失败，请检查网络或 DNS 设置。", "The update server address could not be resolved. Check your network or DNS settings.")
        } else if has(&["etimedout", "err_timed_out"])
            || message
                .split_whitespace()
                .collect::<Vec<_>>()
                .windows(2)
                .any(|pair| {
                    (pair[0].ends_with("time") || pair[0].ends_with("timed"))
                        && pair[1].starts_with("out")
                })
        {
            locale.text(
                "连接更新服务器超时，请检查网络后重试。",
                "The connection to the update server timed out. Check your network and try again.",
            )
        } else if has(&[
            "enetunreach",
            "err_internet_disconnected",
            "network is unreachable",
        ]) {
            locale.text(
                "当前设备尚未接入网络，请恢复网络后重试。",
                "This device is not connected to a network. Reconnect and try again.",
            )
        } else if has(&["econnrefused", "err_connection_refused"]) {
            locale.text(
                "更新服务器暂时拒绝连接，请稍后重试。",
                "The update server refused the connection. Please try again shortly.",
            )
        } else if has(&["econnreset", "err_connection_reset", "socket hang up"]) {
            locale.text("下载连接被中途断开，应用已保留当前版本，请重新尝试。", "The download connection was cut off. Matou kept the current version, please try again.")
        } else if has(&["cert_", "ssl", "tls", "certificate"]) {
            locale.text("更新服务器的安全连接校验异常，请检查系统时间后重试。", "The secure connection to the update server failed verification. Check the system clock and try again.")
        } else if let Some(status) = http_status(&message) {
            return match locale {
                UpdateLocale::Chinese => format!("更新服务器返回 HTTP {status}，请稍后重试。"),
                UpdateLocale::English => {
                    format!("The update server returned HTTP {status}. Please try again later.")
                }
            };
        } else if has(&["stable-mac.yml", "yaml", "parse"])
            || message.split_whitespace().any(|word| {
                word.split_once("latest")
                    .is_some_and(|(_, suffix)| suffix.contains(".yml"))
            })
        {
            locale.text("更新信息格式异常，当前版本保持不变，请稍后重试。", "The update manifest is malformed. The current version is unchanged, please try again later.")
        } else if has(&["network", "server", "fetch", "econn", "timeout"]) {
            locale.text(
                "暂时没有连接到更新服务器，请检查网络后重试。",
                "Matou could not reach the update server. Check your network and try again.",
            )
        } else if self == Self::Download {
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
}

fn http_status(message: &str) -> Option<&str> {
    message
        .match_indices(|c| c == 'h' || c == 's')
        .find_map(|(index, _)| {
            let tail = message[index..]
                .strip_prefix("http")
                .or_else(|| message[index..].strip_prefix("status"))?;
            let tail = tail.trim_start_matches(|c: char| !c.is_ascii_digit());
            let status = tail.get(..3)?;
            numeric(status).then_some(status)
        })
}

pub struct DownloadChecksums {
    sha256: Sha256,
    sha512: Sha512,
}

impl Default for DownloadChecksums {
    fn default() -> Self {
        Self {
            sha256: Sha256::new(),
            sha512: Sha512::new(),
        }
    }
}

impl DownloadChecksums {
    pub fn update(&mut self, bytes: &[u8]) {
        self.sha256.update(bytes);
        self.sha512.update(bytes);
    }

    pub fn matches(self, sha256: Option<&str>, sha512: Option<&str>) -> bool {
        sha256.is_none_or(|expected| {
            format!("{:x}", self.sha256.finalize()).eq_ignore_ascii_case(expected)
        }) && sha512.is_none_or(|expected| STANDARD.encode(self.sha512.finalize()) == expected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_feed_targets_github_release_channel_assets() {
        let feed = FeedConfiguration::resolve(None, None, None);
        for (platform, arch, asset) in [
            (Platform::Darwin, Architecture::Arm64, "stable-mac.yml"),
            (Platform::Win32, Architecture::X64, "stable.yml"),
        ] {
            assert_eq!(
                feed.manifest_url(platform, arch, 42).unwrap().as_str(),
                format!(
                    "https://github.com/icesword0760/matou/releases/latest/download/{asset}?noCache=1a"
                )
            );
        }
    }

    #[test]
    fn update_policy_semver_precedence() {
        let ordered = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.10",
            "1.0.0-rc.1",
            "1.0.0",
        ];
        for (i, left) in ordered.into_iter().enumerate() {
            let current = CurrentVersion::parse(left).unwrap();
            for (j, right) in ordered.into_iter().enumerate() {
                assert_eq!(current.offers(right).unwrap(), j > i, "{left}, {right}");
            }
        }
        let current = CurrentVersion::parse("v1.0.0+local").unwrap();
        assert!(!current.offers("1.0.0+feed.01").unwrap());
        for invalid in ["1.0", "1.0.0+", "01.0.0", "1.0.0-01", "1.0.0-α"] {
            assert!(current.offers(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn update_policy_filenames_and_escaped_base_path() {
        let feed =
            FeedConfiguration::resolve(Some("https://example.invalid/a%20b?x=1"), None, None);
        for (platform, arch, expected) in [
            (Platform::Darwin, Architecture::X64, "stable-mac.yml"),
            (Platform::Darwin, Architecture::Arm64, "stable-mac.yml"),
            (Platform::Win32, Architecture::X64, "stable.yml"),
            (Platform::Win32, Architecture::Arm64, "stable.yml"),
            (Platform::Linux, Architecture::X64, "stable-linux.yml"),
            (
                Platform::Linux,
                Architecture::Arm64,
                "stable-linux-arm64.yml",
            ),
        ] {
            assert_eq!(feed.channel_filename(platform, arch), expected);
            assert_eq!(
                feed.manifest_url(platform, arch, 0).unwrap().as_str(),
                format!("https://example.invalid/a%20b/{expected}?x=1")
            );
        }
    }
}
