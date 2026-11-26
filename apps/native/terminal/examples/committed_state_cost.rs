//! Isolate the parser/history cost of a protocol state plus a committed display state.
//! Uses the same 8 MiB payload as tooling/native/output-workload.py.
//! This measures parser work, not PTY, persistence, UI latency, or total app memory.
use std::{hint::black_box, time::Instant};

use anyhow::{Result, ensure};
use matou_native_terminal::{Terminal, TerminalOptions};

fn main() -> Result<()> {
    let count: usize = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "2".into())
        .parse()?;
    ensure!(matches!(count, 1 | 2), "use one or two terminal states");
    let mut terminals = (0..count)
        .map(|_| Terminal::new(TerminalOptions::new(80, 24, 9, 18)))
        .collect::<Result<Vec<_>>>()?;
    let chunk = format!("{}\n", "H".repeat(127)).repeat(64).into_bytes();
    let batches = 8 * 1024 * 1024 / chunk.len();
    let mut elapsed = Vec::with_capacity(batches);
    let start = Instant::now();
    for _ in 0..batches {
        let batch = Instant::now();
        for terminal in &mut terminals {
            terminal.feed(&chunk);
            black_box(terminal.drain_responses());
        }
        elapsed.push(batch.elapsed().as_secs_f64() * 1000.);
    }
    let total_ms = start.elapsed().as_secs_f64() * 1000.;
    elapsed.sort_by(f64::total_cmp);
    let screen = terminals.last_mut().unwrap().screen()?;
    println!(
        "{}",
        serde_json::json!({
            "terminal_states": count,
            "payload_bytes": batches * chunk.len(),
            "batch_bytes": chunk.len(),
            "grid": [screen.cols, screen.rows],
            "retained_history_rows": screen.history_lines,
            "total_parse_ms": total_ms,
            "batch_median_ms": elapsed[elapsed.len() / 2],
            "batch_p95_ms": elapsed[(elapsed.len() * 95 / 100).min(elapsed.len() - 1)],
            "batch_max_ms": elapsed[elapsed.len() - 1],
            "scope": "parser and retained history only; not end-to-end performance"
        })
    );
    Ok(())
}
