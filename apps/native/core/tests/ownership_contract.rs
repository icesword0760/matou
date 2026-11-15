use std::{
    fs,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use matou_native_core::ownership::{
    DatabaseOwner, OwnerRecord, OwnerRecoveryError, read_database_owner,
};
use serde_json::{Value, json};

const GENERATION: &str = "native-contract";
const CONFLICT: &str = "database is already owned by a live Runtime";
const DEAD_PID: u64 = 2_147_483_647;
const WAIT: Duration = Duration::from_secs(15);
const CHILD_ROOT: &str = "MATOU_OWNERSHIP_TEST_ROOT";
const CHILD_ID: &str = "MATOU_OWNERSHIP_TEST_ID";

#[test]
fn agrees_with_nine_captured_reference_cases() -> Result<()> {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/owner-reference.json"))?;
    let cases = fixture["cases"].as_array().context("reference cases")?;
    assert_eq!(cases.len(), 9);
    for expected in cases {
        let name = expected["name"].as_str().context("case name")?;
        let root = tempfile::tempdir()?;
        let directory = if name == "empty" {
            root.path().join("new").join("data")
        } else {
            root.path().to_owned()
        };
        let db = directory.join("matou.sqlite");
        let owner_path = directory.join("matou.sqlite.owner");
        let sidecar = directory.join("matou.sqlite.owner.takeover.sqlite");
        let pid = if name.starts_with("live") {
            std::process::id().into()
        } else {
            DEAD_PID
        };
        match name {
            "empty" | "generation-replaced" => {}
            "live-legacy" | "dead-legacy" => {
                fs::create_dir(&owner_path)?;
                write_owner(&owner_path.join("owner.json"), pid, "previous")?;
            }
            "malformed" => fs::write(&owner_path, "{")?,
            "invalid-pid" => write_owner(&owner_path, 0, "previous")?,
            "unusable-sidecar" => {
                write_owner(&owner_path, pid, "previous")?;
                fs::create_dir(&sidecar)?;
            }
            _ => write_owner(&owner_path, pid, "previous")?,
        }
        let mut acquired = DatabaseOwner::acquire(&db, GENERATION);
        let mut actual = json!({
            "name": name,
            "acquired": normalized(read_database_owner(&owner_path)),
            "format": if owner_path.is_file() { "file" } else if owner_path.is_dir() { "directory" } else { "absent" }
        });
        if let Err(error) = &acquired {
            let directory = root.path().to_string_lossy();
            let normalized = error.to_string().replace(directory.as_ref(), "<root>");
            actual["error"] = json!(normalized);
            if let Some(recovery) = error.downcast_ref::<OwnerRecoveryError>() {
                actual["issue"] = json!(recovery.issue);
                actual["code"] = json!(recovery.code);
                assert_eq!(
                    recovery.state_path,
                    if name == "unusable-sidecar" {
                        &sidecar
                    } else {
                        &owner_path
                    }
                    .as_path()
                );
            }
        }
        if name == "generation-replaced" {
            write_owner(&owner_path, std::process::id().into(), "replacement")?;
        }
        if let Ok(owner) = &mut acquired {
            owner.release()?;
            owner.release()?;
        }
        drop(acquired);
        actual["afterRelease"] = normalized(read_database_owner(&owner_path));
        actual["remains"] = json!(owner_path.exists());
        let mut expected = expected.clone();
        if name == "malformed" {
            let prefix = "database ownership state requires recovery: owner-record-malformed at <root>/matou.sqlite.owner: ";
            let observed = actual["error"].as_str().context("native recovery error")?;
            let original = expected["error"]
                .as_str()
                .context("reference recovery error")?;
            assert!(
                observed
                    .strip_prefix(prefix)
                    .is_some_and(|diagnostic| !diagnostic.is_empty())
            );
            assert!(
                original
                    .strip_prefix(prefix)
                    .is_some_and(|diagnostic| !diagnostic.is_empty())
            );
            // Parser-specific detail is internal. The recovery issue and path are compared exactly.
            actual["error"] = json!(prefix);
            expected["error"] = json!(prefix);
        }
        assert_eq!(actual, expected, "{name}");
        assert!(
            !fs::read_dir(&directory)?
                .filter_map(std::result::Result::ok)
                .any(|entry| { entry.file_name().to_string_lossy().contains(".claim-") }),
            "{name}: temporary claim was left behind"
        );
        assert!(!db.exists(), "{name}: ownership must not open the database");
    }
    Ok(())
}

#[test]
fn rejects_corrupted_sqlite_fence_without_replacing_the_owner() -> Result<()> {
    let root = tempfile::tempdir()?;
    let db = root.path().join("matou.sqlite");
    let owner_path = root.path().join("matou.sqlite.owner");
    let sidecar = root.path().join("matou.sqlite.owner.takeover.sqlite");
    write_owner(&owner_path, DEAD_PID, "previous")?;
    fs::write(&sidecar, b"not a SQLite database")?;

    let expected = read_database_owner(&owner_path);
    let error = DatabaseOwner::acquire(&db, GENERATION).expect_err("corrupted takeover fence");
    let recovery = error
        .downcast_ref::<OwnerRecoveryError>()
        .context("recovery error")?;
    assert_eq!(recovery.code, "OWNERSHIP_RECOVERY_REQUIRED");
    assert_eq!(recovery.issue, "takeover-sidecar-unusable");
    assert_eq!(recovery.state_path, sidecar);
    assert_eq!(read_database_owner(&owner_path), expected);
    Ok(())
}

#[test]
fn contending_processes_allow_one_owner_and_recover_after_crash() -> Result<()> {
    for scenario in ["empty", "stale-file"] {
        let root = tempfile::tempdir()?;
        let db = root.path().join("matou.sqlite");
        let owner_path = root.path().join("matou.sqlite.owner");
        if scenario == "stale-file" {
            write_owner(&owner_path, DEAD_PID, "earlier-generation")?;
            let abandoned = root.path().join("matou.sqlite.owner.takeover");
            fs::create_dir(&abandoned)?;
            fs::write(abandoned.join("orphan"), b"legacy")?;
        }
        let executable = std::env::current_exe()?;
        let mut children = Children(Vec::new());
        for id in 0..5 {
            children.0.push(
                Command::new(&executable)
                    .args(["--exact", "ownership_contender_process", "--nocapture"])
                    .env(CHILD_ROOT, root.path())
                    .env(CHILD_ID, id.to_string())
                    .stdin(Stdio::null())
                    .spawn()?,
            );
        }
        wait_for(|| {
            (0..children.0.len()).all(|id| root.path().join(format!("ready-{id}")).exists())
        })?;
        fs::write(root.path().join("start"), b"")?;
        wait_for(|| (0..children.0.len()).all(|id| result_path(root.path(), id).exists()))?;

        let claims = (0..children.0.len())
            .map(|id| fs::read_to_string(result_path(root.path(), id)))
            .collect::<std::io::Result<Vec<_>>>()?;
        let winners: Vec<_> = claims
            .iter()
            .enumerate()
            .filter(|(_, claim)| *claim == "owned")
            .collect();
        assert_eq!(winners.len(), 1, "{scenario}: {claims:?}");
        assert!(
            claims
                .iter()
                .all(|claim| claim == "owned" || claim == CONFLICT),
            "{claims:?}"
        );
        let winner = winners[0].0;
        let winning_record = read_database_owner(&owner_path).context("winning owner record")?;
        assert_eq!(winning_record.pid, u64::from(children.0[winner].id()));
        assert_eq!(
            winning_record.runtime_generation,
            format!("contender-{winner}")
        );
        assert!(!root.path().join("matou.sqlite.owner.takeover").exists());
        for (id, child) in children.0.iter_mut().enumerate() {
            if id != winner {
                wait_for(|| child.try_wait().ok().flatten().is_some())?;
                assert!(child.wait()?.success());
            }
        }

        children.0[winner].kill()?;
        children.0[winner].wait()?;
        assert_eq!(read_database_owner(&owner_path), Some(winning_record));
        {
            let _owner = DatabaseOwner::acquire(&db, "after-crash")?;
            assert_eq!(
                read_database_owner(&owner_path).unwrap().runtime_generation,
                "after-crash"
            );
        }
        assert!(!owner_path.exists());
    }
    Ok(())
}

#[test]
fn ownership_contender_process() -> Result<()> {
    let Some(root) = std::env::var_os(CHILD_ROOT).map(PathBuf::from) else {
        return Ok(());
    };
    let id: usize = std::env::var(CHILD_ID)?.parse()?;
    let db = root.join("matou.sqlite");
    fs::write(root.join(format!("ready-{id}")), b"")?;
    wait_for(|| root.join("start").is_file())?;
    let lease = DatabaseOwner::acquire(&db, &format!("contender-{id}"));
    let message = match &lease {
        Ok(_) => "owned".to_owned(),
        Err(error) => error.to_string(),
    };
    let temp = root.join(format!("partial-{id}"));
    fs::write(&temp, message)?;
    fs::rename(temp, result_path(&root, id))?;
    if let Ok(_lease) = lease {
        wait_for(|| root.join("finish").exists())?;
    }
    Ok(())
}

struct Children(Vec<Child>);

impl Drop for Children {
    fn drop(&mut self) {
        for child in &mut self.0 {
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
    }
}

fn result_path(root: &Path, id: usize) -> PathBuf {
    root.join(format!("result-{id}"))
}

fn wait_for(mut ready: impl FnMut() -> bool) -> Result<()> {
    let deadline = Instant::now() + WAIT;
    while !ready() {
        ensure!(
            Instant::now() < deadline,
            "timed out waiting for ownership test process"
        );
        thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}

fn write_owner(path: &Path, pid: u64, generation: &str) -> Result<()> {
    fs::write(
        path,
        serde_json::to_vec(&OwnerRecord {
            pid,
            runtime_generation: generation.into(),
        })?,
    )?;
    Ok(())
}

fn normalized(record: Option<OwnerRecord>) -> Value {
    record.map_or(Value::Null, |record| json!({
        "pid": if record.pid == u64::from(std::process::id()) { json!("<self>") } else { json!(record.pid) },
        "runtimeGeneration": record.runtime_generation,
    }))
}
