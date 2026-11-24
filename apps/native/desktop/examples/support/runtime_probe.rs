//! Command sink for hidden UI handler probes; it never claims a runtime owner.
#![allow(dead_code)]
include!("../../src/runtime_bridge.rs");

pub(super) struct ProbeReceiver {
    shared: Arc<Shared>,
    incoming: std::sync::mpsc::Receiver<Outgoing>,
}
impl ProbeReceiver {
    pub(super) fn try_recv(&self) -> std::result::Result<Command, std::sync::mpsc::TryRecvError> {
        loop {
            match self.incoming.try_recv()? {
                Outgoing::Business(command) => return Ok(command),
                Outgoing::Stop => {}
            }
        }
    }
    pub(super) fn generation(&self, generation: u64) {
        let mut state = self.shared.state.lock().unwrap();
        state.generation = generation;
        state.phase = ConnectionPhase::Ready;
    }
    #[allow(dead_code)]
    pub(super) fn starting(&self) {
        self.shared.state.lock().unwrap().phase = ConnectionPhase::Starting;
    }
    pub(super) fn read_only(&self) {
        self.shared.state.lock().unwrap().phase = ConnectionPhase::Blocked(StartupBlock {
            kind: StartupBlockKind::ReadOnly,
            message: "probe read-only recovery".into(),
        });
    }
    #[allow(dead_code)]
    pub(super) fn read_only_mode(&self, generation: u64) {
        let mut state = self.shared.state.lock().unwrap();
        state.generation = generation;
        state.phase = ConnectionPhase::ReadOnly { pending: None };
    }
    pub(super) fn database_recovery(&self, generation: u64, details: Arc<DatabaseRecoveryDetails>) {
        let mut state = self.shared.state.lock().unwrap();
        state.generation = generation;
        state.phase = ConnectionPhase::DatabaseRecovery {
            details,
            pending: None,
        };
    }
}
pub(super) fn probe_channel() -> (RuntimeCommands, ProbeReceiver) {
    let (send, incoming) = mpsc::channel();
    let shared = Arc::new(Shared {
        #[cfg(test)]
        hooks: TestHooks::default(),
        state: Mutex::new(State {
            generation: 1,
            phase: ConnectionPhase::Ready,
            sender: Some(send),
            stopping: false,
            retry: false,
            pid: 0,
        }),
        changed: Condvar::new(),
    });
    (
        RuntimeCommands {
            shared: shared.clone(),
        },
        ProbeReceiver { shared, incoming },
    )
}
