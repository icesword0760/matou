//! Read Claude Code's project transcripts without modifying the provider's catalog.
use std::{
    collections::{BTreeSet, HashMap},
    fs::{self, File},
    io::{self, BufRead, BufReader, Read},
    path::{Component, Path, PathBuf},
};

use rusqlite::{Connection, Statement};
use serde::Serialize;
use serde_json::Value;

const LIST_LIMIT: usize = 200;
const MAX_LINE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CatalogEntry {
    pub provider_session_id: String,
    pub title: String,
    pub updated_at: i64,
    pub model: Option<String>,
    pub permission_mode: String,
    pub message_count: usize,
    pub loaded_by: Option<Value>,
}

#[derive(Default)]
struct Skipped {
    outside_root: usize,
    malformed_lines: usize,
    oversized_lines: usize,
    unreadable_files: usize,
}

pub(crate) fn projects_root() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::home_dir().map(|home| home.join(".claude")))
        .map(|config| config.join("projects"))
}

pub(crate) fn read(
    root: Option<&Path>,
    cwd: &str,
    selected: Option<&str>,
) -> Result<Vec<CatalogEntry>, &'static str> {
    if selected.is_some_and(|id| !valid_id(id)) {
        return Err("Invalid provider session ID");
    }
    let Some(root) = root else {
        return Err("Claude Code configuration directory is unavailable");
    };
    let cwd =
        absolute(Path::new(cwd)).map_err(|_| "Could not resolve the catalog working directory")?;
    let workspace = match fs::canonicalize(&cwd) {
        Ok(path) if path.is_dir() => path,
        Ok(_) => return Ok(Vec::new()),
        Err(error) if missing(&error) => return Ok(Vec::new()),
        Err(_) => return Err("Could not resolve the catalog working directory"),
    };
    let root = match fs::canonicalize(root) {
        Ok(path) => path,
        Err(error) if missing(&error) => return Ok(Vec::new()),
        Err(_) => return Err("Could not resolve the Claude Code projects directory"),
    };
    let mut skipped = Skipped::default();
    let result = scan(&root, &cwd, &workspace, selected, &mut skipped);
    if skipped.outside_root
        + skipped.malformed_lines
        + skipped.oversized_lines
        + skipped.unreadable_files
        > 0
    {
        eprintln!(
            "[provider-catalog] skipped: outside_root={}, malformed_lines={}, oversized_lines={}, unreadable_files={}",
            skipped.outside_root,
            skipped.malformed_lines,
            skipped.oversized_lines,
            skipped.unreadable_files
        );
    }
    result
}

fn scan(
    root: &Path,
    cwd: &Path,
    workspace: &Path,
    selected: Option<&str>,
    skipped: &mut Skipped,
) -> Result<Vec<CatalogEntry>, &'static str> {
    let names = [encode(cwd), encode(&display_path(workspace))];
    let mut visited = BTreeSet::new();
    let mut entries = HashMap::new();
    // SQLite is already linked. Use its ISO-8601 parser for the CLI's timestamp
    // strings, including offsets, while preserving numeric millisecond values.
    let dates = Connection::open_in_memory()
        .map_err(|_| "Could not initialize catalog timestamp parsing")?;
    let mut timestamp = dates
        .prepare("SELECT ROUND(unixepoch(?1, 'subsec') * 1000)")
        .map_err(|_| "Could not initialize catalog timestamp parsing")?;
    for name in names {
        let directory = match contained(root, &root.join(name), skipped) {
            Ok(Some(directory)) if directory.is_dir() && visited.insert(directory.clone()) => {
                directory
            }
            Ok(_) => continue,
            Err(_) => return Err("Could not inspect a Claude Code project directory"),
        };
        let candidates = if let Some(id) = selected {
            vec![directory.join(format!("{id}.jsonl"))]
        } else {
            fs::read_dir(&directory)
                .and_then(|files| {
                    files
                        .map(|file| file.map(|file| file.path()))
                        .collect::<io::Result<Vec<_>>>()
                })
                .map_err(|_| "Could not list a Claude Code project directory")?
        };
        for candidate in candidates {
            let Some(id) = candidate
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_suffix(".jsonl"))
                .filter(|id| valid_id(id))
            else {
                continue;
            };
            let entry = (|| {
                let Some(path) = contained(root, &candidate, skipped)? else {
                    return Ok(None);
                };
                if !fs::metadata(&path)?.is_file() {
                    return Ok(None);
                }
                let file = File::open(&path)?;
                // Do not read after the candidate has been replaced or redirected.
                if contained(root, &candidate, skipped)?.as_ref() != Some(&path)
                    || !file.metadata()?.is_file()
                {
                    skipped.unreadable_files += 1;
                    return Ok(None);
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    let opened = file.metadata()?;
                    let checked = fs::metadata(&path)?;
                    if (opened.dev(), opened.ino()) != (checked.dev(), checked.ino()) {
                        skipped.outside_root += 1;
                        return Ok(None);
                    }
                }
                index(file, id, workspace, &mut timestamp, skipped)
            })();
            match entry {
                Ok(Some(entry)) => {
                    entries.insert(entry.provider_session_id.clone(), entry);
                }
                Ok(None) => {}
                Err(error) if missing(&error) => {}
                Err(_) => skipped.unreadable_files += 1,
            }
        }
    }
    let mut entries: Vec<_> = entries.into_values().collect();
    entries.sort_unstable_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.title.cmp(&right.title))
            .then_with(|| left.provider_session_id.cmp(&right.provider_session_id))
    });
    entries.truncate(LIST_LIMIT);
    Ok(entries)
}

fn contained(root: &Path, path: &Path, skipped: &mut Skipped) -> io::Result<Option<PathBuf>> {
    match fs::canonicalize(path) {
        Ok(path) if path.starts_with(root) => Ok(Some(path)),
        Ok(_) => {
            skipped.outside_root += 1;
            Ok(None)
        }
        Err(error) if missing(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

fn missing(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

fn absolute(path: &Path) -> io::Result<PathBuf> {
    let mut resolved = PathBuf::new();
    // Normalize dot components before realpath, as path.resolve does.
    for component in std::path::absolute(path)?.components() {
        if component == Component::ParentDir {
            resolved.pop();
        } else {
            resolved.push(component);
        }
    }
    Ok(resolved)
}

#[cfg(windows)]
fn display_path(path: &Path) -> PathBuf {
    // realpath emits ordinary drive and UNC paths for the project encoder.
    let path = path.to_string_lossy();
    if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{unc}"))
    } else {
        PathBuf::from(path.strip_prefix(r"\\?\").unwrap_or(&path))
    }
}

#[cfg(not(windows))]
fn display_path(path: &Path) -> PathBuf {
    path.to_owned()
}

fn encode(absolute: &Path) -> String {
    absolute
        .to_string_lossy()
        .encode_utf16()
        .map(|unit| match unit {
            48..=57 | 65..=90 | 97..=122 => char::from(unit as u8),
            _ => '-',
        })
        .collect()
}

fn valid_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    (1..=160).contains(&bytes.len())
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..]
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(byte))
}

fn index(
    file: File,
    id: &str,
    workspace: &Path,
    timestamp: &mut Statement<'_>,
    skipped: &mut Skipped,
) -> io::Result<Option<CatalogEntry>> {
    let mut reader = BufReader::new(file);
    let mut line = Vec::new();
    let mut origin_found = false;
    let mut title = None;
    let mut auto_title = None;
    let mut first_prompt = None;
    let mut entry = CatalogEntry {
        provider_session_id: id.to_owned(),
        title: String::new(),
        updated_at: 0,
        model: None,
        permission_mode: "default".into(),
        message_count: 0,
        loaded_by: None,
    };
    loop {
        line.clear();
        if reader
            .by_ref()
            .take(MAX_LINE_BYTES + 1)
            .read_until(b'\n', &mut line)?
            == 0
        {
            break;
        }
        if line.len() as u64 > MAX_LINE_BYTES {
            skipped.oversized_lines += 1;
            if !line.ends_with(b"\n") {
                reader.skip_until(b'\n')?;
            }
            continue;
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        let Ok(row) = serde_json::from_slice::<Value>(&line) else {
            skipped.malformed_lines += 1;
            continue;
        };
        // The first cwd is the conversation's origin even after later cd commands.
        if !origin_found && let Some(origin) = nonempty(&row["cwd"]) {
            let origin = absolute(Path::new(origin))?;
            if fs::canonicalize(&origin).unwrap_or(origin) != workspace {
                return Ok(None);
            }
            origin_found = true;
        }
        if let Some(custom) = nonempty(&row["customTitle"]) {
            title = Some(custom.to_owned());
        }
        if row["type"] == "ai-title"
            && let Some(auto) = nonempty(&row["aiTitle"])
        {
            auto_title = Some(auto.to_owned());
        }
        let date = &row["timestamp"];
        let millis = date.as_f64().map(|millis| millis as i64).or_else(|| {
            timestamp
                .query_row([date.as_str()?], |row| row.get::<_, Option<f64>>(0))
                .ok()
                .flatten()
                .map(|millis| millis as i64)
        });
        if let Some(millis) = millis {
            entry.updated_at = entry.updated_at.max(millis);
        }
        let message = &row["message"];
        for mode in [&row["permissionMode"], &message["permissionMode"]] {
            if let Some(
                mode @ ("default" | "auto" | "acceptEdits" | "plan" | "bypassPermissions"),
            ) = mode.as_str()
            {
                entry.permission_mode = mode.to_owned();
            }
        }
        if let Some(model) = message["model"]
            .as_str()
            .filter(|model| !model.trim().is_empty())
        {
            entry.model = Some(model.to_owned());
        }
        let (has_text, tool, text) = message_content(
            &message["content"],
            first_prompt.is_none() && message["role"] == "user",
        );
        if has_text {
            entry.message_count += 1;
            if !tool && first_prompt.is_none() && !text.is_empty() {
                first_prompt = Some(compact_title(&text));
            }
        }
    }
    entry.title = title
        .or(auto_title)
        .or(first_prompt)
        .unwrap_or_else(|| "未命名 Claude 会话".into());
    Ok((origin_found && entry.message_count > 0).then_some(entry))
}

fn nonempty(value: &Value) -> Option<&str> {
    value
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

fn message_content(content: &Value, collect_title: bool) -> (bool, bool, String) {
    if let Some(text) = nonempty(content) {
        return (
            true,
            false,
            if collect_title {
                text.to_owned()
            } else {
                String::new()
            },
        );
    }
    let mut has_text = false;
    let mut tool = false;
    let mut title = String::new();
    for block in content.as_array().into_iter().flatten() {
        match block["type"].as_str() {
            Some("text") if let Some(text) = nonempty(&block["text"]) => {
                has_text = true;
                if collect_title {
                    if !title.is_empty() {
                        title.push(' ');
                    }
                    title.push_str(text);
                }
            }
            Some("tool_use") if block["name"].is_string() => {
                tool = true;
                has_text |= nonempty(&block["name"]).is_some() || block.get("input").is_some();
            }
            Some("tool_result") => {
                tool = true;
                has_text |= nonempty(&block["content"]).is_some()
                    || block
                        .get("content")
                        .is_some_and(|content| !content.is_string());
            }
            _ => {}
        }
    }
    (has_text, tool, title)
}

fn compact_title(text: &str) -> String {
    let mut title = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.encode_utf16().count() > 72 {
        let mut units = 0;
        let end = title
            .char_indices()
            .find(|(_, c)| {
                units += c.len_utf16();
                units > 71
            })
            .map_or(title.len(), |(index, _)| index);
        title.truncate(end);
        title.push('…');
    }
    title
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn transcript(cwd: &Path, timestamp: Value) -> Value {
        json!({
            "cwd": cwd, "timestamp": timestamp, "permissionMode": "plan",
            "message": {"role": "user", "content": "First   question"}
        })
    }

    fn write_rows(path: &Path, rows: &[Value]) -> io::Result<()> {
        let body = rows
            .iter()
            .map(Value::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(path, body)
    }

    #[test]
    fn encoding_matches_utf16_path_resolve() -> io::Result<()> {
        let path = Path::new(if cfg!(windows) {
            r"C:\tmp\drop\..\中 文🚀"
        } else {
            "/tmp/drop/../中 文🚀"
        });
        assert_eq!(
            encode(&absolute(path)?),
            if cfg!(windows) {
                "C--tmp------"
            } else {
                "-tmp------"
            }
        );
        assert!(valid_id("Claude_01.a-b"));
        assert!(valid_id(&"a".repeat(160)));
        for id in [
            "",
            "../valid",
            "-leading",
            "with space",
            "历史",
            &"a".repeat(161),
        ] {
            assert!(!valid_id(id));
        }
        Ok(())
    }

    #[test]
    fn list_filters_by_origin_and_names_then_sorts_and_caps() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let cwd = root.path().join("project");
        let other = root.path().join("other");
        fs::create_dir(&cwd)?;
        fs::create_dir(&other)?;
        let projects = root.path().join("projects");
        let bucket = projects.join(encode(&cwd));
        fs::create_dir_all(&bucket)?;
        for i in 0..202 {
            write_rows(
                &bucket.join(format!("valid-{i}.jsonl")),
                &[transcript(&cwd, json!(i + 1))],
            )?;
        }
        write_rows(
            &bucket.join("space invalid.jsonl"),
            &[transcript(&cwd, json!(1000))],
        )?;
        write_rows(
            &bucket.join("wrong-origin.jsonl"),
            &[
                transcript(&other, json!(2000)),
                transcript(&cwd, json!(3000)),
            ],
        )?;
        write_rows(
            &bucket.join("missing-origin.jsonl"),
            &[json!({"message": {"role": "user", "content": "missing cwd"}})],
        )?;
        write_rows(&bucket.join("no-events.jsonl"), &[json!({"cwd": cwd})])?;
        write_rows(
            &bucket.join("not-jsonl.txt"),
            &[transcript(&cwd, json!(4000))],
        )?;
        let entries =
            read(Some(&projects), cwd.to_str().unwrap(), None).map_err(anyhow::Error::msg)?;
        assert_eq!(entries.len(), 200);
        assert_eq!(entries[0].provider_session_id, "valid-201");
        assert_eq!(entries[199].provider_session_id, "valid-2");
        assert!(
            entries
                .windows(2)
                .all(|window| window[0].updated_at > window[1].updated_at)
        );
        assert_eq!(entries[0].title, "First question");
        assert_eq!(entries[0].permission_mode, "plan");
        let selected = |id| read(Some(&projects), cwd.to_str().unwrap(), Some(id));
        assert_eq!(
            selected("valid-0").unwrap()[0].provider_session_id,
            "valid-0"
        );
        assert!(selected("wrong-origin").unwrap().is_empty());
        assert!(selected("missing").unwrap().is_empty());
        assert!(selected("../valid-0").is_err());
        assert!(
            read(Some(&projects), other.to_str().unwrap(), None)
                .unwrap()
                .is_empty()
        );
        assert!(
            read(
                Some(&projects),
                root.path().join("missing").to_str().unwrap(),
                None
            )
            .unwrap()
            .is_empty()
        );
        Ok(())
    }

    #[test]
    fn metadata_prefers_custom_then_auto_and_skips_bad_lines() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let cwd = root.path();
        let projects = cwd.join("projects");
        let bucket = projects.join(encode(cwd));
        fs::create_dir_all(&bucket)?;
        let path = bucket.join("valid.jsonl");
        let prompt = "中".repeat(73);
        let rows = [
            json!({"cwd": cwd, "timestamp": "2026-10-30T17:01:11.012+01:00",
                "message": {"role": "user", "content": prompt}}),
            json!({"cwd": "/different/later", "timestamp": "2026-10-30T16:02:00.013Z",
                "message": {"role": "assistant", "model": "test-model", "permissionMode": "acceptEdits",
                    "content": [{"type": "tool_use", "name": "Read", "input": {}}]}}),
            json!({"message": {"role": "assistant", "content": [{"type": "thinking", "thinking": "ignored"}]}}),
        ];
        write_rows(&path, &rows)?;
        let list = || read(Some(&projects), cwd.to_str().unwrap(), None).unwrap();
        assert_eq!(list()[0].title, format!("{}…", "中".repeat(71)));
        let mut rows = rows.to_vec();
        rows.push(json!({"type": "ai-title", "aiTitle": "  Generated  "}));
        write_rows(&path, &rows)?;
        assert_eq!(list()[0].title, "Generated");
        use std::io::Write;
        let mut append = fs::OpenOptions::new().append(true).open(&path)?;
        append.write_all(b"\n{bad\n")?;
        io::copy(&mut io::repeat(b'x').take(MAX_LINE_BYTES + 9), &mut append)?;
        writeln!(
            append,
            "\n{}",
            json!({"type": "custom-title", "customTitle": "  Chosen  "})
        )?;
        let mut skipped = Skipped::default();
        let dates = Connection::open_in_memory()?;
        let mut date = dates.prepare("SELECT ROUND(unixepoch(?1, 'subsec') * 1000)")?;
        let entry = index(
            File::open(&path)?,
            "valid",
            &fs::canonicalize(cwd)?,
            &mut date,
            &mut skipped,
        )?
        .unwrap();
        assert_eq!(entry.title, "Chosen");
        assert_eq!(entry.model.as_deref(), Some("test-model"));
        assert_eq!(entry.permission_mode, "acceptEdits");
        assert_eq!(entry.updated_at, 1_793_376_120_013);
        assert_eq!(entry.message_count, 2);
        assert_eq!(skipped.malformed_lines, 1);
        assert_eq!(skipped.oversized_lines, 1);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn scans_alias_and_canonical_buckets_without_following_escapes() -> anyhow::Result<()> {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir()?;
        let workspace = root.path().join("actual 工作");
        let alias = root.path().join("alias here");
        fs::create_dir(&workspace)?;
        symlink(&workspace, &alias)?;
        let projects = root.path().join("projects");
        let alias_bucket = projects.join(encode(&alias));
        let canonical_bucket = projects.join(encode(&fs::canonicalize(&workspace)?));
        fs::create_dir_all(&alias_bucket)?;
        fs::create_dir_all(&canonical_bucket)?;
        write_rows(
            &alias_bucket.join("via-alias.jsonl"),
            &[transcript(&workspace, json!(1))],
        )?;
        write_rows(
            &canonical_bucket.join("via-real.jsonl"),
            &[transcript(&alias, json!(2))],
        )?;
        let outside = root.path().join("outside");
        fs::create_dir(&outside)?;
        write_rows(
            &outside.join("secret.jsonl"),
            &[transcript(&alias, json!(3))],
        )?;
        symlink(
            outside.join("secret.jsonl"),
            alias_bucket.join("escaping-file.jsonl"),
        )?;
        let mut skipped = Skipped::default();
        let projects = fs::canonicalize(projects)?;
        let real = fs::canonicalize(&workspace)?;
        let entries =
            scan(&projects, &alias, &real, None, &mut skipped).map_err(anyhow::Error::msg)?;
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.provider_session_id.as_str())
                .collect::<Vec<_>>(),
            ["via-real", "via-alias"]
        );
        assert_eq!(skipped.outside_root, 1);
        fs::rename(&alias_bucket, projects.join("hidden-alias"))?;
        symlink(&outside, &alias_bucket)?;
        let entries =
            scan(&projects, &alias, &real, None, &mut skipped).map_err(anyhow::Error::msg)?;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].provider_session_id, "via-real");
        assert_eq!(skipped.outside_root, 2);
        Ok(())
    }
}
