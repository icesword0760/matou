# Session 模型与 Provider 激活一致性跟进报告

**日期：** 2026-09-04
**实现基线：** `58466b0a956b07dba2726f419a2af7c8deeaa13b`
**分支：** `codex/ai-structure-control`
**结论：** 本轮两条一致性链路均已闭合。运行中 Session 从模型 A 切到 B 后，只有终端发送成功才写回原有 provider binding；立即 Fork 与 App 重启都继续使用 B。切换全局 provider 时，资格判断、binding 更新、旧进程退出和新进程启动收进同一个 Session 排他区间；持久化故障、恢复未就绪和身份待确认的 Session 保持旧 binding 与旧进程，并向设置界面返回明确的暂缓结果。

## 1. 对用户与产品的实际结果

1. **Session 模型选择成为可恢复的会话设置。** 用户在运行中的 Claude Code Session 选择模型 B，Matou 先把 `/model B` 发给当前 Session；发送成功后才把 B 写回这个 Session 已有的 provider binding。随后立刻创建 Fork，子 Session 的 binding 与真实启动参数均为 B。
2. **全局默认不再覆盖既有 Session 的模型。** Session 已从 A 切到 B 后，即使全局默认再切到 C，退出并重新启动 App，该 Session 仍以原 provider 配置、模型 B 和原权限启动，HUD 同步显示 B。
3. **发送失败保持旧状态。** 终端写入抛错时，binding 与 HUD 都停留在 A，RPC 返回错误；不会先向用户展示 B 或把 B 留在数据库。
4. **Provider hook 可校正精确模型。** 当前 provider hook 若携带精确 `model` 字符串或 `model.id`，会把该精确值写回 Session；旧 run 仅携带的启动时 A 不再覆盖已持久化的 B。
5. **全局 provider 激活不会制造“数据库已是 B、进程仍是 A”的分裂状态。** 每个 Session 在排他区间内先检查 durability、recovery 与 provider identity gate。未通过的 Session 返回 `deferred` 与具体原因，binding、PID、权限和会话身份保持原值；符合条件的 Session 才写 binding、停旧进程并以 B 重启。
6. **用户可见反馈与真实结果一致。** 设置界面会分别汇总“已更新”和“暂缓并保持原配置”的 Session 数量；影响说明也明确符合条件的 Claude Code Session 才自动更新。
7. **没有数据库 migration。** 继续复用现有 `provider_bindings.metadata_json`，Fork 与普通 restore 仍读取同一份 durable binding truth。

## 2. 实现证据

| 链路 | 生产代码证据 | 一致性结果 |
|---|---|---|
| 模型动作进入 provider mutation gate | `apps/runtime/src/runtime-server.ts:993-1009` | `session.set-model` 与权限切换共用 durability / recovery / pending-identity 门禁。 |
| 模型发送后写回 | `apps/runtime/src/runtime-server.ts:3064-3113` | 在 `runExclusive(sessionId)` 内校验当前 AI Session，先 `write('/model ...')`，随后只更新原 binding 的 provider launch settings；数据库写入异常时尽力把 live model 发回旧值，HUD 仍保持旧值。 |
| 恢复时 HUD 与进程同源 | `apps/runtime/src/runtime-server.ts:1724-1751` | 启动前从 Session binding 解析 provider 与模型，进程参数和首次 HUD 都使用同一个 `providerModel`。 |
| Hook 当前模型 reconciliation | `apps/runtime/src/session/provider-hook-server.ts:349-371,615-621` | 已知会话身份不再回灌 launch-time model；仅 hook 报告的精确当前模型更新 binding。 |
| HUD 精确模型同步 | `apps/runtime/src/session/session-hud-registry.ts:178-187` | provider 激活/恢复后可把精确模型写入 HUD，同时保持可识别的 model strategy。 |
| 激活结果产品 contract | `packages/contracts/src/provider-config.ts:19-36` | 新增逐 Session 的 `updated` / `deferred` 结果与稳定原因枚举，无 schema 变化。 |
| Provider 激活排他事务 | `apps/runtime/src/runtime-server.ts:3187-3344` | 按共享 RuntimeSessionRegistry 收集 live Session；每个 Session 在单次排他转换中完成资格判断、旧 hook fencing、binding 更新、进程轮换与启动存活校验。失败路径恢复旧 binding，并返回 `restart-unavailable`。 |
| 设置界面反馈 | `apps/desktop/src/renderer/src/hierarchy/ModelSwitchSettings.tsx:86-102,188-191` | Toast 分别显示已更新与暂缓数量；静态说明告知暂缓 Session 保持原配置。 |

### 2.1 保留的身份与权限边界

- 模型动作更新既有 binding id，只改 `model`；`providerConfigId`、profile、provider conversation id 和 permission 继续来自该 Session 原 binding。
- Provider 激活保留原 permission；重启描述符保留 Session id、执行上下文、cwd、尺寸、附着状态和原恢复身份。
- 激活开始时先 retire 当前 run 的 hook registration，再变更 binding；延迟到达的旧 hook 得到正常 HTTP 响应，但其旧身份/model 写入受 run fencing 拦截。
- 对外结构化证据仅记录 provider 配置 id、endpoint、model、PID 是否轮换及 credential digest 是否存在；没有记录原始 key。

## 3. TDD RED / GREEN

所有命令均从 `/Users/icesword/Documents/AIProjects/matou/.worktrees/ai-structure-control` 执行。

### 3.1 Session 模型 A → B、Fork、重启与发送失败

- **RED：** `pnpm --filter @matou/runtime exec vitest run --testTimeout=15000 src/runtime-server.test.ts -t 'persists a successful live model change|restores a successful Session model change|keeps the prior binding and HUD model'`
  结果：3 failed / 109 skipped。旧实现只更新 HUD 并发送命令；Fork 仍继承 A、重启仍按 A 启动、发送失败前 HUD 已显示 B。
  日志：`followup-consistency-artifacts/logs/red-session-model-consistency-2.log`
- **GREEN：** 同一聚焦命令。
  结果：3 passed / 109 skipped。
  日志：`followup-consistency-artifacts/logs/green-session-model-consistency-2.log`
- **测试锚点：** `apps/runtime/src/runtime-server.test.ts:4719-4832`（立即 Fork）、`:4834-4944`（全局 C 后重启）、`:4946-5013`（发送失败保持 A）。

### 3.2 Provider hook 模型 reconciliation

- **RED：** `pnpm --filter @matou/runtime exec vitest run src/session/provider-hook-server.test.ts -t 'old launch registration|exact model id'`
  结果：2 failed / 21 skipped。
  日志：`followup-consistency-artifacts/logs/red-provider-hook-model.log`
- **GREEN：** 同一聚焦命令。
  结果：2 passed / 21 skipped。
  日志：`followup-consistency-artifacts/logs/green-provider-hook-model.log`
- **测试锚点：** `apps/runtime/src/session/provider-hook-server.test.ts:628-672`。

### 3.3 Provider 激活资格与原子转换

- **RED（durability fault）：** 1 failed / 111 skipped；旧实现未返回 deferred 结果。
  日志：`followup-consistency-artifacts/logs/red-provider-activation-durability.log`
- **RED（recovery-not-ready）：** 1 failed / 111 skipped；旧实现未返回 deferred 结果。
  日志：`followup-consistency-artifacts/logs/red-provider-activation-recovery.log`
- **RED（pending identity）：** 1 failed / 112 skipped；旧实现缺少逐 Session 结果。
  日志：`followup-consistency-artifacts/logs/red-provider-activation-pending-2.log`
- **RED（eligible + delayed old hook）：** 1 failed / 113 skipped；旧流程在排他区间外先写 binding。
  日志：`followup-consistency-artifacts/logs/red-provider-activation-atomic-hook.log`
- **GREEN：** `pnpm --filter @matou/runtime exec vitest run --testTimeout=15000 src/runtime-server.test.ts -t 'global Claude provider only to healthy|defers provider activation while a restored|atomically activates an eligible|recovering Claude process, permission'`
  结果：4 passed / 110 skipped。
  日志：`followup-consistency-artifacts/logs/green-provider-activation-consistency.log`
- **测试锚点：** `apps/runtime/src/runtime-server.test.ts:762-886`（eligible 原子切换与旧 hook fencing）、`:1128-1242`（durability）、`:1244-1320`（pending identity）、`:5968-6062`（recovery）。

### 3.4 产品反馈

- **RED：** `pnpm --filter @matou/desktop exec vitest run src/renderer/src/hierarchy/ModelSwitchSettings.test.tsx`
  结果：1 failed / 2 skipped；界面仍笼统宣称全部 Claude Code Session 已更新。
  日志：`followup-consistency-artifacts/logs/red-provider-activation-product-feedback.log`
- **GREEN：** 同一命令，3 passed。
  日志：`followup-consistency-artifacts/logs/green-provider-activation-product-feedback.log`
- **测试锚点：** `apps/desktop/src/renderer/src/hierarchy/ModelSwitchSettings.test.tsx:45-76`。

## 4. 验收矩阵

| 场景 | 预期 | 自动化 / 运行证据 | 结论 |
|---|---|---|---|
| 运行中模型 A → B 后立即 Fork | 父 Session binding 先落 B；子 binding、启动 `--model` 均为 B，provider 仍为 A | `runtime-server.test.ts:4719-4832` | 通过 |
| A → B，随后全局默认 C，再重启 | source Session 使用 provider A endpoint、模型 B、原 conversation；HUD 为 B | `runtime-server.test.ts:4834-4944`；打包 App 结果 JSON | 通过 |
| 模型命令发送抛错 | binding 与 HUD 保持 A；RPC 报错 | `runtime-server.test.ts:4946-5013` | 通过 |
| Hook 仅持有旧启动模型 A | 已持久化的 B 保持不变 | `provider-hook-server.test.ts:628-650` | 通过 |
| 当前 hook 精确报告 B | binding reconciliation 到 B | `provider-hook-server.test.ts:651-672` | 通过 |
| durability fault | binding A、PID A；结果 `deferred/durability-fault` | `runtime-server.test.ts:1128-1242` | 通过 |
| recovery-not-ready | binding A、PID A、permission 原值；结果 `deferred/recovery-not-ready` | `runtime-server.test.ts:5968-6062` | 通过 |
| provider identity pending | binding A、PID A；结果 `deferred/provider-identity-pending` | `runtime-server.test.ts:1244-1320` | 通过 |
| eligible provider A → B | binding、真实进程参数、endpoint、HUD 同步为 B；PID 轮换；旧 hook 回写 A 被 fencing 拦截 | `runtime-server.test.ts:762-886` | 通过 |
| 设置界面有更新与暂缓混合结果 | 展示 1 个已更新、2 个暂缓并保持原配置 | `ModelSwitchSettings.test.tsx:45-76` | 通过 |
| 重新打包 App：Session B 与全局 C 不同 | App 重启后恢复 provider A / model B；全局 active 仍为 C；provider PID 轮换 | `followup-consistency-artifacts/results/session-model-restart-packaged.json` 与截图 | 通过 |

## 5. 最终验证

| 验证 | 精确命令 | 最终结果 | 日志 |
|---|---|---|---|
| 修改前聚焦基线 | `pnpm --filter @matou/runtime exec vitest run --testTimeout=15000 src/runtime-server.test.ts src/session/provider-hook-server.test.ts` | 2 files / 130 passed | `followup-consistency-artifacts/logs/baseline-focused.log` |
| 最终 Runtime 聚焦 | `pnpm --filter @matou/runtime exec vitest run --testTimeout=15000 src/runtime-server.test.ts src/session/provider-hook-server.test.ts -t '...'` | 2 files；10 passed / 127 skipped | `followup-consistency-artifacts/logs/focused-runtime-final.log` |
| 最终 Desktop 聚焦 | `pnpm --filter @matou/desktop exec vitest run src/renderer/src/hierarchy/ModelSwitchSettings.test.tsx` | 1 file / 3 passed | `followup-consistency-artifacts/logs/focused-desktop-final.log` |
| 全量单元/集成 | `pnpm test` | exit 0；identifier 4 passed；Vitest 194 files / 1886 passed；合计 1890 passed | `followup-consistency-artifacts/logs/pnpm-test-final.log` |
| 全仓类型 | `pnpm typecheck` | exit 0；contracts、domain、ui、runtime、desktop 全部通过 | `followup-consistency-artifacts/logs/typecheck-final.log` |
| 三组 AI Host E2E | `pnpm exec playwright test tests/e2e/ai-host-control-cli.spec.ts tests/e2e/ai-host-structure-control.spec.ts tests/e2e/ai-host-navigation.spec.ts --workers=1 --reporter=line` | 最终完整重跑 11 passed (1.4m) | `followup-consistency-artifacts/logs/three-ai-host-e2e-final-rerun.log` |
| 目录打包 | `pnpm package:dir` | exit 0；最终源码重新构建并生成 macOS arm64 App | `followup-consistency-artifacts/logs/package-dir-final.log` |
| 打包 App 重启场景 | `MATOU_E2E_EXECUTABLE_PATH="$PWD/apps/desktop/release/mac-arm64/码头.app/Contents/MacOS/码头" pnpm exec playwright test tests/e2e/.followup-consistency-packaged.spec.ts --workers=1 --reporter=line` | 1 passed (4.9s) | `followup-consistency-artifacts/logs/packaged-session-model-restart-post-package.log` |
| 命名门禁 | `pnpm check:identifiers` | exit 0 | `followup-consistency-artifacts/logs/check-identifiers-final-postpackage.log` |
| Diff 卫生 | `git diff --check` | exit 0 | `followup-consistency-artifacts/logs/git-diff-check-final-postpackage.log` |

三组 E2E 的一次中间回放出现 10 passed / 1 failed：失败点是 macOS 原生前台窗口焦点等待。该 navigation spec 随即独立重跑 2/2 passed，随后三个 spec 按同一完整命令再跑得到 11/11 passed。中间与重试日志分别为 `three-ai-host-e2e-final.log`、`ai-host-navigation-retry.log` 和最终 `three-ai-host-e2e-final-rerun.log`；本轮生产修改没有触碰窗口焦点代码。

## 6. 打包 App 证据

**App：** `/Users/icesword/Documents/AIProjects/matou/.worktrees/ai-structure-control/apps/desktop/release/mac-arm64/码头.app`

| 组件 | SHA-256 |
|---|---|
| `Contents/MacOS/码头` | `221d5695ab9eb2263b9107e4a5bb3f5780adc35963530e322673d5535e8eeae5` |
| `Contents/Resources/runtime/index.cjs` | `825766fad381777faa192f9add2fe2e509a0dc4d58f805fb5d2c533d9bf1403d` |
| `Contents/Resources/app.asar` | `aecd6ec991e35b18fb93619c117bcd89ba0851038ce76262017b6c42bb927b73` |

哈希清单：`followup-consistency-artifacts/package-component-sha256.txt`。

打包 App 场景通过真实 Electron 主进程、Renderer、Runtime、SQLite 与 PTY，使用 deterministic local provider fixture：

1. 全局 provider A / model A 启动既有 Session。
2. 通过 Matou 同一模型动作所发出的 `/model claude-sonnet-4-6` 命令切到 B，并由 provider hook 报告精确当前模型 B。
3. 退出 App，把全局 active 改为 provider C / model C。
4. 再次启动同一打包 App；同一 Session 恢复 provider A endpoint、model B、原 permission 与 conversation identity，真实 provider PID 已轮换；全局 active 仍保持 C。

结构化结果：`followup-consistency-artifacts/results/session-model-restart-packaged.json`。
截图：`followup-consistency-artifacts/screenshots/session-model-restart-packaged.png`。截图终端与底部 HUD 均显示 `claude-sonnet-4-6`。

## 7. 证据边界与关注点

- 打包验收使用 deterministic local provider fixture，已覆盖 Matou 的真实打包 App、终端、Runtime、SQLite、hook、重启与 HUD 链路；外部云 provider 在线响应证据不足。
- “模型动作成功”的持久化边界是当前 PTY 接受写入。若 provider 随后通过 hook 报告另一个精确当前模型，hook reconciliation 会以实际报告值更新 binding。
- Provider 激活按 Session 逐个原子转换，而不是把所有 Session 包成一个全局事务。因此可出现部分 Session 已更新、部分 Session 暂缓；返回 contract 与设置界面会逐项/汇总呈现，暂缓 Session 保持旧配置与旧进程。
- `package:dir` 产物缺少 Developer ID Application 签名；本轮功能回放使用该本地目录产物，正式分发仍走项目既有签名与公证流程。
- 最终完整 AI Host E2E 已通过；中间一次原生窗口前台焦点等待抖动已在独立重试和完整重跑中复核。
