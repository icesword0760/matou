use anyhow::Result;
use matou_native_terminal::{SelectionAction, SelectionMode, Terminal, TerminalOptions};

fn select_word(terminal: &mut Terminal, row: usize, x: f32) -> Result<()> {
    terminal.selection_action(SelectionAction::Begin {
        mode: SelectionMode::Word,
        row,
        x,
    })
}

#[test]
fn owned_selection_keeps_a_word_after_history_is_trimmed() -> Result<()> {
    let mut terminal = Terminal::new(TerminalOptions::new(80, 24, 8, 16))?;
    const WORD_ROW: usize = 9_000;
    terminal.feed(&b"history filler\r\n".repeat(WORD_ROW));
    terminal.feed(b"prefix RETAINED suffix\r\n");
    select_word(&mut terminal, WORD_ROW, 9.)?;
    terminal.selection_action(SelectionAction::Extend {
        row: WORD_ROW,
        x: 9.,
    })?;
    assert_eq!(terminal.selected_text()?, "RETAINED");
    let before = terminal.selection_range()?.unwrap();
    let trimmed_before = terminal.history_trimmed_rows()?;

    terminal.feed(&b"more history\r\n".repeat(5_000));
    let removed = terminal.history_trimmed_rows()? - trimmed_before;
    assert!(removed > 0 && removed < WORD_ROW);
    assert_eq!(terminal.selected_text()?, "RETAINED");
    let after = terminal.selection_range()?.unwrap();
    assert_eq!(after.anchor.row, before.anchor.row - removed);
    assert_eq!(after.end.row, before.end.row - removed);
    Ok(())
}

#[test]
fn owned_selection_clears_after_same_feed_buffer_switch_and_hard_reset() -> Result<()> {
    let mut terminal = Terminal::new(TerminalOptions::new(12, 4, 8, 16))?;
    terminal.feed(b"SELECTED");
    select_word(&mut terminal, 0, 2.)?;
    assert_eq!(terminal.selected_text()?, "SELECTED");

    terminal.feed(b"\x1b[?1049hALTERNATE\x1b[?1049l");
    assert!(!terminal.screen()?.alternate);
    assert_eq!(terminal.selected_text()?, "");
    assert_eq!(terminal.selection_range()?, None);
    select_word(&mut terminal, 0, 2.)?;
    assert_eq!(terminal.selected_text()?, "SELECTED");

    terminal.feed(b"\x1bcNEW_BUFFER");
    assert_eq!(terminal.selection_range()?, None);
    assert_eq!(terminal.selected_text()?, "");
    Ok(())
}

#[test]
fn owned_selection_keeps_rows_when_the_scroll_region_or_viewport_moves() -> Result<()> {
    let mut terminal = Terminal::new(TerminalOptions::new(10, 4, 8, 16))?;
    terminal.feed(b"A\r\nB\r\nC\r\nD");
    select_word(&mut terminal, 1, 0.)?;
    assert_eq!(terminal.selected_text()?, "B");
    let selected = terminal.selection_range()?;

    terminal.feed(b"\x1b[2;4r\x1b[4;1H\nE");
    assert_eq!(terminal.selection_range()?, selected);
    assert_eq!(terminal.selected_text()?, "C");

    terminal.feed(b"\x1b[r\x1b[4;1H\r\nF\r\nG\r\nH");
    let bottom = terminal.screen()?.scrollbar.offset;
    terminal.scroll(2)?;
    assert!(terminal.screen()?.scrollbar.offset < bottom);
    assert_eq!(terminal.selection_range()?, selected);
    assert_eq!(terminal.selected_text()?, "C");
    Ok(())
}
