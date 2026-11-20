//! Per-run API Key transport. The URL's unguessable path is the local credential.
use std::{
    borrow::Cow,
    convert::Infallible,
    error::Error as _,
    fmt::Write as _,
    net::Ipv4Addr,
    pin::Pin,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    task::{Context as TaskContext, Poll},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use http_body_util::{BodyExt, Full, LengthLimitError, Limited, combinators::UnsyncBoxBody};
use hyper::{
    HeaderMap, Method, Request, Response, StatusCode, Uri, Version,
    body::{Body, Bytes, Frame, Incoming, SizeHint},
    client::conn::http1 as client,
    header::{self, HeaderValue},
    server::conn::http1 as server,
    service::service_fn,
};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use hyper_util::{
    client::legacy::connect::HttpConnector,
    rt::{TokioIo, TokioTimer},
};
use matou_native_core::provider_launch::{
    ClaudeGatewayConnection, ClaudeGatewayProjection, ClaudeGatewayRole, EFFORTS,
};
use serde_json::{Map, Value, json};
use tokio::{net::TcpListener, sync::oneshot, task::AbortHandle};
use tower_service::Service;

const MAX_REQUEST_BYTES: usize = 64 * 1024 * 1024;
const PATH_ERROR: &str = "Claude request gateway route was not found.";
const LIMIT_ERROR: &str = "Claude request exceeds the gateway size limit.";
const CONNECTION_ERROR: &str = "Claude request gateway could not reach the upstream service.";
const SOURCE_ERROR: &str =
    "Original supplier source connection was not bound at startup; choose a configured role.";

type BodyError = Box<dyn std::error::Error + Send + Sync>;
type GatewayBody = UnsyncBoxBody<Bytes, BodyError>;

pub(crate) struct Gateway {
    base_url: String,
    stopped: Arc<AtomicBool>,
    stop: Option<(oneshot::Sender<()>, thread::JoinHandle<()>)>,
}

impl Gateway {
    pub(crate) fn start(projection: Arc<ClaudeGatewayProjection>) -> Result<Self> {
        Self::start_inner(
            projection,
            #[cfg(test)]
            None,
        )
    }

    fn start_inner(
        projection: Arc<ClaudeGatewayProjection>,
        #[cfg(test)] loopback: Option<std::net::SocketAddr>,
    ) -> Result<Self> {
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(|_| anyhow!("Generate Claude gateway credential"))?;
        let mut prefix = String::with_capacity(65);
        prefix.push('/');
        for byte in random {
            write!(prefix, "{byte:02x}").expect("writing to String");
        }
        let stopped = Arc::new(AtomicBool::new(false));
        let state = Arc::new(State {
            prefix,
            projection,
            stopped: Arc::clone(&stopped),
            #[cfg(test)]
            loopback,
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
            stopped,
            stop: Some((stop, serving_thread)),
        })
    }

    pub(crate) fn base_url(&self) -> &str {
        &self.base_url
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

struct State {
    prefix: String,
    projection: Arc<ClaudeGatewayProjection>,
    stopped: Arc<AtomicBool>,
    #[cfg(test)]
    loopback: Option<std::net::SocketAddr>,
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
        let limited = Limited::new(incoming, MAX_REQUEST_BYTES);
        let (body, length, connection, destination, rewritten) = if is_message || is_count {
            let bytes = limited
                .collect()
                .await
                .map_err(|error| Fault::request_body(error.as_ref()))?
                .to_bytes();
            let (bytes, connection, destination, rewritten) =
                self.rewrite_json(bytes, path, query, &parts.headers)?;
            let length = bytes.len() as u64;
            let body = Full::new(bytes)
                .map_err(|never| match never {})
                .boxed_unsync();
            (body, Some(length), connection, destination, rewritten)
        } else {
            let connection = self
                .projection
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
            let destination =
                destination(connection, path, query).ok_or(Fault::bad_gateway(CONNECTION_ERROR))?;
            (body, size, connection, destination, false)
        };
        let mut key = HeaderValue::from_str(&connection.api_key)
            .map_err(|_| Fault::bad_gateway("Claude model target credential is unavailable."))?;
        key.set_sensitive(true);
        request_headers(
            &mut parts.headers,
            [(header::HeaderName::from_static("x-api-key"), key)],
        );
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
        // Tests remap only the socket destination, after routing, rewriting and Host selection.
        #[cfg(test)]
        let destination = match self
            .loopback
            .filter(|_| destination.host() == Some(deepseek_tools::HOST))
        {
            Some(address) => Uri::builder()
                .scheme("http")
                .authority(address.to_string())
                .path_and_query(destination.path_and_query().unwrap().clone())
                .build()
                .expect("local fixture destination"),
            None => destination,
        };
        send_once(destination, Request::from_parts(parts, body)).await
    }

    fn rewrite_json(
        &self,
        mut body: Bytes,
        path: &str,
        query: Option<&str>,
        headers: &HeaderMap,
    ) -> Result<(Bytes, &ClaudeGatewayConnection, Uri, bool), Fault> {
        let is_message = path == "/v1/messages";
        let mut message = serde_json::from_slice::<Map<String, Value>>(&body).ok();
        let model = message
            .as_ref()
            .and_then(|object| object.get("model"))
            .and_then(Value::as_str);
        let role = model.and_then(|model| self.projection.request_role(model));
        if role.is_none() && model.is_some_and(|model| model.starts_with("matou-role/")) {
            return Err(Fault::bad_gateway("Unknown Claude model role."));
        }
        let search = self.projection.web_search.as_ref().filter(|_| {
            model.is_some()
                && message.as_ref().is_some_and(pure_web_search)
                && !compaction_marked(headers)
        });
        let target =
            search.or_else(|| role.and_then(|index| self.projection.roles[index].as_ref()));
        if target.is_none() && role.is_some() {
            return Err(Fault::bad_gateway(
                "Claude model role target is unavailable.",
            ));
        }
        let connection = target
            .map(|role| &role.connection)
            .or(self.projection.source.as_ref())
            .ok_or(Fault::bad_gateway(SOURCE_ERROR))?;
        let literal_effort = model.and_then(|model| self.projection.literal_effort(model));
        let literal_model = model.and_then(|model| self.projection.literal_model_override(model));
        let mut rewritten = false;
        if let (Some(target), Some(message)) = (target, message.as_mut()) {
            message.insert("model".into(), json!(target.model));
            if is_message {
                apply_role_generation(message, target).map_err(|error| {
                    if search.is_some() {
                        Fault::bad_gateway(format!(
                            "网页搜索 · {}: model target or reasoning effort is unavailable",
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
        if deepseek_tools::matches(&destination)
            && let Some(message) = message.as_mut()
        {
            rewritten |= deepseek_tools::convert(message)
                .map_err(|message| Fault::new(StatusCode::BAD_REQUEST, message))?;
        }
        if rewritten {
            body = serde_json::to_vec(&message)
                .map_err(|_| Fault::bad_gateway(CONNECTION_ERROR))?
                .into();
            if body.len() > MAX_REQUEST_BYTES {
                return Err(Fault::too_large());
            }
        }
        Ok((body, connection, destination, rewritten))
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
) -> Result<(), Fault> {
    let effort = target.effort.as_deref();
    if let Some(effort) = effort
        && target
            .supported_efforts
            .as_ref()
            .is_some_and(|levels| !levels.iter().any(|level| level == effort))
    {
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

fn http_connector() -> HttpConnector {
    let mut connector = HttpConnector::new();
    connector.set_connect_timeout(Some(Duration::from_secs(30)));
    connector
}

async fn send_once(
    destination: Uri,
    request: Request<GatewayBody>,
) -> Result<Response<GatewayBody>, Fault> {
    // The low-level HTTP/1 connection has no redirect, decompression, retry,
    // or pooling layer. Keep the driver owned by this downstream exchange.
    let failure = || Fault::bad_gateway(CONNECTION_ERROR);
    let (mut sender, task) = if destination.scheme_str() == Some("https") {
        static CONNECTOR: OnceLock<Option<HttpsConnector<HttpConnector>>> = OnceLock::new();
        let mut connector = CONNECTOR
            .get_or_init(|| {
                let mut http = http_connector();
                http.enforce_http(false);
                Some(
                    HttpsConnectorBuilder::new()
                        .with_native_roots()
                        .ok()?
                        .https_only()
                        .enable_http1()
                        .wrap_connector(http),
                )
            })
            .as_ref()
            .ok_or_else(failure)?
            .clone();
        connect(connector.call(destination).await.map_err(|_| failure())?).await?
    } else {
        connect(
            http_connector()
                .call(destination)
                .await
                .map_err(|_| failure())?,
        )
        .await?
    };
    let response = sender.send_request(request).await.map_err(|error| {
        if error.is_user()
            && let Some(cause) = error.source()
        {
            Fault::request_body(cause)
        } else {
            failure()
        }
    })?;
    let (mut parts, incoming) = response.into_parts();
    if parts.status.is_redirection() && parts.headers.contains_key(header::LOCATION) {
        return Err(Fault::bad_gateway(
            "Claude request gateway cannot forward an upstream redirect.",
        ));
    }
    filter_hop_by_hop(&mut parts.headers);
    let body = UpstreamBody {
        incoming,
        _connection: task,
    };
    Ok(Response::from_parts(parts, body.boxed_unsync()))
}

struct UpstreamBody {
    incoming: Incoming,
    _connection: AbortOnDrop,
}

impl Body for UpstreamBody {
    type Data = Bytes;
    type Error = BodyError;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, BodyError>>> {
        loop {
            match std::task::ready!(Pin::new(&mut self.incoming).poll_frame(cx)) {
                Some(Ok(frame)) if frame.is_trailers() => continue,
                Some(result) => return Poll::Ready(Some(result.map_err(Into::into))),
                None => return Poll::Ready(None),
            }
        }
    }

    fn size_hint(&self) -> SizeHint {
        self.incoming.size_hint()
    }
}

struct AbortOnDrop(AbortHandle);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn connect<T: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static>(
    socket: T,
) -> Result<(client::SendRequest<GatewayBody>, AbortOnDrop), Fault> {
    let (sender, connection) = client::handshake(socket)
        .await
        .map_err(|_| Fault::bad_gateway(CONNECTION_ERROR))?;
    let task = tokio::spawn(async move {
        let _ = connection.await;
    });
    Ok((sender, AbortOnDrop(task.abort_handle())))
}

struct Fault(StatusCode, Cow<'static, str>);

impl Fault {
    fn new(status: StatusCode, message: &'static str) -> Self {
        Self(status, Cow::Borrowed(message))
    }

    fn bad_gateway(message: impl Into<Cow<'static, str>>) -> Self {
        Self(StatusCode::BAD_GATEWAY, message.into())
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
        let payload = json!({ "type": "error", "error": { "type": kind, "message": self.1 } });
        Response::builder()
            .status(self.0)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::CONNECTION, "close")
            .body(
                Full::new(Bytes::from(payload.to_string()))
                    .map_err(|never| match never {})
                    .boxed_unsync(),
            )
            .expect("static gateway response")
    }
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

#[cfg(test)]
mod tests;
