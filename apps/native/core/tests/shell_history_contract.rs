use std::{fmt::Write, path::Path};

use anyhow::{Context, Result};
use matou_native_core::{
    shell_history::{
        CollectedShellBlock, Collector, ShellHistoryBlock, complete,
        format_shell_history_for_terminal, list, list_for_launch, purge,
    },
    storage::{configure_connection, migrate},
};
use rusqlite::Connection;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const FIXTURE: &str = include_str!("fixtures/shell-history-reference.json");

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    collector_cases: Vec<CollectorCase>,
    repository: Repository,
    formatting: Formatting,
}

#[derive(Deserialize)]
struct CollectorCase {
    name: String,
    steps: Vec<CollectorStep>,
}

#[derive(Deserialize)]
struct CollectorStep {
    parts: Vec<Part>,
    now: i64,
    completed: Vec<Completed>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Part {
    Text(String),
    Repeat {
        repeat: String,
        count: usize,
    },
    Lines {
        lines: usize,
        start: usize,
        trailing: bool,
    },
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Completed {
    command: String,
    exit_code: i64,
    started_at: i64,
    completed_at: i64,
    output: OutputSummary,
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OutputSummary {
    sha256: String,
    utf16_length: usize,
    lines: usize,
    prefix: String,
    suffix: String,
}

#[derive(Deserialize)]
struct Repository {
    seed: Vec<String>,
    inputs: Vec<HistoryInput>,
    ids: Vec<String>,
    reopened: Vec<ShellHistoryBlock>,
    disabled: Vec<ShellHistoryBlock>,
    limits: Vec<RepositoryLimit>,
    purged: Vec<ShellHistoryBlock>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryInput {
    session_id: String,
    cwd: String,
    #[serde(flatten)]
    block: CollectedShellBlock,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RepositoryLimit {
    name: String,
    parts: Vec<Part>,
    input: LimitInput,
    id: String,
    expected_output: OutputSummary,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LimitInput {
    session_id: String,
    cwd: String,
    command: String,
    exit_code: i64,
    started_at: i64,
    completed_at: i64,
}

#[derive(Deserialize)]
struct Formatting {
    blocks: Vec<ShellHistoryBlock>,
    result: String,
    empty: String,
}

fn expand(parts: &[Part]) -> String {
    let mut value = String::new();
    for part in parts {
        match part {
            Part::Text(text) => value.push_str(text),
            Part::Repeat { repeat, count } => value.push_str(&repeat.repeat(*count)),
            Part::Lines {
                lines,
                start,
                trailing,
            } => {
                for index in 0..*lines {
                    if index > 0 {
                        value.push('\n');
                    }
                    write!(value, "line-{}", index + start).unwrap();
                }
                if *trailing {
                    value.push('\n');
                }
            }
        }
    }
    value
}

fn describe(output: &str) -> OutputSummary {
    let units: Vec<u16> = output.encode_utf16().collect();
    OutputSummary {
        sha256: format!("{:x}", Sha256::digest(output.as_bytes())),
        utf16_length: units.len(),
        lines: output.split('\n').count(),
        prefix: String::from_utf16_lossy(&units[..units.len().min(40)]),
        suffix: String::from_utf16_lossy(&units[units.len().saturating_sub(40)..]),
    }
}

fn open_migrated(path: &Path) -> Result<Connection> {
    let mut db = Connection::open(path)?;
    configure_connection(&db)?;
    migrate(&mut db, 1)?;
    Ok(db)
}

#[test]
fn collector_matches_every_frozen_chunk() -> Result<()> {
    let fixture: Fixture = serde_json::from_str(FIXTURE)?;
    for case in fixture.collector_cases {
        let mut collector = Collector::new();
        for (index, step) in case.steps.into_iter().enumerate() {
            let actual: Vec<_> = collector
                .ingest(&expand(&step.parts), step.now)
                .into_iter()
                .map(|block| Completed {
                    command: block.command,
                    exit_code: block.exit_code,
                    started_at: block.started_at,
                    completed_at: block.completed_at,
                    output: describe(&block.output),
                })
                .collect();
            assert_eq!(actual, step.completed, "{} step {index}", case.name);
        }
    }
    Ok(())
}

#[test]
fn shell_markers_keep_legacy_base64_and_safe_exit_code_semantics() {
    let mut collector = Collector::new();
    assert!(collector.ingest("\x1b]633;E;8J-Z!gg\x1b", 10).is_empty());
    assert!(collector.ingest("\\\x1b]133;A;foo\x07", 11).is_empty());
    let invalid_exit = "\x1b]133;D;9007199254740992\x07";
    assert!(collector.ingest(invalid_exit, 12).is_empty());
    let done = collector.ingest("\x1b]133;D;-9007199254740991\x07", 13);
    assert_eq!(
        done,
        vec![CollectedShellBlock {
            command: "🙂".into(),
            output: invalid_exit.into(),
            exit_code: -9_007_199_254_740_991,
            started_at: 10,
            completed_at: 13,
        }]
    );
    let done = collector.ingest("\x1b]633;E;_w=ignore\x07\x1b]133;D;0\x07", 20);
    assert_eq!(done[0].command, "�");
}

#[test]
fn stored_history_matches_frozen_limits_reopen_and_purge() -> Result<()> {
    let fixture: Fixture = serde_json::from_str(FIXTURE)?;
    let reference = fixture.repository;
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("matou.sqlite");
    let mut db = open_migrated(&path)?;
    for statement in reference.seed {
        db.execute(&statement, [])?;
    }
    assert_eq!(reference.inputs.len(), 105);
    assert_eq!(reference.ids.len(), reference.inputs.len());
    for (input, id) in reference.inputs.into_iter().zip(reference.ids) {
        let expected = ShellHistoryBlock {
            id: id.clone(),
            session_id: input.session_id.clone(),
            cwd: input.cwd.clone(),
            command: input.block.command.clone(),
            output: input.block.output.clone(),
            exit_code: input.block.exit_code,
            started_at: input.block.started_at,
            completed_at: input.block.completed_at,
        };
        let stored = complete(&mut db, &id, &input.session_id, &input.cwd, input.block)?;
        assert_eq!(stored, expected);
    }
    db.close().map_err(|(_, error)| error)?;
    let mut db = open_migrated(&path)?;
    assert_eq!(reference.reopened.len(), 100);
    assert_eq!(list(&db, "s")?, reference.reopened);
    assert_eq!(list_for_launch(&db, "s", true)?, reference.reopened);
    assert_eq!(list_for_launch(&db, "s", false)?, reference.disabled);
    assert_eq!(
        list(&db, "s")?,
        reference.reopened,
        "disabled reads must preserve the stored rows"
    );

    for limit in reference.limits {
        let input = limit.input;
        let block = CollectedShellBlock {
            command: input.command,
            output: expand(&limit.parts),
            exit_code: input.exit_code,
            started_at: input.started_at,
            completed_at: input.completed_at,
        };
        let stored = complete(&mut db, &limit.id, &input.session_id, &input.cwd, block)?;
        assert_eq!(
            describe(&stored.output),
            limit.expected_output,
            "{}",
            limit.name
        );
        let saved = list(&db, &input.session_id)?
            .into_iter()
            .find(|block| block.id == limit.id)
            .context("prune removed the new block")?;
        assert_eq!(saved, stored, "{}", limit.name);
    }

    purge(&db, "s")?;
    assert_eq!(list(&db, "s")?, reference.purged);
    Ok(())
}

#[test]
fn restoration_text_matches_frozen_crlf_and_divider() -> Result<()> {
    let fixture: Fixture = serde_json::from_str(FIXTURE)?;
    let reference = fixture.formatting;
    let divider = "──────── 会话已恢复 ────────";
    assert_eq!(
        format_shell_history_for_terminal(&reference.blocks, divider),
        reference.result
    );
    assert_eq!(
        format_shell_history_for_terminal(&[], divider),
        reference.empty
    );
    Ok(())
}
