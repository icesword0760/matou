#[cfg(target_os = "macos")]
use std::cell::Cell;
use std::{cell::RefCell, rc::Rc};

use anyhow::Result;
use gpui::*;
#[cfg(target_os = "macos")]
use matou_native_desktop::macos;
use matou_native_desktop::{
    assets::NativeAssets,
    runtime_bridge::{LaunchOptions, RuntimeBridge},
    workspace::Workspace,
};

actions!(matou_native, [Quit]);

fn main() -> Result<()> {
    let options = options()?;
    let runtime = Rc::new(RefCell::new(RuntimeBridge::start(options)?));
    let commands = runtime.borrow().commands.clone();
    let events = runtime.borrow().events.clone();
    let quit_runtime = runtime.clone();
    let application = Application::new().with_assets(NativeAssets);
    #[cfg(target_os = "macos")]
    let termination_hook = match macos::install_termination_hook() {
        Ok(()) => true,
        Err(error) => {
            eprintln!("Install native termination hook: {error:#}");
            false
        }
    };
    #[cfg(target_os = "macos")]
    let quitting = Rc::new(Cell::new(false));
    #[cfg(target_os = "macos")]
    let main_window = Rc::new(Cell::new(None::<WindowHandle<Workspace>>));
    #[cfg(target_os = "macos")]
    application.on_reopen({
        let main_window = main_window.clone();
        let quitting = quitting.clone();
        move |cx| {
            if quitting.get() {
                return;
            }
            if let Some(window) = main_window.get() {
                cx.activate(true);
                if let Err(error) = window.update(cx, |_, window, _| window.activate_window()) {
                    eprintln!("Reactivate native window: {error:#}");
                }
            }
        }
    });
    application.run(move |cx| {
        #[cfg(target_os = "macos")]
        {
            let window = main_window.clone();
            let quitting = quitting.clone();
            let runtime = quit_runtime.clone();
            let mut cx = cx.to_async();
            if termination_hook
                && let Err(error) = macos::defer_termination(move || {
                    quitting.set(true);
                    if let Some(window) = window.get()
                        && let Err(error) = window.update(&mut cx, |view, _, cx| {
                            // Queue the last geometry write while the runtime still accepts commands.
                            view.flush_geometry(cx);
                        })
                    {
                        eprintln!("Flush native window geometry before quitting: {error:#}");
                    }
                    runtime.borrow_mut().shutdown_async()
                })
            {
                eprintln!("Defer native termination: {error:#}");
            }
        }
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
        cx.set_menus(vec![Menu {
            name: "Matou Native".into(),
            items: vec![MenuItem::action("退出 Matou Native", Quit)],
        }]);
        let bounds = Bounds::centered(None, size(px(1200.), px(780.)), cx);
        #[cfg(all(target_os = "macos", debug_assertions))]
        let window_diag = match std::env::var("MATOU_WINDOW_DIAG").as_deref() {
            Ok("1") => Some(false),
            Ok("2") => Some(true),
            _ => None,
        };
        #[cfg(all(target_os = "macos", debug_assertions))]
        if window_diag.is_some() {
            eprintln!("[window-diag] open_window begin");
        }
        match cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(720.), px(480.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Matou Native".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(14.), px(16.))),
                }),
                window_background: WindowBackgroundAppearance::Transparent,
                app_id: Some("com.matou.native-parity".into()),
                ..Default::default()
            },
            |window, cx| {
                #[cfg(target_os = "macos")]
                if let Err(error) = macos::apply_sidebar_material(window) {
                    eprintln!("Apply native sidebar material: {error:#}");
                }
                let workspace = cx.new(|cx| Workspace::new(commands, events, window, cx));
                #[cfg(target_os = "macos")]
                {
                    let workspace = workspace.downgrade();
                    let quitting = quitting.clone();
                    window.on_window_should_close(cx, move |window, cx| {
                        if quitting.get() {
                            return !macos::termination_pending();
                        }
                        if let Err(error) = workspace.update(cx, |view, cx| view.flush_geometry(cx))
                        {
                            eprintln!("Flush native window geometry before hiding: {error:#}");
                            return false;
                        }
                        if let Err(error) = macos::hide_window(window) {
                            eprintln!("Hide native window: {error:#}");
                        }
                        false
                    });
                }
                #[cfg(all(target_os = "macos", debug_assertions))]
                if let Some(visibility_probe) = window_diag {
                    macos::window_diag::window_created(window, cx, visibility_probe);
                }
                workspace
            },
        ) {
            Ok(window) => {
                #[cfg(target_os = "macos")]
                main_window.set(Some(window));
                #[cfg(not(target_os = "macos"))]
                cx.on_app_quit(move |cx| {
                    if let Err(error) = window.update(cx, |view, _, cx| view.flush_geometry(cx)) {
                        eprintln!("Flush native window geometry before quitting: {error:#}");
                    }
                    if let Err(error) = quit_runtime.borrow_mut().shutdown() {
                        eprintln!("Close native runtime: {error:#}");
                    }
                    async {}
                })
                .detach();
                cx.activate(true);
            }
            Err(error) => {
                eprintln!("Open native window: {error:#}");
                cx.quit();
            }
        }
        #[cfg(target_os = "macos")]
        cx.on_app_quit(move |cx| {
            if !quitting.replace(true)
                && let Some(window) = main_window.get()
                && let Err(error) = window.update(cx, |view, _, cx| view.flush_geometry(cx))
            {
                eprintln!("Flush native window geometry before quitting: {error:#}");
            }
            if let Err(error) = quit_runtime.borrow_mut().shutdown() {
                eprintln!("Close native runtime: {error:#}");
            }
            async {}
        })
        .detach();
    });
    runtime.borrow_mut().shutdown()
}

fn options() -> Result<LaunchOptions> {
    LaunchOptions::from_args(std::env::args_os().skip(1))
}
