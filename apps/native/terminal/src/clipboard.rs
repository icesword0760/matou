use base64::{
    Engine, alphabet,
    engine::general_purpose::{GeneralPurpose, NO_PAD},
};

const MAX_OSC_UNITS: usize = 10_000_000;
const MAX_PAYLOAD_UNITS: usize = 4 * 1024 * 1024;
const BASE64: GeneralPurpose = GeneralPurpose::new(
    &alphabet::STANDARD,
    NO_PAD.with_decode_allow_trailing_bits(true),
);

/// Decode the text of a clipboard-write OSC 52 request.
pub fn decode_clipboard(content: &str) -> Option<String> {
    if content.encode_utf16().count() > MAX_OSC_UNITS {
        return None;
    }
    let (_, payload) = content.split_once(';')?;
    if payload.is_empty() || payload == "?" || payload.encode_utf16().count() > MAX_PAYLOAD_UNITS {
        return None;
    }

    let data: Vec<_> = payload
        .bytes()
        .filter(|&byte| !matches!(byte, b' ' | b'\t' | b'\n' | b'\x0c' | b'\r'))
        .collect();
    // Browser Base64 decoding permits missing padding but accepts supplied
    // padding only when the whitespace-free input has a multiple of four bytes.
    let data = if data.len() % 4 == 0 {
        data.strip_suffix(b"==")
            .or_else(|| data.strip_suffix(b"="))
            .unwrap_or(&data)
    } else {
        &data
    };
    let mut text = String::from_utf8(BASE64.decode(data).ok()?).ok()?;
    const BOM: char = '\u{feff}';
    if text.starts_with(BOM) {
        text.drain(..BOM.len_utf8());
    }
    Some(text)
}
