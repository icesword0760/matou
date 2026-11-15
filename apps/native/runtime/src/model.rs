use matou_native_core::{
    journal::JournalOptions,
    pty::{ProcessExit, ShellLaunch},
    shell_history::CollectedShellBlock,
};
use matou_native_terminal::TerminalOptions;

pub struct SessionOptions {
    pub session_id: String,
    pub launch: ShellLaunch,
    pub terminal: TerminalOptions,
    pub journal: JournalOptions,
    /// Completed Shell blocks, formatted by the database owner before live launch.
    /// This is display history only; it must not be collected or journaled again.
    pub restored_history: Vec<u8>,
    pub collect_history: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageFaultCode {
    QuotaExceeded,
    ReadOnly,
    WriteFailed,
}

#[derive(Debug)]
pub enum SessionEvent {
    Started {
        pid: u32,
        sequence: u64,
    },
    InputSubmitted,
    CwdCaptured {
        run_id: String,
        cwd: String,
    },
    Output {
        sequence: u64,
        data: Vec<u8>,
    },
    Resized {
        sequence: u64,
        cols: u16,
        rows: u16,
    },
    ScreenInvalidated,
    /// Emitted once for live output after it reaches the journal.
    SavedOutput {
        sequence: u64,
        data: String,
        completed_blocks: Vec<CollectedShellBlock>,
    },
    StorageFault {
        sequence: u64,
        retained_bytes: usize,
        code: StorageFaultCode,
        message: String,
    },
    StorageRecovered {
        sequence: u64,
    },
    /// Emitted after the final stored frame even if display delivery is paused.
    SavedExit {
        sequence: u64,
        status: ProcessExit,
        /// A disposal was requested before normal exit cleanup.
        disposed: bool,
    },
    Exited {
        sequence: u64,
        status: ProcessExit,
    },
    Diagnostic {
        message: String,
    },
}
