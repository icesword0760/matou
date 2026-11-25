//! Per-run Claude request transport. The URL's unguessable path is the local credential.
use std::{
    borrow::Cow,
    collections::{HashMap, VecDeque},
    convert::Infallible,
    error::Error as _,
    fmt::Write as _,
    future::{Future, poll_fn},
    io,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc,
    },
    task::{Context as TaskContext, Poll},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow};
use http_body_util::{BodyExt, Full, LengthLimitError, Limited, combinators::UnsyncBoxBody};
use hyper::{
    HeaderMap, Method, Request, Response, StatusCode, Uri, Version,
    body::{Body, Bytes, Frame, Incoming, SizeHint},
    header::{self, HeaderValue},
    server::conn::http1 as server,
    service::service_fn,
};
use hyper_util::rt::{TokioIo, TokioTimer};
use matou_native_core::{
    claude_role_budget::CLAUDE_ROLES,
    provider_launch::{
        ClaudeGatewayCompaction, ClaudeGatewayConnection, ClaudeGatewayProjection,
        ClaudeGatewayRefresh, ClaudeGatewayRole, EFFORTS, decode_claude_gateway_error,
        startup_role_alias,
    },
    sessions::InterfaceLocale,
};
use serde_json::{Map, Value, json};
use tokio::{
    net::TcpListener,
    sync::{Mutex as AsyncMutex, OnceCell, oneshot},
};
use tower_service::Service;
use uuid::Uuid;

use crate::{
    agent_hud_projection::ContextUsageSample,
    egress_probe::Family,
    host::{load_role_preferences, native_role_catalog, role_assignments_revision},
    http_proxy::{AbortOnDrop, connect_http1, direct_https, http_connector},
    model_router::RouterConnection,
    subscription_launch::RouterRequest,
    subscription_service::{
        LaunchIssue, SessionSubscriptions, TargetBinding, launch_issue_message,
    },
};

#[cfg(test)]
#[path = "claude_request_gateway/main_delta_contract.rs"]
mod main_delta_contract;

#[path = "claude_request_gateway/deepseek_token_usage.rs"]
mod deepseek_token_usage;
#[path = "claude_request_gateway/gemini_sse_keepalive.rs"]
mod gemini_sse_keepalive;
mod proxy;
pub(crate) use crate::http_proxy::ProxyEnvironment;

const MAX_REQUEST_BYTES: usize = 64 * 1024 * 1024;
const MAX_AUTH_ERROR_BYTES: usize = 256 * 1024;
const PATH_ERROR: &str = "Claude request gateway route was not found.";
const LIMIT_ERROR: &str = "Claude request exceeds the gateway size limit.";
const CONNECTION_ERROR: &str = "Claude request gateway could not reach the upstream service.";
const SOURCE_ERROR: &str =
    "Original supplier source connection was not bound at startup; choose a configured role.";

type BodyError = Box<dyn std::error::Error + Send + Sync>;
type GatewayBody = UnsyncBoxBody<Bytes, BodyError>;
pub(crate) type ReadContextUsage =
    Arc<dyn Fn() -> Option<(Option<u64>, Option<u64>)> + Send + Sync>;

pub(crate) struct Gateway {
    base_url: String,
    token_usage: Arc<Mutex<deepseek_token_usage::Tracker>>,
    stopped: Arc<AtomicBool>,
    stop: Option<(oneshot::Sender<()>, thread::JoinHandle<()>)>,
}

impl Gateway {
    #[cfg(test)]
    pub(crate) fn start(projection: Arc<ClaudeGatewayProjection>) -> Result<Self> {
        Self::start_inner(
            projection,
            #[cfg(test)]
            None,
        )
    }

    #[cfg(test)]
    fn start_inner(
        projection: Arc<ClaudeGatewayProjection>,
        #[cfg(test)] loopback: Option<std::net::SocketAddr>,
    ) -> Result<Self> {
        Self::start_with_interval(projection, gemini_sse_keepalive::INTERVAL, loopback)
    }

    pub(crate) fn start_with_context(
        projection: Arc<ClaudeGatewayProjection>,
        refresh: Option<(ClaudeGatewayRefresh, PathBuf)>,
        read_context_usage: Option<ReadContextUsage>,
        subscriptions: Option<SessionSubscriptions>,
        blocked_families: Arc<AtomicU8>,
        locale: InterfaceLocale,
        proxy_env: ProxyEnvironment,
    ) -> Result<Self> {
        Self::start_gateway(
            ProjectionSlot::new(projection, subscriptions, refresh),
            gemini_sse_keepalive::INTERVAL,
            read_context_usage,
            blocked_families,
            locale,
            proxy_env,
            #[cfg(test)]
            None,
        )
    }

    #[cfg(test)]
    pub(crate) fn start_with_interval(
        projection: Arc<ClaudeGatewayProjection>,
        keep_alive_interval: Duration,
        loopback: Option<std::net::SocketAddr>,
    ) -> Result<Self> {
        Self::start_gateway(
            ProjectionSlot::new(projection, None, None),
            keep_alive_interval,
            None,
            Arc::default(),
            InterfaceLocale::default(),
            ProxyEnvironment::default(),
            loopback.map(|address| FixtureUpstream {
                host: deepseek_tools::HOST,
                address,
                catalog: None,
            }),
        )
    }

    fn start_gateway(
        projection: ProjectionSlot,
        keep_alive_interval: Duration,
        read_context_usage: Option<ReadContextUsage>,
        blocked_families: Arc<AtomicU8>,
        locale: InterfaceLocale,
        proxy_env: ProxyEnvironment,
        #[cfg(test)] fixture: Option<FixtureUpstream>,
    ) -> Result<Self> {
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|_| anyhow!("Generate Claude gateway credential"))?;
        let mut prefix = String::with_capacity(65);
        prefix.push('/');
        for byte in random {
            write!(prefix, "{byte:02x}").expect("writing to String");
        }
        let stopped = Arc::new(AtomicBool::new(false));
        let token_usage = Arc::default();
        let state = Arc::new(State {
            prefix,
            projection,
            lazy_compaction: OnceCell::new(),
            lazy_subscription: AsyncMutex::default(),
            stopped: Arc::clone(&stopped),
            token_usage: Arc::clone(&token_usage),
            read_context_usage,
            locale,
            blocked_families,
            keep_alive_interval,
            proxy_env,
            #[cfg(test)]
            fixture,
        });
        let (stop, closed) = oneshot::channel();
        let (ready, bound) = mpsc::sync_channel(1);
        let serving_thread = thread::Builder::new()
            .name("claude-request-gateway".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                let result = runtime.and_then(|runtime| {
                    let result = runtime.block_on(async {
                        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
                        let address = listener.local_addr()?;
                        if ready
                            .send(Ok(format!("http://{address}{}", state.prefix)))
                            .is_err()
                        {
                            return Ok(());
                        }
                        let serving = tokio::spawn(serve(listener, state));
                        let _ = closed.await;
                        serving.abort();
                        let _ = serving.await;
                        Ok(())
                    });
                    // Shutdown closes all socket tasks. An OS DNS lookup already
                    // in a blocking pool must not delay listener retirement.
                    runtime.shutdown_background();
                    result
                });
                if result.is_err() {
                    let _ = ready.send(Err(anyhow!("Start Claude request gateway listener")));
                }
            })
            .context("Start Claude request gateway thread")?;
        let base_url = bound
            .recv()
            .context("Claude request gateway listener stopped")??;
        Ok(Self {
            base_url,
            token_usage,
            stopped,
            stop: Some((stop, serving_thread)),
        })
    }

    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
    }

    pub(crate) fn bind_usage_identity(&self, id: &str) {
        self.token_usage
            .lock()
            .unwrap()
            .bind_usage_identity(id.parse().ok());
    }

    pub(crate) fn context_usage(&self) -> Option<ContextUsageSample> {
        self.token_usage.lock().unwrap().context_usage()
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some((stop, serving_thread)) = self.stop.take() {
            let _ = stop.send(());
            // The listener must be closed before a replacement gets its URL.
            let _ = serving_thread.join();
        }
    }
}

struct GatewayRoutes {
    projection: Arc<ClaudeGatewayProjection>,
    subscriptions: Option<SessionSubscriptions>,
}

impl GatewayRoutes {
    fn refresh(&self, context: &ClaudeGatewayRefresh, data_root: &Path) -> Result<Option<Self>> {
        let preferences = load_role_preferences(data_root);
        anyhow::ensure!(!preferences.assignments.is_empty());
        if self.projection.role_assignments() == &preferences.assignments {
            return Ok(None);
        }
        let projection = context
            .refresh_roles(&self.projection, &preferences, &native_role_catalog)
            .context("Claude role projection could not be rebuilt")?;
        let subscriptions = if let Some(previous) = &self.subscriptions {
            let mut targets = HashMap::new();
            for target in projection
                .connections()
                .filter_map(|connection| connection.subscription.as_ref())
            {
                if !targets.contains_key(target) {
                    let binding = match previous.targets.get(target) {
                        Some(binding) => Arc::clone(binding),
                        None => previous.service.prepare_model_target(target)?,
                    };
                    targets.insert(target.clone(), binding);
                }
            }
            Some(SessionSubscriptions {
                service: Arc::clone(&previous.service),
                targets,
            })
        } else {
            anyhow::ensure!(projection.subscription_providers().next().is_none());
            None
        };
        Ok(Some(Self {
            projection: Arc::new(projection),
            subscriptions,
        }))
    }
}

struct ProjectionSlot {
    cache: AsyncMutex<ProjectionCache>,
    refresh: Option<(Arc<ClaudeGatewayRefresh>, PathBuf)>,
}

struct ProjectionCache {
    revision: u64,
    routes: Arc<GatewayRoutes>,
}

impl ProjectionSlot {
    fn new(
        projection: Arc<ClaudeGatewayProjection>,
        subscriptions: Option<SessionSubscriptions>,
        refresh: Option<(ClaudeGatewayRefresh, PathBuf)>,
    ) -> Self {
        Self {
            // A save can finish between launch planning and listener bind.
            // Compare the database with the launch assignments on the first request.
            cache: AsyncMutex::new(ProjectionCache {
                revision: 0,
                routes: Arc::new(GatewayRoutes {
                    projection,
                    subscriptions,
                }),
            }),
            refresh: refresh.map(|(context, data_root)| (Arc::new(context), data_root)),
        }
    }

    async fn current(&self) -> Arc<GatewayRoutes> {
        let mut cached = self.cache.lock().await;
        let Some((context, data_root)) = &self.refresh else {
            return Arc::clone(&cached.routes);
        };
        let revision = role_assignments_revision();
        if cached.revision != revision {
            let current = Arc::clone(&cached.routes);
            let context = Arc::clone(context);
            let data_root = data_root.clone();
            let next = tokio::task::spawn_blocking(move || current.refresh(&context, &data_root));
            match next.await {
                Ok(Ok(Some(routes))) => cached.routes = Arc::new(routes),
                Ok(Ok(None)) => {}
                _ => eprintln!(
                    "[claude-request-gateway] Kept previous role assignments after refresh failed (revision {revision})"
                ),
            }
            cached.revision = revision;
        }
        Arc::clone(&cached.routes)
    }
}

struct State {
    prefix: String,
    projection: ProjectionSlot,
    lazy_compaction: OnceCell<ClaudeGatewayRole>,
    lazy_subscription: AsyncMutex<Option<Arc<TargetBinding>>>,
    locale: InterfaceLocale,
    blocked_families: Arc<AtomicU8>,
    stopped: Arc<AtomicBool>,
    token_usage: Arc<Mutex<deepseek_token_usage::Tracker>>,
    read_context_usage: Option<ReadContextUsage>,
    keep_alive_interval: Duration,
    proxy_env: ProxyEnvironment,
    #[cfg(test)]
    fixture: Option<FixtureUpstream>,
}

#[cfg(test)]
struct FixtureUpstream {
    host: &'static str,
    address: std::net::SocketAddr,
    catalog: Option<fn() -> Vec<Value>>,
}

#[derive(Default)]
struct MessageResponse {
    track_usage: bool,
    hud_scope: Option<HudUsageScope>,
    gemini_stream: bool,
    lazy_compaction: bool,
    authentication_label: Option<&'static str>,
}

#[derive(Clone, Copy)]
struct HudUsageScope {
    identity: Uuid,
    started: Instant,
    at_ms: f64,
    window: Option<u64>,
    estimated_ttl_sec: Option<u64>,
}

fn request_session_id(message: &Map<String, Value>) -> Option<Uuid> {
    let metadata = message.get("metadata")?;
    let user = metadata["user_id"].as_str()?;
    let user: Value = serde_json::from_str(user).ok()?;
    // Requests from a sidechain may share a gateway. Only accept requests
    // naming the bound conversation, without an explicit child identity.
    for key in ["agent_id", "subagent_id", "parent_session_id"] {
        if !metadata[key].is_null() || !user[key].is_null() {
            return None;
        }
    }
    user["session_id"].as_str()?.parse().ok()
}

fn estimated_cache_ttl(connection: &ClaudeGatewayConnection, destination: &Uri) -> Option<u64> {
    if let Some(subscription) = &connection.subscription {
        return match subscription.provider {
            "codex" => Some(300),
            "antigravity" => Some(900),
            _ => None,
        };
    }
    match destination.host()?.to_ascii_lowercase().as_str() {
        "api.anthropic.com" | "api.openai.com" | "generativelanguage.googleapis.com" => Some(300),
        "api.deepseek.com" => Some(600),
        _ => None,
    }
}

async fn serve(listener: TcpListener, state: Arc<State>) {
    loop {
        let Ok((socket, _)) = listener.accept().await else {
            tokio::time::sleep(Duration::from_millis(25)).await;
            continue;
        };
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            let service = service_fn(move |request| {
                let state = Arc::clone(&state);
                async move { Ok::<_, Infallible>(state.forward(request).await) }
            });
            // A downstream EOF cancels the service future and drops the
            // upstream connection task, including before response headers.
            let _ = server::Builder::new()
                .timer(TokioTimer::new())
                .half_close(false)
                .serve_connection(TokioIo::new(socket), service)
                .await;
        });
    }
}

impl State {
    async fn forward(&self, request: Request<Incoming>) -> Response<GatewayBody> {
        match self.try_forward(request).await {
            Ok(response) => response,
            Err(fault) => fault.response(),
        }
    }

    async fn try_forward(
        &self,
        request: Request<Incoming>,
    ) -> Result<Response<GatewayBody>, Fault> {
        let (mut parts, incoming) = request.into_parts();
        let (path, query) = self
            .route(&parts.uri)
            .ok_or(Fault::new(StatusCode::NOT_FOUND, PATH_ERROR))?;
        let size = incoming.size_hint().exact();
        if size.is_some_and(|size| size > MAX_REQUEST_BYTES as u64) {
            return Err(Fault::too_large());
        }
        let is_message = parts.method == Method::POST && path == "/v1/messages";
        let is_count = parts.method == Method::POST && path == "/v1/messages/count_tokens";
        let routes = self.projection.current().await;
        let projection = &routes.projection;
        let limited = Limited::new(incoming, MAX_REQUEST_BYTES);
        let (body, buffered, length, connection, destination, rewritten, message_response) =
            if is_message || is_count {
                let bytes = limited
                    .collect()
                    .await
                    .map_err(|error| Fault::request_body(error.as_ref()))?
                    .to_bytes();
                let (bytes, connection, destination, rewritten, message_response) = self
                    .rewrite_json(projection, bytes, path, query, &parts.headers)
                    .await?;
                let length = bytes.len() as u64;
                let body = full_body(bytes.clone());
                (
                    body,
                    Some(bytes),
                    Some(length),
                    connection,
                    destination,
                    rewritten,
                    message_response,
                )
            } else {
                let connection = projection
                    .source
                    .as_ref()
                    .ok_or(Fault::bad_gateway(SOURCE_ERROR))?;
                let body = limited
                    .map_frame(|frame| {
                        if frame.is_trailers() {
                            Frame::data(Bytes::new())
                        } else {
                            frame
                        }
                    })
                    .boxed_unsync();
                let destination = destination(connection, path, query)
                    .ok_or(Fault::bad_gateway(CONNECTION_ERROR))?;
                (
                    body,
                    None,
                    size,
                    connection,
                    destination,
                    false,
                    MessageResponse::default(),
                )
            };
        self.check_egress(connection, &destination)?;
        if connection.subscription.is_some() {
            if !matches!(
                path,
                "/v1/messages" | "/v1/messages/count_tokens" | "/v1/models"
            ) {
                return Err(Fault::new(StatusCode::NOT_FOUND, PATH_ERROR));
            }
            request_headers(&mut parts.headers, []);
            parts.headers.insert(
                header::ACCEPT_ENCODING,
                HeaderValue::from_static("identity"),
            );
        } else if let Some(official) = &connection.official_oauth {
            let official = official.clone();
            let token = tokio::task::spawn_blocking(move || official.read_token())
                .await
                .map_err(|_| Fault::bad_gateway(CONNECTION_ERROR))?
                .map_err(|error| Fault::bad_gateway(error.to_string()))?;
            let mut bearer = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
                Fault::bad_gateway("Claude model target credential is unavailable.")
            })?;
            bearer.set_sensitive(true);
            request_headers(
                &mut parts.headers,
                [
                    (header::AUTHORIZATION, bearer),
                    (
                        header::HeaderName::from_static("anthropic-beta"),
                        HeaderValue::from_static("oauth-2025-04-20"),
                    ),
                ],
            );
            parts.headers.insert(
                header::ACCEPT_ENCODING,
                HeaderValue::from_static("identity"),
            );
        } else {
            let mut key = HeaderValue::from_str(&connection.api_key).map_err(|_| {
                Fault::bad_gateway("Claude model target credential is unavailable.")
            })?;
            key.set_sensitive(true);
            request_headers(
                &mut parts.headers,
                [(header::HeaderName::from_static("x-api-key"), key)],
            );
        }
        let track_deepseek = message_response.track_usage && deepseek_tools::matches(&destination);
        if track_deepseek {
            remove_brotli(&mut parts.headers);
        }
        if rewritten {
            for name in ["content-md5", "content-digest", "digest"] {
                parts.headers.remove(name);
            }
        }
        // The outgoing framing describes the forwarded body. Hyper otherwise
        // assumes an unknown-length GET or HEAD has no body.
        if let Some(length) = length {
            if length > 0 || rewritten || parts.headers.contains_key(header::CONTENT_LENGTH) {
                parts
                    .headers
                    .insert(header::CONTENT_LENGTH, HeaderValue::from(length));
            }
        } else {
            parts.version = Version::HTTP_11;
            parts.headers.remove(header::CONTENT_LENGTH);
            parts.headers.insert(
                header::TRANSFER_ENCODING,
                HeaderValue::from_static("chunked"),
            );
        }
        parts.headers.insert(
            header::HOST,
            HeaderValue::from_str(
                destination
                    .authority()
                    .ok_or(Fault::bad_gateway(CONNECTION_ERROR))?
                    .as_str(),
            )
            .map_err(|_| Fault::bad_gateway(CONNECTION_ERROR))?,
        );
        parts.uri = Uri::from_parts({
            let mut origin_form = hyper::http::uri::Parts::default();
            origin_form.path_and_query = destination.path_and_query().cloned();
            origin_form
        })
        .map_err(|_| Fault::bad_gateway(CONNECTION_ERROR))?;
        if self.stopped.load(Ordering::Acquire) {
            return Err(Fault::new(StatusCode::NOT_FOUND, PATH_ERROR));
        }
        let label = message_response.authentication_label.unwrap_or("main");
        let (mut response, mut body) = if connection.subscription.is_some() {
            let subscriptions = routes.subscriptions.as_ref().ok_or_else(|| {
                Fault::bad_gateway(LaunchIssue::ServiceNotStarted.message(self.locale))
            })?;
            let (response, body, binding) = self
                .send_subscription(
                    subscriptions,
                    connection,
                    &destination,
                    parts,
                    body,
                    buffered,
                    message_response.lazy_compaction,
                )
                .await?;
            if response.status.is_client_error() || response.status.is_server_error() {
                return self
                    .forward_authenticated_error(
                        &binding.provider_name,
                        Some((subscriptions, &binding)),
                        response,
                        body,
                        label,
                    )
                    .await;
            }
            (response, body)
        } else {
            self.check_egress(connection, &destination)?;
            let (response, body) = send_once(
                destination.clone(),
                Request::from_parts(parts, body),
                &self.proxy_env,
                #[cfg(test)]
                self.fixture
                    .as_ref()
                    .filter(|fixture| destination.host() == Some(fixture.host))
                    .map(|fixture| fixture.address),
            )
            .await?;
            if let Some(official) = &connection.official_oauth
                && (response.status.is_client_error() || response.status.is_server_error())
            {
                return self
                    .forward_authenticated_error(
                        &official.provider_name,
                        None,
                        response,
                        body,
                        label,
                    )
                    .await;
            }
            (response, body)
        };
        if response.status.is_success()
            && let Some(scope) = message_response.hud_scope
        {
            // An unobservable response supersedes older request evidence. A
            // completed observer fills in this same request if usage is present.
            self.token_usage
                .lock()
                .unwrap()
                .publish_context_usage(scope, None);
        }
        if (message_response.track_usage || message_response.gemini_stream)
            && response.status.is_success()
            && let Some(media) = observable_media(&response.headers)
            && let Some(encoding) = observable_encoding(&response.headers)
        {
            if message_response.track_usage {
                body.usage = Some(
                    deepseek_token_usage::ResponseObserver::new(
                        media,
                        encoding,
                        Arc::clone(&self.token_usage),
                    )
                    .with_hud_scope(message_response.hud_scope),
                );
            }
            if message_response.gemini_stream
                && media == ObservableMedia::Sse
                && encoding == Encoding::Identity
            {
                response.headers.remove(header::CONTENT_LENGTH);
                body.keep_alive = Some(gemini_sse_keepalive::KeepAlive::new(
                    self.keep_alive_interval,
                ));
            }
        }
        filter_hop_by_hop(&mut response.headers);
        Ok(Response::from_parts(response, body.boxed_unsync()))
    }

    async fn send_subscription(
        &self,
        subscriptions: &SessionSubscriptions,
        target_connection: &ClaudeGatewayConnection,
        destination: &Uri,
        parts: hyper::http::request::Parts,
        body: GatewayBody,
        buffered: Option<Bytes>,
        lazy: bool,
    ) -> Result<
        (
            hyper::http::response::Parts,
            UpstreamBody,
            Arc<TargetBinding>,
        ),
        Fault,
    > {
        let target = target_connection
            .subscription
            .as_ref()
            .expect("subscription target");
        let binding = match subscriptions.targets.get(target) {
            Some(binding) => Arc::clone(binding),
            None if lazy => {
                let mut cached = self.lazy_subscription.lock().await;
                if let Some(binding) = cached.as_ref().filter(|binding| !binding.is_revoked()) {
                    Arc::clone(binding)
                } else {
                    cached.take();
                    let service = Arc::clone(&subscriptions.service);
                    let target = target.clone();
                    let binding =
                        tokio::task::spawn_blocking(move || service.prepare_model_target(&target))
                            .await
                            .map_err(|_| Fault::bad_gateway(CONNECTION_ERROR))?
                            .map_err(|error| {
                                Fault::bad_gateway(launch_issue_message(self.locale, &error))
                            })?;
                    *cached = Some(Arc::clone(&binding));
                    binding
                }
            }
            None => return Err(Fault::bad_gateway("Subscription target was not prepared")),
        };

        let route = |failed_generation| {
            let service = Arc::clone(&subscriptions.service);
            let binding = Arc::clone(&binding);
            async move {
                tokio::task::spawn_blocking(move || service.connection(&binding, failed_generation))
                    .await
                    .map_err(|_| Fault::bad_gateway(CONNECTION_ERROR))?
                    .map_err(|error| Fault::bad_gateway(launch_issue_message(self.locale, &error)))
            }
        };
        let mut revoked = Box::pin(binding.cancelled());
        let connection = with_revocation(revoked.as_mut(), route(None)).await?;
        self.check_egress(target_connection, destination)?;
        let first = with_revocation(
            revoked.as_mut(),
            send_router(&connection, parts.clone(), body),
        )
        .await;

        let (response, mut body) = match first {
            Err(error)
                if error.2.is_some_and(|kind| {
                    RouterRequest {
                        local_router_target: true,
                        body_buffered: buffered.is_some(),
                        recovery_available: true,
                        cancelled: binding.is_revoked(),
                        ..RouterRequest::default()
                    }
                    .should_reconnect(kind)
                }) =>
            {
                self.check_egress(target_connection, destination)?;
                let recovered =
                    with_revocation(revoked.as_mut(), route(Some(connection.generation))).await?;
                self.check_egress(target_connection, destination)?;
                with_revocation(
                    revoked.as_mut(),
                    send_router(
                        &recovered,
                        parts,
                        full_body(buffered.expect("replayable request")),
                    ),
                )
                .await
            }
            result => result,
        }
        .map_err(|mut fault| {
            if fault.1 == CONNECTION_ERROR {
                fault.1 = Cow::Borrowed(self.locale.text(
                    "连接订阅服务失败：本地服务没有响应，请重新打开设置或重启码头",
                    "Couldn't connect to the subscription service: the local service didn't respond. Reopen Settings or restart Matou.",
                ));
            }
            fault
        })?;
        drop(revoked);
        let active = Arc::clone(&binding);
        body.cancelled = Some(Box::pin(async move { active.cancelled().await }));
        Ok((response, body, binding))
    }

    async fn forward_authenticated_error(
        &self,
        provider_name: &str,
        subscription: Option<(&SessionSubscriptions, &Arc<TargetBinding>)>,
        mut response: hyper::http::response::Parts,
        mut body: UpstreamBody,
        label: &str,
    ) -> Result<Response<GatewayBody>, Fault> {
        let status = response.status;
        let expired = matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN);
        if !expired {
            filter_hop_by_hop(&mut response.headers);
            if body.size_hint().lower() > MAX_AUTH_ERROR_BYTES as u64 {
                return Ok(Response::from_parts(response, body.boxed_unsync()));
            }
            let mut buffered = Vec::new();
            while let Some(frame) = body.frame().await {
                let frame = frame.map_err(|_| {
                    Fault::bad_gateway(
                        "Claude request gateway could not read the upstream response.",
                    )
                })?;
                if let Ok(data) = frame.into_data() {
                    if data.len() > MAX_AUTH_ERROR_BYTES - buffered.len() {
                        // Preserve the already-read bytes and resume streaming on overflow.
                        body.prefetched = VecDeque::from([buffered.into(), data]);
                        return Ok(Response::from_parts(response, body.boxed_unsync()));
                    }
                    buffered.extend_from_slice(&data);
                }
            }
            if !explicit_authentication_error(&response.headers, &buffered) {
                body.prefetched.push_back(buffered.into());
                return Ok(Response::from_parts(response, body.boxed_unsync()));
            }
        }

        drop(body);
        let label = if label == "webSearch" {
            self.locale.text("网页搜索", "Web search")
        } else {
            label
        };
        let guidance = self.locale.text(
            "登录已过期，请到 设置 → Claude Code 重新登录后重试。",
            "sign-in has expired. Go to Settings → Claude Code, sign in again, then retry.",
        );
        let message = format!("{label} · {provider_name} {guidance}");
        if let Some((subscriptions, binding)) = subscription {
            let service = Arc::clone(&subscriptions.service);
            let binding = Arc::clone(binding);
            // Account-state notification failures must not replace the sanitized response.
            let _ = tokio::task::spawn_blocking(move || service.mark_needs_sign_in(&binding)).await;
        }
        Ok(error_response(status, "authentication_error", &message))
    }

    fn check_egress(
        &self,
        connection: &ClaudeGatewayConnection,
        destination: &Uri,
    ) -> Result<(), Fault> {
        let family = if connection
            .subscription
            .as_ref()
            .is_some_and(|target| target.provider == "codex")
        {
            Some(Family::ChatgptSubscription)
        } else if connection.official_oauth.is_some()
            || destination
                .host()
                .is_some_and(|host| host.eq_ignore_ascii_case("api.anthropic.com"))
        {
            Some(Family::Anthropic)
        } else {
            None
        };
        if family.is_some_and(|family| {
            self.blocked_families.load(Ordering::Acquire) & (1 << family as u8) != 0
        }) {
            return Err(Fault::bad_gateway(
                "出口异常，已暂停。请在网络检查中恢复出口或明确豁免后重试。 Network egress protection is active.",
            ));
        }
        Ok(())
    }

    async fn rewrite_json<'a>(
        &'a self,
        projection: &'a ClaudeGatewayProjection,
        mut body: Bytes,
        path: &str,
        query: Option<&str>,
        headers: &HeaderMap,
    ) -> Result<(Bytes, &'a ClaudeGatewayConnection, Uri, bool, MessageResponse), Fault> {
        let is_message = path == "/v1/messages";
        let mut message = serde_json::from_slice::<Map<String, Value>>(&body).ok();
        let request_identity = is_message
            .then(|| message.as_ref().and_then(request_session_id))
            .flatten();
        let model = message
            .as_ref()
            .and_then(|object| object.get("model"))
            .and_then(Value::as_str);
        let role = model.and_then(|model| projection.request_role(model));
        if role.is_none()
            && model.is_some_and(|model| {
                model.starts_with("matou-role/")
                    && projection.literal_model_override(model).is_none()
            })
        {
            return Err(Fault::bad_gateway("Unknown Claude model role."));
        }
        let marked = compaction_marked(headers);
        if is_message && marked && model.is_some() {
            self.token_usage.lock().unwrap().note_compaction();
        }
        let role_target = role.and_then(|index| projection.role_target(index));
        let compaction = projection
            .compaction
            .as_ref()
            .filter(|_| is_message && marked && model.is_some());
        let search = projection.web_search.as_ref().filter(|_| {
            model.is_some() && message.as_ref().is_some_and(pure_web_search) && !marked
        });
        let custom = match compaction {
            Some(ClaudeGatewayCompaction::Snapshot { target, .. }) => Some(target),
            Some(ClaudeGatewayCompaction::OnFirstRequest(preparation)) => Some(
                self.lazy_compaction
                    .get_or_try_init(|| async move {
                        let preparation = preparation.clone();
                        let catalog: fn() -> Vec<Value> = crate::host::native_role_catalog;
                        #[cfg(test)]
                        let catalog = self
                            .fixture
                            .as_ref()
                            .and_then(|fixture| fixture.catalog)
                            .unwrap_or(catalog);
                        tokio::task::spawn_blocking(move || {
                            preparation
                                .resolve(&catalog)
                                .map_err(|_| Fault::bad_gateway(CONNECTION_ERROR))
                        })
                        .await
                        .map_err(|_| Fault::bad_gateway(CONNECTION_ERROR))?
                    })
                    .await?,
            ),
            None => None,
        };
        let target = custom.or(search).or(role_target);
        let authentication_label = if search.is_some() {
            Some("webSearch")
        } else {
            role.map(|index| CLAUDE_ROLES[index])
                .or_else(|| model.and_then(startup_role_alias))
                .or(compaction.map(|_| "compaction"))
        };
        if target.is_none() && role.is_some() {
            return Err(Fault::bad_gateway(
                "Claude model role target is unavailable.",
            ));
        }
        let connection = target
            .map(|role| &role.connection)
            .or(projection.source.as_ref())
            .ok_or(Fault::bad_gateway(SOURCE_ERROR))?;
        let literal_effort = model.and_then(|model| projection.literal_effort(model));
        let fallback_alias = connection
            .subscription
            .as_ref()
            .filter(|target| target.model.is_none() && model == Some("matou-opus-fallback"))
            .map(|target| format!("{}/matou-opus-fallback", target.prefix()));
        let literal_model = fallback_alias
            .as_deref()
            .or_else(|| model.and_then(|model| projection.literal_model_override(model)));
        let keep_alive = target.is_some_and(|target| target.stream_keep_alive)
            || target.is_none()
                && model.is_some_and(|model| projection.literal_stream_keep_alive(model));
        let target_window = target.and_then(|target| target.context_window).or_else(|| {
            target
                .is_none()
                .then(|| model.and_then(|model| projection.literal_context_window(model)))
                .flatten()
        });
        let mut rewritten = false;
        if let (Some(target), Some(message)) = (target, message.as_mut()) {
            message.insert("model".into(), json!(target.model));
            if is_message {
                let effort = target.effort.as_deref();
                let result = match compaction {
                    Some(ClaudeGatewayCompaction::OnFirstRequest(_)) => {
                        let effort = effort
                            .or(literal_effort)
                            .filter(|effort| EFFORTS.contains(effort) && *effort != "follow");
                        if let Some(effort) = effort {
                            if !supports_effort(target, effort) {
                                return Err(Fault::bad_gateway(CONNECTION_ERROR));
                            }
                            set_effort(message, Some(effort));
                            message.insert("thinking".into(), json!({"type": "adaptive"}));
                        }
                        Ok(())
                    }
                    Some(ClaudeGatewayCompaction::Snapshot {
                        inherit_role_effort: true,
                        ..
                    }) => {
                        let effort = match role_target {
                            Some(role) => role.effort.as_deref(),
                            None => effort,
                        };
                        apply_role_generation(message, target, effort)
                    }
                    _ => apply_role_generation(message, target, effort),
                };
                result.map_err(|error| {
                    if search.is_some() {
                        Fault::bad_gateway(format!(
                            "网页搜索 · {}: model target or reasoning effort is unavailable",
                            target.provider_name
                        ))
                    } else if custom.is_some() {
                        let label = role.map_or("compaction", |index| CLAUDE_ROLES[index]);
                        Fault::bad_gateway(format!(
                            "{label} · {}: model target or reasoning effort is unavailable",
                            target.provider_name
                        ))
                    } else {
                        error
                    }
                })?;
            }
            rewritten = true;
        } else if let Some(message) = message.as_mut() {
            if let Some(model) = literal_model {
                message.insert("model".into(), json!(model));
                rewritten = true;
            }
            if is_message
                && let Some(effort) = literal_effort
                && EFFORTS.contains(&effort)
                && effort != "follow"
            {
                set_effort(message, Some(effort));
                rewritten = true;
            }
        }
        let destination =
            destination(connection, path, query).ok_or(Fault::bad_gateway(CONNECTION_ERROR))?;
        if let Some(message) = message.as_mut() {
            if deepseek_tools::matches(&destination) {
                rewritten |= deepseek_thinking::normalize(message);
                rewritten |= deepseek_tools::convert(message)
                    .map_err(|message| Fault::new(StatusCode::BAD_REQUEST, message))?;
                if is_message && !marked {
                    let (hud_tokens, hud_window) = self
                        .read_context_usage
                        .as_ref()
                        .and_then(|read| read())
                        .unwrap_or_default();
                    rewritten |= self.token_usage.lock().unwrap().attach(
                        message,
                        deepseek_token_usage::ContextUsage {
                            session_window: projection.context_window,
                            hud_tokens,
                            hud_window,
                            target_window,
                        },
                    );
                }
            } else if connection.official_oauth.is_none()
                && destination
                    .host()
                    .is_none_or(|host| !host.eq_ignore_ascii_case("api.anthropic.com"))
                && (connection.subscription.is_some()
                    || message
                        .get("model")
                        .and_then(Value::as_str)
                        .is_some_and(|model| {
                            !model.trim().to_ascii_lowercase().starts_with("claude")
                        }))
            {
                rewritten |= strip_unsupported_deferred_tools(message);
            }
        }
        let message_response = MessageResponse {
            track_usage: is_message && !marked,
            hud_scope: request_identity
                .filter(|_| !marked && search.is_none())
                .map(|identity| HudUsageScope {
                    identity,
                    started: Instant::now(),
                    at_ms: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as f64,
                    window: target_window.or(projection.context_window),
                    estimated_ttl_sec: estimated_cache_ttl(connection, &destination),
                }),
            lazy_compaction: matches!(compaction, Some(ClaudeGatewayCompaction::OnFirstRequest(_))),
            authentication_label,
            gemini_stream: is_message
                && message.as_ref().is_some_and(|message| {
                    message.get("stream").and_then(Value::as_bool) == Some(true)
                        && (keep_alive
                            || message
                                .get("model")
                                .and_then(Value::as_str)
                                .is_some_and(gemini_sse_keepalive::is_gemini_model))
                }),
        };
        if rewritten {
            body = serde_json::to_vec(&message)
                .map_err(|_| Fault::bad_gateway(CONNECTION_ERROR))?
                .into();
            if body.len() > MAX_REQUEST_BYTES {
                return Err(Fault::too_large());
            }
        }
        Ok((body, connection, destination, rewritten, message_response))
    }

    fn route<'a>(&self, uri: &'a Uri) -> Option<(&'a str, Option<&'a str>)> {
        if self.stopped.load(Ordering::Acquire)
            || uri.scheme().is_some()
            || uri.authority().is_some()
            || uri.path_and_query()?.as_str().contains('#')
        {
            return None;
        }
        let path = uri.path().strip_prefix(&self.prefix)?;
        if !path.starts_with('/') || path.starts_with("//") || path.contains('\\') {
            return None;
        }
        let folded = path.to_ascii_lowercase();
        if ["%2f", "%5c", "%25"]
            .iter()
            .any(|part| folded.contains(part))
            || folded
                .split('/')
                .any(|part| matches!(part.replace("%2e", ".").as_str(), "." | ".."))
        {
            return None;
        }
        Some((path, uri.query()))
    }
}

fn strip_unsupported_deferred_tools(message: &mut Map<String, Value>) -> bool {
    let choice = message.get("tool_choice");
    let choice_type = choice
        .and_then(|choice| choice.get("type"))
        .and_then(Value::as_str);
    let forced_choice = matches!(choice_type, Some("any" | "tool"));
    let forced_tool = choice
        .filter(|_| choice_type == Some("tool"))
        .and_then(|choice| choice.get("name"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let Some(tools) = message
        .get_mut("tools")
        .and_then(Value::as_array_mut)
        .filter(|tools| !tools.is_empty())
    else {
        return false;
    };

    let mut rewritten = false;
    let mut removed_forced_tool = false;
    tools.retain_mut(|tool| {
        let Some(tool) = tool.as_object_mut() else {
            return true;
        };
        let name = tool.get("name").and_then(Value::as_str);
        if matches!(
            name,
            Some("DeferredToolPlaceholder" | "tool_search_deferred_anchor")
        ) && tool.get("defer_loading").and_then(Value::as_bool) == Some(true)
        {
            rewritten = true;
            removed_forced_tool |= name == forced_tool.as_deref();
            return false;
        }
        rewritten |= tool.shift_remove("defer_loading").is_some();
        true
    });

    if tools.is_empty() && (!forced_choice || removed_forced_tool) {
        message.shift_remove("tools");
    }
    if removed_forced_tool {
        message.shift_remove("tool_choice");
    }
    rewritten
}

fn pure_web_search(message: &Map<String, Value>) -> bool {
    message
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| {
            !tools.is_empty()
                && tools.iter().all(|tool| {
                    tool.as_object()
                        .and_then(|tool| tool.get("type"))
                        .and_then(Value::as_str)
                        .is_some_and(|kind| kind.starts_with("web_search_"))
                })
        })
}

fn compaction_marked(headers: &HeaderMap) -> bool {
    let all = headers.get_all("x-claude-code-compaction");
    let mut values = all.iter();
    values
        .next()
        .is_some_and(|value| matches!(value.as_bytes(), b"manual" | b"auto"))
        && values.next().is_none()
}

fn destination(
    connection: &ClaudeGatewayConnection,
    path: &str,
    query: Option<&str>,
) -> Option<Uri> {
    let base = &connection.endpoint;
    let mut url = format!(
        "{}{}{}",
        &base[..url::Position::BeforePath],
        base.path().trim_end_matches('/'),
        path
    );
    let queries: Vec<&str> = [base.query(), query]
        .into_iter()
        .flatten()
        .filter(|query| !query.is_empty())
        .collect();
    if !queries.is_empty() {
        url.push('?');
        url.push_str(&queries.join("&"));
    }
    url.parse().ok()
}

fn apply_role_generation(
    message: &mut Map<String, Value>,
    target: &ClaudeGatewayRole,
    effort: Option<&str>,
) -> Result<(), Fault> {
    if effort.is_some_and(|effort| !supports_effort(target, effort)) {
        return Err(Fault::bad_gateway(
            "Claude model target reasoning effort is unavailable.",
        ));
    }
    set_effort(message, effort);
    if effort.is_some() && target.model.starts_with("claude-") {
        message.insert("thinking".into(), json!({ "type": "adaptive" }));
    } else {
        message.remove("thinking");
    }
    if let Some(cap) = target.max_output_tokens
        && message
            .get("max_tokens")
            .and_then(Value::as_f64)
            .is_some_and(|tokens| tokens > cap as f64)
    {
        message.insert("max_tokens".into(), json!(cap));
    }
    Ok(())
}

fn supports_effort(target: &ClaudeGatewayRole, effort: &str) -> bool {
    target
        .supported_efforts
        .as_ref()
        .is_none_or(|levels| levels.iter().any(|level| level == effort))
}

fn set_effort(message: &mut Map<String, Value>, effort: Option<&str>) {
    let mut config = match message.remove("output_config") {
        Some(Value::Object(config)) => config,
        _ => Map::new(),
    };
    if let Some(effort) = effort {
        config.insert("effort".into(), json!(effort));
    } else {
        config.remove("effort");
    }
    if !config.is_empty() {
        message.insert("output_config".into(), Value::Object(config));
    }
}

fn filter_hop_by_hop(headers: &mut HeaderMap) {
    let named = headers
        .get_all(header::CONNECTION)
        .iter()
        .flat_map(|value| value.as_bytes().split(|byte| *byte == b','))
        .filter_map(|name| header::HeaderName::from_bytes(name.trim_ascii()).ok())
        .collect::<Vec<_>>();
    for name in named {
        headers.remove(name);
    }
    for name in [
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "proxy-connection",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
    ] {
        headers.remove(name);
    }
}

fn request_headers<const N: usize>(
    headers: &mut HeaderMap,
    target: [(header::HeaderName, HeaderValue); N],
) {
    static AUTH_HEADER: OnceLock<regex::Regex> = OnceLock::new();
    let auth = AUTH_HEADER.get_or_init(|| {
        regex::Regex::new(r"(^|[-_])(auth(entication|orization)?|oauth|(api|subscription|access)[-_]?key|token|secret|credentials?|signature)($|[-_])")
            .expect("static auth header pattern")
    });
    filter_hop_by_hop(headers);
    let sensitive = headers
        .keys()
        .filter(|name| {
            matches!(name.as_str(), "host" | "expect" | "cookie") || auth.is_match(name.as_str())
        })
        .cloned()
        .collect::<Vec<_>>();
    for name in sensitive {
        headers.remove(name);
    }
    const BETA: &str = "anthropic-beta";
    let mut beta = headers.get_all(BETA).iter().cloned().collect::<Vec<_>>();
    for (name, value) in target {
        if name == BETA {
            beta.push(value);
        } else {
            headers.insert(name, value);
        }
    }
    if !beta.is_empty() {
        let mut unique = Vec::new();
        for entry in &beta {
            for value in entry
                .as_bytes()
                .split(|byte| *byte == b',')
                .map(<[u8]>::trim_ascii)
            {
                if !value.is_empty() && !unique.contains(&value) {
                    unique.push(value);
                }
            }
        }
        headers.remove(BETA);
        if let Ok(value) = HeaderValue::from_bytes(&unique.join(&b',')) {
            headers.insert(BETA, value);
        }
    }
}

fn remove_brotli(headers: &mut HeaderMap) {
    let mut removed = false;
    let mut kept: Vec<&[u8]> = Vec::new();
    for token in headers
        .get_all(header::ACCEPT_ENCODING)
        .iter()
        .flat_map(|value| value.as_bytes().split(|byte| *byte == b','))
        .map(<[u8]>::trim_ascii)
        .filter(|token| !token.is_empty())
    {
        let coding = token.split(|byte| *byte == b';').next().unwrap_or_default();
        if coding.trim_ascii().eq_ignore_ascii_case(b"br") {
            removed = true;
        } else {
            kept.push(token);
        }
    }
    if removed {
        let value = if kept.is_empty() {
            HeaderValue::from_static("gzip, deflate")
        } else {
            let joined = kept.join(&b", "[..]);
            HeaderValue::from_bytes(&joined).expect("validated header values joined with commas")
        };
        headers.insert(header::ACCEPT_ENCODING, value);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ObservableMedia {
    Sse,
    Json,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Encoding {
    Identity,
    Gzip,
    Deflate,
}

fn observable_encoding(headers: &HeaderMap) -> Option<Encoding> {
    let mut encodings = headers.get_all(header::CONTENT_ENCODING).iter();
    let encoding = encodings
        .next()
        .map_or(&[][..], |value| value.as_bytes().trim_ascii());
    if encodings.next().is_some() {
        return None;
    }
    if encoding.is_empty() || encoding.eq_ignore_ascii_case(b"identity") {
        Some(Encoding::Identity)
    } else if encoding.eq_ignore_ascii_case(b"gzip") {
        Some(Encoding::Gzip)
    } else if encoding.eq_ignore_ascii_case(b"deflate") {
        Some(Encoding::Deflate)
    } else {
        None
    }
}

fn observable_media(headers: &HeaderMap) -> Option<ObservableMedia> {
    let mut json = false;
    for value in headers.get_all(header::CONTENT_TYPE) {
        let contains = |needle: &[u8]| {
            value
                .as_bytes()
                .windows(needle.len())
                .any(|part| part.eq_ignore_ascii_case(needle))
        };
        if contains(b"text/event-stream") {
            return Some(ObservableMedia::Sse);
        }
        json |= contains(b"application/json");
    }
    json.then_some(ObservableMedia::Json)
}

fn full_body(bytes: Bytes) -> GatewayBody {
    Full::new(bytes)
        .map_err(|never| match never {})
        .boxed_unsync()
}

fn explicit_authentication_error(headers: &HeaderMap, body: &[u8]) -> bool {
    static EXPIRED_MESSAGE: OnceLock<regress::Regex> = OnceLock::new();
    const EXPIRED_CODES: [&str; 6] = [
        "invalid_grant",
        "invalid_token",
        "token_expired",
        "authentication_error",
        "login_required",
        "reauthentication_required",
    ];
    let decoded;
    let body = match headers.get(header::CONTENT_ENCODING) {
        None => body,
        Some(encoding) if encoding.as_bytes().eq_ignore_ascii_case(b"identity") => body,
        Some(encoding) => {
            let Ok(encoding) = encoding.to_str() else {
                return false;
            };
            let Some(bytes) = decode_claude_gateway_error(
                &encoding.trim().to_ascii_lowercase(),
                body,
                MAX_AUTH_ERROR_BYTES,
            ) else {
                return false;
            };
            decoded = bytes;
            &decoded
        }
    };
    let Ok(payload) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    let error = &payload["error"];
    ["code", "type"]
        .into_iter()
        .filter_map(|field| error[field].as_str())
        .any(|code| EXPIRED_CODES.contains(&code))
        || error["message"].as_str().is_some_and(|message| {
            let pattern = EXPIRED_MESSAGE.get_or_init(|| {
                regress::Regex::with_flags(
                    r"(?:\b(?:access token|refresh token|oauth token|session|authentication|credentials?)\b.{0,40}\b(?:expired|revoked)\b|\b(?:please|must|need to|required to) (?:re-?authenticate|(?:sign|log) in again)\b)",
                    "i",
                ).expect("static authentication error pattern")
            });
            let units = message.encode_utf16().collect::<Vec<_>>();
            pattern.find_from_ucs2(&units, 0).next().is_some()
        })
}

async fn with_revocation<T>(
    mut cancelled: Pin<&mut impl Future<Output = ()>>,
    send: impl Future<Output = Result<T, Fault>>,
) -> Result<T, Fault> {
    let mut send = std::pin::pin!(send);
    poll_fn(|cx| {
        if cancelled.as_mut().poll(cx).is_ready() {
            Poll::Ready(Err(Fault::bad_gateway(
                "Subscription account changed. Restart this Claude session.",
            )))
        } else {
            send.as_mut().poll(cx)
        }
    })
    .await
}

async fn send_router(
    connection: &RouterConnection,
    mut parts: hyper::http::request::Parts,
    body: GatewayBody,
) -> Result<(hyper::http::response::Parts, UpstreamBody), Fault> {
    let authority = format!("127.0.0.1:{}", connection.port);
    let destination = Uri::builder()
        .scheme("http")
        .authority(authority.as_str())
        .path_and_query(
            parts
                .uri
                .path_and_query()
                .cloned()
                .ok_or(Fault::bad_gateway(PATH_ERROR))?,
        )
        .build()
        .map_err(|_| Fault::bad_gateway(PATH_ERROR))?;
    let mut bearer = HeaderValue::from_str(&format!("Bearer {}", connection.client_key))
        .map_err(|_| Fault::bad_gateway(CONNECTION_ERROR))?;
    bearer.set_sensitive(true);
    parts.headers.insert(header::AUTHORIZATION, bearer);
    parts.headers.insert(
        header::HOST,
        HeaderValue::from_str(&authority).map_err(|_| Fault::bad_gateway(CONNECTION_ERROR))?,
    );
    send_once(
        destination,
        Request::from_parts(parts, body),
        &ProxyEnvironment::default(),
        #[cfg(test)]
        None,
    )
    .await
}

async fn send_once(
    destination: Uri,
    mut request: Request<GatewayBody>,
    proxy_env: &ProxyEnvironment,
    #[cfg(test)] fixture: Option<std::net::SocketAddr>,
) -> Result<(hyper::http::response::Parts, UpstreamBody), Fault> {
    // The low-level HTTP/1 connection has no redirect, decompression, retry,
    // or pooling layer. Keep the driver owned by this downstream exchange.
    let (mut sender, task) = if let Some(exchange) =
        proxy::connect_proxy(proxy_env, &destination, &mut request).await?
    {
        exchange
    } else {
        // Only the actual connection is overridden, after route and proxy selection.
        #[cfg(test)]
        let destination = if let Some(address) = fixture {
            Uri::builder()
                .scheme("http")
                .authority(address.to_string())
                .path_and_query(destination.path_and_query().unwrap().clone())
                .build()
                .expect("local fixture destination")
        } else {
            destination
        };
        if destination.scheme_str() == Some("https") {
            let socket = direct_https(destination)
                .await
                .map_err(|error| Fault::transport(error.as_ref()))?;
            connect_http1(socket).await
        } else {
            let socket = http_connector()
                .call(destination)
                .await
                .map_err(|error| Fault::transport(&error))?;
            connect_http1(socket).await
        }
        .map_err(|error| Fault::transport(&error))?
    };
    let response = sender.send_request(request).await.map_err(|error| {
        if error.is_user()
            && let Some(cause) = error.source()
        {
            Fault::request_body(cause)
        } else if error.is_incomplete_message() || error.is_closed() {
            Fault::connection(io::ErrorKind::ConnectionReset)
        } else {
            Fault::transport(&error)
        }
    })?;
    let (parts, incoming) = response.into_parts();
    if parts.status.is_redirection() && parts.headers.contains_key(header::LOCATION) {
        return Err(Fault::bad_gateway(
            "Claude request gateway cannot forward an upstream redirect.",
        ));
    }
    let body = UpstreamBody {
        incoming,
        prefetched: VecDeque::new(),
        _connection: task,
        usage: None,
        keep_alive: None,
        cancelled: None,
    };
    Ok((parts, body))
}

struct UpstreamBody {
    incoming: Incoming,
    prefetched: VecDeque<Bytes>,
    _connection: AbortOnDrop,
    usage: Option<deepseek_token_usage::ResponseObserver>,
    keep_alive: Option<gemini_sse_keepalive::KeepAlive>,
    cancelled: Option<Pin<Box<dyn Future<Output = ()> + Send>>>,
}

impl UpstreamBody {
    fn finish(&mut self) {
        self.keep_alive = None;
        if let Some(usage) = self.usage.take() {
            usage.finish();
        }
    }
}

impl Body for UpstreamBody {
    type Data = Bytes;
    type Error = BodyError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BodyError>>> {
        if self
            .cancelled
            .as_mut()
            .is_some_and(|signal| signal.as_mut().poll(cx).is_ready())
        {
            self.finish();
            return Poll::Ready(Some(Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Subscription account changed",
            )
            .into())));
        }
        if let Some(bytes) = self.prefetched.pop_front() {
            return Poll::Ready(Some(Ok(Frame::data(bytes))));
        }
        loop {
            match Pin::new(&mut self.incoming).poll_frame(cx) {
                Poll::Ready(Some(Ok(frame))) => {
                    let Some(data) = frame.data_ref() else {
                        continue;
                    };
                    if self.usage.as_mut().is_some_and(|usage| !usage.push(data)) {
                        self.usage = None;
                    }
                    if let Some(keep_alive) = self.keep_alive.as_mut() {
                        keep_alive.observe(data);
                    }
                    // A fixed-length response can complete on its final data
                    // frame without another poll from the downstream writer.
                    if self.incoming.is_end_stream() {
                        self.finish();
                    }
                    return Poll::Ready(Some(Ok(frame)));
                }
                Poll::Ready(Some(Err(error))) => {
                    self.keep_alive = None;
                    self.usage = None;
                    return Poll::Ready(Some(Err(error.into())));
                }
                Poll::Ready(None) => {
                    self.finish();
                    return Poll::Ready(None);
                }
                Poll::Pending => {
                    if let Some(keep_alive) = self.keep_alive.as_mut()
                        && keep_alive.poll_ping(cx).is_ready()
                    {
                        return Poll::Ready(Some(Ok(Frame::data(Bytes::from_static(
                            gemini_sse_keepalive::PING,
                        )))));
                    }
                    return Poll::Pending;
                }
            }
        }
    }

    fn size_hint(&self) -> SizeHint {
        if self.keep_alive.is_some() || !self.prefetched.is_empty() {
            SizeHint::default()
        } else {
            self.incoming.size_hint()
        }
    }
}

struct Fault(StatusCode, Cow<'static, str>, Option<io::ErrorKind>);

impl Fault {
    fn new(status: StatusCode, message: &'static str) -> Self {
        Self(status, Cow::Borrowed(message), None)
    }

    fn bad_gateway(message: impl Into<Cow<'static, str>>) -> Self {
        Self(StatusCode::BAD_GATEWAY, message.into(), None)
    }

    fn connection(kind: io::ErrorKind) -> Self {
        Self(
            StatusCode::BAD_GATEWAY,
            Cow::Borrowed(CONNECTION_ERROR),
            Some(kind),
        )
    }

    fn transport(error: &(dyn std::error::Error + 'static)) -> Self {
        let mut cause = Some(error);
        while let Some(error) = cause {
            if let Some(io_error) = error.downcast_ref::<io::Error>() {
                return Self::connection(io_error.kind());
            }
            cause = error.source();
        }
        Self::bad_gateway(CONNECTION_ERROR)
    }

    fn too_large() -> Self {
        Self::new(StatusCode::PAYLOAD_TOO_LARGE, LIMIT_ERROR)
    }

    fn request_body(error: &(dyn std::error::Error + 'static)) -> Self {
        if error.is::<LengthLimitError>() {
            Self::too_large()
        } else {
            Self::new(
                StatusCode::BAD_REQUEST,
                "Claude request gateway could not read the request.",
            )
        }
    }

    fn response(self) -> Response<GatewayBody> {
        let kind = if self.0.is_server_error() {
            "api_error"
        } else {
            "invalid_request_error"
        };
        error_response(self.0, kind, &self.1)
    }
}

fn error_response(status: StatusCode, kind: &str, message: &str) -> Response<GatewayBody> {
    let payload = json!({ "type": "error", "error": { "type": kind, "message": message } });
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CONNECTION, "close")
        .body(full_body(Bytes::from(payload.to_string())))
        .expect("static gateway response")
}

mod deepseek_tools {
    use std::collections::{HashMap, HashSet};

    use super::{Map, Uri, Value, json};

    pub(super) const HOST: &str = "api.deepseek.com";

    pub(super) fn matches(destination: &Uri) -> bool {
        destination.host() == Some(HOST)
            && matches!(
                destination.path(),
                "/anthropic/v1/messages" | "/anthropic/v1/messages/count_tokens"
            )
    }

    pub(super) fn convert(message: &mut Map<String, Value>) -> Result<bool, &'static str> {
        let mut changed = false;
        if let Some(tools) = message.get_mut("tools").and_then(Value::as_array_mut) {
            for tool in tools.iter_mut().filter_map(Value::as_object_mut) {
                if client_name(tool).is_some() && kind(tool) == Some("custom") {
                    tool.shift_remove("type");
                    changed = true;
                }
            }
        }
        changed |= convert_availability(message)?;
        let mut available = HashSet::new();
        if let Some(tools) = message.get_mut("tools").and_then(Value::as_array_mut) {
            for tool in tools.iter_mut().filter_map(Value::as_object_mut) {
                if client_name(tool).is_some()
                    && let Some(schema) = tool.get_mut("input_schema")
                {
                    changed |= normalize_schema(schema);
                }
                if let Some(name) = tool.get("name").and_then(Value::as_str) {
                    available.insert(name.to_owned());
                }
            }
        }
        if let Some(messages) = message.get_mut("messages").and_then(Value::as_array_mut) {
            for block in role_blocks(messages, "user") {
                if kind(block) == Some("tool_result")
                    && let Some(content) = block.get_mut("content").and_then(Value::as_array_mut)
                {
                    for reference in content.iter_mut().filter_map(Value::as_object_mut) {
                        if kind(reference) == Some("tool_reference")
                            && let Some(name) = reference.get("tool_name").and_then(Value::as_str)
                            && !available.contains(name)
                        {
                            let name = json!(name);
                            reference.shift_remove("tool_name");
                            reference.insert("type".into(), json!("text"));
                            reference.insert("text".into(), json!(format!(
                                "Historical tool discovery: {name} was previously discovered, but its definition is not provided in the current tools list and it is not callable."
                            )));
                            changed = true;
                        }
                    }
                }
            }
        }
        Ok(changed)
    }

    fn kind(object: &Map<String, Value>) -> Option<&str> {
        object.get("type").and_then(Value::as_str)
    }

    fn named(value: &Value) -> Option<&str> {
        value.as_str().filter(|name| {
            // ECMAScript trim includes BOM and excludes NEL.
            !name
                .trim_matches(|ch: char| ch == '\u{feff}' || (ch.is_whitespace() && ch != '\u{85}'))
                .is_empty()
        })
    }

    fn client_name(definition: &Map<String, Value>) -> Option<&str> {
        if definition.get("input_schema").is_some_and(Value::is_object)
            && (definition.get("type").is_none() || kind(definition) == Some("custom"))
        {
            definition.get("name").and_then(named)
        } else {
            None
        }
    }

    fn role_blocks<'a>(
        messages: &'a mut [Value],
        role: &'static str,
    ) -> impl Iterator<Item = &'a mut Map<String, Value>> {
        messages
            .iter_mut()
            .filter(move |entry| entry["role"].as_str() == Some(role))
            .filter_map(|entry| entry.get_mut("content").and_then(Value::as_array_mut))
            .flatten()
            .filter_map(Value::as_object_mut)
    }

    fn inline_definition(block: &Map<String, Value>) -> Option<&Value> {
        let tool = block.get("tool")?.as_object()?;
        (kind(block) == Some("tool_addition") && kind(tool) == Some("tool_definition"))
            .then(|| tool.get("definition").unwrap_or(&Value::Null))
    }

    fn convert_availability(message: &mut Map<String, Value>) -> Result<bool, &'static str> {
        let Some(messages) = message.get_mut("messages").and_then(Value::as_array_mut) else {
            return Ok(false);
        };
        let fold = role_blocks(messages, "system").any(|block| kind(block) == Some("tool_removal"));
        if !fold {
            let mut has_definition = false;
            for definition in
                role_blocks(messages, "system").filter_map(|block| inline_definition(block))
            {
                if definition.as_object().and_then(client_name).is_none() {
                    return Err(
                        "DeepSeek cannot convert this inline tool definition: a named custom tool with input_schema is required.",
                    );
                }
                has_definition = true;
            }
            if !has_definition {
                return Ok(false);
            }
        }

        let tools = match message.get_mut("tools") {
            Some(Value::Array(tools)) => std::mem::take(tools),
            None => Vec::new(),
            Some(_) if fold => {
                return Err("DeepSeek tool availability conversion requires a tools array.");
            }
            Some(_) => return Err("DeepSeek inline tool conversion requires a tools array."),
        };
        let forced_tool = message
            .get("tool_choice")
            .and_then(Value::as_object)
            .filter(|choice| kind(choice) == Some("tool"))
            .and_then(|choice| choice.get("name").and_then(Value::as_str))
            .map(str::to_owned);
        let messages = message
            .get_mut("messages")
            .and_then(Value::as_array_mut)
            .expect("messages array");
        let tools = if fold {
            fold_availability(messages, tools, forced_tool.as_deref())?
        } else {
            promote_definitions(messages, tools)?
        };
        message.insert("tools".into(), Value::Array(tools));
        Ok(true)
    }

    fn definition_fields(
        definition: &Map<String, Value>,
    ) -> impl Iterator<Item = (&String, &Value)> {
        definition.iter().filter(|(key, value)| match key.as_str() {
            "defer_loading" => false,
            "type" => value.as_str() != Some("custom"),
            _ => true,
        })
    }

    // Frozen JSON numbers are JavaScript numbers, and object field order is immaterial.
    fn equivalent_value(left: &Value, right: &Value) -> bool {
        match (left, right) {
            (Value::Object(left), Value::Object(right)) => {
                left.len() == right.len()
                    && left.iter().all(|(key, value)| {
                        right
                            .get(key)
                            .is_some_and(|other| equivalent_value(value, other))
                    })
            }
            (Value::Array(left), Value::Array(right)) => {
                left.len() == right.len()
                    && left
                        .iter()
                        .zip(right)
                        .all(|(value, other)| equivalent_value(value, other))
            }
            (Value::Number(left), Value::Number(right)) => {
                left.as_f64().map(f64::to_bits) == right.as_f64().map(f64::to_bits)
            }
            _ => left == right,
        }
    }

    fn same_definition(left: &Map<String, Value>, right: &Map<String, Value>) -> bool {
        definition_fields(left).count() == definition_fields(right).count()
            && definition_fields(left).all(|(key, value)| {
                right.get(key).is_some_and(|other| {
                    (key != "type" || other.as_str() != Some("custom"))
                        && equivalent_value(value, other)
                })
            })
    }

    fn deferred(definition: &Map<String, Value>) -> bool {
        definition.get("defer_loading").and_then(Value::as_bool) == Some(true)
    }

    fn set_deferred(definition: &mut Map<String, Value>, value: bool) {
        if kind(definition) == Some("custom") {
            definition.shift_remove("type");
        }
        definition.shift_remove("defer_loading");
        definition.insert("defer_loading".into(), Value::Bool(value));
    }

    fn promote_definitions(
        messages: &mut [Value],
        mut tools: Vec<Value>,
    ) -> Result<Vec<Value>, &'static str> {
        for block in role_blocks(messages, "system") {
            let Some(definition) = inline_definition(block).and_then(Value::as_object) else {
                continue;
            };
            let name = client_name(definition).expect("validated client definition");
            let mut found = false;
            for existing in tools
                .iter()
                .filter_map(Value::as_object)
                .filter(|tool| tool.get("name").and_then(Value::as_str) == Some(name))
            {
                if !same_definition(existing, definition) {
                    return Err(
                        "DeepSeek cannot convert inline tools with conflicting definitions for the same name.",
                    );
                }
                found = true;
            }
            if !found {
                let mut definition = definition.clone();
                set_deferred(&mut definition, true);
                tools.push(Value::Object(definition));
            }
            block.insert("tool".into(), json!({"type":"tool_reference","name":name}));
        }
        if !tools
            .iter()
            .filter_map(Value::as_object)
            .any(|tool| !deferred(tool))
        {
            return Err(
                "DeepSeek inline tool conversion requires at least one existing non-deferred tool.",
            );
        }
        Ok(tools)
    }

    struct Registration {
        definition: Map<String, Value>,
        active: bool,
        touched: bool,
    }

    fn fold_availability(
        messages: &mut [Value],
        tools: Vec<Value>,
        forced_tool: Option<&str>,
    ) -> Result<Vec<Value>, &'static str> {
        // Keep the order of first registration even when a definition gets replaced.
        let mut indices = HashMap::new();
        let mut registry = Vec::new();
        for tool in tools {
            let Value::Object(definition) = tool else {
                return Err(
                    "DeepSeek tool availability conversion supports only named client tools with input_schema.",
                );
            };
            let name = client_name(&definition)
                .ok_or("DeepSeek tool availability conversion supports only named client tools with input_schema.")?;
            if indices.insert(name.to_owned(), registry.len()).is_some() {
                return Err(
                    "DeepSeek cannot resolve duplicate top-level tool names during tool availability conversion.",
                );
            }
            registry.push(Registration {
                definition,
                active: true,
                touched: false,
            });
        }
        for block in role_blocks(messages, "system") {
            let addition = match kind(block) {
                Some("tool_addition") => true,
                Some("tool_removal") => false,
                _ => continue,
            };
            let tool = block.get("tool").and_then(Value::as_object);
            let name = if let Some(definition) = inline_definition(block) {
                let definition = definition.as_object().ok_or(
                    "DeepSeek tool availability conversion supports only named client tool definitions with input_schema."
                )?;
                let name = client_name(definition).ok_or(
                    "DeepSeek tool availability conversion supports only named client tool definitions with input_schema."
                )?;
                let entry = Registration {
                    definition: definition.clone(),
                    active: true,
                    touched: true,
                };
                if let Some(&index) = indices.get(name) {
                    registry[index] = entry;
                } else {
                    indices.insert(name.to_owned(), registry.len());
                    registry.push(entry);
                }
                name
            } else {
                let name = tool
                    .filter(|tool| kind(tool) == Some("tool_reference"))
                    .and_then(|tool| tool.get("name"))
                    .and_then(named)
                    .ok_or("DeepSeek cannot resolve this tool availability reference.")?;
                let index = *indices
                    .get(name)
                    .ok_or("DeepSeek cannot resolve an unknown tool availability reference.")?;
                registry[index].active = addition;
                registry[index].touched = true;
                name
            };
            let name = json!(name);
            let action = if addition { "added" } else { "removed" };
            block.shift_remove("tool");
            block.insert("type".into(), json!("text"));
            block.insert("text".into(), json!(format!(
                "Historical tool availability change: {name} was {action}. The tools list in this request determines which tools are currently callable."
            )));
        }
        if forced_tool
            .and_then(|name| indices.get(name))
            .is_some_and(|&index| !registry[index].active)
        {
            return Err(
                "DeepSeek cannot force tool_choice to a tool removed from the current tools list.",
            );
        }
        let mut tools = Vec::new();
        let mut has_eager_tool = false;
        for mut tool in registry.into_iter().filter(|tool| tool.active) {
            if tool.touched {
                set_deferred(&mut tool.definition, false);
            }
            has_eager_tool |= !deferred(&tool.definition);
            tools.push(Value::Object(tool.definition));
        }
        if !tools.is_empty() && !has_eager_tool {
            return Err(
                "DeepSeek tool availability conversion requires at least one non-deferred tool when tools remain.",
            );
        }
        Ok(tools)
    }

    fn normalize_schema(schema: &mut Value) -> bool {
        let Some(schema) = schema.as_object_mut() else {
            return false;
        };
        let mut changed = false;
        for (key, value) in schema {
            match key.as_str() {
                "pattern" => {
                    if let Some(normalized) = value.as_str().and_then(normalize_null_escape) {
                        *value = Value::String(normalized);
                        changed = true;
                    }
                }
                "properties" | "patternProperties" | "$defs" | "definitions"
                | "dependentSchemas" => {
                    if let Some(children) = value.as_object_mut() {
                        for child in children.values_mut() {
                            changed |= normalize_schema(child);
                        }
                    }
                }
                "items" | "allOf" | "anyOf" | "oneOf" | "prefixItems" if value.is_array() => {
                    for child in value.as_array_mut().unwrap() {
                        changed |= normalize_schema(child);
                    }
                }
                "items"
                | "additionalItems"
                | "additionalProperties"
                | "unevaluatedItems"
                | "unevaluatedProperties"
                | "propertyNames"
                | "contains"
                | "not"
                | "if"
                | "then"
                | "else"
                | "contentSchema" => changed |= normalize_schema(value),
                _ => {}
            }
        }
        changed
    }

    fn normalize_null_escape(pattern: &str) -> Option<String> {
        let mut normalized = String::new();
        let mut copied = 0;
        let mut backslashes = 0;
        for (index, byte) in pattern.bytes().enumerate() {
            if byte == b'\\' {
                backslashes += 1;
                continue;
            }
            if byte == b'0'
                && backslashes % 2 == 1
                && !pattern
                    .as_bytes()
                    .get(index + 1)
                    .is_some_and(u8::is_ascii_digit)
            {
                normalized.push_str(&pattern[copied..index - 1]);
                normalized.push_str(r"\x00");
                copied = index + 1;
            }
            backslashes = 0;
        }
        if copied == 0 {
            return None;
        }
        normalized.push_str(&pattern[copied..]);
        Some(normalized)
    }
}

/// DeepSeek validates the assistant history of a reasoning request before it
/// answers, and rejects histories where assistant turns disagree about carrying
/// a `thinking` block with "The `content[].thinking` in the thinking mode must be
/// passed back to the API". Observed rejections include two tool rounds where one
/// assistant turn has no thinking, a `tool_use` turn without thinking next to a
/// turn that has one, and consecutive assistant turns whose first has none;
/// uniform histories pass either way. Sessions that switched providers
/// mid-conversation carry exactly those mixed histories, because turns written
/// through another supplier kept their own thinking blocks or none at all.
/// Filling the gaps with empty thinking blocks makes every assistant turn carry
/// one without rewriting any reasoning text that was actually produced, and the
/// upstream accepts both empty and non-empty thinking blocks side by side.
mod deepseek_thinking {
    use super::{Map, Value, json};

    pub(super) fn normalize(message: &mut Map<String, Value>) -> bool {
        let Some(messages) = message.get_mut("messages").and_then(Value::as_array_mut) else {
            return false;
        };
        let mut changed = false;
        for entry in messages.iter_mut() {
            if entry.get("role").and_then(Value::as_str) != Some("assistant")
                || carries_thinking(entry)
            {
                continue;
            }
            let Some(content) = entry
                .as_object_mut()
                .and_then(|entry| entry.get_mut("content"))
                .and_then(Value::as_array_mut)
            else {
                continue;
            };
            content.insert(
                0,
                json!({"type": "thinking", "thinking": "", "signature": ""}),
            );
            changed = true;
        }
        changed
    }

    fn carries_thinking(entry: &Value) -> bool {
        entry
            .get("content")
            .and_then(Value::as_array)
            .is_some_and(|blocks| {
                blocks
                    .iter()
                    .any(|block| block.get("type").and_then(Value::as_str) == Some("thinking"))
            })
    }
}

#[cfg(test)]
#[path = "claude_request_gateway/tests.rs"]
pub(crate) mod tests;
