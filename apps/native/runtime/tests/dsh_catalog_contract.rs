use std::{
    fs::{self, File, FileTimes, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Barrier, Mutex},
    thread,
    time::{Duration, UNIX_EPOCH},
};

use matou_native_runtime::{
    dsh_catalog::{
        CatalogError, DetailOptions, DshSessionCatalog, DshTitleState, ListOptions, SearchScope,
        TitleSource, encode_project_path,
    },
    dsh_events::{DshEventOutcome, DshEvents},
    dsh_launch::DshLaunchMode,
    dsh_resume::{BeginOutcome, BindingState, DshResume, ResumeFailure, ResumeState},
};
use ruzstd::encoding::{CompressionLevel, compress_to_vec};
use serde_json::{Value, json, to_value};

const REFERENCE: &str =
    include_str!("../../../../tooling/native-parity/fixtures/provider-history-reference.json");
const FIXTURE_CWD: &str = "/__fixture__/workspace";

fn plain_row(row: &Value) -> Vec<u8> {
    let mut bytes = match row.as_str() {
        Some(raw) => raw.as_bytes().to_vec(),
        None => serde_json::to_vec(row).unwrap(),
    };
    bytes.push(b'\n');
    bytes
}

fn frame(bytes: &[u8]) -> Vec<u8> {
    compress_to_vec(bytes, CompressionLevel::Fastest)
}

fn transcript_path(root: &Path, workspace: &str, id: &str, compressed: bool) -> PathBuf {
    let workspace = std::path::absolute(workspace).unwrap();
    root.join(encode_project_path(workspace.to_str().unwrap()))
        .join(id)
        .join(if compressed {
            "session.v4.jsonl.zstd"
        } else {
            "session.v4.jsonl"
        })
}

fn write_file(path: &Path, bytes: &[u8], modified: Option<u64>) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut file = File::create(path).unwrap();
    file.write_all(bytes).unwrap();
    if let Some(ms) = modified {
        file.set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_millis(ms)))
            .unwrap();
    }
}

fn append(path: &Path, bytes: &[u8]) {
    OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(bytes)
        .unwrap();
}

fn write_fixture(root: &Path, file: &Value) {
    let rows = file["rows"].as_array().unwrap();
    let header_cwd = rows[0]["cwd"].as_str().unwrap();
    let id = file["id"].as_str().unwrap();
    let compressed = file["zstd"] == true;
    let path = transcript_path(root, header_cwd, id, compressed);
    let line_ending = file["lineEnding"].as_str().unwrap_or("\n");
    let mut content = Vec::new();
    for (at, row) in rows.iter().enumerate() {
        let mut bytes = plain_row(row);
        bytes.pop();
        if at + 1 != rows.len() || file["noFinalNewline"] != true {
            bytes.extend_from_slice(line_ending.as_bytes());
        }
        if compressed {
            // Match the independent frames emitted by the provider.
            content.extend(frame(&bytes));
        } else {
            content.extend(bytes);
        }
    }
    write_file(&path, &content, file["mtimeMs"].as_u64());
}

#[test]
fn replays_every_dsh_step_in_the_frozen_provider_history_fixture() {
    let fixture: Value = serde_json::from_str(REFERENCE).unwrap();
    let mut checked = Vec::new();
    for case in fixture["cases"].as_array().unwrap() {
        let input = &case["input"];
        let steps = input["steps"].as_array().unwrap();
        let dsh_steps: Vec<_> = steps
            .iter()
            .enumerate()
            .filter(|(_, step)| step["args"]["provider"] == "dsh")
            .collect();
        if dsh_steps.is_empty() {
            continue;
        }
        let root = tempfile::tempdir().unwrap();
        let mut catalog = DshSessionCatalog::new(root.path());
        for name in input["datasets"].as_array().unwrap() {
            for file in fixture["datasets"][name.as_str().unwrap()]
                .as_array()
                .unwrap()
            {
                if file["provider"] == "dsh" {
                    write_fixture(root.path(), file);
                }
            }
        }
        let case_id = case["id"].as_str().unwrap();
        for (at, step) in dsh_steps {
            let args = &step["args"];
            let cwd = Path::new(args["cwd"].as_str().unwrap_or(FIXTURE_CWD));
            let id = args["providerSessionId"].as_str().unwrap_or_default();
            let query = args["query"].as_str().unwrap_or_default();
            let limit = args["limit"].as_i64();
            let offset = args["offset"].as_i64();
            let actual = match step["operation"].as_str().unwrap() {
                "list" => catalog
                    .list(
                        cwd,
                        ListOptions {
                            query,
                            limit,
                            offset,
                            scope: if args["searchScope"] == "metadata" {
                                SearchScope::Metadata
                            } else {
                                SearchScope::All
                            },
                            ..Default::default()
                        },
                    )
                    .map(|value| to_value(value).unwrap()),
                "detail" => catalog
                    .detail(
                        cwd,
                        id,
                        DetailOptions {
                            query,
                            limit,
                            before_event_index: args["beforeEventIndex"].as_i64(),
                            around_event_index: args["aroundEventIndex"].as_i64(),
                        },
                    )
                    .map(|value| to_value(value).unwrap()),
                "search" => catalog
                    .search(cwd, id, query, offset, limit)
                    .map(|value| to_value(value).unwrap()),
                operation => panic!("unhandled DSH fixture operation {operation}"),
            };
            let mut expected = case["expected"]["steps"][at].clone();
            project_fixture_cwd(&mut expected);
            if let Some(sessions) = expected.get_mut("sessions").and_then(Value::as_array_mut) {
                for session in sessions {
                    project_fixture_cwd(session);
                }
            }
            if step["fail"] == true {
                let error = actual.unwrap_err();
                match case_id {
                    "dsh-required-event-unsupported" => {
                        assert!(matches!(error, CatalogError::UnsupportedEvent))
                    }
                    "dsh-sequence-repeated" => assert!(matches!(error, CatalogError::Corrupt)),
                    _ => panic!("unhandled DSH fixture error {case_id}: {error}"),
                }
                assert_eq!(
                    json!({"error": {"name": "Error", "message": error.message(false)}}),
                    expected
                );
            } else {
                assert_eq!(
                    actual.unwrap_or_else(|error| panic!("{case_id} step {at}: {error}")),
                    expected,
                    "{case_id} step {at}"
                );
            }
            checked.push((case_id, at));
        }
    }
    assert_eq!(checked.len(), 9, "{checked:?}");
}

fn project_fixture_cwd(summary: &mut Value) {
    // The frozen fixture uses synthetic POSIX paths. Windows resolves them
    // under the current drive, including when the workspace does not exist.
    if let Some(cwd) = summary["cwd"].as_str() {
        summary["cwd"] = json!(std::path::absolute(cwd).unwrap());
    }
}

fn header(workspace: &str, id: &str) -> Value {
    json!({"type": "session", "version": 4, "id": id, "cwd": workspace, "createdAt": 1000})
}

fn user(seq: usize, text: &str) -> Value {
    json!({"type": "user/message", "seq": seq, "surfaceOp": "append", "data": {"content": [{"text": text}]}})
}

#[test]
fn plain_and_compressed_live_tails_refresh_without_rewriting_history() {
    let root = tempfile::tempdir().unwrap();
    let mut catalog = DshSessionCatalog::new(root.path());
    let cwd = Path::new(FIXTURE_CWD);
    for compressed in [false, true] {
        let id = if compressed { "compressed" } else { "plain" };
        let path = transcript_path(root.path(), FIXTURE_CWD, id, compressed);
        let encode = |bytes: &[u8]| {
            if compressed {
                frame(bytes)
            } else {
                bytes.to_vec()
            }
        };
        let mut initial = encode(&plain_row(&header(FIXTURE_CWD, id)));
        initial.extend(encode(&plain_row(&user(0, "first"))));
        let next = plain_row(&user(1, "next"));
        let encoded_next = encode(next.strip_suffix(b"\n").unwrap());
        let written = encoded_next.len() / 2;
        initial.extend_from_slice(&encoded_next[..written]);
        write_file(&path, &initial, None);
        assert_eq!(
            catalog
                .detail(cwd, id, DetailOptions::default())
                .unwrap()
                .summary
                .event_count,
            1
        );
        assert_eq!(fs::read(&path).unwrap(), initial);

        append(&path, &encoded_next[written..]);
        assert_eq!(
            catalog
                .detail(cwd, id, DetailOptions::default())
                .unwrap()
                .events[1]
                .text,
            "next"
        );
        if !compressed {
            append(&path, b"\n{broken\n");
        } else {
            let malformed = frame(b"{broken");
            append(&path, &malformed);
        }
        assert!(matches!(
            catalog.validate_history(cwd, id),
            Err(CatalogError::Corrupt)
        ));
    }
}

#[test]
fn validates_workspace_header_and_pages_metadata_and_body_independently() {
    let root = tempfile::tempdir().unwrap();
    let mut catalog = DshSessionCatalog::new(root.path());
    let cwd = Path::new(FIXTURE_CWD);
    for (id, time) in [("older", 10), ("newer", 20), ("newest", 30)] {
        let bytes = [
            plain_row(&header(FIXTURE_CWD, id)),
            plain_row(&json!({"type": "session/title", "seq": 0, "data": {"title": "Native title"}})),
            plain_row(&json!({"type": "request/header", "seq": 1, "data": {"header": {"config": {"model": "test-model"}}}})),
            plain_row(&user(2, "earlier needle")),
            plain_row(&user(3, "middle")),
            plain_row(&user(4, "latest needle")),
        ].concat();
        write_file(
            &transcript_path(root.path(), FIXTURE_CWD, id, false),
            &bytes,
            Some(time),
        );
    }
    let foreign = transcript_path(root.path(), FIXTURE_CWD, "foreign", false);
    write_file(
        &foreign,
        &[
            plain_row(&header("/__fixture__/other", "foreign")),
            b"{broken\n".to_vec(),
        ]
        .concat(),
        None,
    );
    let list = catalog
        .list(
            cwd,
            ListOptions {
                query: "NEEDLE",
                offset: Some(1),
                limit: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(list.pagination.total, 3);
    assert_eq!(list.pagination.next_offset, 2);
    assert!(list.pagination.has_more);
    assert_eq!(list.sessions[0].provider_session_id, "newer");
    assert_eq!(list.sessions[0].match_count, 2);
    assert_eq!(list.sessions[0].model.as_deref(), Some("test-model"));

    let metadata = catalog
        .list(
            cwd,
            ListOptions {
                query: "chosen",
                scope: SearchScope::Metadata,
                title_for_session: Some(&|id| (id == "newer").then(|| "Chosen title".into())),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(metadata.pagination.total, 1);
    assert_eq!(metadata.sessions[0].title, "Chosen title");
    assert_eq!(metadata.sessions[0].match_count, 1);
    assert!(metadata.sessions[0].hits.is_empty());
    assert!(
        catalog
            .list(
                cwd,
                ListOptions {
                    query: "needle",
                    scope: SearchScope::Metadata,
                    ..Default::default()
                }
            )
            .unwrap()
            .sessions
            .is_empty()
    );

    let detail = catalog
        .detail(
            cwd,
            "newer",
            DetailOptions {
                query: "needle",
                before_event_index: Some(3),
                limit: Some(2),
                ..Default::default()
            },
        )
        .unwrap();
    assert_eq!(
        detail
            .events
            .iter()
            .map(|row| row.matched)
            .collect::<Vec<_>>(),
        [true, false]
    );
    assert_eq!(detail.transcript_title, "Native title");
    assert!(detail.page.has_later);
    let search = catalog
        .search(cwd, "newer", "needle", Some(1), Some(1))
        .unwrap();
    assert_eq!(search.pagination.total, 2);
    assert_eq!(search.hits[0].event_index, 3);
    assert!(!search.pagination.has_more);
    assert!(matches!(
        catalog.validate_history(cwd, "foreign"),
        Err(CatalogError::Missing)
    ));
    assert!(matches!(
        catalog.validate_history(cwd, "../escape"),
        Err(CatalogError::InvalidId)
    ));
    assert!(
        DshSessionCatalog::new(root.path().join("missing"))
            .list(cwd, ListOptions::default())
            .unwrap()
            .sessions
            .is_empty()
    );
}

fn observation(
    run: &str,
    id: &str,
    sequence: usize,
    kind: &str,
    persisted: bool,
    title: &str,
) -> Value {
    json!({
        "dsh_observation": 1, "run_id": run, "foreground": true, "generation": 1,
        "sequence": sequence, "session_id": id, "event_type": kind, "ready": kind == "ready",
        "persisted": persisted, "hud": {"title": title}
    })
}

fn apply_title(
    state: &mut DshTitleState,
    events: &mut DshEvents,
    run: &str,
    event: &Value,
) -> bool {
    match events.ingest(event) {
        DshEventOutcome::Observed(update) => state.observe(run, &update),
        _ => false,
    }
}

#[test]
fn live_title_yields_to_manual_name_and_ignores_late_runs() {
    let mut title = DshTitleState::new("DSH".into(), TitleSource::Default, None);
    assert!(!title.restore_auto_title());
    let mut previous = DshEvents::new("previous", None);
    title.start_run("previous");
    assert!(apply_title(
        &mut title,
        &mut previous,
        "previous",
        &observation("previous", "saved", 1, "ready", true, "  自动标题  ")
    ));
    assert_eq!(
        (title.title(), title.source()),
        ("自动标题", TitleSource::Auto)
    );
    assert!(title.rename("  手动命名  "));
    assert!(apply_title(
        &mut title,
        &mut previous,
        "previous",
        &observation("previous", "saved", 2, "turn/start", true, "最新自动标题")
    ));
    assert_eq!(
        (title.title(), title.source(), title.provider_title()),
        ("手动命名", TitleSource::Manual, Some("最新自动标题"))
    );

    title.start_run("current");
    title.end_run("previous");
    assert!(!apply_title(
        &mut title,
        &mut previous,
        "previous",
        &observation("previous", "saved", 3, "turn/end", true, "旧运行迟到标题")
    ));
    assert!(title.restore_auto_title());
    assert_eq!(
        (title.title(), title.source()),
        ("最新自动标题", TitleSource::Auto)
    );
    let mut current = DshEvents::new("current", Some("saved"));
    assert!(!apply_title(
        &mut title,
        &mut current,
        "current",
        &observation("current", "saved", 1, "ready", false, "临时标题")
    ));
    assert!(apply_title(
        &mut title,
        &mut current,
        "current",
        &observation("current", "saved", 2, "ready", true, "新运行标题")
    ));
    assert!(!apply_title(
        &mut title,
        &mut current,
        "current",
        &observation("current", "saved", 3, "turn/start", true, "新运行标题")
    ));
    assert_eq!(title.title(), "新运行标题");
}

#[test]
fn matching_native_ready_is_required_and_parallel_starts_share_one_slot() {
    let root = tempfile::tempdir().unwrap();
    let cwd = Path::new(FIXTURE_CWD);
    write_file(
        &transcript_path(root.path(), FIXTURE_CWD, "saved", false),
        &plain_row(&header(FIXTURE_CWD, "saved")),
        None,
    );
    let restore = Mutex::new(DshResume::new("saved"));
    let barrier = Barrier::new(2);
    let outcomes = thread::scope(|scope| {
        let start = |run| {
            let mut catalog = DshSessionCatalog::new(root.path());
            barrier.wait();
            restore.lock().unwrap().start(run, &mut catalog, cwd)
        };
        let a = scope.spawn(move || ("a", start("a")));
        let b = scope.spawn(move || ("b", start("b")));
        [a.join().unwrap(), b.join().unwrap()]
    });
    let [(run, _)] = outcomes.iter().filter(|(_, outcome)| matches!(outcome, BeginOutcome::Launch(DshLaunchMode::Resume { expected_session_id }) if expected_session_id == "saved")).collect::<Vec<_>>()[..] else {
        panic!("expected one launch: {outcomes:?}");
    };
    assert_eq!(
        outcomes
            .iter()
            .filter(|(_, outcome)| *outcome == BeginOutcome::InFlight)
            .count(),
        1
    );
    let mut restore = restore.into_inner().unwrap();
    for event in [
        observation("old", "saved", 1, "ready", true, "old"),
        observation(run, "saved", 1, "turn/start", true, "premature"),
        observation(run, "saved", 2, "ready", false, "provisional"),
        json!({"run_id": run, "session_id": "saved", "persisted": true}),
    ] {
        assert_eq!(restore.observe(run, &event), DshEventOutcome::Ignored);
        assert!(matches!(restore.state(), ResumeState::Starting(_)));
    }

    let ready = observation(run, "saved", 3, "ready", true, "confirmed");
    assert!(matches!(
        restore.observe(run, &ready),
        DshEventOutcome::Observed(_)
    ));
    assert_eq!(
        restore.state(),
        &ResumeState::Ready {
            session_id: "saved".into(),
            persisted: true
        }
    );
    assert_eq!(
        restore.original_binding(),
        ("saved", BindingState::Available)
    );
    assert!(!restore.timeout(run));
}

#[test]
fn missing_then_corrupt_history_parks_until_retried_or_expired_by_fresh_start() {
    let root = tempfile::tempdir().unwrap();
    let mut catalog = DshSessionCatalog::new(root.path());
    let mut restore = DshResume::new("saved");
    let cwd = Path::new(FIXTURE_CWD);
    let path = transcript_path(root.path(), FIXTURE_CWD, "saved", false);
    assert_eq!(
        restore.start("missing", &mut catalog, cwd),
        BeginOutcome::Parked
    );
    assert_eq!(
        restore.state(),
        &ResumeState::Failed(ResumeFailure::HistoryMissing)
    );
    assert_eq!(restore.original_binding(), ("saved", BindingState::Failed));

    write_file(&path, b"{bad\n", None);
    assert_eq!(
        restore.start("mount-again", &mut catalog, cwd),
        BeginOutcome::Parked
    );
    assert_eq!(
        restore.retry("corrupt", &mut catalog, cwd),
        BeginOutcome::Parked
    );
    assert_eq!(
        restore.state(),
        &ResumeState::Failed(ResumeFailure::HistoryCorrupt)
    );

    write_file(&path, &plain_row(&header(FIXTURE_CWD, "saved")), None);
    assert_eq!(
        restore.retry("fixed", &mut catalog, cwd),
        BeginOutcome::Launch(DshLaunchMode::Resume {
            expected_session_id: "saved".into()
        })
    );
    let wrong = observation("fixed", "other", 1, "ready", true, "wrong");
    assert!(
        matches!(restore.observe("fixed", &wrong), DshEventOutcome::IdentityMismatch { expected_session_id, actual_session_id: "other" } if expected_session_id == "saved")
    );
    assert_eq!(
        restore.state(),
        &ResumeState::Failed(ResumeFailure::IdentityMismatch {
            expected: "saved".into(),
            actual: "other".into()
        })
    );
    let late = observation("fixed", "saved", 2, "ready", true, "late");
    assert_eq!(restore.observe("fixed", &late), DshEventOutcome::Ignored);

    assert_eq!(
        restore.start_fresh("fresh"),
        BeginOutcome::Launch(DshLaunchMode::Fresh)
    );
    assert_eq!(restore.original_binding(), ("saved", BindingState::Expired));
    assert_eq!(
        restore.retry("racing", &mut catalog, cwd),
        BeginOutcome::InFlight
    );
    assert_eq!(restore.observe("fixed", &late), DshEventOutcome::Ignored);
    let provisional = observation("fresh", "new-id", 1, "ready", false, "new");
    assert!(matches!(
        restore.observe("fresh", &provisional),
        DshEventOutcome::Observed(_)
    ));
    assert_eq!(
        restore.state(),
        &ResumeState::Ready {
            session_id: "new-id".into(),
            persisted: false
        }
    );
    let persisted = observation("fresh", "new-id", 2, "ready", true, "new");
    assert!(matches!(
        restore.observe("fresh", &persisted),
        DshEventOutcome::Observed(_)
    ));
    assert_eq!(
        restore.state(),
        &ResumeState::Ready {
            session_id: "new-id".into(),
            persisted: true
        }
    );
    assert_eq!(restore.original_binding(), ("saved", BindingState::Expired));
    assert_eq!(
        restore.retry("old-target", &mut catalog, cwd),
        BeginOutcome::Unavailable
    );
    assert!(path.is_file());
}
