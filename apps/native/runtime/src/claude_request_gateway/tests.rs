use std::{
    collections::BTreeMap,
    io::{self, BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener as StdListener, TcpStream},
    thread::JoinHandle,
};

use anyhow::ensure;
use matou_native_core::provider_launch::{
    PreparedSubscriptionTarget, parse_claude_role_assignments, prepare_claude_launch,
    prepare_new_claude_launch, prepare_new_claude_launch_with_budget,
};
use matou_native_core::sessions::read_claude_role_launch_preferences;

use super::*;

pub(crate) const TIMEOUT: Duration = Duration::from_secs(8);
const MESSAGES: &str = "/v1/messages";
const COUNT: &str = "/v1/messages/count_tokens";
const GZIP: &[u8] = &[
    31, 139, 8, 0, 0, 0, 0, 0, 2, 3, 203, 72, 205, 201, 201, 7, 0, 134, 166, 16, 54, 5, 0, 0, 0,
];

fn within_workspace() -> Result<tempfile::TempDir> {
    Ok(tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?)
}

pub(crate) fn projection(source: SocketAddr, other: SocketAddr) -> Result<ClaudeGatewayProjection> {
    let temp = within_workspace()?;
    let config = json!({
        "version": 1,
        "activeProviderIds": {"claude-code": "a"},
        "providers": {"claude-code": [
            {"id": "a", "endpoint": format!("http://{source}/a///?tenant=a"),
             "model": "source-opus", "apiKey": "synthetic-a",
             "modelMapping": {"opus": "source-opus", "sonnet": "source-sonnet", "haiku": "source-haiku"}},
            {"id": "b", "endpoint": format!("http://{other}/b?tenant=b"),
             "model": "shared", "apiKey": "synthetic-b",
             "modelMetadata": [{"id": "shared", "supportedEfforts": ["high"], "maxOutputTokens": 2048}]}
        ]}
    });
    std::fs::write(temp.path().join("provider-config.json"), config.to_string())?;
    let assignments = parse_claude_role_assignments(&json!({
        "opus": {"providerId": "a", "model": "shared", "effort": "follow"},
        "sonnet": {"providerId": "b", "model": "shared", "effort": "high"}
    }))?;
    prepare_new_claude_launch(
        temp.path(),
        temp.path(),
        &Value::Null,
        Some(&assignments),
        BTreeMap::from([(
            "MATOU_CLAUDE_COMMAND".into(),
            std::env::current_exe()?.into_os_string(),
        )]),
        80,
        24,
    )?
    .gateway
    .context("missing API gateway projection")
}

#[test]
fn saved_roles_switch_the_next_request_without_interrupting_the_current_stream() -> Result<()> {
    use crate::host::save_role_assignments;

    const FIRST: &[u8] = b"event: message_start\ndata: first\n\n";
    const TAIL: &[u8] = b"event: message_stop\ndata: done\n\n";
    let root = within_workspace()?;
    let old_upstream = listener()?;
    let new_upstream = listener()?;
    let config = json!({
        "version": 1,
        "activeProviderIds": {"claude-code": "old"},
        "providers": {"claude-code": [
            {"id": "old", "endpoint": "https://api.deepseek.com/anthropic",
             "model": "deepseek-old", "apiKey": "old-key", "modelMapping": {"opus": "deepseek-old"}},
            {"id": "new", "endpoint": format!("http://{}/new", new_upstream.local_addr()?),
             "model": "gemini-new", "apiKey": "new-key",
             "modelMetadata": [{"id": "gemini-new", "supportedEfforts": ["high"]}]}
        ]}
    });
    std::fs::write(root.path().join("provider-config.json"), config.to_string())?;
    rusqlite::Connection::open(root.path().join("matou.sqlite"))?.execute_batch(
        "CREATE TABLE preferences (key TEXT PRIMARY KEY, value_json TEXT, updated_at INTEGER)",
    )?;
    let old = json!({"opus": {"providerId": "old", "model": "deepseek-old", "effort": "low"}});
    save_role_assignments(root.path(), 1, &old, None, None)?;
    let plan = prepare_new_claude_launch(
        root.path(),
        root.path(),
        &Value::Null,
        Some(&parse_claude_role_assignments(&old)?),
        BTreeMap::from([(
            "MATOU_CLAUDE_COMMAND".into(),
            std::env::current_exe()?.into_os_string(),
        )]),
        80,
        24,
    )?;
    let slot = ProjectionSlot::new(
        Arc::new(plan.gateway.context("missing launch gateway")?),
        None,
        Some((
            plan.gateway_refresh.context("missing refresh context")?,
            root.path().to_owned(),
        )),
    );
    let gateway = Gateway::start_gateway(
        slot,
        gemini_sse_keepalive::INTERVAL,
        None,
        Arc::default(),
        InterfaceLocale::default(),
        ProxyEnvironment::default(),
        Some(FixtureUpstream {
            host: deepseek_tools::HOST,
            address: old_upstream.local_addr()?,
            catalog: None,
        }),
    )?;
    let (release, wait) = mpsc::channel();
    let old_worker = upstream(old_upstream, move |mut io, start, headers| {
        assert!(start.starts_with("POST /anthropic/v1/messages "));
        assert_eq!(headers["x-api-key"], "old-key");
        let mut body = vec![0; length(&headers)?];
        io.read_exact(&mut body)?;
        let payload: Value = serde_json::from_slice(&body)?;
        assert_eq!(payload["model"], "deepseek-old");
        assert_eq!(payload["output_config"]["effort"], "low");
        let out = io.get_mut();
        out.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n")?;
        write_chunk(out, FIRST)?;
        wait.recv_timeout(TIMEOUT)?;
        write_chunk(out, TAIL)?;
        write_chunk(out, b"")?;
        Ok(())
    });
    let mut stream = BufReader::new(open_request(
        &gateway,
        MESSAGES,
        br#"{"model":"matou-role/opus","stream":true}"#,
        "",
    )?);
    assert!(head(&mut stream)?.0.starts_with("HTTP/1.1 200"));
    let mut received = Vec::new();
    while received.len() < FIRST.len() {
        read_chunk(&mut stream, &mut received)?;
    }
    assert_eq!(received, FIRST);

    let new = json!({"opus": {"providerId": "new", "model": "gemini-new", "effort": "high"}});
    let (requests, workers) = record(new_upstream, 3, "200 OK", "", b"ok");
    let assert_new_request = || -> Result<()> {
        let (status, _, body) = exchange(&gateway, MESSAGES, br#"{"model":"matou-role/opus"}"#, "")?;
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
        let (start, headers, body) = requests.recv_timeout(TIMEOUT)?;
        assert!(start.starts_with("POST /new/v1/messages "));
        assert_eq!(headers["x-api-key"], "new-key");
        let payload: Value = serde_json::from_slice(&body)?;
        assert_eq!(payload["model"], "gemini-new");
        assert_eq!(payload["output_config"]["effort"], "high");
        Ok(())
    };
    save_role_assignments(root.path(), 2, &new, None, None)?;
    assert_new_request()?;
    release.send(())?;
    while read_chunk(&mut stream, &mut received)? != 0 {}
    assert_eq!(received, [FIRST, TAIL].concat());
    old_worker.join().unwrap()?;

    let mut invalid = new;
    invalid["opus"]["effort"] = json!("low");
    assert!(save_role_assignments(root.path(), 3, &invalid, None, None).is_err());
    assert_new_request()?;
    save_role_assignments(root.path(), 4, &json!({}), None, None)?;
    assert_new_request()?;
    for worker in workers {
        worker.join().unwrap()?;
    }
    assert!(!gateway.stopped.load(Ordering::Acquire));
    Ok(())
}

#[test]
fn fable_aliases_use_their_own_target_or_the_legacy_opus_target() -> Result<()> {
    let temp = within_workspace()?;
    let upstream = listener()?;
    let address = upstream.local_addr()?;
    let ids = [
        "matou-role/fable",
        "matou-api/fable",
        "fable",
        "claude-fable-5-2",
        "claude-fable-5-5[1m]",
    ];
    let config = json!({
        "version": 1,
        "activeProviderIds": {"claude-code": "a"},
        "providers": {"claude-code": [
            {"id": "a", "endpoint": format!("http://{address}/opus"),
             "model": "source-opus", "apiKey": "synthetic-opus",
             "modelMapping": {"opus": "source-opus"}, "effortMapping": {"opus": "low"}},
            {"id": "b", "endpoint": format!("http://{address}/fable"),
             "model": "fable-target", "apiKey": "synthetic-fable",
             "modelMetadata": [{"id": "fable-target", "resolvedModel": "fable-upstream",
                                "supportedEfforts": ["high"]}]}
        ]}
    });
    let path = temp.path().join("provider-config.json");
    std::fs::write(&path, config.to_string())?;
    for (assignments, lane, model, effort) in [
        (
            json!({"fable": {"providerId": "b", "model": "fable-target", "effort": "high"}}),
            "fable",
            "fable-upstream",
            "high",
        ),
        (
            json!({"opus": {"providerId": "a", "model": "source-opus"}}),
            "opus",
            "source-opus",
            "low",
        ),
        (json!({}), "opus", "source-opus", "low"),
    ] {
        let assignments = parse_claude_role_assignments(&assignments)?;
        let plan = prepare_new_claude_launch(
            temp.path(),
            temp.path(),
            &Value::Null,
            Some(&assignments),
            BTreeMap::from([(
                "MATOU_CLAUDE_COMMAND".into(),
                std::env::current_exe()?.into_os_string(),
            )]),
            80,
            24,
        )?;
        let gateway = Gateway::start(Arc::new(plan.gateway.context("missing Fable gateway")?))?;
        let (requests, workers) = record(upstream.try_clone()?, ids.len(), "200 OK", "", b"ok");
        for id in ids {
            let request = json!({"model": id});
            assert_eq!(
                exchange(&gateway, MESSAGES, &serde_json::to_vec(&request)?, "")?.0,
                200
            );
            let (start, headers, body) = requests.recv_timeout(TIMEOUT)?;
            assert!(start.starts_with(&format!("POST /{lane}/v1/messages ")));
            assert_eq!(headers["x-api-key"], format!("synthetic-{lane}"));
            let payload: Value = serde_json::from_slice(&body)?;
            assert_eq!(payload["model"], model);
            assert_eq!(payload["output_config"]["effort"], effort);
        }
        for worker in workers {
            worker.join().unwrap()?;
        }
    }
    Ok(())
}

fn search_projection(
    source: SocketAddr,
    search: SocketAddr,
    effort: Option<&str>,
) -> Result<ClaudeGatewayProjection> {
    let temp = within_workspace()?;
    let config = json!({
        "version": 1,
        "activeProviderIds": {"claude-code": "work"},
        "providers": {"claude-code": [
            {"id": "work", "endpoint": format!("http://{source}/work?tenant=work"),
             "model": "work-opus", "apiKey": "synthetic-work",
             "modelMapping": {"opus": "work-opus", "sonnet": "work-sonnet", "haiku": "work-haiku"}},
            {"id": "search", "name": "搜索供应商", "endpoint": format!("http://{search}/search///?tenant=search"),
             "model": "unused-default", "apiKey": "synthetic-search",
             "reasoningEffort": "high", "effortMapping": {"haiku": "low"},
             "modelMetadata": [{"id": "search-alias", "resolvedModel": "claude-search-exact",
                                "supportedEfforts": ["high"], "maxOutputTokens": 512}]}
        ]}
    });
    std::fs::write(temp.path().join("provider-config.json"), config.to_string())?;
    let db = rusqlite::Connection::open_in_memory()?;
    let mut target = json!({"providerId": "search", "model": "search-alias"});
    if let Some(effort) = effort {
        target["effort"] = json!(effort);
    }
    db.execute_batch("CREATE TABLE preferences (key TEXT PRIMARY KEY, value_json TEXT)")?;
    db.execute(
        "INSERT INTO preferences VALUES ('agent.modelPreferences', ?)",
        [
            json!({"contextWindows": {}, "compaction": {"mode": "follow", "effort": "follow"},
                "claudeWebSearch": target})
            .to_string(),
        ],
    )?;
    prepare_new_claude_launch_with_budget(
        temp.path(),
        temp.path(),
        &Value::Null,
        (&read_claude_role_launch_preferences(&db), &Vec::new),
        BTreeMap::from([(
            "MATOU_CLAUDE_COMMAND".into(),
            std::env::current_exe()?.into_os_string(),
        )]),
        80,
        24,
    )?
    .gateway
    .context("missing search gateway projection")
}

pub(crate) fn listener() -> io::Result<StdListener> {
    StdListener::bind((Ipv4Addr::LOCALHOST, 0))
}

fn socket(addr: SocketAddr) -> io::Result<TcpStream> {
    let stream = TcpStream::connect_timeout(&addr, TIMEOUT)?;
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    Ok(stream)
}

fn gateway_address(gateway: &Gateway) -> Result<(SocketAddr, String)> {
    let url = url::Url::parse(gateway.base_url())?;
    Ok((
        (Ipv4Addr::LOCALHOST, url.port().context("gateway port")?).into(),
        url.path().into(),
    ))
}

pub(crate) fn head(stream: &mut impl BufRead) -> Result<(String, HeaderMap)> {
    let mut first = String::new();
    ensure!(stream.read_line(&mut first)? > 0, "missing HTTP start line");
    let mut headers = HeaderMap::new();
    loop {
        let mut line = String::new();
        ensure!(stream.read_line(&mut line)? > 0, "incomplete HTTP headers");
        if line == "\r\n" {
            break;
        }
        let (name, value) = line.split_once(':').context("HTTP header")?;
        headers.append(
            header::HeaderName::from_bytes(name.as_bytes())?,
            HeaderValue::from_str(value.trim())?,
        );
    }
    Ok((first, headers))
}

pub(crate) fn length(headers: &HeaderMap) -> Result<usize> {
    Ok(headers
        .get(header::CONTENT_LENGTH)
        .context("no content-length")?
        .to_str()?
        .parse()?)
}

pub(crate) fn open_request(
    gateway: &Gateway,
    path: &str,
    body: &[u8],
    headers: &str,
) -> Result<TcpStream> {
    open_request_method(gateway, "POST", path, body, headers)
}

fn open_request_method(
    gateway: &Gateway,
    method: &str,
    path: &str,
    body: &[u8],
    headers: &str,
) -> Result<TcpStream> {
    let (addr, prefix) = gateway_address(gateway)?;
    let mut stream = socket(addr)?;
    write!(
        stream,
        "{method} {prefix}{path} HTTP/1.1\r\nHost: local-gateway\r\nContent-Length: {}\r\n{headers}\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    Ok(stream)
}

pub(crate) fn exchange(
    gateway: &Gateway,
    path: &str,
    body: &[u8],
    headers: &str,
) -> Result<(u16, HeaderMap, Vec<u8>)> {
    exchange_method(gateway, "POST", path, body, headers)
}

fn exchange_method(
    gateway: &Gateway,
    method: &str,
    path: &str,
    body: &[u8],
    headers: &str,
) -> Result<(u16, HeaderMap, Vec<u8>)> {
    let mut stream = BufReader::new(open_request_method(gateway, method, path, body, headers)?);
    let (start, headers) = head(&mut stream)?;
    let status = start
        .split_whitespace()
        .nth(1)
        .context("HTTP status")?
        .parse()?;
    let mut bytes = vec![0; length(&headers)?];
    stream.read_exact(&mut bytes)?;
    Ok((status, headers, bytes))
}

pub(crate) fn upstream(
    listener: StdListener,
    handler: impl FnOnce(BufReader<TcpStream>, String, HeaderMap) -> Result<()> + Send + 'static,
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
                        "upstream accept timed out"
                    );
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => return Err(error.into()),
            }
        };
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(TIMEOUT))?;
        stream.set_write_timeout(Some(TIMEOUT))?;
        let mut stream = BufReader::new(stream);
        let (start, headers) = head(&mut stream)?;
        handler(stream, start, headers)
    })
}

fn reply(io: &mut TcpStream, status: &str, headers: &str, bytes: &[u8]) -> Result<()> {
    write!(
        io,
        "HTTP/1.1 {status}\r\nConnection: close, x-private-response\r\nx-private-response: remove\r\nContent-Length: {}\r\n{headers}\r\n",
        bytes.len()
    )?;
    io.write_all(bytes)?;
    Ok(())
}

pub(crate) fn record(
    listener: StdListener,
    count: usize,
    status: &'static str,
    headers: &'static str,
    bytes: &'static [u8],
) -> (
    mpsc::Receiver<(String, HeaderMap, Vec<u8>)>,
    Vec<JoinHandle<Result<()>>>,
) {
    let (tx, rx) = mpsc::channel();
    let tasks = (0..count)
        .map(|_| {
            let tx = tx.clone();
            upstream(
                listener.try_clone().expect("clone listener"),
                move |mut io, start, request| {
                    let mut body = vec![0; length(&request)?];
                    io.read_exact(&mut body)?;
                    tx.send((start, request, body))
                        .map_err(|_| anyhow!("recording closed"))?;
                    reply(io.get_mut(), status, headers, bytes)
                },
            )
        })
        .collect();
    (rx, tasks)
}

#[test]
fn hud_usage_is_scoped_to_the_authenticated_session_without_changing_forwarding() -> Result<()> {
    const WITH_USAGE: &[u8] = br#"{"content":"fixture","usage":{"input_tokens":10000,"output_tokens":2000,"cache_read_input_tokens":40000,"cache_creation_input_tokens":20000,"cache_expires_at":"2026-11-25T05:00:00Z"}}"#;
    const WITHOUT_USAGE: &[u8] = br#"{"content":"fixture"}"#;
    let upstream = listener()?;
    let address = upstream.local_addr()?;
    let plan = Arc::new(projection(address, address)?);
    let first = Gateway::start(Arc::clone(&plan))?;
    let second = Gateway::start(plan)?;
    let first_id = Uuid::new_v4();
    let second_id = Uuid::new_v4();
    first.bind_usage_identity(&first_id.to_string());
    second.bind_usage_identity(&second_id.to_string());

    let request = |id: Uuid| {
        serde_json::to_vec(&json!({
            "model": "literal-model",
            "messages": [{"role":"user","content":"synthetic prompt"}],
            "metadata": {"user_id": json!({"session_id": id.to_string()}).to_string()}
        }))
        .unwrap()
    };
    let first_body = request(first_id);
    let second_body = request(second_id);
    let (recorded, tasks) = record(
        upstream.try_clone()?,
        3,
        "200 OK",
        "Content-Type: application/json\r\n",
        WITH_USAGE,
    );
    assert_eq!(exchange(&first, MESSAGES, &first_body, "")?.2, WITH_USAGE);
    assert_eq!(recorded.recv_timeout(TIMEOUT)?.2, first_body);
    let sample = first
        .context_usage()
        .context("missing matched request usage")?;
    assert_eq!(sample.tokens.used_tokens(), Some(72_000.));
    assert_eq!(
        sample.cache_expires_at_ms,
        crate::agent_hud_projection::timestamp_ms(&json!("2026-11-25T05:00:00Z"))
    );
    assert!(sample.observed_at_ms.is_some());
    assert!(sample.estimated_cache_ttl_sec.is_none());
    assert!(second.context_usage().is_none());

    assert_eq!(exchange(&second, MESSAGES, &first_body, "")?.2, WITH_USAGE);
    assert_eq!(recorded.recv_timeout(TIMEOUT)?.2, first_body);
    assert!(second.context_usage().is_none());
    assert_eq!(exchange(&second, MESSAGES, &second_body, "")?.2, WITH_USAGE);
    assert_eq!(recorded.recv_timeout(TIMEOUT)?.2, second_body);
    assert_eq!(second.context_usage().unwrap().tokens, sample.tokens);
    for task in tasks {
        task.join().unwrap()?;
    }

    let (recorded, tasks) = record(
        upstream,
        1,
        "200 OK",
        "Content-Type: application/json\r\n",
        WITHOUT_USAGE,
    );
    assert_eq!(
        exchange(&first, MESSAGES, &first_body, "")?.2,
        WITHOUT_USAGE
    );
    assert_eq!(recorded.recv_timeout(TIMEOUT)?.2, first_body);
    assert!(first.context_usage().is_none());
    assert!(second.context_usage().is_some());
    for task in tasks {
        task.join().unwrap()?;
    }
    Ok(())
}

fn search_payload(model: Value) -> Value {
    json!({
        "model": model,
        "tools": [
            {"type": "web_search_20250305", "name": "web_search"},
            {"type": "web_search_20260209", "name": "web_search"}
        ],
        "system": [{"type": "text", "text": "system sentinel"}],
        "messages": [
            {"role": "user", "content": "whole conversation 哨兵"},
            {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "old-call",
                                          "content": "historical tool result sentinel"}]}
        ],
        "max_tokens": 9999,
        "thinking": {"type": "enabled", "budget_tokens": 1024},
        "output_config": {"effort": "low", "format": {"type": "json_schema"}},
        "metadata": {"user_id": "fixture"},
        "stream": true
    })
}

#[test]
fn web_search_gateway_routes_each_request_by_tools_method_path_and_marker() -> Result<()> {
    let work = listener()?;
    let search = listener()?;
    let gateway = Gateway::start(Arc::new(search_projection(
        work.local_addr()?,
        search.local_addr()?,
        None,
    )?))?;
    let pure = search_payload(json!("matou-role/haiku"));
    let literal = search_payload(json!("work-literal"));
    let mut cases = vec![
        (
            "messages",
            "POST",
            MESSAGES,
            "",
            pure.clone(),
            Some("search"),
        ),
        ("count", "POST", COUNT, "", pure.clone(), Some("search")),
        (
            "different marker",
            "POST",
            MESSAGES,
            "MANUAL",
            pure.clone(),
            Some("search"),
        ),
        (
            "literal search",
            "POST",
            COUNT,
            "",
            literal.clone(),
            Some("search"),
        ),
        (
            "manual messages",
            "POST",
            MESSAGES,
            "manual",
            pure.clone(),
            Some("role"),
        ),
        (
            "auto messages",
            "POST",
            MESSAGES,
            "auto",
            pure.clone(),
            Some("role"),
        ),
        (
            "manual count",
            "POST",
            COUNT,
            "manual",
            pure.clone(),
            Some("role"),
        ),
        (
            "auto count",
            "POST",
            COUNT,
            "auto",
            pure.clone(),
            Some("role"),
        ),
        ("other method", "PUT", MESSAGES, "", literal.clone(), None),
        (
            "other path",
            "POST",
            "/v1/messages/",
            "",
            literal.clone(),
            None,
        ),
        (
            "invalid model",
            "POST",
            MESSAGES,
            "",
            search_payload(json!(3)),
            None,
        ),
    ];
    for (label, tools) in [
        ("no tools", None),
        ("empty tools", Some(json!([]))),
        ("null tools", Some(Value::Null)),
        ("only client", Some(json!([{"name": "client"}]))),
        ("only fetch", Some(json!([{"type": "web_fetch_20250910"}]))),
        (
            "mixed client",
            Some(json!([{"type": "web_search_20250305"}, {"name": "client"}])),
        ),
        (
            "mixed fetch",
            Some(json!([{"type": "web_search_20250305"}, {"type": "web_fetch_20250910"}])),
        ),
        (
            "mixed null",
            Some(json!([{"type": "web_search_20250305"}, null])),
        ),
        (
            "mixed scalar",
            Some(json!([{"type": "web_search_20250305"}, 4])),
        ),
        (
            "mixed invalid type",
            Some(json!([{"type": "web_search_20250305"}, {"type": 4}])),
        ),
    ] {
        let mut body = literal.clone();
        if let Some(tools) = tools {
            body["tools"] = tools;
        } else {
            body.as_object_mut().unwrap().remove("tools");
        }
        cases.push((label, "POST", MESSAGES, "", body, None));
    }
    let search_hits = 4;
    let (work_requests, work_tasks) = record(
        work.try_clone()?,
        cases.len() - search_hits,
        "200 OK",
        "",
        b"work",
    );
    let (search_requests, search_tasks) =
        record(search.try_clone()?, search_hits, "200 OK", "", b"search");
    let mut total_search = 0;
    for (label, method, path, marker, payload, target) in cases {
        let mut headers = "x-api-key: old\r\nauthorization: Bearer old\r\ncookie: old\r\ncontent-md5: old\r\ncontent-digest: old\r\ndigest: old\r\nanthropic-beta: existing, shared\r\nanthropic-beta: shared\r\n".to_string();
        if !marker.is_empty() {
            write!(headers, "x-claude-code-compaction: {marker}\r\n")?;
        }
        let original = serde_json::to_vec_pretty(&payload)?;
        let (status, _, response) = exchange_method(
            &gateway,
            method,
            &format!("{path}?beta=1"),
            &original,
            &headers,
        )?;
        let is_search = target == Some("search");
        assert_eq!(status, 200, "{label}");
        let (requests, other, provider) = if is_search {
            total_search += 1;
            (&search_requests, &work_requests, "search")
        } else {
            (&work_requests, &search_requests, "work")
        };
        assert_eq!(response, provider.as_bytes(), "{label}");
        let (start, headers, bytes) = requests.recv_timeout(TIMEOUT)?;
        assert!(
            other.try_recv().is_err(),
            "{label} leaked to the other upstream"
        );
        assert_eq!(
            start,
            format!("{method} /{provider}{path}?tenant={provider}&beta=1 HTTP/1.1\r\n"),
            "{label}"
        );
        assert_eq!(
            headers["x-api-key"],
            format!("synthetic-{provider}"),
            "{label}"
        );
        assert_eq!(headers.get_all("x-api-key").iter().count(), 1, "{label}");
        assert!(
            !headers.contains_key("authorization") && !headers.contains_key("cookie"),
            "{label}"
        );
        assert_eq!(headers["anthropic-beta"], "existing,shared", "{label}");
        assert_eq!(length(&headers)?, bytes.len(), "{label}");
        let mut expected = payload;
        match target {
            Some("search") => {
                expected["model"] = json!("claude-search-exact");
                if path == MESSAGES {
                    expected["max_tokens"] = json!(512);
                    expected["thinking"] = json!({"type": "adaptive"});
                    expected["output_config"]["effort"] = json!("high");
                }
            }
            Some("role") => {
                expected["model"] = json!("work-haiku");
                if path == MESSAGES {
                    expected.as_object_mut().unwrap().remove("thinking");
                    expected["output_config"]
                        .as_object_mut()
                        .unwrap()
                        .remove("effort");
                }
            }
            _ => assert_eq!(bytes, original, "{label} changed original bytes"),
        }
        for name in ["content-md5", "content-digest", "digest"] {
            assert_eq!(
                headers.contains_key(name),
                target.is_none(),
                "{label} {name}"
            );
        }
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes)?,
            expected,
            "{label}"
        );
    }
    assert_eq!(total_search, search_hits);
    for task in work_tasks.into_iter().chain(search_tasks) {
        task.join().expect("fake upstream thread")?;
    }
    for upstream in [work, search] {
        upstream.set_nonblocking(true)?;
        assert_eq!(
            upstream.accept().unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
    }
    Ok(())
}

#[test]
fn web_search_gateway_preserves_http_failures_and_never_falls_back() -> Result<()> {
    let work = listener()?;
    work.set_nonblocking(true)?;
    let original = serde_json::to_vec(&search_payload(json!("matou-role/haiku")))?;
    for (status, line) in [
        (401, "401 Unauthorized"),
        (429, "429 Too Many Requests"),
        (503, "503 Service Unavailable"),
    ] {
        let search = listener()?;
        let gateway = Gateway::start(Arc::new(search_projection(
            work.local_addr()?,
            search.local_addr()?,
            Some("follow"),
        )?))?;
        let (requests, tasks) = record(search, 1, line, "Retry-After: 4\r\n", b"upstream response");
        let (actual, headers, response) = exchange(&gateway, MESSAGES, &original, "")?;
        assert_eq!(actual, status);
        assert_eq!(headers["retry-after"], "4");
        assert_eq!(response, b"upstream response");
        let (_, headers, body) = requests.recv_timeout(TIMEOUT)?;
        assert_eq!(headers["x-api-key"], "synthetic-search");
        let mut expected = search_payload(json!("claude-search-exact"));
        expected["max_tokens"] = json!(512);
        expected.as_object_mut().unwrap().remove("thinking");
        expected["output_config"]
            .as_object_mut()
            .unwrap()
            .remove("effort");
        assert_eq!(serde_json::from_slice::<Value>(&body)?, expected);
        for task in tasks {
            task.join().expect("fake upstream thread")?;
        }
        assert_eq!(work.accept().unwrap_err().kind(), io::ErrorKind::WouldBlock);
    }
    let search = listener()?;
    let addr = search.local_addr()?;
    let gateway = Gateway::start(Arc::new(search_projection(
        work.local_addr()?,
        addr,
        Some("low"),
    )?))?;
    let (status, _, body) = exchange(&gateway, MESSAGES, &original, "")?;
    assert_eq!(status, 502);
    assert_eq!(
        serde_json::from_slice::<Value>(&body)?["error"]["message"],
        "网页搜索 · 搜索供应商: model target or reasoning effort is unavailable"
    );
    search.set_nonblocking(true)?;
    assert_eq!(
        search.accept().unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    assert_eq!(work.accept().unwrap_err().kind(), io::ErrorKind::WouldBlock);
    let gateway = Gateway::start(Arc::new(search_projection(work.local_addr()?, addr, None)?))?;
    drop(search);
    let (status, _, body) = exchange(&gateway, MESSAGES, &original, "")?;
    assert_eq!(status, 502);
    assert_eq!(
        serde_json::from_slice::<Value>(&body)?["error"]["message"],
        CONNECTION_ERROR
    );
    assert_eq!(work.accept().unwrap_err().kind(), io::ErrorKind::WouldBlock);
    Ok(())
}

#[test]
fn gateway_routes_messages_and_count_to_distinct_api_keys() -> Result<()> {
    let a = listener()?;
    let b = listener()?;
    let gateway = Gateway::start(Arc::new(projection(a.local_addr()?, b.local_addr()?)?))?;
    let (a, a_tasks) = record(
        a,
        4,
        "200 OK",
        "Content-Type: application/json\r\n",
        br#"{"upstream":"a"}"#,
    );
    let (b, b_tasks) = record(
        b,
        3,
        "429 Too Many Requests",
        "Retry-After: 5\r\nX-RateLimit-Remaining: 0\r\nContent-Encoding: gzip\r\n",
        GZIP,
    );

    let key = "authorization: Bearer caller\r\nX-Api-Key: caller\r\nCookie: caller=1\r\nx-client_secret: caller\r\nX-Custom-Credentials: caller\r\nConnection: x-remove\r\nx-remove: caller\r\nanthropic-beta: first, second\r\nanthropic-beta: second, third\r\ncontent-md5: old\r\ncontent-digest: old\r\ndigest: old\r\nanthropic-version: 2023-06-01\r\n";
    let payload = |model: &str| {
        json!({
            "model": model, "messages": [{"role":"user","content":"你好"}],
            "max_tokens": 5000, "thinking": {"type": "enabled"},
            "output_config": {"effort": "low", "format": "preserve"}
        })
    };
    let check_request = |stream: &mpsc::Receiver<(String, HeaderMap, Vec<u8>)>,
                         path: &str,
                         expected_key: &str|
     -> Result<_> {
        let (start, headers, body) = stream.recv_timeout(TIMEOUT)?;
        assert_eq!(start, format!("POST {path} HTTP/1.1\r\n"));
        assert!(
            headers["x-api-key"] == expected_key,
            "wrong upstream credential"
        );
        for name in [
            "authorization",
            "cookie",
            "x-client_secret",
            "x-custom-credentials",
            "x-remove",
        ] {
            assert!(!headers.contains_key(name), "{name}");
        }
        Ok((headers, body))
    };

    let body = payload("matou-role/opus");
    let (status, headers, bytes) = exchange(&gateway, MESSAGES, body.to_string().as_bytes(), key)?;
    assert_eq!(status, 200);
    assert_eq!(bytes, br#"{"upstream":"a"}"#);
    assert!(!headers.contains_key("x-private-response"));
    let (headers, bytes) = check_request(&a, "/a/v1/messages?tenant=a", "synthetic-a")?;
    assert_eq!(headers["anthropic-beta"], "first,second,third");
    assert_eq!(headers["anthropic-version"], "2023-06-01");
    assert_eq!(length(&headers)?, bytes.len());
    for name in ["content-md5", "content-digest", "digest"] {
        assert!(!headers.contains_key(name));
    }
    let actual: Value = serde_json::from_slice(&bytes)?;
    assert_eq!(actual["model"], "shared");
    assert_eq!(actual["messages"], body["messages"]);
    assert_eq!(actual["output_config"], json!({"format":"preserve"}));
    assert!(actual.get("thinking").is_none());

    for model in ["matou-role/sonnet[1M]", "source-sonnet"] {
        let (status, headers, bytes) = exchange(
            &gateway,
            "/v1/messages?beta=1",
            payload(model).to_string().as_bytes(),
            key,
        )?;
        assert_eq!(status, 429);
        assert_eq!(headers["retry-after"], "5");
        assert_eq!(headers["x-ratelimit-remaining"], "0");
        assert_eq!(headers["content-encoding"], "gzip");
        assert_eq!(bytes, GZIP);
        let (_, bytes) = check_request(&b, "/b/v1/messages?tenant=b&beta=1", "synthetic-b")?;
        let actual: Value = serde_json::from_slice(&bytes)?;
        assert_eq!(actual["model"], "shared");
        assert_eq!(actual["max_tokens"], 2048);
        assert_eq!(
            actual["output_config"],
            json!({"effort":"high","format":"preserve"})
        );
        assert!(actual.get("thinking").is_none());
    }

    let count = payload("matou-role/sonnet");
    assert_eq!(
        exchange(&gateway, COUNT, count.to_string().as_bytes(), key)?.0,
        429
    );
    let (_, bytes) = check_request(&b, "/b/v1/messages/count_tokens?tenant=b", "synthetic-b")?;
    let mut expected = count;
    expected["model"] = json!("shared");
    assert_eq!(serde_json::from_slice::<Value>(&bytes)?, expected);

    for body in [
        br#"{ "model": "arbitrary-literal" }"#.as_slice(),
        br#"["invalid"]"#,
        br#"{"messages":[]}"#,
    ] {
        assert_eq!(exchange(&gateway, MESSAGES, body, key)?.0, 200);
        assert_eq!(
            check_request(&a, "/a/v1/messages?tenant=a", "synthetic-a")?.1,
            body
        );
    }
    assert_eq!(
        exchange(&gateway, MESSAGES, br#"{"model":"matou-role/unknown"}"#, "")?.0,
        502
    );
    for path in [
        "/../v1/messages",
        "//v1/messages",
        "/%2e%2E/v1/messages",
        "/%2Fv1/messages",
        "/%255cv1/messages",
    ] {
        assert_eq!(exchange(&gateway, path, b"{}", "")?.0, 404);
    }
    let (addr, _) = gateway_address(&gateway)?;
    let mut bare = socket(addr)?;
    bare.write_all(b"GET /wrong/v1/messages HTTP/1.1\r\nHost: localhost\r\n\r\n")?;
    assert!(head(&mut BufReader::new(bare))?.0.contains("404"));
    for task in a_tasks.into_iter().chain(b_tasks) {
        task.join().expect("fake upstream panicked")?;
    }
    assert!(a.try_recv().is_err() && b.try_recv().is_err());
    Ok(())
}

#[test]
fn gateway_merges_target_beta_and_never_follows_or_replays() -> Result<()> {
    let mut headers = HeaderMap::new();
    headers.append("anthropic-beta", HeaderValue::from_static("client, shared"));
    headers.append("anthropic-beta", HeaderValue::from_static("shared"));
    request_headers(
        &mut headers,
        [(
            header::HeaderName::from_static("anthropic-beta"),
            HeaderValue::from_static("target, shared"),
        )],
    );
    assert_eq!(headers["anthropic-beta"], "client,shared,target");

    let a = listener()?;
    let b = listener()?;
    let unused = b.try_clone()?;
    unused.set_nonblocking(true)?;
    let addr = b.local_addr()?;
    let gateway = Gateway::start(Arc::new(projection(a.local_addr()?, addr)?))?;
    let retry_probe = a.try_clone()?;
    let redirect = upstream(a.try_clone()?, move |mut io, _, headers| {
        io.read_exact(&mut vec![0; length(&headers)?])?;
        reply(
            io.get_mut(),
            "307 Temporary Redirect",
            &format!("Location: http://{addr}/never\r\n"),
            b"",
        )
    });
    let (status, headers, _) = exchange(&gateway, MESSAGES, br#"{"model":"opus"}"#, "")?;
    assert_eq!(status, 502);
    assert!(!headers.contains_key(header::LOCATION));
    redirect.join().unwrap()?;
    assert_eq!(
        unused.accept().unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );

    let disconnected = upstream(a, |mut io, _, headers| {
        io.read_exact(&mut vec![0; length(&headers)?])?;
        // Close after the request was received but before writing any response.
        Ok(())
    });
    assert_eq!(
        exchange(&gateway, MESSAGES, br#"{"model":"opus"}"#, "")?.0,
        502
    );
    disconnected.join().unwrap()?;
    retry_probe.set_nonblocking(true)?;
    assert_eq!(
        retry_probe.accept().unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    Ok(())
}

pub(crate) fn write_chunk(stream: &mut TcpStream, bytes: &[u8]) -> io::Result<()> {
    write!(stream, "{:x}\r\n", bytes.len())?;
    stream.write_all(bytes)?;
    stream.write_all(b"\r\n")
}

pub(crate) fn read_chunk(stream: &mut BufReader<TcpStream>, output: &mut Vec<u8>) -> Result<usize> {
    let mut length = String::new();
    stream.read_line(&mut length)?;
    let length = usize::from_str_radix(length.trim(), 16)?;
    let start = output.len();
    output.resize(start + length, 0);
    stream.read_exact(&mut output[start..])?;
    let mut line = [0; 2];
    stream.read_exact(&mut line)?;
    assert_eq!(&line, b"\r\n");
    Ok(length)
}

#[test]
fn gateway_streams_first_sse_bytes_before_tail_and_backpressures_slow_client() -> Result<()> {
    for deepseek in [false, true] {
        check_sse_backpressure(deepseek)?;
    }
    Ok(())
}

const STREAM_REQUEST: &[u8] = br#"{"model":"sonnet","stream":true,"tools":[{"type":"custom","name":"fixture","input_schema":{}}]}"#;
const DEEPSEEK_STREAM_BODY: &[u8] = br#"{"model":"shared","stream":true,"tools":[{"name":"fixture","input_schema":{}}],"output_config":{"effort":"high"}}"#;

fn stream_gateway(a: SocketAddr, b: SocketAddr, deepseek: bool) -> Result<Gateway> {
    let mut plan = projection(a, b)?;
    if deepseek {
        let index = plan.request_role("sonnet").context("test role")?;
        plan.roles[index].as_mut().unwrap().connection.endpoint =
            url::Url::parse("https://api.deepseek.com/anthropic")?;
    }
    Gateway::start_inner(Arc::new(plan), deepseek.then_some(b))
}

fn check_sse_backpressure(deepseek: bool) -> Result<()> {
    const FIRST: &[u8] = b"event: message_start\ndata: first\n\n";
    const TAIL: &[u8] = b"event: message_stop\ndata: done\n\n";
    let a = listener()?;
    let b = listener()?;
    let gateway = stream_gateway(a.local_addr()?, b.local_addr()?, deepseek)?;
    let (release, wait) = mpsc::channel();
    let (finished, writing) = mpsc::channel();
    let padding = vec![b'x'; 64 * 1024];
    let upstream = upstream(b, move |mut io, _, headers| {
        let mut body = vec![0; length(&headers)?];
        io.read_exact(&mut body)?;
        if deepseek {
            assert_eq!(body, DEEPSEEK_STREAM_BODY);
        }
        let out = io.get_mut();
        out.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n")?;
        write_chunk(out, FIRST)?;
        wait.recv_timeout(TIMEOUT)?;
        for _ in 0..256 {
            write_chunk(out, &padding)?;
        }
        write_chunk(out, TAIL)?;
        write_chunk(out, b"")?;
        finished.send(())?;
        Ok(())
    });
    let mut client = BufReader::new(open_request(&gateway, MESSAGES, STREAM_REQUEST, "")?);
    let (start, headers) = head(&mut client)?;
    assert!(start.contains("200"));
    assert_eq!(headers["content-type"], "text/event-stream");
    assert_eq!(headers["transfer-encoding"], "chunked");
    let mut received = Vec::new();
    while received.len() < FIRST.len() {
        read_chunk(&mut client, &mut received)?;
    }
    assert_eq!(received, FIRST);
    release.send(())?;
    assert_eq!(
        writing.recv_timeout(Duration::from_millis(150)),
        Err(mpsc::RecvTimeoutError::Timeout)
    );
    // A second route is independent even with an unread response on this run.
    let (observed, tasks) = record(a, 1, "200 OK", "", b"ok");
    assert_eq!(
        exchange(&gateway, COUNT, br#"{"model":"opus"}"#, "")?.2,
        b"ok"
    );
    assert!(observed.recv_timeout(TIMEOUT)?.1["x-api-key"] == "synthetic-a");
    for task in tasks {
        task.join().unwrap()?;
    }
    while read_chunk(&mut client, &mut received)? != 0 {}
    assert_eq!(received.len(), FIRST.len() + 16 * 1024 * 1024 + TAIL.len());
    assert!(received.ends_with(TAIL));
    upstream.join().unwrap()?;
    eprintln!("PASS SSE first chunk and slow downstream (DeepSeek conversion: {deepseek})");
    Ok(())
}

#[test]
fn gateway_cancels_upstream_before_headers_after_first_sse_and_on_drop() -> Result<()> {
    for deepseek in [false, true] {
        check_stream_cancellation(deepseek)?;
    }
    Ok(())
}

fn check_stream_cancellation(deepseek: bool) -> Result<()> {
    for (send_headers, retire) in [(false, false), (true, false), (true, true)] {
        let a = listener()?;
        let b = listener()?;
        let gateway = stream_gateway(a.local_addr()?, b.local_addr()?, deepseek)?;
        let (addr, _) = gateway_address(&gateway)?;
        let (received, wait) = mpsc::channel();
        let upstream = upstream(b, move |mut io, _, headers| {
            let mut body = vec![0; length(&headers)?];
            io.read_exact(&mut body)?;
            if deepseek {
                assert_eq!(body, DEEPSEEK_STREAM_BODY);
            }
            if send_headers {
                io.get_mut().write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n")?;
                write_chunk(io.get_mut(), b"data: first\n\n")?;
            }
            received.send(())?;
            match io.read(&mut [0]) {
                Ok(0) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::ConnectionReset => Ok(()),
                outcome => anyhow::bail!("upstream remained open after cancel: {outcome:?}"),
            }
        });
        let mut client = BufReader::new(open_request(&gateway, MESSAGES, STREAM_REQUEST, "")?);
        wait.recv_timeout(TIMEOUT)?;
        if send_headers {
            assert!(head(&mut client)?.0.contains("200"));
            let mut received = Vec::new();
            while received.len() < b"data: first\n\n".len() {
                read_chunk(&mut client, &mut received)?;
            }
            assert_eq!(received, b"data: first\n\n");
        }
        if retire {
            drop(gateway);
            assert_eq!(
                client.read(&mut [0])?,
                0,
                "old downstream stream remained open"
            );
            upstream.join().unwrap()?;
            assert_eq!(
                socket(addr).unwrap_err().kind(),
                io::ErrorKind::ConnectionRefused
            );
        } else {
            drop(client);
            upstream.join().unwrap()?;
        }
    }
    eprintln!(
        "PASS cancellation before headers, after first SSE, and on gateway retirement (DeepSeek conversion: {deepseek})"
    );
    Ok(())
}

#[test]
fn gateway_bounds_declared_and_chunked_requests_at_64_mib() -> Result<()> {
    let a = listener()?;
    let b = listener()?;
    let gateway = Gateway::start(Arc::new(projection(a.local_addr()?, b.local_addr()?)?))?;
    let (addr, prefix) = gateway_address(&gateway)?;
    let fresh = || -> io::Result<TcpStream> { socket(addr) };
    let mut declared = fresh()?;
    write!(
        declared,
        "POST {prefix}{MESSAGES} HTTP/1.1\r\nHost: gateway\r\nContent-Length: {}\r\n\r\n",
        MAX_REQUEST_BYTES + 1
    )?;
    assert!(head(&mut BufReader::new(declared))?.0.contains("413"));

    let upstream_probe = a.try_clone()?;
    let upstream = upstream(a, |mut io, _, headers| {
        assert_eq!(length(&headers)?, MAX_REQUEST_BYTES);
        assert_eq!(
            io::copy(
                &mut io.by_ref().take(MAX_REQUEST_BYTES as u64),
                &mut io::sink()
            )?,
            MAX_REQUEST_BYTES as u64
        );
        reply(io.get_mut(), "200 OK", "", b"ok")
    });
    let block = vec![b' '; 1024 * 1024];
    let mut exact = fresh()?;
    write!(
        exact,
        "POST {prefix}{MESSAGES} HTTP/1.1\r\nHost: gateway\r\nContent-Length: {MAX_REQUEST_BYTES}\r\n\r\n"
    )?;
    for _ in 0..64 {
        exact.write_all(&block)?;
    }
    assert!(head(&mut BufReader::new(exact))?.0.contains("200"));
    upstream.join().unwrap()?;

    let mut chunked = fresh()?;
    write!(
        chunked,
        "POST {prefix}{MESSAGES} HTTP/1.1\r\nHost: gateway\r\nTransfer-Encoding: chunked\r\n\r\n"
    )?;
    for _ in 0..64 {
        write_chunk(&mut chunked, &block)?;
    }
    // The server may close after receiving the byte beyond the limit.
    let _ = chunked.write_all(b"1\r\nx\r\n0\r\n\r\n");
    assert!(head(&mut BufReader::new(chunked))?.0.contains("413"));
    upstream_probe.set_nonblocking(true)?;
    assert_eq!(
        upstream_probe.accept().unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    Ok(())
}

#[test]
fn gateway_streams_general_post_before_the_client_finishes_uploading() -> Result<()> {
    const SIZE: usize = 1024 * 1024;
    const FIRST: usize = 16 * 1024;
    let a = listener()?;
    let b = listener()?;
    let gateway = Gateway::start(Arc::new(projection(a.local_addr()?, b.local_addr()?)?))?;
    let (addr, prefix) = gateway_address(&gateway)?;
    let (arrived, first_segment) = mpsc::channel();
    let upstream = upstream(a, move |mut io, start, headers| {
        assert_eq!(
            start.trim(),
            "POST /a/v1/models?tenant=a&beta=true HTTP/1.1"
        );
        assert_eq!(headers["x-api-key"], "synthetic-a");
        assert_eq!(length(&headers)?, SIZE);
        let mut first = vec![0; FIRST];
        io.read_exact(&mut first)?;
        assert_eq!(first, vec![b'a'; FIRST]);
        arrived.send(())?;
        let mut tail = vec![0; SIZE - FIRST];
        io.read_exact(&mut tail)?;
        assert_eq!(tail, vec![b'b'; SIZE - FIRST]);
        reply(io.get_mut(), "200 OK", "", b"ok")
    });

    let mut client = socket(addr)?;
    write!(
        client,
        "POST {prefix}/v1/models?beta=true HTTP/1.1\r\nHost: gateway\r\nContent-Length: {SIZE}\r\n\r\n"
    )?;
    client.write_all(&vec![b'a'; FIRST])?;
    first_segment.recv_timeout(TIMEOUT)?;
    println!("Upstream read {FIRST} of {SIZE} bytes before the client sent the remaining bytes");
    client.write_all(&vec![b'b'; SIZE - FIRST])?;
    assert!(head(&mut BufReader::new(client))?.0.contains("200"));
    upstream.join().unwrap()?;
    Ok(())
}

#[test]
fn gateway_streams_general_chunked_requests_and_aborts_at_64_mib() -> Result<()> {
    let a = listener()?;
    let b = listener()?;
    let gateway = Gateway::start(Arc::new(projection(a.local_addr()?, b.local_addr()?)?))?;
    let (addr, prefix) = gateway_address(&gateway)?;
    let overflow_upstream = a.try_clone()?;
    let valid_upstream = upstream(a, |mut io, start, headers| {
        assert_eq!(start.trim(), "GET /a/v1/messages?tenant=a HTTP/1.1");
        assert_eq!(headers["transfer-encoding"], "chunked");
        assert!(!headers.contains_key(header::TRAILER));
        let mut bytes = Vec::new();
        while read_chunk(&mut io, &mut bytes)? != 0 {}
        assert_eq!(bytes, br#"{"model":"matou-role/sonnet"}"#);
        reply(io.get_mut(), "200 OK", "", b"")
    });
    let mut valid = socket(addr)?;
    write!(
        valid,
        "GET {prefix}{MESSAGES} HTTP/1.1\r\nHost: gateway\r\nTransfer-Encoding: chunked\r\nTrailer: x-api-key\r\n\r\n"
    )?;
    write_chunk(&mut valid, br#"{"model":"matou-role/sonnet"}"#)?;
    valid.write_all(b"0\r\nx-api-key: client-trailer\r\n\r\n")?;
    assert!(head(&mut BufReader::new(valid))?.0.contains("200"));
    valid_upstream.join().unwrap()?;

    let (arrived, full_prefix) = mpsc::channel();
    let oversized_upstream = upstream(overflow_upstream, move |mut io, start, headers| {
        assert_eq!(start.trim(), "POST /a/v1/models?tenant=a HTTP/1.1");
        assert_eq!(headers["transfer-encoding"], "chunked");
        let mut received = 0;
        let mut bytes = Vec::new();
        while received < MAX_REQUEST_BYTES {
            bytes.clear();
            let size = read_chunk(&mut io, &mut bytes)?;
            ensure!(size > 0, "upstream received an early end of body");
            assert!(bytes.iter().all(|byte| *byte == b'a'));
            received += size;
        }
        assert_eq!(received, MAX_REQUEST_BYTES);
        arrived.send(())?;
        let outcome = io.read(&mut [0]);
        assert!(
            matches!(outcome, Ok(0))
                || matches!(&outcome, Err(error) if error.kind() == io::ErrorKind::ConnectionReset),
            "expected upstream abort after the allowed prefix, got {outcome:?}"
        );
        Ok(())
    });
    let mut client = socket(addr)?;
    write!(
        client,
        "POST {prefix}/v1/models HTTP/1.1\r\nHost: gateway\r\nTransfer-Encoding: chunked\r\n\r\n"
    )?;
    let block = vec![b'a'; 1024 * 1024];
    for _ in 0..64 {
        write_chunk(&mut client, &block)?;
    }
    full_prefix.recv_timeout(TIMEOUT)?;
    let _ = client.write_all(b"1\r\nx\r\n0\r\n\r\n");
    assert!(head(&mut BufReader::new(client))?.0.contains("413"));
    oversized_upstream.join().unwrap()?;
    println!("Upstream received {MAX_REQUEST_BYTES} bytes, then EOF/reset; client received 413");
    Ok(())
}

#[test]
fn gateway_effort_only_uses_the_source_and_rewrites_messages_not_counts() -> Result<()> {
    let temp = within_workspace()?;
    let a = listener()?;
    let addr = a.local_addr()?;
    let prepare = |mapping: Value, global: Value, key: Value| {
        let config = json!({
            "version": 1,
            "activeProviderIds": {"claude-code": "a"},
            "providers": {"claude-code": [{
                "id": "a", "endpoint": format!("http://{addr}/a"),
                "model": "literal", "apiKey": key,
                "modelMapping": {"sonnet": "shared"},
                "effortMapping": mapping, "reasoningEffort": global
            }]}
        });
        std::fs::write(temp.path().join("provider-config.json"), config.to_string())?;
        prepare_new_claude_launch(
            temp.path(),
            temp.path(),
            &Value::Null,
            None,
            BTreeMap::from([(
                "MATOU_CLAUDE_COMMAND".into(),
                std::env::current_exe()?.into_os_string(),
            )]),
            80,
            24,
        )
    };
    let key = json!("synthetic-effort-source");
    let mut plan = prepare(json!({"sonnet": "high"}), Value::Null, key.clone())?;
    let projection = Arc::new(
        plan.gateway
            .take()
            .context("effort mapping enables gateway")?,
    );
    let gateway = Gateway::start(Arc::clone(&projection))?;
    let environment = projection.project_environment(&mut plan.launch, gateway.base_url());
    assert_eq!(
        environment["ANTHROPIC_DEFAULT_SONNET_MODEL"],
        "matou-api/sonnet"
    );
    assert!(environment.get("ANTHROPIC_DEFAULT_OPUS_MODEL").is_none());
    assert_eq!(environment["NO_PROXY"], "127.0.0.1,localhost");
    assert!(projection.model_picker().is_none());
    assert_eq!(plan.launch.argv, ["--model", "literal"]);

    let literal = br#"{"model":"unmatched","output_config":{"effort":"low"}}"#;
    let (requests, workers) = record(a.try_clone()?, 4, "200 OK", "", b"ok");
    for (path, payload, expected) in [
        (
            MESSAGES,
            br#"{"model":"matou-api/sonnet","output_config":{"kept":true}}"#.as_slice(),
            json!({"model":"shared","output_config":{"effort":"high","kept":true}}),
        ),
        (
            MESSAGES,
            br#"{"model":"shared"}"#.as_slice(),
            json!({"model":"shared","output_config":{"effort":"high"}}),
        ),
        (
            COUNT,
            br#"{"model":"matou-api/sonnet","output_config":{"effort":"low"}}"#.as_slice(),
            json!({"model":"shared","output_config":{"effort":"low"}}),
        ),
    ] {
        assert_eq!(exchange(&gateway, path, payload, "")?.0, 200);
        let (start, headers, bytes) = requests.recv_timeout(TIMEOUT)?;
        assert!(start.starts_with(&format!("POST /a{path} ")));
        assert_eq!(headers["x-api-key"], "synthetic-effort-source");
        assert_eq!(serde_json::from_slice::<Value>(&bytes)?, expected);
    }
    assert_eq!(exchange(&gateway, MESSAGES, literal, "")?.0, 200);
    assert_eq!(requests.recv_timeout(TIMEOUT)?.2, literal);
    for worker in workers {
        worker.join().unwrap()?;
    }

    let global = prepare(json!({}), json!("medium"), key.clone())?;
    let gateway = Gateway::start(Arc::new(
        global.gateway.context("global effort enables gateway")?,
    ))?;
    let (requests, workers) = record(a, 1, "200 OK", "", b"ok");
    assert_eq!(exchange(&gateway, MESSAGES, literal, "")?.0, 200);
    assert_eq!(
        serde_json::from_slice::<Value>(&requests.recv_timeout(TIMEOUT)?.2)?,
        json!({"model":"unmatched","output_config":{"effort":"medium"}})
    );
    for worker in workers {
        worker.join().unwrap()?;
    }

    let direct = prepare(json!({"sonnet": "follow"}), json!("follow"), key)?;
    assert!(direct.gateway.is_none());
    let direct_environment = direct.launch.env;
    assert_eq!(
        direct_environment.get(std::ffi::OsStr::new("ANTHROPIC_BASE_URL")),
        Some(&std::ffi::OsString::from(format!("http://{addr}/a")))
    );
    assert!(
        prepare(json!({"sonnet": "high"}), Value::Null, Value::Null)?
            .gateway
            .is_none()
    );
    Ok(())
}

#[test]
fn deferred_tool_cleanup_follows_the_resolved_route_and_model() -> Result<()> {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, 1));
    let plan = projection(address, address)?;
    let runtime = tokio::runtime::Builder::new_current_thread().build()?;
    let tools = json!([
        {"name":"DeferredToolPlaceholder","input_schema":{"type":"object"},"defer_loading":true},
        {"name":"tool_search_deferred_anchor","input_schema":{},"defer_loading":true},
        {"name":"lookup","input_schema":{"type":"object"},"defer_loading":true},
        {"name":"list","input_schema":{},"defer_loading":false}
    ]);
    let stripped = json!([
        {"name":"lookup","input_schema":{"type":"object"}},
        {"name":"list","input_schema":{}}
    ]);
    let history = json!([{"role":"user","content":[{"type":"tool_result","tool_use_id":"old-call",
        "content":[{"type":"tool_reference","tool_name":"lookup"}]}]}]);
    for (provider, endpoint, model, strip) in [
        (
            Some("antigravity"),
            "http://127.0.0.1:1",
            "claude-opus",
            true,
        ),
        (Some("codex"), "http://127.0.0.1:1", "codex-fixture", true),
        (None, "http://127.0.0.1:1", "  GeMiNi-fixture  ", true),
        (None, "http://127.0.0.1:1", "  ClAuDe-fixture  ", false),
        (None, "https://API.ANTHROPIC.COM", "gpt-fixture", false),
        (
            None,
            "https://api.deepseek.com/anthropic",
            "deepseek-fixture",
            false,
        ),
    ] {
        let mut plan = plan.clone();
        let target = plan.roles[0].as_mut().context("missing opus target")?;
        target.model = model.into();
        target.connection.endpoint = endpoint.parse()?;
        target.connection.subscription = provider.map(|provider| PreparedSubscriptionTarget {
            provider_id: format!("{provider}-fixture"),
            provider,
            model: Some(model.into()),
        });
        let state = State {
            prefix: String::new(),
            projection: ProjectionSlot::new(Arc::new(plan), None, None),
            lazy_compaction: OnceCell::new(),
            lazy_subscription: AsyncMutex::default(),
            locale: InterfaceLocale::default(),
            blocked_families: Arc::default(),
            stopped: Arc::default(),
            token_usage: Arc::default(),
            read_context_usage: None,
            keep_alive_interval: gemini_sse_keepalive::INTERVAL,
            proxy_env: ProxyEnvironment::default(),
            fixture: None,
        };
        let routes = runtime.block_on(state.projection.current());
        for path in [MESSAGES, COUNT] {
            let input = json!({"model":"matou-role/opus","tools":tools,"messages":history});
            let (body, _, _, _, _) = runtime
                .block_on(state.rewrite_json(
                    &routes.projection,
                    serde_json::to_vec(&input)?.into(),
                    path,
                    None,
                    &HeaderMap::new(),
                ))
                .map_err(|fault| anyhow!("rewrite {path}: {}", fault.1))?;
            let body: Value = serde_json::from_slice(&body)?;
            assert_eq!(body["model"], model);
            let expected = if strip { &stripped } else { &tools };
            assert_eq!(&body["tools"], expected, "{provider:?} {endpoint} {path}");
            assert_eq!(body["messages"], history);
        }
    }
    Ok(())
}

#[test]
fn deferred_tool_cleanup_handles_forced_choices_and_preserves_untouched_bodies() -> Result<()> {
    let server = listener()?;
    let address = server.local_addr()?;
    let gateway = Gateway::start(Arc::new(projection(address, address)?))?;
    let cases = [
        (
            r#"{ "model":"gpt-fixture", "tools":[{"name":"DeferredToolPlaceholder","defer_loading":true}] }"#,
            Some(json!({"model":"gpt-fixture"})),
        ),
        (
            r#"{ "model":"gpt-fixture", "tools":[{"name":"tool_search_deferred_anchor","defer_loading":true}], "tool_choice":{"type":"tool","name":"tool_search_deferred_anchor"} }"#,
            Some(json!({"model":"gpt-fixture"})),
        ),
        (
            r#"{ "model":"gpt-fixture", "tools":[{"name":"DeferredToolPlaceholder","defer_loading":true}], "tool_choice":{"type":"any"} }"#,
            Some(json!({"model":"gpt-fixture","tools":[],"tool_choice":{"type":"any"}})),
        ),
        (
            r#"{ "model":"gpt-fixture", "tools":[{"name":"DeferredToolPlaceholder","defer_loading":true}], "tool_choice":{"type":"auto"} }"#,
            Some(json!({"model":"gpt-fixture","tool_choice":{"type":"auto"}})),
        ),
        (
            r#"{ "model":"gpt-fixture", "tools":[null,17,{"name":"DeferredToolPlaceholder","defer_loading":false},{"name":"lookup","defer_loading":null}], "tool_choice":{"type":"tool","name":"lookup"} }"#,
            Some(
                json!({"model":"gpt-fixture","tools":[null,17,{"name":"DeferredToolPlaceholder"},
                {"name":"lookup"}],"tool_choice":{"type":"tool","name":"lookup"}}),
            ),
        ),
        (r#"{ "model":"gpt-fixture", "tools":[] }"#, None),
        (
            r#"{ "model":"gpt-fixture", "tools":{"defer_loading":true} }"#,
            None,
        ),
        (r#"{ "model":"gpt-fixture" }"#, None),
        ("not JSON", None),
    ];
    let (requests, workers) = record(server, cases.len(), "200 OK", "", b"ok");
    for (input, expected) in cases {
        assert_eq!(
            exchange(&gateway, MESSAGES, input.as_bytes(), "Digest: fixture\r\n")?.0,
            200
        );
        let (_, headers, bytes) = requests.recv_timeout(TIMEOUT)?;
        if let Some(expected) = expected {
            assert_eq!(serde_json::from_slice::<Value>(&bytes)?, expected);
            assert!(!headers.contains_key("digest"));
        } else {
            assert_eq!(bytes, input.as_bytes());
            assert_eq!(headers["digest"], "fixture");
        }
    }
    for worker in workers {
        worker.join().unwrap()?;
    }
    Ok(())
}

fn tool_reference(kind: &str, name: &str) -> Value {
    json!({"type": kind, "tool": {"type": "tool_reference", "name": name}})
}

fn inline_tool(definition: &Value) -> Value {
    json!({"type":"tool_addition", "tool":{"type":"tool_definition", "definition":definition},
        "cache_control":{"type":"ephemeral"}})
}

fn deepseek_fixture(destination: SocketAddr) -> Result<Gateway> {
    let mut plan = projection(destination, destination)?;
    plan.source.as_mut().unwrap().endpoint =
        url::Url::parse("https://api.deepseek.com/anthropic///?tenant=fixture")?;
    Gateway::start_inner(Arc::new(plan), Some(destination))
}

#[test]
fn deepseek_tool_conversion_matrix_matches_upstream_bytes() -> Result<()> {
    let server = listener()?;
    let gateway = deepseek_fixture(server.local_addr()?)?;
    let base = json!({"name":"base","input_schema":{}});
    let base_custom = json!({"type":"custom","name":"base","input_schema":{}});
    let search =
        json!({"name":"search", "type":"web_search_20250305", "input_schema":{"pattern":r"\0"}});
    let invalid_custom = json!({"name":"  ","type":"custom","input_schema":{}});
    let lookup = json!({"name":"lookup","input_schema":{"type":"object","maxProperties":1},"eager_input_streaming":true});
    let lookup_inline = json!({"type":"custom","name":"lookup","eager_input_streaming":true,
        "input_schema":{"maxProperties":1.0,"type":"object"}});
    let lookup_deferred = json!({"name":"lookup","input_schema":{"type":"object","maxProperties":1},
        "eager_input_streaming":true,"defer_loading":true});
    let promoted_reference = json!({"type":"tool_addition", "tool":{"type":"tool_reference","name":"lookup"},
        "cache_control":{"type":"ephemeral"}});
    let removal = tool_reference("tool_removal", "lookup");
    let restore = tool_reference("tool_addition", "lookup");
    let removed_text = json!({"type":"text","text":
        "Historical tool availability change: \"lookup\" was removed. The tools list in this request determines which tools are currently callable."});
    let restored_text = json!({"type":"text","text":
        "Historical tool availability change: \"lookup\" was added. The tools list in this request determines which tools are currently callable."});
    let result = json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"old-call","content":[
        {"type":"tool_reference","tool_name":"lookup","cache_control":{"type":"ephemeral"}},
        {"type":"tool_reference","tool_name":"missing"},
        {"type":"tool_reference","tool_name":"base"}
    ]},{"type":"tool_reference","tool_name":"missing"}]});
    let missing_text = json!({"type":"text","text":
        "Historical tool discovery: \"missing\" was previously discovered, but its definition is not provided in the current tools list and it is not callable."});
    let mut removed_result = result.clone();
    removed_result["content"][0]["content"][0] = json!({"type":"text","cache_control":{"type":"ephemeral"},"text":
        "Historical tool discovery: \"lookup\" was previously discovered, but its definition is not provided in the current tools list and it is not callable."});
    removed_result["content"][0]["content"][1] = missing_text.clone();
    let mut restored_result = result.clone();
    restored_result["content"][0]["content"][1] = missing_text;
    let old_call = json!({"role":"assistant","content":[
        {"type":"thinking","thinking":"","signature":""},
        {"type":"tool_use","id":"old-call","name":"lookup","input":{"pattern":r"\0"}}]});
    let mut lookup_eager = lookup.clone();
    lookup_eager["defer_loading"] = json!(false);
    let replacement = json!({"name":"lookup","input_schema":{"type":"string"}, "type":"custom"});
    let replacement_eager =
        json!({"name":"lookup","input_schema":{"type":"string"},"defer_loading":false});
    let mut definition_text = restored_text.clone();
    definition_text
        .as_object_mut()
        .unwrap()
        .shift_remove("text");
    definition_text["cache_control"] = json!({"type":"ephemeral"});
    definition_text["text"] = restored_text["text"].clone();
    let schema = json!({"type":"object",
        "pattern":r"λ\0é\\0\\\0\01\099\x00\0",
        "properties":{"paths":{"items":[{"pattern":r"\0"},false]}},
        "patternProperties":{r"\0":{"allOf":[{"pattern":r"\0"}]}},
        "$defs":{"child":{"not":{"pattern":r"\0"}}},
        "default":{"pattern":r"\0"}, "examples":[{"pattern":r"\0"}]});
    let mut normalized_schema = schema.clone();
    normalized_schema["pattern"] = json!(r"λ\x00é\\0\\\x00\01\099\x00\x00");
    normalized_schema["properties"]["paths"]["items"][0]["pattern"] = json!(r"\x00");
    normalized_schema["patternProperties"][r"\0"]["allOf"][0]["pattern"] = json!(r"\x00");
    normalized_schema["$defs"]["child"]["not"]["pattern"] = json!(r"\x00");
    let with_tools = |tools: Value, messages: Value| json!({"model":"literal","tools":tools,"messages":messages,"max_tokens":9000});
    let cases = [
        (
            "top-level custom and server tool",
            MESSAGES,
            with_tools(json!([base_custom, search, invalid_custom]), json!([])),
            with_tools(json!([base, search, invalid_custom]), json!([])),
        ),
        (
            "promote definition and retain history fields",
            MESSAGES,
            with_tools(
                json!([base]),
                json!([{"role":"system","content":[inline_tool(&lookup)]}, old_call]),
            ),
            with_tools(
                json!([base, lookup_deferred]),
                json!([{"role":"system","content":[promoted_reference]}, old_call]),
            ),
        ),
        (
            "reuse equivalent definitions",
            MESSAGES,
            with_tools(
                json!([base, lookup_deferred]),
                json!([{"role":"system","content":[inline_tool(&lookup_inline), inline_tool(&lookup), restore]}]),
            ),
            with_tools(
                json!([base, lookup_deferred]),
                json!([{"role":"system","content":[promoted_reference, promoted_reference, restore]}]),
            ),
        ),
        (
            "remove and mark historical discoveries",
            MESSAGES,
            with_tools(
                json!([base, lookup]),
                json!([old_call, result, {"role":"system","content":[removal]}]),
            ),
            with_tools(
                json!([base]),
                json!([old_call, removed_result, {"role":"system","content":[removed_text]}]),
            ),
        ),
        (
            "restore by reference",
            MESSAGES,
            with_tools(
                json!([base, lookup_deferred]),
                json!([result, {"role":"system","content":[removal]},{"role":"system","content":[restore]}]),
            ),
            with_tools(
                json!([base, lookup_eager]),
                json!([restored_result, {"role":"system","content":[removed_text]},{"role":"system","content":[restored_text]}]),
            ),
        ),
        (
            "replace definition in registration order",
            MESSAGES,
            with_tools(
                json!([lookup, base]),
                json!([{"role":"system","content":[removal, inline_tool(&replacement)]},old_call]),
            ),
            with_tools(
                json!([replacement_eager, base]),
                json!([{"role":"system","content":[removed_text, definition_text]},old_call]),
            ),
        ),
        (
            "count_tokens schema-only pattern normalization",
            COUNT,
            with_tools(
                json!([{"name":"base","input_schema":schema}]),
                json!([{"role":"system","content":[inline_tool(&json!({"name":"lookup","input_schema":schema}))]},old_call]),
            ),
            with_tools(
                json!([{"name":"base","input_schema":normalized_schema},
                {"name":"lookup","input_schema":normalized_schema,"defer_loading":true}]),
                json!([{"role":"system","content":[promoted_reference]},old_call]),
            ),
        ),
    ];
    let (upstream, tasks) = record(server, cases.len(), "200 OK", "", b"ok");
    for (label, path, input, expected) in cases {
        let response = exchange(
            &gateway,
            path,
            &serde_json::to_vec_pretty(&input)?,
            "Digest: old\r\n",
        )?;
        assert_eq!(response.0, 200, "{label}");
        let (line, headers, bytes) = upstream.recv_timeout(TIMEOUT)?;
        assert_eq!(
            line,
            format!("POST /anthropic{path}?tenant=fixture HTTP/1.1\r\n")
        );
        assert_eq!(headers["host"], "api.deepseek.com");
        assert_eq!(headers["x-api-key"], "synthetic-a");
        assert_eq!(length(&headers)?, bytes.len());
        assert!(!headers.contains_key("digest"));
        assert_eq!(String::from_utf8(bytes)?, expected.to_string(), "{label}");
        eprintln!("PASS {label}");
    }
    for task in tasks {
        task.join().unwrap()?;
    }
    Ok(())
}

#[test]
fn deepseek_tool_errors_stop_before_upstream_and_next_request_recovers() -> Result<()> {
    let server = listener()?;
    let gateway = deepseek_fixture(server.local_addr()?)?;
    let base = json!({"name":"base","input_schema":{}});
    let deferred = json!({"name":"later","input_schema":{},"defer_loading":true});
    let server_tool = json!({"name":"search","type":"web_search_20250305"});
    let removal = tool_reference("tool_removal", "base");
    let conflicting = inline_tool(&json!({"name":"base","input_schema":{"type":"object"}}));
    let invalid = inline_tool(&json!({"name":"bad","type":"custom","input_schema":[]}));
    let request = |tools: Value, events: Value| json!({"model":"literal","tools":tools,"messages":[{"role":"system","content":events}]});
    let mut forced = request(json!([base]), json!([removal]));
    forced["tool_choice"] = json!({"type":"tool","name":"base"});
    let cases = [
        (
            "definition conflict",
            request(json!([base]), json!([conflicting])),
            "DeepSeek cannot convert inline tools with conflicting definitions for the same name.",
        ),
        (
            "invalid promoted definition",
            request(json!([base]), json!([invalid])),
            "DeepSeek cannot convert this inline tool definition: a named custom tool with input_schema is required.",
        ),
        (
            "promotion needs array",
            request(json!(null), json!([conflicting])),
            "DeepSeek inline tool conversion requires a tools array.",
        ),
        (
            "promotion needs existing non-deferred tool",
            request(json!([deferred]), json!([conflicting])),
            "DeepSeek inline tool conversion requires at least one existing non-deferred tool.",
        ),
        (
            "fold needs array",
            request(json!({}), json!([removal])),
            "DeepSeek tool availability conversion requires a tools array.",
        ),
        (
            "fold requires valid top-level client tools",
            request(json!([base, server_tool]), json!([removal])),
            "DeepSeek tool availability conversion supports only named client tools with input_schema.",
        ),
        (
            "duplicate fold tool",
            request(json!([base, base]), json!([removal])),
            "DeepSeek cannot resolve duplicate top-level tool names during tool availability conversion.",
        ),
        (
            "invalid folded definition",
            request(json!([base]), json!([removal, invalid])),
            "DeepSeek tool availability conversion supports only named client tool definitions with input_schema.",
        ),
        (
            "invalid folded reference",
            request(json!([base]), json!([tool_reference("tool_removal", " ")])),
            "DeepSeek cannot resolve this tool availability reference.",
        ),
        (
            "unknown removal",
            request(
                json!([base]),
                json!([tool_reference("tool_removal", "missing")]),
            ),
            "DeepSeek cannot resolve an unknown tool availability reference.",
        ),
        (
            "unknown restoration",
            request(
                json!([base]),
                json!([removal, tool_reference("tool_addition", "missing")]),
            ),
            "DeepSeek cannot resolve an unknown tool availability reference.",
        ),
        (
            "only deferred tools remain",
            request(json!([base, deferred]), json!([removal])),
            "DeepSeek tool availability conversion requires at least one non-deferred tool when tools remain.",
        ),
        (
            "force removed tool",
            forced,
            "DeepSeek cannot force tool_choice to a tool removed from the current tools list.",
        ),
    ];
    server.set_nonblocking(true)?;
    for (index, (label, input, message)) in cases.into_iter().enumerate() {
        let path = if index % 2 == 0 { MESSAGES } else { COUNT };
        let (status, headers, body) = exchange(&gateway, path, &serde_json::to_vec(&input)?, "")?;
        assert_eq!(status, 400, "{label}");
        assert_eq!(headers["content-type"], "application/json", "{label}");
        assert_eq!(
            String::from_utf8(body)?,
            json!({"type":"error","error":{"type":"invalid_request_error","message":message}})
                .to_string(),
            "{label}"
        );
        assert_eq!(
            server.accept().unwrap_err().kind(),
            io::ErrorKind::WouldBlock,
            "{label}"
        );
        eprintln!("PASS {label} (400, zero upstream requests)");
    }
    let (upstream, tasks) = record(server, 2, "200 OK", "", b"ok");
    let input = request(json!([base]), json!([removal]));
    let expected = request(
        json!([]),
        json!([{"type":"text","text":
        "Historical tool availability change: \"base\" was removed. The tools list in this request determines which tools are currently callable."}]),
    );
    assert_eq!(
        exchange(&gateway, COUNT, &serde_json::to_vec(&input)?, "")?.0,
        200
    );
    assert_eq!(
        String::from_utf8(upstream.recv_timeout(TIMEOUT)?.2)?,
        expected.to_string()
    );
    eprintln!("PASS same listener recovers and forwards an empty tools list");

    let reference_only = br#"{ "model":"literal", "messages":[{"role":"system","content":[{"type":"tool_addition","tool":{"type":"tool_reference","name":"not-declared"}}]}] }"#;
    assert_eq!(
        exchange(&gateway, MESSAGES, reference_only, "Digest: untouched\r\n")?.0,
        200
    );
    let (_, headers, body) = upstream.recv_timeout(TIMEOUT)?;
    assert_eq!(body, reference_only);
    assert_eq!(headers["digest"], "untouched");
    eprintln!("PASS reference-only addition leaves original bytes and digest");
    for task in tasks {
        task.join().unwrap()?;
    }
    Ok(())
}

#[test]
fn deepseek_thinking_history_carries_thinking_on_every_assistant_turn() -> Result<()> {
    let server = listener()?;
    let gateway = deepseek_fixture(server.local_addr()?)?;
    let request =
        |messages: Value| json!({"model": "literal", "max_tokens": 9000, "messages": messages});
    let user = |text: &str| json!({"role": "user", "content": [{"type": "text", "text": text}]});
    let reply =
        |text: &str| json!({"role": "assistant", "content": [{"type": "text", "text": text}]});
    let foreign =
        json!({"type": "thinking", "thinking": "", "signature": "cpa-gemini-carrier-v1:fixture"});
    let filled = json!({"type": "thinking", "thinking": "", "signature": ""});
    let reasoned = |thinking: &Value, text: &str| json!({"role": "assistant", "content": [thinking, {"type": "text", "text": text}]});
    let tool_call = json!({"role": "assistant", "content": [foreign,
        {"type": "tool_use", "id": "call-1", "name": "Read", "input": {}}]});
    let tool_result = json!({"role": "user", "content": [
        {"type": "tool_result", "tool_use_id": "call-1", "content": "ok"}]});
    let cases = [
        (
            "mixed history fills only the thinking-less turns",
            MESSAGES,
            request(json!([
                user("start"),
                reasoned(&foreign, "a"),
                user("go"),
                reply("b")
            ])),
            Some(request(json!([
                user("start"),
                reasoned(&foreign, "a"),
                user("go"),
                reasoned(&filled, "b")
            ]))),
        ),
        (
            "thinking-less run head is filled",
            MESSAGES,
            request(json!([user("start"), reply("a"), tool_call, tool_result])),
            Some(request(json!([
                user("start"),
                reasoned(&filled, "a"),
                tool_call,
                tool_result
            ]))),
        ),
        (
            "uniform thinking history keeps original bytes",
            MESSAGES,
            request(json!([
                user("start"),
                reasoned(&foreign, "a"),
                tool_call,
                tool_result
            ])),
            None,
        ),
        (
            "lone thinking-less assistant is filled",
            MESSAGES,
            request(json!([user("start"), reply("a")])),
            Some(request(json!([user("start"), reasoned(&filled, "a")]))),
        ),
        (
            "thinking-less tool rounds are filled",
            MESSAGES,
            request(json!([
                user("start"),
                {"role": "assistant", "content": [{"type": "tool_use", "id": "call-1", "name": "Read", "input": {}}]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "call-1", "content": "ok"}]},
                user("go"),
                {"role": "assistant", "content": [{"type": "tool_use", "id": "call-2", "name": "Read", "input": {}}]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "call-2", "content": "ok"}]}
            ])),
            Some(request(json!([
                user("start"),
                {"role": "assistant", "content": [filled, {"type": "tool_use", "id": "call-1", "name": "Read", "input": {}}]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "call-1", "content": "ok"}]},
                user("go"),
                {"role": "assistant", "content": [filled, {"type": "tool_use", "id": "call-2", "name": "Read", "input": {}}]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "call-2", "content": "ok"}]}
            ]))),
        ),
        (
            "count_tokens forwards the same normal form",
            COUNT,
            request(json!([user("start"), reply("a"), reasoned(&foreign, "b")])),
            Some(request(json!([
                user("start"),
                reasoned(&filled, "a"),
                reasoned(&foreign, "b")
            ]))),
        ),
    ];
    let (upstream, tasks) = record(server, cases.len(), "200 OK", "", b"ok");
    for (label, path, input, expected) in cases {
        let original = serde_json::to_vec_pretty(&input)?;
        let response = exchange(&gateway, path, &original, "")?;
        assert_eq!(response.0, 200, "{label}");
        let (line, headers, bytes) = upstream.recv_timeout(TIMEOUT)?;
        assert_eq!(
            line,
            format!("POST /anthropic{path}?tenant=fixture HTTP/1.1\r\n"),
            "{label}"
        );
        assert_eq!(headers["host"], "api.deepseek.com", "{label}");
        let expected = expected.map_or(original, |value| value.to_string().into_bytes());
        assert_eq!(bytes, expected, "{label}");
        eprintln!("PASS {label}");
    }
    for task in tasks {
        task.join().unwrap()?;
    }
    Ok(())
}

#[test]
fn deepseek_tool_conversion_follows_final_url_and_preserves_search_marker_precedence() -> Result<()>
{
    let work = listener()?;
    let search = listener()?;
    let work_address = work.local_addr()?;
    let search_address = search.local_addr()?;
    let mut to_deepseek = search_projection(work_address, search_address, None)?;
    let mut from_deepseek = search_projection(work_address, search_address, None)?;
    let official = url::Url::parse("https://api.deepseek.com/anthropic")?;
    to_deepseek.web_search.as_mut().unwrap().connection.endpoint = official.clone();
    from_deepseek.source.as_mut().unwrap().endpoint = official;
    let to_deepseek = Gateway::start_inner(Arc::new(to_deepseek), Some(search_address))?;
    let from_deepseek = Gateway::start_inner(Arc::new(from_deepseek), Some(work_address))?;
    let (work_requests, work_tasks) = record(work, 5, "200 OK", "", b"ok");
    let (search_requests, search_tasks) = record(search, 3, "200 OK", "", b"ok");
    let request = br#"{ "model":"deepseek-literal", "max_tokens":9000, "tools":[{"name":"search","type":"web_search_20250305"}], "messages":[{"role":"system","content":[{"type":"tool_addition","tool":{"type":"tool_definition","definition":{"name":"lookup","input_schema":{"pattern":"\\0"},"type":"custom"}}}]}] }"#;
    let original: Value = serde_json::from_slice(request)?;
    for path in [MESSAGES, COUNT] {
        assert_eq!(exchange(&to_deepseek, path, request, "")?.0, 200);
        let (line, headers, body) = search_requests.recv_timeout(TIMEOUT)?;
        assert_eq!(line, format!("POST /anthropic{path} HTTP/1.1\r\n"));
        assert_eq!(headers["x-api-key"], "synthetic-search");
        let mut expected = original.clone();
        expected["model"] = json!("claude-search-exact");
        if path == MESSAGES {
            expected["max_tokens"] = json!(512);
            expected["output_config"] = json!({"effort":"high"});
            expected["thinking"] = json!({"type":"adaptive"});
        }
        expected["tools"] = json!([original["tools"][0], {"name":"lookup","input_schema":{"pattern":r"\x00"},"defer_loading":true}]);
        expected["messages"][0]["content"][0] = tool_reference("tool_addition", "lookup");
        assert_eq!(String::from_utf8(body)?, expected.to_string());
        for marker in ["manual", "auto"] {
            assert_eq!(
                exchange(
                    &to_deepseek,
                    path,
                    request,
                    &format!("x-claude-code-compaction: {marker}\r\n")
                )?
                .0,
                200
            );
            let (line, headers, body) = work_requests.recv_timeout(TIMEOUT)?;
            assert_eq!(line, format!("POST /work{path}?tenant=work HTTP/1.1\r\n"));
            assert_eq!(headers["x-api-key"], "synthetic-work");
            assert_eq!(body, request);
        }
        eprintln!(
            "PASS {path} search routes into DeepSeek, both compaction markers preserve work bytes"
        );
    }
    assert_eq!(exchange(&from_deepseek, COUNT, request, "")?.0, 200);
    let (line, headers, body) = search_requests.recv_timeout(TIMEOUT)?;
    assert_eq!(
        line,
        "POST /search/v1/messages/count_tokens?tenant=search HTTP/1.1\r\n"
    );
    assert_eq!(headers["x-api-key"], "synthetic-search");
    let mut expected = original.clone();
    expected["model"] = json!("claude-search-exact");
    assert_eq!(String::from_utf8(body)?, expected.to_string());
    eprintln!("PASS DeepSeek source rerouted out preserves original tool structure");

    let mut wrong_path = projection(work_address, search_address)?;
    wrong_path.source.as_mut().unwrap().endpoint =
        url::Url::parse("https://api.deepseek.com/compatible/anthropic")?;
    let wrong_path = Gateway::start_inner(Arc::new(wrong_path), Some(work_address))?;
    assert_eq!(exchange(&wrong_path, MESSAGES, request, "")?.0, 200);
    let (line, _, body) = work_requests.recv_timeout(TIMEOUT)?;
    assert_eq!(line, "POST /compatible/anthropic/v1/messages HTTP/1.1\r\n");
    assert_eq!(body, request);
    for url in [
        "https://api.deepseek.com.example.invalid/anthropic/v1/messages",
        "https://deepseek.com/anthropic/v1/messages",
        "https://api.deepseek.com./anthropic/v1/messages",
        "https://api.deepseek.com/v1/messages",
        "https://api.deepseek.com/anthropic/v1/messages/",
    ] {
        assert!(!deepseek_tools::matches(&url.parse()?), "{url}");
    }
    eprintln!(
        "PASS exact host/path predicate; wrong-path and model alias do not trigger conversion"
    );
    for task in work_tasks.into_iter().chain(search_tasks) {
        task.join().unwrap()?;
    }
    Ok(())
}

const MANUAL: &str = "x-claude-code-compaction: manual\r\n";
const AUTO: &str = "x-claude-code-compaction: auto\r\n";
const CONTEXT_ENV: &str = "CLAUDE_CODE_MAX_CONTEXT_TOKENS";

fn compaction_config(work: SocketAddr, search: SocketAddr, compact: &str) -> Value {
    json!({
        "version": 1, "activeProviderIds": {"claude-code": "work"},
        "providers": {"claude-code": [
            {"id": "work", "endpoint": format!("http://{work}/work"), "apiKey": "synthetic-work",
             "model": "work-sonnet", "reasoningEffort": "medium",
             "modelMapping": {"opus": "work-opus", "sonnet": "work-sonnet", "haiku": "work-haiku"},
             "effortMapping": {"opus": "low", "sonnet": "high", "haiku": "follow"}},
            {"id": "search", "endpoint": format!("http://{search}/search"), "apiKey": "synthetic-search",
             "model": "search-model",
             "modelMetadata": [{"id": "search-model", "contextWindow": 50_000}]},
            {"id": "compact", "name": "Compact fixture", "endpoint": compact,
             "apiKey": "synthetic-compact", "model": "unused", "reasoningEffort": "medium",
             "modelMetadata": [
                {"id": "compact-alias", "resolvedModel": "claude-compact", "contextWindow": 190_000,
                 "supportedEfforts": ["high", "medium"], "maxOutputTokens": 512},
                {"id": "other", "resolvedModel": "other-exact",
                 "supportedEfforts": ["low", "medium", "high"], "maxOutputTokens": 256}
             ]}
        ]}
    })
}

fn compaction_preferences(search: bool, mode: &str, effort: &str) -> Value {
    let mut preferences =
        json!({"contextWindows": {}, "compaction": {"mode": mode, "effort": effort}});
    if mode == "custom" {
        preferences["compaction"]["providerId"] = json!("compact");
        preferences["compaction"]["model"] = json!("compact-alias");
    }
    if search {
        preferences["claudeWebSearch"] = json!({"providerId": "search", "model": "search-model"});
    }
    preferences
}

fn persist_compaction(data: &std::path::Path, config: &Value, preferences: &Value) -> Result<()> {
    std::fs::write(data.join("provider-config.json"), config.to_string())?;
    let db = rusqlite::Connection::open(data.join("matou.sqlite"))?;
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS preferences (key TEXT PRIMARY KEY, value_json TEXT)",
    )?;
    db.execute(
        "INSERT OR REPLACE INTO preferences VALUES ('agent.modelPreferences', ?)",
        [preferences.to_string()],
    )?;
    Ok(())
}

fn prepare_compaction(
    data: &std::path::Path,
) -> Result<matou_native_core::provider_launch::ClaudeLaunch> {
    let db = rusqlite::Connection::open(data.join("matou.sqlite"))?;
    prepare_new_claude_launch_with_budget(
        data,
        data,
        &Value::Null,
        (&read_claude_role_launch_preferences(&db), &Vec::new),
        BTreeMap::from([(
            "MATOU_CLAUDE_COMMAND".into(),
            std::env::current_exe()?.into_os_string(),
        )]),
        80,
        24,
    )
}

fn assert_no_request(listener: &StdListener) -> Result<()> {
    listener.set_nonblocking(true)?;
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    Ok(())
}

fn assert_gateway_502(response: (u16, HeaderMap, Vec<u8>), message: &str) -> Result<()> {
    assert_eq!(response.0, 502);
    let error: Value = serde_json::from_slice(&response.2)?;
    assert_eq!(error["error"]["message"], message);
    Ok(())
}

#[test]
fn compaction_marker_matrix_keeps_search_count_history_and_authentication() -> Result<()> {
    let root = within_workspace()?;
    let work = listener()?;
    let search = listener()?;
    let compact = listener()?;
    let config = compaction_config(
        work.local_addr()?,
        search.local_addr()?,
        &format!("http://{}/compact", compact.local_addr()?),
    );
    persist_compaction(
        root.path(),
        &config,
        &compaction_preferences(true, "custom", "medium"),
    )?;
    let plan = prepare_compaction(root.path())?;
    assert_eq!(
        plan.launch
            .env
            .get(std::ffi::OsStr::new(CONTEXT_ENV))
            .unwrap(),
        "190000"
    );
    assert!(!plan.gateway.as_ref().unwrap().unknown_compaction_context);
    println!("PASS exact alias startup context 190000; search context 50000 excluded");
    let gateway = Gateway::start(Arc::new(plan.gateway.unwrap()))?;
    let input = search_payload(json!("matou-role/sonnet"));
    let bytes = serde_json::to_vec_pretty(&input)?;
    let cases = [
        ("POST", MESSAGES, "", "search"),
        ("POST", MESSAGES, MANUAL, "compact"),
        ("POST", MESSAGES, AUTO, "compact"),
        (
            "POST",
            MESSAGES,
            "X-Claude-Code-Compaction: manual\r\n",
            "compact",
        ),
        (
            "POST",
            MESSAGES,
            "x-claude-code-compaction: Manual\r\n",
            "search",
        ),
        (
            "POST",
            MESSAGES,
            "x-claude-code-compaction: AUTO\r\n",
            "search",
        ),
        (
            "POST",
            MESSAGES,
            "x-claude-code-compaction: other\r\n",
            "search",
        ),
        (
            "POST",
            MESSAGES,
            "x-claude-code-compaction: manual, auto\r\n",
            "search",
        ),
        (
            "POST",
            MESSAGES,
            "x-claude-code-compaction: manual\r\nX-CLAUDE-CODE-COMPACTION: manual\r\n",
            "search",
        ),
        ("POST", COUNT, "", "search"),
        ("POST", COUNT, MANUAL, "work"),
        ("POST", COUNT, AUTO, "work"),
        (
            "POST",
            COUNT,
            "x-claude-code-compaction: Manual\r\n",
            "search",
        ),
        (
            "POST",
            COUNT,
            "x-claude-code-compaction: auto\r\nx-claude-code-compaction: auto\r\n",
            "search",
        ),
        ("GET", MESSAGES, MANUAL, "work"),
    ];
    for (method, path, marker, target) in cases {
        let listener = match target {
            "work" => &work,
            "search" => &search,
            _ => &compact,
        };
        let (received, tasks) = record(listener.try_clone()?, 1, "200 OK", "", b"ok");
        let auth = format!(
            "{marker}Authorization: bearer source\r\nCookie: source=1\r\nX-Api-Key: source\r\nX-Access-Token: source\r\nContent-MD5: stale\r\nContent-Digest: stale\r\nDigest: stale\r\n"
        );
        assert_eq!(
            exchange_method(&gateway, method, path, &bytes, &auth)?.0,
            200
        );
        let (line, headers, body) = received.recv_timeout(TIMEOUT)?;
        assert_eq!(line.trim(), format!("{method} /{target}{path} HTTP/1.1"));
        assert_eq!(length(&headers)?, body.len());
        assert_eq!(headers["x-api-key"], format!("synthetic-{target}"));
        for name in ["authorization", "cookie", "x-access-token"] {
            assert!(!headers.contains_key(name), "{target} retained {name}");
        }
        let mut expected = input.clone();
        if method == "POST" {
            expected["model"] = json!(match target {
                "work" => "work-sonnet",
                "search" => "search-model",
                _ => "claude-compact",
            });
            for name in ["content-md5", "content-digest", "digest"] {
                assert!(!headers.contains_key(name), "{target} retained {name}");
            }
        }
        if method == "POST" && path == MESSAGES {
            match target {
                "compact" => {
                    expected["output_config"]["effort"] = json!("medium");
                    expected["thinking"] = json!({"type":"adaptive"});
                    expected["max_tokens"] = json!(512);
                }
                "search" => {
                    expected["output_config"]
                        .as_object_mut()
                        .unwrap()
                        .remove("effort");
                    expected.as_object_mut().unwrap().remove("thinking");
                }
                _ => unreachable!(),
            }
        }
        assert_eq!(serde_json::from_slice::<Value>(&body)?, expected);
        for task in tasks {
            task.join().unwrap()?;
        }
        println!("PASS {method} {path} {marker:?} => {target}");
    }
    for socket in [&work, &search, &compact] {
        assert_no_request(socket)?;
    }
    Ok(())
}

#[test]
fn compaction_snapshot_inherits_role_or_target_effort_and_never_falls_back() -> Result<()> {
    let root = within_workspace()?;
    let work = listener()?;
    let search = listener()?;
    let compact = listener()?;
    let mut config = compaction_config(
        work.local_addr()?,
        search.local_addr()?,
        &format!("http://{}/compact", compact.local_addr()?),
    );
    let mut preferences = compaction_preferences(false, "custom", "follow");
    preferences["claudeRoleAssignments"] =
        json!({"sonnet": {"providerId": "work", "model":"work-sonnet"}});
    persist_compaction(root.path(), &config, &preferences)?;
    let gateway = Gateway::start(Arc::new(prepare_compaction(root.path())?.gateway.unwrap()))?;
    let (received, tasks) = record(
        compact.try_clone()?,
        3,
        "429 Too Many Requests",
        "",
        b"upstream-limit",
    );
    for (model, effort) in [
        ("matou-role/sonnet", Some("high")),
        ("matou-role/haiku", None),
        ("literal-work", Some("medium")),
    ] {
        let input = search_payload(json!(model));
        let response = exchange(&gateway, MESSAGES, &serde_json::to_vec(&input)?, MANUAL)?;
        assert_eq!(response.0, 429);
        assert_eq!(response.2, b"upstream-limit");
        let (_, _, body) = received.recv_timeout(TIMEOUT)?;
        let mut expected = input;
        expected["model"] = json!("claude-compact");
        expected["max_tokens"] = json!(512);
        if let Some(effort) = effort {
            expected["output_config"]["effort"] = json!(effort);
            expected["thinking"] = json!({"type": "adaptive"});
        } else {
            expected["output_config"]
                .as_object_mut()
                .unwrap()
                .remove("effort");
            expected.as_object_mut().unwrap().remove("thinking");
        }
        assert_eq!(serde_json::from_slice::<Value>(&body)?, expected);
        println!("PASS snapshot {model} => {effort:?}; upstream 429 passed through");
    }
    for task in tasks {
        task.join().unwrap()?;
    }
    let opus = serde_json::to_vec(&search_payload(json!("matou-role/opus")))?;
    assert_gateway_502(
        exchange(&gateway, MESSAGES, &opus, AUTO)?,
        "opus · Compact fixture: model target or reasoning effort is unavailable",
    )?;
    for socket in [&work, &search, &compact] {
        assert_no_request(socket)?;
    }
    println!("PASS unsupported inherited effort => frozen 502, zero outbound");

    preferences["compaction"]["model"] = json!("other");
    persist_compaction(root.path(), &config, &preferences)?;
    let plan = prepare_compaction(root.path())?;
    assert!(
        !plan
            .launch
            .env
            .contains_key(std::ffi::OsStr::new(CONTEXT_ENV))
    );
    assert!(plan.gateway.as_ref().unwrap().unknown_compaction_context);
    let gateway_other = Gateway::start(Arc::new(plan.gateway.unwrap()))?;
    let (received, tasks) = record(compact.try_clone()?, 1, "200 OK", "", b"ok");
    assert_eq!(exchange(&gateway_other, MESSAGES, &opus, AUTO)?.0, 200);
    let (_, _, body) = received.recv_timeout(TIMEOUT)?;
    let actual: Value = serde_json::from_slice(&body)?;
    assert_eq!(actual["model"], "other-exact");
    assert_eq!(actual["output_config"]["effort"], "low");
    assert_eq!(actual["max_tokens"], 256);
    assert!(actual.get("thinking").is_none());
    for task in tasks {
        task.join().unwrap()?;
    }
    println!("PASS unknown context allowed and recorded; non-Claude target strips thinking");

    let compact_provider = &mut config["providers"]["claude-code"][2];
    compact_provider["modelMetadata"][1]["contextWindow"] = json!(99_999);
    persist_compaction(root.path(), &config, &preferences)?;
    let error = prepare_compaction(root.path())
        .err()
        .context("expected low-window error")?;
    assert_eq!(
        error.to_string(),
        "compaction · Compact fixture: context window is under 100K"
    );
    println!("PASS context 99999 rejected at preparation");
    config["providers"]["claude-code"][2]
        .as_object_mut()
        .unwrap()
        .remove("apiKey");
    persist_compaction(root.path(), &config, &preferences)?;
    let error = prepare_compaction(root.path())
        .err()
        .context("expected target unavailable")?;
    assert_eq!(
        error.to_string(),
        "compaction · Compact fixture: model target is unavailable"
    );
    println!("PASS invalidated target rejected at preparation");

    drop(compact);
    let sonnet = serde_json::to_vec(&search_payload(json!("matou-role/sonnet")))?;
    assert_gateway_502(
        exchange(&gateway, MESSAGES, &sonnet, MANUAL)?,
        CONNECTION_ERROR,
    )?;
    for socket in [&work, &search] {
        assert_no_request(socket)?;
    }
    println!("PASS captured target connection failure => frozen 502, zero work traffic");

    preferences["compaction"] = json!({"mode": "follow", "effort": "follow"});
    preferences["claudeWebSearch"] = json!({"providerId":"search", "model":"search-model"});
    persist_compaction(root.path(), &config, &preferences)?;
    let follow = Gateway::start(Arc::new(prepare_compaction(root.path())?.gateway.unwrap()))?;
    let (received, tasks) = record(work.try_clone()?, 2, "200 OK", "", b"ok");
    for (model, expected_model, effort) in [
        ("matou-role/sonnet", "work-sonnet", "high"),
        ("literal-work", "literal-work", "medium"),
    ] {
        let input = search_payload(json!(model));
        assert_eq!(
            exchange(&follow, MESSAGES, &serde_json::to_vec(&input)?, MANUAL)?.0,
            200
        );
        let (_, _, body) = received.recv_timeout(TIMEOUT)?;
        let actual: Value = serde_json::from_slice(&body)?;
        assert_eq!(actual["model"], expected_model);
        assert_eq!(actual["output_config"]["effort"], effort);
        assert_eq!(actual["messages"], input["messages"]);
        println!("PASS follow marker with search configured: {model} => working {expected_model}");
    }
    for task in tasks {
        task.join().unwrap()?;
    }
    assert_no_request(&search)?;
    Ok(())
}

#[test]
fn compaction_legacy_resolves_lazily_retries_then_caches_and_preserves_legacy_generation()
-> Result<()> {
    let root = within_workspace()?;
    let work = listener()?;
    let compact = listener()?;
    let later = listener()?;
    let mut config = compaction_config(
        work.local_addr()?,
        work.local_addr()?,
        &format!("http://{}/first", compact.local_addr()?),
    );
    let mut preferences = compaction_preferences(false, "custom", "follow");
    preferences["contextWindows"]["compact"] = json!("512k");
    let original = config["providers"]["claude-code"][2].clone();
    config["providers"]["claude-code"][2]
        .as_object_mut()
        .unwrap()
        .remove("apiKey");
    persist_compaction(root.path(), &config, &preferences)?;
    let plan = prepare_compaction(root.path())?;
    assert!(
        !plan
            .launch
            .env
            .contains_key(std::ffi::OsStr::new(CONTEXT_ENV))
    );
    let gateway_plan = Arc::new(plan.gateway.unwrap());
    let gateway = Gateway::start(Arc::clone(&gateway_plan))?;
    let input = search_payload(json!("matou-api/sonnet"));
    let bytes = serde_json::to_vec(&input)?;
    let (work_requests, work_tasks) = record(work.try_clone()?, 2, "200 OK", "", b"ok");
    for (path, marker) in [(MESSAGES, ""), (COUNT, MANUAL)] {
        assert_eq!(exchange(&gateway, path, &bytes, marker)?.0, 200);
        let (_, _, body) = work_requests.recv_timeout(TIMEOUT)?;
        let actual: Value = serde_json::from_slice(&body)?;
        assert_eq!(actual["model"], "work-sonnet");
        assert_eq!(actual["max_tokens"], input["max_tokens"]);
        assert_eq!(actual["thinking"], input["thinking"]);
        assert_eq!(
            actual["output_config"]["effort"],
            if path == COUNT { "low" } else { "high" }
        );
        println!("PASS legacy {path} {marker:?} uses source while target is invalid");
    }
    for task in work_tasks {
        task.join().unwrap()?;
    }
    assert_gateway_502(
        exchange(&gateway, MESSAGES, &bytes, MANUAL)?,
        CONNECTION_ERROR,
    )?;
    assert_no_request(&work)?;
    assert_no_request(&compact)?;
    println!("PASS legacy first parse fails at request time; no source fallback or cached error");

    config["providers"]["claude-code"][2] = original;
    config["providers"]["claude-code"][2]["modelMetadata"][0]["contextWindow"] = json!(600_000);
    config["providers"]["claude-code"][2]["modelMetadata"][0]["maxContextWindow"] =
        json!(1_000_000);
    let mut changed = compaction_preferences(false, "follow", "follow");
    changed["contextWindows"]["compact"] = json!("maximum");
    persist_compaction(root.path(), &config, &changed)?;
    let Some(ClaudeGatewayCompaction::OnFirstRequest(preparation)) =
        gateway_plan.compaction.as_ref()
    else {
        panic!("expected legacy compaction");
    };
    assert_eq!(
        preparation.resolve(&Vec::new)?.context_window,
        Some(512_000)
    );
    let (received, tasks) = record(compact.try_clone()?, 2, "200 OK", "", b"ok");
    for attempt in [
        "retry after repair",
        "subsequent request after settings changed",
    ] {
        assert_eq!(exchange(&gateway, MESSAGES, &bytes, AUTO)?.0, 200);
        let (line, headers, body) = received.recv_timeout(TIMEOUT)?;
        assert!(line.starts_with("POST /first/"));
        assert_eq!(headers["x-api-key"], "synthetic-compact");
        let mut expected = input.clone();
        expected["model"] = json!("compact-alias");
        expected["output_config"]["effort"] = json!("high");
        expected["thinking"] = json!({"type":"adaptive"});
        assert_eq!(serde_json::from_slice::<Value>(&body)?, expected);
        println!("PASS legacy {attempt}: first target, inherited effort, adaptive, original cap");
        config["providers"]["claude-code"][2]["endpoint"] =
            json!(format!("http://{}/later", later.local_addr()?));
        config["providers"]["claude-code"][2]["apiKey"] = json!("synthetic-new");
        persist_compaction(
            root.path(),
            &config,
            &compaction_preferences(false, "custom", "medium"),
        )?;
    }
    for task in tasks {
        task.join().unwrap()?;
    }
    assert_no_request(&later)?;
    config["activeProviderIds"]["claude-code"] = json!("compact");
    persist_compaction(
        root.path(),
        &config,
        &compaction_preferences(false, "custom", "medium"),
    )?;
    let db = rusqlite::Connection::open(root.path().join("matou.sqlite"))?;
    let mut resumed = prepare_claude_launch(
        root.path(),
        root.path(),
        &json!({"providerConfigId": "work", "model": "work-sonnet"}),
        Some("synthetic-resume"),
        BTreeMap::from([(
            "MATOU_CLAUDE_COMMAND".into(),
            std::env::current_exe()?.into_os_string(),
        )]),
        80,
        24,
    )?;
    resumed.capture_restart_gateway(
        &read_claude_role_launch_preferences(&db),
        &Vec::new,
        &BTreeMap::new(),
    )?;
    let next = Gateway::start(Arc::new(resumed.gateway.unwrap()))?;
    let (working, work_tasks) = record(work.try_clone()?, 1, "200 OK", "", b"ok");
    assert_eq!(exchange(&next, COUNT, &bytes, AUTO)?.0, 200);
    let (_, headers, _) = working.recv_timeout(TIMEOUT)?;
    assert_eq!(headers["x-api-key"], "synthetic-work");
    for task in work_tasks {
        task.join().unwrap()?;
    }
    println!(
        "PASS resumed legacy run keeps the bound working source after active provider changes"
    );
    let (received, tasks) = record(
        later.try_clone()?,
        1,
        "401 Unauthorized",
        "",
        b"upstream-key",
    );
    let response = exchange(&next, MESSAGES, &bytes, MANUAL)?;
    assert_eq!(
        (response.0, response.2.as_slice()),
        (401, b"upstream-key".as_slice())
    );
    let (line, headers, body) = received.recv_timeout(TIMEOUT)?;
    assert!(line.starts_with("POST /later/"));
    assert_eq!(headers["x-api-key"], "synthetic-new");
    assert_eq!(
        serde_json::from_slice::<Value>(&body)?["output_config"]["effort"],
        "medium"
    );
    for task in tasks {
        task.join().unwrap()?;
    }
    println!("PASS new legacy generation reads updated target and forwards upstream 401");
    Ok(())
}

#[test]
fn compaction_deepseek_conversion_uses_final_destination_in_both_paths() -> Result<()> {
    let root = within_workspace()?;
    let work = listener()?;
    let deepseek = listener()?;
    let config = compaction_config(
        work.local_addr()?,
        work.local_addr()?,
        "https://api.deepseek.com/anthropic",
    );
    let request = json!({"model": "literal-work", "max_tokens": 9000,
        "tools": [{"name": "search", "type": "web_search_20250305"}],
        "messages": [{"role": "system", "content": [{"type":"tool_addition", "tool":{
          "type":"tool_definition", "definition":{"name":"lookup", "input_schema":{"pattern": r"\0"}, "type":"custom"}
        }}]}]
    });
    let bytes = serde_json::to_vec(&request)?;
    for snapshot in [false, true] {
        let mut preferences = compaction_preferences(snapshot, "custom", "high");
        preferences["compaction"]["model"] = json!("other");
        persist_compaction(root.path(), &config, &preferences)?;
        let plan = prepare_compaction(root.path())?;
        let gateway = Gateway::start_inner(
            Arc::new(plan.gateway.unwrap()),
            Some(deepseek.local_addr()?),
        )?;
        let (received, tasks) = record(deepseek.try_clone()?, 1, "200 OK", "", b"ok");
        let (status, _, response) = exchange(&gateway, MESSAGES, &bytes, MANUAL)?;
        assert_eq!(status, 200, "{}", String::from_utf8_lossy(&response));
        let (line, headers, body) = received.recv_timeout(TIMEOUT)?;
        assert!(line.starts_with("POST /anthropic/v1/messages"));
        assert_eq!(headers["host"], "api.deepseek.com");
        assert_eq!(headers["x-api-key"], "synthetic-compact");
        let actual: Value = serde_json::from_slice(&body)?;
        assert_eq!(actual["tools"][0], request["tools"][0]);
        assert_eq!(actual["tools"][1]["name"], "lookup");
        assert_eq!(actual["tools"][1]["input_schema"]["pattern"], r"\x00");
        assert_eq!(
            actual["messages"][0]["content"][0],
            tool_reference("tool_addition", "lookup")
        );
        assert_eq!(actual["max_tokens"], if snapshot { 256 } else { 9000 });
        if snapshot {
            assert!(actual.get("thinking").is_none());
        } else {
            assert_eq!(actual["thinking"], json!({"type": "adaptive"}));
        }
        for task in tasks {
            task.join().unwrap()?;
        }
        println!(
            "PASS compaction snapshot={snapshot} selected DeepSeek and converted inline definition"
        );
    }
    assert_no_request(&work)?;
    Ok(())
}

#[test]
fn deepseek_gateway_reads_resumed_usage_from_hud_before_any_upstream_response() -> Result<()> {
    let server = listener()?;
    let address = server.local_addr()?;
    let mut plan = projection(address, address)?;
    plan.source.as_mut().unwrap().endpoint = url::Url::parse("https://api.deepseek.com/anthropic")?;
    let usage = Arc::new(Mutex::new((Some(186_500), Some(512_000))));
    let observed = Arc::clone(&usage);
    let gateway = Gateway::start_gateway(
        ProjectionSlot::new(Arc::new(plan), None, None),
        Duration::from_secs(15),
        Some(Arc::new(move || observed.lock().ok().map(|usage| *usage))),
        Arc::default(),
        InterfaceLocale::default(),
        ProxyEnvironment::default(),
        Some(FixtureUpstream {
            host: deepseek_tools::HOST,
            address,
            catalog: None,
        }),
    )?;
    let (requests, workers) = record(server, 2, "200 OK", "", b"ok");
    let send = || -> Result<Value> {
        let body = json!({"model":"deepseek-fixture","max_tokens":1000,"tools":[{"name":"Bash"}],
            "messages":[{"role":"assistant","content":"earlier answer"},{"role":"user","content":"resume"}]}).to_string();
        assert_eq!(exchange(&gateway, MESSAGES, body.as_bytes(), "")?.0, 200);
        Ok(serde_json::from_slice(&requests.recv_timeout(TIMEOUT)?.2)?)
    };
    assert_eq!(
        send()?["messages"][1]["content"][1]["text"],
        "<system-reminder>\nToken usage: 186500/511000; 324500 remaining\n</system-reminder>"
    );
    usage.lock().unwrap().0 = None;
    assert_eq!(send()?["messages"][1]["content"], "resume");
    for worker in workers {
        worker.join().unwrap()?;
    }
    Ok(())
}

#[path = "tests/proxy_and_oauth.rs"]
mod proxy_and_oauth;
