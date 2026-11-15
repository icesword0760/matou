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
};
use gpui::Window;
use objc::{class, msg_send, sel, sel_impl};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

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
