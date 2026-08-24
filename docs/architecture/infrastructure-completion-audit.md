# Matou INF-01～INF-25 完成审计

状态：通过  
审计日期：2026-08-24  
范围：基础设施准入；不包含功能 UI 开发

## 1. 结论

INF-01～INF-25 已落地，并有自动化测试、边界扫描、构建产物和 Electron 打包态 E2E 证据。当前代码基线满足进入功能开发阶段的准入条件。

## 2. INF 逐项证据

| INF | 结论 | 直接实现与测试证据 |
|---|---|---|
| INF-01 | 通过 | `storage/database.ts`；`storage/storage-queue.ts`；`storage/database.test.ts` 覆盖 WAL/FULL/foreign keys/busy timeout、owner lock、queue 顺序。 |
| INF-02 | 通过 | `storage/migration-runner.ts`、`storage/migrations.ts`；`storage/migration-runner.test.ts` 覆盖 fixture、checksum、backup、事务回滚；当前 schema v7。 |
| INF-03 | 通过 | `storage/domain-transaction.ts` 强制 mutation + emit + command dedup 同事务；`storage/domain-transaction.test.ts` 覆盖原子性、回滚与幂等。 |
| INF-04 | 通过 | `events/domain-event-store.ts`、`runtime-server.ts`；持久 consumer cursor、RPC replay、subscription batch 与 generation fencing 由对应测试覆盖。 |
| INF-05 | 通过 | `journal/segment-journal.ts` Journal V2；`segment-journal.test.ts` 覆盖 frame、checksum、rotation/compression、尾部修复和中段损坏。 |
| INF-06 | 通过 | `recovery/journal-event-alignment.ts`；测试覆盖 SQLite commit/marker append/flush 崩溃窗口和 required terminal sequence。 |
| INF-07 | 通过 | `checkpoints/checkpoint-manager.ts`；双水位、两代保留、checksum fallback、孤儿清理由测试覆盖；Runtime replay 已加载 checkpoint + tail。 |
| INF-08 | 通过 | `domain/workspace-task-repository.ts`；Workspace/Task create/update/archive、父子与 cascade 约束由测试覆盖。 |
| INF-09 | 通过 | `worktrees/worktree-service.ts`；创建、绑定、dirty retain、active run 阻止清理、失败记录由测试覆盖。 |
| INF-10 | 通过 | `domain/session-repository.ts`；Session/SessionRun/ProviderBinding 身份拆分、resume 筛选及单 Session 失败隔离由测试覆盖。 |
| INF-11 | 通过 | `relations/session-relation-repository.ts`；append-only facts + current projection、fork/depends 深链环性质测试、单父节点、兄弟派生。 |
| INF-12 | 通过 | `scenes/scene-repository.ts`；Scene/Node/Window/Mount 结构命令、快照与归档测试。 |
| INF-13 | 通过 | `scenes/geometry-repository.ts`；几何状态独立直写、不进 Outbox，stale layout revision 被拒绝。 |
| INF-14 | 通过 | `agents/agent-adapters.ts`；Claude Code/Codex/generic fixture、稳定 provider event identity 与置信来源测试。 |
| INF-15 | 通过 | `anchors/anchor-resolver.ts`；semantic/command/screen 三类锚点、marker cache 与 retention degradation 测试。 |
| INF-16 | 通过 | `rpc/runtime-rpc-router.ts`、`runtime-server.ts`、`session/runtime-session-registry.ts`、`control/host-control-server.ts`；RPC、Renderer reconnect、PTY reattach、Host Control token/边界/故障隔离测试。 |
| INF-17 | 通过 | `domain/product-foundation-repository.ts`；Annotation/Artifact/Validation 事务事件与 Task telemetry 当前 generation、订阅、容量测试。 |
| INF-18 | 通过 | `product/experience-foundation.ts`；preferences allowlist、notification dedup/navigation、campaign version/seen/restore/debug 测试。 |
| INF-19 | 通过 | `retention/retention-manager.ts`；quota dry-run、两阶段 trash、rollback、anchor degradation、archive/purge、权限测试。 |
| INF-20 | 通过 | `observability/diagnostics.ts`、`presets/preset-capability-registry.ts`；脱敏诊断、metrics、lock/idempotency/offline seed/checksum/upgrade rollback/drift repair 测试。 |
| INF-21 | 通过 | `compat/kooky-bridge/kooky-importer.ts`；Kooky snapshot/checkpoint/metadata/journal fixture、确定性映射、provider/team 恢复与坏记录隔离。 |
| INF-22 | 通过 | `compat/kooky-bridge/shadow-write-bridge.ts`；legacy-first shadow write、byte cursor、partial line、diff/lag、repair queue 测试。 |
| INF-23 | 通过 | `compat/kooky-bridge/read-switch.ts`；SQLite read authority、projection equality、SQLite-first compatibility backup、rollback 与迁移 telemetry 测试。 |
| INF-24 | 通过 | `compat/kooky-bridge/legacy-retirement.ts`、`kooky-migration.md`；退役窗口、authority scan、Renderer snapshot export scan。 |
| INF-25 | 通过 | 本文；`observability/infrastructure-load.test.ts`、`recovery/runtime-recovery-service.test.ts`、`tests/e2e/terminal-channel.spec.ts`、`tests/e2e/packaged-runtime.spec.ts` 以及全量命令。 |

## 3. 37 类测试矩阵闭合

| # | 测试类 | 自动化证据 |
|---:|---|---|
| 1 | SQLite PRAGMA/single writer/queue | `storage/database.test.ts` |
| 2 | migration/checksum/backup/failure | `storage/migration-runner.test.ts` |
| 3 | mutation/outbox atomicity | `storage/domain-transaction.test.ts` |
| 4 | command idempotency | `storage/domain-transaction.test.ts` |
| 5 | domain cursor replay | `events/domain-event-store.test.ts`、`runtime-server.test.ts` |
| 6 | Journal checksum/tail/middle corruption | `journal/segment-journal.test.ts` |
| 7 | SQLite/Journal crash windows | `recovery/journal-event-alignment.test.ts` |
| 8 | paired checkpoint fallback | `checkpoints/checkpoint-manager.test.ts`、`runtime-server.test.ts` |
| 9 | Workspace/Task cascade/archive | `domain/workspace-task-repository.test.ts` |
| 10 | Worktree dirty retain | `worktrees/worktree-service.test.ts` |
| 11 | Session/Run/Provider split | `domain/session-repository.test.ts` |
| 12 | relation event/current | `relations/session-relation-repository.test.ts` |
| 13 | fork/dependency cycle property | 同上深链 back-edge 参数化性质测试 |
| 14 | sibling derivation | 同上，验证零 sibling 持久边 |
| 15 | Scene structure/geometry split | `scenes/scene-repository.test.ts` |
| 16 | stale geometry revision | 同上 |
| 17 | Agent event idempotency | `agents/agent-adapters.test.ts` |
| 18 | Anchor degradation | `anchors/anchor-resolver.test.ts` |
| 19 | Renderer reload/reconnect | `runtime-server.test.ts` live PTY reattach；`RuntimeProjectionStore.test.ts` |
| 20 | Runtime crash/restart | `recovery/runtime-recovery-service.test.ts`；`main/runtime-host.test.ts` |
| 21 | 单 Session 损坏隔离 | `recovery/runtime-recovery-service.test.ts` |
| 22 | provider resume failure | `domain/session-repository.test.ts` |
| 23 | Kooky importer fixtures | `compat/kooky-bridge/kooky-importer.test.ts` |
| 24 | shadow diff/repair | `compat/kooky-bridge/shadow-write-bridge.test.ts` |
| 25 | packaged Electron SQLite/node-pty | `tests/e2e/packaged-runtime.spec.ts` |
| 26 | 多终端 throughput/credit/outbox latency | `observability/infrastructure-load.test.ts`、`flow-control/credit-window.test.ts`、large replay credit test |
| 27 | disk full/read-only/partial write | `journal/segment-journal.test.ts` 注入 ENOSPC、partial write 修复、read-only isolation |
| 28 | retention/purge | `retention/retention-manager.test.ts` |
| 29 | dependency boundary | `storage/dependency-boundary.test.ts` |
| 30 | Renderer authority | `compat/kooky-bridge/legacy-retirement.test.ts` |
| 31 | Host Control same-user/token/default deny | `control/host-control-server.test.ts`；Unix 0700/0600 与 Windows Named Pipe endpoint |
| 32 | ordinal revision/stale/stable ID | 同上 |
| 33 | terminal read/send-key/errors | 同上及 `control/runtime-control-backend.ts` |
| 34 | telemetry generation/subscription/capacity | `domain/product-foundation-repository.test.ts` |
| 35 | campaigns | `product/experience-foundation.test.ts` |
| 36 | presets | `observability/diagnostics-and-presets.test.ts` |
| 37 | compat contract/mapping/authority | contracts `kooky-bridge.test.ts` 与 Runtime 三阶段测试 |

## 4. 准入条件核对

- Runtime 独占 SQLite connection；Renderer/Main 生产源码的 `node:sqlite` boundary scan 通过。
- 领域 Repository 写入走 `DomainTransactionManager`；几何、preference、telemetry 等明确的非领域高频状态按设计隔离。
- Terminal Data 使用 Renderer ↔ Runtime MessagePort；Main 只建链和移交。
- live 与 replay 均有累计 ACK flow control；replay 会追平 detach 期间的新 Journal tail 后再切回 live。
- Runtime 启动先执行 migration，再逐 Session recovery；单 Session corruption 进入独立报告。
- Renderer disconnect 不结束 PTY；Runtime crash 由 Main supervisor 重启并重建端口。
- Runtime replay 返回 paired checkpoint + Journal tail；Renderer 重置并恢复 checkpoint 后消费 tail。
- SessionRelation 只存事实边；兄弟关系派生，不存冗余 sibling edge。
- Kooky 三阶段 migration authority、shadow repair、rollback、retirement 均有持久状态和测试。
- 打包产物包含 Runtime bundle 与 node-pty native prebuild；打包态测试实际启动 `Matou.app` 两次并验证 SQLite v7、PTY 输出、replay、torn-tail recovery。

## 5. 最终验证命令

在 `/Users/icesword/Documents/AIProjects/matou` 执行：

```bash
pnpm test
pnpm typecheck
pnpm build
pnpm test:e2e
pnpm test:packaged
```

最后一次准入运行结果：全部退出码为 0；详细测试数量以各命令的 Vitest/Playwright 输出为准。
