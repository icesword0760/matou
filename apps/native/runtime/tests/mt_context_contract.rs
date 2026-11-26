#[cfg(unix)]
use matou_native_runtime::context_guard::redact_secrets;
use rusqlite::Connection;
use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn mt(workspace: &Path, data_root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_matou-native-runtime"));
    command
        .args(["--mt", "ctx"])
        .current_dir(workspace)
        .env("MATOU_DATA_ROOT", data_root)
        .env_remove("MATOU_SESSION_ID")
        .env("MATOU_CONTROL_CALLER_SESSION", "test-card");
    command
}

fn text(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.len() <= 8_800);
    String::from_utf8(output.stdout).unwrap()
}

#[cfg(unix)]
#[test]
fn exec_keeps_raw_streams_shell_status_and_direct_arguments() {
    let dir = tempfile::tempdir().unwrap();
    let run = |args: &[&str]| mt(dir.path(), dir.path()).args(args).output().unwrap();
    let output = run(&["exec", "--", "printf stdout; printf stderr >&2; exit 7"]);
    assert_eq!(output.status.code(), Some(7));
    assert_eq!(output.stdout, b"stdout");
    assert_eq!(output.stderr, b"stderr");

    let output = run(&["exec", "--", "/bin/echo", "--json", "literal space; false"]);
    assert!(output.status.success());
    assert_eq!(output.stdout, b"--json literal space; false\n");
    assert!(output.stderr.is_empty());

    let output = run(&["exec", "--", "kill -TERM $$"]);
    assert_eq!(output.status.code(), Some(143));

    let output = run(&["exec", "--session", "--", "touch should-not-exist"]);
    assert!(!output.status.success());
    assert!(!dir.path().join("should-not-exist").exists());
}

#[cfg(unix)]
#[test]
fn exec_captures_full_redacted_log_under_host_budget_and_records_savings() {
    let dir = tempfile::tempdir().unwrap();
    let script = "i=0; while [ \"$i\" -lt 1400 ]; do printf 'ordinary build output 012345678901234567890\n'; i=$((i+1)); done; \
                  printf 'ERROR first cause token=sk-secret1234567890abcdef\n' >&2; \
                  i=0; while [ \"$i\" -lt 400 ]; do printf 'FAIL repeated error 012345678901234567890\n' >&2; i=$((i+1)); done; \
                  printf 'panic last failure\n' >&2; exit 23";
    let raw = Command::new("/bin/sh")
        .args(["-c", script])
        .output()
        .unwrap();
    let bytes = raw.stdout.len() + raw.stderr.len();
    assert!(bytes > 50_000);

    let output = mt(dir.path(), dir.path())
        .args(["exec", "--", script])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(23));
    assert!(output.stderr.is_empty());
    assert!(output.stdout.len() <= 7_900);
    let summary = String::from_utf8(output.stdout).unwrap();
    assert!(summary.contains("ERROR first cause"));
    assert!(summary.contains("panic last failure"));
    assert!(summary.contains("402 errors; 400 error lines omitted"));
    assert!(summary.contains("log_0001"));
    assert!(!summary.contains("sk-secret1234567890abcdef"));

    let mut whole_log = String::new();
    let mut offset = "0".to_owned();
    loop {
        let page = text(
            mt(dir.path(), dir.path())
                .args(["log", "log_0001", "--offset", &offset])
                .output()
                .unwrap(),
        );
        if let Some((body, next)) =
            page.rsplit_once("\n\n[Continue with `mt ctx log log_0001 --offset ")
        {
            let next = next.strip_suffix("`.]\n").unwrap();
            assert!(next.parse::<usize>().unwrap() > offset.parse::<usize>().unwrap());
            whole_log.push_str(body);
            offset = next.to_owned();
        } else {
            whole_log.push_str(page.strip_suffix('\n').unwrap());
            break;
        }
    }
    let expected = redact_secrets(&String::from_utf8([raw.stdout, raw.stderr].concat()).unwrap());
    assert_eq!(whole_log, expected);
    assert!(!whole_log.contains("sk-secret1234567890abcdef"));

    let resume = text(mt(dir.path(), dir.path()).arg("resume").output().unwrap());
    assert!(resume.contains("mt ctx exec (exit 23)"));
    assert!(resume.contains("log_0001"));
    let db = Connection::open(dir.path().join("context_guard.sqlite")).unwrap();
    let savings = db
        .query_row(
            "SELECT bytes, events FROM saved_stats WHERE agent_id = 'antigravity'",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .unwrap();
    assert_eq!(savings, ((bytes - summary.len()) as i64, 1));
}

#[cfg(unix)]
#[test]
fn exec_does_not_repeat_or_change_status_when_log_storage_fails() {
    for locked in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("context_guard.sqlite");
        let _lock = if locked {
            let connection = Connection::open(&db_path).unwrap();
            connection
                .execute_batch("PRAGMA journal_mode=WAL; BEGIN IMMEDIATE")
                .unwrap();
            Some(connection)
        } else {
            fs::write(&db_path, "not a database").unwrap();
            None
        };
        let output = mt(dir.path(), dir.path())
            .args(["exec", "--max-bytes", "32", "--",
                "printf x >> invocations; printf 'ERROR database fixture output 01234567890123456789\n' >&2; exit 19"])
            .output().unwrap();
        assert_eq!(output.status.code(), Some(19));
        assert_eq!(fs::read(dir.path().join("invocations")).unwrap(), b"x");
        assert!(output.stdout.len() <= 7_900);
        let summary = String::from_utf8(output.stdout).unwrap();
        assert!(summary.contains("ERROR database fixture"));
        assert!(summary.contains("Full log could not be saved"));
    }
}

#[cfg(unix)]
#[test]
fn exec_uses_the_selected_agents_configured_threshold_and_savings() {
    use matou_native_core::context_optimizer_config::{
        AgentOptimizerSettings, ContextOptimizerConfig,
    };

    let dir = tempfile::tempdir().unwrap();
    ContextOptimizerConfig::for_data_root(dir.path())
        .set_agent_settings(
            "codex",
            AgentOptimizerSettings {
                max_log_bytes: 4_096,
                ..Default::default()
            },
        )
        .unwrap();
    let script = "printf '%9000s' content";
    let passthrough = mt(dir.path(), dir.path())
        .args(["exec", "--", script])
        .output()
        .unwrap();
    assert!(passthrough.status.success());
    assert_eq!(passthrough.stdout.len(), 9_000);
    let trimmed = mt(dir.path(), dir.path())
        .args(["exec", "--agent", "codex", "--", script])
        .output()
        .unwrap();
    assert!(trimmed.status.success());
    assert!(trimmed.stdout.len() <= 7_900);
    assert!(String::from_utf8_lossy(&trimmed.stdout).contains("log_0001"));
    let conn = Connection::open(dir.path().join("context_guard.sqlite")).unwrap();
    let agent: String = conn
        .query_row("SELECT agent_id FROM saved_stats", [], |row| row.get(0))
        .unwrap();
    assert_eq!(agent, "codex");
}

#[test]
fn data_root_prefers_explicit_then_env_then_existing_platform_directory() {
    let dir = tempfile::tempdir().unwrap();
    #[cfg(not(windows))]
    let (platform_env, platform_root) = ("HOME", dir.path().join(".matou"));
    #[cfg(windows)]
    let (platform_env, platform_root) = ("APPDATA", dir.path().join("Matou"));
    let env_root = dir.path().join("env");
    let explicit = dir.path().join("explicit");
    fs::write(dir.path().join("doc"), "fixture").unwrap();
    let invoke = || {
        let mut command = mt(dir.path(), &env_root);
        command.env(platform_env, dir.path()).args(["index", "doc"]);
        #[cfg(windows)]
        command.env("USERPROFILE", dir.path());
        command
    };

    let missing = invoke().env_remove("MATOU_DATA_ROOT").output().unwrap();
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("MATOU_DATA_ROOT"));
    fs::create_dir(&platform_root).unwrap();
    for (root, output) in [
        (
            &explicit,
            invoke().arg("--data-root").arg(&explicit).output().unwrap(),
        ),
        (&env_root, invoke().output().unwrap()),
        (
            &platform_root,
            invoke().env_remove("MATOU_DATA_ROOT").output().unwrap(),
        ),
    ] {
        assert!(text(output).contains("log_0001"));
        assert!(root.join("context_guard.sqlite").is_file());
    }
}

#[test]
fn managed_environment_scopes_search_and_resume_and_expired_ids_stay_distinct() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("data");
    let workspace = dir.path().join("工作区 with spaces");
    fs::create_dir(&workspace).unwrap();
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .current_dir(&workspace)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    };
    git(&["init", "--quiet"]);
    fs::write(workspace.join("first.txt"), "alpha common_lookup").unwrap();
    git(&["add", "first.txt"]);
    fs::write(workspace.join("first.txt"), "alpha common_lookup changed").unwrap();
    fs::write(workspace.join("other file.txt"), "beta common_lookup").unwrap();

    let run = |args: &[&str]| mt(&workspace, &root).args(args).output().unwrap();
    assert!(text(run(&["index", "first.txt"])).contains("log_0001"));
    assert!(
        text(run(&["index", "other file.txt", "--session", "other-card"])).contains("log_0002")
    );
    let found = text(run(&["search", "common_lookup"]));
    assert!(found.contains("log_0001") && !found.contains("log_0002"));
    assert!(text(run(&["log", "log_0001"])).contains("alpha"));

    let resume = text(run(&["resume"]));
    assert!(resume.contains("test-card") && !resume.contains("log_0002"));
    assert!(resume.contains("first.txt") && resume.contains("other file.txt"));
    let changed = text(run(&["git"]));
    assert!(changed.contains("first.txt") && changed.contains("other file.txt"));

    let empty_path = dir.path().join("no executables");
    fs::create_dir(&empty_path).unwrap();
    let without_git = |subcommand| {
        mt(&workspace, &root)
            .env("PATH", &empty_path)
            .arg(subcommand)
            .output()
            .unwrap()
    };
    let unavailable = without_git("git");
    assert!(!unavailable.status.success());
    assert!(String::from_utf8_lossy(&unavailable.stderr).contains("GIT_NOT_INSTALLED"));
    let resume_without_git = text(without_git("resume"));
    assert!(resume_without_git.contains("log_0001"));
    assert!(!resume_without_git.contains("Git changed files"));
    let outside = mt(dir.path(), &root).arg("git").output().unwrap();
    assert!(!outside.status.success());
    assert!(String::from_utf8_lossy(&outside.stderr).contains("GIT_NOT_REPOSITORY"));

    let env_resume = text(
        mt(&workspace, &root)
            .env("MATOU_SESSION_ID", "other-card")
            .arg("resume")
            .output()
            .unwrap(),
    );
    assert!(env_resume.contains("log_0002") && !env_resume.contains("log_0001"));
    let db = Connection::open(root.join("context_guard.sqlite")).unwrap();
    let marker: i64 = db
        .query_row(
            "SELECT manual_resume_at FROM session_resume WHERE session_id = 'test-card'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(marker > 0);

    db.execute(
        "UPDATE log_sources SET created_at_sec = 0 WHERE id = 'log_0001'",
        [],
    )
    .unwrap();
    let before_cleanup = run(&["log", "log_0001"]);
    assert!(!before_cleanup.status.success());
    assert!(String::from_utf8_lossy(&before_cleanup.stderr).contains("has expired"));
    assert!(text(run(&["index", "first.txt"])).contains("log_0003"));
    for table in ["log_chunks", "log_chunks_fts"] {
        let count: i64 = db
            .query_row(
                &format!("SELECT count(*) FROM {table} WHERE source_id = 'log_0001'"),
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }
    for args in [
        &["log", "log_0001"][..],
        &["search", "alpha", "--source", "log_0001"],
    ] {
        let output = run(args);
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("has expired"));
    }
    let unknown = run(&["log", "log_9999"]);
    assert!(!unknown.status.success());
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("was not found"));
}
