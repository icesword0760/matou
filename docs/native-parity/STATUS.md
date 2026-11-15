# Native parity execution state

## Contract and baseline

Full user objective: OBJECTIVE.md. It supersedes the old external-reference CLI-only scope: this migration covers the ENTIRE existing Matou. GPUI + libghostty-vt + Rust; Mac first, Windows required for final completion. No production install, merge or publication. Minimal equivalent implementation and sufficient validation rules apply to every task.

Branch: codex/native-parity. Workspace: /Users/icesword/Documents/AIProjects/matou-native-parity.
Original main and native-dual-engine-lab remain untouched.

Captured 2026-11-15 11:27 +08:00. Reference installed package: 0.2.19; local immutable capture artifacts/native-parity/baseline/MatouReference.app. Original source HEAD: 26580359f55125492bfc0d7df608aec291a59af3. Full working-tree patch and untracked snapshot preserved under the ignored baseline directory. No credentials or user databases copied.

Installed ASAR SHA256: 97f1c60e17cf99253c9feb1d907addf86e6b91d35bb54ad7b84ccd90523ee37d.
Main bundle and all eight inspected main/renderer asset entries match existing local build output. This alone does not prove the entire build corresponds to HEAD.
Runtime sourcemap proves three installed source files exactly match uncommitted source and do NOT match HEAD: claude-model-routing.ts, claude-request-gateway.ts, deepseek-inline-tools.ts. Their captured sources and associated tests are therefore included in this branch as baseline, not newly implemented migration work. Root resolved the read-only inventory's initial suggestion to treat them as future increments using this package evidence.
Evidence: baseline/manifest.json, package-source-comparison.json, runtime-source-comparison.json. Full runtime sourcemap audit subsequently compared 196 repository source entries across index/CLI/supervisor maps: all match captured branch (runtime-full-source-audit.json). Desktop source correspondence is now proven by independent offline rebuild at b4160b7e1534b2f2006b1522b8cd9d29229792a2 with frozen lockfile: all 10 packaged out/ files (main, preload, renderer JS/CSS/HTML and five images) exactly match rebuilt bytes, no extra/missing files. See desktop-rebuild-audit.json and build logs. This does not audit original signing/pipeline. Package/source capture is closed; scenario and performance baseline is not yet fully frozen.

## Isolated reference launch and observed constraints

Launcher: tooling/native-parity/launch-reference.py. First preparation requires --asar-module pointing at an existing @electron/asar lib/asar.js (current installed tool: /Users/icesword/Documents/AIProjects/matou/node_modules/.pnpm/@electron+asar@3.4.1/node_modules/@electron/asar/lib/asar.js). Later launches reuse environment.json; check launch.json PID before starting, do not create duplicate instances. Current diagnostic reference PID 56861 (--debug-port 9446 --locale system); process must be revalidated. Previous PID 87615 genuinely quit before restart. Loopback binding confirmed with lsof. Diagnostic port only enabled explicitly; --locale system removes MATOU_LOCALE so preference switching can be tested. Ordinary screenshot runs default to fixed zh-CN.

Data/home/user-data/workspace are independent directories under the recorded temporary root. Original repository path exceeded macOS Unix socket path limits; that failed instance was terminated, then a shorter temporary root succeeded. No E2E mock or altered-close mode enabled. Auto updates disabled; feed points to localhost port 9. Reference identity com.matou.parity-reference; bundle executable/name retained because Electron helper lookup depends on CFBundleName.

Important isolation correction: Electron app.getPath('home') ignored HOME on this Mac. First successful startup reinstalled ~/Library/Services/进入码头.workflow; its complete file content was verified equal to the original installed bundle resource. No content change, but mtime changed at 11:32:18. This is a recorded harness side effect, not proof of untouched system metadata. Subsequent runs use a one-line test-harness injection app.setPath('home', process.env.MATOU_REFERENCE_HOME) before app setup. Frozen original remains unchanged. Running reference renderer/runtime are unchanged; main differs only by that line. Repacked ASAR integrity updated normally and app signed ad-hoc. Finder workflow now exists in isolated home; original mtime unchanged across subsequent runs. See home-harness.json. Native-vs-reference platform integration must account for this isolation; it is not a product feature modification.

Evidence directory: artifacts/native-parity/reference/evidence. At 2400x1560 screenshot pixels (1200x780 logical window), first-launch and settings repeated screenshots each have ZERO changed pixels (reference-repeat-diff.json). No broad antialias tolerance or screenshot mask introduced. These are repeatability observations, not native parity results. Active/inactive window, cursor, background and focus must be matched per screenshot.

## Task DAG and ownership

| Node | Dependencies | Owner / write scope | Result and gate | State |
| --- | --- | --- | --- | --- |
| B01 Capture package/source | none | root / baseline artifacts, source snapshot | frozen bytes and proven mapping | done; runtime 196 source entries and independent desktop 10/10 byte reproduction |
| B02 Inventory actual entries | B01 source | root integrates read-only audit / MATRIX.md | every capability traced to UI/code/test | 17 groups identified; concrete observed scenarios recorded, remaining states pending |
| B03 Isolated reference app | B01 | root / reference launch tooling | separate home/data/updates; path-only harness declared | running and observed |
| B04 Reference scenarios and visual/perf baseline | B02,B03 | root / evidence and matrix | repeatable screenshots, operations, budgets | expanded reference scenarios captured; first real renderer trace captured; per-scenario budgets and remaining scope pending |
| N01 Minimal native boundaries | B02,B04 | root / architecture decision in this file | state/storage/PTY/view ownership and first slice specified | pending |
| N02 Mac first vertical slice | N01 | assigned implementation agents, disjoint files | actual shell, persistence, restore + reference comparison | pending |
| N03 Mac remaining capability groups | N02 and explicit group dependencies | at most two implementers | each matrix scenario closes without regressions | pending |
| M01 Mac package acceptance | all Mac scenarios | root | all required Mac evidence current | pending |
| W01 Windows parity | M01; early compile checks during N02/N03 | root + assigned implementers | corresponding Windows baseline and full actual validation | pending |
| F01 Final audit | M01,W01 | root | objective clause-by-clause completion | pending |

No implementation task is released before root fixes its requirements, path and verification. Read-only inventory agent native_stack_evidence finished. Reuse existing agents; current user AGENTS delegation/provider rules apply. Shared docs owned by root. No new runtime abstraction justified yet.

## Next concrete actions

1. Resolve observed Shell restore discrepancy: completed command blocks exist in isolated DB, but two genuine quit/reopen captures show only new prompt, contrary to source/test expectations. Read-only advisor ruled out missing blocks/wrong active session/preference disabled. Source has restored-history then replay reset path, not traced live. USER APPROVED 2026-11-15: restore completed command history according to existing tests, record as approved deviation D01. Original installed app unchanged. This unblocks native restore semantics; do not reproduce observed blank-prompt bug.
2. Continue reference capture: resize/card motion/drag, Agent workflows, Git/worktrees, provider forms and errors, multiwindow/platform paths, CLI and recovery/update. Last-tab protection, main close/process retention, independent terminal return, theme toggle, search, DAG navigation, loader empties, notifications, board and language switching now have actual evidence. Current isolated state: only 默认 task, one canvas, two Shell nodes; second is focused. 原生对照 and extra canvas were removed through product confirmations in test data.
3. Capture applicable performance baseline and budgets before implementation; do not label screenshot acquisition delay as input/render latency. New optional CDP launcher and trace-reference.mjs run against real package, input still via CUA. First 45-second mixed trace is diagnostic only, not frozen budgets. Run node tooling/native-parity/trace-reference.mjs <seconds> then real UI actions; summarize with summarize-trace.py <trace-dir>. Existing scale fixtures use E2E mode and cannot substitute for this baseline. Need repeatable startup/target-ready, resize/scroll/history/sustained-output samples and visible-time evidence.
4. Define first native vertical slice contract (bootstrap + real Shell + canvas + durable structure) using observed and source contracts. Source advisor findings are recorded in MATRIX.md. No implementation delegate released yet.

## Current verification

New reference harness runs actual captured package with isolated data and path-only main injection. Concrete actual observations are in MATRIX.md. Structure/names survive true quit; completed command visual restoration is approved difference D01, not an unresolved product choice. Native product implementation has not started; performance diagnostics have started but acceptance/budgets remain open. Offline dependency install, package and desktop rebuild passed, desktop bytes match frozen package 10/10. Python/Node harness syntax, git diff --check and identifier gate pass after consolidation. A consistent read-only SQLite backup of the final isolated state is evidence/baseline-after-expanded-scenarios.sqlite (database evidence only; not a standalone journal/restore fixture). Goal remains active.


## Approved difference D01

User explicitly chose: 按已有测试恢复已完成命令历史，登记为批准差异（推荐）. Native ordinary Shell must preserve existing block collection/retention, reopen with new PTY and restore completed command blocks visibly. Do not restore interrupted commands as completed history. Reference observed blank prompt remains recorded as baseline defect; do not change reference or original product. Validation: existing prd-04-session-recovery behavior plus actual native quit/reopen and visible marker.
