use anyhow::Result;
use matou_native_terminal::{Terminal, TerminalOptions};

#[test]
fn detects_buffer_round_trip_and_hard_reset_within_output_batches() -> Result<()> {
    let mut terminal = Terminal::new(TerminalOptions::new(80, 24, 8, 16))?;
    let initial = terminal.selection_epoch()?;
    terminal.feed(b"primary\x1b[?1049halternate\x1b[?1049l");
    assert!(!terminal.screen()?.alternate);
    assert_eq!(terminal.selection_epoch()?.wrapping_sub(initial), 2);
    let switched = terminal.selection_epoch()?;
    terminal.feed(b"\x1b[?1049l\x1b[2;4r\x1b[4;1H\nE\x1b[r\x1b[!p");
    assert_eq!(terminal.selection_epoch()?, switched);
    terminal.feed(b"\x1b");
    assert_eq!(terminal.selection_epoch()?, switched);
    terminal.feed(b"cafter reset");
    assert_eq!(terminal.selection_epoch()?.wrapping_sub(switched), 1);
    Ok(())
}
