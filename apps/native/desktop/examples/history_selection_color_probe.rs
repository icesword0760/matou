//! Compare this machine's live selection colors with a captured source fixture.
#![allow(unexpected_cfgs)] // objc 0.2 macros probe their historical cargo-clippy feature.

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    use anyhow::{Context as _, ensure};
    use cocoa::{
        base::{id, nil},
        foundation::{NSAutoreleasePool, NSString},
    };
    use gpui::rgba;
    use matou_native_desktop::macos::history_selection_background;
    use objc::{class, msg_send, sel, sel_impl};
    use serde_json::Value;

    let path = std::env::args_os()
        .nth(1)
        .context("Pass the source selection-color fixture for this system palette")?;
    let fixture: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let (ordinary, scoped, ordinary_restored, scoped_restored) = unsafe {
        let pool = NSAutoreleasePool::new(nil);
        let appearance = class!(NSAppearance);
        let initial: id = msg_send![appearance, currentAppearance];
        let initial: id = msg_send![initial, retain];
        let ordinary = [true, false].map(history_selection_background);
        let after_ordinary: id = msg_send![appearance, currentAppearance];

        let dark_name = NSString::alloc(nil).init_str("NSAppearanceNameDarkAqua");
        let dark: id = msg_send![appearance, appearanceNamed: dark_name];
        let _: () = msg_send![dark_name, release];
        let _: () = msg_send![appearance, setCurrentAppearance: dark];
        let before_scoped: id = msg_send![appearance, currentAppearance];
        let scoped = [true, false].map(history_selection_background);
        let after_scoped: id = msg_send![appearance, currentAppearance];

        let _: () = msg_send![appearance, setCurrentAppearance: initial];
        let _: () = msg_send![initial, release];
        pool.drain();
        (
            ordinary,
            scoped,
            after_ordinary == initial,
            dark != nil && before_scoped == dark && after_scoped == dark,
        )
    };

    ensure!(ordinary_restored, "the ambient appearance changed");
    ensure!(scoped_restored, "the explicit Dark Aqua appearance changed");
    for ((name, ordinary), scoped) in ["active", "inactive"].into_iter().zip(ordinary).zip(scoped) {
        let expected: [u8; 4] = serde_json::from_value(fixture["expected"][name].clone())?;
        let expected_color = rgba(u32::from_be_bytes(expected));
        let actual = ordinary?;
        assert_eq!(actual, expected_color, "{name} under the system appearance");
        assert_eq!(scoped?, expected_color, "{name} under Dark Aqua");
        println!(
            "AppKit {name}: {actual:?} matches Chromium {} for current palette input {}",
            fixture["version"], fixture["inputs"][name]
        );
    }
    println!("Both the system appearance and explicit Dark Aqua were restored.");
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn main() {}
