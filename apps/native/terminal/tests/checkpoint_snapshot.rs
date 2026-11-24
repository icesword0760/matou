use anyhow::Result;
use matou_native_terminal::{Terminal, TerminalOptions};

#[test]
fn vt_snapshot_restores_color_scrollback_and_cursor() -> Result<()> {
    let options = TerminalOptions::new(12, 4, 8, 16);
    let mut source = Terminal::new(options)?;
    source.feed(b"old line\r\nwrapped line with more text\r\nthird line\r\n\x1b[31mred\x1b[0m\r\nlast\x1b[2;7H");
    let before = source.screen()?;
    assert!(before.history_lines > 0);

    let snapshot = source.format_vt_snapshot()?;
    let mut restored = Terminal::new(options)?;
    restored.feed(&snapshot);
    let after = restored.screen()?;
    assert_eq!(
        (after.cursor_col, after.cursor_row),
        (before.cursor_col, before.cursor_row),
        "{:?}",
        String::from_utf8_lossy(&snapshot),
    );
    assert_eq!(after.history_lines, before.history_lines);
    assert_eq!(after, before);
    for row in 0..before.history_lines {
        assert_eq!(restored.history_row(row)?, source.history_row(row)?);
    }
    Ok(())
}

#[test]
fn vt_snapshot_preserves_continuation_after_full_width_write() -> Result<()> {
    let options = TerminalOptions::new(8, 4, 8, 16);
    let mut source = Terminal::new(options)?;
    source.feed(b"12345678");
    let snapshot = source.format_vt_snapshot()?;
    let mut restored = Terminal::new(options)?;
    restored.feed(&snapshot);
    source.feed(b"!");
    restored.feed(b"!");
    assert_eq!(
        restored.screen()?,
        source.screen()?,
        "{:?}",
        String::from_utf8_lossy(&snapshot)
    );
    Ok(())
}

#[test]
fn vt_snapshot_preserves_cursor_in_origin_mode() -> Result<()> {
    let options = TerminalOptions::new(8, 6, 8, 16);
    let mut source = Terminal::new(options)?;
    source.feed(b"hello\x1b[2;5r\x1b[?6h\x1b[2;3H");
    let before = source.screen()?;
    let snapshot = source.format_vt_snapshot()?;
    let mut restored = Terminal::new(options)?;
    restored.feed(&snapshot);
    let after = restored.screen()?;
    assert_eq!(
        (after.cursor_col, after.cursor_row),
        (before.cursor_col, before.cursor_row),
        "{:?}",
        String::from_utf8_lossy(&snapshot),
    );
    Ok(())
}

#[test]
fn vt_snapshot_restores_cursor_appearance() -> Result<()> {
    let options = TerminalOptions::new(8, 6, 8, 16);
    let mut source = Terminal::new(options)?;
    source.feed(b"hello\x1b[2 q\x1b]12;#112233\x07");
    let before = source.screen()?;
    let snapshot = source.format_vt_snapshot()?;
    let mut restored = Terminal::new(options)?;
    restored.feed(&snapshot);
    let after = restored.screen()?;
    assert_eq!(
        (
            after.cursor_style,
            after.cursor_blinking,
            after.cursor_color
        ),
        (
            before.cursor_style,
            before.cursor_blinking,
            before.cursor_color
        )
    );
    Ok(())
}

#[test]
fn vt_snapshot_requires_primary_screen_and_a_complete_stream() -> Result<()> {
    let mut terminal = Terminal::new(TerminalOptions::new(8, 6, 8, 16))?;
    terminal.feed(b"main\x1b[?1049halternate");
    assert!(!terminal.can_format_vt_snapshot()?);
    assert!(terminal.format_vt_snapshot().is_err());
    terminal.feed(b"\x1b[?1049l\x1b[?2026h");
    assert!(!terminal.can_format_vt_snapshot()?);
    terminal.feed(b"\x1b[?2026l\x1b[31");
    assert!(!terminal.can_format_vt_snapshot()?);
    terminal.feed(b"m");
    assert!(terminal.can_format_vt_snapshot()?);
    Ok(())
}
