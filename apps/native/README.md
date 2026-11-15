# Matou native workspace

This workspace implements the frozen Matou behavior in Rust. It currently contains
the SQLite persistence and initial window hierarchy foundations, not a runnable
desktop application. Product scope and acceptance remain in
`docs/native-parity/OBJECTIVE.md`, `STATUS.md` and `MATRIX.md` at the repository root.

## Current modules

- `core::storage`: frozen schema migration, atomic domain mutation/events/receipt,
  deduplicated command replay and request conflict detection.
- `core::hierarchy`: default workspace/task/canvas/Shell creation and persisted
  window navigation readback. The runtime caller will supply resolved paths,
  generated identities, locale messages and timestamps.

The connection caller owns data-directory locking, pre-migration backups and the
recovery interface. No current entry point opens production data. Runtime/PTY,
GPUI views, native packaging and platform acceptance are still pending.

## Check the implemented contracts

From the repository root with Rust/Cargo installed:

```sh
cargo test --manifest-path apps/native/Cargo.toml -p matou-native-core
cargo fmt --manifest-path apps/native/Cargo.toml --check
```

Tests use temporary databases and fixtures emitted by the frozen TypeScript
services on the captured Electron runtime. Fixture exporters live in
`tooling/native-parity/export-storage-contract.ts` and
`tooling/native-parity/export-bootstrap-contract.ts`. The final native product
will not run those exporters or require the old Node/Electron runtime.

Do not regenerate expected fixtures from Rust, or regenerate the frozen source
contract merely to make a failing native test pass.
