//! Desktop facade over the private native runtime child. No in-process fallback.
use std::{
    io,
    path::Path,
    process::{Command as ProcessCommand, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
};

use anyhow::{Context, Result, anyhow, bail, ensure};
use matou_native_runtime::protocol::{
    ClientMessage, ServerMessage, VERSION, check_version, read_messages, write_message,
};

pub use matou_native_runtime::host::{Command, Event, LaunchOptions, RecoveryState};

pub struct RuntimeBridge {
    pub commands: Sender<Command>,
    pub events: async_channel::Receiver<Event>,
    pid: u32,
    thread: Option<JoinHandle<Result<()>>>,
}

impl RuntimeBridge {
    pub fn start(options: LaunchOptions) -> Result<Self> {
        let executable = std::env::current_exe().context("locate native desktop executable")?;
        let binary = executable
            .parent()
            .context("native desktop executable has no parent")?
            .join(if cfg!(windows) {
                "matou-native-runtime.exe"
            } else {
                "matou-native-runtime"
            });
        Self::start_with_binary(options, &binary)
    }

    pub fn start_with_binary(options: LaunchOptions, binary: &Path) -> Result<Self> {
        let binary = binary.to_owned();
        let (commands, incoming) = mpsc::channel();
        let (send, events) = async_channel::unbounded();
        let (started, startup) = mpsc::channel();
        let wake_writer = commands.clone();
        // Spawn the manager before the child: failure to create this thread can
        // never strand a process. The manager owns every child cleanup path.
        let thread = thread::Builder::new()
            .name("matou-runtime-bridge".into())
            .spawn(move || run_child(options, &binary, incoming, wake_writer, send, started))
            .context("start native runtime bridge manager")?;
        match startup.recv() {
            Ok(Ok(pid)) => Ok(Self {
                commands,
                events,
                pid,
                thread: Some(thread),
            }),
            outcome => {
                drop(commands);
                let result = thread
                    .join()
                    .map_err(|_| anyhow!("native runtime manager panicked during startup"))?;
                result?;
                match outcome {
                    Ok(Err(error)) => bail!(error),
                    _ => bail!("native runtime manager ended before spawning child"),
                }
            }
        }
    }

    /// Actual child identity, also useful when diagnosing runtime ownership.
    pub fn runtime_pid(&self) -> u32 {
        self.pid
    }

    pub fn shutdown(&mut self) -> Result<()> {
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        let _ = self.commands.send(Command::Shutdown);
        thread
            .join()
            .map_err(|_| anyhow!("native runtime bridge manager panicked"))?
    }
}

impl Drop for RuntimeBridge {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
    }
}

fn forward_commands(
    mut stdin: impl io::Write,
    incoming: Receiver<Command>,
    hello: Receiver<()>,
    stop: &AtomicBool,
) -> Result<()> {
    write_message(&mut stdin, &ClientMessage::Hello { version: VERSION })?;
    hello
        .recv()
        .context("native runtime ended before compatible Hello")?;
    loop {
        let command = incoming
            .recv()
            .context("native runtime command sender ended")?;
        // The manager owns a Sender only to wake this recv on stop. Check after
        // receiving so a failed child never receives already-queued input.
        if stop.load(Ordering::Acquire) {
            return Ok(());
        }
        let shutdown = matches!(&command, Command::Shutdown);
        write_message(&mut stdin, &ClientMessage::Command(command))?;
        if shutdown {
            return Ok(()); // Drop stdin; keep reading stdout until real exit.
        }
    }
}

fn run_child(
    options: LaunchOptions,
    binary: &Path,
    incoming: Receiver<Command>,
    wake_writer: Sender<Command>,
    events: async_channel::Sender<Event>,
    started: Sender<std::result::Result<u32, String>>,
) -> Result<()> {
    let mut child = match ProcessCommand::new(binary)
        .arg("--data-root")
        .arg(&options.data_root)
        .arg("--workspace")
        .arg(&options.workspace)
        .arg("--shell-home")
        .arg(&options.shell_home)
        .arg("--shell")
        .arg(&options.shell)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .with_context(|| format!("start required native runtime child {}", binary.display()))
    {
        Ok(child) => child,
        Err(error) => {
            let _ = started.send(Err(format!("{error:#}")));
            return Err(error);
        }
    };
    // Stdio::piped guarantees both handles. Still route any acquisition failure
    // through the same child wait path rather than returning with a live child.
    let mut writer = None;
    let stop = Arc::new(AtomicBool::new(false));
    let read_result = (|| -> Result<()> {
        let stdin = child.stdin.take().context("native runtime child stdin")?;
        let stdout = child.stdout.take().context("native runtime child stdout")?;
        let (confirm, hello) = mpsc::channel();
        let writer_stop = stop.clone();
        writer = Some(
            thread::Builder::new()
                .name("matou-runtime-writer".into())
                .spawn(move || forward_commands(stdin, incoming, hello, &writer_stop))
                .context("start native runtime pipe writer")?,
        );
        // This means only that a process and its pipe writer exist. Ready is
        // forwarded exclusively from the domain owner after compatible Hello.
        let _ = started.send(Ok(child.id()));
        let mut messages = read_messages::<ServerMessage>(stdout);
        match messages
            .next()
            .context("native runtime EOF before Hello")??
        {
            ServerMessage::Hello { version } => check_version(version)?,
            ServerMessage::Event(_) => bail!("native runtime event before Hello"),
        }
        confirm
            .send(())
            .context("native runtime writer ended during Hello")?;
        let mut closed = false;
        for message in messages {
            ensure!(!closed, "native runtime sent data after Closed");
            match message.context("read native runtime event")? {
                ServerMessage::Hello { .. } => bail!("native runtime repeated Hello"),
                ServerMessage::Event(Event::Closed) => closed = true,
                ServerMessage::Event(event) => {
                    // Never ACK frames here: Workspace owns application credit.
                    if events.send_blocking(event).is_err() {
                        stop.store(true, Ordering::Release);
                        let _ = wake_writer.send(Command::Shutdown);
                    }
                }
            }
        }
        ensure!(closed, "native runtime EOF without Closed");
        Ok(())
    })();
    // The Hello confirmation sender has left scope, releasing a Hello wait on
    // every read failure. Wake the command recv explicitly before joining. The
    // manager's Sender keeps that channel connected, so disconnect is not an
    // exit signal; normal shutdown comes from RuntimeBridge::shutdown/Drop.
    stop.store(true, Ordering::Release);
    let _ = wake_writer.send(Command::Shutdown);
    drop(child.stdin.take());
    drop(child.stdout.take());
    let write_result = match writer {
        Some(writer) => writer
            .join()
            .map_err(|_| anyhow!("native runtime pipe writer panicked"))
            .and_then(|result| result),
        None => Ok(()),
    };
    let wait_result = child.wait().context("wait for native runtime child exit");
    // Preserve exit and transport failures together; neither Closed nor a
    // secondary broken pipe should erase the actual cleanup outcome.
    let mut failures = Vec::new();
    match wait_result {
        Ok(status) if !status.success() => {
            failures.push(format!(
                "native runtime child {} exited {status}",
                child.id()
            ));
        }
        Err(error) => failures.push(format!("{error:#}")),
        _ => {}
    }
    for outcome in [read_result, write_result] {
        if let Err(error) = outcome {
            failures.push(format!("{error:#}"));
        }
    }
    let result = if failures.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(failures.join("; ")))
    };
    if let Err(error) = &result {
        let text = format!("{error:#}");
        let _ = started.send(Err(text.clone()));
        let _ = events.send_blocking(Event::Diagnostic(text));
    }
    // Exactly one terminal notification, after the real wait and any diagnostic.
    let _ = events.send_blocking(Event::Closed);
    result
}
