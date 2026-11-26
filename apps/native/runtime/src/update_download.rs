use std::future::Future;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use anyhow::Context as _;
use matou_native_core::update_policy::{
    Architecture, CurrentVersion, DownloadChecksums, ErrorDialog, ErrorStage, FeedConfiguration,
    InstallMode, Platform, PolicyError, UpdateLocale, UpdatePreferences,
};
use percent_encoding::percent_decode_str;
use regex::Regex;
use serde::{Deserialize, Serialize};
use url::Url;

mod windows;

pub const CHECK_RETRY_DELAYS_MS: &[u64] = &[600, 1_600];
const PROGRESS_INTERVAL_MS: u64 = 100;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub version: String,
    pub release_date: Option<String>,
    pub release_notes: Option<ManifestNotes>,
    #[serde(default)]
    pub files: Vec<ManifestFile>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum ManifestNotes {
    Text(String),
    Entries(Vec<ManifestNote>),
}

#[derive(Clone, Debug, Deserialize)]
pub struct ManifestNote {
    pub note: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ManifestFile {
    pub url: Option<String>,
    pub size: Option<u64>,
    pub sha256: Option<String>,
    pub sha512: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub version: String,
    pub release_notes: Vec<String>,
    pub install_mode: InstallMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_date: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_download_url: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ManualDownload {
    pub url: Url,
    pub size: Option<u64>,
    pub expected_sha256: Option<String>,
    pub expected_sha512: Option<String>,
}

fn resolve_manual_download(
    files: &[ManifestFile],
    feed: &FeedConfiguration,
    extension: &str,
) -> Option<ManualDownload> {
    let suffix = format!(".{extension}");
    let file = files.iter().find(|file| {
        file.url.as_deref().is_some_and(|value| {
            let path = value.split(['?', '#']).next().unwrap_or_default();
            path.to_ascii_lowercase().ends_with(&suffix)
        })
    })?;
    let base = Url::parse(&format!("{}/", feed.url.trim_end_matches('/'))).ok()?;
    Some(ManualDownload {
        url: base.join(file.url.as_deref()?).ok()?,
        size: file.size,
        expected_sha256: file.sha256.clone().filter(|hash| !hash.is_empty()),
        expected_sha512: file.sha512.clone().filter(|hash| !hash.is_empty()),
    })
}

fn resolve_windows_download(
    manifest: &Manifest,
    feed: &FeedConfiguration,
    arch: Architecture,
) -> Result<Option<ManualDownload>, UpdateError> {
    let base = feed.manifest_url(Platform::Win32, arch, 0)?;
    let matching = manifest.files.iter().filter_map(|file| {
        let url = base.join(file.url.as_deref()?).ok()?;
        let name = percent_decode_str(url.path_segments()?.next_back()?)
            .decode_utf8()
            .ok()?;
        let priority = windows::installer_name_priority(&name, &manifest.version, arch)?;
        Some((priority, url, file))
    });
    let invalid = || UpdateError {
        code: Some("ERR_UPDATER_INVALID_INSTALLER"),
        message:
            "The update manifest has no matching Windows installer with a valid SHA-512 checksum"
                .into(),
    };
    let mut best = None;
    for (priority, url, file) in matching {
        match &best {
            None => best = Some((priority, url, file)),
            Some((previous, _, _)) if priority > *previous => {
                best = Some((priority, url, file));
            }
            _ => {}
        }
    }
    let (_, url, file) = best.ok_or_else(invalid)?;
    let expected_sha512 = file
        .sha512
        .as_deref()
        .filter(|value| windows::valid_sha512(value))
        .ok_or_else(invalid)?;
    Ok(Some(ManualDownload {
        url,
        size: file.size,
        expected_sha256: file.sha256.clone().filter(|hash| !hash.is_empty()),
        expected_sha512: Some(expected_sha512.to_owned()),
    }))
}

fn normalize_notes(notes: Option<ManifestNotes>) -> Vec<String> {
    static BLOCK_TAGS: OnceLock<Regex> = OnceLock::new();
    static TAGS: OnceLock<Regex> = OnceLock::new();
    static SPACE: OnceLock<Regex> = OnceLock::new();
    static AMPERSAND: OnceLock<Regex> = OnceLock::new();
    let blocks = BLOCK_TAGS.get_or_init(|| Regex::new(r"(?i)</?(?:p|div|li|br)[^>]*>").unwrap());
    let tags = TAGS.get_or_init(|| Regex::new(r"<[^>]+>").unwrap());
    let space = SPACE.get_or_init(|| Regex::new(r"(?i)&nbsp;").unwrap());
    let ampersand = AMPERSAND.get_or_init(|| Regex::new(r"(?i)&amp;").unwrap());
    let notes = match notes {
        Some(ManifestNotes::Text(text)) => vec![text],
        Some(ManifestNotes::Entries(entries)) => {
            entries.into_iter().map(|entry| entry.note).collect()
        }
        None => return Vec::new(),
    };
    let mut lines = Vec::new();
    for note in notes {
        let note = blocks.replace_all(&note, "\n");
        let note = tags.replace_all(&note, "");
        let note = space.replace_all(&note, " ");
        let note = ampersand.replace_all(&note, "&");
        lines.extend(
            note.split('\n')
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(String::from),
        );
    }
    lines
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub percent: f64,
    pub transferred_bytes: u64,
    pub total_bytes: u64,
    pub bytes_per_second: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remaining_seconds: Option<u64>,
}

impl Progress {
    pub fn new(
        percent: f64,
        transferred_bytes: u64,
        total_bytes: u64,
        bytes_per_second: u64,
    ) -> Self {
        Self {
            percent,
            transferred_bytes,
            total_bytes,
            bytes_per_second,
            remaining_seconds: (bytes_per_second != 0).then(|| {
                (total_bytes.saturating_sub(transferred_bytes) as f64 / bytes_per_second as f64)
                    .round() as u64
            }),
        }
    }

    fn streaming(bytes: u64, total: u64, elapsed_ms: u64, complete: bool) -> Self {
        let percent = if complete {
            100.0
        } else if total > 0 {
            (bytes as f64 / total as f64 * 100.0).min(100.0)
        } else {
            0.0
        };
        let rate = (bytes as f64 / (elapsed_ms.max(1) as f64 / 1_000.0)).round() as u64;
        Self::new(
            percent,
            bytes,
            if complete && total == 0 { bytes } else { total },
            rate,
        )
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateState {
    pub current_version: String,
    #[serde(flatten)]
    pub phase: UpdatePhase,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(
    tag = "status",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum UpdatePhase {
    Idle,
    Checking {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_attempt: Option<usize>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_retry_attempts: Option<usize>,
    },
    Available {
        #[serde(flatten)]
        release: Release,
    },
    NotAvailable,
    Downloading {
        #[serde(flatten)]
        release: Release,
        progress: Progress,
    },
    Downloaded {
        #[serde(flatten)]
        release: Release,
    },
    Error {
        error_message: String,
        error_stage: ErrorStage,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        manual_download_url: Option<String>,
    },
}

impl UpdatePhase {
    fn release(&self) -> Option<&Release> {
        match self {
            Self::Available { release }
            | Self::Downloading { release, .. }
            | Self::Downloaded { release } => Some(release),
            _ => None,
        }
    }
}

impl UpdateState {
    pub fn error_dialog(&self, locale: UpdateLocale) -> Option<ErrorDialog> {
        match &self.phase {
            UpdatePhase::Error {
                error_message,
                error_stage,
                manual_download_url,
                ..
            } => Some(error_stage.dialog(error_message, manual_download_url.is_some(), locale)),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckResult {
    pub is_update_available: bool,
    pub update_info: CheckVersion,
}

#[derive(Clone, Debug, Serialize)]
pub struct CheckVersion {
    pub version: String,
}

#[derive(Debug, Serialize)]
pub struct UpdateError {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<&'static str>,
    pub message: String,
}

impl From<PolicyError> for UpdateError {
    fn from(error: PolicyError) -> Self {
        Self {
            code: Some(error.code),
            message: error.message,
        }
    }
}

impl From<String> for UpdateError {
    fn from(message: String) -> Self {
        Self {
            code: None,
            message,
        }
    }
}

pub struct DownloadResponse {
    pub status: u16,
    pub content_length: Option<u64>,
    pub body: Option<Box<dyn Read + Send>>,
}

#[derive(Debug)]
pub enum DownloadError {
    Rejected(String),
    UpdaterEvent(String),
    Verification(String),
}

/// The owner supplies fetch and automatic-updater adapters on an appropriate worker.
pub trait UpdateFetcher {
    fn check(&self, url: &Url) -> Result<Manifest, UpdateError>;
    fn installer_extension(&self) -> &'static str {
        "dmg"
    }
    fn fetch(&self, request: &ManualDownload) -> io::Result<DownloadResponse>;
    fn download_automatic(
        &self,
        release: &Release,
        on_progress: &mut dyn FnMut(Progress),
    ) -> Result<(), DownloadError>;
}

pub trait PendingUpdateFile: Write {
    fn verify_windows_installer(&mut self, arch: Architecture, version: &str) -> io::Result<()>;
    fn commit(self) -> io::Result<PathBuf>;
}

pub trait UpdateFileWriter {
    /// Return a staging file that discards its partial data when dropped.
    fn create(&self, filename: &str) -> io::Result<impl PendingUpdateFile>;
}

pub trait UpdateInstaller {
    fn prepare(&self) -> impl Future<Output = Result<(), String>>;
    fn quit_and_install(&self, silent: bool, relaunch: bool) -> Result<(), String>;
    fn open(&self, path: &Path) -> impl Future<Output = Result<(), String>>;
}

pub struct LocalUpdateFiles {
    directory: PathBuf,
}

impl LocalUpdateFiles {
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }
}

struct LocalPendingFile {
    writer: Option<std::fs::File>,
    staging: tempfile::TempPath,
    destination: PathBuf,
}

impl LocalPendingFile {
    fn writer(&mut self) -> io::Result<&mut std::fs::File> {
        self.writer
            .as_mut()
            .ok_or_else(|| io::Error::other("The staged installer is already closed for writing"))
    }
}

impl Write for LocalPendingFile {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.writer()?.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer()?.flush()
    }
}

impl PendingUpdateFile for LocalPendingFile {
    fn verify_windows_installer(&mut self, arch: Architecture, version: &str) -> io::Result<()> {
        // Windows cannot query the version resource while a writable handle is open.
        drop(self.writer.take());
        windows::verify_installer(&self.staging, arch, version)
    }

    fn commit(self) -> io::Result<PathBuf> {
        drop(self.writer);
        self.staging.persist(&self.destination)?;
        Ok(self.destination)
    }
}

impl UpdateFileWriter for LocalUpdateFiles {
    fn create(&self, filename: &str) -> io::Result<impl PendingUpdateFile> {
        let name = Path::new(filename);
        if name.file_name().and_then(|name| name.to_str()) != Some(filename)
            || filename.contains(['/', '\\', ':'])
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid installer filename",
            ));
        }
        std::fs::create_dir_all(&self.directory)?;
        let (writer, staging) = tempfile::Builder::new()
            .suffix(".part")
            .tempfile_in(&self.directory)?
            .into_parts();
        Ok(LocalPendingFile {
            writer: Some(writer),
            staging,
            destination: self.directory.join(filename),
        })
    }
}

fn installer_filename(url: &Url, extension: &str) -> Result<String, String> {
    let suffix = format!(".{extension}");
    let filename = url
        .path_segments()
        .and_then(Iterator::last)
        .filter(|segment| !segment.is_empty());
    let fallback = format!("Matou-update{suffix}");
    let filename = filename.unwrap_or(&fallback);
    let mut safe_name = percent_decode_str(filename)
        .decode_utf8()
        .map_err(|_| "Invalid installer filename encoding".to_string())?
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if !safe_name.to_ascii_lowercase().ends_with(&suffix) {
        safe_name.push_str(&suffix);
    }
    Ok(safe_name)
}

fn download_manual(
    request: &ManualDownload,
    fetcher: &impl UpdateFetcher,
    files: &impl UpdateFileWriter,
    options: &UpdateOptions,
    version: &str,
    mut now: impl FnMut() -> u64,
    mut on_progress: impl FnMut(Progress),
) -> Result<PathBuf, String> {
    let locale = options.locale;
    let response = fetcher.fetch(request).map_err(|error| error.to_string())?;
    if !(200..300).contains(&response.status) {
        return Err(locale.download_http(response.status));
    }
    let mut body = response.body.ok_or_else(|| {
        locale.text(
            "更新服务器没有返回文件内容",
            "The update server returned no file body",
        )
    })?;
    let filename = installer_filename(&request.url, fetcher.installer_extension())?;
    let started_at = now();
    let mut last_progress_at = started_at;
    let mut file = files.create(&filename).map_err(|error| error.to_string())?;
    let mut transferred = 0;
    let total = response.content_length.or(request.size).unwrap_or(0);
    let mut checksums = DownloadChecksums::default();
    let mut buffer = vec![0; 64 * 1024];
    loop {
        let count = match body.read(&mut buffer) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result.map_err(|error| error.to_string())?,
        };
        if count == 0 {
            break;
        }
        let chunk = &buffer[..count];
        file.write_all(chunk).map_err(|error| error.to_string())?;
        checksums.update(chunk);
        transferred += count as u64;
        let time = now();
        if time.saturating_sub(last_progress_at) >= PROGRESS_INTERVAL_MS {
            last_progress_at = time;
            on_progress(Progress::streaming(
                transferred,
                total,
                time.saturating_sub(started_at),
                false,
            ));
        }
    }
    if response
        .content_length
        .is_some_and(|length| length != transferred)
    {
        return Err(locale
            .text("更新文件下载中断", "Update download was interrupted")
            .into());
    }
    if matches!(options.platform, Platform::Win32)
        && request.size.is_some_and(|length| length != transferred)
    {
        return Err(locale
            .text(
                "安装包大小与更新清单不符",
                "Installer size does not match the update manifest",
            )
            .into());
    }
    if !checksums.matches(
        request.expected_sha256.as_deref(),
        request.expected_sha512.as_deref(),
    ) {
        return Err(locale
            .text(
                "更新文件完整性校验失败",
                "Update file integrity check failed",
            )
            .into());
    }
    file.flush().map_err(|error| error.to_string())?;
    if matches!(options.platform, Platform::Win32) {
        file.verify_windows_installer(options.arch, version)
            .map_err(|error| error.to_string())?;
    }
    let path = file.commit().map_err(|error| error.to_string())?;
    on_progress(Progress::streaming(
        transferred,
        total,
        now().saturating_sub(started_at),
        true,
    ));
    Ok(path)
}

pub struct UpdateOptions {
    pub feed: FeedConfiguration,
    pub platform: Platform,
    pub arch: Architecture,
    pub enabled: bool,
    pub install_mode: InstallMode,
    pub locale: UpdateLocale,
}

struct State {
    phase: UpdatePhase,
    last_release: Option<Release>,
    manual_download: Option<ManualDownload>,
    manual_path: Option<PathBuf>,
}

pub struct UpdateManager {
    current_version: CurrentVersion,
    options: UpdateOptions,
    state: Mutex<State>,
    operation: tokio::sync::Mutex<()>,
    automatic_install_requested: AtomicBool,
}

impl UpdateManager {
    pub fn new(
        current_version: CurrentVersion,
        options: UpdateOptions,
    ) -> Result<Self, PolicyError> {
        options.feed.validate()?;
        Ok(Self::new_live(current_version, options))
    }

    /// Report an invalid live feed when updates are checked, leaving startup usable.
    pub(crate) fn new_live(current_version: CurrentVersion, options: UpdateOptions) -> Self {
        Self {
            current_version,
            options,
            state: Mutex::new(State {
                phase: UpdatePhase::Idle,
                last_release: None,
                manual_download: None,
                manual_path: None,
            }),
            operation: tokio::sync::Mutex::new(()),
            automatic_install_requested: AtomicBool::new(false),
        }
    }

    pub fn preferences(&self) -> UpdatePreferences {
        let mut preferences = UpdatePreferences::new(&self.current_version);
        if self.options.enabled {
            preferences.start();
        }
        preferences
    }

    pub fn state(&self) -> UpdateState {
        self.snapshot(self.state.lock().unwrap().phase.clone())
    }

    fn snapshot(&self, phase: UpdatePhase) -> UpdateState {
        UpdateState {
            current_version: self.current_version.as_str().into(),
            phase,
        }
    }

    fn publish(&self, phase: UpdatePhase, publish: &mut impl FnMut(UpdateState)) {
        self.state.lock().unwrap().phase = phase.clone();
        publish(self.snapshot(phase));
    }

    fn fail(&self, stage: ErrorStage, message: String, publish: &mut impl FnMut(UpdateState)) {
        let state = self.state();
        let release = state.phase.release();
        self.publish(
            UpdatePhase::Error {
                error_message: message,
                error_stage: stage,
                version: release.map(|release| release.version.clone()),
                manual_download_url: release
                    .and_then(|release| release.manual_download_url.clone()),
            },
            publish,
        );
    }

    pub fn check(
        &self,
        fetcher: &impl UpdateFetcher,
        mut unix_ms: impl FnMut() -> u64,
        mut sleep: impl FnMut(u64),
        mut publish: impl FnMut(UpdateState),
    ) -> Option<Result<CheckResult, UpdateError>> {
        if !self.options.enabled {
            return None;
        }
        let _operation = self.operation.try_lock().ok()?;
        let checking = |attempt| UpdatePhase::Checking {
            retry_attempt: (attempt != 0).then_some(attempt),
            max_retry_attempts: (attempt != 0).then_some(CHECK_RETRY_DELAYS_MS.len()),
        };
        self.publish(checking(0), &mut publish);
        for attempt in 0..=CHECK_RETRY_DELAYS_MS.len() {
            self.publish(checking(attempt), &mut publish);
            let result = (|| {
                let url = self.options.feed.manifest_url(
                    self.options.platform,
                    self.options.arch,
                    unix_ms(),
                )?;
                let manifest = fetcher.check(&url)?;
                let available = self.current_version.offers(&manifest.version)?;
                let manual_download = if !available {
                    None
                } else if matches!(self.options.platform, Platform::Win32)
                    && self.options.install_mode == InstallMode::Manual
                {
                    resolve_windows_download(&manifest, &self.options.feed, self.options.arch)?
                } else {
                    resolve_manual_download(
                        &manifest.files,
                        &self.options.feed,
                        fetcher.installer_extension(),
                    )
                };
                Ok((manifest, available, manual_download))
            })();
            match result {
                Ok((manifest, available, manual_download)) => {
                    let result = CheckResult {
                        is_update_available: available,
                        update_info: CheckVersion {
                            version: manifest.version.clone(),
                        },
                    };
                    if available {
                        self.accept_release(manifest, manual_download, &mut publish);
                    } else {
                        self.publish(UpdatePhase::NotAvailable, &mut publish);
                    }
                    return Some(Ok(result));
                }
                Err(error) => {
                    let error: UpdateError = error;
                    if let Some(&delay) = CHECK_RETRY_DELAYS_MS.get(attempt) {
                        self.publish(checking(attempt + 1), &mut publish);
                        sleep(delay);
                    } else {
                        self.fail(ErrorStage::Check, error.message.clone(), &mut publish);
                        return Some(Err(error));
                    }
                }
            }
        }
        unreachable!("the last check attempt returns")
    }

    fn accept_release(
        &self,
        manifest: Manifest,
        manual_download: Option<ManualDownload>,
        publish: &mut impl FnMut(UpdateState),
    ) {
        let first_size = manifest.files.iter().find_map(|file| file.size);
        let size = match self.options.install_mode {
            InstallMode::Manual => manual_download
                .as_ref()
                .and_then(|download| download.size)
                .or(first_size),
            InstallMode::Automatic => first_size,
        };
        let release = Release {
            version: manifest.version,
            release_notes: normalize_notes(manifest.release_notes),
            install_mode: self.options.install_mode,
            release_date: manifest.release_date.filter(|date| !date.is_empty()),
            size_bytes: size,
            manual_download_url: manual_download
                .as_ref()
                .map(|download| download.url.to_string()),
        };
        {
            let mut state = self.state.lock().unwrap();
            state.last_release = Some(release.clone());
            state.manual_download = manual_download;
            state.manual_path = None;
        }
        self.publish(UpdatePhase::Available { release }, publish);
    }

    pub fn report_updater_error(&self, message: String, mut publish: impl FnMut(UpdateState)) {
        if self.options.enabled {
            if let Ok(_operation) = self.operation.try_lock() {
                self.fail(self.updater_error_stage(&message), message, &mut publish);
            }
        }
    }

    fn updater_error_stage(&self, message: &str) -> ErrorStage {
        static SIGNATURE: OnceLock<Regex> = OnceLock::new();
        if SIGNATURE.get_or_init(|| Regex::new(
            r"(?i)code signature|signature.+(?:validation|valid)|did not pass validation|签名.+(?:验证|资源)"
        ).unwrap()).is_match(message) {
            return ErrorStage::Verify;
        }
        match self.state.lock().unwrap().phase {
            UpdatePhase::Available { .. } | UpdatePhase::Downloading { .. } => ErrorStage::Download,
            UpdatePhase::Downloaded { .. } => ErrorStage::Verify,
            _ => ErrorStage::Check,
        }
    }

    pub fn download(
        &self,
        fetcher: &impl UpdateFetcher,
        files: &impl UpdateFileWriter,
        now: impl FnMut() -> u64,
        mut publish: impl FnMut(UpdateState),
    ) {
        if !self.options.enabled {
            return;
        }
        let Ok(_operation) = self.operation.try_lock() else {
            return;
        };
        let (mut release, manual) = {
            let state = self.state.lock().unwrap();
            let (release, use_manual) = match &state.phase {
                UpdatePhase::Available { release } => {
                    (release, release.install_mode == InstallMode::Manual)
                }
                UpdatePhase::Error {
                    manual_download_url: Some(_),
                    ..
                } => {
                    let Some(release) = &state.last_release else {
                        return;
                    };
                    (release, true)
                }
                _ => return,
            };
            (
                release.clone(),
                if use_manual {
                    state.manual_download.clone()
                } else {
                    None
                },
            )
        };
        if let Some(manual) = &manual {
            release.install_mode = InstallMode::Manual;
            release.size_bytes = manual.size.or(release.size_bytes);
            let mut state = self.state.lock().unwrap();
            state.last_release = Some(release.clone());
            state.manual_path = None;
        } else if !matches!(self.state().phase, UpdatePhase::Available { .. }) {
            return;
        }

        let mut progress = |progress| {
            self.publish(
                UpdatePhase::Downloading {
                    release: release.clone(),
                    progress,
                },
                &mut publish,
            )
        };
        let result = if let Some(manual) = &manual {
            progress(Progress::new(0.0, 0, release.size_bytes.unwrap_or(0), 0));
            download_manual(
                manual,
                fetcher,
                files,
                &self.options,
                &release.version,
                now,
                progress,
            )
            .map(Some)
            .map_err(DownloadError::Rejected)
        } else {
            fetcher
                .download_automatic(&release, &mut progress)
                .map(|()| None)
        };

        match result {
            Ok(path) => {
                self.state.lock().unwrap().manual_path = path;
                self.publish(UpdatePhase::Downloaded { release }, &mut publish);
            }
            Err(error) => {
                let (stage, message) = match error {
                    DownloadError::Rejected(message) => (ErrorStage::Download, message),
                    DownloadError::UpdaterEvent(message) => {
                        (self.updater_error_stage(&message), message)
                    }
                    DownloadError::Verification(message) => (ErrorStage::Verify, message),
                };
                self.fail(stage, message, &mut publish);
            }
        }
    }

    pub(crate) fn open_downloaded_installer(
        &self,
        data_root: &Path,
        open: impl FnOnce(&Path) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        let _operation = self
            .operation
            .try_lock()
            .map_err(|_| anyhow::anyhow!("Another update operation is in progress"))?;
        let (path, version, manual) = {
            let state = self.state.lock().unwrap();
            let UpdatePhase::Downloaded { release } = &state.phase else {
                anyhow::bail!("No downloaded update installer is ready to open");
            };
            let path = state
                .manual_path
                .clone()
                .ok_or_else(|| anyhow::anyhow!("No downloaded installer path is available"))?;
            (path, release.version.clone(), state.manual_download.clone())
        };
        let extension = match self.options.platform {
            Platform::Darwin => "dmg",
            Platform::Win32 => "exe",
            _ => anyhow::bail!("Opening an update installer is unsupported on this platform"),
        };

        let root = data_root
            .canonicalize()
            .context("Update data directory is unavailable")?;
        let directory = root.join("updates");
        anyhow::ensure!(
            directory
                .canonicalize()
                .context("Update directory is unavailable")?
                == directory,
            "Update directory resolves outside its expected location"
        );
        let path = path
            .canonicalize()
            .context("Downloaded installer path is unavailable")?;
        anyhow::ensure!(
            path.starts_with(&directory) && path != directory,
            "Downloaded installer is outside the update directory"
        );
        anyhow::ensure!(path.is_file(), "Downloaded installer is not a regular file");
        anyhow::ensure!(
            path.extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case(extension)),
            "Downloaded installer must have a .{extension} extension"
        );
        if matches!(self.options.platform, Platform::Win32) {
            let manual = manual.context("Windows update verification metadata is unavailable")?;
            windows::verify_installer(&path, self.options.arch, &version)?;
            let mut file = std::fs::File::open(&path)?;
            anyhow::ensure!(
                manual
                    .size
                    .is_none_or(|size| file.metadata().is_ok_and(|info| info.len() == size)),
                "Downloaded installer size does not match the update manifest"
            );
            let mut checksums = DownloadChecksums::default();
            let mut buffer = [0; 64 * 1024];
            loop {
                let count = file.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                checksums.update(&buffer[..count]);
            }
            anyhow::ensure!(
                checksums.matches(
                    manual.expected_sha256.as_deref(),
                    manual.expected_sha512.as_deref()
                ),
                "Downloaded installer integrity check failed"
            );
        }
        open(&path)
    }

    pub async fn install(
        &self,
        installer: &impl UpdateInstaller,
        mut publish: impl FnMut(UpdateState),
    ) {
        if !self.options.enabled {
            return;
        }
        let mode = match &self.state.lock().unwrap().phase {
            UpdatePhase::Downloaded { release } => release.install_mode,
            _ => return,
        };
        let _operation = match mode {
            // A second automatic request waits for the in-flight install to settle.
            InstallMode::Automatic => self.operation.lock().await,
            InstallMode::Manual => match self.operation.try_lock() {
                Ok(operation) => operation,
                Err(_) => return,
            },
        };
        let (mode, manual_path) = {
            let state = self.state.lock().unwrap();
            let UpdatePhase::Downloaded { release } = &state.phase else {
                return;
            };
            (release.install_mode, state.manual_path.clone())
        };
        let result = match mode {
            InstallMode::Manual => match manual_path {
                Some(path) => installer.open(&path).await,
                None => Err(self
                    .options
                    .locale
                    .text(
                        "已下载的 DMG 路径不存在",
                        "The downloaded DMG path does not exist",
                    )
                    .into()),
            },
            InstallMode::Automatic => {
                if self
                    .automatic_install_requested
                    .swap(true, Ordering::Relaxed)
                {
                    return;
                }
                installer
                    .prepare()
                    .await
                    .and_then(|()| installer.quit_and_install(false, true))
            }
        };
        if let Err(message) = result {
            self.fail(ErrorStage::Install, message, &mut publish);
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn open_installer(path: &Path) -> anyhow::Result<()> {
    let output = std::process::Command::new("/usr/bin/open")
        .arg("--")
        .arg(path)
        .output()
        .context("Could not open the downloaded installer")?;
    anyhow::ensure!(
        output.status.success(),
        "Could not open the downloaded installer: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

#[cfg(windows)]
pub(crate) fn open_installer(path: &Path) -> anyhow::Result<()> {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    // Shell paths use DOS or UNC syntax. Verify that removing the verbatim prefix
    // still resolves to the same path before handing it to the shell.
    let wide = path.as_os_str().encode_wide().collect::<Vec<_>>();
    let prefix = r"\\?\".encode_utf16().collect::<Vec<_>>();
    let unc_prefix = r"\\?\UNC\".encode_utf16().collect::<Vec<_>>();
    let mut shell_path = if let Some(unc) = wide.strip_prefix(unc_prefix.as_slice()) {
        r"\\".encode_utf16().chain(unc.iter().copied()).collect()
    } else {
        wide.strip_prefix(prefix.as_slice())
            .unwrap_or(&wide)
            .to_vec()
    };
    let resolved = PathBuf::from(OsString::from_wide(&shell_path))
        .canonicalize()
        .context("Could not resolve installer shell path")?;
    anyhow::ensure!(
        resolved == path,
        "Installer shell path resolves to a different file"
    );
    shell_path.push(0);
    let result = shell_open(&shell_path);
    anyhow::ensure!(
        result > 32,
        "Could not open the downloaded installer (Windows shell error {result})"
    );
    Ok(())
}

#[cfg(windows)]
pub(crate) fn shell_open(target: &[u16]) -> isize {
    use std::{ffi::c_void, ptr::null};

    if target.last() != Some(&0) {
        return 0;
    }
    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ShellExecuteW(
            window: *mut c_void,
            operation: *const u16,
            file: *const u16,
            parameters: *const u16,
            directory: *const u16,
            show: i32,
        ) -> isize;
    }

    let operation = [b'o' as u16, b'p' as u16, b'e' as u16, b'n' as u16, 0];
    unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            target.as_ptr(),
            null(),
            null(),
            1,
        )
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
pub(crate) fn open_installer(_: &Path) -> anyhow::Result<()> {
    anyhow::bail!("Opening an update installer is unsupported on this platform")
}

#[cfg(test)]
mod install_tests {
    use super::*;

    #[test]
    fn opens_only_a_downloaded_file_within_the_platform_update_directory() -> anyhow::Result<()> {
        for (platform, extension, wrong_extension) in [
            (Platform::Darwin, "dmg", "exe"),
            (Platform::Win32, "exe", "dmg"),
        ] {
            let root = tempfile::tempdir()?;
            let updates = root.path().join("updates");
            std::fs::create_dir(&updates)?;
            let installer = updates.join(format!("Matou.{extension}"));
            let wrong_format = updates.join(format!("Matou.{wrong_extension}"));
            let sibling = root.path().join(format!("Matou.{extension}"));
            for path in [&installer, &wrong_format, &sibling] {
                std::fs::write(path, b"dry-run update")?;
            }
            let manager = UpdateManager::new(
                CurrentVersion::parse("0.2.26")?,
                UpdateOptions {
                    feed: FeedConfiguration::resolve(None, None, None),
                    platform,
                    arch: Architecture::Arm64,
                    enabled: true,
                    install_mode: InstallMode::Manual,
                    locale: UpdateLocale::default(),
                },
            )?;
            manager.state.lock().unwrap().phase = UpdatePhase::Downloaded {
                release: Release {
                    version: "0.2.27".into(),
                    release_notes: vec![],
                    install_mode: InstallMode::Manual,
                    release_date: None,
                    size_bytes: None,
                    manual_download_url: None,
                },
            };
            for (path, reason) in [
                (None, "No downloaded installer path"),
                (Some(wrong_format), "extension"),
                (Some(sibling.clone()), "outside the update directory"),
            ] {
                manager.state.lock().unwrap().manual_path = path;
                let error = manager
                    .open_downloaded_installer(root.path(), |_| panic!("invalid installer opened"))
                    .unwrap_err();
                assert!(error.to_string().contains(reason), "{error:#}");
            }
            let expected = installer.canonicalize()?;
            manager.state.lock().unwrap().manual_path = Some(installer.clone());
            manager.open_downloaded_installer(root.path(), |path| {
                assert_eq!(path, expected);
                Ok(())
            })?;
            #[cfg(unix)]
            {
                std::fs::remove_file(&installer)?;
                std::os::unix::fs::symlink(&sibling, &installer)?;
                let error = manager
                    .open_downloaded_installer(root.path(), |_| panic!("escaping symlink opened"))
                    .unwrap_err();
                assert!(error.to_string().contains("outside the update directory"));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
pub enum InstallAction {
    ToggleIdle,
    InstallNow,
    Close,
}

#[derive(Debug)]
pub struct InstallChoice {
    pub action: InstallAction,
    pub text: &'static str,
    pub disabled: bool,
}

#[derive(Debug)]
pub struct InstallPrompt {
    pub title: &'static str,
    pub active_sessions: Option<String>,
    pub idle_update_keeps_state: Option<&'static str>,
    pub waiting: Option<&'static str>,
    pub choices: Vec<InstallChoice>,
}

pub const UPDATED_NOTICE_MS: u64 = 5_000;

/// Tracks the session-protection choices and the updated notice for one control instance.
#[derive(Default)]
pub struct UpdateProtection {
    open: bool,
    waiting_for_idle: bool,
    requested: Option<InstallMode>,
    last_seen_version: Option<String>,
    updated_version: Option<String>,
}

impl UpdateProtection {
    pub fn new(last_seen_version: Option<String>) -> Self {
        Self {
            last_seen_version,
            ..Self::default()
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn last_seen_version(&self) -> Option<&str> {
        self.last_seen_version.as_deref()
    }

    pub fn updated_version(&self) -> Option<&str> {
        self.updated_version.as_deref()
    }

    pub fn expire_updated_notice(&mut self) {
        self.updated_version = None;
    }

    /// Returns a timer delay when a newly observed version needs an updated notice.
    pub fn accept(&mut self, state: &UpdateState) -> Option<u64> {
        if matches!(
            state.phase,
            UpdatePhase::Available { .. } | UpdatePhase::Downloaded { .. }
        ) {
            self.open = true;
        }
        if state.current_version.is_empty() {
            return None;
        }
        if self.last_seen_version.as_ref() == Some(&state.current_version) {
            return None;
        }
        let previous = self
            .last_seen_version
            .replace(state.current_version.clone());
        if previous.is_some_and(|version| !version.is_empty()) {
            self.updated_version = Some(state.current_version.clone());
            Some(UPDATED_NOTICE_MS)
        } else {
            None
        }
    }

    pub fn on_activity(&mut self, state: &UpdateState, active_count: usize) -> bool {
        if self.waiting_for_idle
            && active_count == 0
            && matches!(&state.phase, UpdatePhase::Downloaded { release } if release.install_mode == InstallMode::Automatic)
        {
            return self.request(InstallMode::Automatic);
        }
        false
    }

    /// Returns true when the owner should invoke the installer.
    pub fn press(
        &mut self,
        action: InstallAction,
        state: &UpdateState,
        active_count: usize,
    ) -> bool {
        if !self.open {
            return false;
        }
        if matches!(action, InstallAction::Close) {
            self.open = false;
            return false;
        }
        let UpdatePhase::Downloaded { release } = &state.phase else {
            return false;
        };
        match action {
            InstallAction::ToggleIdle => {
                if release.install_mode == InstallMode::Automatic
                    && active_count > 0
                    && self.requested.is_none()
                {
                    self.waiting_for_idle = !self.waiting_for_idle;
                }
                false
            }
            InstallAction::InstallNow => self.request(release.install_mode),
            InstallAction::Close => unreachable!(),
        }
    }

    fn request(&mut self, mode: InstallMode) -> bool {
        if self.requested.is_some() {
            return false;
        }
        self.requested = Some(mode);
        true
    }

    pub fn settle_install(&mut self) {
        if self.requested == Some(InstallMode::Manual) {
            self.requested = None;
        }
    }

    pub fn trigger_label(&self, state: &UpdateState, locale: UpdateLocale) -> String {
        let prefix = locale.text("应用更新", "App update");
        let suffix = match &state.phase {
            UpdatePhase::Idle | UpdatePhase::NotAvailable => return prefix.into(),
            UpdatePhase::Checking { .. } => locale.text("正在检查", "checking").into(),
            UpdatePhase::Available { release } => match locale {
                UpdateLocale::Chinese => format!("发现 {}", release.version),
                UpdateLocale::English => format!("{} available", release.version),
            },
            UpdatePhase::Downloading { progress, .. } => format!(
                "{} {:.0}%",
                locale.text("下载中", "downloading"),
                progress.percent.round(),
            ),
            UpdatePhase::Downloaded { release } => match release.install_mode {
                InstallMode::Manual => locale
                    .text("等待打开安装包", "waiting to open installer")
                    .into(),
                InstallMode::Automatic => locale.text("等待安装", "waiting to install").into(),
            },
            UpdatePhase::Error { error_stage, .. } => error_stage.title(locale).into(),
        };
        format!("{prefix}{}{suffix}", locale.text("：", ": "))
    }

    pub fn updated_notice(&self, locale: UpdateLocale) -> Option<[String; 2]> {
        let version = self.updated_version()?;
        Some([
            locale.updated_to(version),
            locale
                .text("工作空间与会话已恢复", "Workspaces and sessions restored")
                .into(),
        ])
    }

    pub fn install_prompt(
        &self,
        state: &UpdateState,
        count: usize,
        locale: UpdateLocale,
    ) -> Option<InstallPrompt> {
        let UpdatePhase::Downloaded { release } = &state.phase else {
            return None;
        };
        let installing = self.requested.is_some();
        let choice = |action, text, disabled| InstallChoice {
            action,
            text,
            disabled,
        };
        if release.install_mode == InstallMode::Manual {
            return Some(InstallPrompt {
                title: locale.text("DMG 已下载完成", "DMG downloaded"),
                active_sessions: None,
                idle_update_keeps_state: None,
                waiting: None,
                choices: vec![
                    choice(
                        InstallAction::InstallNow,
                        if installing {
                            locale.text("正在打开…", "Opening…")
                        } else {
                            locale.text("打开 DMG 安装", "Open DMG installer")
                        },
                        installing,
                    ),
                    choice(
                        InstallAction::Close,
                        locale.text("稍后安装", "Install later"),
                        false,
                    ),
                ],
            });
        }
        let mut choices = Vec::new();
        let restart = if count > 0 {
            let toggle = if self.waiting_for_idle {
                locale.text("取消空闲更新", "Cancel idle update")
            } else {
                locale.text("空闲后自动更新", "Update once idle")
            };
            choices.push(choice(InstallAction::ToggleIdle, toggle, installing));
            locale.text("立即重启并更新", "Install and restart now")
        } else if installing {
            locale.text("正在准备更新…", "Preparing update…")
        } else {
            locale.text("重启并更新", "Install and restart")
        };
        choices.push(choice(InstallAction::InstallNow, restart, installing));
        choices.push(choice(
            InstallAction::Close,
            locale.text("退出时安装", "Install on quit"),
            false,
        ));
        Some(InstallPrompt {
            title: locale.text("更新已准备好", "Update ready"),
            active_sessions: (count > 0).then(|| locale.active_sessions(count)),
            idle_update_keeps_state: (count > 0).then(|| locale.idle_update_keeps_state()),
            waiting: self
                .waiting_for_idle
                .then(|| locale.idle_update_scheduled()),
            choices,
        })
    }
}
