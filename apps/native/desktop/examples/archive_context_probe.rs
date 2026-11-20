//! Exercise the production archive handlers with a deterministic reply order.

#[cfg(target_os = "macos")]
pub use matou_native_desktop::{
    assets, macos, recovery_water_view, rename_input, tab_dialog_style, workspace_drag,
};

#[cfg(target_os = "macos")]
#[path = "../src/terminal_input.rs"]
mod terminal_input;
#[cfg(target_os = "macos")]
#[path = "../src/terminal_scrollbar.rs"]
mod terminal_scrollbar;
#[cfg(target_os = "macos")]
#[path = "../src/terminal_surface.rs"]
mod terminal_surface;

#[cfg(target_os = "macos")]
#[path = "support/runtime_probe.rs"]
mod runtime_bridge;

#[cfg(target_os = "macos")]
#[path = "../src/carousel_motion.rs"]
mod carousel_motion;

#[cfg(target_os = "macos")]
#[allow(dead_code)]
mod ui_text {
    include!("../src/ui_text.rs");
}

#[cfg(target_os = "macos")]
mod sidebar_scrollbar {
    include!("../src/sidebar_scrollbar.rs");
}

#[cfg(target_os = "macos")]
mod history_view {
    include!("../src/history_view.rs");

    impl HistoryView {
        pub(super) fn probe_state(&self) -> Option<&HistoryViewState> {
            self.state.as_ref()
        }
    }
}

#[cfg(target_os = "macos")]
#[path = "../src/provider_settings.rs"]
mod provider_settings;
#[cfg(target_os = "macos")]
#[path = "../src/settings_shell.rs"]
mod settings_shell;

#[cfg(target_os = "macos")]
#[allow(dead_code)]
mod workspace {
    include!("../src/workspace.rs");

    use matou_native_runtime::{
        history::{HistoryPage, HistorySearchResult},
        search::SearchResult,
    };
    use std::sync::mpsc::{Receiver, TryRecvError};

    const SESSION: &str = "archive-probe-shell";

    impl Workspace {
        pub(super) fn verify_archive_replies(
            &mut self,
            commands: &crate::runtime_bridge::ProbeReceiver,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            self.runtime_generation = 1;
            self.domain_ready = true;
            self.transport_connected = true;
            self.session_frames.insert(SESSION.into());
            self.recovery.insert(SESSION.into(), RecoveryState::Ready);
            self.focused = Some(SESSION.into());
            self.open_search(window, cx);
            assert!(matches!(
                commands.try_recv().expect("initial search command"),
                Command::Search { session_id, request }
                    if session_id == SESSION && request.query.is_empty()
            ));

            let older = line(21, "older archive line");
            let matched = line(22, "old context arrives");
            let first = self.begin_archive_search("first query", true, commands, window, cx);
            let first_cursor = self
                .complete_archive_search(
                    first,
                    vec![matched.clone(), older.clone()],
                    commands,
                    window,
                    cx,
                )
                .expect("first context request");
            assert_eq!(first_cursor, matched.cursor);
            self.assert_loading(1, 2, cx);

            let zero = self.begin_archive_search("no matches", false, commands, window, cx);
            self.assert_loading(1, 2, cx);
            assert_eq!(self.search.as_ref().unwrap().sequence, zero);
            assert!(
                self.search
                    .as_ref()
                    .unwrap()
                    .result
                    .as_ref()
                    .unwrap()
                    .matches
                    .is_empty()
            );

            let original = page(vec![older, matched.clone()], 1);
            self.apply_history(
                context_reply(first, first_cursor, original.clone()),
                window,
                cx,
            );
            self.assert_ready(1, 2, &original, cx);
            // The history worker reads requests in order. The second search
            // completes only after the prior context reply, and keeps it visible.
            assert!(
                self.complete_archive_search(zero, Vec::new(), commands, window, cx)
                    .is_none()
            );
            self.assert_ready(1, 2, &original, cx);
            let search = self.search.as_ref().unwrap();
            assert_eq!(search.sequence, zero);
            assert_eq!(search.query, "no matches");
            assert_eq!(search.archive_count, Some((0, 0)));
            println!(
                "The in-flight page finishes after the next buffer miss and stays ready when the archive count becomes 0/0."
            );

            let dismiss = self.begin_archive_search("dismiss query", false, commands, window, cx);
            let dismiss_cursor = self
                .complete_archive_search(dismiss, vec![matched.clone()], commands, window, cx)
                .expect("dismissible context request");
            self.assert_loading(0, 1, cx);
            assert!(self.search_key(&Keystroke::parse("escape").unwrap(), window, cx));
            assert!(self.history_request.is_none());
            self.apply_history(
                context_reply(dismiss, dismiss_cursor, original.clone()),
                window,
                cx,
            );
            assert!(self.history_request.is_none());
            self.assert_loading(0, 1, cx);
            println!("Escape dismisses the panel and its late page does not update it.");

            let outgoing = self.begin_archive_search("outgoing query", false, commands, window, cx);
            let outgoing_cursor = self
                .complete_archive_search(outgoing, vec![matched.clone()], commands, window, cx)
                .expect("outgoing context request");
            // Next reuses these cached matches, so both context requests can
            // be outstanding without another archive search returning first.
            let replacement = self.retained_miss("outgoing query", false, commands, window, cx);
            let replacement_cursor =
                requested_context(commands, replacement).expect("replacement context request");
            assert_eq!(outgoing_cursor, replacement_cursor);
            self.apply_history(
                context_reply(outgoing, outgoing_cursor, original),
                window,
                cx,
            );
            assert_eq!(self.history_request.as_ref().unwrap().sequence, replacement);
            self.assert_loading(0, 1, cx);
            let final_page = page(vec![matched], 0);
            self.apply_history(
                context_reply(replacement, replacement_cursor, final_page.clone()),
                window,
                cx,
            );
            self.assert_ready(0, 1, &final_page, cx);
            assert!(matches!(commands.try_recv(), Err(TryRecvError::Empty)));
            println!("The replacement panel ignores an older reply for the same cursor.");
        }

        fn begin_archive_search(
            &mut self,
            query: &str,
            previous: bool,
            commands: &crate::runtime_bridge::ProbeReceiver,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) -> u64 {
            let sequence = self.retained_miss(query, previous, commands, window, cx);
            assert!(matches!(
                commands.try_recv().expect("archive search command"),
                Command::History(HistoryRequest::Search { session_id, sequence: requested, query: archived, .. })
                    if session_id == SESSION && requested == sequence && archived == query
            ));
            sequence
        }

        fn retained_miss(
            &mut self,
            query: &str,
            previous: bool,
            commands: &crate::runtime_bridge::ProbeReceiver,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) -> u64 {
            self.search.as_mut().unwrap().query = query.into();
            self.run_search(previous, cx);
            let Command::Search {
                session_id,
                request,
            } = commands.try_recv().expect("retained-buffer search command")
            else {
                panic!("expected retained-buffer search");
            };
            assert_eq!(session_id, SESSION);
            assert_eq!(request.query, query);
            self.apply_search_results(
                session_id,
                request.sequence,
                Ok(SearchResult {
                    sequence: request.sequence,
                    matches: Vec::new(),
                    active: None,
                    result_index: -1,
                }),
                window,
                cx,
            );
            request.sequence
        }

        fn complete_archive_search(
            &mut self,
            sequence: u64,
            matches: Vec<HistoryLine>,
            commands: &crate::runtime_bridge::ProbeReceiver,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) -> Option<HistoryCursor> {
            self.apply_history(
                HistoryReply::Search {
                    session_id: SESSION.into(),
                    sequence,
                    result: Ok(HistorySearchResult {
                        matches,
                        ..HistorySearchResult::default()
                    }),
                },
                window,
                cx,
            );
            requested_context(commands, sequence)
        }

        fn assert_loading(&self, index: usize, count: usize, cx: &App) {
            assert!(
                matches!(
                    self.history_view.as_ref().unwrap().read(cx).probe_state(),
                    Some(HistoryViewState::Loading { result_index, result_count, .. })
                        if (*result_index, *result_count) == (index, count)
                ),
                "expected loading panel with its original match metadata"
            );
        }

        fn assert_ready(&self, index: usize, count: usize, expected: &HistoryPage, cx: &App) {
            assert!(
                matches!(
                    self.history_view.as_ref().unwrap().read(cx).probe_state(),
                    Some(HistoryViewState::Ready { result_index, result_count, page })
                        if (*result_index, *result_count, page) == (index, count, expected)
                ),
                "expected original archive context to be ready"
            );
        }
    }

    fn requested_context(
        commands: &crate::runtime_bridge::ProbeReceiver,
        sequence: u64,
    ) -> Option<HistoryCursor> {
        match commands.try_recv() {
            Ok(Command::History(HistoryRequest::Context {
                session_id,
                sequence: requested,
                cursor,
            })) => {
                assert_eq!(session_id, SESSION);
                assert_eq!(requested, sequence);
                Some(cursor)
            }
            Err(TryRecvError::Empty) => None,
            _ => panic!("expected a context request or no follow-up command"),
        }
    }

    fn line(sequence: u64, text: &str) -> HistoryLine {
        HistoryLine {
            sequence,
            cursor: HistoryCursor {
                sequence,
                line_index: 0,
            },
            text: text.into(),
        }
    }

    fn page(lines: Vec<HistoryLine>, anchor: usize) -> HistoryPage {
        HistoryPage {
            lines,
            anchor_index: Some(anchor),
            has_more_before: Some(false),
            has_more_after: Some(false),
            ..HistoryPage::default()
        }
    }

    fn context_reply(sequence: u64, cursor: HistoryCursor, page: HistoryPage) -> HistoryReply {
        HistoryReply::Context {
            session_id: SESSION.into(),
            sequence,
            cursor,
            result: Ok(page),
        }
    }
}

#[cfg(target_os = "macos")]
fn main() {
    use gpui::AppContext as _;

    gpui::Application::new()
        .with_assets(assets::NativeAssets)
        .run(|cx| {
            let (send, commands) = runtime_bridge::probe_channel();
            let (_events, incoming) = async_channel::bounded(1);
            let window = cx
                .open_window(
                    gpui::WindowOptions {
                        show: false,
                        focus: false,
                        ..Default::default()
                    },
                    |window, cx| cx.new(|cx| workspace::Workspace::new(send, incoming, window, cx)),
                )
                .expect("archive context probe window");
            window
                .update(cx, |workspace, window, cx| {
                    workspace.verify_archive_replies(&commands, window, cx);
                })
                .expect("verify archive context in the hidden window");
            cx.quit();
        });
}

#[cfg(not(target_os = "macos"))]
fn main() {}
