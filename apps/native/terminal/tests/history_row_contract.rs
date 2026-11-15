use std::{hint::black_box, time::Instant};

use anyhow::Result;
use matou_native_terminal::{Terminal, TerminalOptions};

#[test]
fn absolute_rows_match_scrolled_screens_without_moving_the_viewport() -> Result<()> {
    let mut terminal = Terminal::new(TerminalOptions::new(10, 4, 8, 16))?;
    let cluster = format!("a{}", "\u{301}".repeat(10));
    terminal
        .feed(format!("HEAD\r\n中e\u{301}  \r\n{cluster}\r\nabcdefghi中文tail \r\n").as_bytes());
    for row in 0..60 {
        terminal.feed(format!("HIST{row:02}\r\n").as_bytes());
    }
    let initial = terminal.screen()?;
    assert!(initial.history_lines > 40);
    let total = usize::try_from(initial.scrollbar.total)?;

    let wide_and_spaces = terminal.history_row(1)?;
    let cells = &wide_and_spaces.cells;
    assert_eq!(
        (&*cells[0].text, cells[0].width, cells[0].has_content),
        ("中", 2, true)
    );
    assert_eq!(
        (&*cells[1].text, cells[1].width, cells[1].has_content),
        ("", 0, false)
    );
    assert_eq!(cells[2].text, "e\u{301}");
    assert!(cells[3].has_content && cells[4].has_content);
    assert_eq!(cells[4].text, " ");
    assert_eq!(cells[5].text, " ");
    assert!(!cells[5].has_content);
    assert_eq!(terminal.history_row(2)?.cells[0].text, cluster);
    assert!(terminal.history_row(3)?.metadata.wrap);
    assert!(terminal.history_row(4)?.metadata.wrap_continuation);
    assert!(terminal.history_row(total).is_err());
    assert!(terminal.history_row(usize::MAX).is_err());
    assert_eq!(terminal.screen()?, initial);

    terminal.scroll(i32::MAX)?;
    loop {
        let viewport = terminal.screen()?;
        let start = usize::try_from(viewport.scrollbar.offset)?;
        for (index, screen_cells) in viewport.cells.chunks_exact(viewport.cols).enumerate() {
            let absolute = start + index;
            let row = terminal.history_row(absolute)?;
            assert_eq!(row.metadata, viewport.row_metadata[index], "row {absolute}");
            assert_eq!(row.cells.len(), screen_cells.len());
            for (col, (history, screen)) in row.cells.iter().zip(screen_cells).enumerate() {
                assert_eq!(
                    (&history.text, history.width, history.has_content),
                    (&screen.text, screen.width, screen.has_content),
                    "row {absolute}, column {col}",
                );
            }
        }
        assert_eq!(terminal.screen()?, viewport);
        if start + viewport.rows == total {
            break;
        }
        terminal.scroll(-i32::try_from(viewport.rows)?)?;
    }
    assert_eq!(terminal.screen()?, initial);
    Ok(())
}

#[test]
#[ignore = "local performance measurement"]
fn measure_ten_thousand_absolute_rows() -> Result<()> {
    const ROWS: usize = 10_000;
    let mut terminal = Terminal::new(TerminalOptions::new(80, 24, 8, 16))?;
    let line = "matou saved terminal line, Chinese 中文, and combining e\u{301}\r\n";
    terminal.feed(line.repeat(ROWS).as_bytes());
    let before = terminal.screen()?;
    assert!(before.scrollbar.total >= ROWS as u64);
    let start = Instant::now();
    for row in 0..ROWS {
        black_box(terminal.history_row(row)?);
    }
    eprintln!(
        "Read {ROWS} absolute rows at 80 columns in {:?}",
        start.elapsed()
    );
    assert_eq!(terminal.screen()?, before);
    Ok(())
}
