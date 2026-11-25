use std::ops::Range;

use matou_native_terminal::InputModes;

/// Native positive values scroll upward; reference pointer velocity is downward-positive.
pub(crate) fn selection_scroll_amount(y: f32, height: f32) -> i32 {
    if y >= 0. && y <= height {
        return 0;
    }
    let distance = if y > height { y - height } else { y };
    let ratio = distance.clamp(-50., 50.) / 50.;
    // Math.round uses ties toward positive infinity, including negative values.
    -(ratio.signum() as i32 + (ratio * 14. + 0.5).floor() as i32)
}

/// When the source provides only `public.utf16-plain-text`, the system's
/// synthesized UTF-8 representation starts with U+FEFF. AppKit's standard
/// string reader returns the text without that prefix.
pub(crate) fn read_clipboard_text(cx: &gpui::App) -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        use cocoa::{appkit::NSPasteboard, base::nil};

        if let Some(text) = unsafe { macos_pasteboard_text(NSPasteboard::generalPasteboard(nil)) } {
            return Some(text);
        }
    }

    cx.read_from_clipboard().and_then(|item| item.text())
}

#[cfg(target_os = "macos")]
unsafe fn macos_pasteboard_text(pasteboard: cocoa::base::id) -> Option<String> {
    use cocoa::{
        appkit::{NSPasteboard, NSPasteboardTypeString},
        base::nil,
        foundation::NSString,
    };

    unsafe {
        let text = pasteboard.stringForType(NSPasteboardTypeString);
        if text == nil {
            return None;
        }
        let bytes = text.UTF8String();
        if bytes.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(bytes.cast(), text.len());
        String::from_utf8(bytes.to_vec()).ok()
    }
}

pub(crate) fn read_terminal_paste(cx: &gpui::App, modes: InputModes) -> Option<Vec<u8>> {
    terminal_paste_bytes(read_clipboard_text(cx).as_deref(), modes, || {
        clipboard_has_content(cx)
    })
}

fn terminal_paste_bytes(
    text: Option<&str>,
    modes: InputModes,
    has_content: impl FnOnce() -> bool,
) -> Option<Vec<u8>> {
    let text = match text {
        Some(text) => text,
        // A bracketed paste with no text lets the CLI read the clipboard itself.
        None if modes.bracketed_paste && has_content() => "",
        None => return None,
    };
    let bytes = encode_paste(text, modes);
    (!bytes.is_empty()).then_some(bytes)
}

fn clipboard_has_content(cx: &gpui::App) -> bool {
    #[cfg(target_os = "macos")]
    {
        use cocoa::{appkit::NSPasteboard, base::nil};

        if unsafe { macos_pasteboard_has_items(NSPasteboard::generalPasteboard(nil)) } {
            return true;
        }
    }

    #[cfg(target_os = "windows")]
    {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn CountClipboardFormats() -> i32;
        }

        if unsafe { CountClipboardFormats() } > 0 {
            return true;
        }
    }

    cx.read_from_clipboard().is_some()
}

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)] // objc 0.2 probes its historical cargo-clippy feature.
unsafe fn macos_pasteboard_has_items(pasteboard: cocoa::base::id) -> bool {
    use cocoa::{appkit::NSPasteboard, base::nil};
    use objc::{msg_send, sel, sel_impl};

    if pasteboard == nil {
        return false;
    }
    unsafe {
        let types = pasteboard.types();
        if types == nil {
            return false;
        }
        let count: usize = msg_send![types, count];
        count > 0
    }
}

pub(crate) fn encode_paste(text: &str, modes: InputModes) -> Vec<u8> {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if !modes.bracketed_paste {
        return text.into_bytes();
    }
    let mut bytes = Vec::with_capacity(text.len() + 12);
    bytes.extend_from_slice(b"\x1b[200~");
    bytes.extend_from_slice(text.as_bytes());
    bytes.extend_from_slice(b"\x1b[201~");
    bytes
}

pub(crate) const CLAUDE_SCROLL_TO_BOTTOM: &[u8] = b"\x1b[1;5F";

/// Printable characters are committed by the platform text handler.
pub(crate) fn terminal_key(
    key: &str,
    shift: bool,
    control: bool,
    alt: bool,
    modes: InputModes,
) -> Option<Vec<u8>> {
    let application = modes.application_cursor_keys;
    let special: &[u8] = match key {
        "enter" => b"\r",
        "backspace" => b"\x7f",
        "tab" if shift => b"\x1b[Z",
        "tab" => b"\t",
        "escape" => b"\x1b",
        "up" if application => b"\x1bOA",
        "down" if application => b"\x1bOB",
        "right" if application => b"\x1bOC",
        "left" if application => b"\x1bOD",
        "up" => b"\x1b[A",
        "down" => b"\x1b[B",
        "right" => b"\x1b[C",
        "left" => b"\x1b[D",
        "home" => b"\x1b[H",
        "end" => b"\x1b[F",
        "delete" => b"\x1b[3~",
        "pageup" => b"\x1b[5~",
        "pagedown" => b"\x1b[6~",
        _ => {
            if control
                && !alt
                && let [letter @ b'a'..=b'z'] = key.as_bytes()
            {
                return Some(vec![letter - b'a' + 1]);
            }
            return None;
        }
    };
    Some(special.to_vec())
}

pub(crate) fn byte_from_utf16(text: &str, requested: usize) -> usize {
    let mut units = 0;
    for (byte, ch) in text.char_indices() {
        units += ch.len_utf16();
        if units > requested {
            return byte;
        }
    }
    text.len()
}

pub(crate) fn utf16_from_byte(text: &str, byte: usize) -> usize {
    text[..byte].encode_utf16().count()
}

fn byte_range(text: &str, range: Range<usize>) -> Range<usize> {
    byte_from_utf16(text, range.start)..byte_from_utf16(text, range.end.max(range.start))
}

#[derive(Default)]
pub(crate) struct Composition {
    pub text: String,
    pub selection: Range<usize>,
}

impl Composition {
    pub fn is_active(&self) -> bool {
        !self.text.is_empty()
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.selection = 0..0;
    }

    pub fn text_for_range(&self, range: Range<usize>) -> (String, Range<usize>) {
        let bytes = byte_range(&self.text, range);
        let adjusted =
            utf16_from_byte(&self.text, bytes.start)..utf16_from_byte(&self.text, bytes.end);
        (self.text[bytes].to_owned(), adjusted)
    }

    pub fn replace(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
    ) {
        let replaced = range
            .map(|range| byte_range(&self.text, range))
            .unwrap_or(0..self.text.len());
        let start = utf16_from_byte(&self.text, replaced.start);
        self.text.replace_range(replaced, text);
        let marked_len = text.encode_utf16().count();
        let selected = byte_range(text, selected.unwrap_or(marked_len..marked_len));
        self.selection = start + utf16_from_byte(text, selected.start)
            ..start + utf16_from_byte(text, selected.end);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paste_modes(bracketed_paste: bool) -> InputModes {
        InputModes {
            application_cursor_keys: false,
            bracketed_paste,
            mouse_protocol: matou_native_terminal::MouseProtocol::None,
            mouse_encoding: matou_native_terminal::MouseEncoding::Default,
            input_epoch: 0,
        }
    }

    #[test]
    fn paste_matches_reference_in_both_modes() {
        for (text, unbracketed, bracketed) in [
            ("ABC123", "ABC123", "\x1b[200~ABC123\x1b[201~"),
            (
                "中文粘贴测试",
                "中文粘贴测试",
                "\x1b[200~中文粘贴测试\x1b[201~",
            ),
            ("第1行 END", "第1行 END", "\x1b[200~第1行 END\x1b[201~"),
            ("👩‍💻", "👩‍💻", "\x1b[200~👩‍💻\x1b[201~"),
            (
                "\x1b[31mred\u{009b}0m\x1b[200~text\x1b[201~",
                "\x1b[31mred\u{009b}0m\x1b[200~text\x1b[201~",
                "\x1b[200~\x1b[31mred\u{009b}0m\x1b[200~text\x1b[201~\x1b[201~",
            ),
            ("\x1b", "\x1b", "\x1b[200~\x1b\x1b[201~"),
            ("", "", "\x1b[200~\x1b[201~"),
            (
                "中文粘贴-👩‍💻-END\r\n第二行\nline 3\r",
                "中文粘贴-👩‍💻-END\r第二行\rline 3\r",
                "\x1b[200~中文粘贴-👩‍💻-END\r第二行\rline 3\r\x1b[201~",
            ),
        ] {
            assert_eq!(
                encode_paste(text, paste_modes(false)),
                unbracketed.as_bytes()
            );
            assert_eq!(encode_paste(text, paste_modes(true)), bracketed.as_bytes());
        }
    }

    #[test]
    fn terminal_paste_distinguishes_text_non_text_and_empty_clipboards() {
        for (bracketed, expected_text, expected_empty_text) in [
            (false, "line\rnext", None),
            (
                true,
                "\x1b[200~line\rnext\x1b[201~",
                Some("\x1b[200~\x1b[201~"),
            ),
        ] {
            let modes = paste_modes(bracketed);
            let unexpected_probe = || panic!("text paste should not probe other formats");
            assert_eq!(
                terminal_paste_bytes(Some("line\nnext"), modes, unexpected_probe).as_deref(),
                Some(expected_text.as_bytes())
            );
            assert_eq!(
                terminal_paste_bytes(Some(""), modes, unexpected_probe).as_deref(),
                expected_empty_text.map(str::as_bytes)
            );
        }
        assert_eq!(
            terminal_paste_bytes(None, paste_modes(true), || true).as_deref(),
            Some(b"\x1b[200~\x1b[201~".as_slice())
        );
        assert_eq!(
            terminal_paste_bytes(None, paste_modes(false), || true),
            None
        );
        assert_eq!(
            terminal_paste_bytes(None, paste_modes(true), || false),
            None
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn appkit_image_without_text_triggers_bracketed_paste() {
        use cocoa::{
            appkit::NSPasteboard,
            base::{NO, id, nil},
            foundation::{NSAutoreleasePool, NSData, NSString},
        };

        let (was_empty, wrote_image, text, has_items, paste) = unsafe {
            let pool = NSAutoreleasePool::new(nil);
            let pasteboard = NSPasteboard::pasteboardWithUniqueName(nil);
            assert_ne!(pasteboard, nil);
            let was_empty = !macos_pasteboard_has_items(pasteboard);

            let png_type: id = NSString::alloc(nil).init_str("public.png").autorelease();
            let png = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x04\x00\x00\x00\xb5\x1c\x0c\x02\
                        \x00\x00\x00\x0bIDAT\x78\xda\x63\x64\xf8\x0f\x00\x01\x05\x01\x01\x27\x18\xe3\x66\
                        \x00\x00\x00\x00IEND\xae\x42\x60\x82";
            let data = NSData::dataWithBytes_length_(nil, png.as_ptr().cast(), png.len() as u64);
            pasteboard.clearContents();
            let wrote_image = pasteboard.setData_forType(data, png_type) != NO;
            let text = macos_pasteboard_text(pasteboard);
            let has_items = macos_pasteboard_has_items(pasteboard);
            let paste = terminal_paste_bytes(text.as_deref(), paste_modes(true), || has_items);

            pasteboard.releaseGlobally();
            pool.drain();
            (was_empty, wrote_image, text, has_items, paste)
        };
        assert!(was_empty);
        assert!(wrote_image);
        assert_eq!(text, None);
        assert!(has_items);
        assert_eq!(paste.as_deref(), Some(b"\x1b[200~\x1b[201~".as_slice()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn appkit_reads_unicode_from_utf8_and_utf16_pasteboard_types() {
        use cocoa::{
            appkit::{NSPasteboard, NSPasteboardTypeString},
            base::{NO, id, nil},
            foundation::{NSAutoreleasePool, NSData, NSString},
        };

        let text = "中文粘贴-👩‍💻-END\n第二行";
        let utf16: Vec<u8> = std::iter::once(0xfeff)
            .chain(text.encode_utf16())
            .flat_map(u16::to_le_bytes)
            .collect();

        unsafe {
            let pool = NSAutoreleasePool::new(nil);
            let pasteboard = NSPasteboard::pasteboardWithUniqueName(nil);
            assert_ne!(pasteboard, nil);
            assert_eq!(macos_pasteboard_text(pasteboard), None);

            let utf16_type: id = NSString::alloc(nil)
                .init_str("public.utf16-plain-text")
                .autorelease();
            for (kind, bytes) in [
                (NSPasteboardTypeString, text.as_bytes()),
                (utf16_type, &utf16),
            ] {
                let data =
                    NSData::dataWithBytes_length_(nil, bytes.as_ptr().cast(), bytes.len() as u64);
                pasteboard.clearContents();
                assert_ne!(pasteboard.setData_forType(data, kind), NO);
                assert_eq!(macos_pasteboard_text(pasteboard).as_deref(), Some(text));
            }

            pasteboard.releaseGlobally();
            pool.drain();
        }
    }

    #[test]
    fn edge_velocity_matches_reference_mouse_offsets() {
        // Frozen SelectionService method execution; native direction is reversed.
        for (y, expected) in [
            (-100., 15),
            (-50., 15),
            (-25., 8),
            (-12.5, 4),
            (-1., 1),
            (0., 0),
            (40., 0),
            (80., 0),
            (81., -1),
            (92.5, -5),
            (105., -8),
            (130., -15),
            (180., -15),
        ] {
            assert_eq!(
                super::selection_scroll_amount(y, 80.),
                expected,
                "offset {y}"
            );
        }
    }

    #[test]
    fn ime_uses_utf16_offsets_without_splitting_surrogates() {
        let mut mark = Composition::default();
        mark.replace(None, "中🙂a", Some(1..3));
        assert_eq!(mark.selection, 1..3);
        assert_eq!(mark.text_for_range(2..3), ("🙂".into(), 1..3));
        mark.replace(Some(2..3), "字", Some(1..1));
        assert_eq!(mark.text, "中字a");
        assert_eq!(mark.selection, 2..2);
        mark.clear();
        assert!(!mark.is_active());
        assert_eq!(mark.selection, 0..0);
    }
}
