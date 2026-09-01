# Matou 恢复、数据安全、Worktree 与数据库实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把 Matou 的启动恢复、数据库降级、Worktree 环境、Fork 长任务和运行中存储故障改造成可观察、可恢复、按影响范围隔离的状态系统，确保会话资产始终保留，执行环境异常时也不会静默切换目录或丢失用户操作。

**Architecture:** Runtime 使用单一恢复状态机串联数据库、Worktree、Fork 和会话恢复层；数据库启动结果改为 `writable / read-only / recovery-required` 判别联合，Worktree 作为会话可替换的运行环境独立建模。所有长任务先提交持久化 intent，再由带 fencing 的后台协调器执行；PTY 输出先通过单会话 durability gate，写失败只暂停对应进程并保留有界尾部。

**Tech Stack:** TypeScript、Electron UtilityProcess/IPC、React 19、Node `node:sqlite`、SQLite WAL/online backup、Git worktree、node-pty、Vitest、Playwright。

**Spec:** `docs/audits/2026-09-01-internal-hardening-product-decisions.md`（D-02、D-03、D-05、D-06、D-07、D-11）

## Global Constraints

- 会话、历史、DAG 关系是用户资产；PTY、xterm、目录和 Worktree 是可恢复环境。
- Worktree 失效时保留会话与历史，执行入口保持锁定，路径恢复、定位或 Handoff 成功后才继续输入。
- 数据库只读模式只开放浏览、搜索、复制和导出；所有 mutation 在 Runtime 服务端统一拒绝并返回稳定错误码。
- 数据库损坏先进入恢复页；创建全新空库必须由用户在二次确认后触发。
- SQLite online backup 在 schema migration 前和正常退出时生成，校验成功后进入最近 7 份轮转。
- 存储写失败按 Session 隔离；受影响进程暂停，最后 4 MiB 未落盘输出留在内存，恢复后按原 sequence 继续写入。
- Fork RPC 只负责持久化受理并快速返回；Git、setup、provider 恢复和窗口启动在后台继续。
- Fork 当前阶段不设置取消入口；同一 `submissionKey` 只生成一个 intent、Session、branch 和 Worktree。
- 真实故障验收使用 Electron、真实 SQLite/WAL、真实 Git worktree、真实 PTY；单元测试中的 fake 只用于精确覆盖状态迁移。
- Kooky CLI 黑色区域内既有视觉与交互继续作为截图和行为对照基线。

---

## 1. 状态与接口总图

### Runtime 分层恢复

```ts
export type RuntimeMode = 'normal' | 'read-only' | 'recovery-required'

export type RuntimeRecoveryStage =
  | 'opening-database'
  | 'reconciling-worktrees'
  | 'reconciling-forks'
  | 'recovering-active-session'
  | 'recovering-background-sessions'
  | 'ready'

export interface RuntimeRecoverySnapshot {
  mode: RuntimeMode
  stage: RuntimeRecoveryStage
  completed: number
  total: number
  activeSessionId?: string
  failures: Array<{
    layer: 'database' | 'worktree' | 'fork' | 'session' | 'journal'
    resourceId: string
    code: string
    message: string
  }>
}
```

### 会话环境与 Git 分离

```ts
export interface SessionEnvironment {
  kind: 'local' | 'worktree'
  state: 'ready' | 'missing' | 'recovering' | 'handoff' | 'failed'
  path: string
  localExecutionContextId: string
  worktreeId?: string
  worktreeExecutionContextId?: string
  error?: string
}

export interface SessionGitState {
  state: 'ready' | 'unavailable'
  branch?: string
  detachedHead?: string
  dirty: boolean
}
```

### Fork 阶段

```ts
export type ForkStage =
  | 'queued'
  | 'creating-worktree'
  | 'applying-setup'
  | 'binding-session'
  | 'restoring-provider'
  | 'starting-window'
  | 'succeeded'
  | 'failed'

export interface ForkProgress {
  operationId: string
  sessionId: string
  submissionKey: string
  stage: ForkStage
  completedSteps: number
  totalSteps: number
  attempt: number
  error?: string
}
```

### 依赖顺序

```text
Task 1 状态契约
  ├─ Task 3 数据库启动三态 ─ Task 4 生命周期IPC/恢复页 ─ Task 5 只读模式
  ├─ Task 6 会话环境绑定 ─ Task 7 Worktree对账 ─ Task 8 恢复/定位/Handoff ─ Task 9 右下角环境UI
  ├─ Task 10 Fork持久化阶段 ─ Task 11 后台协调/重启对账 ─ Task 12 Fork阶段UI
  └─ Task 13 单会话durability gate ─ Task 14 存储故障UI

Task 2 七份数据库备份 ─ Task 3 / Task 4
Task 3 + Task 7 + Task 11 + Task 13 ─ Task 15 分层恢复协调器
Task 4 + Task 5 + Task 8 + Task 9 + Task 12 + Task 14 + Task 15 ─ Task 16 真实故障E2E
```

---

## Task 1：冻结恢复状态、环境状态和生命周期协议

**Files:**

- Create: `packages/contracts/src/runtime-lifecycle.ts`
- Create: `packages/contracts/src/runtime-lifecycle.test.ts`
- Modify: `packages/contracts/src/index.ts`
- Modify: `packages/contracts/src/protocol.ts`
- Modify: `packages/contracts/src/protocol.test.ts`
- Modify: `packages/domain/src/model.ts`
- Modify: `packages/domain/src/model.test.ts`

**Produces:** `RuntimeRecoverySnapshot`、`RuntimeLifecycleEvent`、`RuntimeRecoveryCommand`、`SessionEnvironment`、`SessionGitState`、`ForkProgress` 以及错误码 `STORAGE_READ_ONLY`、`DATABASE_RECOVERY_REQUIRED`、`SESSION_ENVIRONMENT_UNAVAILABLE`、`FORK_ALREADY_RUNNING`。

**TDD steps:**

- [ ] 写失败测试：生命周期 schema 接受合法阶段，拒绝倒退的进度、负数计数、缺少 recovery backup ID 的 restore 命令。
- [ ] 写失败测试：`SessionGraphNode` 的 environment 与 git 可分别存在；Worktree missing 时仍要求 `sessionId/title/latestLines` 完整。
- [ ] 运行：

```bash
pnpm --filter @matou/contracts exec vitest run src/runtime-lifecycle.test.ts src/protocol.test.ts
pnpm --filter @matou/domain exec vitest run src/model.test.ts
```

  预期：新类型/schema 尚未存在而失败。

- [ ] 实现上节列出的判别联合；`RuntimeLifecycleEvent` 使用 `{ type: 'runtime.lifecycle'; snapshot }`，恢复命令使用 `{ type: 'runtime.recovery-command'; requestId; action; backupId? }`。
- [ ] 把 `RuntimeMessage` 增加 `terminal.storage-fault` 和 `terminal.storage-recovered`；fault 消息包含 `sessionId`、`sequence`、`code`、`message`、`retainedBytes`。
- [ ] 重跑上述测试和两个 package 的 typecheck，预期全绿。

**Acceptance:** 后续任务只引用本任务公开类型，不再各自定义同义字符串状态；协议 schema 对未知 action 和非法阶段 fail closed。

---

## Task 2：SQLite 一致性备份、校验、恢复与最近七份轮转

**Files:**

- Create: `apps/runtime/src/storage/database-backup-service.ts`
- Create: `apps/runtime/src/storage/database-backup-service.test.ts`
- Create: `apps/runtime/src/storage/database-lifecycle-service.ts`
- Create: `apps/runtime/src/storage/database-lifecycle-service.test.ts`
- Modify: `apps/runtime/src/storage/database.ts`
- Modify: `apps/runtime/src/storage/migration-runner.ts`
- Modify: `apps/runtime/src/storage/migration-runner.test.ts`
- Modify: `apps/runtime/src/index.ts`

**Interfaces:**

```ts
export type DatabaseBackupReason = 'pre-migration' | 'clean-exit'

export interface DatabaseBackupDescriptor {
  id: string
  path: string
  createdAt: number
  reason: DatabaseBackupReason
  schemaVersion: number
  size: number
  sha256: string
}

export class DatabaseBackupService {
  create(database: RuntimeDatabase, reason: DatabaseBackupReason): Promise<DatabaseBackupDescriptor>
  listValid(): Promise<DatabaseBackupDescriptor[]>
  restore(backupId: string, targetDatabasePath: string): Promise<void>
  rotate(maxCount?: number): Promise<void>
}
```

**TDD steps:**

- [ ] 写失败测试：带未 checkpoint WAL 的 committed row 进入 online backup，打开备份后记录存在且 `PRAGMA integrity_check` 返回 `ok`。
- [ ] 写失败测试：连续创建 9 份后只保留按 `createdAt` 排序的最近 7 份；临时文件、checksum 错误和 integrity 失败的备份不进入列表。
- [ ] 写失败测试：restore 先写同目录临时文件并校验，再原子替换目标 DB；目标 WAL/SHM 被移走；注入 rename 中断时原数据库或完整恢复库至少保留一份。
- [ ] 写失败测试：MigrationRunner 发现 pending migration 时调用 `create(..., 'pre-migration')`，备份完成后才执行第一条 migration。
- [ ] 写失败测试：`DatabaseLifecycleService.closeCleanly()` 等待 `create(..., 'clean-exit')` 与轮转完成后才关闭连接；backup 失败时保留错误并仍有界关闭 Runtime。
- [ ] 运行：

```bash
pnpm --filter @matou/runtime exec vitest run \
  src/storage/database-backup-service.test.ts \
  src/storage/database-lifecycle-service.test.ts \
  src/storage/migration-runner.test.ts
```

- [ ] 实现备份目录 `<dataRoot>/backups/`，数据文件名 `matou-<createdAt>-<reason>-v<schema>.sqlite`，旁路 manifest 使用同名 `.json`。
- [ ] `create` 通过 `RuntimeDatabase.backupTo()` 生成 `.partial`，校验 integrity、size、SHA-256 后 rename；manifest 最后写入。
- [ ] `restore` 重新校验 manifest 与 SQLite，目标原库移动到 `.replaced-<createdAt>`，随后替换 DB 并清理旧 WAL/SHM。
- [ ] MigrationRunner 移除散落在 DB 旁的旧 `pre-v*.sqlite` 逻辑，统一注入 `DatabaseBackupService`。
- [ ] Runtime 正常 shutdown 通过 `DatabaseLifecycleService.closeCleanly()` 生成 clean-exit 备份；崩溃退出依赖最近一次已完成备份，不在 signal handler 中伪造成功记录。
- [ ] 重跑测试与 `pnpm --filter @matou/runtime typecheck`。

**Acceptance:** 备份始终是 SQLite 可打开的一致快照；列表最多 7 份；migration 在备份完成前没有执行；恢复中断不会同时破坏源备份和原数据库。

**Real fault injection:** 在 `/tmp` 创建 WAL 数据库，提交后保持连接打开，执行 backup，再用独立 `sqlite3`/Node `DatabaseSync` 验证 committed row 与 `integrity_check`。

---

## Task 3：数据库启动改为 writable / read-only / recovery-required 三态

**Files:**

- Modify: `apps/runtime/src/storage/database.ts`
- Modify: `apps/runtime/src/storage/runtime-database-bootstrap.ts`
- Modify: `apps/runtime/src/storage/runtime-database-bootstrap.test.ts`
- Modify: `apps/runtime/src/index.ts`

**Interfaces:**

```ts
export type RuntimeDatabaseBootstrapResult =
  | { kind: 'writable'; database: RuntimeDatabase; dataRoot: string }
  | {
      kind: 'read-only'
      database: RuntimeDatabase
      dataRoot: string
      reason: 'filesystem-read-only' | 'newer-schema'
    }
  | {
      kind: 'recovery-required'
      reason: 'physical-corruption'
      durableDatabasePath: string
      quarantinedPath: string
      backups: DatabaseBackupDescriptor[]
    }
```

**TDD steps:**

- [ ] 把当前“read-only 时打开可写 `/tmp` 副本”的测试改成：结果为 `kind: 'read-only'`，查询成功，`run/exec/transaction/enqueueWrite` 统一抛 `STORAGE_READ_ONLY`，原 DB 无变化。
- [ ] 把当前“损坏后创建干净库”的测试改成：返回 `recovery-required`，原文件进入 quarantine，`matou.sqlite` 空位保持未创建，备份列表可用。
- [ ] 增加中段页损坏测试：修改已有 SQLite 文件中非 header 页面，启动前 `quick_check`/`integrity_check` 检出后进入恢复态。
- [ ] 增加 newer-schema 测试：以 read-only 打开并保留 v999；mutation 被统一拒绝。
- [ ] 运行：

```bash
pnpm --filter @matou/runtime exec vitest run src/storage/runtime-database-bootstrap.test.ts
```

- [ ] 为 `RuntimeDatabase` 增加 `openReadOnly(path)`；使用 SQLite read-only option 与 `PRAGMA query_only=ON`，跳过 owner 写入、WAL 模式切换和 `_runtime_meta` 更新。
- [ ] writable 打开后、MigrationRunner 前执行完整 integrity 检查；物理损坏只隔离并列出 Task 2 的有效备份。
- [ ] `index.ts` 按判别联合进入 normal runtime、read-only runtime 或 boot recovery loop，删除对 `ephemeral` 的依赖。
- [ ] 重跑 bootstrap、database、migration 测试和 typecheck。

**Acceptance:** 权限变化后新操作不会写入临时副本；损坏启动不会展示默认空 Scene；损坏文件、七份备份和用户选择前的状态全部保留。

---

## Task 4：Runtime 生命周期 IPC 与数据库恢复页

**Files:**

- Modify: `apps/runtime/src/index.ts`
- Modify: `apps/desktop/src/main/runtime-host.ts`
- Modify: `apps/desktop/src/main/runtime-host.test.ts`
- Modify: `apps/desktop/src/shared/desktop-api.ts`
- Modify: `apps/desktop/src/preload/index.ts`
- Modify: `apps/desktop/src/renderer/src/App.tsx`
- Create: `apps/desktop/src/renderer/src/recovery/DatabaseRecoveryPage.tsx`
- Create: `apps/desktop/src/renderer/src/recovery/DatabaseRecoveryPage.test.tsx`
- Create: `apps/desktop/src/renderer/src/recovery/recovery.css`
- Modify: `apps/desktop/src/main/index.ts`

**Consumes:** Task 1 lifecycle protocol、Task 2 backup restore、Task 3 bootstrap union。

**TDD steps:**

- [ ] RuntimeHost 测试先要求：child `spawn` 后状态仍为 `opening-database`；只有收到 `runtime.lifecycle` 的 `ready/read-only/recovery-required` 才向 Renderer 发布对应状态。
- [ ] 增加永久启动失败测试：重启延迟按 100ms、500ms、1s、2s、5s 上限退避；收到 ready 后计数清零。
- [ ] 恢复页组件测试覆盖：按最近时间展示最多 7 份、默认选中最新有效备份、恢复按钮防重复、导出损坏数据、重新检查、二次确认后进入空状态。
- [ ] 恢复控制测试覆盖：`restore-backup` 完成后重新 bootstrap 并进入 `opening-database → ... → ready`；restore 失败停留在恢复页并显示原错误。
- [ ] 运行：

```bash
pnpm --filter @matou/desktop exec vitest run \
  src/main/runtime-host.test.ts \
  src/renderer/src/recovery/DatabaseRecoveryPage.test.tsx
```

- [ ] Runtime child 通过 `parentPort.postMessage` 发布 lifecycle；RuntimeHost 监听 child message，并通过 `DESKTOP_CHANNELS.runtimeLifecycle` 转发。
- [ ] Preload 暴露 `getRuntimeLifecycle()`、`onRuntimeLifecycle()`、`restoreDatabaseBackup(backupId)`、`exportDatabaseRecoveryBundle()`、`retryDatabaseOpen()`、`startWithEmptyDatabase()`。
- [ ] `startWithEmptyDatabase` 在 Runtime 内完成：保存 quarantine/backup 后创建新库；Main 只传递命令，不直接操作 SQLite。
- [ ] `App.tsx` 在 `recovery-required` 时只渲染恢复页；read-only 和 normal 继续进入 HierarchyShell。
- [ ] 重跑测试、desktop typecheck 和 build。

**Acceptance:** 恢复页出现前主工作区不会短暂闪现；恢复操作全程有阶段与结果；全新空状态入口有明确二次确认；RuntimeHost 不再用固定 1 秒猜测 ready。

**Real fault injection:** 把 `matou.sqlite` header 第 1–16 字节改为随机值，启动真实 Electron，断言只显示恢复页；从最新备份恢复后原 Workspace/Task/Session 全部出现。

---

## Task 5：只读恢复模式与服务端 mutation guard

**Files:**

- Create: `apps/runtime/src/storage/runtime-access-policy.ts`
- Create: `apps/runtime/src/storage/runtime-access-policy.test.ts`
- Modify: `apps/runtime/src/rpc/runtime-rpc-router.ts`
- Modify: `apps/runtime/src/runtime-server.ts`
- Modify: `apps/desktop/src/renderer/src/runtime/RuntimeProvider.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/hierarchy-commands.ts`
- Create: `apps/desktop/src/renderer/src/recovery/ReadOnlyRecoveryBanner.tsx`
- Create: `apps/desktop/src/renderer/src/recovery/ReadOnlyRecoveryBanner.test.tsx`

**Interfaces:** `RuntimeAccessPolicy.assertRpcAllowed(method)` 与 `assertTerminalAllowed(messageType)`；allowlist 只有 projection、events replay/ack、Claude session list/detail、terminal replay、搜索所需只读 RPC 和数据库导出控制。

**TDD steps:**

- [ ] 参数化失败测试遍历 `RPC_METHODS`：每个 mutation 在 read-only 模式返回 `STORAGE_READ_ONLY`；查询方法仍成功。
- [ ] 协议测试要求 `terminal.spawn/input/resize/retry-last-input` 在只读模式被拒绝，`terminal.replay-request/ack` 保持可用。
- [ ] UI 测试要求仍可切换 Workspace/Task/Scene、搜索、选择文本、复制和导出；创建、重命名、删除、Fork、Git、输入与 Handoff 显示禁用原因“数据库处于只读恢复模式”。
- [ ] 运行：

```bash
pnpm --filter @matou/runtime exec vitest run src/storage/runtime-access-policy.test.ts
pnpm --filter @matou/desktop exec vitest run src/renderer/src/recovery/ReadOnlyRecoveryBanner.test.tsx
```

- [ ] 在 Runtime RPC/terminal 入口做唯一权威 guard；Renderer 的 disabled 仅负责即时反馈。
- [ ] HierarchyShell 在 read-only 模式跳过会写 `bootstrap-window` 的路径，直接请求只读 projection；持续显示恢复 banner 和导出入口。
- [ ] 重跑 runtime/desktop 测试、typecheck、build。

**Acceptance:** 通过 DevTools 或手工构造协议也写不进只读 DB；浏览、搜索、复制与导出仍可使用；重启后不会出现“本次修改消失”。

---

## Task 6：持久化会话与运行环境的独立绑定

**Files:**

- Modify: `apps/runtime/src/storage/migrations.ts`（新增 migration v22）
- Modify: `apps/runtime/src/storage/migration-runner.test.ts`
- Create: `apps/runtime/src/session/session-environment-repository.ts`
- Create: `apps/runtime/src/session/session-environment-repository.test.ts`
- Modify: `packages/domain/src/model.ts`
- Modify: `apps/runtime/src/session-canvas/session-graph-repository.ts`
- Modify: `apps/runtime/src/session-canvas/session-graph-repository.test.ts`

**Schema:**

```sql
CREATE TABLE session_environment_bindings (
  session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
  local_execution_context_id TEXT NOT NULL REFERENCES execution_contexts(id),
  managed_worktree_id TEXT UNIQUE REFERENCES worktrees(id),
  active_target TEXT NOT NULL CHECK (active_target IN ('local', 'worktree')),
  state TEXT NOT NULL CHECK (state IN ('ready', 'missing', 'recovering', 'handoff', 'failed')),
  error_message TEXT,
  updated_at INTEGER NOT NULL
) STRICT;
```

**TDD steps:**

- [ ] migration 测试从 v21 数据库升级：普通 Session 回填 local；独立 worktree Session 回填 `managed_worktree_id + active_target=worktree`；会话和 DAG relation 数量保持一致。
- [ ] repository 测试覆盖：切到 local 后仍保留自己的 managed worktree；同一 worktree 仅归属一个 Session 的独占绑定；环境状态变化不修改 Session archived/status。
- [ ] projection 测试覆盖：worktree missing 时 node、latestLines、父子关系仍在；environment 与 git 独立投影。
- [ ] 运行：

```bash
pnpm --filter @matou/runtime exec vitest run \
  src/storage/migration-runner.test.ts \
  src/session/session-environment-repository.test.ts \
  src/session-canvas/session-graph-repository.test.ts
```

- [ ] 实现 repository 的 `get(sessionId)`、`bindOwnedWorktree(...)`、`beginTransition(...)`、`completeTransition(...)`、`markMissing(...)`、`markFailed(...)`。
- [ ] Fork 创建新 worktree 时写独占绑定；选择 current worktree 时 active target 保持 local/shared 环境，不抢占其他 Session 的 owned binding。
- [ ] 重跑测试和 typecheck。

**Acceptance:** Session 从 Worktree Handoff 到 Local 后，Worktree 关系仍可恢复；Worktree 行删除/失效不级联删除 Session；选择其他 Worktree 时可以通过 owned binding 找到并切换对应 Session。

---

## Task 7：Worktree 健康检查与启动对账

**Files:**

- Create: `apps/runtime/src/worktrees/worktree-health-service.ts`
- Create: `apps/runtime/src/worktrees/worktree-health-service.test.ts`
- Create: `apps/runtime/src/worktrees/worktree-reconciler.ts`
- Create: `apps/runtime/src/worktrees/worktree-reconciler.test.ts`
- Modify: `apps/runtime/src/worktrees/worktree-service.ts`
- Modify: `apps/runtime/src/runtime-server.ts`
- Modify: `apps/runtime/src/index.ts`

**Interfaces:**

```ts
export type WorktreeHealth =
  | { kind: 'ready'; canonicalPath: string; branch?: string; detachedHead?: string; dirty: boolean }
  | { kind: 'missing'; reason: 'path-missing' | 'not-listed-by-git' }
  | { kind: 'mismatch'; reason: 'wrong-repository' | 'wrong-branch' | 'wrong-head' }

export class WorktreeReconciler {
  reconcileAll(now: number): Promise<{ checked: number; repaired: number; degraded: number }>
}
```

**TDD steps:**

- [ ] 使用真实临时 Git repo 写 health 测试：ready、目录删除、移动后未登记、错误 repo、错误 branch、detached HEAD、dirty。
- [ ] reconciler 测试覆盖 `creating`：branch-only、directory-ready、完全缺失三种中断点；覆盖 `removing`：目录已删和 dirty retained。
- [ ] RuntimeServer 测试：managed worktree missing 时返回 `SESSION_ENVIRONMENT_UNAVAILABLE`，Session.cwd 不会被改成 workspace root 或 HOME；普通 local context 继续走 workspace path 策略。
- [ ] 运行：

```bash
pnpm --filter @matou/runtime exec vitest run \
  src/worktrees/worktree-health-service.test.ts \
  src/worktrees/worktree-reconciler.test.ts \
  src/runtime-server.test.ts
```

- [ ] 实现基于 `realpath`、`git worktree list --porcelain`、`rev-parse --show-toplevel`、`symbolic-ref --short HEAD` 和 `status --porcelain` 的 identity 检查。
- [ ] 对账只更新 Worktree/environment operation state；Session、history、graph、provider binding 保持原值。
- [ ] 在 spawn 选 cwd 之前先检查 `SessionEnvironment`；删除 managed worktree 的 workspace/HOME fallback。
- [ ] 重跑测试和 typecheck。

**Acceptance:** 任何 managed worktree identity 不一致时 provider/PTY 都不会启动；会话仍能打开并查看历史；creating/removing 中断可以在重启后收敛到 ready、retained、removed 或 failed。

**Real fault injection:** 创建真实 worktree 后分别在 branch 创建后、目录生成后、DB 标记 ready 前强杀 Runtime；重启后检查 worktree 数量、路径、branch 和 DB state 一致。

---

## Task 8：恢复、定位与 Local/Worktree Handoff

**Files:**

- Create: `apps/runtime/src/session/session-environment-service.ts`
- Create: `apps/runtime/src/session/session-environment-service.test.ts`
- Modify: `packages/contracts/src/protocol.ts`
- Modify: `apps/runtime/src/rpc/runtime-rpc-router.ts`
- Modify: `apps/runtime/src/runtime-server.ts`
- Modify: `apps/desktop/src/renderer/src/hierarchy/hierarchy-commands.ts`
- Modify: `apps/desktop/src/renderer/src/hierarchy/hierarchy-types.ts`
- Modify: `apps/desktop/src/shared/desktop-api.ts`
- Modify: `apps/desktop/src/preload/index.ts`
- Modify: `apps/desktop/src/main/index.ts`

**RPC methods:** `session.environment-restore`、`session.environment-locate`、`session.environment-handoff`、`session.environment-open`。

**TDD steps:**

- [ ] restore 测试：缺失 owned worktree 使用原 `worktreeId/path/branch` 重建，同一 Session 不新增第二条 binding；成功后才切回 active target。
- [ ] locate 测试：用户选择的目录必须属于同 repository 且 branch/HEAD 与 owned worktree identity 匹配；错误目录保持 missing，并返回稳定 reason。
- [ ] Handoff 测试：先停/暂停当前 run，持久化 `state=handoff`，验证目标，原子更新 Session execution context/cwd/active target，再按同一 provider binding 启动；中断后 reconciler 可继续或回到原目标。
- [ ] “选择其他 Worktree”测试：返回 `{ kind: 'switch-session', sessionId }`，不会重绑当前 Session。
- [ ] 运行：

```bash
pnpm --filter @matou/runtime exec vitest run src/session/session-environment-service.test.ts
```

- [ ] Renderer 定位动作复用原生目录选择器；路径只经 preload 传给 Runtime 校验。
- [ ] Finder/终端打开由 Main 完成，Runtime 返回当前 canonical path；missing path 时打开动作禁用。
- [ ] 重跑 tests/typecheck。

**Acceptance:** restore、locate、Handoff 全程状态可恢复；操作中断不把 Session 资产归档；切换别人的 Worktree 实际切换到对应会话。

**Real fault injection:** 把 worktree 移到新目录后启动 App，选择“定位”并绑定新路径；再选择错误 repo 和错误 branch，验证两次都保持输入锁定。

---

## Task 9：右下角独立展示运行环境与 Git 状态

**Files:**

- Create: `apps/desktop/src/renderer/src/hud/EnvironmentControlMenu.tsx`
- Create: `apps/desktop/src/renderer/src/hud/EnvironmentControlMenu.test.tsx`
- Modify: `apps/desktop/src/renderer/src/hud/TerminalHud.tsx`
- Modify: `apps/desktop/src/renderer/src/hud/TerminalHud.test.tsx`
- Modify: `apps/desktop/src/renderer/src/hud/GitControlMenu.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/TerminalPane.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/hierarchy.css`

**TDD steps:**

- [ ] HUD 测试要求右下角始终是两个独立按钮：环境显示 `Local / Worktree / 待恢复 / 交接中`，Git 显示 branch 或 detached HEAD 和 dirty `*`。
- [ ] Environment menu 测试覆盖路径、Finder、终端、恢复、定位、Handoff；missing 状态只开放恢复/定位/Handoff 到 Local。
- [ ] 卡片测试要求 missing/recovering/handoff 覆盖完整卡片 Loading/恢复层，历史仍在 DOM，输入与卡片 mutation 操作锁定。
- [ ] 运行：

```bash
pnpm --filter @matou/desktop exec vitest run \
  src/renderer/src/hud/EnvironmentControlMenu.test.tsx \
  src/renderer/src/hud/TerminalHud.test.tsx \
  src/renderer/src/hierarchy/TerminalPane.test.tsx
```

- [ ] 从现有 TerminalHud 的 cwd/Git 混合展示中抽出 EnvironmentControlMenu；GitControlMenu 只负责 Git 状态和 Git 操作。
- [ ] 进行 Kooky 黑色区域截图对照，确认新增状态位于右下快捷栏，卡片尺寸、滚动宽度和现有 HUD 优先级不跳动。
- [ ] 重跑 component tests、desktop typecheck/build。

**Acceptance:** 用户一眼可以区分“运行在哪”和“Git 处于什么状态”；worktree 缺失不会只显示一个泛化路径错误；恢复期间整卡操作被锁定且阶段持续可见。

---

## Task 10：Fork intent 增加 operation identity、阶段、fencing 与重复提交门槛

**Files:**

- Modify: `apps/runtime/src/storage/migrations.ts`（新增 migration v23）
- Modify: `apps/runtime/src/storage/migration-runner.test.ts`
- Modify: `apps/runtime/src/session/session-fork-intent-repository.ts`
- Modify: `apps/runtime/src/session/session-fork-intent-repository.test.ts`
- Modify: `apps/runtime/src/session-canvas/fork-workflow-service.ts`
- Modify: `apps/runtime/src/session-canvas/fork-workflow-service.test.ts`
- Modify: `apps/runtime/src/session-canvas/session-graph-repository.ts`

**Schema additions:** `operation_id`、`submission_key`、`stage`、`completed_steps`、`total_steps`、`lease_owner`、`lease_token`、`lease_expires_at`、`last_heartbeat_at`；`submission_key` 唯一。

**TDD steps:**

- [ ] migration 测试把旧 pending/starting intent 映射到 `stage=queued`，succeeded/failed 映射到终态。
- [ ] repository 测试：同一 submissionKey 连续受理两次返回同一个 operation/session/worktree；不同 key 才创建新操作。
- [ ] fencing 测试：lease token B 接管后，token A 的完成写入返回 stale，不覆盖 B 的阶段与错误。
- [ ] stage 测试只允许按顺序推进或进入 failed；retry 增加 attempt 并回到失败阶段对应的可重试起点。
- [ ] 运行：

```bash
pnpm --filter @matou/runtime exec vitest run \
  src/storage/migration-runner.test.ts \
  src/session/session-fork-intent-repository.test.ts \
  src/session-canvas/fork-workflow-service.test.ts
```

- [ ] 将 create Fork 拆为 `acceptFork(...) -> ForkProgress` 和后台 `executeFork(operationId, lease)`；accept 事务一次性创建 Session、关系、binding、worktree intent。
- [ ] `SceneSessionGraph` 投影 `ForkProgress`；失败仍保留卡片、Session 和 operation identity。
- [ ] 重跑测试和 typecheck。

**Acceptance:** 双击、网络重发、Renderer 超时重试都只产生一个 Fork；旧 Runtime 的迟到结果由 fencing 判为 stale；每个阶段都能从 projection 还原。

---

## Task 11：Fork 后台协调器与重启对账

**Files:**

- Create: `apps/runtime/src/session-canvas/fork-operation-coordinator.ts`
- Create: `apps/runtime/src/session-canvas/fork-operation-coordinator.test.ts`
- Create: `apps/runtime/src/session/session-execution-service.ts`
- Create: `apps/runtime/src/session/session-execution-service.test.ts`
- Modify: `apps/runtime/src/session-canvas/fork-workflow-service.ts`
- Modify: `apps/runtime/src/worktrees/worktree-service.ts`
- Modify: `apps/runtime/src/runtime-server.ts`
- Modify: `apps/runtime/src/index.ts`
- Modify: `apps/runtime/src/notifications/agent-notification-repository.ts`

**Behavior:** 后台 Git/setup 并发上限为 2；RPC 在 durable accept 后立即返回。协调器按 `creating-worktree → applying-setup → binding-session → restoring-provider → starting-window` 推进，并每 2 秒续租；启动时接管过期 lease。

**TDD steps:**

- [ ] fake clock 测试：两个操作并行，第三个 queued；前两个完成后第三个启动。
- [ ] kill-point 表格测试覆盖：intent commit 后、branch 创建后、目录创建后、setup 后、Session bind 后、provider spawn 前；每个点重启都使用同一 operation/worktree/branch/session。
- [ ] 长 setup 测试：accept 在 1 秒内返回，10 秒后仍显示阶段而非 RPC timeout；当前页面卸载后 operation 继续。
- [ ] provider 恢复测试：只有 authoritative provider identity 到达才进入 starting-window/succeeded；普通 2001 字符输出不判成功。
- [ ] 通知测试：后台完成和失败各产生一条可定位 Session 的通知；重启重复对账不重复通知。
- [ ] 运行：

```bash
pnpm --filter @matou/runtime exec vitest run \
  src/session-canvas/fork-operation-coordinator.test.ts \
  src/session/provider-resume-monitor.test.ts
```

- [ ] 协调器通过 WorktreeHealthService 判断外部 Git 实际状态，再决定复用 branch、继续 add、跳过已完成步骤或进入 failed。
- [ ] 从 RuntimeServer 抽出 `SessionExecutionService.startOrResume(sessionId)`；后台 Fork 直接启动 PTY/provider，Renderer 只负责 attach xterm，因此切走 Workspace 后 provider 阶段仍可继续。
- [ ] setup 每一步把开始/结果写入 `setup_result_json`；当前生产 setup policy 为空，未来新增 setup 命令必须声明幂等 key 后才能进入后台恢复。
- [ ] 删除固定 10 秒 Fork 完成等待；协议中不增加 cancel action。
- [ ] 重跑测试/typecheck。

**Acceptance:** Fork 可以离开当前页面继续；App 重启后继续同一操作；阶段和结果有通知；进行中重复提交在 Renderer 和 Runtime 两层都被拦截。

**Real fault injection:** 使用 setup command `node -e "setTimeout(() => process.exit(0), 15000)"`，操作中强杀 Runtime，重启后检查只有一个 worktree、一个 branch、一个 child Session，最终进入 succeeded 或带明确错误的 failed。

---

## Task 12：Fork 全卡阶段 Loading、后台反馈与失败清理

**Files:**

- Modify: `apps/desktop/src/renderer/src/session-canvas/BranchDialog.tsx`
- Modify: `apps/desktop/src/renderer/src/session-canvas/BranchDialog.test.tsx`
- Create: `apps/desktop/src/renderer/src/session-canvas/ForkProgressOverlay.tsx`
- Create: `apps/desktop/src/renderer/src/session-canvas/ForkProgressOverlay.test.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/TerminalPane.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/hierarchy-commands.ts`
- Modify: `apps/desktop/src/renderer/src/session-canvas/session-canvas.css`
- Modify: `apps/desktop/src/renderer/src/notifications/agent-event-ingestion.ts`

**TDD steps:**

- [ ] BranchDialog 测试：首次提交后按钮立即禁用，重复 Enter/click 复用同一 submissionKey；accepted 后关闭弹窗并聚焦创建中的卡片。
- [ ] overlay 测试逐项验证中文阶段：等待创建、正在创建 Worktree、正在应用修改、正在初始化环境、正在恢复 Provider、正在启动窗口。
- [ ] 进行中 overlay 没有取消按钮；失败状态提供“重试”和“清理残留 Worktree”，dirty 残留显示保留提示。
- [ ] 页面切走后通知测试：完成/失败通知点击可回到对应 Scene/Session。
- [ ] 运行：

```bash
pnpm --filter @matou/desktop exec vitest run \
  src/renderer/src/session-canvas/BranchDialog.test.tsx \
  src/renderer/src/session-canvas/ForkProgressOverlay.test.tsx \
  src/renderer/src/notifications/notification-ui-integration.test.tsx
```

- [ ] 用 `ForkProgress` 驱动整卡 overlay，移除基于 RPC Promise 和本地定时器推断完成的状态。
- [ ] 清理残留调用 WorktreeService 的 clean-only removal；dirty worktree进入 retained 并提供 Finder 入口。
- [ ] 重跑 component tests、typecheck、build，并补 Kooky 对照截图。

**Acceptance:** 长 Fork 没有 10 秒假失败；离开页面后继续；返回时从 projection 恢复准确阶段；当前版本 UI 没有取消入口；失败残留可安全清理。

---

## Task 13：单会话 durability gate 与受影响 PTY 暂停/恢复

**Files:**

- Create: `apps/runtime/src/session/session-durability-gate.ts`
- Create: `apps/runtime/src/session/session-durability-gate.test.ts`
- Create: `apps/runtime/src/session/pty-execution-pauser.ts`
- Create: `apps/runtime/src/session/pty-execution-pauser.test.ts`
- Modify: `apps/runtime/src/session/pty-session.ts`
- Modify: `apps/runtime/src/session/pty-session.test.ts`
- Modify: `apps/runtime/src/runtime-server.ts`
- Modify: `apps/runtime/src/journal/segment-journal.ts`

**Interfaces:**

```ts
export interface PendingDurableFrame {
  sequence: number
  kind: 'output' | 'resize' | 'exit'
  bytes: Uint8Array
}

export class SessionDurabilityGate {
  append(frame: PendingDurableFrame): Promise<void>
  retry(): Promise<void>
  get state(): 'healthy' | 'pausing' | 'paused' | 'recovering'
  get retainedBytes(): number
}
```

**TDD steps:**

- [ ] 测试第 N 帧抛 ENOSPC：N 之前已落盘，N 及之后进入最大 4 MiB FIFO，状态只触发一次 paused，sequence 不跳号，write chain 后续可 retry。
- [ ] 测试 buffer 达到 4 MiB：PTY 已先暂停，因此不继续接收新输出；内存不会无界增长。
- [ ] 两 Session 测试：A journal 抛 EACCES 只暂停 A；B 的 append、输入和输出继续。
- [ ] POSIX 真实进程测试：pauser 对 PTY process group 发送 SIGSTOP，计数文件停止增长；SIGCONT 后继续。`IPty.pause/resume` 同时控制读取背压。
- [ ] RuntimeServer 测试：storage-fault 后拒绝该 Session 新输入，其他 Session 消息不受影响；retry 成功按原顺序发送 retained output 并恢复输入。
- [ ] 运行：

```bash
pnpm --filter @matou/runtime exec vitest run \
  src/session/session-durability-gate.test.ts \
  src/session/pty-execution-pauser.test.ts \
  src/session/pty-session.test.ts
```

- [ ] 将 `onOutput` 元数据副作用移动到 journal append 成功后；storage error 不再毒化整个 Promise chain。
- [ ] 首次失败调用 `IPty.pause()`，当前 macOS/Linux 内部版同时对进程组发送 SIGSTOP；恢复时先落盘 retained frames，再 SIGCONT 和 `IPty.resume()`。
- [ ] exit/close 独立使用可恢复链，确保 storage fault 后 Runtime shutdown 仍有界完成。
- [ ] 重跑 journal、PTY、RuntimeServer 测试和 typecheck。

**Acceptance:** 首次持久化失败后只有对应执行进程暂停；用户已看到的输出要么已在 journal，要么在 4 MiB retained buffer；修复后从失败 sequence 继续，无重复和缺口。

**Real fault injection:** 运行两个真实 Shell，在 A 的 journal session 目录撤销写权限；A 停在 storage-fault，B 继续完成命令。恢复权限并点击重试，A retained marker 只出现一次且原进程继续。

---

## Task 14：存储异常卡片、重试、结束会话与 Claude 恢复

**Files:**

- Create: `apps/desktop/src/renderer/src/hierarchy/StorageFaultOverlay.tsx`
- Create: `apps/desktop/src/renderer/src/hierarchy/StorageFaultOverlay.test.tsx`
- Modify: `apps/desktop/src/renderer/src/terminal/TerminalSurface.tsx`
- Modify: `apps/desktop/src/renderer/src/terminal/TerminalSurface.test.tsx`
- Modify: `apps/desktop/src/renderer/src/runtime/RuntimeClient.ts`
- Modify: `apps/desktop/src/renderer/src/hierarchy/TerminalPane.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/hierarchy.css`
- Modify: `apps/runtime/src/rpc/runtime-rpc-router.ts`

**TDD steps:**

- [ ] RuntimeClient 测试接收 `terminal.storage-fault/recovered` 并只通知对应 terminal consumer。
- [ ] Overlay 测试显示错误类型、保留输出大小、“重试写入”“结束会话”；fault 期间 terminal input、resize 和 provider action 被锁定。
- [ ] 重试测试要求按钮防重复，恢复消息到达后 overlay 淡出并恢复焦点。
- [ ] Claude 测试：暂停时间导致网络连接过期后仍以原 provider session ID resume，而不是新建对话。
- [ ] 运行：

```bash
pnpm --filter @matou/desktop exec vitest run \
  src/renderer/src/hierarchy/StorageFaultOverlay.test.tsx \
  src/renderer/src/terminal/TerminalSurface.test.tsx \
  src/renderer/src/runtime/RuntimeClient.test.ts
```

- [ ] 增加 RPC `session.retry-storage` 与 `session.end-after-storage-fault`；结束会话先尽力落盘 retained buffer，再标记 interrupted/exited，不删除 Session。
- [ ] retry 成功后 Claude provider connection 失效则走现有 provider resume binding。
- [ ] 重跑 tests/typecheck/build。

**Acceptance:** 存储异常不会表现为终端无响应；用户能识别影响范围并重试或结束；其他卡片继续运行和交互。

---

## Task 15：启动分层恢复协调器与活动会话优先

**Files:**

- Create: `apps/runtime/src/recovery/runtime-recovery-coordinator.ts`
- Create: `apps/runtime/src/recovery/runtime-recovery-coordinator.test.ts`
- Modify: `apps/runtime/src/recovery/runtime-recovery-service.ts`
- Modify: `apps/runtime/src/index.ts`
- Modify: `apps/runtime/src/runtime-server.ts`
- Modify: `apps/desktop/src/renderer/src/runtime/RuntimeProvider.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx`
- Modify: `apps/desktop/src/renderer/src/hierarchy/TerminalPane.tsx`

**Consumes:** Tasks 3、7、11、13；会话有界队列与 checkpoint/history 的具体吞吐参数对接性能实施计划，接口在本任务固定。

**Behavior:** 顺序为 database → worktrees → forks → 当前活动 Session → 后台 Session。当前活动 Session ready 后允许进入工作现场；后台队列默认并发 4，前台横向列表内其余 Session 优先于其他页签/事项/工作空间。

**TDD steps:**

- [ ] 状态机测试要求 stage 单向推进，单个 worktree/session 失败进入 failures 并继续其他资源；database recovery-required 是唯一全局 blocked 状态。
- [ ] 优先级测试构造当前活动、同场景前台、其他页签、其他 Task、其他 Workspace，断言队列顺序和最大并发 4。
- [ ] 删除/停止语义测试：archived/deleted 不入队，显式 stopped 保持停止，running/needs-input/interrupted 按合同入队；未完成 Shell 命令标记 interrupted 且不重放输入。
- [ ] Renderer 测试：当前卡片和每个恢复中的卡片显示自己的阶段；当前活动完成后即可操作，不等待后台总数归零。
- [ ] 运行：

```bash
pnpm --filter @matou/runtime exec vitest run \
  src/recovery/runtime-recovery-coordinator.test.ts \
  src/recovery/runtime-recovery-service.test.ts
pnpm --filter @matou/desktop exec vitest run src/renderer/src/hierarchy/HierarchyShell.test.tsx
```

- [ ] coordinator 每层发布 Task 1 的 `RuntimeRecoverySnapshot`；failure 包含 layer/resourceId/code，禁止只写 console。
- [ ] RuntimeServer 的 terminal attach/spawn 服从 coordinator permit；后台恢复只恢复 PTY，Renderer 进入对应前台时再绑定 xterm 并从 checkpoint/journal 补画面。
- [ ] 重跑 recovery、projection、desktop component tests、typecheck/build。

**Acceptance:** App 先恢复当前工作现场；后台故障不会把全局 ready 卡住；删除节点不会复活；恢复进度来自真实状态而非定时器。

---

## Task 16：真实故障注入、Electron E2E 与回归门槛

**Files:**

- Create: `tests/e2e/recovery-database.spec.ts`
- Create: `tests/e2e/recovery-worktree.spec.ts`
- Create: `tests/e2e/recovery-fork-reconciliation.spec.ts`
- Create: `tests/e2e/recovery-storage-fault.spec.ts`
- Modify: `tests/e2e/prd-04-session-recovery.spec.ts`
- Modify: `tests/e2e/prd-06-session-fork.spec.ts`
- Create: `tests/fixtures/fault-injection/git-worktree-killpoints.ts`
- Create: `tests/fixtures/fault-injection/sqlite-faults.ts`
- Create: `tests/fixtures/fault-injection/pty-storage-fault.ts`
- Create: `docs/audits/recovery-acceptance-matrix.md`

**TDD/E2E matrix:**

- [ ] **DB read-only:** chmod 数据目录与 DB；进入只读恢复模式，浏览/搜索/复制/导出成功，所有写入口禁用，重启数据不变。
- [ ] **DB corruption:** header 损坏和中段页损坏各跑一次；只出现恢复页；从最新有效备份恢复后 Workspace/Task/Session/关系一致。
- [ ] **Backup rotation:** 生成 9 份备份只列 7 份；破坏最新备份后默认选择下一份有效备份。
- [ ] **Migration kill:** 在 pre-migration backup 完成后、第一条 migration 前强杀；重启后原 DB 或备份可恢复，migration 幂等完成。
- [ ] **Missing worktree:** 删除和移动目录各跑一次；Session/history/DAG 保留，输入锁定，workspace root/HOME 中没有启动 provider。
- [ ] **Environment actions:** restore、locate、Handoff Local、Handoff 回自有 Worktree；错误 repo/branch locate 保持待恢复。
- [ ] **Fork killpoints:** intent、branch、directory、setup、binding、provider 六个点强杀；每次重启最终只有一个 operation/session/branch/worktree。
- [ ] **Fork background:** 15 秒 setup 期间切换 Workspace，原操作继续，完成后通知可定位；连续双击/Enter 只创建一份。
- [ ] **Storage permission loss:** 两个真实 PTY 中只暂停故障 Session；恢复权限并重试后 retained output 无重复，Claude 使用原 provider session。
- [ ] **Deletion contract:** 把 `prd-04-session-recovery:227` 拆成 `test.step`，分别记录 Task、Scene、terminal 删除、窗口关闭、重启、最终断言；三类删除均不复活。
- [ ] **Scale smoke:** 100 个 Session 中 20 个需要恢复、10 个 worktree、5 个 interrupted Fork；当前活动 Session 优先 ready，后台并发不超过配置，恢复错误可定位。

**Commands:**

```bash
pnpm --filter @matou/contracts test
pnpm --filter @matou/domain test
pnpm --filter @matou/runtime test
pnpm --filter @matou/desktop test
pnpm -r typecheck
pnpm -r build
pnpm exec playwright test \
  tests/e2e/recovery-database.spec.ts \
  tests/e2e/recovery-worktree.spec.ts \
  tests/e2e/recovery-fork-reconciliation.spec.ts \
  tests/e2e/recovery-storage-fault.spec.ts \
  tests/e2e/prd-04-session-recovery.spec.ts \
  tests/e2e/prd-06-session-fork.spec.ts \
  --trace on --workers=1
```

**Acceptance:** `recovery-acceptance-matrix.md` 每个场景包含产品合同、故障注入方式、截图/trace、DB/Git/进程证据和结论；全部恢复相关测试串行重复 3 次均通过；完整 E2E 中没有恢复相关超时、静默 fallback、重复 Fork 或删除复活。

---

## 2. 实施批次与评审门槛

### Batch A：数据库数据安全

- Tasks 1–5。
- 评审门槛：只读目录真实启动、两类损坏真实启动、七份备份轮转、恢复页截图与原数据恢复证据。

### Batch B：会话环境与 Worktree

- Tasks 6–9。
- 评审门槛：missing worktree 绝不落回 workspace/HOME；restore/locate/Handoff 真实 Git E2E；右下角环境与 Git 双状态 Kooky 对照。

### Batch C：Fork 长任务

- Tasks 10–12。
- 评审门槛：六个 killpoint 对账、重复提交、15 秒后台 setup、完成/失败通知、残留 clean/dirty 处理。

### Batch D：运行中存储故障与总恢复编排

- Tasks 13–16。
- 评审门槛：单会话暂停隔离、retained output 顺序、provider resume、活动会话优先、删除不复活、完整 E2E 三连跑。

## 3. 关键外部依赖

1. **性能计划的 checkpoint/history producer**：Task 15 只固定恢复优先级和并发接口；10,000 行即时恢复、旧历史压缩索引、256 MiB raw journal 由性能实施计划提供。
2. **通知保留计划**：Fork 完成/失败和待恢复状态先复用现有通知投影；1,000 条/30 天清理策略由 D-12 对应计划实现。
3. **macOS 进程暂停语义**：当前内部版使用 PTY process group `SIGSTOP/SIGCONT`；Windows 发布前需要等价的 ConPTY 进程树暂停实现及独立真实进程验收。
4. **迁移序号协调**：本计划按当前 schema v21 使用 v22/v23；其他并行实现若先加入 migration，合并时保持 SQL 内容和依赖顺序，顺延版本号并更新 checksum 测试。

## 4. 本计划范围边界

- 本计划覆盖恢复、数据库、Worktree、Fork 长任务与存储故障隔离。
- DAG 远层聚合、通知长期清理、大段粘贴、拖入路径和 Resize 分别进入对应性能/交互计划。
- 实施中涉及的产品语义均来自已定稿 D-02/D-03/D-05/D-06/D-07/D-11；正常工程选择按本计划连续推进。
