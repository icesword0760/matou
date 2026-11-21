//! Global defaults shared with the dispatch script.

use std::{
    collections::HashMap,
    fmt,
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

use serde::{Serialize, Serializer};
use serde_json::{Value, value::RawValue};
use uuid::Uuid;

pub mod form;

const CONFIG_RELATIVE_PATH: &str = ".claude/agent-foreman/config.json";
const AGENT_IDS: [&str; 3] = ["codex", "agy", "dsh"];
const EFFORT_IDS: [&str; 4] = ["low", "medium", "high", "xhigh"];

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DispatchDefaults {
    pub agent: String,
    pub model: String,
    pub effort: String,
    #[serde(serialize_with = "serialize_minutes")]
    pub idle_min: f64,
    #[serde(serialize_with = "serialize_minutes")]
    pub max_min: f64,
    #[serde(serialize_with = "serialize_minutes")]
    pub ask_wait_min: f64,
    pub retries: u8,
}

impl Default for DispatchDefaults {
    fn default() -> Self {
        Self {
            agent: "codex".into(),
            model: "sol".into(),
            effort: "high".into(),
            idle_min: 10.0,
            max_min: 45.0,
            ask_wait_min: 10.0,
            retries: 3,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DispatchConfigSnapshot {
    pub exists: bool,
    pub path: PathBuf,
    pub defaults: DispatchDefaults,
}

#[derive(Debug)]
pub enum DispatchConfigError {
    InvalidField(&'static str),
    Io(io::Error),
}

impl fmt::Display for DispatchConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidField(field) => write!(f, "Invalid dispatch config field: {field}"),
            Self::Io(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for DispatchConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::InvalidField(_) => None,
        }
    }
}

impl From<io::Error> for DispatchConfigError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Supply the home directory explicitly so reads and writes can also run in isolation.
pub struct DispatchConfig {
    path: PathBuf,
}

impl DispatchConfig {
    pub fn for_home(home: impl AsRef<Path>) -> Self {
        Self {
            path: home.as_ref().join(CONFIG_RELATIVE_PATH),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read fresh on every call. Missing or malformed configuration has no side effects.
    pub fn snapshot(&self) -> io::Result<DispatchConfigSnapshot> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(self.make_snapshot(false, DispatchDefaults::default()));
            }
            Err(error) => return Err(error),
        };
        let content = String::from_utf8_lossy(&bytes);
        let Some(file) = serde_json::from_str::<HashMap<String, &RawValue>>(&content)
            .ok()
            .filter(|file| raw_value(file.get("version").copied()).as_f64() == Some(1.0))
        else {
            return Ok(self.make_snapshot(false, DispatchDefaults::default()));
        };
        let fields: HashMap<String, &RawValue> = file
            .get("defaults")
            .and_then(|defaults| serde_json::from_str(defaults.get()).ok())
            .unwrap_or_default();
        let defaults = read_defaults(|key| raw_value(fields.get(key).copied()), false)
            .expect("fallback supplies each invalid field");
        Ok(self.make_snapshot(true, defaults))
    }

    /// Validate a complete set of defaults. Extra fields carry no configuration.
    pub fn save(&self, input: &Value) -> Result<DispatchConfigSnapshot, DispatchConfigError> {
        if !input.is_object() {
            return Err(DispatchConfigError::InvalidField("defaults"));
        }
        let defaults = read_defaults(|key| input[key].clone(), true)?;
        self.publish(defaults, |from, to| fs::rename(from, to))
    }

    /// Save values prepared by the native settings form.
    pub fn save_defaults(
        &self,
        defaults: DispatchDefaults,
    ) -> Result<DispatchConfigSnapshot, DispatchConfigError> {
        let input = serde_json::to_value(defaults).map_err(io::Error::from)?;
        self.save(&input)
    }

    fn make_snapshot(&self, exists: bool, defaults: DispatchDefaults) -> DispatchConfigSnapshot {
        DispatchConfigSnapshot {
            exists,
            path: self.path.clone(),
            defaults,
        }
    }

    fn publish(
        &self,
        defaults: DispatchDefaults,
        replace: impl FnOnce(&Path, &Path) -> io::Result<()>,
    ) -> Result<DispatchConfigSnapshot, DispatchConfigError> {
        let mut bytes = serde_json::to_vec_pretty(&SavedFile {
            version: 1,
            defaults: &defaults,
        })
        .map_err(io::Error::from)?;
        bytes.push(b'\n');

        let parent = self.path.parent().expect("config path has a parent");
        fs::create_dir_all(parent)?;
        let temporary = parent.join(format!("config.json.{}.tmp", Uuid::new_v4()));
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        let written = file.write_all(&bytes);
        drop(file);
        let result = written.and_then(|()| replace(&temporary, &self.path));
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result?;
        Ok(self.make_snapshot(true, defaults))
    }
}

#[derive(Serialize)]
struct SavedFile<'a> {
    version: u8,
    defaults: &'a DispatchDefaults,
}

fn raw_value(raw: Option<&RawValue>) -> Value {
    // An overflowing number invalidates only its field, like JSON.parse
    // followed by a finite-number check.
    raw.and_then(|raw| serde_json::from_str(raw.get()).ok())
        .unwrap_or(Value::Null)
}

fn read_defaults(
    mut get: impl FnMut(&str) -> Value,
    strict: bool,
) -> Result<DispatchDefaults, DispatchConfigError> {
    let fallback = DispatchDefaults::default();
    macro_rules! field {
        ($key:literal, $read:expr, $fallback:expr) => {
            match $read(&get($key)) {
                Some(value) => value,
                None if strict => return Err(DispatchConfigError::InvalidField($key)),
                None => $fallback,
            }
        };
    }
    Ok(DispatchDefaults {
        agent: field!("agent", |v| read_choice(v, &AGENT_IDS), fallback.agent),
        model: field!("model", read_model, fallback.model),
        effort: field!("effort", |v| read_choice(v, &EFFORT_IDS), fallback.effort),
        idle_min: field!("idleMin", read_minutes, fallback.idle_min),
        max_min: field!("maxMin", read_minutes, fallback.max_min),
        ask_wait_min: field!("askWaitMin", read_minutes, fallback.ask_wait_min),
        retries: field!("retries", read_retries, fallback.retries),
    })
}

fn read_choice(value: &Value, allowed: &[&str]) -> Option<String> {
    value
        .as_str()
        .filter(|value| allowed.contains(value))
        .map(str::to_owned)
}

fn read_model(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|value| {
            (1..=64).contains(&value.encode_utf16().count()) && !value.chars().any(js_whitespace)
        })
        .map(str::to_owned)
}

fn read_minutes(value: &Value) -> Option<f64> {
    value.as_f64().filter(|n| *n > 0.0 && *n <= 600.0)
}

fn read_retries(value: &Value) -> Option<u8> {
    value
        .as_f64()
        .and_then(|n| ((0.0..=5.0).contains(&n) && n.fract() == 0.0).then_some(n as u8))
}

fn serialize_minutes<S: Serializer>(number: &f64, serializer: S) -> Result<S::Ok, S::Error> {
    if (0.0..=600.0).contains(number) && number.fract() == 0.0 {
        serializer.serialize_u16(*number as u16)
    } else {
        serializer.serialize_f64(*number)
    }
}

fn js_whitespace(c: char) -> bool {
    c.is_whitespace() && c != '\u{0085}' || c == '\u{feff}'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_replacement_keeps_original_bytes_and_cleans_the_staged_file() -> anyhow::Result<()> {
        let home = tempfile::tempdir_in(std::env::current_dir()?)?;
        let store = DispatchConfig::for_home(home.path());
        let original = DispatchDefaults::default();
        store.save_defaults(original.clone())?;
        let before = fs::read(store.path())?;
        let edited = DispatchDefaults {
            model: "luna".into(),
            ..original.clone()
        };
        let result = store.publish(edited.clone(), |temporary, target| {
            assert_eq!(target, store.path());
            assert_eq!(fs::read(target)?, before);
            assert!(fs::read_to_string(temporary)?.contains("\"luna\""));
            Err(io::Error::other("fixture rename failed"))
        });
        assert!(matches!(result, Err(DispatchConfigError::Io(_))));
        assert_eq!(fs::read(store.path())?, before);
        assert_eq!(fs::read_dir(store.path().parent().unwrap())?.count(), 1);
        assert_eq!(store.snapshot()?.defaults, original);
        assert_eq!(store.save_defaults(edited.clone())?.defaults, edited);
        Ok(())
    }
}
