use std::{
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender, TryRecvError},
    thread::{self, JoinHandle},
};

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::{
    history::{
        HistoryCursor, HistoryLine, HistoryPage, HistoryPageInput, HistorySearchInput,
        HistorySearchOptions, HistorySearchResult, JournalHistoryReader, MAX_LINES, SearchPattern,
    },
    provider_catalog,
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

enum HistorySource {
    Journal,
    Claude(ClaudeHistoryBinding),
}

struct HistoryJob {
    request: HistoryRequest,
    source: HistorySource,
}

#[derive(PartialEq, Eq)]
struct ClaudeHistoryBinding {
    cwd: String,
    provider_session_id: String,
}

struct ClaudeSearchSnapshot {
    session_id: String,
    binding: ClaudeHistoryBinding,
    lines: Vec<HistoryLine>,
}

const CLAUDE_CONTEXT_LINES: usize = 12;
const CLAUDE_CONTEXT_EXPIRED: &str = "Claude conversation search expired; search again";
const HISTORY_PREVIEW_CHARS: usize = 400;
const HISTORY_PREVIEW_BEFORE_MATCH: usize = 16;

struct LiteralPreview {
    query: String,
    insensitive: Option<regex::Regex>,
}

impl LiteralPreview {
    fn new(query: &str, options: HistorySearchOptions) -> Option<Self> {
        if query.is_empty() || options.regex {
            return None;
        }
        let insensitive = if options.case_sensitive {
            None
        } else {
            regex::RegexBuilder::new(&regex::escape(query))
                .case_insensitive(true)
                .build()
                .ok()
        };
        Some(Self {
            query: query.to_owned(),
            insensitive,
        })
    }

    fn offset(&self, text: &str) -> Option<usize> {
        text.find(&self.query)
            .or_else(|| self.insensitive.as_ref()?.find(text).map(|hit| hit.start()))
    }
}

fn shorten_preview(line: &mut HistoryLine, literal: Option<&LiteralPreview>) {
    let text = &line.text;
    if text.chars().nth(HISTORY_PREVIEW_CHARS).is_none() {
        return;
    }
    let start = literal
        .and_then(|literal| literal.offset(text))
        .and_then(|offset| {
            text[..offset]
                .char_indices()
                .rev()
                .nth(HISTORY_PREVIEW_BEFORE_MATCH - 1)
                .map(|(start, _)| start)
        })
        .unwrap_or(0);
    let before = if start == 0 { "" } else { "…" };
    let budget = HISTORY_PREVIEW_CHARS - usize::from(start > 0);
    let (preview, after) = match text[start..].char_indices().nth(budget) {
        Some(_) => {
            let end = text[start..].char_indices().nth(budget - 1).unwrap().0;
            (&text[start..start + end], "…")
        }
        None => (&text[start..], ""),
    };
    let preview: String = preview
        .chars()
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
    line.text = format!("{before}{preview}{after}");
}

impl ClaudeSearchSnapshot {
    fn search(
        session_id: &str,
        binding: ClaudeHistoryBinding,
        projects_root: Option<&Path>,
        query: &str,
        options: HistorySearchOptions,
    ) -> Result<(Self, HistorySearchResult), String> {
        let pattern =
            SearchPattern::compile(query, options).map_err(|error| format!("{error:#}"))?;
        let lines = if query.is_empty() {
            Vec::new()
        } else {
            provider_catalog::search_lines(
                projects_root,
                &binding.cwd,
                &binding.provider_session_id,
            )
            .map_err(|error| format!("Could not search Claude conversation: {error}"))?
        };
        let mut hits = lines
            .iter()
            .rev()
            .filter(|line| pattern.matches(&line.text, options.case_sensitive));
        let matches = hits.by_ref().take(MAX_LINES).cloned().collect();
        let result = HistorySearchResult {
            matches,
            has_more: hits.next().is_some(),
            ..HistorySearchResult::default()
        };
        Ok((
            Self {
                session_id: session_id.to_owned(),
                binding,
                lines,
            },
            result,
        ))
    }

    fn context(&self, cursor: HistoryCursor) -> Result<HistoryPage, String> {
        let anchor = self
            .lines
            .binary_search_by_key(&cursor, |line| line.cursor)
            .map_err(|_| CLAUDE_CONTEXT_EXPIRED.to_owned())?;
        let start = anchor.saturating_sub(CLAUDE_CONTEXT_LINES);
        let end = (anchor + 1 + CLAUDE_CONTEXT_LINES).min(self.lines.len());
        Ok(HistoryPage {
            lines: self.lines[start..end].to_vec(),
            anchor_index: Some(anchor - start),
            has_more_before: Some(start > 0),
            has_more_after: Some(end < self.lines.len()),
            has_more: start > 0 || end < self.lines.len(),
            ..HistoryPage::default()
        })
    }
}

/// Reads terminal journal and bound Claude conversation history away from the owner and UI.
pub struct HistoryWorker {
    requests: Option<Sender<HistoryJob>>,
    replies: Option<Receiver<HistoryReply>>,
    thread: Option<JoinHandle<()>>,
}

impl HistoryWorker {
    pub fn new(data_root: PathBuf) -> Result<Self> {
        Self::with_projects_root(data_root, provider_catalog::projects_root())
    }

    fn with_projects_root(data_root: PathBuf, projects_root: Option<PathBuf>) -> Result<Self> {
        let (requests, incoming) = mpsc::channel::<HistoryJob>();
        let (outgoing, replies) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("matou-journal-history".into())
            .spawn(move || {
                let reader = JournalHistoryReader::new(data_root);
                let mut claude_snapshot: Option<ClaudeSearchSnapshot> = None;
                let mut last_literal: Option<(String, LiteralPreview)> = None;
                for HistoryJob { request, source } in incoming {
                    let reply = match request {
                        HistoryRequest::Search {
                            session_id,
                            sequence,
                            query,
                            options,
                        } => {
                            claude_snapshot = None;
                            last_literal = LiteralPreview::new(&query, options)
                                .map(|literal| (session_id.clone(), literal));
                            let mut result = match source {
                                HistorySource::Claude(binding) => ClaudeSearchSnapshot::search(
                                    &session_id,
                                    binding,
                                    projects_root.as_deref(),
                                    &query,
                                    options,
                                )
                                .map(|(snapshot, result)| {
                                    claude_snapshot = Some(snapshot);
                                    result
                                }),
                                HistorySource::Journal => reader
                                    .search(
                                        &session_id,
                                        HistorySearchInput {
                                            query,
                                            options: Some(options),
                                            ..HistorySearchInput::default()
                                        },
                                    )
                                    .map_err(|error| format!("{error:#}")),
                            };
                            if let Ok(result) = &mut result {
                                let literal = last_literal.as_ref().map(|(_, literal)| literal);
                                for line in &mut result.matches {
                                    shorten_preview(line, literal);
                                }
                            }
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
                            let mut result = match source {
                                HistorySource::Claude(binding) => claude_snapshot
                                    .as_ref()
                                    .filter(|snapshot| {
                                        snapshot.session_id == session_id
                                            && snapshot.binding == binding
                                    })
                                    .ok_or_else(|| CLAUDE_CONTEXT_EXPIRED.to_owned())
                                    .and_then(|snapshot| snapshot.context(cursor)),
                                HistorySource::Journal
                                    if claude_snapshot.as_ref().is_some_and(|snapshot| {
                                        snapshot.session_id == session_id
                                    }) =>
                                {
                                    Err(CLAUDE_CONTEXT_EXPIRED.to_owned())
                                }
                                HistorySource::Journal => reader
                                    .page(
                                        &session_id,
                                        HistoryPageInput {
                                            around: Some(cursor),
                                            ..HistoryPageInput::default()
                                        },
                                    )
                                    .map_err(|error| format!("{error:#}")),
                            };
                            if let Ok(page) = &mut result {
                                let literal = last_literal
                                    .as_ref()
                                    .filter(|(id, _)| id == &session_id)
                                    .map(|(_, literal)| literal);
                                for (index, line) in page.lines.iter_mut().enumerate() {
                                    let focus = if page.anchor_index == Some(index) {
                                        literal
                                    } else {
                                        None
                                    };
                                    shorten_preview(line, focus);
                                }
                            }
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
        self.send_job(HistoryJob {
            request,
            source: HistorySource::Journal,
        })
    }

    pub(crate) fn send_claude(
        &self,
        request: HistoryRequest,
        cwd: String,
        provider_session_id: String,
    ) -> Result<()> {
        self.send_job(HistoryJob {
            request,
            source: HistorySource::Claude(ClaudeHistoryBinding {
                cwd,
                provider_session_id,
            }),
        })
    }

    fn send_job(&self, job: HistoryJob) -> Result<()> {
        self.requests
            .as_ref()
            .context("journal history worker stopped")?
            .send(job)
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn claude_search_reuses_conversation_after_transcript_is_removed() -> Result<()> {
        let fixture = tempfile::tempdir()?;
        let workspace = fixture.path().join("work");
        std::fs::create_dir(&workspace)?;
        let workspace = workspace.canonicalize()?;
        let cwd = workspace.to_string_lossy().into_owned();
        let directory = fixture.path().join("projects");
        let project = directory.join(cwd.replace(|ch: char| !ch.is_ascii_alphanumeric(), "-"));
        std::fs::create_dir_all(&project)?;
        let transcript = project.join("test-dialogue.jsonl");

        let mut rows = vec![
            json!({"type":"system","cwd":cwd,"subtype":"compact_boundary",
                "message":{"role":"assistant","content":"交代书 compaction boundary"}}),
            json!({"type":"user","isCompactSummary":true,
                "message":{"role":"user","content":"交代书 summary"}}),
            json!({"type":"user","isMeta":true,
                "message":{"role":"user","content":"交代书 metadata"}}),
            json!({"type":"assistant","isSidechain":true,
                "message":{"role":"assistant","content":"交代书 side chain"}}),
            json!({"type":"user","message":{"role":"user","content":
                "<local-command-stdout>交代书 output</local-command-stdout>"}}),
            json!({"type":"assistant","message":{"role":"assistant","content":[
                {"type":"thinking","thinking":"交代书 thinking"},
                {"type":"tool_use","name":"Read","input":{"file_path":"交代书"}},
                {"type":"text","text":"First visible reply."}]}}),
            json!({"type":"user","message":{"role":"user","content":[
                {"type":"tool_result","content":"交代书 result"},
                {"type":"text","text":"交代书：开头\n<system-reminder>提示</system-reminder>"}]}}),
        ];
        let long_match = format!(
            "{}需要更新交代书。{}",
            "中\r".repeat(2_000),
            "尾".repeat(2_000)
        );
        for index in 0..30 {
            let text = if index == 14 {
                long_match.clone()
            } else {
                format!("可见上下文 {index}")
            };
            rows.push(json!({"type":"assistant","message":{"role":"assistant","content":text}}));
        }
        let body = rows
            .iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>();
        std::fs::write(&transcript, body)?;

        let mut worker = HistoryWorker::with_projects_root(fixture.path().into(), Some(directory))?;
        let receive = |worker: &HistoryWorker| -> Result<HistoryReply> {
            worker
                .replies
                .as_ref()
                .unwrap()
                .recv_timeout(std::time::Duration::from_secs(5))
                .context("history worker reply")
        };
        let search = |session_id: &str, sequence| HistoryRequest::Search {
            session_id: session_id.into(),
            sequence,
            query: "交代书".into(),
            options: HistorySearchOptions::default(),
        };
        worker.send_claude(search("card", 1), cwd.clone(), "test-dialogue".into())?;
        let HistoryReply::Search { result, .. } = receive(&worker)? else {
            panic!("expected conversation search reply");
        };
        let hits = result.map_err(|error| anyhow!(error))?.matches;
        assert_eq!(hits.len(), 2);
        assert!(hits[0].text.contains("需要更新交代书。"));
        assert_eq!(hits[0].text.chars().count(), HISTORY_PREVIEW_CHARS);
        assert!(!hits[0].text.chars().any(char::is_control));
        assert_eq!(hits[1].text, "交代书：开头");

        std::fs::remove_file(transcript)?;
        let context = |sequence, cursor| HistoryRequest::Context {
            session_id: "card".into(),
            sequence,
            cursor,
        };
        for (offset, hit) in hits.iter().enumerate() {
            let sequence = 3 + offset as u64;
            worker.send_claude(
                context(sequence, hit.cursor),
                cwd.clone(),
                "test-dialogue".into(),
            )?;
            let HistoryReply::Context {
                sequence: echoed,
                result,
                ..
            } = receive(&worker)?
            else {
                panic!("expected conversation context reply");
            };
            assert_eq!(echoed, sequence);
            let page = result.map_err(|error| anyhow!(error))?;
            let index = page.anchor_index.context("context anchor")?;
            assert_eq!(page.lines[index], *hit);
            if offset == 0 {
                assert_eq!(index, 12);
                assert_eq!(page.lines.len(), 25);
                assert!(page.has_more);
            } else {
                assert_eq!(page.lines[0].text, "First visible reply.");
            }
            assert!(page.gaps.is_empty());
        }

        worker.send_claude(
            context(5, hits[0].cursor),
            cwd.clone(),
            "different-binding".into(),
        )?;
        assert!(
            matches!(receive(&worker)?, HistoryReply::Context { result: Err(error), .. }
            if error == CLAUDE_CONTEXT_EXPIRED)
        );

        worker.send(search("shell", 6))?;
        assert!(
            matches!(receive(&worker)?, HistoryReply::Search { result: Ok(result), .. }
            if result.matches.is_empty())
        );
        worker.send_claude(
            context(7, hits[0].cursor),
            cwd.clone(),
            "test-dialogue".into(),
        )?;
        assert!(
            matches!(receive(&worker)?, HistoryReply::Context { result: Err(error), .. }
            if error == CLAUDE_CONTEXT_EXPIRED)
        );

        worker.send_claude(search("card", 8), cwd, "test-dialogue".into())?;
        assert!(
            matches!(receive(&worker)?, HistoryReply::Search { result: Err(error), .. }
            if error.starts_with("Could not search Claude conversation:"))
        );
        worker.shutdown()
    }
}
