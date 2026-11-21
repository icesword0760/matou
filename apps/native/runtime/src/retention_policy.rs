//! Quota evaluation and checkpoint cleanup for a detached data root.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, ensure};
use matou_native_core::journal::MAX_SAFE_INTEGER;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionQuota {
    pub global_bytes: u64,
    pub per_session_bytes: u64,
    pub checkpoint_generations: usize,
}

impl Default for RetentionQuota {
    fn default() -> Self {
        Self {
            global_bytes: 2 * 1024 * 1024 * 1024,
            per_session_bytes: 256 * 1024 * 1024,
            checkpoint_generations: 2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRetention {
    Inactive,
    Active,
    Protected,
}

/// The caller supplies the current checkpoint index and protection state for
/// each session eligible for evaluation. Unlisted sessions cannot be cleaned.
#[derive(Debug, Clone, Copy)]
pub struct IndexedSession<'a> {
    pub id: &'a str,
    pub retention: SessionRetention,
    pub checkpoints: &'a [IndexedCheckpoint],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexedCheckpoint {
    pub generation: u64,
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckpointRemoval {
    pub session_id: String,
    pub generation: u64,
    pub relative_path: PathBuf,
    pub bytes: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub struct RetentionPlan {
    pub before_bytes: u128,
    pub reclaimed_bytes: u128,
    pub remaining_global_overage: u128,
    pub remaining_session_overages: BTreeMap<String, u128>,
    pub actions: Vec<CheckpointRemoval>,
}

/// Evaluate the frozen quota policy. Every journal representation contributes
/// to usage and is protected, including archived segments and legacy names.
/// Checkpoint generations are counted from the newest indexed files present.
pub fn plan_retention(
    root: &Path,
    quota: RetentionQuota,
    sessions: &[IndexedSession<'_>],
) -> Result<RetentionPlan> {
    evaluate(&isolated_root(root)?, quota, sessions)
}

/// Use only on a caller-owned, isolated root with no writers or other cleanup
/// running. Re-inventory immediately before doing any removals. Only obsolete
/// checkpoint files are unlinked, so retrying after an I/O failure is safe.
pub fn cleanup_isolated(
    root: &Path,
    quota: RetentionQuota,
    sessions: &[IndexedSession<'_>],
) -> Result<RetentionPlan> {
    let root = isolated_root(root)?;
    let plan = evaluate(&root, quota, sessions)?;
    for action in &plan.actions {
        fs::remove_file(root.join(&action.relative_path))
            .with_context(|| format!("removing obsolete checkpoint {:?}", action.relative_path))?;
    }
    Ok(plan)
}

fn evaluate(
    root: &Path,
    quota: RetentionQuota,
    sessions: &[IndexedSession<'_>],
) -> Result<RetentionPlan> {
    ensure!(
        (1..=MAX_SAFE_INTEGER).contains(&quota.global_bytes)
            && (1..=quota.global_bytes).contains(&quota.per_session_bytes)
            && (2..=MAX_SAFE_INTEGER as u64).contains(&(quota.checkpoint_generations as u64)),
        "invalid retention quota"
    );

    let mut by_id = BTreeMap::new();
    for session in sessions {
        ensure!(
            Path::new(session.id).file_name() == Some(OsStr::new(session.id)),
            "invalid retention session ID"
        );
        ensure!(
            by_id.insert(OsStr::new(session.id), session).is_none(),
            "duplicate retention session"
        );
        let mut generations = BTreeSet::new();
        for row in session.checkpoints {
            ensure!(
                (1..=MAX_SAFE_INTEGER).contains(&row.generation)
                    && generations.insert(row.generation),
                "invalid checkpoint generation"
            );
        }
    }

    let mut usage = BTreeMap::<String, u128>::new();
    let mut removals = Vec::new();
    let journal_root = root.join("journal");
    for dir in ordinary_children(&journal_root)?.into_iter().flatten() {
        let dir = dir?;
        if !dir.file_type()?.is_dir() {
            continue;
        }
        let session_id = dir.file_name().to_string_lossy().into_owned();
        for entry in ordinary_children(&dir.path())?.into_iter().flatten() {
            let entry = entry?;
            let name = entry.file_name();
            if name.to_str().is_some_and(is_journal_segment) && entry.file_type()?.is_file() {
                *usage.entry(session_id.clone()).or_default() +=
                    u128::from(entry.metadata()?.len());
            }
        }
    }
    let checkpoint_root = root.join("checkpoints");
    for dir in ordinary_children(&checkpoint_root)?.into_iter().flatten() {
        let dir = dir?;
        let Some(session) = by_id.get(dir.file_name().as_os_str()) else {
            continue;
        };
        let directory = dir.path();
        let Some(files) = ordinary_children(&directory)? else {
            continue;
        };
        let files = files
            .map(|entry| entry.map(|entry| (entry.file_name(), entry)))
            .collect::<io::Result<BTreeMap<_, _>>>()?;
        let mut indexed = session.checkpoints.to_vec();
        indexed.sort_unstable_by_key(|row| std::cmp::Reverse(row.generation));
        let bytes = usage.entry(session.id.to_owned()).or_default();
        let mut present = 0;
        for row in indexed {
            let name = format!("checkpoint-{:06}.bin", row.generation);
            let Some(entry) = files.get(OsStr::new(&name)) else {
                continue;
            };
            let info = entry.metadata()?;
            ensure!(
                info.is_file() && !info.file_type().is_symlink(),
                "invalid checkpoint file {:?}",
                entry.path()
            );
            *bytes += u128::from(info.len());
            present += 1;
            if session.retention == SessionRetention::Inactive
                && present > quota.checkpoint_generations
            {
                removals.push((
                    row.created_at_ms,
                    CheckpointRemoval {
                        session_id: session.id.to_owned(),
                        generation: row.generation,
                        relative_path: Path::new("checkpoints").join(session.id).join(name),
                        bytes: info.len(),
                    },
                ));
            }
        }
    }
    let before_bytes = usage.values().sum::<u128>();
    removals.sort_unstable_by(|(left_time, left), (right_time, right)| {
        left_time
            .cmp(right_time)
            .then_with(|| left.relative_path.cmp(&right.relative_path))
    });
    let actions: Vec<_> = removals.into_iter().map(|(_, action)| action).collect();
    let reclaimed_bytes = actions
        .iter()
        .map(|action| u128::from(action.bytes))
        .sum::<u128>();
    for action in &actions {
        *usage
            .get_mut(&action.session_id)
            .expect("inventoried session") -= u128::from(action.bytes);
    }
    Ok(RetentionPlan {
        before_bytes,
        reclaimed_bytes,
        remaining_global_overage: (before_bytes - reclaimed_bytes)
            .saturating_sub(quota.global_bytes.into()),
        remaining_session_overages: usage
            .into_iter()
            .filter_map(|(id, bytes)| {
                let excess = bytes.saturating_sub(quota.per_session_bytes.into());
                (excess != 0).then_some((id, excess))
            })
            .collect(),
        actions,
    })
}

fn isolated_root(root: &Path) -> Result<PathBuf> {
    let path: PathBuf = root.components().collect();
    ensure!(
        path.is_absolute(),
        "retention requires an absolute isolated data root"
    );
    let info = fs::symlink_metadata(&path)?;
    ensure!(
        info.is_dir() && !info.file_type().is_symlink(),
        "invalid retention data root"
    );
    let root = path.canonicalize()?;
    ensure!(
        root.parent().is_some(),
        "retention requires an isolated data directory"
    );
    Ok(root)
}

/// All paths below the canonical root must be real directories.
fn ordinary_children(path: &Path) -> Result<Option<fs::ReadDir>> {
    let info = match fs::symlink_metadata(path) {
        Ok(info) => info,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        info.is_dir() && !info.file_type().is_symlink() && path.canonicalize()? == path,
        "invalid retention directory {path:?}"
    );
    Ok(Some(fs::read_dir(path)?))
}

fn is_journal_segment(name: &str) -> bool {
    name.strip_prefix("segment-")
        .and_then(|name| name.split_once('.'))
        .is_some_and(|(number, suffix)| {
            number.len() == 6
                && number.bytes().all(|byte| byte.is_ascii_digit())
                && matches!(suffix, "mtj" | "bin" | "mtj.gz" | "bin.gz")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_frozen_defaults_and_quota_validation() {
        let root = tempfile::tempdir().unwrap();
        let defaults = RetentionQuota::default();
        assert_eq!(
            (
                defaults.global_bytes,
                defaults.per_session_bytes,
                defaults.checkpoint_generations
            ),
            (2_147_483_648, 268_435_456, 2)
        );
        for quota in [
            RetentionQuota {
                global_bytes: 0,
                ..defaults
            },
            RetentionQuota {
                global_bytes: MAX_SAFE_INTEGER + 1,
                ..defaults
            },
            RetentionQuota {
                per_session_bytes: 0,
                ..defaults
            },
            RetentionQuota {
                per_session_bytes: defaults.global_bytes + 1,
                ..defaults
            },
            RetentionQuota {
                checkpoint_generations: 1,
                ..defaults
            },
        ] {
            assert!(plan_retention(root.path(), quota, &[]).is_err());
        }
        let equality = RetentionQuota {
            global_bytes: 1,
            per_session_bytes: 1,
            checkpoint_generations: 2,
        };
        assert!(
            plan_retention(root.path(), equality, &[])
                .unwrap()
                .actions
                .is_empty()
        );
    }

    #[test]
    fn rejects_ambiguous_indexes() {
        let root = tempfile::tempdir().unwrap();
        let row = IndexedCheckpoint {
            generation: 2,
            created_at_ms: 0,
        };
        let valid = IndexedSession {
            id: "session",
            retention: SessionRetention::Inactive,
            checkpoints: &[row],
        };
        let quota = RetentionQuota::default();
        assert!(plan_retention(root.path(), quota, &[valid, valid]).is_err());
        for id in [
            "../outside",
            "sub/session",
            "./session",
            "session/",
            "..",
            "",
        ] {
            assert!(plan_retention(root.path(), quota, &[IndexedSession { id, ..valid }]).is_err());
        }
        assert!(
            plan_retention(
                root.path(),
                quota,
                &[IndexedSession {
                    checkpoints: &[row, row],
                    ..valid
                }]
            )
            .is_err()
        );
    }
}
