fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc")
    {
        return;
    }
    let manifest = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .join("windows.manifest");
    println!("cargo:rerun-if-changed={}", manifest.display());
    println!("cargo:rustc-link-arg-bin=matou-native-desktop=/MANIFEST:EMBED");
    println!(
        "cargo:rustc-link-arg-bin=matou-native-desktop=/MANIFESTINPUT:{}",
        manifest.display()
    );
}
