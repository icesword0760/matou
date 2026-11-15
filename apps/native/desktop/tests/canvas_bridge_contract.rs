#![cfg(target_os = "macos")]

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use matou_native_core::hierarchy::{HierarchyResult, Scene, Task};
use matou_native_desktop::runtime_bridge::{Command, Event, LaunchOptions, RuntimeBridge};
use serde_json::Value;
use uuid::Uuid;

const TIMEOUT: Duration = Duration::from_secs(8);
const PROMPT: &str = "MATOU_CANVAS_READY>";
const DIVIDER: &str = "──────── 会话已恢复 ────────";

#[test]
fn canvas_navigation_retains_live_shells_and_restores_the_active_path() -> Result<()> {
    let root = std::env::temp_dir().join(format!("matou-native-canvas-{}", Uuid::new_v4()));
    verify_canvas(&root).with_context(|| format!("isolated canvas data: {}", root.display()))?;
    fs::remove_dir_all(root)?;
    Ok(())
}

fn verify_canvas(root: &Path) -> Result<()> {
    let home = root.join("home");
    fs::create_dir_all(&home)?;
    fs::write(home.join(".zshrc"), format!("PROMPT='{PROMPT} '\n"))?;
    let options = LaunchOptions {
        data_root: root.join("data"),
        workspace: root.join("workspace"),
        shell_home: home,
        shell: PathBuf::from("/bin/zsh"),
    };
    let mut bridge = RuntimeBridge::start(options.clone())?;
    let mut probe = Probe::new(&bridge);
    let initial = probe.ready("initial Ready")?;
    assert_eq!(initial.scenes, [initial.scene_id.as_str()]);
    assert_eq!(initial.nodes, BTreeSet::from([initial.session_id.clone()]));
    probe.frame(&initial.session_id, "original prompt", &[PROMPT])?;
    probe.input(
        &initial.session_id,
        "export MATOU_CANVAS_LIVE=ORIGINAL_ENV_KEPT; printf 'CANVAS_%s\\n' ORIGINAL_STARTED; until [ -f .matou-release-delayed ]; do sleep 0.05; done; printf 'CANVAS_%s\\n' ORIGINAL_DELAYED",
    )?;
    probe.frame(
        &initial.session_id,
        "original command started",
        &["CANVAS_ORIGINAL_STARTED"],
    )?;

    let canvas = probe.navigate(
        "create canvas",
        Command::CreateCanvas {
            task_id: initial.task_id.clone(),
        },
    )?;
    assert_eq!(canvas.task_id, initial.task_id);
    assert_ne!(canvas.scene_id, initial.scene_id);
    assert_ne!(canvas.session_id, initial.session_id);
    assert_eq!(
        canvas.scenes,
        [initial.scene_id.as_str(), canvas.scene_id.as_str()]
    );
    assert_eq!(canvas.nodes, BTreeSet::from([canvas.session_id.clone()]));
    probe.frame(&canvas.session_id, "new canvas prompt", &[PROMPT])?;
    probe.input(&canvas.session_id, "printf 'CANVAS_%s\\n' FIRST_READY")?;
    probe.frame(
        &canvas.session_id,
        "first canvas output",
        &["CANVAS_FIRST_READY", PROMPT],
    )?;

    let sibling = probe.navigate(
        "create sibling Shell",
        Command::CreateShellSibling {
            scene_id: canvas.scene_id.clone(),
            source_session_id: canvas.session_id.clone(),
            parent_session_id: None,
        },
    )?;
    assert_eq!(sibling.scene_id, canvas.scene_id);
    assert_ne!(sibling.session_id, canvas.session_id);
    assert_ne!(sibling.session_id, initial.session_id);
    assert_eq!(sibling.scenes, canvas.scenes);
    assert_eq!(
        sibling.nodes,
        BTreeSet::from([canvas.session_id.clone(), sibling.session_id.clone()])
    );
    probe.frame(&sibling.session_id, "sibling prompt", &[PROMPT])?;
    probe.input(&sibling.session_id, "printf 'CANVAS_%s\\n' SIBLING_READY")?;
    probe.frame(
        &sibling.session_id,
        "sibling output",
        &["CANVAS_SIBLING_READY", PROMPT],
    )?;
    assert!(!probe.contains(&sibling.session_id, &["CANVAS_FIRST_READY"]));

    // Release the original command only after both canvas workers are live.
    fs::write(
        options.workspace.join(".matou-release-delayed"),
        b"continue\n",
    )?;
    probe.frame(
        &initial.session_id,
        "original output while its scene is in the background",
        &["CANVAS_ORIGINAL_DELAYED", PROMPT],
    )?;
    let back = probe.navigate(
        "select original scene",
        Command::SelectScene {
            scene_id: initial.scene_id.clone(),
        },
    )?;
    assert_eq!(
        (back.scene_id, back.session_id),
        (initial.scene_id.clone(), initial.session_id.clone())
    );
    probe.frame(
        &initial.session_id,
        "original display after scene switch",
        &["CANVAS_ORIGINAL_DELAYED", PROMPT],
    )?;
    probe.input(
        &initial.session_id,
        "printf 'CANVAS_%s\\n' \"$MATOU_CANVAS_LIVE\"",
    )?;
    probe.frame(
        &initial.session_id,
        "environment retained by original Shell",
        &["CANVAS_ORIGINAL_ENV_KEPT", PROMPT],
    )?;

    let selected = probe.navigate(
        "return to the new canvas",
        Command::SelectScene {
            scene_id: canvas.scene_id.clone(),
        },
    )?;
    assert_eq!(selected.scene_id, canvas.scene_id);
    assert_eq!(selected.session_id, sibling.session_id);
    let first = probe.navigate(
        "focus first canvas session",
        Command::FocusSession {
            scene_id: canvas.scene_id.clone(),
            session_id: canvas.session_id.clone(),
        },
    )?;
    assert_eq!(first.session_id, canvas.session_id);
    probe.input(&first.session_id, "printf 'CANVAS_%s\\n' FIRST_FOCUSED")?;
    probe.frame(
        &first.session_id,
        "input addressed to first canvas session",
        &["CANVAS_FIRST_READY", "CANVAS_FIRST_FOCUSED", PROMPT],
    )?;
    let active = probe.navigate(
        "focus sibling canvas session",
        Command::FocusSession {
            scene_id: canvas.scene_id.clone(),
            session_id: sibling.session_id.clone(),
        },
    )?;
    assert_eq!(active.session_id, sibling.session_id);
    probe.input(&active.session_id, "printf 'CANVAS_%s\\n' SIBLING_FOCUSED")?;
    probe.frame(
        &active.session_id,
        "input addressed to sibling canvas session",
        &["CANVAS_SIBLING_READY", "CANVAS_SIBLING_FOCUSED", PROMPT],
    )?;
    for marker in ["CANVAS_SIBLING_READY", "CANVAS_SIBLING_FOCUSED"] {
        assert!(
            !probe.contains(&first.session_id, &[marker]),
            "sibling output reached first Shell: {marker}"
        );
    }
    for marker in ["CANVAS_FIRST_READY", "CANVAS_FIRST_FOCUSED"] {
        assert!(
            !probe.contains(&active.session_id, &[marker]),
            "first output reached sibling Shell: {marker}"
        );
    }
    bridge.shutdown()?;

    let mut reopened = RuntimeBridge::start(options)?;
    let mut probe = Probe::new(&reopened);
    let restored = probe.ready("Ready after reopen")?;
    assert_eq!(restored, active);
    probe.frame(
        &restored.session_id,
        "completed history of the active Shell after reopen",
        &[
            "CANVAS_SIBLING_READY",
            "CANVAS_SIBLING_FOCUSED",
            DIVIDER,
            PROMPT,
        ],
    )?;
    reopened.shutdown()?;
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
struct Navigation {
    workspace_id: String,
    task_id: String,
    scene_id: String,
    session_id: String,
    scenes: Vec<String>,
    nodes: BTreeSet<String>,
}

impl Navigation {
    fn read(hierarchy: HierarchyResult, scenes: Vec<Scene>, graph: Value) -> Result<Self> {
        let scene_id = hierarchy
            .scene
            .context("navigation has no active scene")?
            .id;
        let session_id = hierarchy
            .session
            .context("navigation has no active session")?
            .id;
        ensure!(
            graph["sceneId"] == scene_id,
            "graph scene disagrees with navigation"
        );
        ensure!(
            graph["focusedSessionId"] == session_id,
            "graph focus disagrees with navigation"
        );
        let nodes = graph["nodes"]
            .as_array()
            .context("navigation graph has no nodes")?
            .iter()
            .map(|node| {
                ensure!(
                    node["currentMode"] == "shell",
                    "unexpected non-shell graph node"
                );
                Ok(node["sessionId"]
                    .as_str()
                    .context("graph node has no session ID")?
                    .to_owned())
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            workspace_id: hierarchy
                .workspace
                .context("navigation has no workspace")?
                .id,
            task_id: hierarchy.task.context("navigation has no active task")?.id,
            scene_id,
            session_id,
            scenes: scenes.into_iter().map(|scene| scene.id).collect(),
            nodes,
        })
    }
}

struct Probe<'a> {
    bridge: &'a RuntimeBridge,
    frames: BTreeMap<String, (u64, Vec<String>)>,
    tasks: Vec<Task>,
    disposed: Vec<String>,
}

impl<'a> Probe<'a> {
    fn new(bridge: &'a RuntimeBridge) -> Self {
        Self {
            bridge,
            frames: BTreeMap::new(),
            tasks: Vec::new(),
            disposed: Vec::new(),
        }
    }

    fn ready(&mut self, phase: &str) -> Result<Navigation> {
        self.wait(phase, |_, event| match event {
            Event::Ready {
                hierarchy,
                scenes,
                graph,
                ..
            } => Ok(Some(Navigation::read(hierarchy, scenes, graph)?)),
            _ => Ok(None),
        })
    }

    fn navigate(&mut self, phase: &str, command: Command) -> Result<Navigation> {
        self.send(command)?;
        self.wait(phase, |_, event| match event {
            Event::Navigation {
                hierarchy,
                scenes,
                graph,
                ..
            } => Ok(Some(Navigation::read(hierarchy, scenes, graph)?)),
            _ => Ok(None),
        })
    }

    fn send(&self, command: Command) -> Result<()> {
        self.bridge
            .commands
            .send(command)
            .map_err(|_| anyhow!("runtime command channel closed"))
    }

    fn input(&self, session_id: &str, command: &str) -> Result<()> {
        self.send(Command::Input {
            session_id: session_id.to_owned(),
            bytes: format!("{command}\r").into_bytes(),
            defer_ordering: false,
        })
    }

    fn contains(&self, session_id: &str, expected: &[&str]) -> bool {
        self.frames.get(session_id).is_some_and(|(_, rows)| {
            expected
                .iter()
                .all(|expected| rows.iter().any(|row| row == expected))
        })
    }

    fn frame(&mut self, session_id: &str, phase: &str, expected: &[&str]) -> Result<()> {
        self.wait(phase, |probe, event| {
            let current = matches!(event, Event::Frame { session_id: id, .. } if id == session_id);
            Ok((current && probe.contains(session_id, expected)).then_some(()))
        })
    }

    fn wait<T>(
        &mut self,
        phase: &str,
        accept: impl Fn(&Self, Event) -> Result<Option<T>>,
    ) -> Result<T> {
        self.wait_until(Instant::now() + TIMEOUT, accept)
            .with_context(|| format!("{phase}, last frames: {:#?}", self.frames))
    }

    fn wait_until<T>(
        &mut self,
        deadline: Instant,
        accept: impl Fn(&Self, Event) -> Result<Option<T>>,
    ) -> Result<T> {
        while Instant::now() < deadline {
            match self.bridge.events.try_recv() {
                Ok(Event::Diagnostic(message)) => bail!("bridge diagnostic: {message}"),
                Ok(Event::StorageFault {
                    session_id,
                    message,
                }) => bail!("{session_id} storage fault: {message}"),
                Ok(Event::Closed) | Err(async_channel::TryRecvError::Closed) => {
                    bail!("bridge closed")
                }
                Ok(event) => {
                    if let Event::Tasks(tasks) = &event {
                        self.tasks = tasks.clone();
                    }
                    if let Event::ScenesDisposed(ids) = &event {
                        self.disposed.extend(ids.clone());
                    }
                    if let Event::NavigationFailed { message, .. } = &event {
                        bail!("navigation failed: {message}");
                    }
                    if let Event::Frame {
                        session_id,
                        screen,
                        sequence,
                        ..
                    } = &event
                    {
                        self.send(Command::Applied {
                            session_id: session_id.clone(),
                            sequence: *sequence,
                        })?;
                        let rows = screen
                            .cells
                            .chunks(screen.cols)
                            .map(|cells| {
                                let row = cells
                                    .iter()
                                    .map(|cell| cell.text.as_str())
                                    .collect::<String>();
                                row.trim_end_matches(' ').to_owned()
                            })
                            .filter(|row| !row.is_empty())
                            .collect();
                        self.frames.insert(session_id.clone(), (*sequence, rows));
                    }
                    if let Some(result) = accept(self, event)? {
                        return Ok(result);
                    }
                }
                Err(async_channel::TryRecvError::Empty) => thread::sleep(Duration::from_millis(4)),
            }
        }
        bail!("timed out waiting for bridge event")
    }
}

#[test]
fn task_lifecycle_uses_independent_shells_and_retires_all_deleted_sessions() -> Result<()> {
    let root = std::env::temp_dir().join(format!("matou-native-tasks-{}", Uuid::new_v4()));
    let result = verify_tasks(&root);
    result.with_context(|| format!("isolated task data: {}", root.display()))?;
    fs::remove_dir_all(root)?;
    Ok(())
}

fn verify_tasks(root: &Path) -> Result<()> {
    let home = root.join("home");
    fs::create_dir_all(&home)?;
    fs::write(home.join(".zshrc"), format!("PROMPT='{PROMPT} '\n"))?;
    let options = LaunchOptions {
        data_root: root.join("data"),
        workspace: root.join("workspace"),
        shell_home: home,
        shell: PathBuf::from("/bin/zsh"),
    };
    let mut bridge = RuntimeBridge::start(options.clone())?;
    let mut probe = Probe::new(&bridge);
    let first = probe.ready("initial task")?;
    probe.frame(&first.session_id, "first task prompt", &[PROMPT])?;
    probe.input(
        &first.session_id,
        "export TASK_PERSISTED=TASK_KEPT; printf 'TASK_%s\\n' ORIGINAL",
    )?;
    probe.frame(
        &first.session_id,
        "first task output",
        &["TASK_ORIGINAL", PROMPT],
    )?;
    let second = probe.navigate(
        "new task",
        Command::CreateTask {
            workspace_id: first.workspace_id.clone(),
        },
    )?;
    assert_ne!(first.task_id, second.task_id);
    assert_eq!(probe.tasks.len(), 2);
    assert_eq!(second.nodes, BTreeSet::from([second.session_id.clone()]));
    probe.frame(&second.session_id, "new task shell", &[PROMPT])?;
    probe.send(Command::Input {
        session_id: second.session_id.clone(),
        bytes: b"printf 'TASK_%s\\n' RECENCY".to_vec(),
        defer_ordering: true,
    })?;
    probe.send(Command::Input {
        session_id: second.session_id.clone(),
        bytes: vec![13],
        defer_ordering: true,
    })?;
    probe.frame(
        &second.session_id,
        "submitted input updates task recency",
        &["TASK_RECENCY", PROMPT],
    )?;
    assert_eq!(probe.tasks[0].id, second.task_id);
    assert!(probe.tasks[0].last_opened_at > 0);

    let sibling = probe.navigate(
        "new task second shell",
        Command::CreateShellSibling {
            scene_id: second.scene_id.clone(),
            source_session_id: second.session_id.clone(),
            parent_session_id: None,
        },
    )?;
    probe.frame(&sibling.session_id, "new task sibling prompt", &[PROMPT])?;
    probe.navigate(
        "rename task",
        Command::RenameTask {
            task_id: second.task_id.clone(),
            title: "中文事项".into(),
        },
    )?;
    assert_eq!(
        probe
            .tasks
            .iter()
            .find(|task| task.id == second.task_id)
            .unwrap()
            .title,
        "中文事项"
    );
    probe.navigate(
        "pin task",
        Command::SetTaskPinned {
            task_id: second.task_id.clone(),
            pinned: true,
        },
    )?;
    assert_eq!(probe.tasks[0].id, second.task_id);
    assert!(probe.tasks[0].is_pinned);
    let back = probe.navigate(
        "back to original task",
        Command::SelectTask {
            task_id: first.task_id.clone(),
        },
    )?;
    assert_eq!(back.session_id, first.session_id);
    probe.input(&back.session_id, "printf '%s\\n' \"$TASK_PERSISTED\"")?;
    probe.frame(
        &back.session_id,
        "original environment survives task switching",
        &["TASK_KEPT", PROMPT],
    )?;
    let restored = probe.navigate(
        "back to new task",
        Command::SelectTask {
            task_id: second.task_id.clone(),
        },
    )?;
    assert_eq!(restored.session_id, sibling.session_id);
    probe.navigate(
        "select original before deletion",
        Command::SelectTask {
            task_id: first.task_id.clone(),
        },
    )?;
    let retained = probe.navigate(
        "delete other task",
        Command::DeleteTask {
            task_id: second.task_id.clone(),
        },
    )?;
    assert_eq!(retained.task_id, first.task_id);
    assert_eq!(probe.tasks.len(), 1);
    assert_eq!(
        probe.disposed.iter().cloned().collect::<BTreeSet<_>>(),
        BTreeSet::from([second.session_id, sibling.session_id])
    );
    probe.input(&retained.session_id, "printf 'TASK_%s\\n' AFTER_DELETE")?;
    probe.frame(
        &retained.session_id,
        "retained Shell usable after deletion",
        &["TASK_AFTER_DELETE", PROMPT],
    )?;
    let replacement = probe.navigate(
        "delete final task",
        Command::DeleteTask {
            task_id: first.task_id.clone(),
        },
    )?;
    assert_ne!(replacement.task_id, first.task_id);
    assert_eq!(probe.tasks.len(), 1);
    assert_eq!(probe.tasks[0].title, "默认");
    probe.frame(
        &replacement.session_id,
        "replacement Shell prompt",
        &[PROMPT],
    )?;
    probe.input(&replacement.session_id, "printf 'TASK_%s\\n' REPLACEMENT")?;
    probe.frame(
        &replacement.session_id,
        "replacement output",
        &["TASK_REPLACEMENT", PROMPT],
    )?;
    bridge.shutdown()?;
    let mut reopened = RuntimeBridge::start(options)?;
    let mut probe = Probe::new(&reopened);
    assert_eq!(probe.ready("task reopened")?, replacement);
    assert_eq!(probe.tasks.len(), 1);
    probe.frame(
        &replacement.session_id,
        "replacement history restored",
        &["TASK_REPLACEMENT", DIVIDER, PROMPT],
    )?;
    reopened.shutdown()?;
    Ok(())
}
