use std::{
    io,
    net::{Ipv4Addr, Ipv6Addr, TcpListener},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use http_body_util::Full;
use hyper::{
    Method, Request, Response, StatusCode,
    body::{Bytes, Incoming},
    server::conn::http1,
    service::service_fn,
};
use hyper_util::rt::TokioIo;
use subtle::ConstantTimeEq;
use tokio::{
    net::{TcpListener as AsyncListener, TcpSocket},
    sync::oneshot,
};
use url::{Host, Url, form_urlencoded};

use crate::{
    model_router::{ManagementClient, RouterConnection},
    subscription_launch::SubscriptionProvider,
};

pub(super) fn callback_endpoint(provider: SubscriptionProvider) -> (u16, &'static str) {
    match provider {
        SubscriptionProvider::ChatGpt => (1455, "/auth/callback"),
        SubscriptionProvider::Antigravity => (51121, "/oauth-callback"),
    }
}

pub(super) fn validate_authorization_url(
    provider: SubscriptionProvider,
    address: &str,
    state: &str,
    port: u16,
) -> Result<()> {
    fn validate(
        provider: SubscriptionProvider,
        address: &str,
        state: &str,
        port: u16,
    ) -> Option<()> {
        let authority = address
            .strip_prefix("https://")?
            .split(['/', '?', '#'])
            .next()?;
        if !matches!(authority, "auth.openai.com" | "accounts.google.com") {
            return None;
        }
        let url = Url::parse(address).ok()?;
        let allowed = match provider {
            SubscriptionProvider::ChatGpt => {
                url.host_str() == Some("auth.openai.com")
                    && matches!(url.path(), "/oauth/authorize" | "/api/accounts/authorize")
            }
            SubscriptionProvider::Antigravity => {
                url.host_str() == Some("accounts.google.com")
                    && matches!(url.path(), "/o/oauth2/v2/auth" | "/o/oauth2/auth")
            }
        };
        if !allowed
            || url.scheme() != "https"
            || url.port().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return None;
        }
        let mut query = url.query_pairs();
        let mut redirects = query.by_ref().filter(|(key, _)| key == "redirect_uri");
        let redirect_address = redirects.next()?.1;
        let redirect_authority = redirect_address
            .strip_prefix("http://")?
            .split(['/', '?', '#'])
            .next()?;
        if redirect_authority.contains('@') {
            return None;
        }
        let redirect = Url::parse(&redirect_address).ok()?;
        if redirects.next().is_some() {
            return None;
        }
        let mut states = url.query_pairs().filter(|(key, _)| key == "state");
        if states.next()?.1 != state || states.next().is_some() {
            return None;
        }
        let loopback = match redirect.host()? {
            Host::Domain(name) => name == "localhost",
            Host::Ipv4(ip) => ip == Ipv4Addr::LOCALHOST,
            Host::Ipv6(ip) => ip == Ipv6Addr::LOCALHOST,
        };
        (loopback
            && redirect.scheme() == "http"
            && redirect.port() == Some(port)
            && redirect.path() == callback_endpoint(provider).1
            && redirect.username().is_empty()
            && redirect.password().is_none()
            && redirect.query().is_none()
            && redirect.fragment().is_none())
        .then_some(())
    }
    ensure!(
        address.len() <= 8192
            && !address.chars().any(char::is_whitespace)
            && validate(provider, address, state, port).is_some(),
        "Subscription authorization address was rejected"
    );
    Ok(())
}

/// Reserve the port before starting OAuth so a conflicting listener never
/// receives the URL for this attempt. The IPv4 socket is mandatory.
pub(super) fn bind(port: u16) -> Result<(u16, Vec<TcpListener>)> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .build()?;
    let _guard = runtime.enter();
    let ipv4 = listen((Ipv4Addr::LOCALHOST, port).into()).map_err(bind_error)?;
    let port = ipv4.local_addr()?.port();
    let mut listeners = vec![ipv4];
    match listen((Ipv6Addr::LOCALHOST, port).into()) {
        Ok(ipv6) => listeners.push(ipv6),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::AddrNotAvailable | io::ErrorKind::Unsupported
            ) || ipv6_unavailable(&error) => {}
        Err(error) => return Err(bind_error(error)),
    }
    Ok((port, listeners))
}

fn listen(address: std::net::SocketAddr) -> io::Result<TcpListener> {
    let socket = if address.is_ipv4() {
        TcpSocket::new_v4()?
    } else {
        TcpSocket::new_v6()?
    };
    // On Unix this permits a new attempt while old callback connections are in TIME_WAIT.
    // Windows SO_REUSEADDR can also allow an unrelated live listener to bind.
    #[cfg(unix)]
    socket.set_reuseaddr(true)?;
    socket.bind(address)?;
    socket.listen(64)?.into_std()
}

fn ipv6_unavailable(error: &io::Error) -> bool {
    #[cfg(unix)]
    return matches!(
        error.raw_os_error(),
        Some(libc::EAFNOSUPPORT | libc::EPROTONOSUPPORT)
    );
    #[cfg(windows)]
    return matches!(error.raw_os_error(), Some(10047 | 10043));
    #[cfg(not(any(unix, windows)))]
    return false;
}

fn bind_error(error: io::Error) -> anyhow::Error {
    if error.kind() == io::ErrorKind::AddrInUse {
        anyhow::anyhow!("Sign-in port is in use. Close the other sign-in window and try again.")
    } else {
        anyhow::anyhow!("Could not listen for the browser sign-in callback")
    }
}

pub(super) struct Callback {
    stop: Option<oneshot::Sender<()>>,
    worker: Option<JoinHandle<()>>,
    pub accepted: Arc<AtomicBool>,
    pub failed: Arc<AtomicBool>,
}

impl Drop for Callback {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Callback {
    pub fn start(
        listeners: Vec<TcpListener>,
        provider: SubscriptionProvider,
        state: String,
        connection: RouterConnection,
        chinese: bool,
    ) -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let listeners = {
            let _guard = runtime.enter();
            listeners
                .into_iter()
                .map(AsyncListener::from_std)
                .collect::<io::Result<Vec<_>>>()?
        };
        let (stop, close) = oneshot::channel();
        let accepted = Arc::new(AtomicBool::new(false));
        let failed = Arc::new(AtomicBool::new(false));
        let flow = Arc::new(CallbackFlow {
            provider,
            state,
            connection,
            accepted: accepted.clone(),
            failed: failed.clone(),
            chinese,
        });
        let worker = thread::Builder::new()
            .name("matou-subscription-callback".into())
            .spawn(move || {
                runtime.block_on(async {
                    for listener in listeners {
                        let flow = flow.clone();
                        tokio::spawn(async move {
                            while let Ok((socket, _)) = listener.accept().await {
                                let flow = flow.clone();
                                tokio::spawn(async move {
                                    let service = service_fn(move |request| {
                                        let flow = flow.clone();
                                        async move {
                                            Ok::<_, std::convert::Infallible>(
                                                flow.respond(request).await,
                                            )
                                        }
                                    });
                                    let mut server = http1::Builder::new();
                                    server.keep_alive(false).max_buf_size(16 * 1024);
                                    let _ = tokio::time::timeout(
                                        Duration::from_secs(20),
                                        server.serve_connection(TokioIo::new(socket), service),
                                    )
                                    .await;
                                });
                            }
                        });
                    }
                    let _ = close.await;
                });
                runtime.shutdown_background();
            })
            .context("Start subscription callback listener")?;
        Ok(Self {
            stop: Some(stop),
            worker: Some(worker),
            accepted,
            failed,
        })
    }
}

struct CallbackFlow {
    provider: SubscriptionProvider,
    state: String,
    connection: RouterConnection,
    accepted: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
    chinese: bool,
}

impl CallbackFlow {
    async fn respond(self: Arc<Self>, request: Request<Incoming>) -> Response<Full<Bytes>> {
        let page = |status, ok| callback_page(status, ok, self.chinese);
        let uri = request.uri();
        if request.method() != Method::GET
            || uri.scheme().is_some()
            || uri.authority().is_some()
            || uri.to_string().len() > 8192
        {
            return page(StatusCode::BAD_REQUEST, false);
        }
        if uri.path() != callback_endpoint(self.provider).1 {
            return page(StatusCode::NOT_FOUND, false);
        }
        let Some((parameter, value)) = parse_query(uri.query().unwrap_or_default(), &self.state)
        else {
            return page(StatusCode::BAD_REQUEST, false);
        };
        if self.accepted.swap(true, Ordering::AcqRel) {
            return page(StatusCode::CONFLICT, false);
        }
        let flow = self.clone();
        let sent = tokio::task::spawn_blocking(move || {
            ManagementClient::new(&flow.connection).oauth_callback(
                flow.provider,
                &flow.state,
                parameter,
                &value,
            )
        })
        .await
        .is_ok_and(|result| result.is_ok());
        if !sent || parameter == "error" {
            self.failed.store(true, Ordering::Release);
        }
        page(
            if sent && parameter == "code" {
                StatusCode::OK
            } else {
                StatusCode::BAD_REQUEST
            },
            sent && parameter == "code",
        )
    }
}

fn parse_query(query: &str, expected: &str) -> Option<(&'static str, String)> {
    let (mut state, mut code, mut error) = (None, None, None);
    for (key, value) in form_urlencoded::parse(query.as_bytes()) {
        let slot = match key.as_ref() {
            "state" => &mut state,
            "code" => &mut code,
            "error" => &mut error,
            _ => continue,
        };
        if slot.is_some() || value.is_empty() || value.chars().any(char::is_control) {
            return None;
        }
        *slot = Some(value.into_owned());
    }
    if !bool::from(state?.as_bytes().ct_eq(expected.as_bytes())) {
        return None;
    }
    match (code, error) {
        (Some(code), None) => Some(("code", code)),
        (None, Some(error)) => Some(("error", error)),
        _ => None,
    }
}

/// Only static, localized copy is interpolated. No callback parameter is
/// reflected and the page has no script, links or third-party resources.
fn callback_page(status: StatusCode, success: bool, chinese: bool) -> Response<Full<Bytes>> {
    let (lang, message) = match (chinese, success) {
        (true, true) => ("zh-CN", "已收到授权。请返回码头继续。"),
        (true, false) => ("zh-CN", "登录未完成。请返回码头重试。"),
        (false, true) => ("en", "Authorization received. Return to Matou to continue."),
        (false, false) => (
            "en",
            "Sign-in did not complete. Return to Matou to try again.",
        ),
    };
    Response::builder().status(status)
        .header("Content-Type", "text/html; charset=utf-8")
        .header("Cache-Control", "no-store")
        .header("Content-Security-Policy", "default-src 'none'; frame-ancestors 'none'")
        .header("Referrer-Policy", "no-referrer")
        .header("X-Content-Type-Options", "nosniff")
        .body(Full::new(Bytes::from(format!(
            "<!doctype html><html lang=\"{lang}\"><meta charset=\"utf-8\"><title>Matou</title><p>{message}</p></html>"
        )))).expect("static callback response")
}
