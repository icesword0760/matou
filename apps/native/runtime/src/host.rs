//! Native desktop connection to the serialized domain owner.
//!
//! No SQLite, PTY, or Ghostty handle is accessed on the GPUI thread. The current
//! vertical slice hosts the owner in a background thread; standalone runtime
//! transport and additional windows remain separate migration work.
#[path = "provider_model_discovery.rs"]
mod provider_model_discovery;

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    ffi::OsString,
    path::{Component, Path, PathBuf},
    sync::{
        Arc, LazyLock, OnceLock,
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use crate::{
    SessionEvent, SessionOptions,
    antigravity_catalog::{self, Catalog as AntigravityCatalog, RecoveryCandidate},
    antigravity_launch::{
        self, LaunchMode as AntigravityMode, PermissionMode as AntigravityPermission,
    },
    browser_integration, claude_model_catalog, codex_catalog,
    codex_launch::{self, CodexLaunch},
    codex_permissions,
    codex_resume::{self, Resume as CodexResume},
    dsh_catalog::{self, DshSessionCatalog},
    egress_probe::{self, EgressMonitor, EgressSample, Family, LiveEgressSampler, RuleMode},
    environment_runtime::{self, EnvironmentTarget, WorktreeExpectation},
    history_worker::{HistoryReply, HistoryRequest, HistoryWorker},
    host_control::ControlServer,
    owner::{DshLaunch, OwnerEvent, ShellOwner},
    protocol::{
        ClaudeModelCatalogReply, ClaudeModelCatalogResult, ClaudeModelPreferencesReply,
        ClaudeRoleAssignmentsReply, ProviderSettingsReply, ProviderSettingsResult,
        TimeZoneSettingsError, TimeZoneSettingsReply, TimeZoneSettingsSnapshot,
        deserialize_present,
    },
    provider_catalog,
    provider_hook::HookSetup,
    retention_policy::{self, IndexedCheckpoint, IndexedSession, RetentionQuota, SessionRetention},
    search::{SearchRequest, SearchResult},
    shell_input::{InteractiveLaunch, ShellInput},
    subscription_launch,
    subscription_service::{PreparedClaudeLaunch, SubscriptionService},
    update_download, update_http,
    worker::{SelectionCopy, ShellSubmission, WorkerCommand},
    workspace_paths::WorkspacePathChecks,
};
use anyhow::{Context, Result, anyhow, bail, ensure};
use matou_native_core::{
    account_store::AccountStore,
    backup::{BackupReason, BackupService, DEFAULT_MAX_BACKUPS},
    database_recovery::{OpeningAction, RECOVERY_ID_CHANGED, RecoveryActionError, RecoveryActions},
    git_repo::{self, GitActionError, GitStatusDetail, Head, Repository, WorktreeScope},
    hierarchy::{
        self, BootstrapWindowInput, HierarchyIds, HierarchyResult, Scene, Task, WindowNavigation,
        Workspace,
        canvas::{
            CanvasError, CanvasIds, CanvasMessages, CreateCanvasInput, CreateShellSiblingInput,
            ForkSessionInput,
        },
        cards::{CloseSessionCardInput, RemoveScope, RemoveSessionBranchInput},
        tabs::{CloseSceneInput, RenameSceneInput, ReorderSceneInput},
        tasks, workspaces,
    },
    journal::JournalOptions,
    link_routing::{self, LinkSource},
    provider_config::{ProviderConfigLocale, ProviderConfigStore},
    provider_launch::{
        CLAUDE_PERMISSION_MODES, ClaudeLaunch, ClaudeTarget, automatic_time_zone,
        available_time_zones, official_time_zone_endpoint, prepare_claude_fork,
        prepare_claude_launch, prepare_claude_permission_restart, prepare_managed_claude_launch,
        prepare_new_claude_launch_with_budget, project_cli_time_zone, system_time_zone,
        valid_claude_time_zone, validate_claude_compaction, validate_claude_role_assignments,
        validate_claude_web_search, validate_cwd,
    },
    pty::{LaunchProfile, ShellLaunch},
    sessions::{
        ClaudeCatalogLoad, ClaudeRestore, ClaudeRoleLaunchPreferences, InterfaceLocale,
        LOCALE_FILE, LocalePreference, ProviderBinding, current_interface_locale,
        read_claude_role_launch_preferences, read_claude_time_zone, read_locale_preference,
        save_claude_model_preferences, save_claude_time_zone,
    },
    startup::{self, BootstrapResult, ReadOnlyReason, RecoveryError, RecoveryRequired},
    storage::{CommandMetadata, configure_connection},
    update_policy::{
        self, Architecture, CurrentVersion, FeedConfiguration, InstallMode, Platform, UpdateLocale,
    },
    workspace_paths::WorkspacePathState,
};
use matou_native_terminal::{
    GridSelection, InputModes, Screen, SelectionAction, TerminalOptions, Theme,
};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::environment_runtime::CheckedEnvironment;
pub use crate::environment_runtime::EnvironmentChangeResult;
pub use crate::provider_catalog::{RecentReplies, RecentReply, deferred_summary_text};
pub use matou_native_core::hierarchy::canvas::{ForkPlacement, ForkWorktreeMode};
pub use matou_native_core::hierarchy::session_environment::Target as SessionEnvironmentTarget;
use matou_native_core::hierarchy::session_environment::{
    self, Kind as EnvironmentChangeKind, Transition,
};

#[derive(Clone)]
pub struct LaunchOptions {
    pub data_root: PathBuf,
    pub workspace: PathBuf,
    /// The acceptance launcher supplies a disposable home. No user shell files
    /// are rewritten or copied to this directory.
    pub shell_home: PathBuf,
    pub shell: PathBuf,
}

impl LaunchOptions {
    pub fn from_args(args: impl IntoIterator<Item = OsString>) -> Result<Self> {
        // The shipped runtime enters here before reading Hello. The private
        // shim mode must finish before normal options or runtime startup.
        let args = {
            let mut args = args.into_iter().peekable();
            if args
                .peek()
                .is_some_and(|flag| flag == crate::mt_cli::MODE || flag == "--internal-mt")
            {
                args.next();
                std::process::exit(crate::mt_cli::run(args));
            }
            args
        };
        Self::from_args_with_env(
            args,
            |key| std::env::var_os(key),
            || std::env::current_exe().ok(),
        )
    }

    fn from_args_with_env(
        args: impl IntoIterator<Item = OsString>,
        env: impl Fn(&str) -> Option<OsString>,
        executable: impl FnOnce() -> Option<PathBuf>,
    ) -> Result<Self> {
        let (mut data_root, mut workspace, mut shell_home, mut shell) = (None, None, None, None);
        let mut args = args.into_iter();
        while let Some(flag) = args.next() {
            if flag.as_encoded_bytes().starts_with(b"-psn_") {
                continue;
            }
            let value = args
                .next()
                .context("native acceptance launcher requires a path after each flag")?;
            let path = PathBuf::from(value);
            match flag.to_str() {
                Some("--data-root") => data_root = Some(path),
                Some("--workspace") => workspace = Some(path),
                Some("--shell-home") => shell_home = Some(path),
                Some("--shell") => shell = Some(path),
                _ => bail!("Unknown native launcher option: {}", flag.to_string_lossy()),
            }
        }
        let env_path = |key| {
            env(key)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        let home = || {
            let home = env_path("HOME");
            #[cfg(windows)]
            let home = home.or_else(|| env_path("USERPROFILE"));
            home.context("Cannot resolve native launcher defaults: HOME is unset or empty")
        };
        let data_override = || {
            let value = env("MATOU_DATA_DIR")?;
            if let Some(text) = value.to_str() {
                let trimmed = text.trim();
                return (!trimmed.is_empty()).then(|| PathBuf::from(trimmed));
            }
            Some(PathBuf::from(value))
        };
        let data_root = match data_root.or_else(data_override) {
            Some(path) => path,
            None => {
                let beta = executable().as_deref().is_some_and(is_beta_executable);
                #[cfg(windows)]
                let path = env_path("APPDATA")
                    .context("Cannot resolve native data directory: APPDATA is unset or empty")?
                    .join(if beta { "MatouBeta" } else { "Matou" });
                #[cfg(not(windows))]
                let path = home()?.join(if beta { ".matou-beta" } else { ".matou" });
                path
            }
        };
        let options = Self {
            data_root,
            workspace: workspace.map_or_else(home, Ok)?,
            shell_home: shell_home.map_or_else(home, Ok)?,
            shell: shell.unwrap_or_else(|| {
                #[cfg(windows)]
                return "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe".into();
                #[cfg(unix)]
                return env_path("SHELL")
                    .filter(|path| {
                        path.is_absolute()
                            && matches!(browser_integration::inspect_executable(path), Ok(true))
                    })
                    .unwrap_or_else(|| "/bin/zsh".into());
                #[cfg(not(any(unix, windows)))]
                PathBuf::from("/bin/zsh")
            }),
        };
        for path in [
            &options.data_root,
            &options.workspace,
            &options.shell_home,
            &options.shell,
        ] {
            if !path.is_absolute() {
                bail!("Native launcher paths must be absolute: {}", path.display());
            }
        }
        Ok(options)
    }
}

#[cfg(target_os = "macos")]
fn is_beta_executable(executable: &Path) -> bool {
    let Some(info) = executable
        .parent()
        .and_then(Path::parent)
        .map(|dir| dir.join("Info.plist"))
    else {
        return false;
    };
    info.is_file()
        && std::process::Command::new("/usr/bin/plutil")
            .args([
                "-extract",
                "CFBundleIdentifier",
                "raw",
                "-expect",
                "string",
                "-o",
                "-",
                "--",
            ])
            .arg(info)
            .output()
            .is_ok_and(|output| {
                output.status.success()
                    && String::from_utf8_lossy(&output.stdout)
                        .trim()
                        .ends_with(".beta")
            })
}

#[cfg(windows)]
fn is_beta_executable(executable: &Path) -> bool {
    executable
        .parent()
        .and_then(|dir| std::fs::read(dir.join("resources/version.json")).ok())
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .is_some_and(|info| {
            info["appId"]
                .as_str()
                .is_some_and(|id| id.ends_with(".beta"))
        })
}

#[cfg(not(any(target_os = "macos", windows)))]
fn is_beta_executable(_: &Path) -> bool {
    false
}

#[cfg(test)]
mod launch_options_tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_defaults_follow_the_bundle_identifier() -> Result<()> {
        let home = tempfile::tempdir()?;
        let contents = home.path().join("fixture.app/Contents");
        std::fs::create_dir_all(contents.join("MacOS"))?;
        let executable = contents.join("MacOS/fixture");
        let plist = contents.join("Info.plist");
        let env = |key: &str| match key {
            "HOME" => Some(home.path().into()),
            "MATOU_DATA_DIR" => Some(" \t ".into()),
            _ => None,
        };
        for (id, directory) in [
            (None, ".matou"),
            (Some("com.matou.desktop"), ".matou"),
            (Some("com.matou.desktop.beta"), ".matou-beta"),
        ] {
            if let Some(id) = id {
                std::fs::write(
                    &plist,
                    format!(
                        "<plist version=\"1.0\"><dict><key>CFBundleIdentifier</key>\n<string>{id}</string></dict></plist>"
                    ),
                )?;
            }
            let options = LaunchOptions::from_args_with_env([], env, || Some(executable.clone()))?;
            assert_eq!(options.data_root, home.path().join(directory));
            assert_eq!(options.workspace, home.path());
            assert_eq!(options.shell_home, home.path());
            assert_eq!(options.shell, Path::new("/bin/zsh"));
            assert!(!options.data_root.exists());
        }
        Ok(())
    }

    #[cfg(windows)]
    #[test]
    fn windows_defaults_use_roaming_app_data_and_package_id() -> Result<()> {
        let home = tempfile::tempdir()?;
        let appdata = home.path().join("AppData/Roaming");
        let executable = home.path().join("package/fixture.exe");
        let resources = home.path().join("package/resources");
        std::fs::create_dir_all(&resources)?;
        let env = |key: &str| match key {
            "USERPROFILE" => Some(home.path().into()),
            "APPDATA" => Some(appdata.clone().into()),
            _ => None,
        };
        for (id, directory) in [
            ("com.matou.desktop", "Matou"),
            ("com.matou.desktop.beta", "MatouBeta"),
        ] {
            std::fs::write(
                resources.join("version.json"),
                json!({ "appId": id }).to_string(),
            )?;
            let options = LaunchOptions::from_args_with_env([], env, || Some(executable.clone()))?;
            assert_eq!(options.data_root, appdata.join(directory));
            assert_eq!(options.workspace, home.path());
            assert_eq!(options.shell_home, home.path());
            assert!(!options.data_root.exists());
        }
        Ok(())
    }

    #[test]
    fn data_override_is_trimmed_and_finder_flag_leaves_explicit_options_intact() -> Result<()> {
        let home = tempfile::tempdir()?;
        let override_root = home.path().join("environment data");
        let env = |key: &str| match key {
            "HOME" => Some(home.path().into()),
            "MATOU_DATA_DIR" => Some(format!(" \t{} \n", override_root.display()).into()),
            _ => None,
        };
        let unused_executable = || panic!("data override should skip bundle identification");
        let options = LaunchOptions::from_args_with_env([], env, unused_executable)?;
        assert_eq!(options.data_root, override_root);

        let data = home.path().join("explicit data");
        let workspace = home.path().join("explicit workspace");
        let shell_home = home.path().join("explicit home");
        let shell = home.path().join("explicit shell");
        let args = [
            "-psn_0_98765".into(),
            "--data-root".into(),
            data.clone().into(),
            "--workspace".into(),
            workspace.clone().into(),
            "--shell-home".into(),
            shell_home.clone().into(),
            "--shell".into(),
            shell.clone().into(),
        ];
        let options = LaunchOptions::from_args_with_env(args.clone(), env, unused_executable)?;
        assert_eq!(options.data_root, data);
        assert_eq!(options.workspace, workspace);
        assert_eq!(options.shell_home, shell_home);
        assert_eq!(options.shell, shell);
        LaunchOptions::from_args_with_env(args.clone(), |_| None, unused_executable)?;
        for index in [2, 4, 6, 8] {
            let mut args = args.clone();
            args[index] = "relative".into();
            let error = LaunchOptions::from_args_with_env(args, env, unused_executable)
                .err()
                .unwrap();
            assert!(error.to_string().contains("paths must be absolute"));
        }
        Ok(())
    }

    #[test]
    fn missing_home_explains_which_environment_variable_is_needed() {
        let error = LaunchOptions::from_args_with_env(
            ["--data-root".into(), std::env::temp_dir().into()],
            |_| None,
            || None,
        )
        .err()
        .unwrap();
        assert!(error.to_string().contains("HOME is unset or empty"));
    }

    #[cfg(unix)]
    #[test]
    fn default_shell_uses_only_an_absolute_executable_file() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let home = tempfile::tempdir()?;
        let shell = home.path().join("test-shell");
        let not_executable = home.path().join("not-executable");
        std::fs::write(&shell, "#!/bin/sh\n")?;
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700))?;
        std::fs::write(&not_executable, "#!/bin/sh\n")?;
        std::fs::set_permissions(&not_executable, std::fs::Permissions::from_mode(0o600))?;
        for (value, expected) in [
            (shell.as_path(), shell.as_path()),
            (not_executable.as_path(), Path::new("/bin/zsh")),
            (
                home.path().join("missing-shell").as_path(),
                Path::new("/bin/zsh"),
            ),
            (home.path(), Path::new("/bin/zsh")),
            (Path::new("relative-shell"), Path::new("/bin/zsh")),
            (Path::new(""), Path::new("/bin/zsh")),
        ] {
            let options = LaunchOptions::from_args_with_env(
                [],
                |key| match key {
                    "HOME" => Some(home.path().into()),
                    "SHELL" => Some(value.into()),
                    _ => None,
                },
                || None,
            )?;
            assert_eq!(options.shell, expected);
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
pub enum Command {
    DatabaseRecovery {
        #[serde(rename = "requestId")]
        request_id: String,
        action: DatabaseRecoveryAction,
    },
    History(HistoryRequest),
    ReadOnlyQuery {
        request_id: u64,
        query: ReadOnlyQuery,
    },
    GetClaudeModelCatalog {
        request_id: u64,
    },
    GetClaudeRoleAssignments {
        request_id: u64,
    },
    GetClaudeModelPreferences {
        request_id: u64,
    },
    GetTimeZoneSettings {
        request_id: u64,
    },
    SetTimeZoneSettings {
        request_id: u64,
        #[serde(default)]
        time_zone: Value,
    },
    SetClaudeRoleAssignments {
        request_id: u64,
        assignments: Value,
        #[serde(
            rename = "webSearch",
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_present"
        )]
        web_search: Option<Value>,
    },
    SetClaudeModelPreferences {
        request_id: u64,
        assignments: Value,
        #[serde(
            rename = "webSearch",
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_present"
        )]
        web_search: Option<Value>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "deserialize_present"
        )]
        compaction: Option<Value>,
    },
    GetProviderSettings {
        request_id: u64,
    },
    SaveProvider {
        request_id: u64,
        input: Value,
    },
    FetchProviderModels {
        request_id: u64,
        input: Value,
    },
    DeleteProvider {
        request_id: u64,
        cli: String,
        id: String,
    },
    SetActiveProvider {
        request_id: u64,
        cli: String,
        id: String,
    },
    ConfirmAntigravityRisk {
        request_id: u64,
    },
    Search {
        session_id: String,
        request: SearchRequest,
    },
    ClearSearch {
        session_id: String,
    },
    ScrollSelection {
        session_id: String,
        lines: i32,
    },
    Select {
        session_id: String,
        action: SelectionAction,
    },
    CopySelection {
        session_id: String,
        request_id: u64,
        interrupt_if_empty: bool,
    },
    Input {
        session_id: String,
        bytes: Vec<u8>,
        defer_ordering: bool,
    },
    Resize {
        session_id: String,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    },
    Scroll {
        session_id: String,
        lines: i32,
    },
    ScrollTo {
        session_id: String,
        row: u64,
        request_id: u64,
        /// The desktop already resolved the track position to a viewport row.
        #[serde(default)]
        pixel_target: bool,
    },
    Theme {
        session_id: String,
        theme: Theme,
    },
    Applied {
        session_id: String,
        sequence: u64,
    },
    RetryStorage {
        session_id: String,
    },
    RetryRecovery {
        session_id: String,
    },
    RestartSession {
        session_id: String,
    },
    RestoreSessionEnvironment {
        request_id: u64,
        session_id: String,
    },
    LocateSessionEnvironment {
        request_id: u64,
        session_id: String,
        path: PathBuf,
    },
    HandoffSessionEnvironment {
        request_id: u64,
        session_id: String,
        target: SessionEnvironmentTarget,
    },
    RetryProviderRestore {
        session_id: String,
    },
    StartFreshProviderSession {
        session_id: String,
    },
    SetProviderPermissionMode {
        session_id: String,
        mode: String,
    },
    ListProviderSessions {
        cwd: String,
    },
    GetProviderSession {
        cwd: String,
        provider_session_id: String,
        start_event_index: Option<i64>,
        limit: Option<i64>,
    },
    GetDeferredRecoveryPreview {
        session_id: String,
    },
    LoadProviderSession {
        session_id: String,
        provider_session_id: String,
    },
    ListCodexSessions {
        request_id: u64,
        cwd: String,
        query: String,
        full_text: bool,
        offset: Option<i64>,
        limit: Option<i64>,
    },
    GetCodexSession {
        request_id: u64,
        cwd: String,
        provider_session_id: String,
        start_event_index: Option<i64>,
        around_event_index: Option<i64>,
        limit: Option<i64>,
    },
    SearchCodexSession {
        request_id: u64,
        cwd: String,
        provider_session_id: String,
        query: String,
        offset: Option<i64>,
        limit: Option<i64>,
    },
    LoadCodexSession {
        session_id: String,
        provider_session_id: String,
    },
    ListDshSessions {
        request_id: u64,
        cwd: String,
        query: String,
        full_text: bool,
        offset: Option<i64>,
        limit: Option<i64>,
    },
    GetDshSession {
        request_id: u64,
        cwd: String,
        provider_session_id: String,
        before_event_index: Option<i64>,
        around_event_index: Option<i64>,
        limit: Option<i64>,
    },
    SearchDshSession {
        request_id: u64,
        cwd: String,
        provider_session_id: String,
        query: String,
        offset: Option<i64>,
        limit: Option<i64>,
    },
    LoadDshSession {
        request_id: u64,
        session_id: String,
        provider_session_id: String,
    },
    ListAntigravitySessions {
        request_id: u64,
        cwd: String,
        query: String,
        full_text: bool,
        offset: Option<i64>,
        limit: Option<i64>,
    },
    GetAntigravitySession {
        request_id: u64,
        cwd: String,
        provider_session_id: String,
        before_event_index: Option<i64>,
        around_event_index: Option<i64>,
        limit: Option<i64>,
    },
    SearchAntigravitySession {
        request_id: u64,
        cwd: String,
        provider_session_id: String,
        query: String,
        offset: Option<i64>,
        limit: Option<i64>,
    },
    LoadAntigravitySession {
        request_id: u64,
        session_id: String,
        provider_session_id: String,
    },
    GetEnvironmentDiagnostics {
        request_id: u64,
        cwd: PathBuf,
        #[serde(default)]
        repair: Option<crate::protocol::WorktreeRepairQuery>,
    },
    CheckGitRepo {
        request_id: u64,
        cwd: PathBuf,
    },
    GitStatusDetail {
        request_id: u64,
        cwd: PathBuf,
    },
    GitCheckout {
        request_id: u64,
        cwd: PathBuf,
        branch: String,
    },
    GitCreateBranch {
        request_id: u64,
        cwd: PathBuf,
        name: String,
        start_point: Option<String>,
    },
    GitCommit {
        request_id: u64,
        cwd: PathBuf,
        message: String,
        stage_all: bool,
    },
    GitPush {
        request_id: u64,
        cwd: PathBuf,
    },
    GitWorktreeCreate {
        request_id: u64,
        cwd: PathBuf,
        branch: String,
        path: Option<PathBuf>,
    },
    GitWorktreeRemove {
        request_id: u64,
        cwd: PathBuf,
        worktree_path: PathBuf,
    },
    GetEgressStatus {
        request_id: u64,
    },
    RecheckEgress {
        request_id: u64,
    },
    SetEgressExemption {
        request_id: u64,
        family: Family,
    },
    SetEgressRuleMode {
        request_id: u64,
        mode: RuleMode,
    },
    SetEgressProtectionEnabled {
        request_id: u64,
        enabled: bool,
    },
    AcceptEgressBaseline {
        request_id: u64,
        family: Family,
    },
    GetBrowserDiagnostics {
        request_id: u64,
    },
    EvaluateExternalLink {
        request_id: u64,
        url: Option<String>,
        #[serde(default)]
        sign_in: bool,
    },
    GetUpdateStatus {
        request_id: u64,
    },
    CheckForUpdate {
        request_id: u64,
    },
    DownloadUpdate {
        request_id: u64,
    },
    InstallUpdate {
        request_id: u64,
    },
    RunRetentionCleanup {
        request_id: u64,
    },
    GetClaudeAccounts {
        request_id: u64,
    },
    ClaudeAccountCommand {
        request_id: u64,
        action: crate::claude_accounts::ClaudeAccountCommand,
    },
    GetAccountSummary {
        request_id: u64,
    },
    GetAgentHud {
        request_id: u64,
        session_id: String,
    },
    ForkSession {
        scene_id: String,
        source_session_id: String,
        placement: ForkPlacement,
        name: String,
        worktree_mode: ForkWorktreeMode,
        submission_key: String,
    },
    RetryFork {
        session_id: String,
    },
    CreateWorkspace {
        root_directory: String,
    },
    SelectWorkspace {
        workspace_id: String,
    },
    RelinkWorkspace {
        workspace_id: String,
        root_directory: String,
    },
    SetWorkspacePinned {
        workspace_id: String,
        pinned: bool,
    },
    ReorderPinnedWorkspace {
        workspace_id: String,
        before_workspace_id: Option<String>,
    },
    RemoveWorkspace {
        workspace_id: String,
    },
    CreateTask {
        workspace_id: String,
    },
    SelectTask {
        task_id: String,
    },
    RenameTask {
        task_id: String,
        title: String,
    },
    SetSessionTitle {
        session_id: String,
        title: Option<String>,
    },
    SetTaskStatus {
        request_id: u64,
        workspace_id: String,
        task_id: String,
        status: String,
        before_task_id: Option<String>,
    },
    SetTaskPinned {
        task_id: String,
        pinned: bool,
    },
    ReorderPinnedTask {
        workspace_id: String,
        task_id: String,
        before_task_id: Option<String>,
    },
    DeleteTask {
        task_id: String,
    },
    CreateCanvas {
        task_id: String,
    },
    CreateShellSibling {
        scene_id: String,
        source_session_id: String,
        parent_session_id: Option<String>,
    },
    SelectScene {
        scene_id: String,
    },
    RenameScene {
        scene_id: String,
        name: String,
    },
    ReorderScene {
        scene_id: String,
        before_scene_id: Option<String>,
    },
    CloseScene {
        scene_id: String,
        confirmed: bool,
    },
    PreviewSceneClose {
        scene_id: String,
    },
    CloseSessionCard {
        scene_id: String,
        session_id: String,
        confirmed_intent: Option<String>,
        #[serde(default)]
        preserve_scene_on_last_session: bool,
    },
    RemoveSessionBranch {
        scene_id: String,
        session_id: String,
        #[serde(default)]
        scope: RemoveScope,
    },
    FocusSession {
        scene_id: String,
        session_id: String,
    },
    PutGeometry {
        sequence: u64,
        update: matou_native_core::geometry::GeometryUpdate,
    },
    Refresh,
    #[cfg(all(test, target_os = "macos"))]
    #[serde(skip)]
    TestPause {
        paused: Sender<()>,
        resume: Receiver<()>,
    },
    #[cfg(all(test, target_os = "macos"))]
    #[serde(skip)]
    TestPauseShellWorker {
        session_id: String,
        paused: Sender<()>,
        resume: Receiver<()>,
    },
    #[cfg(all(test, target_os = "macos"))]
    #[serde(skip)]
    TestFreezeShellWrites {
        session_id: String,
        frozen: bool,
        reply: Sender<Result<()>>,
    },
    #[cfg(all(test, target_os = "macos"))]
    #[serde(skip)]
    TestFailPromotionClear {
        session_id: String,
    },
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecoveryState {
    Deferred,
    Restoring,
    Ready,
    Failed(String),
    StartFailed(String),
    Restarting(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StartupBlockKind {
    ReadOnly,
    RecoveryRequired,
    MigrationChecksum,
    Configuration,
    Protocol,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartupBlock {
    pub kind: StartupBlockKind,
    pub message: String,
}

pub const STORAGE_RECOVERY_WAITING: &str = "STORAGE_READ_ONLY: database is waiting for recovery";
pub const STORAGE_READ_ONLY: &str = "STORAGE_READ_ONLY";
pub const STORAGE_READ_ONLY_MESSAGE: &str =
    "STORAGE_READ_ONLY: database is open in read-only recovery mode";
pub const STORAGE_READ_ONLY_TITLE: &str = "数据库处于只读恢复模式";
pub const DATABASE_RECOVERY_INTERRUPTED: &str = "数据库恢复操作未完成：Runtime 在恢复操作期间退出";
pub const DATABASE_RECOVERY_NOT_PENDING: &str = "当前没有待处理的数据库恢复操作";
pub const DATABASE_RECOVERY_BUSY: &str = "正在处理数据库恢复，请保持 Matou 开启…";

const MAIN_WINDOW_ID: &str = "main-window-1";

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StoragePhase {
    Writable,
    Recovery,
    ReadOnly,
}

impl StoragePhase {
    pub fn accepts(self, command: &Command) -> bool {
        match (self, command) {
            (_, Command::Shutdown) => true,
            (Self::Writable, Command::DatabaseRecovery { .. } | Command::ReadOnlyQuery { .. }) => {
                false
            }
            (Self::Writable, _) | (Self::Recovery, Command::DatabaseRecovery { .. }) => true,
            (
                Self::ReadOnly,
                Command::ReadOnlyQuery { .. }
                | Command::History(_)
                | Command::GetTimeZoneSettings { .. }
                | Command::ListDshSessions { .. }
                | Command::GetDshSession { .. }
                | Command::SearchDshSession { .. }
                | Command::ListAntigravitySessions { .. }
                | Command::GetAntigravitySession { .. }
                | Command::SearchAntigravitySession { .. }
                | Command::GetEnvironmentDiagnostics { .. }
                | Command::CheckGitRepo { .. }
                | Command::GitStatusDetail { .. }
                | Command::GetEgressStatus { .. }
                | Command::GetBrowserDiagnostics { .. }
                | Command::EvaluateExternalLink { .. }
                | Command::GetUpdateStatus { .. }
                | Command::GetAccountSummary { .. }
                | Command::GetAgentHud { .. },
            ) => true,
            (
                Self::ReadOnly,
                Command::DatabaseRecovery {
                    action: DatabaseRecoveryAction::ExportRecoveryBundle,
                    ..
                },
            ) => true,
            _ => false,
        }
    }
}

#[derive(Serialize, Deserialize)]
pub enum ReadOnlyQuery {
    Workspace {
        workspace_id: String,
    },
    Task {
        task_id: String,
    },
    Scene {
        scene_id: String,
    },
    Session {
        scene_id: String,
        session_id: String,
    },
}

#[derive(Serialize, Deserialize)]
pub struct ReadOnlyNavigation {
    pub hierarchy: HierarchyResult,
    pub scenes: Vec<Scene>,
    pub graph: Value,
    pub geometry: Vec<matou_native_core::geometry::GeometryUpdate>,
}

#[derive(Serialize, Deserialize)]
pub struct ReadOnlyProjection {
    pub workspaces: Vec<Workspace>,
    pub tasks: Vec<Task>,
    pub active: ReadOnlyNavigation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DatabaseRecoveryReason {
    Ownership,
    Wal,
    Integrity,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseRecoveryBackup {
    pub id: String,
    pub created_at: i64,
    pub schema_version: i64,
    pub size: u64,
    pub reason: BackupReason,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseRecoveryDetails {
    pub recovery_id: String,
    pub reason: DatabaseRecoveryReason,
    pub durable_database_path: PathBuf,
    pub quarantined_path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ownership_issue: Option<String>,
    pub backups: Vec<DatabaseRecoveryBackup>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RecoveryError>,
}

impl From<&RecoveryRequired> for DatabaseRecoveryDetails {
    fn from(recovery: &RecoveryRequired) -> Self {
        Self {
            recovery_id: recovery.recovery_id.clone(),
            reason: match recovery.reason.as_str() {
                "ownership-recovery-required" => DatabaseRecoveryReason::Ownership,
                "wal-recovery-required" => DatabaseRecoveryReason::Wal,
                _ => DatabaseRecoveryReason::Integrity,
            },
            durable_database_path: recovery.durable_database_path.clone(),
            quarantined_path: recovery.quarantined_path.clone(),
            ownership_issue: recovery.ownership_issue.clone(),
            backups: recovery
                .backups
                .iter()
                .map(|backup| DatabaseRecoveryBackup {
                    id: backup.id.clone(),
                    created_at: backup.created_at,
                    schema_version: backup.schema_version,
                    size: backup.size,
                    reason: backup.reason,
                    sha256: backup.sha256.clone(),
                })
                .collect(),
            error: {
                let errors = [
                    &recovery.backup_list_error,
                    &recovery.marker_error,
                    &recovery.move_error,
                ];
                let mut errors = errors.into_iter().flatten();
                errors.next().map(|first| {
                    let mut error = first.clone();
                    for next in errors {
                        error.message.push('；');
                        error.message.push_str(&next.message);
                    }
                    error
                })
            },
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum DatabaseRecoveryAction {
    RestoreBackup {
        backup_id: String,
        #[serde(default)]
        expected_recovery_id: String,
    },
    RetryOpen {
        #[serde(default)]
        expected_recovery_id: String,
    },
    StartEmptyDatabase {
        #[serde(default)]
        expected_recovery_id: String,
    },
    ExportRecoveryBundle,
}

impl DatabaseRecoveryAction {
    pub fn expected_recovery_id(&self) -> Option<&str> {
        match self {
            Self::RetryOpen {
                expected_recovery_id,
            }
            | Self::RestoreBackup {
                expected_recovery_id,
                ..
            }
            | Self::StartEmptyDatabase {
                expected_recovery_id,
            } => Some(expected_recovery_id),
            Self::ExportRecoveryBundle => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseRecoveryResult {
    pub request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exported_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<RecoveryError>,
}

impl DatabaseRecoveryResult {
    pub fn failed(request_id: String, code: &str, message: impl Into<String>) -> Self {
        Self {
            request_id,
            exported_path: None,
            error: Some(RecoveryError {
                code: code.into(),
                message: message.into(),
                retryable: true,
            }),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GitFailure {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GitReply {
    pub request_id: u64,
    pub result: Result<GitStatusDetail, GitFailure>,
    pub worktree_session_counts: BTreeMap<PathBuf, usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEnvironmentReply {
    pub request_id: u64,
    pub session_id: String,
    pub result: EnvironmentChangeResult,
}

#[derive(Serialize, Deserialize)]
pub enum Event {
    StartupBlocked(StartupBlock),
    DatabaseRecoveryRequired(DatabaseRecoveryDetails),
    DatabaseRecoveryResult(DatabaseRecoveryResult),
    StorageCommandRejected {
        request_id: Option<u64>,
        code: String,
        message: String,
    },
    RestoreSessionEnvironmentResult(SessionEnvironmentReply),
    LocateSessionEnvironmentResult(SessionEnvironmentReply),
    HandoffSessionEnvironmentResult(SessionEnvironmentReply),
    GitStatusDetail(GitReply),
    GitCheckout(GitReply),
    GitCreateBranch(GitReply),
    GitCommit(GitReply),
    GitPush(GitReply),
    GitWorktreeCreate(GitReply),
    GitWorktreeRemove(GitReply),
    ReadOnlyReady {
        reason: ReadOnlyReason,
        projection: Result<ReadOnlyProjection, String>,
    },
    ReadOnlyQueryResult {
        request_id: u64,
        result: Result<ReadOnlyNavigation, String>,
    },
    History(HistoryReply),
    TimeZoneSettings(TimeZoneSettingsReply),
    SearchResults {
        session_id: String,
        sequence: u64,
        result: Result<SearchResult, String>,
    },
    Recovery {
        session_id: String,
        state: RecoveryState,
    },
    DeferredRecoveryPreview {
        session_id: String,
        cwd: String,
        preview: Option<RecentReplies>,
    },
    Workspaces(Vec<Workspace>),
    WorkspacePaths(Vec<WorkspacePathState>),
    Tasks(Vec<Task>),
    WorkspaceActionResult {
        action: String,
        workspace_id: Option<String>,
        error: Option<String>,
    },
    TaskRenameResult {
        task_id: String,
        title: String,
        error: Option<String>,
    },
    SessionTitleResult {
        session_id: String,
        title: Option<String>,
        error: Option<String>,
    },
    Ready {
        hierarchy: HierarchyResult,
        scenes: Vec<Scene>,
        graph: Value,
        geometry: Vec<matou_native_core::geometry::GeometryUpdate>,
    },
    Navigation {
        hierarchy: HierarchyResult,
        scenes: Vec<Scene>,
        graph: Value,
        geometry: Vec<matou_native_core::geometry::GeometryUpdate>,
    },
    NavigationFailed {
        message: String,
        attempted_focus: Option<(String, String)>,
    },
    /// Session IDs retired by a navigation command, before its projection.
    ScenesDisposed(Vec<String>),
    SceneClosePreview {
        scene_id: String,
        graph: Value,
        is_last: bool,
    },
    GeometryResult {
        sequence: u64,
        update: matou_native_core::geometry::GeometryUpdate,
        error: Option<String>,
    },
    /// Graph with optional card, provider, catalog and new notification results.
    Graph(Value),
    ClipboardWrite {
        session_id: String,
        text: String,
    },
    Copied {
        request_id: u64,
        text: String,
    },
    Frame {
        session_id: String,
        screen: Screen,
        selection: Option<GridSelection>,
        modes: InputModes,
        sequence: u64,
        /// All absolute scroll requests up to this id preceded the view read.
        scroll_request_id: u64,
    },
    StorageFault {
        session_id: String,
        message: String,
    },
    StorageRecovered {
        session_id: String,
    },
    Diagnostic(String),
    Closed,
}

/// Offline adapters supplied by the host's caller, never by IPC.
#[derive(Default)]
pub struct OfflineFixtures {
    pub egress_sample: Option<Box<dyn FnMut() -> Result<EgressSample> + Send>>,
    pub dsh_session_root: Option<PathBuf>,
    pub provider_environment: Option<BTreeMap<OsString, OsString>>,
    pub fork_worktree_root: Option<PathBuf>,
    pub fork_setup: Option<Box<dyn Fn(&Path) -> Result<()> + Send>>,
    pub browser_executable: Option<PathBuf>,
    pub claude_accounts_engine: Option<PathBuf>,
    pub update: Option<OfflineUpdate>,
    pub retention: Option<OfflineRetention>,
}

pub struct OfflineUpdate {
    pub manifest: update_download::Manifest,
    pub artifact: Vec<u8>,
}

pub struct OfflineRetention {
    pub directory: tempfile::TempDir,
    pub sessions: Vec<(String, SessionRetention, Vec<IndexedCheckpoint>)>,
}

impl update_download::UpdateFetcher for OfflineUpdate {
    fn installer_extension(&self) -> &'static str {
        if cfg!(windows) { "exe" } else { "dmg" }
    }

    fn check(
        &self,
        _: &url::Url,
    ) -> std::result::Result<update_download::Manifest, update_download::UpdateError> {
        Ok(self.manifest.clone())
    }

    fn fetch(
        &self,
        _: &update_download::ManualDownload,
    ) -> std::io::Result<update_download::DownloadResponse> {
        Ok(update_download::DownloadResponse {
            status: 200,
            content_length: Some(self.artifact.len() as u64),
            body: Some(Box::new(std::io::Cursor::new(self.artifact.clone()))),
        })
    }

    fn download_automatic(
        &self,
        _: &update_download::Release,
        _: &mut dyn FnMut(update_download::Progress),
    ) -> std::result::Result<(), update_download::DownloadError> {
        Err(update_download::DownloadError::Rejected(
            "Offline updates use manual verification".into(),
        ))
    }
}

struct RuntimeWiring {
    fixtures: OfflineFixtures,
    dsh: Option<DshSessionCatalog>,
    antigravity: AntigravityCatalog,
    update: Arc<update_download::UpdateManager>,
    live_update: Option<LiveUpdate>,
    next_egress_attempt: u64,
    live_egress: Option<LiveEgress>,
    claude_accounts: Option<crate::claude_accounts::AccountWorker>,
    subscriptions: Option<Arc<SubscriptionService>>,
}

impl Drop for RuntimeWiring {
    fn drop(&mut self) {
        if let Some(service) = &self.subscriptions {
            service.shutdown();
        }
    }
}

struct LiveUpdate {
    platform: Platform,
    open_installers: bool,
    worker: Option<mpsc::Sender<(&'static str, u64)>>,
}

impl LiveUpdate {
    fn queue(
        &mut self,
        kind: &'static str,
        request_id: u64,
        update: &Arc<update_download::UpdateManager>,
        data_root: &Path,
        events: &async_channel::Sender<Event>,
    ) -> Result<()> {
        if self.worker.is_none() {
            let (commands, requests) = mpsc::channel();
            let update = Arc::clone(update);
            let events = events.clone();
            let data_root = data_root.to_owned();
            let files = update_download::LocalUpdateFiles::new(data_root.join("updates"));
            let platform = self.platform;
            let open_installers = self.open_installers;
            thread::Builder::new()
                .name("matou-update".into())
                .spawn(move || {
                    let fetcher = update_http::HttpUpdateFetcher::new(platform);
                    for (kind, request_id) in requests {
                        let result = if kind == "updateInstallResult" {
                            install_update(&update, &data_root, open_installers)
                        } else {
                            match &fetcher {
                                Ok(fetcher) => {
                                    if kind == "updateCheckResult" {
                                        update.check(
                                            fetcher,
                                            || now() as u64,
                                            |ms| thread::sleep(Duration::from_millis(ms)),
                                            |_| {},
                                        );
                                    } else {
                                        update.download(fetcher, &files, || now() as u64, |_| {});
                                    }
                                    Ok(json!(update.state()))
                                }
                                Err(error) => {
                                    Err(anyhow!("Could not start update worker: {error}"))
                                }
                            }
                        };
                        if events
                            .send_blocking(wiring_result(kind, request_id, result))
                            .is_err()
                        {
                            break;
                        }
                    }
                })?;
            self.worker = Some(commands);
        }
        self.worker
            .as_ref()
            .unwrap()
            .send((kind, request_id))
            .context("Update worker is unavailable")
    }
}

fn install_update(
    update: &update_download::UpdateManager,
    data_root: &Path,
    open_installers: bool,
) -> Result<Value> {
    update.open_downloaded_installer(data_root, |path| {
        if open_installers {
            update_download::open_installer(path)?;
        }
        Ok(())
    })?;
    Ok(json!({"ok": true}))
}

struct LiveEgress {
    sampler: LiveEgressSampler,
    next_check: Instant,
    in_flight: bool,
    first_sample: bool,
    status_requests: Vec<u64>,
    recheck_requests: Vec<u64>,
}

impl LiveEgress {
    fn start(
        data_root: PathBuf,
        subscriptions: Option<Arc<SubscriptionService>>,
    ) -> Result<Option<Self>> {
        let Some(sampler) = LiveEgressSampler::start(data_root, subscriptions)? else {
            return Ok(None);
        };
        let mut live = Self {
            sampler,
            next_check: Instant::now() + egress_probe::LIVE_RECHECK_INTERVAL,
            in_flight: false,
            first_sample: true,
            status_requests: Vec::new(),
            recheck_requests: Vec::new(),
        };
        live.request_check()?;
        Ok(Some(live))
    }

    fn request_check(&mut self) -> Result<()> {
        if !self.in_flight {
            self.sampler.request()?;
            self.in_flight = true;
        }
        Ok(())
    }

    fn handle(&mut self, command: &Command) -> bool {
        match command {
            // Desktop asks once when Ready arrives. Keep that request pending
            // until the first sample so the existing reply updates its footer.
            Command::GetEgressStatus { request_id } if self.first_sample => {
                self.status_requests.push(*request_id);
            }
            Command::RecheckEgress { request_id } => {
                self.recheck_requests.push(*request_id);
            }
            _ => return false,
        }
        true
    }

    fn poll(&mut self, owner: &mut ShellOwner) -> Result<Option<Result<Value>>> {
        match self.sampler.samples.try_recv() {
            Ok(sample) => {
                self.in_flight = false;
                self.first_sample = false;
                return Ok(Some(
                    owner
                        .apply_egress_sample(sample)
                        .map(|()| egress_projection(owner.egress().status())),
                ));
            }
            Err(mpsc::TryRecvError::Disconnected) => bail!("Egress sampling worker stopped"),
            Err(mpsc::TryRecvError::Empty) => {}
        }
        let time = Instant::now();
        let periodic = !self.in_flight && self.next_check <= time;
        if periodic {
            self.next_check = time + egress_probe::LIVE_RECHECK_INTERVAL;
        }
        if periodic
            || !self.recheck_requests.is_empty()
            || owner
                .egress()
                .recovery_deadline()
                .is_some_and(|due| due <= now() as u64)
        {
            self.request_check()?;
        }
        Ok(None)
    }

    fn replies(&mut self, result: Result<Value>) -> Vec<Event> {
        let reply = |kind, request_id| {
            wiring_result(
                kind,
                request_id,
                result
                    .as_ref()
                    .cloned()
                    .map_err(|error| anyhow!("{error:#}")),
            )
        };
        let mut replies = vec![reply("egressStatusChanged", 0)];
        replies.extend(
            self.status_requests
                .drain(..)
                .map(|id| reply("egressStatusResult", id)),
        );
        replies.extend(
            self.recheck_requests
                .drain(..)
                .map(|id| reply("egressRecheckResult", id)),
        );
        replies
    }
}

fn wiring_result(kind: &str, request_id: u64, result: Result<Value>) -> Event {
    Event::Graph(match result {
        Ok(result) => json!({"type": kind, "requestId": request_id, "result": result}),
        Err(error) => json!({"type": kind, "requestId": request_id, "error": format!("{error:#}")}),
    })
}

fn git_reply_event(command: &Command) -> Option<(u64, fn(GitReply) -> Event)> {
    Some(match command {
        Command::GitStatusDetail { request_id, .. } => (*request_id, Event::GitStatusDetail),
        Command::GitCheckout { request_id, .. } => (*request_id, Event::GitCheckout),
        Command::GitCreateBranch { request_id, .. } => (*request_id, Event::GitCreateBranch),
        Command::GitCommit { request_id, .. } => (*request_id, Event::GitCommit),
        Command::GitPush { request_id, .. } => (*request_id, Event::GitPush),
        Command::GitWorktreeCreate { request_id, .. } => (*request_id, Event::GitWorktreeCreate),
        Command::GitWorktreeRemove { request_id, .. } => (*request_id, Event::GitWorktreeRemove),
        _ => return None,
    })
}

fn git_session_directories(data_root: &Path) -> Result<Vec<Vec<PathBuf>>> {
    let connection = startup::open_read_only(&data_root.join("matou.sqlite"))?;
    let mut statement = connection.prepare(
        "SELECT session.cwd, context.cwd
         FROM sessions AS session
         LEFT JOIN execution_contexts AS context ON context.id = session.execution_context_id
         WHERE session.archived_at IS NULL",
    )?;
    let rows = statement.query_map([], |row| {
        Ok([row.get::<_, Option<String>>(0)?, row.get(1)?])
    })?;
    rows.map(|row| {
        Ok(row?
            .into_iter()
            .flatten()
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .map(|path| path.canonicalize().unwrap_or(path))
            .collect())
    })
    .collect()
}

fn git_session_count(directories: &[Vec<PathBuf>], worktree: &Path) -> usize {
    directories
        .iter()
        .filter(|session| session.iter().any(|cwd| cwd.starts_with(worktree)))
        .count()
}

fn run_git_command(command: &Command, data_root: &Path) -> Option<Event> {
    let (request_id, event) = git_reply_event(command)?;
    let result = (|| {
        let status = match command {
            Command::GitStatusDetail { cwd, .. } => git_repo::git_status_detail(cwd),
            Command::GitCheckout { cwd, branch, .. } => git_repo::git_checkout(cwd, branch),
            Command::GitCreateBranch {
                cwd,
                name,
                start_point,
                ..
            } => git_repo::git_create_branch(cwd, name, start_point.as_deref()),
            Command::GitCommit {
                cwd,
                message,
                stage_all,
                ..
            } => git_repo::git_commit(cwd, message, *stage_all),
            Command::GitPush { cwd, .. } => git_repo::git_push(cwd),
            Command::GitWorktreeCreate {
                cwd, branch, path, ..
            } => {
                let default_path;
                let path = if let Some(path) = path {
                    path
                } else {
                    let root = data_root.join("worktrees");
                    std::fs::create_dir_all(&root)?;
                    ensure!(
                        !root.symlink_metadata()?.is_symlink(),
                        "Worktree directory is a symlink"
                    );
                    default_path = root.join(id());
                    &default_path
                };
                git_repo::git_worktree_create(cwd, branch, Some(path))
            }
            Command::GitWorktreeRemove {
                cwd, worktree_path, ..
            } => {
                let path = cwd.join(worktree_path);
                let target = path.canonicalize().unwrap_or(path);
                if git_session_count(&git_session_directories(data_root)?, &target) > 0 {
                    return Err(GitActionError("GIT_WORKTREE_IN_USE").into());
                }
                git_repo::git_worktree_remove(cwd, worktree_path)
            }
            _ => unreachable!("not a Git command"),
        }?;
        Ok(status)
    })();
    let worktree_session_counts = result
        .as_ref()
        .ok()
        .and_then(|status| {
            let directories = git_session_directories(data_root).ok()?;
            Some(
                status
                    .worktrees
                    .iter()
                    .map(|worktree| {
                        (
                            worktree.path.clone(),
                            git_session_count(&directories, &worktree.path),
                        )
                    })
                    .collect(),
            )
        })
        .unwrap_or_default();
    Some(event(GitReply {
        request_id,
        result: result.map_err(|error: anyhow::Error| {
            let code = error
                .downcast_ref::<GitActionError>()
                .map_or("GIT_OPERATION_FAILED", |error| error.0);
            GitFailure {
                code: code.into(),
                message: format!("{error:#}"),
            }
        }),
        worktree_session_counts,
    }))
}

// Desktop treats any Graph event as a replacement for its active scene.
fn with_graph_projection(reply: Event, mut projection: Value) -> Event {
    match reply {
        Event::Graph(Value::Object(fields)) => {
            if let Some(graph) = projection.as_object_mut() {
                graph.extend(fields);
                Event::Graph(projection)
            } else {
                Event::Graph(Value::Object(fields))
            }
        }
        other => other,
    }
}

impl RuntimeWiring {
    fn new(fixtures: OfflineFixtures, shell_home: &Path, live_services: bool) -> Result<Self> {
        let dsh = fixtures
            .dsh_session_root
            .clone()
            .or_else(dsh_catalog::default_session_root)
            .map(DshSessionCatalog::new);
        let environment = fixtures
            .provider_environment
            .clone()
            .unwrap_or_else(|| std::env::vars_os().collect());
        let mut antigravity =
            AntigravityCatalog::new(antigravity_catalog::catalog_root(shell_home, &environment));
        if antigravity_launch::env_value(&environment, "MATOU_LOCALE")
            .is_some_and(|value| value == "en")
        {
            antigravity = antigravity.with_english_titles();
        }
        let platform = if cfg!(target_os = "macos") {
            Platform::Darwin
        } else if cfg!(windows) {
            Platform::Win32
        } else {
            Platform::Linux
        };
        let (settings, live_update) = if fixtures.update.is_some() {
            (
                update_http::UpdateSettings {
                    feed: FeedConfiguration::resolve(None, None, None),
                    version: None,
                    enabled: update_policy::update_checks_enabled(true, None),
                },
                None,
            )
        } else {
            let executable = std::env::current_exe().ok();
            let settings =
                update_http::UpdateSettings::for_executable(executable.as_deref(), |key| {
                    antigravity_launch::env_value(&environment, key)
                        .and_then(|value| value.to_str())
                        .map(String::from)
                });
            let live = LiveUpdate {
                platform,
                // The hidden desktop probes launch the standalone Runtime.
                open_installers: live_services
                    && !cfg!(test)
                    && antigravity_launch::env_value(&environment, "MATOU_DAG_WINDOW_PROBE")
                        .is_none()
                    && antigravity_launch::env_value(&environment, "MATOU_WINDOW_REGISTRY_PROBE")
                        .is_none(),
                worker: None,
            };
            (settings, Some(live))
        };
        let version = settings
            .version
            .as_deref()
            .unwrap_or(env!("CARGO_PKG_VERSION"));
        let update = Arc::new(update_download::UpdateManager::new_live(
            CurrentVersion::parse(version)?,
            update_download::UpdateOptions {
                feed: settings.feed,
                platform,
                arch: if cfg!(target_arch = "aarch64") {
                    Architecture::Arm64
                } else {
                    Architecture::X64
                },
                enabled: settings.enabled,
                install_mode: InstallMode::Manual,
                locale: UpdateLocale::default(),
            },
        ));
        Ok(Self {
            fixtures,
            dsh,
            antigravity,
            update,
            live_update,
            next_egress_attempt: 0,
            live_egress: None,
            claude_accounts: None,
            subscriptions: None,
        })
    }

    fn dispatch_live_update(
        &mut self,
        command: &Command,
        data_root: &Path,
        events: &async_channel::Sender<Event>,
    ) -> bool {
        let Some(live) = &mut self.live_update else {
            return false;
        };
        let (kind, request_id) = match command {
            Command::CheckForUpdate { request_id } => ("updateCheckResult", *request_id),
            Command::DownloadUpdate { request_id } => ("updateDownloadResult", *request_id),
            Command::InstallUpdate { request_id } => ("updateInstallResult", *request_id),
            _ => return false,
        };
        if let Err(error) = live.queue(kind, request_id, &self.update, data_root, events) {
            let _ = events.send_blocking(wiring_result(kind, request_id, Err(error)));
        }
        true
    }

    fn send_claude_account(
        &self,
        request_id: u64,
        action: crate::claude_accounts::ClaudeAccountCommand,
        send: &async_channel::Sender<Event>,
        projection: impl FnOnce() -> Result<Value>,
    ) -> Result<()> {
        if !self
            .claude_accounts
            .as_ref()
            .is_some_and(|worker| worker.send(request_id, action))
        {
            let reply = wiring_result(
                "claudeAccountsResult",
                request_id,
                Err(anyhow::anyhow!("runtime")),
            );
            send.send_blocking(with_graph_projection(reply, projection()?))?;
        }
        Ok(())
    }

    fn catalog(&mut self) -> Result<&mut DshSessionCatalog> {
        self.dsh
            .as_mut()
            .context("DSH session directory is unavailable")
    }

    fn poll_egress(&mut self, owner: &mut ShellOwner) -> Result<()> {
        let time = now() as u64;
        if time >= self.next_egress_attempt
            && owner
                .egress()
                .recovery_deadline()
                .is_some_and(|deadline| deadline <= time)
            && let Some(sampler) = &mut self.fixtures.egress_sample
        {
            self.next_egress_attempt = time.saturating_add(egress_probe::RECOVERY_CONFIRM_MS);
            owner.apply_egress_sample(sampler()?)?;
        }
        Ok(())
    }

    fn handle(
        &mut self,
        command: &Command,
        data_root: &Path,
        owner: Option<&mut ShellOwner>,
    ) -> Option<Event> {
        if let Some(reply) = run_git_command(command, data_root) {
            return Some(reply);
        }
        let (kind, request_id, result) = match command {
            Command::ListDshSessions {
                request_id,
                cwd,
                query,
                full_text,
                offset,
                limit,
            } => (
                "dshSessionListResult",
                *request_id,
                (|| {
                    Ok(serde_json::to_value(self.catalog()?.list(
                        Path::new(cwd),
                        dsh_catalog::ListOptions {
                            query,
                            scope: if *full_text {
                                dsh_catalog::SearchScope::All
                            } else {
                                dsh_catalog::SearchScope::Metadata
                            },
                            offset: *offset,
                            limit: *limit,
                            ..Default::default()
                        },
                    )?)?)
                })(),
            ),
            Command::GetDshSession {
                request_id,
                cwd,
                provider_session_id,
                before_event_index,
                around_event_index,
                limit,
            } => (
                "dshSessionDetailResult",
                *request_id,
                (|| {
                    Ok(serde_json::to_value(self.catalog()?.detail(
                        Path::new(cwd),
                        provider_session_id,
                        dsh_catalog::DetailOptions {
                            before_event_index: *before_event_index,
                            around_event_index: *around_event_index,
                            limit: *limit,
                            ..Default::default()
                        },
                    )?)?)
                })(),
            ),
            Command::SearchDshSession {
                request_id,
                cwd,
                provider_session_id,
                query,
                offset,
                limit,
            } => (
                "dshSessionSearchResult",
                *request_id,
                (|| {
                    Ok(serde_json::to_value(self.catalog()?.search(
                        Path::new(cwd),
                        provider_session_id,
                        query,
                        *offset,
                        *limit,
                    )?)?)
                })(),
            ),
            Command::ListAntigravitySessions {
                request_id,
                cwd,
                query,
                full_text,
                offset,
                limit,
            } => (
                "antigravitySessionListResult",
                *request_id,
                (|| {
                    Ok(serde_json::to_value(self.antigravity.list(
                        Path::new(cwd),
                        antigravity_catalog::ListOptions {
                            query,
                            scope: if *full_text {
                                antigravity_catalog::SearchScope::All
                            } else {
                                antigravity_catalog::SearchScope::Metadata
                            },
                            offset: offset.unwrap_or_default(),
                            limit: *limit,
                            ..Default::default()
                        },
                    )?)?)
                })(),
            ),
            Command::GetAntigravitySession {
                request_id,
                cwd,
                provider_session_id,
                before_event_index,
                around_event_index,
                limit,
            } => (
                "antigravitySessionDetailResult",
                *request_id,
                (|| {
                    Ok(serde_json::to_value(self.antigravity.detail(
                        Path::new(cwd),
                        provider_session_id,
                        antigravity_catalog::DetailOptions {
                            before_event_index: *before_event_index,
                            around_event_index: *around_event_index,
                            limit: *limit,
                            ..Default::default()
                        },
                    )?)?)
                })(),
            ),
            Command::SearchAntigravitySession {
                request_id,
                cwd,
                provider_session_id,
                query,
                offset,
                limit,
            } => (
                "antigravitySessionSearchResult",
                *request_id,
                (|| {
                    Ok(serde_json::to_value(self.antigravity.search(
                        Path::new(cwd),
                        provider_session_id,
                        query,
                        offset.unwrap_or_default(),
                        *limit,
                    )?)?)
                })(),
            ),
            Command::GetEnvironmentDiagnostics {
                request_id,
                cwd,
                repair,
            } => (
                "environmentDiagnosticsResult",
                *request_id,
                environment_diagnostics(cwd, repair.as_ref()),
            ),
            Command::CheckGitRepo { request_id, cwd } => (
                "gitRepoResult",
                *request_id,
                Repository::discover(cwd).and_then(|repo| {
                    let branches = repo.as_ref().and_then(|repo| repo.local_branches().ok());
                    let mut value = git_projection(repo);
                    if let Some(branches) = branches {
                        value["branches"] = json!(
                            branches
                                .branches
                                .into_iter()
                                .map(|branch| json!({
                                    "name": branch.name, "current": branch.current,
                                    "checkedOutPath": branch.checked_out_path,
                                }))
                                .collect::<Vec<_>>()
                        );
                    }
                    Ok(value)
                }),
            ),
            Command::GetEgressStatus { request_id } => {
                let status = owner.map_or_else(
                    || EgressMonitor::default().status(),
                    |owner| owner.egress().status(),
                );
                (
                    "egressStatusResult",
                    *request_id,
                    Ok(egress_projection(status)),
                )
            }
            Command::RecheckEgress { request_id } => {
                let result = (|| {
                    let owner = owner.context("Runtime is read-only")?;
                    let sample = self
                        .fixtures
                        .egress_sample
                        .as_mut()
                        .context("Offline egress sample unavailable")?(
                    )?;
                    owner.apply_egress_sample(sample)?;
                    Ok(egress_projection(owner.egress().status()))
                })();
                ("egressRecheckResult", *request_id, result)
            }
            Command::SetEgressExemption { request_id, family } => {
                let result = (|| {
                    let owner = owner.context("Runtime is read-only")?;
                    owner.exempt_egress(*family)?;
                    Ok(egress_projection(owner.egress().status()))
                })();
                ("egressExemptionResult", *request_id, result)
            }
            Command::SetEgressRuleMode { request_id, mode } => {
                let result = (|| {
                    let owner = owner.context("Runtime is read-only")?;
                    owner.set_egress_rule_mode(*mode)?;
                    Ok(egress_projection(owner.egress().status()))
                })();
                ("egressRuleModeResult", *request_id, result)
            }
            Command::SetEgressProtectionEnabled {
                request_id,
                enabled,
            } => {
                let result = (|| {
                    let owner = owner.context("Runtime is read-only")?;
                    owner.set_egress_protection(*enabled)?;
                    Ok(egress_projection(owner.egress().status()))
                })();
                ("egressProtectionEnabledResult", *request_id, result)
            }
            Command::AcceptEgressBaseline { request_id, family } => {
                let result = (|| {
                    let owner = owner.context("Runtime is read-only")?;
                    owner.accept_egress_baseline(*family)?;
                    Ok(egress_projection(owner.egress().status()))
                })();
                ("egressBaselineResult", *request_id, result)
            }
            Command::GetBrowserDiagnostics { request_id } => (
                "browserDiagnosticsResult",
                *request_id,
                Ok(browser_diagnostics(
                    data_root,
                    self.fixtures.browser_executable.as_deref(),
                )),
            ),
            Command::EvaluateExternalLink {
                request_id,
                url,
                sign_in,
            } => {
                let source = if *sign_in {
                    LinkSource::SignIn
                } else {
                    LinkSource::External
                };
                let result = match link_routing::route_link(url.as_deref(), source) {
                    Ok(link) => {
                        json!({"allowed": true, "url": link.url(), "route": "controlled-browser"})
                    }
                    Err(reason) => json!({"allowed": false, "reason": format!("{reason:?}")}),
                };
                ("externalLinkResult", *request_id, Ok(result))
            }
            Command::GetUpdateStatus { request_id } => (
                "updateStatusResult",
                *request_id,
                Ok(json!(self.update.state())),
            ),
            Command::CheckForUpdate { request_id } => {
                let result = (|| {
                    let fixture = self
                        .fixtures
                        .update
                        .as_ref()
                        .context("Offline update fixture unavailable")?;
                    self.update.check(fixture, || now() as u64, |_| {}, |_| {});
                    Ok(json!(self.update.state()))
                })();
                ("updateCheckResult", *request_id, result)
            }
            Command::DownloadUpdate { request_id } => {
                let result = (|| {
                    let fixture = self
                        .fixtures
                        .update
                        .as_ref()
                        .context("Offline update fixture unavailable")?;
                    let files = update_download::LocalUpdateFiles::new(data_root.join("updates"));
                    self.update
                        .download(fixture, &files, || now() as u64, |_| {});
                    Ok(json!(self.update.state()))
                })();
                ("updateDownloadResult", *request_id, result)
            }
            Command::InstallUpdate { request_id } => (
                "updateInstallResult",
                *request_id,
                install_update(&self.update, data_root, false),
            ),
            Command::RunRetentionCleanup { request_id } => {
                ("retentionCleanupResult", *request_id, self.cleanup())
            }
            Command::GetAccountSummary { request_id } => (
                "accountSummaryResult",
                *request_id,
                account_summary(data_root),
            ),
            Command::GetAgentHud {
                request_id,
                session_id,
            } => (
                "agentHudResult",
                *request_id,
                owner.map_or_else(
                    || Ok(Value::Null),
                    |owner| owner.agent_hud(session_id, data_root),
                ),
            ),
            _ => return None,
        };
        Some(wiring_result(kind, request_id, result))
    }

    fn cleanup(&self) -> Result<Value> {
        let fixture = self
            .fixtures
            .retention
            .as_ref()
            .context("Detached retention fixture unavailable")?;
        let sessions = fixture
            .sessions
            .iter()
            .map(|(id, retention, checkpoints)| IndexedSession {
                id,
                retention: *retention,
                checkpoints,
            })
            .collect::<Vec<_>>();
        let quota = RetentionQuota::default();
        let plan = retention_policy::cleanup_isolated(fixture.directory.path(), quota, &sessions)?;
        let actions = plan
            .actions
            .iter()
            .map(|action| {
                json!({
                    "sessionId": action.session_id, "generation": action.generation,
                    "relativePath": action.relative_path, "bytes": action.bytes,
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({
            "globalQuotaBytes": quota.global_bytes, "sessionQuotaBytes": quota.per_session_bytes,
            "beforeBytes": plan.before_bytes, "reclaimedBytes": plan.reclaimed_bytes,
            "remainingGlobalOverage": plan.remaining_global_overage,
            "remainingSessionOverages": plan.remaining_session_overages, "actions": actions,
        }))
    }
}

fn browser_diagnostics(data_root: &Path, executable: Option<&Path>) -> Value {
    let executable = browser_integration::browser_executable(executable.map(Path::as_os_str));
    let status = browser_integration::detect_browser(
        &executable,
        browser_integration::inspect_executable,
        |path| {
            #[cfg(target_os = "macos")]
            {
                let output = std::process::Command::new("/usr/bin/plutil")
                    .args(["-extract", "CFBundleShortVersionString", "raw", "-o", "-"])
                    .arg(path)
                    .output()?;
                if output.status.success() {
                    return String::from_utf8(output.stdout).map_err(std::io::Error::other);
                }
            }
            let _ = path;
            Err(std::io::Error::other("Browser version unavailable"))
        },
    );
    let label = status.label(current_interface_locale(read_locale_preference(data_root)));
    let details = match status {
        browser_integration::BrowserStatus::Installed { version } => {
            json!({"installed": true, "version": version})
        }
        browser_integration::BrowserStatus::Unavailable(diagnostic) => {
            json!({"installed": false, "diagnostic": format!("{diagnostic:?}")})
        }
    };
    json!({"executable": executable, "label": label, "status": details})
}

fn account_summary(data_root: &Path) -> Result<Value> {
    let (accounts, state) = AccountStore::project_data_root(data_root, now() as u64)
        .map_err(|_| anyhow!("Could not read the account snapshot"))?;
    #[cfg(target_os = "macos")]
    let macos_major = std::process::Command::new("/usr/bin/sw_vers")
        .arg("-productVersion")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|version| version.trim().split('.').next()?.parse().ok());
    #[cfg(not(target_os = "macos"))]
    let macos_major = None;
    Ok(json!({
        "accountSummary": accounts,
        "loadState": match state {
            matou_native_core::account_store::LoadState::Loaded => "loaded",
            matou_native_core::account_store::LoadState::Missing => "missing",
            matou_native_core::account_store::LoadState::Corrupt => "corrupt",
        },
        "subscriptionSupported": subscription_launch::subscription_supported(cfg!(target_os = "macos"), macos_major),
    }))
}

fn egress_projection(status: egress_probe::Status) -> Value {
    let mut frozen = BTreeSet::new();
    let families = status
        .families
        .iter()
        .map(|family| {
            frozen.extend(family.frozen_session_ids.iter().cloned());
            json!({
                "family": family.evaluation.family, "state": family.evaluation.state,
                "exit": family.evaluation.exit, "violation": family.evaluation.violation,
                "hosts": family.hosts, "baseline": family.baseline, "overridden": family.overridden,
                "frozenSessionIds": family.frozen_session_ids, "recovering": family.recovering,
            })
        })
        .collect::<Vec<_>>();
    let transition = status.last_transition.map(|transition| {
        json!({
            "id": transition.id, "kind": match transition.kind {
                egress_probe::TransitionKind::Frozen => "frozen",
                egress_probe::TransitionKind::Resumed => "resumed",
            },
            "family": transition.family, "sessionCount": transition.session_count,
            "at": transition.at, "reason": transition.reason,
        })
    });
    json!({
        "state": status.state, "families": families, "frozenSessionIds": frozen,
        "checks": status.checks, "ruleMode": status.rule_mode, "checkedAt": status.checked_at,
        "protectionEnabled": status.protection_enabled, "sessionFamilies": status.session_families,
        "lastTransition": transition,
    })
}

fn git_projection(repository: Option<Repository>) -> Value {
    match repository {
        Some(repo) => {
            json!({"isRepository": true, "root": repo.root(), "linkedWorktree": repo.is_linked_worktree()})
        }
        None => json!({"isRepository": false}),
    }
}

fn environment_diagnostics(
    cwd: &Path,
    repair: Option<&crate::protocol::WorktreeRepairQuery>,
) -> Result<Value> {
    let ready = if let Some(repair) = repair {
        let repo = Repository::discover(&repair.repository_root)?
            .context("Worktree repository unavailable")?;
        let scope = WorktreeScope::new(&repair.isolation_root, &repo)?;
        let head = Head::Branch(repair.branch.clone());
        let expectation = WorktreeExpectation {
            repository_root: repo.root(),
            path: cwd,
            head: &head,
        };
        match environment_runtime::check_repair(&scope, expectation, &repair.candidate) {
            Ok(environment_runtime::RepairReadiness::Ready(ready)) => Ok(ready),
            Ok(environment_runtime::RepairReadiness::RepairableMoved { cwd }) => {
                return Ok(json!({"state": "repairable-moved", "cwd": cwd}));
            }
            Err(problem) => Err(problem),
        }
    } else {
        environment_runtime::probe_environment(EnvironmentTarget::Local(cwd))
    };
    Ok(match ready {
        Ok(ready) => {
            let git = ready.git.map(|git| {
                let mut value = git_projection(Some(git.repository));
                value["head"] = match git.head {
                    Head::Branch(branch) => json!({"branch": branch}),
                    Head::Detached(revision) => json!({"revision": revision}),
                };
                value["changes"] = json!({
                    "staged": git.changes.staged, "unstaged": git.changes.unstaged,
                    "untracked": git.changes.untracked,
                });
                value
            });
            json!({"state": "ready", "cwd": ready.cwd, "git": git})
        }
        Err(environment_runtime::EnvironmentProblem::Missing(reason)) => {
            json!({"state": "missing", "reason": format!("{reason:?}")})
        }
        Err(environment_runtime::EnvironmentProblem::Unavailable(reason)) => {
            json!({"state": "unavailable", "reason": format!("{reason:?}")})
        }
    })
}

pub struct RuntimeHost {
    pub commands: Sender<Command>,
    pub events: async_channel::Receiver<Event>,
    shutdown_requested: Arc<OnceLock<bool>>,
    thread: Option<JoinHandle<Result<()>>>,
}

impl RuntimeHost {
    pub fn start(options: LaunchOptions) -> Result<Self> {
        Self::start_inner(options, OfflineFixtures::default(), true)
    }

    pub fn start_with_offline_fixtures(
        options: LaunchOptions,
        fixtures: OfflineFixtures,
    ) -> Result<Self> {
        Self::start_inner(options, fixtures, false)
    }

    fn start_inner(
        options: LaunchOptions,
        fixtures: OfflineFixtures,
        live_services: bool,
    ) -> Result<Self> {
        LazyLock::force(&PERMISSION_STARTUP_WINDOW);
        let (commands, incoming) = mpsc::channel();
        let (send, events) = async_channel::unbounded();
        let shutdown_requested = Arc::new(OnceLock::new());
        let domain_shutdown = Arc::clone(&shutdown_requested);
        let thread = thread::Builder::new()
            .name("matou-domain-owner".into())
            .spawn(move || {
                let result = run(
                    options,
                    incoming,
                    &send,
                    &domain_shutdown,
                    fixtures,
                    live_services,
                );
                if let Err(error) = &result {
                    let _ = send.send_blocking(Event::Diagnostic(format!("{error:#}")));
                }
                let _ = send.send_blocking(Event::Closed);
                result
            })?;
        Ok(Self {
            commands,
            events,
            shutdown_requested,
            thread: Some(thread),
        })
    }

    /// Lets the input thread request shutdown while the caller still owns the join handle.
    pub fn shutdown_request(&self) -> impl FnOnce(bool) + Send + use<> {
        let commands = self.commands.clone();
        let shutdown_requested = Arc::clone(&self.shutdown_requested);
        move |clean_exit| request_shutdown(&commands, &shutdown_requested, clean_exit)
    }

    pub fn shutdown(&mut self) -> Result<()> {
        self.shutdown_and_join(false)
    }

    pub fn shutdown_clean(&mut self) -> Result<()> {
        self.shutdown_and_join(true)
    }

    fn shutdown_and_join(&mut self, clean_exit: bool) -> Result<()> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        request_shutdown(&self.commands, &self.shutdown_requested, clean_exit);
        thread
            .join()
            .map_err(|_| anyhow!("native runtime thread panicked"))?
    }
}

impl Drop for RuntimeHost {
    fn drop(&mut self) {
        let _ = self.shutdown_and_join(false);
    }
}

fn request_shutdown(commands: &Sender<Command>, requested: &OnceLock<bool>, clean_exit: bool) {
    // Publish the first request's intent before waking the domain thread.
    if requested.set(clean_exit).is_ok() {
        let _ = commands.send(Command::Shutdown);
    }
}

struct PendingView {
    receive: Receiver<Result<(Screen, InputModes, Option<GridSelection>)>>,
    sequence: u64,
    scroll_request_id: u64,
}

struct LaunchRetry {
    workspace_id: String,
    validated: bool,
    recovering: bool,
    fresh_codex: bool,
}

#[derive(Clone)]
struct EnvironmentChangeRequest {
    request_id: u64,
    session_id: String,
    kind: EnvironmentChangeKind,
    target: SessionEnvironmentTarget,
    candidate: Option<PathBuf>,
}

impl EnvironmentChangeRequest {
    fn from_command(command: &Command) -> Option<Self> {
        use EnvironmentChangeKind::{Handoff, Locate, Restore};
        let (request_id, session_id, kind, target, candidate) = match command {
            Command::RestoreSessionEnvironment {
                request_id,
                session_id,
            } => (
                request_id,
                session_id,
                Restore,
                SessionEnvironmentTarget::Worktree,
                None,
            ),
            Command::LocateSessionEnvironment {
                request_id,
                session_id,
                path,
            } => (
                request_id,
                session_id,
                Locate,
                SessionEnvironmentTarget::Worktree,
                Some(path.clone()),
            ),
            Command::HandoffSessionEnvironment {
                request_id,
                session_id,
                target,
            } => (request_id, session_id, Handoff, *target, None),
            _ => return None,
        };
        Some(Self {
            request_id: *request_id,
            session_id: session_id.clone(),
            kind,
            target,
            candidate,
        })
    }

    fn reply(&self, result: EnvironmentChangeResult) -> Event {
        let reply = SessionEnvironmentReply {
            request_id: self.request_id,
            session_id: self.session_id.clone(),
            result,
        };
        match self.kind {
            EnvironmentChangeKind::Restore => Event::RestoreSessionEnvironmentResult(reply),
            EnvironmentChangeKind::Locate => Event::LocateSessionEnvironmentResult(reply),
            EnvironmentChangeKind::Handoff => Event::HandoffSessionEnvironmentResult(reply),
        }
    }

    fn begin(
        &self,
        owner: &mut ShellOwner,
    ) -> Result<Result<EnvironmentChange, EnvironmentChangeResult>> {
        let storage = owner.environment_storage();
        if storage.is_readonly("main")?
            || storage.pragma_query_value(None, "query_only", |row| row.get::<_, bool>(0))?
        {
            return Ok(Err(EnvironmentChangeResult::rejected(STORAGE_READ_ONLY)));
        }
        let binding = session_environment::binding(storage, &self.session_id)?
            .context("session-unavailable")?;
        let owned = if self.target == SessionEnvironmentTarget::Worktree {
            session_environment::owned_worktrees(storage)?
        } else {
            Vec::new()
        };
        if self.kind == EnvironmentChangeKind::Handoff
            && binding.target == self.target
            && binding.state == "ready"
        {
            let cwd = if self.target == SessionEnvironmentTarget::Local {
                binding.local_path
            } else {
                owned
                    .iter()
                    .find(|item| item.session_id == self.session_id)
                    .context("no-owned-worktree")?
                    .path
                    .clone()
            };
            return Ok(Err(EnvironmentChangeResult::Ok {
                active_target: self.target,
                cwd,
            }));
        }
        let profile = owner
            .session(&self.session_id)?
            .context("session-unavailable")?
            .kind;
        let resume_binding = owner
            .environment_provider_binding(&self.session_id, &profile)?
            .filter(|binding| binding.resumable)
            .map(|binding| binding.id);
        let candidate = self
            .candidate
            .as_deref()
            .map(|path| path.to_str().context("invalid-path"))
            .transpose()?;
        let operation = session_environment::begin_transition(
            owner.environment_storage_mut(),
            &self.session_id,
            id(),
            self.kind,
            self.target,
            candidate.map(str::to_owned),
            now(),
        )?;
        let request = self.clone();
        let worker = thread::Builder::new()
            .name("session-environment".into())
            .spawn(move || {
                environment_runtime::check_change(
                    request.kind,
                    request.target,
                    &request.session_id,
                    request.candidate.as_deref(),
                    &binding,
                    &owned,
                )
            });
        match worker {
            Ok(worker) => Ok(Ok(EnvironmentChange {
                request: self.clone(),
                operation,
                profile,
                resume_binding,
                phase: EnvironmentPhase::Checking(EnvironmentCheck(Some(worker))),
            })),
            Err(error) => {
                session_environment::rollback_transition(
                    owner.environment_storage_mut(),
                    &operation,
                    false,
                    &error.to_string(),
                    now(),
                )?;
                Err(error.into())
            }
        }
    }
}

struct EnvironmentChange {
    request: EnvironmentChangeRequest,
    operation: Transition,
    profile: String,
    resume_binding: Option<String>,
    phase: EnvironmentPhase,
}

struct EnvironmentCheck(Option<thread::JoinHandle<Result<CheckedEnvironment>>>);

impl Drop for EnvironmentCheck {
    fn drop(&mut self) {
        // A Git write must finish before the host releases its SQLite lease.
        if let Some(worker) = self.0.take() {
            let _ = worker.join();
        }
    }
}

enum EnvironmentPhase {
    Checking(EnvironmentCheck),
    Stopping {
        path: String,
        stopped: Option<Receiver<Result<()>>>,
    },
    Launching {
        failure: Option<String>,
    },
    Confirming {
        failure: Option<String>,
        started_at: Instant,
    },
}

enum EnvironmentProgress {
    Pending,
    Stopping,
    Reply(EnvironmentChangeResult),
}

impl EnvironmentChange {
    fn advance(&mut self, owner: &mut ShellOwner, window_id: &str) -> Result<EnvironmentProgress> {
        let id = &self.operation.session_id;
        match &mut self.phase {
            EnvironmentPhase::Checking(worker) => {
                if !worker.0.as_ref().unwrap().is_finished() {
                    return Ok(EnvironmentProgress::Pending);
                }
                let checked = worker
                    .0
                    .take()
                    .unwrap()
                    .join()
                    .map_err(|_| anyhow!("environment check panicked"))??;
                let path = match checked {
                    CheckedEnvironment::Ready(path) => path,
                    CheckedEnvironment::OtherSession(session_id) => {
                        session_environment::rollback_transition(
                            owner.environment_storage_mut(),
                            &self.operation,
                            false,
                            "path-owned-by-another-session",
                            now(),
                        )?;
                        let scene_id = owner.notification_scene_id(window_id, &session_id)?;
                        return Ok(EnvironmentProgress::Reply(
                            EnvironmentChangeResult::SwitchSession {
                                session_id,
                                scene_id,
                            },
                        ));
                    }
                };
                if self.operation.kind != EnvironmentChangeKind::Handoff {
                    session_environment::mark_transition_external_ready(
                        owner.environment_storage(),
                        &self.operation,
                        now(),
                    )?;
                }
                let stopped = owner.stop_for_environment_transition(id)?;
                self.phase = EnvironmentPhase::Stopping { path, stopped };
                Ok(EnvironmentProgress::Stopping)
            }
            EnvironmentPhase::Stopping { path, stopped } => {
                if let Some(result) = stopped
                    && poll_promotion_result(result)?.is_none()
                {
                    return Ok(EnvironmentProgress::Pending);
                }
                session_environment::complete_transition(
                    owner.environment_storage_mut(),
                    &self.operation,
                    path,
                    now(),
                )?;
                self.phase = EnvironmentPhase::Launching { failure: None };
                Ok(EnvironmentProgress::Pending)
            }
            _ => Ok(EnvironmentProgress::Pending),
        }
    }

    fn rollback(
        &self,
        owner: &mut ShellOwner,
        committed: bool,
        error: &anyhow::Error,
    ) -> Result<String> {
        let reason = environment_runtime::change_failure(self.operation.kind, error);
        session_environment::rollback_transition(
            owner.environment_storage_mut(),
            &self.operation,
            committed,
            &reason,
            now(),
        )?;
        if committed
            && self.operation.previous_state == "ready"
            && let Some(binding_id) = &self.resume_binding
        {
            owner.reset_environment_resume(
                &self.operation.session_id,
                &self.profile,
                binding_id,
            )?;
        }
        Ok(reason)
    }

    fn launch_result(
        &self,
        owner: &ShellOwner,
        failure: Option<String>,
    ) -> Result<EnvironmentChangeResult> {
        if let Some(reason) = failure {
            return Ok(EnvironmentChangeResult::rejected(reason));
        }
        let session = owner
            .session(&self.operation.session_id)?
            .filter(|session| session.archived_at.is_none());
        Ok(match session {
            Some(session) => EnvironmentChangeResult::Ok {
                active_target: self.operation.target,
                cwd: session.cwd,
            },
            None => EnvironmentChangeResult::rejected("session-unavailable"),
        })
    }
}

enum RoleAssignmentsRequest {
    Get {
        request_id: u64,
        include_compaction: bool,
    },
    Set {
        request_id: u64,
        assignments: Value,
        web_search: Option<Value>,
        compaction: Option<Value>,
        include_compaction: bool,
    },
    ForLaunch(Sender<ClaudeRoleLaunchPreferences>),
    TimeZone {
        request_id: u64,
        setting: Option<Value>,
    },
}

enum ProviderSettingsRequest {
    Get(u64),
    Save(u64, Value),
    FetchModels(u64, Value),
    Delete(u64, String, String),
    SetActive(u64, String, String),
    ConfirmRisk(u64),
}

struct ProviderSettingsWorker {
    requests: Option<Sender<ProviderSettingsRequest>>,
    replies: Receiver<ProviderSettingsReply>,
    thread: Option<JoinHandle<()>>,
}

impl ProviderSettingsWorker {
    fn new(
        data_root: PathBuf,
        store: Arc<ProviderConfigStore>,
        subscriptions: Option<Arc<SubscriptionService>>,
    ) -> Result<Self> {
        let (requests, incoming) = mpsc::channel();
        let (outgoing, replies) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("matou-provider-settings".into())
            .spawn(move || {
                // The host's owner lease excludes another native runtime for this root.
                // Legacy or external processes writing the file outside that lease can race.
                let project_windows = |mut snapshot: Value| {
                    if let Ok(db) = startup::open_read_only(&data_root.join("matou.sqlite")) {
                        ProviderConfigStore::project_context_windows(&db, &mut snapshot);
                    }
                    Snapshot(snapshot)
                };
                use ProviderSettingsResult::{Accepted, Error, Models, Provider, Snapshot};
                for request in incoming {
                    let (request_id, result) = match request {
                        ProviderSettingsRequest::Get(id) => {
                            (id, store.snapshot().map(&project_windows))
                        }
                        ProviderSettingsRequest::Save(id, input)
                            if input.get("contextWindowMode").is_some() =>
                        {
                            let saved =
                                open_role_assignments_writer(&data_root).and_then(|mut db| {
                                    store.upsert_with_context_window(&mut db, &input, now())
                                });
                            (id, saved.map(Provider))
                        }
                        ProviderSettingsRequest::Save(id, input) => {
                            (id, store.upsert(&input).map(Provider))
                        }
                        ProviderSettingsRequest::FetchModels(id, input) => (
                            id,
                            Ok(Models(provider_model_discovery::fetch(&store, &input))),
                        ),
                        ProviderSettingsRequest::Delete(id, cli, provider_id) => {
                            let result = if let Some(service) = &subscriptions {
                                service.delete(&cli, &provider_id)
                            } else {
                                store.delete(&cli, &provider_id)
                            };
                            (id, result.map(&project_windows))
                        }
                        ProviderSettingsRequest::SetActive(id, cli, provider_id) => {
                            let result = match &subscriptions {
                                Some(service) if cli == "claude-code" => {
                                    service.activate(&provider_id)
                                }
                                None if cli == "claude-code" => {
                                    store.subscription(&provider_id).and_then(|row| {
                                        ensure!(
                                            row.is_none(),
                                            "Subscription launch service is unavailable"
                                        );
                                        store.set_active(&cli, &provider_id)
                                    })
                                }
                                _ => store.set_active(&cli, &provider_id),
                            };
                            (id, result.map(&project_windows))
                        }
                        ProviderSettingsRequest::ConfirmRisk(id) => (
                            id,
                            store.confirm_antigravity_risk().map(|()| Accepted(true)),
                        ),
                    };
                    let result = result.unwrap_or_else(|error| Error(error.to_string()));
                    if outgoing
                        .send(ProviderSettingsReply { request_id, result })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .context("start provider settings worker")?;
        Ok(Self {
            requests: Some(requests),
            replies,
            thread: Some(thread),
        })
    }

    fn send(&self, request: ProviderSettingsRequest) -> Result<()> {
        self.requests
            .as_ref()
            .context("provider settings worker stopped")?
            .send(request)
            .map_err(|_| anyhow!("provider settings worker stopped"))
    }

    fn shutdown(&mut self) -> Result<()> {
        // Close the queue and wait for accepted writes before releasing the owner lease.
        self.requests.take();
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| anyhow!("provider settings worker panicked"))?;
        }
        Ok(())
    }
}

impl Drop for ProviderSettingsWorker {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

struct PendingInput {
    workspace_id: String,
    session_id: String,
    bytes: Vec<u8>,
    defer_ordering: bool,
}

struct PendingShellSubmission {
    defer_ordering: bool,
    record_on_match: bool,
    decision: Receiver<Option<ShellSubmission>>,
    following: VecDeque<Command>,
}

const WORKSPACE_PATH_INVALID: &str = "工作区目录不可用，请先在本地恢复原路径，或移出该工作区";
const ACTIVE_PATH_INTERVAL: Duration = Duration::from_millis(400);
const WORKSPACE_POLL_INTERVAL: Duration = Duration::from_secs(30);
type WorkerOperation = (String, Receiver<Result<()>>, bool);

pub fn database_recovery_request_id() -> String {
    id()
}

fn recovery_result(
    send: &async_channel::Sender<Event>,
    result: DatabaseRecoveryResult,
) -> Result<()> {
    send.send_blocking(Event::DatabaseRecoveryResult(result))?;
    Ok(())
}

pub fn storage_command_rejected(command: Command, code: &str, message: &str) -> Event {
    if let Some(request) = EnvironmentChangeRequest::from_command(&command) {
        return request.reply(EnvironmentChangeResult::rejected(code));
    }
    if let Some((request_id, event)) = git_reply_event(&command) {
        return event(GitReply {
            request_id,
            result: Err(GitFailure {
                code: code.into(),
                message: message.into(),
            }),
            worktree_session_counts: BTreeMap::new(),
        });
    }
    if let Command::DatabaseRecovery { request_id, .. } = command {
        let mut result = DatabaseRecoveryResult::failed(request_id, code, message);
        if code == STORAGE_READ_ONLY {
            result.error.as_mut().unwrap().retryable = false;
        }
        return Event::DatabaseRecoveryResult(result);
    }
    if let Command::ReadOnlyQuery { request_id, .. } = command {
        return Event::ReadOnlyQueryResult {
            request_id,
            result: Err(message.into()),
        };
    }
    if let Command::History(request) = command {
        return Event::History(request.failed(message.into()));
    }
    let wiring_request = match &command {
        Command::GetClaudeAccounts { request_id }
        | Command::ClaudeAccountCommand { request_id, .. } => {
            Some(("claudeAccountsResult", *request_id))
        }
        Command::SetTaskStatus { request_id, .. } => Some(("taskStatusResult", *request_id)),
        Command::LoadDshSession { request_id, .. } => Some(("dshSessionLoadResult", *request_id)),
        Command::LoadAntigravitySession { request_id, .. } => {
            Some(("antigravitySessionLoadResult", *request_id))
        }
        Command::RecheckEgress { request_id } => Some(("egressRecheckResult", *request_id)),
        Command::SetEgressExemption { request_id, .. } => {
            Some(("egressExemptionResult", *request_id))
        }
        Command::SetEgressRuleMode { request_id, .. } => {
            Some(("egressRuleModeResult", *request_id))
        }
        Command::SetEgressProtectionEnabled { request_id, .. } => {
            Some(("egressProtectionEnabledResult", *request_id))
        }
        Command::AcceptEgressBaseline { request_id, .. } => {
            Some(("egressBaselineResult", *request_id))
        }
        Command::CheckForUpdate { request_id } => Some(("updateCheckResult", *request_id)),
        Command::DownloadUpdate { request_id } => Some(("updateDownloadResult", *request_id)),
        Command::InstallUpdate { request_id } => Some(("updateInstallResult", *request_id)),
        Command::RunRetentionCleanup { request_id } => {
            Some(("retentionCleanupResult", *request_id))
        }
        _ => None,
    };
    if let Some((kind, request_id)) = wiring_request {
        return Event::Graph(
            json!({"type": kind, "requestId": request_id, "code": code, "error": message}),
        );
    }
    let request_id = match &command {
        Command::GetClaudeModelCatalog { request_id }
        | Command::GetClaudeRoleAssignments { request_id }
        | Command::GetClaudeModelPreferences { request_id }
        | Command::GetTimeZoneSettings { request_id }
        | Command::SetTimeZoneSettings { request_id, .. }
        | Command::SetClaudeRoleAssignments { request_id, .. }
        | Command::SetClaudeModelPreferences { request_id, .. }
        | Command::GetProviderSettings { request_id }
        | Command::SaveProvider { request_id, .. }
        | Command::FetchProviderModels { request_id, .. }
        | Command::DeleteProvider { request_id, .. }
        | Command::SetActiveProvider { request_id, .. }
        | Command::ConfirmAntigravityRisk { request_id }
        | Command::CopySelection { request_id, .. }
        | Command::ScrollTo { request_id, .. } => Some(*request_id),
        Command::PutGeometry { sequence, .. } | Command::Applied { sequence, .. } => {
            Some(*sequence)
        }
        _ => None,
    };
    Event::StorageCommandRejected {
        request_id,
        code: code.into(),
        message: message.into(),
    }
}

type WritableDatabase = (
    Connection,
    matou_native_core::ownership::DatabaseOwner,
    PathBuf,
);

enum RecoveryOutcome {
    Opened(WritableDatabase),
    Exported(PathBuf),
}

fn await_database_recovery(
    mut recovery: RecoveryRequired,
    generation: &str,
    incoming: &Receiver<Command>,
    send: &async_channel::Sender<Event>,
) -> Result<Option<(WritableDatabase, String)>> {
    send.send_blocking(Event::DatabaseRecoveryRequired(
        DatabaseRecoveryDetails::from(&recovery),
    ))?;
    thread::scope(|scope| {
        type Attempt = (
            RecoveryActions,
            RecoveryRequired,
            std::result::Result<RecoveryOutcome, RecoveryActionError>,
        );
        let mut active: Option<(String, thread::ScopedJoinHandle<'_, Attempt>)> = None;
        let mut actions = Some(RecoveryActions::default());
        let mut closing = false;
        loop {
            if active
                .as_ref()
                .is_some_and(|(_, worker)| worker.is_finished())
                || closing
            {
                let Some((request_id, worker)) = active.take() else {
                    return Ok(None);
                };
                let (state, updated_recovery, result) = worker
                    .join()
                    .map_err(|_| anyhow!("database recovery worker panicked"))?;
                if closing {
                    return Ok(None);
                }
                actions = Some(state);
                recovery = updated_recovery;
                match result {
                    Ok(RecoveryOutcome::Opened(database)) => {
                        return Ok(Some((database, request_id)));
                    }
                    Ok(RecoveryOutcome::Exported(path)) => {
                        recovery_result(
                            send,
                            DatabaseRecoveryResult {
                                request_id,
                                exported_path: Some(path),
                                error: None,
                            },
                        )?;
                    }
                    Err(error) => {
                        send.send_blocking(Event::DatabaseRecoveryRequired(
                            DatabaseRecoveryDetails::from(&recovery),
                        ))?;
                        recovery_result(
                            send,
                            DatabaseRecoveryResult::failed(
                                request_id,
                                error.stage,
                                format!("{:#}", error.cause),
                            ),
                        )?;
                    }
                }
            }
            let command = if active.is_some() {
                incoming.recv_timeout(Duration::from_millis(10))
            } else {
                incoming
                    .recv()
                    .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
            };
            match command {
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) | Ok(Command::Shutdown) => {
                    closing = true;
                }
                Ok(command) if !StoragePhase::Recovery.accepts(&command) => {
                    send.send_blocking(storage_command_rejected(
                        command,
                        STORAGE_READ_ONLY,
                        STORAGE_RECOVERY_WAITING,
                    ))?;
                }
                Ok(Command::DatabaseRecovery { request_id, action }) => {
                    let reject = if active.is_some() {
                        Some(("RECOVERY_BUSY", DATABASE_RECOVERY_BUSY))
                    } else if action
                        .expected_recovery_id()
                        .is_some_and(|id| id != recovery.recovery_id)
                    {
                        Some(("RECOVERY_ID_CHANGED", RECOVERY_ID_CHANGED))
                    } else {
                        None
                    };
                    if let Some((code, message)) = reject {
                        recovery_result(
                            send,
                            DatabaseRecoveryResult::failed(request_id, code, message),
                        )?;
                        continue;
                    }
                    let Ok(archive_id) = Uuid::parse_str(&request_id) else {
                        recovery_result(
                            send,
                            DatabaseRecoveryResult::failed(
                                request_id,
                                "RECOVERY_REQUEST_ID_INVALID",
                                "无效的数据库恢复请求编号",
                            ),
                        )?;
                        continue;
                    };
                    let mut state = actions.take().expect("idle recovery state");
                    let mut snapshot = recovery.clone();
                    let worker = thread::Builder::new()
                        .name("matou-database-recovery".into())
                        .spawn_scoped(scope, move || {
                            let opening = match &action {
                                DatabaseRecoveryAction::RetryOpen { .. } => OpeningAction::Retry,
                                DatabaseRecoveryAction::RestoreBackup { backup_id, .. } => {
                                    OpeningAction::Restore(backup_id)
                                }
                                DatabaseRecoveryAction::StartEmptyDatabase { .. } => {
                                    OpeningAction::StartEmpty
                                }
                                DatabaseRecoveryAction::ExportRecoveryBundle => {
                                    let result = state.retry_pending_cleanup().and_then(|()| {
                                        crate::readonly_export::export_pending(&snapshot, now)
                                            .map(RecoveryOutcome::Exported)
                                            .map_err(|cause| RecoveryActionError {
                                                stage: "DATABASE_EXPORT_FAILED",
                                                cause,
                                            })
                                    });
                                    return (state, snapshot, result);
                                }
                            };
                            let result = state.open(
                                &snapshot,
                                generation,
                                archive_id,
                                action.expected_recovery_id().expect("opening action"),
                                opening,
                            );
                            if let Err(error) = &result
                                && !matches!(action, DatabaseRecoveryAction::RetryOpen { .. })
                                && !matches!(error.stage, "marker" | "fence" | "cleanup" | "lease")
                            {
                                refresh_recovery_backups(&mut snapshot);
                            }
                            (state, snapshot, result.map(RecoveryOutcome::Opened))
                        })
                        .context("start database recovery worker")?;
                    active = Some((request_id, worker));
                }
                Ok(_) => unreachable!("only recovery commands passed the storage gate"),
            }
        }
    })
}

fn refresh_recovery_backups(recovery: &mut RecoveryRequired) {
    let root = recovery
        .durable_database_path
        .parent()
        .expect("validated recovery data root");
    match BackupService::new(root).and_then(|backups| backups.list_valid()) {
        Ok(mut backups) => {
            backups.truncate(DEFAULT_MAX_BACKUPS);
            recovery.backups = backups;
            recovery.backup_list_error = None;
        }
        Err(error) => {
            recovery.backup_list_error = Some(RecoveryError {
                code: "BACKUP_LIST_FAILED".into(),
                message: format!("{error:#}"),
                retryable: true,
            });
        }
    }
}

fn run(
    options: LaunchOptions,
    incoming: Receiver<Command>,
    send: &async_channel::Sender<Event>,
    shutdown_requested: &OnceLock<bool>,
    fixtures: OfflineFixtures,
    live_services: bool,
) -> Result<()> {
    let mut wiring = RuntimeWiring::new(fixtures, &options.shell_home, live_services)?;
    let generation = id();
    let bootstrap = match startup::open(&options.data_root, &generation) {
        Ok(value) => value,
        Err(error) => {
            let kind = if error
                .downcast_ref::<matou_native_core::storage::MigrationChecksumMismatch>()
                .is_some()
            {
                Some(StartupBlockKind::MigrationChecksum)
            } else if error.downcast_ref::<std::io::Error>().is_some_and(|e| {
                matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound
                        | std::io::ErrorKind::PermissionDenied
                        | std::io::ErrorKind::NotADirectory
                        | std::io::ErrorKind::AlreadyExists
                )
            }) {
                Some(StartupBlockKind::Configuration)
            } else {
                None
            };
            if let Some(kind) = kind {
                let _ = send.send_blocking(Event::StartupBlocked(StartupBlock {
                    kind,
                    message: format!("{error:#}"),
                }));
            }
            return Err(error);
        }
    };
    let (database, mut recovery_request) = match bootstrap {
        BootstrapResult::Writable {
            connection,
            ownership,
            data_root,
        } => ((connection, ownership, data_root), None),
        BootstrapResult::ReadOnly {
            connection,
            data_root,
            reason,
        } => return run_read_only(connection, data_root, reason, incoming, send, &mut wiring),
        BootstrapResult::RecoveryRequired(recovery) => {
            let Some((database, request_id)) =
                await_database_recovery(recovery, &generation, &incoming, send)?
            else {
                return Ok(());
            };
            (database, Some(request_id))
        }
    };
    let result = run_writable(
        options,
        incoming,
        send,
        shutdown_requested,
        database,
        &mut recovery_request,
        &generation,
        &mut wiring,
        live_services,
    );
    if let (Some(request_id), Err(error)) = (recovery_request, &result) {
        recovery_result(
            send,
            DatabaseRecoveryResult::failed(
                request_id,
                "runtime-initialization",
                format!("{error:#}"),
            ),
        )?;
    }
    result
}

fn run_read_only(
    connection: Connection,
    data_root: PathBuf,
    reason: ReadOnlyReason,
    incoming: Receiver<Command>,
    send: &async_channel::Sender<Event>,
    wiring: &mut RuntimeWiring,
) -> Result<()> {
    let history = HistoryWorker::new(data_root.clone())?;
    let (workspace, projection) = match ReadOnlyWorkspace::new(&connection) {
        Ok((workspace, active)) => {
            let projection = ReadOnlyProjection {
                workspaces: workspace.workspaces.clone(),
                tasks: workspace.tasks.clone(),
                active,
            };
            (Ok(workspace), Ok(projection))
        }
        Err(error) => {
            let message = format!("{error:#}");
            (Err(message.clone()), Err(message))
        }
    };
    let mut current_graph = projection
        .as_ref()
        .map(|projection| projection.active.graph.clone())
        .unwrap_or(Value::Null);
    send.send_blocking(Event::ReadOnlyReady { reason, projection })?;
    thread::scope(|scope| -> Result<()> {
        let mut export: Option<(String, thread::ScopedJoinHandle<'_, Result<PathBuf>>)> = None;
        let mut pending_history = 0;
        loop {
            while let Some(reply) = history.try_recv()? {
                pending_history -= 1;
                send.send_blocking(Event::History(reply))?;
            }
            if export.as_ref().is_some_and(|(_, job)| job.is_finished()) {
                let (request_id, job) = export.take().unwrap();
                let result = job
                    .join()
                    .map_err(|_| anyhow!("database export worker panicked"))?;
                recovery_result(
                    send,
                    match result {
                        Ok(path) => DatabaseRecoveryResult {
                            request_id,
                            exported_path: Some(path),
                            error: None,
                        },
                        Err(error) => DatabaseRecoveryResult::failed(
                            request_id,
                            "DATABASE_EXPORT_FAILED",
                            format!("{error:#}"),
                        ),
                    },
                )?;
            }
            let received = if export.is_some() || pending_history != 0 {
                incoming.recv_timeout(Duration::from_millis(10))
            } else {
                incoming
                    .recv()
                    .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
            };
            let command = match received {
                Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Ok(command) => command,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
            };
            if !StoragePhase::ReadOnly.accepts(&command) {
                let event =
                    storage_command_rejected(command, STORAGE_READ_ONLY, STORAGE_READ_ONLY_MESSAGE);
                send.send_blocking(with_graph_projection(event, current_graph.clone()))?;
                continue;
            }
            if let Some(reply) = wiring.handle(&command, &data_root, None) {
                send.send_blocking(with_graph_projection(reply, current_graph.clone()))?;
                continue;
            }
            match command {
                Command::GetTimeZoneSettings { request_id } => {
                    let result = read_claude_time_zone(&connection)
                        .map(time_zone_snapshot)
                        .map_err(|_| TimeZoneSettingsError::Storage);
                    send.send_blocking(Event::TimeZoneSettings(TimeZoneSettingsReply {
                        request_id,
                        result,
                    }))?;
                }
                Command::ReadOnlyQuery { request_id, query } => {
                    let result = match &workspace {
                        Ok(workspace) => workspace
                            .navigate(query)
                            .map_err(|error| format!("{error:#}")),
                        Err(error) => Err(error.clone()),
                    };
                    if let Ok(navigation) = &result {
                        current_graph = navigation.graph.clone();
                    }
                    send.send_blocking(Event::ReadOnlyQueryResult { request_id, result })?;
                }
                Command::History(request) => {
                    let id = request.session_id();
                    match matou_native_core::sessions::get_session(&connection, id)
                        .and_then(|session| session.with_context(|| format!("会话不存在：{id}")))
                    {
                        Ok(_) => {
                            history.send(request)?;
                            pending_history += 1;
                        }
                        Err(error) => send
                            .send_blocking(Event::History(request.failed(format!("{error:#}"))))?,
                    }
                }
                Command::DatabaseRecovery { request_id, .. } => {
                    if export.is_some() {
                        recovery_result(
                            send,
                            DatabaseRecoveryResult::failed(
                                request_id,
                                "RECOVERY_BUSY",
                                DATABASE_RECOVERY_BUSY,
                            ),
                        )?;
                    } else if Uuid::parse_str(&request_id).is_err() {
                        recovery_result(
                            send,
                            DatabaseRecoveryResult::failed(
                                request_id,
                                "RECOVERY_REQUEST_ID_INVALID",
                                "无效的数据库恢复请求编号",
                            ),
                        )?;
                    } else {
                        let root = &data_root;
                        let job = thread::Builder::new()
                            .name("matou-read-only-export".into())
                            .spawn_scoped(scope, move || {
                                crate::readonly_export::export(root, now())
                            });
                        match job {
                            Ok(job) => export = Some((request_id, job)),
                            Err(error) => recovery_result(
                                send,
                                DatabaseRecoveryResult::failed(
                                    request_id,
                                    "DATABASE_EXPORT_FAILED",
                                    error.to_string(),
                                ),
                            )?,
                        }
                    }
                }
                _ => unreachable!("only read-only commands passed the storage gate"),
            }
        }
        Ok(())
    })?;
    drop(workspace);
    connection.close().map_err(|(_, error)| error)?;
    Ok(())
}

struct ReadOnlyWorkspace<'a> {
    connection: &'a Connection,
    navigation: WindowNavigation,
    workspaces: Vec<Workspace>,
    tasks: Vec<Task>,
}

impl<'a> ReadOnlyWorkspace<'a> {
    fn new(connection: &'a Connection) -> Result<(Self, ReadOnlyNavigation)> {
        let hierarchy = hierarchy::read_hierarchy(connection, MAIN_WINDOW_ID)?;
        let mut workspaces = workspaces::list_workspaces(connection)?;
        sort_workspaces(&mut workspaces);
        let mut tasks = Vec::new();
        for workspace in &workspaces {
            tasks.extend(tasks::list_tasks(
                connection,
                MAIN_WINDOW_ID,
                &workspace.id,
            )?);
        }
        let mut workspace = Self {
            connection,
            navigation: hierarchy.navigation,
            workspaces,
            tasks,
        };
        let initial = workspace.project(workspace.navigation.clone(), None)?;
        workspace.navigation = initial.hierarchy.navigation.clone();
        Ok((workspace, initial))
    }

    fn navigate(&self, query: ReadOnlyQuery) -> Result<ReadOnlyNavigation> {
        let mut navigation = self.navigation.clone();
        let mut required_session = None;
        let selected_scene = match query {
            ReadOnlyQuery::Workspace { workspace_id } => {
                ensure!(
                    self.workspaces
                        .iter()
                        .any(|workspace| workspace.id == workspace_id),
                    "工作空间不存在：{workspace_id}"
                );
                navigation.active_workspace_id = Some(workspace_id);
                None
            }
            ReadOnlyQuery::Task { task_id } => {
                self.select_task(&mut navigation, &task_id)?;
                None
            }
            ReadOnlyQuery::Scene { scene_id } => Some(scene_id),
            ReadOnlyQuery::Session {
                scene_id,
                session_id,
            } => {
                navigation
                    .session_by_scene
                    .insert(scene_id.clone(), session_id.clone());
                required_session = Some(session_id);
                Some(scene_id)
            }
        };
        if let Some(scene_id) = selected_scene {
            let task_id: String = self
                .connection
                .query_row(
                    "SELECT task_id FROM scenes WHERE id = ? AND archived_at IS NULL",
                    [&scene_id],
                    |row| row.get(0),
                )
                .with_context(|| format!("页签不存在或无法读取：{scene_id}"))?;
            self.select_task(&mut navigation, &task_id)?;
            navigation.scene_by_task.insert(task_id, scene_id);
        }
        self.project(navigation, required_session.as_deref())
    }

    fn select_task(&self, navigation: &mut WindowNavigation, task_id: &str) -> Result<()> {
        let task = self
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .with_context(|| format!("事项不存在：{task_id}"))?;
        navigation.active_workspace_id = Some(task.workspace_id.clone());
        navigation
            .task_by_workspace
            .insert(task.workspace_id.clone(), task_id.into());
        Ok(())
    }

    fn project(
        &self,
        mut navigation: WindowNavigation,
        required_session: Option<&str>,
    ) -> Result<ReadOnlyNavigation> {
        let workspace = choose_read_only(
            &self.workspaces,
            navigation.active_workspace_id.as_ref(),
            |w| &w.id,
        );
        navigation.active_workspace_id = workspace.map(|workspace| workspace.id.clone());
        let mut scenes = Vec::new();
        let mut graph = json!({"nodes": []});
        let mut geometry = Vec::new();
        if let Some(workspace) = workspace {
            let tasks = self
                .tasks
                .iter()
                .filter(|task| task.workspace_id == workspace.id);
            let task = navigation
                .task_by_workspace
                .get(&workspace.id)
                .and_then(|id| tasks.clone().find(|task| &task.id == id))
                .or_else(|| tasks.clone().next());
            if let Some(task) = task {
                navigation
                    .task_by_workspace
                    .insert(workspace.id.clone(), task.id.clone());
                scenes = hierarchy::list_scenes(self.connection, &task.id)?;
                let scene =
                    choose_read_only(&scenes, navigation.scene_by_task.get(&task.id), |s| &s.id);
                if let Some(scene) = scene {
                    navigation
                        .scene_by_task
                        .insert(task.id.clone(), scene.id.clone());
                    graph = matou_native_core::session_graph::project_scene_graph(
                        self.connection,
                        &scene.id,
                        None,
                    )?;
                    let counts = hierarchy::cards::scene_workspace_session_counts(
                        self.connection,
                        &scene.id,
                    )?;
                    graph["workspaceSessionCount"] = json!(counts.values().sum::<i64>());
                    graph["workspaceSessionCountByTask"] = json!(counts);
                    let ids = graph["nodes"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|node| node["archivedAt"].is_null())
                        .filter_map(|node| node["sessionId"].as_str());
                    let session = navigation
                        .session_by_scene
                        .get(&scene.id)
                        .and_then(|id| ids.clone().find(|session_id| *session_id == id))
                        .or_else(|| ids.clone().next());
                    if let Some(id) = required_session {
                        ensure!(session == Some(id), "页签中没有该会话：{id}");
                    }
                    if let Some(id) = session {
                        navigation
                            .session_by_scene
                            .insert(scene.id.clone(), id.into());
                        graph["focusedSessionId"] = json!(id);
                    } else {
                        navigation.session_by_scene.remove(&scene.id);
                    }
                    geometry = matou_native_core::geometry::list(self.connection, &scene.id)?;
                } else {
                    navigation.scene_by_task.remove(&task.id);
                }
            } else {
                navigation.task_by_workspace.remove(&workspace.id);
            }
        }
        let hierarchy = hierarchy::read_hierarchy_at(self.connection, navigation)?;
        Ok(ReadOnlyNavigation {
            hierarchy,
            scenes,
            graph,
            geometry,
        })
    }
}

fn choose_read_only<'a, T>(
    items: &'a [T],
    preferred: Option<&String>,
    id: impl Fn(&T) -> &String,
) -> Option<&'a T> {
    preferred
        .and_then(|preferred| items.iter().find(|item| id(item) == preferred))
        .or_else(|| items.first())
}

fn run_writable(
    options: LaunchOptions,
    incoming: Receiver<Command>,
    send: &async_channel::Sender<Event>,
    shutdown_requested: &OnceLock<bool>,
    (mut connection, ownership, data_root): WritableDatabase,
    recovery_request: &mut Option<String>,
    generation: &str,
    wiring: &mut RuntimeWiring,
    live_services: bool,
) -> Result<()> {
    for path in [&options.workspace, &options.shell_home] {
        if let Err(error) = std::fs::create_dir_all(path) {
            send.send_blocking(Event::StartupBlocked(StartupBlock {
                kind: StartupBlockKind::Configuration,
                message: error.to_string(),
            }))?;
            return Err(error.into());
        }
    }
    wiring.claude_accounts = crate::claude_accounts::AccountWorker::start(
        &data_root,
        live_services,
        wiring.fixtures.claude_accounts_engine.take(),
    )
    .ok();
    let history = HistoryWorker::new(data_root.clone())?;
    let window_id = MAIN_WINDOW_ID;
    environment_runtime::reconcile_transitions(&mut connection, now())?;
    let input = BootstrapWindowInput {
        window_id: window_id.into(),
        default_root_directory: options.workspace.to_string_lossy().into_owned(),
        default_name: options
            .workspace
            .file_name()
            .context("workspace name")?
            .to_string_lossy()
            .into_owned(),
        now: now(),
    };
    let hierarchy = hierarchy::bootstrap_window(
        &mut connection,
        &CommandMetadata {
            command_id: id(),
            command_type: "window.bootstrap".into(),
            request_hash: serde_json::to_string(&input)?,
            causation_id: None,
            correlation_id: None,
        },
        &input,
        &HierarchyIds {
            workspace_id: id(),
            execution_context_id: id(),
            task_id: id(),
            scene_id: id(),
            root_node_id: id(),
            session_id: id(),
            mount_id: id(),
        },
        "默认",
        now(),
    )?;
    let mut scene_id = hierarchy.scene.as_ref().map(|scene| scene.id.clone());
    let mut active_task = hierarchy.task.as_ref().map(|task| task.id.clone());
    let mut owner = ShellOwner::new(connection, ownership, id)?;
    owner.provider_environment = wiring.fixtures.provider_environment.take();
    owner.dsh_session_root = wiring
        .fixtures
        .dsh_session_root
        .clone()
        .or_else(dsh_catalog::default_session_root);
    owner.fork_worktree_root = wiring.fixtures.fork_worktree_root.take();
    owner.fork_setup = wiring.fixtures.fork_setup.take();
    // Declared after the lease-owning host state so early returns also join the worker first.
    let locale = match current_interface_locale(read_locale_preference(&data_root)) {
        InterfaceLocale::English => ProviderConfigLocale::En,
        InterfaceLocale::Chinese => ProviderConfigLocale::ZhCn,
    };
    let provider_store = Arc::new(ProviderConfigStore::new(&data_root, locale));
    let subscriptions = live_services
        .then(|| {
            SubscriptionService::new(
                data_root.clone(),
                Arc::clone(&provider_store),
                owner.launch_environment(),
            )
            .map(Arc::new)
        })
        .transpose()?;
    wiring.subscriptions = subscriptions.clone();
    owner.subscriptions = subscriptions.clone();
    let mut provider_settings =
        ProviderSettingsWorker::new(data_root.clone(), provider_store, subscriptions.clone())?;
    let mut sessions = HashSet::new();
    let mut active_workspace = hierarchy
        .workspace
        .as_ref()
        .map(|workspace| workspace.id.clone());
    let mut checks = WorkspacePathChecks::default();
    let mut startup_order = HashMap::new();
    let mut startup_restores = Vec::new();
    // Startup parking is independent from the existing path-repair rebuild queue.
    let mut startup_deferred = HashSet::new();
    for (session_id, workspace_id, automatic) in
        owner.startup_recovery_candidates(scene_id.as_deref())?
    {
        if automatic && startup_order.len() < 16 {
            startup_order.insert(session_id.clone(), startup_order.len());
            startup_restores.push((session_id, workspace_id));
        } else {
            startup_deferred.insert(session_id);
        }
    }
    let graph = active_graph(&owner, scene_id.as_deref(), window_id, &startup_deferred)?;
    let mut focused_session = graph["focusedSessionId"].as_str().map(str::to_owned);
    let mut pending_launch = unstarted_sessions(&owner, &graph, &sessions)?;
    pending_launch.retain(|id| !startup_deferred.contains(id));
    let mut foreground_sessions: HashSet<_> = launchable_ids(&graph).map(str::to_owned).collect();
    let mut deferred_recovery = HashSet::new();
    let mut environment_changes = HashMap::<String, EnvironmentChange>::new();
    let mut launch_preparations = HashMap::<String, Receiver<Result<PreparedLaunch>>>::new();
    let mut recovering = HashMap::<String, String>::new();
    // A failed launch stays blocked until an explicit retry starts a worker.
    let mut failed_launches = HashMap::new();
    let mut launch_retries = HashMap::<String, LaunchRetry>::new();
    let mut launch_validated = false;
    let mut pending_input = VecDeque::<PendingInput>::new();
    let mut shell_promotions = HashMap::<String, ShellPromotion>::new();
    let mut retry_pending_input = false;
    let mut terminal_layouts = HashMap::new();
    let mut next_active_check = Instant::now() + ACTIVE_PATH_INTERVAL;
    let mut next_workspace_poll = Instant::now() + WORKSPACE_POLL_INTERVAL;
    if let Some(workspace_id) = &active_workspace {
        checks.request(&owner, workspace_id)?;
    }
    for (session_id, workspace_id) in startup_restores {
        if !pending_launch.contains(&session_id) {
            let fresh_codex = owner.pending_codex_fresh(&session_id)?;
            checks.request(&owner, &workspace_id)?;
            launch_retries.insert(
                session_id,
                LaunchRetry {
                    workspace_id,
                    recovering: true,
                    validated: false,
                    fresh_codex,
                },
            );
        }
    }
    let mut projected_paths = owner
        .workspace_path_states()?
        .into_iter()
        .map(|state| (state.workspace_id.clone(), state))
        .collect::<BTreeMap<_, _>>();
    let mut active_path_invalid =
        workspace_is_invalid(&projected_paths, active_workspace.as_deref());
    send.send_blocking(Event::WorkspacePaths(
        projected_paths.values().cloned().collect(),
    ))?;
    let scenes = hierarchy
        .task
        .as_ref()
        .map(|task| owner.scenes(&task.id))
        .transpose()?
        .unwrap_or_default();
    send_sidebar(&owner, window_id, send)?;
    let ready_event = Event::Ready {
        hierarchy,
        scenes,
        graph,
        geometry: scene_id
            .as_deref()
            .map(|id| owner.geometry(id))
            .transpose()?
            .unwrap_or_default(),
    };
    let mut dirty = HashSet::<String>::new();
    let mut sequence = HashMap::<String, u64>::new();
    let mut views = HashMap::<String, PendingView>::new();
    let mut scroll_requests = HashMap::<String, u64>::new();
    let mut awaiting_ui = HashMap::<String, u64>::new();
    let mut operations = Vec::<WorkerOperation>::new();
    let mut shell_submissions = HashMap::<String, PendingShellSubmission>::new();
    let mut replay_shell_inputs = VecDeque::<Command>::new();
    let mut searches = Vec::<(String, u64, Receiver<Result<SearchResult>>)>::new();
    let mut copies = Vec::<(u64, String, bool, Receiver<Result<SelectionCopy>>)>::new();
    let mut navigation_publications = VecDeque::new();
    let mut graph_dirty = false;
    let mut provider_identity_events = Vec::new();
    let mut agent_notifications = Vec::new();
    let mut provider_starting = HashMap::new();
    let mut claude_launch_targets = HashMap::new();
    let mut permission_changes = HashMap::<String, PermissionChange>::new();
    let mut permission_results = Vec::new();
    let (catalog_requests, catalog_replies) = start_provider_catalog(
        owner.dsh_session_root.clone(),
        owner.codex_session_root(),
        antigravity_catalog::catalog_root(&options.shell_home, &owner.launch_environment()),
    )?;
    let (model_catalog_requests, model_catalog_commands) = mpsc::channel();
    let (model_catalog_results, model_catalog_replies) = mpsc::channel();
    let worker_results = model_catalog_results.clone();
    let model_catalog_worker = thread::Builder::new()
        .name("claude-model-catalog".into())
        .spawn(move || {
            for request_id in model_catalog_commands {
                let result = ClaudeModelCatalogResult::Models(claude_model_catalog::models());
                if worker_results
                    .send(ClaudeModelCatalogReply { request_id, result })
                    .is_err()
                {
                    break;
                }
            }
        });
    let (role_requests, role_commands) = mpsc::channel();
    let (role_results, role_replies) = mpsc::channel();
    let (time_zone_changed, time_zone_changes) = mpsc::channel();
    let role_data_root = data_root.clone();
    let time_zone_events = send.clone();
    let role_worker = thread::Builder::new()
        .name("claude-role-assignments".into())
        .spawn(move || {
            for request in role_commands {
                let reply = match request {
                    RoleAssignmentsRequest::Get {
                        request_id,
                        include_compaction,
                    } => {
                        let preferences = load_role_preferences(&role_data_root);
                        ClaudeModelPreferencesReply {
                            roles: ClaudeRoleAssignmentsReply {
                                request_id,
                                assignments: Some(preferences.assignments),
                                web_search: preferences.web_search.map(Some),
                                error: None,
                            },
                            compaction: include_compaction.then_some(preferences.compaction),
                        }
                    }
                    RoleAssignmentsRequest::Set {
                        request_id,
                        assignments,
                        web_search,
                        compaction,
                        include_compaction,
                    } => save_role_assignments(
                        &role_data_root,
                        request_id,
                        &assignments,
                        web_search.as_ref(),
                        compaction.as_ref(),
                    )
                    .map(|mut reply| {
                        if !include_compaction {
                            reply.compaction = None;
                        }
                        reply
                    })
                    .unwrap_or_else(|error| ClaudeModelPreferencesReply {
                        roles: ClaudeRoleAssignmentsReply {
                            request_id,
                            assignments: None,
                            web_search: None,
                            error: Some(format!("{error:#}")),
                        },
                        compaction: None,
                    }),
                    RoleAssignmentsRequest::ForLaunch(reply) => {
                        let _ = reply.send(load_role_preferences(&role_data_root));
                        continue;
                    }
                    RoleAssignmentsRequest::TimeZone {
                        request_id,
                        setting,
                    } => {
                        let result = time_zone_settings(&role_data_root, setting.as_ref());
                        if setting.is_some() && result.is_ok() {
                            let _ = time_zone_changed.send(());
                        }
                        if time_zone_events
                            .send_blocking(Event::TimeZoneSettings(TimeZoneSettingsReply {
                                request_id,
                                result,
                            }))
                            .is_err()
                        {
                            break;
                        }
                        continue;
                    }
                };
                if role_results.send(reply).is_err() {
                    break;
                }
            }
        })?;
    let mut catalog_loads = HashMap::<String, CatalogLoad>::new();
    let mut catalog_lists = Vec::new();
    let mut catalog_details = Vec::new();
    let mut catalog_load_errors = Vec::new();
    let mut tasks_dirty = false;
    if recovery_request.is_some() {
        // Reject commands already on the wire during the recovered bootstrap.
        // Neither a second action nor ordinary writes join the new workspace.
        for command in incoming.try_iter() {
            match command {
                Command::Shutdown => return Ok(()),
                Command::DatabaseRecovery { request_id, .. } => recovery_result(
                    send,
                    DatabaseRecoveryResult::failed(
                        request_id,
                        "RECOVERY_BUSY",
                        DATABASE_RECOVERY_BUSY,
                    ),
                )?,
                command => send.send_blocking(storage_command_rejected(
                    command,
                    STORAGE_READ_ONLY,
                    STORAGE_RECOVERY_WAITING,
                ))?,
            }
        }
    }
    // A recovery startup keeps its existing isolation even after it opens a
    // replacement database. A subsequent ordinary startup may serve control.
    let mut control = if recovery_request.is_none() {
        match ControlServer::start(&data_root, generation) {
            Ok(control) => {
                let bin = match crate::mt_cli::install_shim(&data_root) {
                    Ok(bin) => Some(bin),
                    Err(error) => {
                        eprintln!("[host-control] could not install mt: {error:#}");
                        None
                    }
                };
                owner.set_run_control(control.runs(), bin);
                Some(control)
            }
            Err(error) => {
                eprintln!("[host-control] {error:#}");
                None
            }
        }
    } else {
        None
    };
    if live_services {
        match LiveEgress::start(data_root.clone(), subscriptions.clone()) {
            Ok(live) => wiring.live_egress = live,
            Err(error) => send.send_blocking(Event::Diagnostic(format!(
                "Start egress sampler: {error:#}"
            )))?,
        }
    }
    send.send_blocking(ready_event)?;
    if let Some(request_id) = recovery_request.take() {
        recovery_result(
            send,
            DatabaseRecoveryResult {
                request_id,
                exported_path: None,
                error: None,
            },
        )?;
    }
    for session_id in &startup_deferred {
        send.send_blocking(Event::Recovery {
            session_id: session_id.clone(),
            state: RecoveryState::Deferred,
        })?;
    }
    let clean_exit = 'running: loop {
        let now = Instant::now();
        if now >= next_active_check {
            if let Some(workspace_id) = &active_workspace {
                checks.request(&owner, workspace_id)?;
            }
            next_active_check = now + ACTIVE_PATH_INTERVAL;
        }
        if now >= next_workspace_poll {
            for workspace_id in owner.workspace_path_poll_targets()? {
                if let Err(error) = checks.request(&owner, &workspace_id) {
                    eprintln!("[workspace-path:{workspace_id}] {error:#}");
                }
            }
            next_workspace_poll = now + WORKSPACE_POLL_INTERVAL;
        }
        let completed_checks = checks.poll(&mut owner);
        let path_checked = !completed_checks.is_empty();
        for completion in completed_checks {
            let state = match completion {
                Ok(state) => state,
                Err(error) => {
                    send.send_blocking(Event::Diagnostic(format!("{error:#}")))?;
                    continue;
                }
            };
            projected_paths.insert(state.workspace_id.clone(), state.clone());
            let invalid = state.status == "invalid";
            if invalid {
                let before = pending_input.len();
                pending_input.retain(|input| input.workspace_id != state.workspace_id);
                if pending_input.len() != before {
                    send.send_blocking(Event::Diagnostic(WORKSPACE_PATH_INVALID.into()))?;
                }
            }
            let retries: Vec<_> = launch_retries
                .iter_mut()
                .filter(|(_, retry)| retry.workspace_id == state.workspace_id)
                .filter_map(|(session_id, retry)| {
                    retry.validated = !invalid;
                    invalid.then(|| session_id.clone())
                })
                .collect();
            for session_id in retries {
                let retry = launch_retries.remove(&session_id).expect("checked retry");
                claude_launch_targets.remove(&session_id);
                failed_launches.insert(
                    session_id.clone(),
                    report_session_launch_failure(
                        &mut owner,
                        send,
                        &session_id,
                        retry.recovering,
                        retry.fresh_codex,
                        workspace_path_error(&state),
                    )?,
                );
            }
            if active_workspace.as_ref() != Some(&state.workspace_id) {
                continue;
            }
            if invalid {
                let failed: Vec<_> = pending_launch
                    .iter()
                    .filter(|id| !recovering.contains_key(*id))
                    .cloned()
                    .collect();
                for id in failed {
                    pending_launch.remove(&id);
                    claude_launch_targets.remove(&id);
                    let rebuilding = owner.has_shell_run(&id)?;
                    let message = match owner.session(&id)? {
                        Some(session) if session.kind == "claude-code" => {
                            validate_cwd(Path::new(&session.cwd)).err().map_or_else(
                                || workspace_path_error(&state),
                                |error| format!("{error:#}"),
                            )
                        }
                        _ => workspace_path_error(&state),
                    };
                    let failure = report_session_launch_failure(
                        &mut owner, send, &id, rebuilding, false, message,
                    )?;
                    failed_launches.insert(id, failure);
                }
            } else {
                if active_path_invalid && let Some(task_id) = &active_task {
                    for scene in owner.scenes(task_id)? {
                        let graph = owner.graph(&scene.id, None)?;
                        deferred_recovery.extend(
                            shell_ids(&graph)
                                .filter(|id| {
                                    sessions.contains(*id) && !shell_promotions.contains_key(*id)
                                })
                                .map(str::to_owned),
                        );
                    }
                }
                // The reference increments the background pane's revision, then
                // replaces its worker only when the pane attaches in the foreground.
                let running: Vec<_> = foreground_sessions
                    .iter()
                    .filter(|id| deferred_recovery.remove(*id))
                    .cloned()
                    .collect();
                owner.retire_sessions(&running);
                operations.retain(|(id, _, _)| !running.contains(id));
                for id in running {
                    shell_submissions.remove(&id);
                    sessions.remove(&id);
                    dirty.remove(&id);
                    sequence.remove(&id);
                    scroll_requests.remove(&id);
                    views.remove(&id);
                    awaiting_ui.remove(&id);
                    recovering.insert(id, state.workspace_id.clone());
                }
            }
            active_path_invalid = invalid;
            launch_validated = !invalid;
        }
        if path_checked {
            send.send_blocking(Event::WorkspacePaths(
                projected_paths.values().cloned().collect(),
            ))?;
        }

        let changes: Vec<_> = environment_changes.keys().cloned().collect();
        for session_id in changes {
            let change = environment_changes.get_mut(&session_id).unwrap();
            match change.advance(&mut owner, window_id) {
                Ok(EnvironmentProgress::Pending) => {}
                Ok(EnvironmentProgress::Stopping) => {
                    send.send_blocking(Event::Recovery {
                        session_id: session_id.clone(),
                        state: RecoveryState::Restoring,
                    })?;
                    sessions.remove(&session_id);
                    dirty.remove(&session_id);
                    sequence.remove(&session_id);
                    views.remove(&session_id);
                    awaiting_ui.remove(&session_id);
                    scroll_requests.remove(&session_id);
                    pending_input.retain(|input| input.session_id != session_id);
                    operations.retain(|(id, _, _)| id != &session_id);
                    searches.retain(|(id, _, _)| id != &session_id);
                    copies.retain(|(_, id, _, _)| id != &session_id);
                    failed_launches.remove(&session_id);
                    startup_deferred.remove(&session_id);
                    deferred_recovery.remove(&session_id);
                    graph_dirty = true;
                }
                Ok(EnvironmentProgress::Reply(result)) => {
                    send.send_blocking(change.request.reply(result))?;
                    environment_changes.remove(&session_id);
                    graph_dirty = true;
                }
                Err(error) => {
                    let stopped = matches!(change.phase, EnvironmentPhase::Stopping { .. });
                    let reason = change.rollback(&mut owner, false, &error)?;
                    if stopped && change.operation.previous_state == "ready" {
                        change.phase = EnvironmentPhase::Launching {
                            failure: Some(reason),
                        };
                    } else {
                        send.send_blocking(
                            change
                                .request
                                .reply(EnvironmentChangeResult::rejected(reason)),
                        )?;
                        environment_changes.remove(&session_id);
                    }
                    graph_dirty = true;
                }
            }
        }

        let mut launch_candidates = HashSet::new();
        if launch_validated {
            launch_candidates.extend(
                pending_launch
                    .iter()
                    .filter(|id| !recovering.contains_key(*id))
                    .cloned(),
            );
        }
        for (session_id, workspace_id) in &recovering {
            if !owner.has_shell_worker(session_id)
                && owner
                    .workspace_path_state(workspace_id)?
                    .is_some_and(|state| state.status == "valid")
            {
                launch_candidates.insert(session_id.clone());
            }
        }
        launch_candidates.extend(
            launch_retries
                .iter()
                .filter(|(id, retry)| retry.validated && !owner.shell_worker_is_retiring(id))
                .map(|(id, _)| id.clone()),
        );
        launch_candidates.retain(|id| !environment_changes.contains_key(id));
        launch_candidates.extend(environment_changes.iter().filter_map(|(id, change)| {
            (matches!(change.phase, EnvironmentPhase::Launching { .. })
                && !owner.shell_worker_is_retiring(id))
            .then(|| id.clone())
        }));
        launch_preparations.retain(|id, _| launch_candidates.contains(id));
        let mut launch_candidates: Vec<_> = launch_candidates.into_iter().collect();
        launch_candidates.sort_unstable_by(|left, right| {
            foreground_sessions
                .contains(right)
                .cmp(&foreground_sessions.contains(left))
                .then_with(|| {
                    startup_order
                        .get(left)
                        .unwrap_or(&usize::MAX)
                        .cmp(startup_order.get(right).unwrap_or(&usize::MAX))
                })
                .then_with(|| left.cmp(right))
        });
        // A fork can wait for the user's first prompt. Only actual restores
        // consume a recovery slot during identity confirmation.
        let restoring = provider_starting
            .values()
            .filter(|&&restoring| restoring)
            .count();
        let mut available_slots = 4usize.saturating_sub(restoring);
        let mut eligible = HashSet::new();
        let mut completed_launches = Vec::new();
        for session_id in launch_candidates {
            let fresh_codex = launch_retries
                .get(&session_id)
                .is_some_and(|retry| retry.fresh_codex);
            if owner
                .claude_binding(&session_id)?
                .is_some_and(|binding| binding.resumable)
                || (!fresh_codex
                    && owner
                        .codex_binding(&session_id)?
                        .is_some_and(|binding| binding.resumable))
            {
                if available_slots == 0 {
                    continue;
                }
                available_slots -= 1;
            }
            eligible.insert(session_id.clone());
            if !launch_preparations.contains_key(&session_id) {
                if let Err(error) = owner.prepare_control_fork_worktree(&session_id) {
                    completed_launches.push((session_id, Err(error)));
                    continue;
                }
                match prepare_launch(
                    &owner,
                    &options,
                    &session_id,
                    if environment_changes.contains_key(&session_id) {
                        LaunchIntent::Environment
                    } else if fresh_codex {
                        LaunchIntent::FreshCodex
                    } else {
                        LaunchIntent::Start
                    },
                    &role_requests,
                ) {
                    Ok(result) => {
                        launch_preparations.insert(session_id, result);
                    }
                    Err(error) => completed_launches.push((session_id, Err(error))),
                }
            }
        }
        launch_preparations.retain(|id, _| eligible.contains(id));
        launch_preparations.retain(|id, result| match result.try_recv() {
            Ok(launch) => {
                completed_launches.push((id.clone(), launch));
                false
            }
            Err(mpsc::TryRecvError::Empty) => true,
            Err(mpsc::TryRecvError::Disconnected) => {
                completed_launches.push((
                    id.clone(),
                    Err(anyhow!("session launch preparation disconnected")),
                ));
                false
            }
        });
        let mut started_shell = false;
        for (session_id, launch) in completed_launches {
            pending_launch.remove(&session_id);
            startup_deferred.remove(&session_id);
            startup_order.remove(&session_id);
            let rebuilding = recovering.remove(&session_id).is_some();
            let retry = launch_retries.remove(&session_id);
            let fresh_codex = retry.as_ref().is_some_and(|retry| retry.fresh_codex);
            let rebuilding = match retry {
                Some(retry) => retry.recovering,
                None => rebuilding || owner.has_shell_run(&session_id)?,
            };
            let binding = owner.claude_binding(&session_id)?;
            let resume = binding.as_ref().filter(|binding| binding.resumable);
            let mut waits_for_identity = resume.is_some();
            let mut uses_recovery_slot = resume.is_some();
            let result = launch.and_then(|launch| match launch {
                PreparedLaunch::Shell(directory) => {
                    owner.set_shell_launch_directory(&session_id, &directory)?;
                    launch_shell(
                        &mut owner,
                        &options,
                        &session_id,
                        directory,
                        &terminal_layouts,
                    )
                }
                PreparedLaunch::ClaudeCode {
                    launch,
                    fork_source,
                } => {
                    claude_launch_targets
                        .insert(session_id.clone(), claude_target_metadata(&launch.launch));
                    let terminal = terminal_layouts
                        .get(&session_id)
                        .copied()
                        .unwrap_or_else(default_terminal_options);
                    if let Some(source_id) = fork_source {
                        waits_for_identity = true;
                        return owner.launch_claude_fork(
                            session_id.clone(),
                            terminal,
                            launch,
                            source_id,
                        );
                    }
                    owner.launch_claude(session_id.clone(), terminal, launch, binding.as_ref())
                }
                PreparedLaunch::Antigravity(launch, permission) => {
                    waits_for_identity = launch.mode.conversation_id().is_some();
                    // Antigravity can stay at its authentication screen
                    // until the user acts. Keep other providers' restores moving.
                    uses_recovery_slot = false;
                    let terminal = terminal_layouts
                        .get(&session_id)
                        .copied()
                        .unwrap_or_else(default_terminal_options);
                    owner
                        .launch_antigravity(
                            &session_id,
                            terminal,
                            &data_root,
                            launch,
                            permission,
                            false,
                        )
                        .map(|_| ())
                }
                PreparedLaunch::Dsh(launch) => {
                    waits_for_identity = launch.plan.mode.expected_session_id().is_some();
                    uses_recovery_slot = waits_for_identity;
                    let terminal = terminal_layouts
                        .get(&session_id)
                        .copied()
                        .unwrap_or_else(default_terminal_options);
                    owner
                        .launch_dsh(&session_id, terminal, &data_root, launch, false)
                        .map(|_| ())
                }
                PreparedLaunch::CodexFork => {
                    waits_for_identity = true;
                    let terminal = terminal_layouts
                        .get(&session_id)
                        .copied()
                        .unwrap_or_else(default_terminal_options);
                    owner.launch_codex_fork(&session_id, terminal, &data_root)
                }
                PreparedLaunch::Codex(launch, resume) => {
                    waits_for_identity = resume.is_some();
                    uses_recovery_slot = waits_for_identity;
                    let terminal = terminal_layouts
                        .get(&session_id)
                        .copied()
                        .unwrap_or_else(default_terminal_options);
                    owner
                        .launch_codex(&session_id, terminal, launch, resume, false)
                        .map(|_| ())
                }
            });
            match result {
                Ok(()) => {
                    if let Some(change) = environment_changes.get_mut(&session_id) {
                        let EnvironmentPhase::Launching { failure } = &mut change.phase else {
                            unreachable!("queued environment launch")
                        };
                        change.phase = EnvironmentPhase::Confirming {
                            failure: failure.take(),
                            started_at: Instant::now(),
                        };
                        waits_for_identity = true;
                    }
                    failed_launches.remove(&session_id);
                    if waits_for_identity {
                        provider_starting.insert(session_id.clone(), uses_recovery_slot);
                    } else {
                        send.send_blocking(Event::Recovery {
                            session_id: session_id.clone(),
                            state: RecoveryState::Ready,
                        })?;
                    }
                    dirty.insert(session_id.clone());
                    sessions.insert(session_id);
                    graph_dirty = true;
                    started_shell = true;
                }
                Err(error) => {
                    if let Some(change) = environment_changes.get_mut(&session_id) {
                        if let EnvironmentPhase::Launching {
                            failure: Some(reason),
                        } = &change.phase
                        {
                            send.send_blocking(
                                change
                                    .request
                                    .reply(EnvironmentChangeResult::rejected(reason.clone())),
                            )?;
                            environment_changes.remove(&session_id);
                        } else {
                            let reason = change.rollback(&mut owner, true, &error)?;
                            graph_dirty = true;
                            if change.operation.previous_state == "ready" {
                                change.phase = EnvironmentPhase::Launching {
                                    failure: Some(reason),
                                };
                                continue;
                            }
                            send.send_blocking(
                                change
                                    .request
                                    .reply(EnvironmentChangeResult::rejected(reason)),
                            )?;
                            environment_changes.remove(&session_id);
                            continue;
                        }
                    }
                    claude_launch_targets.remove(&session_id);
                    pending_input.retain(|input| input.session_id != session_id);
                    let recovering = rebuilding && error.downcast_ref::<HookSetup>().is_none();
                    let failure = report_session_launch_failure(
                        &mut owner,
                        send,
                        &session_id,
                        recovering,
                        fresh_codex,
                        if fresh_codex {
                            codex_launch::failure_message(&error)
                        } else {
                            format!("{error:#}")
                        },
                    )?;
                    failed_launches.insert(session_id, failure);
                }
            }
        }
        // Only the submitted Shell's later input waits for its worker.
        let mut resolved_submissions = Vec::new();
        for (session_id, pending) in &shell_submissions {
            match pending.decision.try_recv() {
                Ok(decision) => {
                    if let Some(ShellSubmission {
                        launch,
                        before,
                        cleared,
                    }) = decision
                    {
                        shell_promotions.insert(
                            session_id.clone(),
                            ShellPromotion {
                                launch,
                                phase: ShellPromotionPhase::Clearing {
                                    result: cleared,
                                    before,
                                },
                                promoted: false,
                                inputs: VecDeque::new(),
                            },
                        );
                        if pending.record_on_match
                            && record_terminal_interaction(
                                &mut owner,
                                session_id,
                                "submit",
                                pending.defer_ordering,
                                send,
                            )?
                        {
                            tasks_dirty = true;
                            graph_dirty = true;
                        }
                    }
                    resolved_submissions.push(session_id.clone());
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    resolved_submissions.push(session_id.clone());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        retry_pending_input |= path_checked || started_shell || !resolved_submissions.is_empty();
        for id in resolved_submissions {
            if let Some(mut pending) = shell_submissions.remove(&id) {
                replay_shell_inputs.append(&mut pending.following);
            }
        }
        if !copies.iter().any(|(_, _, interrupt, _)| *interrupt)
            && std::mem::take(&mut retry_pending_input)
        {
            for _ in 0..pending_input.len() {
                let input = pending_input.pop_front().expect("queued terminal input");
                if !shell_submissions.contains_key(&input.session_id)
                    && (sessions.contains(&input.session_id)
                        || shell_promotions.contains_key(&input.session_id))
                    && !(foreground_sessions.contains(&input.session_id)
                        && deferred_recovery.contains(&input.session_id))
                    && owner
                        .workspace_path_state(&input.workspace_id)?
                        .is_some_and(|state| state.status == "valid")
                {
                    if send_input(
                        &mut owner,
                        input,
                        &mut shell_promotions,
                        &mut shell_submissions,
                        &mut operations,
                        send,
                    )? {
                        tasks_dirty = true;
                        graph_dirty = true;
                    }
                } else {
                    pending_input.push_back(input);
                }
            }
        }
        if let Err(error) = wiring.poll_egress(&mut owner) {
            send.send_blocking(Event::Diagnostic(format!("Egress recheck: {error:#}")))?;
        }
        let time_zone_changed = time_zone_changes.try_iter().last().is_some();
        if let Some(live) = &mut wiring.live_egress {
            if time_zone_changed {
                live.next_check = Instant::now();
            }
            let (replies, stopped) = match live.poll(&mut owner) {
                Ok(Some(result)) => (live.replies(result), false),
                Ok(None) => (Vec::new(), false),
                Err(error) => (live.replies(Err(error)), true),
            };
            if !replies.is_empty() {
                let graph =
                    active_graph(&owner, scene_id.as_deref(), window_id, &startup_deferred)?;
                for reply in replies {
                    send.send_blocking(with_graph_projection(reply, graph.clone()))?;
                }
            }
            if stopped {
                wiring.live_egress = None;
            }
        }
        if let Some(accounts) = &wiring.claude_accounts {
            for (request_id, state) in accounts.replies.try_iter() {
                let reply = wiring_result("claudeAccountsResult", request_id, Ok(json!(state)));
                let projection =
                    active_graph(&owner, scene_id.as_deref(), window_id, &startup_deferred)?;
                send.send_blocking(with_graph_projection(reply, projection))?;
            }
        }
        for _ in 0..128 {
            // Resolve Ctrl+C before a subsequent focus/shutdown can overtake its
            // control interaction. Ordinary copy does not hold this queue.
            if copies.iter().any(|(_, _, interrupt, _)| *interrupt) {
                break;
            }
            let command = match replay_shell_inputs
                .pop_front()
                .map_or_else(|| incoming.try_recv(), Ok)
            {
                Ok(command) => command,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => break 'running false,
            };
            if !StoragePhase::Writable.accepts(&command) {
                send.send_blocking(storage_command_rejected(
                    command,
                    "RECOVERY_NOT_PENDING",
                    DATABASE_RECOVERY_NOT_PENDING,
                ))?;
                continue;
            }
            let command = match command {
                Command::GetClaudeAccounts { request_id } => {
                    wiring.send_claude_account(
                        request_id,
                        crate::claude_accounts::ClaudeAccountCommand::List,
                        send,
                        || active_graph(&owner, scene_id.as_deref(), window_id, &startup_deferred),
                    )?;
                    continue;
                }
                Command::ClaudeAccountCommand { request_id, action } => {
                    wiring.send_claude_account(request_id, action, send, || {
                        active_graph(&owner, scene_id.as_deref(), window_id, &startup_deferred)
                    })?;
                    continue;
                }
                command => command,
            };
            if wiring
                .live_egress
                .as_mut()
                .is_some_and(|live| live.handle(&command))
            {
                continue;
            }
            if matches!(command, Command::GitPush { .. }) {
                let send = send.clone();
                let data_root = data_root.clone();
                thread::spawn(move || {
                    if let Some(reply) = run_git_command(&command, &data_root) {
                        let _ = send.send_blocking(reply);
                    }
                });
                continue;
            }
            if wiring.dispatch_live_update(&command, &data_root, send) {
                continue;
            }
            if let Some(reply) = wiring.handle(&command, &data_root, Some(&mut owner)) {
                let projection =
                    active_graph(&owner, scene_id.as_deref(), window_id, &startup_deferred)?;
                send.send_blocking(with_graph_projection(reply, projection))?;
                continue;
            }
            if let Command::Input { session_id, .. } | Command::CopySelection { session_id, .. } =
                &command
                && let Some(pending) = shell_submissions.get_mut(session_id)
            {
                pending.following.push_back(command);
                continue;
            }
            if matches!(
                &command,
                Command::CreateWorkspace { .. }
                    | Command::SelectWorkspace { .. }
                    | Command::RelinkWorkspace { .. }
                    | Command::SetWorkspacePinned { .. }
                    | Command::ReorderPinnedWorkspace { .. }
                    | Command::RemoveWorkspace { .. }
                    | Command::CreateTask { .. }
                    | Command::SelectTask { .. }
                    | Command::RenameTask { .. }
                    | Command::SetTaskPinned { .. }
                    | Command::ReorderPinnedTask { .. }
                    | Command::DeleteTask { .. }
                    | Command::CreateCanvas { .. }
                    | Command::CreateShellSibling { .. }
                    | Command::ForkSession { .. }
                    | Command::RetryFork { .. }
                    | Command::SelectScene { .. }
                    | Command::RenameScene { .. }
                    | Command::ReorderScene { .. }
                    | Command::CloseScene { .. }
                    | Command::CloseSessionCard { .. }
                    | Command::RemoveSessionBranch { .. }
                    | Command::FocusSession { .. }
            ) {
                // Each command mutates the authoritative domain before the view changes.
                // A failed launch leaves its real created structure available for recovery.
                let attempted_focus = match &command {
                    Command::FocusSession {
                        scene_id,
                        session_id,
                    } => Some((scene_id.clone(), session_id.clone())),
                    _ => None,
                };
                let rename = match &command {
                    Command::RenameTask { task_id, title } => {
                        Some((task_id.clone(), title.clone()))
                    }
                    _ => None,
                };
                let (fork_submission, fork_retry) = match &command {
                    Command::ForkSession { submission_key, .. } => {
                        (Some(submission_key.clone()), None)
                    }
                    Command::RetryFork { session_id } => (None, Some(session_id.clone())),
                    _ => (None, None),
                };
                let workspace_action = match &command {
                    Command::CreateWorkspace { .. } => Some(("create", None)),
                    Command::RelinkWorkspace { workspace_id, .. } => {
                        Some(("relink", Some(workspace_id.clone())))
                    }
                    Command::SetWorkspacePinned { workspace_id, .. } => {
                        Some(("pin", Some(workspace_id.clone())))
                    }
                    _ => None,
                };
                let check_target = matches!(
                    &command,
                    Command::CreateWorkspace { .. }
                        | Command::SelectWorkspace { .. }
                        | Command::RelinkWorkspace { .. }
                        | Command::CreateTask { .. }
                        | Command::SelectTask { .. }
                        | Command::CreateCanvas { .. }
                        | Command::CreateShellSibling { .. }
                        | Command::RetryFork { .. }
                        | Command::SelectScene { .. }
                );
                match navigate(&mut owner, window_id, command) {
                    Ok((hierarchy, disposed, action)) => {
                        let (card_action, fork_result) = match action {
                            Some(NavigationAction::Card(action)) => (Some(action), None),
                            Some(NavigationAction::Fork(mut result)) => {
                                if let Some(submission_key) = &fork_submission {
                                    result["submissionKey"] = json!(submission_key);
                                }
                                if result["replayed"] == true {
                                    let mut graph = active_graph(
                                        &owner,
                                        scene_id.as_deref(),
                                        window_id,
                                        &startup_deferred,
                                    )?;
                                    graph["forkSessionResult"] = result;
                                    send.send_blocking(Event::Graph(graph))?;
                                    continue;
                                }
                                (None, Some(result))
                            }
                            None => (None, None),
                        };
                        let wait_for_workers = card_action.is_some();
                        let defer_publication =
                            wait_for_workers || !navigation_publications.is_empty();
                        if !disposed.is_empty() {
                            for id in &disposed {
                                shell_submissions.remove(id);
                                shell_promotions.remove(id);
                                if let Some(load) = catalog_loads.remove(id) {
                                    let error =
                                        anyhow!("Card was closed while loading the conversation");
                                    if !load.antigravity_requests.is_empty() {
                                        let projection = active_graph(
                                            &owner,
                                            scene_id.as_deref(),
                                            window_id,
                                            &startup_deferred,
                                        )?;
                                        for request_id in load.antigravity_requests {
                                            send.send_blocking(with_graph_projection(
                                                wiring_result(
                                                    "antigravitySessionLoadResult",
                                                    request_id,
                                                    Err(anyhow!("{error:#}")),
                                                ),
                                                projection.clone(),
                                            ))?;
                                        }
                                    }
                                    catalog_load_errors.push(catalog_load_error(
                                        id,
                                        &load.provider_session_id,
                                        &error,
                                    ));
                                }
                                if let Some(change) = permission_changes.remove(id) {
                                    permission_results.push(permission_result(
                                        id,
                                        &change.mode,
                                        Some("Card was closed during permission respawn".into()),
                                    ));
                                    if let Err(error) = change.rollback_if_pending(&mut owner, id) {
                                        send.send_blocking(Event::Diagnostic(format!(
                                            "{error:#}"
                                        )))?;
                                    }
                                }
                                sessions.remove(id);
                                dirty.remove(id);
                                sequence.remove(id);
                                scroll_requests.remove(id);
                                views.remove(id);
                                awaiting_ui.remove(id);
                                terminal_layouts.remove(id);
                                recovering.remove(id);
                                deferred_recovery.remove(id);
                                startup_deferred.remove(id);
                                failed_launches.remove(id);
                                launch_retries.remove(id);
                                launch_preparations.remove(id);
                                claude_launch_targets.remove(id);
                            }
                            pending_input.retain(|input| !disposed.contains(&input.session_id));
                            operations.retain(|(id, _, _)| sessions.contains(id));
                            searches.retain(|(id, _, _)| !disposed.contains(id));
                            copies.retain(|(_, id, _, _)| !disposed.contains(id));
                            if !defer_publication {
                                send.send_blocking(Event::ScenesDisposed(disposed.clone()))?;
                            }
                        }
                        let next_scene = hierarchy.scene.as_ref().map(|scene| scene.id.clone());
                        let next_workspace = hierarchy
                            .workspace
                            .as_ref()
                            .map(|workspace| workspace.id.clone());
                        let workspace_changed = active_workspace != next_workspace;
                        let target_changed = scene_id != next_scene || workspace_changed;
                        if workspace_changed {
                            active_path_invalid =
                                workspace_is_invalid(&projected_paths, next_workspace.as_deref());
                        }
                        scene_id = next_scene;
                        active_task = hierarchy.task.as_ref().map(|task| task.id.clone());
                        active_workspace = next_workspace;
                        let mut graph = active_graph(
                            &owner,
                            scene_id.as_deref(),
                            window_id,
                            &startup_deferred,
                        )?;
                        let next_focus = graph["focusedSessionId"].as_str();
                        if !target_changed
                            && focused_session.as_deref() != next_focus
                            && let Some(pending) = focused_session
                                .as_ref()
                                .and_then(|id| shell_submissions.get_mut(id))
                        {
                            // The focus switch already flushed this card's deferred ordering.
                            pending.defer_ordering = false;
                        }
                        focused_session = next_focus.map(str::to_owned);
                        pending_launch = unstarted_sessions(&owner, &graph, &sessions)?;
                        pending_launch.retain(|id| {
                            !startup_deferred.contains(id)
                                && !failed_launches.contains_key(id)
                                && !launch_retries.contains_key(id)
                                && !catalog_loads.contains_key(id)
                                && !permission_changes.contains_key(id)
                                && !shell_promotions.contains_key(id)
                        });
                        foreground_sessions = launchable_ids(&graph).map(str::to_owned).collect();
                        if (target_changed || check_target || !pending_launch.is_empty())
                            && let Some(workspace_id) = &active_workspace
                        {
                            checks.request(&owner, workspace_id)?;
                            launch_validated = false;
                            next_active_check = Instant::now() + ACTIVE_PATH_INTERVAL;
                        }
                        if let Some(result) = fork_result {
                            let child = result["sessionId"]
                                .as_str()
                                .context("Fork result has no session")?;
                            let workspace_id = active_workspace
                                .clone()
                                .context("Fork result has no workspace")?;
                            pending_launch.remove(child);
                            failed_launches.remove(child);
                            launch_retries.insert(
                                child.to_owned(),
                                LaunchRetry {
                                    workspace_id,
                                    recovering: false,
                                    validated: false,
                                    fresh_codex: false,
                                },
                            );
                            graph["forkSessionResult"] = result;
                        }
                        let scenes = hierarchy
                            .task
                            .as_ref()
                            .map(|task| owner.scenes(&task.id))
                            .transpose()?
                            .unwrap_or_default();
                        if !defer_publication {
                            send_sidebar(&owner, window_id, send)?;
                        }
                        if let Some((action, workspace_id)) = workspace_action {
                            if action == "relink"
                                && let Some(workspace_id) = &workspace_id
                                && Some(workspace_id) != active_workspace.as_ref()
                            {
                                checks.request(&owner, workspace_id)?;
                            }
                            let workspace_id = workspace_id.or_else(|| {
                                hierarchy
                                    .workspace
                                    .as_ref()
                                    .map(|workspace| workspace.id.clone())
                            });
                            send.send_blocking(Event::WorkspaceActionResult {
                                action: action.into(),
                                workspace_id,
                                error: None,
                            })?;
                        }
                        if let Some((task_id, title)) = rename {
                            send.send_blocking(Event::TaskRenameResult {
                                task_id,
                                title,
                                error: None,
                            })?;
                        }
                        if let Some(mut action) = card_action {
                            // The outer graph is already the affected canvas for foreground actions.
                            if action["sceneId"] == graph["sceneId"]
                                && let Some(action) = action.as_object_mut()
                            {
                                action.remove("graph");
                            }
                            graph["sessionCardAction"] = action;
                        }
                        let event = Event::Navigation {
                            hierarchy,
                            scenes,
                            graph,
                            geometry: scene_id
                                .as_deref()
                                .map(|id| owner.geometry(id))
                                .transpose()?
                                .unwrap_or_default(),
                        };
                        if defer_publication {
                            navigation_publications.push_back((disposed, wait_for_workers, event));
                        } else {
                            send.send_blocking(event)?;
                        }
                        // Existing workers and terminal surfaces remain alive across tab changes.
                        dirty.extend(sessions.iter().cloned());
                    }
                    Err(error) => {
                        if fork_submission.is_some() || fork_retry.is_some() {
                            let mut graph = active_graph(
                                &owner,
                                scene_id.as_deref(),
                                window_id,
                                &startup_deferred,
                            )?;
                            let mut result = json!({
                                "code": error.downcast_ref::<CanvasError>().map_or("FORK_CREATE_FAILED", |error| error.code),
                                "error": format!("{error:#}"),
                            });
                            if let Some(submission_key) = fork_submission {
                                result["submissionKey"] = json!(submission_key);
                            }
                            if let Some(session_id) = fork_retry {
                                result["sessionId"] = json!(session_id);
                            }
                            graph["forkSessionResult"] = result;
                            send.send_blocking(Event::Graph(graph))?;
                        } else if let Some((action, workspace_id)) = workspace_action {
                            send.send_blocking(Event::WorkspaceActionResult {
                                action: action.into(),
                                workspace_id,
                                error: Some(format!("{error:#}")),
                            })?;
                        } else if let Some((task_id, title)) = rename {
                            send.send_blocking(Event::TaskRenameResult {
                                task_id,
                                title,
                                error: Some(format!("{error:#}")),
                            })?;
                        } else {
                            send.send_blocking(Event::NavigationFailed {
                                message: format!("{error:#}"),
                                attempted_focus,
                            })?;
                        }
                    }
                }
                continue;
            }
            let (reply, result) = mpsc::channel();
            let (session_id, request, refresh) = match command {
                Command::SetTaskStatus {
                    request_id,
                    workspace_id,
                    task_id,
                    status,
                    before_task_id,
                } => {
                    let time = self::now();
                    let input = tasks::MoveTaskOnBoardInput {
                        workspace_id,
                        task_id,
                        status,
                        before_task_id,
                        now: time,
                    };
                    let result = owner.move_task_on_board(
                        &metadata(
                            "hierarchy.move-task-on-board",
                            &serde_json::to_value(&input)?,
                        ),
                        &input,
                        time,
                    );
                    let workspaces = owner.workspaces()?;
                    send.send_blocking(Event::Tasks(sidebar_tasks(
                        &owner,
                        window_id,
                        &workspaces,
                    )?))?;
                    if result.is_ok() {
                        send.send_blocking(Event::Graph(active_graph(
                            &owner,
                            scene_id.as_deref(),
                            window_id,
                            &startup_deferred,
                        )?))?;
                    }
                    send.send_blocking(wiring_result("taskStatusResult", request_id, result))?;
                    continue;
                }
                Command::DatabaseRecovery { .. } | Command::ReadOnlyQuery { .. } => {
                    unreachable!("storage-only commands cannot pass the writable gate");
                }
                Command::SetSessionTitle { session_id, title } => {
                    let error = match owner.set_session_title(&session_id, title.as_deref()) {
                        Ok(commit) => {
                            graph_dirty |= commit.is_some();
                            None
                        }
                        Err(error) => Some(format!("{error:#}")),
                    };
                    send.send_blocking(Event::SessionTitleResult {
                        session_id,
                        title,
                        error,
                    })?;
                    continue;
                }
                Command::GetClaudeModelCatalog { request_id } => {
                    if model_catalog_worker.is_err()
                        || model_catalog_requests.send(request_id).is_err()
                    {
                        model_catalog_results.send(ClaudeModelCatalogReply {
                            request_id,
                            result: ClaudeModelCatalogResult::Error(
                                "Claude model catalog worker is unavailable".into(),
                            ),
                        })?;
                    }
                    continue;
                }
                Command::GetClaudeRoleAssignments { request_id }
                | Command::GetClaudeModelPreferences { request_id } => {
                    role_requests.send(RoleAssignmentsRequest::Get {
                        request_id,
                        include_compaction: matches!(
                            command,
                            Command::GetClaudeModelPreferences { .. }
                        ),
                    })?;
                    continue;
                }
                Command::GetTimeZoneSettings { request_id } => {
                    role_requests.send(RoleAssignmentsRequest::TimeZone {
                        request_id,
                        setting: None,
                    })?;
                    continue;
                }
                Command::SetTimeZoneSettings {
                    request_id,
                    time_zone,
                } => {
                    role_requests.send(RoleAssignmentsRequest::TimeZone {
                        request_id,
                        setting: Some(time_zone),
                    })?;
                    continue;
                }
                Command::SetClaudeRoleAssignments {
                    request_id,
                    assignments,
                    web_search,
                } => {
                    role_requests.send(RoleAssignmentsRequest::Set {
                        request_id,
                        assignments,
                        web_search,
                        compaction: None,
                        include_compaction: false,
                    })?;
                    continue;
                }
                Command::SetClaudeModelPreferences {
                    request_id,
                    assignments,
                    web_search,
                    compaction,
                } => {
                    role_requests.send(RoleAssignmentsRequest::Set {
                        request_id,
                        assignments,
                        web_search,
                        compaction,
                        include_compaction: true,
                    })?;
                    continue;
                }
                Command::GetProviderSettings { request_id } => {
                    provider_settings.send(ProviderSettingsRequest::Get(request_id))?;
                    continue;
                }
                Command::SaveProvider { request_id, input } => {
                    provider_settings.send(ProviderSettingsRequest::Save(request_id, input))?;
                    continue;
                }
                Command::FetchProviderModels { request_id, input } => {
                    provider_settings
                        .send(ProviderSettingsRequest::FetchModels(request_id, input))?;
                    continue;
                }
                Command::DeleteProvider {
                    request_id,
                    cli,
                    id,
                } => {
                    provider_settings.send(ProviderSettingsRequest::Delete(request_id, cli, id))?;
                    continue;
                }
                Command::SetActiveProvider {
                    request_id,
                    cli,
                    id,
                } => {
                    provider_settings
                        .send(ProviderSettingsRequest::SetActive(request_id, cli, id))?;
                    continue;
                }
                Command::ConfirmAntigravityRisk { request_id } => {
                    provider_settings.send(ProviderSettingsRequest::ConfirmRisk(request_id))?;
                    continue;
                }
                Command::Input {
                    session_id,
                    bytes,
                    defer_ordering,
                } => {
                    if permission_changes.contains_key(&session_id) {
                        if let Err(error) = owner.send_permission_consent_input(&session_id, bytes)
                        {
                            send.send_blocking(Event::Diagnostic(format!("{error:#}")))?;
                        }
                        continue;
                    }
                    if failed_launches.contains_key(&session_id)
                        || launch_retries.contains_key(&session_id)
                        || catalog_loads
                            .get(&session_id)
                            .is_some_and(|load| load.stopping.is_some())
                        || (!sessions.contains(&session_id)
                            && !shell_promotions.contains_key(&session_id)
                            && !pending_launch.contains(&session_id)
                            && owner
                                .claude_binding(&session_id)?
                                .is_some_and(|binding| binding.restore_state == "failed"))
                    {
                        continue;
                    }
                    let Some(workspace_id) = owner.session_workspace(&session_id)? else {
                        continue;
                    };
                    let state = owner.workspace_path_state(&workspace_id)?;
                    if state
                        .as_ref()
                        .is_some_and(|state| state.status == "invalid")
                    {
                        send.send_blocking(Event::Diagnostic(WORKSPACE_PATH_INVALID.into()))?;
                        continue;
                    }
                    let input = PendingInput {
                        workspace_id,
                        session_id,
                        bytes,
                        defer_ordering,
                    };
                    if !shell_promotions.contains_key(&input.session_id)
                        && (state.is_none()
                            || !sessions.contains(&input.session_id)
                            || (foreground_sessions.contains(&input.session_id)
                                && deferred_recovery.contains(&input.session_id)))
                    {
                        checks.request(&owner, &input.workspace_id)?;
                        pending_input.push_back(input);
                    } else if send_input(
                        &mut owner,
                        input,
                        &mut shell_promotions,
                        &mut shell_submissions,
                        &mut operations,
                        send,
                    )? {
                        tasks_dirty = true;
                        graph_dirty = true;
                    }
                    continue;
                }
                Command::Resize {
                    session_id,
                    cols,
                    rows,
                    cell_width_px,
                    cell_height_px,
                } => {
                    let live = sessions.contains(&session_id);
                    if live || owner.session_workspace(&session_id)?.is_some() {
                        let options = terminal_layouts
                            .entry(session_id.clone())
                            .or_insert_with(default_terminal_options);
                        options.cols = cols;
                        options.rows = rows;
                        options.cell_width_px = cell_width_px;
                        options.cell_height_px = cell_height_px;
                    }
                    if !live {
                        continue;
                    }
                    (
                        session_id,
                        WorkerCommand::Resize {
                            cols,
                            rows,
                            cell_width_px,
                            cell_height_px,
                            reply,
                        },
                        true,
                    )
                }
                Command::History(request) => {
                    history.send(request)?;
                    continue;
                }
                Command::Search {
                    session_id,
                    request,
                } => {
                    if sessions.contains(&session_id) {
                        let sequence = request.sequence;
                        let (reply, receive) = mpsc::channel();
                        owner.send(&session_id, WorkerCommand::Search { request, reply })?;
                        searches.push((session_id, sequence, receive));
                    }
                    continue;
                }
                Command::ClearSearch { session_id } => {
                    if !sessions.contains(&session_id) {
                        continue;
                    }
                    (session_id, WorkerCommand::ClearSearch { reply }, true)
                }
                Command::CopySelection {
                    session_id,
                    request_id,
                    interrupt_if_empty,
                } => {
                    if sessions.contains(&session_id) {
                        let (reply, receive) = mpsc::channel();
                        // Starting/Verifying use the current PTY for copy and empty-selection interrupts.
                        // Suppress interrupts only in Clearing/Stopping/Preparing/RollingBack/RestoringShell.
                        let interrupt_if_empty = interrupt_if_empty
                            && !permission_changes.contains_key(&session_id)
                            && shell_promotions
                                .get(&session_id)
                                .is_none_or(ShellPromotion::attached);
                        owner.send(
                            &session_id,
                            WorkerCommand::CopySelection {
                                reply,
                                interrupt_if_empty,
                            },
                        )?;
                        copies.push((request_id, session_id, interrupt_if_empty, receive));
                    } else if shell_promotions.contains_key(&session_id) {
                        send.send_blocking(Event::Copied {
                            request_id,
                            text: String::new(),
                        })?;
                    }
                    continue;
                }
                Command::ScrollSelection { session_id, lines } => {
                    if !sessions.contains(&session_id) {
                        continue;
                    }
                    (
                        session_id,
                        WorkerCommand::ScrollSelection { lines, reply },
                        true,
                    )
                }
                Command::Select { session_id, action } => {
                    if !sessions.contains(&session_id) {
                        continue;
                    }
                    (session_id, WorkerCommand::Select { action, reply }, true)
                }
                Command::Scroll { session_id, lines } => {
                    if !sessions.contains(&session_id) {
                        continue;
                    }
                    (session_id, WorkerCommand::Scroll { lines, reply }, true)
                }
                Command::ScrollTo {
                    session_id,
                    row,
                    request_id,
                    pixel_target,
                } => {
                    if !sessions.contains(&session_id) {
                        continue;
                    }
                    scroll_requests.insert(session_id.clone(), request_id);
                    (
                        session_id,
                        WorkerCommand::ScrollTo {
                            row,
                            pixel_target,
                            reply,
                        },
                        false,
                    )
                }
                Command::Theme { session_id, theme } => {
                    let live = sessions.contains(&session_id);
                    if live || owner.session_workspace(&session_id)?.is_some() {
                        terminal_layouts
                            .entry(session_id.clone())
                            .or_insert_with(default_terminal_options)
                            .theme = theme;
                    }
                    if !live {
                        continue;
                    }
                    (session_id, WorkerCommand::SetTheme { theme, reply }, true)
                }
                Command::Applied {
                    session_id,
                    sequence,
                } => {
                    if awaiting_ui.get(&session_id) != Some(&sequence) {
                        continue;
                    }
                    awaiting_ui.remove(&session_id);
                    // A layout frame before any PTY output has no output credit.
                    if sequence == 0 || !sessions.contains(&session_id) {
                        continue;
                    }
                    (
                        session_id,
                        WorkerCommand::Acknowledge { sequence, reply },
                        false,
                    )
                }
                Command::SetProviderPermissionMode { session_id, mode } => {
                    let profile = match owner
                        .session(&session_id)?
                        .as_ref()
                        .map(|session| session.kind.as_str())
                    {
                        Some("antigravity") => LaunchProfile::Antigravity,
                        Some("codex") => LaunchProfile::Codex,
                        _ => LaunchProfile::ClaudeCode,
                    };
                    let modes: &[&str] = if profile == LaunchProfile::Codex {
                        &codex_permissions::MODES
                    } else {
                        &CLAUDE_PERMISSION_MODES
                    };
                    if !modes.contains(&mode.as_str()) {
                        permission_results.push(permission_result(
                            &session_id,
                            &mode,
                            Some("invalid permission mode".into()),
                        ));
                        continue;
                    }
                    if let Some(change) = permission_changes.get(&session_id) {
                        if change.mode != mode {
                            permission_results.push(permission_result(
                                &session_id,
                                &mode,
                                Some("AI Session is already stopping or recovering".into()),
                            ));
                        }
                        continue;
                    }
                    let attempt = (|| -> Result<Option<PermissionChange>> {
                        ensure!(
                            !pending_launch.contains(&session_id)
                                && !recovering.contains_key(&session_id)
                                && !launch_retries.contains_key(&session_id)
                                && !shell_promotions.contains_key(&session_id)
                                && !catalog_loads.contains_key(&session_id),
                            "AI Session is already stopping or recovering"
                        );
                        let binding = owner.permission_binding(&session_id)?;
                        if binding.as_ref().is_some_and(|binding| {
                            let permission = binding.metadata.get("permissionMode");
                            let current = if profile == LaunchProfile::Codex {
                                codex_permissions::selected_mode(permission)
                            } else {
                                permission
                                    .and_then(Value::as_str)
                                    .filter(|mode| {
                                        CLAUDE_PERMISSION_MODES.contains(mode)
                                            || profile == LaunchProfile::Antigravity
                                                && *mode == "ask"
                                    })
                                    .unwrap_or("default")
                            };
                            current == mode
                        }) {
                            return Ok(None);
                        }
                        let stopped = owner.stop_for_permission_change(&session_id)?;
                        Ok(Some(PermissionChange {
                            profile,
                            mode: mode.clone(),
                            binding,
                            phase: PermissionPhase::Stopping(stopped),
                        }))
                    })();
                    match attempt {
                        Ok(Some(change)) => {
                            if change.binding.is_none() {
                                failed_launches.remove(&session_id);
                            }
                            sessions.remove(&session_id);
                            dirty.remove(&session_id);
                            sequence.remove(&session_id);
                            scroll_requests.remove(&session_id);
                            views.remove(&session_id);
                            awaiting_ui.remove(&session_id);
                            pending_input.retain(|input| input.session_id != session_id);
                            operations.retain(|(id, _, _)| id != &session_id);
                            searches.retain(|(id, _, _)| id != &session_id);
                            copies.retain(|(_, id, _, _)| id != &session_id);
                            permission_changes.insert(session_id.clone(), change);
                            send.send_blocking(Event::Recovery {
                                session_id,
                                state: RecoveryState::Restoring,
                            })?;
                        }
                        Ok(None) => {
                            permission_results.push(permission_result(&session_id, &mode, None))
                        }
                        Err(error) => permission_results.push(permission_result(
                            &session_id,
                            &mode,
                            Some(format!("{error:#}")),
                        )),
                    }
                    continue;
                }
                Command::ListProviderSessions { cwd } => {
                    if let Err(
                        mpsc::TrySendError::Full(CatalogRequest::Index { cwd, .. })
                        | mpsc::TrySendError::Disconnected(CatalogRequest::Index { cwd, .. }),
                    ) = catalog_requests.try_send(CatalogRequest::Index { cwd, load: None })
                    {
                        catalog_lists.push(json!({"cwd": cwd, "error": CATALOG_BUSY}));
                    }
                    continue;
                }
                Command::GetProviderSession {
                    cwd,
                    provider_session_id,
                    start_event_index,
                    limit,
                } => {
                    if let Err(
                        mpsc::TrySendError::Full(CatalogRequest::Detail(request))
                        | mpsc::TrySendError::Disconnected(CatalogRequest::Detail(request)),
                    ) = catalog_requests.try_send(CatalogRequest::Detail(CatalogDetailRequest {
                        cwd,
                        provider_session_id,
                        start_event_index,
                        limit,
                    })) {
                        catalog_details.push(request.result(Err(CATALOG_BUSY)));
                    }
                    continue;
                }
                Command::ListCodexSessions {
                    request_id,
                    cwd,
                    query,
                    full_text,
                    offset,
                    limit,
                } => {
                    let request = CatalogRequest::CodexList {
                        request_id,
                        cwd: cwd.clone(),
                        options: codex_catalog::ListOptions {
                            query,
                            full_text,
                            offset,
                            limit,
                        },
                    };
                    if catalog_requests.try_send(request).is_err() {
                        catalog_lists.push(codex_catalog_result(
                            request_id,
                            &cwd,
                            Err(CODEX_CATALOG_BUSY),
                        ));
                    }
                    continue;
                }
                Command::GetCodexSession {
                    request_id,
                    cwd,
                    provider_session_id,
                    start_event_index,
                    around_event_index,
                    limit,
                } => {
                    let request = CatalogRequest::CodexDetail {
                        request_id,
                        cwd: cwd.clone(),
                        provider_session_id,
                        start: start_event_index,
                        around: around_event_index,
                        limit,
                    };
                    if catalog_requests.try_send(request).is_err() {
                        catalog_details.push(codex_catalog_result(
                            request_id,
                            &cwd,
                            Err(CODEX_CATALOG_BUSY),
                        ));
                    }
                    continue;
                }
                Command::SearchCodexSession {
                    request_id,
                    cwd,
                    provider_session_id,
                    query,
                    offset,
                    limit,
                } => {
                    let request = CatalogRequest::CodexSearch {
                        request_id,
                        cwd: cwd.clone(),
                        provider_session_id,
                        query,
                        offset,
                        limit,
                    };
                    if catalog_requests.try_send(request).is_err() {
                        let mut result =
                            codex_catalog_result(request_id, &cwd, Err(CODEX_CATALOG_BUSY));
                        result["search"] = json!(true);
                        catalog_details.push(result);
                    }
                    continue;
                }
                Command::GetDeferredRecoveryPreview { session_id } => {
                    if !startup_deferred.contains(&session_id)
                        || !foreground_sessions.contains(&session_id)
                    {
                        continue;
                    }
                    let Some(session) = owner.session(&session_id)? else {
                        continue;
                    };
                    let cwd = session.cwd;
                    let binding = owner.claude_binding(&session_id)?;
                    if let Some(binding) =
                        binding.filter(|binding| !binding.provider_session_id.is_empty())
                    {
                        let request = CatalogRecentRequest {
                            session_id,
                            cwd,
                            provider_session_id: binding.provider_session_id,
                        };
                        if let Err(
                            mpsc::TrySendError::Full(CatalogRequest::Recent(request))
                            | mpsc::TrySendError::Disconnected(CatalogRequest::Recent(request)),
                        ) = catalog_requests.try_send(CatalogRequest::Recent(request))
                        {
                            send.send_blocking(request.event(None))?;
                        }
                    } else {
                        send.send_blocking(Event::DeferredRecoveryPreview {
                            session_id,
                            cwd,
                            preview: None,
                        })?;
                    }
                    continue;
                }
                command @ (Command::LoadProviderSession { .. }
                | Command::LoadCodexSession { .. }
                | Command::LoadDshSession { .. }
                | Command::LoadAntigravitySession { .. }) => {
                    let (provider, session_id, provider_session_id, request_id) = match command {
                        Command::LoadProviderSession {
                            session_id,
                            provider_session_id,
                        } => ("claude-code", session_id, provider_session_id, None),
                        Command::LoadCodexSession {
                            session_id,
                            provider_session_id,
                        } => (
                            "codex",
                            session_id,
                            provider_session_id.to_ascii_lowercase(),
                            None,
                        ),
                        Command::LoadDshSession {
                            request_id,
                            session_id,
                            provider_session_id,
                        } => ("dsh", session_id, provider_session_id, Some(request_id)),
                        Command::LoadAntigravitySession {
                            request_id,
                            session_id,
                            provider_session_id,
                        } => (
                            "antigravity",
                            session_id,
                            provider_session_id,
                            Some(request_id),
                        ),
                        _ => unreachable!(),
                    };
                    let attempt = (|| -> Result<()> {
                        anyhow::ensure!(
                            foreground_sessions.contains(&session_id),
                            "Card is no longer available in this scene"
                        );
                        ensure!(
                            !permission_changes.contains_key(&session_id)
                                && !shell_promotions.contains_key(&session_id),
                            "AI Session is already stopping or recovering"
                        );
                        if provider == "codex" {
                            owner.ensure_codex_catalog_available(
                                &session_id,
                                &provider_session_id,
                            )?;
                            ensure_codex_catalog_not_pending(
                                &catalog_loads,
                                &session_id,
                                &provider_session_id,
                            )?;
                        }
                        if let Some(loading) = catalog_loads.get_mut(&session_id) {
                            anyhow::ensure!(
                                loading.provider == provider
                                    && loading.provider_session_id == provider_session_id,
                                "Another conversation is already loading"
                            );
                            if provider == "antigravity"
                                && let Some(request_id) = request_id
                            {
                                if loading.spawn_revision.is_some() {
                                    return Err(anyhow!(
                                        "The Antigravity conversation is already restoring"
                                    ));
                                }
                                loading.antigravity_requests.push(request_id);
                            }
                            return Ok(());
                        }
                        if provider == "claude-code"
                            && let Some(identity) = owner.already_loaded_catalog_identity(
                                &session_id,
                                &provider_session_id,
                            )?
                        {
                            provider_identity_events.push(identity);
                            return Ok(());
                        }
                        if launch_retries.contains_key(&session_id)
                            || owner.waiting_for_provider_restore(&session_id)
                        {
                            let duplicate = provider == "claude-code"
                                && owner.claude_binding(&session_id)?.is_some_and(|binding| {
                                    binding.resumable
                                        && binding.provider_session_id == provider_session_id
                                        && binding.metadata["loadedFromCatalog"] == true
                                });
                            anyhow::ensure!(
                                duplicate,
                                "Card already has a running or starting process"
                            );
                            return Ok(());
                        }
                        anyhow::ensure!(
                            !pending_launch.contains(&session_id)
                                && !recovering.contains_key(&session_id),
                            "Card already has a starting process"
                        );
                        anyhow::ensure!(
                            owner.can_launch_provider(&session_id)
                                || (owner.can_replace_catalog_run(&session_id)
                                    && !owner.claude_binding(&session_id)?.is_some_and(
                                        |binding| { binding.restore_state == "failed" }
                                    )
                                    && !owner.codex_binding(&session_id)?.is_some_and(|binding| {
                                        binding.restore_state == "failed"
                                    })),
                            "Card already has a running or starting process"
                        );
                        let stored = owner
                            .session(&session_id)?
                            .filter(|session| {
                                session.archived_at.is_none()
                                    && matches!(
                                        session.kind.as_str(),
                                        "shell" | "claude-code" | "codex" | "dsh" | "antigravity"
                                    )
                            })
                            .context("Card is no longer available")?;
                        catalog_requests
                            .try_send(CatalogRequest::Index {
                                cwd: stored.cwd,
                                load: Some(CatalogLoadTarget {
                                    provider,
                                    scene_id: scene_id.clone().context("No active scene")?,
                                    session_id: session_id.clone(),
                                    provider_session_id: provider_session_id.clone(),
                                }),
                            })
                            .map_err(|_| {
                                anyhow!(if provider == "codex" {
                                    CODEX_CATALOG_BUSY
                                } else {
                                    CATALOG_BUSY
                                })
                            })?;
                        catalog_loads.insert(
                            session_id.clone(),
                            CatalogLoad {
                                provider,
                                provider_session_id: provider_session_id.clone(),
                                spawn_revision: None,
                                stopping: None,
                                antigravity_requests: request_id
                                    .filter(|_| provider == "antigravity")
                                    .into_iter()
                                    .collect(),
                            },
                        );
                        Ok(())
                    })();
                    if let Some(request_id) = request_id
                        && (provider != "antigravity" || attempt.is_err())
                    {
                        let result = match &attempt {
                            Ok(()) => Ok(json!({
                                "sessionId": session_id, "providerSessionId": provider_session_id, "status": "queued",
                            })),
                            Err(error) => Err(anyhow!("{error:#}")),
                        };
                        let projection = active_graph(
                            &owner,
                            scene_id.as_deref(),
                            window_id,
                            &startup_deferred,
                        )?;
                        send.send_blocking(with_graph_projection(
                            wiring_result(
                                if provider == "antigravity" {
                                    "antigravitySessionLoadResult"
                                } else {
                                    "dshSessionLoadResult"
                                },
                                request_id,
                                result,
                            ),
                            projection,
                        ))?;
                    }
                    if let Err(error) = attempt {
                        catalog_load_errors.push(catalog_load_error(
                            &session_id,
                            &provider_session_id,
                            &error,
                        ));
                    }
                    continue;
                }
                Command::RetryProviderRestore { ref session_id }
                | Command::StartFreshProviderSession { ref session_id } => {
                    let Some(stored) = owner
                        .session(session_id)?
                        .filter(|session| session.archived_at.is_none())
                    else {
                        continue;
                    };
                    let codex = stored.kind == "codex";
                    let antigravity = stored.kind == "antigravity";
                    let fresh = matches!(command, Command::StartFreshProviderSession { .. });
                    let change = if fresh {
                        ClaudeRestore::Fresh
                    } else {
                        ClaudeRestore::Retry
                    };
                    let blocked = !foreground_sessions.contains(session_id)
                        || environment_changes.contains_key(session_id)
                        || permission_changes.contains_key(session_id)
                        || shell_promotions.contains_key(session_id)
                        || catalog_loads.contains_key(session_id)
                        || pending_launch.contains(session_id)
                        || ((codex || antigravity) && recovering.contains_key(session_id))
                        || launch_retries.contains_key(session_id)
                        || (!owner.can_launch_provider(session_id)
                            && !((codex || antigravity)
                                && owner.shell_worker_is_retiring(session_id)));
                    if codex
                        && !fresh
                        && !blocked
                        && let Some(binding) = owner
                            .codex_binding(session_id)?
                            .filter(|binding| binding.restore_state == "failed")
                    {
                        let id = &binding.provider_session_id;
                        if let Err(error) = owner
                            .ensure_codex_catalog_available(session_id, id)
                            .and_then(|_| {
                                ensure_codex_catalog_not_pending(&catalog_loads, session_id, id)
                            })
                        {
                            provider_identity_events.push(json!({
                                "result": "rejected", "provider": "codex", "sessionId": session_id,
                                "reason": "catalog-identity-occupied", "error": error.to_string(),
                            }));
                            continue;
                        }
                    }
                    let accepted = if blocked {
                        false
                    } else if codex {
                        owner.change_codex_restore(session_id, change)?
                    } else if antigravity {
                        owner.change_antigravity_restore(session_id, change)?
                    } else if stored.kind == "dsh" {
                        owner.change_dsh_restore(session_id, change)?
                    } else {
                        stored.kind == "claude-code"
                            && owner.change_claude_restore(session_id, change)?
                    };
                    if !accepted {
                        if codex {
                            provider_identity_events.push(json!({
                                "result": "rejected", "provider": "codex", "sessionId": session_id,
                                "reason": "restore-action-not-available", "error": codex_resume::action_error(fresh),
                            }));
                        }
                        continue;
                    }
                    let Some(workspace_id) = owner.session_workspace(session_id)? else {
                        continue;
                    };
                    failed_launches.remove(session_id);
                    sessions.remove(session_id);
                    dirty.remove(session_id);
                    sequence.remove(session_id);
                    scroll_requests.remove(session_id);
                    views.remove(session_id);
                    awaiting_ui.remove(session_id);
                    pending_input.retain(|input| input.session_id != *session_id);
                    operations.retain(|(id, _, _)| id != session_id);
                    searches.retain(|(id, _, _)| id != session_id);
                    copies.retain(|(_, id, _, _)| id != session_id);
                    if codex || antigravity {
                        // Retire immediately, then let the shared queue wait for
                        // the old worker's exit before it launches a new run.
                        owner.retire_sessions(std::slice::from_ref(session_id));
                        send.send_blocking(Event::Recovery {
                            session_id: session_id.clone(),
                            state: RecoveryState::Restoring,
                        })?;
                    }
                    checks.request(&owner, &workspace_id)?;
                    launch_retries.insert(
                        session_id.clone(),
                        LaunchRetry {
                            workspace_id,
                            recovering: matches!(change, ClaudeRestore::Retry),
                            validated: false,
                            fresh_codex: codex && fresh,
                        },
                    );
                    continue;
                }
                Command::RestoreSessionEnvironment { .. }
                | Command::LocateSessionEnvironment { .. }
                | Command::HandoffSessionEnvironment { .. } => {
                    let request = EnvironmentChangeRequest::from_command(&command).unwrap();
                    let id = &request.session_id;
                    if environment_changes.contains_key(id)
                        || pending_launch.contains(id)
                        || recovering.contains_key(id)
                        || launch_retries.contains_key(id)
                        || owner.shell_worker_is_retiring(id)
                        || permission_changes.contains_key(id)
                        || shell_promotions.contains_key(id)
                        || shell_submissions.contains_key(id)
                        || catalog_loads.contains_key(id)
                        || provider_starting.contains_key(id)
                    {
                        send.send_blocking(request.reply(EnvironmentChangeResult::rejected(
                            "environment-transition-in-progress",
                        )))?;
                        continue;
                    }
                    let session_id = id.clone();
                    match request.begin(&mut owner) {
                        Ok(Ok(change)) => {
                            environment_changes.insert(session_id, change);
                            graph_dirty = true;
                        }
                        Ok(Err(result)) => send.send_blocking(request.reply(result))?,
                        Err(error) => {
                            send.send_blocking(request.reply(EnvironmentChangeResult::rejected(
                                environment_runtime::change_failure(request.kind, &error),
                            )))?
                        }
                    }
                    continue;
                }
                Command::RetryRecovery { ref session_id }
                | Command::RestartSession { ref session_id } => {
                    let rebuilding = matches!(command, Command::RetryRecovery { .. });
                    let codex = owner
                        .session(session_id)?
                        .is_some_and(|session| session.kind == "codex");
                    if environment_changes.contains_key(session_id)
                        || catalog_loads.contains_key(session_id)
                        || permission_changes.contains_key(session_id)
                        || shell_promotions.contains_key(session_id)
                        || launch_retries.contains_key(session_id)
                        || owner
                            .claude_binding(session_id)?
                            .is_some_and(|binding| binding.restore_state == "failed")
                    {
                        continue;
                    }
                    let parked = rebuilding && startup_deferred.contains(session_id);
                    if parked && !foreground_sessions.contains(session_id) {
                        continue;
                    }
                    let failed = failed_launches.get(session_id);
                    if codex
                        && (owner
                            .codex_binding(session_id)?
                            .is_some_and(|binding| binding.restore_state == "failed")
                            || (!parked && matches!(failed, Some(RecoveryState::Failed(_)))))
                    {
                        continue;
                    }
                    let fresh_codex = codex && owner.pending_codex_fresh(session_id)?;
                    let state = if rebuilding {
                        if !parked && !matches!(failed, Some(RecoveryState::Failed(_))) {
                            continue;
                        }
                        RecoveryState::Restoring
                    } else {
                        let reason = match failed {
                            Some(RecoveryState::StartFailed(reason)) => Some(reason),
                            Some(_) => continue,
                            None => None,
                        };
                        if !foreground_sessions.contains(session_id)
                            || pending_launch.contains(session_id)
                            || recovering.contains_key(session_id)
                            || !owner.can_restart_session(session_id, reason.is_some())?
                        {
                            continue;
                        }
                        RecoveryState::Restarting(reason.cloned().unwrap_or_default())
                    };
                    let Some(workspace_id) = owner.session_workspace(session_id)? else {
                        continue;
                    };
                    startup_deferred.remove(session_id);
                    send.send_blocking(Event::Recovery {
                        session_id: session_id.clone(),
                        state,
                    })?;
                    match checks.request(&owner, &workspace_id) {
                        Ok(()) => {
                            if !rebuilding {
                                shell_submissions.remove(session_id);
                                deferred_recovery.remove(session_id);
                                sessions.remove(session_id);
                                dirty.remove(session_id);
                                sequence.remove(session_id);
                                scroll_requests.remove(session_id);
                                views.remove(session_id);
                                awaiting_ui.remove(session_id);
                                pending_input.retain(|input| input.session_id != *session_id);
                                operations.retain(|(id, _, _)| id != session_id);
                                searches.retain(|(id, _, _)| id != session_id);
                                copies.retain(|(_, id, _, _)| id != session_id);
                            }
                            launch_retries.insert(
                                session_id.clone(),
                                LaunchRetry {
                                    workspace_id,
                                    validated: false,
                                    recovering: rebuilding,
                                    fresh_codex,
                                },
                            );
                        }
                        Err(error) => {
                            claude_launch_targets.remove(session_id);
                            let failure = report_launch_failure(
                                send,
                                session_id,
                                rebuilding,
                                format!("{error:#}"),
                            )?;
                            failed_launches.insert(session_id.clone(), failure);
                        }
                    }
                    continue;
                }
                Command::RetryStorage { session_id } => {
                    if !sessions.contains(&session_id) {
                        continue;
                    }
                    (session_id, WorkerCommand::RetryStorage { reply }, true)
                }
                Command::PutGeometry { sequence, update } => {
                    let error = owner
                        .put_geometry(&update)
                        .err()
                        .map(|error| format!("{error:#}"));
                    send.send_blocking(Event::GeometryResult {
                        sequence,
                        update,
                        error,
                    })?;
                    continue;
                }
                Command::PreviewSceneClose { scene_id } => {
                    let event = match owner.scene_close_preview(window_id, &scene_id) {
                        Ok((graph, is_last)) => Event::SceneClosePreview {
                            scene_id,
                            graph,
                            is_last,
                        },
                        Err(error) => Event::NavigationFailed {
                            message: format!("{error:#}"),
                            attempted_focus: None,
                        },
                    };
                    if navigation_publications.is_empty() {
                        send.send_blocking(event)?;
                    } else {
                        navigation_publications.push_back((Vec::new(), false, event));
                    }
                    continue;
                }
                Command::Refresh => {
                    dirty.extend(sessions.iter().cloned());
                    awaiting_ui.clear();
                    let graph =
                        active_graph(&owner, scene_id.as_deref(), window_id, &startup_deferred)?;
                    pending_launch = unstarted_sessions(&owner, &graph, &sessions)?;
                    pending_launch.retain(|id| {
                        !startup_deferred.contains(id)
                            && !failed_launches.contains_key(id)
                            && !launch_retries.contains_key(id)
                            && !catalog_loads.contains_key(id)
                            && !permission_changes.contains_key(id)
                            && !shell_promotions.contains_key(id)
                    });
                    foreground_sessions = launchable_ids(&graph).map(str::to_owned).collect();
                    if let Some(workspace_id) = &active_workspace {
                        checks.request(&owner, workspace_id)?;
                        launch_validated = false;
                    }
                    if navigation_publications.is_empty() {
                        send.send_blocking(Event::Graph(graph))?;
                    } else {
                        graph_dirty = true;
                    }
                    continue;
                }
                Command::CreateWorkspace { .. }
                | Command::SelectWorkspace { .. }
                | Command::RelinkWorkspace { .. }
                | Command::SetWorkspacePinned { .. }
                | Command::ReorderPinnedWorkspace { .. }
                | Command::RemoveWorkspace { .. }
                | Command::CreateTask { .. }
                | Command::SelectTask { .. }
                | Command::RenameTask { .. }
                | Command::SetTaskPinned { .. }
                | Command::ReorderPinnedTask { .. }
                | Command::DeleteTask { .. }
                | Command::CreateCanvas { .. }
                | Command::CreateShellSibling { .. }
                | Command::ForkSession { .. }
                | Command::RetryFork { .. }
                | Command::SelectScene { .. }
                | Command::RenameScene { .. }
                | Command::ReorderScene { .. }
                | Command::CloseScene { .. }
                | Command::CloseSessionCard { .. }
                | Command::RemoveSessionBranch { .. }
                | Command::FocusSession { .. } => unreachable!("navigation handled above"),
                #[cfg(all(test, target_os = "macos"))]
                Command::TestPause { paused, resume } => {
                    paused.send(())?;
                    resume.recv_timeout(Duration::from_secs(8))?;
                    continue;
                }
                #[cfg(all(test, target_os = "macos"))]
                Command::TestPauseShellWorker {
                    session_id,
                    paused,
                    resume,
                } => {
                    owner.send(&session_id, WorkerCommand::TestPause { paused, resume })?;
                    continue;
                }
                Command::ListDshSessions { .. }
                | Command::GetDshSession { .. }
                | Command::SearchDshSession { .. }
                | Command::ListAntigravitySessions { .. }
                | Command::GetAntigravitySession { .. }
                | Command::SearchAntigravitySession { .. }
                | Command::GetEnvironmentDiagnostics { .. }
                | Command::CheckGitRepo { .. }
                | Command::GitStatusDetail { .. }
                | Command::GitCheckout { .. }
                | Command::GitCreateBranch { .. }
                | Command::GitCommit { .. }
                | Command::GitPush { .. }
                | Command::GitWorktreeCreate { .. }
                | Command::GitWorktreeRemove { .. }
                | Command::GetEgressStatus { .. }
                | Command::RecheckEgress { .. }
                | Command::SetEgressExemption { .. }
                | Command::SetEgressRuleMode { .. }
                | Command::SetEgressProtectionEnabled { .. }
                | Command::AcceptEgressBaseline { .. }
                | Command::GetBrowserDiagnostics { .. }
                | Command::EvaluateExternalLink { .. }
                | Command::GetUpdateStatus { .. }
                | Command::CheckForUpdate { .. }
                | Command::DownloadUpdate { .. }
                | Command::InstallUpdate { .. }
                | Command::RunRetentionCleanup { .. }
                | Command::GetAccountSummary { .. }
                | Command::GetClaudeAccounts { .. }
                | Command::ClaudeAccountCommand { .. }
                | Command::GetAgentHud { .. } => {
                    unreachable!("Runtime wiring command handled above")
                }
                #[cfg(all(test, target_os = "macos"))]
                Command::TestFreezeShellWrites {
                    session_id,
                    frozen,
                    reply,
                } => {
                    owner.send(
                        &session_id,
                        WorkerCommand::SetEgressFrozen { frozen, reply },
                    )?;
                    continue;
                }
                #[cfg(all(test, target_os = "macos"))]
                Command::TestFailPromotionClear { session_id } => {
                    owner.send(&session_id, WorkerCommand::TestFailPromotionClear)?;
                    continue;
                }
                Command::Shutdown => {
                    break 'running shutdown_requested.get() == Some(&true);
                }
            };
            let absolute_scroll = matches!(request, WorkerCommand::ScrollTo { .. });
            if let Err(error) = owner.send(&session_id, request) {
                send.send_blocking(Event::Diagnostic(format!("{error:#}")))?;
                continue;
            }
            // The view request enters the same worker queue, after the scroll.
            if absolute_scroll {
                dirty.insert(session_id.clone());
            }
            operations.push((session_id, result, refresh));
        }
        // Revalidate a selected card after the background read, after any queued
        // navigation/close commands have had a chance to change its authority.
        let stopped_loads: Vec<_> = catalog_loads
            .values_mut()
            .filter_map(|load| {
                let stopped = match load.stopping.as_ref()?.completion.try_recv() {
                    Ok(result) => result,
                    Err(mpsc::TryRecvError::Empty) => return None,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        Err(anyhow!("Shell worker did not report its retirement"))
                    }
                };
                Some((load.stopping.take().unwrap().selection, Some(stopped)))
            })
            .collect();
        for (reply, stopped) in stopped_loads
            .into_iter()
            .chain(catalog_replies.try_iter().map(|reply| (reply, None)))
        {
            let (cwd, target, result) = match reply {
                CatalogReply::Detail(result) => {
                    catalog_details.push(result);
                    continue;
                }
                CatalogReply::Recent(event) => {
                    send.send_blocking(event)?;
                    continue;
                }
                CatalogReply::Index { cwd, result } => {
                    let result = result.and_then(|mut entries| {
                        let ids: Vec<_> = entries
                            .iter()
                            .map(|entry| entry.provider_session_id.as_str())
                            .collect();
                        let mut owners = owner
                            .catalog_binding_owners("claude-code", &ids)
                            .map_err(|_| "Could not read provider session bindings")?;
                        for entry in &mut entries {
                            entry.loaded_by = owners.remove(&entry.provider_session_id);
                        }
                        Ok(entries)
                    });
                    catalog_lists.push(match result {
                        Ok(sessions) => json!({"cwd": cwd, "sessions": sessions}),
                        Err(error) => json!({"cwd": cwd, "error": error}),
                    });
                    continue;
                }
                CatalogReply::CodexList {
                    request_id,
                    cwd,
                    result,
                } => {
                    let result = result.and_then(|mut page| {
                        let ids: Vec<_> = page
                            .items
                            .iter()
                            .map(|entry| entry.provider_session_id.as_str())
                            .collect();
                        let mut owners = owner
                            .catalog_binding_owners("codex", &ids)
                            .map_err(|_| "无法读取 Codex 会话占用状态")?;
                        for (session_id, load) in &catalog_loads {
                            if load.provider == "codex"
                                && ids.contains(&load.provider_session_id.as_str())
                                && let Some(session) = owner
                                    .session(session_id)
                                    .map_err(|_| "无法读取 Codex 会话占用状态")?
                            {
                                owners
                                    .entry(load.provider_session_id.clone())
                                    .or_insert_with(
                                        || json!({"sessionId": session_id, "title": session.title}),
                                    );
                            }
                        }
                        for entry in &mut page.items {
                            entry.loaded_by = owners.remove(&entry.provider_session_id);
                        }
                        let mut result = json!(page.window);
                        result["sessions"] = json!(page.items);
                        Ok(result)
                    });
                    catalog_lists.push(codex_catalog_result(request_id, &cwd, result));
                    continue;
                }
                CatalogReply::Load {
                    cwd,
                    target,
                    result,
                } => (cwd, target, result),
            };
            let CatalogLoadTarget {
                provider,
                scene_id: requested_scene,
                session_id,
                provider_session_id,
            } = target;
            let Some(load) = catalog_loads.get_mut(&session_id).filter(|load| {
                load.provider == provider && load.provider_session_id == provider_session_id
            }) else {
                // The navigation command already rejected and disposed this load.
                continue;
            };
            let replaced = stopped.is_some();
            let attempt = (|| -> Result<Option<(String, i64)>> {
                if let Some(result) = stopped {
                    result?;
                }
                anyhow::ensure!(
                    scene_id.as_ref() == Some(&requested_scene)
                        && foreground_sessions.contains(&session_id),
                    "Card is no longer available in this scene"
                );
                anyhow::ensure!(
                    !pending_launch.contains(&session_id)
                        && !recovering.contains_key(&session_id)
                        && !launch_retries.contains_key(&session_id)
                        && !owner.waiting_for_provider_restore(&session_id),
                    "Card already has a running or starting process"
                );
                let entry = result.map_err(|error| anyhow!(error))?;
                if provider == "codex" {
                    owner.ensure_codex_catalog_available(&session_id, &provider_session_id)?;
                }
                if !owner.can_launch_provider(&session_id) {
                    anyhow::ensure!(!replaced, "Card already has a running or starting process");
                    anyhow::ensure!(
                        owner.session(&session_id)?.is_some_and(|session| {
                            session.archived_at.is_none() && session.cwd == cwd
                        }),
                        "Card is no longer available in this working directory"
                    );
                    let completion = owner.stop_for_provider_relaunch(&session_id)?;
                    load.stopping = Some(CatalogStop {
                        selection: CatalogReply::Load {
                            cwd,
                            target: CatalogLoadTarget {
                                provider,
                                scene_id: requested_scene,
                                session_id: session_id.clone(),
                                provider_session_id: provider_session_id.clone(),
                            },
                            result: Ok(entry),
                        },
                        completion,
                    });
                    return Ok(None);
                }
                let workspace_id = owner
                    .session_workspace(&session_id)?
                    .context("Card is no longer available")?;
                checks.request(&owner, &workspace_id)?;
                let revision = owner.load_provider_catalog_binding(
                    &ClaudeCatalogLoad {
                        binding_id: &id(),
                        session_id: &session_id,
                        scene_id: &requested_scene,
                        cwd: &cwd,
                        provider_session_id: &entry.provider_session_id,
                        title: &entry.title,
                        model: entry.model.as_deref(),
                        permission_mode: &entry.permission_mode,
                    },
                    provider,
                    entry.completed_turn,
                )?;
                Ok(Some((workspace_id, revision)))
            })();
            if !matches!(attempt, Ok(None)) && !load.antigravity_requests.is_empty() {
                let projection =
                    active_graph(&owner, scene_id.as_deref(), window_id, &startup_deferred)?;
                for request_id in std::mem::take(&mut load.antigravity_requests) {
                    let result = match &attempt {
                        Ok(_) => Ok(json!({
                            "sessionId": session_id, "providerSessionId": provider_session_id, "status": "restoring",
                        })),
                        Err(error) => Err(anyhow!("{error:#}")),
                    };
                    send.send_blocking(with_graph_projection(
                        wiring_result("antigravitySessionLoadResult", request_id, result),
                        projection.clone(),
                    ))?;
                }
            }
            match attempt {
                Ok(launch) => {
                    failed_launches.remove(&session_id);
                    deferred_recovery.remove(&session_id);
                    sessions.remove(&session_id);
                    dirty.remove(&session_id);
                    sequence.remove(&session_id);
                    scroll_requests.remove(&session_id);
                    views.remove(&session_id);
                    awaiting_ui.remove(&session_id);
                    pending_input.retain(|input| input.session_id != session_id);
                    operations.retain(|(id, _, _)| id != &session_id);
                    searches.retain(|(id, _, _)| id != &session_id);
                    copies.retain(|(_, id, _, _)| id != &session_id);
                    if let Some((workspace_id, revision)) = launch {
                        claude_launch_targets.remove(&session_id);
                        load.spawn_revision = Some(revision);
                        launch_retries.insert(
                            session_id,
                            LaunchRetry {
                                workspace_id,
                                recovering: true,
                                validated: false,
                                fresh_codex: false,
                            },
                        );
                    }
                }
                Err(error) => {
                    if replaced {
                        claude_launch_targets.remove(&session_id);
                        let failure =
                            report_launch_failure(send, &session_id, true, error.to_string())?;
                        failed_launches.insert(session_id.clone(), failure);
                    }
                    catalog_loads.remove(&session_id);
                    catalog_load_errors.push(catalog_load_error(
                        &session_id,
                        &provider_session_id,
                        &error,
                    ));
                }
            }
        }
        for event in owner.poll(Duration::from_millis(4))? {
            match event {
                OwnerEvent::Domain(mut commit) => {
                    graph_dirty = true;
                    // The persisted exit arrives even when terminal delivery is paused.
                    // Keep the target during intentional retirement for permission changes.
                    if commit.result["profile"] == "claude-code"
                        && commit.result["status"] == "exited"
                        && (commit.result["exitCode"]
                            .as_i64()
                            .is_some_and(|code| code != 0)
                            || commit.result["signal"]
                                .as_i64()
                                .is_some_and(|signal| signal != 0))
                        && let Some(id) = commit.result["sessionId"].as_str()
                        && claude_launch_targets.contains_key(id)
                        && !permission_changes.contains_key(id)
                        && !catalog_loads.contains_key(id)
                        && owner
                            .claude_binding(id)?
                            .is_none_or(|binding| !binding.resumable)
                    {
                        claude_launch_targets.remove(id);
                    }
                    if !commit.replayed
                        && let Some(notification) = commit
                            .result
                            .as_object_mut()
                            .and_then(|result| result.remove("agentNotification"))
                    {
                        agent_notifications.push(notification);
                    }
                }
                OwnerEvent::ProviderIdentity(result) => {
                    if let Some(change) = result["sessionId"]
                        .as_str()
                        .and_then(|id| permission_changes.get_mut(id))
                    {
                        change.observe_identity(&result);
                    }
                    if matches!(
                        result["result"].as_str(),
                        Some("confirmed" | "restore-failed")
                    ) && let Some(session_id) = result["sessionId"].as_str()
                        && catalog_loads.get(session_id).is_some_and(|load| {
                            load.spawn_revision.is_some()
                                && load.spawn_revision == result["spawnRevision"].as_i64()
                                && result["providerSessionId"] == load.provider_session_id
                                && result["provider"] == load.provider
                        })
                    {
                        catalog_loads.remove(session_id);
                    }
                    if matches!(
                        result["result"].as_str(),
                        Some("restore-failed" | "fork-failed")
                    ) && let (Some(id), Some(error)) = (
                        result["sessionId"].as_str(),
                        result["restoreError"]
                            .as_str()
                            .or_else(|| result["forkError"].as_str()),
                    ) {
                        claude_launch_targets.remove(id);
                        pending_input.retain(|input| input.session_id != id);
                        failed_launches
                            .insert(id.to_owned(), RecoveryState::Failed(error.to_owned()));
                        if result.get("runId").is_some() {
                            sessions.remove(id);
                            dirty.remove(id);
                            // A retry starts with a fresh PTY output credit window.
                            sequence.remove(id);
                            views.remove(id);
                            awaiting_ui.remove(id);
                            if !environment_changes.contains_key(id) {
                                send.send_blocking(Event::Recovery {
                                    session_id: id.to_owned(),
                                    state: RecoveryState::Failed(error.to_owned()),
                                })?;
                            }
                        }
                    }
                    provider_identity_events.push(result);
                }
                OwnerEvent::Diagnostic { message, .. } => {
                    send.send_blocking(Event::Diagnostic(message))?
                }
                OwnerEvent::Terminal(event) => {
                    let session_id = event.session_id;
                    if !sessions.contains(&session_id) {
                        continue;
                    }
                    match event.event {
                        SessionEvent::Output { sequence: next, .. } => {
                            sequence.insert(session_id.clone(), next);
                            dirty.insert(session_id);
                        }
                        // The display credit window accounts for output only;
                        // start/resize/exit records are not acknowledged as bytes.
                        SessionEvent::Started { .. }
                        | SessionEvent::Resized { .. }
                        | SessionEvent::Exited { .. } => {
                            dirty.insert(session_id);
                        }
                        SessionEvent::SearchUpdated { result } => {
                            dirty.insert(session_id.clone());
                            send.send_blocking(Event::SearchResults {
                                session_id,
                                sequence: result.sequence,
                                result: Ok(result),
                            })?;
                        }
                        SessionEvent::ClipboardWrite { text } => {
                            send.send_blocking(Event::ClipboardWrite { session_id, text })?;
                        }
                        SessionEvent::ScreenInvalidated => {
                            dirty.insert(session_id);
                        }
                        SessionEvent::StorageFault { message, .. } => {
                            send.send_blocking(Event::StorageFault {
                                session_id,
                                message,
                            })?
                        }
                        SessionEvent::StorageRecovered { .. } => {
                            dirty.insert(session_id.clone());
                            send.send_blocking(Event::StorageRecovered { session_id })?;
                        }
                        SessionEvent::Diagnostic { message } => {
                            send.send_blocking(Event::Diagnostic(message))?
                        }
                        _ => {}
                    }
                }
            }
        }
        let mut finished_promotions = Vec::new();
        for (id, promotion) in &mut shell_promotions {
            match promotion.advance(
                &mut owner,
                &options,
                id,
                &terminal_layouts,
                &role_requests,
                &mut claude_launch_targets,
            ) {
                Ok(ShellPromotionProgress::Pending) => continue,
                Ok(ShellPromotionProgress::Attached) => {
                    sessions.insert(id.clone());
                    dirty.insert(id.clone());
                    graph_dirty = true;
                    continue;
                }
                Ok(ShellPromotionProgress::Ready) => {
                    sessions.insert(id.clone());
                    dirty.insert(id.clone());
                    pending_input.append(&mut promotion.inputs);
                    retry_pending_input = true;
                    finished_promotions.push(id.clone());
                    graph_dirty = true;
                    continue;
                }
                Ok(ShellPromotionProgress::Detached) => {}
                Err(error) => {
                    eprintln!("[session.shell-promote-agent:{id}] {error:#}");
                    claude_launch_targets.remove(id);
                    let clearing = matches!(promotion.phase, ShellPromotionPhase::Clearing { .. });
                    if clearing
                        || matches!(
                            promotion.phase,
                            ShellPromotionPhase::RestoringShell(_)
                                | ShellPromotionPhase::RollingBack
                        )
                    {
                        owner.retire_sessions(std::slice::from_ref(id));
                        let message = format!("Shell promotion for {id}: {error:#}");
                        let failure = if clearing {
                            let rebuilding = owner.has_shell_run(id)?;
                            report_session_launch_failure(
                                &mut owner, send, id, rebuilding, false, message,
                            )?
                        } else {
                            report_launch_failure(send, id, true, message)?
                        };
                        failed_launches.insert(id.clone(), failure);
                        pending_input.retain(|input| &input.session_id != id);
                        finished_promotions.push(id.clone());
                    } else {
                        if promotion.launch.profile == LaunchProfile::Codex {
                            send.send_blocking(Event::Diagnostic(codex_launch::failure_message(
                                &error,
                            )))?;
                        }
                        owner.retire_sessions(std::slice::from_ref(id));
                        promotion.phase = ShellPromotionPhase::RollingBack;
                    }
                }
            }
            sessions.remove(id);
            dirty.remove(id);
            sequence.remove(id);
            scroll_requests.remove(id);
            views.remove(id);
            awaiting_ui.remove(id);
            operations.retain(|(session_id, _, _)| session_id != id);
            searches.retain(|(session_id, _, _)| session_id != id);
            graph_dirty = true;
        }
        for id in finished_promotions {
            shell_promotions.remove(&id);
        }
        let mut completed_changes = Vec::new();
        for (id, change) in &mut permission_changes {
            let terminal = terminal_layouts
                .get(id)
                .copied()
                .unwrap_or_else(default_terminal_options);
            match change.advance(
                &mut owner,
                &options,
                id,
                terminal,
                &role_requests,
                &mut claude_launch_targets,
            ) {
                Ok(PermissionProgress::Pending) => {}
                Ok(PermissionProgress::Launched) => {
                    sessions.insert(id.clone());
                    dirty.insert(id.clone());
                    graph_dirty = true;
                }
                Ok(PermissionProgress::ConsentPrompt) => {
                    // The terminal already knows how to accept keystrokes in Ready.
                    // The host continues to gate which bytes reach the replacement.
                    send.send_blocking(Event::Recovery {
                        session_id: id.clone(),
                        state: RecoveryState::Ready,
                    })?;
                }
                result => completed_changes.push((id.clone(), result)),
            }
        }
        for (id, result) in completed_changes {
            let change = permission_changes
                .remove(&id)
                .expect("completed permission respawn");
            let error = match result {
                Ok(PermissionProgress::Confirmed) => {
                    send.send_blocking(Event::Recovery {
                        session_id: id.clone(),
                        state: RecoveryState::Ready,
                    })?;
                    None
                }
                Ok(PermissionProgress::RestoreFailed(error)) => {
                    if change.profile == LaunchProfile::Antigravity
                        && let Err(rollback) = change.rollback_if_pending(&mut owner, &id)
                    {
                        send.send_blocking(Event::Diagnostic(format!(
                            "rollback permission mode: {rollback:#}"
                        )))?;
                    }
                    Some(error)
                }
                Err(error) => {
                    claude_launch_targets.remove(&id);
                    if let Err(rollback) = change.rollback_if_pending(&mut owner, &id) {
                        send.send_blocking(Event::Diagnostic(format!(
                            "rollback permission mode: {rollback:#}"
                        )))?;
                    }
                    owner.retire_sessions(std::slice::from_ref(&id));
                    sessions.remove(&id);
                    dirty.remove(&id);
                    sequence.remove(&id);
                    views.remove(&id);
                    awaiting_ui.remove(&id);
                    let message = format!("{error:#}");
                    let failure = if change.profile == LaunchProfile::Codex {
                        report_session_launch_failure(
                            &mut owner,
                            send,
                            &id,
                            true,
                            false,
                            message.clone(),
                        )?
                    } else {
                        report_launch_failure(send, &id, false, message.clone())?
                    };
                    failed_launches.insert(id.clone(), failure);
                    Some(message)
                }
                _ => unreachable!("permission respawn still in progress"),
            };
            permission_results.push(permission_result(&id, &change.mode, error));
            graph_dirty = true;
        }
        let released: Vec<_> = provider_starting
            .keys()
            .filter(|id| {
                if let Some(change) = environment_changes.get(*id)
                    && let EnvironmentPhase::Confirming { started_at, .. } = change.phase
                {
                    if !owner.environment_worker_active(id) || failed_launches.contains_key(*id) {
                        return true;
                    }
                    if started_at.elapsed() < *PERMISSION_STARTUP_WINDOW {
                        return false;
                    }
                    if owner.waiting_for_permission_consent(id)
                        || started_at.elapsed() >= crate::provider_hook::IDENTITY_TIMEOUT
                    {
                        return true;
                    }
                }
                !owner.waiting_for_provider_restore(id)
            })
            .cloned()
            .collect();
        for session_id in released {
            provider_starting.remove(&session_id);
            if let Some(change) = environment_changes.get_mut(&session_id)
                && let EnvironmentPhase::Confirming { failure, .. } = &mut change.phase
            {
                let original_failure = failure.take();
                let startup_error = failed_launches
                    .get(&session_id)
                    .and_then(|failure| match failure {
                        RecoveryState::Failed(error) | RecoveryState::StartFailed(error) => {
                            Some(error.clone())
                        }
                        _ => None,
                    })
                    .or_else(|| {
                        (!owner.environment_worker_active(&session_id)).then(|| {
                            "process exited before confirming the restored session".to_owned()
                        })
                    });
                let startup_error = startup_error.or_else(|| {
                    (owner.waiting_for_provider_restore(&session_id)
                        && !owner.waiting_for_permission_consent(&session_id))
                    .then(|| "provider identity confirmation timed out".to_owned())
                });
                let result = if let Some(error) = startup_error {
                    owner.retire_sessions(std::slice::from_ref(&session_id));
                    sessions.remove(&session_id);
                    dirty.remove(&session_id);
                    sequence.remove(&session_id);
                    views.remove(&session_id);
                    awaiting_ui.remove(&session_id);
                    scroll_requests.remove(&session_id);
                    pending_input.retain(|input| input.session_id != session_id);
                    graph_dirty = true;
                    if let Some(reason) = original_failure {
                        let failure = report_launch_failure(send, &session_id, true, error)?;
                        failed_launches.insert(session_id.clone(), failure);
                        EnvironmentChangeResult::rejected(reason)
                    } else {
                        let reason = change.rollback(&mut owner, true, &anyhow!(error))?;
                        if change.operation.previous_state == "ready" {
                            change.phase = EnvironmentPhase::Launching {
                                failure: Some(reason),
                            };
                            continue;
                        }
                        EnvironmentChangeResult::rejected(reason)
                    }
                } else {
                    change.launch_result(&owner, original_failure)?
                };
                send.send_blocking(change.request.reply(result))?;
                environment_changes.remove(&session_id);
                graph_dirty = true;
            }
            if !failed_launches.contains_key(&session_id) && sessions.contains(&session_id) {
                send.send_blocking(Event::Recovery {
                    session_id,
                    state: RecoveryState::Ready,
                })?;
            }
        }
        while let Some(reply) = history.try_recv()? {
            send.send_blocking(Event::History(reply))?;
        }
        let role_graph_replies = role_replies
            .try_iter()
            .map(|reply| ("claudeRoleAssignmentsResult", serde_json::to_value(reply)));
        let provider_graph_replies = provider_settings
            .replies
            .try_iter()
            .map(|reply| ("providerSettingsResult", serde_json::to_value(reply)));
        let model_catalog_graph_replies = model_catalog_replies
            .try_iter()
            .map(|reply| ("claudeModelCatalogResult", serde_json::to_value(reply)));
        for (field, reply) in role_graph_replies
            .chain(provider_graph_replies)
            .chain(model_catalog_graph_replies)
        {
            let mut graph =
                active_graph(&owner, scene_id.as_deref(), window_id, &startup_deferred)?;
            graph[field] = reply?;
            let event = Event::Graph(graph);
            if navigation_publications.is_empty() {
                send.send_blocking(event)?;
            } else {
                navigation_publications.push_back((Vec::new(), false, event));
            }
        }
        searches.retain(|(session_id, sequence, receive)| {
            let result = match receive.try_recv() {
                Ok(result) => result.map_err(|error| format!("{error:#}")),
                Err(mpsc::TryRecvError::Empty) => return true,
                Err(mpsc::TryRecvError::Disconnected) => Err("Shell search disconnected".into()),
            };
            dirty.insert(session_id.clone());
            let _ = send.send_blocking(Event::SearchResults {
                session_id: session_id.clone(),
                sequence: *sequence,
                result,
            });
            false
        });
        copies.retain(
            |(request_id, session_id, _, receive)| match receive.try_recv() {
                Ok(Ok(SelectionCopy { text, interrupted })) => {
                    if interrupted && owner.provider_allows_derivation(session_id) {
                        match owner.record_interaction(session_id, "control", true) {
                            Ok(_) => {
                                graph_dirty = true;
                                tasks_dirty = true;
                            }
                            Err(error) => {
                                let _ = send.send_blocking(Event::Diagnostic(format!("{error:#}")));
                            }
                        }
                    }
                    if sessions.contains(session_id) {
                        dirty.insert(session_id.clone());
                    }
                    let _ = send.send_blocking(Event::Copied {
                        request_id: *request_id,
                        text,
                    });
                    false
                }
                Ok(Err(error)) => {
                    let _ = send.send_blocking(Event::Diagnostic(format!("{error:#}")));
                    false
                }
                Err(mpsc::TryRecvError::Empty) => true,
                Err(mpsc::TryRecvError::Disconnected) => {
                    let _ = send.send_blocking(Event::Diagnostic("Shell copy disconnected".into()));
                    false
                }
            },
        );
        operations.retain(|(session_id, result, refresh)| match result.try_recv() {
            Ok(Ok(())) => {
                if *refresh {
                    dirty.insert(session_id.clone());
                }
                false
            }
            Ok(Err(error)) => {
                let _ = send.send_blocking(Event::Diagnostic(format!("{error:#}")));
                false
            }
            Err(mpsc::TryRecvError::Empty) => true,
            Err(mpsc::TryRecvError::Disconnected) => {
                let _ = send.send_blocking(Event::Diagnostic("Shell request disconnected".into()));
                false
            }
        });
        // Keep processing other Shell traffic while the existing owner loop
        // drains SavedExit and joins each retired worker. Publish in command order.
        while navigation_publications
            .front()
            .is_some_and(|(disposed, wait, _)| {
                !wait || disposed.iter().all(|id| !owner.has_shell_worker(id))
            })
        {
            let (disposed, _, event) = navigation_publications
                .pop_front()
                .expect("navigation result");
            if !disposed.is_empty() {
                send.send_blocking(Event::ScenesDisposed(disposed))?;
            }
            if matches!(event, Event::Navigation { .. }) {
                send_sidebar(&owner, window_id, send)?;
            }
            send.send_blocking(event)?;
        }
        if shutdown_requested.get().is_none()
            && navigation_publications.is_empty()
            && let Some(control) = &mut control
            && let Some(update) = control.poll(&mut owner, window_id)
        {
            // The owner has processed worker exits and older navigation publications.
            // Commit exactly one resolved path before resolving another control request.
            use crate::host_control::{ControlUpdate, NavigationPath};
            let (navigation, disposed) = match update {
                ControlUpdate::Navigation(navigation) => (Some(navigation), Vec::new()),
                ControlUpdate::Changed(disposed, launches) => {
                    for session in launches {
                        if let Some(workspace_id) = owner.session_workspace(&session)? {
                            checks.request(&owner, &workspace_id)?;
                            failed_launches.remove(&session);
                            launch_retries.insert(
                                session,
                                LaunchRetry {
                                    workspace_id,
                                    recovering: false,
                                    validated: false,
                                    fresh_codex: false,
                                },
                            );
                        }
                    }
                    (None, disposed)
                }
            };
            let refresh = |path: Option<&NavigationPath>| -> Result<()> {
                if let Some(path) = path {
                    owner.focus_session(
                        window_id,
                        &path.scene_id,
                        &path.session_id,
                        self::now(),
                        "会话不在当前画布中",
                    )?;
                }
                if !disposed.is_empty() {
                    for id in &disposed {
                        shell_submissions.remove(id);
                        shell_promotions.remove(id);
                        if let Some(load) = catalog_loads.remove(id) {
                            let error = anyhow!("Card was closed while loading the conversation");
                            if !load.antigravity_requests.is_empty() {
                                let projection = active_graph(
                                    &owner,
                                    scene_id.as_deref(),
                                    window_id,
                                    &startup_deferred,
                                )?;
                                for request_id in load.antigravity_requests {
                                    send.send_blocking(with_graph_projection(
                                        wiring_result(
                                            "antigravitySessionLoadResult",
                                            request_id,
                                            Err(anyhow!("{error:#}")),
                                        ),
                                        projection.clone(),
                                    ))?;
                                }
                            }
                            catalog_load_errors.push(catalog_load_error(
                                id,
                                &load.provider_session_id,
                                &error,
                            ));
                        }
                        if let Some(change) = permission_changes.remove(id) {
                            permission_results.push(permission_result(
                                id,
                                &change.mode,
                                Some("Card was closed during permission respawn".into()),
                            ));
                            if let Err(error) = change.rollback_if_pending(&mut owner, id) {
                                send.send_blocking(Event::Diagnostic(format!("{error:#}")))?;
                            }
                        }

                        sessions.remove(id);
                        dirty.remove(id);
                        sequence.remove(id);
                        scroll_requests.remove(id);
                        views.remove(id);
                        awaiting_ui.remove(id);
                        terminal_layouts.remove(id);
                        recovering.remove(id);
                        deferred_recovery.remove(id);
                        startup_deferred.remove(id);
                        failed_launches.remove(id);
                        launch_retries.remove(id);
                        launch_preparations.remove(id);
                        claude_launch_targets.remove(id);
                    }
                    pending_input.retain(|input| !disposed.contains(&input.session_id));
                    operations.retain(|(id, _, _)| sessions.contains(id));
                    searches.retain(|(id, _, _)| !disposed.contains(id));
                    copies.retain(|(_, id, _, _)| !disposed.contains(id));
                    send.send_blocking(Event::ScenesDisposed(disposed))?;
                }
                let hierarchy = owner.hierarchy(window_id)?;
                let next_workspace = hierarchy.workspace.as_ref().map(|item| item.id.clone());
                let next_scene = hierarchy.scene.as_ref().map(|item| item.id.clone());
                let target_changed = active_workspace != next_workspace || scene_id != next_scene;
                if active_workspace != next_workspace {
                    active_path_invalid =
                        workspace_is_invalid(&projected_paths, next_workspace.as_deref());
                }
                active_workspace = next_workspace;
                active_task = hierarchy.task.as_ref().map(|item| item.id.clone());
                scene_id = next_scene;
                let graph =
                    active_graph(&owner, scene_id.as_deref(), window_id, &startup_deferred)?;
                if !target_changed
                    && focused_session.as_deref() != graph["focusedSessionId"].as_str()
                    && let Some(pending) = focused_session
                        .as_ref()
                        .and_then(|id| shell_submissions.get_mut(id))
                {
                    pending.defer_ordering = false;
                }
                focused_session = graph["focusedSessionId"].as_str().map(str::to_owned);
                pending_launch = unstarted_sessions(&owner, &graph, &sessions)?;
                pending_launch.retain(|id| {
                    !startup_deferred.contains(id)
                        && !failed_launches.contains_key(id)
                        && !launch_retries.contains_key(id)
                        && !catalog_loads.contains_key(id)
                        && !permission_changes.contains_key(id)
                        && !shell_promotions.contains_key(id)
                });
                foreground_sessions = launchable_ids(&graph).map(str::to_owned).collect();
                if (target_changed || !pending_launch.is_empty())
                    && let Some(workspace_id) = &active_workspace
                {
                    checks.request(&owner, workspace_id)?;
                    launch_validated = false;
                    next_active_check = Instant::now() + ACTIVE_PATH_INTERVAL;
                }
                let scenes = active_task
                    .as_deref()
                    .map(|id| owner.scenes(id))
                    .transpose()?
                    .unwrap_or_default();
                send_sidebar(&owner, window_id, send)?;
                send.send_blocking(Event::Navigation {
                    hierarchy,
                    scenes,
                    graph: graph.clone(),
                    geometry: scene_id
                        .as_deref()
                        .map(|id| owner.geometry(id))
                        .transpose()?
                        .unwrap_or_default(),
                })?;
                send.send_blocking(Event::Graph(graph))?;
                dirty.extend(sessions.iter().cloned());
                Ok(())
            };
            if let Some(navigation) = navigation {
                navigation.complete(window_id, |path| refresh(Some(path)));
            } else {
                refresh(None)?;
            }
        }
        graph_dirty |= owner.poll_codex_hud();
        if owner.take_egress_membership_change() {
            let status = owner.egress().status();
            if status
                .session_families
                .values()
                .any(|families| families.contains(&Family::ChatgptSubscription))
                && let Some(live) = &mut wiring.live_egress
            {
                live.next_check = Instant::now();
            }
            send.send_blocking(wiring_result(
                "egressStatusChanged",
                0,
                Ok(egress_projection(status)),
            ))?;
        }
        if (graph_dirty
            || !provider_identity_events.is_empty()
            || !permission_results.is_empty()
            || !catalog_lists.is_empty()
            || !catalog_details.is_empty()
            || !catalog_load_errors.is_empty())
            && navigation_publications.is_empty()
        {
            let mut graph =
                active_graph(&owner, scene_id.as_deref(), window_id, &startup_deferred)?;
            if !provider_identity_events.is_empty() {
                graph["providerIdentityEvents"] =
                    json!(std::mem::take(&mut provider_identity_events));
            }
            if !agent_notifications.is_empty() {
                for notification in &mut agent_notifications {
                    if let Some(session_id) = notification["sessionId"].as_str() {
                        match owner.notification_scene_id(window_id, session_id) {
                            Ok(Some(scene_id)) => notification["sceneId"] = json!(scene_id),
                            Ok(None) => {}
                            Err(error) => eprintln!("Resolve notification scene: {error:#}"),
                        }
                    }
                }
                graph["agentNotifications"] =
                    Value::Array(std::mem::take(&mut agent_notifications));
            }
            if !permission_results.is_empty() {
                graph["providerPermissionModeResults"] =
                    json!(std::mem::take(&mut permission_results));
            }
            if !catalog_lists.is_empty() {
                graph["providerSessionCatalogResults"] = json!(std::mem::take(&mut catalog_lists));
            }
            if !catalog_details.is_empty() {
                graph["providerSessionDetailResults"] =
                    Value::Array(std::mem::take(&mut catalog_details));
            }
            if !catalog_load_errors.is_empty() {
                graph["providerSessionLoadErrors"] =
                    json!(std::mem::take(&mut catalog_load_errors));
            }
            send.send_blocking(Event::Graph(graph))?;
            graph_dirty = false;
        }
        if tasks_dirty && navigation_publications.is_empty() {
            send_sidebar(&owner, window_id, send)?;
            tasks_dirty = false;
        }
        views.retain(|session_id, pending| match pending.receive.try_recv() {
            Ok(Ok((screen, modes, selection))) => {
                awaiting_ui.insert(session_id.clone(), pending.sequence);
                let _ = send.send_blocking(Event::Frame {
                    session_id: session_id.clone(),
                    screen,
                    selection,
                    modes,
                    sequence: pending.sequence,
                    scroll_request_id: pending.scroll_request_id,
                });
                false
            }
            Ok(Err(error)) => {
                let _ = send.send_blocking(Event::Diagnostic(format!("{error:#}")));
                false
            }
            Err(mpsc::TryRecvError::Empty) => true,
            Err(mpsc::TryRecvError::Disconnected) => {
                let _ = send.send_blocking(Event::Diagnostic("Shell view disconnected".into()));
                false
            }
        });
        for session_id in dirty.clone() {
            if views.contains_key(&session_id) || awaiting_ui.contains_key(&session_id) {
                continue;
            }
            let (reply, receive) = mpsc::channel();
            owner.send(&session_id, WorkerCommand::SelectedView { reply })?;
            views.insert(
                session_id.clone(),
                PendingView {
                    receive,
                    sequence: sequence.get(&session_id).copied().unwrap_or(0),
                    scroll_request_id: scroll_requests.get(&session_id).copied().unwrap_or(0),
                },
            );
            dirty.remove(&session_id);
        }
    };
    drop(environment_changes);
    drop(wiring.live_egress.take());
    drop(control);
    shell_promotions.retain(|id, promotion| !promotion.started_at_shutdown(&mut owner, id));
    for (id, change) in permission_changes {
        if let Err(error) = change.rollback_if_pending(&mut owner, &id) {
            eprintln!("[permission-respawn:{id}] {error:#}");
        }
    }
    drop(model_catalog_requests);
    drop(model_catalog_replies);
    drop(role_requests);
    let role_worker = role_worker
        .join()
        .map_err(|_| anyhow!("Claude role assignment worker panicked"));
    if let Some(service) = &subscriptions {
        service.shutdown();
    }
    let provider_worker = provider_settings.shutdown();
    let mut sessions = owner.shutdown().context("stop native runtime sessions");
    if let Ok(worker) = model_catalog_worker {
        let _ = worker.join();
    }
    for (id, promotion) in shell_promotions {
        if !promotion.promoted {
            continue;
        }
        let restored = promotion.restore_kind(&mut owner, &id).and_then(|()| {
            // The old Shell was stopped for the handoff, not an explicit exit.
            owner.mark_shell_promotion_interrupted(&id)
        });
        if let Err(error) = restored {
            eprintln!("[session.shell-promote-rollback:{id}] {error:#}");
            sessions = sessions.and(Err(error));
        }
    }
    #[cfg(unix)]
    let clean_exit = clean_exit
        && !sessions
            .as_ref()
            .is_err_and(|error| error.is::<crate::session::PtyShutdownIncomplete>());
    let backup = clean_exit.then(|| owner.backup_for_clean_exit(&data_root));
    drop(owner);
    shutdown_result(
        sessions.map(|_| ()).and(role_worker).and(provider_worker),
        backup,
    )
}

fn shutdown_result(sessions: Result<()>, backup: Option<Result<()>>) -> Result<()> {
    match (sessions, backup) {
        (Err(sessions), Some(Err(backup))) => Err(sessions.context(format!(
            "session shutdown and clean exit backup both failed ({backup:#})"
        ))),
        (Err(sessions), Some(Ok(()))) => Err(sessions.context("clean exit backup completed")),
        (Ok(()), Some(backup)) => backup,
        (sessions, None) => sessions,
    }
}

fn workspace_path_error(state: &WorkspacePathState) -> String {
    format!("{WORKSPACE_PATH_INVALID} ({})", state.reason)
}

const CATALOG_BUSY: &str = "Claude Code catalog is busy or unavailable";
const CODEX_CATALOG_BUSY: &str = "Codex 会话目录暂时不可用";
// Process contract tests can allow more time for the provider stub to exit.
static PERMISSION_STARTUP_WINDOW: LazyLock<Duration> = LazyLock::new(|| {
    let millis = std::env::var("MATOU_PERMISSION_STARTUP_WINDOW_MS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(150);
    Duration::from_millis(millis)
});

// Preserve the Claude handoff even if its process stays quiet at launch.
const SHELL_PROMOTION_SILENT_STARTUP_WINDOW: Duration = Duration::from_secs(2);

struct ShellPromotion {
    launch: InteractiveLaunch,
    phase: ShellPromotionPhase,
    promoted: bool,
    inputs: VecDeque<PendingInput>,
}

enum ShellPromotionPhase {
    Clearing {
        result: Receiver<Result<bool>>,
        before: ShellInput,
    },
    Stopping(Receiver<Result<()>>),
    Preparing(Receiver<Result<PreparedLaunch>>),
    Starting {
        run_id: String,
        started_at: Instant,
    },
    Verifying {
        run_id: String,
    },
    RollingBack,
    RestoringShell(Receiver<Result<PreparedLaunch>>),
}

enum ShellPromotionProgress {
    Pending,
    Detached,
    Attached,
    Ready,
}

impl ShellPromotion {
    fn advance(
        &mut self,
        owner: &mut ShellOwner,
        options: &LaunchOptions,
        session_id: &str,
        terminal_layouts: &HashMap<String, TerminalOptions>,
        role_requests: &Sender<RoleAssignmentsRequest>,
        claude_launch_targets: &mut HashMap<String, Value>,
    ) -> Result<ShellPromotionProgress> {
        match &mut self.phase {
            ShellPromotionPhase::Clearing { result, before } => {
                let result = poll_promotion_result(result);
                if result.is_err() {
                    owner.restore_shell_input(session_id, std::mem::take(before));
                }
                let Some(cleared) = result? else {
                    return Ok(ShellPromotionProgress::Pending);
                };
                if !cleared {
                    return Ok(ShellPromotionProgress::Ready);
                }
                self.phase =
                    ShellPromotionPhase::Stopping(owner.stop_for_provider_relaunch(session_id)?);
                Ok(ShellPromotionProgress::Detached)
            }
            ShellPromotionPhase::Stopping(result) => {
                let Some(()) = poll_promotion_result(result)? else {
                    return Ok(ShellPromotionProgress::Pending);
                };
                owner.promote_shell_to_agent(session_id, self.launch.profile)?;
                self.promoted = true;
                self.phase = ShellPromotionPhase::Preparing(prepare_launch(
                    owner,
                    options,
                    session_id,
                    LaunchIntent::ShellPromotion(self.launch),
                    role_requests,
                )?);
                Ok(ShellPromotionProgress::Pending)
            }
            ShellPromotionPhase::Preparing(result) => {
                let Some(launch) = poll_promotion_result(result)? else {
                    return Ok(ShellPromotionProgress::Pending);
                };
                let terminal = terminal_layouts
                    .get(session_id)
                    .copied()
                    .unwrap_or_else(default_terminal_options);
                let run_id = match launch {
                    PreparedLaunch::ClaudeCode {
                        launch,
                        fork_source: None,
                    } => {
                        claude_launch_targets.insert(
                            session_id.to_owned(),
                            claude_target_metadata(&launch.launch),
                        );
                        owner.launch_promoted_claude(session_id, terminal, launch)?
                    }
                    PreparedLaunch::Antigravity(launch, permission) => owner.launch_antigravity(
                        session_id,
                        terminal,
                        &options.data_root,
                        launch,
                        permission,
                        true,
                    )?,
                    PreparedLaunch::Dsh(launch) => {
                        owner.launch_dsh(session_id, terminal, &options.data_root, launch, true)?
                    }
                    PreparedLaunch::Codex(launch, None) => {
                        owner.launch_codex(session_id, terminal, launch, None, true)?
                    }
                    _ => bail!("promotion requires a fresh provider launch"),
                };
                self.phase = ShellPromotionPhase::Starting {
                    run_id,
                    started_at: Instant::now(),
                };
                Ok(ShellPromotionProgress::Attached)
            }
            ShellPromotionPhase::Starting { run_id, started_at } => {
                let has_output = owner.check_provider_startup(session_id, run_id)?;
                let elapsed = started_at.elapsed();
                let ready = codex_launch::first_screen_ready(elapsed, has_output)
                    || (matches!(
                        self.launch.profile,
                        LaunchProfile::ClaudeCode | LaunchProfile::Dsh | LaunchProfile::Antigravity
                    ) && elapsed >= SHELL_PROMOTION_SILENT_STARTUP_WINDOW);
                if !ready {
                    return Ok(ShellPromotionProgress::Pending);
                }
                owner.begin_promotion_process_check(session_id)?;
                self.phase = ShellPromotionPhase::Verifying {
                    run_id: run_id.clone(),
                };
                Ok(ShellPromotionProgress::Pending)
            }
            ShellPromotionPhase::Verifying { run_id } => {
                if !owner.finish_promotion_startup(session_id, run_id)? {
                    return Ok(ShellPromotionProgress::Pending);
                }
                Ok(ShellPromotionProgress::Ready)
            }
            ShellPromotionPhase::RollingBack => {
                if owner.has_shell_worker(session_id) {
                    return Ok(ShellPromotionProgress::Pending);
                }
                self.restore_kind(owner, session_id)?;
                // A directory can disappear while the provider is starting. Use
                // the usual Shell directory selection, including its fallback.
                self.phase = ShellPromotionPhase::RestoringShell(prepare_launch(
                    owner,
                    options,
                    session_id,
                    LaunchIntent::Start,
                    role_requests,
                )?);
                Ok(ShellPromotionProgress::Pending)
            }
            ShellPromotionPhase::RestoringShell(result) => {
                let Some(launch) = poll_promotion_result(result)? else {
                    return Ok(ShellPromotionProgress::Pending);
                };
                let PreparedLaunch::Shell(directory) = launch else {
                    bail!("promotion rollback requires a Shell launch");
                };
                owner.set_shell_launch_directory(session_id, &directory)?;
                launch_shell(owner, options, session_id, directory, terminal_layouts)?;
                Ok(ShellPromotionProgress::Ready)
            }
        }
    }

    fn attached(&self) -> bool {
        matches!(
            self.phase,
            ShellPromotionPhase::Starting { .. } | ShellPromotionPhase::Verifying { .. }
        )
    }

    fn started_at_shutdown(&self, owner: &mut ShellOwner, session_id: &str) -> bool {
        matches!(&self.phase, ShellPromotionPhase::Verifying { run_id }
            if owner.keep_promotion_at_shutdown(session_id, run_id))
    }

    fn restore_kind(&self, owner: &mut ShellOwner, session_id: &str) -> Result<()> {
        // RestoringShell has already rolled the kind back, but the handoff
        // still happened and counts when shutting down that pending restart.
        if self.promoted && !matches!(self.phase, ShellPromotionPhase::RestoringShell(_)) {
            owner.return_promoted_agent_to_shell(session_id, self.launch.profile)?;
        }
        Ok(())
    }
}

fn poll_promotion_result<T>(result: &Receiver<Result<T>>) -> Result<Option<T>> {
    match result.try_recv() {
        Ok(result) => result.map(Some),
        Err(mpsc::TryRecvError::Empty) => Ok(None),
        Err(error) => Err(error).context("Shell promotion worker disconnected"),
    }
}

struct PermissionChange {
    profile: LaunchProfile,
    mode: String,
    binding: Option<ProviderBinding>,
    phase: PermissionPhase,
}

enum PermissionPhase {
    Stopping(Option<Receiver<Result<()>>>),
    Preparing(Option<Receiver<Result<PreparedLaunch>>>),
    AwaitingIdentity {
        run_id: String,
        startup_until: Option<Instant>,
        consent_shown: bool,
        outcome: Option<Result<(), String>>,
    },
}

enum PermissionProgress {
    Pending,
    Launched,
    ConsentPrompt,
    Confirmed,
    RestoreFailed(String),
}

impl PermissionChange {
    fn advance(
        &mut self,
        owner: &mut ShellOwner,
        options: &LaunchOptions,
        session_id: &str,
        terminal: TerminalOptions,
        role_requests: &Sender<RoleAssignmentsRequest>,
        claude_launch_targets: &mut HashMap<String, Value>,
    ) -> Result<PermissionProgress> {
        match &mut self.phase {
            PermissionPhase::Stopping(stopped) => {
                if let Some(stopped) = stopped {
                    match stopped.try_recv() {
                        Ok(result) => result?,
                        Err(mpsc::TryRecvError::Empty) => return Ok(PermissionProgress::Pending),
                        Err(error) => {
                            return Err(error).context("wait for the current AI Session to stop");
                        }
                    }
                }
                if let Some(binding) = &self.binding {
                    owner.write_provider_permission(
                        session_id,
                        &binding.id,
                        Some(&json!(self.mode)),
                    )?;
                } else {
                    owner.change_claude_restore(session_id, ClaudeRestore::PermissionFresh)?;
                }
                // Record that the metadata was written before scheduling fallible preparation.
                self.phase = PermissionPhase::Preparing(None);
                let fresh_permission = self.binding.is_none().then_some(self.mode.as_str());
                self.phase = PermissionPhase::Preparing(Some(prepare_launch(
                    owner,
                    options,
                    session_id,
                    LaunchIntent::PermissionRestart {
                        fresh_mode: fresh_permission,
                        previous_target: claude_launch_targets.get(session_id),
                    },
                    role_requests,
                )?));
                Ok(PermissionProgress::Pending)
            }
            PermissionPhase::Preparing(preparation) => {
                let preparation = preparation.as_ref().expect("started launch preparation");
                let launch = match preparation.try_recv() {
                    Ok(launch) => launch?,
                    Err(mpsc::TryRecvError::Empty) => return Ok(PermissionProgress::Pending),
                    Err(error) => {
                        return Err(error).context("prepare the AI Session permission respawn");
                    }
                };
                if let PreparedLaunch::Codex(plan, Some(mut resume)) = launch {
                    ensure!(
                        self.profile == LaunchProfile::Codex
                            && plan.permission_mode == self.mode
                            && self.binding.as_ref().is_some_and(|binding| {
                                resume.binding_id == binding.id
                                    && resume.provider_session_id == binding.provider_session_id
                            })
                            && owner.codex_binding(session_id)?.is_some_and(|binding| {
                                binding.id == resume.binding_id
                                    && binding.resumable
                                    && binding.restore_state == "none"
                            }),
                        "AI Session permission respawn lost its binding"
                    );
                    resume.permission_respawn = true;
                    let run_id =
                        owner.launch_codex(session_id, terminal, plan, Some(resume), false)?;
                    self.phase = PermissionPhase::AwaitingIdentity {
                        run_id,
                        startup_until: None,
                        consent_shown: false,
                        outcome: None,
                    };
                    return Ok(PermissionProgress::Launched);
                }
                if let PreparedLaunch::Antigravity(plan, permission) = launch {
                    ensure!(
                        self.profile == LaunchProfile::Antigravity
                            && permission.as_str() == self.mode
                            && matches!(&plan.mode, AntigravityMode::Resume(identity)
                                if self.binding.as_ref().is_some_and(|binding| binding.provider_session_id == *identity))
                            && owner.antigravity_binding(session_id)?.as_ref().is_some_and(
                                |current| self
                                    .binding
                                    .as_ref()
                                    .is_some_and(|binding| binding.id == current.id)
                            ),
                        "AI Session permission respawn lost its binding"
                    );
                    let run_id = owner.launch_antigravity_permission(
                        session_id,
                        terminal,
                        &options.data_root,
                        plan,
                        permission,
                    )?;
                    self.phase = PermissionPhase::AwaitingIdentity {
                        run_id,
                        startup_until: None,
                        consent_shown: false,
                        outcome: None,
                    };
                    return Ok(PermissionProgress::Launched);
                }
                let PreparedLaunch::ClaudeCode {
                    launch: plan,
                    fork_source: None,
                } = launch
                else {
                    bail!("AI Session permission respawn lost its binding");
                };
                let binding = if let Some(previous) = &self.binding {
                    Some(
                        owner
                            .claude_binding(session_id)?
                            .filter(|binding| {
                                binding.id == previous.id
                                    && binding.resumable
                                    && binding.provider_session_id == previous.provider_session_id
                                    && binding.metadata["permissionMode"] == self.mode
                                    && plan.launch.permission_mode == self.mode
                            })
                            .context("AI Session permission respawn lost its binding")?,
                    )
                } else {
                    None
                };
                claude_launch_targets
                    .insert(session_id.to_owned(), claude_target_metadata(&plan.launch));
                let run_id =
                    owner.launch_claude_permission(session_id, terminal, plan, binding.as_ref())?;
                self.phase = PermissionPhase::AwaitingIdentity {
                    run_id,
                    startup_until: Some(Instant::now() + *PERMISSION_STARTUP_WINDOW),
                    consent_shown: false,
                    outcome: None,
                };
                Ok(PermissionProgress::Launched)
            }
            PermissionPhase::AwaitingIdentity {
                run_id,
                startup_until,
                consent_shown,
                outcome,
            } => {
                if let Some(Err(error)) = outcome {
                    return Ok(PermissionProgress::RestoreFailed(std::mem::take(error)));
                }
                if self.profile != LaunchProfile::ClaudeCode {
                    // The owner settles the restore on the matching hook, exit or timeout.
                    return Ok(if outcome.is_some() {
                        PermissionProgress::Confirmed
                    } else {
                        PermissionProgress::Pending
                    });
                }
                if let Some(until) = *startup_until {
                    if Instant::now() < until {
                        return Ok(PermissionProgress::Pending);
                    }
                    owner
                        .finish_provider_startup(session_id, run_id)
                        .context("AI Session permission respawn failed")?;
                    *startup_until = None;
                } else if outcome.is_none() {
                    let running = owner
                        .check_provider_startup(session_id, run_id)
                        .context("AI Session permission respawn failed");
                    if *consent_shown {
                        running
                            .context("Claude Code exited during bypass permissions confirmation")?;
                    } else {
                        running?;
                    }
                }
                if outcome.is_some() {
                    return Ok(PermissionProgress::Confirmed);
                }
                if !*consent_shown && owner.waiting_for_permission_consent(session_id) {
                    *consent_shown = true;
                    return Ok(PermissionProgress::ConsentPrompt);
                }
                Ok(PermissionProgress::Pending)
            }
        }
    }

    fn observe_identity(&mut self, identity: &Value) {
        if let PermissionPhase::AwaitingIdentity {
            run_id, outcome, ..
        } = &mut self.phase
            && identity["runId"] == *run_id
            && self
                .binding
                .as_ref()
                .is_none_or(|binding| identity["bindingId"] == binding.id)
        {
            match identity["result"].as_str() {
                Some("confirmed") => *outcome = Some(Ok(())),
                Some("restore-failed") => {
                    *outcome = Some(Err(identity["restoreError"]
                        .as_str()
                        .unwrap_or("AI Session permission respawn failed")
                        .to_owned()));
                }
                _ => {}
            }
        }
    }

    fn rollback_if_pending(&self, owner: &mut ShellOwner, session_id: &str) -> Result<()> {
        let pending = matches!(
            self.phase,
            PermissionPhase::Preparing(_)
                | PermissionPhase::AwaitingIdentity {
                    outcome: None | Some(Ok(())),
                    ..
                }
        );
        let antigravity_failed = self.profile == LaunchProfile::Antigravity
            && matches!(
                self.phase,
                PermissionPhase::AwaitingIdentity {
                    outcome: Some(Err(_)),
                    ..
                }
            );
        if self.profile != LaunchProfile::Codex
            && (pending || antigravity_failed)
            && let Some(binding) = &self.binding
        {
            owner.write_provider_permission(
                session_id,
                &binding.id,
                binding.metadata.get("permissionMode"),
            )?;
        }
        Ok(())
    }
}

fn permission_result(session_id: &str, mode: &str, error: Option<String>) -> Value {
    let mut result = json!({ "sessionId": session_id, "permissionMode": mode });
    if let Some(error) = error {
        result["error"] = json!(error);
    }
    result
}

struct CatalogLoad {
    provider: &'static str,
    provider_session_id: String,
    spawn_revision: Option<i64>,
    stopping: Option<CatalogStop>,
    antigravity_requests: Vec<u64>,
}

fn ensure_codex_catalog_not_pending(
    loads: &HashMap<String, CatalogLoad>,
    session_id: &str,
    provider_session_id: &str,
) -> Result<()> {
    if let Some((owner, _)) = loads.iter().find(|(_, load)| {
        load.provider == "codex" && load.provider_session_id == provider_session_id
    }) {
        ensure!(owner != session_id, "正在载入当前卡片");
        bail!("Codex 会话正在载入另一张卡片");
    }
    Ok(())
}

struct CatalogStop {
    selection: CatalogReply,
    completion: Receiver<Result<()>>,
}

struct CatalogLoadTarget {
    provider: &'static str,
    scene_id: String,
    session_id: String,
    provider_session_id: String,
}

enum CatalogRequest {
    Index {
        cwd: String,
        load: Option<CatalogLoadTarget>,
    },
    Detail(CatalogDetailRequest),
    Recent(CatalogRecentRequest),
    CodexList {
        request_id: u64,
        cwd: String,
        options: codex_catalog::ListOptions,
    },
    CodexDetail {
        request_id: u64,
        cwd: String,
        provider_session_id: String,
        start: Option<i64>,
        around: Option<i64>,
        limit: Option<i64>,
    },
    CodexSearch {
        request_id: u64,
        cwd: String,
        provider_session_id: String,
        query: String,
        offset: Option<i64>,
        limit: Option<i64>,
    },
}

struct CatalogRecentRequest {
    session_id: String,
    cwd: String,
    provider_session_id: String,
}

impl CatalogRecentRequest {
    fn event(self, preview: Option<RecentReplies>) -> Event {
        Event::DeferredRecoveryPreview {
            session_id: self.session_id,
            cwd: self.cwd,
            preview,
        }
    }
}

struct CatalogDetailRequest {
    cwd: String,
    provider_session_id: String,
    start_event_index: Option<i64>,
    limit: Option<i64>,
}

impl CatalogDetailRequest {
    fn result(self, result: Result<provider_catalog::SessionDetail, &'static str>) -> Value {
        match result {
            Ok(detail) => json!({
                "cwd": self.cwd, "providerSessionId": self.provider_session_id,
                "events": detail.events, "page": detail.page,
            }),
            Err(error) => json!({
                "cwd": self.cwd, "providerSessionId": self.provider_session_id, "error": error,
            }),
        }
    }
}

struct CatalogSelection {
    provider_session_id: String,
    title: String,
    model: Option<String>,
    permission_mode: String,
    completed_turn: bool,
}

enum CatalogReply {
    Index {
        cwd: String,
        result: Result<Vec<provider_catalog::CatalogEntry>, &'static str>,
    },
    Load {
        cwd: String,
        target: CatalogLoadTarget,
        result: Result<CatalogSelection, &'static str>,
    },
    CodexList {
        request_id: u64,
        cwd: String,
        result: Result<codex_catalog::Page<codex_catalog::CatalogEntry>, &'static str>,
    },
    Detail(Value),
    Recent(Event),
}

fn codex_catalog_result(request_id: u64, cwd: &str, result: Result<Value, &'static str>) -> Value {
    let mut result = result.unwrap_or_else(|error| json!({"error": error}));
    result["provider"] = json!("codex");
    result["cwd"] = json!(cwd);
    result["requestId"] = json!(request_id);
    result
}

fn start_provider_catalog(
    dsh_root: Option<PathBuf>,
    codex_root: Option<PathBuf>,
    antigravity_root: PathBuf,
) -> Result<(mpsc::SyncSender<CatalogRequest>, Receiver<CatalogReply>)> {
    let (requests, incoming) = mpsc::sync_channel::<CatalogRequest>(16);
    let (outgoing, replies) = mpsc::channel();
    let root = provider_catalog::projects_root();
    thread::Builder::new()
        .name("provider-catalog".into())
        .spawn(move || {
            let mut codex = codex_catalog::Catalog::new(codex_root);
            let mut dsh = dsh_root.map(DshSessionCatalog::new);
            let mut antigravity = AntigravityCatalog::new(antigravity_root);
            for request in incoming {
                let reply = match request {
                    CatalogRequest::Index { cwd, load: None } => {
                        let result = provider_catalog::read(root.as_deref(), &cwd, None);
                        CatalogReply::Index { cwd, result }
                    }
                    CatalogRequest::Index {
                        cwd,
                        load: Some(target),
                    } => {
                        let id = target.provider_session_id.as_str();
                        let result = if target.provider == "codex" {
                            codex.load(&cwd, id).map(|entry| CatalogSelection {
                                provider_session_id: entry.provider_session_id,
                                title: entry.title,
                                model: entry.model,
                                permission_mode: entry.permission_mode.to_owned(),
                                completed_turn: entry.has_completed_turn,
                            })
                        } else if target.provider == "antigravity" {
                            antigravity.detail(Path::new(&cwd), id, antigravity_catalog::DetailOptions {
                                limit: Some(1), ..Default::default()
                            }).map(|detail| CatalogSelection {
                                provider_session_id: detail.summary.provider_session_id,
                                title: detail.summary.title,
                                model: None,
                                permission_mode: detail.summary.permission_mode.to_owned(),
                                completed_turn: false,
                            }).map_err(|_| "Antigravity session history is unavailable in this working directory")
                        } else if target.provider == "dsh" {
                            dsh.as_mut().ok_or("DSH session directory is unavailable").and_then(|catalog| {
                                catalog.detail(Path::new(&cwd), id, dsh_catalog::DetailOptions {limit: Some(1), ..Default::default()})
                                    .map_err(|_| "DSH session is unreadable or belongs to another working directory")
                                    .map(|detail| CatalogSelection {
                                        provider_session_id: detail.summary.provider_session_id,
                                        title: detail.summary.title,
                                        model: detail.summary.model,
                                        permission_mode: detail.summary.permission_mode.to_owned(),
                                        completed_turn: false,
                                    })
                            })
                        } else {
                            provider_catalog::read(root.as_deref(), &cwd, Some(id))
                                .and_then(|entries| {
                                    entries.into_iter().next().ok_or(
                                        "Provider session was not found in this working directory",
                                    )
                                })
                                .map(|entry| CatalogSelection {
                                    provider_session_id: entry.provider_session_id,
                                    title: entry.title,
                                    model: entry.model,
                                    permission_mode: entry.permission_mode,
                                    completed_turn: false,
                                })
                        };
                        CatalogReply::Load {
                            cwd,
                            target,
                            result,
                        }
                    }
                    CatalogRequest::CodexList {
                        request_id,
                        cwd,
                        options,
                    } => {
                        let result = codex.list(&cwd, &options);
                        CatalogReply::CodexList {
                            request_id,
                            cwd,
                            result,
                        }
                    }
                    CatalogRequest::CodexDetail {
                        request_id,
                        cwd,
                        provider_session_id,
                        start,
                        around,
                        limit,
                    } => {
                        let result = codex.detail(&cwd, &provider_session_id, start, around, limit);
                        let mut result = codex_catalog_result(request_id, &cwd, result);
                        result["providerSessionId"] = json!(provider_session_id);
                        CatalogReply::Detail(result)
                    }
                    CatalogRequest::CodexSearch {
                        request_id,
                        cwd,
                        provider_session_id,
                        query,
                        offset,
                        limit,
                    } => {
                        let result = codex
                            .search(&cwd, &provider_session_id, &query, offset, limit)
                            .map(|page| {
                                let mut result = json!(page.window);
                                result["hits"] = json!(page.items);
                                result
                            });
                        let mut result = codex_catalog_result(request_id, &cwd, result);
                        result["providerSessionId"] = json!(provider_session_id);
                        result["search"] = json!(true);
                        CatalogReply::Detail(result)
                    }
                    CatalogRequest::Recent(request) => {
                        let preview = provider_catalog::recent_replies(
                            root.as_deref(),
                            &request.cwd,
                            &request.provider_session_id,
                        )
                        .ok();
                        CatalogReply::Recent(request.event(preview))
                    }
                    CatalogRequest::Detail(request) => {
                        let result = provider_catalog::detail(
                            root.as_deref(),
                            &request.cwd,
                            &request.provider_session_id,
                            request.start_event_index,
                            request.limit,
                        );
                        CatalogReply::Detail(request.result(result))
                    }
                };
                if outgoing.send(reply).is_err() {
                    break;
                }
            }
        })?;
    Ok((requests, replies))
}

fn catalog_load_error(session_id: &str, provider_session_id: &str, error: &anyhow::Error) -> Value {
    json!({"sessionId": session_id, "providerSessionId": provider_session_id, "error": error.to_string()})
}

fn report_session_launch_failure(
    owner: &mut ShellOwner,
    send: &async_channel::Sender<Event>,
    session_id: &str,
    recovering: bool,
    fresh_codex: bool,
    message: String,
) -> Result<RecoveryState> {
    if owner.pending_codex_fork(session_id)?.is_some() {
        let reason = format!("Fork CLI could not start: {message}");
        owner.fail_claude_fork(session_id, &reason)?;
        return report_launch_failure(send, session_id, true, reason);
    }
    if let Some(binding) = owner
        .antigravity_binding(session_id)?
        .filter(|binding| binding.resumable)
        && owner
            .session(session_id)?
            .is_some_and(|session| session.kind == "antigravity")
    {
        owner.change_antigravity_restore(
            session_id,
            ClaudeRestore::Fail {
                binding_id: &binding.id,
                error: &message,
            },
        )?;
        return report_launch_failure(send, session_id, true, message);
    }
    if let Some(binding) = owner
        .dsh_binding(session_id)?
        .filter(|binding| binding.resumable)
    {
        owner.change_dsh_restore(
            session_id,
            ClaudeRestore::Fail {
                binding_id: &binding.id,
                error: &message,
            },
        )?;
        return report_launch_failure(send, session_id, true, message);
    }
    if owner
        .session(session_id)?
        .is_some_and(|session| session.kind == "codex")
    {
        if fresh_codex {
            return report_launch_failure(send, session_id, false, message);
        }
        let binding = owner.codex_binding(session_id)?;
        let binding_id = binding.as_ref().map_or("", |binding| binding.id.as_str());
        let reason = if message == codex_resume::MISSING {
            message
        } else {
            format!("provider process could not start: {message}")
        };
        owner.change_codex_restore(
            session_id,
            ClaudeRestore::Fail {
                binding_id,
                error: &reason,
            },
        )?;
        return report_launch_failure(send, session_id, true, reason);
    }
    if owner.pending_claude_fork(session_id)?.is_some()
        || owner.pending_antigravity_fork(session_id)?.is_some()
    {
        let reason = format!("Fork CLI could not start: {message}");
        owner.fail_claude_fork(session_id, &reason)?;
        return report_launch_failure(send, session_id, true, reason);
    }
    if let Some(binding) = owner
        .claude_binding(session_id)?
        .filter(|binding| binding.resumable)
        && owner
            .session(session_id)?
            .is_some_and(|session| session.kind == "claude-code")
    {
        let reason = format!("provider process could not start: {message}");
        if owner.change_claude_restore(
            session_id,
            ClaudeRestore::Fail {
                binding_id: &binding.id,
                error: &reason,
            },
        )? {
            return report_launch_failure(send, session_id, true, reason);
        }
    }
    report_launch_failure(send, session_id, recovering, message)
}

fn report_launch_failure(
    send: &async_channel::Sender<Event>,
    session_id: &str,
    recovering: bool,
    message: String,
) -> Result<RecoveryState> {
    let state = if recovering {
        RecoveryState::Failed(message.clone())
    } else {
        RecoveryState::StartFailed(message.clone())
    };
    send.send_blocking(Event::Recovery {
        session_id: session_id.to_owned(),
        state: state.clone(),
    })?;
    send.send_blocking(Event::Diagnostic(message))?;
    Ok(state)
}

fn send_input(
    owner: &mut ShellOwner,
    input: PendingInput,
    promotions: &mut HashMap<String, ShellPromotion>,
    shell_submissions: &mut HashMap<String, PendingShellSubmission>,
    operations: &mut Vec<WorkerOperation>,
    send: &async_channel::Sender<Event>,
) -> Result<bool> {
    if let Some(promotion) = promotions.get_mut(&input.session_id) {
        promotion.inputs.push_back(input);
        return Ok(false);
    }
    let PendingInput {
        session_id,
        bytes,
        defer_ordering,
        ..
    } = input;
    let interaction = match bytes.as_slice() {
        [13] | [10] => Some("submit"),
        [3] | [4] => Some("control"),
        _ => None,
    };
    let (reply, result) = mpsc::channel();
    let mut pending = None;
    let request = if owner.tracks_shell_input(&session_id) {
        let submission = if bytes.iter().any(|byte| matches!(byte, b'\r' | b'\n')) {
            let (request, decision) = mpsc::channel();
            pending = Some(PendingShellSubmission {
                defer_ordering,
                record_on_match: interaction != Some("submit"),
                decision,
                following: VecDeque::new(),
            });
            Some(request)
        } else {
            None
        };
        WorkerCommand::WriteShellInput {
            bytes,
            submission,
            reply,
        }
    } else {
        WorkerCommand::Write { bytes, reply }
    };
    if let Err(error) = owner.send(&session_id, request) {
        send.send_blocking(Event::Diagnostic(format!("{error:#}")))?;
        return Ok(false);
    }
    operations.push((session_id.clone(), result, true));
    if let Some(pending) = pending {
        shell_submissions.insert(session_id.clone(), pending);
    }

    // Enter or Ctrl+C is known as soon as the bytes are queued. Only a match
    // within a larger write needs the worker's decision to count as a submit.
    match interaction {
        Some(kind) => record_terminal_interaction(owner, &session_id, kind, defer_ordering, send),
        None => Ok(false),
    }
}

fn record_terminal_interaction(
    owner: &mut ShellOwner,
    session_id: &str,
    kind: &str,
    defer_ordering: bool,
    send: &async_channel::Sender<Event>,
) -> Result<bool> {
    if !owner.provider_allows_derivation(session_id) {
        return Ok(false);
    }
    match owner.record_interaction(session_id, kind, defer_ordering) {
        Ok(_) => Ok(true),
        Err(error) => {
            send.send_blocking(Event::Diagnostic(format!("{error:#}")))?;
            Ok(false)
        }
    }
}

fn workspace_is_invalid(paths: &BTreeMap<String, WorkspacePathState>, id: Option<&str>) -> bool {
    id.and_then(|id| paths.get(id))
        .is_some_and(|state| state.status == "invalid")
}

fn shell_ids(graph: &Value) -> impl Iterator<Item = &str> {
    graph["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|node| node["currentMode"] == "shell" && node.get("archivedAt").is_none())
        .filter_map(|node| node["sessionId"].as_str())
}

fn launchable_nodes(graph: &Value) -> impl Iterator<Item = &Value> {
    graph["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|node| {
            matches!(
                node["currentMode"].as_str(),
                Some("shell" | "claude-code" | "codex" | "dsh" | "antigravity")
            ) && node.get("archivedAt").is_none()
        })
}

fn launchable_ids(graph: &Value) -> impl Iterator<Item = &str> {
    launchable_nodes(graph).filter_map(|node| node["sessionId"].as_str())
}

fn unstarted_sessions(
    owner: &ShellOwner,
    graph: &Value,
    sessions: &HashSet<String>,
) -> Result<HashSet<String>> {
    let mut pending = HashSet::new();
    for node in launchable_nodes(graph).filter(|node| {
        node["environment"]["state"]
            .as_str()
            .is_none_or(|state| state == "ready")
    }) {
        let Some(id) = node["sessionId"]
            .as_str()
            .filter(|id| !sessions.contains(*id))
        else {
            continue;
        };
        if node["currentMode"] == "shell" {
            pending.insert(id.to_owned());
            continue;
        }
        if owner.fresh_control_session(id)? {
            pending.insert(id.to_owned());
            continue;
        }
        if node["currentMode"] == "antigravity" {
            if owner.pending_antigravity_fork(id)?.is_some()
                || owner
                    .antigravity_binding(id)?
                    .is_some_and(|binding| binding.resumable)
            {
                pending.insert(id.to_owned());
            }
            continue;
        }
        if node["currentMode"] == "dsh" {
            if owner
                .dsh_binding(id)?
                .is_some_and(|binding| binding.resumable)
            {
                pending.insert(id.to_owned());
            }
            continue;
        }
        if node["currentMode"] == "codex" {
            if owner.pending_codex_fork(id)?.is_some()
                || node["providerRestoreState"] != "failed"
                    && owner
                        .codex_binding(id)?
                        .is_some_and(|binding| binding.resumable)
            {
                pending.insert(id.to_owned());
            }
            continue;
        }
        if owner.pending_claude_fork(id)?.is_some() {
            pending.insert(id.to_owned());
            continue;
        }
        let binding = owner.claude_binding(id)?;
        if binding
            .as_ref()
            .is_some_and(|binding| binding.restore_state == "failed")
        {
            continue;
        }
        if binding.as_ref().is_some_and(|binding| binding.resumable)
            || (owner
                .session(id)?
                .is_some_and(|session| session.status == "created")
                && !owner.has_shell_run(id)?)
            || (binding.is_some()
                && owner
                    .session(id)?
                    .is_some_and(|session| session.status == "starting"))
        {
            pending.insert(id.to_owned());
        }
    }
    Ok(pending)
}

fn default_terminal_options() -> TerminalOptions {
    TerminalOptions::new(80, 24, 7, 14)
}

enum PreparedLaunch {
    Shell(String),
    ClaudeCode {
        launch: PreparedClaudeLaunch,
        fork_source: Option<String>,
    },
    Codex(CodexLaunch, Option<CodexResume>),
    Dsh(DshLaunch),
    Antigravity(antigravity_launch::LaunchPlan, AntigravityPermission),
    CodexFork,
}

// A binding may be missing or stale before SessionStart confirms this launch.
fn claude_target_metadata(plan: &ClaudeLaunch) -> Value {
    json!({
        "providerConfigId": plan.provider_config_id,
        "model": plan.model,
        "assignedRole": plan.assigned_role,
        "effort": plan.effort,
        "clearAssignmentEnvironment": plan.clear_assignment_environment,
    })
}

enum LaunchIntent<'a> {
    Start,
    Environment,
    FreshCodex,
    ShellPromotion(InteractiveLaunch),
    PermissionRestart {
        fresh_mode: Option<&'a str>,
        previous_target: Option<&'a Value>,
    },
}

fn load_role_preferences(data_root: &Path) -> ClaudeRoleLaunchPreferences {
    startup::open_read_only(&data_root.join("matou.sqlite"))
        .map(|db| read_claude_role_launch_preferences(&db))
        .unwrap_or_default()
}

/// Locale is available even before the database can be opened for recovery.
pub fn save_locale_preference(
    data_root: &Path,
    preference: LocalePreference,
) -> Result<InterfaceLocale> {
    use std::io::Write;

    std::fs::create_dir_all(data_root)?;
    let mut file = tempfile::NamedTempFile::new_in(data_root)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    serde_json::to_writer(&mut file, &json!({ "preference": preference }))?;
    file.flush()?;
    file.as_file().sync_all()?;
    file.persist(data_root.join(LOCALE_FILE))?;
    Ok(current_interface_locale(preference))
}

fn time_zone_settings(
    data_root: &Path,
    setting: Option<&Value>,
) -> Result<TimeZoneSettingsSnapshot, TimeZoneSettingsError> {
    let time_zone = if let Some(setting) = setting {
        let setting = setting
            .as_str()
            .filter(|zone| valid_claude_time_zone(zone))
            .ok_or(TimeZoneSettingsError::InvalidRequest)?;
        open_role_assignments_writer(data_root)
            .and_then(|db| save_claude_time_zone(&db, setting, now()))
            .map_err(|_| TimeZoneSettingsError::Storage)?;
        setting.to_owned()
    } else {
        startup::open_read_only(&data_root.join("matou.sqlite"))
            .and_then(|db| read_claude_time_zone(&db))
            .map_err(|_| TimeZoneSettingsError::Storage)?
    };
    Ok(time_zone_snapshot(time_zone))
}

fn time_zone_snapshot(time_zone: String) -> TimeZoneSettingsSnapshot {
    let system_time_zone = system_time_zone();
    let auto_time_zone = automatic_time_zone(&system_time_zone, None).map(str::to_owned);
    static ZONES: LazyLock<Vec<String>> = LazyLock::new(available_time_zones);
    let mut zones = ZONES.clone();
    if !matches!(time_zone.as_str(), "auto" | "system") && !zones.contains(&time_zone) {
        zones.push(time_zone.clone());
    }
    TimeZoneSettingsSnapshot {
        time_zone,
        system_time_zone,
        auto_time_zone,
        zones,
    }
}

pub(crate) fn native_role_catalog() -> Vec<Value> {
    claude_model_catalog::models()
        .into_iter()
        .filter_map(|model| serde_json::to_value(model).ok())
        .collect()
}

fn save_role_assignments(
    data_root: &Path,
    request_id: u64,
    value: &Value,
    web_search: Option<&Value>,
    compaction: Option<&Value>,
) -> Result<ClaudeModelPreferencesReply> {
    let assignments = validate_claude_role_assignments(data_root, value)?;
    let search_patch = web_search
        .map(|target| {
            if target.is_null() {
                Ok(None)
            } else {
                validate_claude_web_search(data_root, target, &assignments, &native_role_catalog)
                    .map(Some)
            }
        })
        .transpose()?;
    let compaction_patch = compaction
        .map(|target| {
            if target.is_null() {
                Ok(None)
            } else {
                validate_claude_compaction(data_root, target, &native_role_catalog).map(Some)
            }
        })
        .transpose()?;
    let (saved_search, saved_compaction) = (|| {
        let mut connection = open_role_assignments_writer(data_root)?;
        save_claude_model_preferences(
            &mut connection,
            &assignments,
            search_patch.as_ref().map(Option::as_ref),
            compaction_patch.as_ref().map(Option::as_ref),
            now(),
        )
    })()
    .context("保存未确认，请重试。原选择已保留。")?;
    Ok(ClaudeModelPreferencesReply {
        roles: ClaudeRoleAssignmentsReply {
            request_id,
            assignments: Some(assignments),
            web_search: if web_search.is_some() {
                Some(saved_search)
            } else {
                saved_search.map(Some)
            },
            error: None,
        },
        compaction: Some(saved_compaction),
    })
}

fn open_role_assignments_writer(data_root: &Path) -> Result<Connection> {
    // The host holds the database lease. The worker writes the preferences row
    // through its own configured connection to the existing database.
    let connection = Connection::open_with_flags(
        data_root.join("matou.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_WRITE,
    )?;
    configure_connection(&connection)?;
    Ok(connection)
}

fn prepare_launch(
    owner: &ShellOwner,
    options: &LaunchOptions,
    session_id: &str,
    intent: LaunchIntent<'_>,
    role_requests: &Sender<RoleAssignmentsRequest>,
) -> Result<Receiver<Result<PreparedLaunch>>> {
    let session = owner
        .session(session_id)?
        .context("read the authoritative session")?;
    let (send, receive) = mpsc::channel();
    match session.kind.as_str() {
        "shell" => {
            let candidates = if matches!(intent, LaunchIntent::Environment) {
                vec![session.cwd]
            } else {
                owner.shell_directory_candidates(session_id, &options.shell_home)?
            };
            thread::Builder::new()
                .name("shell-directory".into())
                .spawn(move || {
                    let directory = candidates.into_iter().find(|path| Path::new(path).is_dir());
                    let _ = send.send(
                        directory
                            .map(PreparedLaunch::Shell)
                            .context("no usable Shell launch directory"),
                    );
                })?;
        }
        "claude-code" => {
            // A promoted card always starts a new conversation even if old
            // data for this session ID includes a provider binding.
            let (binding, fork) = if matches!(intent, LaunchIntent::ShellPromotion(_)) {
                ensure!(
                    owner.pending_claude_fork(session_id)?.is_none(),
                    "Shell card has an unconfirmed fork"
                );
                (None, None)
            } else {
                (
                    owner.claude_binding(session_id)?,
                    owner.pending_claude_fork(session_id)?,
                )
            };
            let mut metadata = binding
                .as_ref()
                .map_or(Value::Null, |binding| binding.metadata.clone());
            let mut remembered = false;
            if let LaunchIntent::ShellPromotion(InteractiveLaunch {
                profile: LaunchProfile::ClaudeCode,
                mode,
            }) = intent
            {
                metadata = json!({"permissionMode": mode});
            }
            let resume_id = if let LaunchIntent::PermissionRestart {
                fresh_mode: Some(mode),
                previous_target,
            } = intent
            {
                ensure!(fork.is_none(), "Card has an unconfirmed fork");
                if binding.as_ref().is_none_or(|binding| !binding.resumable)
                    && let Some(target) = previous_target
                {
                    metadata = target.clone();
                    remembered = true;
                }
                if !metadata.is_object() {
                    metadata = json!({});
                }
                metadata["permissionMode"] = json!(mode);
                None
            } else {
                binding
                    .filter(|binding| binding.resumable)
                    .map(|binding| binding.provider_session_id)
            };
            let fresh_launch = fork.is_none()
                && resume_id.is_none()
                && matches!(
                    intent,
                    LaunchIntent::Start
                        | LaunchIntent::Environment
                        | LaunchIntent::ShellPromotion(_)
                );
            let (reply, role_preferences) = mpsc::channel();
            // Every new PTY captures its own preferences. Use the save queue
            // so a prior save is visible before its reply reaches the client.
            role_requests.send(RoleAssignmentsRequest::ForLaunch(reply))?;
            let permission_fresh = matches!(
                intent,
                LaunchIntent::PermissionRestart {
                    fresh_mode: Some(_),
                    ..
                }
            );
            let data_root = options.data_root.clone();
            let environment = owner.launch_environment();
            let subscriptions = owner.subscriptions.clone();
            let codex_exit = owner
                .egress()
                .exit_country(Family::ChatgptSubscription)
                .map(str::to_owned);
            thread::Builder::new()
                .name("claude-launch".into())
                .spawn(move || {
                    let terminal = default_terminal_options();
                    let cwd = Path::new(&session.cwd);
                    let result = (|| {
                        let preferences = role_preferences.recv()?;
                        let mut launch = if subscriptions.is_some() {
                            let (binding, target) = if let Some(fork) = &fork {
                                (&fork.metadata, Some(ClaudeTarget::Fork(&fork.source_id)))
                            } else if fresh_launch {
                                (
                                    &metadata,
                                    Some(ClaudeTarget::New {
                                        assignments: Some(&preferences.assignments),
                                        budget_preferences: Some((
                                            &preferences,
                                            &native_role_catalog,
                                        )),
                                    }),
                                )
                            } else if permission_fresh {
                                (
                                    &metadata,
                                    Some(ClaudeTarget::PermissionRestart { remembered }),
                                )
                            } else {
                                (&metadata, resume_id.as_deref().map(ClaudeTarget::Resume))
                            };
                            prepare_managed_claude_launch(
                                &data_root,
                                cwd,
                                binding,
                                target,
                                environment,
                                terminal.cols,
                                terminal.rows,
                            )
                        } else if let Some(fork) = &fork {
                            prepare_claude_fork(
                                &data_root,
                                cwd,
                                &fork.metadata,
                                &fork.source_id,
                                environment,
                                terminal.cols,
                                terminal.rows,
                            )
                        } else if fresh_launch {
                            prepare_new_claude_launch_with_budget(
                                &data_root,
                                cwd,
                                &metadata,
                                (&preferences, &native_role_catalog),
                                environment,
                                terminal.cols,
                                terminal.rows,
                            )
                        } else if permission_fresh {
                            prepare_claude_permission_restart(
                                &data_root,
                                cwd,
                                &metadata,
                                remembered,
                                environment,
                                terminal.cols,
                                terminal.rows,
                            )
                        } else {
                            prepare_claude_launch(
                                &data_root,
                                cwd,
                                &metadata,
                                resume_id.as_deref(),
                                environment,
                                terminal.cols,
                                terminal.rows,
                            )
                        }?;
                        if !fresh_launch {
                            launch.capture_restart_gateway(&preferences, &native_role_catalog)?;
                        }
                        let zone = startup::open_read_only(&data_root.join("matou.sqlite"))
                            .and_then(|db| read_claude_time_zone(&db))
                            .unwrap_or_else(|_| "auto".into());
                        let mut launch = match &subscriptions {
                            Some(service) => service.prepare_session(launch, &preferences)?,
                            None => launch.into(),
                        };
                        launch.launch.project_time_zone_with_exit(
                            &zone,
                            &system_time_zone(),
                            codex_exit.as_deref(),
                        );
                        Ok(PreparedLaunch::ClaudeCode {
                            launch,
                            fork_source: fork.map(|fork| fork.source_id),
                        })
                    })();
                    let _ = send.send(result);
                })?;
        }
        "antigravity" => {
            let mut environment = owner.launch_environment();
            let shell_home = options.shell_home.clone();
            let catalog_root = antigravity_catalog::catalog_root(&shell_home, &environment);
            let (mode, metadata) = if let LaunchIntent::ShellPromotion(launch) = intent {
                (
                    AntigravityMode::Fresh,
                    json!({"permissionMode": launch.mode}),
                )
            } else if let Some(fork) = owner.pending_antigravity_fork(session_id)? {
                (AntigravityMode::Fork(fork.source_id), fork.metadata)
            } else {
                match owner.antigravity_binding(session_id)? {
                    Some(binding) if binding.resumable => (
                        AntigravityMode::Resume(binding.provider_session_id),
                        binding.metadata,
                    ),
                    Some(binding) if owner.fresh_control_session(session_id)? => {
                        (AntigravityMode::Fresh, binding.metadata)
                    }
                    _ => (AntigravityMode::Fresh, Value::Null),
                }
            };
            let environment_handoff = matches!(intent, LaunchIntent::Environment);
            thread::Builder::new().name("antigravity-launch".into()).spawn(move || {
                let result = (|| {
                    let cwd = Path::new(&session.cwd);
                    if !environment_handoff && let AntigravityMode::Resume(id) = &mode {
                        let mut catalog = AntigravityCatalog::new(catalog_root);
                        ensure!(
                            matches!(catalog.recovery_candidate(cwd, id), Ok(RecoveryCandidate::AwaitingIdentity(_))),
                            "Antigravity session history is unavailable in this working directory"
                        );
                    }
                    antigravity_launch::prepare_environment(&shell_home, &mut environment)?;
                    let permission = AntigravityPermission::from_saved(metadata["permissionMode"].as_str().unwrap_or_default());
                    let launch = antigravity_launch::prepare(antigravity_launch::LaunchInput {
                        cwd, settings_path: None, model: metadata["model"].as_str(), permission, mode,
                        inherited_environment: environment, run_environment: Default::default(),
                    })?;
                    Ok(PreparedLaunch::Antigravity(launch, permission))
                })();
                let _ = send.send(result);
            })?;
        }
        "dsh" => {
            let catalog_root = owner.dsh_session_root.clone();
            let mut environment = owner.launch_environment();
            if let Some(root) = &catalog_root {
                environment.insert("DSH_TUI_SESSION_ROOT".into(), root.as_os_str().to_owned());
            }
            let data_root = options.data_root.clone();
            let binding = if matches!(intent, LaunchIntent::ShellPromotion(_)) {
                None
            } else {
                owner
                    .dsh_binding(session_id)?
                    .filter(|binding| binding.resumable)
            };
            let environment_handoff = matches!(intent, LaunchIntent::Environment);
            thread::Builder::new()
                .name("dsh-launch".into())
                .spawn(move || {
                    let result = (|| {
                        let plan = crate::dsh_launch::prepare(crate::dsh_launch::DshLaunchInput {
                            cwd: Path::new(&session.cwd),
                            patch_path: None,
                            resume_session_id: binding
                                .as_ref()
                                .map(|binding| binding.provider_session_id.as_str()),
                            inherited_environment: environment,
                            run_environment: Default::default(),
                        })?;
                        let config_dump = dump_dsh_config(&plan, &data_root)?;
                        Ok(PreparedLaunch::Dsh(DshLaunch {
                            plan,
                            config_dump,
                            catalog_root,
                            environment_handoff,
                        }))
                    })();
                    let _ = send.send(result);
                })?;
        }
        "codex" => {
            if owner.pending_codex_fork(session_id)?.is_some() {
                let _ = send.send(Ok(PreparedLaunch::CodexFork));
                return Ok(receive);
            }
            let env = owner.launch_environment();
            let data_root = options.data_root.clone();
            let promotion = match intent {
                LaunchIntent::ShellPromotion(InteractiveLaunch {
                    profile: LaunchProfile::Codex,
                    mode,
                }) => Some(mode),
                LaunchIntent::Start
                | LaunchIntent::Environment
                | LaunchIntent::FreshCodex
                | LaunchIntent::PermissionRestart {
                    fresh_mode: None, ..
                } => None,
                _ => bail!("unsupported Codex launch intent"),
            };
            let binding = if promotion.is_none() {
                owner.codex_binding(session_id)?
            } else {
                None
            };
            let fresh = matches!(intent, LaunchIntent::FreshCodex)
                || (matches!(intent, LaunchIntent::Environment) && binding.is_none())
                || owner.fresh_control_session(session_id)?;
            thread::Builder::new()
                .name("codex-launch".into())
                .spawn(move || {
                    let terminal = default_terminal_options();
                    let cwd = Path::new(&session.cwd);
                    let result = if let Some(mode) = promotion {
                        codex_launch::prepare(
                            &data_root,
                            cwd,
                            Some(&json!(mode)),
                            env,
                            terminal.cols,
                            terminal.rows,
                        )
                        .map(|launch| (launch, None))
                    } else {
                        codex_resume::prepare(
                            &data_root,
                            cwd,
                            binding.as_ref(),
                            fresh,
                            env,
                            terminal.cols,
                            terminal.rows,
                        )
                    }
                    .map(|(mut launch, resume)| {
                        let official = official_time_zone_endpoint(
                            launch
                                .launch
                                .env
                                .get(std::ffi::OsStr::new("OPENAI_BASE_URL"))
                                .map(OsString::as_os_str),
                            "api.openai.com",
                        );
                        project_cli_time_zone(
                            &mut launch.launch,
                            "auto",
                            &system_time_zone(),
                            official,
                        );
                        PreparedLaunch::Codex(launch, resume)
                    });
                    let _ = send.send(result);
                })?;
        }
        _ => bail!("unsupported session launch kind"),
    }
    Ok(receive)
}

fn dump_dsh_config(plan: &crate::dsh_launch::DshLaunchPlan, data_root: &Path) -> Result<String> {
    use std::io::Read;
    const MAX_CONFIG_BYTES: u64 = 1024 * 1024;
    let mut output = tempfile::tempfile_in(data_root)?;
    let mut child = std::process::Command::new(&plan.executable)
        .args(crate::dsh_launch::DUMP_CONFIG_ARGS)
        .current_dir(&plan.cwd)
        .env_clear()
        .envs(&plan.environment)
        .stdin(std::process::Stdio::null())
        .stdout(output.try_clone()?)
        .stderr(std::process::Stdio::null())
        .spawn()
        .context("Start the DSH configuration probe")?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None)
                if Instant::now() < deadline
                    && output
                        .metadata()
                        .is_ok_and(|info| info.len() <= MAX_CONFIG_BYTES) =>
            {
                thread::sleep(Duration::from_millis(20))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                bail!("DSH configuration probe exceeded its time or output limit");
            }
        }
    };
    ensure!(status.success(), "DSH configuration probe failed");
    ensure!(
        output.metadata()?.len() <= MAX_CONFIG_BYTES,
        "DSH configuration probe returned too much data"
    );
    std::io::Seek::rewind(&mut output)?;
    let mut contents = String::new();
    output
        .take(MAX_CONFIG_BYTES + 1)
        .read_to_string(&mut contents)?;
    Ok(contents)
}

fn launch_shell(
    owner: &mut ShellOwner,
    options: &LaunchOptions,
    session_id: &str,
    cwd: String,
    terminal_layouts: &HashMap<String, TerminalOptions>,
) -> Result<()> {
    let terminal = terminal_layouts
        .get(session_id)
        .copied()
        .unwrap_or_else(default_terminal_options);
    let mut inherited_env: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
    inherited_env.insert("HOME".into(), options.shell_home.clone().into_os_string());
    inherited_env.insert(
        "ZDOTDIR".into(),
        options.shell_home.clone().into_os_string(),
    );
    inherited_env.insert("SHELL".into(), options.shell.clone().into_os_string());
    owner.launch(
        SessionOptions {
            session_id: session_id.to_owned(),
            launch: ShellLaunch {
                profile: LaunchProfile::Shell,
                argv: Vec::new(),
                executable: options.shell.clone(),
                cwd: cwd.into(),
                data_root: options.data_root.clone(),
                inherited_env,
                env: BTreeMap::new(),
                cols: terminal.cols,
                rows: terminal.rows,
            },
            terminal,
            journal: JournalOptions::default(),
            restored_history: Vec::new(),
            collect_history: true,
        },
        true,
        "──────── 会话已恢复 ────────",
    )
}

fn active_graph(
    owner: &ShellOwner,
    scene_id: Option<&str>,
    window_id: &str,
    startup_deferred: &HashSet<String>,
) -> Result<Value> {
    match scene_id {
        Some(scene_id) => {
            let mut graph = owner.graph(scene_id, Some(window_id))?;
            let counts = owner.scene_workspace_session_counts(scene_id)?;
            graph["workspaceSessionCount"] = json!(counts.values().sum::<i64>());
            graph["workspaceSessionCountByTask"] = json!(counts);
            if let Some(nodes) = graph["nodes"].as_array_mut() {
                for node in nodes {
                    let Some(id) = node["sessionId"].as_str() else {
                        continue;
                    };
                    let profile = node["currentMode"].as_str().unwrap_or("shell");
                    let deferred = startup_deferred.contains(id);
                    if profile == "shell" && !deferred {
                        continue;
                    }
                    let Some(session) = owner.session(id)? else {
                        continue;
                    };
                    let codex = profile == "codex";
                    // History stays readable even when the bound conversation cannot resume.
                    let provider_session_id = owner
                        .environment_provider_binding(id, profile)?
                        .filter(|binding| !binding.provider_session_id.is_empty())
                        .map(|binding| binding.provider_session_id);
                    node["historyProviderSessionId"] = json!(provider_session_id);
                    if codex && let Some(hud) = owner.codex_hud(&session.id) {
                        node["codexHud"] = hud;
                    }
                    node["titleSource"] = json!(session.title_source);
                    node["providerTitle"] = json!(session.provider_title);
                }
            }
            Ok(graph)
        }
        None => Ok(serde_json::json!({"nodes": []})),
    }
}

fn send_sidebar(
    owner: &ShellOwner,
    window_id: &str,
    send: &async_channel::Sender<Event>,
) -> Result<()> {
    let mut workspaces = owner.workspaces()?;
    sort_workspaces(&mut workspaces);
    let tasks = sidebar_tasks(owner, window_id, &workspaces)?;
    send.send_blocking(Event::Workspaces(workspaces))?;
    send.send_blocking(Event::Tasks(tasks))?;
    Ok(())
}

fn sidebar_tasks(
    owner: &ShellOwner,
    window_id: &str,
    workspaces: &[Workspace],
) -> Result<Vec<Task>> {
    let mut tasks = Vec::new();
    for workspace in workspaces {
        tasks.extend(owner.tasks(window_id, &workspace.id)?);
    }
    Ok(tasks)
}

fn sort_workspaces(workspaces: &mut [Workspace]) {
    workspaces.sort_by(|left, right| {
        right
            .is_pinned
            .cmp(&left.is_pinned)
            .then_with(|| {
                if left.is_pinned {
                    left.pin_sort_key.cmp(&right.pin_sort_key)
                } else {
                    right
                        .last_opened_at
                        .cmp(&left.last_opened_at)
                        .then(left.created_at.cmp(&right.created_at))
                }
            })
            .then(left.id.cmp(&right.id))
    });
}

pub(crate) fn resolve_workspace_directory(directory: &str) -> Result<PathBuf> {
    let mut resolved = PathBuf::new();
    let directory = Path::new(if directory.is_empty() { "." } else { directory });
    for component in std::path::absolute(directory)?.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            other => resolved.push(other.as_os_str()),
        }
    }
    Ok(resolved)
}

enum NavigationAction {
    Card(Value),
    Fork(Value),
}

fn metadata(kind: &str, input: &Value) -> CommandMetadata {
    CommandMetadata {
        command_id: id(),
        command_type: kind.into(),
        request_hash: input.to_string(),
        causation_id: None,
        correlation_id: None,
    }
}

fn navigate(
    owner: &mut ShellOwner,
    window_id: &str,
    command: Command,
) -> Result<(HierarchyResult, Vec<String>, Option<NavigationAction>)> {
    let time = now();
    let ids = CanvasIds {
        scene_id: id(),
        root_node_id: id(),
        secondary_node_id: id(),
        session_id: id(),
        mount_id: id(),
        relation_id: id(),
    };
    let messages = CanvasMessages {
        new_canvas: "新画布",
        duplicate_scene_name: "当前事项下已存在同名页签",
        workspace_path_invalid: WORKSPACE_PATH_INVALID,
        detached_session_must_return: "独立窗口中的会话需要先回到原会话列表",
    };
    match command {
        Command::CreateWorkspace { root_directory } => {
            let directory = resolve_workspace_directory(&root_directory)?;
            let input = workspaces::CreateWorkspaceInput {
                window_id: window_id.into(),
                name: directory
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "工作区".into()),
                root_directory: directory.to_string_lossy().into_owned(),
                navigation: None,
                now: time,
            };
            owner.create_workspace(
                &metadata("hierarchy.create-workspace", &serde_json::to_value(&input)?),
                &input,
                &hierarchy_ids(),
                time,
            )?;
        }
        Command::SelectWorkspace { workspace_id } => {
            owner.select_workspace(&workspaces::SelectWorkspaceInput {
                window_id: window_id.into(),
                workspace_id,
                now: time,
            })?;
        }
        Command::RelinkWorkspace {
            workspace_id,
            root_directory,
        } => {
            let directory = resolve_workspace_directory(&root_directory)?;
            let input = workspaces::RelinkWorkspaceInput {
                workspace_id,
                root_directory: directory.to_string_lossy().into_owned(),
                now: time,
            };
            owner.relink_workspace(
                &metadata("hierarchy.relink-workspace", &serde_json::to_value(&input)?),
                &input,
                time,
            )?;
        }
        Command::SetWorkspacePinned {
            workspace_id,
            pinned,
        } => {
            let input = workspaces::SetWorkspacePinnedInput {
                workspace_id,
                pinned,
                now: time,
            };
            owner.set_workspace_pinned(
                &metadata(
                    "hierarchy.set-workspace-pinned",
                    &serde_json::to_value(&input)?,
                ),
                &input,
                time,
            )?;
        }
        Command::ReorderPinnedWorkspace {
            workspace_id,
            before_workspace_id,
        } => {
            let input = workspaces::ReorderPinnedWorkspaceInput {
                workspace_id,
                before_workspace_id,
                now: time,
            };
            owner.reorder_pinned_workspace(
                &metadata(
                    "hierarchy.reorder-pinned-workspace",
                    &serde_json::to_value(&input)?,
                ),
                &input,
                time,
            )?;
        }
        Command::RemoveWorkspace { workspace_id } => {
            let input = workspaces::RemoveWorkspaceInput {
                window_id: window_id.into(),
                confirmed_intent: format!("remove-workspace:{workspace_id}"),
                workspace_id,
                now: time,
            };
            let disposed = owner.remove_workspace(
                &metadata("hierarchy.remove-workspace", &serde_json::to_value(&input)?),
                &input,
                time,
            )?;
            return Ok((owner.hierarchy(window_id)?, disposed, None));
        }
        Command::CreateTask { workspace_id } => {
            let input = tasks::CreateTaskInput {
                window_id: window_id.into(),
                workspace_id,
                title: None,
                navigation: None,
                now: time,
            };
            owner.create_task(
                &metadata("hierarchy.create-task", &serde_json::to_value(&input)?),
                &input,
                &hierarchy_ids(),
                time,
            )?;
        }
        Command::SelectTask { task_id } => {
            owner.select_task(&tasks::SelectTaskInput {
                window_id: window_id.into(),
                task_id,
                now: time,
            })?;
        }
        Command::RenameTask { task_id, title } => {
            let input = tasks::RenameTaskInput {
                task_id,
                title,
                now: time,
            };
            owner.rename_task(
                &metadata("hierarchy.rename-task", &serde_json::to_value(&input)?),
                &input,
                time,
            )?;
        }
        Command::SetTaskPinned { task_id, pinned } => {
            let input = tasks::SetTaskPinnedInput {
                task_id,
                pinned,
                now: time,
            };
            owner.set_task_pinned(
                &metadata("hierarchy.set-task-pinned", &serde_json::to_value(&input)?),
                &input,
                time,
            )?;
        }
        Command::ReorderPinnedTask {
            workspace_id,
            task_id,
            before_task_id,
        } => {
            let input = tasks::ReorderPinnedTaskInput {
                workspace_id,
                task_id,
                before_task_id,
                now: time,
            };
            owner.reorder_pinned_task(
                &metadata(
                    "hierarchy.reorder-pinned-task",
                    &serde_json::to_value(&input)?,
                ),
                &input,
                time,
            )?;
        }
        Command::DeleteTask { task_id } => {
            let input = tasks::DeleteTaskInput {
                window_id: window_id.into(),
                confirmed_intent: Some(format!("delete-task:{task_id}")),
                task_id,
                now: time,
            };
            let disposed = owner.delete_task(
                &metadata("hierarchy.delete-task", &serde_json::to_value(&input)?),
                &input,
                &hierarchy_ids(),
                time,
            )?;
            return Ok((owner.hierarchy(window_id)?, disposed, None));
        }
        Command::CreateCanvas { task_id } => {
            let input = CreateCanvasInput {
                window_id: window_id.into(),
                task_id,
                title: None,
                navigation: None,
                now: time,
            };
            owner.create_canvas(
                &metadata("hierarchy.create-canvas", &serde_json::to_value(&input)?),
                &input,
                &ids,
                &messages,
                time,
            )?;
        }
        Command::CreateShellSibling {
            scene_id,
            source_session_id,
            parent_session_id,
        } => {
            let input = CreateShellSiblingInput {
                window_id: window_id.into(),
                scene_id,
                source_session_id,
                parent_session_id,
                execution_context_id: None,
                title: None,
                navigation: None,
                now: time,
            };
            owner.create_shell_sibling(
                &metadata(
                    "hierarchy.create-shell-sibling",
                    &serde_json::to_value(&input)?,
                ),
                &input,
                &ids,
                &messages,
                time,
            )?;
        }
        Command::ForkSession {
            scene_id,
            source_session_id,
            placement,
            name,
            worktree_mode,
            submission_key,
        } => {
            let input = ForkSessionInput {
                window_id: window_id.into(),
                scene_id,
                source_session_id,
                placement,
                name,
                worktree_mode,
                submission_key,
                now: time,
            };
            let result = owner.create_fork_child(
                &metadata("hierarchy.fork-session", &serde_json::to_value(&input)?),
                &input,
                &ids,
                &messages,
            )?;
            return Ok((
                owner.hierarchy(window_id)?,
                Vec::new(),
                Some(NavigationAction::Fork(result)),
            ));
        }
        Command::RetryFork { session_id } => {
            let result = owner.retry_fork(
                &metadata("hierarchy.retry-fork", &json!({"sessionId": session_id})),
                window_id,
                &session_id,
                time,
            )?;
            return Ok((
                owner.hierarchy(window_id)?,
                Vec::new(),
                Some(NavigationAction::Fork(result)),
            ));
        }
        Command::SelectScene { scene_id } => {
            owner.select_scene(window_id, &scene_id, time)?;
        }
        Command::RenameScene { scene_id, name } => {
            let input = RenameSceneInput {
                scene_id,
                name,
                now: time,
            };
            owner.rename_scene(
                &metadata("hierarchy.rename-scene", &serde_json::to_value(&input)?),
                &input,
                messages.duplicate_scene_name,
                time,
            )?;
        }
        Command::ReorderScene {
            scene_id,
            before_scene_id,
        } => {
            let input = ReorderSceneInput {
                window_id: window_id.into(),
                scene_id,
                before_scene_id,
                now: time,
            };
            owner.reorder_scene(
                &metadata("hierarchy.reorder-scene", &serde_json::to_value(&input)?),
                &input,
                time,
            )?;
        }
        Command::CloseScene {
            scene_id,
            confirmed,
        } => {
            let confirmed_intent = confirmed.then(|| format!("close-scene:{scene_id}"));
            let input = CloseSceneInput {
                window_id: window_id.into(),
                scene_id,
                confirmed_intent,
                now: time,
            };
            let disposed = owner.close_scene(
                &metadata("hierarchy.close-scene", &serde_json::to_value(&input)?),
                &input,
                time,
            )?;
            return Ok((owner.hierarchy(window_id)?, disposed, None));
        }
        Command::CloseSessionCard {
            scene_id,
            session_id,
            confirmed_intent,
            preserve_scene_on_last_session,
        } => {
            let input = CloseSessionCardInput {
                window_id: window_id.into(),
                scene_id,
                session_id,
                confirmed_intent,
                preserve_scene_on_last_session,
                now: time,
            };
            let result = owner.close_session_card(
                &metadata("hierarchy.delete-session", &serde_json::to_value(&input)?),
                &input,
                &hierarchy_ids(),
                time,
            )?;
            let disposed = serde_json::from_value(result["disposedSessionIds"].clone())?;
            let action = json!({
                "action": "close-session-card",
                "sceneId": input.scene_id,
                "sessionId": input.session_id,
                "outcome": result["outcome"],
                "rebuildCanvas": result["rebuildCanvas"],
                "disposedSessionIds": disposed,
            });
            return Ok((
                owner.hierarchy(window_id)?,
                disposed,
                Some(NavigationAction::Card(action)),
            ));
        }
        Command::RemoveSessionBranch {
            scene_id,
            session_id,
            scope,
        } => {
            let input = RemoveSessionBranchInput {
                window_id: window_id.into(),
                scene_id,
                session_id,
                scope,
                now: time,
            };
            let result = owner.remove_session_branch(
                &metadata(
                    "hierarchy.remove-session-branch",
                    &serde_json::to_value(&input)?,
                ),
                &input,
                time,
            )?;
            let disposed = serde_json::from_value(result["disposedSessionIds"].clone())?;
            let action = json!({
                "action": "remove-session-branch",
                "sceneId": input.scene_id,
                "sessionId": input.session_id,
                "scope": scope,
                "graph": result["graph"],
                "removedSessionIds": result["removedSessionIds"],
                "disposedSessionIds": disposed,
            });
            return Ok((
                owner.hierarchy(window_id)?,
                disposed,
                Some(NavigationAction::Card(action)),
            ));
        }
        Command::FocusSession {
            scene_id,
            session_id,
        } => {
            owner.focus_session(
                window_id,
                &scene_id,
                &session_id,
                time,
                "会话不在当前画布中",
            )?;
        }
        _ => unreachable!("only navigation commands reach navigate"),
    }
    Ok((owner.hierarchy(window_id)?, Vec::new(), None))
}

pub(crate) fn hierarchy_ids() -> HierarchyIds {
    HierarchyIds {
        workspace_id: id(),
        execution_context_id: id(),
        task_id: id(),
        scene_id: id(),
        root_node_id: id(),
        session_id: id(),
        mount_id: id(),
    }
}
fn id() -> String {
    Uuid::new_v4().to_string()
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod git_command_tests {
    use super::*;

    #[test]
    fn git_menu_protocol_returns_updated_status_and_protects_attached_worktrees() -> Result<()> {
        let fixture = tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?;
        let cwd = fixture.path().join("repository");
        std::fs::create_dir(&cwd)?;
        let git = |args: &[&str]| -> Result<()> {
            let result = std::process::Command::new("git")
                .current_dir(&cwd)
                .env("GIT_CONFIG_GLOBAL", fixture.path().join("no-git-config"))
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(args)
                .output()?;
            ensure!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            Ok(())
        };
        git(&["init", "--initial-branch=main"])?;
        git(&["config", "user.name", "Fixture"])?;
        git(&["config", "user.email", "fixture@example.invalid"])?;
        git(&["config", "commit.gpgsign", "false"])?;
        let hooks = fixture.path().join("no-hooks");
        git(&["config", "core.hooksPath", hooks.to_str().unwrap()])?;
        git(&["commit", "--allow-empty", "-m", "initial"])?;
        let db = Connection::open(fixture.path().join("matou.sqlite"))?;
        db.execute_batch(
            "CREATE TABLE sessions (cwd TEXT, execution_context_id TEXT, archived_at INTEGER);
             CREATE TABLE execution_contexts (id TEXT, cwd TEXT);",
        )?;
        let reply = |command: Command, variant: &str, id| -> Result<GitReply> {
            let command = serde_json::from_value(serde_json::to_value(command)?)?;
            let event = run_git_command(&command, fixture.path()).context("missing Git reply")?;
            let payload = serde_json::to_value(event)?;
            let reply: GitReply = serde_json::from_value(payload[variant].clone())?;
            assert_eq!(reply.request_id, id);
            Ok(reply)
        };
        assert_eq!(
            reply(
                Command::GitStatusDetail {
                    request_id: 1,
                    cwd: cwd.clone()
                },
                "GitStatusDetail",
                1
            )?
            .result
            .unwrap()
            .current_branch
            .as_deref(),
            Some("main")
        );
        assert_eq!(
            reply(
                Command::GitCreateBranch {
                    request_id: 2,
                    cwd: cwd.clone(),
                    name: "test/branch".into(),
                    start_point: Some("main".into()),
                },
                "GitCreateBranch",
                2
            )?
            .result
            .unwrap()
            .current_branch
            .as_deref(),
            Some("test/branch")
        );
        assert_eq!(
            reply(
                Command::GitCheckout {
                    request_id: 3,
                    cwd: cwd.clone(),
                    branch: "main".into(),
                },
                "GitCheckout",
                3
            )?
            .result
            .unwrap()
            .current_branch
            .as_deref(),
            Some("main")
        );

        std::fs::write(cwd.join("tracked.txt"), "changed\n")?;
        assert!(
            !reply(
                Command::GitCommit {
                    request_id: 4,
                    cwd: cwd.clone(),
                    message: "test commit".into(),
                    stage_all: true,
                },
                "GitCommit",
                4
            )?
            .result
            .unwrap()
            .dirty
        );
        git(&["checkout", "--detach", "HEAD"])?;
        assert_eq!(
            reply(
                Command::GitPush {
                    request_id: 5,
                    cwd: cwd.clone()
                },
                "GitPush",
                5
            )?
            .result
            .unwrap_err()
            .code,
            "GIT_DETACHED_HEAD"
        );
        git(&["checkout", "main"])?;

        let created = reply(
            Command::GitWorktreeCreate {
                request_id: 6,
                cwd: cwd.clone(),
                branch: "feature/wt".into(),
                path: None,
            },
            "GitWorktreeCreate",
            6,
        )?
        .result
        .unwrap();
        let target = created.worktrees[1].path.clone();
        assert!(target.starts_with(fixture.path().join("worktrees").canonicalize()?));
        db.execute(
            "INSERT INTO execution_contexts VALUES ('wt', ?1)",
            [target.to_str().unwrap()],
        )?;
        db.execute(
            "INSERT INTO sessions VALUES (?1, 'wt', NULL)",
            [target.to_str().unwrap()],
        )?;
        let snapshot = reply(
            Command::GitStatusDetail {
                request_id: 7,
                cwd: cwd.clone(),
            },
            "GitStatusDetail",
            7,
        )?;
        assert_eq!(snapshot.worktree_session_counts[&target], 1);
        assert_eq!(
            reply(
                Command::GitWorktreeRemove {
                    request_id: 8,
                    cwd: cwd.clone(),
                    worktree_path: target.clone(),
                },
                "GitWorktreeRemove",
                8
            )?
            .result
            .unwrap_err()
            .code,
            "GIT_WORKTREE_IN_USE"
        );
        assert!(target.is_dir());
        db.execute("UPDATE sessions SET archived_at = 1", [])?;
        let removed = reply(
            Command::GitWorktreeRemove {
                request_id: 9,
                cwd,
                worktree_path: target.clone(),
            },
            "GitWorktreeRemove",
            9,
        )?
        .result
        .unwrap();
        assert_eq!(removed.worktrees.len(), 1);
        assert!(!target.exists());
        Ok(())
    }

    #[test]
    fn recovery_modes_reject_every_git_write_with_correlated_replies() {
        let cwd = PathBuf::from("/unused");
        assert!(StoragePhase::ReadOnly.accepts(&Command::GitStatusDetail {
            request_id: 1,
            cwd: cwd.clone(),
        }));
        let commands = [
            Command::GitCheckout {
                request_id: 2,
                cwd: cwd.clone(),
                branch: "any".into(),
            },
            Command::GitCreateBranch {
                request_id: 3,
                cwd: cwd.clone(),
                name: "any".into(),
                start_point: None,
            },
            Command::GitCommit {
                request_id: 4,
                cwd: cwd.clone(),
                message: "example".into(),
                stage_all: true,
            },
            Command::GitPush {
                request_id: 5,
                cwd: cwd.clone(),
            },
            Command::GitWorktreeCreate {
                request_id: 6,
                cwd: cwd.clone(),
                branch: "any".into(),
                path: None,
            },
            Command::GitWorktreeRemove {
                request_id: 7,
                cwd,
                worktree_path: "/unused-wt".into(),
            },
        ];
        for (index, command) in commands.into_iter().enumerate() {
            assert!(StoragePhase::Writable.accepts(&command));
            assert!(!StoragePhase::ReadOnly.accepts(&command));
            assert!(!StoragePhase::Recovery.accepts(&command));
            let reply = match storage_command_rejected(
                command,
                STORAGE_READ_ONLY,
                STORAGE_READ_ONLY_MESSAGE,
            ) {
                Event::GitCheckout(reply)
                | Event::GitCreateBranch(reply)
                | Event::GitCommit(reply)
                | Event::GitPush(reply)
                | Event::GitWorktreeCreate(reply)
                | Event::GitWorktreeRemove(reply) => reply,
                _ => panic!("expected the rejected Git request's normal reply variant"),
            };
            assert_eq!(reply.request_id, (index + 2) as u64);
            assert_eq!(reply.result.unwrap_err().code, STORAGE_READ_ONLY);
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn role_assignment_writer_uses_runtime_pragmas() -> Result<()> {
        let root = tempfile::tempdir()?;
        let owner = Connection::open(root.path().join("matou.sqlite"))?;
        configure_connection(&owner)?;
        let writer = open_role_assignments_writer(root.path())?;
        let pragma = |name| writer.pragma_query_value(None, name, |row| row.get::<_, i64>(0));
        let journal: String = writer.pragma_query_value(None, "journal_mode", |row| row.get(0))?;
        assert_eq!(journal, "wal");
        assert_eq!(pragma("busy_timeout")?, 5000);
        assert_eq!(pragma("foreign_keys")?, 1);
        assert_eq!(pragma("trusted_schema")?, 0);
        assert_eq!(pragma("synchronous")?, 2);
        Ok(())
    }

    #[test]
    fn permission_restart_with_expired_binding_reads_preferences_for_the_new_run() -> Result<()> {
        let root = tempfile::tempdir()?;
        let options = LaunchOptions {
            data_root: root.path().join("data"),
            workspace: root.path().join("workspace"),
            shell_home: root.path().join("home"),
            shell: "/bin/zsh".into(),
        };
        let BootstrapResult::Writable {
            mut connection,
            ownership,
            ..
        } = startup::open(&options.data_root, "seed")?
        else {
            bail!("test database is not writable");
        };
        let session = hierarchy::bootstrap_window(
            &mut connection,
            &CommandMetadata {
                command_id: id(),
                command_type: "window.bootstrap".into(),
                request_hash: "seed".into(),
                causation_id: None,
                correlation_id: None,
            },
            &BootstrapWindowInput {
                window_id: "main-window-1".into(),
                default_root_directory: options.workspace.to_string_lossy().into_owned(),
                default_name: "Workspace".into(),
                now: 1,
            },
            &hierarchy_ids(),
            "默认",
            1,
        )?
        .session
        .context("seed session")?;
        connection.execute(
            "UPDATE sessions SET kind='claude-code' WHERE id=?",
            [&session.id],
        )?;
        connection.execute(
            "INSERT INTO provider_bindings
                (id, session_id, provider, provider_session_id, resume_state, metadata_json,
                 created_at, updated_at, invalidated_at, validated_at)
             VALUES ('old-binding', ?, 'claude-code', 'old-conversation', 'expired',
                     '{\"model\":\"bound-fixture-model\"}', 1, 1, 1, 1)",
            [&session.id],
        )?;
        let mut owner = ShellOwner::new(connection, ownership, id)?;
        assert!(
            !owner
                .claude_binding(&session.id)?
                .context("expired binding")?
                .resumable
        );
        let (role_requests, requests) = mpsc::channel();
        let mut change = PermissionChange {
            profile: LaunchProfile::ClaudeCode,
            mode: "plan".into(),
            binding: None,
            phase: PermissionPhase::Stopping(None),
        };
        assert!(matches!(
            change.advance(
                &mut owner,
                &options,
                &session.id,
                default_terminal_options(),
                &role_requests,
                &mut HashMap::new(),
            )?,
            PermissionProgress::Pending
        ));
        let RoleAssignmentsRequest::ForLaunch(reply) = requests.try_recv()? else {
            bail!("permission restart did not request current preferences");
        };
        reply.send(Default::default())?;
        assert!(matches!(change.phase, PermissionPhase::Preparing(Some(_))));
        Ok(())
    }

    #[test]
    fn session_shutdown_failure_reports_backup_outcome() {
        let session_error = || std::io::Error::other("worker cleanup failed").into();
        let completed = shutdown_result(Err(session_error()), Some(Ok(()))).unwrap_err();
        assert!(format!("{completed:#}").contains("clean exit backup completed"));
        assert!(completed.downcast_ref::<std::io::Error>().is_some());

        let backup =
            anyhow!("backup file could not be written").context("clean exit backup failed");
        let failed = shutdown_result(Err(session_error()), Some(Err(backup))).unwrap_err();
        let report = format!("{failed:#}");
        assert!(report.contains("worker cleanup failed"));
        assert!(report.contains("clean exit backup failed: backup file could not be written"));
        assert!(failed.downcast_ref::<std::io::Error>().is_some());
    }

    fn open_raw_test_shell(root: &Path) -> Result<(RuntimeHost, String)> {
        let home = root.join("home");
        std::fs::create_dir_all(&home)?;
        std::fs::write(
            home.join(".zshrc"),
            "[[ -t 0 ]] || return\n/bin/stty raw -echo\nprintf 'MATOU_RAW_SHELL_READY\\r\\n'\nexec /bin/cat > \"$HOME/bytes\"\n",
        )?;
        let runtime = RuntimeHost::start_with_offline_fixtures(
            LaunchOptions {
                data_root: root.join("data"),
                workspace: root.join("workspace"),
                shell_home: home,
                shell: "/bin/zsh".into(),
            },
            OfflineFixtures::default(),
        )?;
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            match next_test_event(&runtime, deadline)? {
                Event::Frame {
                    session_id, screen, ..
                } if screen.cells.chunks(screen.cols).any(|row| {
                    row.iter()
                        .map(|cell| cell.text.as_str())
                        .collect::<String>()
                        .contains("MATOU_RAW_SHELL_READY")
                }) =>
                {
                    return Ok((runtime, session_id));
                }
                Event::Diagnostic(message) => bail!("{message}"),
                _ => {}
            }
        }
    }

    fn next_test_event(runtime: &RuntimeHost, deadline: Instant) -> Result<Event> {
        loop {
            match runtime.events.try_recv() {
                Ok(event) => {
                    if let Event::Frame {
                        session_id,
                        sequence,
                        ..
                    } = &event
                    {
                        send_test_command(
                            runtime,
                            Command::Applied {
                                session_id: session_id.clone(),
                                sequence: *sequence,
                            },
                        )?;
                    }
                    return Ok(event);
                }
                Err(async_channel::TryRecvError::Empty) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(2));
                }
                Err(error) => return Err(error).context("waiting for host event"),
            }
        }
    }

    fn send_test_command(runtime: &RuntimeHost, command: Command) -> Result<()> {
        runtime
            .commands
            .send(command)
            .map_err(|_| anyhow!("runtime command channel closed"))
    }

    fn test_input(session: &str, bytes: &[u8]) -> Command {
        Command::Input {
            session_id: session.to_owned(),
            bytes: bytes.to_vec(),
            defer_ordering: true,
        }
    }

    fn queue_test_batch(
        runtime: &RuntimeHost,
        commands: impl IntoIterator<Item = Command>,
    ) -> Result<()> {
        let (paused, wait_paused) = mpsc::channel();
        let (resume, wait_resume) = mpsc::channel();
        send_test_command(
            runtime,
            Command::TestPause {
                paused,
                resume: wait_resume,
            },
        )?;
        wait_paused.recv_timeout(Duration::from_secs(8))?;
        for command in commands {
            send_test_command(runtime, command)?;
        }
        resume.send(())?;
        Ok(())
    }

    fn assert_raw_shell_input(root: &Path, expected: &[u8]) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let actual = match std::fs::read(root.join("home/bytes")) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(error) => return Err(error.into()),
            };
            assert!(
                expected.starts_with(&actual),
                "unexpected Shell input: {actual:?}"
            );
            if actual == expected {
                return Ok(());
            }
            ensure!(
                Instant::now() < deadline,
                "incomplete Shell input: {actual:?}"
            );
            thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn frozen_write_does_not_advance_the_shell_tracker() -> Result<()> {
        let root = tempfile::tempdir()?;
        let (mut runtime, session_id) = open_raw_test_shell(root.path())?;
        let freeze = |frozen| -> Result<()> {
            let (reply, result) = mpsc::channel();
            send_test_command(
                &runtime,
                Command::TestFreezeShellWrites {
                    session_id: session_id.clone(),
                    frozen,
                    reply,
                },
            )?;
            result.recv_timeout(Duration::from_secs(3))??;
            Ok(())
        };
        freeze(true)?;
        for bytes in [&b"claude\r"[..], b"clau"] {
            send_test_command(&runtime, test_input(&session_id, bytes))?;
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Event::Diagnostic(error) = next_test_event(&runtime, deadline)? {
                    assert_eq!(error, "session output is frozen");
                    break;
                }
            }
        }
        freeze(false)?;
        send_test_command(&runtime, test_input(&session_id, b"de\r"))?;
        assert_raw_shell_input(root.path(), b"de\r")?;
        runtime.shutdown()?;
        let db = startup::open_read_only(&root.path().join("data/matou.sqlite"))?;
        let card =
            matou_native_core::sessions::get_session(&db, &session_id)?.context("Shell card")?;
        assert_eq!(card.kind, "shell");
        assert_eq!(
            matou_native_core::sessions::list_runs(&db, &session_id)?.len(),
            1
        );
        Ok(())
    }

    #[test]
    fn saving_language_and_time_zone_does_not_unlock_a_frozen_process() -> Result<()> {
        let root = tempfile::tempdir()?;
        let (mut runtime, session_id) = open_raw_test_shell(root.path())?;
        let freeze = |frozen| -> Result<()> {
            let (reply, result) = mpsc::channel();
            send_test_command(
                &runtime,
                Command::TestFreezeShellWrites {
                    session_id: session_id.clone(),
                    frozen,
                    reply,
                },
            )?;
            result.recv_timeout(Duration::from_secs(3))??;
            Ok(())
        };
        freeze(true)?;
        let data = root.path().join("data");
        save_locale_preference(&data, LocalePreference::English)?;
        assert_eq!(read_locale_preference(&data), LocalePreference::English);
        send_test_command(
            &runtime,
            Command::SetTimeZoneSettings {
                request_id: 51,
                time_zone: json!("UTC"),
            },
        )?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Event::TimeZoneSettings(reply) = next_test_event(&runtime, deadline)? {
                assert_eq!(reply.request_id, 51);
                assert_eq!(reply.result.unwrap().time_zone, "UTC");
                break;
            }
        }
        send_test_command(&runtime, test_input(&session_id, b"must remain frozen\r"))?;
        loop {
            if let Event::Diagnostic(error) = next_test_event(&runtime, deadline)? {
                assert_eq!(error, "session output is frozen");
                break;
            }
        }
        freeze(false)?;
        send_test_command(&runtime, test_input(&session_id, b"allowed\r"))?;
        assert_raw_shell_input(root.path(), b"allowed\r")?;
        runtime.shutdown()
    }

    #[test]
    fn storage_pause_drops_input_before_the_shell_tracker() -> Result<()> {
        let root = tempfile::tempdir()?;
        let BootstrapResult::Writable {
            mut connection,
            ownership,
            ..
        } = startup::open(root.path(), "storage-pause")?
        else {
            bail!("test database is not writable");
        };
        let ids = hierarchy_ids();
        hierarchy::bootstrap_window(
            &mut connection,
            &CommandMetadata {
                command_id: id(),
                command_type: "window.bootstrap".into(),
                request_hash: "fixture".into(),
                causation_id: None,
                correlation_id: None,
            },
            &BootstrapWindowInput {
                window_id: "test-window".into(),
                default_root_directory: root.path().to_string_lossy().into_owned(),
                default_name: "Workspace".into(),
                now: 1,
            },
            &ids,
            "默认",
            1,
        )?;
        let shell = root.path().join("fixture-shell");
        std::fs::write(
            &shell,
            "#!/bin/sh\nprintf 'READY\\n'\nwhile IFS= read -r line; do printf '%s\\n' \"$line\"; done\n",
        )?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700))?;
        let mut owner = ShellOwner::new(connection, ownership, id)?;
        let session_id = ids.session_id;
        owner.launch(
            SessionOptions {
                session_id: session_id.clone(),
                launch: ShellLaunch {
                    profile: LaunchProfile::Shell,
                    argv: Vec::new(),
                    executable: shell,
                    cwd: root.path().into(),
                    data_root: root.path().into(),
                    inherited_env: BTreeMap::from([("HOME".into(), root.path().into())]),
                    env: BTreeMap::new(),
                    cols: 80,
                    rows: 24,
                },
                terminal: default_terminal_options(),
                journal: JournalOptions {
                    max_segment_bytes: 512,
                    ..JournalOptions::default()
                },
                restored_history: Vec::new(),
                collect_history: false,
            },
            false,
            "",
        )?;

        fn wait_for_owner_event(
            owner: &mut ShellOwner,
            found: impl Fn(&SessionEvent) -> bool,
        ) -> Result<()> {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                for event in owner.poll(Duration::from_millis(20))? {
                    if let OwnerEvent::Terminal(event) = event
                        && found(&event.event)
                    {
                        return Ok(());
                    }
                }
                ensure!(Instant::now() < deadline, "expected terminal event");
            }
        }
        wait_for_owner_event(&mut owner, |event| {
            matches!(event, SessionEvent::Output { .. })
        })?;
        let range = root
            .path()
            .join("journal")
            .join(&session_id)
            .join("range-index.json");
        std::fs::create_dir(&range)?;
        let (reply, written) = mpsc::channel();
        owner.send(
            &session_id,
            WorkerCommand::Write {
                bytes: format!("{}\n", "x".repeat(2048)).into_bytes(),
                reply,
            },
        )?;
        written.recv_timeout(Duration::from_secs(5))??;
        wait_for_owner_event(&mut owner, |event| {
            matches!(event, SessionEvent::StorageFault { .. })
        })?;

        let (events, _) = async_channel::unbounded();
        let mut promotions = HashMap::new();
        let mut submission = HashMap::new();
        let mut operations = Vec::new();
        let mut send_keys =
            |owner: &mut ShellOwner, operations: &mut Vec<WorkerOperation>, bytes: &[u8]| {
                send_input(
                    owner,
                    PendingInput {
                        workspace_id: ids.workspace_id.clone(),
                        session_id: session_id.clone(),
                        bytes: bytes.to_vec(),
                        defer_ordering: true,
                    },
                    &mut promotions,
                    &mut submission,
                    operations,
                    &events,
                )
            };
        send_keys(&mut owner, &mut operations, b"clau")?;
        let (_, rejected, _) = operations.pop().context("paused write")?;
        let error = rejected.recv_timeout(Duration::from_secs(5))?.unwrap_err();
        assert_eq!(error.to_string(), "session storage is paused");

        std::fs::remove_dir(range)?;
        let (reply, recovered) = mpsc::channel();
        owner.send(&session_id, WorkerCommand::RetryStorage { reply })?;
        recovered.recv_timeout(Duration::from_secs(5))??;
        wait_for_owner_event(&mut owner, |event| {
            matches!(event, SessionEvent::StorageRecovered { .. })
        })?;

        send_keys(&mut owner, &mut operations, b"de\r")?;
        let pending = submission
            .remove(&session_id)
            .context("resumed Shell submission")?;
        assert!(
            pending
                .decision
                .recv_timeout(Duration::from_secs(5))?
                .is_none()
        );
        let (_, written, _) = operations.pop().context("resumed write")?;
        written.recv_timeout(Duration::from_secs(5))??;
        assert!(promotions.is_empty());
        owner.shutdown()?;
        Ok(())
    }

    #[test]
    fn clearing_copy_suppresses_interrupt_and_forwards_the_failed_clear_input() -> Result<()> {
        let root = tempfile::tempdir()?;
        let (mut runtime, session_id) = open_raw_test_shell(root.path())?;
        send_test_command(&runtime, test_input(&session_id, b"cla"))?;
        assert_raw_shell_input(root.path(), b"cla")?;

        // Queue Copy behind the qualifying submission and ahead of the owner's
        // Clearing poll. The failed Ctrl+U keeps the old PTY available for inspection.
        queue_test_batch(
            &runtime,
            [
                Command::TestFailPromotionClear {
                    session_id: session_id.clone(),
                },
                test_input(&session_id, b"ude\r"),
                Command::CopySelection {
                    session_id: session_id.clone(),
                    request_id: 42,
                    interrupt_if_empty: true,
                },
            ],
        )?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match next_test_event(&runtime, deadline)? {
                Event::Copied {
                    request_id: 42,
                    text,
                } => {
                    assert_eq!(text, "");
                    break;
                }
                Event::Diagnostic(error) => bail!("{error}"),
                _ => {}
            }
        }

        assert_raw_shell_input(root.path(), b"claude\r")?;
        runtime.shutdown()?;
        Ok(())
    }

    #[test]
    fn successful_fallback_does_not_reuse_the_submitted_shell_prefix() -> Result<()> {
        let root = tempfile::tempdir()?;
        let (mut runtime, session_id) = open_raw_test_shell(root.path())?;
        send_test_command(&runtime, test_input(&session_id, b"clau"))?;
        assert_raw_shell_input(root.path(), b"clau")?;
        send_test_command(
            &runtime,
            Command::TestFailPromotionClear {
                session_id: session_id.clone(),
            },
        )?;
        send_test_command(&runtime, test_input(&session_id, b"de\r"))?;
        assert_raw_shell_input(root.path(), b"claude\r")?;
        send_test_command(&runtime, test_input(&session_id, b"de\r"))?;
        assert_raw_shell_input(root.path(), b"claude\rde\r")?;
        runtime.shutdown()?;
        let db = startup::open_read_only(&root.path().join("data/matou.sqlite"))?;
        assert_eq!(
            matou_native_core::sessions::list_runs(&db, &session_id)?.len(),
            1
        );
        Ok(())
    }

    #[test]
    fn shutdown_in_clearing_has_the_same_saved_status_as_plain_shell_shutdown() -> Result<()> {
        for clearing in [false, true] {
            let root = tempfile::tempdir()?;
            let (mut runtime, session_id) = open_raw_test_shell(root.path())?;
            if clearing {
                queue_test_batch(
                    &runtime,
                    [test_input(&session_id, b"claude\r"), Command::Shutdown],
                )?;
            }
            runtime.shutdown()?;
            let db = startup::open_read_only(&root.path().join("data/matou.sqlite"))?;
            let (kind, status): (String, String) = db.query_row(
                "SELECT kind, work_status FROM sessions WHERE id=?",
                [&session_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            assert_eq!((kind.as_str(), status.as_str()), ("shell", "exited"));
            assert_eq!(
                matou_native_core::sessions::list_runs(&db, &session_id)?.len(),
                1
            );
        }
        Ok(())
    }

    #[test]
    fn interaction_precedes_queued_focus_and_shutdown() -> Result<()> {
        let root = std::env::temp_dir().join(format!("matou-interaction-order-{}", id()));
        check_interaction_order(&root)
            .with_context(|| format!("isolated bridge data: {}", root.display()))?;
        std::fs::remove_dir_all(root)?;
        Ok(())
    }

    fn two_test_shells(root: &Path) -> Result<(RuntimeHost, String, String, String)> {
        let options = LaunchOptions {
            data_root: root.join("data"),
            workspace: root.join("workspace"),
            shell_home: root.join("home"),
            shell: "/bin/zsh".into(),
        };
        let window_id = "main-window-1";
        let (scene, first, second) = {
            let BootstrapResult::Writable {
                mut connection,
                ownership,
                ..
            } = startup::open(&options.data_root, "seed")?
            else {
                bail!("test database is not writable");
            };
            let original = hierarchy::bootstrap_window(
                &mut connection,
                &CommandMetadata {
                    command_id: id(),
                    command_type: "window.bootstrap".into(),
                    request_hash: "seed".into(),
                    causation_id: None,
                    correlation_id: None,
                },
                &BootstrapWindowInput {
                    window_id: window_id.into(),
                    default_root_directory: options.workspace.to_string_lossy().into_owned(),
                    default_name: "Workspace".into(),
                    now: 1,
                },
                &hierarchy_ids(),
                "默认",
                1,
            )?;
            let scene = original.scene.context("seed scene")?.id;
            let first = original.session.context("seed session")?.id;
            let mut owner = ShellOwner::new(connection, ownership, id)?;
            let (selection, _, _) = navigate(
                &mut owner,
                window_id,
                Command::CreateShellSibling {
                    scene_id: scene.clone(),
                    source_session_id: first.clone(),
                    parent_session_id: None,
                },
            )?;
            (
                scene,
                first,
                selection.session.context("sibling session")?.id,
            )
        };

        let bridge = RuntimeHost::start_with_offline_fixtures(options, OfflineFixtures::default())?;
        let mut ready = HashSet::new();
        let deadline = Instant::now() + Duration::from_secs(8);
        while !ready.contains(&first) || !ready.contains(&second) {
            match next_test_event(&bridge, deadline)? {
                Event::Frame { session_id, .. } => {
                    ready.insert(session_id);
                }
                Event::Diagnostic(message) => bail!("{message}"),
                _ => {}
            }
        }
        Ok((bridge, scene, first, second))
    }

    #[test]
    fn undecided_shell_submission_does_not_block_other_input_focus_or_shutdown() -> Result<()> {
        let root = tempfile::tempdir()?;
        let home = root.path().join("home");
        std::fs::create_dir_all(&home)?;
        std::fs::write(
            home.join(".zshrc"),
            "TRAPHUP() { if [[ $OTHER = 1 ]]; then : > \"$HOME/other-stopping\"; fi; exit 0; }\n",
        )?;
        let (mut bridge, scene, other, blocked) = two_test_shells(root.path())?;
        let pause_worker = |bridge: &RuntimeHost| -> Result<Sender<()>> {
            let (paused, wait_paused) = mpsc::channel();
            let (resume, wait_resume) = mpsc::channel();
            send_test_command(
                bridge,
                Command::TestPauseShellWorker {
                    session_id: blocked.clone(),
                    paused,
                    resume: wait_resume,
                },
            )?;
            wait_paused.recv_timeout(Duration::from_secs(3))?;
            Ok(resume)
        };
        let await_file = |name, expected| -> Result<()> {
            let deadline = Instant::now() + Duration::from_secs(3);
            while std::fs::read_to_string(home.join(name)).ok().as_deref() != Some(expected) {
                ensure!(
                    Instant::now() < deadline,
                    "waiting for Shell to write {name}"
                );
                thread::sleep(Duration::from_millis(2));
            }
            Ok(())
        };

        let resume = pause_worker(&bridge)?;
        for command in [
            test_input(&blocked, b"printf first > \"$HOME/ordered\"\r"),
            test_input(&blocked, b"printf second >> \"$HOME/ordered\"\r"),
            test_input(&other, b"OTHER=1; printf ready > \"$HOME/other\"\r"),
            Command::FocusSession {
                scene_id: scene,
                session_id: other.clone(),
            },
        ] {
            send_test_command(&bridge, command)?;
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match next_test_event(&bridge, deadline)? {
                Event::Navigation { graph, .. } if graph["focusedSessionId"] == other => break,
                Event::NavigationFailed { message, .. } | Event::Diagnostic(message) => {
                    bail!("{message}");
                }
                _ => {}
            }
        }
        await_file("other", "ready")?;
        assert!(!home.join("ordered").exists());
        resume.send(())?;
        await_file("ordered", "firstsecond")?;

        let resume = pause_worker(&bridge)?;
        send_test_command(&bridge, test_input(&blocked, b"printf queued\r"))?;
        send_test_command(&bridge, Command::Shutdown)?;
        // Shutdown must reach the other PTY while the first worker is still paused.
        await_file("other-stopping", "")?;
        resume.send(())?;
        bridge.shutdown()?;
        Ok(())
    }

    fn check_interaction_order(root: &Path) -> Result<()> {
        let database = root.join("data/matou.sqlite");
        let (mut bridge, scene, first, second) = two_test_shells(root)?;
        // Hold the owner at a test barrier so all four messages are in one batch.
        let (paused, wait_paused) = mpsc::channel();
        let (resume, wait_resume) = mpsc::channel();
        bridge
            .commands
            .send(Command::TestPause {
                paused,
                resume: wait_resume,
            })
            .map_err(|_| anyhow!("runtime command channel closed"))?;
        wait_paused.recv_timeout(Duration::from_secs(8))?;
        let input = |session: &str| Command::Input {
            session_id: session.to_owned(),
            bytes: vec![13],
            defer_ordering: true,
        };
        for command in [
            input(&second),
            Command::FocusSession {
                scene_id: scene,
                session_id: first.clone(),
            },
            input(&first),
            Command::Shutdown,
        ] {
            bridge
                .commands
                .send(command)
                .map_err(|_| anyhow!("runtime command channel closed"))?;
        }
        resume.send(())?;
        let events = bridge.events.clone();
        bridge.shutdown()?;

        let focus_graph = std::iter::from_fn(|| events.try_recv().ok())
            .find_map(|event| match event {
                Event::Navigation { graph, .. } => Some(graph),
                _ => None,
            })
            .context("focus navigation graph")?;
        let previous = focus_graph["nodes"]
            .as_array()
            .context("focus navigation nodes")?
            .iter()
            .find(|node| node["sessionId"] == second)
            .context("previously focused Shell")?;
        let connection = startup::open_read_only(&database)?;
        let membership = |session: &str| {
            connection.query_row(
                "SELECT last_user_interaction_seq, pending_user_interaction_seq
                 FROM session_canvas_memberships WHERE session_id = ?",
                [session],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
        };
        let receipts: i64 = connection.query_row(
            "SELECT count(*) FROM command_deduplication WHERE command_type = 'session.user-interaction'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(
            (
                previous["lastUserInteractionSeq"].as_i64(),
                membership(&second)?,
                membership(&first)?,
                receipts
            ),
            (Some(1), (1, 0), (0, 2), 2),
            "the switch must apply the old Shell interaction and shutdown must retain the new one"
        );
        Ok(())
    }
}
