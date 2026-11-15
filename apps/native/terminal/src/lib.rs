mod ghostty;
mod model;

pub use ghostty::{Terminal, TerminalOptions};
pub use model::{
    Cell, Color, CursorStyle, HistoryCell, HistoryRow, InputModes, RowMetadata, Screen, Scrollbar,
    Theme,
};
