use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::{Arc, Barrier},
    thread,
};

use anyhow::{Context, Result};
use matou_native_core::recovery_marker::{
    RecoveryMarker, durable_after_failure, is_resolved, load_pending, publish, read_active, resolve,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::{Uuid, Version};

const MARKER: &str = "matou.sqlite.recovery.json";

#[derive(Deserialize)]
struct ReferenceFixture {
    cases: Vec<ReferenceCase>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReferenceCase {
    name: String,
    action: String,
    source_root: String,
    before: BTreeMap<String, String>,
    after: BTreeMap<String, String>,
    #[serde(default)]
    directories: Vec<String>,
    expected: Option<Value>,
    error: Option<String>,
}

fn sha256(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

// Rebase only the temporary path from the capture. For a generation claim,
// translate its nested record as well and update the path-dependent hashes.
fn relocate_files(
    files: BTreeMap<String, String>,
    source_root: &str,
    root: &Path,
) -> Result<BTreeMap<String, String>> {
    let root = root.to_str().context("fixture path is not UTF-8")?;
    let path_prefix = serde_json::to_string(&format!("{root}{}", std::path::MAIN_SEPARATOR))?;
    let path_prefix = path_prefix.trim_matches('"');
    let source_prefix = format!("{source_root}/");
    let relocate = |raw: &str| raw.replace(&source_prefix, path_prefix);
    let mut relocated = BTreeMap::new();
    for (name, raw) in files {
        if !name.ends_with(".claim") {
            relocated.insert(name, relocate(&raw));
            continue;
        }
        let mut claim: Value = serde_json::from_str(&raw)?;
        let marker = serde_json::to_string(&claim["marker"])?;
        assert_eq!(claim["markerDigest"], sha256(&marker));
        let source = &claim["source"];
        assert_eq!(
            name,
            format!(
                "{MARKER}.generation-{}.claim",
                sha256(&serde_json::to_string(source)?)
            )
        );
        if let Some(bytes) = source["markerBytes"].as_str() {
            assert_eq!(source["markerDigest"], sha256(bytes));
            let bytes = relocate(bytes);
            claim["source"]["markerDigest"] = json!(sha256(&bytes));
            claim["source"]["markerBytes"] = json!(bytes);
        }
        let marker = relocate(&marker);
        claim["markerDigest"] = json!(sha256(&marker));
        claim["marker"] = serde_json::from_str(&marker)?;
        let name = format!(
            "{MARKER}.generation-{}.claim",
            sha256(&serde_json::to_string(&claim["source"])?)
        );
        relocated.insert(name, serde_json::to_string(&claim)?);
    }
    Ok(relocated)
}

#[test]
fn recovery_marker_matches_frozen_bootstrap() -> Result<()> {
    let fixture: ReferenceFixture =
        serde_json::from_str(include_str!("fixtures/recovery-marker-reference.json"))?;
    for case in fixture.cases {
        let workspace = tempfile::tempdir()?;
        let root = workspace.path();
        let db = root.join("matou.sqlite");
        let marker_path = root.join(MARKER);
        let before = relocate_files(case.before, &case.source_root, root)?;
        let mut after = relocate_files(case.after, &case.source_root, root)?;
        for directory in case.directories {
            fs::create_dir(root.join(directory))?;
        }
        for (name, raw) in &before {
            fs::write(root.join(name), raw)?;
        }
        match case.action.as_str() {
            "load" => {
                let pending = load_pending(&db);
                if let Some(error) = case.error {
                    assert_eq!(pending.unwrap_err().to_string(), error, "{}", case.name);
                } else {
                    let pending = pending?.context("frozen reference exposes pending recovery")?;
                    assert_eq!(pending.marker_error, None, "{}", case.name);
                    assert_eq!(
                        read_active(&db)?.as_ref(),
                        Some(&pending.marker),
                        "{}",
                        case.name
                    );
                    let captured: RecoveryMarker = serde_json::from_str(&after[MARKER])?;
                    let mut actual = pending.marker;
                    if case.name == "legacy-no-id" {
                        assert_eq!(
                            Uuid::parse_str(&actual.recovery_id)?.get_version(),
                            Some(Version::Random)
                        );
                        assert_eq!(load_pending(&db)?.unwrap().marker, actual);
                        let canonical = after.get_mut(MARKER).unwrap();
                        *canonical = canonical.replace(&captured.recovery_id, &actual.recovery_id);
                        actual.recovery_id.clone_from(&captured.recovery_id);
                    }
                    assert_eq!(actual, captured, "{}", case.name);
                }
            }
            "resolve" => {
                let expected = case
                    .expected
                    .context("frozen resolution outcome is missing")?;
                let original: RecoveryMarker = serde_json::from_str(&before[MARKER])?;
                let tombstone: Value = serde_json::from_str(
                    &after[&format!("{MARKER}.resolved-{}", original.recovery_id)],
                )?;
                let now = tombstone["resolvedAt"]
                    .as_i64()
                    .context("resolution time")?;
                let changed = expected["wrongIdError"]
                    .as_str()
                    .context("wrong ID error")?;
                assert_eq!(
                    json!(is_resolved(&marker_path, &original.recovery_id)?),
                    expected["wasResolved"]
                );
                assert_eq!(
                    resolve(&marker_path, "wrong-generation", now, changed)
                        .unwrap_err()
                        .to_string(),
                    changed,
                );
                resolve(&marker_path, &original.recovery_id, now, changed)?;
                assert_eq!(
                    json!(is_resolved(&marker_path, &original.recovery_id)?),
                    expected["resolved"]
                );
                assert!(load_pending(&db)?.is_none());
            }
            action => panic!("unexpected reference action {action}"),
        }
        let marker_names = fs::read_dir(root)?
            .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
            .collect::<std::io::Result<Vec<_>>>()?;
        let mut marker_names: Vec<_> = marker_names
            .into_iter()
            .filter(|name| name.starts_with(MARKER))
            .collect();
        marker_names.sort();
        assert_eq!(
            marker_names.iter().collect::<Vec<_>>(),
            after.keys().collect::<Vec<_>>(),
            "{}: marker inventory",
            case.name
        );
        for (name, raw) in after {
            assert_eq!(
                fs::read_to_string(root.join(&name))?,
                raw,
                "{}: {name}",
                case.name
            );
        }
    }
    Ok(())
}

fn required(db: &Path, recovery_id: &str) -> Result<RecoveryMarker> {
    Ok(serde_json::from_value(json!({
        "version": 1,
        "state": "required",
        "recoveryId": recovery_id,
        "reason": "physical-corruption",
        "durableDatabasePath": db,
        "quarantinedPath": format!("{}.corrupt-123", db.display()),
        "markerPath": format!("{}.recovery.json", db.display()),
        "createdAt": 123,
    }))?)
}

#[test]
fn concurrent_publishers_choose_one_generation_even_without_action_fence() -> Result<()> {
    for broken_fence in [false, true] {
        let root = tempfile::tempdir()?;
        let db = root.path().join("matou.sqlite");
        let sidecar = root.path().join("matou.sqlite.recovery-action.sqlite");
        if broken_fence {
            fs::write(&sidecar, b"This is not a SQLite database")?;
        }
        let barrier = Arc::new(Barrier::new(4));
        let mut handles = Vec::new();
        for index in 0..4 {
            let marker = required(&db, &format!("recovery-{index}"))?;
            let barrier = barrier.clone();
            handles.push(thread::spawn(move || {
                barrier.wait();
                publish(marker)
            }));
        }
        let markers: Vec<_> = handles
            .into_iter()
            .map(|thread| {
                thread
                    .join()
                    .map_err(|_| anyhow::anyhow!("publisher panicked"))?
            })
            .collect::<Result<_>>()?;
        let marker = &markers[0];
        assert!(markers.iter().all(|candidate| candidate == marker));
        assert_eq!(read_active(&db)?.as_ref(), Some(marker));
        assert_eq!(
            durable_after_failure(&db, &anyhow::anyhow!("publication error"))?.as_ref(),
            Some(marker)
        );

        if broken_fence {
            // Simulate a crash after the generation claim was published and
            // before the canonical marker was installed.
            fs::remove_file(&marker.marker_path)?;
            assert!(read_active(&db)?.is_none());
            let pending = load_pending(&db)?.context("durable generation claim")?;
            assert_eq!(pending.marker, *marker);
            assert_eq!(pending.marker_error, None);
        }

        let canonical = fs::read(&marker.marker_path)?;
        let changed = "recovery state changed";
        assert_eq!(
            resolve(&marker.marker_path, "different-generation", 456, changed)
                .unwrap_err()
                .to_string(),
            changed
        );
        assert!(!is_resolved(&marker.marker_path, "different-generation")?);
        assert!(read_active(&db)?.is_some());
        resolve(&marker.marker_path, &marker.recovery_id, 456, changed)?;
        assert!(is_resolved(&marker.marker_path, &marker.recovery_id)?);
        assert!(!is_resolved(&marker.marker_path, "different-generation")?);
        assert!(load_pending(&db)?.is_none());
        assert_eq!(fs::read(&marker.marker_path)?, canonical);
        let done = root.path().join(format!(
            "matou.sqlite.recovery.json.resolved-{}",
            marker.recovery_id
        ));
        let tombstone: Value = serde_json::from_slice(&fs::read(done)?)?;
        assert_eq!(
            tombstone,
            json!({
                "version": 1, "state": "resolved", "recoveryId": marker.recovery_id, "markerPath": marker.marker_path, "resolvedAt": 456,
            })
        );

        let next = publish(required(&db, "after-resolution")?)?;
        assert_eq!(next.recovery_id, "after-resolution");
        assert!(!is_resolved(&marker.marker_path, &marker.recovery_id)?);
        assert_eq!(load_pending(&db)?.unwrap().marker, next);
        if broken_fence {
            // This claim's source is the exact prior marker and tombstone.
            // Reinstall the prior generation to exercise claim validation and repair.
            fs::write(&marker.marker_path, canonical)?;
            assert_eq!(load_pending(&db)?.unwrap().marker, next);
        }
    }
    Ok(())
}
