use std::borrow::Cow;

use anyhow::Result;
use matou_native_terminal::decode_clipboard;
use serde_json::Value;

#[test]
fn clipboard_decode_matches_frozen_osc_handler() -> Result<()> {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/clipboard-reference.json"))?;
    for case in fixture["cases"].as_array().unwrap() {
        let content = case_text(case, "content");
        let name = &case["name"];
        assert_eq!(
            content.encode_utf16().count(),
            usize::try_from(case["contentUnits"].as_u64().unwrap())?,
            "{name}"
        );
        assert_eq!(
            decode_clipboard(&content).as_deref(),
            case["text"].as_str(),
            "{name}"
        );
    }
    Ok(())
}

#[test]
fn clipboard_stream_matches_frozen_parser() -> Result<()> {
    use matou_native_terminal::{Terminal, TerminalOptions};
    let fixture: Value = serde_json::from_str(include_str!("fixtures/clipboard-reference.json"))?;
    for case in fixture["streams"].as_array().unwrap() {
        let mut terminal = Terminal::new(TerminalOptions::new(80, 24, 7, 14))?;
        let mut writes = Vec::new();
        for chunk in case["chunks"].as_array().unwrap() {
            // Split at every byte, including within UTF-8 and terminators.
            for byte in chunk.as_str().unwrap().as_bytes() {
                terminal.feed(&[*byte]);
                writes.extend(terminal.drain_clipboard());
            }
        }
        assert_eq!(
            serde_json::to_value(writes)?,
            case["writes"],
            "{}",
            case["name"]
        );
        assert!(
            terminal.drain_responses().is_empty(),
            "queries must not receive clipboard data"
        );
    }
    Ok(())
}

#[test]
fn clipboard_stream_skips_invalid_raw_utf8_but_keeps_real_replacement_characters() -> Result<()> {
    use matou_native_terminal::{Terminal, TerminalOptions};

    let mut terminal = Terminal::new(TerminalOptions::new(80, 24, 7, 14))?;
    for data in [
        b"\x1b]52;\xff;YQ==\x07".as_slice(),
        b"\x1b]52;\xf0",
        b"\x9f",
        b";Yg==\x07",
        b"\x1b]52;c;Yw==\xff\x07",
        "\x1b]52;c;ZA==�\x07".as_bytes(),
    ] {
        terminal.feed(data);
    }
    assert_eq!(terminal.drain_clipboard(), ["a", "b", "c"]);
    assert!(terminal.drain_responses().is_empty());
    Ok(())
}

// Compare after capture so a discrepancy cannot hide the remaining cases.
#[test]
#[ignore = "run with the K-terminal-clipboard gate driver"]
fn capture_native_clipboard_gate() -> Result<()> {
    use anyhow::{Context, ensure};
    use matou_native_terminal::{Terminal, TerminalOptions};
    use serde_json::json;
    use std::path::Path;

    let directory = std::env::var_os("MATOU_CLIPBOARD_GATE_RUN").context("gate run directory")?;
    let directory = Path::new(&directory);
    let cases: Value =
        serde_json::from_reader(std::fs::File::open(directory.join("inputs.json"))?)?;
    let mut results = Vec::new();
    for case in cases["paste"].as_array().unwrap() {
        let mut terminal = Terminal::new(TerminalOptions::new(80, 24, 7, 14))?;
        for (mode, sequence) in [(true, b"\x1b[?2004h"), (false, b"\x1b[?2004l")] {
            terminal.feed(sequence);
            let actual = terminal.input_modes()?.bracketed_paste;
            ensure!(
                actual == mode,
                "DEC mode {:?} for {}",
                sequence,
                case["name"]
            );
            let text = case_text(case, "text");
            let output = run_probe(
                directory,
                if actual { "on" } else { "off" },
                text.as_bytes(),
            )?;
            results.push(json!({
                "kind": "paste", "name": case["name"], "bracketed": actual,
                "data": byte_hex(&output), "text": String::from_utf8(output)?, "error": null
            }));
        }
    }
    for case in cases["decode"].as_array().unwrap() {
        let input = case_text(case, "text");
        let text = decode_clipboard(&input);
        results.push(json!({
            "kind": "decode", "name": case["name"], "contentUnits": input.encode_utf16().count(),
            "write": text.as_deref().map(|text| byte_hex(text.as_bytes())), "text": text, "error": null
        }));
    }
    for case in cases["stream"].as_array().unwrap() {
        let mut terminal = Terminal::new(TerminalOptions::new(80, 24, 7, 14))?;
        let mut chunks = case["chunks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|chunk| {
                if let Some(hex) = chunk["hex"].as_str() {
                    ensure!(hex.len() % 2 == 0, "odd-length hex input");
                    (0..hex.len())
                        .step_by(2)
                        .map(|index| {
                            u8::from_str_radix(&hex[index..index + 2], 16).map_err(Into::into)
                        })
                        .collect()
                } else {
                    Ok(case_text(chunk, "text").as_bytes().to_vec())
                }
            })
            .collect::<Result<Vec<Vec<u8>>>>()?;
        let mut parser_input = None;
        if case["ptyDecode"].as_bool() == Some(true) {
            // The fixed malformed-byte samples have no NUL, which frames the probe's chunks.
            ensure!(
                chunks.iter().all(|chunk| !chunk.contains(&0)),
                "NUL in PTY decoder fixture"
            );
            let output = run_probe(directory, "decode-stream", &chunks.join(&0))?;
            chunks = output
                .split(|&byte| byte == 0)
                .filter(|chunk| !chunk.is_empty())
                .map(<[u8]>::to_vec)
                .collect();
            parser_input = Some(
                chunks
                    .iter()
                    .map(|chunk| byte_hex(chunk))
                    .collect::<Vec<_>>(),
            );
        }
        let mut writes = Vec::new();
        for chunk in &chunks {
            terminal.feed(chunk);
            writes.extend(terminal.drain_clipboard());
        }
        let (visible, error) = match terminal.screen() {
            Ok(screen) => {
                let mut rows: Vec<_> = screen
                    .cells
                    .chunks_exact(screen.cols)
                    .map(|row| {
                        let text: String = row
                            .iter()
                            .filter(|cell| cell.width > 0)
                            .map(|cell| {
                                if cell.has_content {
                                    cell.text.as_str()
                                } else {
                                    " "
                                }
                            })
                            .collect();
                        text.trim_end_matches(' ').to_owned()
                    })
                    .collect();
                rows.truncate(
                    rows.iter()
                        .rposition(|row| !row.is_empty())
                        .map_or(0, |index| index + 1),
                );
                (Some(rows), None)
            }
            Err(error) => (None, Some(error.to_string())),
        };
        let mut result = json!({
            "kind": "stream", "name": case["name"],
            "writes": writes.iter().map(|text| byte_hex(text.as_bytes())).collect::<Vec<_>>(),
            "texts": writes, "responses": byte_hex(&terminal.drain_responses()),
            "visibleRows": visible, "error": error
        });
        if let Some(parser_input) = parser_input {
            result["parserInput"] = json!(parser_input);
        }
        results.push(result);
    }
    let file = std::fs::File::create(directory.join("native.json"))?;
    serde_json::to_writer_pretty(file, &results)?;
    println!("Captured {} native clipboard observations", results.len());
    Ok(())
}

fn case_text<'a>(case: &'a Value, field: &str) -> Cow<'a, str> {
    if let Some(text) = case[field].as_str() {
        return Cow::Borrowed(text);
    }
    let pattern = &case["generated"];
    Cow::Owned(format!(
        "{}{}{}",
        pattern["prefix"].as_str().unwrap(),
        pattern["unit"]
            .as_str()
            .unwrap()
            .repeat(usize::try_from(pattern["count"].as_u64().unwrap()).unwrap()),
        pattern["suffix"].as_str().unwrap(),
    ))
}

fn run_probe(directory: &std::path::Path, mode: &str, input: &[u8]) -> Result<Vec<u8>> {
    use anyhow::ensure;
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut probe = Command::new(directory.join("work/clipboard-probe"))
        .arg(mode)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    probe.stdin.take().unwrap().write_all(input)?;
    let output = probe.wait_with_output()?;
    ensure!(
        output.status.success(),
        "clipboard probe failed in {mode} mode"
    );
    Ok(output.stdout)
}

fn byte_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(hex, "{byte:02x}").unwrap();
    }
    hex
}
