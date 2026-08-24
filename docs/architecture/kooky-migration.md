# Kooky → Matou 分阶段迁移

状态：基础设施定稿  
日期：2026-08-24

## 1. 权威边界

- Kooky 的 `snapshot.json`、`checkpoint.json`、`checkpoint.prev.json`、`metadata.ndjson`、terminal journals 与 scrollback 只允许由 `apps/runtime/src/compat/kooky-bridge/` 读取。
- Runtime 独占 Matou SQLite；Renderer 和 Electron Main 不打开数据库，也不导出权威对象快照。
- 初次导入后，legacy snapshot 只作为兼容输入，后续增量必须是版本化、白名单化的 `KookyMutationEnvelope`。
- 所有映射后的领域修改与 `legacy.mutation-applied` Outbox 事件在同一个 SQLite 事务提交。

## 2. 实体映射

| Kooky | Matou |
|---|---|
| Project | Workspace |
| Workbench | Task |
| Tab + split tree | Scene + SceneNode |
| Panel | Session + SessionMount |
| Panel cwd | PlainDirectory ExecutionContext |
| `claudeSessionId` | ProviderBinding |
| team teammate → leader | `team-member-of` SessionRelation |
| terminal journal / scrollback | Journal V2 output frame |

所有 Matou ID 由 legacy type + legacy ID 确定性派生；`legacy_entity_mappings` 保存可审计的双向依据。单个坏 panel、坏布局叶子、缺失 team leader 或坏 journal 进入导入报告，不中断其他 Session。

## 3. 阶段

### 阶段 0：影子写

1. `KookyImporter` 从有效 snapshot 读取；snapshot 损坏时按 checkpoint、previous checkpoint 顺序回退。
2. 从 checkpoint 的 `recoveryOffsets.metadataJournalBytes` 继续应用完整 NDJSON 记录；撕裂尾行留给下次补齐。
3. `ShadowWriteBridge` 先完成原 Kooky 写入，再尝试 Matou 映射；Matou 失败进入 `shadow_repair_queue`，不回滚 Kooky。
4. cursor 按字节持久化；projection diff、pending bytes 和 repair depth 进入迁移诊断。

### 阶段 1：SQLite 读取

1. `migration_authority/read-authority` 切为 `sqlite`，UI 投影来自 Runtime。
2. mutation 先提交 SQLite，再由 Runtime-owned `LegacyCompatibilityBackupWriter` 原子生成兼容备份。
3. 兼容备份失败只记录 telemetry，不回滚 SQLite commit。
4. feature flag 可回滚到 legacy read；持续观测 restore、provider resume、relation 与 projection equality。

### 阶段 2：退役

1. `migration-phase=retired`，同时强制 `read-authority=sqlite`。
2. Shadow mutation、metadata tail 和 repair write 停止。
3. 只读 importer/backup 在明确的本地时间窗内保留；窗口结束后不再把 legacy 数据用作运行时读取源。
4. 静态 boundary tests 保证 Renderer 不出现 `saveSnapshot`/`snapshot.json` 权威出口，legacy 文件名和映射表访问只存在于 compatibility package 与 schema migration。

## 4. 恢复语义

- 普通 Shell 只恢复结构、cwd 与归属；导入的旧画面不会作为新 Shell 启动画面自动回放。
- Claude Code/Codex 通过独立 ProviderBinding 恢复；失效身份只降级对应 Session。
- fork/team 的每个 Panel 都映射为独立 Session；导入不重跑 fork 命令。
- 独立窗口是临时 UI 状态；导入后 SessionMount 回到主 Scene。

