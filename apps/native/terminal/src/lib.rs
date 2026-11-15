mod clipboard;
mod ghostty;
mod model;
mod mouse;
mod selection;

pub use clipboard::decode_clipboard;
pub use ghostty::{Terminal, TerminalOptions};
pub use model::{
    Cell, Color, CursorStyle, HistoryCell, HistoryRow, InputModes, RowMetadata, Screen, Scrollbar,
    Theme,
};
pub use mouse::{MouseEncoding, MouseEvent, MouseProtocol, MouseReporter, MouseWheel};
pub use selection::{
    GridPoint, GridSelection, SelectionAction, SelectionDrag, SelectionMode, SelectionState,
};
