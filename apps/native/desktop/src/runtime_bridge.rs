//! Native desktop connection to the serialized domain owner.
//!
//! No SQLite, PTY, or Ghostty handle is accessed on the GPUI thread. The current
//! vertical slice hosts the owner in a background thread; standalone runtime
//! transport and additional windows remain separate migration work.
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ffi::OsString,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use matou_native_core::{
    hierarchy::{self, BootstrapWindowInput, HierarchyIds, HierarchyResult},
    journal::JournalOptions,
    pty::ShellLaunch,
    startup::{self, BootstrapResult},
    storage::CommandMetadata,
};
use matou_native_runtime::{
    SessionEvent, SessionOptions,
    owner::{OwnerEvent, ShellOwner},
    worker::WorkerCommand,
};
use matou_native_terminal::{InputModes, Screen, TerminalOptions, Theme};
use serde_json::Value;
use uuid::Uuid;

#[derive(Clone)]
pub struct LaunchOptions {
    pub data_root: PathBuf,
    pub workspace: PathBuf,
    /// The acceptance launcher supplies a disposable home. No user shell files
    /// are rewritten or copied to this directory.
    pub shell_home: PathBuf,
    pub shell: PathBuf,
}

pub enum Command {
    Input {
        session_id: String,
        bytes: Vec<u8>,
    },
    Resize {
        session_id: String,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    },
    Scroll {
        session_id: String,
        lines: i32,
    },
    Theme {
        session_id: String,
        theme: Theme,
    },
    Applied {
        session_id: String,
        sequence: u64,
    },
    RetryStorage {
        session_id: String,
    },
    Refresh,
    Shutdown,
}

pub enum Event {
    Ready {
        hierarchy: HierarchyResult,
        graph: Value,
    },
    Graph(Value),
    Frame {
        session_id: String,
        screen: Screen,
        modes: InputModes,
        sequence: u64,
    },
    StorageFault {
        session_id: String,
        message: String,
    },
    StorageRecovered {
        session_id: String,
    },
    Diagnostic(String),
    Closed,
}

pub struct RuntimeBridge {
    pub commands: Sender<Command>,
    pub events: async_channel::Receiver<Event>,
    thread: Option<JoinHandle<Result<()>>>,
}

impl RuntimeBridge {
    pub fn start(options: LaunchOptions) -> Result<Self> {
        let (commands, incoming) = mpsc::channel();
        let (send, events) = async_channel::unbounded();
        let thread = thread::Builder::new()
            .name("matou-domain-owner".into())
            .spawn(move || {
                let result = run(options, incoming, &send);
                if let Err(error) = &result {
                    let _ = send.send_blocking(Event::Diagnostic(format!("{error:#}")));
                }
                let _ = send.send_blocking(Event::Closed);
                result
            })?;
        Ok(Self {
            commands,
            events,
            thread: Some(thread),
        })
    }

    pub fn shutdown(&mut self) -> Result<()> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        let _ = self.commands.send(Command::Shutdown);
        thread
            .join()
            .map_err(|_| anyhow!("native runtime thread panicked"))?
    }
}

impl Drop for RuntimeBridge {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
    }
}

struct PendingView {
    receive: Receiver<Result<(Screen, InputModes)>>,
    sequence: u64,
}

fn run(
    options: LaunchOptions,
    incoming: Receiver<Command>,
    send: &async_channel::Sender<Event>,
) -> Result<()> {
    std::fs::create_dir_all(&options.workspace)?;
    std::fs::create_dir_all(&options.shell_home)?;
    let generation = id();
    let (mut connection, ownership) = match startup::open(&options.data_root, &generation)? {
        BootstrapResult::Writable {
            connection,
            ownership,
            ..
        } => (connection, ownership),
        BootstrapResult::ReadOnly { reason, .. } => {
            bail!("Native startup requires the recovery interface: {reason:?}")
        }
        BootstrapResult::RecoveryRequired(recovery) => bail!(
            "Database recovery required: {} ({})",
            recovery.reason,
            recovery.durable_database_path.display()
        ),
    };
    let window_id = "main-window-1";
    let input = BootstrapWindowInput {
        window_id: window_id.into(),
        default_root_directory: options.workspace.to_string_lossy().into_owned(),
        default_name: options
            .workspace
            .file_name()
            .context("workspace name")?
            .to_string_lossy()
            .into_owned(),
        now: now(),
    };
    let hierarchy = hierarchy::bootstrap_window(
        &mut connection,
        &CommandMetadata {
            command_id: id(),
            command_type: "window.bootstrap".into(),
            request_hash: serde_json::to_string(&input)?,
            causation_id: None,
            correlation_id: None,
        },
        &input,
        &HierarchyIds {
            workspace_id: id(),
            execution_context_id: id(),
            task_id: id(),
            scene_id: id(),
            root_node_id: id(),
            session_id: id(),
            mount_id: id(),
        },
        "默认",
        now(),
    )?;
    let scene_id = hierarchy
        .scene
        .as_ref()
        .context("active native scene")?
        .id
        .clone();
    let mut owner = ShellOwner::new(connection, ownership, id)?;
    let graph = owner.graph(&scene_id, None)?;
    let nodes = graph["nodes"].as_array().context("scene graph nodes")?;
    let mut sessions = Vec::new();
    for node in nodes {
        if node["currentMode"] != "shell" {
            continue;
        }
        let session_id = node["sessionId"]
            .as_str()
            .context("graph session id")?
            .to_owned();
        let stored = owner
            .session(&session_id)?
            .context("stored scene session")?;
        if stored.archived_at.is_some() {
            continue;
        }
        let mut inherited_env: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
        inherited_env.insert("HOME".into(), options.shell_home.clone().into_os_string());
        inherited_env.insert(
            "ZDOTDIR".into(),
            options.shell_home.clone().into_os_string(),
        );
        inherited_env.insert("SHELL".into(), options.shell.clone().into_os_string());
        owner.launch(
            SessionOptions {
                session_id: session_id.clone(),
                launch: ShellLaunch {
                    executable: options.shell.clone(),
                    cwd: stored.cwd.into(),
                    data_root: options.data_root.clone(),
                    inherited_env,
                    env: BTreeMap::new(),
                    cols: 80,
                    rows: 24,
                },
                terminal: TerminalOptions::new(80, 24, 7, 14),
                journal: JournalOptions::default(),
                restored_history: Vec::new(),
                collect_history: true,
            },
            true,
            "──────── 会话已恢复 ────────",
        )?;
        sessions.push(session_id);
    }
    send.send_blocking(Event::Ready {
        hierarchy,
        graph: owner.graph(&scene_id, None)?,
    })?;
    let mut dirty: HashSet<String> = sessions.iter().cloned().collect();
    let mut sequence = HashMap::<String, u64>::new();
    let mut views = HashMap::<String, PendingView>::new();
    let mut awaiting_ui = HashSet::<String>::new();
    let mut operations = Vec::<(String, Receiver<Result<()>>, bool)>::new();
    loop {
        let mut shutdown = false;
        for _ in 0..128 {
            let command = match incoming.try_recv() {
                Ok(command) => command,
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    shutdown = true;
                    break;
                }
            };
            let (reply, result) = mpsc::channel();
            let (session_id, request, refresh) = match command {
                Command::Input { session_id, bytes } => {
                    (session_id, WorkerCommand::Write { bytes, reply }, false)
                }
                Command::Resize {
                    session_id,
                    cols,
                    rows,
                    cell_width_px,
                    cell_height_px,
                } => (
                    session_id,
                    WorkerCommand::Resize {
                        cols,
                        rows,
                        cell_width_px,
                        cell_height_px,
                        reply,
                    },
                    true,
                ),
                Command::Scroll { session_id, lines } => {
                    (session_id, WorkerCommand::Scroll { lines, reply }, true)
                }
                Command::Theme { session_id, theme } => {
                    (session_id, WorkerCommand::SetTheme { theme, reply }, true)
                }
                Command::Applied {
                    session_id,
                    sequence,
                } => {
                    awaiting_ui.remove(&session_id);
                    // A first layout/theme frame can precede any PTY output.
                    // Zero means no output credit has been sent yet; it still
                    // releases the UI frame slot, but has nothing to acknowledge.
                    if sequence == 0 {
                        continue;
                    }
                    (
                        session_id,
                        WorkerCommand::Acknowledge { sequence, reply },
                        false,
                    )
                }
                Command::RetryStorage { session_id } => {
                    (session_id, WorkerCommand::RetryStorage { reply }, true)
                }
                Command::Refresh => {
                    dirty.extend(sessions.iter().cloned());
                    awaiting_ui.clear();
                    send.send_blocking(Event::Graph(owner.graph(&scene_id, None)?))?;
                    continue;
                }
                Command::Shutdown => {
                    shutdown = true;
                    break;
                }
            };
            if let Err(error) = owner.send(&session_id, request) {
                send.send_blocking(Event::Diagnostic(format!("{error:#}")))?;
            } else {
                operations.push((session_id, result, refresh));
            }
        }
        if shutdown {
            break;
        }

        let mut graph_dirty = false;
        for event in owner.poll(Duration::from_millis(4))? {
            match event {
                OwnerEvent::Domain(_) => graph_dirty = true,
                OwnerEvent::Diagnostic { message, .. } => {
                    send.send_blocking(Event::Diagnostic(message))?
                }
                OwnerEvent::Terminal(event) => {
                    let session_id = event.session_id;
                    match event.event {
                        SessionEvent::Output { sequence: next, .. } => {
                            sequence.insert(session_id.clone(), next);
                            dirty.insert(session_id);
                        }
                        // The display credit window accounts for output only;
                        // start/resize/exit records are not acknowledged as bytes.
                        SessionEvent::Started { .. }
                        | SessionEvent::Resized { .. }
                        | SessionEvent::Exited { .. } => {
                            dirty.insert(session_id);
                        }
                        SessionEvent::ScreenInvalidated => {
                            dirty.insert(session_id);
                        }
                        SessionEvent::StorageFault { message, .. } => {
                            send.send_blocking(Event::StorageFault {
                                session_id,
                                message,
                            })?
                        }
                        SessionEvent::StorageRecovered { .. } => {
                            dirty.insert(session_id.clone());
                            send.send_blocking(Event::StorageRecovered { session_id })?;
                        }
                        SessionEvent::Diagnostic { message } => {
                            send.send_blocking(Event::Diagnostic(message))?
                        }
                        _ => {}
                    }
                }
            }
        }
        if graph_dirty {
            send.send_blocking(Event::Graph(owner.graph(&scene_id, None)?))?;
        }
        operations.retain(|(session_id, result, refresh)| match result.try_recv() {
            Ok(Ok(())) => {
                if *refresh {
                    dirty.insert(session_id.clone());
                }
                false
            }
            Ok(Err(error)) => {
                let _ = send.send_blocking(Event::Diagnostic(format!("{error:#}")));
                false
            }
            Err(mpsc::TryRecvError::Empty) => true,
            Err(mpsc::TryRecvError::Disconnected) => {
                let _ = send.send_blocking(Event::Diagnostic("Shell request disconnected".into()));
                false
            }
        });
        views.retain(|session_id, pending| match pending.receive.try_recv() {
            Ok(Ok((screen, modes))) => {
                awaiting_ui.insert(session_id.clone());
                let _ = send.send_blocking(Event::Frame {
                    session_id: session_id.clone(),
                    screen,
                    modes,
                    sequence: pending.sequence,
                });
                false
            }
            Ok(Err(error)) => {
                let _ = send.send_blocking(Event::Diagnostic(format!("{error:#}")));
                false
            }
            Err(mpsc::TryRecvError::Empty) => true,
            Err(mpsc::TryRecvError::Disconnected) => {
                let _ = send.send_blocking(Event::Diagnostic("Shell view disconnected".into()));
                false
            }
        });
        for session_id in dirty.clone() {
            if views.contains_key(&session_id) || awaiting_ui.contains(&session_id) {
                continue;
            }
            let (reply, receive) = mpsc::channel();
            owner.send(&session_id, WorkerCommand::View { reply })?;
            views.insert(
                session_id.clone(),
                PendingView {
                    receive,
                    sequence: sequence.get(&session_id).copied().unwrap_or(0),
                },
            );
            dirty.remove(&session_id);
        }
    }
    owner.shutdown()?;
    Ok(())
}

fn id() -> String {
    Uuid::new_v4().to_string()
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
