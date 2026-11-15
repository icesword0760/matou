mod ghostty;
mod model;
mod selection;

pub use ghostty::{Terminal, TerminalOptions};
pub use model::{
    Cell, Color, CursorStyle, HistoryCell, HistoryRow, InputModes, RowMetadata, Screen, Scrollbar,
    Theme,
};
pub use selection::{
    GridPoint, GridSelection, SelectionAction, SelectionDrag, SelectionMode, SelectionState,
};
