use super::*;
use matou_native_core::sessions::{
    InterfaceLocale, LocalePreference, current_interface_locale, list_runs, read_locale_preference,
};
use matou_native_runtime::{
    host::save_locale_preference,
    protocol::{TimeZoneSettingsError, TimeZoneSettingsSnapshot},
};
use serde_json::{Value, json};

fn time_zone(
    child: &mut RuntimeChild,
    request_id: u64,
    value: Option<Value>,
) -> Result<Result<TimeZoneSettingsSnapshot, TimeZoneSettingsError>> {
    child.send(match value {
        Some(time_zone) => Command::SetTimeZoneSettings {
            request_id,
            time_zone,
        },
        None => Command::GetTimeZoneSettings { request_id },
    })?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match child.event(deadline, true)? {
            Event::TimeZoneSettings(reply) => {
                assert_eq!(reply.request_id, request_id);
                return Ok(reply.result);
            }
            Event::Diagnostic(error) => bail!("{error}"),
            _ => {}
        }
    }
}

#[test]
fn time_zone_saves_with_cross_workspace_readback_and_only_changes_new_cli_processes() -> Result<()>
{
    let root = tempfile::tempdir()?;
    let stub = root.path().join("offline-tz-cli");
    fs::write(
        &stub,
        "#!/bin/sh\nprintf 'BEGIN_TZ=%s\\n' \"${TZ-unset}\"\nwhile IFS= read -r line; do\n  printf 'RUN_TZ=%s\\n' \"${TZ-unset}\"\ndone\n",
    )?;
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o700))?;
    let spawn = || {
        RuntimeChild::spawn_configured_with(
            root.path(),
            &format!("PROMPT='{PROMPT} '\n"),
            Path::new("/bin/zsh"),
            Some(&stub),
            |command| {
                command
                    .env("TZ", "Asia/Shanghai")
                    .env("MATOU_CODEX_COMMAND", &stub);
            },
        )
    };
    let mut child = spawn()?;
    child.hello()?;
    let initial = time_zone(&mut child, 1, None)?.unwrap();
    assert_eq!(initial.time_zone, "auto");
    assert_eq!(initial.system_time_zone, "Asia/Shanghai");
    assert_eq!(initial.auto_time_zone.as_deref(), Some("Asia/Singapore"));
    assert!(initial.zones.contains(&"Asia/Tokyo".into()));

    child.input_bytes(b"claude\r")?;
    child.frame(|_, screen, _| screen_has_line(screen, "BEGIN_TZ=Asia/Singapore"))?;
    let session = child.session.clone();
    let db = Connection::open(child.data.join("matou.sqlite"))?;
    db.busy_timeout(TIMEOUT)?;
    let first_workspace: String = db.query_row(
        "SELECT workspace_id FROM tasks WHERE id=?",
        [&child.task],
        |row| row.get(0),
    )?;
    let before = list_runs(&db, &session)?;
    assert_eq!(
        time_zone(&mut child, 2, Some(json!("UTC")))?
            .unwrap()
            .time_zone,
        "UTC"
    );
    for (id, invalid) in [
        (3, json!("Mars/Olympus")),
        (4, json!("+08:00")),
        (5, json!(17)),
    ] {
        assert_eq!(
            time_zone(&mut child, id, Some(invalid))?,
            Err(TimeZoneSettingsError::InvalidRequest)
        );
    }
    let stored: String = db.query_row(
        "SELECT value_json FROM preferences WHERE key = 'claude.timeZone'",
        [],
        |row| row.get(0),
    )?;
    assert_eq!(stored, "\"UTC\"");
    child.input("PROBE")?;
    child.frame(|_, screen, _| screen_has_line(screen, "RUN_TZ=Asia/Singapore"))?;
    let after = list_runs(&db, &session)?;
    assert_eq!(before.len(), after.len());
    assert_eq!(before.last().unwrap().pid, after.last().unwrap().pid);

    let other = root.path().join("second workspace");
    fs::create_dir(&other)?;
    child.send(Command::CreateWorkspace {
        root_directory: other.to_string_lossy().into_owned(),
    })?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Event::Navigation { hierarchy, .. } = child.event(deadline, true)?
            && hierarchy
                .workspace
                .is_some_and(|workspace| Path::new(&workspace.root_directory) == other)
        {
            child.session = hierarchy.session.context("second workspace session")?.id;
            break;
        }
    }
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Event::Frame {
            session_id, screen, ..
        } = child.event(deadline, true)?
            && session_id == child.session
            && screen_has_line(&screen, PROMPT)
        {
            break;
        }
    }
    assert_eq!(time_zone(&mut child, 6, None)?.unwrap().time_zone, "UTC");
    child.input_bytes(b"codex\r")?;
    child.frame(|_, screen, _| screen_has_line(screen, "BEGIN_TZ=Asia/Singapore"))?;
    child.send(Command::Shutdown)?;
    assert!(child.closed()?.is_empty());
    let (status, stderr) = child.wait()?;
    ensure!(status.success(), "runtime shutdown: {stderr}");

    let mut reopened = spawn()?;
    reopened.connect()?;
    assert_eq!(time_zone(&mut reopened, 7, None)?.unwrap().time_zone, "UTC");
    reopened.send(Command::SelectWorkspace {
        workspace_id: first_workspace.clone(),
    })?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Event::Navigation { hierarchy, .. } = reopened.event(deadline, true)?
            && hierarchy
                .workspace
                .is_some_and(|workspace| workspace.id == first_workspace)
        {
            break;
        }
    }
    reopened.send(Command::RetryRecovery {
        session_id: session.clone(),
    })?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        match reopened.event(deadline, true)? {
            Event::Frame {
                session_id, screen, ..
            } if session_id == session && screen_has_line(&screen, "BEGIN_TZ=UTC") => break,
            Event::Diagnostic(error) => bail!("{error}"),
            _ => {}
        }
    }
    assert!(list_runs(&db, &session)?.len() > before.len());

    for (index, broken) in ["{", "{}", "\"Mars/Olympus\""].into_iter().enumerate() {
        db.execute(
            "UPDATE preferences SET value_json=? WHERE key='claude.timeZone'",
            [broken],
        )?;
        assert_eq!(
            time_zone(&mut reopened, 8 + index as u64, None)?
                .unwrap()
                .time_zone,
            "auto"
        );
    }
    assert_eq!(
        time_zone(&mut reopened, 11, Some(json!("system")))?
            .unwrap()
            .time_zone,
        "system"
    );
    assert_eq!(
        time_zone(&mut reopened, 12, Some(json!("auto")))?
            .unwrap()
            .time_zone,
        "auto"
    );
    reopened.send(Command::Shutdown)?;
    assert!(reopened.closed()?.is_empty());
    let (status, stderr) = reopened.wait()?;
    ensure!(status.success(), "reopened runtime shutdown: {stderr}");
    Ok(())
}

#[test]
fn language_file_is_atomic_private_and_independent_of_database_recovery() -> Result<()> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("locale.json");
    fs::write(
        root.path().join("matou.sqlite"),
        b"unreadable test database",
    )?;
    assert_eq!(
        read_locale_preference(root.path()),
        LocalePreference::System
    );
    for broken in [
        "{",
        "null",
        "{\"preference\":17}",
        "{\"preference\":\"en-US\"}",
    ] {
        fs::write(&path, broken)?;
        assert_eq!(
            read_locale_preference(root.path()),
            LocalePreference::System
        );
    }
    let locale = save_locale_preference(root.path(), LocalePreference::English)?;
    assert_eq!(fs::read_to_string(&path)?, "{\"preference\":\"en\"}");
    assert_eq!(path.metadata()?.permissions().mode() & 0o777, 0o600);
    assert_eq!(
        read_locale_preference(root.path()),
        LocalePreference::English
    );
    assert_eq!(locale, current_interface_locale(LocalePreference::English));
    save_locale_preference(root.path(), LocalePreference::System)?;
    assert_eq!(fs::read_to_string(&path)?, "{\"preference\":\"system\"}");
    assert_eq!(
        fs::read(root.path().join("matou.sqlite"))?,
        b"unreadable test database"
    );
    assert_eq!(root.path().read_dir()?.count(), 2);
    assert_eq!(InterfaceLocale::English.value(), "en");
    Ok(())
}
