use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::Result;
use matou_native_core::journal::{
    Frame, JournalOptions, Record, SegmentJournal, for_each_frame, wait_for_compression,
};
use matou_native_runtime::retention_policy::{
    IndexedCheckpoint, IndexedSession, RetentionQuota, SessionRetention, cleanup_isolated,
    plan_retention,
};
use serde_json::{Value, json};

const MAGIC: &[u8] = b"MTCPV4\n";

fn checkpoint(root: &Path, session: &str, generation: u64) -> Result<PathBuf> {
    let dir = root.join("checkpoints").join(session);
    fs::create_dir_all(&dir)?;
    let path = dir.join(format!("checkpoint-{generation:06}.bin"));
    let snapshot = format!("snapshot-{generation}");
    let header = serde_json::to_vec(&json!({
        "id": format!("{session}-{generation}"),
        "sessionId": session,
        "generation": generation,
        "terminalSequence": generation,
        "domainEventSequence": 0,
        "screenEpoch": 0,
        "snapshotLength": snapshot.len(),
    }))?;
    let mut contents = MAGIC.to_vec();
    contents.extend_from_slice(&u32::try_from(header.len())?.to_be_bytes());
    contents.extend_from_slice(&header);
    contents.extend_from_slice(snapshot.as_bytes());
    fs::write(&path, contents)?;
    Ok(path)
}

fn read_checkpoint(path: &Path) -> Result<(u64, String)> {
    let file = fs::read(path)?;
    assert!(file.starts_with(MAGIC));
    let start = MAGIC.len() + 4;
    let end = start + u32::from_be_bytes(file[MAGIC.len()..start].try_into()?) as usize;
    let header: Value = serde_json::from_slice(&file[start..end])?;
    let snapshot = &file[end..];
    assert_eq!(header["snapshotLength"], snapshot.len());
    Ok((
        header["generation"].as_u64().unwrap(),
        String::from_utf8(snapshot.to_vec())?,
    ))
}

fn read_journal(root: &Path, session: &str) -> Result<Vec<Record>> {
    let mut records = Vec::new();
    for_each_frame(root, session, 1, None, |record| {
        records.push(record);
        Ok(())
    })?;
    Ok(records)
}

#[test]
fn removes_only_obsolete_inactive_checkpoints_and_keeps_real_history_readable() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    let mut expected = Vec::new();
    let mut journal = SegmentJournal::open(
        root,
        "inactive",
        JournalOptions {
            max_segment_bytes: 512,
            raw_hot_bytes: 650,
            checkpoint_protected_sequences: vec![1],
            ..JournalOptions::default()
        },
    )?;
    for sequence in 1..=6 {
        let record = Record {
            timestamp: sequence as i64,
            frame: Frame::Output {
                sequence,
                data: vec![b'x'; 300],
            },
        };
        journal.append(&record)?;
        expected.push(record);
    }
    journal.close()?;
    wait_for_compression();
    assert_eq!(read_journal(root, "inactive")?, expected);
    for id in ["live", "protected"] {
        let mut journal = SegmentJournal::open(root, id, JournalOptions::default())?;
        journal.append(&expected[0])?;
        journal.close()?;
    }
    let journal_dir = root.join("journal/inactive");
    assert!(
        journal_dir.join("segment-000001.mtj").is_file(),
        "protected segment"
    );
    assert!(
        journal_dir.join("segment-000002.mtj.gz").is_file(),
        "cold compressed segment"
    );
    assert!(
        journal_dir.join("segment-000006.mtj").is_file(),
        "latest raw segment"
    );
    let journal_names = || -> std::io::Result<Vec<_>> {
        let mut names = fs::read_dir(&journal_dir)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<std::io::Result<Vec<_>>>()?;
        names.sort_unstable();
        Ok(names)
    };
    let before_journal = journal_names()?;

    // Creation times order removals, while generation numbers decide retention.
    // A missing indexed checkpoint must not take a slot from a readable one.
    let rows = [2, 4, 1, 3, 5].map(|generation| IndexedCheckpoint {
        generation,
        created_at_ms: if generation == 2 { 1 } else { 10_000_000_000 },
    });
    let sessions = [
        IndexedSession {
            id: "inactive",
            retention: SessionRetention::Inactive,
            checkpoints: &rows,
        },
        IndexedSession {
            id: "live",
            retention: SessionRetention::Active,
            checkpoints: &rows,
        },
        IndexedSession {
            id: "protected",
            retention: SessionRetention::Protected,
            checkpoints: &rows,
        },
    ];
    for id in ["inactive", "live", "protected", "unlisted"] {
        for generation in 1..=4 {
            checkpoint(root, id, generation)?;
        }
    }
    let unindexed = checkpoint(root, "inactive", 6)?;
    let quota = RetentionQuota {
        global_bytes: 500,
        per_session_bytes: 450,
        checkpoint_generations: 2,
    };
    let plan = plan_retention(root, quota, &sessions)?;
    let first = root.join("checkpoints/inactive/checkpoint-000001.bin");
    let second = root.join("checkpoints/inactive/checkpoint-000002.bin");
    let reclaimed = u128::from(fs::metadata(&first)?.len() + fs::metadata(&second)?.len());
    assert_eq!(plan.reclaimed_bytes, reclaimed);
    assert_eq!(
        plan.actions
            .iter()
            .map(|action| action.generation)
            .collect::<Vec<_>>(),
        [2, 1]
    );
    assert_eq!(cleanup_isolated(root, quota, &sessions)?, plan);

    assert!(!first.exists() && !second.exists());
    for id in ["live", "protected", "unlisted"] {
        let path = root
            .join("checkpoints")
            .join(id)
            .join("checkpoint-000001.bin");
        assert_eq!(read_checkpoint(&path)?, (1, "snapshot-1".to_owned()));
    }
    assert_eq!(read_checkpoint(&unindexed)?, (6, "snapshot-6".to_owned()));
    for generation in [3, 4] {
        let path = root
            .join("checkpoints/inactive")
            .join(format!("checkpoint-{generation:06}.bin"));
        assert_eq!(
            read_checkpoint(&path)?,
            (generation, format!("snapshot-{generation}"))
        );
    }
    assert_eq!(journal_names()?, before_journal);
    assert_eq!(read_journal(root, "inactive")?, expected);
    for id in ["live", "protected"] {
        assert_eq!(read_journal(root, id)?, expected[..1]);
    }
    let after = cleanup_isolated(root, quota, &sessions)?;
    assert_eq!(after.before_bytes, plan.before_bytes - reclaimed);
    assert_eq!(
        after.remaining_global_overage,
        plan.remaining_global_overage
    );
    assert!(after.actions.is_empty());
    Ok(())
}

#[test]
fn byte_limits_include_legacy_and_sealed_history_below_at_and_above_the_limits() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path();
    let quota = RetentionQuota::default();
    // Sparse files cross exact production byte thresholds without materializing
    // GiB of data. The other test uses fully decodable files.
    let journals = root.join("journal");
    for index in 1..=8 {
        let path = journals.join(format!("session-{index}"));
        fs::create_dir_all(&path)?;
        fs::File::create(path.join("segment-000001.bin.gz"))?
            .set_len(quota.per_session_bytes - 1)?;
        fs::File::create(path.join("segment-000002.mtj"))?.set_len(1)?;
        fs::write(path.join("segment-000003.mtj.gz.partial"), b"ignored")?;
    }
    let first = journals.join("session-1/segment-000001.bin.gz");
    for (delta, expected) in [(-1, 0), (0, 0), (1, 1)] {
        fs::File::options()
            .write(true)
            .open(&first)?
            .set_len((quota.per_session_bytes as i64 - 1 + delta) as u64)?;
        let plan = cleanup_isolated(root, quota, &[])?;
        assert_eq!(
            plan.before_bytes,
            (quota.global_bytes as i64 + delta) as u128
        );
        assert_eq!(plan.remaining_global_overage, expected);
        assert_eq!(
            plan.remaining_session_overages
                .get("session-1")
                .copied()
                .unwrap_or(0),
            expected
        );
        assert!(plan.actions.is_empty());
        assert!(first.is_file());
    }
    Ok(())
}

#[test]
fn keeps_the_configured_number_of_present_checkpoint_generations_below_quota() -> Result<()> {
    let root = tempfile::tempdir()?;
    let rows = [1, 2, 3, 4].map(|generation| IndexedCheckpoint {
        generation,
        created_at_ms: 0,
    });
    for row in rows {
        checkpoint(root.path(), "inactive", row.generation)?;
    }
    let index = [IndexedSession {
        id: "inactive",
        retention: SessionRetention::Inactive,
        checkpoints: &rows,
    }];
    let quota = RetentionQuota {
        checkpoint_generations: 3,
        ..RetentionQuota::default()
    };
    let plan = cleanup_isolated(root.path(), quota, &index)?;
    assert_eq!(plan.actions.len(), 1);
    assert_eq!(plan.actions[0].generation, 1);
    assert_eq!(plan.remaining_global_overage, 0);
    assert!(plan.remaining_session_overages.is_empty());
    Ok(())
}

#[cfg(unix)]
#[test]
fn refuses_symlinked_checkpoint_paths_before_any_removal() -> Result<()> {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir()?;
    let outside = tempfile::tempdir()?;
    let rows = [1, 2, 3].map(|generation| IndexedCheckpoint {
        generation,
        created_at_ms: 0,
    });
    for row in rows {
        checkpoint(root.path(), "first", row.generation)?;
        checkpoint(outside.path(), "other", row.generation)?;
    }
    let checkpoints = root.path().join("checkpoints");
    let target = outside.path().join("checkpoints/other");
    let linked = checkpoints.join("other");
    symlink(&target, &linked)?;
    let sessions = ["first", "other"].map(|id| IndexedSession {
        id,
        retention: SessionRetention::Inactive,
        checkpoints: &rows,
    });
    let quota = RetentionQuota::default();
    assert!(cleanup_isolated(root.path(), quota, &sessions).is_err());
    assert!(checkpoints.join("first/checkpoint-000001.bin").is_file());
    assert!(target.join("checkpoint-000001.bin").is_file());
    fs::remove_file(linked)?;
    fs::remove_file(checkpoints.join("first/checkpoint-000001.bin"))?;
    symlink(
        target.join("checkpoint-000001.bin"),
        checkpoints.join("first/checkpoint-000001.bin"),
    )?;
    assert!(cleanup_isolated(root.path(), quota, &sessions).is_err());
    assert!(target.join("checkpoint-000001.bin").is_file());
    Ok(())
}
