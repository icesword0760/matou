//! The recovery marker, durable claim and resolution records used by startup.
use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::ownership::{sync_directory, with_recovery_action_fence};

const MARKER_SUFFIX: &str = ".recovery.json";
const INVALID_MARKER: &str = "database recovery marker is invalid";
const INVALID_CLAIM: &str = "database recovery generation claim is invalid";
const REQUIRED: &str = "required";
const RESOLVED: &str = "resolved";
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryMarker {
    pub version: u8,
    pub state: String,
    pub recovery_id: String,
    pub reason: String,
    pub durable_database_path: PathBuf,
    pub quarantined_path: PathBuf,
    pub marker_path: PathBuf,
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ownership_issue: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRecovery {
    pub marker: RecoveryMarker,
    pub marker_error: Option<String>,
}

// Retain both the parsed document (including unknown fields and property order)
// and the exact decoded bytes used as evidence of the previous generation.
struct MarkerDocument {
    marker: RecoveryMarker,
    value: Value,
    bytes: String,
}

impl MarkerDocument {
    fn is_legacy(&self) -> bool {
        self.marker.state.is_empty()
    }

    fn upgrade(mut self) -> Self {
        let marker = &mut self.marker;
        if marker.recovery_id.is_empty() {
            marker.recovery_id = Uuid::new_v4().to_string();
        }
        marker.state = REQUIRED.to_owned();
        self.value["recoveryId"] = json!(marker.recovery_id);
        self.value["state"] = json!(marker.state);
        self
    }
}

#[derive(Serialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
enum GenerationSource {
    Initial,
    Legacy {
        marker_digest: String,
        marker_bytes: String,
    },
    Resolved {
        recovery_id: String,
        marker_digest: String,
        marker_bytes: String,
    },
}

#[derive(Debug)]
struct PublicationError {
    durable_marker: RecoveryMarker,
    cause: anyhow::Error,
}

impl fmt::Display for PublicationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "database recovery generation is durable but canonical publication failed: {}",
            self.cause.root_cause()
        )
    }
}

impl std::error::Error for PublicationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.cause.as_ref())
    }
}

pub fn load_pending(database_path: &Path) -> Result<Option<PendingRecovery>> {
    let initial = read_shape(database_path)?;
    if let Some(claimed) = pending_claim(database_path, initial.as_ref())? {
        let result =
            replace_marker(&claimed).and_then(|()| require_canonical_generation(&claimed.marker));
        return Ok(Some(match result {
            Ok(marker) => PendingRecovery {
                marker,
                marker_error: None,
            },
            Err(error) => PendingRecovery {
                marker: claimed.marker,
                marker_error: Some(error.to_string()),
            },
        }));
    }

    let active = match initial {
        Some(shape) if shape.is_legacy() => upgrade_legacy(database_path)?.and_then(active_marker),
        Some(shape) => active_marker(shape.marker),
        None => None,
    };
    Ok(active.map(|marker| PendingRecovery {
        marker,
        marker_error: None,
    }))
}

pub fn read_active(database_path: &Path) -> Result<Option<RecoveryMarker>> {
    Ok(read_canonical(database_path)?.and_then(active_marker))
}

pub fn publish(marker: RecoveryMarker) -> Result<RecoveryMarker> {
    let database_path = marker.durable_database_path.clone();
    let proposed = parse_shape(serde_json::to_string(&marker)?, &database_path)?;
    ensure!(!proposed.is_legacy(), "{INVALID_MARKER}");
    match with_recovery_action_fence(&database_path, || {
        if let Some(active) = read_canonical_locked(&database_path)?.and_then(active_marker) {
            return Ok(active);
        }
        replace_marker(&proposed)?;
        require_canonical_generation(&proposed.marker)
    }) {
        Ok(marker) => Ok(marker),
        Err(fence_error) => publish_without_fence(proposed, fence_error),
    }
}

fn publish_without_fence(
    proposed: MarkerDocument,
    fence_error: anyhow::Error,
) -> Result<RecoveryMarker> {
    let shape = read_shape(&proposed.marker.durable_database_path)?;
    if let Some(document) = &shape
        && !document.is_legacy()
        && is_active(&document.marker)
    {
        return Ok(document.marker.clone());
    }
    let Some(source) = source_for_shape(shape.as_ref()) else {
        return Ok(shape.context("active recovery state has no marker")?.marker);
    };
    let proposed = match shape {
        Some(document) if document.is_legacy() => document.upgrade(),
        _ => proposed,
    };
    publish_claimed(proposed, &source, fence_error)
}

fn publish_claimed(
    proposed: MarkerDocument,
    source: &GenerationSource,
    fence_error: anyhow::Error,
) -> Result<RecoveryMarker> {
    let database_path = &proposed.marker.durable_database_path;
    let marker_path = &proposed.marker.marker_path;
    let claim_path = claim_path(marker_path, source)?;
    let claim = json!({
        "version": 1,
        "kind": "recovery-generation-claim",
        "source": serde_json::to_value(source)?,
        "markerDigest": marker_digest(&proposed.value)?,
        "marker": proposed.value,
    });
    let partial = PartialFile::prepare(&claim_path, &claim, || Ok(()))?;
    match fs::hard_link(&partial.0, &claim_path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    // Both publishers and observers of the winning hard link must establish
    // its directory durability before using the winning generation.
    sync_directory(parent(&claim_path)?)?;
    read_claim(&claim_path, database_path, source)?;
    drop(partial);

    let claimed = read_claim(&claim_path, database_path, source)?;
    if let Err(error) = replace_marker(&claimed) {
        if let Some(canonical) = matching_generation(&claimed.marker)? {
            return Ok(canonical);
        }
        return Err(PublicationError {
            durable_marker: read_claim(&claim_path, database_path, source)?.marker,
            cause: error.context(format!(
                "canonical recovery publication failed after action fence failed: {fence_error:#}"
            )),
        }
        .into());
    }
    require_canonical_generation(&claimed.marker)
}

fn claim_path(marker_path: &Path, source: &GenerationSource) -> Result<PathBuf> {
    let digest = digest(stringify(&serde_json::to_value(source)?)?.as_bytes());
    Ok(suffix(marker_path, &format!(".generation-{digest}.claim")))
}

fn pending_claim(
    database_path: &Path,
    shape: Option<&MarkerDocument>,
) -> Result<Option<MarkerDocument>> {
    let Some(source) = source_for_shape(shape) else {
        return Ok(None);
    };
    let path = claim_path(&suffix(database_path, MARKER_SUFFIX), &source)?;
    optional_read(read_claim(&path, database_path, &source))
}

fn source_for_shape(shape: Option<&MarkerDocument>) -> Option<GenerationSource> {
    let Some(shape) = shape else {
        return Some(GenerationSource::Initial);
    };
    let marker_digest = digest(shape.bytes.as_bytes());
    if shape.is_legacy() {
        return Some(GenerationSource::Legacy {
            marker_digest,
            marker_bytes: shape.bytes.clone(),
        });
    }
    if is_active(&shape.marker) {
        return None;
    }
    Some(GenerationSource::Resolved {
        recovery_id: shape.marker.recovery_id.clone(),
        marker_digest,
        marker_bytes: shape.bytes.clone(),
    })
}

fn read_claim(
    path: &Path,
    database_path: &Path,
    source: &GenerationSource,
) -> Result<MarkerDocument> {
    let bytes = match read_utf8(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(error.into()),
        Err(_) => return Err(anyhow!(INVALID_CLAIM)),
    };
    let claim: Value = serde_json::from_str(&bytes).map_err(|_| anyhow!(INVALID_CLAIM))?;
    ensure!(
        claim.is_object()
            && number(claim.get("version")) == Some(1)
            && claim["kind"] == "recovery-generation-claim"
            && stringify(&claim["source"])? == stringify(&serde_json::to_value(source)?)?
            && claim["marker"].is_object(),
        "{INVALID_CLAIM}"
    );
    // Validate the digest against the original parsed Value, without projecting
    // to the Rust struct and reordering or dropping its fields.
    let marker = parse_shape(stringify(&claim["marker"])?, database_path)?;
    ensure!(
        !marker.is_legacy()
            && marker.marker.state == REQUIRED
            && claim["markerDigest"].as_str() == Some(marker_digest(&marker.value)?.as_str()),
        "{INVALID_CLAIM}"
    );
    if let GenerationSource::Resolved { recovery_id, .. } = source {
        ensure!(&marker.marker.recovery_id != recovery_id, "{INVALID_CLAIM}");
    }
    verify_source(source, &marker.marker, database_path)?;
    Ok(marker)
}

fn verify_source(
    source: &GenerationSource,
    claimed: &RecoveryMarker,
    database_path: &Path,
) -> Result<()> {
    let (bytes, hash) = match source {
        GenerationSource::Initial => return Ok(()),
        GenerationSource::Legacy {
            marker_bytes,
            marker_digest,
        }
        | GenerationSource::Resolved {
            marker_bytes,
            marker_digest,
            ..
        } => (marker_bytes, marker_digest),
    };
    ensure!(&digest(bytes.as_bytes()) == hash, "{INVALID_CLAIM}");
    let shape = parse_shape(bytes.clone(), database_path)?;
    let prior = &shape.marker;
    match source {
        GenerationSource::Initial => unreachable!("initial source has no bytes"),
        GenerationSource::Legacy { .. } => {
            ensure!(
                shape.is_legacy() && same_evidence(prior, claimed),
                "{INVALID_CLAIM}"
            );
        }
        GenerationSource::Resolved { recovery_id, .. } => ensure!(
            !shape.is_legacy()
                && &prior.recovery_id == recovery_id
                && (prior.state == RESOLVED || matching_tombstone(prior)),
            "{INVALID_CLAIM}"
        ),
    }
    Ok(())
}

fn same_evidence(legacy: &RecoveryMarker, claimed: &RecoveryMarker) -> bool {
    legacy.version == claimed.version
        && legacy.reason == claimed.reason
        && legacy.durable_database_path == claimed.durable_database_path
        && legacy.quarantined_path == claimed.quarantined_path
        && legacy.marker_path == claimed.marker_path
        && legacy.created_at == claimed.created_at
        && legacy.ownership_issue == claimed.ownership_issue
        && (legacy.recovery_id.is_empty() || legacy.recovery_id == claimed.recovery_id)
}

fn marker_digest(marker: &Value) -> Result<String> {
    Ok(digest(stringify(marker)?.as_bytes()))
}

/// Match compact JSON.stringify for hashes of source-generated records.
pub(crate) fn stringify(value: &Value) -> Result<String> {
    let mut output = String::new();
    append_json(value, &mut output)?;
    Ok(output)
}

fn append_json(value: &Value, output: &mut String) -> Result<()> {
    match value {
        Value::Object(object) => {
            // JavaScript visits array-index property names before other keys.
            // For all other keys, preserve the order in the parsed document.
            let mut indexes = object
                .keys()
                .filter_map(|key| array_index(key).map(|position| (position, key)))
                .collect::<Vec<_>>();
            indexes.sort_unstable_by_key(|(position, _)| *position);
            output.push('{');
            for (position, key) in indexes
                .into_iter()
                .map(|(_, key)| key)
                .chain(object.keys().filter(|key| array_index(key).is_none()))
                .enumerate()
            {
                if position != 0 {
                    output.push(',');
                }
                output.push_str(&serde_json::to_string(key)?);
                output.push(':');
                append_json(&object[key], output)?;
            }
            output.push('}');
        }
        Value::Array(array) => {
            output.push('[');
            for (position, item) in array.iter().enumerate() {
                if position != 0 {
                    output.push(',');
                }
                append_json(item, output)?;
            }
            output.push(']');
        }
        Value::Number(number) => {
            let number = number.as_f64().context("invalid recovery JSON number")?;
            if number == 0.0 {
                output.push('0');
            } else if (1e-6..1e21).contains(&number.abs()) {
                output.push_str(&number.to_string());
            } else {
                let exponent = format!("{number:e}");
                if let Some((significand, power)) = exponent.split_once('e') {
                    output.push_str(significand);
                    output.push('e');
                    if !power.starts_with('-') {
                        output.push('+');
                    }
                    output.push_str(power);
                } else {
                    output.push_str(&exponent);
                }
            }
        }
        scalar => output.push_str(&serde_json::to_string(scalar)?),
    }
    Ok(())
}

fn array_index(key: &str) -> Option<u32> {
    if key.is_empty()
        || (key.len() > 1 && key.starts_with('0'))
        || !key.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    key.parse::<u32>().ok().filter(|&value| value < u32::MAX)
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn matching_generation(expected: &RecoveryMarker) -> Result<Option<RecoveryMarker>> {
    Ok(read_shape(&expected.durable_database_path)?
        .filter(|shape| !shape.is_legacy() && shape.marker.recovery_id == expected.recovery_id)
        .map(|shape| shape.marker))
}

fn require_canonical_generation(expected: &RecoveryMarker) -> Result<RecoveryMarker> {
    matching_generation(expected)?.context("database recovery generation was not durably published")
}

pub fn durable_after_failure(
    database_path: &Path,
    error: &anyhow::Error,
) -> Result<Option<RecoveryMarker>> {
    if let Some(active) = read_shape(database_path)?
        .filter(|shape| !shape.is_legacy())
        .and_then(|shape| active_marker(shape.marker))
    {
        return Ok(Some(active));
    }
    Ok(error
        .downcast_ref::<PublicationError>()
        .map(|publication| publication.durable_marker.clone()))
}

pub fn resolve(
    marker_path: &Path,
    recovery_id: &str,
    now: i64,
    state_changed_message: &str,
) -> Result<()> {
    resolve_with(marker_path, recovery_id, now, state_changed_message, |_| {
        Ok(())
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ResolveStage {
    BeforeFileSync,
    BeforePublish,
    AfterPublish,
    BeforeDirectorySync,
}

fn resolve_with(
    marker_path: &Path,
    recovery_id: &str,
    now: i64,
    state_changed_message: &str,
    mut observe: impl FnMut(ResolveStage) -> Result<()>,
) -> Result<()> {
    let database_path = database_path_from_marker(marker_path)?;
    let required = read_canonical(&database_path)?
        .filter(|marker| marker.recovery_id == recovery_id && is_active(marker))
        .ok_or_else(|| anyhow!(state_changed_message.to_owned()))?;
    let tombstone_path = tombstone_path(&required);
    let tombstone = json!({
        "version": 1,
        "state": "resolved",
        "recoveryId": recovery_id,
        "markerPath": marker_path,
        "resolvedAt": now,
    });
    let partial = PartialFile::prepare(&tombstone_path, &tombstone, || {
        observe(ResolveStage::BeforeFileSync)
    })?;
    observe(ResolveStage::BeforePublish)?;
    match fs::hard_link(&partial.0, &tombstone_path) {
        Ok(()) => {}
        Err(error)
            if error.kind() == io::ErrorKind::AlreadyExists && matching_tombstone(&required) => {}
        Err(error) => return Err(error.into()),
    }
    // The hard link is the commit point. A later sync failure can at most
    // reopen the same generation after a crash; the required marker is intact.
    let _ = observe(ResolveStage::AfterPublish);
    if observe(ResolveStage::BeforeDirectorySync).is_ok() {
        let _ = parent(&tombstone_path).and_then(|path| Ok(sync_directory(path)?));
    }
    Ok(())
}

pub fn is_resolved(marker_path: &Path, recovery_id: &str) -> Result<bool> {
    let database_path = database_path_from_marker(marker_path)?;
    Ok(read_canonical(&database_path)?.is_some_and(|marker| {
        marker.recovery_id == recovery_id
            && (marker.state == RESOLVED || matching_tombstone(&marker))
    }))
}

fn database_path_from_marker(path: &Path) -> Result<PathBuf> {
    Ok(PathBuf::from(
        path.to_str()
            .context("invalid database recovery marker path")?
            .strip_suffix(MARKER_SUFFIX)
            .unwrap_or_default(),
    ))
}

fn tombstone_path(marker: &RecoveryMarker) -> PathBuf {
    suffix(
        &marker.marker_path,
        &format!(".resolved-{}", marker.recovery_id),
    )
}

fn matching_tombstone(marker: &RecoveryMarker) -> bool {
    let Ok(bytes) = read_utf8(&tombstone_path(marker)) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<Value>(&bytes) else {
        return false;
    };
    number(value.get("version")) == Some(1)
        && value["state"] == RESOLVED
        && value["recoveryId"].as_str() == Some(marker.recovery_id.as_str())
        && marker
            .marker_path
            .to_str()
            .is_some_and(|path| value["markerPath"] == path)
        && number(value.get("resolvedAt")).is_some()
}

fn is_active(marker: &RecoveryMarker) -> bool {
    marker.state == REQUIRED && !matching_tombstone(marker)
}

fn active_marker(marker: RecoveryMarker) -> Option<RecoveryMarker> {
    is_active(&marker).then_some(marker)
}

fn read_canonical(database_path: &Path) -> Result<Option<RecoveryMarker>> {
    match read_shape(database_path)? {
        Some(shape) if shape.is_legacy() => upgrade_legacy(database_path),
        Some(shape) => Ok(Some(shape.marker)),
        None => Ok(None),
    }
}

fn upgrade_legacy(database_path: &Path) -> Result<Option<RecoveryMarker>> {
    let error =
        match with_recovery_action_fence(database_path, || read_canonical_locked(database_path)) {
            Ok(marker) => return Ok(marker),
            Err(error) => error,
        };
    let Some(shape) = read_shape(database_path)? else {
        return Ok(None);
    };
    if !shape.is_legacy() {
        return Ok(Some(shape.marker));
    }
    let source = source_for_shape(Some(&shape)).expect("legacy source has generation evidence");
    match publish_claimed(shape.upgrade(), &source, error) {
        Ok(marker) => Ok(Some(marker)),
        Err(error) => match error.downcast::<PublicationError>() {
            Ok(publication) => Ok(Some(publication.durable_marker)),
            Err(error) => Err(error),
        },
    }
}

fn read_canonical_locked(database_path: &Path) -> Result<Option<RecoveryMarker>> {
    let Some(shape) = read_shape(database_path)? else {
        return Ok(None);
    };
    if !shape.is_legacy() {
        return Ok(Some(shape.marker));
    }
    let upgraded = shape.upgrade();
    replace_marker(&upgraded)?;
    Ok(Some(require_canonical_generation(&upgraded.marker)?))
}

fn read_shape(database_path: &Path) -> Result<Option<MarkerDocument>> {
    let bytes = optional_read(
        read_utf8(&suffix(database_path, MARKER_SUFFIX)).map_err(anyhow::Error::from),
    )?;
    bytes
        .map(|bytes| parse_shape(bytes, database_path))
        .transpose()
}

fn parse_shape(bytes: String, database_path: &Path) -> Result<MarkerDocument> {
    let value: Value = serde_json::from_str(&bytes).map_err(|_| anyhow!(INVALID_MARKER))?;
    let object = value.as_object().context(INVALID_MARKER)?;
    let string = |name| {
        object
            .get(name)
            .and_then(Value::as_str)
            .context(INVALID_MARKER)
    };
    ensure!(number(object.get("version")) == Some(1), "{INVALID_MARKER}");
    let id = if object.contains_key("recoveryId") {
        let id = string("recoveryId")?;
        ensure!(valid_recovery_id(id), "{INVALID_MARKER}");
        id
    } else {
        ""
    };
    let state = if object.contains_key("state") {
        let state = string("state")?;
        ensure!(
            !id.is_empty() && matches!(state, REQUIRED | RESOLVED),
            "{INVALID_MARKER}"
        );
        state
    } else {
        ""
    };
    let reason = string("reason")?;
    ensure!(
        matches!(
            reason,
            "physical-corruption" | "wal-recovery-required" | "ownership-recovery-required"
        ),
        "{INVALID_MARKER}"
    );
    let durable_path = lexical_absolute(database_path)?;
    let marker_path = suffix(database_path, MARKER_SUFFIX);
    ensure!(
        durable_path.to_str() == Some(string("durableDatabasePath")?)
            && marker_path.to_str() == Some(string("markerPath")?),
        "{INVALID_MARKER}"
    );
    let quarantine = string("quarantinedPath")?;
    let quarantined_path = PathBuf::from(quarantine);
    ensure!(
        quarantined_path.parent() == durable_path.parent(),
        "{INVALID_MARKER}"
    );
    let ownership_issue = if reason == "ownership-recovery-required" {
        let issue = string("ownershipIssue")?;
        ensure!(
            durable_path.to_str() == Some(quarantine)
                && matches!(
                    issue,
                    "owner-record-malformed" | "takeover-sidecar-unusable"
                ),
            "{INVALID_MARKER}"
        );
        Some(issue.to_owned())
    } else {
        let durable = durable_path.to_str().context(INVALID_MARKER)?;
        ensure!(
            quarantine
                .strip_prefix(durable)
                .is_some_and(|suffix| suffix.starts_with(".corrupt-"))
                && !object.contains_key("ownershipIssue"),
            "{INVALID_MARKER}"
        );
        None
    };
    let created_at = number(object.get("createdAt")).context(INVALID_MARKER)?;
    let marker = RecoveryMarker {
        version: 1,
        state: state.to_owned(),
        recovery_id: id.to_owned(),
        reason: reason.to_owned(),
        durable_database_path: durable_path,
        quarantined_path,
        marker_path,
        created_at,
        ownership_issue,
    };
    Ok(MarkerDocument {
        marker,
        value,
        bytes,
    })
}

fn valid_recovery_id(id: &str) -> bool {
    let mut bytes = id.bytes();
    id.len() <= 160
        && bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn number(value: Option<&Value>) -> Option<i64> {
    let number = value?.as_f64()?;
    ((-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER).contains(&number) && number.fract() == 0.0)
        .then_some(number as i64)
}

fn lexical_absolute(path: &Path) -> io::Result<PathBuf> {
    let path = if path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        path
    };
    let mut absolute = PathBuf::new();
    for component in std::path::absolute(path)?.components() {
        if component == Component::ParentDir {
            absolute.pop();
        } else {
            absolute.push(component);
        }
    }
    Ok(absolute)
}

fn replace_marker(document: &MarkerDocument) -> Result<()> {
    let path = &document.marker.marker_path;
    let partial = PartialFile::prepare(path, &document.value, || Ok(()))?;
    fs::rename(&partial.0, path)?;
    sync_directory(parent(path)?)?;
    Ok(())
}

fn optional_read<T>(result: Result<T>) -> Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error)
            if error
                .downcast_ref::<io::Error>()
                .is_some_and(|error| error.kind() == io::ErrorKind::NotFound) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn read_utf8(path: &Path) -> io::Result<String> {
    Ok(String::from_utf8_lossy(&fs::read(path)?).into_owned())
}

fn suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    name.into()
}

fn parent(path: &Path) -> Result<&Path> {
    let directory = path
        .parent()
        .context("recovery state has no parent directory")?;
    Ok(if directory.as_os_str().is_empty() {
        Path::new(".")
    } else {
        directory
    })
}

struct PartialFile(PathBuf);

impl PartialFile {
    fn prepare(
        path: &Path,
        value: &Value,
        before_sync: impl FnOnce() -> Result<()>,
    ) -> Result<Self> {
        let temporary = suffix(path, &format!(".partial-{}", Uuid::new_v4()));
        let mut opts = OpenOptions::new();
        opts.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let file: File = opts.open(&temporary)?;
        let partial = Self(temporary);
        let mut file = file;
        file.write_all(stringify(value)?.as_bytes())?;
        before_sync()?;
        file.sync_all()?;
        Ok(partial)
    }
}

impl Drop for PartialFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hard_link_is_the_resolution_commit_point() -> Result<()> {
        for stage in [
            ResolveStage::BeforeFileSync,
            ResolveStage::BeforePublish,
            ResolveStage::AfterPublish,
            ResolveStage::BeforeDirectorySync,
        ] {
            let root = tempfile::tempdir()?;
            let database_path = root.path().join("matou.sqlite");
            let marker = RecoveryMarker {
                version: 1,
                state: REQUIRED.to_owned(),
                recovery_id: "frozen-generation".into(),
                reason: "physical-corruption".into(),
                quarantined_path: suffix(&database_path, ".corrupt-1"),
                marker_path: suffix(&database_path, MARKER_SUFFIX),
                durable_database_path: database_path,
                created_at: 1,
                ownership_issue: None,
            };
            let published = publish(marker)?;
            let canonical = fs::read(&published.marker_path)?;
            let result = resolve_with(
                &published.marker_path,
                &published.recovery_id,
                2,
                "state changed",
                |current| {
                    ensure!(current != stage, "injected resolution fault");
                    Ok(())
                },
            );
            let committed = matches!(
                stage,
                ResolveStage::AfterPublish | ResolveStage::BeforeDirectorySync
            );
            assert_eq!(result.is_ok(), committed);
            assert_eq!(
                is_resolved(&published.marker_path, &published.recovery_id)?,
                committed
            );
            assert_eq!(fs::read(&published.marker_path)?, canonical);
            assert!(!fs::read_dir(root.path())?.any(|file| {
                file.is_ok_and(|file| file.file_name().to_string_lossy().contains(".partial-"))
            }));
        }
        Ok(())
    }
}
