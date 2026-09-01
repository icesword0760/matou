# Matou INF-01～INF-25 基础设施设计规格

- 状态：待用户书面复核
- 日期：2026-08-24
- 目标：完成基础设施准入门后，再进入通知、HUD、Fork、Teams、DAG 等功能开发
- 技术栈：Electron + React + xterm.js + UtilityProcess + node-pty + SQLite

## 1. 决策依据与冲突优先级

需求发生冲突时按以下顺序裁决：

1. 当前对话中用户确认的架构决策；
2. `智能体终端升级`目录中的最终 PRD；
3. reference product 当前代码所表达的既有行为；
4. Matou 既有架构文档；
5. 工程默认值。

本规格主动覆盖 PRD 06 中“Fork 事实不持久化”的旧范围。Matou 必须持久化 Fork 关系，才能形成可恢复的 Session Graph。

主要证据：

- reference product 的 Project → Workbench → Tab → Panel 层级分别位于：
  - `/Users/icesword/Documents/AIProjects/kookey/src/modules/terminal/stores/project.js`
  - `/Users/icesword/Documents/AIProjects/kookey/src/modules/terminal/stores/workbench.js`
  - `/Users/icesword/Documents/AIProjects/kookey/src/modules/terminal/stores/tab.js`
  - `/Users/icesword/Documents/AIProjects/kookey/src/modules/terminal/stores/panel.js`
- reference product Panel 同时持有 UI、终端、provider 和 team 字段，见 `panel.js:140-211`。
- reference product 的 snapshot/checkpoint/metadata journal 位于 `electron/session-persistence.js:17-35,506-625,695-740,2195-2244`。
- PRD 05 定义四级层级和独立分屏布局，见 `05-四级层级管理.md` §4.2、§4.6～§4.13。
- PRD 06 定义 Fork 的上下文继承与独立身份，见 `06-会话增强-fork.md` §4.2～§4.4。
- PRD 07 定义团队成员、身份、状态和恢复，见 `07-会话增强-agent-teams.md` §4.1～§4.4。
- PRD 03 定义 Task 级状态、进度和日志通道，见 `03-工作台状态.md` §4.2～§4.4。
- PRD 08 定义 Agent 对宿主终端的零配置控制面、稳定目标编号和默认拒绝边界，见 `08-AI控制宿主终端.md` §4.1～§4.6。
- PRD 09 定义独立于应用版本的功能介绍批次及本地已读状态，见 `09-新功能介绍.md` §4.1～§4.4。
- PRD 10 定义预置插件/能力的声明、安装、升级、移除抑制和离线来源，见 `10-预置插件.md` §4.1～§4.6。
- PRD 04 定义恢复、provider resume、失败隔离和数据清理，见 `04-会话持久化和自动恢复.md` §4.2～§4.4。

## 2. 核心结论

### 2.1 权威业务层级

```text
Workspace
└── Task
    ├── ExecutionContext
    ├── Session
    │   ├── SessionRun
    │   └── ProviderBinding
    ├── SessionRelation
    ├── Scene
    │   ├── SceneNode
    │   ├── SessionMount
    │   └── SceneGeometry
    ├── Annotation
    ├── Artifact
    └── ValidationRun
```

映射规则：

```text
reference product Project   → Matou Workspace
reference product Workbench → Matou Task
reference product Tab       → Matou Scene
reference product Panel     → Matou Session + SessionMount
reference product terminalId      → legacy SessionRun reference
reference product claudeSessionId → ProviderBinding
reference product layoutRoot      → SceneNode structure + SceneGeometry
```

Session 是稳定的工作会话，Scene 是展示投影。Session 可以从 tile、card、DAG 或独立窗口呈现，切换 Scene 不改变 Session 身份和运行进程。

### 2.2 三类持久化

| 类型 | 介质 | 写入者 | 用途 |
|---|---|---|---|
| 领域元数据 | SQLite | Runtime | Workspace、Task、Session、Relation、Scene 结构等 |
| 终端字节流 | 分段 Journal 文件 | Runtime | PTY output、resize、reset、exit、domain cursor |
| 终端画面快照 | Checkpoint 文件 + SQLite 索引 | Runtime | 快速恢复 xterm 状态与双流水位 |

### 2.3 Runtime 独占写入

- Runtime UtilityProcess 是 SQLite 文件的唯一所有者和写入者。
- V1 使用 Electron 内置 Node 的 `node:sqlite` `DatabaseSync`。
- Main、Preload、Renderer 均通过 RPC 访问领域状态。
- Runtime 内部使用一个写连接和串行 Storage Queue。
- PTY Journal I/O 与 SQLite metadata queue 分离，避免终端高频流阻塞领域事务。

数据库启动参数：

```sql
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
PRAGMA synchronous = FULL;
PRAGMA busy_timeout = 5000;
PRAGMA trusted_schema = OFF;
```

元数据写入量远低于 PTY output，因此 V1 优先选择 `synchronous=FULL` 的崩溃一致性。后续只能通过基准测试和 ADR 调整。

## 3. INF-01：SQLite 所有权与连接模型

Runtime 新增 `storage/Database.ts`：

- 创建数据目录和数据库文件；
- 设置 PRAGMA；
- 启动时执行 integrity/migration 检查；
- 暴露 query、transaction、close；
- 记录 runtime generation；
- 拒绝 transaction callback 逃逸；
- 将 SQLite 错误映射为稳定的 Runtime 错误码。

所有 Repository 只能依赖 `DatabaseTransaction`，业务模块不能直接构造数据库连接。通过依赖边界测试阻止 Renderer/Main 引入 `node:sqlite`。

## 4. INF-02：Schema Migration

```sql
CREATE TABLE schema_migrations (
  version INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  checksum TEXT NOT NULL,
  applied_at INTEGER NOT NULL
);
```

规则：

- migration 使用编号 SQL 文件；
- checksum 改变视为发布错误；
- 每个 migration 在单独事务中运行；
- migration 前调用 SQLite backup API 生成升级备份；
- schema migration 与 IPC protocol version 分开；
- reference product import 版本由 `legacy_import_runs` 管理，不复用 schema version。

## 5. INF-03：事务与事件原语

唯一领域写入口：

```ts
withDomainTransaction(commandContext, async tx => {
  const result = await repository.mutate(tx, command)
  await tx.emit(domainEvent)
  return result
})
```

约束：

- mutation、同步 projection 和 domain event insert 在同一 SQLite transaction；
- command 必须携带 `commandId`；
- `command_deduplication` 保存 commandId、request hash、result reference；
- 相同 commandId + 相同 request 返回原结果；
- 相同 commandId + 不同 request 返回 `IDEMPOTENCY_CONFLICT`；
- Repository 写方法首参必须是 transaction；
- Scene geometry 使用 `withUiStateTransaction`，不产生 Domain Event；
- transaction 使用 `BEGIN IMMEDIATE`，避免写事务在执行中后段才升级锁。

## 6. INF-04：Domain Events / Outbox

Outbox 是 V1 Semantic Event Plane 的持久化源，而非未来扩展项。

```sql
CREATE TABLE domain_events (
  sequence INTEGER PRIMARY KEY AUTOINCREMENT,
  event_id TEXT NOT NULL UNIQUE,
  aggregate_type TEXT NOT NULL,
  aggregate_id TEXT NOT NULL,
  workspace_id TEXT,
  task_id TEXT,
  session_id TEXT,
  event_type TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  required_terminal_sequence INTEGER,
  occurred_at INTEGER NOT NULL,
  causation_id TEXT,
  correlation_id TEXT,
  schema_version INTEGER NOT NULL
);
```

配套表：

- `consumer_cursors(consumer_id, domain_sequence, updated_at)`；
- `command_deduplication(command_id, request_hash, result_json, committed_event_sequence)`。

发布规则：

1. SQLite commit 成功后唤醒 Outbox Publisher；
2. Publisher 按 sequence 查询新事件；
3. 每个 Renderer connection 独立维护内存 cursor；
4. 持久 consumer 可保存 cursor；
5. Renderer reconnect 发送 `afterDomainSequence`；
6. consumer 以 eventId 幂等；
7. domain event 表按 checkpoint/retention 策略压缩，压缩前必须存在可恢复投影 checkpoint。

## 7. INF-05～INF-07：双流、Journal 与 Checkpoint

### 7.1 Journal V2

每个 Session 使用独立目录：

```text
journals/<session-id>/
├── manifest.json
├── segment-000001.bin
├── segment-000002.bin
└── ...
```

frame：

```ts
type JournalFrame =
  | { kind: 'output'; terminalSequence: number; timestamp: number; data: Uint8Array }
  | { kind: 'resize'; terminalSequence: number; timestamp: number; cols: number; rows: number }
  | { kind: 'reset'; terminalSequence: number; timestamp: number; screenEpoch: number }
  | { kind: 'encoding'; terminalSequence: number; timestamp: number; encoding: 'utf-8' }
  | { kind: 'exit'; terminalSequence: number; timestamp: number; exitCode: number; signal?: number }
  | {
      kind: 'domain-cursor'
      terminalSequence: number
      domainEventSequence: number
      timestamp: number
    }
```

每帧包含 magic、formatVersion、frameLength、headerLength 和 checksum。Reader 遇到不完整尾帧时截断到最后有效 frame；中段 checksum 错误时封存损坏 segment，并从上一个 checkpoint 恢复。

### 7.2 跨介质写入顺序

SQLite 与 Journal 文件之间没有跨介质原子事务，采用可恢复有序协议：

1. 先把相关 terminal frame 写入 Journal，并取得 terminal sequence；
2. 在 SQLite transaction 中提交 mutation + domain event；
3. domain event 保存 `requiredTerminalSequence`；
4. SQLite commit 成功后追加 `domain-cursor` frame；
5. 再向 live subscribers 发布 terminal/domain 数据。

崩溃窗口：

| 崩溃位置 | 恢复结果 |
|---|---|
| Journal frame 前 | 两边都没有新事实 |
| Journal frame 后、SQLite commit 前 | 保留纯终端输出，不生成富组件 |
| SQLite commit 后、cursor frame 前 | 通过 domain event 的 `requiredTerminalSequence` 对齐 |
| cursor frame 后、发布前 | reconnect 从 durable cursor 补播 |
| 发布后、client ACK 前 | client 通过 eventId/sequence 幂等 |

### 7.3 恢复水位

```ts
interface RecoveryWatermark {
  terminalSequence: number
  domainEventSequence: number
}
```

Renderer 只在 terminal replay 达到 `requiredTerminalSequence` 后应用相关语义投影。无终端关联的事件立即应用。

### 7.4 Checkpoint

```ts
interface TerminalCheckpoint {
  sessionId: string
  runtimeGeneration: string
  throughTerminalSequence: number
  throughDomainEventSequence: number
  cols: number
  rows: number
  screenEpoch: number
  serializedTerminalState: Uint8Array
  formatVersion: number
  checksum: string
  createdAt: number
}
```

Checkpoint 文件使用临时文件、fsync、rename 和 parent directory fsync。`checkpoint.prev` 只用于文件 checkpoint；SQLite WAL 自身承担数据库原子性。SQLite `terminal_checkpoints` 只保存索引和 checksum。

## 8. INF-08～INF-10：核心领域实体

### 8.1 Workspace

字段：id、name、rootDirectory、pathIdentity、archivedAt、createdAt、updatedAt、version。

- 路径失效不会删除历史；
- 路径状态由 Runtime 检测并形成 projection；
- 默认 Workspace 是否已被用户显式移除，需要持久 preference，避免重启后重新创建。

### 8.2 Task

字段：id、workspaceId、parentTaskId、title、status、executionContextId、sortKey、archivedAt、createdAt、updatedAt、version。

- reference product Workbench 映射为 Task；
- 同一 Workspace 名称约束按 PRD/reference product 规则执行；
- Task 父子与 SessionRelation 分离；
- Task archive 保留 Session Graph 和 Journal。

### 8.3 ExecutionContext / Worktree

`execution_contexts` 使用 discriminated kind：plain-directory、git-worktree。

`worktrees` 保存 repositoryRoot、path、branch、baseRevision、state、setupPolicy、createdAt、retainedAt。

- Renderer 只提交 executionContextId；
- worktree 创建、检查、保留、删除由 Runtime 执行；
- dirty worktree 默认 retained；
- 删除 worktree 前结束绑定的 SessionRun；
- 非 Git Workspace 使用 plain-directory；
- setup policy 记录执行步骤和结果，便于审计。

### 8.4 Session / SessionRun / ProviderBinding

`sessions` 是逻辑会话：

- id、taskId、executionContextId、kind、status、title、archivedAt、createdAt、lastActivityAt、version。

`session_runs` 是每次运行实例：

- id、sessionId、runtimeGeneration、profile、pid、status、cols、rows、startedAt、exitedAt、exitCode、signal。

`provider_session_bindings`：

- id、sessionId、provider、providerSessionId、state、validatedAt、invalidatedAt、metadataJson。

同一 Matou Session 在 PTY 重建后保持 id；pid 和 terminal runtime id 只属于 SessionRun。Provider binding 保留历史，只有 validated binding 才参与 resume。

## 9. INF-11：SessionRelation 局部事件溯源

### 9.1 权威事件

```sql
CREATE TABLE session_relation_events (
  sequence INTEGER PRIMARY KEY AUTOINCREMENT,
  event_id TEXT NOT NULL UNIQUE,
  relation_id TEXT NOT NULL,
  operation TEXT NOT NULL CHECK (operation IN ('created', 'revoked', 'restored')),
  task_id TEXT NOT NULL,
  from_session_id TEXT NOT NULL,
  to_session_id TEXT NOT NULL,
  relation_kind TEXT NOT NULL,
  metadata_json TEXT NOT NULL,
  occurred_at INTEGER NOT NULL
);
```

### 9.2 同步当前投影

`session_relations_current` 与 relation event 在同一个 `withDomainTransaction` 中更新。查询无需等待异步 projection。

首批 kind：forked-from、derived-from、depends-on、supports、blocks、references、team-member-of。

规则：

- `forked-from` 的 from 是 child、to 是 parent；
- 一个 Session 最多一个 active forked-from parent；
- sibling 由共同 active parent 推导；
- forked-from 和 depends-on 在写入前做 cycle detection；
- relation 删除使用 revoked 事件；
- soft-deleted Session 保留历史边；
- V1 team-member-of 指向 leader Session，metadata 保存 providerTeamId、role、label、agentId；leader 关闭后节点仍作为历史节点存在；
- 若后续出现跨 leader 生命周期仍独立存在的复杂 team/group，再通过 migration 引入 SessionGroup，不在 V1 提前增加多态图节点。

## 10. INF-12～INF-13：Scene、Mount 与 Geometry

### 10.1 结构性状态

表：scenes、scene_nodes、session_mounts、scene_windows。

结构性变更：

- Scene 创建、删除；
- tile/card/dag 模式切换；
- Session mount/unmount；
- split node 创建、移动、删除；
- 独立窗口 attach/detach；
- 跨窗口迁移。

以上通过领域事务并产生事件。

### 10.2 几何性状态

`scene_geometry` 保存：

- split ratios；
- card x/y/width/height/scale/rotation；
- DAG viewport、zoom；
- scroll position；
- collapsed state；
- native window bounds。

几何写入走 Runtime RPC + debounce + coalescing，使用 `layoutRevision` 防止旧窗口覆盖新状态，不进入 Outbox。结构恢复优先，几何引用失效时只丢弃对应 geometry row。

Focus state 默认是 per-window 易失 projection；只有 PRD 要求重启恢复的 active Workspace/Task/Scene/Mount 才进入持久 UI state。

## 11. INF-14～INF-16：Adapters、Anchors、RPC 与 Projection

### 11.1 Agent Adapter

适配器：Claude Code、Codex、Generic Shell。

事件来源优先级：结构化 stream/hook → transcript → 明确标记为低置信度的终端解析。Provider event ID 映射到稳定 eventId，`tool_use_id` 保存为 sourceRef。

规范化语义：agent.message、agent.todo、agent.tool-started、agent.tool-finished、agent.permission-requested、file.changed、artifact.observed、validation.status-changed。

### 11.2 Anchors

- SemanticAnchor：sessionId + eventId + provider sourceRef；
- CommandOutputAnchor：sessionId + commandId + terminal sequence range；
- ScreenCaptureAnchor：sessionId + screenEpoch + sequence + geometry + capturedText。

xterm Marker/Decoration 只作为显示缓存。Journal retention 导致定位数据缺失时，Annotation 进入 degraded 状态，仍展示 captured text。

### 11.3 RPC

MessagePort 协议扩展为：

- Commands：领域修改；
- Queries：snapshot/read model；
- Subscriptions：domain events；
- Terminal stream：data/ack/replay；
- Geometry updates：coalesced UI state；
- Diagnostics：health/migration/recovery。

所有请求携带 requestId、protocolVersion、clientId、capability。支持 timeout、cancel、structured error 和 stale response protection。

### 11.4 Renderer Projection

Renderer 通过 Runtime snapshot + domain events 构建：

- Workspace/Task projection；
- Session Graph projection；
- Scene projection；
- Notification projection；
- Artifact/Validation projection；
- Terminal checkpoint + tail projection。

Renderer 不再向权威层导出完整对象快照。迁移期 legacy snapshot exporter 只能存在于 compatibility adapter，不能覆盖 SQLite 权威实体。

### 11.5 Host Control Plane

INF-16 同时提供 Agent 控制宿主终端所需的本地控制面，不能把 Renderer IPC 直接暴露给 Agent：

- Runtime 监听应用私有 Unix Domain Socket；Windows 使用 Named Pipe；
- 启动路径和权限限定为当前 OS 用户，socket 目录为 0700；
- 每个受信任的 Agent `SessionRun` 获得短期 capability token，由 Runtime 自动注入环境变量，用户零配置；
- 外部进程默认没有 token，所有请求默认拒绝；token 绑定 runId、能力集合、过期时间和 runtime generation；
- Runtime 重启、SessionRun 结束或权限收回时 token 失效；
- 控制面使用长度前缀 JSON frame、协议版本、requestId、deadline 和结构化响应；
- 服务故障与 Agent/PTY 生命周期隔离，控制 socket 异常不得终止终端会话。

V1 capability scopes：

```text
host.list
terminal.read-current
terminal.read-history
terminal.read-commands
terminal.send-text
terminal.send-key
task.status.write
task.progress.write
task.log.append
```

目标列表可返回当前 Scene 中的稳定人类可读序号，但序号只是带 `projectionRevision` 的临时引用。执行命令时 Runtime 将它解析为稳定的 workspaceId/taskId/sessionId/mountId；revision 已过期时返回冲突，禁止把旧序号静默指向另一个终端。

读取必须有字节、行数和时间范围上限；`send-key` 使用 allowlist；接口不接收任意文件路径或任意宿主 shell 命令。稳定错误码至少包含：`TARGET_NOT_FOUND`、`RUNTIME_NOT_READY`、`AMBIGUOUS_TARGET`、`TIMEOUT`、`CAPABILITY_DENIED`、`CONFLICT`、`UNSUPPORTED`。

## 12. INF-17～INF-20：通用底座

### 12.1 Annotation / Artifact / Validation

基础表和 Repository 在功能 UI 前完成：

- annotations：kind、text snapshot、anchor、status；
- artifacts：path identity、media type、state、producer Session；
- validation_runs：checkId、status、Session、timestamps；
- 所有权威变化进入 Domain Events。

Task 运行信息使用独立模型，避免塞回 Session 或 Scene：

- `task_status_entries`：taskId、key、value、updatedAt、runtimeGeneration，支持 replace/delete；
- `task_progress`：taskId、0～100 progress、label、updatedAt、runtimeGeneration；
- `task_logs`：append-only id、taskId、level、source、message、createdAt、runtimeGeneration；
- status/progress/log 默认只属于当前应用运行代，启动新 generation 时清理旧代数据；
- 同一运行代内保留已有历史并实时订阅新增记录；
- 写入口统一经过 INF-16 capability 检查；日志设置条数和字节上限，严禁借日志接口写任意二进制流。

### 12.2 Notification / Preferences / Feature Campaign

- 通知是 Semantic/Domain Events 的会话态 projection；
- 通知列表和未读状态跨应用重启清空，沿用 PRD 01；
- sound、retention、默认 Workspace 显式移除状态等偏好进入 SQLite；
- dedup/cooldown 使用 eventId + sessionId + eventType；
- 点击跳转使用稳定 SessionId 和可用 SessionMount；mount 已消失时降级定位到 Task/Session 历史。

功能介绍批次使用 bundled manifest + 本地已读状态：

- campaignId/version 与 appVersion 分离，同一批次升级后可以再次展示；
- `feature_campaign_views` 保存 campaignId、version、viewedAt；
- manifest 只声明内容 key、排序和适用版本，不在 SQLite 复制展示文案；
- 自动恢复旧会话不触发“新建会话”类介绍，Runtime 只提供明确的 launch/recovery reason signal；
- 已读写入幂等，并通过 Preferences RPC 访问。

### 12.3 Retention / Privacy

- 每 Session 和全局磁盘配额；
- segment rotation、sealed compression、checkpoint retention；
- purge plan 先列出会删除的 metadata、journals、worktrees；
- Journal 目录权限 0700，文件 0600；
- archive 与 purge 分离；
- purge 生成审计事件；
- retention 后更新 anchor degraded state。

### 12.4 Observability / Preset Capability Registry

记录：runtime generation、commandId、eventId、sessionId、runId、journal lag、outbox lag、subscriber lag、migration status、recovery result、repair action。

诊断日志不得直接复制完整终端内容；用户主动导出的 diagnostics bundle 才包含经过明确选择的内容样本。

预置能力使用声明式 Registry，而不是在 UI 或启动脚本里散落安装逻辑：

- bundled manifest 声明 capabilityId、provider、pluginId、desiredVersion、source、checksum 和 schemaVersion；
- `preset_capability_state` 保存 installedVersion、status、lastAttempt、lastError、sourceFingerprint；
- `preset_capability_suppressions` 保存用户主动卸载/禁用的 capability，后续启动不得自动装回，只有 manifest 的强制安全迁移策略可以另行提示；
- reconcile 使用跨进程文件锁 + commandId，具备幂等、崩溃恢复和并发启动保护；
- 支持随包离线 seed 和受校验的在线 source；网络失败时不破坏已安装版本；
- 升级先 materialize 到临时目录，校验 checksum 后原子切换；
- Registry 只管理产品声明的预置能力，不扫描或接管用户自行安装的插件；
- 安装状态、版本漂移、抑制和修复动作进入 diagnostics，插件文件内容不进入 Domain Outbox。

## 13. INF-21～INF-24：reference product 三阶段迁移

### 13.1 Legacy importer

Importer 支持：snapshot.json、checkpoint.json、metadata.ndjson、terminal journals、scrollback、provider identities、team metadata。

Importer 必须幂等，生成：

- import run id；
- source fingerprint；
- entity mapping；
- ignored/repaired records；
- consistency report。

### 13.2 阶段 0：影子写

- UI 仍从 legacy 数据读取；
- 在 Matou 仓库提供 `compat/legacy-bridge`；reference product 的结构变更通过该窄适配器同时发送到 Matou Runtime；
- bridge 只映射已定义的 legacy mutation/event，不导入 reference product Store 对象，也不让 legacy snapshot 覆盖 SQLite；
- Runtime 独占 SQLite；
- 初次导入 checkpoint，随后消费逐条 legacy metadata event；
- 定期比较 legacy 与 SQLite 的规范化 projection；
- SQLite 只做校验和诊断；
- shadow write 失败不阻断 legacy 用户操作，但必须记录 lag 和 repair queue；
- repair queue 按 commandId 幂等重放。

### 13.3 阶段 1：读取切换

- UI 从 Runtime projection 读取；
- SQLite mutation 先提交；
- legacy compatibility writer 生成备份 snapshot/event；
- feature flag 可以回退 legacy read；
- 监控 restore success、projection diff、provider resume 和 relation correctness。

### 13.4 阶段 2：旧路径退役

- 停止 Renderer snapshot export；
- 停止 legacy metadata journal write；
- 保留一次性 importer 和只读备份期限；
- 删除旧 Store → snapshot 权威数据流；
- 更新安装、升级、卸载、诊断和数据清理路径。

## 14. INF-25：测试、故障注入与准入门

### 14.1 测试矩阵

1. SQLite PRAGMA、single-writer 边界、并发 storage queue；
2. migration fixture、checksum、备份和失败恢复；
3. mutation/outbox 原子性；
4. command idempotency；
5. domain cursor replay；
6. Journal frame checksum、尾部截断、中段损坏；
7. SQLite commit 与 Journal marker 各崩溃窗口；
8. paired checkpoint fallback；
9. Workspace/Task cascade 与 archive；
10. Worktree dirty retain；
11. Session/Run/Provider identity 分离；
12. relation event/current projection 同步；
13. fork/dependency cycle property tests；
14. sibling derivation；
15. Scene structure/geometry 分离；
16. stale geometry revision；
17. Agent event idempotency；
18. Anchor degradation；
19. Renderer reload/reconnect；
20. Runtime crash/restart；
21. 单 Session 损坏隔离；
22. provider resume 失败；
23. reference product importer fixtures；
24. shadow-write diff 和 repair queue；
25. packaged Electron SQLite/node-pty E2E；
26. 多终端吞吐、credit、outbox latency；
27. 磁盘满、只读目录、partial write；
28. retention/purge；
29. dependency-boundary scan；
30. Renderer authority scan。
31. Host Control socket 同用户校验、token 生命周期和默认拒绝；
32. Host Control 序号 revision、stale reference 和稳定 ID 解析；
33. terminal read 边界、send-key allowlist 和结构化错误；
34. Task status/progress/log 当前运行代、订阅、清理和容量限制；
35. feature campaign version/seen-state/restore reason；
36. preset capability reconcile 幂等、锁、离线 seed、checksum、升级失败回滚和用户卸载抑制；
37. `compat/legacy-bridge` contract、legacy mutation mapping 和 authority boundary。

### 14.2 可进入功能开发阶段的准入条件

全部满足：

- INF-01～INF-25 均有实现、测试和文档证据；
- Runtime 是 SQLite 唯一写入者；
- 所有领域写入均走 transaction + emit；
- Semantic Event Plane 可从持久 cursor replay；
- Journal/Domain 双流能在故障注入后对齐；
- Workspace/Task/Session/Relation/Scene 可通过 RPC 完整创建、读取、修改、归档和恢复；
- Relation 历史和 current projection 一致；
- Renderer reload 后仅通过 Runtime 重建；
- Host Control Plane 对受信任 Agent 零配置、对外部调用默认拒绝，且控制面故障不影响 PTY；
- Task status/progress/log 可在当前运行代内回放并实时订阅；
- feature campaign 和 preset capability registry 通过版本、幂等与失败恢复测试；
- reference product 阶段 0/1 的 importer、shadow write、diff、repair 和 read switch 经过 fixture 与 E2E 验证；
- legacy Renderer snapshot 已限制在 compatibility adapter；
- 打包后的 Electron 应用通过 SQLite、PTY、replay 和 recovery E2E；
- 全量测试、typecheck、build、boundary scan 通过；
- completion audit 对 INF-01～25 逐项提供直接证据。

## 15. INF-01～INF-25 追踪表

| 编号 | 设计章节 | 主要证明 |
|---|---|---|
| INF-01 | §3 | single-writer tests、PRAGMA tests、packaged smoke |
| INF-02 | §4 | migration fixtures、backup/failure tests |
| INF-03 | §5 | atomicity、idempotency、boundary tests |
| INF-04 | §6 | replay、cursor、event schema tests |
| INF-05 | §7.1 | Journal V2 codec/corruption tests |
| INF-06 | §7.2～7.3 | crash-window and watermark tests |
| INF-07 | §7.4 | checkpoint fallback tests |
| INF-08 | §8.1～8.2 | Workspace/Task repository tests |
| INF-09 | §8.3 | ExecutionContext/Worktree lifecycle tests |
| INF-10 | §8.4 | Session/Run/Provider tests |
| INF-11 | §9 | relation event/projection/property tests |
| INF-12 | §10.1 | Scene structure tests |
| INF-13 | §10.2 | geometry debounce/revision tests |
| INF-14 | §11.1 | adapter contract fixtures |
| INF-15 | §11.2 | anchor resolution/degradation tests |
| INF-16 | §11.3～11.5 | RPC reconnect/projection、Host Control auth/bounds E2E |
| INF-17 | §12.1 | schema/repository/event、Task telemetry tests |
| INF-18 | §12.2 | notification/preference/campaign tests |
| INF-19 | §12.3 | retention/purge/privacy tests |
| INF-20 | §12.4 | diagnostics/metrics、preset reconcile tests |
| INF-21 | §13.1 | importer fixture and mapping report |
| INF-22 | §13.2 | shadow-write diff/repair tests |
| INF-23 | §13.3 | read-switch/rollback E2E |
| INF-24 | §13.4 | legacy authority scans |
| INF-25 | §14 | full verification and completion audit |
