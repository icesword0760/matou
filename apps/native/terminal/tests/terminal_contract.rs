use std::{thread, time::Instant};

use anyhow::{Result, ensure};
use matou_native_terminal::{
    Cell, Color, CursorStyle, Screen, Scrollbar, Terminal, TerminalOptions, Theme,
};
use serde::Deserialize;
use serde_json::Value;

const REFERENCE: &str = include_str!("fixtures/reference.json");

#[derive(Deserialize)]
struct Reference {
    themes: Value,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    cols: u16,
    rows: u16,
    steps: Vec<Step>,
}

#[derive(Deserialize)]
struct Step {
    data: String,
    expected: ExpectedScreen,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExpectedScreen {
    cols: usize,
    rows: Vec<ExpectedRow>,
    cursor_col: usize,
    cursor_row: usize,
    cursor_style: CursorStyle,
    cursor_blink: bool,
    cursor_visible: bool,
    history_lines: usize,
    alternate: bool,
    application_cursor_keys: bool,
    bracketed_paste: bool,
    replies: Vec<String>,
}

#[derive(Deserialize)]
struct ExpectedRow {
    continuation: bool,
    cells: Vec<ExpectedCell>,
}

#[derive(Debug, PartialEq, Eq, Deserialize)]
struct ExpectedCell {
    text: String,
    width: u8,
    fg: Color,
    bg: Color,
    bold: bool,
    italic: bool,
    dim: bool,
    underline: bool,
    inverse: bool,
    invisible: bool,
    strikethrough: bool,
    overline: bool,
}

impl From<&Cell> for ExpectedCell {
    fn from(cell: &Cell) -> Self {
        Self {
            text: cell.text.clone(),
            width: cell.width,
            fg: cell.fg,
            bg: cell.bg,
            bold: cell.bold,
            italic: cell.italic,
            dim: cell.dim,
            underline: cell.underline,
            inverse: cell.inverse,
            invisible: cell.invisible,
            strikethrough: cell.strikethrough,
            overline: cell.overline,
        }
    }
}

fn compare_fixture(name: &str) -> Result<()> {
    let reference: Reference = serde_json::from_str(REFERENCE)?;
    let case = reference
        .cases
        .into_iter()
        .find(|case| case.name == name)
        .unwrap();
    let mut terminal = Terminal::new(TerminalOptions::new(case.cols, case.rows, 10, 19))?;
    let mut differences = Vec::new();
    for (step, Step { data, expected }) in case.steps.into_iter().enumerate() {
        terminal.feed(data.as_bytes());
        let screen = terminal.screen()?;
        let modes = terminal.input_modes()?;
        let replies = terminal.drain_responses();
        macro_rules! compare {
            ($field:literal, $actual:expr, $expected:expr) => {
                if $actual != $expected {
                    differences.push(format!(
                        "step {step} {}: native {:?}; reference {:?}",
                        $field, $actual, $expected,
                    ));
                }
            };
        }
        compare!("columns", screen.cols, expected.cols);
        compare!("rows", screen.rows, expected.rows.len());
        compare!(
            "cursor position available",
            screen.cursor_has_position,
            true
        );
        compare!("cursor column", screen.cursor_col, expected.cursor_col);
        compare!("cursor row", screen.cursor_row, expected.cursor_row);
        compare!("cursor style", screen.cursor_style, expected.cursor_style);
        compare!(
            "cursor blink",
            screen.cursor_blinking,
            expected.cursor_blink
        );
        compare!(
            "cursor visibility",
            screen.cursor_visible,
            expected.cursor_visible
        );
        compare!(
            "history lines",
            screen.history_lines,
            expected.history_lines
        );
        compare!("alternate screen", screen.alternate, expected.alternate);
        compare!(
            "application cursor keys",
            modes.application_cursor_keys,
            expected.application_cursor_keys
        );
        compare!(
            "bracketed paste",
            modes.bracketed_paste,
            expected.bracketed_paste
        );
        let expected_replies = expected.replies.concat();
        compare!(
            "PTY replies",
            replies.as_slice(),
            expected_replies.as_bytes()
        );

        for (row, line) in expected.rows.iter().enumerate().take(screen.rows) {
            let metadata = screen.row_metadata[row];
            if metadata.wrap_continuation != line.continuation {
                differences.push(format!(
                    "step {step} row {row}: wrap continuation is {}, expected {}",
                    metadata.wrap_continuation, line.continuation
                ));
            }
            let wrap = expected
                .rows
                .get(row + 1)
                .is_some_and(|next| next.continuation);
            if metadata.wrap != wrap {
                differences.push(format!(
                    "step {step} row {row}: wrap is {}, expected {wrap}",
                    metadata.wrap
                ));
            }
            for (col, expected) in line.cells.iter().enumerate().take(screen.cols) {
                let actual = ExpectedCell::from(&screen.cells[row * screen.cols + col]);
                if &actual != expected {
                    differences.push(format!(
                        "step {step} row {row} column {col}: native {actual:?}; reference {expected:?}"
                    ));
                }
            }
        }
    }
    ensure!(differences.is_empty(), "{name}\n{}", differences.join("\n"));
    Ok(())
}

#[test]
fn unicode_wrap() -> Result<()> {
    compare_fixture("unicode-wrap")
}
#[test]
fn style_colors() -> Result<()> {
    compare_fixture("style-colors")
}
#[test]
fn alternate() -> Result<()> {
    compare_fixture("alternate")
}
#[test]
fn input_modes_and_reply() -> Result<()> {
    compare_fixture("input-modes-and-reply")
}
#[test]
fn cursor_shape() -> Result<()> {
    compare_fixture("cursor-shape")
}
#[test]
fn history() -> Result<()> {
    compare_fixture("history")
}

fn theme_colors_match(screen: &Screen, theme: Theme, expected: &Value) -> Result<()> {
    let hex = |name: &str| -> Result<u32> {
        Ok(u32::from_str_radix(
            expected[name].as_str().unwrap().trim_start_matches('#'),
            16,
        )?)
    };
    assert_eq!(screen.foreground, hex("foreground")?);
    assert_eq!(screen.background, hex("background")?);
    assert_eq!(screen.cursor_color, Some(hex("cursor")?));
    assert_eq!(theme.cursor_accent, hex("cursorAccent")?);
    assert_eq!(theme.selection_background, hex("selectionBackground")?);
    let names = [
        "black",
        "red",
        "green",
        "yellow",
        "blue",
        "magenta",
        "cyan",
        "white",
        "brightBlack",
        "brightRed",
        "brightGreen",
        "brightYellow",
        "brightBlue",
        "brightMagenta",
        "brightCyan",
        "brightWhite",
    ];
    for (index, name) in names.into_iter().enumerate() {
        assert_eq!(screen.palette[index], hex(name)?, "{name}");
    }
    for (index, rgb) in [
        (16, 0x000000),
        (17, 0x00005f),
        (123, 0x87ffff),
        (231, 0xffffff),
        (232, 0x080808),
        (255, 0xeeeeee),
    ] {
        assert_eq!(screen.palette[index], rgb, "palette {index}");
    }
    Ok(())
}

#[test]
fn reference_themes_apply_to_existing_cells_and_keep_raw_color_tags() -> Result<()> {
    let reference: Reference = serde_json::from_str(REFERENCE)?;
    let mut terminal = Terminal::new(TerminalOptions::new(15, 2, 10, 19))?;
    terminal.feed(b"\x1b[31mA\x1b[0mB\x1b[38;2;228;86;73mC\x1b[0m");
    let light = terminal.screen()?;
    theme_colors_match(&light, Theme::LIGHT, &reference.themes["light"])?;
    assert_eq!(light.cells[0].fg, Color::Palette(1));
    assert_eq!(light.cells[1].fg, Color::Default);
    // The same RGB as the light red palette slot remains an explicit color.
    assert_eq!(light.cells[2].fg, Color::Rgb(0xe45649));

    terminal.set_theme(Theme::DARK)?;
    let dark = terminal.screen()?;
    theme_colors_match(&dark, Theme::DARK, &reference.themes["dark"])?;
    assert_eq!(dark.cells, light.cells);
    assert_eq!(
        (dark.cursor_col, dark.cursor_row),
        (light.cursor_col, light.cursor_row)
    );
    Ok(())
}

#[test]
fn blank_background_and_osc_colors_are_preserved() -> Result<()> {
    let mut terminal = Terminal::new(TerminalOptions::new(12, 2, 10, 19))?;
    terminal.feed(
        b"\x1b[48;5;123m\x1b[2X\x1b[0m\x1b[3G\x1b[48;2;1;2;3m\x1b[2X\x1b[0m\
          \x1b]4;123;#123456\x1b\\\x1b]10;#abcdef\x1b\\\x1b]11;#456789\x1b\\\x1b]12;#cba987\x1b\\\
          \x1b[5G\x1b[4:2;58:2::7:8:9;53mD\x1b[0m",
    );
    let screen = terminal.screen()?;
    assert_eq!(&screen.cells[0].text, " ");
    assert_eq!(screen.cells[0].bg, Color::Palette(123));
    assert_eq!(&screen.cells[2].text, " ");
    assert_eq!(screen.cells[2].bg, Color::Rgb(0x010203));
    assert_eq!(screen.palette[123], 0x123456);
    assert_eq!(screen.foreground, 0xabcdef);
    assert_eq!(screen.background, 0x456789);
    assert_eq!(screen.cursor_color, Some(0xcba987));
    assert!(screen.cells[4].underline && screen.cells[4].overline);
    assert_eq!(screen.cells[4].underline_style, 2);
    assert_eq!(screen.cells[4].underline_color, Color::Rgb(0x070809));
    Ok(())
}

#[test]
fn reports_measured_cell_size_and_pixel_extent_after_resize() -> Result<()> {
    let mut terminal = Terminal::new(TerminalOptions::new(12, 5, 10, 19))?;
    terminal.feed(b"\x1b[16t\x1b[14t");
    assert_eq!(terminal.drain_responses(), b"\x1b[6;19;10t\x1b[4;95;120t");
    terminal.resize(13, 6, 11, 23)?;
    terminal.feed(b"\x1b[16t\x1b[14t");
    assert_eq!(terminal.drain_responses(), b"\x1b[6;23;11t\x1b[4;138;143t");
    assert!(terminal.resize(13, 6, 0, 23).is_err());
    terminal.feed(b"\x1b[16t");
    assert_eq!(terminal.drain_responses(), b"\x1b[6;23;11t");
    Ok(())
}

#[test]
fn scroll_move_and_resize_pull_are_independent() -> Result<()> {
    for pull in [true, false] {
        let mut options = TerminalOptions::new(12, 2, 10, 19);
        options.resize_pull_scrollback = pull;
        let mut terminal = Terminal::new(options)?;
        terminal.feed(b"one\r\ntwo\r\nthree\r\nfour");
        assert_eq!(
            terminal.screen()?.scrollbar,
            Scrollbar {
                offset: 2,
                total: 4,
                len: 2
            }
        );
        terminal.scroll(1)?;
        let screen = terminal.screen()?;
        assert_eq!(
            screen.scrollbar,
            Scrollbar {
                offset: 1,
                total: 4,
                len: 2
            }
        );
        assert_eq!(screen.cells[0].text, "t"); // "two"
        assert!(!screen.cursor_has_position);
        terminal.scroll(-1)?;
        terminal.resize(12, 4, 10, 19)?;
        let screen = terminal.screen()?;
        let (history, first) = if pull { (0, "o") } else { (2, "t") };
        assert_eq!(screen.history_lines, history);
        assert_eq!(screen.cells[0].text, first);
    }
    Ok(())
}

#[test]
fn synchronized_output_ends_on_sequence_resize_and_expiry() -> Result<()> {
    let mut terminal = Terminal::new(TerminalOptions::new(12, 3, 10, 19))?;
    terminal.feed(b"\x1b[?2026h");
    let first_deadline = terminal.sync_deadline().unwrap();
    assert!(first_deadline > Instant::now());
    terminal.feed(b"held\x1b[?2026l");
    assert!(!terminal.render_held());

    terminal.feed(b"\x1b[?2026h");
    terminal.resize(13, 3, 10, 19)?;
    assert!(!terminal.render_held());
    terminal.feed(b"\x1b[?2026h\x1b[2;");
    let deadline = terminal.sync_deadline().unwrap();
    thread::sleep(deadline.saturating_duration_since(Instant::now()));
    terminal.expire_sync()?;
    assert!(!terminal.render_held());
    // Ending the hold must not break an escape sequence received in two chunks.
    terminal.feed(b"3H\x1b[6n");
    assert_eq!(terminal.drain_responses(), b"\x1b[2;3R");
    Ok(())
}
