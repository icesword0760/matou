use std::{
    ffi::OsString,
    fs::{self, File, Metadata, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

#[cfg(any(target_os = "macos", test))]
use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
#[cfg(windows)]
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};

use anyhow::{Context, Result, anyhow, ensure};
use serde_json::{Value, json};

const SEED_FILE: &str = "safe-browser.json";
const MAX_SEED: u32 = 2_147_483_647;
pub(crate) const ACCOUNT_ENV_KEYS: [&str; 3] = [
    "MATOU_SAFE_BROWSER_BIN",
    "MATOU_SAFE_BROWSER_TZ",
    "MATOU_SAFE_BROWSER_SEED",
];

fn browser_time_zone(country: Option<&str>) -> &'static str {
    match country.map(str::to_ascii_uppercase).as_deref() {
        Some("US") => "America/Los_Angeles",
        Some("JP") => "Asia/Tokyo",
        Some("TW") => "Asia/Taipei",
        Some("HK") => "Asia/Hong_Kong",
        Some("GB") => "Europe/London",
        Some("DE") => "Europe/Berlin",
        _ => "Asia/Singapore",
    }
}

fn resolve_executable(override_path: Option<&Path>) -> Option<PathBuf> {
    let path = match override_path.filter(|path| !path.as_os_str().is_empty()) {
        Some(path) => path,
        #[cfg(target_os = "macos")]
        None => Path::new("/Applications/Mantle.app/Contents/MacOS/Chromium"),
        #[cfg(not(target_os = "macos"))]
        None => return None,
    };
    // The login bridge may start in a different working directory.
    let path = std::path::absolute(path).ok()?;
    let metadata = fs::metadata(&path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o111 == 0 {
        return None;
    }
    Some(path)
}

fn fingerprint_seed(value: &Value) -> Option<u32> {
    let seed = value.get("fingerprintSeed")?.as_u64()?;
    (1..=u64::from(MAX_SEED))
        .contains(&seed)
        .then_some(seed as u32)
}

fn load_or_create_fingerprint_seed(data_root: &Path) -> Result<u32> {
    create_directory(data_root)?;
    let path = data_root.join(SEED_FILE);
    if entry_metadata(&path)?.is_none() {
        let seed = loop {
            let seed = getrandom::u32()
                .map_err(|error| anyhow!("Generate browser seed: {error}"))?
                & MAX_SEED;
            if seed != 0 {
                break seed;
            }
        };
        let bytes = serde_json::to_vec(&json!({ "fingerprintSeed": seed }))?;
        match write_private_file(&path, &bytes) {
            Ok(()) => return Ok(seed),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    let file = open_regular_file(&path)?;
    let seed = fingerprint_seed(&serde_json::from_reader(&file)?)
        .context("Invalid safe browser fingerprint seed")?;
    #[cfg(unix)]
    if file.metadata()?.permissions().mode() & 0o7777 != 0o600 {
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    Ok(seed)
}

pub(crate) fn account_env(
    data_root: &Path,
    country: Option<&str>,
    env: impl Fn(&str) -> Option<OsString>,
) -> Vec<(&'static str, OsString)> {
    if !crate::browser_integration::prefers_controlled_browser(data_root) {
        return Vec::new();
    }

    let external: Vec<_> = ACCOUNT_ENV_KEYS
        .into_iter()
        .filter_map(|key| env(key).map(|value| (key, value)))
        .collect();
    if external.len() == ACCOUNT_ENV_KEYS.len() {
        return external;
    }
    let path = env("MATOU_SAFE_BROWSER_PATH");
    let Some(executable) = resolve_executable(path.as_deref().map(Path::new)) else {
        return Vec::new();
    };
    let seed = match load_or_create_fingerprint_seed(data_root) {
        Ok(seed) => seed,
        Err(error) => {
            eprintln!("Safe browser settings unavailable: {error:#}");
            return Vec::new();
        }
    };
    ACCOUNT_ENV_KEYS
        .into_iter()
        .zip([
            executable.into_os_string(),
            browser_time_zone(country).into(),
            seed.to_string().into(),
        ])
        .collect()
}

fn entry_metadata(path: &Path) -> io::Result<Option<Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn is_link(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        // FILE_ATTRIBUTE_REPARSE_POINT includes directory junctions.
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn directory_exists(path: &Path) -> io::Result<bool> {
    match entry_metadata(path)? {
        None => Ok(false),
        Some(metadata) if metadata.is_dir() && !is_link(&metadata) => Ok(true),
        Some(_) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Expected a directory without symlinks",
        )),
    }
}

fn create_directory(path: &Path) -> io::Result<()> {
    if directory_exists(path)? {
        return Ok(());
    }
    #[cfg(unix)]
    let created = fs::DirBuilder::new().mode(0o700).create(path);
    #[cfg(not(unix))]
    let created = fs::create_dir(path);
    match created {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists && directory_exists(path)? => {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn open_regular_file(path: &Path) -> Result<File> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file() && !is_link(&metadata),
        "Expected a regular file without symlinks"
    );
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    #[cfg(windows)]
    options.custom_flags(0x00200000); // FILE_FLAG_OPEN_REPARSE_POINT
    let file = options.open(path)?;
    let opened = file.metadata()?;
    ensure!(
        opened.is_file() && !is_link(&opened),
        "Expected a regular file without symlinks"
    );
    Ok(file)
}

fn write_private_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(path.parent().expect("settings directory"))?;
    #[cfg(unix)]
    file.as_file()
        .set_permissions(fs::Permissions::from_mode(0o600))?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    // A concurrent launcher may have already published this setting.
    file.persist_noclobber(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn migrate_legacy_electron_preferences(legacy_user_data: &Path, data_root: &Path) {
    match directory_exists(legacy_user_data) {
        Ok(false) => return,
        Err(error) => {
            eprintln!("Skip migration of legacy settings: {error}");
            return;
        }
        Ok(true) => {}
    }
    let settings: [(&str, fn(&Value) -> bool); 3] = [
        ("locale.json", Value::is_object),
        ("claude-accounts/preferences.json", Value::is_object),
        (SEED_FILE, |value| fingerprint_seed(value).is_some()),
    ];
    for (relative, valid) in settings {
        if let Err(error) = copy_preference(legacy_user_data, data_root, Path::new(relative), valid)
        {
            eprintln!("Skip migration of {relative}: {error:#}");
        }
    }
}

#[cfg(any(target_os = "macos", test))]
fn copy_preference(
    source_root: &Path,
    data_root: &Path,
    relative: &Path,
    valid: fn(&Value) -> bool,
) -> Result<()> {
    directory_exists(data_root)?;
    let subdirectory = relative
        .parent()
        .filter(|path| !path.as_os_str().is_empty());
    if let Some(subdirectory) = subdirectory {
        directory_exists(&data_root.join(subdirectory))?;
    }
    let target = data_root.join(relative);
    if entry_metadata(&target)?.is_some() {
        return Ok(());
    }
    if let Some(subdirectory) = subdirectory {
        ensure!(
            directory_exists(&source_root.join(subdirectory))?,
            "Missing legacy directory"
        );
    }
    let mut bytes = Vec::new();
    open_regular_file(&source_root.join(relative))?.read_to_end(&mut bytes)?;
    ensure!(
        valid(&serde_json::from_slice(&bytes)?),
        "Invalid legacy settings"
    );
    create_directory(data_root)?;
    if let Some(subdirectory) = subdirectory {
        let directory = data_root.join(subdirectory);
        create_directory(&directory)?;
        #[cfg(unix)]
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
            .open(directory)?
            .set_permissions(fs::Permissions::from_mode(0o700))?;
    }
    match write_private_file(&target, &bytes) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_zones_match_the_desktop_mapping() {
        for (country, expected) in [
            (Some("US"), "America/Los_Angeles"),
            (Some("JP"), "Asia/Tokyo"),
            (Some("SG"), "Asia/Singapore"),
            (Some("TW"), "Asia/Taipei"),
            (Some("HK"), "Asia/Hong_Kong"),
            (Some("GB"), "Europe/London"),
            (Some("DE"), "Europe/Berlin"),
            (Some("jp"), "Asia/Tokyo"),
            (Some("XX"), "Asia/Singapore"),
            (None, "Asia/Singapore"),
        ] {
            assert_eq!(browser_time_zone(country), expected);
        }
    }

    #[test]
    fn creates_and_reuses_a_private_seed() -> Result<()> {
        let root = tempfile::tempdir()?;
        let seed = load_or_create_fingerprint_seed(root.path())?;
        let path = root.path().join(SEED_FILE);
        assert!((1..=MAX_SEED).contains(&seed));
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(&path)?)?,
            json!({ "fingerprintSeed": seed })
        );
        #[cfg(unix)]
        assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o777, 0o600);
        assert_eq!(load_or_create_fingerprint_seed(root.path())?, seed);
        Ok(())
    }

    #[test]
    fn concurrent_seed_creation_uses_one_persisted_value() -> Result<()> {
        let root = tempfile::tempdir()?;
        let gate = std::sync::Barrier::new(4);
        let seeds = std::thread::scope(|scope| {
            let workers: Vec<_> = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        gate.wait();
                        load_or_create_fingerprint_seed(root.path())
                    })
                })
                .collect();
            workers
                .into_iter()
                .map(|worker| worker.join().unwrap())
                .collect::<Result<Vec<_>>>()
        })?;
        assert!(seeds.iter().all(|&seed| seed == seeds[0]));
        assert_eq!(load_or_create_fingerprint_seed(root.path())?, seeds[0]);
        assert_eq!(fs::read_dir(root.path())?.count(), 1);
        Ok(())
    }

    #[test]
    fn malformed_seeds_are_left_alone() -> Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join(SEED_FILE);
        for bytes in [
            "invalid JSON",
            "{}",
            r#"{"fingerprintSeed":0}"#,
            r#"{"fingerprintSeed":2147483648}"#,
            r#"{"fingerprintSeed":1.5}"#,
            r#"{"fingerprintSeed":"12"}"#,
        ] {
            fs::write(&path, bytes)?;
            assert!(load_or_create_fingerprint_seed(root.path()).is_err());
            assert_eq!(fs::read_to_string(&path)?, bytes);
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_symlinked_seed_file_or_root() -> Result<()> {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        let linked_file = outside.path().join(SEED_FILE);
        let bytes = r#"{"fingerprintSeed":1234}"#;
        fs::write(&linked_file, bytes)?;
        fs::set_permissions(&linked_file, fs::Permissions::from_mode(0o644))?;
        symlink(&linked_file, root.path().join(SEED_FILE))?;
        let linked_root = root.path().join("linked-data");
        symlink(outside.path(), &linked_root)?;
        for data_root in [root.path(), &linked_root] {
            assert!(load_or_create_fingerprint_seed(data_root).is_err());
        }
        assert_eq!(fs::read_to_string(&linked_file)?, bytes);
        assert_eq!(
            fs::metadata(&linked_file)?.permissions().mode() & 0o777,
            0o644
        );
        Ok(())
    }

    #[test]
    fn account_env_uses_the_executable_and_current_country() -> Result<()> {
        let root = tempfile::tempdir()?;
        let executable = root.path().join("fixture browser");
        let env = |key: &str| match key {
            "MATOU_SAFE_BROWSER_PATH" => Some(executable.clone().into_os_string()),
            "MATOU_SAFE_BROWSER_TZ" => Some("partial override".into()),
            _ => None,
        };
        assert!(account_env(root.path(), None, env).is_empty());
        assert!(!root.path().join(SEED_FILE).exists());
        fs::write(&executable, "")?;
        #[cfg(unix)]
        {
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o600))?;
            assert!(account_env(root.path(), None, env).is_empty());
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))?;
        }
        assert!(account_env(root.path(), Some("JP"), env).is_empty());
        assert!(!root.path().join(SEED_FILE).exists());
        crate::browser_integration::set_controlled_browser_preference(root.path(), true)?;
        let first = account_env(root.path(), Some("JP"), env);
        let next = account_env(root.path(), Some("GB"), env);
        let seed = load_or_create_fingerprint_seed(root.path())?.to_string();
        assert_eq!(
            first,
            [
                ("MATOU_SAFE_BROWSER_BIN", executable.clone().into_os_string()),
                ("MATOU_SAFE_BROWSER_TZ", "Asia/Tokyo".into()),
                ("MATOU_SAFE_BROWSER_SEED", seed.clone().into()),
            ]
        );
        assert_eq!(next[0], first[0]);
        assert_eq!(next[1], ("MATOU_SAFE_BROWSER_TZ", "Europe/London".into()));
        assert_eq!(next[2], first[2]);
        crate::browser_integration::set_controlled_browser_preference(root.path(), false)?;
        assert!(account_env(root.path(), Some("JP"), env).is_empty());
        Ok(())
    }

    #[test]
    fn account_env_preserves_a_complete_external_override() -> Result<()> {
        let root = tempfile::tempdir()?;
        crate::browser_integration::set_controlled_browser_preference(root.path(), true)?;
        let external = [
            ("MATOU_SAFE_BROWSER_BIN", OsString::from("/fixture/browser")),
            ("MATOU_SAFE_BROWSER_TZ", "America/Los_Angeles".into()),
            ("MATOU_SAFE_BROWSER_SEED", "987".into()),
        ];
        let mut empty_value = external.clone();
        empty_value[2].1.clear();
        for expected in [external, empty_value] {
            let actual = account_env(root.path(), Some("JP"), |key| {
                Some(
                    expected
                        .iter()
                        .find(|(name, _)| *name == key)
                        .unwrap()
                        .1
                        .clone(),
                )
            });
            assert_eq!(actual, expected);
        }
        assert!(!root.path().join(SEED_FILE).exists());
        Ok(())
    }

    const LEGACY_SETTINGS: [(&str, &str); 3] = [
        ("locale.json", r#"{"preference":"zh-CN"}"#),
        (
            "claude-accounts/preferences.json",
            r#"{"names":{"id":"工作账户"},"rememberScope":true}"#,
        ),
        (SEED_FILE, r#"{"fingerprintSeed":2147483647}"#),
    ];

    #[test]
    fn migration_copies_valid_preferences_once_and_keeps_the_source_intact() -> Result<()> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("legacy");
        let target = root.path().join("data");
        fs::create_dir_all(source.join("claude-accounts"))?;
        for (name, contents) in LEGACY_SETTINGS {
            let path = source.join(name);
            fs::write(&path, contents)?;
            #[cfg(unix)]
            fs::set_permissions(path, fs::Permissions::from_mode(0o644))?;
        }
        migrate_legacy_electron_preferences(&source, &target);
        for (name, contents) in LEGACY_SETTINGS {
            let target_file = target.join(name);
            assert_eq!(fs::read_to_string(&target_file)?, contents);
            #[cfg(unix)]
            assert_eq!(
                fs::metadata(&target_file)?.permissions().mode() & 0o777,
                0o600
            );
            fs::write(target_file, "existing native setting")?;
        }
        #[cfg(unix)]
        assert_eq!(
            fs::metadata(target.join("claude-accounts"))?
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        migrate_legacy_electron_preferences(&source, &target);
        for (name, contents) in LEGACY_SETTINGS {
            assert_eq!(
                fs::read_to_string(target.join(name))?,
                "existing native setting"
            );
            let source_file = source.join(name);
            assert_eq!(fs::read_to_string(&source_file)?, contents);
            #[cfg(unix)]
            assert_eq!(
                fs::metadata(source_file)?.permissions().mode() & 0o777,
                0o644
            );
        }
        Ok(())
    }

    #[test]
    fn migration_skips_corrupt_preferences_without_creating_the_target() -> Result<()> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("legacy");
        let target = root.path().join("data");
        fs::create_dir_all(source.join("claude-accounts"))?;
        let malformed = [
            ("locale.json", "[]"),
            ("claude-accounts/preferences.json", "{"),
            (SEED_FILE, r#"{"fingerprintSeed":-1}"#),
        ];
        for (name, contents) in malformed {
            fs::write(source.join(name), contents)?;
        }
        migrate_legacy_electron_preferences(&source, &target);
        assert!(!target.exists());
        for (name, contents) in malformed {
            assert_eq!(fs::read_to_string(source.join(name))?, contents);
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn migration_skips_linked_files_and_directories() -> Result<()> {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir()?;
        let source = root.path().join("legacy");
        let target = root.path().join("data");
        let outside = root.path().join("outside");
        fs::create_dir(&source)?;
        fs::create_dir(&target)?;
        fs::create_dir_all(outside.join("claude-accounts"))?;
        for (name, contents) in LEGACY_SETTINGS {
            fs::write(outside.join(name), contents)?;
        }
        symlink(
            outside.join("claude-accounts"),
            source.join("claude-accounts"),
        )?;
        symlink(outside.join("locale.json"), source.join("locale.json"))?;
        symlink(outside.join(SEED_FILE), source.join(SEED_FILE))?;
        migrate_legacy_electron_preferences(&source, &target);
        assert_eq!(fs::read_dir(&target)?.count(), 0);

        symlink(
            outside.join("claude-accounts"),
            target.join("claude-accounts"),
        )?;
        let missing = outside.join("missing");
        symlink(&missing, target.join(SEED_FILE))?;
        migrate_legacy_electron_preferences(&outside, &target);
        assert!(
            fs::symlink_metadata(target.join(SEED_FILE))?
                .file_type()
                .is_symlink()
        );
        assert!(!missing.exists());
        assert!(target.join("locale.json").is_file());

        let linked_root = root.path().join("linked");
        symlink(&outside, &linked_root)?;
        let new_target = root.path().join("new-target");
        migrate_legacy_electron_preferences(&linked_root, &new_target);
        assert!(!new_target.exists());
        migrate_legacy_electron_preferences(&source, &linked_root);
        for (name, contents) in LEGACY_SETTINGS {
            assert_eq!(fs::read_to_string(outside.join(name))?, contents);
        }
        Ok(())
    }
}
