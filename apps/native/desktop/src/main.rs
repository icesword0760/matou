use std::{
    cell::{Cell, RefCell},
    io,
    rc::Rc,
    time::Duration,
};

use anyhow::Result;
use gpui::*;
use matou_native_core::sessions::{
    InterfaceLocale, current_interface_locale, read_locale_preference,
};
use matou_native_desktop::{
    assets::NativeAssets,
    instance_open::{self, DesktopOptions, Instance, InstanceOwner, OpenRequest},
    runtime_bridge::{Command, ConnectionPhase, RuntimeBridge},
    window_registry::WindowRegistry,
    workspace::Workspace,
};
#[cfg(target_os = "macos")]
use matou_native_desktop::{macos, window_registry::file_url_path};

#[cfg(target_os = "windows")]
mod windows_bounds;
#[cfg(target_os = "windows")]
use matou_native_desktop::{windows_theme, workspace::QuitConfirmed};

actions!(matou_native, [Quit]);

fn main() -> Result<()> {
    let DesktopOptions {
        runtime: options,
        paths,
    } = DesktopOptions::from_args(std::env::args_os().skip(1))?;
    let dag_probe = cfg!(all(target_os = "macos", debug_assertions))
        && std::env::var("MATOU_DAG_WINDOW_PROBE").as_deref() == Ok("hidden");
    if dag_probe {
        anyhow::ensure!(
            options.data_root.join("dag-window-probe.fixture").is_file(),
            "DAG probe requires an isolated fixture data root"
        );
    }
    let (send, requests) = async_channel::bounded(128);
    let Instance::Primary(instance) =
        instance_open::claim(&options.data_root, &paths, send.clone())?
    else {
        return Ok(());
    };
    let probe = if dag_probe {
        Some(Probe::DagWindow)
    } else if cfg!(debug_assertions) {
        match std::env::var("MATOU_WINDOW_REGISTRY_PROBE").as_deref() {
            Ok("hidden") => Some(Probe::Hidden),
            Ok("startup") => Some(Probe::Startup),
            Ok("no-window") => Some(Probe::NoWindow),
            _ => None,
        }
    } else {
        None
    };
    if let Some(probe) = probe {
        eprintln!("[instance-probe] owner pid={}", std::process::id());
        if probe == Probe::Startup {
            io::stdin().read_line(&mut String::new())?;
        }
    }
    let locale = current_interface_locale(read_locale_preference(&options.data_root));
    #[cfg(target_os = "windows")]
    let bounds_path = options.data_root.join("desktop-window-bounds.json");
    let runtime = Rc::new(RefCell::new(RuntimeBridge::start(options)?));
    let desktop = Rc::new(Desktop {
        instance: RefCell::new(Some(instance)),
        runtime: runtime.clone(),
        quitting: Cell::new(false),
        window: Cell::new(None),
        probe,
        #[cfg(target_os = "windows")]
        bounds_path,
    });
    let application = Application::new().with_assets(NativeAssets);
    #[cfg(target_os = "macos")]
    {
        let open = send.clone();
        application.on_open_urls(move |urls| {
            let paths = urls
                .iter()
                .filter_map(|url| file_url_path(url))
                .collect::<Vec<_>>();
            if !paths.is_empty()
                && let Err(error) = open.try_send(OpenRequest::Directories(paths))
            {
                eprintln!("Queue native file open request: {error}");
            }
        });
        application.on_reopen({
            let send = send.clone();
            move |_| {
                if let Err(error) = send.try_send(OpenRequest::Reopen) {
                    eprintln!("Queue native reopen request: {error}");
                }
            }
        });
    }
    #[cfg(target_os = "macos")]
    let termination_hook = match macos::install_termination_hook() {
        Ok(()) => true,
        Err(error) => {
            eprintln!("Install native termination hook: {error:#}");
            false
        }
    };
    application.run(move |cx| {
        #[cfg(target_os = "macos")]
        {
            let desktop = desktop.clone();
            let mut cx = cx.to_async();
            if termination_hook
                && let Err(error) = macos::defer_termination(move || {
                    cx.update(|cx| desktop.begin_quit(cx))
                        .unwrap_or_else(|error| eprintln!("Prepare native quit: {error:#}"));
                    desktop.runtime.borrow_mut().shutdown_async()
                })
            {
                eprintln!("Defer native termination: {error:#}");
            }
        }
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
        set_app_menu(locale, cx);
        cx.on_app_quit({
            let desktop = desktop.clone();
            move |cx| {
                desktop.begin_quit(cx);
                if let Err(error) = desktop.runtime.borrow_mut().shutdown() {
                    eprintln!("Close native runtime: {error:#}");
                }
                // AppKit can exit the process without unwinding main. Release the
                // channel here, after the Runtime stopped accepting work.
                desktop.instance.borrow_mut().take();
                async {}
            }
        })
        .detach();
        if desktop.probe != Some(Probe::NoWindow) {
            if !desktop.ensure_window(cx) {
                cx.quit();
                return;
            }
            if desktop.probe.is_none() {
                cx.activate(true);
            }
        }
        if desktop.probe.is_some() {
            eprintln!(
                "[instance-probe] initial windows={} runtime={}",
                cx.windows().len(),
                desktop.runtime.borrow().runtime_pid()
            );
            if desktop.probe != Some(Probe::DagWindow) {
                listen_for_probe_quit(cx);
            }
        }
        cx.spawn(async move |cx| run_requests(desktop, requests, paths, cx).await)
            .detach();
    });
    runtime.borrow_mut().shutdown()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Probe {
    Hidden,
    Startup,
    NoWindow,
    DagWindow,
}

#[cfg(any(target_os = "windows", test))]
fn should_save_window_bounds(probe: Option<Probe>, is_read_only: bool) -> bool {
    probe.is_none() && !is_read_only
}

#[cfg(any(target_os = "windows", test))]
fn should_confirm_main_window_close(probe: Option<Probe>, confirmed: bool) -> bool {
    probe.is_none() && !confirmed
}

#[cfg(any(target_os = "windows", test))]
fn is_windows_close_shortcut(stroke: &Keystroke) -> bool {
    let mods = stroke.modifiers;
    stroke.key.eq_ignore_ascii_case("f4") && mods.alt && !mods.control && !mods.platform
}

#[cfg(target_os = "windows")]
fn post_windows_close(window: &Window) -> bool {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};

    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return false;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return false;
    };
    // Route the system shortcut through the same native callback as the title-bar X,
    // even when a focused control would otherwise consume Alt+F4.
    unsafe { PostMessageW(handle.hwnd.get() as _, WM_CLOSE, 0, 0) != 0 }
}

#[cfg(test)]
mod tests {
    use super::{
        Probe, is_windows_close_shortcut, should_confirm_main_window_close,
        should_save_window_bounds,
    };
    use gpui::Keystroke;

    #[test]
    fn windows_main_close_requires_confirmation_unless_probing() {
        assert!(should_confirm_main_window_close(None, false));
        assert!(!should_confirm_main_window_close(None, true));
        for probe in [
            Probe::Hidden,
            Probe::Startup,
            Probe::NoWindow,
            Probe::DagWindow,
        ] {
            assert!(!should_confirm_main_window_close(Some(probe), false));
        }
    }

    #[test]
    fn saves_window_bounds_only_when_writable_and_not_probing() {
        assert!(should_save_window_bounds(None, false));
        assert!(!should_save_window_bounds(None, true));
        for probe in [
            Probe::Hidden,
            Probe::Startup,
            Probe::NoWindow,
            Probe::DagWindow,
        ] {
            assert!(!should_save_window_bounds(Some(probe), false));
            assert!(!should_save_window_bounds(Some(probe), true));
        }
    }

    #[test]
    fn windows_close_shortcut_requires_alt_f4() {
        for (stroke, closes) in [
            ("alt-f4", true),
            ("f4", false),
            ("ctrl-alt-f4", false),
            ("alt-f3", false),
        ] {
            assert_eq!(
                is_windows_close_shortcut(&Keystroke::parse(stroke).unwrap()),
                closes,
                "{stroke}"
            );
        }
    }
}

struct Desktop {
    instance: RefCell<Option<InstanceOwner>>,
    runtime: Rc<RefCell<RuntimeBridge>>,
    quitting: Cell<bool>,
    window: Cell<Option<WindowHandle<Workspace>>>,
    probe: Option<Probe>,
    #[cfg(target_os = "windows")]
    bounds_path: std::path::PathBuf,
}

impl Desktop {
    fn stop_forwarding_for_quit(&self) -> bool {
        if self.quitting.replace(true) {
            return false;
        }
        if let Some(instance) = self.instance.borrow().as_ref() {
            instance.stop_forwarding();
        }
        true
    }

    fn begin_quit(&self, cx: &mut App) {
        if !self.stop_forwarding_for_quit() {
            return;
        }
        if let Some(window) = self.live_window(cx)
            && let Err(error) = window.update(cx, |view, _, cx| view.flush_geometry(cx))
        {
            eprintln!("Flush native window geometry before quitting: {error:#}");
        }
    }

    #[cfg(target_os = "windows")]
    fn finish_windows_main_close(
        &self,
        workspace: &Entity<Workspace>,
        window: &Window,
        cx: &mut App,
    ) {
        let confirmed = workspace.read(cx).is_quit_confirmed();
        if confirmed && !self.stop_forwarding_for_quit() {
            return;
        }
        let save_bounds = workspace.update(cx, |view, cx| {
            view.flush_geometry(cx);
            should_save_window_bounds(self.probe, view.is_read_only())
        });
        if save_bounds
            && let Err(error) = windows_bounds::save(&self.bounds_path, window.window_bounds())
        {
            eprintln!("Save native window bounds: {error:#}");
        }
        if confirmed {
            // Quit the app even if detached windows are still open. The existing
            // app-quit observer owns runtime shutdown and instance cleanup.
            cx.quit();
        }
    }

    fn live_window(&self, cx: &App) -> Option<WindowHandle<Workspace>> {
        self.window
            .get()
            .filter(|window| cx.windows().contains(&AnyWindowHandle::from(*window)))
    }

    fn ensure_window(self: &Rc<Self>, cx: &mut App) -> bool {
        if self.live_window(cx).is_some() {
            return true;
        }
        match self.open_window(cx) {
            Ok(window) => {
                self.window.set(Some(window));
                #[cfg(all(target_os = "macos", debug_assertions))]
                if self.probe == Some(Probe::DagWindow) {
                    window
                        .update(cx, |view, window, cx| view.run_dag_window_probe(window, cx))
                        .expect("start hidden DAG window probe");
                }
                if self.probe.is_some() {
                    #[cfg(target_os = "macos")]
                    {
                        let visible =
                            window.update(cx, |_, window, _| macos::is_window_visible(window));
                        match visible {
                            Ok(Ok(visible)) => {
                                eprintln!("[instance-probe] native visible={visible}")
                            }
                            failure => eprintln!(
                                "[instance-probe] native visibility unavailable: {failure:?}"
                            ),
                        }
                    }
                    eprintln!(
                        "[instance-probe] window opened windows={} id={:?}",
                        cx.windows().len(),
                        window.window_id()
                    );
                }
                true
            }
            Err(error) => {
                eprintln!("Open native window: {error:#}");
                false
            }
        }
    }

    fn open_window(self: &Rc<Self>, cx: &mut App) -> Result<WindowHandle<Workspace>> {
        let min_size = size(px(720.), px(480.));
        let options = WindowOptions {
            show: self.probe.is_none(),
            focus: self.probe.is_none(),
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1200.), px(780.)),
                cx,
            ))),
            window_min_size: Some(min_size),
            titlebar: Some(TitlebarOptions {
                title: Some(if cfg!(target_os = "macos") {
                    "Matou Native".into()
                } else {
                    "码头".into()
                }),
                appears_transparent: cfg!(target_os = "macos"),
                traffic_light_position: cfg!(target_os = "macos")
                    .then_some(point(px(14.), px(16.))),
            }),
            window_background: if cfg!(target_os = "macos") {
                WindowBackgroundAppearance::Transparent
            } else {
                WindowBackgroundAppearance::Opaque
            },
            app_id: Some("com.matou.native-parity".into()),
            ..Default::default()
        };
        #[cfg(target_os = "windows")]
        let options = if self.probe.is_none()
            && let Some((bounds, display)) = windows_bounds::load(&self.bounds_path, min_size, cx)
        {
            WindowOptions {
                window_bounds: Some(bounds),
                display_id: Some(display),
                ..options
            }
        } else {
            options
        };
        #[cfg(all(target_os = "macos", debug_assertions))]
        let window_diag = match std::env::var("MATOU_WINDOW_DIAG").as_deref() {
            Ok("1") => Some(false),
            Ok("2") => Some(true),
            _ => None,
        };
        #[cfg(all(target_os = "macos", debug_assertions))]
        if self.probe.is_none() && window_diag.is_some() {
            eprintln!("[window-diag] open_window begin");
        }
        cx.open_window(options, |window, cx| {
            #[cfg(target_os = "macos")]
            if let Err(error) = macos::apply_sidebar_material(window) {
                eprintln!("Apply native sidebar material: {error:#}");
            }
            let runtime = self.runtime.borrow();
            let commands = runtime.commands.clone();
            let events = runtime.events.clone();
            let workspace = cx.new(|cx| Workspace::new(commands, events, window, cx));
            #[cfg(target_os = "windows")]
            windows_theme::set_dark(window, workspace.read(cx).uses_dark_theme());
            cx.subscribe(&workspace, |_, locale: &InterfaceLocale, cx| {
                set_app_menu(*locale, cx)
            })
            .detach();
            #[cfg(target_os = "windows")]
            {
                if self.probe.is_none() {
                    let main_window = window.window_handle();
                    cx.intercept_keystrokes(move |event, window, cx| {
                        if window.window_handle() == main_window
                            && is_windows_close_shortcut(&event.keystroke)
                            && post_windows_close(window)
                        {
                            cx.stop_propagation();
                        }
                    })
                    .detach();
                    let desktop = self.clone();
                    window
                        .subscribe(
                            &workspace,
                            cx,
                            move |workspace, _: &QuitConfirmed, window, cx| {
                                desktop.finish_windows_main_close(&workspace, window, cx);
                            },
                        )
                        .detach();
                }
                let workspace = workspace.downgrade();
                let desktop = self.clone();
                window.on_window_should_close(cx, move |window, cx| {
                    let Some(workspace) = workspace.upgrade() else {
                        return true;
                    };
                    if should_confirm_main_window_close(
                        desktop.probe,
                        workspace.read(cx).is_quit_confirmed(),
                    ) {
                        workspace.update(cx, |view, cx| view.open_quit_confirmation(window, cx));
                        return false;
                    }
                    desktop.finish_windows_main_close(&workspace, window, cx);
                    true
                });
            }
            #[cfg(target_os = "macos")]
            {
                let workspace = workspace.downgrade();
                let desktop = self.clone();
                window.on_window_should_close(cx, move |window, cx| {
                    if desktop.quitting.get() {
                        return !macos::termination_pending();
                    }
                    if let Err(error) = workspace.update(cx, |view, cx| view.flush_geometry(cx)) {
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
            if self.probe.is_none()
                && let Some(visibility_probe) = window_diag
            {
                macos::window_diag::window_created(window, cx, visibility_probe);
            }
            workspace
        })
    }

    fn focus(&self, window: WindowHandle<Workspace>, cx: &mut App) {
        if self.probe.is_some() {
            eprintln!(
                "[instance-probe] focus windows={} id={:?}",
                cx.windows().len(),
                window.window_id()
            );
            return;
        }
        cx.activate(true);
        if let Err(error) = window.update(cx, |_, window, _| window.activate_window()) {
            eprintln!("Reactivate native window: {error:#}");
        }
    }

    fn dispatch(self: &Rc<Self>, requests: &mut WindowRegistry, cx: &mut App) -> bool {
        if self.quitting.get() || !requests.has_requests() {
            return false;
        }
        let commands = self.runtime.borrow().commands.clone();
        let (generation, phase) = commands.connection();
        if generation == 0 || matches!(phase, ConnectionPhase::Starting) {
            return true;
        }
        let ready = phase == ConnectionPhase::Ready;
        if requests.open_window(ready) && !self.ensure_window(cx) {
            // Let a later user request retry a failed native window creation.
            return false;
        }
        let focus = requests.take_focus();
        let Some(window) = self.live_window(cx) else {
            if self.probe.is_some() && focus {
                eprintln!("[instance-probe] no focus windows=0");
            }
            return requests.next_path().is_some();
        };
        if focus {
            self.focus(window, cx);
        }
        if ready {
            while let Some(path) = requests.next_path() {
                if commands
                    .send_at(
                        generation,
                        Command::CreateWorkspace {
                            root_directory: path.to_owned(),
                        },
                    )
                    .is_err()
                {
                    // The Runtime changed generation after the snapshot. Retry on Ready.
                    break;
                }
                if self.probe.is_some() {
                    eprintln!(
                        "[instance-probe] dispatch ready generation={generation} path={path}"
                    );
                }
                requests.path_dispatched();
            }
        }
        requests.next_path().is_some()
    }
}

async fn run_requests(
    desktop: Rc<Desktop>,
    inbox: async_channel::Receiver<OpenRequest>,
    paths: Vec<String>,
    cx: &mut AsyncApp,
) {
    let mut registry = WindowRegistry::default();
    let mut batch = vec![OpenRequest::Directories(paths)];
    while !desktop.quitting.get() {
        while let Ok(request) = inbox.try_recv() {
            batch.push(request);
        }
        if !batch.is_empty() {
            // Checking a directory may touch a mounted volume. Keep it off the UI thread.
            let requests = std::mem::take(&mut batch);
            let validated = cx
                .background_executor()
                .spawn(async move {
                    requests
                        .into_iter()
                        .map(OpenRequest::existing_directories)
                        .collect::<Vec<_>>()
                })
                .await;
            for request in validated {
                registry.enqueue(request);
            }
        }
        match cx.update(|cx| desktop.dispatch(&mut registry, cx)) {
            Ok(true) => {
                // Sleep only while a request is waiting for the Runtime to become writable.
                cx.background_executor()
                    .timer(Duration::from_millis(40))
                    .await;
            }
            Ok(false) => {
                let Ok(request) = inbox.recv().await else {
                    break;
                };
                batch.push(request);
            }
            Err(_) => break,
        }
    }
}

fn listen_for_probe_quit(cx: &App) {
    let (send, wait) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = io::stdin().read_line(&mut String::new());
        let _ = send.try_send(());
    });
    cx.spawn(async move |cx| {
        if wait.recv().await.is_ok() {
            let _ = cx.update(|cx| cx.quit());
        }
    })
    .detach();
}

fn set_app_menu(locale: InterfaceLocale, cx: &mut App) {
    cx.set_menus(vec![Menu {
        name: "Matou Native".into(),
        items: vec![MenuItem::action(
            locale.text("退出 Matou Native", "Quit Matou Native"),
            Quit,
        )],
    }]);
}
