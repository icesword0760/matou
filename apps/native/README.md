# Matou native workspace

This workspace implements the frozen Matou behavior in Rust. It currently contains
the SQLite persistence, initial window hierarchy and live Shell runtime, not a runnable
desktop application. Product scope and acceptance remain in
`docs/native-parity/OBJECTIVE.md`, `STATUS.md` and `MATRIX.md` at the repository root.

## Current modules

- `core::storage`: frozen schema migration, atomic domain mutation/events/receipt,
  deduplicated command replay and request conflict detection.
- `core::hierarchy`: default workspace/task/canvas/Shell creation and persisted
  window navigation readback. The runtime caller will supply resolved paths,
  generated identities, locale messages and timestamps.
- `terminal`: thread-affine libghostty-vt parser, owned screen cells, frozen
  light/dark themes, cursor/soft-wrap metadata, scrollback, PTY replies and
  synchronized-output hold. The caller owns the PTY and displayed-frame timing.
- `core::shell_history`: completed-command stream collector, existing SQLite
  block retention and restoration text. The runtime supplies IDs, timestamps,
  the restoration preference and localized divider text.
- `core::ownership`: atomic data-directory owner publication, live-process
  exclusion, stale takeover and generation-safe release before opening SQLite.
- `core::backup` and `core::recovery_marker`: SQLite online backups, validated
  retention/restore, stable recovery generations and durable resolution records.
- `core::startup`: owner-first inspection, pre-migration backups, read-only
  fallback and corruption/WAL recovery state. It opens only the caller-supplied
  data root; the app entry point and recovery actions remain to be connected.
- `core::sessions`: ordinary-Shell creation, run lifecycle, title/cwd updates
  and archive transactions, preserving existing events and replay receipts.
- `core::journal` and `core::durability`: ordered segmented output, indexes,
  compression, recovery and bounded storage-fault pause/retry.
- `core::pty`: real platform PTY launch, environment, resize, process control
  and exit status, including the existing zsh integration.
- `runtime::session` and `runtime::worker`: real Shell/terminal owner threads,
  incremental decoding/batching, persisted event delivery, credited log replay,
  terminal replies and shutdown. `WorkerCommand` carries typed reply channels
  so the central owner need not block on another session's disk or PTY.
- `core::session_graph`: existing graph projection and work-status transactions,
  including domain events and replay receipts.
- `runtime::derived`: source-compatible Shell OSC state/cwd tracking and the
  existing four-line graph summary filters.
- `runtime::owner`: consumes the writable startup connection/lease, interrupts
  previous-generation runs, records real PTY runs, and serializes completed
  history, cwd, work status and graph summaries. `OwnerEvent::Domain` exposes
  committed event ranges; saved output/exit callbacks are independent of display
  credit and are never rerun during display replay. Worker run identities reject
  late events after relaunch. This owner runs off the UI thread.

The application must connect startup results and recovery choices to the runtime
owner and views. No current entry point opens production data. Clean-exit backup,
application process/IPC integration, GPUI views, native packaging
and full platform acceptance are still pending.

## Check the implemented contracts

From the repository root with Rust/Cargo installed:

```sh
cargo test --manifest-path apps/native/Cargo.toml -p matou-native-core
cargo test --manifest-path apps/native/Cargo.toml -p matou-native-runtime
cargo fmt --manifest-path apps/native/Cargo.toml --all --check
```

The terminal library is built from the pinned libghostty-vt 1.0.1 release with
Zig 0.17.x. Its generated width table uses the frozen renderer's Unicode 6
policy: the upstream default counts some characters differently (for example,
🙂), changing wrapping and cursor positions. Original PTY bytes are preserved.
Upstream Unicode/grapheme properties other than width remain unchanged.

```sh
python3 apps/native/terminal/scripts/build-libs.py --zig /absolute/path/to/zig
cargo test --locked --manifest-path apps/native/Cargo.toml -p matou-native-terminal
```

`build-libs.py` verifies the pinned source/dependency archives, patches only the
width lookup generator in an ignored copy, and records its inputs in the
installed library's `matou-build.json`. Windows cross-builds use
`--target x86_64-pc-windows-msvc`; Mac Intel uses `--target x86_64-apple-darwin`.
macOS links a uniquely named static archive; Windows uses the matching DLL.
Do not replace this library with unmodified upstream release binaries.
`tooling/native-parity/export-terminal-width.mjs` reproduces the checked-in
width ranges from the frozen JS dependency; it is a development exporter and
is not part of the native product runtime. MIT attribution is retained beside
the generated table and installed library.

Tests use temporary databases and fixtures emitted by the frozen TypeScript
services on the captured Electron runtime. Fixture exporters live in
`tooling/native-parity/export-storage-contract.ts` and
`tooling/native-parity/export-bootstrap-contract.ts`. The final native product
will not run those exporters or require the old Node/Electron runtime.

Do not regenerate expected fixtures from Rust, or regenerate the frozen source
contract merely to make a failing native test pass.
