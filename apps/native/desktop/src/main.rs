#[cfg(target_os = "macos")]
use std::cell::Cell;
use std::{cell::RefCell, path::PathBuf, rc::Rc};

use anyhow::{Context, Result, bail};
use gpui::*;
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
    let main_window = Rc::new(Cell::new(None::<WindowHandle<Workspace>>));
    #[cfg(target_os = "macos")]
    application.on_reopen({
        let main_window = main_window.clone();
        move |cx| {
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
        let quitting = Rc::new(Cell::new(false));
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
        cx.set_menus(vec![Menu {
            name: "Matou Native".into(),
            items: vec![MenuItem::action("退出 Matou Native", Quit)],
        }]);
        let bounds = Bounds::centered(None, size(px(1200.), px(780.)), cx);
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
                if let Err(error) = matou_native_desktop::macos::apply_sidebar_material(window) {
                    eprintln!("Apply native sidebar material: {error:#}");
                }
                let workspace = cx.new(|cx| Workspace::new(commands, events, window, cx));
                #[cfg(target_os = "macos")]
                {
                    let workspace = workspace.downgrade();
                    let quitting = quitting.clone();
                    window.on_window_should_close(cx, move |window, cx| {
                        if quitting.get() {
                            return true;
                        }
                        if let Err(error) = workspace.update(cx, |view, cx| view.flush_geometry(cx))
                        {
                            eprintln!("Flush native window geometry before hiding: {error:#}");
                            return false;
                        }
                        if let Err(error) = matou_native_desktop::macos::hide_window(window) {
                            eprintln!("Hide native window: {error:#}");
                        }
                        false
                    });
                }
                workspace
            },
        ) {
            Ok(window) => {
                #[cfg(target_os = "macos")]
                main_window.set(Some(window));
                cx.on_app_quit(move |cx| {
                    #[cfg(target_os = "macos")]
                    quitting.set(true);
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
    });
    runtime.borrow_mut().shutdown()
}

fn options() -> Result<LaunchOptions> {
    let mut data_root = None;
    let mut workspace = None;
    let mut shell_home = None;
    let mut shell = PathBuf::from(if cfg!(windows) {
        "C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"
    } else {
        "/bin/zsh"
    });
    let mut args = std::env::args_os().skip(1);
    while let Some(flag) = args.next() {
        let value = args
            .next()
            .context("native acceptance launcher requires a path after each flag")?;
        let path = PathBuf::from(value);
        match flag.to_str() {
            Some("--data-root") => data_root = Some(path),
            Some("--workspace") => workspace = Some(path),
            Some("--shell-home") => shell_home = Some(path),
            Some("--shell") => shell = path,
            _ => bail!("Unknown native launcher option: {}", flag.to_string_lossy()),
        }
    }
    let options = LaunchOptions {
        data_root: data_root.context("Pass --data-root with an isolated native data directory")?,
        workspace: workspace.context("Pass --workspace with an isolated working directory")?,
        shell_home: shell_home.context("Pass --shell-home with an isolated Shell home")?,
        shell,
    };
    for path in [
        &options.data_root,
        &options.workspace,
        &options.shell_home,
        &options.shell,
    ] {
        if !path.is_absolute() {
            bail!("Native launcher paths must be absolute: {}", path.display());
        }
    }
    Ok(options)
}
