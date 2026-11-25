use std::{path::Path, str::FromStr};

use matou_native_core::{
    provider_launch::ClaudeRoleAssignment, sessions::ClaudeRoleLaunchPreferences,
};

use super::*;

const EXTERNAL: &str = "upstream.fixture.invalid";
const OFFICIAL: &str = "api.anthropic.com";
const OFFICIAL_ID: &str = "anthropic-official";
const ACCOUNT_NAME: &str = "Official fixture";
const OAUTH_MODEL: &str = "claude-fixture-resolved";
const ORIGINAL_HEADERS: &str = "x-api-key: incoming-api-secret\r\nAuthorization: Bearer incoming-token\r\nanthropic-beta: caller-beta,oauth-2025-04-20\r\n";

fn gateway_with(
    plan: ClaudeGatewayProjection,
    proxies: &[(&str, &str)],
    locale: InterfaceLocale,
    blocked: Arc<AtomicU8>,
    fixture: Option<FixtureUpstream>,
) -> Result<Gateway> {
    let launch = proxies
        .iter()
        .map(|(key, value)| ((*key).into(), (*value).into()))
        .collect();
    Gateway::start_gateway(
        ProjectionSlot::new(Arc::new(plan), None, None),
        gemini_sse_keepalive::INTERVAL,
        None,
        blocked,
        locale,
        ProxyEnvironment::capture(&BTreeMap::new(), &launch),
        fixture,
    )
}

fn proxy_plan(endpoint: &str) -> Result<ClaudeGatewayProjection> {
    let unused = SocketAddr::from((Ipv4Addr::LOCALHOST, 1));
    let mut plan = projection(unused, unused)?;
    plan.source.as_mut().unwrap().endpoint = url::Url::parse(endpoint)?;
    Ok(plan)
}

#[test]
fn forward_http_proxy_uses_absolute_form_and_only_its_own_basic_auth() -> Result<()> {
    let proxy = listener()?;
    let endpoint = format!("http://{EXTERNAL}:8181/root?tenant=one");
    let auth_proxy = format!("http://proxy%2Duser:p%40ss@{}", proxy.local_addr()?);
    let gateway = gateway_with(
        proxy_plan(&endpoint)?,
        &[("HTTP_PROXY", &auth_proxy)],
        InterfaceLocale::English,
        Arc::default(),
        None,
    )?;
    let (requests, workers) = record(proxy, 1, "200 OK", "", b"proxied");
    let (status, _, body) = exchange_method(
        &gateway,
        "GET",
        "/v1/models?limit=2",
        b"",
        "Proxy-Authorization: Basic incoming-do-not-forward\r\n",
    )?;
    assert_eq!((status, body.as_slice()), (200, b"proxied".as_slice()));
    let (line, headers, _) = requests.recv_timeout(TIMEOUT)?;
    assert_eq!(
        line.trim_end(),
        format!("GET http://{EXTERNAL}:8181/root/v1/models?tenant=one&limit=2 HTTP/1.1")
    );
    assert_eq!(headers["host"], format!("{EXTERNAL}:8181"));
    assert_eq!(headers["proxy-authorization"], "Basic cHJveHktdXNlcjpwQHNz");
    assert_eq!(headers["x-api-key"], "synthetic-a");
    for worker in workers {
        worker.join().unwrap()?;
    }
    Ok(())
}

#[test]
fn bypassed_proxy_requests_go_direct_and_invalid_proxy_errors_are_redacted() -> Result<()> {
    let upstream = listener()?;
    let address = upstream.local_addr()?;
    let proxy = listener()?;
    let proxy_url = format!("http://{}", proxy.local_addr()?);
    let (requests, workers) = record(upstream, 2, "200 OK", "", b"direct");
    for (endpoint, no_proxy, fixture) in [
        (
            format!("http://{EXTERNAL}:8181"),
            format!("*.fixture.invalid:8181"),
            Some(FixtureUpstream {
                host: EXTERNAL,
                address,
                catalog: None,
            }),
        ),
        (format!("http://{address}"), String::new(), None),
    ] {
        let gateway = gateway_with(
            proxy_plan(&endpoint)?,
            &[("HTTP_PROXY", &proxy_url), ("NO_PROXY", &no_proxy)],
            InterfaceLocale::English,
            Arc::default(),
            fixture,
        )?;
        assert_eq!(
            exchange_method(&gateway, "GET", "/v1/models", b"", "")?.0,
            200
        );
        let (line, headers, _) = requests.recv_timeout(TIMEOUT)?;
        assert_eq!(line.trim_end(), "GET /v1/models HTTP/1.1");
        assert!(!headers.contains_key(header::PROXY_AUTHORIZATION));
    }
    for worker in workers {
        worker.join().unwrap()?;
    }
    assert_no_request(&proxy)?;

    let gateway = gateway_with(
        proxy_plan(&format!("http://{EXTERNAL}"))?,
        &[(
            "ALL_PROXY",
            "ftp://private-user:private-password@proxy.fixture.invalid:9",
        )],
        InterfaceLocale::English,
        Arc::default(),
        None,
    )?;
    let (status, _, body) = exchange_method(&gateway, "GET", "/v1/models", b"", "")?;
    assert_eq!(status, 502);
    let body = String::from_utf8(body)?;
    assert!(body.contains("proxy configuration is unavailable or unsupported"));
    for private in ["private-user", "private-password", "proxy.fixture.invalid"] {
        assert!(!body.contains(private));
    }
    Ok(())
}

fn proxy_socket(
    listener: StdListener,
    run: impl FnOnce(TcpStream) -> Result<()> + Send + 'static,
) -> JoinHandle<Result<()>> {
    thread::spawn(move || {
        listener.set_nonblocking(true)?;
        let deadline = std::time::Instant::now() + TIMEOUT;
        let stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    ensure!(
                        std::time::Instant::now() < deadline,
                        "waiting for proxy connection"
                    );
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => return Err(error.into()),
            }
        };
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(TIMEOUT))?;
        stream.set_write_timeout(Some(TIMEOUT))?;
        run(stream)
    })
}

fn expect_bytes(stream: &mut TcpStream, bytes: &[u8]) -> Result<()> {
    let mut received = vec![0; bytes.len()];
    stream.read_exact(&mut received)?;
    assert_eq!(received, bytes);
    Ok(())
}

fn verify_tunneled_http(socket: TcpStream, authority: &str) -> Result<()> {
    let mut io = BufReader::new(socket);
    let (line, headers) = head(&mut io)?;
    assert_eq!(line.trim_end(), "GET /v1/models HTTP/1.1");
    assert_eq!(headers["host"], authority);
    assert_eq!(headers["x-api-key"], "synthetic-a");
    assert!(!headers.contains_key(header::PROXY_AUTHORIZATION));
    reply(io.get_mut(), "200 OK", "", b"tunneled")
}

#[test]
fn socks5_proxies_forward_ipv4_ipv6_and_remote_dns_with_optional_authentication() -> Result<()> {
    for (scheme, host, userinfo, ip_address) in [
        ("socks5", "192.0.2.42", "", Some(vec![1, 192, 0, 2, 42])),
        (
            "socks5",
            "[2001:db8::42]",
            "",
            Some(
                [
                    &[4][..],
                    &std::net::Ipv6Addr::from_str("2001:db8::42")?.octets(),
                ]
                .concat(),
            ),
        ),
        ("socks5h", EXTERNAL, "proxy%2Duser:p%40ss@", None),
        ("socks", EXTERNAL, "", None),
    ] {
        let proxy = listener()?;
        let proxy_url = format!("{scheme}://{userinfo}{}", proxy.local_addr()?);
        let endpoint = format!("http://{host}:8181");
        let authority = format!("{host}:8181");
        let gateway = gateway_with(
            proxy_plan(&endpoint)?,
            &[("ALL_PROXY", &proxy_url)],
            InterfaceLocale::English,
            Arc::default(),
            None,
        )?;
        let worker = proxy_socket(proxy, move |mut stream| {
            if userinfo.is_empty() {
                expect_bytes(&mut stream, &[5, 1, 0])?;
                stream.write_all(&[5, 0])?;
            } else {
                expect_bytes(&mut stream, &[5, 2, 0, 2])?;
                stream.write_all(&[5, 2])?;
                expect_bytes(&mut stream, b"\x01\x0aproxy-user\x04p@ss")?;
                stream.write_all(&[1, 0])?;
            }
            let address = ip_address
                .unwrap_or_else(|| [&[3, host.len() as u8][..], host.as_bytes()].concat());
            let request = [&[5, 1, 0][..], &address, &8181u16.to_be_bytes()].concat();
            expect_bytes(&mut stream, &request)?;
            stream.write_all(b"\x05\x00\x00\x03\x01x\x00\x00")?;
            verify_tunneled_http(stream, &authority)
        });
        let response = exchange_method(&gateway, "GET", "/v1/models", b"", "")?;
        worker.join().unwrap()?;
        assert_eq!(
            (response.0, response.2.as_slice()),
            (200, b"tunneled".as_slice())
        );
    }
    Ok(())
}

#[test]
fn socks4_and_socks4a_proxies_forward_requests() -> Result<()> {
    for (scheme, host, address) in [
        ("socks4", "192.0.2.42", [192, 0, 2, 42]),
        ("socks4a", EXTERNAL, [0, 0, 0, 1]),
    ] {
        let proxy = listener()?;
        let proxy_url = format!("{scheme}://fixture-user@{}", proxy.local_addr()?);
        let endpoint = format!("http://{host}:8181");
        let authority = format!("{host}:8181");
        let gateway = gateway_with(
            proxy_plan(&endpoint)?,
            &[("ALL_PROXY", &proxy_url)],
            InterfaceLocale::English,
            Arc::default(),
            None,
        )?;
        let worker = proxy_socket(proxy, move |mut stream| {
            let request = [
                &[4, 1][..],
                &8181u16.to_be_bytes(),
                &address,
                b"fixture-user\0",
            ]
            .concat();
            expect_bytes(&mut stream, &request)?;
            if scheme == "socks4a" {
                expect_bytes(&mut stream, &[host.as_bytes(), b"\0"].concat())?;
            }
            stream.write_all(&[0, 90, 0, 0, 0, 0, 0, 0])?;
            verify_tunneled_http(stream, &authority)
        });
        let response = exchange_method(&gateway, "GET", "/v1/models", b"", "")?;
        worker.join().unwrap()?;
        assert_eq!(response.0, 200);
    }
    Ok(())
}

fn official_catalog() -> Vec<Value> {
    vec![json!({"id":"opus", "resolvedModel":OAUTH_MODEL, "contextWindow":200_000})]
}

fn write_identity(root: &Path, id: &str) -> Result<()> {
    std::fs::write(root.join(".claude.json"), json!({
        "oauthAccount": {"accountUuid": id, "emailAddress": "fallback@fixture.invalid", "organizationUuid": "fixture-org"}
    }).to_string())?;
    Ok(())
}

fn write_token(root: &Path, token: &str, expires_at: u64) -> Result<()> {
    std::fs::write(
        root.join(".credentials.json"),
        json!({
            "claudeAiOauth": {"accessToken": token, "expiresAt": expires_at}
        })
        .to_string(),
    )?;
    Ok(())
}

fn official_plan(
    root: &Path,
    preferences: &ClaudeRoleLaunchPreferences,
    official_source: bool,
) -> Result<ClaudeGatewayProjection> {
    let work = json!({
        "id": "work", "endpoint": "http://127.0.0.1:1",
        "model":"work-opus", "apiKey":"work-api-secret",
        "modelMapping":{"opus":"work-opus","sonnet":"work-sonnet","haiku":"work-haiku"},
    });
    let official = json!({
        "id":OFFICIAL_ID,"endpoint":format!("https://{OFFICIAL}"),"model":"opus","builtIn":true,"name":ACCOUNT_NAME
    });
    let source = if official_source { OFFICIAL_ID } else { "work" };
    let config = json!({
        "version":1,
        "activeProviderIds":{"claude-code":source},
        "providers":{"claude-code":[work, official]}
    });
    std::fs::write(root.join("provider-config.json"), config.to_string())?;
    let env = BTreeMap::from([
        (
            "MATOU_CLAUDE_COMMAND".into(),
            std::env::current_exe()?.into_os_string(),
        ),
        ("CLAUDE_CONFIG_DIR".into(), root.as_os_str().into()),
        ("MATOU_CLAUDE_DISABLE_KEYCHAIN".into(), "1".into()),
        ("USER".into(), "isolated-fixture-user".into()),
    ]);
    prepare_new_claude_launch_with_budget(
        root,
        root,
        &Value::Null,
        (preferences, &official_catalog),
        env,
        80,
        24,
    )?
    .gateway
    .context("missing OAuth gateway projection")
}

fn official_assignment() -> ClaudeRoleAssignment {
    ClaudeRoleAssignment {
        provider_id: OFFICIAL_ID.into(),
        model: "opus".into(),
        effort: None,
    }
}

fn official_compaction() -> Value {
    json!({"mode":"custom", "providerId":OFFICIAL_ID, "model":"opus", "effort":"follow"})
}

fn official_gateway(
    plan: ClaudeGatewayProjection,
    address: SocketAddr,
    locale: InterfaceLocale,
    blocked: Arc<AtomicU8>,
) -> Result<Gateway> {
    gateway_with(
        plan,
        &[],
        locale,
        blocked,
        Some(FixtureUpstream {
            host: OFFICIAL,
            address,
            catalog: Some(official_catalog),
        }),
    )
}

#[test]
fn official_oauth_routes_roles_search_and_both_compaction_modes_with_current_tokens() -> Result<()>
{
    let temp = within_workspace()?;
    let root = temp.path();
    write_identity(root, "account-1")?;
    write_token(root, "token-first", 0)?;
    let upstream = listener()?;
    let addr = upstream.local_addr()?;
    let (requests, workers) = record(upstream, 5, "200 OK", "", b"ok");
    let mut preferences = ClaudeRoleLaunchPreferences::default();
    preferences
        .assignments
        .insert("opus".into(), official_assignment());
    preferences.web_search = Some(official_assignment());
    preferences.compaction = official_compaction();
    let snapshot = official_gateway(
        official_plan(root, &preferences, false)?,
        addr,
        InterfaceLocale::English,
        Arc::default(),
    )?;

    let mut preferences = ClaudeRoleLaunchPreferences::default();
    preferences.compaction = official_compaction();
    let lazy = official_gateway(
        official_plan(root, &preferences, false)?,
        addr,
        InterfaceLocale::English,
        Arc::default(),
    )?;
    let cases = [
        (
            &snapshot,
            json!({"model":"matou-role/opus"}),
            "",
            "token-first",
        ),
        (
            &snapshot,
            search_payload(json!("work-opus")),
            "",
            "token-first",
        ),
        (
            &snapshot,
            json!({"model":"work-opus"}),
            "x-claude-code-compaction: manual\r\n",
            "token-first",
        ),
        (
            &lazy,
            json!({"model":"work-opus"}),
            "x-claude-code-compaction: auto\r\n",
            "token-first",
        ),
        (
            &lazy,
            json!({"model":"work-opus"}),
            "x-claude-code-compaction: auto\r\n",
            "token-second",
        ),
    ];
    for (gateway, body, marker, token) in cases {
        write_token(root, token, 0)?;
        let headers = format!("{ORIGINAL_HEADERS}{marker}");
        assert_eq!(
            exchange(gateway, MESSAGES, &serde_json::to_vec(&body)?, &headers)?.0,
            200
        );
        let (line, headers, body) = requests.recv_timeout(TIMEOUT)?;
        assert_eq!(line.trim_end(), "POST /v1/messages HTTP/1.1");
        assert_eq!(headers["host"], OFFICIAL);
        assert_eq!(headers["authorization"], format!("Bearer {token}"));
        assert_eq!(headers["anthropic-beta"], "caller-beta,oauth-2025-04-20");
        assert!(!headers.contains_key("x-api-key"));
        assert_eq!(
            serde_json::from_slice::<Value>(&body)?["model"],
            OAUTH_MODEL
        );
    }
    for worker in workers {
        worker.join().unwrap()?;
    }
    Ok(())
}

#[test]
fn official_oauth_identity_changes_and_expired_file_tokens_fail_before_sending() -> Result<()> {
    let temp = within_workspace()?;
    let root = temp.path();
    write_identity(root, "account-1")?;
    write_token(root, "token-first", 0)?;
    let upstream = listener()?;
    let mut preferences = ClaudeRoleLaunchPreferences::default();
    preferences.web_search = Some(official_assignment());
    let gateway = official_gateway(
        official_plan(root, &preferences, false)?,
        upstream.local_addr()?,
        InterfaceLocale::English,
        Arc::default(),
    )?;
    let send = || {
        exchange(
            &gateway,
            MESSAGES,
            &serde_json::to_vec(&search_payload(json!("work-opus")))?,
            ORIGINAL_HEADERS,
        )
    };
    write_identity(root, "account-2")?;
    assert_gateway_502(send()?, "Official Claude account identity changed")?;
    assert_no_request(&upstream)?;
    write_identity(root, "account-1")?;
    write_token(root, "expired-secret", 1)?;
    assert_gateway_502(
        send()?,
        "Sign in to the official Claude account before using this target",
    )?;
    assert_no_request(&upstream)?;
    Ok(())
}

#[test]
fn official_oauth_rewrites_upstream_authentication_errors_and_preserves_other_429s() -> Result<()> {
    const EXPIRED_GZIP: &[u8] = &[
        31, 139, 8, 0, 0, 0, 0, 0, 2, 19, 171, 86, 74, 45, 42, 202, 47, 82, 178, 170, 86, 74, 206,
        79, 73, 85, 178, 82, 42, 201, 207, 78, 205, 139, 79, 173, 40, 200, 44, 74, 77, 81, 170,
        173, 5, 0, 25, 178, 161, 107, 34, 0, 0, 0,
    ];
    let temp = within_workspace()?;
    let root = temp.path();
    write_identity(root, "account-1")?;
    write_token(root, "token-first", 0)?;
    let upstream = listener()?;
    let mut preferences = ClaudeRoleLaunchPreferences::default();
    preferences.web_search = Some(official_assignment());
    let plan = official_plan(root, &preferences, false)?;
    for (locale, status, extra, body, expected) in [
        (
            InterfaceLocale::Chinese,
            "401 Unauthorized",
            "X-Upstream: private\r\n",
            b"private raw diagnostics".as_slice(),
            Some(
                "网页搜索 · Official fixture 登录已过期，请到 设置 → Claude Code 重新登录后重试。",
            ),
        ),
        (
            InterfaceLocale::English,
            "403 Forbidden",
            "",
            b"expired".as_slice(),
            Some(
                "Web search · Official fixture sign-in has expired. Go to Settings → Claude Code, sign in again, then retry.",
            ),
        ),
        (
            InterfaceLocale::English,
            "429 Too Many Requests",
            "Content-Encoding: gzip\r\n",
            EXPIRED_GZIP,
            Some(
                "Web search · Official fixture sign-in has expired. Go to Settings → Claude Code, sign in again, then retry.",
            ),
        ),
        (
            InterfaceLocale::English,
            "429 Too Many Requests",
            "Retry-After: 11\r\n",
            br#"{"error":{"type":"rate_limit_error"}}"#.as_slice(),
            None,
        ),
    ] {
        let gateway =
            official_gateway(plan.clone(), upstream.local_addr()?, locale, Arc::default())?;
        let (requests, workers) = record(upstream.try_clone()?, 1, status, extra, body);
        let response = exchange(
            &gateway,
            MESSAGES,
            &serde_json::to_vec(&search_payload(json!("work-opus")))?,
            ORIGINAL_HEADERS,
        )?;
        requests.recv_timeout(TIMEOUT)?;
        for worker in workers {
            worker.join().unwrap()?;
        }
        assert_eq!(response.0, status[..3].parse::<u16>()?);
        if let Some(expected) = expected {
            assert!(!response.1.contains_key("x-upstream"));
            assert!(!response.1.contains_key(header::CONTENT_ENCODING));
            assert_eq!(
                serde_json::from_slice::<Value>(&response.2)?,
                json!({"type":"error", "error":{"type":"authentication_error", "message":expected}})
            );
        } else {
            assert_eq!(response.1["retry-after"], "11");
            assert_eq!(response.2, body);
        }
    }
    Ok(())
}

#[test]
fn request_egress_freeze_blocks_both_official_keyed_and_oauth_targets() -> Result<()> {
    let root = within_workspace()?;
    write_identity(root.path(), "account-1")?;
    write_token(root.path(), "token-first", 0)?;
    let upstream = listener()?;
    let addr = upstream.local_addr()?;
    let mut preferences = ClaudeRoleLaunchPreferences::default();
    preferences.web_search = Some(official_assignment());
    let blocked = Arc::new(AtomicU8::new(1 << Family::Anthropic as u8));
    let oauth = official_gateway(
        official_plan(root.path(), &preferences, false)?,
        addr,
        InterfaceLocale::English,
        Arc::clone(&blocked),
    )?;
    let api = official_gateway(
        proxy_plan(&format!("https://{OFFICIAL}"))?,
        addr,
        InterfaceLocale::English,
        blocked,
    )?;
    for (gateway, path, body) in [
        (
            &oauth,
            MESSAGES,
            serde_json::to_vec(&search_payload(json!("work-opus")))?,
        ),
        (&api, "/v1/models", Vec::new()),
    ] {
        let (status, _, body) = exchange(gateway, path, &body, "")?;
        assert_eq!(status, 502);
        assert!(String::from_utf8(body)?.contains("Network egress protection is active"));
        assert_no_request(&upstream)?;
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn authenticated_http_connect_https_proxy_and_socks_tls_forward_end_to_end() -> Result<()> {
    use std::process::{Command, Stdio};

    let root = within_workspace()?;
    let openssl = |args: &[&str]| -> Result<()> {
        let output = Command::new("/usr/bin/openssl")
            .current_dir(root.path())
            .args(args)
            .output()?;
        ensure!(
            output.status.success(),
            "openssl fixture setup: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(())
    };
    openssl(&[
        "req",
        "-x509",
        "-newkey",
        "rsa:2048",
        "-nodes",
        "-days",
        "2",
        "-subj",
        "/CN=Matou gateway test root",
        "-addext",
        "basicConstraints=critical,CA:TRUE",
        "-keyout",
        "ca.key",
        "-out",
        "ca.pem",
    ])?;
    openssl(&[
        "req",
        "-new",
        "-newkey",
        "rsa:2048",
        "-nodes",
        "-subj",
        "/CN=upstream.fixture.invalid",
        "-keyout",
        "leaf.key",
        "-out",
        "leaf.csr",
    ])?;
    std::fs::write(
        root.path().join("extensions"),
        "subjectAltName=DNS:upstream.fixture.invalid,IP:127.0.0.1\nextendedKeyUsage=serverAuth\n",
    )?;
    openssl(&[
        "x509",
        "-req",
        "-in",
        "leaf.csr",
        "-CA",
        "ca.pem",
        "-CAkey",
        "ca.key",
        "-set_serial",
        "1",
        "-days",
        "1",
        "-extfile",
        "extensions",
        "-out",
        "leaf.pem",
    ])?;

    let python = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/gateway-proxy-tls.py"
    );
    let mut proxy = Command::new("python3")
        .current_dir(root.path())
        .args([python, "leaf.pem", "leaf.key"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut addresses = String::new();
    BufReader::new(proxy.stdout.take().context("proxy fixture stdout")?)
        .read_line(&mut addresses)?;
    let output = Command::new(std::env::current_exe()?)
        .args([
            "--exact",
            "claude_request_gateway::tests::proxy_and_oauth::tls_proxy_fixture_child",
            "--nocapture",
        ])
        .env("SSL_CERT_FILE", root.path().join("ca.pem"))
        .env_remove("SSL_CERT_DIR")
        .env("MATOU_TEST_GATEWAY_TLS_PROXY_PORTS", addresses.trim())
        .output();
    let proxy_output = proxy.wait_with_output()?;
    let output = output?;
    ensure!(
        proxy_output.status.success() && output.status.success(),
        "gateway TLS fixtures failed\n{}{}{}",
        String::from_utf8_lossy(&proxy_output.stderr),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn tls_proxy_fixture_child() -> Result<()> {
    // Only the child gets the fixture CA. Other parallel tests never see it.
    let Ok(ports) = std::env::var("MATOU_TEST_GATEWAY_TLS_PROXY_PORTS") else {
        return Ok(());
    };
    let ports = ports
        .split_whitespace()
        .map(str::parse)
        .collect::<std::result::Result<Vec<u16>, _>>()?;
    let [http, https, socks] = ports[..] else {
        anyhow::bail!("expected three fixture proxy ports")
    };
    for (proxy, endpoint) in [
        (
            format!("http://user:p%40ss@127.0.0.1:{http}"),
            format!("https://{EXTERNAL}:8443"),
        ),
        (
            format!("https://user:p%40ss@127.0.0.1:{https}"),
            format!("https://{EXTERNAL}:8443"),
        ),
        (
            format!("https://user:p%40ss@127.0.0.1:{https}"),
            format!("http://{EXTERNAL}:8181"),
        ),
        (
            format!("socks5h://127.0.0.1:{socks}"),
            format!("https://{EXTERNAL}:8443"),
        ),
    ] {
        let gateway = gateway_with(
            proxy_plan(&endpoint)?,
            &[("ALL_PROXY", &proxy)],
            InterfaceLocale::English,
            Arc::default(),
            None,
        )?;
        let (status, _, body) = exchange_method(&gateway, "GET", "/v1/models", b"", "")?;
        assert_eq!((status, body.as_slice()), (200, b"ok".as_slice()));
    }
    Ok(())
}

#[test]
fn unused_official_source_launches_without_reading_account_identity() -> Result<()> {
    let root = within_workspace()?;
    let mut preferences = ClaudeRoleLaunchPreferences::default();
    for role in CLAUDE_ROLES {
        preferences.assignments.insert(
            role.into(),
            ClaudeRoleAssignment {
                provider_id: "work".into(),
                model: "work-opus".into(),
                effort: None,
            },
        );
    }
    let plan = official_plan(root.path(), &preferences, true)?;
    assert!(plan.source.is_none());
    assert!(plan.roles.iter().all(Option::is_some));
    assert!(!root.path().join(".claude.json").exists());
    Ok(())
}

#[test]
fn logged_in_builtin_official_role_projects_oauth_before_provider_file_exists() -> Result<()> {
    let root = within_workspace()?;
    write_identity(root.path(), "account-1")?;
    write_token(root.path(), "builtin-token", 0)?;
    let mut preferences = ClaudeRoleLaunchPreferences::default();
    for role in CLAUDE_ROLES {
        preferences
            .assignments
            .insert(role.into(), official_assignment());
    }
    let env = BTreeMap::from([
        (
            "MATOU_CLAUDE_COMMAND".into(),
            std::env::current_exe()?.into_os_string(),
        ),
        ("CLAUDE_CONFIG_DIR".into(), root.path().as_os_str().into()),
        ("MATOU_CLAUDE_DISABLE_KEYCHAIN".into(), "1".into()),
    ]);
    let launch = prepare_new_claude_launch_with_budget(
        root.path(),
        root.path(),
        &Value::Null,
        (&preferences, &official_catalog),
        env,
        80,
        24,
    )?;
    let plan = launch
        .gateway
        .context("missing built-in official gateway")?;
    let account = plan
        .source
        .context("missing built-in official source")?
        .official_oauth
        .context("missing built-in OAuth account")?;
    assert_eq!(account.read_token()?, "builtin-token");
    assert!(!root.path().join("provider-config.json").exists());
    Ok(())
}
