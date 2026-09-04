# Matou AI 结构控制最终修复报告

**日期：** 2026-09-04
**修复基线：** `33479d222aaa616b3b204c23ae6055e88226633c`
**结论：** 5 项 Important 与 2 项 Minor 均已闭合；生产修复、聚焦测试、三组 AI Host E2E、重新打包后的受影响真实 App 场景及全量门禁均通过。

## 1. 用户结果

- `child:N` 的旧投影在子节点插入、移除或顺序变化后会稳定返回 stale，不再落到另一个子节点。
- 脱出会话的 `focus` 只在目标脱出窗口已成为原生前台窗口、且该窗口中目标终端真实持有输入焦点后完成。
- Workspace / Task / Canvas 的 `switch` 遇到已脱出的活动 Session 时，仍由所属主窗口完成层级切换，同时把脱出原生窗口置前，并保持 `focusTerminal:false`。
- Fork 接受时冻结源 Session 的精确 profile、provider 配置身份、模型、权限与对话上下文；`claude-code` 枚举保持原值，Codex 使用既有 `codex` profile 与 provider 专属 `fork` 路径。
- App 重启、普通恢复与 Fork 子节点首次启动均从 Session 既有 provider binding 恢复配置；后续全局默认变化不再改写已接受配置。用户显式切换 provider 时，当前已挂载 Claude Session 会明确重绑后重启。
- 对外结果与归档证据仅保留产品字段和不可逆哈希；没有写入原始控制凭证或 provider key。

## 2. Findings 闭合映射

| Finding | 生产代码 | 测试与验收 | 闭合结果 |
|---|---|---|---|
| Important 1：关系 revision 未覆盖有序 children | `apps/runtime/src/control/host-target-revision.ts:6-18` 统一哈希稳定定位字段、`parentRef` 与有序 `childRefs`；`host-action-target-resolver.ts:136-138`、`host-control-server.ts:313` 共用同一实现。 | `host-action-target-resolver.test.ts:117-158` 分别制造插入、移除、重排；Server/Resolver 聚焦回归 62 项通过。 | 位置型 child selector 在三类顺序变化后均 fail-closed 为 stale。 |
| Important 2：脱出 focus 过早 ACK | `desktop-api.ts` / preload 增加 attempt-bound 请求与回报；`main/detached-terminal-focus-coordinator.ts:33-105` 校验 request、attempt、route、target、session、sender webContents、deadline 与原生窗口焦点；`DetachedTerminalApp.tsx:153-190,242-276` 验证目标 terminal 的 active element；`HierarchyShell.tsx:539-568,1828-1850` 在脱出与主窗口路径上等待真实终端焦点。 | Main coordinator、Detached Renderer、Hierarchy Renderer 聚焦测试共 95 项；三组 E2E 11 项；打包 App 脱出 focus 场景通过。 | `showWindow` 只负责发起置前，不再构成成功证明；错误 Renderer、错误 attempt、失焦与超时均不完成导航。 |
| Important 3：switch 的 route/target 身份矛盾 | `runtime-host-action-facade.ts:716-761` 由保存的活动 Session 解析 native `targetWindowId`，同时保留所属主窗口 `routeWindowId`；switch 继续传 `focusTerminal:false`。 | `runtime-host-action-facade.test.ts:1123-1176` 覆盖 Workspace / Task / Canvas；`ai-host-navigation.spec.ts:43-98` 与打包 App 覆盖脱出窗口。 | 三层 switch 均恢复保存路径、置前脱出原生窗口且不发起额外终端输入聚焦。 |
| Important 4：Fork 未持久继承 provider/profile/model/permission | `fork-workflow-service.ts:570-594,653-708,917-940` 在接受事务前解析并冻结 provider 上下文，使用现有 `provider_bindings.metadata_json` 写入子 Session；`provider-launch-plan.ts:38-59` 选择 Claude/Codex 专属 Fork 形式；`provider-hook-server.ts:169-266,326-397,537-562` 为 Codex 建立身份 hook 并保存精确启动设置；`runtime-server.ts:1655-1899` 对两类 AI profile 走恢复/身份确认。没有新增数据库 migration。 | Fork service 同时覆盖 `claude-code` 与 `codex`；Runtime 覆盖全局默认变化后启动 Codex Fork；Codex SessionStart 身份 fencing、专属 fork 命令与本机 CLI 配置解析均通过。 | 子节点从接受时起固定 provider 配置、模型、权限及源对话身份；Codex 不再落入 Claude 硬编码路径。 |
| Important 5：普通 restore 忽略 Session 绑定设置 | `provider-config-store.ts:125-157` 按稳定配置身份解析启动环境；`session-repository.ts:371-391,906-963` 复用/更新现有 binding 元数据；`runtime-server.ts:1674-1788` 优先读取 Session 设置，只有首启缺少设置时采用当前全局默认并补写；`runtime-server.ts:3137-3175` 在用户显式激活 provider 时重绑同一 RuntimeSessionRegistry 下的已挂载 Session。 | Provider store、首次身份登记、普通 App restore、显式 provider 激活、跨窗口身份权威回归；打包 App 在全局默认改变后重启源 Session 与 Fork 子节点。 | HUD、进程参数、endpoint、模型与权限保持一致；全局默认变化不再静默改变既有 Session。 |
| Minor 1：ready-before-input 只读一次快照 | 无生产改动。`tests/e2e/ai-host-structure-control.spec.ts:275-293` 在 `expect.poll` 每轮重读完整事件序列并验证每个 provider 的 ready 序号早于首个 input。 | 三组 E2E 最终 11/11 通过。 | 消除事件落盘时序造成的假阴性与漏检。 |
| Minor 2：identify/read 后未独立复核 UI | 无生产改动。`tests/e2e/ai-host-control-cli.spec.ts:31-71,85-105` 在 identify、send、read 每一步前后分别对比活动 Workspace、Task、Canvas、聚焦 Session、active element、滚动与通知状态。 | 三组 E2E 最终 11/11 通过。 | 三种 CLI 只读/远程操作都独立证明不移动用户当前界面与输入焦点。 |

## 3. TDD RED / GREEN 证据

所有命令均从 `/Users/icesword/Documents/AIProjects/matou/.worktrees/ai-structure-control` 执行。

### 3.1 Important 1

- **RED**
  `pnpm --filter @matou/runtime exec vitest run src/control/host-action-target-resolver.test.ts`
  结果：1 failed / 7 passed；插入 child 后旧 revision 未报 stale。
  日志：`final-fix-artifacts/logs/important-1-relation-revision-red.log`
- **GREEN**
  `pnpm --filter @matou/runtime exec vitest run src/control/host-action-target-resolver.test.ts src/control/host-control-server.test.ts`
  结果：2 files、62 passed。
  日志：`final-fix-artifacts/logs/important-1-relation-revision-green.log`

### 3.2 Important 2

- **RED**
  `pnpm --filter @matou/desktop exec vitest run src/main/detached-terminal-focus-coordinator.test.ts src/renderer/src/hierarchy/DetachedTerminalApp.test.tsx src/renderer/src/hierarchy/HierarchyShell.test.tsx`
  结果：3 files failed；coordinator 尚不存在，Detached/Main Renderer 的真实焦点断言未满足。
  日志：`final-fix-artifacts/logs/important-2-detached-focus-red.log`
- **GREEN**
  同一命令。
  结果：3 files、95 passed。
  日志：`final-fix-artifacts/logs/important-2-detached-focus-green.log`

### 3.3 Important 3

- **RED**
  `pnpm --filter @matou/runtime exec vitest run src/control/runtime-host-action-facade.test.ts -t 'routes navigation.switch.*through the detached native window'`
  结果：Workspace / Task / Canvas 三项均失败，收到的 target 仍为主窗口。
  日志：`final-fix-artifacts/logs/important-3-detached-switch-red.log`
- **GREEN**
  `pnpm --filter @matou/runtime exec vitest run src/control/runtime-host-action-facade.test.ts`
  结果：62 passed。
  日志：`final-fix-artifacts/logs/important-3-detached-switch-green.log`

### 3.4 Important 4 与 Important 5 的持久化主链

- **RED**
  `pnpm --filter @matou/runtime exec vitest run src/provider-config/provider-config-store.test.ts src/session/provider-launch-plan.test.ts src/session-canvas/fork-workflow-service.test.ts src/runtime-server.test.ts -t 'stable configuration identity|provider-specific subcommand|freezes the|bound provider configuration|accepted Codex Fork'`
  结果：6 failed / 138 skipped；稳定 provider 选择、两类 profile 冻结、Codex fork 与普通 restore 均未满足。
  日志：`final-fix-artifacts/logs/important-4-5-provider-inheritance-red.log`
- **GREEN**
  同一命令。
  结果：4 files、6 passed / 138 skipped。
  日志：`final-fix-artifacts/logs/important-4-5-provider-inheritance-green.log`

### 3.5 首启补写与 Codex 身份链

- **RED / GREEN（首启补写）**
  `pnpm --filter @matou/runtime exec vitest run src/session/provider-hook-server.test.ts -t 'persists identity from the first supported'`
  RED：1 failed / 19 skipped；GREEN：1 passed / 19 skipped。
  日志：`important-5-first-launch-settings-red.log`、`important-5-first-launch-settings-green.log`
- **RED / GREEN（Codex identity + fork adapter）**
  `pnpm --filter @matou/runtime exec vitest run src/session/provider-hook-server.test.ts src/session/provider-launch-plan.test.ts`
  RED：2 failed / 28 passed；GREEN：2 files、30 passed。
  日志：`important-4-codex-identity-red.log`、`important-4-codex-identity-green.log`
- **RED（Codex inline TOML）**
  `pnpm --filter @matou/runtime exec vitest run --testTimeout=15000 src/session/provider-launch-plan.test.ts`
  结果：1 failed / 8 passed；旧实现未传 inline hooks table。
  `pnpm --filter @matou/runtime exec vitest run --testTimeout=15000 src/session/provider-hook-server.test.ts -t 'records a fenced Codex Fork'`
  结果：1 failed / 20 skipped；hook timeout 结构未满足。
  日志：`important-4-codex-inline-hook-config-red.log`、`important-4-codex-inline-hook-registration-red.log`
- **GREEN（Codex inline TOML）**
  `pnpm --filter @matou/runtime exec vitest run --testTimeout=15000 src/session/provider-hook-server.test.ts src/session/provider-launch-plan.test.ts`
  结果：2 files、30 passed。
  日志：`important-4-codex-inline-hook-config-green.log`
- **本机 CLI 兼容探针**
  结果：Codex CLI `0.149.0` 严格配置解析通过，SessionStart hook 被观察到，attempt identity 存在，未发现 config error。
  日志：`important-4-codex-inline-hook-local-cli-compatibility.log`

### 3.6 用户显式激活 provider 与跨实例回归

- **RED / GREEN（显式重绑）**
  `pnpm --filter @matou/runtime exec vitest run src/runtime-server.test.ts -t 'explicitly rebinds an attached Claude Session'`
  RED：1 failed / 108 skipped；进程仍使用旧绑定。GREEN：1 passed / 108 skipped。
  日志：`important-5-explicit-activation-rebind-red.log`、`important-5-explicit-activation-rebind-green.log`
- **RED / GREEN（同一 registry 隔离）**
  `pnpm --filter @matou/runtime exec vitest run --testTimeout=15000 src/provider-config/provider-config-store.test.ts src/session/provider-launch-plan.test.ts src/session/provider-hook-server.test.ts src/session-canvas/fork-workflow-service.test.ts src/domain/session-repository.test.ts src/runtime-server.test.ts src/session/session-fork-intent-repository.test.ts`
  RED：1 failed / 194 passed；激活遍历到测试中另一个 registry 的残留 Server。GREEN：7 files、195 passed。
  日志：`provider-regression-final.log`、`provider-regression-final-green.log`

## 4. 整体验证

| 验证 | 精确命令 | 结果 | 日志 |
|---|---|---|---|
| 全量单元/集成 | `pnpm test` | exit 0；identifier Node tests 4 passed；Vitest 共 194 files / 1878 tests passed | `final-fix-artifacts/logs/full-pnpm-test-final.log` |
| 全仓类型 | `pnpm typecheck` | exit 0；contracts/domain/ui/runtime/desktop 全部通过 | `final-fix-artifacts/logs/full-typecheck-final.log` |
| E2E 前构建 | `pnpm build` | exit 0 | `final-fix-artifacts/logs/build-before-e2e-final.log` |
| 三组 AI Host E2E | `pnpm exec playwright test tests/e2e/ai-host-control-cli.spec.ts tests/e2e/ai-host-structure-control.spec.ts tests/e2e/ai-host-navigation.spec.ts --workers=1 --reporter=line` | 11 passed (1.4m) | `final-fix-artifacts/logs/three-ai-host-e2e-post-codex-final-green.log` |
| 重新打包 | `pnpm package:dir` | exit 0；arm64 App 生成 | `final-fix-artifacts/logs/package-dir-post-codex-final.log` |
| 受影响打包 App | `MATOU_E2E_EXECUTABLE_PATH="$PWD/apps/desktop/release/mac-arm64/码头.app/Contents/MacOS/码头" pnpm exec playwright test tests/e2e/.final-fix-packaged-acceptance.spec.ts --workers=1 --reporter=line` | 2 passed (11.1s) | `final-fix-artifacts/logs/packaged-affected-scenarios-post-codex-final.log` |
| 命名门禁 | `pnpm check:identifiers` | exit 0 | `final-fix-artifacts/logs/check-identifiers-final.log` |
| Diff 卫生 | `git diff --check` | exit 0 | `final-fix-artifacts/logs/git-diff-check-final.log` |

三组 E2E 的首轮最终回放曾出现 2 failed / 9 passed：Playwright 从非原生前台的脱出窗口注入按键时未送达 fixture provider，随后下一用例也受前一超时清理影响。测试先显式让命令来源窗口获得原生焦点；真正的“所属主窗口在前台 → Host Control 把脱出窗口置前”仍由独立的打包 App 进程外调用覆盖。修正后脱出用例单独 1 passed，三组最终 11/11 passed；对应日志为 `three-ai-host-e2e-post-codex-final.log`、`navigation-detached-key-delivery-green.log` 与最终 green 日志。

## 5. 打包产物与受影响真实 App 证据

**App：** `/Users/icesword/Documents/AIProjects/matou/.worktrees/ai-structure-control/apps/desktop/release/mac-arm64/码头.app`

| 组件 | SHA-256 |
|---|---|
| `Contents/MacOS/码头` | `221d5695ab9eb2263b9107e4a5bb3f5780adc35963530e322673d5535e8eeae5` |
| `Contents/Resources/runtime/index.cjs` | `332a241bf92309f89c50c52ba1dfa53087a0e1aea17b1b5dfac66570aa07e51b` |
| `Contents/Resources/app.asar` | `a1b15805787e13c86a734d8bde37f98de6ea2c01e0ae303326711b39a69c75cd` |

哈希记录：`final-fix-artifacts/package-component-sha256.txt`。

受影响场景：

1. 源会话脱出后，从所属主窗口发起 Session focus；完成点晚于目标 Renderer 的原生焦点与 terminal active-element 证明。随后 Workspace / Task / Canvas switch 都返回相同所属主窗口 route、目标脱出窗口 native target，并保持输入终端。
2. 源 Session 明确选择 provider A、模型 A 与 Bypass Permissions 后创建 Fork；退出 App，把全局默认改为 provider B / 模型 B，再启动 App。源 Session 与 Fork 子节点都恢复 provider A / 模型 A / Bypass Permissions；两条进程和 run-bound token 均轮换。

截图：

- `final-fix-artifacts/screenshots/detached-focus-and-switch-packaged.png`
- `final-fix-artifacts/screenshots/provider-settings-after-restart-packaged.png`

结构化结果：

- `final-fix-artifacts/results/detached-focus-and-switch-packaged.json`
- `final-fix-artifacts/results/provider-settings-after-restart-packaged.json`

验收矩阵只更新本轮受影响的第 9、12 场景及重新打包证据：`docs/prd/ai-structure-control-reference-matrix.md`。

## 6. 证据边界

- E2E 与打包 App 的 provider 交互使用 deterministic local fixture；它证明 Matou Runtime、Desktop、SQLite、provider 参数、窗口与终端链路，不代表外部云 provider 已完成在线验证。
- Codex CLI 探针只证明本机 `0.149.0` 接受生成的 inline TOML、触发 SessionStart hook 并交付 attempt identity；它不代表云端模型响应或账号能力验证。
- 打包 App 的 native focus 证据来自本机 macOS 实际 BrowserWindow 和 Renderer active element，不是纯 DOM mock。
- `package:dir` 产物本轮没有 Developer ID Application 签名；功能回放使用该本地目录产物。
- 归档 JSON 只保留 endpoint、模型、权限、PID 是否变化及 token 哈希。测试进程临时读取的原始控制凭证位于一次性 fixture 目录，场景结束即清理，也未进入报告与公开日志。

## 7. 剩余关注点

- 若用户删除一个当前非全局 active、但仍被既有 Session 绑定的 provider 配置，该 Session 下次恢复会以“会话绑定的供应商配置不存在”结束启动。这是保留配置身份的 fail-closed 结果；现有删除 UI 仅保护全局 active 配置，后续可增加“仍被 Session 引用”的删除前提示。
- 原生窗口置前会受到各平台窗口管理策略影响；本轮真实 App 证据覆盖当前 macOS arm64 环境，Windows 与 Linux 仍依赖既有跨平台回归。
- 目录打包未签名，发布安装前仍需走项目既有签名与公证链。
