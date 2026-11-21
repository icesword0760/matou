use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::future::{Future, poll_fn};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::task::{Context, Poll, Waker};

use matou_native_core::update_policy::{
    Architecture, CurrentVersion, ErrorStage, FeedConfiguration, Platform, SignatureInspection,
    UpdateLocale, UpdatePreferences, active_session_count, install_mode,
};
use matou_native_runtime::update_download::{
    DownloadError, DownloadResponse, LocalUpdateFiles, Manifest, ManualDownload, PendingUpdateFile,
    Progress, UpdateError, UpdateFetcher, UpdateFileWriter, UpdateInstaller, UpdateManager,
    UpdateOptions, UpdatePhase, UpdateProtection, UpdateState,
};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use url::Url;

const REFERENCE: &str =
    include_str!("../../../../tooling/native-parity/fixtures/update-policy-reference.json");

#[derive(Deserialize)]
struct Reference {
    gate: String,
    defaults: Defaults,
    normalization: Normalization,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Defaults {
    current_version: String,
    candidate: String,
    channel: String,
    simulated_arch: Architecture,
    update_base_url: String,
    locale: UpdateLocale,
}

#[derive(Deserialize)]
struct Normalization {
    clock: Clock,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Clock {
    manifest_unix_ms: u64,
    download_start_ms: u64,
    download_step_ms: u64,
}

#[derive(Deserialize)]
struct Case {
    id: String,
    inventory: Vec<String>,
    input: Value,
    expected: Value,
}

fn decode<T: DeserializeOwned>(value: &Value) -> T {
    serde_json::from_value(value.clone()).expect("fixture input schema")
}

fn optional<T: DeserializeOwned>(value: &Value) -> Option<T> {
    (!value.is_null()).then(|| decode(value))
}

fn values(value: &Value) -> &[Value] {
    value.as_array().expect("fixture array")
}

fn field<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().expect("fixture string")
}

#[derive(Default)]
struct ScriptedFetcher {
    manifest: Manifest,
    attempts: RefCell<VecDeque<Value>>,
    step: RefCell<Value>,
    requests: RefCell<Vec<String>>,
    downloads: RefCell<Vec<Value>>,
    automatic_calls: Cell<usize>,
}

impl ScriptedFetcher {
    fn with_manifest(manifest: Manifest) -> Self {
        Self {
            manifest,
            ..Self::default()
        }
    }
}

impl UpdateFetcher for ScriptedFetcher {
    fn check(&self, url: &Url) -> Result<Manifest, UpdateError> {
        self.requests.borrow_mut().push(url.to_string());
        let Some(attempt) = self.attempts.borrow_mut().pop_front() else {
            return Ok(self.manifest.clone());
        };
        if let Some(error) = attempt["reject"]
            .as_str()
            .or(attempt["eventError"].as_str())
        {
            return Err(error.to_string().into());
        }
        if attempt["notAvailable"] == true {
            return Ok(Manifest {
                version: self.manifest.version.clone(),
                ..Manifest::default()
            });
        }
        assert_eq!(attempt["available"], true);
        Ok(self.manifest.clone())
    }

    fn fetch(&self, request: &ManualDownload) -> io::Result<DownloadResponse> {
        let mut entry = json!({ "url": request.url.as_str() });
        if let Some(sha512) = &request.expected_sha512 {
            entry["expectedSha512"] = json!(sha512);
        }
        if let Some(sha256) = &request.expected_sha256 {
            entry["expectedSha256"] = json!(sha256);
        }
        self.downloads.borrow_mut().push(entry);

        let step = self.step.borrow();
        let status = optional(&step["status"]).unwrap_or(200);
        let body = step["chunks"].as_array().map(|chunks| {
            let mut chunks = chunks
                .iter()
                .map(|chunk| Ok(chunk.as_str().expect("chunk string").as_bytes().to_vec()))
                .collect::<VecDeque<_>>();
            if let Some(error) = step["streamError"].as_str() {
                chunks.push_back(Err(io::Error::other(error)));
            }
            Box::new(ScriptedBody(chunks)) as Box<dyn Read + Send>
        });
        Ok(DownloadResponse {
            status,
            content_length: step["contentLength"].as_u64(),
            body,
        })
    }

    fn download_automatic(
        &self,
        _release: &matou_native_runtime::update_download::Release,
        on_progress: &mut dyn FnMut(Progress),
    ) -> Result<(), DownloadError> {
        self.automatic_calls.set(self.automatic_calls.get() + 1);
        let step = self.step.borrow();
        if let Some(progress) = step["progress"].as_array() {
            for event in progress {
                on_progress(Progress::new(
                    decode(&event["percent"]),
                    decode(&event["transferred"]),
                    decode(&event["total"]),
                    decode(&event["bytesPerSecond"]),
                ));
            }
        }
        if let Some(error) = step["eventError"].as_str() {
            return Err(DownloadError::UpdaterEvent(error.into()));
        }
        if let Some(error) = step["reject"].as_str() {
            return Err(DownloadError::Rejected(error.into()));
        }
        if let Some(error) = step["verificationError"].as_str() {
            return Err(DownloadError::Verification(error.into()));
        }
        assert_eq!(step["complete"], true);
        Ok(())
    }
}

struct ScriptedBody(VecDeque<io::Result<Vec<u8>>>);

impl Read for ScriptedBody {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let Some(chunk) = self.0.pop_front() else {
            return Ok(0);
        };
        let chunk = chunk?;
        let count = chunk.len().min(buffer.len());
        buffer[..count].copy_from_slice(&chunk[..count]);
        if count < chunk.len() {
            self.0.push_front(Ok(chunk[count..].to_vec()));
        }
        Ok(count)
    }
}

#[derive(Default)]
struct ScriptedInstaller {
    root: PathBuf,
    calls: RefCell<Vec<Value>>,
    prepare_error: Option<String>,
    quit_error: Option<String>,
    open_error: Option<String>,
    hold: Cell<bool>,
}

impl ScriptedInstaller {
    async fn wait(&self, error: &Option<String>) -> Result<(), String> {
        poll_fn(|_| {
            if self.hold.get() {
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        })
        .await;
        error.clone().map_or(Ok(()), Err)
    }
}

impl UpdateInstaller for ScriptedInstaller {
    async fn prepare(&self) -> Result<(), String> {
        self.calls.borrow_mut().push(json!(["prepareInstall"]));
        self.wait(&self.prepare_error).await
    }

    fn quit_and_install(&self, silent: bool, relaunch: bool) -> Result<(), String> {
        self.calls
            .borrow_mut()
            .push(json!(["quitAndInstall", silent, relaunch]));
        self.quit_error.clone().map_or(Ok(()), Err)
    }

    async fn open(&self, path: &Path) -> Result<(), String> {
        let path = path.strip_prefix(&self.root).unwrap();
        let path = path
            .iter()
            .map(|part| part.to_str().unwrap())
            .collect::<Vec<_>>()
            .join("/");
        self.calls
            .borrow_mut()
            .push(json!(["openManualInstaller", path]));
        self.wait(&self.open_error).await
    }
}

fn poll(future: std::pin::Pin<&mut impl Future<Output = ()>>) -> Poll<()> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

fn options(defaults: &Defaults, input: &Value, feed: FeedConfiguration) -> UpdateOptions {
    UpdateOptions {
        feed,
        platform: optional(&input["platform"]).unwrap_or(Platform::Darwin),
        arch: optional(&input["arch"]).unwrap_or(defaults.simulated_arch),
        enabled: input["enabled"].as_bool().unwrap_or(true),
        install_mode: optional(&input["mode"]).unwrap_or_default(),
        locale: optional(&input["locale"]).unwrap_or(defaults.locale),
    }
}

fn manager(reference: &Reference, input: &Value) -> UpdateManager {
    let defaults = &reference.defaults;
    let feed = FeedConfiguration::resolve(Some(&defaults.update_base_url), None, None);
    UpdateManager::new(
        CurrentVersion::parse(&defaults.current_version).unwrap(),
        options(defaults, input, feed),
    )
    .unwrap()
}

fn feed_case(reference: &Reference, input: &Value) -> Value {
    let defaults = &reference.defaults;
    let version = input["version"]
        .as_str()
        .unwrap_or(&defaults.current_version);
    let current = match CurrentVersion::parse(version) {
        Ok(current) => current,
        Err(error) => return json!({ "feedCalls": [], "requests": [], "thrown": error }),
    };
    let feed = FeedConfiguration::resolve(
        input["overrideUrl"].as_str(),
        input["packagedConfig"].as_str(),
        input["channel"].as_str().or(Some(&defaults.channel)),
    );
    let initial_preferences = UpdatePreferences::new(&current);
    let mut actual = json!({ "channel": feed.channel, "feedCalls": [feed], "requests": [] });
    let manager = UpdateManager::new(current, options(defaults, input, feed.clone()));
    let preferences = manager
        .as_ref()
        .map(UpdateManager::preferences)
        .unwrap_or(initial_preferences);
    actual["allowDowngrade"] = json!(preferences.allow_downgrade);
    actual["allowPrerelease"] = json!(preferences.allow_prerelease);
    actual["autoDownload"] = json!(preferences.auto_download);
    actual["autoInstallOnAppQuit"] = json!(preferences.auto_install_on_app_quit);

    let manager = match manager {
        Ok(manager) => manager,
        Err(error) => {
            actual["thrown"] = json!(error);
            return actual;
        }
    };
    actual["returnedUrl"] = json!(feed.url);
    let fetcher = ScriptedFetcher::with_manifest(Manifest {
        version: input["candidate"]
            .as_str()
            .unwrap_or(&defaults.candidate)
            .into(),
        ..Manifest::default()
    });
    let mut states = Vec::new();
    let mut sleeps = Vec::new();
    let result = manager
        .check(
            &fetcher,
            || reference.normalization.clock.manifest_unix_ms,
            |ms| sleeps.push(ms),
            |state| states.push(state),
        )
        .unwrap();
    actual["updaterResult"] = match result {
        Ok(result) => json!(result),
        Err(error) => json!({ "error": error }),
    };
    actual["requests"] = json!(fetcher.requests.into_inner());
    actual["states"] = json!(states);
    actual["sleepCallsMs"] = json!(sleeps);
    actual
}

fn mode_case(input: &Value) -> Value {
    let mut calls = 0;
    let mode = install_mode(
        decode(&input["platform"]),
        decode(&input["isPackaged"]),
        || {
            calls += 1;
            decode::<SignatureInspection>(&input["signature"])
        },
    );
    json!({ "installMode": mode, "inspectCalls": calls })
}

fn checkpoint(state: &UpdateState, locale: UpdateLocale) -> Value {
    let mut result = json!({ "state": state });
    if let Some(dialog) = state.error_dialog(locale) {
        result["dialog"] = json!(dialog);
    }
    result
}

fn check_case(reference: &Reference, input: &Value) -> Value {
    let manager = manager(reference, input);
    let fetcher = ScriptedFetcher {
        manifest: optional(&input["release"]).unwrap_or_else(|| Manifest {
            version: reference.defaults.current_version.clone(),
            ..Manifest::default()
        }),
        attempts: RefCell::new(decode(&input["attempts"])),
        ..ScriptedFetcher::default()
    };
    let installer = ScriptedInstaller::default();
    let initial = manager.state();
    let mut states = Vec::new();
    let mut sleeps = Vec::new();
    let mut checkpoints = Vec::new();
    for _ in 0..input["checks"].as_u64().unwrap_or(1) {
        let _ = manager.check(
            &fetcher,
            || reference.normalization.clock.manifest_unix_ms,
            |delay| sleeps.push(delay),
            |state| states.push(state),
        );
        checkpoints.push(checkpoint(&manager.state(), reference.defaults.locale));
    }
    assert!(fetcher.attempts.borrow().is_empty());
    json!({
        "initial": initial, "states": states, "checkpoints": checkpoints,
        "sleepCallsMs": sleeps, "checkCalls": fetcher.requests.borrow().len(),
        "downloadCalls": fetcher.automatic_calls.get(), "installCalls": installer.calls.borrow().len()
    })
}

fn seed_release(
    reference: &Reference,
    manager: &UpdateManager,
    fetcher: &ScriptedFetcher,
    states: &mut Vec<UpdateState>,
) {
    let result = manager
        .check(
            fetcher,
            || reference.normalization.clock.manifest_unix_ms,
            |_| panic!("fixture release must be valid"),
            |state| {
                if !matches!(state.phase, UpdatePhase::Checking { .. }) {
                    states.push(state);
                }
            },
        )
        .unwrap()
        .unwrap();
    assert!(result.is_update_available);
}

fn temp_dir() -> tempfile::TempDir {
    tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR")).unwrap()
}

fn pending_files(path: &Path) -> Vec<String> {
    if !path.exists() {
        return Vec::new();
    }
    let mut names: Vec<_> = path
        .read_dir()
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort_unstable();
    names
}

fn stream_clock(clock: &Clock) -> impl FnMut() -> u64 {
    let mut next = clock.download_start_ms;
    move || {
        let current = next;
        next += clock.download_step_ms;
        current
    }
}

fn download_case(reference: &Reference, input: &Value, manual: bool) -> Value {
    let manager = manager(reference, input);
    let fetcher = ScriptedFetcher::with_manifest(decode(&input["release"]));
    let temp = temp_dir();
    let destination = temp.path().join("pending-updates");
    let files = LocalUpdateFiles::new(destination.clone());
    let installer = ScriptedInstaller {
        root: temp.path().to_path_buf(),
        ..ScriptedInstaller::default()
    };
    let locale = optional(&input["locale"]).unwrap_or(reference.defaults.locale);
    let mut states = Vec::new();
    let mut checkpoints = Vec::new();
    seed_release(reference, &manager, &fetcher, &mut states);

    for step in values(&input["steps"]) {
        if let Some(error) = step["beforeDownloadEvent"].as_str() {
            manager.report_updater_error(error.into(), |state| states.push(state));
        }
        fetcher.step.replace(step.clone());
        manager.download(
            &fetcher,
            &files,
            stream_clock(&reference.normalization.clock),
            |state| states.push(state),
        );
        if manual {
            let state = manager.state();
            let mut point = checkpoint(&state, locale);
            point["files"] = json!(pending_files(&destination));
            checkpoints.push(point);
            if matches!(state.phase, UpdatePhase::Downloaded { .. }) {
                let mut install = std::pin::pin!(
                    manager.install(&installer, |_| panic!("manual install should open"))
                );
                assert!(poll(install.as_mut()).is_ready());
                let calls = installer.calls.borrow();
                let opened = &calls.last().expect("manual installer not called")[1];
                let mut downloads = fetcher.downloads.borrow_mut();
                let entry = downloads.last_mut().expect("download request not made");
                entry["openedPath"] = opened.clone();
                entry["openedContents"] = json!(
                    std::fs::read_to_string(temp.path().join(opened.as_str().unwrap())).unwrap()
                );
            }
        }
    }

    if manual {
        json!({
            "states": states, "downloads": fetcher.downloads.into_inner(),
            "automaticCalls": fetcher.automatic_calls.get(), "checkpoints": checkpoints,
        })
    } else {
        let mut actual =
            json!({ "states": states, "downloadCalls": fetcher.automatic_calls.get() });
        if let Some(dialog) = manager.state().error_dialog(locale) {
            actual["dialog"] = json!(dialog);
        }
        actual
    }
}

fn session_count_case(input: &Value) -> Value {
    let nodes = input["graphs"]
        .as_object()
        .into_iter()
        .flat_map(|graphs| graphs.values())
        .flat_map(|graph| values(&graph["nodes"]));
    let count = active_session_count(nodes.map(|node| {
        (
            field(node, "sessionId"),
            field(node, "workStatus"),
            node.get("archivedAt").is_some(),
        )
    }));
    json!({ "activeSessionCount": count })
}

fn guard_snapshot(
    protection: &UpdateProtection,
    state: &UpdateState,
    active_count: usize,
    calls: usize,
    locale: UpdateLocale,
) -> Value {
    let mut snapshot = json!({
        "trigger": protection.trigger_label(state, locale),
        "open": protection.is_open(), "installCalls": calls
    });
    if protection.is_open() {
        if let Some(prompt) = protection.install_prompt(state, active_count, locale) {
            snapshot["title"] = json!(prompt.title);
            snapshot["actions"] = json!(
                prompt
                    .choices
                    .iter()
                    .map(|choice| json!({ "text": choice.text, "disabled": choice.disabled }))
                    .collect::<Vec<_>>()
            );
            for (key, value) in [
                ("activeSessions", prompt.active_sessions.as_deref()),
                ("idleUpdateKeepsState", prompt.idle_update_keeps_state),
                ("waiting", prompt.waiting),
            ] {
                if let Some(value) = value {
                    snapshot[key] = json!(value);
                }
            }
        } else if let Some(dialog) = state.error_dialog(locale) {
            snapshot["title"] = json!(dialog.title);
            snapshot["error"] = json!(dialog.error);
            snapshot["actions"] = json!(
                dialog
                    .actions
                    .iter()
                    .map(|text| json!({ "text": text, "disabled": false }))
                    .collect::<Vec<_>>()
            );
        }
    }
    if let Some(text) = protection.updated_notice(locale) {
        snapshot["toast"] = json!(text);
    }
    snapshot
}

fn guard_case(reference: &Reference, input: &Value) -> Value {
    let locale = reference.defaults.locale;
    let mut state: UpdateState = decode(&input["state"]);
    let mut protection = UpdateProtection::new(optional(&input["previousVersion"]));
    let timer = protection.accept(&state);
    let mut active_count = optional(&input["activeSessionCount"]).unwrap_or(0);
    let mut calls = 0;
    let mut snapshots = vec![guard_snapshot(
        &protection,
        &state,
        active_count,
        calls,
        locale,
    )];
    for step in values(&input["steps"]) {
        if let Some(click) = step["click"].as_str() {
            let prompt = protection
                .install_prompt(&state, active_count, locale)
                .expect("install controls");
            let choice = prompt
                .choices
                .into_iter()
                .find(|choice| choice.text == click)
                .unwrap_or_else(|| panic!("missing frozen action {click}"));
            if !choice.disabled && protection.press(choice.action, &state, active_count) {
                calls += 1;
            }
        }
        if let Some(count) = optional(&step["activeSessionCount"]) {
            active_count = count;
        }
        if let Some(next) = optional(&step["publish"]) {
            state = next;
            protection.accept(&state);
        }
        if step["settleInstall"] == true {
            protection.settle_install();
        }
        if step["expireToast"] == true {
            protection.expire_updated_notice();
        }
        if protection.on_activity(&state, active_count) {
            calls += 1;
        }
        snapshots.push(guard_snapshot(
            &protection,
            &state,
            active_count,
            calls,
            locale,
        ));
    }
    let mut result = json!({ "snapshots": snapshots });
    if input.get("previousVersion").is_some() {
        result["lastSeenVersion"] = json!(protection.last_seen_version());
        result["initialTimersMs"] = json!(timer.into_iter().collect::<Vec<_>>());
    }
    result
}

// The frozen install-only fake returned an opaque file path immediately. Streaming behavior
// and real filenames are covered by the manual download cases using LocalUpdateFiles.
struct InstallFixtureFile(io::Sink);

impl Write for InstallFixtureFile {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        self.0.write(input)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

impl PendingUpdateFile for InstallFixtureFile {
    fn commit(self) -> io::Result<PathBuf> {
        Ok(PathBuf::from("pending-updates/fixture.dmg"))
    }
}

struct InstallFixtureWriter;

impl UpdateFileWriter for InstallFixtureWriter {
    fn create(&self, _: &str) -> io::Result<impl PendingUpdateFile> {
        Ok(InstallFixtureFile(io::sink()))
    }
}

fn install_case(reference: &Reference, input: &Value) -> Value {
    let manager = manager(reference, input);
    let fetcher = ScriptedFetcher::with_manifest(decode(&input["release"]));
    let mut states = Vec::new();
    seed_release(reference, &manager, &fetcher, &mut states);
    let download = |states: &mut Vec<UpdateState>| {
        fetcher
            .step
            .replace(json!({ "complete": true, "chunks": ["dmg!"], "contentLength": 4 }));
        manager.download(
            &fetcher,
            &InstallFixtureWriter,
            || 0,
            |state| states.push(state),
        );
    };
    if input["initialStatus"] == "downloaded" {
        download(&mut states);
        if input["mode"] == "manual" {
            // The real stream emits an additional 100% tick before the frozen install setup.
            let final_progress = states.remove(states.len() - 2);
            assert!(matches!(
                final_progress.phase, UpdatePhase::Downloading { progress, .. }
                    if progress.percent == 100.0 && progress.transferred_bytes == 4
            ));
        }
    }

    let installer = ScriptedInstaller {
        prepare_error: optional(&input["prepareError"]),
        quit_error: optional(&input["quitError"]),
        open_error: optional(&input["openError"]),
        hold: Cell::new(true),
        ..ScriptedInstaller::default()
    };
    let before = manager.state();
    let during;
    {
        let mut first = std::pin::pin!(manager.install(&installer, |state| states.push(state)));
        let first_poll = poll(first.as_mut());
        during = json!({ "state": manager.state(), "calls": *installer.calls.borrow() });
        if first_poll.is_pending() {
            let mut parallel =
                std::pin::pin!(manager.install(&installer, |_| panic!("duplicate install")));
            let parallel_poll = poll(parallel.as_mut());
            assert_eq!(parallel_poll.is_pending(), input["mode"] == "automatic");
            installer.hold.set(false);
            assert!(poll(first.as_mut()).is_ready());
            if parallel_poll.is_pending() {
                assert!(poll(parallel.as_mut()).is_ready());
            }
        }
    }
    let after = manager.state();
    if input["retry"] == true {
        seed_release(reference, &manager, &fetcher, &mut states);
        download(&mut states);
        let mut retry =
            std::pin::pin!(manager.install(&installer, |_| panic!("prepared again after failure")));
        assert!(poll(retry.as_mut()).is_ready());
    }
    let mut result = json!({
        "before": before, "during": during, "after": after,
        "calls": installer.calls.into_inner(), "states": states
    });
    if let Some(dialog) = after.error_dialog(reference.defaults.locale) {
        result["dialog"] = json!(dialog);
    }
    result
}

fn assert_fixture(case: &str, path: &str, actual: &Value, expected: &Value) {
    match (actual, expected) {
        // The frozen JavaScript output serializes integral floating-point values as integers.
        (Value::Number(left), Value::Number(right)) if left.as_f64() == right.as_f64() => {}
        (Value::Array(left), Value::Array(right)) if left.len() == right.len() => {
            for (index, (left, right)) in left.iter().zip(right).enumerate() {
                assert_fixture(case, &format!("{path}/{index}"), left, right);
            }
        }
        (Value::Object(left), Value::Object(right))
            if left.len() == right.len() && left.keys().all(|key| right.contains_key(key)) =>
        {
            for (key, value) in left {
                assert_fixture(case, &format!("{path}/{key}"), value, &right[key]);
            }
        }
        _ => assert_eq!(actual, expected, "{case} at {path}"),
    }
}

#[test]
fn frozen_update_policy_contract() {
    let reference: Reference = serde_json::from_str(REFERENCE).unwrap();
    assert_eq!(reference.gate, "K-update-policy");
    assert_eq!(reference.cases.len(), 58);
    let mut unique_ids = BTreeSet::new();
    let mut inventory = BTreeMap::new();
    for case in &reference.cases {
        assert!(unique_ids.insert(&case.id));
        for tag in &case.inventory {
            *inventory.entry(tag.as_str()).or_default() += 1;
        }
        let (group, _) = case.id.split_once('-').unwrap();
        let input = &case.input;
        let actual = match (group, case.id.as_str()) {
            ("feed" | "version", _) => feed_case(&reference, input),
            ("mode", _) => mode_case(input),
            ("check", _) => check_case(&reference, input),
            (_, "guard-active-count" | "guard-no-graphs") => session_count_case(input),
            ("guard", _) => guard_case(&reference, input),
            ("install", _) => install_case(&reference, input),
            ("download" | "verify", _) => {
                let manual = input["mode"] == "manual"
                    || values(&input["steps"])
                        .iter()
                        .any(|step| step.get("beforeDownloadEvent").is_some());
                download_case(&reference, input, manual)
            }
            _ => panic!("unhandled fixture case {}", case.id),
        };
        assert_fixture(&case.id, "", &actual, &case.expected);
        println!("{} passed", case.id);
    }
    assert_eq!(
        inventory,
        BTreeMap::from([
            ("PKG-01", 27),
            ("PKG-02", 15),
            ("PKG-03", 12),
            ("PKG-04", 14),
        ])
    );
}

const DMG_SHA256: &str = "a3d2970b4042011c1e275c50e1b39e8f013d7e86b5738c0e454c7c410521d7bf";

#[test]
fn manual_sha256_stream_commits_after_progress_and_integrity_check() {
    let reference: Reference = serde_json::from_str(REFERENCE).unwrap();
    let manager = manager(&reference, &json!({ "mode": "manual" }));
    let temp = temp_dir();
    let files = LocalUpdateFiles::new(temp.path().to_path_buf());
    let fetcher = ScriptedFetcher::with_manifest(decode(&json!({
        "version": "1.3.0",
        "files": [{ "url": "archive%20name.dmg", "size": 4, "sha256": DMG_SHA256 }],
    })));
    seed_release(&reference, &manager, &fetcher, &mut Vec::new());
    fetcher
        .step
        .replace(json!({ "chunks": ["dm", "g!"], "contentLength": 4 }));

    let mut seen = Vec::new();
    manager.download(
        &fetcher,
        &files,
        stream_clock(&reference.normalization.clock),
        |state| {
            if let UpdatePhase::Downloading { progress, .. } = state.phase {
                seen.push(progress.transferred_bytes);
                let names = pending_files(temp.path());
                if progress.transferred_bytes == 2 {
                    assert_eq!(names.len(), 1);
                    assert!(names[0].ends_with(".part"));
                }
            }
        },
    );
    assert_eq!(seen, [0, 2, 4, 4]);
    assert!(matches!(
        manager.state().phase,
        UpdatePhase::Downloaded { .. }
    ));
    assert_eq!(pending_files(temp.path()), ["archive_name.dmg"]);
    assert_eq!(
        std::fs::read(temp.path().join("archive_name.dmg")).unwrap(),
        b"dmg!"
    );
}

#[test]
fn manual_interruption_keeps_previous_file_and_retries_from_zero() {
    let reference: Reference = serde_json::from_str(REFERENCE).unwrap();
    let manager = manager(&reference, &json!({ "mode": "manual" }));
    let temp = temp_dir();
    let destination = temp.path().join("archive.dmg");
    std::fs::write(&destination, b"previous").unwrap();
    let files = LocalUpdateFiles::new(temp.path().to_path_buf());
    let fetcher = ScriptedFetcher::with_manifest(decode(&json!({
        "version": "1.3.0",
        "files": [{ "url": "archive.dmg", "size": 4, "sha256": DMG_SHA256 }],
    })));
    seed_release(&reference, &manager, &fetcher, &mut Vec::new());
    fetcher.step.replace(json!({
        "chunks": ["dm"], "contentLength": 4, "streamError": "socket hang up",
    }));
    manager.download(
        &fetcher,
        &files,
        stream_clock(&reference.normalization.clock),
        |_| {},
    );
    assert!(matches!(
        manager.state().phase,
        UpdatePhase::Error {
            error_stage: ErrorStage::Download,
            ..
        }
    ));
    assert_eq!(pending_files(temp.path()), ["archive.dmg"]);
    assert_eq!(std::fs::read(&destination).unwrap(), b"previous");

    fetcher
        .step
        .replace(json!({ "chunks": ["dm", "g!"], "contentLength": 4 }));
    let mut starts = Vec::new();
    manager.download(
        &fetcher,
        &files,
        stream_clock(&reference.normalization.clock),
        |state| {
            if let UpdatePhase::Downloading { progress, .. } = state.phase {
                starts.push(progress.transferred_bytes);
            }
        },
    );
    assert_eq!(starts.first(), Some(&0));
    assert_eq!(std::fs::read(destination).unwrap(), b"dmg!");
}

#[test]
fn manual_sha256_mismatch_and_short_body_fail_in_download_and_remove_partials() {
    let reference: Reference = serde_json::from_str(REFERENCE).unwrap();
    for (step, message) in [
        (
            json!({ "chunks": ["dm", "g?"], "contentLength": 4 }),
            "更新文件完整性校验失败",
        ),
        (
            json!({ "chunks": ["dm"], "contentLength": 4 }),
            "更新文件下载中断",
        ),
    ] {
        let manager = manager(&reference, &json!({ "mode": "manual" }));
        let temp = temp_dir();
        let files = LocalUpdateFiles::new(temp.path().to_path_buf());
        let fetcher = ScriptedFetcher::with_manifest(decode(&json!({
            "version": "1.3.0",
            "files": [{ "url": "archive.dmg", "size": 4, "sha256": DMG_SHA256 }],
        })));
        seed_release(&reference, &manager, &fetcher, &mut Vec::new());
        fetcher.step.replace(step);
        manager.download(
            &fetcher,
            &files,
            stream_clock(&reference.normalization.clock),
            |_| {},
        );
        assert!(matches!(
            manager.state().phase, UpdatePhase::Error { error_stage: ErrorStage::Download, error_message, .. }
                if error_message == message
        ));
        assert!(pending_files(temp.path()).is_empty());
    }
}

#[test]
fn explicitly_classified_verification_failure_is_verify_even_without_signature_text() {
    let reference: Reference = serde_json::from_str(REFERENCE).unwrap();
    let manager = manager(&reference, &Value::Null);
    let fetcher = ScriptedFetcher::with_manifest(Manifest {
        version: reference.defaults.candidate.clone(),
        ..Manifest::default()
    });
    seed_release(&reference, &manager, &fetcher, &mut Vec::new());
    fetcher
        .step
        .replace(json!({ "verificationError": "identity mismatch" }));
    manager.download(&fetcher, &InstallFixtureWriter, || 0, |_| {});
    assert!(matches!(
        manager.state().phase,
        UpdatePhase::Error {
            error_stage: ErrorStage::Verify,
            ..
        }
    ));
}
