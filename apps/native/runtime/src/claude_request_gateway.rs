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
        let (body, length, connection, rewritten) = if is_message || is_count {
            let bytes = limited
                .collect()
                .await
                .map_err(|error| Fault::request_body(error.as_ref()))?
                .to_bytes();
            let (bytes, connection, rewritten) =
                self.rewrite_json(bytes, is_message, &parts.headers)?;
            let length = bytes.len() as u64;
            let body = Full::new(bytes)
                .map_err(|never| match never {})
                .boxed_unsync();
            (body, Some(length), connection, rewritten)
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
            (body, size, connection, false)
        };
        let destination =
            destination(connection, path, query).ok_or(Fault::bad_gateway(CONNECTION_ERROR))?;
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
        send_once(destination, Request::from_parts(parts, body)).await
    }

    fn rewrite_json(
        &self,
        mut body: Bytes,
        is_message: bool,
        headers: &HeaderMap,
    ) -> Result<(Bytes, &ClaudeGatewayConnection, bool), Fault> {
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
        if rewritten {
            body = serde_json::to_vec(&message)
                .map_err(|_| Fault::bad_gateway(CONNECTION_ERROR))?
                .into();
            if body.len() > MAX_REQUEST_BYTES {
                return Err(Fault::too_large());
            }
        }
        Ok((body, connection, rewritten))
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

#[cfg(test)]
mod tests;
