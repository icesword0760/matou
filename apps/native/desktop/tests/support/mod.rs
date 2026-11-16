use anyhow::{Context, Result};
use matou_native_desktop::runtime_bridge::{LaunchOptions, RuntimeBridge};
use std::path::PathBuf;

pub fn runtime_binary() -> Result<PathBuf> {
    let test = std::env::current_exe()?;
    let debug = test
        .parent()
        .and_then(|deps| deps.parent())
        .context("test debug/deps path")?;
    Ok(debug.join("matou-native-runtime"))
}

pub fn start(options: LaunchOptions) -> Result<RuntimeBridge> {
    RuntimeBridge::start_with_binary(options, &runtime_binary()?)
}
