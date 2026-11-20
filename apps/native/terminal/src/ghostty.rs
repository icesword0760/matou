use std::{
    ffi::{c_int, c_void},
    mem::{self, MaybeUninit},
    ptr::{self, NonNull},
    slice, str,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail, ensure};

use crate::{
    Cell, Color, CursorStyle, GridSelection, HistoryCell, HistoryRow, InputModes, RowMetadata,
    Screen, Scrollbar, SelectionAction, SelectionState, Theme,
};

const SYNC_TIMEOUT: Duration = Duration::from_millis(150);

// Only local C shim layouts cross the FFI boundary.
#[repr(C)]
struct ScreenMetadata {
    history_lines: usize,
    scroll_offset: u64,
    scroll_total: u64,
    scroll_len: u64,
    foreground: u32,
    background: u32,
    cursor_color: u32,
    palette: [u32; 256],
    cols: u16,
    rows: u16,
    cursor_col: u16,
    cursor_row: u16,
    cursor_has_position: u8,
    cursor_visible: u8,
    cursor_style: u8,
    cursor_blinking: u8,
    cursor_has_color: u8,
    alternate: u8,
}

type WriteCallback = unsafe extern "C" fn(*mut c_void, *const u8, usize);
type HoldCallback = unsafe extern "C" fn(*mut c_void, bool);
type RowCallback = unsafe extern "C" fn(*mut c_void, bool, bool);
type CellCallback = unsafe extern "C" fn(*mut c_void, *const u8, usize, u32, u32, u32, u16, u8);
type HistoryCellCallback = unsafe extern "C" fn(*mut c_void, *const u32, usize, u8);

unsafe extern "C" {
    fn matou_vt_new(
        out: *mut *mut c_void,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
        history_lines: usize,
        pull_scrollback: bool,
        data: *mut c_void,
        write: WriteCallback,
        hold: HoldCallback,
        clipboard: WriteCallback,
    ) -> c_int;
    fn matou_vt_free(terminal: *mut c_void);
    fn matou_vt_set_theme(
        terminal: *mut c_void,
        foreground: u32,
        background: u32,
        cursor: u32,
        palette: *const u32,
    ) -> c_int;
    fn matou_vt_feed(terminal: *mut c_void, data: *const u8, len: usize);
    fn matou_vt_input_modes(
        terminal: *mut c_void,
        cursor_keys: *mut bool,
        bracketed_paste: *mut bool,
        mouse_modes: *mut usize,
        input_epoch: *mut usize,
    ) -> c_int;
    fn matou_vt_resize(
        terminal: *mut c_void,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> c_int;
    fn matou_vt_scroll(terminal: *mut c_void, lines: i32) -> c_int;
    fn matou_vt_expire_hold(terminal: *mut c_void) -> c_int;
    fn matou_vt_screen(
        terminal: *mut c_void,
        screen: *mut ScreenMetadata,
        emit_row: RowCallback,
        emit_cell: CellCallback,
        data: *mut c_void,
    ) -> c_int;
    fn matou_vt_history_row(
        terminal: *const c_void,
        row: usize,
        wrap: *mut bool,
        continuation: *mut bool,
        emit_cell: HistoryCellCallback,
        data: *mut c_void,
    ) -> c_int;
    fn matou_vt_selection_epoch(terminal: *const c_void, out: *mut usize) -> c_int;
    fn matou_vt_history_trimmed_rows(terminal: *const c_void, out: *mut usize) -> c_int;
}

#[derive(Default)]
struct Effects {
    responses: Vec<u8>,
    clipboard: Vec<String>,
    hold_deadline: Option<Instant>,
}

#[derive(Default)]
struct ScreenContent {
    cells: Vec<Cell>,
    rows: Vec<RowMetadata>,
    invalid_text: bool,
}

#[derive(Default)]
struct HistoryContent {
    cells: Vec<HistoryCell>,
    invalid_text: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct TerminalOptions {
    pub cols: u16,
    pub rows: u16,
    pub cell_width_px: u32,
    pub cell_height_px: u32,
    pub history_lines: usize,
    pub resize_pull_scrollback: bool,
    pub theme: Theme,
}

impl TerminalOptions {
    /// Create options from the measured terminal geometry.
    pub fn new(cols: u16, rows: u16, cell_width_px: u32, cell_height_px: u32) -> Self {
        Self {
            cols,
            rows,
            cell_width_px,
            cell_height_px,
            history_lines: 10_000,
            resize_pull_scrollback: !cfg!(windows),
            theme: Theme::default(),
        }
    }
}

/// Thread-affine terminal state. Pass the owned Screen to other threads.
pub struct Terminal {
    handle: NonNull<c_void>,
    // Owned Box<Effects> allocation borrowed synchronously by the C callbacks.
    // Using a raw owner lets the allocation keep the same provenance on moves.
    effects: NonNull<Effects>,
    pending_utf8: [u8; 4],
    pending_utf8_len: usize,
    rows: u16,
    selection: SelectionState,
    last_selection_epoch: usize,
    last_history_trim: usize,
}

impl Terminal {
    pub fn new(options: TerminalOptions) -> Result<Self> {
        check_dimensions(
            options.cols,
            options.rows,
            options.cell_width_px,
            options.cell_height_px,
        )?;
        let effects = NonNull::from(Box::leak(Box::<Effects>::default()));
        let mut handle = ptr::null_mut();
        let status = unsafe {
            matou_vt_new(
                &mut handle,
                options.cols,
                options.rows,
                options.cell_width_px,
                options.cell_height_px,
                options.history_lines,
                options.resize_pull_scrollback,
                effects.as_ptr().cast(),
                on_write,
                on_hold,
                on_clipboard,
            )
        };
        let handle = check(status, "initialize")
            .and_then(|()| NonNull::new(handle).context("Ghostty VT returned no terminal handle"));
        if handle.is_err() {
            unsafe { drop(Box::from_raw(effects.as_ptr())) };
        }
        let mut terminal = Self {
            handle: handle?,
            effects,
            pending_utf8: [0; 4],
            pending_utf8_len: 0,
            rows: options.rows,
            selection: SelectionState::default(),
            last_selection_epoch: 0,
            last_history_trim: 0,
        };
        terminal.last_selection_epoch = terminal.selection_epoch()?;
        terminal.last_history_trim = terminal.history_trimmed_rows()?;
        terminal.set_theme(options.theme)?;
        Ok(terminal)
    }

    /// Feed valid UTF-8 to the VT parser, dropping malformed bytes and keeping
    /// an incomplete character for the next call.
    pub fn feed(&mut self, mut bytes: &[u8]) {
        while self.pending_utf8_len != 0 {
            let Some((&byte, rest)) = bytes.split_first() else {
                return;
            };
            self.pending_utf8[self.pending_utf8_len] = byte;
            let len = self.pending_utf8_len + 1;
            let pending = &self.pending_utf8[..len];
            match str::from_utf8(pending) {
                Ok(_) => {
                    self.feed_vt(pending);
                    self.pending_utf8_len = 0;
                }
                Err(error) if error.error_len().is_none() => self.pending_utf8_len = len,
                Err(_) => {
                    // Reprocess the byte that broke the sequence. It can start a
                    // valid character or a control sequence.
                    self.pending_utf8_len = 0;
                    break;
                }
            }
            bytes = rest;
        }

        while !bytes.is_empty() {
            let Err(error) = str::from_utf8(bytes) else {
                self.feed_vt(bytes);
                break;
            };
            let (valid, tail) = bytes.split_at(error.valid_up_to());
            if !valid.is_empty() {
                self.feed_vt(valid);
            }
            match error.error_len() {
                Some(invalid_len) => bytes = &tail[invalid_len..],
                None => {
                    self.pending_utf8[..tail.len()].copy_from_slice(tail);
                    self.pending_utf8_len = tail.len();
                    break;
                }
            }
        }
    }

    fn feed_vt(&self, bytes: &[u8]) {
        unsafe { matou_vt_feed(self.handle.as_ptr(), bytes.as_ptr(), bytes.len()) };
    }

    pub fn resize(
        &mut self,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> Result<()> {
        check_dimensions(cols, rows, cell_width_px, cell_height_px)?;
        self.reconcile_selection()?;
        check(
            unsafe {
                matou_vt_resize(
                    self.handle.as_ptr(),
                    cols,
                    rows,
                    cell_width_px,
                    cell_height_px,
                )
            },
            "resize",
        )?;
        if rows != self.rows {
            self.rows = rows;
            self.selection.clear();
        }
        self.reconcile_selection()
    }

    pub fn set_theme(&mut self, theme: Theme) -> Result<()> {
        let palette = theme.palette();
        check(
            unsafe {
                matou_vt_set_theme(
                    self.handle.as_ptr(),
                    theme.foreground,
                    theme.background,
                    theme.cursor,
                    palette.as_ptr(),
                )
            },
            "set theme",
        )
    }

    pub fn input_modes(&self) -> Result<InputModes> {
        let (mut cursor_keys, mut bracketed_paste) = (false, false);
        let mut mouse_modes = 0usize;
        let mut input_epoch = 0usize;
        check(
            unsafe {
                matou_vt_input_modes(
                    self.handle.as_ptr(),
                    &mut cursor_keys,
                    &mut bracketed_paste,
                    &mut mouse_modes,
                    &mut input_epoch,
                )
            },
            "read input modes",
        )?;
        Ok(InputModes {
            application_cursor_keys: cursor_keys,
            bracketed_paste,
            input_epoch,
            mouse_protocol: match mouse_modes & 0xff {
                0 => crate::MouseProtocol::None,
                1 => crate::MouseProtocol::X10,
                2 => crate::MouseProtocol::Vt200,
                3 => crate::MouseProtocol::Drag,
                4 => crate::MouseProtocol::Any,
                other => bail!("Unknown terminal mouse protocol {other}"),
            },
            mouse_encoding: match mouse_modes >> 8 {
                0 => crate::MouseEncoding::Default,
                2 => crate::MouseEncoding::Sgr,
                4 => crate::MouseEncoding::SgrPixels,
                other => bail!("Unknown terminal mouse encoding {other}"),
            },
        })
    }

    pub fn screen(&mut self) -> Result<Screen> {
        let mut metadata = MaybeUninit::<ScreenMetadata>::zeroed();
        let mut content = ScreenContent::default();
        check(
            unsafe {
                matou_vt_screen(
                    self.handle.as_ptr(),
                    metadata.as_mut_ptr(),
                    on_row,
                    on_cell,
                    ptr::from_mut(&mut content).cast(),
                )
            },
            "read screen",
        )?;
        // All fields are integers and the C shim fills in the active state.
        let metadata = unsafe { metadata.assume_init() };
        let (cols, rows) = (usize::from(metadata.cols), usize::from(metadata.rows));
        ensure!(!content.invalid_text, "Ghostty VT returned invalid UTF-8");
        ensure!(
            content.rows.len() == rows && content.cells.len() == cols * rows,
            "Ghostty VT returned an incomplete screen"
        );
        let cursor_style = match metadata.cursor_style {
            0 => CursorStyle::Bar,
            1 => CursorStyle::Block,
            2 => CursorStyle::Underline,
            3 => CursorStyle::BlockHollow,
            style => bail!("Ghostty VT returned unsupported cursor style {style}"),
        };
        Ok(Screen {
            cols,
            rows,
            cells: content.cells,
            row_metadata: content.rows,
            cursor_col: metadata.cursor_col.into(),
            cursor_row: metadata.cursor_row.into(),
            cursor_has_position: metadata.cursor_has_position != 0,
            cursor_visible: metadata.cursor_visible != 0,
            cursor_style,
            cursor_blinking: metadata.cursor_blinking != 0,
            foreground: metadata.foreground,
            background: metadata.background,
            cursor_color: (metadata.cursor_has_color != 0).then_some(metadata.cursor_color),
            palette: metadata.palette,
            history_lines: metadata.history_lines,
            scrollbar: Scrollbar {
                offset: metadata.scroll_offset,
                total: metadata.scroll_total,
                len: metadata.scroll_len,
            },
            alternate: metadata.alternate != 0,
        })
    }

    /// Read an absolute row of the active buffer, including its retained history.
    /// Row zero is the oldest retained row. The viewport stays where it is.
    pub fn history_row(&self, row: usize) -> Result<HistoryRow> {
        let mut wrap = false;
        let mut wrap_continuation = false;
        let mut content = HistoryContent::default();
        check(
            unsafe {
                matou_vt_history_row(
                    self.handle.as_ptr(),
                    row,
                    &mut wrap,
                    &mut wrap_continuation,
                    on_history_cell,
                    ptr::from_mut(&mut content).cast(),
                )
            },
            "read history row",
        )?;
        ensure!(!content.invalid_text, "Ghostty VT returned invalid UTF-32");
        Ok(HistoryRow {
            cells: content.cells,
            metadata: RowMetadata {
                wrap,
                wrap_continuation,
            },
        })
    }

    /// Changes whenever a buffer switch or hard reset invalidates selections,
    /// including a switch away and back within a single feed.
    pub fn selection_epoch(&self) -> Result<usize> {
        let mut epoch = 0;
        check(
            unsafe { matou_vt_selection_epoch(self.handle.as_ptr(), &mut epoch) },
            "read selection lifecycle",
        )?;
        Ok(epoch)
    }

    /// Count history rows removed from the start of the active buffer's page list.
    /// Use deltas within the same page list. Resets and buffer switches can change it.
    pub fn history_trimmed_rows(&self) -> Result<usize> {
        let mut rows = 0;
        check(
            unsafe { matou_vt_history_trimmed_rows(self.handle.as_ptr(), &mut rows) },
            "read history trim count",
        )?;
        Ok(rows)
    }

    pub fn selection_action(&mut self, action: SelectionAction) -> Result<()> {
        self.reconcile_selection()?;
        match action {
            SelectionAction::Select { row, col, length } => self.selection.select(row, col, length),
            SelectionAction::Begin { mode, row, x } => {
                self.update_selection(|selection, terminal| selection.begin(terminal, mode, row, x))
            }
            SelectionAction::IncrementalClick { row, x } => {
                self.update_selection(|selection, terminal| {
                    selection.incremental_click(terminal, row, x)
                })
            }
            SelectionAction::Extend { row, x } => {
                self.update_selection(|selection, terminal| selection.extend(terminal, row, x))
            }
            SelectionAction::ExtendOutside { row, x, below } => {
                self.update_selection(|selection, terminal| {
                    selection.extend_outside(terminal, row, x, below)
                })
            }
            SelectionAction::Scroll { lines } => {
                self.update_selection(|selection, terminal| selection.scroll(terminal, lines))
            }
            SelectionAction::SetEnd(end) => {
                self.selection.set_end(end);
                Ok(())
            }
            SelectionAction::Clear => {
                self.selection.clear();
                Ok(())
            }
            SelectionAction::SelectAll => {
                self.selection.select_all();
                Ok(())
            }
        }
    }

    pub fn selection_range(&mut self) -> Result<Option<GridSelection>> {
        self.reconcile_selection()?;
        let (cols, total) = crate::selection::grid_size(self)?;
        Ok(self.selection.range(cols, total))
    }

    pub fn selected_text(&mut self) -> Result<String> {
        match self.selection_range()? {
            Some(range) => range.text(self),
            None => Ok(String::new()),
        }
    }

    fn update_selection(
        &mut self,
        update: impl FnOnce(&mut SelectionState, &mut Self) -> Result<()>,
    ) -> Result<()> {
        let mut selection = mem::take(&mut self.selection);
        let result = update(&mut selection, self);
        self.selection = selection;
        result
    }

    fn reconcile_selection(&mut self) -> Result<()> {
        let epoch = self.selection_epoch()?;
        let trimmed = self.history_trimmed_rows()?;
        if epoch != self.last_selection_epoch {
            self.selection.reset();
        } else if trimmed > self.last_history_trim {
            self.selection.trim(trimmed - self.last_history_trim);
        }
        self.last_selection_epoch = epoch;
        self.last_history_trim = trimmed;
        Ok(())
    }

    /// Scroll up by a positive number of lines, or down by a negative number.
    pub fn scroll(&mut self, lines: i32) -> Result<()> {
        check(
            unsafe { matou_vt_scroll(self.handle.as_ptr(), lines) },
            "scroll viewport",
        )
    }

    pub fn drain_clipboard(&mut self) -> Vec<String> {
        mem::take(&mut unsafe { self.effects.as_mut() }.clipboard)
    }

    pub fn drain_responses(&mut self) -> Vec<u8> {
        std::mem::take(&mut unsafe { self.effects.as_mut() }.responses)
    }

    pub fn render_held(&self) -> bool {
        self.sync_deadline().is_some()
    }

    pub fn sync_deadline(&self) -> Option<Instant> {
        unsafe { self.effects.as_ref() }.hold_deadline
    }

    /// End an expired synchronized-output hold. The caller manages the displayed frame.
    pub fn expire_sync(&mut self) -> Result<()> {
        if self
            .sync_deadline()
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.force_sync_end()?;
        }
        Ok(())
    }

    pub fn force_sync_end(&mut self) -> Result<()> {
        if self.render_held() {
            check(
                unsafe { matou_vt_expire_hold(self.handle.as_ptr()) },
                "end synchronized output",
            )?;
            unsafe { self.effects.as_mut() }.hold_deadline = None;
        }
        Ok(())
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // Keep callback state alive until all C handles have been released.
        unsafe {
            matou_vt_free(self.handle.as_ptr());
            drop(Box::from_raw(self.effects.as_ptr()));
        }
    }
}

fn check_dimensions(cols: u16, rows: u16, width: u32, height: u32) -> Result<()> {
    ensure!(
        cols >= 2 && rows > 0,
        "unsupported terminal dimensions {cols}×{rows}"
    );
    ensure!(
        width > 0
            && height > 0
            && width.checked_mul(u32::from(cols)).is_some()
            && height.checked_mul(u32::from(rows)).is_some(),
        "unsupported terminal cell size {width}×{height} pixels for {cols}×{rows} cells"
    );
    Ok(())
}

fn check(status: c_int, operation: &str) -> Result<()> {
    ensure!(
        status == 0,
        "Ghostty VT failed to {operation} (status {status})"
    );
    Ok(())
}

unsafe extern "C" fn on_write(data: *mut c_void, bytes: *const u8, len: usize) {
    if len != 0 {
        // The callback runs synchronously during an exclusive terminal operation.
        let effects = unsafe { &mut *data.cast::<Effects>() };
        effects
            .responses
            .extend_from_slice(unsafe { slice::from_raw_parts(bytes, len) });
    }
}

unsafe extern "C" fn on_clipboard(data: *mut c_void, bytes: *const u8, len: usize) {
    if len == 0 {
        return;
    }
    let content = unsafe { slice::from_raw_parts(bytes, len) };
    if let Ok(content) = str::from_utf8(content)
        && let Some(text) = crate::decode_clipboard(content)
    {
        let effects = unsafe { &mut *data.cast::<Effects>() };
        effects.clipboard.push(text);
    }
}

unsafe extern "C" fn on_hold(data: *mut c_void, held: bool) {
    let effects = unsafe { &mut *data.cast::<Effects>() };
    effects.hold_deadline = held.then(|| Instant::now() + SYNC_TIMEOUT);
}

unsafe extern "C" fn on_row(data: *mut c_void, wrap: bool, wrap_continuation: bool) {
    let screen = unsafe { &mut *data.cast::<ScreenContent>() };
    screen.rows.push(RowMetadata {
        wrap,
        wrap_continuation,
    });
}

unsafe extern "C" fn on_history_cell(
    data: *mut c_void,
    codepoints: *const u32,
    len: usize,
    width: u8,
) {
    let content = unsafe { &mut *data.cast::<HistoryContent>() };
    let text = match (width, len) {
        (0, _) => String::new(),
        (_, 0) => " ".to_owned(),
        _ => {
            let codepoints = unsafe { slice::from_raw_parts(codepoints, len) };
            let Some(text) = codepoints.iter().copied().map(char::from_u32).collect() else {
                content.invalid_text = true;
                return;
            };
            text
        }
    };
    content.cells.push(HistoryCell {
        text,
        width,
        has_content: width != 0 && len != 0,
    });
}

fn color_from_shim(color: u32) -> Color {
    match color >> 24 {
        1 => Color::Palette(color as u8),
        2 => Color::Rgb(color & 0x00ff_ffff),
        _ => Color::Default,
    }
}

unsafe extern "C" fn on_cell(
    data: *mut c_void,
    bytes: *const u8,
    len: usize,
    fg: u32,
    bg: u32,
    underline_color: u32,
    flags: u16,
    width: u8,
) {
    let screen = unsafe { &mut *data.cast::<ScreenContent>() };
    let text = match (width, len) {
        (0, _) => String::new(),
        (_, 0) => " ".to_owned(),
        _ => {
            let Ok(text) = str::from_utf8(unsafe { slice::from_raw_parts(bytes, len) }) else {
                screen.invalid_text = true;
                return;
            };
            text.to_owned()
        }
    };
    // See the stable GhosttyStyleFlags bit layout in ghostty/vt/style.h.
    let underline_style = ((flags >> 8) & 7) as u8;
    screen.cells.push(Cell {
        text,
        has_content: width != 0 && len != 0,
        fg: color_from_shim(fg),
        bg: color_from_shim(bg),
        width,
        bold: flags & (1 << 0) != 0,
        italic: flags & (1 << 1) != 0,
        dim: flags & (1 << 2) != 0,
        blink: flags & (1 << 3) != 0,
        inverse: flags & (1 << 4) != 0,
        invisible: flags & (1 << 5) != 0,
        strikethrough: flags & (1 << 6) != 0,
        overline: flags & (1 << 7) != 0,
        underline: underline_style != 0,
        underline_style,
        underline_color: color_from_shim(underline_color),
    });
}
