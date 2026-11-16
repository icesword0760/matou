use std::io::{self, Read, Write};

use anyhow::{Context, Result, bail};
use matou_native_core::{
    geometry::GeometryUpdate,
    hierarchy::{BootstrapWindowInput, HierarchyIds, bootstrap_window},
    storage::{CommandMetadata, configure_connection, migrate},
};
use matou_native_runtime::{
    history::{HistoryCursor, HistoryLine, HistoryPage},
    history_worker::{HistoryReply, HistoryRequest},
    host::{Command, Event},
    protocol::{
        ClientMessage, ServerMessage, VERSION, check_version, read_messages, write_message,
    },
    search::{SearchRequest, SearchResult},
};
use matou_native_terminal::{
    Color, MouseEncoding, MouseProtocol, SearchMatch, SearchOptions, SelectionAction, Terminal,
    TerminalOptions,
};
use rusqlite::Connection;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;

#[test]
fn actual_host_messages_support_serde() {
    fn require_wire_data<T: Serialize + DeserializeOwned>() {}
    require_wire_data::<Command>();
    require_wire_data::<Event>();
}

/// Every byte, including UTF-8 continuation bytes, crosses a Read boundary.
struct Fragmented<'a>(&'a [u8]);

impl Read for Fragmented<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        let count = out.len().min(self.0.len()).min(1);
        out[..count].copy_from_slice(&self.0[..count]);
        self.0 = &self.0[count..];
        Ok(count)
    }
}

#[derive(Default)]
struct Writes {
    bytes: Vec<u8>,
    flushes: usize,
}

impl Write for Writes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flushes += 1;
        Ok(())
    }
}

fn event_roundtrip(event: Event) -> Result<Event> {
    let mut stream = Writes::default();
    write_message(&mut stream, &ServerMessage::Event(event))?;
    assert_eq!(stream.flushes, 1);
    assert_eq!(stream.bytes.last(), Some(&b'\n'));
    let mut messages = read_messages::<ServerMessage>(Fragmented(&stream.bytes));
    let message = messages.next().context("event message")??;
    assert!(messages.next().is_none(), "complete EOF ends iteration");
    match message {
        ServerMessage::Event(event) => Ok(event),
        _ => bail!("expected event"),
    }
}

fn styled_terminal() -> Result<Terminal> {
    let mut terminal = Terminal::new(TerminalOptions::new(20, 4, 8, 16))?;
    terminal.feed(
        "\x1b[?1h\x1b[?2004h\x1b[?1003h\x1b[?1006h\
         \x1b]4;123;#123456\x1b\\\x1b[1;3;4;38;2;7;8;9m中🙂A\x1b[0m"
            .as_bytes(),
    );
    terminal.selection_action(SelectionAction::Select {
        row: 0,
        col: 0,
        length: 2,
    })?;
    Ok(terminal)
}

#[test]
fn real_terminal_frame_preserves_cells_palette_modes_selection_and_ack_tags() -> Result<()> {
    let mut terminal = styled_terminal()?;
    let screen = terminal.screen()?;
    let modes = terminal.input_modes()?;
    let selection = terminal.selection_range()?;
    assert_eq!((&*screen.cells[0].text, screen.cells[0].width), ("中", 2));
    assert!(screen.cells[0].bold && screen.cells[0].italic && screen.cells[0].underline);
    assert_eq!(screen.cells[0].fg, Color::Rgb(0x070809));
    assert_eq!(screen.palette[123], 0x123456);
    assert!(modes.application_cursor_keys && modes.bracketed_paste);
    assert_eq!(modes.mouse_protocol, MouseProtocol::Any);
    assert_eq!(modes.mouse_encoding, MouseEncoding::Sgr);
    assert!(selection.is_some());

    let event = event_roundtrip(Event::Frame {
        session_id: "会话🙂".into(),
        screen: screen.clone(),
        selection,
        modes,
        sequence: u64::MAX,
        scroll_request_id: u64::MAX - 1,
    })?;
    let Event::Frame {
        session_id,
        screen: decoded,
        selection: decoded_selection,
        modes: decoded_modes,
        sequence,
        scroll_request_id,
    } = event
    else {
        bail!("expected Frame");
    };
    assert_eq!(session_id, "会话🙂");
    assert_eq!(decoded, screen);
    assert_eq!(decoded_selection, selection);
    assert_eq!(decoded_modes, modes);
    assert_eq!((sequence, scroll_request_id), (u64::MAX, u64::MAX - 1));
    Ok(())
}

#[test]
fn continuous_client_values_preserve_scroll_input_ack_and_unicode_order() -> Result<()> {
    let session = "会话\n🙂";
    let bytes = vec![0, 255, 128, 27, 13, 10];
    let messages = [
        ClientMessage::Hello { version: VERSION },
        ClientMessage::Command(Command::ScrollTo {
            session_id: session.into(),
            row: u64::MAX - 2,
            request_id: u64::MAX,
        }),
        ClientMessage::Command(Command::Scroll {
            session_id: session.into(),
            lines: i32::MIN,
        }),
        ClientMessage::Command(Command::Input {
            session_id: session.into(),
            bytes: bytes.clone(),
            defer_ordering: true,
        }),
        ClientMessage::Command(Command::Applied {
            session_id: session.into(),
            sequence: u64::MAX - 3,
        }),
        ClientMessage::Command(Command::Search {
            session_id: session.into(),
            request: SearchRequest {
                sequence: u64::MAX - 4,
                query: "中文🙂\n\"".into(),
                options: SearchOptions {
                    regex: true,
                    ..Default::default()
                },
                previous: true,
            },
        }),
        ClientMessage::Command(Command::History(HistoryRequest::Context {
            session_id: session.into(),
            sequence: u64::MAX - 5,
            cursor: HistoryCursor {
                sequence: u64::MAX - 6,
                line_index: 9,
            },
        })),
    ];
    let mut stream = Writes::default();
    for message in &messages {
        write_message(&mut stream, message)?;
    }
    assert_eq!(stream.flushes, messages.len());
    // Adjacent JSON values prove this is not a fresh line parser per message.
    stream.bytes.retain(|byte| *byte != b'\n');
    let mut decoded = read_messages::<ClientMessage>(Fragmented(&stream.bytes));
    let ClientMessage::Hello { version } = decoded.next().context("Hello")?? else {
        bail!("Hello")
    };
    check_version(version)?;
    let ClientMessage::Command(Command::ScrollTo {
        session_id,
        row,
        request_id,
    }) = decoded.next().context("ScrollTo")??
    else {
        bail!("ScrollTo")
    };
    assert_eq!(session_id, session);
    assert_eq!((row, request_id), (u64::MAX - 2, u64::MAX));
    let ClientMessage::Command(Command::Scroll { lines, .. }) =
        decoded.next().context("Scroll")??
    else {
        bail!("Scroll")
    };
    assert_eq!(lines, i32::MIN);
    let ClientMessage::Command(Command::Input {
        bytes: received,
        defer_ordering,
        ..
    }) = decoded.next().context("Input")??
    else {
        bail!("Input")
    };
    assert_eq!(received, bytes);
    assert!(defer_ordering);
    let ClientMessage::Command(Command::Applied { sequence, .. }) =
        decoded.next().context("Applied")??
    else {
        bail!("Applied")
    };
    assert_eq!(sequence, u64::MAX - 3);
    let ClientMessage::Command(Command::Search { request, .. }) =
        decoded.next().context("Search")??
    else {
        bail!("Search")
    };
    assert_eq!(request.sequence, u64::MAX - 4);
    assert_eq!(request.query, "中文🙂\n\"");
    assert!(request.previous && request.options.regex);
    let ClientMessage::Command(Command::History(HistoryRequest::Context {
        sequence, cursor, ..
    })) = decoded.next().context("History")??
    else {
        bail!("History")
    };
    assert_eq!(
        (sequence, cursor.sequence, cursor.line_index),
        (u64::MAX - 5, u64::MAX - 6, 9)
    );
    assert!(decoded.next().is_none());
    Ok(())
}

#[test]
fn raw_utf16_signed_search_and_history_success_or_error_survive() -> Result<()> {
    let found = SearchMatch {
        term: vec![0xd800, 0x4e2d, 0xdc00],
        row: 3,
        col: -2,
        size: -1,
    };
    let expected = SearchResult {
        sequence: u64::MAX,
        matches: vec![found.clone()],
        active: Some(found),
        result_index: -1,
    };
    let Event::SearchResults {
        result: Ok(result),
        sequence,
        ..
    } = event_roundtrip(Event::SearchResults {
        session_id: "查找".into(),
        sequence: u64::MAX,
        result: Ok(expected.clone()),
    })?
    else {
        bail!("SearchResults")
    };
    assert_eq!(sequence, u64::MAX);
    assert_eq!(result, expected);
    let cursor = HistoryCursor {
        sequence: u64::MAX - 1,
        line_index: 7,
    };
    let page = HistoryPage {
        lines: vec![HistoryLine {
            sequence: cursor.sequence,
            cursor,
            text: "中文\n🙂".into(),
        }],
        has_more: true,
        anchor_index: Some(0),
        has_more_before: Some(true),
        has_more_after: Some(false),
        ..Default::default()
    };
    let Event::History(HistoryReply::Context {
        result: Ok(decoded),
        cursor: decoded_cursor,
        ..
    }) = event_roundtrip(Event::History(HistoryReply::Context {
        session_id: "历史".into(),
        sequence: u64::MAX,
        cursor,
        result: Ok(page.clone()),
    }))?
    else {
        bail!("history context")
    };
    assert_eq!(decoded, page);
    assert_eq!(decoded_cursor, cursor);
    let Event::History(HistoryReply::Search {
        result: Err(error),
        sequence,
        ..
    }) = event_roundtrip(Event::History(HistoryReply::Search {
        session_id: "历史".into(),
        sequence: u64::MAX,
        result: Err("损坏段：\"未完成\"\n".into()),
    }))?
    else {
        bail!("history error")
    };
    assert_eq!(error, "损坏段：\"未完成\"\n");
    assert_eq!(sequence, u64::MAX);
    Ok(())
}

#[test]
fn ready_and_navigation_use_real_bootstrap_owned_types_and_paths() -> Result<()> {
    let mut db = Connection::open_in_memory()?;
    configure_connection(&db)?;
    migrate(&mut db, 1)?;
    let hierarchy = bootstrap_window(
        &mut db,
        &CommandMetadata {
            command_id: "wire-bootstrap".into(),
            command_type: "window.bootstrap".into(),
            request_hash: "wire".into(),
            causation_id: None,
            correlation_id: None,
        },
        &BootstrapWindowInput {
            window_id: "main".into(),
            default_root_directory: "/协议/中文 工作区".into(),
            default_name: "中文工作区🙂".into(),
            now: 1,
        },
        &HierarchyIds {
            workspace_id: "w".into(),
            execution_context_id: "e".into(),
            task_id: "t".into(),
            scene_id: "s".into(),
            root_node_id: "n".into(),
            session_id: "p".into(),
            mount_id: "m".into(),
        },
        "事项🙂",
        1,
    )?;
    let scenes = vec![hierarchy.scene.clone().context("scene")?];
    let geometry = vec![GeometryUpdate {
        scene_id: scenes[0].id.clone(),
        owner_key: "canvas".into(),
        layout_revision: 7.0,
        geometry: json!({"offset": 12.5, "title": "终端🙂"}),
        now: 2,
    }];
    let graph = json!({"nodes": [{"sessionId": "p"}], "sequence": u64::MAX});
    let events = [
        Event::Ready {
            hierarchy: hierarchy.clone(),
            scenes: scenes.clone(),
            graph: graph.clone(),
            geometry: geometry.clone(),
        },
        Event::Navigation {
            hierarchy: hierarchy.clone(),
            scenes: scenes.clone(),
            graph: graph.clone(),
            geometry: geometry.clone(),
        },
    ];
    for (index, event) in events.into_iter().enumerate() {
        let event = event_roundtrip(event)?;
        assert_eq!(matches!(&event, Event::Ready { .. }), index == 0);
        let (received, received_scenes, received_graph, received_geometry) = match event {
            Event::Ready {
                hierarchy,
                scenes,
                graph,
                geometry,
            }
            | Event::Navigation {
                hierarchy,
                scenes,
                graph,
                geometry,
            } => (hierarchy, scenes, graph, geometry),
            _ => bail!("Ready/Navigation"),
        };
        assert_eq!(received, hierarchy);
        assert_eq!(received_scenes, scenes);
        assert_eq!(received_graph, graph);
        assert_eq!(received_geometry, geometry);
    }
    Ok(())
}

#[test]
fn eof_truncation_invalid_json_palette_version_and_io_errors_remain_distinct() -> Result<()> {
    assert!(
        read_messages::<ClientMessage>(&b" \n\t"[..])
            .next()
            .is_none()
    );
    let mut stream = Vec::new();
    write_message(&mut stream, &ServerMessage::Hello { version: VERSION })?;
    let mut messages = read_messages::<ServerMessage>(&stream[..]);
    let ServerMessage::Hello { version } = messages.next().context("server Hello")?? else {
        bail!("Hello")
    };
    check_version(version)?;
    assert!(messages.next().is_none());
    assert!(
        check_version(VERSION + 1)
            .unwrap_err()
            .to_string()
            .contains("version 2")
    );

    let mut truncated = stream.clone();
    truncated.extend_from_slice(b"{\"Event\":");
    let mut values = read_messages::<ServerMessage>(Fragmented(&truncated));
    assert!(values.next().context("first complete value")?.is_ok());
    assert!(
        values
            .next()
            .context("truncated value")?
            .err()
            .context("truncation must be an error")?
            .is_eof()
    );
    assert!(
        read_messages::<ClientMessage>(&b"{!}"[..])
            .next()
            .context("invalid JSON")?
            .is_err()
    );
    assert!(
        read_messages::<ClientMessage>(&b"{\"Command\":{\"TestPause\":{}}}"[..])
            .next()
            .context("test-only variant")?
            .is_err()
    );

    let mut terminal = styled_terminal()?;
    let frame = ServerMessage::Event(Event::Frame {
        session_id: "坏palette".into(),
        screen: terminal.screen()?,
        selection: terminal.selection_range()?,
        modes: terminal.input_modes()?,
        sequence: 1,
        scroll_request_id: 0,
    });
    for length in [255, 257] {
        let mut malformed = serde_json::to_value(&frame)?;
        malformed["Event"]["Frame"]["screen"]["palette"] = json!(vec![0_u32; length]);
        let mut stream = Vec::new();
        write_message(&mut stream, &malformed)?;
        let error = read_messages::<ServerMessage>(Fragmented(&stream))
            .next()
            .context("bad palette")?
            .err()
            .context("palette must fail")?;
        assert!(error.to_string().contains("exactly 256 palette entries"));
    }

    struct BrokenRead;
    impl Read for BrokenRead {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("read failure"))
        }
    }
    assert!(
        read_messages::<ClientMessage>(BrokenRead)
            .next()
            .context("read error")?
            .err()
            .context("read must fail")?
            .is_io()
    );
    struct BrokenFlush;
    impl Write for BrokenFlush {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::other("flush failure"))
        }
    }
    assert!(
        write_message(&mut BrokenFlush, &ClientMessage::Hello { version: VERSION })
            .unwrap_err()
            .to_string()
            .contains("flush failure")
    );
    Ok(())
}
