use serde::{Deserialize, Serialize};

/// A raw cell color. Default and indexed colors follow the active terminal theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "lowercase")]
pub enum Color {
    Default,
    Palette(u8),
    Rgb(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CursorStyle {
    Bar,
    Block,
    Underline,
    BlockHollow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub foreground: u32,
    pub background: u32,
    pub cursor: u32,
    pub cursor_accent: u32,
    pub selection_background: u32,
    pub ansi: [u32; 16],
}

impl Theme {
    pub const LIGHT: Self = Self {
        foreground: 0x2f3547,
        background: 0xfcfcfd,
        cursor: 0xff7809,
        cursor_accent: 0xffffff,
        selection_background: 0xbfceff,
        ansi: [
            0x383a42, 0xe45649, 0x50a14f, 0xc18401, 0x4078f2, 0xa626a4, 0x0184bc, 0xa0a1a7,
            0x4f525e, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0x383a42,
        ],
    };

    pub const DARK: Self = Self {
        foreground: 0xfafafa,
        background: 0x1b1b1b,
        cursor: 0xff7809,
        cursor_accent: 0x0d1117,
        selection_background: 0x264f78,
        ansi: [
            0x484f58, 0xff7b72, 0x3fb950, 0xd29922, 0x58a6ff, 0xbc8cff, 0x39d353, 0xb1bac4,
            0x6e7681, 0xffa198, 0x56d364, 0xe3b341, 0x79c0ff, 0xd2a8ff, 0x56d364, 0xf0f6fc,
        ],
    };

    pub fn palette(self) -> [u32; 256] {
        let mut palette = [0; 256];
        palette[..16].copy_from_slice(&self.ansi);
        let levels = [0, 95, 135, 175, 215, 255];
        for (index, color) in palette[16..232].iter_mut().enumerate() {
            let (red, green, blue) = (
                levels[index / 36],
                levels[(index / 6) % 6],
                levels[index % 6],
            );
            *color = (red << 16) | (green << 8) | blue;
        }
        for (index, color) in palette[232..].iter_mut().enumerate() {
            let channel = 8 + 10 * index as u32;
            *color = channel * 0x010101;
        }
        palette
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::LIGHT
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    /// Invisible text remains available to selection, search, and copy.
    pub text: String,
    /// Whether this cell contains an explicit character, including a literal space.
    pub has_content: bool,
    /// 0 for the continuation cell after a wide glyph, otherwise 1 or 2.
    pub width: u8,
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub italic: bool,
    pub dim: bool,
    pub underline: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub strikethrough: bool,
    pub overline: bool,
    pub blink: bool,
    /// Ghostty SGR underline style (0 is none, 1 is single, 2 is double).
    pub underline_style: u8,
    pub underline_color: Color,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowMetadata {
    pub wrap: bool,
    pub wrap_continuation: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scrollbar {
    pub offset: u64,
    pub total: u64,
    pub len: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Screen {
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<Cell>,
    pub row_metadata: Vec<RowMetadata>,
    /// Coordinates describe the viewport and are valid when cursor_has_position holds.
    pub cursor_col: usize,
    pub cursor_row: usize,
    pub cursor_has_position: bool,
    pub cursor_visible: bool,
    pub cursor_style: CursorStyle,
    pub cursor_blinking: bool,
    pub foreground: u32,
    pub background: u32,
    pub cursor_color: Option<u32>,
    pub palette: [u32; 256],
    pub history_lines: usize,
    pub scrollbar: Scrollbar,
    pub alternate: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputModes {
    pub application_cursor_keys: bool,
    pub bracketed_paste: bool,
}
