use serde::{Deserialize, Serialize};

const BUTTON_NONE: u8 = 3;
const BUTTON_WHEEL: u8 = 4;
const ACTION_UP: u8 = 0;
const ACTION_DOWN: u8 = 1;
const ACTION_LEFT: u8 = 2;
const ACTION_RIGHT: u8 = 3;
const ACTION_MOVE: u8 = 32;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseProtocol {
    #[default]
    None,
    X10,
    Vt200,
    Drag,
    Any,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouseEncoding {
    #[default]
    Default,
    Sgr,
    SgrPixels,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseEvent {
    pub col: i32,
    pub row: i32,
    pub x: i32,
    pub y: i32,
    pub button: u8,
    pub action: u8,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl MouseEvent {
    fn matches(self, last: Self, encoding: MouseEncoding) -> bool {
        let same_position = if encoding == MouseEncoding::SgrPixels {
            (self.x, self.y) == (last.x, last.y)
        } else {
            (self.col, self.row) == (last.col, last.row)
        };
        same_position
            && (self.button, self.action, self.ctrl, self.alt, self.shift)
                == (last.button, last.action, last.ctrl, last.alt, last.shift)
    }

    fn code(self, encoding: MouseEncoding) -> u8 {
        let modifiers = (if self.ctrl { 16 } else { 0 })
            | (if self.alt { 8 } else { 0 })
            | (if self.shift { 4 } else { 0 });
        if self.button == BUTTON_WHEEL {
            return modifiers | 64 | self.action;
        }

        let mut code = modifiers | (self.button & 3);
        if self.button & 4 != 0 {
            code |= 64;
        }
        if self.button & 8 != 0 {
            code |= 128;
        }
        if self.action == ACTION_MOVE {
            code |= ACTION_MOVE;
        } else if self.action == ACTION_UP && encoding == MouseEncoding::Default {
            code |= BUTTON_NONE;
        }
        code
    }

    fn encode(self, encoding: MouseEncoding) -> Option<Vec<u8>> {
        let code = self.code(encoding);
        let col = i64::from(self.col) + 1;
        let row = i64::from(self.row) + 1;
        if encoding == MouseEncoding::Default {
            return Some(vec![
                b'\x1b',
                b'[',
                b'M',
                code.checked_add(32)?,
                u8::try_from(col + 32).ok()?,
                u8::try_from(row + 32).ok()?,
            ]);
        }
        let (x, y) = if encoding == MouseEncoding::SgrPixels {
            (i64::from(self.x), i64::from(self.y))
        } else {
            (col, row)
        };
        let suffix = if self.action == ACTION_UP && self.button != BUTTON_WHEEL {
            'm'
        } else {
            'M'
        };
        Some(format!("\x1b[<{code};{x};{y}{suffix}").into_bytes())
    }
}

#[derive(Default)]
pub struct MouseReporter {
    last_event: Option<MouseEvent>,
}

impl MouseReporter {
    /// Cell coordinates are zero-based. Pixel coordinates are reported as supplied.
    pub fn report(
        &mut self,
        protocol: MouseProtocol,
        encoding: MouseEncoding,
        mut event: MouseEvent,
        cols: usize,
        rows: usize,
    ) -> Option<Vec<u8>> {
        if event.col < 0
            || event.col as usize >= cols
            || event.row < 0
            || event.row as usize >= rows
        {
            return None;
        }
        if (event.button == BUTTON_WHEEL && event.action == ACTION_MOVE)
            || (event.button == BUTTON_NONE && event.action != ACTION_MOVE)
            || (event.button != BUTTON_WHEEL && matches!(event.action, ACTION_LEFT | ACTION_RIGHT))
        {
            return None;
        }
        if event.action == ACTION_MOVE
            && self
                .last_event
                .is_some_and(|last| event.matches(last, encoding))
        {
            return None;
        }
        match protocol {
            MouseProtocol::None => return None,
            MouseProtocol::X10 => {
                if event.button == BUTTON_WHEEL || event.action != ACTION_DOWN {
                    return None;
                }
                event.ctrl = false;
                event.alt = false;
                event.shift = false;
            }
            MouseProtocol::Vt200 if event.action == ACTION_MOVE => return None,
            MouseProtocol::Drag if event.action == ACTION_MOVE && event.button == BUTTON_NONE => {
                return None;
            }
            _ => {}
        }
        // Neither a mode change nor extending outside the legacy encoding limit
        // resets duplicate detection. Only a fresh reporter resets it.
        self.last_event = Some(event);
        event.encode(encoding)
    }
}

#[derive(Default)]
pub struct MouseWheel {
    partial: f64,
}

impl MouseWheel {
    /// Positive input moves down. Pixel input accumulates against the height of one cell.
    pub fn consume(
        &mut self,
        delta_y: f32,
        pixel_delta: bool,
        cell_height: f32,
        shift: bool,
        alt: bool,
        ctrl: bool,
    ) -> f32 {
        if delta_y == 0. || shift {
            return 0.;
        }
        let mut amount = f64::from(delta_y);
        if alt || ctrl {
            amount *= 5.;
        }
        if !pixel_delta {
            return amount as f32;
        }
        amount /= f64::from(cell_height);
        if delta_y.abs() < 50. {
            amount *= 0.3;
        }
        self.partial += amount;
        let whole = self.partial.trunc();
        self.partial %= 1.;
        whole as f32
    }
}
