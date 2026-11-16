#![cfg(target_os = "macos")]

mod support;

use std::{
    collections::BTreeSet,
    fs,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use matou_native_core::{
    hierarchy::{HierarchyResult, Task, Workspace},
    sessions::list_runs,
    startup::open_read_only,
    workspace_paths::WorkspacePathState,
};
use matou_native_desktop::runtime_bridge::{
    Command, Event, LaunchOptions, RecoveryState, RuntimeBridge,
};
use uuid::Uuid;

const PROMPT: &str = "MATOU_WORKSPACE_READY>";
const TIMEOUT: Duration = Duration::from_secs(8);

#[test]
fn workspace_navigation_preserves_shells_and_removes_only_the_target() -> Result<()> {
    let root = std::env::temp_dir().join(format!("matou-native-workspaces-{}", Uuid::new_v4()));
    verify_workspaces(&root)
        .with_context(|| format!("isolated workspace data: {}", root.display()))?;
    fs::remove_dir_all(root)?;
    Ok(())
}

#[test]
fn unavailable_workspace_reopen_requires_an_explicit_retry() -> Result<()> {
    let root = std::env::temp_dir().join(format!("matou-workspace-retry-{}", Uuid::new_v4()));
    verify_workspace_retry(&root)
        .with_context(|| format!("isolated workspace data: {}", root.display()))?;
    fs::remove_dir_all(root)?;
    Ok(())
}

fn verify_workspace_retry(root: &Path) -> Result<()> {
    let home = root.join("home");
    fs::create_dir_all(&home)?;
    fs::write(home.join(".zshrc"), format!("PROMPT='{PROMPT} '\n"))?;
    let options = LaunchOptions {
        data_root: root.join("data"),
        workspace: root.join("default"),
        shell_home: home,
        shell: "/bin/zsh".into(),
    };
    let project = root.join("project");
    let missing = root.join("missing");
    fs::create_dir_all(&project)?;
    let mut bridge = support::start(options.clone())?;
    let mut probe = Probe::new(&bridge);
    let default = probe.ready()?.workspace.context("default workspace")?.id;
    let added = probe.navigate(Command::CreateWorkspace {
        root_directory: project.to_string_lossy().into_owned(),
    })?;
    let workspace_id = added.workspace.context("created workspace")?.id;
    let session_id = added.session.context("created Shell")?.id;
    probe.output(&session_id, PROMPT)?;
    bridge.shutdown()?;
    fs::rename(&project, &missing)?;

    let database = options.data_root.join("matou.sqlite");
    let mut bridge = support::start(options)?;
    let mut probe = Probe::new(&bridge);
    let reopened = probe.ready()?.session.context("reopened session")?.id;
    assert_eq!(reopened, session_id);
    let failed = RecoveryState::Failed(format!(
        "Session {session_id} did not reach a running state"
    ));
    probe.recovery_state(&session_id, &failed)?;
    let initial_path = probe.path_state(&workspace_id, "invalid")?;
    assert_eq!(initial_path.reason, "missing");
    assert!(!probe.frames.contains(&session_id));

    let db = open_read_only(&database)?;
    let before = list_runs(&db, &session_id)?;
    assert_eq!(before.len(), 1, "invalid startup must not spawn a Shell");
    let retry = || Command::RetryRecovery {
        session_id: session_id.clone(),
    };
    probe.send(retry())?;
    probe.recovery_state(&session_id, &RecoveryState::Restoring)?;
    probe.recovery_state(&session_id, &failed)?;
    probe.path_state(&workspace_id, "invalid")?;
    assert!(!probe.frames.contains(&session_id));
    assert_eq!(list_runs(&db, &session_id)?.len(), 1);
    probe.input(&session_id, "touch .input-while-invalid")?;

    fs::rename(&missing, &project)?;
    probe.path_state(&workspace_id, "valid")?;
    probe.input(&session_id, "touch .input-before-retry")?;
    probe.navigate(Command::SelectWorkspace {
        workspace_id: default,
    })?;
    let selected = probe.navigate(Command::SelectWorkspace {
        workspace_id: workspace_id.clone(),
    })?;
    assert_eq!(
        selected.session.context("same failed Shell")?.id,
        session_id
    );
    probe.path_state(&workspace_id, "valid")?;
    assert!(
        !probe.frames.contains(&session_id),
        "restoring the path must not start the failed Shell"
    );
    assert_eq!(list_runs(&db, &session_id)?.len(), 1);

    probe.send(retry())?;
    probe.recovery_state(&session_id, &RecoveryState::Restoring)?;
    probe.recovery_state(&session_id, &RecoveryState::Ready)?;
    probe.input(&session_id, "printf 'WORKSPACE_%s\\n' RETRY_SUCCEEDED")?;
    probe.output(&session_id, "WORKSPACE_RETRY_SUCCEEDED")?;
    let after = list_runs(&db, &session_id)?;
    assert_eq!(after.len(), 2);
    assert_eq!(after[0].id, before[0].id);
    assert_ne!(after[1].id, before[0].id);
    assert_eq!(after[1].status, "running");
    assert!(!project.join(".input-while-invalid").exists());
    assert!(!project.join(".input-before-retry").exists());
    drop(db);
    bridge.shutdown()?;
    Ok(())
}

fn verify_workspaces(root: &Path) -> Result<()> {
    let home = root.join("home");
    fs::create_dir_all(&home)?;
    fs::write(home.join(".zshrc"), format!("PROMPT='{PROMPT} '\n"))?;
    let options = LaunchOptions {
        data_root: root.join("data"),
        workspace: root.join("default"),
        shell_home: home,
        shell: PathBuf::from("/bin/zsh"),
    };
    let project = root.join("project");
    let selected = root.join("project-link");
    fs::create_dir_all(&project)?;
    symlink(&project, &selected)?;

    let mut bridge = support::start(options.clone())?;
    let mut probe = Probe::new(&bridge);
    let default = probe.ready()?;
    let default_id = default.workspace.context("default workspace")?.id;
    let default_session = default.session.context("default session")?.id;
    probe.output(&default_session, PROMPT)?;
    probe.input(&default_session, "export MATOU_DEFAULT=DEFAULT_SURVIVES")?;

    let added = probe.navigate(Command::CreateWorkspace {
        root_directory: selected.to_string_lossy().into_owned(),
    })?;
    let workspace = added.workspace.context("created workspace")?;
    let workspace_id = workspace.id;
    let task_id = added.task.context("created default task")?.id;
    let session_id = added.session.context("created Shell")?.id;
    assert_eq!(workspace.name, "project-link");
    assert_eq!(Path::new(&workspace.root_directory), selected);
    assert_eq!(
        probe.last_action.as_ref(),
        Some(&("create".into(), Some(workspace_id.clone()), None))
    );
    assert_eq!(probe.workspaces.len(), 2);
    assert_eq!(
        probe
            .tasks
            .iter()
            .map(|task| task.workspace_id.as_str())
            .collect::<Vec<_>>(),
        [default_id.as_str(), workspace_id.as_str()],
    );
    probe.output(&session_id, PROMPT)?;
    probe.input(
        &session_id,
        "export MATOU_WORKSPACE=PROJECT_SURVIVES; printf 'PROJECT_%s\\n' FIRST_RUN",
    )?;
    probe.output(&session_id, "PROJECT_FIRST_RUN")?;

    let original = probe.navigate(Command::SelectWorkspace {
        workspace_id: default_id.clone(),
    })?;
    assert_eq!(
        original.session.context("original Shell")?.id,
        default_session
    );
    let returned = probe.navigate(Command::SelectWorkspace {
        workspace_id: workspace_id.clone(),
    })?;
    assert_eq!(returned.session.context("returning Shell")?.id, session_id);
    probe.input(&session_id, r#"printf '%s\n' "$MATOU_WORKSPACE""#)?;
    probe.output(&session_id, "PROJECT_SURVIVES")?;

    let repeated = probe.navigate(Command::CreateWorkspace {
        root_directory: format!("{}/../project-link/./", selected.display()),
    })?;
    assert_eq!(
        repeated.workspace.context("reused workspace")?.id,
        workspace_id
    );
    assert_eq!(repeated.task.context("reused task")?.id, task_id);
    assert_eq!(repeated.session.context("reused Shell")?.id, session_id);
    let original_scene = repeated.scene.context("reused scene")?.id;
    assert_eq!(probe.workspaces.len(), 2);

    probe.send(Command::RelinkWorkspace {
        workspace_id: workspace_id.clone(),
        root_directory: options.workspace.to_string_lossy().into_owned(),
    })?;
    let rejected = probe.wait("duplicate-directory relink result", |event| match event {
        Event::WorkspaceActionResult {
            action,
            workspace_id,
            error,
        } if action == "relink" => Ok(Some((workspace_id, error))),
        _ => Ok(None),
    })?;
    assert_eq!(
        rejected,
        (
            Some(workspace_id.clone()),
            Some("该目录已经属于另一个工作空间".into())
        )
    );

    probe.navigate(Command::SetWorkspacePinned {
        workspace_id: workspace_id.clone(),
        pinned: true,
    })?;
    assert_eq!(
        probe.last_action.as_ref(),
        Some(&("pin".into(), Some(workspace_id.clone()), None))
    );
    probe.navigate(Command::ReorderPinnedWorkspace {
        workspace_id: workspace_id.clone(),
        before_workspace_id: Some(default_id.clone()),
    })?;
    assert_eq!(
        probe
            .workspaces
            .iter()
            .map(|row| row.id.as_str())
            .collect::<Vec<_>>(),
        [workspace_id.as_str(), default_id.as_str()]
    );

    let peer = probe.navigate(Command::CreateCanvas { task_id })?;
    let peer_session = peer.session.context("second canvas Shell")?.id;
    probe.output(&peer_session, PROMPT)?;
    probe.input(
        &peer_session,
        "export MATOU_PEER=PROJECT_PEER_SURVIVES; printf 'PROJECT_%s\\n' PEER_FIRST_RUN",
    )?;
    probe.output(&peer_session, "PROJECT_PEER_FIRST_RUN")?;
    probe.input(
        &peer_session,
        "trap '' HUP; printf 'PROJECT_%s\\n' PEER_WAITING; builtin read -r line",
    )?;
    probe.output(&peer_session, "PROJECT_PEER_WAITING")?;

    let database_path = options.data_root.join("matou.sqlite");
    let db = open_read_only(&database_path)?;
    let before = list_runs(&db, &session_id)?;
    let peer_before = list_runs(&db, &peer_session)?;
    assert_eq!(before.len(), 1);
    assert_eq!(peer_before.len(), 1);

    let moved = root.join("temporarily-unavailable");
    fs::rename(&project, &moved)?;
    let invalid = probe.path_state(&workspace_id, "invalid")?;
    assert_eq!(invalid.reason, "missing");
    probe.input(&peer_session, "touch .blocked-input")?;
    probe.wait(
        "input rejected for missing directory",
        |event| match event {
            Event::Diagnostic(message) => {
                assert_eq!(
                    message,
                    "工作区目录不可用，请先在本地恢复原路径，或移出该工作区"
                );
                Ok(Some(()))
            }
            _ => Ok(None),
        },
    )?;

    fs::rename(&moved, &project)?;
    let valid = probe.path_state(&workspace_id, "valid")?;
    assert!(valid.validation_generation > invalid.validation_generation);
    let background = list_runs(&db, &session_id)?;
    assert_eq!(background.len(), 1);
    assert_eq!(background[0].id, before[0].id);
    assert_eq!(background[0].status, "running");
    probe.input(&session_id, r#"printf 'BG_%s\n' "$MATOU_WORKSPACE""#)?;
    probe.output(&session_id, "BG_PROJECT_SURVIVES")?;

    let returned = probe.navigate(Command::SelectScene {
        scene_id: original_scene,
    })?;
    assert_eq!(
        returned
            .session
            .context("original Shell after recovery")?
            .id,
        session_id
    );
    probe.input(
        &session_id,
        r#"printf '%s\n' "${MATOU_WORKSPACE:-FRESH_WORKSPACE_SHELL}""#,
    )?;
    probe.output_all(&session_id, &["PROJECT_FIRST_RUN", "FRESH_WORKSPACE_SHELL"])?;
    assert!(!project.join(".blocked-input").exists());
    let recovered = list_runs(&db, &session_id)?;
    assert_eq!(recovered.len(), 2);
    assert_eq!(recovered[0].id, before[0].id);
    assert_eq!(recovered[0].status, "exited");
    assert_ne!(recovered[1].id, before[0].id);
    assert_eq!(recovered[1].status, "running");

    probe.input(
        &peer_session,
        r#"printf '%s\n' "${MATOU_PEER:-FRESH_PEER_SHELL}""#,
    )?;
    probe.output_all(
        &peer_session,
        &["PROJECT_PEER_FIRST_RUN", "FRESH_PEER_SHELL"],
    )?;
    let peer_recovered = list_runs(&db, &peer_session)?;
    assert_eq!(peer_recovered.len(), 2);
    assert_eq!(peer_recovered[0].id, peer_before[0].id);
    assert_eq!(peer_recovered[0].status, "exited");
    assert_ne!(peer_recovered[1].id, peer_before[0].id);
    assert_eq!(peer_recovered[1].status, "running");

    probe.input(
        &session_id,
        "export MATOU_WORKSPACE=PROJECT_AWAY; printf 'PROJECT_%s\\n' BEFORE_SWITCH_AWAY",
    )?;
    probe.output(&session_id, "PROJECT_BEFORE_SWITCH_AWAY")?;
    fs::rename(&project, &moved)?;
    let invalid_before_switch = probe.path_state(&workspace_id, "invalid")?;
    let away = probe.navigate(Command::SelectWorkspace {
        workspace_id: default_id.clone(),
    })?;
    assert_eq!(
        away.session
            .context("default Shell during path recovery")?
            .id,
        default_session
    );
    fs::rename(&moved, &project)?;
    let returned = probe.navigate(Command::SelectWorkspace {
        workspace_id: workspace_id.clone(),
    })?;
    assert_eq!(
        returned.session.context("returning project Shell")?.id,
        session_id
    );
    let valid_after_switch = probe.path_state(&workspace_id, "valid")?;
    assert!(valid_after_switch.validation_generation > invalid_before_switch.validation_generation);
    probe.input(
        &session_id,
        r#"printf '%s\n' "${MATOU_WORKSPACE:-FRESH_AFTER_SWITCH}""#,
    )?;
    probe.output_all(
        &session_id,
        &["PROJECT_BEFORE_SWITCH_AWAY", "FRESH_AFTER_SWITCH"],
    )?;
    let recovered_after_switch = list_runs(&db, &session_id)?;
    assert_eq!(recovered_after_switch.len(), 3);
    assert_eq!(recovered_after_switch[1].id, recovered[1].id);
    assert_eq!(recovered_after_switch[1].status, "exited");
    assert_ne!(recovered_after_switch[2].id, recovered[1].id);
    assert_eq!(recovered_after_switch[2].status, "running");
    let still_background = list_runs(&db, &peer_session)?;
    assert_eq!(still_background.len(), 2);
    assert_eq!(still_background[1].id, peer_recovered[1].id);
    assert_eq!(still_background[1].status, "running");

    probe.input(
        &session_id,
        "printf 'PROJECT_%s\\n' WAITING; builtin read -r line",
    )?;
    probe.output(&session_id, "PROJECT_WAITING")?;
    assert_eq!(list_runs(&db, &session_id)?[2].status, "running");

    let retained = probe.navigate(Command::RemoveWorkspace {
        workspace_id: workspace_id.clone(),
    })?;
    assert_eq!(
        retained.workspace.context("fallback workspace")?.id,
        default_id
    );
    assert_eq!(
        retained.session.context("retained Shell")?.id,
        default_session
    );
    assert_eq!(
        probe
            .disposed
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([session_id.as_str(), peer_session.as_str()])
    );
    assert_eq!(probe.workspaces.len(), 1);
    assert_eq!(probe.tasks.len(), 1);
    assert_eq!(probe.tasks[0].workspace_id, default_id);
    probe.input(&default_session, r#"printf '%s\n' "$MATOU_DEFAULT""#)?;
    probe.output(&default_session, "DEFAULT_SURVIVES")?;
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if list_runs(&db, &session_id)?[2].status == "exited"
            && list_runs(&db, &peer_session)?
                .iter()
                .all(|run| run.status == "exited")
        {
            break;
        }
        if Instant::now() >= deadline {
            bail!("removed workspace Shell did not retire");
        }
        thread::sleep(Duration::from_millis(5));
    }
    drop(db);
    bridge.shutdown()?;

    let mut bridge = support::start(options)?;
    let mut probe = Probe::new(&bridge);
    let restored = probe.ready()?;
    assert_eq!(
        restored.workspace.context("restored workspace")?.id,
        default_id
    );
    assert_eq!(
        restored.session.context("restored Shell")?.id,
        default_session
    );
    assert_eq!(probe.workspaces.len(), 1);
    assert!(probe.workspaces[0].is_pinned);
    assert_eq!(probe.workspaces[0].pin_sort_key, "a00000001");
    assert_eq!(probe.tasks[0].workspace_id, default_id);
    probe.output(&default_session, "DEFAULT_SURVIVES")?;
    bridge.shutdown()?;
    Ok(())
}

type ActionResult = (String, Option<String>, Option<String>);

struct Probe<'a> {
    bridge: &'a RuntimeBridge,
    workspaces: Vec<Workspace>,
    tasks: Vec<Task>,
    disposed: Vec<String>,
    last_action: Option<ActionResult>,
    frames: BTreeSet<String>,
}

impl<'a> Probe<'a> {
    fn new(bridge: &'a RuntimeBridge) -> Self {
        Self {
            bridge,
            workspaces: Vec::new(),
            tasks: Vec::new(),
            disposed: Vec::new(),
            last_action: None,
            frames: BTreeSet::new(),
        }
    }

    fn send(&self, command: Command) -> Result<()> {
        self.bridge
            .commands
            .send(command)
            .map_err(|_| anyhow!("workspace bridge closed"))
    }

    fn input(&self, session_id: &str, text: &str) -> Result<()> {
        self.send(Command::Input {
            session_id: session_id.into(),
            bytes: format!("{text}\r").into_bytes(),
            defer_ordering: true,
        })
    }

    fn ready(&mut self) -> Result<HierarchyResult> {
        self.wait("workspace bootstrap", |event| match event {
            Event::Ready { hierarchy, .. } => Ok(Some(hierarchy)),
            _ => Ok(None),
        })
    }

    fn navigate(&mut self, command: Command) -> Result<HierarchyResult> {
        self.last_action = None;
        self.send(command)?;
        self.wait("workspace navigation", |event| match event {
            Event::Navigation { hierarchy, .. } => Ok(Some(hierarchy)),
            _ => Ok(None),
        })
    }

    fn output(&mut self, session_id: &str, expected: &str) -> Result<()> {
        self.output_all(session_id, &[expected])
    }

    fn path_state(&mut self, workspace_id: &str, status: &str) -> Result<WorkspacePathState> {
        self.wait(status, |event| match event {
            Event::WorkspacePaths(paths) => Ok(paths
                .into_iter()
                .find(|state| state.workspace_id == workspace_id && state.status == status)),
            _ => Ok(None),
        })
    }

    fn recovery_state(&mut self, session_id: &str, expected: &RecoveryState) -> Result<()> {
        self.wait("Shell recovery state", |event| match event {
            Event::Recovery {
                session_id: id,
                state,
            } if id == session_id => {
                assert_eq!(&state, expected);
                Ok(Some(()))
            }
            _ => Ok(None),
        })
    }

    fn output_all(&mut self, session_id: &str, expected: &[&str]) -> Result<()> {
        self.wait(&expected.join(", "), |event| match event {
            Event::Frame {
                session_id: current,
                screen,
                ..
            } if current == session_id => {
                let lines: Vec<_> = screen
                    .cells
                    .chunks(screen.cols)
                    .map(|row| {
                        row.iter()
                            .map(|cell| cell.text.as_str())
                            .collect::<String>()
                    })
                    .collect();
                let found = expected
                    .iter()
                    .all(|expected| lines.iter().any(|line| line.trim_end() == *expected));
                Ok(found.then_some(()))
            }
            _ => Ok(None),
        })
    }

    fn wait<T>(&mut self, label: &str, accept: impl Fn(Event) -> Result<Option<T>>) -> Result<T> {
        let deadline = Instant::now() + TIMEOUT;
        while Instant::now() < deadline {
            match self.bridge.events.try_recv() {
                Ok(event) => {
                    let failure = match &event {
                        Event::Diagnostic(message)
                        | Event::NavigationFailed { message, .. }
                        | Event::StorageFault { message, .. } => Some(message.clone()),
                        Event::Closed => Some("bridge closed".into()),
                        _ => None,
                    };
                    match &event {
                        Event::Workspaces(workspaces) => self.workspaces = workspaces.clone(),
                        Event::Tasks(tasks) => self.tasks = tasks.clone(),
                        Event::ScenesDisposed(ids) => self.disposed.extend(ids.clone()),
                        Event::WorkspaceActionResult {
                            action,
                            workspace_id,
                            error,
                        } => {
                            self.last_action =
                                Some((action.clone(), workspace_id.clone(), error.clone()));
                        }
                        Event::Frame {
                            session_id,
                            sequence,
                            ..
                        } => {
                            self.frames.insert(session_id.clone());
                            self.send(Command::Applied {
                                session_id: session_id.clone(),
                                sequence: *sequence,
                            })?;
                        }
                        _ => {}
                    }
                    if let Some(value) = accept(event)? {
                        return Ok(value);
                    }
                    if let Some(message) = failure {
                        bail!("{label}: {message}");
                    }
                }
                Err(async_channel::TryRecvError::Empty) => thread::sleep(Duration::from_millis(5)),
                Err(async_channel::TryRecvError::Closed) => bail!("{label}: event channel closed"),
            }
        }
        bail!("timed out waiting for {label}")
    }
}
