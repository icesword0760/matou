use std::{env, fs, path::PathBuf};

fn main() {
    let target = env::var("TARGET").expect("Cargo supplies TARGET");
    let root = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("Cargo supplies CARGO_MANIFEST_DIR"),
    );
    let (platform, link) = match target.as_str() {
        "aarch64-apple-darwin" => ("macos-aarch64", "static"),
        "x86_64-apple-darwin" => ("macos-x86_64", "static"),
        "x86_64-pc-windows-msvc" => ("windows-x86_64", "dylib"),
        "aarch64-pc-windows-msvc" => ("windows-aarch64", "dylib"),
        _ => panic!("No libghostty-vt package configured for {target}"),
    };
    let package = root.join("vendor/ghostty-vt").join(platform);
    let header = package.join("include");
    let lib = package.join("lib");
    println!("cargo:rerun-if-changed=native/ghostty_shim.c");
    println!("cargo:rerun-if-changed={}", package.display());
    assert!(
        header.join("ghostty/vt.h").is_file(),
        "Missing libghostty-vt for {target}. Run python3 scripts/build-libs.py --target {target}."
    );
    let mut shim = cc::Build::new();
    shim.file("native/ghostty_shim.c")
        .include(header)
        .std("c11");
    if link == "static" {
        shim.define("GHOSTTY_STATIC", None);
    }
    shim.compile("matou_ghostty_shim");
    println!("cargo:rustc-link-search=native={}", lib.display());
    let library = if link == "static" {
        "matou-ghostty-vt"
    } else {
        "ghostty-vt"
    };
    println!("cargo:rustc-link-lib={link}={library}");

    if link == "dylib" {
        let out = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo supplies OUT_DIR"));
        let profile = out.ancestors().nth(3).expect("Cargo profile directory");
        let dependencies = profile.join("deps");
        fs::create_dir_all(&dependencies).expect("create Cargo test executable directory");
        for executable_dir in [profile, dependencies.as_path()] {
            fs::copy(
                package.join("bin/ghostty-vt.dll"),
                executable_dir.join("ghostty-vt.dll"),
            )
            .expect("copy the Ghostty VT DLL beside local executables");
        }
    }
}
