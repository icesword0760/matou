use super::*;
use crate::{
    host::{
        Command, DatabaseRecoveryAction, Event, LaunchOptions, RuntimeHost,
        database_recovery_request_id,
    },
    owner::ShellOwner,
};
use matou_native_core::{
    hierarchy::{
        self, BootstrapWindowInput, HierarchyIds,
        canvas::{self, CanvasIds, CanvasMessages, CreateShellSiblingInput},
        tasks::{self, CreateTaskInput},
        workspaces::{self, CreateWorkspaceInput},
    },
    journal::{Frame, JournalOptions, Record, SegmentJournal},
    pty::LaunchProfile,
    startup::{self, BootstrapResult},
    storage::CommandMetadata,
};
use rusqlite::{params, types::Value as SqlValue};
use std::{collections::BTreeMap, sync::atomic::AtomicI64};
use tempfile::TempDir;

const NOW: i64 = 1_700_000_000_000;
const GENERATION: &str = "fixture-generation";
const WINDOW: &str = "window-1";
const WORKSPACE: &str = "/__fixture__/workspace";
const RUN: &str = "run-fixture";
const READ: &str = "terminal.read-current";
static QUEUE_CLOCK: AtomicI64 = AtomicI64::new(NOW);

fn fixed_now() -> i64 {
    NOW
}
fn queue_now() -> i64 {
    QUEUE_CLOCK.load(Ordering::SeqCst)
}
fn fixture_id(number: u64) -> String {
    format!("00000000-0000-4000-8000-{number:012}")
}
fn hierarchy_ids(start: u64) -> HierarchyIds {
    HierarchyIds {
        workspace_id: fixture_id(start),
        execution_context_id: fixture_id(start + 1),
        task_id: fixture_id(start + 2),
        scene_id: fixture_id(start + 3),
        root_node_id: fixture_id(start + 4),
        session_id: fixture_id(start + 6),
        mount_id: fixture_id(start + 7),
    }
}
fn command(label: &str) -> CommandMetadata {
    CommandMetadata {
        command_id: label.into(),
        command_type: label.into(),
        request_hash: label.into(),
        causation_id: None,
        correlation_id: None,
    }
}
fn temporary_root() -> Result<TempDir> {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .find(|parent| parent.join(".git").exists())
        .context("locate the repository for the short socket test root")?;
    Ok(tempfile::Builder::new()
        .prefix(".tmp-mtc-")
        .tempdir_in(repository)?)
}

struct Harness {
    server: ControlServer,
    owner: ShellOwner,
    observe: Connection,
    root: TempDir,
}

impl Harness {
    fn new(
        discovery: bool,
        secondary_workspace: bool,
        generation: &str,
        clock: Clock,
        idle: Duration,
    ) -> Result<Self> {
        let root = temporary_root()?;
        let BootstrapResult::Writable {
            mut connection,
            ownership,
            ..
        } = startup::open(root.path(), generation)?
        else {
            bail!("expected writable fixture");
        };
        let first = hierarchy_ids(2);
        hierarchy::bootstrap_window(
            &mut connection,
            &command("test-bootstrap"),
            &BootstrapWindowInput {
                window_id: WINDOW.into(),
                default_root_directory: WORKSPACE.into(),
                default_name: "Default fixture workspace".into(),
                now: NOW + 1,
            },
            &first,
            "默认",
            NOW + 1,
        )?;
        if discovery {
            for (index, title, session) in [(2, "Peer", 16), (3, "Child", 25)] {
                canvas::create_shell_sibling(
                    &mut connection,
                    &command(title),
                    &CreateShellSiblingInput {
                        window_id: WINDOW.into(),
                        scene_id: first.scene_id.clone(),
                        source_session_id: first.session_id.clone(),
                        parent_session_id: (title == "Child").then(|| first.session_id.clone()),
                        execution_context_id: None,
                        title: Some(title.into()),
                        navigation: Some("preserve".into()),
                        now: NOW + index,
                    },
                    &CanvasIds {
                        scene_id: first.scene_id.clone(),
                        root_node_id: fixture_id(session + 4),
                        secondary_node_id: fixture_id(session + 3),
                        session_id: fixture_id(session),
                        mount_id: fixture_id(session + 1),
                        relation_id: fixture_id(session + 2),
                    },
                    &CanvasMessages {
                        new_canvas: "Shell",
                        duplicate_scene_name: "duplicate scene",
                        workspace_path_invalid: "path unavailable",
                        detached_session_must_return: "detached session",
                    },
                    NOW + index,
                )?;
            }
            let mut remote = hierarchy_ids(28);
            remote.execution_context_id = fixture_id(29);
            tasks::create_task(
                &mut connection,
                &command("Remote"),
                &CreateTaskInput {
                    window_id: WINDOW.into(),
                    workspace_id: first.workspace_id.clone(),
                    title: Some("Remote".into()),
                    navigation: Some("preserve".into()),
                    now: NOW + 4,
                },
                &remote,
                "New",
                "path unavailable",
                NOW + 4,
            )?;
            connection.execute(
                "UPDATE session_canvas_memberships SET last_user_interaction_seq=100 WHERE session_id=?",
                [&first.session_id],
            )?;
        }
        if secondary_workspace {
            workspaces::create_workspace(
                &mut connection,
                &command("secondary"),
                &CreateWorkspaceInput {
                    window_id: WINDOW.into(),
                    name: "Secondary".into(),
                    root_directory: "/__fixture__/secondary".into(),
                    navigation: Some("preserve".into()),
                    now: NOW + 5,
                },
                &hierarchy_ids(40),
                "默认",
                NOW + 5,
            )?;
            connection.execute(
                "INSERT INTO session_runs (id,session_id,ordinal,runtime_generation,status,started_at)
                 VALUES (?,?,1,?,'running',?)",
                params![RUN, first.session_id, generation, NOW],
            )?;
        }
        let database_path = root.path().join("matou.sqlite");
        let observer = Connection::open_with_flags(
            &database_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let reader = Connection::open_with_flags(
            &database_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let owner = ShellOwner::new(connection, ownership, || uuid::Uuid::new_v4().to_string())?;
        let server = ControlServer::listen(root.path(), generation, reader, clock, idle)?;
        seed_screen(root.path(), 8, "previous terminal output")?;
        if discovery {
            seed_screen(root.path(), 16, "peer terminal output")?;
            seed_screen(
                root.path(),
                25,
                frozen_case("terminal-child-default")["request"]["stub_output"]
                    .as_str().unwrap(),
            )?;
        }
        if secondary_workspace {
            seed_screen(root.path(), 46, "remote workspace output")?;
        }
        Ok(Self {
            server,
            owner,
            observe: observer,
            root,
        })
    }

    fn token(&self, run: &str, scopes: &'static [&'static str], expiry: i64) -> Result<String> {
        self.server
            .state
            .tokens
            .issue(run, &fixture_id(8), scopes, expiry)
    }

    fn verify(
        &mut self,
        label: &str,
        request: Value,
        expected_id: &str,
        error: Option<&str>,
        topology_reads: usize,
        terminal_reads: usize,
    ) -> Result<Value> {
        let before = snapshot(&self.observe, self.root.path())?;
        let reads = (
            self.server.topology_reads.get(),
            self.server.resolved_reads.get(),
        );
        let path = self.server.endpoint.path.clone();
        let reply = self.drive(move || {
            let mut client = client(&path)?;
            client.write_all(&frame(&serde_json::to_vec(&request)?))?;
            receive(&mut client)
        })?;
        check_reply(&reply, expected_id, error);
        assert_eq!(
            snapshot(&self.observe, self.root.path())?,
            before,
            "{label}: row values/journal"
        );
        assert_eq!(
            self.server.topology_reads.get() - reads.0,
            topology_reads,
            "{label}: snapshot reads"
        );
        assert_eq!(
            self.server.resolved_reads.get() - reads.1,
            terminal_reads,
            "{label}: terminal spy"
        );
        assert!(
            self.owner.poll(Duration::ZERO)?.is_empty(),
            "{label}: owner events"
        );
        println!("PASS {label}");
        Ok(reply)
    }

    fn drive<T: Send>(&self, client_task: impl FnOnce() -> Result<T> + Send) -> Result<T> {
        thread::scope(|scope| {
            let worker = scope.spawn(client_task);
            while !worker.is_finished() {
                self.server.poll(&self.owner);
                thread::sleep(Duration::from_millis(1));
            }
            worker.join().unwrap()
        })
    }
}

fn screen_params(target: Value) -> Value {
    json!({ "target": target, "maxLines": 1, "maxBytes": 100 })
}

fn seed_screen(root: &Path, session: u64, text: &str) -> Result<()> {
    seed_journal(root, session, [Frame::Output {
        sequence: 1,
        data: text.as_bytes().to_vec(),
    }])
}

fn seed_journal(root: &Path, session: u64, frames: impl IntoIterator<Item = Frame>) -> Result<()> {
    let mut journal =
        SegmentJournal::open(root, &fixture_id(session), JournalOptions::default())?;
    for frame in frames {
        journal.append(&Record { timestamp: NOW, frame })?;
    }
    journal.close()
}

#[derive(Debug, PartialEq)]
struct Snapshot {
    rows: BTreeMap<String, Vec<Vec<SqlValue>>>,
    files: BTreeMap<PathBuf, Vec<u8>>,
}

fn snapshot(connection: &Connection, root: &Path) -> Result<Snapshot> {
    let mut names =
        connection.prepare("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")?;
    let tables = names
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut rows = BTreeMap::new();
    for table in tables {
        let mut query =
            connection.prepare(&format!("SELECT * FROM \"{}\"", table.replace('"', "\"\"")))?;
        let columns = query.column_count();
        let mut values = query
            .query_map([], |row| {
                (0..columns)
                    .map(|index| row.get::<_, SqlValue>(index))
                    .collect()
            })?
            .collect::<rusqlite::Result<Vec<Vec<SqlValue>>>>()?;
        values.sort_by_cached_key(|row| format!("{row:?}"));
        rows.insert(table, values);
    }
    let mut files = BTreeMap::new();
    snapshot_files(root, root, &mut files)?;
    Ok(Snapshot { rows, files })
}

fn snapshot_files(
    root: &Path,
    directory: &Path,
    files: &mut BTreeMap<PathBuf, Vec<u8>>,
) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            snapshot_files(root, &entry.path(), files)?;
        } else if ty.is_file()
            && !entry
                .file_name()
                .to_string_lossy()
                .starts_with("matou.sqlite")
        {
            files.insert(
                entry.path().strip_prefix(root)?.to_owned(),
                fs::read(entry.path())?,
            );
        }
    }
    Ok(())
}

fn client(path: &Path) -> Result<UnixStream> {
    let socket = UnixStream::connect(path)?;
    socket.set_read_timeout(Some(Duration::from_secs(3)))?;
    socket.set_write_timeout(Some(Duration::from_secs(3)))?;
    Ok(socket)
}

fn frame(body: &[u8]) -> Vec<u8> {
    let mut bytes = (body.len() as u32).to_be_bytes().to_vec();
    bytes.extend(body);
    bytes
}

fn receive(socket: &mut UnixStream) -> Result<Value> {
    let mut prefix = [0; 4];
    socket.read_exact(&mut prefix)?;
    let len = u32::from_be_bytes(prefix) as usize;
    ensure!(len > 0 && len <= MAX_FRAME_BYTES, "invalid reply length");
    let mut body = vec![0; len];
    socket.read_exact(&mut body)?;
    Ok(serde_json::from_slice(&body)?)
}

fn request(id: &str, method: &str, token: &str, params: Value) -> Value {
    json!({ "version": 1, "requestId": id, "method": method, "token": token, "params": params, "deadlineAt": NOW + 100 })
}

fn check_reply(reply: &Value, id: &str, error: Option<&str>) {
    assert_eq!(reply["version"], VERSION);
    assert_eq!(reply["requestId"], id);
    assert_eq!(reply["ok"], error.is_none());
    if let Some(code) = error {
        assert_eq!(reply["error"]["code"], code);
        assert!(!reply["error"]["message"].as_str().unwrap().is_empty());
        assert!(reply.get("result").is_none());
    } else {
        assert!(reply.get("result").is_some());
        assert!(reply.get("error").is_none());
    }
}

fn frozen_case(name: &str) -> Value {
    let data: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../tooling/native-parity/fixtures/host-control-reference.json"
    )))
    .unwrap();
    data["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["id"] == name)
        .unwrap()
        .clone()
}

#[test]
fn matches_frozen_identify_and_list_payloads() -> Result<()> {
    for (name, discovery) in [("identify-caller", false), ("list-scopes", true)] {
        let mut harness = Harness::new(discovery, false, GENERATION, fixed_now, IDLE_TIMEOUT)?;
        let token = harness.token(RUN, MANAGED_SCOPES, NOW)?;
        let case = frozen_case(name);
        let calls = case["request"]["calls"].as_array().unwrap();
        let expected = case["expected_response"]["frames"].as_array().unwrap();
        for (call, expected) in calls.iter().zip(expected) {
            let mut envelope = call["envelope"].clone();
            envelope["token"] = json!(&token);
            let id = envelope["requestId"].as_str().unwrap().to_owned();
            let actual = harness.verify(&id, envelope, &id, None, 1, 0)?;
            assert_eq!(&actual, expected);
        }
    }
    Ok(())
}

#[test]
fn wire_rejections_precede_topology_and_match_public_envelope() -> Result<()> {
    let mut harness = Harness::new(false, false, GENERATION, fixed_now, IDLE_TIMEOUT)?;
    let token = harness.token("other-run", MANAGED_SCOPES, NOW)?;
    let managed_canary = format!("canary-{token}");
    let expired = harness.token("expired", MANAGED_SCOPES, NOW - 1)?;
    let weak = harness.token("weak", &["host.identify"], NOW)?;
    let unsupported = harness.token("unused", &["structure.create.workspace"], NOW)?;
    let revoked1 = harness.token(RUN, MANAGED_SCOPES, NOW)?;
    let revoked2 = harness.token(RUN, MANAGED_SCOPES, NOW)?;
    harness.server.runs().revoke(RUN);
    for (id, method, token, code) in [
        (
            "unknown-token",
            "host.identify",
            managed_canary.as_str(),
            "CAPABILITY_DENIED",
        ),
        (
            "token-expired",
            "host.identify",
            expired.as_str(),
            "CAPABILITY_DENIED",
        ),
        (
            "known-out-of-scope",
            "host.list",
            weak.as_str(),
            "CAPABILITY_DENIED",
        ),
        (
            "production-terminal-write-disabled",
            "terminal.send-text",
            token.as_str(),
            "CAPABILITY_DENIED",
        ),
        (
            "run-revoked-first",
            "host.identify",
            revoked1.as_str(),
            "CAPABILITY_DENIED",
        ),
        (
            "run-revoked-second",
            "host.list",
            revoked2.as_str(),
            "CAPABILITY_DENIED",
        ),
        (
            "authorized-disabled",
            "structure.create.workspace",
            unsupported.as_str(),
            "UNSUPPORTED",
        ),
    ] {
        let response = harness.verify(
            id,
            request(id, method, token, json!({})),
            id,
            Some(code),
            0,
            0,
        )?;
        assert!(!response.to_string().contains(token));
    }
    for (name, code) in [
        ("wire-invalid-version", INVALID),
        ("renderer-rpc-name-unsupported", UNSUPPORTED),
        ("wire-missing-token", INVALID),
    ] {
        let case = frozen_case(name);
        let reply = harness.verify(
            name,
            case["request"]["calls"][0]["envelope"].clone(),
            "unknown",
            Some(code),
            0,
            0,
        )?;
        assert_eq!(reply, case["expected_response"]["frames"][0]);
    }
    let mut bad = request("unused", "host.identify", "", json!({}));
    bad["version"] = json!(2);
    bad["method"] = json!("unknown.name");
    harness.verify(
        "version-before-method",
        bad.clone(),
        "unknown",
        Some(INVALID),
        0,
        0,
    )?;
    bad["version"] = json!(1);
    harness.verify(
        "unknown-method-before-token-fields",
        bad,
        "unknown",
        Some(UNSUPPORTED),
        0,
        0,
    )?;

    let case = frozen_case("wire-extra-and-required-fields");
    for (call, expected) in case["request"]["calls"]
        .as_array()
        .unwrap()
        .iter()
        .zip(case["expected_response"]["frames"].as_array().unwrap())
    {
        let mut envelope = call["envelope"].clone();
        envelope["token"] = json!(token);
        let id = expected["requestId"].as_str().unwrap();
        let error = expected["error"]["code"].as_str();
        let label = format!(
            "extra-required-{}",
            call["envelope"]["requestId"]
                .as_str()
                .unwrap_or("missing-id")
        );
        let response =
            harness.verify(&label, envelope, id, error, usize::from(error.is_none()), 0)?;
        assert_eq!(&response, expected);
    }
    let mut boundary = request("deadline-equal", "host.identify", &token, json!({}));
    boundary["deadlineAt"] = json!(NOW);
    harness.verify(
        "deadline-equal",
        boundary.clone(),
        "deadline-equal",
        None,
        1,
        0,
    )?;
    boundary["deadlineAt"] = json!(NOW - 1);
    boundary["token"] = json!(managed_canary);
    harness.verify(
        "timeout-before-token",
        boundary,
        "deadline-equal",
        Some(TIMEOUT),
        0,
        0,
    )?;
    for (id, params) in [
        ("missing-params", Value::Null),
        ("invalid-scope", json!({ "scope": null })),
    ] {
        harness.verify(
            id,
            request(id, "host.list", &token, params),
            id,
            Some(INVALID),
            0,
            0,
        )?;
    }
    let mut missing_params = request("omitted-params", "host.list", &token, json!({}));
    missing_params.as_object_mut().unwrap().remove("params");
    harness.verify(
        "omitted-params",
        missing_params,
        "omitted-params",
        Some(INVALID),
        0,
        0,
    )?;
    Ok(())
}

#[test]
fn selects_targets_with_a_single_owner_snapshot() -> Result<()> {
    let mut harness = Harness::new(true, true, GENERATION, fixed_now, IDLE_TIMEOUT)?;
    let token = harness.token(RUN, &[READ, "host.list"], NOW)?;
    let list = harness.verify(
        "all-across-workspaces",
        request("cross", "host.list", &token, json!({"scope":"all"})),
        "cross",
        None,
        1,
        0,
    )?;
    assert!(
        list["result"]["targets"]
            .as_array()
            .unwrap()
            .iter()
            .any(|target| target["workspaceId"] == fixture_id(40))
    );
    let all = list["result"]["projectionRevision"].as_str().unwrap();
    let current = harness.verify(
        "level-with-child-and-peer",
        request("current", "host.list", &token, json!({})),
        "current",
        None,
        1,
        0,
    )?;
    let level = current["result"]["projectionRevision"].as_str().unwrap();
    let remote = fixture_id(46);
    for (id, target, expected) in [
        (
            "explicit-cross-workspace",
            json!({"sessionId": remote}),
            remote.clone(),
        ),
        (
            "ref-cross-workspace",
            json!({"kind":"ref","ref":session_ref(&remote),"projectionRevision":all}),
            remote,
        ),
        (
            "legacy-ref-current-level",
            json!({"ref":session_ref(&fixture_id(16)),"projectionRevision":level}),
            fixture_id(16),
        ),
        (
            "relative-right",
            json!({"kind":"relative","direction":"right"}),
            fixture_id(16),
        ),
        (
            "sibling-first",
            json!({"kind":"sibling","ordinal":1,"projectionRevision":level}),
            fixture_id(8),
        ),
        (
            "sibling-second",
            json!({"kind":"sibling","ordinal":2,"projectionRevision":level}),
            fixture_id(16),
        ),
        (
            "child-default-first",
            json!({"kind":"relation","relation":"child"}),
            fixture_id(25),
        ),
        ("self", json!({"kind":"self"}), fixture_id(8)),
    ] {
        let response = harness.verify(
            id,
            request(id, READ, &token, screen_params(target)),
            id,
            None,
            1,
            1,
        )?;
        assert_eq!(
            response["result"]["text"],
            match expected.as_str() {
                id if id == fixture_id(8) => "previous terminal output",
                id if id == fixture_id(16) => "peer terminal output",
                id if id == fixture_id(25) => "fixture child output",
                _ => "remote workspace output",
            },
        );
    }
    let child = harness
        .server
        .state
        .tokens
        .issue("child-run", &fixture_id(25), &[READ], NOW)?;
    let parent = harness.verify(
        "relative-parent",
        request(
            "parent",
            READ,
            &child,
            screen_params(json!({"kind":"relation","relation":"parent"})),
        ),
        "parent",
        None,
        1,
        1,
    )?;
    assert_eq!(parent["result"]["text"], "previous terminal output");
    for (id, params) in [
        ("missing-target", json!({})),
        (
            "strict-params-method",
            json!({"method":READ,"target":{"kind":"self"}}),
        ),
        (
            "strict-params-extra",
            json!({"extra":true,"target":{"kind":"self"}}),
        ),
    ] {
        harness.verify(
            id,
            request(id, READ, &token, params),
            id,
            Some(INVALID),
            0,
            0,
        )?;
    }
    for (id, target, error, reads) in [
        (
            "missing-id",
            json!({"kind":"session","sessionId":"absent"}),
            "TARGET_NOT_FOUND",
            1,
        ),
        (
            "missing-ref",
            json!({"kind":"ref","ref":"session:absent","projectionRevision":all}),
            "TARGET_NOT_FOUND",
            1,
        ),
        (
            "stale-ref-precedes-missing",
            json!({"kind":"ref","ref":"session:absent","projectionRevision":"stale"}),
            "STALE_PROJECTION",
            1,
        ),
        (
            "stale-sibling-precedes-range",
            json!({"kind":"sibling","ordinal":10000,"projectionRevision":"stale"}),
            "STALE_PROJECTION",
            1,
        ),
        (
            "sibling-out-of-range",
            json!({"kind":"sibling","ordinal":3,"projectionRevision":level}),
            "TARGET_NOT_FOUND",
            1,
        ),
        (
            "left-out-of-range",
            json!({"kind":"relative","direction":"left"}),
            "TARGET_NOT_FOUND",
            1,
        ),
        (
            "parent-missing",
            json!({"kind":"relation","relation":"parent"}),
            "TARGET_NOT_FOUND",
            1,
        ),
        (
            "child-out-of-range",
            json!({"kind":"relation","relation":"child","ordinal":2}),
            "TARGET_NOT_FOUND",
            1,
        ),
        (
            "zero-ordinal",
            json!({"kind":"sibling","ordinal":0,"projectionRevision":"stale"}),
            INVALID,
            0,
        ),
        (
            "fractional-ordinal",
            json!({"kind":"sibling","ordinal":1.5,"projectionRevision":level}),
            INVALID,
            0,
        ),
        (
            "ordinal-above-limit",
            json!({"kind":"sibling","ordinal":10001,"projectionRevision":level}),
            INVALID,
            0,
        ),
        (
            "extra-selector-field",
            json!({"kind":"sibling","ordinal":1,"projectionRevision":level,"session":"extra"}),
            INVALID,
            0,
        ),
        (
            "missing-revision",
            json!({"kind":"sibling","ordinal":1}),
            INVALID,
            0,
        ),
        (
            "missing-selector-fields",
            json!({"kind":"session"}),
            INVALID,
            0,
        ),
    ] {
        harness.verify(
            id,
            request(id, READ, &token, screen_params(target)),
            id,
            Some(error),
            reads,
            0,
        )?;
    }
    harness.server.projection_aliases = Some(|targets| {
        for target in targets {
            if [fixture_id(8), fixture_id(25)].contains(&target.session_id) {
                target.reference = "legacy:shared".into();
            }
        }
    });
    let alias_list = harness.verify(
        "legacy-aliased-list",
        request("alias-list", "host.list", &token, json!({"scope":"all"})),
        "alias-list",
        None,
        1,
        0,
    )?;
    let ambiguous = harness.verify("ambiguous-ref", request("ambiguous", READ, &token, screen_params(json!({
        "kind":"ref","ref":"legacy:shared","projectionRevision":alias_list["result"]["projectionRevision"]
    }))), "ambiguous", Some("AMBIGUOUS_TARGET"), 1, 0)?;
    let fixture = frozen_case("selector-legacy-ref-ambiguity");
    assert_eq!(
        ambiguous["error"]["details"],
        fixture["expected_response"]["frames"][0]["error"]["details"]
    );
    Ok(())
}

#[test]
fn isolates_profiles_and_runtime_generations() -> Result<()> {
    let first = Harness::new(false, false, GENERATION, fixed_now, IDLE_TIMEOUT)?;
    let mut other = Harness::new(false, false, GENERATION, fixed_now, IDLE_TIMEOUT)?;
    let token = first.token(RUN, MANAGED_SCOPES, NOW)?;
    assert_ne!(first.server.endpoint.path, other.server.endpoint.path);
    let other_token = other.token(RUN, MANAGED_SCOPES, NOW)?;
    other.verify(
        "same-workspace-other-root",
        request("cross-root", "host.identify", &token, json!({})),
        "cross-root",
        Some("CAPABILITY_DENIED"),
        0,
        0,
    )?;
    other.verify(
        "same-workspace-its-own-token",
        request("local", "host.identify", &other_token, json!({})),
        "local",
        None,
        1,
        0,
    )?;
    // Even if a stale digest were carried into the new process, the generation guard rejects it.
    other.server.state.tokens.records.lock().unwrap()[0].runtime_generation =
        "old-generation".into();
    other.verify(
        "mismatched-record-generation",
        request("wrong-gen", "host.identify", &other_token, json!({})),
        "wrong-gen",
        Some("CAPABILITY_DENIED"),
        0,
        0,
    )?;

    let Harness {
        server,
        owner,
        root,
        observe,
    } = first;
    drop(server);
    drop(owner);
    drop(observe);
    let new_generation = "next-generation";
    let BootstrapResult::Writable {
        connection,
        ownership,
        ..
    } = startup::open(root.path(), new_generation)?
    else {
        bail!("expected reopened owner");
    };
    let owner = ShellOwner::new(connection, ownership, || uuid::Uuid::new_v4().to_string())?;
    let observe = Connection::open_with_flags(
        root.path().join("matou.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let reader = Connection::open_with_flags(
        root.path().join("matou.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let server =
        ControlServer::listen(root.path(), new_generation, reader, fixed_now, IDLE_TIMEOUT)?;
    let mut restarted = Harness {
        server,
        owner,
        observe,
        root,
    };
    restarted.verify(
        "same-root-after-restart",
        request("previous-run", "host.identify", &token, json!({})),
        "previous-run",
        Some("CAPABILITY_DENIED"),
        0,
        0,
    )?;
    Ok(())
}

#[test]
fn fragmented_continuous_invalid_and_large_frames() -> Result<()> {
    let mut harness = Harness::new(false, false, GENERATION, fixed_now, IDLE_TIMEOUT)?;
    let token = harness.token(RUN, MANAGED_SCOPES, NOW)?;
    let before = snapshot(&harness.observe, harness.root.path())?;
    let path = harness.server.endpoint.path.clone();
    harness.drive(move || {
        let mut socket = client(&path)?;
        let first = frame(&serde_json::to_vec(&request(
            "fragmented",
            "host.identify",
            &token,
            json!({}),
        ))?);
        let second = frame(&serde_json::to_vec(&request(
            "pipelined",
            "host.list",
            &token,
            json!({}),
        ))?);
        socket.write_all(&first[..2])?;
        thread::sleep(IO_POLL);
        socket.write_all(&first[2..17])?;
        thread::sleep(IO_POLL);
        let tail = [
            first[17..].to_vec(),
            second,
            frame(b"not json"),
            frame(b"{\"bad\":\"\xff\"}"),
        ]
        .concat();
        socket.write_all(&tail)?;
        check_reply(&receive(&mut socket)?, "fragmented", None);
        check_reply(&receive(&mut socket)?, "pipelined", None);
        let malformed = receive(&mut socket)?;
        check_reply(&malformed, "unknown", Some(INVALID));
        let expected = frozen_case("wire-invalid-json");
        assert_eq!(malformed, expected["expected_response"]["frames"][0]);
        check_reply(&receive(&mut socket)?, "unknown", Some(INVALID));

        let encoded = serde_json::to_vec(&request(
            "frame-at-limit",
            "host.identify",
            &token,
            json!({}),
        ))?;
        let mut padded = encoded;
        padded.resize(MAX_FRAME_BYTES, b' ');
        socket.write_all(&frame(&padded))?;
        check_reply(&receive(&mut socket)?, "frame-at-limit", None);
        socket.write_all(&[0; 4])?;
        check_reply(&receive(&mut socket)?, "unknown", Some(INVALID));
        let mut oversized = client(&path)?;
        oversized.write_all(&(MAX_FRAME_BYTES as u32 + 1).to_be_bytes())?;
        let fault = receive(&mut oversized)?;
        check_reply(&fault, "unknown", Some(INVALID));
        assert_eq!(
            fault["error"]["message"],
            "control frame exceeds size limit"
        );
        assert_eq!(oversized.read(&mut [0])?, 0);
        Ok(())
    })?;
    assert_eq!(harness.server.topology_reads.get(), 3);
    assert_eq!(harness.server.resolved_reads.get(), 0);
    assert_eq!(snapshot(&harness.observe, harness.root.path())?, before);
    assert!(harness.owner.poll(Duration::ZERO)?.is_empty());
    println!(
        "PASS fragmented-prefix-and-body, continuous-frames, invalid-json, invalid-utf8, exact-max-frame, zero-frame, oversized-frame"
    );
    Ok(())
}

#[test]
fn endpoint_permissions_conflicts_rebind_and_shutdown() -> Result<()> {
    let root = temporary_root()?;
    let path = root.path().join("control/runtime.sock");
    let bind = |root: &Path| {
        ControlServer::listen(
            root,
            GENERATION,
            Connection::open_in_memory()?,
            fixed_now,
            IDLE_TIMEOUT,
        )
    };
    fs::create_dir(root.path().join("control"))?;
    fs::write(&path, b"unrelated")?;
    let error = bind(root.path()).err().context("non-socket must fail")?;
    assert!(format!("{error:#}").contains("not an owned socket"));
    assert!(format!("{error:#}").contains(path.to_str().unwrap()));
    assert_eq!(fs::read(&path)?, b"unrelated");

    fs::remove_file(&path)?;
    let actual = root.path().join("elsewhere");
    fs::write(&actual, "keep")?;
    std::os::unix::fs::symlink(&actual, &path)?;
    assert!(bind(root.path()).is_err());
    assert!(fs::symlink_metadata(&path)?.is_symlink());
    assert_eq!(fs::read_to_string(&actual)?, "keep");
    fs::remove_file(&path)?;
    let directory = root.path().join("control");
    fs::remove_dir(&directory)?;
    std::os::unix::fs::symlink(root.path(), &directory)?;
    let error = bind(root.path())
        .err()
        .context("control directory symlink must fail")?;
    assert!(format!("{error:#}").contains("not an owned directory"));
    fs::remove_file(&directory)?;
    fs::create_dir(&directory)?;

    let live = UnixListener::bind(&path)?;
    let live_id = socket_identity(&fs::symlink_metadata(&path)?);
    let error = bind(root.path()).err().context("live bind must fail")?;
    assert!(format!("{error:#}").contains("already in use"));
    assert_eq!(socket_identity(&fs::symlink_metadata(&path)?), live_id);
    drop(live); // An unclean stop keeps a socket inode on disk.
    assert!(fs::symlink_metadata(&path)?.file_type().is_socket());
    let server = bind(root.path())?;
    assert_eq!(
        fs::metadata(path.parent().unwrap())?.permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o600);
    let token = server
        .state
        .tokens
        .issue(RUN, &fixture_id(8), MANAGED_SCOPES, NOW)?;
    let mut idle = client(&path)?;
    let mut queued = client(&path)?;
    queued.write_all(&frame(&serde_json::to_vec(&request(
        "during-shutdown",
        "host.identify",
        &token,
        json!({}),
    ))?))?;
    let pending = server
        .pending
        .as_ref()
        .unwrap()
        .recv_timeout(Duration::from_secs(2))?;
    drop(pending);
    drop(server);
    for socket in [&mut idle, &mut queued] {
        let read = socket.read(&mut [0]);
        match read {
            Ok(0) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::ConnectionReset | io::ErrorKind::BrokenPipe
                ) => {}
            other => panic!("expected closed socket, got {other:?}"),
        }
    }
    assert!(!path.exists());
    drop(bind(root.path())?);
    assert!(!path.exists());

    let long = root.path().join("x".repeat(120));
    fs::create_dir(&long)?;
    let error = bind(&long).err().context("overlong endpoint must fail")?;
    assert!(format!("{error:#}").contains("overlong Unix socket path"));
    assert!(format!("{error:#}").contains(long.to_str().unwrap()));
    assert!(!long.join("control").exists());
    println!(
        "PASS permissions-0700-0600, reject-regular-file, reject-socket-symlink, reject-directory-symlink, preserve-live-socket, recover-stale-socket, close-connections, rebind-after-stop, diagnose-long-path"
    );
    Ok(())
}

#[test]
fn bounded_queue_rechecks_revocation_and_deadline_on_owner() -> Result<()> {
    QUEUE_CLOCK.store(NOW, Ordering::SeqCst);
    let mut harness = Harness::new(false, false, GENERATION, queue_now, IDLE_TIMEOUT)?;
    let before = snapshot(&harness.observe, harness.root.path())?;
    let token = harness.token(RUN, MANAGED_SCOPES, NOW + 1000)?;
    let path = &harness.server.endpoint.path;
    let mut clients = Vec::new();
    for index in 0..OWNER_QUEUE_CAPACITY {
        let mut socket = client(path)?;
        let id = format!("queued-{index}");
        socket.write_all(&frame(&serde_json::to_vec(&request(
            &id,
            "host.identify",
            &token,
            json!({}),
        ))?))?;
        clients.push((socket, id));
    }
    // The owner is deliberately paused. Once a request gets the full-queue fault the others
    // cannot be processed until the owner runs.
    thread::sleep(Duration::from_millis(100));
    let mut overflow = client(path)?;
    overflow.write_all(&frame(&serde_json::to_vec(&request(
        "overflow",
        "host.identify",
        &token,
        json!({}),
    ))?))?;
    check_reply(&receive(&mut overflow)?, "overflow", Some(TIMEOUT));
    harness.server.runs().revoke(RUN);
    for _ in 0..OWNER_QUEUE_CAPACITY.div_ceil(OWNER_BATCH) {
        harness.server.poll(&harness.owner);
    }
    for (mut socket, id) in clients {
        check_reply(&receive(&mut socket)?, &id, Some("CAPABILITY_DENIED"));
    }
    let token = harness.token("live", MANAGED_SCOPES, NOW + 1000)?;
    let mut socket = client(path)?;
    socket.write_all(&frame(&serde_json::to_vec(&request(
        "timeout-while-queued",
        "host.list",
        &token,
        json!({}),
    ))?))?;
    let pending = harness
        .server
        .pending
        .as_ref()
        .unwrap()
        .recv_timeout(Duration::from_secs(2))?;
    QUEUE_CLOCK.store(NOW + 101, Ordering::SeqCst);
    let result = harness.server.dispatch(&pending, &harness.owner);
    assert_eq!(result.as_ref().err().unwrap().code, TIMEOUT);
    let _ = pending.reply.send(result);
    check_reply(
        &receive(&mut socket)?,
        "timeout-while-queued",
        Some(TIMEOUT),
    );
    assert_eq!(snapshot(&harness.observe, harness.root.path())?, before);
    assert_eq!(harness.server.topology_reads.get(), 0);
    assert_eq!(harness.server.resolved_reads.get(), 0);
    assert!(harness.owner.poll(Duration::ZERO)?.is_empty());
    println!("PASS bounded-queue, queued-run-revocation, queued-deadline-recheck");
    Ok(())
}

#[test]
fn slow_clients_reclaim_slots_without_blocking_other_connections() -> Result<()> {
    let mut harness = Harness::new(
        false,
        false,
        GENERATION,
        fixed_now,
        Duration::from_millis(500),
    )?;
    let before = snapshot(&harness.observe, harness.root.path())?;
    let token = harness.token(RUN, MANAGED_SCOPES, NOW)?;
    let path = harness.server.endpoint.path.clone();
    harness.drive(move || {
        let mut slow = client(&path)?;
        slow.write_all(&[0, 0])?;
        thread::sleep(IO_POLL);
        let mut fast = client(&path)?;
        fast.write_all(&frame(&serde_json::to_vec(&request(
            "fast",
            "host.identify",
            &token,
            json!({}),
        ))?))?;
        check_reply(&receive(&mut fast)?, "fast", None);
        drop(fast);
        thread::sleep(IO_POLL * 2);
        let mut held = (1..MAX_CONNECTIONS)
            .map(|_| client(&path))
            .collect::<Result<Vec<_>>>()?;
        let mut overflow = client(&path)?;
        assert_eq!(overflow.read(&mut [0])?, 0);
        for socket in &mut held {
            assert_eq!(socket.read(&mut [0])?, 0);
        }
        assert_eq!(slow.read(&mut [0])?, 0);
        let mut next = client(&path)?;
        next.write_all(&frame(&serde_json::to_vec(&request(
            "after-idle",
            "host.identify",
            &token,
            json!({}),
        ))?))?;
        check_reply(&receive(&mut next)?, "after-idle", None);
        Ok(())
    })?;
    assert_eq!(snapshot(&harness.observe, harness.root.path())?, before);
    assert_eq!(harness.server.topology_reads.get(), 2);
    assert_eq!(harness.server.resolved_reads.get(), 0);
    assert!(harness.owner.poll(Duration::ZERO)?.is_empty());
    println!("PASS parallel-fast-while-partial, connection-limit, idle-reclaim");
    Ok(())
}

#[test]
fn runtime_host_connects_managed_shell_and_revokes_exited_run() -> Result<()> {
    let root = temporary_root()?;
    let workspace = root.path().join("project");
    let home = root.path().join("home");
    let data = root.path().join("data");
    fs::create_dir(&workspace)?;
    fs::create_dir(&home)?;
    fs::write(
        home.join(".zshrc"),
        r#"
umask 077
printf '%s\n' "$MATOU_CONTROL_ENDPOINT" "$MATOU_CONTROL_TOKEN" "$MATOU_CONTROL_PROTOCOL" "$MATOU_CONTROL_CALLER_SESSION" "$MATOU_CONTROL_CALLER_RUN" > "$HOME/capture.tmp"
/bin/mv "$HOME/capture.tmp" "$HOME/capture"
while [[ ! -f "$HOME/exit" ]]; do /bin/sleep 0.02; done
exit 0
"#,
    )?;
    let mut host = RuntimeHost::start(LaunchOptions {
        data_root: data.clone(),
        workspace,
        shell_home: home.clone(),
        shell: "/bin/zsh".into(),
    })?;
    let until = Instant::now() + Duration::from_secs(8);
    let capture = loop {
        ensure!(Instant::now() < until, "managed Shell failed to start");
        if let Ok(contents) = fs::read_to_string(home.join("capture")) {
            break contents;
        }
        while let Ok(event) = host.events.try_recv() {
            ensure!(
                !matches!(event, Event::StartupBlocked(_) | Event::Diagnostic(_)),
                "unexpected runtime diagnostic"
            );
        }
        thread::sleep(Duration::from_millis(10));
    };
    let fields: Vec<_> = capture.lines().collect();
    assert_eq!(fields.len(), 5);
    let [endpoint, token, version, session_id, run_id] = fields.as_slice() else {
        unreachable!()
    };
    assert_eq!(Path::new(endpoint), data.join("control/runtime.sock"));
    assert_eq!(*version, "1");
    assert_eq!(URL_SAFE_NO_PAD.decode(token)?.len(), 32);
    assert!(!session_id.is_empty() && !run_id.is_empty());
    fs::remove_file(home.join("capture"))?;
    let observe = Connection::open_with_flags(
        data.join("matou.sqlite"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?;
    let mut socket = client(Path::new(endpoint))?;
    let before = snapshot(&observe, &data)?;
    let identify = |socket: &mut UnixStream| -> Result<Value> {
        let mut input = request("managed", "host.identify", token, json!({}));
        input["deadlineAt"] = json!(now_ms() + 3_000);
        socket.write_all(&frame(&serde_json::to_vec(&input)?))?;
        receive(socket)
    };
    let reply = identify(&mut socket)?;
    check_reply(&reply, "managed", None);
    assert_eq!(
        reply["result"]["caller"],
        json!({"sessionId":session_id, "runId":run_id})
    );
    assert_eq!(snapshot(&observe, &data)?, before);

    for (method, params) in [
        (READ, screen_params(json!({"kind":"self"}))),
        ("terminal.read-history", screen_params(json!({"kind":"self"}))),
        ("terminal.read-commands", json!({"target":{"kind":"self"},"limit":1})),
        ("terminal.send-text", json!({"target":{"kind":"self"},"text":"should never arrive"})),
    ] {
        let mut input = request("managed-read", method, token, params);
        input["deadlineAt"] = json!(now_ms() + 3_000);
        socket.write_all(&frame(&serde_json::to_vec(&input)?))?;
        let reply = receive(&mut socket)?;
        let expected = if method == "terminal.send-text" { Some("CAPABILITY_DENIED") } else { None };
        check_reply(&reply, "managed-read", expected);
    }
    assert_eq!(snapshot(&observe, &data)?, before);

    fs::write(home.join("exit"), b"")?;
    let until = Instant::now() + Duration::from_secs(8);
    loop {
        ensure!(Instant::now() < until, "managed run did not exit");
        let exited: bool = observe.query_row(
            "SELECT status='exited' FROM session_runs WHERE id=?",
            [run_id],
            |row| row.get(0),
        )?;
        if exited {
            break;
        }
        while host.events.try_recv().is_ok() {}
        thread::sleep(Duration::from_millis(10));
    }
    let mut socket = client(Path::new(endpoint))?;
    let after_exit = snapshot(&observe, &data)?;
    let revoked = identify(&mut socket)?;
    check_reply(&revoked, "managed", Some("CAPABILITY_DENIED"));
    assert_eq!(snapshot(&observe, &data)?, after_exit);
    host.shutdown()?;
    assert!(!Path::new(endpoint).exists());
    println!(
        "PASS real-runtime-managed-shell-environment, real-run-exit-revocation, runtime-owner-shutdown-cleanup"
    );
    Ok(())
}

#[test]
fn spawn_clears_inherited_control_markers_even_when_control_is_unavailable() -> Result<()> {
    let root = temporary_root()?;
    let mut launch = ShellLaunch {
        profile: LaunchProfile::Shell,
        executable: "/bin/sh".into(),
        argv: vec![],
        cwd: root.path().into(),
        data_root: root.path().into(),
        inherited_env: BTreeMap::from([("HOME".into(), root.path().into())]),
        env: BTreeMap::new(),
        cols: 80,
        rows: 24,
    };
    for key in CONTROL_ENV {
        launch.inherited_env.insert(key.into(), "inherited".into());
        launch.env.insert(key.into(), "explicit".into());
    }
    clear_control_environment(&mut launch);
    let projected = matou_native_core::pty::prepare_shell_environment(&launch)?;
    for key in CONTROL_ENV {
        assert!(!projected.contains_key(OsStr::new(key)));
    }
    assert!(projected.contains_key(OsStr::new("HOME")));
    println!("PASS inherited-environment-scrub");
    Ok(())
}

fn await_runtime_event(
    host: &RuntimeHost,
    matches_event: impl Fn(&Event) -> bool,
) -> Result<Event> {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        ensure!(Instant::now() < deadline, "expected runtime startup event");
        match host.events.try_recv() {
            Ok(event) if matches_event(&event) => return Ok(event),
            Ok(Event::Diagnostic(message)) => bail!("runtime diagnostic: {message}"),
            Ok(Event::DatabaseRecoveryResult(result)) if result.error.is_some() => {
                let error = result.error.unwrap();
                bail!(
                    "runtime recovery failed ({}): {}",
                    error.code,
                    error.message
                );
            }
            Ok(Event::Closed) => bail!("runtime closed before expected event"),
            Ok(_) => {}
            Err(async_channel::TryRecvError::Empty) => thread::sleep(Duration::from_millis(5)),
            Err(error) => return Err(error.into()),
        }
    }
}

#[test]
fn readonly_and_in_process_recovery_never_bind() -> Result<()> {
    let readonly = temporary_root()?;
    let BootstrapResult::Writable {
        connection,
        ownership,
        ..
    } = startup::open(readonly.path(), "seed")?
    else {
        bail!("seed startup must be writable");
    };
    connection.execute(
        "INSERT INTO schema_migrations (version,name,checksum,applied_at) VALUES (999,'future','future',1)",
        [],
    )?;
    drop(connection);
    drop(ownership);
    let launch = |root: &Path| LaunchOptions {
        data_root: root.into(),
        workspace: root.join("project"),
        shell_home: root.join("home"),
        shell: "/bin/zsh".into(),
    };
    let mut readonly_host = RuntimeHost::start(launch(readonly.path()))?;
    await_runtime_event(&readonly_host, |event| {
        matches!(event, Event::ReadOnlyReady { .. })
    })?;
    assert!(!readonly.path().join("control").exists());
    readonly_host.shutdown()?;

    let recovery = temporary_root()?;
    let path = recovery.path().join("matou.sqlite");
    let db = Connection::open(&path)?;
    db.execute_batch("PRAGMA page_size=4096; CREATE TABLE fixture (value TEXT);")?;
    drop(db);
    let healthy = fs::read(&path)?;
    let mut damaged = healthy.clone();
    damaged[28..32].copy_from_slice(&3u32.to_be_bytes());
    damaged.resize(3 * 4096, 0);
    fs::write(&path, damaged)?;

    let mut recovering = RuntimeHost::start(launch(recovery.path()))?;
    let Event::DatabaseRecoveryRequired(details) = await_runtime_event(&recovering, |event| {
        matches!(event, Event::DatabaseRecoveryRequired(_))
    })?
    else {
        unreachable!()
    };
    assert!(!recovery.path().join("control").exists());
    fs::write(&path, healthy)?;
    recovering
        .commands
        .send(Command::DatabaseRecovery {
            request_id: database_recovery_request_id(),
            action: DatabaseRecoveryAction::RetryOpen {
                expected_recovery_id: details.recovery_id,
            },
        })
        .map_err(|_| anyhow::anyhow!("send recovery retry to runtime"))?;
    await_runtime_event(&recovering, |event| matches!(event, Event::Ready { .. }))?;
    assert!(!recovery.path().join("control").exists());
    recovering.shutdown()?;
    println!(
        "PASS runtime-readonly-no-endpoint, runtime-recovery-waiting-no-endpoint, same-startup-recovery-retry-no-endpoint"
    );
    Ok(())
}

#[test]
fn read_only_terminal_wire_contract() -> Result<()> {
    let mut harness = Harness::new(true, true, GENERATION, fixed_now, IDLE_TIMEOUT)?;
    let token = harness.token(RUN, MANAGED_SCOPES, NOW)?;
    for name in ["terminal-child-default", "stale-terminal-sibling"] {
        let case = frozen_case(name);
        let mut envelope = case["request"]["calls"][0]["envelope"].clone();
        envelope["token"] = json!(token);
        let expected = &case["expected_response"]["frames"][0];
        let id = envelope["requestId"].as_str().unwrap().to_owned();
        let code = expected["error"]["code"].as_str();
        let mut reply = harness.verify(name, envelope, &id, code, 1, usize::from(code.is_none()))?;
        if code.is_none() {
            // The frozen fixture stubs the backend's result down to { text }.
            let result = reply["result"].as_object_mut().unwrap();
            assert_eq!(result.remove("cols"), Some(json!(80)));
            assert_eq!(result.remove("rows"), Some(json!(24)));
            assert_eq!(result.remove("truncated"), Some(json!(false)));
            assert_eq!(result.remove("source"), Some(json!("screen")));
        }
        assert_eq!(&reply, expected);
    }

    let session = 46;
    seed_journal(
        harness.root.path(), session, [
            Frame::Resize { sequence: 2, cols: 40, rows: 8 },
            Frame::Reset { sequence: 3, screen_epoch: 1 },
            Frame::Output { sequence: 4, data: b"first\r\n".to_vec() },
            Frame::Output { sequence: 5, data: b"terminal\r\n".to_vec() },
            Frame::Output { sequence: 6, data: b"\x1b[31m".to_vec() },
            Frame::Output { sequence: 7, data: "尾🙂".as_bytes()[..2].to_vec() },
            Frame::Output { sequence: 8, data: "尾🙂".as_bytes()[2..].to_vec() },
        ],
    )?;
    let index = harness.root.path().join("journal").join(fixture_id(session)).join("range-index.json");
    if index.exists() {
        fs::remove_file(&index)?;
    }
    let write = Connection::open(harness.root.path().join("matou.sqlite"))?;
    write.execute(
        "INSERT INTO terminal_commands VALUES
         ('earlier',?1,4,5,NULL,NULL,NULL,NULL,10,11),
         ('latest',?1,7,7,8,'printf tail','/project',0,12,13),
         ('other',?2,100,NULL,NULL,NULL,NULL,NULL,14,14)",
        [fixture_id(session), fixture_id(8)],
    )?;
    drop(write);
    let target = json!({"sessionId":fixture_id(session)});
    let last = json!({
        "commandId":"latest", "sessionId":fixture_id(session),
        "startedSequence":7,"executedSequence":7,"endedSequence":8,
        "commandText":"printf tail","cwd":"/project","exitCode":0,
        "createdAt":12,"updatedAt":13
    });
    for (id, method, params, expected) in [
        ("grid-and-line-tail", READ,
         json!({"maxLines":2,"maxBytes":100}),
         json!({"text":"terminal\n尾🙂","cols":40,"rows":8,"truncated":true,"source":"screen"})),
        ("grid-multibyte-tail", READ,
         json!({"maxLines":5000,"maxBytes":6}),
         json!({"text":"🙂","cols":40,"rows":8,"truncated":true,"source":"screen"})),
        ("history-range", "terminal.read-history",
         json!({"fromSequence":5,"throughSequence":8,"maxLines":2,"maxBytes":12}),
         json!({"text":"\x1b[31m尾🙂","truncated":true,"firstSequence":5,"lastSequence":8,"source":"journal"})),
        ("history-non-output-cursors", "terminal.read-history",
         json!({"fromSequence":2,"throughSequence":3,"maxLines":1,"maxBytes":1}),
         json!({"text":"","truncated":false,"firstSequence":2,"lastSequence":3,"source":"journal"})),
        ("history-empty-range", "terminal.read-history",
         json!({"fromSequence":9,"maxLines":50,"maxBytes":100}),
         json!({"text":"","truncated":false,"firstSequence":0,"lastSequence":0,"source":"journal"})),
        ("history-line-limit", "terminal.read-history",
         json!({"fromSequence":4,"throughSequence":5,"maxLines":1,"maxBytes":100}),
         json!({"text":"","truncated":true,"firstSequence":4,"lastSequence":5,"source":"journal"})),
        ("commands-omit-unavailable-fields", "terminal.read-commands",
         json!({"limit":1000}),
         json!([{"commandId":"earlier","sessionId":fixture_id(session),
                  "startedSequence":4,"executedSequence":5,"createdAt":10,"updatedAt":11}, last])),
        ("commands-newest", "terminal.read-commands", json!({"limit":1}), json!([last])),
    ] {
        let mut params = params;
        params["target"] = target.clone();
        let reply = harness.verify(id, request(id, method, &token, params), id, None, 1, 1)?;
        assert_eq!(reply["result"], expected, "{id}");
    }
    assert!(!index.exists());

    for (id, method, params, message) in [
        ("read-required-lines", READ, json!({"maxBytes":1}), "maxLines must be between 1 and 5000"),
        ("read-zero-lines", READ, json!({"maxLines":0,"maxBytes":1}), "maxLines must be between 1 and 5000"),
        ("read-too-many-bytes", READ, json!({"maxLines":1,"maxBytes":1_048_577}), "maxBytes must be between 1 and 1048576"),
        ("history-fraction", "terminal.read-history", json!({"maxLines":0.5,"maxBytes":1}), "maxLines must be between 1 and 5000"),
        ("history-non-numeric", "terminal.read-history", json!({"maxLines":1,"maxBytes":"10"}), "maxBytes must be between 1 and 1048576"),
        ("history-bad-sequence", "terminal.read-history", json!({"maxLines":1,"maxBytes":1,"fromSequence":-1}), "fromSequence must be a non-negative integer"),
        ("history-reversed-range", "terminal.read-history", json!({"maxLines":1,"maxBytes":1,"fromSequence":5,"throughSequence":4}), "throughSequence must be at least fromSequence"),
        ("commands-limit", "terminal.read-commands", json!({"limit":1001}), "limit must be between 1 and 1000"),
    ] {
        let mut params = params;
        params["target"] = target.clone();
        let reply = harness.verify(id, request(id, method, &token, params), id, Some(INVALID), 1, 0)?;
        assert_eq!(reply["error"], json!({"code":"INVALID_REQUEST","message":message}));
    }
    Ok(())
}
