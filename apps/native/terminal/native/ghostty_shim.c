#include <ghostty/vt.h>

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef void (*MatouWrite)(void *, const uint8_t *, size_t);
typedef void (*MatouHold)(void *, bool);
typedef void (*MatouRow)(void *, bool, bool);
typedef void (*MatouCell)(void *, const uint8_t *, size_t,
                          uint32_t, uint32_t, uint32_t, uint16_t, uint8_t);
typedef void (*MatouHistoryCell)(void *, const uint32_t *, size_t, uint8_t);

typedef struct {
    size_t history_lines;
    uint64_t scroll_offset, scroll_total, scroll_len;
    uint32_t foreground, background, cursor_color, palette[256];
    uint16_t cols, rows, cursor_col, cursor_row;
    uint8_t cursor_has_position, cursor_visible, cursor_style, cursor_blinking;
    uint8_t cursor_has_color, alternate;
} MatouScreen;

typedef struct {
    GhosttyTerminal terminal;
    GhosttyRenderState render;
    GhosttyRenderStateRowIterator rows;
    GhosttyRenderStateRowCells cells;
    GhosttySizeReportSize size;
    void *userdata;
    MatouWrite write;
    MatouHold hold;
    MatouWrite clipboard;
} MatouVt;

#define TRY(operation) do {                                     \
    GhosttyResult status = (operation);                         \
    if (status != GHOSTTY_SUCCESS) return status;               \
} while (0)

static void write_pty(GhosttyTerminal terminal, void *userdata,
                      const uint8_t *data, size_t len) {
    MatouVt *vt = userdata;
    const uint8_t status_prefix[] = "\x1bP1$r";
    const uint8_t status_suffix[] = "m\x1b\\";
    const uint8_t sgr_reply[] = "\x1bP1$r0m\x1b\\";
    const size_t prefix_len = sizeof(status_prefix) - 1;
    const size_t suffix_len = sizeof(status_suffix) - 1;
    if (len > prefix_len + suffix_len &&
        memcmp(data, status_prefix, prefix_len) == 0 &&
        memcmp(data + len - suffix_len, status_suffix, suffix_len) == 0) {
        // The frozen DECRQSS SGR reply is independent of the active style.
        vt->write(vt->userdata, sgr_reply, sizeof(sgr_reply) - 1);
        return;
    }

    // Standard CPR uses the active screen's absolute row, even under DECOM.
    // Leave the reported column and the distinct private CPR format intact.
    if (len >= 6 && len <= 14 && data[0] == 0x1b && data[1] == '[' && data[len - 1] == 'R') {
        size_t sep = 2;
        while (sep < len - 1 && data[sep] >= '0' && data[sep] <= '9') sep++;
        size_t end = sep + 1;
        while (end < len - 1 && data[end] >= '0' && data[end] <= '9') end++;
        uint16_t row;
        if (sep > 2 && sep < len - 2 && data[sep] == ';' && end == len - 1 &&
            ghostty_terminal_get(terminal, GHOSTTY_TERMINAL_DATA_CURSOR_Y, &row) == GHOSTTY_SUCCESS) {
            char reply[16];
            int size = snprintf(reply, sizeof(reply), "\x1b[%u%.*s", row + 1u,
                                (int)(len - sep), (const char *)data + sep);
            if (size > 0 && (size_t)size < sizeof(reply)) {
                vt->write(vt->userdata, (const uint8_t *)reply, (size_t)size);
                return;
            }
        }
    }
    vt->write(vt->userdata, data, len);
}

static void render_hold(GhosttyTerminal terminal, void *userdata, bool held) {
    (void)terminal;
    MatouVt *vt = userdata;
    vt->hold(vt->userdata, held);
}

static void unknown_sequence(GhosttyTerminal terminal, void *userdata,
                             const GhosttyTerminalUnknownSequence *sequence) {
    (void)terminal;
    if (sequence->tag != GHOSTTY_TERMINAL_UNKNOWN_SEQUENCE_OSC || sequence->value.osc.truncated) return;
    GhosttyString content = sequence->value.osc.content;
    size_t index = 0;
    unsigned command = 0;
    while (index < content.len && content.ptr[index] >= '0' && content.ptr[index] <= '9') {
        command = command * 10 + content.ptr[index++] - '0';
        if (command > 52) return;
    }
    if (index == 0 || command != 52 || index == content.len || content.ptr[index] != ';') return;
    MatouVt *vt = userdata;
    vt->clipboard(vt->userdata, content.ptr + index + 1, content.len - index - 1);
}

static bool report_size(GhosttyTerminal terminal, void *userdata, GhosttySizeReportSize *size) {
    (void)terminal;
    MatouVt *vt = userdata;
    *size = vt->size;
    return true;
}

void matou_vt_free(MatouVt *vt) {
    if (vt == NULL) return;
    ghostty_render_state_row_cells_free(vt->cells);
    ghostty_render_state_row_iterator_free(vt->rows);
    ghostty_render_state_free(vt->render);
    ghostty_terminal_free(vt->terminal);
    free(vt);
}

static GhosttyColorRgb to_rgb(uint32_t color) {
    return (GhosttyColorRgb){ .r = color >> 16, .g = color >> 8, .b = color };
}

static uint32_t from_rgb(GhosttyColorRgb color) {
    return ((uint32_t)color.r << 16) | ((uint32_t)color.g << 8) | color.b;
}

int matou_vt_set_theme(MatouVt *vt, uint32_t foreground, uint32_t background,
                       uint32_t cursor, const uint32_t *palette) {
    GhosttyColorRgb fg = to_rgb(foreground);
    GhosttyColorRgb bg = to_rgb(background);
    GhosttyColorRgb cursor_color = to_rgb(cursor);
    GhosttyColorRgb colors[256];
    for (size_t index = 0; index < 256; index++) colors[index] = to_rgb(palette[index]);

    TRY(ghostty_terminal_set(vt->terminal, GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND, &fg));
    TRY(ghostty_terminal_set(vt->terminal, GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND, &bg));
    TRY(ghostty_terminal_set(vt->terminal, GHOSTTY_TERMINAL_OPT_COLOR_CURSOR, &cursor_color));
    TRY(ghostty_terminal_set(vt->terminal, GHOSTTY_TERMINAL_OPT_COLOR_PALETTE, colors));
    return GHOSTTY_SUCCESS;
}

int matou_vt_resize(MatouVt *vt, uint16_t cols, uint16_t rows,
                    uint32_t cell_width_px, uint32_t cell_height_px) {
    TRY(ghostty_terminal_resize(vt->terminal, cols, rows, cell_width_px, cell_height_px));
    vt->size = (GhosttySizeReportSize){
        .rows = rows, .columns = cols,
        .cell_width = cell_width_px, .cell_height = cell_height_px,
    };
    return GHOSTTY_SUCCESS;
}

static GhosttyResult initialize(MatouVt *vt, uint16_t cols, uint16_t rows,
                                 uint32_t cell_width_px, uint32_t cell_height_px,
                                 size_t history_lines, bool pull_scrollback) {
    TRY(ghostty_terminal_new(NULL, &vt->terminal, cols, rows));
    TRY(ghostty_terminal_set(vt->terminal, GHOSTTY_TERMINAL_OPT_USERDATA, vt));
    TRY(ghostty_terminal_set(vt->terminal, GHOSTTY_TERMINAL_OPT_WRITE_PTY, write_pty));
    TRY(ghostty_terminal_set(vt->terminal, GHOSTTY_TERMINAL_OPT_RENDER_HOLD, render_hold));
    TRY(ghostty_terminal_set(vt->terminal, GHOSTTY_TERMINAL_OPT_SIZE, report_size));
    // The frozen OSC handler caps content at ten million UTF-16 units.
    // This byte ceiling allows any valid UTF-8 representation; Rust enforces the unit limit.
    size_t osc_bytes = 40000003;
    TRY(ghostty_terminal_set(vt->terminal, GHOSTTY_TERMINAL_OPT_UNKNOWN_MAX_BYTES, &osc_bytes));
    TRY(ghostty_terminal_set(vt->terminal, GHOSTTY_TERMINAL_OPT_UNKNOWN_SEQUENCE, unknown_sequence));

    size_t unlimited_bytes = SIZE_MAX;
    TRY(ghostty_terminal_set(vt->terminal, GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_BYTES, &unlimited_bytes));
    TRY(ghostty_terminal_set(vt->terminal, GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES, &history_lines));
    TRY(ghostty_terminal_set(vt->terminal, GHOSTTY_TERMINAL_OPT_RESIZE_PULL_SCROLLBACK, &pull_scrollback));

    GhosttyCursorStyle bar = GHOSTTY_CURSOR_STYLE_BAR;
    bool blink = true;
    TRY(ghostty_terminal_set(vt->terminal, GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_STYLE, &bar));
    TRY(ghostty_terminal_set(vt->terminal, GHOSTTY_TERMINAL_OPT_DEFAULT_CURSOR_BLINK, &blink));
    const uint8_t initial_cursor[] = "\x1b[5 q";
    ghostty_terminal_vt_write(vt->terminal, initial_cursor, sizeof(initial_cursor) - 1);

    TRY(matou_vt_resize(vt, cols, rows, cell_width_px, cell_height_px));
    TRY(ghostty_render_state_new(NULL, &vt->render));
    TRY(ghostty_render_state_row_iterator_new(NULL, &vt->rows));
    return ghostty_render_state_row_cells_new(NULL, &vt->cells);
}

int matou_vt_new(MatouVt **out, uint16_t cols, uint16_t rows,
                 uint32_t cell_width_px, uint32_t cell_height_px,
                 size_t history_lines, bool pull_scrollback,
                 void *userdata, MatouWrite write, MatouHold hold, MatouWrite clipboard) {
    MatouVt *vt = calloc(1, sizeof(*vt));
    if (vt == NULL) return GHOSTTY_OUT_OF_MEMORY;
    vt->userdata = userdata;
    vt->write = write;
    vt->hold = hold;
    vt->clipboard = clipboard;

    GhosttyResult status = initialize(vt, cols, rows, cell_width_px, cell_height_px,
                                       history_lines, pull_scrollback);
    if (status != GHOSTTY_SUCCESS) {
        matou_vt_free(vt);
        return status;
    }
    *out = vt;
    return GHOSTTY_SUCCESS;
}

void matou_vt_feed(MatouVt *vt, const uint8_t *data, size_t len) {
    ghostty_terminal_vt_write(vt->terminal, data, len);
}

int matou_vt_snapshot_ready(MatouVt *vt, bool *ready) {
    GhosttyTerminalScreen active;
    bool ground;
    TRY(ghostty_terminal_get(vt->terminal, GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN, &active));
    TRY(ghostty_terminal_get(vt->terminal, GHOSTTY_TERMINAL_DATA_VT_GROUND, &ground));
    *ready = active == GHOSTTY_TERMINAL_SCREEN_PRIMARY && ground;
    return GHOSTTY_SUCCESS;
}

static GhosttyResult snapshot_cursor(MatouVt *vt, char suffix[32], size_t *len) {
    GhosttyRenderStateCursor cursor = GHOSTTY_INIT_SIZED(GhosttyRenderStateCursor);
    TRY(ghostty_render_state_update(vt->render, vt->terminal));
    TRY(ghostty_render_state_get(vt->render, GHOSTTY_RENDER_STATE_DATA_CURSOR, &cursor));

    int style;
    switch (cursor.visual_style) {
        case GHOSTTY_CURSOR_STYLE_BAR: style = 5; break;
        case GHOSTTY_CURSOR_STYLE_UNDERLINE: style = 3; break;
        default: style = 1; break;
    }
    if (!cursor.blinking) style++;
    static const char decscusr[] = "\x1b[1 q";
    memcpy(suffix, decscusr, sizeof(decscusr) - 1);
    suffix[2] = (char)('0' + style);
    *len = sizeof(decscusr) - 1;

    GhosttyColorRgb current, default_color;
    GhosttyResult status = ghostty_terminal_get(vt->terminal, GHOSTTY_TERMINAL_DATA_COLOR_CURSOR, &current);
    if (status == GHOSTTY_NO_VALUE) return GHOSTTY_SUCCESS;
    if (status != GHOSTTY_SUCCESS) return status;
    status = ghostty_terminal_get(vt->terminal, GHOSTTY_TERMINAL_DATA_COLOR_CURSOR_DEFAULT, &default_color);
    if (status != GHOSTTY_SUCCESS && status != GHOSTTY_NO_VALUE) return status;
    if (status == GHOSTTY_SUCCESS && current.r == default_color.r &&
        current.g == default_color.g && current.b == default_color.b) return GHOSTTY_SUCCESS;
    int written = snprintf(suffix + *len, 32 - *len, "\x1b]12;#%02x%02x%02x\x1b\\",
                           (unsigned)current.r, (unsigned)current.g, (unsigned)current.b);
    if (written < 0 || (size_t)written >= 32 - *len) return GHOSTTY_OUT_OF_SPACE;
    *len += (size_t)written;
    return GHOSTTY_SUCCESS;
}

int matou_vt_format_vt(MatouVt *vt, uint8_t **out, size_t *len, size_t limit) {
    char suffix[32];
    size_t suffix_len;
    TRY(snapshot_cursor(vt, suffix, &suffix_len));
    GhosttyFormatter formatter;
    TRY(ghostty_formatter_terminal_new(NULL, &formatter, vt->terminal));
    const GhosttyFormatterFormat emit = GHOSTTY_FORMATTER_FORMAT_VT;
    const GhosttyFormatterExtras extras = GHOSTTY_FORMATTER_EXTRAS_ALL;
    const bool trim = false;
    const bool unwrap = true;
    GhosttyResult status = ghostty_formatter_set(formatter, GHOSTTY_FORMATTER_OPT_EMIT, &emit);
    if (status == GHOSTTY_SUCCESS)
        status = ghostty_formatter_set(formatter, GHOSTTY_FORMATTER_OPT_EXTRAS, &extras);
    if (status == GHOSTTY_SUCCESS)
        status = ghostty_formatter_set(formatter, GHOSTTY_FORMATTER_OPT_TRIM, &trim);
    if (status == GHOSTTY_SUCCESS)
        status = ghostty_formatter_set(formatter, GHOSTTY_FORMATTER_OPT_UNWRAP, &unwrap);
    if (status == GHOSTTY_SUCCESS) {
        size_t needed = 0;
        status = ghostty_formatter_format_buf(formatter, NULL, 0, &needed);
        if (status == GHOSTTY_SUCCESS || status == GHOSTTY_OUT_OF_SPACE) {
            if (suffix_len > limit || needed > limit - suffix_len) {
                status = GHOSTTY_LIMIT_EXCEEDED;
            } else {
                uint8_t *bytes = malloc(needed + suffix_len);
                if (bytes == NULL) {
                    status = GHOSTTY_OUT_OF_MEMORY;
                } else {
                    status = ghostty_formatter_format_buf(formatter, bytes, needed, len);
                    if (status == GHOSTTY_SUCCESS) {
                        memcpy(bytes + *len, suffix, suffix_len);
                        *len += suffix_len;
                        *out = bytes;
                    } else {
                        free(bytes);
                    }
                }
            }
        }
    }
    ghostty_formatter_free(formatter);
    return status;
}

void matou_vt_free_formatted(uint8_t *bytes) {
    free(bytes);
}

int matou_vt_input_modes(MatouVt *vt, bool *cursor_keys, bool *bracketed_paste, size_t *mouse_modes, size_t *input_epoch) {
    GhosttyTerminalModeConfig cursor = { .mode = GHOSTTY_MODE_CURSOR_KEYS };
    GhosttyTerminalModeConfig paste = { .mode = GHOSTTY_MODE_BRACKETED_PASTE };
    TRY(ghostty_terminal_get(vt->terminal, GHOSTTY_TERMINAL_DATA_MODE, &cursor));
    TRY(ghostty_terminal_get(vt->terminal, GHOSTTY_TERMINAL_DATA_MODE, &paste));
    TRY(ghostty_terminal_get(vt->terminal, (GhosttyTerminalData)0x4d540003, mouse_modes));
    TRY(ghostty_terminal_get(vt->terminal, (GhosttyTerminalData)0x4d540004, input_epoch));
    *cursor_keys = cursor.value;
    *bracketed_paste = paste.value;
    return GHOSTTY_SUCCESS;
}

int matou_vt_scroll(MatouVt *vt, int32_t lines) {
    GhosttyTerminalScrollViewport scroll = {
        .tag = GHOSTTY_TERMINAL_SCROLL_VIEWPORT_DELTA,
        .value.delta = -(intptr_t)lines,
    };
    return ghostty_terminal_scroll_viewport(vt->terminal, scroll);
}

int matou_vt_expire_hold(MatouVt *vt) {
    // Update the mode directly so expiry cannot split a pending escape sequence.
    GhosttyTerminalModeConfig mode = { .mode = GHOSTTY_MODE_SYNCHRONIZED_OUTPUT, .value = false };
    return ghostty_terminal_set(vt->terminal, GHOSTTY_TERMINAL_OPT_MODE, &mode);
}

enum {
    MATOU_PALETTE = 1u << 24,
    MATOU_RGB = 2u << 24,
};

static uint32_t style_color(GhosttyStyleColor color) {
    switch (color.tag) {
        case GHOSTTY_STYLE_COLOR_PALETTE: return MATOU_PALETTE | color.value.palette;
        case GHOSTTY_STYLE_COLOR_RGB: return MATOU_RGB | from_rgb(color.value.rgb);
        default: return 0;
    }
}

static uint8_t cell_width(GhosttyCellWide wide) {
    return wide == GHOSTTY_CELL_WIDE_WIDE ? 2
        : wide == GHOSTTY_CELL_WIDE_SPACER_TAIL ? 0 : 1;
}

static GhosttyResult emit_cell(GhosttyRenderStateRowCells cells, MatouCell emit, void *userdata) {
    GhosttyCell cell;
    GhosttyCellWide wide;
    GhosttyCellContentTag content;
    GhosttyStyle style;
    TRY(ghostty_render_state_row_cells_get(cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW, &cell));
    TRY(ghostty_cell_get(cell, GHOSTTY_CELL_DATA_WIDE, &wide));
    TRY(ghostty_cell_get(cell, GHOSTTY_CELL_DATA_CONTENT_TAG, &content));
    TRY(ghostty_render_state_row_cells_get(cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE, &style));

    const uint32_t fg = style_color(style.fg_color);
    uint32_t bg = style_color(style.bg_color);
    const uint32_t underline_color = style_color(style.underline_color);
    if (content == GHOSTTY_CELL_CONTENT_BG_COLOR_PALETTE) {
        GhosttyColorPaletteIndex index;
        TRY(ghostty_cell_get(cell, GHOSTTY_CELL_DATA_COLOR_PALETTE, &index));
        bg = MATOU_PALETTE | index;
    } else if (content == GHOSTTY_CELL_CONTENT_BG_COLOR_RGB) {
        GhosttyColorRgb color;
        TRY(ghostty_cell_get(cell, GHOSTTY_CELL_DATA_COLOR_RGB, &color));
        bg = MATOU_RGB | from_rgb(color);
    }
    const uint8_t width = cell_width(wide);

    uint8_t storage[128];
    GhosttyBuffer text = { .ptr = storage, .cap = sizeof(storage) };
    GhosttyResult status = GHOSTTY_SUCCESS;
    if (width != 0 && (content == GHOSTTY_CELL_CONTENT_CODEPOINT
                       || content == GHOSTTY_CELL_CONTENT_CODEPOINT_GRAPHEME)) {
        status = ghostty_render_state_row_cells_get(
            cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8, &text);
        if (status == GHOSTTY_OUT_OF_SPACE) {
            text.ptr = malloc(text.len);
            if (text.ptr == NULL) return GHOSTTY_OUT_OF_MEMORY;
            text.cap = text.len;
            status = ghostty_render_state_row_cells_get(
                cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_GRAPHEMES_UTF8, &text);
        }
    }
    if (status == GHOSTTY_SUCCESS) emit(
        userdata, text.ptr, text.len, fg, bg, underline_color, style.flags, width);
    if (text.ptr != storage) free(text.ptr);
    return status;
}

static GhosttyResult emit_history_cell(const GhosttyGridRef *ref, MatouHistoryCell emit,
                                       void *userdata) {
    GhosttyCell cell;
    GhosttyCellWide wide;
    bool has_text;
    TRY(ghostty_grid_ref_cell(ref, &cell));
    TRY(ghostty_cell_get(cell, GHOSTTY_CELL_DATA_WIDE, &wide));
    TRY(ghostty_cell_get(cell, GHOSTTY_CELL_DATA_HAS_TEXT, &has_text));
    const uint8_t width = cell_width(wide);

    uint32_t storage[8];
    uint32_t *text = storage;
    size_t len = 0;
    GhosttyResult status = GHOSTTY_SUCCESS;
    if (width != 0 && has_text) {
        status = ghostty_grid_ref_graphemes(ref, text, sizeof(storage) / sizeof(storage[0]), &len);
        if (status == GHOSTTY_OUT_OF_SPACE) {
            if (len > SIZE_MAX / sizeof(*text)) return GHOSTTY_OUT_OF_MEMORY;
            text = malloc(len * sizeof(*text));
            if (text == NULL) return GHOSTTY_OUT_OF_MEMORY;
            status = ghostty_grid_ref_graphemes(ref, text, len, &len);
        }
    }
    if (status == GHOSTTY_SUCCESS) emit(userdata, text, len, width);
    if (text != storage) free(text);
    return status;
}

int matou_vt_history_row(const MatouVt *vt, size_t row, bool *wrap, bool *continuation,
                          MatouHistoryCell emit, void *userdata) {
    if (row > UINT32_MAX) return GHOSTTY_INVALID_VALUE;
    uint16_t cols;
    TRY(ghostty_terminal_get(vt->terminal, GHOSTTY_TERMINAL_DATA_COLS, &cols));
    GhosttyPoint point = {
        .tag = GHOSTTY_POINT_TAG_SCREEN,
        .value.coordinate = { .x = 0, .y = (uint32_t)row },
    };
    GhosttyGridRef ref = {0};
    GhosttyRow content;
    TRY(ghostty_terminal_grid_ref(vt->terminal, point, &ref));
    TRY(ghostty_grid_ref_row(&ref, &content));
    TRY(ghostty_row_get(content, GHOSTTY_ROW_DATA_WRAP, wrap));
    TRY(ghostty_row_get(content, GHOSTTY_ROW_DATA_WRAP_CONTINUATION, continuation));
    TRY(emit_history_cell(&ref, emit, userdata));
    for (uint16_t col = 1; col < cols; col++) {
        point.value.coordinate.x = col;
        TRY(ghostty_terminal_grid_ref(vt->terminal, point, &ref));
        TRY(emit_history_cell(&ref, emit, userdata));
    }
    return GHOSTTY_SUCCESS;
}

int matou_vt_selection_epoch(const MatouVt *vt, size_t *out) {
    return ghostty_terminal_get(vt->terminal, (GhosttyTerminalData)0x4d540002, out);
}

int matou_vt_history_trimmed_rows(const MatouVt *vt, size_t *out) {
    // Matou's private TerminalData key in the pinned build.
    const GhosttyTerminalData history_trimmed = (GhosttyTerminalData)0x4d540001;
    return ghostty_terminal_get(vt->terminal, history_trimmed, out);
}

int matou_vt_scrollbar(const MatouVt *vt, uint64_t *offset, uint64_t *total, uint64_t *len) {
    GhosttyTerminalScrollbar scrollbar;
    TRY(ghostty_terminal_get(vt->terminal, GHOSTTY_TERMINAL_DATA_SCROLLBAR, &scrollbar));
    *offset = scrollbar.offset;
    *total = scrollbar.total;
    *len = scrollbar.len;
    return GHOSTTY_SUCCESS;
}

int matou_vt_screen(MatouVt *vt, MatouScreen *screen, MatouRow emit_row,
                     MatouCell emit, void *userdata) {
    GhosttyRenderStateCursor cursor = GHOSTTY_INIT_SIZED(GhosttyRenderStateCursor);
    GhosttyRenderStateColors colors = GHOSTTY_INIT_SIZED(GhosttyRenderStateColors);
    GhosttyTerminalScreen active;
    GhosttyTerminalScrollbar scrollbar;

    TRY(ghostty_render_state_update(vt->render, vt->terminal));
    TRY(ghostty_render_state_get(vt->render, GHOSTTY_RENDER_STATE_DATA_COLS, &screen->cols));
    TRY(ghostty_render_state_get(vt->render, GHOSTTY_RENDER_STATE_DATA_ROWS, &screen->rows));
    TRY(ghostty_render_state_get(vt->render, GHOSTTY_RENDER_STATE_DATA_CURSOR, &cursor));
    TRY(ghostty_render_state_get(vt->render, GHOSTTY_RENDER_STATE_DATA_COLORS, &colors));
    TRY(ghostty_terminal_get(vt->terminal, GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN, &active));
    TRY(ghostty_terminal_get(vt->terminal, GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS, &screen->history_lines));
    TRY(ghostty_terminal_get(vt->terminal, GHOSTTY_TERMINAL_DATA_SCROLLBAR, &scrollbar));

    screen->cursor_has_position = cursor.viewport_has_value;
    screen->cursor_visible = cursor.viewport_has_value && cursor.visible;
    if (cursor.viewport_has_value) {
        screen->cursor_col = cursor.viewport_x;
        screen->cursor_row = cursor.viewport_y;
    }
    screen->cursor_style = cursor.visual_style;
    screen->cursor_blinking = cursor.blinking;
    screen->cursor_has_color = colors.cursor_has_value;
    if (colors.cursor_has_value) screen->cursor_color = from_rgb(colors.cursor);
    screen->foreground = from_rgb(colors.foreground);
    screen->background = from_rgb(colors.background);
    for (size_t index = 0; index < 256; index++) screen->palette[index] = from_rgb(colors.palette[index]);

    screen->alternate = active == GHOSTTY_TERMINAL_SCREEN_ALTERNATE;
    screen->scroll_offset = scrollbar.offset;
    screen->scroll_total = scrollbar.total;
    screen->scroll_len = scrollbar.len;

    TRY(ghostty_render_state_get(vt->render, GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR, &vt->rows));
    while (ghostty_render_state_row_iterator_next(vt->rows)) {
        GhosttyRow row;
        bool wrap, continuation;
        TRY(ghostty_render_state_row_get(vt->rows, GHOSTTY_RENDER_STATE_ROW_DATA_RAW, &row));
        TRY(ghostty_row_get(row, GHOSTTY_ROW_DATA_WRAP, &wrap));
        TRY(ghostty_row_get(row, GHOSTTY_ROW_DATA_WRAP_CONTINUATION, &continuation));
        emit_row(userdata, wrap, continuation);
        TRY(ghostty_render_state_row_get(vt->rows, GHOSTTY_RENDER_STATE_ROW_DATA_CELLS, &vt->cells));
        while (ghostty_render_state_row_cells_next(vt->cells)) {
            TRY(emit_cell(vt->cells, emit, userdata));
        }
    }
    return GHOSTTY_SUCCESS;
}
