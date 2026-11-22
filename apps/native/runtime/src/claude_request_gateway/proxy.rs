use std::{
    collections::BTreeMap,
    ffi::OsString,
    future::{Ready, ready},
    net::IpAddr,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use hyper::upgrade::{self, Upgraded};
use hyper_rustls::MaybeHttpsStream;
use hyper_util::client::legacy::connect::{Connected, Connection};
use matou_native_core::pty::get_var;
use percent_encoding::percent_decode_str;
use tokio::net::TcpStream;
use url::Url;

use super::{
    AbortOnDrop, CONNECTION_ERROR, Duration, Fault, GatewayBody, HeaderValue, HttpsConnector,
    HttpsConnectorBuilder, Pin, Poll, Request, Service, TaskContext, TokioIo, Uri, client, connect,
    direct_https, header, io,
};

#[derive(Default)]
pub(crate) struct ProxyEnvironment(BTreeMap<&'static str, String>);

impl ProxyEnvironment {
    pub(crate) fn capture(
        inherited: &BTreeMap<OsString, OsString>,
        launch: &BTreeMap<OsString, OsString>,
    ) -> Self {
        const KEYS: [&str; 8] = [
            "https_proxy",
            "HTTPS_PROXY",
            "http_proxy",
            "HTTP_PROXY",
            "all_proxy",
            "ALL_PROXY",
            "no_proxy",
            "NO_PROXY",
        ];
        Self(
            KEYS.into_iter()
                .filter_map(|key| {
                    let value = get_var(launch, key).or_else(|| get_var(inherited, key))?;
                    Some((key, value.to_string_lossy().into_owned()))
                })
                .collect(),
        )
    }

    fn resolve(&self, destination: &Uri) -> Result<Option<Url>, Fault> {
        let host = hostname(destination);
        let default_port = if destination.scheme_str() == Some("https") {
            443
        } else {
            80
        };
        let port = destination.port_u16().unwrap_or(default_port);
        if matches!(
            host.to_ascii_lowercase().as_str(),
            "127.0.0.1" | "localhost" | "::1"
        ) || self
            .0
            .get("no_proxy")
            .or_else(|| self.0.get("NO_PROXY"))
            .is_some_and(|rules| matches_no_proxy(host, port, rules))
        {
            return Ok(None);
        }
        let keys = if destination.scheme_str() == Some("https") {
            ["https_proxy", "HTTPS_PROXY", "all_proxy", "ALL_PROXY"]
        } else {
            ["http_proxy", "HTTP_PROXY", "all_proxy", "ALL_PROXY"]
        };
        let Some(raw) = keys
            .iter()
            .filter_map(|key| self.0.get(key))
            .map(|value| value.trim())
            .find(|value| !value.is_empty())
        else {
            return Ok(None);
        };
        let normalized;
        let raw = if raw.split_once("://").is_some_and(|(scheme, _)| {
            scheme.starts_with(|ch: char| ch.is_ascii_alphabetic())
                && scheme
                    .bytes()
                    .all(|ch| ch.is_ascii_alphanumeric() || b"+.-".contains(&ch))
        }) {
            raw
        } else {
            normalized = format!("http://{raw}");
            &normalized
        };
        let proxy = Url::parse(raw).map_err(|_| proxy_error())?;
        if !matches!(
            proxy.scheme(),
            "http" | "https" | "socks" | "socks4" | "socks4a" | "socks5" | "socks5h"
        ) || proxy.host().is_none()
        {
            return Err(proxy_error());
        }
        Ok(Some(proxy))
    }
}

fn proxy_error() -> Fault {
    Fault::bad_gateway("Claude request gateway proxy configuration is unavailable or unsupported.")
}

fn hostname(uri: &Uri) -> &str {
    uri.host()
        .unwrap_or_default()
        .trim_start_matches('[')
        .trim_end_matches(']')
}

fn matches_no_proxy(host: &str, port: u16, rules: &str) -> bool {
    let host = host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase();
    let port = port.to_string();
    rules.split(',').map(str::trim).any(|entry| {
        if entry == "*" {
            return true;
        }
        let (name, expected_port) = if let Some(entry) = entry.strip_prefix('[') {
            let Some((name, suffix)) = entry.split_once(']') else {
                return false;
            };
            (name, suffix.strip_prefix(':'))
        } else if let Some((name, suffix)) = entry.split_once(':')
            && !suffix.contains(':')
        {
            (name, Some(suffix))
        } else {
            (entry, None)
        };
        if expected_port.is_some_and(|expected| expected != port) {
            return false;
        }
        let name = name.strip_suffix('.').unwrap_or(name).to_ascii_lowercase();
        if let Some(suffix) = name.strip_prefix('*')
            && suffix.starts_with('.')
        {
            return host.ends_with(suffix);
        }
        let domain = name.trim_start_matches('.');
        !domain.is_empty()
            && (host == domain
                || host
                    .strip_suffix(domain)
                    .is_some_and(|prefix| prefix.ends_with('.')))
    })
}

type Exchange = (client::SendRequest<GatewayBody>, AbortOnDrop);
type ProxyStream = MaybeHttpsStream<TokioIo<TcpStream>>;

pub(super) async fn connect_proxy(
    environment: &ProxyEnvironment,
    destination: &Uri,
    request: &mut Request<GatewayBody>,
) -> Result<Option<Exchange>, Fault> {
    let Some(proxy) = environment.resolve(destination)? else {
        return Ok(None);
    };
    tokio::time::timeout(Duration::from_secs(30), async {
        let exchange = if matches!(proxy.scheme(), "http" | "https") {
            let credentials = proxy_credentials(&proxy);
            let authorization = (!proxy.username().is_empty() || proxy.password().is_some())
                .then(|| basic_auth(&credentials));
            let socket = proxy_stream(&proxy).await?;
            if destination.scheme_str() == Some("https") {
                let socket = http_tunnel(socket, destination, authorization).await?;
                connect(tls_tunnel(socket, destination.clone()).await?).await?
            } else {
                *request.uri_mut() = destination.clone();
                if let Some(value) = authorization {
                    request
                        .headers_mut()
                        .insert(header::PROXY_AUTHORIZATION, value);
                }
                connect(socket).await?
            }
        } else {
            let socket = TcpStream::connect(proxy_address(&proxy))
                .await
                .map_err(|error| Fault::transport(&error))?;
            let socket = socks_tunnel(socket, &proxy, destination)
                .await
                .map_err(|error| Fault::transport(&error))?;
            let socket = TokioIo::new(socket);
            if destination.scheme_str() == Some("https") {
                connect(tls_tunnel(socket, destination.clone()).await?).await?
            } else {
                connect(socket).await?
            }
        };
        Ok(Some(exchange))
    })
    .await
    .map_err(|_| Fault::connection(io::ErrorKind::TimedOut))?
}

fn proxy_address(proxy: &Url) -> (&str, u16) {
    let host = proxy.host_str().expect("validated proxy host");
    (
        host.trim_start_matches('[').trim_end_matches(']'),
        proxy.port_or_known_default().unwrap_or(1080),
    )
}

fn proxy_credentials(proxy: &Url) -> (Vec<u8>, Vec<u8>) {
    let decode = |value: &str| percent_decode_str(value).collect();
    (
        decode(proxy.username()),
        decode(proxy.password().unwrap_or_default()),
    )
}

fn basic_auth((user, password): &(Vec<u8>, Vec<u8>)) -> HeaderValue {
    let credentials = [user.as_slice(), b":", password].concat();
    let mut header = HeaderValue::from_str(&format!("Basic {}", STANDARD.encode(credentials)))
        .expect("base64 proxy credentials are valid in a header");
    header.set_sensitive(true);
    header
}

async fn proxy_stream(proxy: &Url) -> Result<ProxyStream, Fault> {
    if proxy.scheme() == "https" {
        let uri = format!(
            "https://{}/",
            &proxy[url::Position::BeforeHost..url::Position::AfterPort]
        )
        .parse::<Uri>()
        .map_err(|_| proxy_error())?;
        direct_https(uri).await
    } else {
        let stream = TcpStream::connect(proxy_address(proxy))
            .await
            .map_err(|error| Fault::transport(&error))?;
        Ok(MaybeHttpsStream::Http(TokioIo::new(stream)))
    }
}

async fn http_tunnel<T>(
    socket: T,
    destination: &Uri,
    authorization: Option<HeaderValue>,
) -> Result<Upgraded, Fault>
where
    T: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    let host = destination.host().ok_or_else(proxy_error)?;
    let port = destination.port_u16().unwrap_or(443);
    let authority = format!("{host}:{port}");
    let (mut sender, connection) = client::handshake(socket)
        .await
        .map_err(|error| Fault::transport(&error))?;
    let driver = tokio::spawn(async move {
        let _ = connection.with_upgrades().await;
    });
    let _driver = AbortOnDrop(driver.abort_handle());
    let mut request = Request::builder()
        .method(hyper::Method::CONNECT)
        .uri(authority.as_str())
        .header(header::HOST, authority.as_str());
    if let Some(value) = authorization {
        request = request.header(header::PROXY_AUTHORIZATION, value);
    }
    let request = request
        .body(super::full_body(hyper::body::Bytes::new()))
        .map_err(|_| proxy_error())?;
    let response = sender
        .send_request(request)
        .await
        .map_err(|error| Fault::transport(&error))?;
    if !response.status().is_success() {
        return Err(Fault::bad_gateway(CONNECTION_ERROR));
    }
    upgrade::on(response)
        .await
        .map_err(|error| Fault::transport(&error))
}

// Supply hyper-rustls with the established tunnel while retaining the target URI
// for certificate verification and SNI.
struct Tunnel<T>(T);

impl<T> Connection for Tunnel<T> {
    fn connected(&self) -> Connected {
        Connected::new()
    }
}

impl<T: hyper::rt::Read + Unpin> hyper::rt::Read for Tunnel<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: hyper::rt::ReadBufCursor<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl<T: hyper::rt::Write + Unpin> hyper::rt::Write for Tunnel<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(cx)
    }
}

struct TunnelConnector<T>(Option<Tunnel<T>>);

impl<T> Service<Uri> for TunnelConnector<T> {
    type Response = Tunnel<T>;
    type Error = io::Error;
    type Future = Ready<io::Result<Self::Response>>;

    fn poll_ready(&mut self, _: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _: Uri) -> Self::Future {
        ready(
            self.0
                .take()
                .ok_or_else(|| io::Error::other("proxy tunnel already used")),
        )
    }
}

async fn tls_tunnel<T>(socket: T, destination: Uri) -> Result<MaybeHttpsStream<Tunnel<T>>, Fault>
where
    T: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
{
    let mut connector: HttpsConnector<_> = HttpsConnectorBuilder::new()
        .with_native_roots()
        .map_err(|_| Fault::bad_gateway(CONNECTION_ERROR))?
        .https_only()
        .enable_http1()
        .wrap_connector(TunnelConnector(Some(Tunnel(socket))));
    connector
        .call(destination)
        .await
        .map_err(|error| Fault::transport(error.as_ref()))
}

async fn socks_tunnel(socket: TcpStream, proxy: &Url, destination: &Uri) -> io::Result<TcpStream> {
    let host = hostname(destination);
    let port = destination
        .port_u16()
        .unwrap_or(if destination.scheme_str() == Some("https") {
            443
        } else {
            80
        });
    let parsed_ip = host.parse::<IpAddr>().ok();
    let (user, password) = proxy_credentials(proxy);
    let version = proxy.scheme();
    let address = if parsed_ip.is_some() || matches!(version, "socks4a" | "socks5h" | "socks") {
        parsed_ip
    } else {
        Some(
            tokio::net::lookup_host((host, port))
                .await?
                .find(|addr| version != "socks4" || addr.is_ipv4())
                .map(|addr| addr.ip())
                .ok_or_else(socks_error)?,
        )
    };
    if matches!(version, "socks4" | "socks4a") {
        socks4(&socket, address, host, port, &user).await?;
    } else {
        let credentials = (!proxy.username().is_empty() || proxy.password().is_some())
            .then_some((user.as_slice(), password.as_slice()));
        socks5(&socket, address, host, port, credentials).await?;
    }
    Ok(socket)
}

async fn socks4(
    socket: &TcpStream,
    address: Option<IpAddr>,
    host: &str,
    port: u16,
    user: &[u8],
) -> io::Result<()> {
    if user.contains(&0) {
        return Err(socks_error());
    }
    let mut request = vec![4, 1];
    request.extend(port.to_be_bytes());
    request.extend(match address {
        Some(IpAddr::V4(ip)) => ip.octets(),
        Some(IpAddr::V6(_)) => return Err(socks_error()),
        None => [0, 0, 0, 1],
    });
    request.extend(user);
    request.push(0);
    if address.is_none() {
        request.extend(host.as_bytes());
        request.push(0);
    }
    write_all(socket, &request).await?;
    let mut response = [0; 8];
    read_exact(socket, &mut response).await?;
    if response[..2] != [0, 90] {
        return Err(socks_error());
    }
    Ok(())
}

async fn socks5(
    socket: &TcpStream,
    address: Option<IpAddr>,
    host: &str,
    port: u16,
    credentials: Option<(&[u8], &[u8])>,
) -> io::Result<()> {
    let greeting: &[u8] = if credentials.is_some() {
        &[5, 2, 0, 2]
    } else {
        &[5, 1, 0]
    };
    write_all(socket, greeting).await?;
    let mut reply = [0; 2];
    read_exact(socket, &mut reply).await?;
    match (reply, credentials) {
        ([5, 0], _) => {}
        ([5, 2], Some((user, password))) => {
            let mut auth = vec![1];
            append_socks_field(&mut auth, user)?;
            append_socks_field(&mut auth, password)?;
            write_all(socket, &auth).await?;
            read_exact(socket, &mut reply).await?;
            if reply != [1, 0] {
                return Err(socks_error());
            }
        }
        _ => return Err(socks_error()),
    }
    let mut request = vec![5, 1, 0];
    match address {
        Some(IpAddr::V4(ip)) => {
            request.push(1);
            request.extend(ip.octets());
        }
        Some(IpAddr::V6(ip)) => {
            request.push(4);
            request.extend(ip.octets());
        }
        None => {
            request.push(3);
            append_socks_field(&mut request, host.as_bytes())?;
        }
    }
    request.extend(port.to_be_bytes());
    write_all(socket, &request).await?;
    let mut response = [0; 4];
    read_exact(socket, &mut response).await?;
    let length = match response {
        [5, 0, 0, 1] => 4,
        [5, 0, 0, 4] => 16,
        [5, 0, 0, 3] => {
            let mut length = [0];
            read_exact(socket, &mut length).await?;
            usize::from(length[0])
        }
        _ => return Err(socks_error()),
    };
    let mut bound_address = [0; 257];
    read_exact(socket, &mut bound_address[..length + 2]).await
}

fn append_socks_field(request: &mut Vec<u8>, value: &[u8]) -> io::Result<()> {
    let length = u8::try_from(value.len()).map_err(|_| socks_error())?;
    if length == 0 {
        return Err(socks_error());
    }
    request.push(length);
    request.extend(value);
    Ok(())
}

fn socks_error() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "SOCKS proxy negotiation failed")
}

async fn read_exact(socket: &TcpStream, mut bytes: &mut [u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        socket.readable().await?;
        match socket.try_read(bytes) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(size) => bytes = &mut bytes[size..],
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

async fn write_all(socket: &TcpStream, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        socket.writable().await?;
        match socket.try_write(bytes) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(size) => bytes = &bytes[size..],
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(env: &[(&'static str, &str)], destination: &str) -> Option<String> {
        let env = ProxyEnvironment(
            env.iter()
                .map(|(key, value)| (*key, (*value).into()))
                .collect(),
        );
        env.resolve(&destination.parse().unwrap())
            .unwrap_or_else(|_| panic!("bad fixture proxy"))
            .map(|proxy| proxy.to_string())
    }

    #[test]
    fn proxy_routing_matches_no_proxy_domains_ports_ipv6_and_loopback() {
        for (key, rule, target, bypasses) in [
            ("NO_PROXY", "*", "https://upstream.invalid", true),
            (
                "no_proxy",
                " , UPSTREAM.INVALID. , ",
                "https://upstream.invalid.",
                true,
            ),
            (
                "no_proxy",
                "upstream.invalid",
                "https://a.upstream.invalid",
                true,
            ),
            (
                "no_proxy",
                ".upstream.invalid",
                "https://upstream.invalid",
                true,
            ),
            (
                "no_proxy",
                ".upstream.invalid",
                "https://a.upstream.invalid",
                true,
            ),
            (
                "no_proxy",
                "*.upstream.invalid",
                "https://a.upstream.invalid",
                true,
            ),
            (
                "no_proxy",
                "*.upstream.invalid",
                "https://upstream.invalid",
                false,
            ),
            (
                "no_proxy",
                "upstream.invalid",
                "https://notupstream.invalid",
                false,
            ),
            (
                "no_proxy",
                "upstream.invalid:443",
                "https://upstream.invalid",
                true,
            ),
            (
                "no_proxy",
                ".upstream.invalid:443",
                "https://upstream.invalid:444",
                false,
            ),
            (
                "no_proxy",
                "upstream.invalid:80",
                "http://upstream.invalid",
                true,
            ),
            (
                "no_proxy",
                "[2001:db8::1]:443",
                "https://[2001:db8::1]",
                true,
            ),
            (
                "no_proxy",
                "[2001:db8::1]:80",
                "https://[2001:db8::1]",
                false,
            ),
            (
                "no_proxy",
                "2001:db8::1",
                "https://[2001:db8::1]:5050",
                true,
            ),
            ("no_proxy", "[2001:db8::1", "https://[2001:db8::1]", false),
            ("no_proxy", "", "http://127.0.0.1:2212", true),
            ("no_proxy", "", "https://LOCALHOST:7373", true),
            ("no_proxy", "", "http://[::1]:7373", true),
        ] {
            assert_eq!(
                resolve(&[(key, rule), ("ALL_PROXY", "proxy.invalid:8118")], target).is_none(),
                bypasses,
                "{key}={rule}, target={target}"
            );
        }
        assert!(
            resolve(
                &[
                    ("no_proxy", ""),
                    ("NO_PROXY", "*"),
                    ("ALL_PROXY", "proxy.invalid:8118")
                ],
                "https://upstream.invalid"
            )
            .is_some()
        );
    }

    #[test]
    fn proxy_scheme_precedence_and_launch_environment_overrides() {
        let env = [
            ("https_proxy", "  socks5h://lower.invalid:1080  "),
            ("HTTPS_PROXY", "https://upper.invalid:8443"),
            ("http_proxy", "   "),
            ("HTTP_PROXY", "web.invalid:8118"),
            ("all_proxy", "socks4a://fallback.invalid:1080"),
            ("ALL_PROXY", "http://last.invalid:8181"),
        ];
        let https = "https://target.invalid";
        let http = "http://target.invalid";
        assert_eq!(
            resolve(&env, https).as_deref(),
            Some("socks5h://lower.invalid:1080")
        );
        assert_eq!(
            resolve(&env[1..], https).as_deref(),
            Some("https://upper.invalid:8443/")
        );
        assert_eq!(
            resolve(&env, http).as_deref(),
            Some("http://web.invalid:8118/")
        );
        assert_eq!(
            resolve(&env[4..], http).as_deref(),
            Some("socks4a://fallback.invalid:1080")
        );
        assert_eq!(
            resolve(&env[5..], https).as_deref(),
            Some("http://last.invalid:8181/")
        );
        assert_eq!(resolve(&env[2..4], https), None);

        let inherited = BTreeMap::from([
            ("HTTP_PROXY".into(), "http://inherited.invalid:8181".into()),
            ("NO_PROXY".into(), "bypass.invalid".into()),
        ]);
        let launch = BTreeMap::from([("HTTP_PROXY".into(), "http://launch.invalid:8181".into())]);
        let merged = ProxyEnvironment::capture(&inherited, &launch);
        let proxy = merged
            .resolve(&http.parse().unwrap())
            .ok()
            .flatten()
            .unwrap();
        assert_eq!(proxy.host_str(), Some("launch.invalid"));
        assert!(matches!(
            merged.resolve(&"http://bypass.invalid".parse().unwrap()),
            Ok(None)
        ));
    }
}
