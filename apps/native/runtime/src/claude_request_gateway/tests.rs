use std::{
    collections::BTreeMap,
    io::{self, BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener as StdListener, TcpStream},
    thread::JoinHandle,
};

use anyhow::ensure;
use matou_native_core::provider_launch::{
    parse_claude_role_assignments, prepare_new_claude_launch,
};

use super::*;

const TIMEOUT: Duration = Duration::from_secs(8);
const MESSAGES: &str = "/v1/messages";
const COUNT: &str = "/v1/messages/count_tokens";
const GZIP: &[u8] = &[
    31, 139, 8, 0, 0, 0, 0, 0, 2, 3, 203, 72, 205, 201, 201, 7, 0, 134, 166, 16, 54, 5, 0, 0, 0,
];

fn within_workspace() -> Result<tempfile::TempDir> {
    Ok(tempfile::tempdir_in(env!("CARGO_MANIFEST_DIR"))?)
}

fn projection(source: SocketAddr, other: SocketAddr) -> Result<ClaudeGatewayProjection> {
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

fn listener() -> io::Result<StdListener> {
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

fn head(stream: &mut impl BufRead) -> Result<(String, HeaderMap)> {
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

fn length(headers: &HeaderMap) -> Result<usize> {
    Ok(headers
        .get(header::CONTENT_LENGTH)
        .context("no content-length")?
        .to_str()?
        .parse()?)
}

fn open_request(gateway: &Gateway, path: &str, body: &[u8], headers: &str) -> Result<TcpStream> {
    let (addr, prefix) = gateway_address(gateway)?;
    let mut stream = socket(addr)?;
    write!(
        stream,
        "POST {prefix}{path} HTTP/1.1\r\nHost: local-gateway\r\nContent-Length: {}\r\n{headers}\r\n",
        body.len()
    )?;
    stream.write_all(body)?;
    Ok(stream)
}

fn exchange(
    gateway: &Gateway,
    path: &str,
    body: &[u8],
    headers: &str,
) -> Result<(u16, HeaderMap, Vec<u8>)> {
    let mut stream = BufReader::new(open_request(gateway, path, body, headers)?);
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

fn upstream(
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

fn record(
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

fn write_chunk(stream: &mut TcpStream, bytes: &[u8]) -> io::Result<()> {
    write!(stream, "{:x}\r\n", bytes.len())?;
    stream.write_all(bytes)?;
    stream.write_all(b"\r\n")
}

fn read_chunk(stream: &mut BufReader<TcpStream>, output: &mut Vec<u8>) -> Result<usize> {
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
    const FIRST: &[u8] = b"event: message_start\ndata: first\n\n";
    const TAIL: &[u8] = b"event: message_stop\ndata: done\n\n";
    let a = listener()?;
    let b = listener()?;
    let gateway = Gateway::start(Arc::new(projection(a.local_addr()?, b.local_addr()?)?))?;
    let (release, wait) = mpsc::channel();
    let (finished, writing) = mpsc::channel();
    let padding = vec![b'x'; 64 * 1024];
    let upstream = upstream(b, move |mut io, _, headers| {
        io.read_exact(&mut vec![0; length(&headers)?])?;
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
    let mut client = BufReader::new(open_request(
        &gateway,
        MESSAGES,
        br#"{"model":"sonnet","stream":true}"#,
        "",
    )?);
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
    Ok(())
}

#[test]
fn gateway_cancels_upstream_before_headers_after_first_sse_and_on_drop() -> Result<()> {
    for (send_headers, retire) in [(false, false), (true, false), (true, true)] {
        let a = listener()?;
        let b = listener()?;
        let gateway = Gateway::start(Arc::new(projection(a.local_addr()?, b.local_addr()?)?))?;
        let (addr, _) = gateway_address(&gateway)?;
        let (received, wait) = mpsc::channel();
        let upstream = upstream(b, move |mut io, _, headers| {
            io.read_exact(&mut vec![0; length(&headers)?])?;
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
        let mut client = BufReader::new(open_request(
            &gateway,
            MESSAGES,
            br#"{"model":"sonnet","stream":true}"#,
            "",
        )?);
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
