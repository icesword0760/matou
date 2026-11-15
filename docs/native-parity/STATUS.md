# Native parity execution state

## Contract and baseline

Full user objective: OBJECTIVE.md. It supersedes the old external-reference CLI-only scope: this migration covers the ENTIRE existing Matou. GPUI + libghostty-vt + Rust; Mac first, Windows required for final completion. No production install, merge or publication. Minimal equivalent implementation and sufficient validation rules apply to every task.

Branch: codex/native-parity. Workspace: /Users/icesword/Documents/AIProjects/matou-native-parity.
Original main and native-dual-engine-lab remain untouched.

Captured 2026-11-15 11:27 +08:00. Reference installed package: 0.2.19; local immutable capture artifacts/native-parity/baseline/MatouReference.app. Original source HEAD: 26580359f55125492bfc0d7df608aec291a59af3. Full working-tree patch and untracked snapshot preserved under the ignored baseline directory. No credentials or user databases copied.

Installed ASAR SHA256: 97f1c60e17cf99253c9feb1d907addf86e6b91d35bb54ad7b84ccd90523ee37d.
Main bundle and all eight inspected main/renderer asset entries match existing local build output. This alone does not prove the entire build corresponds to HEAD.
Runtime sourcemap proves three installed source files exactly match uncommitted source and do NOT match HEAD: claude-model-routing.ts, claude-request-gateway.ts, deepseek-inline-tools.ts. Their captured sources and associated tests are therefore included in this branch as baseline, not newly implemented migration work. Root resolved the read-only inventory's initial suggestion to treat them as future increments using this package evidence.
Evidence: baseline/manifest.json, package-source-comparison.json, runtime-source-comparison.json. Remaining package/source correspondence still needs audit. Baseline is CAPTURED, not yet fully validated/frozen.

## Isolation before reference launch

Do not launch captured bundle until environment isolation is ready. Existing supported overrides: ELECTRON_USER_DATA_DIR, MATOU_DATA_DIR, MATOU_DEFAULT_WORKSPACE, MATOU_DISABLE_AUTO_UPDATE=1, MATOU_UPDATE_BASE_URL (local test endpoint). Packaged startup also calls installFinderQuickAction using app.getPath('home'); verify isolated HOME applies before launch, otherwise isolate that platform integration without changing product behavior. Never write the original home Services directory or use real account sessions. E2E mode changes close behavior and is not automatically the visual baseline.

## Task DAG and ownership

| Node | Dependencies | Owner / write scope | Result and gate | State |
| --- | --- | --- | --- | --- |
| B01 Capture package/source | none | root / baseline artifacts, source snapshot | frozen bytes and proven mapping | partial; package captured, source map audit pending |
| B02 Inventory actual entries | B01 source | root integrates read-only audit / MATRIX.md | every capability traced to UI/code/test | 17 groups identified; UI not yet observed |
| B03 Isolated reference app | B01 | root / reference launch tooling | separate home/data/updates; unchanged app behavior | next |
| B04 Reference scenarios and visual/perf baseline | B02,B03 | root / evidence and matrix | repeatable screenshots, operations, budgets | pending |
| N01 Minimal native boundaries | B02,B04 | root / architecture decision in this file | state/storage/PTY/view ownership and first slice specified | pending |
| N02 Mac first vertical slice | N01 | assigned implementation agents, disjoint files | actual shell, persistence, restore + reference comparison | pending |
| N03 Mac remaining capability groups | N02 and explicit group dependencies | at most two implementers | each matrix scenario closes without regressions | pending |
| M01 Mac package acceptance | all Mac scenarios | root | all required Mac evidence current | pending |
| W01 Windows parity | M01; early compile checks during N02/N03 | root + assigned implementers | corresponding Windows baseline and full actual validation | pending |
| F01 Final audit | M01,W01 | root | objective clause-by-clause completion | pending |

No implementation task is released before root fixes its requirements, path and verification. Read-only inventory agent native_stack_evidence finished. Reuse existing agents; current user AGENTS delegation/provider rules apply. Shared docs owned by root. No new runtime abstraction justified yet.

## Next concrete actions

1. Audit packaged runtime source map against the captured branch; preserve any additional installed-only code as baseline rather than guessing.
2. Prepare isolated reference launch, check Finder integration/home, account and update effects; observe real empty/startup scene.
3. Expand A01/A02/A07 first into concrete scenario rows; capture stable reference images twice before setting pixel-diff noise rules.
4. Establish the minimal first vertical implementation contract only after this evidence.

## Current verification

No new native product implementation or visual acceptance yet. Only source/package inspection and baseline capture are complete. Identifier gate initially saw a vendored binary inside the local app snapshot; local bundles/data are now ignored rather than changing the repository checker. No goal completion claim.
