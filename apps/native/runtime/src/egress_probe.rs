//! Egress sampling, classification and protection. The owner applies completed
//! samples and their session effects.

use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    io::{self, Read},
    net::{IpAddr, Ipv6Addr},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{LazyLock, mpsc},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use matou_native_core::provider_launch::{
    automatic_time_zone, cli_time_zone, valid_claude_time_zone,
};
pub use matou_native_core::sessions::{
    InterfaceLocale, LocalePreference, resolve_interface_locale,
};
use serde::{Deserialize, Serialize};
use url::Url;

pub const SLOW_THRESHOLD_MS: u64 = 5_000;
pub const DEFAULT_TIMEOUT_MS: u64 = 10_000;
pub const FROZEN_RECHECK_MS: u64 = 15_000;
pub const RECOVERY_CONFIRM_MS: u64 = 5_000;
pub(crate) const LIVE_RECHECK_INTERVAL: Duration = Duration::from_secs(60);
const MAX_TRACE_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Family {
    Anthropic,
    #[serde(rename = "openai")]
    OpenAi,
    ChatgptSubscription,
    Google,
}

pub const FAMILIES: [Family; 4] = [
    Family::Anthropic,
    Family::OpenAi,
    Family::ChatgptSubscription,
    Family::Google,
];
const CHECKING_FAMILIES: [Family; 3] = [Family::Anthropic, Family::OpenAi, Family::Google];

impl Family {
    pub fn name(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::ChatgptSubscription => "chatgpt-subscription",
            Self::Google => "google",
        }
    }

    pub fn hosts(self) -> (&'static str, &'static str) {
        match self {
            Self::Anthropic => ("api.anthropic.com", "claude.ai"),
            Self::OpenAi => ("api.openai.com", "chatgpt.com"),
            Self::ChatgptSubscription => ("chatgpt.com", "auth.openai.com"),
            Self::Google => ("www.cloudflare.com", "accounts.google.com"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HostError {
    Timeout,
    Unreachable,
    Invalid,
}

impl HostError {
    fn name(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Unreachable => "unreachable",
            Self::Invalid => "invalid",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Exit {
    pub ip: String,
    pub country: String,
}

fn is_false(value: &bool) -> bool {
    !value
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostSample {
    pub host: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub not_applicable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub slow: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<HostError>,
}

impl HostSample {
    pub fn new(host: impl Into<String>) -> Self {
        Self {
            host: host.into(),
            ..Self::default()
        }
    }

    pub fn success(host: impl Into<String>, exit: Exit) -> Self {
        Self {
            ip: Some(exit.ip),
            country: Some(exit.country),
            ..Self::new(host)
        }
    }

    pub fn failed(host: impl Into<String>, error: HostError) -> Self {
        Self {
            error: Some(error),
            ..Self::new(host)
        }
    }

    pub fn exit(&self) -> Option<Exit> {
        Some(Exit {
            ip: self.ip.as_ref().filter(|ip| !ip.is_empty())?.clone(),
            country: self
                .country
                .as_ref()
                .filter(|country| !country.is_empty())?
                .clone(),
        })
    }

    fn has_exit(&self) -> bool {
        self.ip.as_ref().is_some_and(|ip| !ip.is_empty())
            && self
                .country
                .as_ref()
                .is_some_and(|country| !country.is_empty())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TraceClassification {
    Fast,
    Slow,
    Timeout,
    Unreachable,
    Invalid,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TraceRequestError<'a> {
    /// A structural error from the requester (for example, an invalid proxy URL).
    pub kind: Option<HostError>,
    pub code: Option<&'a str>,
    pub name: Option<&'a str>,
    pub message: Option<&'a str>,
}

impl TraceRequestError<'_> {
    fn classify(self) -> HostError {
        static TIMEOUT_TEXT: LazyLock<regex::Regex> =
            LazyLock::new(|| regex::Regex::new(r"(?i-u:\bETIMEDOUT\b)|(?i:timed?\s*out)").unwrap());
        if let Some(kind @ (HostError::Invalid | HostError::Timeout)) = self.kind {
            return kind;
        }
        if matches!(
            self.code,
            Some("ETIMEDOUT" | "ESOCKETTIMEDOUT" | "ERR_TIMED_OUT")
        ) || matches!(self.name, Some("TimeoutError" | "AbortError"))
            || self
                .message
                .is_some_and(|message| TIMEOUT_TEXT.is_match(message))
        {
            HostError::Timeout
        } else {
            HostError::Unreachable
        }
    }
}

pub enum TraceEvent<'a> {
    Response { status: u16, body: &'a [u8] },
    Error(TraceRequestError<'a>),
    Timeout,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceResult {
    pub elapsed_ms: u64,
    pub sample: HostSample,
}

impl TraceResult {
    pub fn classification(&self) -> TraceClassification {
        match self.sample.error {
            Some(HostError::Timeout) => TraceClassification::Timeout,
            Some(HostError::Unreachable) => TraceClassification::Unreachable,
            Some(HostError::Invalid) => TraceClassification::Invalid,
            None if self.sample.slow => TraceClassification::Slow,
            None => TraceClassification::Fast,
        }
    }
}

/// Feed terminal events in delivery order. At the same millisecond the first
/// event wins, including the deadline timer; late events cannot beat the deadline.
pub struct TraceProbe {
    host: String,
    timeout_ms: u64,
    result: Option<TraceResult>,
}

impl TraceProbe {
    pub fn new(host: impl Into<String>, timeout_ms: Option<u64>) -> Self {
        Self {
            host: host.into(),
            timeout_ms: timeout_ms.unwrap_or(DEFAULT_TIMEOUT_MS),
            result: None,
        }
    }

    pub fn timeout_ms(&self) -> u64 {
        self.timeout_ms
    }

    pub fn observe(&mut self, elapsed_ms: u64, event: TraceEvent<'_>) -> &TraceResult {
        self.result.get_or_insert_with(|| {
            let result = if elapsed_ms > self.timeout_ms {
                Err(HostError::Timeout)
            } else {
                match event {
                    TraceEvent::Response { status, body }
                        if (200..300).contains(&status) && body.len() <= MAX_TRACE_BYTES =>
                    {
                        parse_trace(&String::from_utf8_lossy(body)).ok_or(HostError::Invalid)
                    }
                    TraceEvent::Response { .. } => Err(HostError::Invalid),
                    TraceEvent::Error(error) => Err(error.classify()),
                    TraceEvent::Timeout => Err(HostError::Timeout),
                }
            };
            let sample = match result {
                Ok(exit) => HostSample {
                    slow: elapsed_ms > SLOW_THRESHOLD_MS,
                    ..HostSample::success(&self.host, exit)
                },
                Err(error) => HostSample::failed(&self.host, error),
            };
            TraceResult {
                elapsed_ms: elapsed_ms.min(self.timeout_ms),
                sample,
            }
        })
    }
}

fn js_trim(value: &str) -> &str {
    value.trim_matches(|c: char| c == '\u{feff}' || (c.is_whitespace() && c != '\u{0085}'))
}

fn trace_ip(value: &str) -> bool {
    if let Some((ip, zone)) = value.split_once('%') {
        // Node accepts scoped IPv6 addresses with an alphanumeric interface name.
        return ip.parse::<Ipv6Addr>().is_ok()
            && !zone.is_empty()
            && zone
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':'));
    }
    value.parse::<IpAddr>().is_ok()
}

fn parse_trace(body: &str) -> Option<Exit> {
    let mut ip = None;
    let mut country = None;
    for (key, value) in body.lines().filter_map(|line| line.split_once('=')) {
        let value = js_trim(value);
        match js_trim(key) {
            "ip" if trace_ip(value) => ip = Some(value),
            "loc" if is_country_code(value) => country = Some(value),
            _ => {}
        }
    }
    Some(Exit {
        ip: ip?.into(),
        country: country?.into(),
    })
}

fn is_country_code(code: &str) -> bool {
    code.len() == 2 && code.bytes().all(|byte| byte.is_ascii_uppercase())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchPlan {
    pub hide_time_zone: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub family: Option<Family>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_family: Option<Family>,
}

pub fn launch_plan(
    profile: &str,
    env: &BTreeMap<String, String>,
    subscription: Option<&str>,
) -> LaunchPlan {
    fn matches_host(endpoint: Option<&String>, family: Family) -> bool {
        endpoint.map(|value| js_trim(value)).is_none_or(|endpoint| {
            endpoint.is_empty()
                || Url::parse(endpoint).is_ok_and(|url| {
                    url.host_str()
                        .is_some_and(|host| host.eq_ignore_ascii_case(family.hosts().0))
                })
        })
    }
    let family = match (profile, subscription) {
        ("claude-code", Some("codex")) => Some(Family::ChatgptSubscription),
        ("claude-code", Some("antigravity")) => None,
        ("claude-code", _) if matches_host(env.get("ANTHROPIC_BASE_URL"), Family::Anthropic) => {
            Some(Family::Anthropic)
        }
        ("codex", _) if matches_host(env.get("OPENAI_BASE_URL"), Family::OpenAi) => {
            Some(Family::OpenAi)
        }
        ("antigravity", _) => Some(Family::Google),
        _ => None,
    };
    LaunchPlan {
        hide_time_zone: family.is_some()
            || (profile == "claude-code" && subscription == Some("antigravity")),
        family,
        exit_family: family,
    }
}

/// Launch-only preference. Saving it leaves the current egress sample and
/// frozen sessions intact. The warning is evaluated on the next sample.
pub struct TimeZonePreference(String);

impl Default for TimeZonePreference {
    fn default() -> Self {
        Self("auto".into())
    }
}

impl TimeZonePreference {
    pub fn set(&mut self, value: &str) -> Result<&str, &'static str> {
        if !valid_claude_time_zone(value) {
            return Err("Invalid Claude Code time zone");
        }
        self.0 = value.into();
        Ok(&self.0)
    }

    pub fn launch_zone<'a>(
        &'a self,
        profile: &str,
        system: &str,
        exit_country: Option<&str>,
        plan: LaunchPlan,
    ) -> Option<&'a str> {
        cli_time_zone(profile, &self.0, system, exit_country, plan.hide_time_zone)
    }

    pub fn mainland_warning(&self, system: &str) -> bool {
        let zone = cli_time_zone("claude-code", &self.0, system, None, true).unwrap_or(system);
        automatic_time_zone(zone, None).is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostPair {
    pub api: HostSample,
    pub login: HostSample,
}

impl HostPair {
    pub fn new(family: Family, api: HostSample, login: HostSample) -> Self {
        Self {
            api,
            login: if family == Family::Google {
                HostSample {
                    not_applicable: true,
                    ..HostSample::new(family.hosts().1)
                }
            } else {
                login
            },
        }
    }

    fn failed(family: Family, error: HostError) -> Self {
        let (api, login) = family.hosts();
        Self::new(
            family,
            HostSample::failed(api, error),
            HostSample::failed(login, error),
        )
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EgressSample {
    hosts: BTreeMap<Family, HostPair>,
    pub browser_login: Option<HostSample>,
    pub agent_has_proxy_env: bool,
    pub system_proxy_enabled: bool,
    pub tun_detected: bool,
    pub ipv6_bypass: bool,
    pub agent_time_zone_mainland: bool,
}

impl EgressSample {
    pub fn new(hosts: BTreeMap<Family, HostPair>) -> Self {
        Self {
            hosts: hosts
                .into_iter()
                .map(|(family, pair)| (family, HostPair::new(family, pair.api, pair.login)))
                .collect(),
            ..Self::default()
        }
    }

    pub fn hosts(&self) -> &BTreeMap<Family, HostPair> {
        &self.hosts
    }

    /// The file sampler always includes the two official API families.
    /// Optional families appear only when the recording includes them.
    pub fn from_recording(mut hosts: BTreeMap<Family, HostPair>) -> Self {
        for family in [Family::Anthropic, Family::OpenAi] {
            hosts
                .entry(family)
                .or_insert_with(|| HostPair::failed(family, HostError::Timeout));
        }
        Self::new(hosts)
    }

    pub fn recorded_light(&self, family: Family) -> HostSample {
        self.hosts
            .get(&family)
            .map(|pair| pair.api.clone())
            .unwrap_or_else(|| HostSample::failed(family.hosts().0, HostError::Timeout))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubscriptionRoute {
    Disabled,
    Enabled,
    Invalid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProbeTarget {
    pub family: Family,
    pub host: &'static str,
}

/// Hosts may execute these targets concurrently. The target family identifies
/// the proxy context even when two requests use the same hostname.
#[derive(Clone, Copy, Debug)]
pub struct SamplingPlan {
    pub subscription: SubscriptionRoute,
}

impl SamplingPlan {
    pub fn full_targets(self) -> impl Iterator<Item = ProbeTarget> {
        FAMILIES.into_iter().flat_map(move |family| {
            let (api, login) = family.hosts();
            let enabled = family != Family::ChatgptSubscription
                || self.subscription == SubscriptionRoute::Enabled;
            [
                enabled.then_some(ProbeTarget { family, host: api }),
                (enabled && family != Family::Google).then_some(ProbeTarget {
                    family,
                    host: login,
                }),
            ]
            .into_iter()
            .flatten()
        })
    }

    pub fn light_target(self, family: Family) -> Result<ProbeTarget, HostSample> {
        let host = family.hosts().0;
        let error = match (family, self.subscription) {
            (Family::ChatgptSubscription, SubscriptionRoute::Disabled) => {
                Some(HostError::Unreachable)
            }
            (Family::ChatgptSubscription, SubscriptionRoute::Invalid) => Some(HostError::Invalid),
            _ => None,
        };
        match error {
            Some(error) => Err(HostSample::failed(host, error)),
            None => Ok(ProbeTarget { family, host }),
        }
    }

    pub fn collect_full(self, mut samples: BTreeMap<ProbeTarget, HostSample>) -> EgressSample {
        let mut hosts = BTreeMap::new();
        for family in FAMILIES {
            if family == Family::ChatgptSubscription {
                match self.subscription {
                    SubscriptionRoute::Disabled => continue,
                    SubscriptionRoute::Invalid => {
                        hosts.insert(family, HostPair::failed(family, HostError::Invalid));
                        continue;
                    }
                    SubscriptionRoute::Enabled => {}
                }
            }
            let (api, login) = family.hosts();
            let mut sample = |host| {
                samples
                    .remove(&ProbeTarget { family, host })
                    .unwrap_or_else(|| HostSample::failed(host, HostError::Timeout))
            };
            hosts.insert(family, HostPair::new(family, sample(api), sample(login)));
        }
        EgressSample::new(hosts)
    }
}

const HTTPS_PROXY_KEYS: [&str; 2] = ["https_proxy", "HTTPS_PROXY"];
const HTTP_PROXY_KEYS: [&str; 2] = ["http_proxy", "HTTP_PROXY"];
const ALL_PROXY_KEYS: [&str; 2] = ["all_proxy", "ALL_PROXY"];
const NO_PROXY_KEYS: [&str; 2] = ["no_proxy", "NO_PROXY"];
const PROXY_KEYS: [[&str; 2]; 4] = [
    HTTPS_PROXY_KEYS,
    HTTP_PROXY_KEYS,
    ALL_PROXY_KEYS,
    NO_PROXY_KEYS,
];
const TRACE_PATH: &str = "/cdn-cgi/trace";
const TRACE_STATUS_BYTES: usize = 4; // Newline and three HTTP status digits.

type ProxyEnvironment = BTreeMap<&'static str, String>;

fn proxy_for_trace<'a>(env: &'a ProxyEnvironment, url: &Url) -> Option<&'a str> {
    let host = url
        .host_str()?
        .trim_matches(['[', ']'])
        .trim_end_matches('.');
    let port = url.port_or_known_default()?;
    if env
        .get("no_proxy")
        .or_else(|| env.get("NO_PROXY"))
        .is_some_and(|rules| {
            rules
                .split(',')
                .any(|rule| matches_no_proxy(rule, host, port))
        })
    {
        return None;
    }
    let keys = if url.scheme() == "https" {
        HTTPS_PROXY_KEYS
    } else {
        HTTP_PROXY_KEYS
    };
    keys.into_iter().chain(ALL_PROXY_KEYS).find_map(|key| {
        let value = env.get(key)?.trim();
        (!value.is_empty()).then_some(value)
    })
}

fn matches_no_proxy(rule: &str, host: &str, port: u16) -> bool {
    let rule = rule.trim();
    if rule == "*" {
        return true;
    }
    let (domain, rule_port) = if let Some(bracketed) = rule.strip_prefix('[') {
        let Some((domain, rest)) = bracketed.split_once(']') else {
            return false;
        };
        (domain, rest.strip_prefix(':'))
    } else if rule.bytes().filter(|byte| *byte == b':').count() == 1 {
        let (domain, port) = rule.split_once(':').expect("one colon");
        (domain, Some(port))
    } else {
        (rule, None)
    };
    if rule_port.is_some_and(|rule_port| rule_port != port.to_string()) {
        return false;
    }
    let domain = domain.trim_end_matches('.').to_ascii_lowercase();
    if let Some(suffix) = domain.strip_prefix('*') {
        return suffix.starts_with('.') && host.ends_with(suffix);
    }
    let domain = domain.trim_start_matches('.');
    !domain.is_empty()
        && (host == domain
            || host
                .strip_suffix(domain)
                .is_some_and(|prefix| prefix.ends_with('.')))
}

fn proxy_environment() -> ProxyEnvironment {
    PROXY_KEYS
        .into_iter()
        .flatten()
        .filter_map(|key| std::env::var(key).ok().map(|value| (key, value)))
        .collect()
}

fn anthropic_proxy_environment(mut env: ProxyEnvironment) -> ProxyEnvironment {
    let Some(home) = std::env::home_dir() else {
        return env;
    };
    let settings = std::fs::File::open(home.join(".claude").join("settings.json"))
        .ok()
        .and_then(|file| {
            serde_json::from_reader::<_, serde_json::Value>(io::BufReader::new(file)).ok()
        });
    if let Some(settings) = settings {
        for key in PROXY_KEYS.into_iter().flatten() {
            if let Some(value) = settings["env"][key].as_str() {
                env.insert(key, value.to_owned());
            }
        }
    }
    env
}

/// Owns the worker and its HTTP children. Only completed samples cross back
/// to the domain owner. Dropping the request sender cancels a running batch.
pub(crate) struct LiveEgressSampler {
    requests: Option<mpsc::Sender<()>>,
    pub(crate) samples: mpsc::Receiver<EgressSample>,
    worker: Option<JoinHandle<()>>,
}

impl LiveEgressSampler {
    pub(crate) fn start() -> anyhow::Result<Option<Self>> {
        if std::env::var_os("MATOU_DISABLE_LIVE_EGRESS").is_some_and(|value| value == "1") {
            return Ok(None);
        }
        let base = std::env::var("MATOU_EGRESS_TRACE_BASE_URL")
            .ok()
            .map(|value| {
                let url = Url::parse(&value)?;
                anyhow::ensure!(
                    matches!(
                        (url.scheme(), url.host_str()),
                        ("http", Some("127.0.0.1" | "[::1]"))
                    ) && url.username().is_empty()
                        && url.password().is_none(),
                    "MATOU_EGRESS_TRACE_BASE_URL must be an HTTP loopback address",
                );
                Ok(url)
            })
            .transpose()?;
        let (requests, incoming) = mpsc::channel();
        let (completed, samples) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("matou-egress-probe".into())
            .spawn(move || {
                while incoming.recv().is_ok() {
                    let Some(sample) = sample_trace_batch(base.as_ref(), &incoming) else {
                        break;
                    };
                    if completed.send(sample).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Some(Self {
            requests: Some(requests),
            samples,
            worker: Some(worker),
        }))
    }

    pub(crate) fn request(&self) -> anyhow::Result<()> {
        self.requests
            .as_ref()
            .expect("worker still owned")
            .send(())
            .map_err(|_| anyhow::anyhow!("Egress sampling worker stopped"))
    }
}

impl Drop for LiveEgressSampler {
    fn drop(&mut self) {
        drop(self.requests.take());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn sample_trace_batch(base: Option<&Url>, incoming: &mpsc::Receiver<()>) -> Option<EgressSample> {
    let env = proxy_environment();
    let anthropic = anthropic_proxy_environment(env.clone());
    let plan = SamplingPlan {
        subscription: SubscriptionRoute::Disabled,
    };
    let mut samples = BTreeMap::new();
    let mut pending = BTreeMap::new();
    for target in plan.full_targets() {
        if incoming.try_recv() == Err(mpsc::TryRecvError::Disconnected) {
            return None;
        }
        let url = match base {
            Some(url) => url.join(TRACE_PATH).expect("loopback trace URL"),
            None => Url::parse(&format!("https://{}{TRACE_PATH}", target.host))
                .expect("official trace URL"),
        };
        let env = if target.family == Family::Anthropic {
            &anthropic
        } else {
            &env
        };
        match CurlTrace::start(&url, proxy_for_trace(env, &url)) {
            Ok(child) => {
                pending.insert(target, child);
            }
            Err(error) => {
                samples.insert(target, HostSample::failed(target.host, error));
            }
        }
    }
    while !pending.is_empty() {
        pending.retain(|target, child| {
            if let Some(sample) = child.poll(target.host) {
                samples.insert(*target, sample);
                false
            } else {
                true
            }
        });
        if !pending.is_empty()
            && incoming.recv_timeout(Duration::from_millis(10))
                == Err(mpsc::RecvTimeoutError::Disconnected)
        {
            return None;
        }
    }
    let mut sample = plan.collect_full(samples);
    sample.agent_has_proxy_env = [HTTPS_PROXY_KEYS, HTTP_PROXY_KEYS, ALL_PROXY_KEYS]
        .into_iter()
        .flatten()
        .any(|key| {
            anthropic
                .get(key)
                .is_some_and(|value| !value.trim().is_empty())
        });
    Some(sample)
}

struct CurlTrace {
    child: Child,
    output: Option<JoinHandle<io::Result<(u64, Vec<u8>)>>>,
}

impl CurlTrace {
    fn start(url: &Url, proxy: Option<&str>) -> Result<Self, HostError> {
        // Curl is already used by the native runtime hooks. The current
        // Runtime crate cannot link the desktop's HTTP client under --locked.
        #[cfg(windows)]
        let mut request = {
            use std::os::windows::process::CommandExt;
            let mut request = Command::new("curl.exe");
            request.creation_flags(0x08000000); // CREATE_NO_WINDOW
            request
        };
        #[cfg(not(windows))]
        let mut request = Command::new("/usr/bin/curl");
        let timeout = Duration::from_millis(DEFAULT_TIMEOUT_MS)
            .as_secs_f64()
            .to_string();
        request
            .args([
                "--disable",
                "--silent",
                "--fail",
                "--globoff",
                "--proto",
                "=http,https",
                "--max-time",
                &timeout,
                "--max-filesize",
                &MAX_TRACE_BYTES.to_string(),
                "--write-out",
                "\n%{http_code}",
                "--noproxy",
                if proxy.is_some() { "" } else { "*" },
                "--url",
                url.as_str(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for key in PROXY_KEYS.into_iter().flatten() {
            request.env_remove(key);
        }
        if let Some(proxy) = proxy {
            let normalized = if let Some(address) = proxy.strip_prefix("socks://") {
                format!("socks5h://{address}")
            } else if proxy.contains("://") {
                proxy.to_owned()
            } else {
                format!("http://{proxy}")
            };
            let url = Url::parse(&normalized).map_err(|_| HostError::Invalid)?;
            if !matches!(
                url.scheme(),
                "http" | "https" | "socks4" | "socks4a" | "socks5" | "socks5h"
            ) {
                return Err(HostError::Invalid);
            }
            // Keep credentials out of the process arguments and diagnostics.
            request.env("all_proxy", normalized);
        }
        let started = Instant::now();
        let mut child = request.spawn().map_err(|_| HostError::Unreachable)?;
        let stdout = child.stdout.take().expect("piped curl response");
        let mut trace = Self {
            child,
            output: None,
        };
        trace.output = Some(
            thread::Builder::new()
                .name("matou-egress-response".into())
                .spawn(move || {
                    let mut bytes = Vec::new();
                    stdout
                        .take((MAX_TRACE_BYTES + TRACE_STATUS_BYTES + 1) as u64)
                        .read_to_end(&mut bytes)?;
                    Ok((started.elapsed().as_millis() as u64, bytes))
                })
                .map_err(|_| HostError::Unreachable)?,
        );
        Ok(trace)
    }

    fn poll(&mut self, host: &str) -> Option<HostSample> {
        let status = match self.child.try_wait() {
            Ok(None) => return None,
            Ok(Some(status)) => status,
            Err(_) => return Some(HostSample::failed(host, HostError::Unreachable)),
        };
        let output = self.output.take()?.join().ok().and_then(Result::ok);
        Some(classify_curl_trace(host, status, output))
    }
}

impl Drop for CurlTrace {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        if let Some(output) = self.output.take() {
            let _ = output.join();
        }
    }
}

fn classify_curl_trace(
    host: &str,
    status: ExitStatus,
    output: Option<(u64, Vec<u8>)>,
) -> HostSample {
    if output
        .as_ref()
        .is_some_and(|(_, bytes)| bytes.len() > MAX_TRACE_BYTES + TRACE_STATUS_BYTES)
    {
        return HostSample::failed(host, HostError::Invalid);
    }
    if !status.success() {
        return HostSample::failed(
            host,
            match status.code() {
                Some(28) => HostError::Timeout,
                Some(1 | 3 | 4 | 22 | 63) => HostError::Invalid,
                _ => HostError::Unreachable,
            },
        );
    }
    let Some((elapsed, bytes)) = output else {
        return HostSample::failed(host, HostError::Unreachable);
    };
    let Some(split) = bytes.len().checked_sub(TRACE_STATUS_BYTES) else {
        return HostSample::failed(host, HostError::Invalid);
    };
    let Some(status) = std::str::from_utf8(&bytes[split + 1..])
        .ok()
        .and_then(|status| status.parse().ok())
    else {
        return HostSample::failed(host, HostError::Invalid);
    };
    TraceProbe::new(host, None)
        .observe(
            elapsed,
            TraceEvent::Response {
                status,
                body: &bytes[..split],
            },
        )
        .sample
        .clone()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleMode {
    #[default]
    Country,
    Ip,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EgressRule {
    pub mode: RuleMode,
    pub baselines: BTreeMap<Family, Exit>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    Checking,
    Ok,
    Warning,
    Abnormal,
    Unknown,
}

impl State {
    fn healthy(self) -> bool {
        matches!(self, Self::Ok | Self::Warning)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Violation {
    Mainland,
    Rule,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CheckId {
    Mainland,
    ExitRule,
    LoginApiMatch,
    AgentProxyPath,
    BrowserPath,
    TimeZone,
    Ipv6,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckResult {
    Pass,
    Warn,
    Fail,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Check {
    pub id: CheckId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub family: Option<Family>,
    pub result: CheckResult,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub facts: BTreeMap<&'static str, String>,
}

impl Check {
    fn new(id: CheckId, family: Option<Family>, result: CheckResult) -> Self {
        Self {
            id,
            family,
            result,
            facts: BTreeMap::new(),
        }
    }

    fn facts<const N: usize>(mut self, facts: [(&'static str, &str); N]) -> Self {
        self.facts = facts
            .into_iter()
            .map(|(key, value)| (key, value.into()))
            .collect();
        self
    }

    fn unknown(id: CheckId, family: Option<Family>, samples: &[&HostSample]) -> Self {
        let check = Self::new(id, family, CheckResult::Unknown);
        let failed = samples
            .iter()
            .find(|sample| !sample.has_exit() && !sample.not_applicable);
        if let Some((sample, error)) =
            failed.and_then(|sample| sample.error.map(|error| (sample, error)))
        {
            check.facts([("error", error.name()), ("host", &sample.host)])
        } else {
            check
        }
    }

    fn pass(id: CheckId, family: Option<Family>, samples: &[&HostSample]) -> Self {
        let check = Self::new(id, family, CheckResult::Pass);
        match samples
            .iter()
            .find(|sample| sample.slow && !sample.not_applicable)
        {
            Some(sample) => check.facts([("slowHost", &sample.host)]),
            None => check,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FamilyEvaluation {
    pub family: Family,
    pub state: State,
    pub exit: Option<Exit>,
    pub violation: Option<Violation>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evaluation {
    pub state: State,
    pub families: Vec<FamilyEvaluation>,
    pub checks: Vec<Check>,
}

pub fn evaluate_egress(sample: &EgressSample, rule: &EgressRule) -> Evaluation {
    let mut families = Vec::new();
    let mut checks = Vec::new();
    for (&family, pair) in &sample.hosts {
        let (evaluation, family_checks) = evaluate_family(family, pair, rule);
        families.push(evaluation);
        checks.extend(family_checks);
    }
    checks.extend([
        global_check(
            CheckId::AgentProxyPath,
            sample.system_proxy_enabled && !sample.agent_has_proxy_env && !sample.tun_detected,
        ),
        browser_check(sample),
        global_check(CheckId::TimeZone, sample.agent_time_zone_mainland),
        global_check(CheckId::Ipv6, sample.ipv6_bypass),
    ]);
    let state = if families
        .iter()
        .any(|family| family.state == State::Abnormal)
    {
        State::Abnormal
    } else if families
        .iter()
        .any(|family| matches!(family.state, State::Unknown | State::Warning))
        || checks.iter().any(|check| {
            check.result == CheckResult::Warn
                || (check.result == CheckResult::Unknown && check.id != CheckId::LoginApiMatch)
        })
    {
        State::Warning
    } else {
        State::Ok
    };
    Evaluation {
        state,
        families,
        checks,
    }
}

fn evaluate_family(
    family: Family,
    pair: &HostPair,
    rule: &EgressRule,
) -> (FamilyEvaluation, Vec<Check>) {
    use CheckResult::{Fail, Unknown, Warn};
    let api = &pair.api;
    let login = (family != Family::Google).then_some(&pair.login);
    let samples: Vec<_> = std::iter::once(api).chain(login).collect();
    let api_exit = api.exit();
    let mainland = mainland_check(family, &samples);
    let exit_rule = exit_rule_check(family, api, api_exit.as_ref(), rule);
    let login_match = login.map(|login| login_check(family, api, login));

    let (state, violation) = match (mainland.result, exit_rule.result) {
        (Fail, _) => (State::Abnormal, Some(Violation::Mainland)),
        (_, Fail) => (State::Abnormal, Some(Violation::Rule)),
        (Unknown, _) | (_, Unknown) => (State::Unknown, None),
        _ if login_match
            .as_ref()
            .is_some_and(|check| check.result == Warn) =>
        {
            (State::Warning, None)
        }
        _ => (State::Ok, None),
    };
    let exit = api_exit.or_else(|| login.and_then(HostSample::exit));
    let mut checks = vec![mainland, exit_rule];
    checks.extend(login_match);
    (
        FamilyEvaluation {
            family,
            state,
            exit,
            violation,
        },
        checks,
    )
}

fn mainland_check(family: Family, samples: &[&HostSample]) -> Check {
    let id = CheckId::Mainland;
    if let Some(sample) = samples
        .iter()
        .find(|sample| sample.country.as_deref() == Some("CN"))
    {
        Check::new(id, Some(family), CheckResult::Fail).facts([("host", &sample.host)])
    } else if samples.iter().any(|sample| sample.has_exit()) {
        Check::pass(id, Some(family), samples)
    } else {
        Check::unknown(id, Some(family), samples)
    }
}

fn exit_rule_check(
    family: Family,
    api: &HostSample,
    exit: Option<&Exit>,
    rule: &EgressRule,
) -> Check {
    let id = CheckId::ExitRule;
    let Some(baseline) = rule.baselines.get(&family) else {
        return Check::pass(id, Some(family), &[api]);
    };
    let Some(exit) = exit else {
        return Check::unknown(id, Some(family), &[api]);
    };
    let matches_rule = match rule.mode {
        RuleMode::Country => baseline.country == exit.country,
        RuleMode::Ip => baseline.ip == exit.ip,
    };
    if matches_rule {
        Check::pass(id, Some(family), &[api])
    } else {
        Check::new(id, Some(family), CheckResult::Fail).facts([
            ("baselineCountry", &baseline.country),
            ("baselineIp", &baseline.ip),
            ("currentCountry", &exit.country),
            ("currentIp", &exit.ip),
        ])
    }
}

fn login_check(family: Family, api: &HostSample, login: &HostSample) -> Check {
    let id = CheckId::LoginApiMatch;
    let pair = [api, login];
    if !api.has_exit() || !login.has_exit() {
        Check::unknown(id, Some(family), &pair)
    } else if api.ip != login.ip {
        Check::new(id, Some(family), CheckResult::Warn).facts([
            ("apiIp", api.ip.as_deref().unwrap_or_default()),
            ("loginIp", login.ip.as_deref().unwrap_or_default()),
        ])
    } else {
        Check::pass(id, Some(family), &pair)
    }
}

fn global_check(id: CheckId, warn: bool) -> Check {
    Check::new(
        id,
        None,
        if warn {
            CheckResult::Warn
        } else {
            CheckResult::Pass
        },
    )
}

fn browser_check(sample: &EgressSample) -> Check {
    let id = CheckId::BrowserPath;
    let Some(browser) = sample
        .browser_login
        .as_ref()
        .filter(|_| sample.system_proxy_enabled)
    else {
        return global_check(id, false);
    };
    let agent = sample.hosts.get(&Family::Anthropic).map(|pair| &pair.login);
    let ip = |sample: &HostSample| sample.ip.as_deref().is_some_and(|ip| !ip.is_empty());
    if !ip(browser) || !agent.is_some_and(ip) {
        let samples: Vec<_> = std::iter::once(browser).chain(agent).collect();
        return Check::unknown(id, None, &samples);
    }
    let browser_ip = browser.ip.as_deref().unwrap_or_default();
    let agent_ip = agent
        .and_then(|host| host.ip.as_deref())
        .unwrap_or_default();
    if browser_ip != agent_ip {
        return global_check(id, true).facts([("browserIp", browser_ip), ("agentIp", agent_ip)]);
    }
    global_check(id, false)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TransitionKind {
    Frozen,
    Resumed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transition {
    pub id: u64,
    pub kind: TransitionKind,
    pub family: Family,
    pub session_count: usize,
    pub at: u64,
    pub reason: Option<Violation>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FamilyStatus {
    pub evaluation: FamilyEvaluation,
    pub hosts: [HostSample; 2],
    pub baseline: Option<Exit>,
    pub overridden: bool,
    pub frozen_session_ids: Vec<String>,
    pub recovering: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub state: State,
    pub families: Vec<FamilyStatus>,
    pub checks: Vec<Check>,
    pub rule_mode: RuleMode,
    pub protection_enabled: bool,
    pub session_families: BTreeMap<String, Vec<Family>>,
    pub checked_at: Option<u64>,
    pub last_transition: Option<Transition>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionEffect {
    Freeze(String),
    Thaw(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum RecoveryPhase {
    #[default]
    Idle,
    Rechecking,
    Confirming,
}

pub trait EgressClock {
    fn now_ms(&self) -> u64;
}

pub trait EgressSampler {
    type Error;

    fn sample_full(&mut self) -> impl Future<Output = Result<EgressSample, Self::Error>> + Send;
    fn sample_light(
        &mut self,
        family: Family,
    ) -> impl Future<Output = Result<HostSample, Self::Error>> + Send;
}

pub struct EgressMonitor {
    rule: EgressRule,
    protection_enabled: bool,
    sessions: Vec<(String, Vec<Family>)>,
    frozen: Vec<String>,
    overrides: BTreeMap<Family, String>,
    last_resolved_ips: BTreeMap<Family, String>,
    phases: BTreeMap<Family, RecoveryPhase>,
    last_sample: Option<EgressSample>,
    evaluation: Option<Evaluation>,
    checked_at: Option<u64>,
    recovery_deadline: Option<u64>,
    last_transition: Option<Transition>,
}

impl Default for EgressMonitor {
    fn default() -> Self {
        Self::new(EgressRule::default(), true)
    }
}

impl EgressMonitor {
    pub fn new(rule: EgressRule, protection_enabled: bool) -> Self {
        Self {
            rule,
            protection_enabled,
            sessions: Vec::new(),
            frozen: Vec::new(),
            overrides: BTreeMap::new(),
            last_resolved_ips: BTreeMap::new(),
            phases: BTreeMap::new(),
            last_sample: None,
            evaluation: None,
            checked_at: None,
            recovery_deadline: None,
            last_transition: None,
        }
    }

    pub fn rule(&self) -> &EgressRule {
        &self.rule
    }

    pub fn recovery_deadline(&self) -> Option<u64> {
        self.recovery_deadline
    }

    pub fn status(&self) -> Status {
        let families = match (&self.evaluation, &self.last_sample) {
            (Some(evaluation), Some(sample)) => evaluation
                .families
                .iter()
                .map(|entry| {
                    let pair = &sample.hosts[&entry.family];
                    self.family_status(entry.clone(), [pair.api.clone(), pair.login.clone()])
                })
                .collect(),
            _ => CHECKING_FAMILIES
                .into_iter()
                .map(|family| {
                    let (api, login) = family.hosts();
                    let pair = HostPair::new(family, HostSample::new(api), HostSample::new(login));
                    self.family_status(
                        FamilyEvaluation {
                            family,
                            state: State::Unknown,
                            exit: None,
                            violation: None,
                        },
                        [pair.api, pair.login],
                    )
                })
                .collect(),
        };
        Status {
            state: self
                .evaluation
                .as_ref()
                .map_or(State::Checking, |evaluation| evaluation.state),
            families,
            checks: self
                .evaluation
                .as_ref()
                .map_or_else(Vec::new, |evaluation| evaluation.checks.clone()),
            rule_mode: self.rule.mode,
            protection_enabled: self.protection_enabled,
            session_families: self.sessions.iter().cloned().collect(),
            checked_at: self.checked_at,
            last_transition: self.last_transition.clone(),
        }
    }

    fn family_status(&self, evaluation: FamilyEvaluation, hosts: [HostSample; 2]) -> FamilyStatus {
        let family = evaluation.family;
        let frozen_session_ids = self
            .frozen_for(family)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        FamilyStatus {
            evaluation,
            hosts,
            baseline: self.rule.baselines.get(&family).cloned(),
            overridden: self.overrides.contains_key(&family),
            recovering: !frozen_session_ids.is_empty() && self.phase(family) != RecoveryPhase::Idle,
            frozen_session_ids,
        }
    }

    /// A true return value tells the caller to launch this session already frozen.
    pub fn register_session(&mut self, id: String, families: &[Family], now: u64) -> bool {
        let mut seen = BTreeSet::new();
        let families: Vec<_> = families
            .iter()
            .copied()
            .filter(|family| seen.insert(*family))
            .collect();
        let born_frozen =
            self.protection_enabled && families.iter().any(|&family| self.blocked(family));
        if born_frozen {
            if !self.frozen.contains(&id) {
                self.frozen.push(id.clone());
            }
            for &family in &families {
                if self.phase(family) == RecoveryPhase::Idle {
                    self.phases.insert(family, RecoveryPhase::Rechecking);
                }
            }
        }
        if let Some((_, registered)) = self.sessions.iter_mut().find(|(session, _)| *session == id)
        {
            *registered = families;
        } else {
            self.sessions.push((id, families));
        }
        if born_frozen {
            self.schedule_recovery(now);
        }
        born_frozen
    }

    pub fn unregister_session(&mut self, id: &str, now: u64) {
        self.sessions.retain(|(session, _)| session != id);
        if self.frozen.iter().any(|session| session == id) {
            self.frozen.retain(|session| session != id);
            for family in FAMILIES {
                if self.frozen_for(family).next().is_none() {
                    self.phases.remove(&family);
                }
            }
            self.schedule_recovery(now);
        }
    }

    pub fn assert_target_allowed(&self, family: Option<Family>) -> Result<(), &'static str> {
        if self.protection_enabled && family.is_some_and(|family| self.blocked(family)) {
            return Err(
                "出口异常，已暂停。请在网络检查中恢复出口或明确豁免后重试。 Network egress protection is active.",
            );
        }
        Ok(())
    }

    /// Applies a complete sample at its completion time.
    pub fn apply_full_sample(&mut self, sample: EgressSample, now: u64) -> Vec<SessionEffect> {
        self.checked_at = Some(now);
        self.apply_sample(sample, now, false)
    }

    pub async fn recheck<S: EgressSampler>(
        &mut self,
        clock: &impl EgressClock,
        sampler: &mut S,
    ) -> Result<Vec<SessionEffect>, S::Error> {
        let sample = sampler.sample_full().await;
        let now = clock.now_ms();
        match sample {
            Ok(sample) => Ok(self.apply_full_sample(sample, now)),
            Err(error) => {
                self.schedule_recovery(now);
                Err(error)
            }
        }
    }

    pub async fn recheck_if_due<S: EgressSampler>(
        &mut self,
        clock: &impl EgressClock,
        sampler: &mut S,
    ) -> Result<Option<Vec<SessionEffect>>, S::Error> {
        if self
            .recovery_deadline
            .is_some_and(|deadline| deadline <= clock.now_ms())
        {
            self.recheck(clock, sampler).await.map(Some)
        } else {
            Ok(None)
        }
    }

    pub fn light_families(&self) -> Vec<Family> {
        let active: BTreeSet<_> = self
            .sessions
            .iter()
            .flat_map(|(_, families)| families)
            .copied()
            .collect();
        if active.is_empty() {
            vec![Family::Anthropic]
        } else {
            active.into_iter().collect()
        }
    }

    pub fn light_needs_full_check(&self, family: Family, light: &HostSample) -> bool {
        let previous_ip = self
            .last_sample
            .as_ref()
            .and_then(|sample| sample.hosts.get(&family))
            .and_then(|pair| pair.api.ip.as_ref());
        light.country.as_deref() == Some("CN")
            || (light.ip.is_some() && light.ip.as_ref() != previous_ip)
    }

    pub fn set_rule_mode(&mut self, mode: RuleMode, now: u64) -> Vec<SessionEffect> {
        self.rule.mode = mode;
        self.reapply_sample(now, true)
    }

    pub fn accept_current(
        &mut self,
        family: Family,
        now: u64,
    ) -> Result<Vec<SessionEffect>, String> {
        let exit = self
            .last_sample
            .as_ref()
            .and_then(|sample| sample.hosts.get(&family))
            .and_then(|pair| pair.api.exit())
            .filter(|exit| exit.country != "CN")
            .ok_or_else(|| {
                format!(
                    "Current API exit for {} does not pass the mainland check",
                    family.name()
                )
            })?;
        self.rule.baselines.insert(family, exit);
        Ok(self.reapply_sample(now, true))
    }

    pub fn set_protection(&mut self, enabled: bool, now: u64) -> Vec<SessionEffect> {
        self.protection_enabled = enabled;
        if enabled {
            return self.reapply_sample(now, false);
        }
        self.phases.clear();
        self.recovery_deadline = None;
        std::mem::take(&mut self.frozen)
            .into_iter()
            .map(SessionEffect::Thaw)
            .collect()
    }

    pub fn override_family(
        &mut self,
        family: Family,
        now: u64,
    ) -> Result<Vec<SessionEffect>, String> {
        let ip = self
            .evaluated_family(family)
            .and_then(|entry| entry.exit.as_ref().map(|exit| &exit.ip))
            .or_else(|| self.last_resolved_ips.get(&family))
            .ok_or_else(|| {
                format!(
                    "Cannot override {} without a resolved exit IP",
                    family.name()
                )
            })?;
        self.overrides.insert(family, ip.clone());
        let mut effects = Vec::new();
        let to_thaw = self
            .frozen_for(family)
            .filter(|id| self.can_thaw(id, None))
            .map(str::to_owned)
            .collect();
        self.thaw(to_thaw, &mut effects);
        self.phases.remove(&family);
        self.schedule_recovery(now);
        Ok(effects)
    }

    fn reapply_sample(&mut self, now: u64, immediate_thaw: bool) -> Vec<SessionEffect> {
        self.last_sample.take().map_or_else(Vec::new, |sample| {
            self.apply_sample(sample, now, immediate_thaw)
        })
    }

    fn apply_sample(
        &mut self,
        sample: EgressSample,
        now: u64,
        immediate_thaw: bool,
    ) -> Vec<SessionEffect> {
        let mut evaluation = evaluate_egress(&sample, &self.rule);
        let mut wrote_baseline = false;
        for check in &evaluation.checks {
            if check.id != CheckId::Mainland || check.result != CheckResult::Pass {
                continue;
            }
            if let Some(family) = check.family
                && let std::collections::btree_map::Entry::Vacant(entry) =
                    self.rule.baselines.entry(family)
                && let Some(exit) = sample.hosts[&family].api.exit()
            {
                entry.insert(exit);
                wrote_baseline = true;
            }
        }
        if wrote_baseline {
            evaluation = evaluate_egress(&sample, &self.rule);
        }

        // Invalidate overrides for every channel against the same sample before thawing any session.
        for entry in &evaluation.families {
            let family = entry.family;
            if let Some(exit) = &entry.exit {
                self.last_resolved_ips.insert(family, exit.ip.clone());
                if self.overrides.get(&family).is_some_and(|ip| *ip != exit.ip) {
                    self.overrides.remove(&family);
                }
            }
            if entry.state.healthy() {
                self.overrides.remove(&family);
            }
        }
        let ready = evaluation
            .families
            .iter()
            .filter(|entry| {
                self.overrides.contains_key(&entry.family)
                    || (entry.state.healthy()
                        && (immediate_thaw
                            || self.phase(entry.family) == RecoveryPhase::Confirming))
            })
            .map(|entry| entry.family)
            .collect();
        self.last_sample = Some(sample);
        self.evaluation = Some(evaluation);

        let mut effects = Vec::new();
        for family in FAMILIES {
            if let Some(entry) = self.evaluated_family(family) {
                self.reconcile(
                    family,
                    entry.state,
                    entry.violation,
                    now,
                    &ready,
                    &mut effects,
                );
            }
        }
        for family in FAMILIES {
            if self.evaluated_family(family).is_some() {
                continue;
            }
            let to_thaw = self
                .frozen_for(family)
                .filter(|id| self.families_for(id).len() == 1)
                .map(str::to_owned)
                .collect();
            self.thaw(to_thaw, &mut effects);
            self.phases.remove(&family);
        }
        self.schedule_recovery(now);
        effects
    }

    fn reconcile(
        &mut self,
        family: Family,
        state: State,
        violation: Option<Violation>,
        now: u64,
        ready: &BTreeSet<Family>,
        effects: &mut Vec<SessionEffect>,
    ) {
        use RecoveryPhase::{Confirming, Idle, Rechecking};

        let phase = if state == State::Abnormal {
            if self.protection_enabled && !self.overrides.contains_key(&family) {
                let to_freeze: Vec<_> = self
                    .sessions
                    .iter()
                    .filter(|(id, families)| {
                        families.contains(&family) && !self.frozen.contains(id)
                    })
                    .map(|(id, _)| id.clone())
                    .collect();
                self.transition(
                    TransitionKind::Frozen,
                    family,
                    to_freeze.len(),
                    now,
                    violation,
                );
                self.frozen.extend(to_freeze.iter().cloned());
                effects.extend(to_freeze.into_iter().map(SessionEffect::Freeze));
                if self.frozen_for(family).next().is_some() {
                    Rechecking
                } else {
                    Idle
                }
            } else {
                Idle
            }
        } else if self.frozen_for(family).next().is_none() {
            Idle
        } else if state == State::Unknown {
            Rechecking
        } else {
            if ready.contains(&family) {
                let to_thaw: Vec<_> = self
                    .frozen_for(family)
                    .filter(|id| self.can_thaw(id, Some(ready)))
                    .map(str::to_owned)
                    .collect();
                self.transition(TransitionKind::Resumed, family, to_thaw.len(), now, None);
                self.thaw(to_thaw, effects);
            }
            if self.frozen_for(family).next().is_some() {
                Confirming
            } else {
                Idle
            }
        };
        self.phases.insert(family, phase);
    }

    fn thaw(&mut self, ids: Vec<String>, effects: &mut Vec<SessionEffect>) {
        self.frozen.retain(|id| !ids.contains(id));
        effects.extend(ids.into_iter().map(SessionEffect::Thaw));
    }

    fn transition(
        &mut self,
        kind: TransitionKind,
        family: Family,
        count: usize,
        now: u64,
        reason: Option<Violation>,
    ) {
        if count > 0 {
            let id = self
                .last_transition
                .as_ref()
                .map_or(1, |transition| transition.id + 1);
            self.last_transition = Some(Transition {
                id,
                kind,
                family,
                session_count: count,
                at: now,
                reason,
            });
        }
    }

    fn evaluated_family(&self, family: Family) -> Option<&FamilyEvaluation> {
        self.evaluation
            .as_ref()?
            .families
            .iter()
            .find(|entry| entry.family == family)
    }

    fn blocked(&self, family: Family) -> bool {
        self.evaluated_family(family)
            .is_some_and(|entry| entry.state == State::Abnormal)
            && !self.overrides.contains_key(&family)
    }

    fn can_thaw(&self, id: &str, ready: Option<&BTreeSet<Family>>) -> bool {
        self.families_for(id).iter().all(|&family| {
            self.overrides.contains_key(&family)
                || (self
                    .evaluated_family(family)
                    .is_some_and(|entry| entry.state.healthy())
                    && ready.is_none_or(|ready| ready.contains(&family)))
        })
    }

    fn families_for(&self, id: &str) -> &[Family] {
        self.sessions
            .iter()
            .find(|(session, _)| session == id)
            .map_or(&[], |(_, families)| families)
    }

    fn frozen_for(&self, family: Family) -> impl Iterator<Item = &str> {
        self.frozen
            .iter()
            .filter(move |id| self.families_for(id).contains(&family))
            .map(String::as_str)
    }

    fn phase(&self, family: Family) -> RecoveryPhase {
        self.phases.get(&family).copied().unwrap_or_default()
    }

    fn schedule_recovery(&mut self, now: u64) {
        self.recovery_deadline = FAMILIES
            .into_iter()
            .filter(|&family| self.frozen_for(family).next().is_some())
            .filter_map(|family| match self.phase(family) {
                RecoveryPhase::Idle => None,
                RecoveryPhase::Rechecking => Some(FROZEN_RECHECK_MS),
                RecoveryPhase::Confirming => Some(RECOVERY_CONFIRM_MS),
            })
            .min()
            .map(|delay| now.saturating_add(delay));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BannerKind {
    None,
    Frozen,
    Resumed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardView {
    pub family: Option<Family>,
    pub frozen: bool,
    pub kind: BannerKind,
    pub message: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct OverridePrompt {
    family: Family,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverrideDecision {
    Cancel,
    Confirm,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OverrideOutcome {
    pub applied_family: Option<Family>,
    pub effects: Vec<SessionEffect>,
}

impl OverridePrompt {
    pub fn new(family: Family) -> Self {
        Self { family }
    }

    /// The IP is resolved at confirmation time, as for a direct family override.
    pub fn respond(
        self,
        decision: OverrideDecision,
        monitor: &mut EgressMonitor,
        now: u64,
    ) -> Result<OverrideOutcome, String> {
        match decision {
            OverrideDecision::Cancel => Ok(OverrideOutcome {
                applied_family: None,
                effects: Vec::new(),
            }),
            OverrideDecision::Confirm => Ok(OverrideOutcome {
                applied_family: Some(self.family),
                effects: monitor.override_family(self.family, now)?,
            }),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CheckCopy {
    pub title: &'static str,
    pub detail: Option<String>,
}

/// Supply the caller's localized ISO region names (for example, from Intl).
/// Unknown region codes fall back to the code, as with the renderer.
pub struct EgressCopy<F> {
    locale: InterfaceLocale,
    region_name: F,
}

impl<F> EgressCopy<F>
where
    F: Fn(&str, InterfaceLocale) -> Option<String>,
{
    pub fn new(locale: InterfaceLocale, region_name: F) -> Self {
        Self {
            locale,
            region_name,
        }
    }

    fn zh(&self) -> bool {
        self.locale == InterfaceLocale::Chinese
    }

    fn country_name(&self, code: Option<&str>) -> String {
        let Some(code) = code.filter(|code| !code.is_empty()) else {
            return self.locale.text("未知地区", "Unknown").into();
        };
        let upper = code.to_ascii_uppercase();
        if is_country_code(&upper) {
            (self.region_name)(&upper, self.locale).unwrap_or(upper)
        } else {
            code.into()
        }
    }

    pub fn check(&self, check: &Check, exit: Option<&Exit>) -> CheckCopy {
        let title = self.check_title(check.id);
        let fact = |key| check.facts.get(key).map(String::as_str).unwrap_or_default();
        let host = fact("host");
        let detail = match check.result {
            CheckResult::Unknown => {
                let target = if host.is_empty() { String::new() } else { format!("{host} ") };
                let reason = match fact("error") {
                    "unreachable" => format!("{target}{}", self.locale.text("连不上", "unreachable")),
                    "invalid" => format!("{target}{}", self.locale.text("返回异常", "invalid response")),
                    _ if !host.is_empty() => format!("{target}{}", self.locale.text("检测超时", "timed out")),
                    _ => self.locale.text("检测超时", "check timed out").into(),
                };
                Some(if self.zh() { format!("无法确认（{reason}）") } else { format!("Couldn't confirm ({reason})") })
            }
            CheckResult::Pass => {
                let slow_host = fact("slowHost");
                let slow = (!slow_host.is_empty()).then(|| if self.zh() {
                    format!("{slow_host} 响应慢（>5 秒）")
                } else {
                    format!("{slow_host} is slow (>5s)")
                });
                let ip = exit.map_or_else(|| fact("ip"), |exit| exit.ip.as_str());
                let country = exit.map_or_else(|| fact("country"), |exit| exit.country.as_str());
                if check.id == CheckId::Mainland && (!ip.is_empty() || !country.is_empty()) {
                    let mut detail = self.country_name(Some(country));
                    if !ip.is_empty() {
                        detail.push_str(" · ");
                        detail.push_str(ip);
                    }
                    if let Some(slow) = slow {
                        detail.push_str(" · ");
                        detail.push_str(&slow);
                    }
                    Some(detail)
                } else {
                    slow
                }
            }
            CheckResult::Warn | CheckResult::Fail => Some(match check.id {
                CheckId::Mainland if self.zh() => format!("{host} 的出口在中国大陆。检查代理是否开启；规则模式下确认该域名走代理。"),
                CheckId::Mainland => format!("{host} exits from mainland China. Check that your proxy is on and that this domain is routed through it."),
                CheckId::ExitRule => {
                    let baseline = self.country_name(Some(fact("baselineCountry")));
                    let current = self.country_name(Some(fact("currentCountry")));
                    let ip = fact("currentIp");
                    if self.zh() {
                        format!("{baseline} → {current}（{ip}）。确认节点；如果是预期变化，点「以当前出口为准」。")
                    } else {
                        format!("{baseline} → {current} ({ip}). Check your node; if this change is expected, choose \"Use current exit\".")
                    }
                }
                CheckId::LoginApiMatch => {
                    let login = fact("loginIp");
                    let api = fact("apiIp");
                    if self.zh() {
                        format!("登录 {login}，API {api}。多半是分流规则把两个域名分到了不同节点。")
                    } else {
                        format!("Sign-in {login}, API {api}. A routing rule probably sends these domains through different nodes.")
                    }
                }
                CheckId::AgentProxyPath => self.locale.text(
                    "已开系统代理，但终端里的 agent 不读系统代理。请在 Clash 开启 TUN，或在 shell 配置里设置 HTTPS_PROXY。",
                    "A system proxy is on, but terminal agents ignore it. Turn on TUN mode in Clash, or set HTTPS_PROXY in your shell profile.",
                ).into(),
                CheckId::BrowserPath => {
                    let browser = fact("browserIp");
                    let agent = fact("agentIp");
                    if self.zh() {
                        format!("浏览器 {browser}，终端 {agent}。授权登录和使用时 IP 不同。")
                    } else {
                        format!("Browser {browser}, terminal {agent}. You would sign in and work from different IPs.")
                    }
                }
                CheckId::TimeZone => self.locale.text(
                    "会话会使用大陆时区。到 设置 → Claude Code → 时区，选「自动」。",
                    "Sessions would use a mainland China time zone. In Settings → Claude Code → Time zone, choose Automatic.",
                ).into(),
                CheckId::Ipv6 => self.locale.text(
                    "本机有公网 IPv6，部分 agent 可能经 IPv6 直连。可在 Clash 开启 IPv6 接管，或在系统里关闭 IPv6。",
                    "This Mac has public IPv6, so some agents may connect directly over IPv6. Let Clash handle IPv6, or turn IPv6 off.",
                ).into(),
            }),
        };
        CheckCopy { title, detail }
    }

    fn check_title(&self, id: CheckId) -> &'static str {
        match id {
            CheckId::Mainland => self
                .locale
                .text("出口不在中国大陆", "Exit is outside mainland China"),
            CheckId::ExitRule => self
                .locale
                .text("出口符合出口规则", "Exit matches your rule"),
            CheckId::LoginApiMatch => self
                .locale
                .text("登录与 API 同一出口", "Sign-in and API share one exit"),
            CheckId::AgentProxyPath => self
                .locale
                .text("终端 agent 走代理", "Terminal agents use the proxy"),
            CheckId::BrowserPath => self.locale.text(
                "登录浏览器与终端同一出口",
                "Browser and terminal share one exit",
            ),
            CheckId::TimeZone => self
                .locale
                .text("没有暴露大陆时区", "No mainland China time zone"),
            CheckId::Ipv6 => self.locale.text("无 IPv6 绕过", "No IPv6 bypass"),
        }
    }

    pub fn family_heading(&self, family: Family) -> &'static str {
        match family {
            Family::Anthropic => self
                .locale
                .text("Claude（Anthropic）", "Claude (Anthropic)"),
            Family::OpenAi => self.locale.text("Codex（OpenAI）", "Codex (OpenAI)"),
            Family::ChatgptSubscription => self
                .locale
                .text("Claude（ChatGPT 订阅）", "Claude (ChatGPT subscription)"),
            Family::Google => self
                .locale
                .text("Google（通用出口检测）", "Google (generic exit)"),
        }
    }

    pub fn local_heading(&self) -> &'static str {
        self.locale.text("本机", "This Mac")
    }

    pub fn sampling_note(&self) -> &'static str {
        self.locale.text(
            "检测为每分钟抽查，无法保证每条连接都使用同一出口。Google 线路使用通用出口检测，不能保证 Google 实际看到的出口。",
            "Checks are sampled every minute and cannot guarantee every connection uses the same exit. Google uses a generic exit check, which cannot guarantee the exit Google actually sees.",
        )
    }

    pub fn card(&self, status: &Status, id: &str) -> CardView {
        let families = status
            .session_families
            .get(id)
            .map_or(&[][..], Vec::as_slice);
        let entries: Vec<_> = families
            .iter()
            .filter_map(|family| {
                status
                    .families
                    .iter()
                    .find(|entry| entry.evaluation.family == *family)
            })
            .collect();
        let frozen = |entry: &&FamilyStatus| {
            !entry.overridden
                && entry
                    .frozen_session_ids
                    .iter()
                    .any(|frozen_id| frozen_id == id)
        };
        let blocked = entries
            .iter()
            .copied()
            .find(|entry| !entry.overridden && entry.evaluation.state == State::Abnormal);
        let frozen_entry = entries.iter().copied().find(frozen);
        let family = blocked
            .or(frozen_entry)
            .map(|entry| entry.evaluation.family)
            .or_else(|| families.first().copied());
        let (kind, message) = if let Some(entry) = frozen_entry {
            (
                BannerKind::Frozen,
                Some(self.freeze_message(blocked.unwrap_or(entry))),
            )
        } else if family.is_some_and(|family| {
            entries
                .iter()
                .any(|entry| entry.evaluation.family == family)
        }) && status.last_transition.as_ref().is_some_and(|transition| {
            transition.kind == TransitionKind::Resumed && families.contains(&transition.family)
        }) {
            (
                BannerKind::Resumed,
                Some(
                    self.locale
                        .text(
                            "出口已恢复，已自动继续",
                            "Exit restored; resumed automatically",
                        )
                        .into(),
                ),
            )
        } else {
            (BannerKind::None, None)
        };
        CardView {
            family,
            frozen: frozen_entry.is_some(),
            kind,
            message,
        }
    }

    fn freeze_message(&self, entry: &FamilyStatus) -> String {
        if entry.evaluation.violation == Some(Violation::Rule) {
            let baseline =
                self.country_name(entry.baseline.as_ref().map(|exit| exit.country.as_str()));
            let current = self.country_name(
                entry
                    .evaluation
                    .exit
                    .as_ref()
                    .map(|exit| exit.country.as_str()),
            );
            if self.zh() {
                format!("出口与规则不符（{baseline} → {current}），已暂停。网络恢复后自动继续。")
            } else {
                format!(
                    "Exit doesn't match your rule ({baseline} → {current}). Paused; resumes automatically when the network recovers."
                )
            }
        } else {
            self.locale.text(
                "出口异常（检测到中国大陆 IP），已暂停。网络恢复后自动继续。",
                "Exit problem (mainland China IP detected). Paused; resumes automatically when the network recovers.",
            ).into()
        }
    }

    pub fn notification(&self, previous: &Status, next: &Status) -> Option<String> {
        let transition = next.last_transition.as_ref()?;
        if previous
            .last_transition
            .as_ref()
            .is_some_and(|old| old.id == transition.id)
        {
            return None;
        }
        let count = transition.session_count;
        Some(match (transition.kind, self.zh()) {
            (TransitionKind::Frozen, true) => format!("出口异常，已暂停 {count} 个会话"),
            (TransitionKind::Frozen, false) => format!("Exit problem: {count} session(s) paused"),
            (TransitionKind::Resumed, true) => format!("出口已恢复，{count} 个会话已继续"),
            (TransitionKind::Resumed, false) => {
                format!("Exit restored: {count} session(s) resumed")
            }
        })
    }

    pub fn freeze_buttons(&self) -> [&'static str; 2] {
        [
            self.locale.text("重新检查", "Recheck"),
            self.locale.text("仍要继续", "Continue anyway"),
        ]
    }

    pub fn override_confirmation(&self) -> [&'static str; 4] {
        [
            self.locale.text("仍要继续？", "Continue anyway?"),
            self.locale.text(
                "出口仍然异常，继续后 agent 会用当前出口访问服务商。出口再次变化前不会再暂停。",
                "The exit is still wrong. Agents will reach the provider through it, and they won't be paused again until the exit changes.",
            ),
            self.locale.text("取消", "Cancel"),
            self.locale.text("继续", "Continue"),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_trace_proxy_matches_frozen_precedence_and_bypass() {
        let https = Url::parse("https://api.anthropic.com/cdn-cgi/trace").unwrap();
        let http = Url::parse("http://api.anthropic.com/cdn-cgi/trace").unwrap();
        let mut env = BTreeMap::from([
            ("HTTPS_PROXY", "secure:443".into()),
            ("https_proxy", " preferred:443 ".into()),
            ("HTTP_PROXY", "plain:80".into()),
            ("ALL_PROXY", "socks5h://fallback:1080".into()),
            ("NO_PROXY", "*.anthropic.com:443".into()),
        ]);
        assert_eq!(proxy_for_trace(&env, &https), None);
        assert_eq!(proxy_for_trace(&env, &http), Some("plain:80"));
        env.insert("no_proxy", String::new());
        assert_eq!(proxy_for_trace(&env, &https), Some("preferred:443"));
        env.remove("https_proxy");
        assert_eq!(proxy_for_trace(&env, &https), Some("secure:443"));
        env.remove("HTTPS_PROXY");
        assert_eq!(
            proxy_for_trace(&env, &https),
            Some("socks5h://fallback:1080")
        );
    }

    #[test]
    fn egress_probe_ignores_events_after_settlement() {
        let mut probe = TraceProbe::new("api.anthropic.com", None);
        let result = probe
            .observe(DEFAULT_TIMEOUT_MS, TraceEvent::Timeout)
            .clone();
        assert_eq!(
            probe.observe(
                DEFAULT_TIMEOUT_MS,
                TraceEvent::Response {
                    status: 200,
                    body: b"ip=192.0.2.10\r\nloc=US\r\n",
                }
            ),
            &result
        );
        assert_eq!(result.classification(), TraceClassification::Timeout);
    }

    #[test]
    fn egress_probe_request_errors_keep_structural_priority() {
        let mut probe = TraceProbe::new("api.anthropic.com", None);
        let result = probe.observe(
            10,
            TraceEvent::Error(TraceRequestError {
                kind: Some(HostError::Invalid),
                code: Some("ETIMEDOUT"),
                ..TraceRequestError::default()
            }),
        );
        assert_eq!(result.classification(), TraceClassification::Invalid);

        let mut probe = TraceProbe::new("api.anthropic.com", None);
        let result = probe.observe(
            10,
            TraceEvent::Error(TraceRequestError {
                message: Some("connection timed out"),
                ..TraceRequestError::default()
            }),
        );
        assert_eq!(result.classification(), TraceClassification::Timeout);
    }

    #[test]
    fn egress_probe_thaws_a_shared_session_after_both_families_confirm() {
        fn sample(openai_mainland: bool, google_mainland: bool) -> EgressSample {
            EgressSample::new(
                [
                    (Family::OpenAi, openai_mainland),
                    (Family::Google, google_mainland),
                ]
                .into_iter()
                .map(|(family, mainland)| {
                    let (api, login) = family.hosts();
                    let (ip, country) = if mainland {
                        ("198.51.100.20", "CN")
                    } else {
                        ("192.0.2.10", "US")
                    };
                    let exit = Exit {
                        ip: ip.into(),
                        country: country.into(),
                    };
                    let pair = HostPair::new(
                        family,
                        HostSample::success(api, exit.clone()),
                        HostSample::success(login, exit),
                    );
                    (family, pair)
                })
                .collect(),
            )
        }

        let mut monitor = EgressMonitor::default();
        monitor.register_session("shared".into(), &[Family::OpenAi, Family::Google], 0);
        assert_eq!(
            monitor.apply_full_sample(sample(true, true), 0),
            [SessionEffect::Freeze("shared".into())]
        );
        assert!(
            monitor
                .apply_full_sample(sample(true, false), 15_000)
                .is_empty()
        );
        assert!(
            monitor
                .apply_full_sample(sample(true, false), 20_000)
                .is_empty()
        );
        assert!(
            monitor
                .apply_full_sample(sample(false, false), 25_000)
                .is_empty()
        );
        assert_eq!(monitor.recovery_deadline(), Some(30_000));
        assert_eq!(
            monitor.apply_full_sample(sample(false, false), 30_000),
            [SessionEffect::Thaw("shared".into())]
        );
        assert_eq!(monitor.recovery_deadline(), None);
    }
}
