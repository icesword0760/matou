//! Match the frozen macOS window's native sidebar material.
//!
//! GPUI's generic blur intentionally strips AppKit's tint and uses Selection.
//! The existing product uses the system Sidebar material with active effects.
#![allow(unexpected_cfgs)] // objc 0.2 macros probe their historical cargo-clippy feature.
use std::{
    cell::Cell,
    ffi::c_void,
    ptr,
    sync::mpsc,
    thread::{self, JoinHandle},
};

use anyhow::{Context, Result, bail};
use cocoa::{
    appkit::{
        NSApp, NSView, NSViewHeightSizable, NSViewWidthSizable, NSVisualEffectBlendingMode,
        NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindow,
        NSWindowOrderingMode,
    },
    base::{YES, id, nil},
    foundation::NSString,
};
use core_foundation::{
    base::TCFType,
    runloop::{
        CFRunLoop, CFRunLoopTimer, CFRunLoopTimerRef, CFRunLoopWakeUp, kCFRunLoopCommonModes,
    },
    string::CFString,
};
use gpui::{Rgba, Window, rgba};
use objc::{
    class,
    declare::MethodImplementation,
    msg_send,
    runtime::{Class, Object, Sel, class_addMethod},
    sel, sel_impl,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

const TERMINATE_NOW: usize = 1;
const TERMINATE_LATER: usize = 2;

type RuntimeShutdown = Option<JoinHandle<Result<()>>>;

enum Termination {
    Immediate,
    Armed(Box<dyn FnOnce()>),
    Waiting,
}

thread_local! {
    static TERMINATION: Cell<Termination> = const { Cell::new(Termination::Immediate) };
}

#[repr(C)]
struct DispatchQueue {
    _opaque: [u8; 0],
}

unsafe extern "C" {
    static _dispatch_main_q: DispatchQueue;
    fn dispatch_async_f(
        queue: *const DispatchQueue,
        context: *mut c_void,
        work: extern "C" fn(*mut c_void),
    );
}

/// Install before GPUI assigns its delegate to NSApplication.
pub fn install_termination_hook() -> Result<()> {
    let delegate = Class::get("GPUIApplicationDelegate").context("GPUI app delegate not found")?;
    let selector = sel!(applicationShouldTerminate:);
    if delegate.instance_method(selector).is_some() {
        bail!("GPUI app delegate already implements applicationShouldTerminate:");
    }
    let implementation = application_should_terminate as extern "C" fn(&Object, Sel, id) -> usize;
    // NSApplicationTerminateReply is an NSUInteger (unsigned 64-bit on macOS).
    let added = unsafe {
        class_addMethod(
            ptr::from_ref(delegate).cast_mut(),
            selector,
            implementation.imp(),
            c"Q@:@".as_ptr(),
        )
    };
    if added != YES {
        bail!("Could not add applicationShouldTerminate: to the GPUI app delegate");
    }
    Ok(())
}

/// Arm the hook when the desktop is ready. Have a worker available before promising AppKit a reply.
pub fn defer_termination(begin: impl FnOnce() -> RuntimeShutdown + 'static) -> Result<()> {
    let (send, receive) = mpsc::channel::<RuntimeShutdown>();
    thread::Builder::new()
        .name("matou-runtime-shutdown".into())
        .spawn(move || {
            let Ok(supervisor) = receive.recv() else {
                return;
            };
            if let Some(supervisor) = supervisor {
                match supervisor.join() {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => eprintln!("Close native runtime: {error:#}"),
                    Err(payload) => {
                        let message = payload
                            .downcast_ref::<String>()
                            .map(String::as_str)
                            .or_else(|| payload.downcast_ref::<&str>().copied())
                            .unwrap_or("non-string panic payload");
                        eprintln!("Close native runtime: native runtime bridge manager panicked: {message}");
                    }
                }
            }
            schedule_termination_reply();
        })
        .context("start native runtime shutdown worker")?;
    TERMINATION.set(Termination::Armed(Box::new(move || {
        // The worker only waits on this channel until the first quit request.
        send.send(begin())
            .expect("native runtime shutdown worker is waiting for its request");
    })));
    Ok(())
}

extern "C" fn application_should_terminate(_: &Object, _: Sel, _: id) -> usize {
    match TERMINATION.replace(Termination::Waiting) {
        Termination::Armed(begin) => {
            begin();
            TERMINATE_LATER
        }
        Termination::Waiting => TERMINATE_LATER,
        Termination::Immediate => {
            TERMINATION.set(Termination::Immediate);
            TERMINATE_NOW
        }
    }
}

fn schedule_termination_reply() {
    extern "C" fn dispatch_reply(_: *mut c_void) {
        reply_to_termination();
    }
    extern "C" fn run_loop_reply(_: CFRunLoopTimerRef, _: *mut c_void) {
        reply_to_termination();
    }

    unsafe {
        dispatch_async_f(&raw const _dispatch_main_q, ptr::null_mut(), dispatch_reply);
    }

    // GPUI calls terminate: from the main dispatch queue. Its nested AppKit
    // loop can service a run loop timer while that queue's callback is active.
    let main = CFRunLoop::get_main();
    let modal = CFString::new("NSModalPanelRunLoopMode");
    let timer = CFRunLoopTimer::new(0., 0., 0, 0, run_loop_reply, ptr::null_mut());
    main.add_timer(&timer, modal.as_concrete_TypeRef());
    unsafe {
        main.add_timer(&timer, kCFRunLoopCommonModes);
        CFRunLoopWakeUp(main.as_concrete_TypeRef());
    }
}

fn reply_to_termination() {
    if !matches!(
        TERMINATION.replace(Termination::Immediate),
        Termination::Waiting
    ) {
        return;
    }
    unsafe {
        let _: () = msg_send![NSApp(), replyToApplicationShouldTerminate: YES];
    }
}

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

/// AppKit visibility, independent of key-window activation.
pub fn is_window_visible(window: &Window) -> Result<bool> {
    let RawWindowHandle::AppKit(handle) = HasWindowHandle::window_handle(window)
        .map_err(|error| anyhow::anyhow!("Read AppKit window handle: {error}"))?
        .as_raw()
    else {
        bail!("Expected an AppKit window");
    };
    unsafe {
        let native_view = handle.ns_view.as_ptr() as id;
        let native_window: id = msg_send![native_view, window];
        if native_window == nil {
            bail!("AppKit view has no window");
        }
        let visible: cocoa::base::BOOL = msg_send![native_window, isVisible];
        Ok(visible == cocoa::base::YES)
    }
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
