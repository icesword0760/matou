# Matou native workspace

Matou builds as a Rust workspace on macOS and Windows. The GPUI desktop draws
windows and terminal sessions. The runtime owns session processes and platform
PTYs, while core persists workspace metadata in SQLite and terminal output in a
segmented journal. The terminal crate embeds `libghostty-vt`. The Python
account sidecar and Go model router are built separately for release packaging.

## Prepare a checkout

Run the commands from the repository root. Install stable Rust with the host
target, Git, curl, Python 3.12+ and Zig 0.17.0. macOS needs the Xcode Command
Line Tools. Windows needs the MSVC C++ build tools and Windows SDK.

On Apple Silicon:

```sh
python3 apps/native/scripts/prepare-gpui.py
python3 apps/native/terminal/scripts/build-libs.py --target aarch64-apple-darwin
cargo fetch --manifest-path apps/native/Cargo.toml --locked
```

On Intel Macs use `x86_64-apple-darwin`. In PowerShell on Windows x64:

```powershell
python apps/native/scripts/prepare-gpui.py
python apps/native/terminal/scripts/build-libs.py --target x86_64-pc-windows-msvc
$ghostty = Join-Path $PWD 'apps/native/terminal/vendor/ghostty-vt/windows-x86_64/bin'
$env:PATH = "$ghostty;$env:PATH"
cargo fetch --manifest-path apps/native/Cargo.toml --locked
```

On Windows ARM64 use `aarch64-pc-windows-msvc` and `windows-aarch64/bin` with
the matching toolchain. The preparation scripts verify GPUI 0.2.2 and
libghostty-vt 1.0.1 before applying the checked-in adaptations. The terminal
build records its inputs in `matou-build.json` and uses Matou's Unicode 6 width
policy. macOS links a static archive. Windows loads the matching DLL.

## Build and test

```sh
pnpm build:native
cargo build --manifest-path apps/native/Cargo.toml --offline --locked -p matou-native-runtime
pnpm test:native
```

The debug runtime build provides the executable used by the process tests.
The macOS `pnpm test` entry also runs naming, Node tooling and Python account
sidecar tests. Use `pnpm check:native-clean-install` for the focused runtime gate
in the [clean-install plan](../../docs/superpowers/plans/2026-11-26-native-clean-install-plan.md).

Checked-in fixtures live in [core](core/tests/fixtures),
[desktop](desktop/tests/fixtures), [runtime](runtime/tests/fixtures) and
[terminal](terminal/tests/fixtures). Release test fixtures are under
[tooling/packaging/fixtures](../../tooling/packaging/fixtures). The original
fixture exporters remain in the local tag `archive/electron-0.2-final`.
Keep reference fixtures independent of the implementation under test.

## Launch an isolated macOS build

```sh
python3 tooling/native/launch-native.py --binary apps/native/target/release/matou-native-desktop
```

The launcher uses the sibling `matou-native-runtime` binary. It creates the
ad-hoc signed bundle `artifacts/native-parity/native/MatouNative.app` and
reuses a separate test data directory on subsequent launches. Quit that app
before refreshing the bundle. With `CARGO_TARGET_DIR` set, pass the binary
path from that directory.

## Package native releases

Release metadata and signing defaults live in
[release-config.json](release-config.json). The packagers are in
[tooling/packaging](../../tooling/packaging).

- macOS uses `pnpm package:mac`. Run `pnpm accounts:build` and
  `pnpm router:build` to stage matching sidecars in
  `apps/native/package-resources/`.
- Windows uses `pnpm package:win --model-router <directory>`. Run
  `pnpm accounts:build` on the matching architecture to stage the account
  engine in `tooling/claude-accounts/dist/`. Pass the matching router output
  printed by `pnpm router:build`.

See the [release guide](../../docs/release/cloud-updates.md) for artifact names,
updater feeds and signing. Native license files are under [third-party](third-party).
