//! Match the frozen macOS window's native sidebar material.
//!
//! GPUI's generic blur intentionally strips AppKit's tint and uses Selection.
//! The existing product uses the system Sidebar material with active effects.
#![allow(unexpected_cfgs)] // objc 0.2 macros probe their historical cargo-clippy feature.
use anyhow::{Result, bail};
use cocoa::{
    appkit::{
        NSView, NSViewHeightSizable, NSViewWidthSizable, NSVisualEffectBlendingMode,
        NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindow,
        NSWindowOrderingMode,
    },
    base::{id, nil},
    foundation::NSString,
};
use gpui::{Rgba, Window, rgba};
use objc::{class, msg_send, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// Hide this window without disposing its Workspace or hiding other app windows.
pub fn hide_window(window: &Window) -> Result<()> {
    let RawWindowHandle::AppKit(handle) = HasWindowHandle::window_handle(window)
        .map_err(|error| anyhow::anyhow!("Read AppKit window handle: {error}"))?
        .as_raw()
    else {
        bail!("Expected an AppKit window");
    };
    // GPUI invokes the close callback on the AppKit main thread, with the live
    // NSView retained by the existing platform window.
    unsafe {
        let native_view = handle.ns_view.as_ptr() as id;
        let native_window: id = msg_send![native_view, window];
        if native_window == nil {
            bail!("AppKit view has no window to hide");
        }
        let _: () = msg_send![native_window, orderOut:nil];
    }
    Ok(())
}

pub fn apply_sidebar_material(window: &Window) -> Result<()> {
    let RawWindowHandle::AppKit(handle) = HasWindowHandle::window_handle(window)
        .map_err(|error| anyhow::anyhow!("Read AppKit window handle: {error}"))?
        .as_raw()
    else {
        bail!("Expected an AppKit window");
    };
    // Called on the GPUI/AppKit main thread while the window is alive. AppKit
    // retains the added subview; the local allocation is then autoreleased.
    unsafe {
        let native_view = handle.ns_view.as_ptr() as id;
        let native_window: id = msg_send![native_view, window];
        let content = native_window.contentView();
        let frame = NSView::bounds(content);
        let effect: id = msg_send![class!(NSVisualEffectView), alloc];
        let effect = NSView::initWithFrame_(effect, frame);
        effect.setAutoresizingMask_(NSViewWidthSizable | NSViewHeightSizable);
        effect.setMaterial_(NSVisualEffectMaterial::Sidebar);
        effect.setState_(NSVisualEffectState::Active);
        effect.setBlendingMode_(NSVisualEffectBlendingMode::BehindWindow);
        let _: () = msg_send![content, addSubview:effect positioned:NSWindowOrderingMode::NSWindowBelow relativeTo:nil];
        let _: id = msg_send![effect, autorelease];
    }
    Ok(())
}

/// Read the same accessibility preference that Chromium exposes to CSS.
pub fn reduced_motion() -> bool {
    unsafe {
        let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
        let reduced: cocoa::base::BOOL =
            msg_send![workspace, accessibilityDisplayShouldReduceMotion];
        reduced == cocoa::base::YES
    }
}

struct DrawingAppearance(id);

impl Drop for DrawingAppearance {
    fn drop(&mut self) {
        unsafe {
            let _: () = msg_send![class!(NSAppearance), setCurrentAppearance: self.0];
            let _: () = msg_send![self.0, release];
        }
    }
}

/// Read the light history panel's system selection color on the AppKit main thread.
pub fn history_selection_background(active: bool) -> Result<Rgba> {
    let channels = unsafe {
        let aqua_name = NSString::alloc(nil).init_str("NSAppearanceNameAqua");
        let aqua: id = msg_send![class!(NSAppearance), appearanceNamed: aqua_name];
        let _: () = msg_send![aqua_name, release];
        if aqua == nil {
            bail!("Read the AppKit Aqua appearance");
        }

        let previous: id = msg_send![class!(NSAppearance), currentAppearance];
        let _appearance = DrawingAppearance(msg_send![previous, retain]);
        let _: () = msg_send![class!(NSAppearance), setCurrentAppearance: aqua];
        let color: id = if active {
            msg_send![class!(NSColor), selectedTextBackgroundColor]
        } else {
            msg_send![class!(NSColor), unemphasizedSelectedContentBackgroundColor]
        };
        let srgb: id = msg_send![class!(NSColorSpace), sRGBColorSpace];
        let color: id = msg_send![color, colorUsingColorSpace: srgb];
        if color == nil {
            bail!("Convert the AppKit history selection color to sRGB");
        }
        let red: f64 = msg_send![color, redComponent];
        let green: f64 = msg_send![color, greenComponent];
        let blue: f64 = msg_send![color, blueComponent];
        let alpha: f64 = msg_send![color, alphaComponent];

        // Match the f64 nextafter(256, 0) conversion in the source renderer.
        [red, green, blue, alpha].map(|channel| (channel * 256_f64.next_down()) as u8)
    };
    Ok(rgba(u32::from_be_bytes(blend_selection_with_white(
        channels,
    ))))
}

fn blend_selection_with_white(channels: [u8; 4]) -> [u8; 4] {
    let [red, green, blue, opacity] = channels;
    if opacity != 255 {
        return channels;
    }

    // Match Blink Color::BlendWithWhite, undoing the white background with
    // progressively less transparency until all RGB components fit.
    let mut blended = channels;
    for alpha in (153..=204).step_by(17) {
        let white = 255 - alpha;
        let opacity = alpha as f32 / 255.0;
        let rgb = [red, green, blue]
            .map(|channel| ((i32::from(channel) - white) as f32 / opacity) as i32);
        let [red, green, blue] = rgb.map(|channel| channel.clamp(0, 255) as u8);
        blended = [red, green, blue, alpha as u8];
        if rgb.iter().all(|&channel| channel >= 0) {
            break;
        }
    }
    blended
}
