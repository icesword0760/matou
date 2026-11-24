//! Match the frozen macOS window's native sidebar material.
//!
//! GPUI's generic blur intentionally strips AppKit's tint and uses Selection.
//! The existing product uses the system Sidebar material with active effects.
#![allow(unexpected_cfgs)] // objc 0.2 macros probe their historical cargo-clippy feature.
use std::{
    cell::RefCell,
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

#[cfg(debug_assertions)]
pub mod window_diag {
    use super::*;
    use cocoa::{
        appkit::{NSColor, NSScreen},
        base::BOOL,
        foundation::{NSArray, NSAutoreleasePool, NSDictionary, NSPoint, NSRect, NSSize},
    };
    use core_foundation::base::{CFType, CFTypeRef};
    use core_graphics::display::CGDisplay;
    use gpui::{AnyWindowHandle, App};
    use std::{
        ffi::CStr,
        sync::{
            Arc, OnceLock,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        time::{Duration, Instant},
    };

    static SAMPLE: OnceLock<FrameSample> = OnceLock::new();

    struct FrameSample {
        window: AnyWindowHandle,
        end: Instant,
        painted: AtomicBool,
        frame_requests: AtomicUsize,
        display_callbacks: AtomicUsize,
    }

    pub fn window_created(window: &Window, cx: &mut App, visibility_probe: bool) {
        let display = snapshot(window, "created").unwrap_or_else(|| CGDisplay::main().id);
        let duration = Duration::from_secs(1);
        let sample = SAMPLE.get_or_init(|| FrameSample {
            window: window.window_handle(),
            end: Instant::now() + duration,
            painted: AtomicBool::new(false),
            frame_requests: AtomicUsize::new(0),
            display_callbacks: AtomicUsize::new(0),
        });
        watch_gpui_frame(window, sample);
        let timer = cx.background_executor().timer(duration);
        cx.background_executor()
            .spawn(async move {
                timer.await;
                let requests = sample.frame_requests.load(Ordering::Relaxed);
                eprintln!("[window-diag] GPUI frame requests in 1s: {requests}");
            })
            .detach();

        if let Err(error) =
            thread::Builder::new().spawn(move || sample_display_link(display, sample))
        {
            eprintln!("[window-diag] start CoreVideo sampling thread: {error}");
        }

        // Wait until GPUI has registered the window before AppKit can notify it about ordering.
        window.defer(cx, move |window, _| {
            if let Some((native_window, _)) = native_window_and_view(window) {
                eprintln!("[window-diag] orderFrontRegardless");
                unsafe {
                    if visibility_probe {
                        let color = NSColor::colorWithSRGBRed_green_blue_alpha_(nil, 1., 0., 1., 1.);
                        native_window.setBackgroundColor_(color);
                        native_window.setOpaque_(YES);
                        eprintln!("[window-diag] visibility probe: NSWindow background sRGB(1,0,1,1), opaque=true");
                    }
                    let _: () = msg_send![native_window, orderFrontRegardless];
                }
            }
        });
    }

    pub fn workspace_painted(window: &Window, cx: &mut App) {
        let Some(sample) = SAMPLE.get() else { return };
        if sample.window == window.window_handle() && !sample.painted.swap(true, Ordering::Relaxed)
        {
            let size = window.viewport_size();
            eprintln!(
                "[window-diag] first paint size={}x{}",
                f32::from(size.width),
                f32::from(size.height)
            );
            window.defer(cx, |window, cx| {
                snapshot(window, "after first paint");
                probe_metal_drawable(window, cx);
            });
        }
    }

    fn watch_gpui_frame(window: &Window, sample: &'static FrameSample) {
        // This also sees AppKit-driven frame requests. It queues no redraw.
        window.on_next_frame(move |window, _| {
            if Instant::now() < sample.end {
                sample.frame_requests.fetch_add(1, Ordering::Relaxed);
                watch_gpui_frame(window, sample);
            }
        });
    }

    fn native_window_and_view(window: &Window) -> Option<(id, id)> {
        let RawWindowHandle::AppKit(handle) = HasWindowHandle::window_handle(window).ok()?.as_raw()
        else {
            return None;
        };
        let view = handle.ns_view.as_ptr() as id;
        let native_window: id = unsafe { msg_send![view, window] };
        (native_window != nil).then_some((native_window, view))
    }

    fn snapshot(window: &Window, phase: &str) -> Option<u32> {
        // The GPUI window keeps these AppKit objects alive on the main thread.
        unsafe {
            let app = NSApp();
            let policy: isize = msg_send![app, activationPolicy];
            let active: BOOL = msg_send![app, isActive];
            eprintln!(
                "[window-diag] {phase} NSApp.activationPolicy={policy} NSApp.isActive={}",
                active == YES
            );

            let Some((native_window, view)) = native_window_and_view(window) else {
                eprintln!("[window-diag] {phase} NSWindow unavailable");
                return None;
            };
            let visible: BOOL = msg_send![native_window, isVisible];
            let key: BOOL = msg_send![native_window, isKeyWindow];
            let main: BOOL = msg_send![native_window, isMainWindow];
            let frame = NSWindow::frame(native_window);
            let content = native_window.contentView();
            let content_frame = (content != nil).then(|| NSView::frame(content));
            let level: isize = msg_send![native_window, level];
            let occlusion: usize = msg_send![native_window, occlusionState];
            let scale: f64 = msg_send![native_window, backingScaleFactor];
            eprintln!(
                "[window-diag] {phase} NSWindow.isVisible={} isKeyWindow={} isMainWindow={} frame={:?} contentView.frame={:?} level={level} occlusionState={occlusion:#x} backingScaleFactor={scale}",
                visible == YES,
                key == YES,
                main == YES,
                frame.as_CGRect(),
                content_frame.as_ref().map(NSRect::as_CGRect),
            );
            let key_window: id = msg_send![app, keyWindow];
            let main_window: id = msg_send![app, mainWindow];
            let can_become_key: BOOL = msg_send![native_window, canBecomeKeyWindow];
            let window_number: isize = msg_send![native_window, windowNumber];
            let color_space_name: id =
                msg_send![NSWindow::colorSpace(native_window), localizedName];
            let background: id = msg_send![native_window.backgroundColor(), description];
            let opaque: BOOL = msg_send![native_window, isOpaque];
            eprintln!(
                "[window-diag] {phase} NSWindow.windowNumber={window_number} NSApp.keyWindow={} NSApp.mainWindow={} NSWindow.canBecomeKeyWindow={} colorSpace={:?} hasShadow={} isOpaque={} backgroundColor={:?}",
                describe_window(key_window, native_window),
                describe_window(main_window, native_window),
                can_become_key == YES,
                ns_string(color_space_name),
                native_window.hasShadow() == YES,
                opaque == YES,
                ns_string(background),
            );
            let screen = native_window.screen();
            let display = if screen != nil {
                let frame = NSScreen::frame(screen);
                let visible_frame = NSScreen::visibleFrame(screen);
                let description = NSScreen::deviceDescription(screen);
                let key = NSString::alloc(nil).init_str("NSScreenNumber");
                let number = description.objectForKey_(key);
                let _: () = msg_send![key, release];
                let display: u32 = msg_send![number, unsignedIntValue];
                eprintln!(
                    "[window-diag] {phase} screen.frame={:?} screen.visibleFrame={:?} display={display}",
                    frame.as_CGRect(),
                    visible_frame.as_CGRect(),
                );
                (number != nil).then_some(display)
            } else {
                eprintln!(
                    "[window-diag] {phase} screen.frame unavailable screen.visibleFrame unavailable"
                );
                None
            };

            log_view_tree(phase, content, view);
            log_layer(phase, "contentView.layer", content);
            if view != content {
                log_layer(phase, "gpuiView.layer", view);
            }
            display
        }
    }

    unsafe fn describe_window(window: id, target: id) -> String {
        if window == nil {
            return "nil".into();
        }
        let number: isize = unsafe { msg_send![window, windowNumber] };
        let relation = if window == target { "this" } else { "other" };
        format!("{relation}(#{number})")
    }

    unsafe fn ns_string(value: id) -> Option<String> {
        let bytes = unsafe { value.UTF8String() };
        (!bytes.is_null()).then(|| unsafe { CStr::from_ptr(bytes).to_string_lossy().into_owned() })
    }

    unsafe fn log_view_tree(phase: &str, content: id, gpui_view: id) {
        if content == nil {
            eprintln!("[window-diag] {phase} contentView=nil");
            return;
        }
        unsafe {
            let subviews: id = msg_send![content, subviews];
            let superview: id = msg_send![gpui_view, superview];
            eprintln!(
                "[window-diag] {phase} contentView.wantsLayer={} contentView.subviews.count={} gpuiView.inContentViewSubviews={}",
                content.wantsLayer() == YES,
                NSArray::count(subviews),
                superview == content,
            );
            log_subviews(phase, "contentView", subviews, gpui_view);
        }
    }

    unsafe fn log_subviews(phase: &str, parent: &str, subviews: id, gpui_view: id) {
        unsafe {
            for index in 0..NSArray::count(subviews) {
                let view = subviews.objectAtIndex(index);
                let path = format!("{parent}.subviews[{index}]");
                let class = (*view).class().name();
                let frame = NSView::frame(view);
                let hidden: BOOL = msg_send![view, isHidden];
                let alpha: f64 = msg_send![view, alphaValue];
                eprintln!(
                    "[window-diag] {phase} {path} class={class} frame={:?} hidden={} alphaValue={alpha} wantsLayer={} isGPUIView={}",
                    frame.as_CGRect(),
                    hidden == YES,
                    view.wantsLayer() == YES,
                    view == gpui_view,
                );
                let children: id = msg_send![view, subviews];
                log_subviews(phase, &path, children, gpui_view);
            }
        }
    }

    unsafe fn allows_drawable_timeout(layer: id) -> Option<bool> {
        unsafe {
            let supported: BOOL =
                msg_send![layer, respondsToSelector: sel!(allowsNextDrawableTimeout)];
            (supported == YES).then(|| {
                let allowed: BOOL = msg_send![layer, allowsNextDrawableTimeout];
                allowed == YES
            })
        }
    }

    unsafe fn log_layer(phase: &str, label: &str, view: id) {
        unsafe {
            let layer: id = msg_send![view, layer];
            if layer == nil {
                eprintln!("[window-diag] {phase} {label}=nil");
                return;
            }
            let name = (*layer).class().name();
            let bounds: NSRect = msg_send![layer, bounds];
            let position: NSPoint = msg_send![layer, position];
            let scale: f64 = msg_send![layer, contentsScale];
            let opaque: BOOL = msg_send![layer, isOpaque];
            let opacity: f32 = msg_send![layer, opacity];
            let masks: BOOL = msg_send![layer, masksToBounds];
            let background: CFTypeRef = msg_send![layer, backgroundColor];
            let background =
                (!background.is_null()).then(|| CFType::wrap_under_get_rule(background));
            let gravity: id = msg_send![layer, contentsGravity];
            let allows_timeout = allows_drawable_timeout(layer);
            let has_drawable_size: BOOL = msg_send![layer, respondsToSelector: sel!(drawableSize)];
            let drawable_size = (has_drawable_size == YES).then(|| {
                let size: NSSize = msg_send![layer, drawableSize];
                (size.width, size.height)
            });
            eprintln!(
                "[window-diag] {phase} {label}={name} bounds={:?} position=({}, {}) contentsScale={scale} opaque={} opacity={opacity} drawableSize={drawable_size:?}",
                bounds.as_CGRect(),
                position.x,
                position.y,
                opaque == YES,
            );
            eprintln!(
                "[window-diag] {phase} {label} masksToBounds={} backgroundColor={background:?} contentsGravity={:?} allowsNextDrawableTimeout={allows_timeout:?}",
                masks == YES,
                ns_string(gravity),
            );
        }
    }

    fn probe_metal_drawable(window: &Window, cx: &App) {
        let Some((_, view)) = native_window_and_view(window) else {
            return;
        };
        unsafe {
            let layer: id = msg_send![view, layer];
            let is_metal_layer: BOOL = msg_send![layer, isKindOfClass: class!(CAMetalLayer)];
            if is_metal_layer != YES {
                eprintln!(
                    "[window-diag] after first paint Metal probe unavailable: no CAMetalLayer"
                );
                return;
            }
            let presents_with_transaction: BOOL = msg_send![layer, presentsWithTransaction];
            let maximum_drawables: usize = msg_send![layer, maximumDrawableCount];
            // currentDrawable is an MTKView property; use only its public accessor.
            let is_mtk_view = Class::get("MTKView").is_some_and(|class| {
                let matches: BOOL = msg_send![view, isKindOfClass: class];
                matches == YES
            });
            let current_drawable = if is_mtk_view {
                let drawable: id = msg_send![view, currentDrawable];
                if drawable == nil { "nil" } else { "available" }
            } else {
                "unavailable (requires MTKView)"
            };
            eprintln!(
                "[window-diag] after first paint Metal currentDrawable={current_drawable} presentsWithTransaction={} maximumDrawableCount={maximum_drawables}",
                presents_with_transaction == YES,
            );
            let layer = DrawableProbeLayer(msg_send![layer, retain]);
            let returned = Arc::new(AtomicBool::new(false));
            let worker_returned = returned.clone();
            if let Err(error) = thread::Builder::new().spawn(move || {
                let start = Instant::now();
                eprintln!("[window-diag] after first paint Metal nextDrawable begin");
                let result = layer.acquire_and_release();
                worker_returned.store(true, Ordering::Relaxed);
                eprintln!(
                    "[window-diag] after first paint Metal nextDrawable={result} elapsed_ms={:.1}",
                    start.elapsed().as_secs_f64() * 1000.,
                );
            }) {
                eprintln!("[window-diag] after first paint Metal nextDrawable worker: {error}");
                return;
            }
            let timer = cx.background_executor().timer(Duration::from_secs(1));
            cx.background_executor()
                .spawn(async move {
                    timer.await;
                    if !returned.load(Ordering::Relaxed) {
                        eprintln!(
                            "[window-diag] after first paint Metal nextDrawable=pending after 1s"
                        );
                    }
                })
                .detach();
        }
    }

    struct DrawableProbeLayer(id);

    // Core Animation supports obtaining a CAMetalLayer drawable on a rendering
    // thread. The worker owns a retained layer and never touches AppKit views.
    unsafe impl Send for DrawableProbeLayer {}

    impl DrawableProbeLayer {
        fn acquire_and_release(&self) -> &'static str {
            // Hold the diagnostic drawable only for this autorelease scope.
            unsafe {
                let pool = NSAutoreleasePool::new(nil);
                let drawable: id = msg_send![self.0, nextDrawable];
                let result = if drawable == nil { "nil" } else { "available" };
                pool.drain();
                result
            }
        }
    }

    impl Drop for DrawableProbeLayer {
        fn drop(&mut self) {
            unsafe {
                let _: () = msg_send![self.0, release];
            }
        }
    }

    type DisplayLinkRef = *mut c_void;
    type DisplayLinkCallback = unsafe extern "C" fn(
        DisplayLinkRef,
        *const c_void,
        *const c_void,
        u64,
        *mut u64,
        *mut c_void,
    ) -> i32;

    #[link(name = "CoreVideo", kind = "framework")]
    unsafe extern "C" {
        fn CVDisplayLinkCreateWithCGDisplay(display: u32, link: *mut DisplayLinkRef) -> i32;
        fn CVDisplayLinkSetOutputCallback(
            link: DisplayLinkRef,
            callback: DisplayLinkCallback,
            context: *mut c_void,
        ) -> i32;
        fn CVDisplayLinkStart(link: DisplayLinkRef) -> i32;
        fn CVDisplayLinkStop(link: DisplayLinkRef) -> i32;
        fn CVDisplayLinkRelease(link: DisplayLinkRef);
    }

    unsafe extern "C" fn count_display_callback(
        _: DisplayLinkRef,
        _: *const c_void,
        _: *const c_void,
        _: u64,
        _: *mut u64,
        context: *mut c_void,
    ) -> i32 {
        // SAMPLE outlives the link and any pending CoreVideo callback.
        let sample = unsafe { &*context.cast::<FrameSample>() };
        if Instant::now() < sample.end {
            sample.display_callbacks.fetch_add(1, Ordering::Relaxed);
        }
        0
    }

    fn sample_display_link(display: u32, sample: &'static FrameSample) {
        unsafe {
            let mut link = ptr::null_mut();
            let status = CVDisplayLinkCreateWithCGDisplay(display, &mut link);
            if status != 0 || link.is_null() {
                eprintln!("[window-diag] CVDisplayLinkCreateWithCGDisplay({display}): {status}");
                return;
            }
            let context = ptr::from_ref(sample).cast_mut().cast();
            let callback_status =
                CVDisplayLinkSetOutputCallback(link, count_display_callback, context);
            if callback_status != 0 {
                eprintln!("[window-diag] CVDisplayLinkSetOutputCallback: {callback_status}");
            } else {
                let start_status = CVDisplayLinkStart(link);
                if start_status != 0 {
                    eprintln!("[window-diag] CVDisplayLinkStart: {start_status}");
                } else {
                    thread::sleep(sample.end.saturating_duration_since(Instant::now()));
                    let callbacks = sample.display_callbacks.load(Ordering::Relaxed);
                    eprintln!(
                        "[window-diag] CoreVideo display link callbacks in 1s: {callbacks} display={display}"
                    );
                    let stop_status = CVDisplayLinkStop(link);
                    if stop_status != 0 {
                        eprintln!("[window-diag] CVDisplayLinkStop: {stop_status}");
                    }
                }
            }
            CVDisplayLinkRelease(link);
        }
    }
}

type RuntimeShutdown = Option<JoinHandle<Result<()>>>;

enum Termination {
    Immediate,
    Armed(Box<dyn FnOnce()>),
    Waiting,
}

thread_local! {
    static TERMINATION: RefCell<Termination> = const { RefCell::new(Termination::Immediate) };
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
    TERMINATION.replace(Termination::Armed(Box::new(move || {
        // The worker only waits on this channel until the first quit request.
        send.send(begin())
            .expect("native runtime shutdown worker is waiting for its request");
    })));
    Ok(())
}

extern "C" fn application_should_terminate(_: &Object, _: Sel, _: id) -> usize {
    request_termination()
}

fn request_termination() -> usize {
    match TERMINATION.replace(Termination::Waiting) {
        Termination::Armed(begin) => {
            begin();
            TERMINATE_LATER
        }
        Termination::Waiting => TERMINATE_LATER,
        Termination::Immediate => {
            TERMINATION.replace(Termination::Immediate);
            TERMINATE_NOW
        }
    }
}

pub fn termination_pending() -> bool {
    // The main thread owns the delegate and window close callbacks.
    TERMINATION.with(|termination| matches!(*termination.borrow(), Termination::Waiting))
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
    if finish_termination() {
        unsafe {
            let _: () = msg_send![NSApp(), replyToApplicationShouldTerminate: YES];
        }
    }
}

fn finish_termination() -> bool {
    if termination_pending() {
        TERMINATION.replace(Termination::Immediate);
        true
    } else {
        false
    }
}

#[cfg(test)]
mod termination_tests {
    use super::*;
    use std::{cell::Cell, rc::Rc};

    #[test]
    fn close_stays_deferred_until_the_shutdown_reply() {
        let starts = Rc::new(Cell::new(0));
        let begin = starts.clone();
        TERMINATION.replace(Termination::Armed(Box::new(move || {
            assert!(termination_pending());
            begin.set(begin.get() + 1);
        })));

        assert!(!finish_termination());
        assert_eq!(request_termination(), TERMINATE_LATER);
        assert_eq!(request_termination(), TERMINATE_LATER);
        assert_eq!(starts.get(), 1);
        assert!(termination_pending());
        assert!(finish_termination());
        assert!(!termination_pending());
        assert!(!finish_termination());
        assert_eq!(request_termination(), TERMINATE_NOW);
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
