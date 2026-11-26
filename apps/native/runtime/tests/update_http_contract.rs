use std::collections::{BTreeMap, VecDeque};
use std::convert::Infallible;
use std::fs;
use std::future::pending;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Command as ProcessCommand;
use std::sync::{Arc, Mutex, mpsc};
use std::task::{Context, Poll};
use std::thread;
use std::time::Duration;

use anyhow::{Result, bail, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
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
use sha2::{Digest, Sha512};
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
    Found(String),
    Artifact { tampered: bool, stall: bool },
    Bytes(Vec<u8>),
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
            Self::Found(location) => {
                headers = headers.status(StatusCode::FOUND).header(LOCATION, location);
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
            Self::Bytes(bytes) => (VecDeque::from([Bytes::from(bytes)]), false),
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
        Self::start_with_socks(None, route)
    }

    fn start_with_socks(
        socks_port: Option<u16>,
        route: impl Fn(&str) -> Reply + Send + Sync + 'static,
    ) -> Result<Self> {
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
                            let tcp = if let Some(port) = socks_port {
                                let result = tokio::task::spawn_blocking(move || {
                                    let socket = tcp.into_std()?;
                                    accept_fixture_socks5(&socket, port)?;
                                    Ok::<_, io::Error>(socket)
                                })
                                .await;
                                let Ok(Ok(socket)) = result else {
                                    return;
                                };
                                tokio::net::TcpStream::from_std(socket).unwrap()
                            } else {
                                tcp
                            };
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

fn accept_fixture_socks5(mut socket: &std::net::TcpStream, target_port: u16) -> io::Result<()> {
    socket.set_nonblocking(false)?;
    socket.set_read_timeout(Some(WAIT))?;
    socket.set_write_timeout(Some(WAIT))?;
    let mut greeting = [0; 3];
    socket.read_exact(&mut greeting)?;
    assert_eq!(greeting, [5, 1, 0]);
    socket.write_all(&[5, 0])?;
    let mut connect = [0; 10];
    socket.read_exact(&mut connect)?;
    assert_eq!(connect[..8], [5, 1, 0, 1, 192, 0, 2, 1]);
    assert_eq!(connect[8..], target_port.to_be_bytes());
    socket.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0])?;
    socket.set_nonblocking(true)
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
    let arch = if matches!(platform, Platform::Win32) {
        Architecture::X64
    } else {
        Architecture::Arm64
    };
    manager_on(base, channel, platform, arch, current)
}

fn manager_on(
    base: &str,
    channel: &str,
    platform: Platform,
    arch: Architecture,
    current: &str,
) -> UpdateManager {
    UpdateManager::new(
        CurrentVersion::parse(current).unwrap(),
        UpdateOptions {
            feed: FeedConfiguration::resolve(Some(base), None, Some(channel)),
            platform,
            arch,
            enabled: true,
            install_mode: InstallMode::Manual,
            locale: UpdateLocale::English,
        },
    )
    .unwrap()
}

fn windows_installer_bytes(machine: u16) -> Vec<u8> {
    // An inert PE header, with no executable code or version resources.
    let mut bytes = vec![0; ARTIFACT_SIZE];
    bytes[..2].copy_from_slice(b"MZ");
    bytes[60..64].copy_from_slice(&64u32.to_le_bytes());
    bytes[64..68].copy_from_slice(b"PE\0\0");
    bytes[68..70].copy_from_slice(&machine.to_le_bytes());
    bytes[84..86].copy_from_slice(&0xf0u16.to_le_bytes());
    bytes[86..88].copy_from_slice(&2u16.to_le_bytes());
    bytes[88..90].copy_from_slice(&0x20bu16.to_le_bytes());
    bytes
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

fn check_and_download_fixture(base: &str, expected_url: &str) -> Result<()> {
    let update = manager(base, "stable", Platform::Darwin, "1.0.0");
    let fetcher = HttpUpdateFetcher::new(Platform::Darwin)?.with_timeout(Duration::from_secs(3));
    let result = update
        .check(&fetcher, || 1, |_| {}, |_| {})
        .unwrap()
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    assert!(result.is_update_available);
    let UpdatePhase::Available { release } = update.state().phase else {
        bail!("expected available fixture release");
    };
    assert_eq!(release.version, "2.0.0");
    assert_eq!(release.manual_download_url.as_deref(), Some(expected_url));
    let root = tempfile::tempdir()?;
    update.download(
        &fetcher,
        &LocalUpdateFiles::new(root.path().to_owned()),
        || 1,
        |_| {},
    );
    assert!(
        matches!(update.state().phase, UpdatePhase::Downloaded { .. }),
        "{:?}",
        update.state().phase
    );
    assert_eq!(
        fs::read(root.path().join("Matou.dmg"))?,
        vec![b'x'; ARTIFACT_SIZE]
    );
    Ok(())
}

#[test]
fn keeps_relative_assets_on_release_origin_through_two_found_redirects() -> Result<()> {
    let cdn = Server::start(|path| match path {
        "/signed/manifest" => Reply::Text(StatusCode::OK, yaml("2.0.0", "Matou.dmg", SHA512)),
        "/signed/installer" => Reply::Artifact {
            tampered: false,
            stall: false,
        },
        _ => not_found(),
    })?;
    let manifest = cdn.url("/signed/manifest?signature=fixture");
    let installer = cdn.url("/signed/installer?signature=fixture");
    let github = Server::start(move |path| match path {
        "/releases/latest/download/stable-mac.yml" => {
            Reply::Found("/releases/download/v2.0.0/stable-mac.yml".into())
        }
        "/releases/download/v2.0.0/stable-mac.yml" => Reply::Found(manifest.clone()),
        "/releases/download/v2.0.0/Matou.dmg" => Reply::Found(installer.clone()),
        _ => not_found(),
    })?;
    check_and_download_fixture(
        &github.url("/releases/latest/download"),
        &github.url("/releases/download/v2.0.0/Matou.dmg"),
    )?;
    assert_eq!(
        github.paths(),
        [
            "/releases/latest/download/stable-mac.yml",
            "/releases/download/v2.0.0/stable-mac.yml",
            "/releases/download/v2.0.0/Matou.dmg",
        ]
    );
    assert_eq!(
        *cdn.requests.lock().unwrap(),
        [
            "/signed/manifest?signature=fixture",
            "/signed/installer?signature=fixture",
        ]
    );
    Ok(())
}

fn proxy_fixture_response(path: &str) -> Reply {
    match path {
        "/stable/stable-mac.yml" => Reply::Text(StatusCode::OK, yaml("2.0.0", "Matou.dmg", SHA512)),
        "/stable/Matou.dmg" => Reply::Artifact {
            tampered: false,
            stall: false,
        },
        _ => not_found(),
    }
}

fn run_proxy_fixture(test: &str, base: &str, key: &str, proxy: &str) -> Result<()> {
    // Give the fetcher real environment variables without racing parallel tests.
    let mut child = ProcessCommand::new(std::env::current_exe()?);
    for key in [
        "http_proxy",
        "HTTP_PROXY",
        "https_proxy",
        "HTTPS_PROXY",
        "all_proxy",
        "ALL_PROXY",
        "no_proxy",
        "NO_PROXY",
    ] {
        child.env_remove(key);
    }
    let output = child
        .args(["--exact", test, "--nocapture"])
        .env("MATOU_TEST_PROXY_FEED", base)
        .env(key, proxy)
        .output()?;
    ensure!(
        output.status.success(),
        "proxy fixture failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[test]
fn checks_and_downloads_through_http_proxy_and_bypasses_it_for_loopback() -> Result<()> {
    if let Ok(base) = std::env::var("MATOU_TEST_PROXY_FEED") {
        return check_and_download_fixture(&base, &format!("{base}/Matou.dmg"));
    }
    let proxy = Server::start(proxy_fixture_response)?;
    run_proxy_fixture(
        "checks_and_downloads_through_http_proxy_and_bypasses_it_for_loopback",
        "http://release-fixture.invalid/stable",
        "HTTP_PROXY",
        &proxy.url(""),
    )?;
    assert_eq!(
        proxy.paths(),
        [
            "http://release-fixture.invalid/stable/stable-mac.yml",
            "http://release-fixture.invalid/stable/Matou.dmg",
        ]
    );

    let local = Server::start(proxy_fixture_response)?;
    run_proxy_fixture(
        "checks_and_downloads_through_http_proxy_and_bypasses_it_for_loopback",
        &local.url("/stable"),
        "HTTP_PROXY",
        "unsupported://proxy-fixture.invalid",
    )?;
    assert_eq!(
        local.paths(),
        ["/stable/stable-mac.yml", "/stable/Matou.dmg"]
    );
    Ok(())
}

#[test]
fn checks_and_downloads_through_socks5_proxy() -> Result<()> {
    if let Ok(base) = std::env::var("MATOU_TEST_PROXY_FEED") {
        return check_and_download_fixture(&base, &format!("{base}/Matou.dmg"));
    }
    let port = 12345;
    let proxy = Server::start_with_socks(Some(port), proxy_fixture_response)?;
    run_proxy_fixture(
        "checks_and_downloads_through_socks5_proxy",
        &format!("http://192.0.2.1:{port}/stable"),
        "ALL_PROXY",
        &format!("socks5://{}", proxy.addr),
    )?;
    assert_eq!(
        proxy.paths(),
        ["/stable/stable-mac.yml", "/stable/Matou.dmg"]
    );
    Ok(())
}

#[test]
#[ignore = "live GitHub manifest only; run explicitly with --ignored --nocapture"]
fn live_github_release_manifest() -> Result<()> {
    let feed = FeedConfiguration::resolve(None, None, None);
    let unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as u64;
    let url = feed.manifest_url(Platform::Darwin, Architecture::Arm64, unix_ms)?;
    let manifest = HttpUpdateFetcher::new(Platform::Darwin)?
        .check(&url)
        .map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let current = CurrentVersion::parse("0.2.25")?;
    assert!(current.offers(&manifest.version)?);
    let artifact = manifest
        .files
        .iter()
        .find(|file| {
            file.url
                .as_deref()
                .is_some_and(|url| url.ends_with("-mac-arm64.dmg"))
        })
        .ok_or_else(|| anyhow::anyhow!("missing arm64 DMG"))?;
    let artifact_url = artifact.url.as_deref().unwrap();
    let sha512 = artifact
        .sha512
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("missing SHA-512"))?;
    assert_eq!(STANDARD.decode(sha512)?.len(), 64);
    println!(
        "Manifest URL: {url}\nCurrent: 0.2.25\nVersion: {}\nArtifact URL: {artifact_url}\nSize: {:?}\nSHA-512: {sha512}",
        manifest.version, artifact.size
    );
    assert_eq!(manifest.version, "0.2.26");
    assert_eq!(
        artifact_url,
        "https://github.com/icesword0760/matou/releases/download/v0.2.26/Matou-0.2.26-mac-arm64.dmg"
    );
    Ok(())
}

#[test]
fn windows_falls_back_to_json_on_404_and_keeps_the_exe_extension() -> Result<()> {
    let installer = windows_installer_bytes(0x8664);
    let checksum = STANDARD.encode(Sha512::digest(&installer));
    let cdn = Server::start(move |path| match path {
        "/signed/json" => Reply::Text(
            StatusCode::OK,
            json!({"version":"2.0.0", "files":[
                {"url":"Matou.dmg", "sha512":SHA512},
                {"url":"Matou.exe", "sha512":checksum, "size":ARTIFACT_SIZE}
            ]})
            .to_string(),
        ),
        _ => not_found(),
    })?;
    let json_location = cdn.url("/signed/json");
    let server = Server::start(move |path| match path {
        "/stable/release-channel.json" => Reply::Found("/versioned/release-channel.json".into()),
        "/versioned/release-channel.json" => Reply::Found(json_location.clone()),
        "/versioned/Matou.exe" => Reply::Bytes(windows_installer_bytes(0x8664)),
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
    assert_eq!(fs::read(root.path().join("Matou.exe"))?, installer);
    assert_eq!(
        server.paths(),
        [
            "/stable/stable.yml",
            "/stable/release-channel.json",
            "/versioned/release-channel.json",
            "/versioned/Matou.exe"
        ]
    );
    assert_eq!(cdn.paths(), ["/signed/json"]);
    Ok(())
}

#[test]
fn windows_arm64_rejects_a_mismatched_feed_and_wrong_pe_even_at_the_correct_hash() -> Result<()> {
    let wrong_arch = windows_installer_bytes(0xaa64);
    let digest = STANDARD.encode(Sha512::digest(&wrong_arch));
    let server = Server::start(move |path| match path {
        "/wrong-feed/stable.yml" => Reply::Text(
            StatusCode::OK,
            yaml("2.0.0", "Matou-2.0.0-win-x64.exe", &digest),
        ),
        "/arm64/stable.yml" => Reply::Text(
            StatusCode::OK,
            yaml("2.0.0", "Matou-2.0.0-win-arm64.exe", &digest),
        ),
        "/arm64/Matou-2.0.0-win-arm64.exe" => Reply::Bytes(wrong_arch.clone()),
        _ => not_found(),
    })?;
    let fetcher = HttpUpdateFetcher::new(Platform::Win32)?;
    let manager = |base: &str| {
        manager_on(
            &server.url(base),
            "stable",
            Platform::Win32,
            Architecture::Arm64,
            "1.0.0",
        )
    };
    let wrong = manager("/wrong-feed")
        .check(&fetcher, || 1, |_| {}, |_| {})
        .unwrap()
        .unwrap_err();
    assert_eq!(wrong.code, Some("ERR_UPDATER_INVALID_INSTALLER"));
    let update = manager("/arm64");
    assert!(
        update
            .check(&fetcher, || 1, |_| {}, |_| {})
            .unwrap()
            .unwrap()
            .is_update_available
    );
    let root = tempfile::tempdir()?;
    update.download(
        &fetcher,
        &LocalUpdateFiles::new(root.path().to_owned()),
        || 1,
        |_| {},
    );
    assert!(matches!(update.state().phase, UpdatePhase::Error { .. }));
    assert_eq!(fs::read_dir(root.path())?.count(), 0);
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
    // Windows can deliver a loopback refusal after the short stalled-response deadline.
    let error = HttpUpdateFetcher::new(Platform::Darwin)?
        .with_timeout(Duration::from_secs(5))
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
        "provider: generic\nurl: http://127.0.0.1:1/yaml\nchannel: beta\n",
    )?;
    let resolve = |env: &[(&str, &str)]| {
        UpdateSettings::resolve(Some(resources.path()), |key| {
            env.iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| (*value).to_owned())
        })
    };
    let yaml_only = resolve(&[]);
    assert_eq!(yaml_only.feed.channel, "beta");
    assert_eq!(
        yaml_only
            .feed
            .channel_filename(Platform::Darwin, Architecture::Arm64),
        "beta-mac.yml"
    );
    fs::write(
        &version_json,
        json!({"version":"1.2.3", "update":{
            "baseUrl":"http://127.0.0.1:1/json", "channel":"packaged"
        }})
        .to_string(),
    )?;

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

fn host_reply_graph(host: &RuntimeHost, id: u64, kind: &str) -> Result<Value> {
    next_event(host, |event| match event {
        Event::Graph(graph) if graph["type"] == kind && graph["requestId"] == id => Some(graph),
        _ => None,
    })
}

fn host_reply(host: &RuntimeHost, id: u64, kind: &str) -> Result<Value> {
    let graph = host_reply_graph(host, id, kind)?;
    ensure!(
        graph.get("error").is_none() && graph.get("result").is_some(),
        "{graph}"
    );
    Ok(graph["result"].clone())
}

#[test]
fn runtime_host_uses_live_http_and_remains_responsive_during_check() -> Result<()> {
    let gate = Arc::new(Notify::new());
    let server_gate = Arc::clone(&gate);
    let (checking, requested) = mpsc::channel();
    let windows_arm64 = cfg!(all(windows, target_arch = "aarch64"));
    let installer = if windows_arm64 {
        "Matou-99.0.0-win-arm64.exe"
    } else if cfg!(windows) {
        "Matou.exe"
    } else {
        "Matou.dmg"
    };
    let installer_contents = if cfg!(windows) {
        windows_installer_bytes(0x8664)
    } else {
        vec![b'x'; ARTIFACT_SIZE]
    };
    let server_artifact = installer_contents.clone();
    let checksum = STANDARD.encode(Sha512::digest(&installer_contents));
    let server = Server::start(move |path| {
        if path.ends_with(".yml") {
            let _ = checking.send(());
            Reply::Gated(
                Arc::clone(&server_gate),
                yaml("99.0.0", installer, &checksum),
            )
        } else if path.ends_with(installer) && cfg!(windows) {
            Reply::Bytes(server_artifact.clone())
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
        .send(Command::InstallUpdate { request_id: 8 })?;
    assert_eq!(
        host_reply_graph(&host, 8, "updateInstallResult")?["error"],
        "No downloaded update installer is ready to open"
    );

    host.commands
        .send(Command::DownloadUpdate { request_id: 3 })?;
    let downloaded = host_reply(&host, 3, "updateDownloadResult")?;
    if windows_arm64 {
        assert_eq!(downloaded["status"], "error");
        assert!(!options.data_root.join("updates").join(installer).exists());
        host.shutdown()?;
        return Ok(());
    }
    assert_eq!(downloaded["status"], "downloaded");
    assert_eq!(downloaded["version"], "99.0.0");
    assert_eq!(
        fs::read(options.data_root.join("updates").join(installer))?,
        installer_contents
    );
    if cfg!(any(target_os = "macos", windows)) {
        // This host uses the actual HTTP worker and an explicit dry-run opener.
        host.commands
            .send(Command::InstallUpdate { request_id: 9 })?;
        assert_eq!(host_reply(&host, 9, "updateInstallResult")?["ok"], true);
        let path = options.data_root.join("updates").join(installer);
        fs::remove_file(&path)?;
        host.commands
            .send(Command::InstallUpdate { request_id: 10 })?;
        let missing = host_reply_graph(&host, 10, "updateInstallResult")?;
        assert!(
            missing["error"]
                .as_str()
                .unwrap()
                .contains("installer path is unavailable")
        );
        fs::create_dir(&path)?;
        host.commands
            .send(Command::InstallUpdate { request_id: 11 })?;
        assert_eq!(
            host_reply_graph(&host, 11, "updateInstallResult")?["error"],
            "Downloaded installer is not a regular file"
        );
    }
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
