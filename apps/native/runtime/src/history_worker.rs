use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender, TryRecvError},
    thread::{self, JoinHandle},
};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::history::{
    HistoryCursor, HistoryPage, HistoryPageInput, HistorySearchInput, HistorySearchOptions,
    HistorySearchResult, JournalHistoryReader,
};

#[derive(Serialize, Deserialize)]
pub enum HistoryRequest {
    Search {
        session_id: String,
        sequence: u64,
        query: String,
        options: HistorySearchOptions,
    },
    Context {
        session_id: String,
        sequence: u64,
        cursor: HistoryCursor,
    },
    Page {
        session_id: String,
        sequence: u64,
        before: Option<HistoryCursor>,
    },
}

impl HistoryRequest {
    pub(crate) fn session_id(&self) -> &str {
        match self {
            Self::Search { session_id, .. }
            | Self::Context { session_id, .. }
            | Self::Page { session_id, .. } => session_id,
        }
    }

    pub(crate) fn failed(self, message: String) -> HistoryReply {
        match self {
            Self::Search {
                session_id,
                sequence,
                ..
            } => HistoryReply::Search {
                session_id,
                sequence,
                result: Err(message),
            },
            Self::Context {
                session_id,
                sequence,
                cursor,
            } => HistoryReply::Context {
                session_id,
                sequence,
                cursor,
                result: Err(message),
            },
            Self::Page {
                session_id,
                sequence,
                before,
            } => HistoryReply::Page {
                session_id,
                sequence,
                before,
                result: Err(message),
            },
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub enum HistoryReply {
    Search {
        session_id: String,
        sequence: u64,
        result: Result<HistorySearchResult, String>,
    },
    Context {
        session_id: String,
        sequence: u64,
        cursor: HistoryCursor,
        result: Result<HistoryPage, String>,
    },
    Page {
        session_id: String,
        sequence: u64,
        before: Option<HistoryCursor>,
        result: Result<HistoryPage, String>,
    },
}

/// Reads journal history away from the owner and UI event loops.
pub struct HistoryWorker {
    requests: Option<Sender<HistoryRequest>>,
    replies: Option<Receiver<HistoryReply>>,
    thread: Option<JoinHandle<()>>,
}

impl HistoryWorker {
    pub fn new(data_root: PathBuf) -> Result<Self> {
        let (requests, incoming) = mpsc::channel();
        let (outgoing, replies) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("matou-journal-history".into())
            .spawn(move || {
                let reader = JournalHistoryReader::new(data_root);
                for request in incoming {
                    let reply = match request {
                        HistoryRequest::Search {
                            session_id,
                            sequence,
                            query,
                            options,
                        } => {
                            let result = reader
                                .search(
                                    &session_id,
                                    HistorySearchInput {
                                        query,
                                        options: Some(options),
                                        ..HistorySearchInput::default()
                                    },
                                )
                                .map_err(|error| format!("{error:#}"));
                            HistoryReply::Search {
                                session_id,
                                sequence,
                                result,
                            }
                        }
                        HistoryRequest::Context {
                            session_id,
                            sequence,
                            cursor,
                        } => {
                            let result = reader
                                .page(
                                    &session_id,
                                    HistoryPageInput {
                                        around: Some(cursor),
                                        ..HistoryPageInput::default()
                                    },
                                )
                                .map_err(|error| format!("{error:#}"));
                            HistoryReply::Context {
                                session_id,
                                sequence,
                                cursor,
                                result,
                            }
                        }
                        HistoryRequest::Page {
                            session_id,
                            sequence,
                            before,
                        } => {
                            let result = reader
                                .page(
                                    &session_id,
                                    HistoryPageInput {
                                        before,
                                        ..HistoryPageInput::default()
                                    },
                                )
                                .map_err(|error| format!("{error:#}"));
                            HistoryReply::Page {
                                session_id,
                                sequence,
                                before,
                                result,
                            }
                        }
                    };
                    if outgoing.send(reply).is_err() {
                        break;
                    }
                }
            })
            .context("start journal history worker")?;
        Ok(Self {
            requests: Some(requests),
            replies: Some(replies),
            thread: Some(thread),
        })
    }

    pub fn send(&self, request: HistoryRequest) -> Result<()> {
        self.requests
            .as_ref()
            .context("journal history worker stopped")?
            .send(request)
            .map_err(|_| anyhow!("journal history worker stopped"))
    }

    pub fn try_recv(&self) -> Result<Option<HistoryReply>> {
        let replies = self
            .replies
            .as_ref()
            .context("journal history worker stopped")?;
        match replies.try_recv() {
            Ok(reply) => Ok(Some(reply)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(error) => Err(error).context("journal history worker stopped"),
        }
    }

    pub fn shutdown(&mut self) -> Result<()> {
        // The worker stops at the next failed reply or a closed request queue.
        self.replies.take();
        self.requests.take();
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| anyhow!("journal history worker panicked"))?;
        }
        Ok(())
    }
}

impl Drop for HistoryWorker {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}
