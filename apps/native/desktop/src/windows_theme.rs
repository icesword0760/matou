use std::ffi::c_void;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};

#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmSetWindowAttribute(
        window: *mut c_void,
        attribute: u32,
        value: *const c_void,
        size: u32,
    ) -> i32;
}

pub fn set_dark(window: &gpui::Window, dark: bool) {
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    let hwnd = handle.hwnd.get() as *mut c_void;
    let enabled = i32::from(dark);
    let value = (&enabled as *const i32).cast();
    let size = size_of_val(&enabled) as u32;
    const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
    const DWMWA_USE_IMMERSIVE_DARK_MODE_LEGACY: u32 = 19;
    // The borrowed handle and BOOL remain valid for both synchronous calls.
    unsafe {
        if DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, value, size) < 0 {
            DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE_LEGACY, value, size);
        }
    }
}
