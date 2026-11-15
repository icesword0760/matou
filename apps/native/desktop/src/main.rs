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
    Application::new().with_assets(NativeAssets).run(move |cx| {
        // AppKit can terminate the process without returning from Application::run.
        // Drain the real session owner before that happens; its PTY shutdown has
        // the existing bounded grace/kill deadlines. GPUI's async quit grace is
        // only 100ms, so it cannot own this durability boundary.
        cx.on_app_quit(move |_| {
            if let Err(error) = quit_runtime.borrow_mut().shutdown() {
                eprintln!("Close native runtime: {error:#}");
            }
            async {}
        })
        .detach();
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
                cx.new(|cx| Workspace::new(commands, events, window, cx))
            },
        ) {
            Ok(_) => cx.activate(true),
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
