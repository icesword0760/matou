use std::collections::{BTreeMap, VecDeque};
use std::convert::Infallible;
use std::fs;
use std::future::pending;
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex, mpsc};
use std::task::{Context, Poll};
use std::thread;
use std::time::Duration;

use anyhow::{Result, bail, ensure};
use hyper::body::{Body, Bytes, Frame};
use hyper::header::LOCATION;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Response, StatusCode};
use hyper_util::rt::TokioIo;
use matou_native_core::update_policy::{
    Architecture, CurrentVersion, DEFAULT_UPDATE_BASE_URL, ErrorStage, FeedConfiguration,
    InstallMode, Platform, UpdateLocale,
};
use matou_native_runtime::host::{Command, Event, LaunchOptions, OfflineFixtures, RuntimeHost};
use matou_native_runtime::update_download::{
    LocalUpdateFiles, UpdateFetcher, UpdateManager, UpdateOptions, UpdatePhase,
};
use matou_native_runtime::update_http::{HttpUpdateFetcher, UpdateSettings};
use serde_json::{Value, json};
use tokio::runtime::Builder;
use tokio::sync::{Notify, oneshot};

const SHA512: &str =
    "226n67AXqbo23wbzGrHaqfAc2dLAFx50ILoQqkvNZ72ehD2GnTYrpqbT8PHQPgKZhG8DA7BOhesmrJnV+GWCnQ==";
const ARTIFACT_SIZE: usize = 3 * 65_536 + 5;
const WAIT: Duration = Duration::from_secs(10);

struct TestBody {
    chunks: VecDeque<Bytes>,
    stall: bool,
}

impl Body for TestBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        if let Some(bytes) = self.chunks.pop_front() {
            Poll::Ready(Some(Ok(Frame::data(bytes))))
        } else if self.stall {
            Poll::Pending
        } else {
            Poll::Ready(None)
        }
    }
}

enum Reply {
    Text(StatusCode, String),
    Redirect(&'static str),
    Artifact { tampered: bool, stall: bool },
    NoHeaders,
    Gated(Arc<Notify>, String),
}

impl Reply {
    async fn response(self) -> Response<TestBody> {
        let mut headers = Response::builder();
        let (chunks, stall) = match self {
            Self::Text(status, text) => {
                headers = headers.status(status);
                (VecDeque::from([Bytes::from(text)]), false)
            }
            Self::Redirect(location) => {
                headers = headers
                    .status(StatusCode::TEMPORARY_REDIRECT)
                    .header(LOCATION, location);
                (VecDeque::new(), false)
            }
            Self::Artifact { tampered, stall } => {
                let mut chunks = VecDeque::from([Bytes::from(vec![b'x'; 65_536])]);
                if !stall {
                    chunks.push_back(Bytes::from(vec![
                        if tampered { b'y' } else { b'x' };
                        ARTIFACT_SIZE - 65_536
                    ]));
                }
                (chunks, stall)
            }
            Self::NoHeaders => pending().await,
            Self::Gated(gate, text) => {
                gate.notified().await;
                (VecDeque::from([Bytes::from(text)]), false)
            }
        };
        headers.body(TestBody { chunks, stall }).unwrap()
    }
}

struct Server {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<String>>>,
    stop: Option<oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Server {
    fn start(route: impl Fn(&str) -> Reply + Send + Sync + 'static) -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let runtime = Builder::new_current_thread().enable_all().build()?;
        let requests = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&requests);
        let (stop, stopped) = oneshot::channel();
        let route = Arc::new(route);
        let thread = thread::spawn(move || {
            runtime.block_on(async {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                let accept = tokio::spawn(async move {
                    while let Ok((tcp, _)) = listener.accept().await {
                        let route = Arc::clone(&route);
                        let log = Arc::clone(&log);
                        tokio::spawn(async move {
                            let service = service_fn(move |request: hyper::Request<_>| {
                                let uri = request.uri();
                                log.lock().unwrap().push(uri.to_string());
                                let response = route(uri.path());
                                async move { Ok::<_, Infallible>(response.response().await) }
                            });
                            let _ = http1::Builder::new()
                                .serve_connection(TokioIo::new(tcp), service)
                                .await;
                        });
                    }
                });
                let _ = stopped.await;
                accept.abort();
            });
        });
        Ok(Self {
            addr,
            requests,
            stop: Some(stop),
            thread: Some(thread),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    fn paths(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|request| request.split('?').next().unwrap().to_owned())
            .collect()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.stop.take().unwrap().send(());
        self.thread.take().unwrap().join().unwrap();
    }
}

fn not_found() -> Reply {
    Reply::Text(StatusCode::NOT_FOUND, "not found".into())
}

fn yaml(version: &str, artifact: &str, checksum: &str) -> String {
    format!(
        "version: '{version}'\nfiles:\n- url: auxiliary.zip\n  sha512: ignored\n- url: {artifact}\n  sha512: {checksum}\n  size: {ARTIFACT_SIZE}\npath: auxiliary.zip\nsha512: ignored\nreleaseDate: '2026-11-22T00:00:00Z'\nreleaseNotes: |-\n  First change\n  Second change\n"
    )
}

fn manager(base: &str, channel: &str, platform: Platform, current: &str) -> UpdateManager {
    UpdateManager::new(
        CurrentVersion::parse(current).unwrap(),
        UpdateOptions {
            feed: FeedConfiguration::resolve(Some(base), None, Some(channel)),
            platform,
            arch: Architecture::Arm64,
            enabled: true,
            install_mode: InstallMode::Manual,
            locale: UpdateLocale::English,
        },
    )
    .unwrap()
}

#[test]
fn checks_frozen_yaml_without_offering_equal_or_older_versions() -> Result<()> {
    let server = Server::start(|path| match path {
        "/stable/preview-mac.yml" => {
            Reply::Text(StatusCode::OK, yaml("2.0.0", "Matou.dmg", SHA512))
        }
        _ => not_found(),
    })?;
    let fetcher = HttpUpdateFetcher::new(Platform::Darwin)?;
    for current in ["2.0.0", "3.0.0"] {
        let update = manager(&server.url("/stable"), "preview", Platform::Darwin, current);
        let result = update
            .check(&fetcher, || 42, |_| {}, |_| {})
            .unwrap()
            .unwrap();
        assert!(!result.is_update_available);
        assert_eq!(result.update_info.version, "2.0.0");
        assert!(matches!(update.state().phase, UpdatePhase::NotAvailable));
    }
    assert_eq!(
        *server.requests.lock().unwrap(),
        ["/stable/preview-mac.yml?noCache=1a"; 2]
    );
    Ok(())
}

#[test]
fn checks_yaml_and_streams_verified_mac_installer_through_relative_redirects() -> Result<()> {
    let server = Server::start(|path| match path {
        "/stable/stable-mac.yml" => Reply::Redirect("../mirror/channel.yml"),
        "/mirror/channel.yml" => Reply::Text(StatusCode::OK, yaml("2.0.0", "Matou.dmg", SHA512)),
        "/mirror/Matou.dmg" => Reply::Redirect("/artifacts/current"),
        "/artifacts/current" => Reply::Artifact {
            tampered: false,
            stall: false,
        },
        _ => not_found(),
    })?;
    let update = manager(&server.url("/stable"), "stable", Platform::Darwin, "1.0.0");
    let fetcher = HttpUpdateFetcher::new(Platform::Darwin)?;
    let result = update
        .check(&fetcher, || 1, |_| {}, |_| {})
        .unwrap()
        .unwrap();
    assert!(result.is_update_available);
    let UpdatePhase::Available { release } = update.state().phase else {
        bail!("expected available release");
    };
    assert_eq!(release.version, "2.0.0");
    assert_eq!(release.release_notes, ["First change", "Second change"]);
    assert_eq!(release.size_bytes, Some(ARTIFACT_SIZE as u64));
    assert_eq!(
        release.manual_download_url,
        Some(server.url("/mirror/Matou.dmg"))
    );

    let root = tempfile::tempdir()?;
    let files = LocalUpdateFiles::new(root.path().to_owned());
    let mut phases = Vec::new();
    update.download(&fetcher, &files, || 1, |state| phases.push(state.phase));
    assert!(
        matches!(&phases[0], UpdatePhase::Downloading { progress, .. } if progress.percent == 0.0)
    );
    assert!(
        matches!(&phases[1], UpdatePhase::Downloading { progress, .. } if progress.percent == 100.0 && progress.transferred_bytes == ARTIFACT_SIZE as u64)
    );
    assert!(matches!(
        phases.last(),
        Some(UpdatePhase::Downloaded { .. })
    ));
    assert_eq!(
        fs::read(root.path().join("Matou.dmg"))?,
        vec![b'x'; ARTIFACT_SIZE]
    );
    assert_eq!(fs::read_dir(root.path())?.count(), 1);
    assert_eq!(
        server.paths(),
        [
            "/stable/stable-mac.yml",
            "/mirror/channel.yml",
            "/mirror/Matou.dmg",
            "/artifacts/current"
        ]
    );
    Ok(())
}

#[test]
fn windows_falls_back_to_json_on_404_and_keeps_the_exe_extension() -> Result<()> {
    let server = Server::start(|path| match path {
        "/stable/release-channel.json" => Reply::Text(
            StatusCode::OK,
            json!({"version":"2.0.0", "files":[
                {"url":"Matou.dmg", "sha512":SHA512},
                {"url":"Matou.exe", "sha512":SHA512, "size":ARTIFACT_SIZE}
            ]})
            .to_string(),
        ),
        "/stable/Matou.exe" => Reply::Artifact {
            tampered: false,
            stall: false,
        },
        _ => not_found(),
    })?;
    let root = tempfile::tempdir()?;
    let update = manager(&server.url("/stable"), "stable", Platform::Win32, "1.0.0");
    let fetcher = HttpUpdateFetcher::new(Platform::Win32)?;
    assert!(
        update
            .check(&fetcher, || 1, |_| {}, |_| {})
            .unwrap()
            .unwrap()
            .is_update_available
    );
    update.download(
        &fetcher,
        &LocalUpdateFiles::new(root.path().to_owned()),
        || 1,
        |_| {},
    );
    assert!(matches!(
        update.state().phase,
        UpdatePhase::Downloaded { .. }
    ));
    assert_eq!(
        fs::read(root.path().join("Matou.exe"))?,
        vec![b'x'; ARTIFACT_SIZE]
    );
    assert_eq!(
        server.paths(),
        [
            "/stable/stable.yml",
            "/stable/release-channel.json",
            "/stable/Matou.exe"
        ]
    );
    Ok(())
}

#[test]
fn rejects_tampering_missing_sha512_and_stalled_or_failed_artifacts() -> Result<()> {
    let server = Server::start(|path| match path {
        "/tampered/stable-mac.yml" | "/stalled/stable-mac.yml" | "/failed/stable-mac.yml" => {
            Reply::Text(StatusCode::OK, yaml("2.0.0", "Matou.dmg", SHA512))
        }
        "/missing/stable-mac.yml" => Reply::Text(StatusCode::OK, yaml("2.0.0", "Matou.dmg", "")),
        "/tampered/Matou.dmg" => Reply::Artifact {
            tampered: true,
            stall: false,
        },
        "/stalled/Matou.dmg" => Reply::Artifact {
            tampered: false,
            stall: true,
        },
        "/failed/Matou.dmg" => Reply::Text(StatusCode::INTERNAL_SERVER_ERROR, "down".into()),
        _ => not_found(),
    })?;
    let fetcher =
        HttpUpdateFetcher::new(Platform::Darwin)?.with_timeout(Duration::from_millis(250));
    let root = tempfile::tempdir()?;
    for (route, error) in [
        ("tampered", "integrity check failed"),
        ("missing", "missing a SHA-512 checksum"),
        ("stalled", "ETIMEDOUT"),
        ("failed", "HTTP 500"),
    ] {
        let update = manager(
            &server.url(&format!("/{route}")),
            "stable",
            Platform::Darwin,
            "1.0.0",
        );
        let directory = root.path().join(route);
        fs::create_dir(&directory)?;
        update
            .check(&fetcher, || 1, |_| {}, |_| {})
            .unwrap()
            .unwrap();
        let files = LocalUpdateFiles::new(directory.clone());
        update.download(&fetcher, &files, || 1, |_| {});
        assert!(
            matches!(update.state().phase, UpdatePhase::Error {
                error_stage: ErrorStage::Download, ref error_message, ..
            } if error_message.contains(error)),
            "{route}: {:?}",
            update.state()
        );
        assert_eq!(fs::read_dir(directory)?.count(), 0, "{route}");
    }
    assert!(
        !server
            .paths()
            .iter()
            .any(|path| path == "/missing/Matou.dmg")
    );
    Ok(())
}

#[test]
fn classifies_http_status_timeout_connection_refusal_and_redirect_errors() -> Result<()> {
    let server = Server::start(|path| match path {
        "/500/stable-mac.yml" => Reply::Text(StatusCode::INTERNAL_SERVER_ERROR, "down".into()),
        "/no-headers/stable-mac.yml" => Reply::NoHeaders,
        "/redirect/stable-mac.yml" => Reply::Redirect("/redirect/stable-mac.yml"),
        "/scheme/stable-mac.yml" => Reply::Redirect("file:///forbidden"),
        _ => not_found(),
    })?;
    let fetcher =
        HttpUpdateFetcher::new(Platform::Darwin)?.with_timeout(Duration::from_millis(250));
    for (route, code, message) in [
        ("404", "ERR_UPDATER_HTTP_STATUS", "HTTP 404"),
        ("500", "ERR_UPDATER_HTTP_STATUS", "HTTP 500"),
        ("no-headers", "ETIMEDOUT", "timed out"),
        ("redirect", "ERR_UPDATER_REDIRECT", "Too many"),
        ("scheme", "ERR_INVALID_URL", "Invalid update HTTP URL"),
    ] {
        let url = server.url(&format!("/{route}/stable-mac.yml")).parse()?;
        let error = fetcher.check(&url).unwrap_err();
        assert_eq!(error.code, Some(code), "{error:?}");
        assert!(error.message.contains(message), "{error:?}");
    }
    let paths = server.paths();
    assert!(paths.iter().any(|path| path == "/404/release-channel.json"));
    assert!(!paths.iter().any(|path| path == "/500/release-channel.json"));
    assert_eq!(
        paths
            .iter()
            .filter(|path| *path == "/redirect/stable-mac.yml")
            .count(),
        6
    );

    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let addr = listener.local_addr()?;
    drop(listener);
    let error = fetcher
        .check(&format!("http://{addr}/stable-mac.yml").parse()?)
        .unwrap_err();
    assert_eq!(error.code, Some("ECONNREFUSED"), "{error:?}");

    let update = manager(&server.url("/500"), "stable", Platform::Darwin, "1.0.0");
    let mut retries = Vec::new();
    let result = update.check(&fetcher, || 1, |delay| retries.push(delay), |_| {});
    assert!(result.unwrap().is_err());
    assert_eq!(retries, [600, 1600]);
    assert!(matches!(update.state().phase, UpdatePhase::Error {
        error_stage: ErrorStage::Check, ref error_message, ..
    } if error_message.contains("HTTP 500")));
    Ok(())
}

#[test]
fn resolves_update_source_version_channel_and_disable_in_frozen_order() -> Result<()> {
    let resources = tempfile::tempdir()?;
    let app_update = resources.path().join("app-update.yml");
    let version_json = resources.path().join("version.json");
    fs::write(
        &app_update,
        "provider: generic\nurl: http://127.0.0.1:1/yaml\nchannel: ignored\n",
    )?;
    fs::write(
        &version_json,
        json!({"version":"1.2.3", "update":{
            "baseUrl":"http://127.0.0.1:1/json", "channel":"packaged"
        }})
        .to_string(),
    )?;
    let resolve = |env: &[(&str, &str)]| {
        UpdateSettings::resolve(Some(resources.path()), |key| {
            env.iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| (*value).to_owned())
        })
    };

    let override_ = resolve(&[
        ("MATOU_UPDATE_BASE_URL", "  http://127.0.0.1:1/env  "),
        ("MATOU_UPDATE_CHANNEL", "preview"),
        ("MATOU_DISABLE_UPDATES", "1"),
    ]);
    assert_eq!(override_.feed.url, "http://127.0.0.1:1/env");
    assert_eq!(override_.feed.channel, "preview");
    assert!(!override_.enabled);
    assert_eq!(override_.version.as_deref(), Some("1.2.3"));

    let yaml = resolve(&[("MATOU_UPDATE_BASE_URL", "   ")]);
    assert_eq!(yaml.feed.url, "http://127.0.0.1:1/yaml");
    assert_eq!(yaml.feed.channel, "packaged");
    assert!(yaml.enabled);

    fs::write(
        app_update,
        "provider: other\nurl: http://127.0.0.1:1/ignored\n",
    )?;
    assert_eq!(resolve(&[]).feed.url, "http://127.0.0.1:1/json");
    fs::remove_file(version_json)?;
    let defaults = resolve(&[]);
    assert_eq!(defaults.feed.url, DEFAULT_UPDATE_BASE_URL);
    assert_eq!(defaults.feed.channel, "stable");
    assert!(defaults.enabled);
    assert!(!resolve(&[("MATOU_DISABLE_AUTO_UPDATE", "1")]).enabled);
    assert!(!UpdateSettings::resolve(None, |_| None).enabled);
    Ok(())
}

fn next_event<T>(host: &RuntimeHost, mut take: impl FnMut(Event) -> Option<T>) -> Result<T> {
    let runtime = Builder::new_current_thread().enable_time().build()?;
    runtime.block_on(async {
        tokio::time::timeout(WAIT, async {
            loop {
                let event = host.events.recv().await?;
                ensure!(
                    !matches!(event, Event::Closed),
                    "Runtime closed before replying"
                );
                if let Event::Diagnostic(message) = &event {
                    eprintln!("{message}");
                }
                if let Some(item) = take(event) {
                    return Ok(item);
                }
            }
        })
        .await?
    })
}

fn host_reply(host: &RuntimeHost, id: u64, kind: &str) -> Result<Value> {
    next_event(host, |event| match event {
        Event::Graph(graph) if graph["type"] == kind && graph["requestId"] == id => Some(graph),
        _ => None,
    })
    .and_then(|graph| {
        ensure!(
            graph.get("error").is_none() && graph.get("result").is_some(),
            "{graph}"
        );
        Ok(graph["result"].clone())
    })
}

#[test]
fn runtime_host_uses_live_http_and_remains_responsive_during_check() -> Result<()> {
    let gate = Arc::new(Notify::new());
    let server_gate = Arc::clone(&gate);
    let (checking, requested) = mpsc::channel();
    let installer = if cfg!(windows) {
        "Matou.exe"
    } else {
        "Matou.dmg"
    };
    let server = Server::start(move |path| {
        if path.ends_with(".yml") {
            let _ = checking.send(());
            Reply::Gated(Arc::clone(&server_gate), yaml("99.0.0", installer, SHA512))
        } else if path.ends_with(installer) {
            Reply::Artifact {
                tampered: false,
                stall: false,
            }
        } else {
            not_found()
        }
    })?;
    let root = tempfile::tempdir()?;
    let options = LaunchOptions {
        data_root: root.path().join("data"),
        workspace: root.path().join("workspace"),
        shell_home: root.path().join("home"),
        shell: PathBuf::from(if cfg!(windows) { "cmd.exe" } else { "/bin/sh" }),
    };
    fs::create_dir(&options.workspace)?;
    fs::create_dir(&options.shell_home)?;
    let mut host = RuntimeHost::start_with_offline_fixtures(
        options.clone(),
        OfflineFixtures {
            // Supply environment with the existing injection point. Update data comes from HTTP.
            provider_environment: Some(BTreeMap::from([
                ("MATOU_UPDATE_BASE_URL".into(), server.url("/stable").into()),
                ("MATOU_UPDATE_CHANNEL".into(), "preview".into()),
            ])),
            ..Default::default()
        },
    )?;
    next_event(&host, |event| {
        matches!(event, Event::Ready { .. }).then_some(())
    })?;

    host.commands
        .send(Command::CheckForUpdate { request_id: 1 })?;
    requested.recv_timeout(WAIT)?;
    host.commands
        .send(Command::GetUpdateStatus { request_id: 2 })?;
    assert_eq!(
        host_reply(&host, 2, "updateStatusResult")?["status"],
        "checking"
    );
    gate.notify_one();
    let checked = host_reply(&host, 1, "updateCheckResult")?;
    assert_eq!(checked["status"], "available");
    assert_eq!(checked["version"], "99.0.0");
    assert_eq!(
        checked["manualDownloadUrl"],
        server.url(&format!("/stable/{installer}"))
    );

    host.commands
        .send(Command::DownloadUpdate { request_id: 3 })?;
    let downloaded = host_reply(&host, 3, "updateDownloadResult")?;
    assert_eq!(downloaded["status"], "downloaded");
    assert_eq!(downloaded["version"], "99.0.0");
    assert_eq!(
        fs::read(options.data_root.join("updates").join(installer))?,
        vec![b'x'; ARTIFACT_SIZE]
    );
    let manifest_path = if cfg!(target_os = "macos") {
        "/stable/preview-mac.yml"
    } else if cfg!(windows) {
        "/stable/preview.yml"
    } else if cfg!(target_arch = "aarch64") {
        "/stable/preview-linux-arm64.yml"
    } else {
        "/stable/preview-linux.yml"
    };
    assert_eq!(
        server.paths(),
        [manifest_path, &format!("/stable/{installer}")]
    );
    host.shutdown()?;

    let mut disabled = RuntimeHost::start_with_offline_fixtures(
        options,
        OfflineFixtures {
            provider_environment: Some(BTreeMap::from([
                (
                    "MATOU_UPDATE_BASE_URL".into(),
                    server.url("/disabled").into(),
                ),
                ("MATOU_DISABLE_UPDATES".into(), "1".into()),
            ])),
            ..Default::default()
        },
    )?;
    next_event(&disabled, |event| {
        matches!(event, Event::Ready { .. }).then_some(())
    })?;
    disabled
        .commands
        .send(Command::CheckForUpdate { request_id: 4 })?;
    assert_eq!(
        host_reply(&disabled, 4, "updateCheckResult")?["status"],
        "idle"
    );
    assert!(
        !server
            .paths()
            .iter()
            .any(|path| path.starts_with("/disabled"))
    );
    disabled.shutdown()?;
    Ok(())
}
