use anyhow::Result;
use matou_native_terminal::{
    MouseEncoding, MouseProtocol, SelectionAction, SelectionMode, Terminal, TerminalOptions,
};
use serde_json::Value;

#[test]
fn effective_mouse_modes_match_frozen_sequence_order() -> Result<()> {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/mouse-modes-reference.json"))?;
    let mut terminal = Terminal::new(TerminalOptions::new(80, 24, 8, 16))?;
    let initial_epoch = terminal.input_modes()?.input_epoch;
    for step in fixture["steps"].as_array().unwrap() {
        let input = step["input"].as_str().unwrap();
        terminal.feed(input.as_bytes());
        let modes = terminal.input_modes()?;
        let protocol = match modes.mouse_protocol {
            MouseProtocol::None => "NONE",
            MouseProtocol::X10 => "X10",
            MouseProtocol::Vt200 => "VT200",
            MouseProtocol::Drag => "DRAG",
            MouseProtocol::Any => "ANY",
        };
        let encoding = match modes.mouse_encoding {
            MouseEncoding::Default => "DEFAULT",
            MouseEncoding::Sgr => "SGR",
            MouseEncoding::SgrPixels => "SGR_PIXELS",
        };
        assert_eq!(protocol, step["protocol"], "{input:?}");
        assert_eq!(encoding, step["encoding"], "{input:?}");
        assert_eq!(
            modes.input_epoch,
            initial_epoch + usize::from(input == "\x1bc")
        );
    }
    Ok(())
}

#[test]
fn enabling_mouse_protocol_clears_selection_even_in_same_output_chunk() -> Result<()> {
    let mut terminal = Terminal::new(TerminalOptions::new(80, 24, 8, 16))?;
    terminal.feed(b"alpha beta");
    terminal.selection_action(SelectionAction::Begin {
        mode: SelectionMode::Word,
        row: 0,
        x: 2.0,
    })?;
    assert_eq!(terminal.selected_text()?, "alpha");
    terminal.feed(b"\x1b[?1000h\x1b[?1000l");
    assert!(terminal.selection_range()?.is_none());
    terminal.selection_action(SelectionAction::Begin {
        mode: SelectionMode::Word,
        row: 0,
        x: 2.0,
    })?;
    terminal.feed(b"\x1b[?1000l");
    assert_eq!(terminal.selected_text()?, "alpha");
    Ok(())
}
