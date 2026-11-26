use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use base64::{Engine, engine::general_purpose::STANDARD};
use matou_native_core::update_policy::Architecture;

const AMD64: u16 = 0x8664;
const I386: u16 = 0x014c;

pub(super) fn valid_sha512(value: &str) -> bool {
    STANDARD.decode(value).is_ok_and(|bytes| bytes.len() == 64)
}

pub(super) fn installer_name_priority(name: &str, version: &str, arch: Architecture) -> Option<u8> {
    let label = match arch {
        Architecture::X64 => "x64",
        Architecture::Arm64 => "arm64",
    };
    if name.eq_ignore_ascii_case(&format!("Matou-{version}-win-{label}.exe")) {
        return Some(2);
    }
    let name = name.to_ascii_lowercase();
    if arch == Architecture::Arm64 || !name.ends_with(".exe") || name.contains(['/', '\\']) {
        return None;
    }
    if name
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|segment| matches!(segment, "arm64" | "aarch64" | "internal"))
        || name.starts_with("matou-") && name.ends_with("-win-x64.exe")
    {
        return None;
    }
    // Keep the existing x64 JSON and generic channel names (for example Matou.exe).
    Some(1)
}

pub(super) fn verify_installer(path: &Path, arch: Architecture, _version: &str) -> io::Result<()> {
    let machine = executable_machine(&mut File::open(path)?)?;
    if machine != AMD64 && !(machine == I386 && arch == Architecture::X64) {
        return Err(invalid(
            "Windows update installer machine does not match the application",
        ));
    }
    #[cfg(windows)]
    {
        let (name, version) = version_info::read(path).unwrap_or_default();
        verify_product(arch, machine, name.as_deref(), version, _version)?;
    }
    Ok(())
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn executable_machine(file: &mut (impl Read + Seek)) -> io::Result<u16> {
    let bad_image = || invalid("The Windows update is not a valid PE executable");
    let mut dos = [0; 64];
    file.read_exact(&mut dos).map_err(|_| bad_image())?;
    if &dos[..2] != b"MZ" {
        return Err(bad_image());
    }
    let offset = u32::from_le_bytes(dos[60..64].try_into().unwrap()) as u64;
    let length = file.seek(SeekFrom::End(0))?;
    if offset < 64 || offset + 26 > length {
        return Err(bad_image());
    }
    file.seek(SeekFrom::Start(offset))?;
    let mut header = [0; 26];
    file.read_exact(&mut header)?;
    let word = |index| u16::from_le_bytes([header[index], header[index + 1]]);
    let machine = word(4);
    let optional_size = word(20);
    let flags = word(22);
    let magic = word(24);
    if &header[..4] != b"PE\0\0"
        || optional_size < 2
        || offset + 24 + u64::from(optional_size) > length
        || flags & 0x2002 != 0x0002
        || magic != if machine == I386 { 0x10b } else { 0x20b }
    {
        return Err(bad_image());
    }
    Ok(machine)
}

#[cfg(any(windows, test))]
fn verify_product(
    arch: Architecture,
    machine: u16,
    name: Option<&str>,
    version: Option<[u16; 4]>,
    expected_version: &str,
) -> io::Result<()> {
    let marker = name.map(str::trim);
    let marked = matches!(marker, Some("Matou ARM64" | "Matou x64"));
    let expected_name = match arch {
        Architecture::Arm64 => "Matou ARM64",
        Architecture::X64 => "Matou x64",
    };
    if arch == Architecture::Arm64 || marked {
        if marker != Some(expected_name) || machine != AMD64 {
            return Err(invalid(
                "Windows update installer payload architecture does not match the application",
            ));
        }
        let mut parts = expected_version.split('.');
        let expected = [parts.next(), parts.next(), parts.next()]
            .map(|part| part.and_then(|part| part.parse::<u16>().ok()));
        if parts.next().is_some()
            || !matches!((expected, version), ([Some(a), Some(b), Some(c)], Some(actual)) if actual == [a, b, c, 0])
        {
            return Err(invalid(
                "Windows update installer version does not match the manifest",
            ));
        }
    }
    Ok(())
}

#[cfg(windows)]
mod version_info {
    use std::ffi::{OsStr, c_void};
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::ptr::null_mut;

    #[link(name = "version")]
    unsafe extern "system" {
        fn GetFileVersionInfoSizeW(file: *const u16, handle: *mut u32) -> u32;
        fn GetFileVersionInfoW(file: *const u16, handle: u32, len: u32, data: *mut c_void) -> i32;
        fn VerQueryValueW(
            data: *const c_void,
            name: *const u16,
            value: *mut *mut c_void,
            len: *mut u32,
        ) -> i32;
    }

    fn wide(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain(std::iter::once(0)).collect()
    }

    fn query<'a>(data: &'a [u32], name: &str, unit: usize) -> Option<&'a [u8]> {
        let mut ptr = null_mut();
        let mut length = 0;
        let name = wide(OsStr::new(name));
        // The version resource is owned by \`data\`; the returned pointer must stay inside it.
        if unsafe { VerQueryValueW(data.as_ptr().cast(), name.as_ptr(), &mut ptr, &mut length) }
            == 0
        {
            return None;
        }
        let offset = (ptr as usize).checked_sub(data.as_ptr() as usize)?;
        let length = (length as usize).checked_mul(unit)?;
        if offset.checked_add(length)? > std::mem::size_of_val(data) {
            return None;
        }
        Some(unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), length) })
    }

    pub(super) fn read(path: &Path) -> Option<(Option<String>, Option<[u16; 4]>)> {
        let path = wide(path.as_os_str());
        let mut handle = 0;
        // Both Windows APIs receive a NUL-terminated path and an owned output buffer.
        let length = unsafe { GetFileVersionInfoSizeW(path.as_ptr(), &mut handle) };
        if length == 0 {
            return None;
        }
        let mut data = vec![0u32; (length as usize).div_ceil(4)];
        if unsafe { GetFileVersionInfoW(path.as_ptr(), 0, length, data.as_mut_ptr().cast()) } == 0 {
            return None;
        }
        let version = query(&data, "\\", 1).and_then(|record| {
            let field = |offset| {
                record
                    .get(offset..offset + 4)
                    .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
            };
            if field(0)? != 0xfeef04bd {
                return None;
            }
            let ms = field(16)?;
            let ls = field(20)?;
            Some([(ms >> 16) as u16, ms as u16, (ls >> 16) as u16, ls as u16])
        });
        let name = query(&data, "\\VarFileInfo\\Translation", 1)?
            .chunks_exact(4)
            .find_map(|item| {
                let language = u16::from_le_bytes([item[0], item[1]]);
                let code_page = u16::from_le_bytes([item[2], item[3]]);
                let key = format!("\\StringFileInfo\\{language:04x}{code_page:04x}\\ProductName");
                let text = query(&data, &key, 2)?;
                let utf16 = text
                    .chunks_exact(2)
                    .map(|word| u16::from_le_bytes([word[0], word[1]]))
                    .take_while(|&word| word != 0)
                    .collect::<Vec<_>>();
                String::from_utf16(&utf16).ok()
            });
        Some((name, version))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_official_installer_names_and_preserves_legacy_x64_names() {
        assert_eq!(
            installer_name_priority("Matou-0.3.0-win-arm64.exe", "0.3.0", Architecture::Arm64),
            Some(2)
        );
        assert_eq!(
            installer_name_priority("Matou-0.3.0-win-arm64.exe", "0.3.0", Architecture::X64),
            None
        );
        assert_eq!(
            installer_name_priority("Matou-0.3.0-win-x64.exe", "0.4.0", Architecture::X64),
            None
        );
        assert_eq!(
            installer_name_priority("Matou-Setup.exe", "0.3.0", Architecture::X64),
            Some(1)
        );
        assert_eq!(
            installer_name_priority("Matou-Setup.exe", "0.3.0", Architecture::Arm64),
            None
        );
        assert_eq!(
            installer_name_priority(
                "Matou-0.3.0-win-x64-internal.exe",
                "0.3.0",
                Architecture::X64
            ),
            None
        );
    }

    #[test]
    fn checks_arm64_payload_marker_and_version_on_the_shared_x64_loader() {
        let version = Some([0, 3, 0, 0]);
        assert!(
            verify_product(
                Architecture::Arm64,
                AMD64,
                Some("Matou ARM64"),
                version,
                "0.3.0"
            )
            .is_ok()
        );
        assert!(
            verify_product(
                Architecture::Arm64,
                AMD64,
                Some("Matou x64"),
                version,
                "0.3.0"
            )
            .is_err()
        );
        assert!(
            verify_product(
                Architecture::Arm64,
                AMD64,
                Some("Matou ARM64"),
                version,
                "0.4.0"
            )
            .is_err()
        );
        assert!(
            verify_product(
                Architecture::X64,
                AMD64,
                Some("Matou ARM64"),
                version,
                "0.3.0"
            )
            .is_err()
        );
        assert!(verify_product(Architecture::X64, I386, None, None, "0.3.0").is_ok());
    }
}
