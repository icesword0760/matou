use anyhow::Result;
use matou_native_terminal::{Terminal, TerminalOptions};

#[test]
fn history_trim_counts_only_rows_removed_from_the_buffer_prefix() -> Result<()> {
    let mut terminal = Terminal::new(TerminalOptions::new(80, 24, 8, 16))?;
    assert_eq!(terminal.screen()?.cursor_row, 0);
    assert_eq!(terminal.history_trimmed_rows()?, 0);

    const FIRST_BATCH: usize = 20_000;
    terminal.feed(b"original history\r\n".repeat(FIRST_BATCH).as_slice());
    let retained_before = usize::try_from(terminal.screen()?.scrollbar.total)?;
    let removed_before = terminal.history_trimmed_rows()?;
    assert!(removed_before > 0);
    assert_eq!(removed_before, FIRST_BATCH + 1 - retained_before);

    const SECOND_BATCH: usize = 12_000;
    terminal.feed(b"additional history\r\n".repeat(SECOND_BATCH).as_slice());
    let retained_after = usize::try_from(terminal.screen()?.scrollbar.total)?;
    let removed_after = terminal.history_trimmed_rows()?;
    assert!(removed_after > removed_before);
    assert_eq!(
        removed_after - removed_before,
        retained_before + SECOND_BATCH - retained_after,
    );

    for action in [
        b"\x1b[2;4r\x1b[4;1H\nE\x1b[r".as_slice(),
        b"\x1b[H\x1b[L",
        b"\x1b[H\x1b[M",
    ] {
        terminal.feed(action);
        assert_eq!(terminal.history_trimmed_rows()?, removed_after);
    }

    let remaining_history = terminal.screen()?.history_lines;
    terminal.feed(b"\x1b[3J");
    assert_eq!(terminal.screen()?.history_lines, 0);
    let removed_by_clear = terminal.history_trimmed_rows()? - removed_after;
    assert_eq!(removed_by_clear, remaining_history);
    println!(
        "Trimmed {removed_before} rows after {FIRST_BATCH} appended, {} after {SECOND_BATCH} more, and {removed_by_clear} on history clear",
        removed_after - removed_before
    );
    Ok(())
}
