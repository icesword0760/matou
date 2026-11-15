# Native parity execution state

## Contract and baseline

Full user objective: OBJECTIVE.md. It supersedes the old external-reference CLI-only scope: this migration covers the ENTIRE existing Matou. GPUI + libghostty-vt + Rust; Mac first, Windows required for final completion. No production install, merge or publication. Minimal equivalent implementation and sufficient validation rules apply to every task.

Branch: codex/native-parity. Workspace: /Users/icesword/Documents/AIProjects/matou-native-parity.
Original main and native-dual-engine-lab remain untouched.

Captured 2026-11-15 11:27 +08:00. Reference installed package: 0.2.19; local immutable capture artifacts/native-parity/baseline/MatouReference.app. Original source HEAD: 26580359f55125492bfc0d7df608aec291a59af3. Full working-tree patch and untracked snapshot preserved under the ignored baseline directory. No credentials or user databases copied.

Installed ASAR SHA256: 97f1c60e17cf99253c9feb1d907addf86e6b91d35bb54ad7b84ccd90523ee37d.
Main bundle and all eight inspected main/renderer asset entries match existing local build output. This alone does not prove the entire build corresponds to HEAD.
Runtime sourcemap proves three installed source files exactly match uncommitted source and do NOT match HEAD: claude-model-routing.ts, claude-request-gateway.ts, deepseek-inline-tools.ts. Their captured sources and associated tests are therefore included in this branch as baseline, not newly implemented migration work. Root resolved the read-only inventory's initial suggestion to treat them as future increments using this package evidence.
Evidence: baseline/manifest.json, package-source-comparison.json, runtime-source-comparison.json. Full runtime sourcemap audit subsequently compared 196 repository source entries across index/CLI/supervisor maps: all match captured branch (runtime-full-source-audit.json). Renderer/main source correspondence beyond matching build output still needs audit. Baseline is CAPTURED, not yet fully validated/frozen.

## Isolated reference launch and observed constraints

Launcher: tooling/native-parity/launch-reference.py. First preparation requires --asar-module pointing at an existing @electron/asar lib/asar.js (current installed tool: /Users/icesword/Documents/AIProjects/matou/node_modules/.pnpm/@electron+asar@3.4.1/node_modules/@electron/asar/lib/asar.js). Later launches reuse environment.json; check launch.json PID before starting, do not create duplicate instances. Reference remains running, latest PID in launch.json, process must be revalidated.

Data/home/user-data/workspace are independent directories under the recorded temporary root. Original repository path exceeded macOS Unix socket path limits; that failed instance was terminated, then a shorter temporary root succeeded. No E2E mock or altered-close mode enabled. Auto updates disabled; feed points to localhost port 9. Reference identity com.matou.parity-reference; bundle executable/name retained because Electron helper lookup depends on CFBundleName.

Important isolation correction: Electron app.getPath('home') ignored HOME on this Mac. First successful startup reinstalled ~/Library/Services/进入码头.workflow; its complete file content was verified equal to the original installed bundle resource. No content change, but mtime changed at 11:32:18. This is a recorded harness side effect, not proof of untouched system metadata. Subsequent runs use a one-line test-harness injection app.setPath('home', process.env.MATOU_REFERENCE_HOME) before app setup. Frozen original remains unchanged. Running reference renderer/runtime are unchanged; main differs only by that line. Repacked ASAR integrity updated normally and app signed ad-hoc. Finder workflow now exists in isolated home; original mtime unchanged across subsequent runs. See home-harness.json. Native-vs-reference platform integration must account for this isolation; it is not a product feature modification.

Evidence directory: artifacts/native-parity/reference/evidence. At 2400x1560 screenshot pixels (1200x780 logical window), first-launch and settings repeated screenshots each have ZERO changed pixels (reference-repeat-diff.json). No broad antialias tolerance or screenshot mask introduced. These are repeatability observations, not native parity results. Active/inactive window, cursor, background and focus must be matched per screenshot.

## Task DAG and ownership

| Node | Dependencies | Owner / write scope | Result and gate | State |
| --- | --- | --- | --- | --- |
| B01 Capture package/source | none | root / baseline artifacts, source snapshot | frozen bytes and proven mapping | partial; package captured, runtime source map audit passed; UI baseline pending |
| B02 Inventory actual entries | B01 source | root integrates read-only audit / MATRIX.md | every capability traced to UI/code/test | 17 groups identified; UI not yet observed |
| B03 Isolated reference app | B01 | root / reference launch tooling | separate home/data/updates; path-only harness declared | running and observed |
| B04 Reference scenarios and visual/perf baseline | B02,B03 | root / evidence and matrix | repeatable screenshots, operations, budgets | initial scenarios captured; performance and remaining scope pending |
| N01 Minimal native boundaries | B02,B04 | root / architecture decision in this file | state/storage/PTY/view ownership and first slice specified | pending |
| N02 Mac first vertical slice | N01 | assigned implementation agents, disjoint files | actual shell, persistence, restore + reference comparison | pending |
| N03 Mac remaining capability groups | N02 and explicit group dependencies | at most two implementers | each matrix scenario closes without regressions | pending |
| M01 Mac package acceptance | all Mac scenarios | root | all required Mac evidence current | pending |
| W01 Windows parity | M01; early compile checks during N02/N03 | root + assigned implementers | corresponding Windows baseline and full actual validation | pending |
| F01 Final audit | M01,W01 | root | objective clause-by-clause completion | pending |

No implementation task is released before root fixes its requirements, path and verification. Read-only inventory agent native_stack_evidence finished. Reuse existing agents; current user AGENTS delegation/provider rules apply. Shared docs owned by root. No new runtime abstraction justified yet.

## Next concrete actions

1. Resolve observed Shell restore discrepancy: completed command blocks exist in isolated DB, but two genuine quit/reopen captures show only new prompt, contrary to source/test expectations. Read-only advisor ruled out missing blocks/wrong active session/preference disabled. Source has restored-history then replay reset path, not traced live. User decision requested asynchronously: restore completed history as intended (approved deviation) vs preserve observed blank prompt. Await actual answer; do not infer consent. Other baseline work continues.
2. Continue reference capture: last remaining tab rule, hide/show vs quit, theme, resize and focus; then remaining capability groups. Current selection is task 原生对照 with one Shell after closing a second canvas. Default task also exists with two tabs.
3. Capture applicable performance baseline and budgets before implementation; do not label screenshot acquisition delay as input/render latency.
4. Define first native vertical slice contract (bootstrap + real Shell + canvas + durable structure) using observed and source contracts. Source advisor findings are recorded in MATRIX.md. No implementation delegate released yet.

## Current verification

New reference harness runs actual captured package with isolated data and path-only main injection. First launch, Shell Unicode output, new canvas, new task, rename menu/dialog, settings navigation, close-canvas confirmation and return selection observed. Structure/names survive true quit; completed command visual restoration is unresolved. Native product implementation and performance acceptance not yet started. Identifier gate previously passed baseline snapshot; rerun for harness/docs before commit. Goal remains active.
