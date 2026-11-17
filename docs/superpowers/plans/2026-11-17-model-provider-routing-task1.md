# Task1 交付：配置持久化、精确目标准备、纯角色快照

基线：`02bdc536fa685d8e794a81fcad575804bb6f3caf`。本批只实现计划 Task1；未改 PTY、RuntimeServer 启动入口、请求转发、出口监控、UI；没有新依赖/数据库变更/真实凭据访问/付费请求/commit。

## 已实现及实际文件清单

| 文件 | 变更 |
| --- | --- |
| `packages/contracts/src/agent-model-settings.ts` | 导出 ClaudeModelRole、ClaudeRoleAssignments；preferences 增加可选 claudeRoleAssignments |
| `packages/contracts/src/protocol.ts` | 新增 patch-assignments RPC 白名单 |
| `apps/runtime/src/rpc/runtime-rpc-router.ts` | 专用 patch 分发，复用原只读访问策略 |
| `apps/runtime/src/session/agent-model-settings.ts` | patchAssignments 校验并局部保存；原 set 在异步校验后保留数据库最新 assignments；读回不删除失效选择 |
| `apps/runtime/src/session/agent-model-settings.test.ts` | 设置/RPC/协议解析、只读、交错写、真实存储变更后旧快照隔离测试 |
| `apps/runtime/src/model-router/subscription-accounts.ts` | 提取共享精确账号/租约预检；prepareLaunch 复用；新增 prepareModelTarget 和明确目录失败类型 |
| `apps/runtime/src/model-router/subscription-model-target.test.ts` | 目录模型不走共享 aliases、严格凭据/注册目录、失败释放、关闭并发、旧启动投影回归 |
| `apps/runtime/src/session/claude-role-assignments.ts` | runtime 内纯解析、深度冻结、默认角色目标、精确订阅连接、OAuth身份约束、能力与官方 family 投影 |
| `apps/runtime/src/session/claude-role-assignments.test.ts` | 同名跨供应商、未知能力、输入/store变更隔离、默认模型、压缩目标、OAuth刷新身份边界等测试 |

这是9个源码/测试文件；另新增本交付说明。原有规格、正式计划、上一关调研文档保持原样。contracts 的 index.ts 已有 wildcard 导出，不新增重复导出。

## 导出接口与后续接入边界

```ts
// contracts
ClaudeModelRole = 'opus' | 'sonnet' | 'haiku'
ClaudeRoleAssignments = Partial<Record<ClaudeModelRole, { providerId: string; model: string }>>
AgentModelPreferences.claudeRoleAssignments?: ClaudeRoleAssignments
// RPC
'agent-model-settings.patch-assignments' // { assignments } → AgentModelSettingsSnapshot
AgentModelSettings.patchAssignments(input: { assignments: ClaudeRoleAssignments }): Promise<AgentModelSettingsSnapshot>

// runtime-only subscription interface
SubscriptionAccounts.prepareModelTarget(providerConfigId: string, model: string, scope?: string): Promise<PreparedSubscriptionModelTarget>
PreparedSubscriptionModelTarget // lease, baseUrl, headers, model, metadata
SubscriptionModelTargetError.targetFailure // catalog-model-missing | registered-model-missing | directory-unavailable

// runtime-only pure snapshot interface
resolveClaudeRoleSnapshot(input: ClaudeRoleSnapshotInput): ResolvedClaudeRoleSnapshot | undefined
ClaudeRoleSnapshotInput // preferences, providers snapshot, connections, optional nativeModels
ClaudeTargetConnection // api | oauth | subscription，均由runtime准备
ResolvedClaudeRoleTarget // 精确provider/model/连接、可选metadata、effort、family、身份与时区策略
ResolvedClaudeRoleSnapshot // 冻结preferences、三角色targets、可选custom compaction target、officialFamilies
```

- 缺失/空分配返回 undefined，不解析新目标，后续调用者据此完整保留旧路径；当前没有接入旧启动入口。
- patch 不调用供应商 activate、不写 session binding。旧 set 忽略请求中的 assignments，仅保存原字段并保留数据库最新分配；patch 则仅保存分配。两者全部异步校验完成后同步读/合并/写，期间无 await。
- API 有明确目录时拒绝缺失模型；无目录沿用自定义模型能力，不探测该 API。订阅必须同时在保存目录和精确 credential 已注册目录中存在；返回 prefix/raw-ID，不按该供应商原 roleMapping 再解析，不写新的共享 aliases。
- 新旧订阅准备共用 credential、adoption、lease 和失败清理；close 在异步准备完成前发生时，准备失败并释放 lease。目录获取失败与明确缺项提供不同 targetFailure，不能将读取失败视为删除。
- 纯解析器不读 store/HTTP/钥匙串，也不 acquire lease；调用者提供已读 runtime 数据和精确订阅准备结果。解析时复制并冻结 endpoint/key/model/嵌套能力，后续修改原输入或真实 ProviderConfigStore 均不改变快照。订阅租约仍归准备者所有，Task2 再接进程退出/构建失败清理。
- OAuth 使用捕获的 account identity 和固定 reader，允许 reader 返回同账号的新 token；身份变化直接拒绝。真实账号身份读取适配与网关接入属于 Task2，本批仅验证接口边界，无真实凭据读取。
- 目标能力不存在时 metadata 保持 undefined；API 的 CLI 默认选择只从 native catalog 解析模型身份，不复制官方能力。API 模型字面名称 default 仍按自定义模型处理。
- 官方 family、Antigravity 现有隐藏时区策略仅在快照投影，不注册/修改监控器；多目标冻结与请求保护留在 Task3。
- 不增加UI snapshot字段：后续复用父组件已有 providers/accounts；本批未新增额度读取或轮询。

## 红绿记录

日志根目录：`/Users/icesword/.claude/agent-foreman/runs/matou-routing-recon-01/logs/`。

| 阶段 | 结果 | 日志 |
| --- | --- | --- |
| 新配置/精确target接口测试，实施前 | 27失败、8原有通过；缺少patchAssignments/prepareModelTarget | task1-red-config-target.log |
| RPC白名单测试，接入前 | 1失败、23筛选排除 | task1-red-rpc.log |
| 角色快照接口测试，实施前 | 9失败、1通过；缺少resolveClaudeRoleSnapshot。非法角色用例随后收紧错误匹配，避免任意异常造成假通过 | task1-red-snapshot.log |
| 首轮配置/target绿灯尝试 | 4失败、32通过；测试误用了英文原始诊断和READ_ONLY错误码；已有接口使用本地化错误与STORAGE_READ_ONLY。目录读取失败的语义也补成明确targetFailure后验证 | task1-green-config-target.log |
| 接口及旧平台/启动回归 | 64通过 | task1-green-interfaces.log |
| Antigravity身份/时区字段补充测试，修复前 | 1失败、11通过 | task1-red-snapshot-policy.log |
| null分配、缺失custom目标、API默认模型边界，修复前 | 3失败、12通过 | task1-red-snapshot-boundaries.log |
| API字面default不误当官方selector，修复前 | 1失败、15筛选排除 | task1-red-api-default.log |
| 最终相关runtime回归 | 13文件通过、1文件跳过；164通过、17跳过 | task1-runtime-final.log |
| contracts协议测试 | 35通过 | task1-contracts.log |

最终计数 **199通过、17既有集成测试跳过、0失败**，不把多轮重复测试相加。17跳过仍来自 `subscription-accounts.test.ts` 的 MATOU_TEST_MODEL_ROUTER 前置开关未启用，不能算通过。

新增核心测试合计54项所在的三个文件：agent-model-settings 25项、claude-role-assignments 16项、subscription-model-target 13项。其中设置原有8项，新增46项；附加存储/旧启动回归验证是在主要红绿周期后加入，未把它们伪称为独立红灯。

## 最终验证命令

```sh
pnpm build:packages
pnpm --filter @matou/contracts exec vitest run src/protocol.test.ts
pnpm --filter @matou/runtime typecheck
pnpm --filter @matou/contracts typecheck
pnpm check:identifiers
git diff --check
```

以上通过。基础包构建日志 `task1-build-packages.log`；类型日志 `task1-runtime-types-final.log`、`task1-contracts-types-final.log`；命名日志 `task1-identifiers.log`。

完整runtime测试命令：

```sh
TMPDIR="$PWD/.cache/task1-tmp" pnpm --filter @matou/runtime exec vitest run --testTimeout=15000 \
 src/session/agent-model-settings.test.ts \
 src/session/claude-role-assignments.test.ts \
 src/model-router/subscription-model-target.test.ts \
 src/model-router/subscription-accounts.test.ts \
 src/model-router/subscription-platform.test.ts \
 src/model-router/subscription-spawn.test.ts \
 src/model-router/subscription-rpc.test.ts \
 src/model-router/claude-launch.test.ts \
 src/session/claude-model-routing.test.ts \
 src/session/claude-request-gateway.test.ts \
 src/provider-config/provider-config-store.test.ts \
 src/provider-config/provider-models.test.ts \
 src/provider-config/subscription-provider-store.test.ts \
 src/session/native-model-catalog.test.ts
```

未跑全量App构建/全仓测试/真实UI/真实router账号集成/付费请求。源码自查由实现者执行，不替代接收方独立审查。未接入新进程、普通/计数请求或多family保护；产品端实际路由尚未实现，不能以本批接口绿灯宣称整功能完成。

## 我自己做的决定

1. 纯快照采用显式输入数据与连接的同步解析函数，拒绝在首次请求延迟读store；订阅lease仍由准备者持有，等待Task2绑定生命周期。理由：保持本批纯解析范围，也避免冻结外部lease对象。
2. 没有新增UI snapshot字段；优先复用父组件已有providers/accounts，避免本批引入重复额度读取。
3. 订阅目录错误用runtime局部targetFailure区分读取失败与缺项，沿用现有本地化错误外壳，不改全局错误框架。
4. 区分API的CLI默认占位与字面default模型；只借native目录解析默认身份，不借官方能力。理由：既兼容旧自定义模型，也不夸大能力。
5. 遵守本批执行者限制，不派子代理；仅做实现者自查，留待用户审查后放行后续集成。

## 未解决的问题

Task1范围内无阻塞。真实OAuth身份reader、启动时一次性采集数据/租约托管、PTY/gateway/egress/UI接入与真实行为验收均按计划留在后续批次；17项固定router集成测试前置条件未启用。
