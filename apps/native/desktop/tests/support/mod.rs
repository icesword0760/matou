use anyhow::{Context, Result};
use matou_native_desktop::runtime_bridge::{
    BridgeEvent, Command, ConnectionPhase, Event, LaunchOptions, RuntimeBridge, RuntimeCommands,
    SendError,
};
use std::path::{Path, PathBuf};

pub fn runtime_binary() -> Result<PathBuf> {
    let test = std::env::current_exe()?;
    let debug = test
        .parent()
        .and_then(|deps| deps.parent())
        .context("test debug/deps path")?;
    Ok(debug.join("matou-native-runtime"))
}
// Existing business contracts are deliberately leased to generation 1. Neither
// dequeuing an event nor sending an old ACK can silently upgrade this lease.
pub struct TestCommands {
    commands: RuntimeCommands,
    generation: u64,
}
impl TestCommands {
    pub fn send(&self, command: Command) -> std::result::Result<(), SendError> {
        self.commands.send_at(self.generation, command)
    }
}
pub struct TestEvents {
    events: async_channel::Receiver<BridgeEvent>,
    generation: u64,
}
impl TestEvents {
    pub fn try_recv(&self) -> std::result::Result<Event, async_channel::TryRecvError> {
        loop {
            match self.events.try_recv()? {
                BridgeEvent::Runtime { generation, event } => {
                    assert_eq!(
                        generation, self.generation,
                        "business contract unexpectedly crossed runtime generation"
                    );
                    return Ok(event);
                }
                BridgeEvent::Connection { generation, phase } => {
                    assert_eq!(
                        generation, self.generation,
                        "business contract unexpectedly reconnected"
                    );
                    if let ConnectionPhase::Blocked(reason) = phase {
                        return Ok(Event::StartupBlocked(reason));
                    }
                }
            }
        }
    }
    pub fn close(&self) -> bool {
        self.events.close()
    }
}
pub struct TestBridge {
    inner: RuntimeBridge,
    pub commands: TestCommands,
    pub events: TestEvents,
}
impl TestBridge {
    pub fn start_with_binary(options: LaunchOptions, binary: &Path) -> Result<Self> {
        let inner = RuntimeBridge::start_with_binary(options, binary)?;
        let commands = TestCommands {
            commands: inner.commands.clone(),
            generation: 1,
        };
        let events = TestEvents {
            events: inner.events.clone(),
            generation: 1,
        };
        Ok(Self {
            inner,
            commands,
            events,
        })
    }
    pub fn runtime_pid(&self) -> u32 {
        self.inner.runtime_pid()
    }
    pub fn connection(&self) -> (u64, ConnectionPhase) {
        self.inner.commands.connection()
    }

    pub fn shutdown(&mut self) -> Result<()> {
        self.inner.shutdown()
    }
}
pub fn start(options: LaunchOptions) -> Result<TestBridge> {
    TestBridge::start_with_binary(options, &runtime_binary()?)
}
