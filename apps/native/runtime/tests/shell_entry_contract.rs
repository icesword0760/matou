use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use matou_native_runtime::flow;
use serde::Deserialize;
use serde_json::{Value, json};

#[path = "../src/shell_input.rs"]
mod shell_input;

#[derive(Deserialize)]
#[serde(untagged)]
enum Write {
    Text(String),
    Repeated {
        repeat: String,
        count: usize,
        suffix: String,
    },
}

impl Write {
    fn as_text(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Repeated {
                repeat,
                count,
                suffix,
            } => repeat.repeat(*count) + suffix,
        }
    }
}

fn case<'a>(cases: &'a [Value], id: &str) -> &'a Value {
    cases
        .iter()
        .find(|case| case["id"] == id)
        .unwrap_or_else(|| panic!("missing fixture case {id}"))
}

#[test]
#[ignore = "run the K-shell-command-fixture gate to load its local evidence"]
fn frozen_shell_entry_fixture() -> Result<()> {
    let directory = PathBuf::from(
        env::var_os("MATOU_SHELL_ENTRY_FIXTURE_DIR").context("MATOU_SHELL_ENTRY_FIXTURE_DIR")?,
    );
    let fixture: Value = serde_json::from_slice(&fs::read(directory.join("fixture.json"))?)?;
    let terminal = fixture["terminal"]
        .as_array()
        .context("terminal fixtures")?;
    let managed = fixture["managedCc"]
        .as_array()
        .context("managed cc fixtures")?;
    let mt = fixture["mt"].as_array().context("mt fixtures")?;

    let mut ids = HashSet::new();
    for case in terminal.iter().chain(managed).chain(mt) {
        let id = case["id"].as_str().context("fixture case ID")?;
        assert!(ids.insert(id), "duplicate case {id}");
        for field in ["input", "expected", "source", "boundary"] {
            assert!(!case[field].is_null(), "{id} missing {field}");
        }
    }

    let lf = fs::read(directory.join("shell-input.lf.tsv"))?;
    let crlf = fs::read(directory.join("shell-input.crlf.tsv"))?;
    for (id, bytes, newline, command) in [
        ("tsv-lf", &lf, &b"\n"[..], "claude "),
        ("tsv-crlf", &crlf, &b"\r\n"[..], ""),
    ] {
        let sample = case(terminal, id);
        assert_eq!(bytes.strip_suffix(newline), Some(&b"claude\t "[..]));
        assert_eq!(
            sample["input"]["writes"][0].as_str().unwrap().as_bytes(),
            bytes
        );
        assert_eq!(sample["expected"]["steps"][0]["submitted"], true);
        assert_eq!(sample["expected"]["steps"][0]["command"], command);
    }

    let mut native_cases = 0;
    for sample in terminal {
        if sample["native"] != true {
            continue;
        }
        native_cases += 1;
        let id = sample["id"].as_str().context("terminal case ID")?;
        let writes: Vec<Write> = serde_json::from_value(sample["input"]["writes"].clone())?;
        let steps = sample["expected"]["steps"]
            .as_array()
            .context("terminal steps")?;
        assert_eq!(writes.len(), steps.len(), "{id}");
        let mut input = shell_input::ShellInput::default();
        for (index, (write, step)) in writes.iter().zip(steps).enumerate() {
            assert_eq!(
                input
                    .observe(write.as_text().as_bytes())
                    .map(|launch| launch.mode),
                step["launch"]["permissionMode"].as_str(),
                "{id}, write {}",
                index + 1
            );
        }
    }
    assert!(native_cases > 0);

    let managed_argv = &case(managed, "cc-managed-argv")["expected"];
    assert_eq!(managed_argv["command"], "mt");
    assert_eq!(
        managed_argv["frozenCli"]["exitCode"],
        managed_argv["exitCode"]
    );
    assert_eq!(
        managed_argv["frozenCli"]["stderr"],
        json!(["多余参数：current"])
    );
    assert_eq!(managed_argv["frozenCli"]["requests"], json!([]));
    for id in [
        "cc-managed-no-token",
        "cc-managed-no-endpoint",
        "cc-managed-no-mt",
    ] {
        assert_eq!(
            case(managed, id)["expected"],
            json!({
                "command": "claude",
                "argv": ["--dangerously-skip-permissions", "arg with spaces", "字"],
                "exitCode": 0
            }),
            "{id}"
        );
    }

    let invalid = &case(mt, "mt-invalid-mode")["expected"];
    assert_eq!(invalid["exitCode"], 2);
    assert_eq!(
        invalid["stderr"],
        json!(["--permission-mode 需要 default、acceptEdits、plan 或 bypassPermissions"])
    );
    assert_eq!(invalid["requests"], json!([]));
    for mode in ["default", "acceptEdits", "plan", "bypassPermissions"] {
        let id = format!("mt-cwd-mode-{mode}");
        let sample = case(mt, &id);
        let argv = sample["input"]["argv"].as_array().context("mt argv")?;
        let cwd = &argv[argv
            .iter()
            .position(|arg| arg == "--cwd")
            .context("cwd flag")?
            + 1];
        assert_eq!(sample["expected"]["exitCode"], 0, "{id}");
        assert_eq!(
            sample["expected"]["requests"],
            json!([{
                "method": "structure.create.session",
                "params": {
                    "canvas": { "kind": "current", "entity": "canvas" },
                    "profile": "claude-code",
                    "cwd": cwd,
                    "permissionMode": mode,
                    "submissionKey": "fixture-submission",
                    "enter": true
                }
            }]),
            "{id}"
        );
    }
    println!(
        "Loaded {} cases and replayed {native_cases} with ShellInput.",
        ids.len()
    );
    Ok(())
}
