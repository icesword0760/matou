use std::collections::BTreeMap;
use std::error::Error;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use http_body_util::{BodyExt, Empty};
use hyper::body::{Body, Bytes};
use hyper::header::{CONTENT_LENGTH, HOST, LOCATION};
use hyper::{Request, Response, StatusCode, Uri, body::Incoming};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use hyper_util::client::legacy::{Client, connect::HttpConnector};
use hyper_util::rt::TokioExecutor;
use matou_native_core::update_policy::{
    FeedConfiguration, Platform, parse_packaged_base_url, update_checks_enabled,
};
use serde::Deserialize;
use tokio::runtime::{Builder, Runtime};
use tokio::time::Instant;
use url::Url;

use crate::egress_probe::read_system_proxy;
use crate::http_proxy::{AbortOnDrop, ProxyEnvironment, connect_proxy};
use crate::update_download::{
    DownloadError, DownloadResponse, Manifest, ManifestFile, ManualDownload, Progress, Release,
    UpdateError, UpdateFetcher,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_REDIRECTS: usize = 5;
const MANIFEST_LIMIT: u64 = 1024 * 1024;

#[derive(Default, Deserialize)]
struct VersionMetadata {
    version: Option<String>,
    #[serde(default)]
    update: PackagedUpdate,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PackagedUpdate {
    base_url: Option<String>,
    channel: Option<String>,
}

pub struct UpdateSettings {
    pub feed: FeedConfiguration,
    pub version: Option<String>,
    pub enabled: bool,
}

impl UpdateSettings {
    pub fn resolve(resources: Option<&Path>, env: impl Fn(&str) -> Option<String>) -> Self {
        let packaged_config =
            resources.and_then(|dir| std::fs::read_to_string(dir.join("app-update.yml")).ok());
        let metadata: VersionMetadata = resources
            .and_then(|dir| std::fs::read(dir.join("version.json")).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let override_url = if cfg!(all(windows, target_arch = "aarch64")) {
            env("MATOU_WIN_ARM64_UPDATE_BASE_URL")
                .filter(|url| !url.trim().is_empty())
                .or_else(|| env("MATOU_UPDATE_BASE_URL"))
        } else {
            env("MATOU_UPDATE_BASE_URL")
        };
        let packaged_url = packaged_config.as_deref().and_then(parse_packaged_base_url);
        let packaged_channel = packaged_config
            .as_deref()
            .filter(|_| packaged_url.is_some())
            .and_then(|config| {
                config.lines().find_map(|line| {
                    line.strip_prefix("channel:")
                        .map(str::trim)
                        .filter(|channel| !channel.is_empty())
                })
            });
        let channel = env("MATOU_UPDATE_CHANNEL").or(metadata.update.channel);
        let channel = channel.as_deref().or(packaged_channel);
        let override_url = override_url.as_deref().filter(|url| !url.trim().is_empty());
        let url = override_url
            .or(packaged_url)
            .or(metadata.update.base_url.as_deref());
        let feed = FeedConfiguration::resolve(url, None, channel);
        let enabled = update_checks_enabled(
            resources.is_some_and(Path::is_dir) || override_url.is_some(),
            env("MATOU_DISABLE_UPDATES").as_deref(),
        ) && env("MATOU_DISABLE_AUTO_UPDATE").as_deref() != Some("1");
        Self {
            feed,
            version: metadata.version,
            enabled,
        }
    }

    pub(crate) fn for_executable(
        executable: Option<&Path>,
        env: impl Fn(&str) -> Option<String>,
    ) -> Self {
        let resources = executable.and_then(packaged_resources);
        let settings = Self::resolve(resources.as_deref(), env);
        #[cfg(target_os = "macos")]
        if let Some(version) = resources
            .as_deref()
            .and_then(Path::parent)
            .and_then(bundle_version)
        {
            return Self {
                version: Some(version),
                ..settings
            };
        }
        settings
    }
}

fn packaged_resources(executable: &Path) -> Option<PathBuf> {
    let directory = executable.parent()?;
    #[cfg(target_os = "macos")]
    if directory.file_name().is_some_and(|name| name == "MacOS") {
        return Some(directory.parent()?.join("Resources"));
    }
    Some(directory.join("resources"))
}

#[cfg(target_os = "macos")]
fn bundle_version(contents: &Path) -> Option<String> {
    let info = contents.join("Info.plist");
    if !info.is_file() {
        return None;
    }
    let output = std::process::Command::new("/usr/bin/plutil")
        .args([
            "-extract",
            "CFBundleShortVersionString",
            "raw",
            "-expect",
            "string",
            "-o",
            "-",
            "--",
        ])
        .arg(info)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().into())
}

#[derive(Clone)]
struct RequestError {
    code: &'static str,
    kind: io::ErrorKind,
    message: String,
}

impl RequestError {
    fn new(code: &'static str, kind: io::ErrorKind, message: impl Into<String>) -> Self {
        Self {
            code,
            kind,
            message: message.into(),
        }
    }

    fn timeout() -> Self {
        Self::new(
            "ETIMEDOUT",
            io::ErrorKind::TimedOut,
            "Update request timed out (ETIMEDOUT)",
        )
    }

    fn status(status: StatusCode) -> Self {
        Self::new(
            "ERR_UPDATER_HTTP_STATUS",
            io::ErrorKind::Other,
            format!("Update server returned HTTP {}", status.as_u16()),
        )
    }

    fn redirect(reason: &'static str) -> Self {
        Self::new("ERR_UPDATER_REDIRECT", io::ErrorKind::InvalidData, reason)
    }

    fn manifest(reason: impl std::fmt::Display) -> Self {
        Self::new(
            "ERR_UPDATER_INVALID_MANIFEST",
            io::ErrorKind::InvalidData,
            format!("Could not parse update manifest: {reason}"),
        )
    }

    fn transport(error: &(dyn Error + 'static)) -> Self {
        let chain: Vec<_> = std::iter::successors(Some(error), |&error| error.source()).collect();
        let kind = chain
            .iter()
            .find_map(|error| error.downcast_ref::<io::Error>().map(io::Error::kind))
            .unwrap_or(io::ErrorKind::Other);
        let details = chain
            .iter()
            .map(|error| error.to_string())
            .collect::<Vec<_>>()
            .join(": ");
        let lower = details.to_ascii_lowercase();
        let code = match kind {
            io::ErrorKind::TimedOut => "ETIMEDOUT",
            io::ErrorKind::ConnectionRefused => "ECONNREFUSED",
            io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::UnexpectedEof => "ECONNRESET",
            io::ErrorKind::NetworkUnreachable | io::ErrorKind::HostUnreachable => "ENETUNREACH",
            _ if lower.contains("dns error") => "ENOTFOUND",
            _ if lower.contains("certificate") || lower.contains("tls") => "ERR_UPDATER_TLS",
            _ => "ERR_UPDATER_NETWORK",
        };
        Self::new(code, kind, format!("{code}: {details}"))
    }
}

impl From<RequestError> for UpdateError {
    fn from(error: RequestError) -> Self {
        Self {
            code: Some(error.code),
            message: error.message,
        }
    }
}

impl From<RequestError> for io::Error {
    fn from(error: RequestError) -> Self {
        Self::new(error.kind, error.message)
    }
}

type HttpsClient = Client<HttpsConnector<HttpConnector>, Empty<Bytes>>;
type RequestResponse = (Response<Incoming>, Option<AbortOnDrop>);

/// Blocking adapter for the update manager. Run it on a dedicated worker.
pub struct HttpUpdateFetcher {
    runtime: Arc<Runtime>,
    http: Client<HttpConnector, Empty<Bytes>>,
    https: OnceLock<Result<HttpsClient, RequestError>>,
    proxy_env: ProxyEnvironment,
    timeout: Duration,
    platform: Platform,
}

impl HttpUpdateFetcher {
    pub fn new(platform: Platform) -> io::Result<Self> {
        Ok(Self {
            runtime: Arc::new(Builder::new_current_thread().enable_all().build()?),
            http: Client::builder(TokioExecutor::new()).build_http(),
            https: OnceLock::new(),
            proxy_env: ProxyEnvironment::capture(&std::env::vars_os().collect(), &BTreeMap::new()),
            timeout: REQUEST_TIMEOUT,
            platform,
        })
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    async fn request(
        &self,
        target: Uri,
        deadline: Instant,
        system_proxy: &OnceLock<Option<String>>,
    ) -> Result<RequestResponse, RequestError> {
        let proxy = self
            .proxy_env
            .resolve(&target, || {
                system_proxy
                    .get_or_init(|| {
                        read_system_proxy(
                            deadline
                                .into_std()
                                .min(std::time::Instant::now() + Duration::from_secs(2)),
                        )
                    })
                    .clone()
            })
            .map_err(|error| RequestError::transport(&error))?;
        if Instant::now() >= deadline {
            return Err(RequestError::timeout());
        }
        if let Some(proxy) = proxy {
            let mut request = Request::builder()
                .uri(target.path_and_query().ok_or_else(invalid_url)?.as_str())
                .header(HOST, target.authority().ok_or_else(invalid_url)?.as_str())
                .body(Empty::<Bytes>::new())
                .map_err(|_| invalid_url())?;
            let (mut sender, connection) = connect_proxy(&proxy, &target, &mut request)
                .await
                .map_err(|error| RequestError::transport(error.as_ref()))?;
            let response = sender
                .send_request(request)
                .await
                .map_err(|error| RequestError::transport(&error))?;
            return Ok((response, Some(connection)));
        }

        let response = if target.scheme_str() == Some("https") {
            let client = self
                .https
                .get_or_init(https_client)
                .as_ref()
                .map_err(Clone::clone)?;
            client.get(target).await
        } else {
            self.http.get(target).await
        }
        .map_err(|error| RequestError::transport(&error))?;
        Ok((response, None))
    }

    fn get(
        &self,
        mut url: Url,
        deadline: Instant,
    ) -> Result<(Url, Url, Response<HttpBody>), RequestError> {
        url.set_fragment(None);
        let origin = url.origin();
        let mut artifact_base = url.clone();
        let system_proxy = OnceLock::new();
        for redirects in 0..=MAX_REDIRECTS {
            validate_url(&url)?;
            let target: Uri = url.as_str().parse().map_err(|_| invalid_url())?;
            let (response, connection) = self.runtime.block_on(async {
                tokio::time::timeout_at(deadline, self.request(target, deadline, &system_proxy))
                    .await
                    .map_err(|_| RequestError::timeout())?
            })?;
            if !matches!(response.status().as_u16(), 301 | 302 | 303 | 307 | 308) {
                return Ok((
                    url,
                    artifact_base,
                    response.map(|body| self.body(body, connection)),
                ));
            }
            if redirects == MAX_REDIRECTS {
                return Err(RequestError::redirect("Too many update HTTP redirects"));
            }
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| {
                    RequestError::redirect("Missing or invalid update redirect location")
                })?;
            url = checked_join(&url, location)?;
            url.set_fragment(None);
            // A signed CDN URL serves one file. Relative assets belong to the
            // last manifest URL on the feed origin, typically a release tag.
            if url.origin() == origin {
                artifact_base = url.clone();
            }
        }
        unreachable!("last redirect returns an error")
    }

    fn manifest(&self, url: &Url) -> Result<Manifest, RequestError> {
        let deadline = Instant::now() + self.timeout;
        let (source, artifact_base, response) = self.get(url.clone(), deadline)?;
        let (source, artifact_base, manifest) = if response.status() == StatusCode::NOT_FOUND {
            let mut json_url = url
                .join("release-channel.json")
                .map_err(|_| invalid_url())?;
            json_url.set_query(url.query());
            let (source, artifact_base, response) = self.get(json_url, deadline)?;
            let bytes = self.read_manifest(response, deadline)?;
            let manifest = serde_json::from_slice(&bytes).map_err(RequestError::manifest)?;
            (source, artifact_base, manifest)
        } else {
            let bytes = self.read_manifest(response, deadline)?;
            (source, artifact_base, parse_manifest(&bytes)?)
        };
        resolve_artifacts(manifest, &artifact_base, &source)
    }

    fn read_manifest(
        &self,
        response: Response<HttpBody>,
        deadline: Instant,
    ) -> Result<Vec<u8>, RequestError> {
        if !response.status().is_success() {
            return Err(RequestError::status(response.status()));
        }
        let mut bytes = Vec::new();
        let mut body = response.into_body();
        body.deadline = Some(deadline);
        body.take(MANIFEST_LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| RequestError::transport(&error))?;
        if bytes.len() as u64 > MANIFEST_LIMIT {
            return Err(RequestError::manifest("response exceeds 1 MiB"));
        }
        Ok(bytes)
    }

    fn body(&self, body: Incoming, connection: Option<AbortOnDrop>) -> HttpBody {
        HttpBody {
            runtime: Arc::clone(&self.runtime),
            body,
            chunk: Bytes::new(),
            timeout: self.timeout,
            deadline: None,
            _connection: connection,
        }
    }
}

fn https_client() -> Result<HttpsClient, RequestError> {
    let connector = HttpsConnectorBuilder::new()
        .with_native_roots()
        .map_err(|error| {
            RequestError::new(
                "ERR_UPDATER_TLS",
                error.kind(),
                format!("Update TLS trust store: {error}"),
            )
        })?
        .https_only()
        .enable_http1()
        .build();
    Ok(Client::builder(TokioExecutor::new()).build(connector))
}

fn invalid_url() -> RequestError {
    RequestError::new(
        "ERR_INVALID_URL",
        io::ErrorKind::InvalidInput,
        "Invalid update HTTP URL",
    )
}

fn validate_url(url: &Url) -> Result<(), RequestError> {
    if !matches!(url.scheme(), "http" | "https")
        || !url.has_host()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(invalid_url());
    }
    Ok(())
}

fn checked_join(base: &Url, reference: &str) -> Result<Url, RequestError> {
    let target = base.join(reference).map_err(|_| invalid_url())?;
    validate_url(&target)?;
    validate_https(base, &target)?;
    Ok(target)
}

fn validate_https(base: &Url, target: &Url) -> Result<(), RequestError> {
    if base.scheme() == "https" && target.scheme() == "http" {
        return Err(RequestError::redirect(
            "Update HTTPS URL cannot redirect or link to HTTP",
        ));
    }
    Ok(())
}

fn resolve_artifacts(
    mut manifest: Manifest,
    artifact_base: &Url,
    source: &Url,
) -> Result<Manifest, RequestError> {
    for file in &mut manifest.files {
        if let Some(url) = &mut file.url {
            let target = checked_join(artifact_base, url)?;
            validate_https(source, &target)?;
            *url = target.into();
        }
    }
    Ok(manifest)
}

fn parse_manifest(bytes: &[u8]) -> Result<Manifest, RequestError> {
    let text = std::str::from_utf8(bytes)
        .map_err(RequestError::manifest)?
        .trim_start_matches('\u{feff}');
    if text.trim_start().starts_with('{') {
        return serde_json::from_str(text).map_err(RequestError::manifest);
    }
    parse_channel_yaml(text)
}

// Read the fields produced by the frozen generic updater's channel manifests.
fn parse_channel_yaml(text: &str) -> Result<Manifest, RequestError> {
    let mut manifest = Manifest::default();
    let mut legacy = ManifestFile {
        url: None,
        size: None,
        sha256: None,
        sha512: None,
    };
    let mut in_files = false;
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || matches!(trimmed, "---" | "...") {
            continue;
        }
        if in_files && let Some(item) = trimmed.strip_prefix("- ") {
            manifest.files.push(ManifestFile {
                url: None,
                size: None,
                sha256: None,
                sha512: None,
            });
            read_file_field(manifest.files.last_mut().unwrap(), item)?;
            continue;
        }
        if line.starts_with(char::is_whitespace) {
            if in_files && let Some(file) = manifest.files.last_mut() {
                read_file_field(file, trimmed)?;
            }
            continue;
        }
        let (key, value) = trimmed
            .split_once(':')
            .ok_or_else(|| RequestError::manifest("invalid YAML field"))?;
        in_files = key == "files";
        match key {
            "version" => manifest.version = yaml_scalar(value)?,
            "releaseDate" => manifest.release_date = Some(yaml_scalar(value)?),
            "releaseNotes" => {
                let value = value.trim();
                let notes = if value.starts_with(['|', '>']) {
                    let mut block = Vec::new();
                    while lines.peek().is_some_and(|line| {
                        line.trim().is_empty() || line.starts_with(char::is_whitespace)
                    }) {
                        block.push(lines.next().unwrap().trim());
                    }
                    block.join(if value.starts_with('>') { " " } else { "\n" })
                } else {
                    yaml_scalar(value)?
                };
                manifest.release_notes = Some(crate::update_download::ManifestNotes::Text(notes));
            }
            "path" => legacy.url = Some(yaml_scalar(value)?),
            "sha512" => legacy.sha512 = Some(yaml_scalar(value)?),
            _ => {}
        }
    }
    if manifest.version.is_empty() {
        return Err(RequestError::manifest("missing version"));
    }
    if manifest.files.is_empty() && legacy.url.is_some() {
        manifest.files.push(legacy);
    }
    Ok(manifest)
}

fn read_file_field(file: &mut ManifestFile, line: &str) -> Result<(), RequestError> {
    let Some((key, value)) = line.split_once(':') else {
        return Err(RequestError::manifest("invalid YAML file field"));
    };
    match key {
        "url" => file.url = Some(yaml_scalar(value)?),
        "sha256" => file.sha256 = Some(yaml_scalar(value)?),
        "sha512" => file.sha512 = Some(yaml_scalar(value)?),
        "size" => {
            file.size = Some(
                yaml_scalar(value)?
                    .parse()
                    .map_err(RequestError::manifest)?,
            )
        }
        _ => {}
    }
    Ok(())
}

fn yaml_scalar(value: &str) -> Result<String, RequestError> {
    let value = value.trim();
    if value.starts_with('"') {
        return serde_json::from_str(value).map_err(RequestError::manifest);
    }
    if let Some(value) = value.strip_prefix('\'') {
        return value
            .strip_suffix('\'')
            .map(|value| value.replace("''", "'"))
            .ok_or_else(|| RequestError::manifest("unterminated YAML string"));
    }
    Ok(value
        .split_once(" #")
        .map_or(value, |(value, _)| value.trim_end())
        .into())
}

impl UpdateFetcher for HttpUpdateFetcher {
    fn check(&self, url: &Url) -> Result<Manifest, UpdateError> {
        self.manifest(url).map_err(Into::into)
    }

    fn installer_extension(&self) -> &'static str {
        match self.platform {
            Platform::Win32 => "exe",
            _ => "dmg",
        }
    }

    fn fetch(&self, request: &ManualDownload) -> io::Result<DownloadResponse> {
        if request.expected_sha512.as_deref().is_none_or(str::is_empty) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Update installer is missing a SHA-512 checksum",
            ));
        }
        let (_, _, response) = self.get(request.url.clone(), Instant::now() + self.timeout)?;
        let status = response.status();
        let content_length = response
            .headers()
            .get(CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok()?.parse().ok())
            .or_else(|| response.body().body.size_hint().exact());
        Ok(DownloadResponse {
            status: status.as_u16(),
            content_length,
            body: (status == StatusCode::OK)
                .then(|| Box::new(response.into_body()) as Box<dyn Read + Send>),
        })
    }

    fn download_automatic(
        &self,
        _: &Release,
        _: &mut dyn FnMut(Progress),
    ) -> Result<(), DownloadError> {
        Err(DownloadError::Rejected(format!(
            "The manifest is missing a .{} update installer",
            self.installer_extension()
        )))
    }
}

struct HttpBody {
    runtime: Arc<Runtime>,
    body: Incoming,
    chunk: Bytes,
    timeout: Duration,
    deadline: Option<Instant>,
    _connection: Option<AbortOnDrop>,
}

impl Read for HttpBody {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        let idle_deadline = Instant::now() + self.timeout;
        let deadline = self
            .deadline
            .map_or(idle_deadline, |deadline| deadline.min(idle_deadline));
        while self.chunk.is_empty() {
            let frame = self
                .runtime
                .block_on(async { tokio::time::timeout_at(deadline, self.body.frame()).await })
                .map_err(|_| RequestError::timeout())?;
            let Some(frame) = frame else {
                return Ok(0);
            };
            let frame = frame.map_err(|error| RequestError::transport(&error))?;
            if let Ok(data) = frame.into_data() {
                self.chunk = data;
            }
        }
        let count = buffer.len().min(self.chunk.len());
        buffer[..count].copy_from_slice(&self.chunk.split_to(count));
        Ok(count)
    }
}
