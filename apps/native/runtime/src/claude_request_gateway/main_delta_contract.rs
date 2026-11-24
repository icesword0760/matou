use std::{
    io::{self, BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener},
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};

use super::{
    Gateway,
    tests::{
        TIMEOUT, exchange, head, length, listener, open_request, projection, read_chunk, record,
        upstream, write_chunk,
    },
};
use anyhow::{Result, ensure};
use flate2::{
    Compression,
    write::{DeflateEncoder, GzEncoder, ZlibEncoder},
};
use serde_json::{Value, json};

fn main_delta_gateway(ordinary: SocketAddr, deepseek: SocketAddr) -> Result<Gateway> {
    let mut plan = projection(ordinary, deepseek)?;
    let sonnet = plan.roles[2].as_mut().unwrap();
    sonnet.model = "deepseek-flash".into();
    sonnet.max_output_tokens = None;
    sonnet.connection.endpoint = url::Url::parse("https://api.deepseek.com/anthropic")?;
    plan.roles[0].as_mut().unwrap().model = "google/gemini-2.5-pro".into();
    Gateway::start_with_interval(Arc::new(plan), Duration::from_millis(20), Some(deepseek))
}

const MESSAGES: &str = "/v1/messages";
const COUNT: &str = "/v1/messages/count_tokens";
const PING: &[u8] = b"event: ping\ndata: {\"type\":\"ping\"}\n\n";
const SSE: &str = "Content-Type: text/event-stream; charset=utf-8\r\n";
const JSON: &str = "Content-Type: application/json\r\n";

fn forward(
    gateway: &Gateway,
    server: &TcpListener,
    path: &str,
    request_headers: &str,
    body: &Value,
    response: (&'static str, &'static str, &'static [u8]),
) -> Result<Value> {
    let (status, headers, bytes) = response;
    let (records, tasks) = record(server.try_clone()?, 1, status, headers, bytes);
    let (_, _, received) = exchange(gateway, path, body.to_string().as_bytes(), request_headers)?;
    assert_eq!(received, bytes);
    for task in tasks {
        task.join().unwrap()?;
    }
    let (_, _, delivered) = records.recv_timeout(TIMEOUT)?;
    Ok(serde_json::from_slice(&delivered)?)
}

fn reminder(used: u64, total: u64) -> Value {
    let remaining = total.saturating_sub(used);
    json!({
        "type":"text",
        "text":format!("<system-reminder>\nToken usage: {used}/{total}; {remaining} remaining\n</system-reminder>")
    })
}

const OBSERVED_TOOL: &[u8] = concat!(
    "event: message_start\r\n",
    "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":246,\"cache_creation_input_tokens\":3,\"cache_read_input_tokens\":382464,\"output_tokens\":1}}}\r\n\r\n",
    "event: content_block_start\n",
    "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"call_bash_1\"}}\n\n",
    "data: {\"type\":\"message_delta\",\"delta\":{\"usage\":{\"output_tokens\":458}}}"
).as_bytes();

const TEXT_REPLY: &[u8] = br#"{"content":[{"type":"thinking","signature":"sig-stop-1"},{"type":"text","text":"18 passed, 19 remaining."}],"usage":{"input_tokens":300,"cache_read_input_tokens":412700,"output_tokens":200}}"#;

const UNUSED_REPLY: (&str, &str, &[u8]) = ("200 OK", JSON, b"{}");

#[test]
fn deepseek_observes_actual_tokens_across_routes_and_honors_request_boundaries() -> Result<()> {
    let ordinary = listener()?;
    let deepseek = listener()?;
    let gateway = main_delta_gateway(ordinary.local_addr()?, deepseek.local_addr()?)?;
    let start = json!({
        "model":"matou-role/sonnet",
        "messages":[{"role":"user","content":"run tests"}]
    });
    let first = forward(
        &gateway,
        &deepseek,
        MESSAGES,
        "",
        &start,
        ("200 OK", SSE, OBSERVED_TOOL),
    )?;
    assert_eq!(first["messages"], start["messages"]);
    let tool_result = json!({
        "type":"tool_result",
        "tool_use_id":"call_bash_1",
        "content":"ok",
        "cache_control":{"type":"ephemeral"}
    });
    let mut continuation = json!({
        "model":"matou-role/sonnet",
        "max_tokens":90_000,
        "messages":[
            {"role":"user","content":"run tests"},
            {"role":"assistant","content":[{"type":"tool_use","id":"call_bash_1","name":"Bash","input":{}}]},
            {"role":"user","content":[tool_result]}
        ]
    });
    let followed = forward(
        &gateway,
        &deepseek,
        MESSAGES,
        "",
        &continuation,
        UNUSED_REPLY,
    )?;
    assert_eq!(
        followed["messages"][2]["content"],
        json!([tool_result, reminder(383_171, 1_016_576)])
    );

    for (path, header) in [
        (COUNT, ""),
        (MESSAGES, "x-claude-code-compaction: auto\r\n"),
    ] {
        let skipped = forward(
            &gateway,
            &deepseek,
            path,
            header,
            &continuation,
            ("200 OK", JSON, TEXT_REPLY),
        )?;
        assert_eq!(skipped["messages"], continuation["messages"]);
    }

    let stop = json!({
        "model":"matou-role/sonnet",
        "max_tokens":32_000,
        "messages":[
            {"role":"assistant","content":[{"type":"text","text":"18 passed, 19 remaining."}]},
            {"role":"user","content":"Stop hook feedback: 19 tests still failing"}
        ]
    });
    assert_eq!(
        forward(&gateway, &deepseek, MESSAGES, "", &stop, UNUSED_REPLY)?["messages"],
        stop["messages"]
    );
    assert_eq!(
        forward(
            &gateway,
            &deepseek,
            MESSAGES,
            "",
            &stop,
            ("502 Bad Gateway", JSON, TEXT_REPLY)
        )?["messages"],
        stop["messages"]
    );
    continuation["model"] = json!("matou-role/opus");
    let switched = forward(
        &gateway,
        &ordinary,
        MESSAGES,
        "",
        &continuation,
        ("200 OK", JSON, TEXT_REPLY),
    )?;
    assert_eq!(switched["messages"], continuation["messages"]);

    let mut stopped = forward(&gateway, &deepseek, MESSAGES, "", &stop, UNUSED_REPLY)?;
    assert_eq!(
        stopped["messages"][1]["content"],
        json!([
            {"type":"text","text":"Stop hook feedback: 19 tests still failing"},
            reminder(413_200, 1_016_576)
        ])
    );
    stopped["model"] = stop["model"].clone();
    assert_eq!(
        forward(&gateway, &deepseek, MESSAGES, "", &stopped, UNUSED_REPLY)?["messages"],
        stopped["messages"]
    );

    let utility =
        json!({"model":"matou-role/sonnet","messages":[{"role":"user","content":"Title"}]});
    assert_eq!(
        forward(&gateway, &deepseek, MESSAGES, "", &utility, UNUSED_REPLY)?["messages"],
        utility["messages"]
    );
    drop(gateway);
    let replacement = main_delta_gateway(ordinary.local_addr()?, deepseek.local_addr()?)?;
    assert_eq!(
        forward(&replacement, &deepseek, MESSAGES, "", &stop, UNUSED_REPLY)?["messages"],
        stop["messages"]
    );
    Ok(())
}

fn compressed(encoding: &str, bytes: &[u8]) -> Result<Vec<u8>> {
    macro_rules! encode {
        ($encoder:ident) => {{
            let mut writer = $encoder::new(Vec::new(), Compression::default());
            writer.write_all(bytes)?;
            writer.finish()?
        }};
    }
    Ok(match encoding {
        "gzip" => encode!(GzEncoder),
        "deflate" => encode!(ZlibEncoder),
        "raw" => encode!(DeflateEncoder),
        _ => unreachable!(),
    })
}

#[test]
fn deepseek_compressed_json_and_sse_preserve_bytes_and_report_actual_usage() -> Result<()> {
    for (media, response, assistant, used) in [
        (
            SSE,
            OBSERVED_TOOL,
            json!([{"type":"tool_use","id":"call_bash_1"}]),
            383_171,
        ),
        (JSON, TEXT_REPLY, json!("18 passed, 19 remaining."), 413_200),
    ] {
        for format in ["gzip", "deflate", "raw", "bad-gzip"] {
            let encoding = match format {
                "raw" => "deflate",
                "bad-gzip" => "gzip",
                _ => format,
            };
            let mut bytes =
                compressed(if format == "bad-gzip" { "gzip" } else { format }, response)?;
            if format == "bad-gzip" {
                let crc = bytes.len() - 8;
                bytes[crc] ^= 1;
            }
            let ordinary = listener()?;
            let deepseek = listener()?;
            let gateway = main_delta_gateway(ordinary.local_addr()?, deepseek.local_addr()?)?;
            let (send_tail, tail) = mpsc::channel();
            let upstream_bytes = bytes.clone();
            let server = upstream(deepseek.try_clone()?, move |mut io, _, headers| {
                assert_eq!(headers["accept-encoding"], "gzip, deflate");
                drain_request(&mut io, length(&headers)?)?;
                let out = io.get_mut();
                write!(
                    out,
                    "HTTP/1.1 200 OK\r\n{media}Content-Encoding: {encoding}\r\nTransfer-Encoding: chunked\r\n\r\n"
                )?;
                write_chunk(out, &upstream_bytes[..1])?;
                tail.recv_timeout(TIMEOUT)?;
                for piece in upstream_bytes[1..].chunks(7) {
                    write_chunk(out, piece)?;
                }
                write_chunk(out, b"")?;
                Ok(())
            });
            let mut client = BufReader::new(open_request(
                &gateway,
                MESSAGES,
                &stream_body("matou-role/sonnet", media == SSE),
                "Accept-Encoding: gzip, deflate, br\r\n",
            )?);
            let (status, headers) = head(&mut client)?;
            assert!(status.contains("200 OK"));
            assert_eq!(headers["content-encoding"], encoding);
            let mut received = Vec::new();
            assert_eq!(read_chunk(&mut client, &mut received)?, 1);
            assert_eq!(received, bytes[..1]);
            send_tail.send(())?;
            while read_chunk(&mut client, &mut received)? != 0 {}
            assert_eq!(received, bytes, "{media} {format}");
            server.join().unwrap()?;

            let follow_up = json!({
                "model":"matou-role/sonnet",
                "messages":[
                    {"role":"assistant","content":assistant},
                    {"role":"user","content":"continue"}
                ]
            });
            let delivered = forward(&gateway, &deepseek, MESSAGES, "", &follow_up, UNUSED_REPLY)?;
            let content = &delivered["messages"][1]["content"];
            if format == "bad-gzip" {
                assert_eq!(content, "continue");
            } else {
                assert_eq!(content[1], reminder(used, 1_048_576), "{media} {format}");
            }
        }
    }
    Ok(())
}

#[test]
fn deepseek_continuation_uses_gzip_usage_from_another_upstream() -> Result<()> {
    let ordinary = listener()?;
    let deepseek = listener()?;
    let ordinary_address = ordinary.local_addr()?;
    let gateway = main_delta_gateway(ordinary_address, deepseek.local_addr()?)?;
    let bytes = compressed("gzip", TEXT_REPLY)?;
    let upstream_bytes = bytes.clone();
    let server = upstream(ordinary, move |mut io, _, headers| {
        assert_eq!(headers["host"], ordinary_address.to_string());
        assert_eq!(headers["accept-encoding"], "gzip, br");
        drain_request(&mut io, length(&headers)?)?;
        let out = io.get_mut();
        write!(
            out,
            "HTTP/1.1 200 OK\r\n{JSON}Content-Encoding: gzip\r\nContent-Length: {}\r\n\r\n",
            upstream_bytes.len()
        )?;
        out.write_all(&upstream_bytes)?;
        Ok(())
    });
    let (status, headers, received) = exchange(
        &gateway,
        MESSAGES,
        &stream_body("matou-role/opus", false),
        "Accept-Encoding: gzip, br\r\n",
    )?;
    assert_eq!(status, 200);
    assert_eq!(headers["content-encoding"], "gzip");
    assert_eq!(received, bytes);
    server.join().unwrap()?;

    let follow_up = json!({
        "model":"matou-role/sonnet",
        "messages":[
            {"role":"assistant","content":"18 passed, 19 remaining."},
            {"role":"user","content":"continue"}
        ]
    });
    let delivered = forward(&gateway, &deepseek, MESSAGES, "", &follow_up, UNUSED_REPLY)?;
    assert_eq!(
        delivered["messages"][1]["content"],
        json!([{"type":"text","text":"continue"}, reminder(413_200, 1_048_576)])
    );
    Ok(())
}

#[test]
fn brotli_filter_is_limited_to_tracked_deepseek_requests() -> Result<()> {
    let ordinary = listener()?;
    let deepseek = listener()?;
    let gateway = main_delta_gateway(ordinary.local_addr()?, deepseek.local_addr()?)?;
    for (server, model, path, headers, expected) in [
        (
            &deepseek,
            "matou-role/sonnet",
            MESSAGES,
            "Accept-Encoding: Br; q=1\r\n",
            vec!["gzip, deflate"],
        ),
        (
            &deepseek,
            "matou-role/sonnet",
            MESSAGES,
            "Accept-Encoding: gzip;q=0.8\r\nAccept-Encoding: Br;q=0.5, deflate, identity\r\n",
            vec!["gzip;q=0.8, deflate, identity"],
        ),
        (
            &deepseek,
            "matou-role/sonnet",
            COUNT,
            "Accept-Encoding: gzip, br\r\n",
            vec!["gzip, br"],
        ),
        (
            &ordinary,
            "matou-role/opus",
            MESSAGES,
            "Accept-Encoding: gzip, br\r\n",
            vec!["gzip, br"],
        ),
        (&deepseek, "matou-role/sonnet", MESSAGES, "", vec![]),
    ] {
        let (requests, tasks) = record(server.try_clone()?, 1, "200 OK", JSON, b"{}");
        assert_eq!(
            exchange(&gateway, path, &stream_body(model, false), headers)?.0,
            200
        );
        let (_, delivered, _) = requests.recv_timeout(TIMEOUT)?;
        let encodings = delivered
            .get_all("accept-encoding")
            .iter()
            .map(|value| value.to_str())
            .collect::<std::result::Result<Vec<_>, _>>()?;
        assert_eq!(encodings, expected);
        for task in tasks {
            task.join().unwrap()?;
        }
    }
    Ok(())
}

fn drain_request(io: &mut impl Read, bytes: usize) -> Result<()> {
    io.read_exact(&mut vec![0; bytes])?;
    Ok(())
}

fn stream_body(model: &str, stream: bool) -> Vec<u8> {
    serde_json::to_vec(
        &json!({"model":model,"stream":stream,"messages":[{"role":"user","content":"start"}]}),
    )
    .unwrap()
}

#[test]
fn gemini_keepalive_only_inserts_between_frames_and_rearms_after_real_data() -> Result<()> {
    const PARTIAL: &[u8] = b"event: content_block_delta\r\ndata: {\"text\":\"hello";
    const PENULTIMATE: &[u8] = b" world\"}\r\n\r";
    const STOP: &[u8] = b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
    let ordinary = listener()?;
    let deepseek = listener()?;
    let gateway = main_delta_gateway(ordinary.local_addr()?, deepseek.local_addr()?)?;
    let (advance, steps) = mpsc::channel();
    let tracked = upstream(ordinary, move |mut io, _, headers| {
        drain_request(&mut io, length(&headers)?)?;
        let out = io.get_mut();
        let bytes = PARTIAL.len() + PENULTIMATE.len() + 1 + STOP.len();
        write!(
            out,
            "HTTP/1.1 200 OK\r\n{SSE}Content-Length: {bytes}\r\n\r\n"
        )?;
        for frame in [PARTIAL, PENULTIMATE, b"\n", STOP] {
            steps.recv_timeout(TIMEOUT)?;
            out.write_all(frame)?;
        }
        Ok(())
    });
    let client = open_request(
        &gateway,
        MESSAGES,
        &stream_body("matou-role/opus", true),
        "",
    )?;
    let mut client = BufReader::new(client);
    let (_, headers) = head(&mut client)?;
    assert!(!headers.contains_key("content-length"));

    fn next(stream: &mut BufReader<std::net::TcpStream>, expected: &[u8]) -> Result<()> {
        let mut actual = Vec::new();
        read_chunk(stream, &mut actual)?;
        assert_eq!(actual, expected);
        Ok(())
    }
    fn quiet(stream: &mut BufReader<std::net::TcpStream>) -> Result<()> {
        stream
            .get_ref()
            .set_read_timeout(Some(Duration::from_millis(90)))?;
        let idle = stream.fill_buf();
        ensure!(
            matches!(idle, Err(ref error) if matches!(error.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut)),
            "unexpected body bytes while idle: {idle:?}"
        );
        stream.get_ref().set_read_timeout(Some(TIMEOUT))?;
        Ok(())
    }
    for _ in 0..30 {
        next(&mut client, PING)?;
    }
    quiet(&mut client)?;

    for part in [PARTIAL, PENULTIMATE] {
        advance.send(())?;
        next(&mut client, part)?;
        quiet(&mut client)?;
    }
    advance.send(())?;
    next(&mut client, b"\n")?;
    for _ in 0..30 {
        next(&mut client, PING)?;
    }
    quiet(&mut client)?;
    advance.send(())?;
    next(&mut client, STOP)?;
    assert_eq!(read_chunk(&mut client, &mut Vec::new())?, 0);
    tracked.join().unwrap()?;
    Ok(())
}

#[test]
fn keepalive_excludes_other_models_nonstreaming_errors_and_encoded_responses() -> Result<()> {
    const BODY: &[u8] = b"data: upstream\n\n";
    let ordinary = listener()?;
    let deepseek = listener()?;
    let gateway = main_delta_gateway(ordinary.local_addr()?, deepseek.local_addr()?)?;
    for (model, stream, status, encoding, media) in [
        ("claude-sonnet", true, "200 OK", "", SSE),
        ("matou-role/sonnet", true, "200 OK", "", SSE),
        ("matou-role/opus", false, "200 OK", "", SSE),
        ("matou-role/opus", true, "429 Too Many Requests", "", SSE),
        (
            "matou-role/opus",
            true,
            "200 OK",
            "Content-Encoding: gzip\r\n",
            SSE,
        ),
        (
            "matou-role/opus",
            true,
            "200 OK",
            "Connection: content-encoding\r\nContent-Encoding: gzip\r\n",
            SSE,
        ),
        ("matou-role/opus", true, "200 OK", "", JSON),
    ] {
        let listener = if model == "matou-role/sonnet" {
            &deepseek
        } else {
            &ordinary
        };
        let server = upstream(listener.try_clone()?, move |mut io, _, headers| {
            drain_request(&mut io, length(&headers)?)?;
            let out = io.get_mut();
            write!(
                out,
                "HTTP/1.1 {status}\r\n{media}{encoding}Content-Length: {}\r\n\r\n",
                BODY.len()
            )?;
            thread::sleep(Duration::from_millis(65));
            out.write_all(BODY)?;
            Ok(())
        });
        let (_, headers, received) = exchange(&gateway, MESSAGES, &stream_body(model, stream), "")?;
        assert_eq!(length(&headers)?, BODY.len());
        assert_eq!(received, BODY);
        server.join().unwrap()?;
    }
    Ok(())
}

#[test]
fn gemini_keepalive_stops_when_upstream_fails_or_client_disconnects() -> Result<()> {
    for downstream_disconnects in [false, true] {
        let ordinary = listener()?;
        let deepseek = listener()?;
        let gateway = main_delta_gateway(ordinary.local_addr()?, deepseek.local_addr()?)?;
        let (release, wait) = mpsc::channel();
        let server = upstream(ordinary, move |mut io, _, headers| {
            drain_request(&mut io, length(&headers)?)?;
            io.get_mut().write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n")?;
            write_chunk(io.get_mut(), b"data: ready\n\n")?;
            if downstream_disconnects {
                assert_eq!(io.read(&mut [0])?, 0);
            } else {
                wait.recv_timeout(TIMEOUT)?;
            }
            Ok(())
        });
        let mut client = BufReader::new(open_request(
            &gateway,
            MESSAGES,
            &stream_body("gemini-2.5-pro", true),
            "",
        )?);
        head(&mut client)?;
        let mut received = Vec::new();
        read_chunk(&mut client, &mut received)?;
        assert_eq!(received, b"data: ready\n\n");
        received.clear();
        read_chunk(&mut client, &mut received)?;
        assert_eq!(received, PING);
        if downstream_disconnects {
            drop(client);
            server.join().unwrap()?;
        } else {
            release.send(())?;
            server.join().unwrap()?;
            let mut line = String::new();
            assert_eq!(client.read_line(&mut line)?, 0, "{line:?}");
        }
    }
    Ok(())
}
