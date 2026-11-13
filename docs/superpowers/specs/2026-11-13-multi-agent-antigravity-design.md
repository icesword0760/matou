# Matou 多 Agent 底座与 Antigravity CLI 接入实现方案

> 日期：2026-11-13  
> 状态：第一关交付物（实现方案，待审定后放行后续代码关卡）  
> 对应 PRD：[PRD-Matou-多Agent接入-Antigravity.md](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/docs/prd/PRD-Matou-多Agent接入-Antigravity.md)

---

## 0. 背景与架构取舍

### 0.1 问题现状

当前 Matou 在跨前后端、Runtime、数据库、IPC 契约和 `mt` CLI 的约 30 个生产文件中将 AI 会话类型硬编码为 `'claude-code' | 'codex'`（外加 `'shell'` 与 `'agent-team-member'`），并把生命周期与产品交互耦合到 Claude Code：

- `ProviderHookServer.#handle` 仅识别 `snake_case` 字段（如 `hook_event_name`），对非 `'claude-code'` 的事件直接跳过或写死 `provider: 'claude-code'`。
- `RuntimeServer.#spawn`、`SessionForkIntentRepository.accept`、`ForkWorkflowService.#freezeProviderContext` 等处默认假定所有 Agent 都接入 `ProviderConfigStore`（服务供应商），或只有 Claude Code 具备状态、HUD、自动标题、Fork 与会话载入能力。
- 前端多处写死 `'Claude Code'` / `'ClaudeMd'` / `ClaudeAccountsControl` / 权限列表与恢复失败文案。

### 0.2 为什么不用“仅在现有三选一上加 `if` 分支”的更简方案？

如果在 `'shell' | 'claude-code' | 'codex'` 上直接追加 `'antigravity'` 的 `if/else` 分支：

1. **三期后续全部返工**：路线图已明确本期后依次接入 Codex 补齐、DSH、Kimi Code CLI。若现在把第 4 个值硬编码塞进 ~30 个文件的条件判断里，接下来每一期都要再次扫全仓改联合类型、RPC 校验、UI 按钮、通知与恢复逻辑。
2. **极易漏网触发崩溃或产品违约**：比如 `RuntimeServer.#spawn`、`ForkWorkflowService.#freezeProviderContext`、`RuntimeServer.#applyHudRpc` 都会自动调用 `ProviderConfigStore.launchSelection()`，若某处漏掉 `if (profile !== 'antigravity')` 就会对禁止进入“服务供应商”面板的 Agent 抛错或破坏隔离；又如 UI 必须做到“不支持的能力不显示入口，不提供点了才报错的按钮”，散落的 `if` 很难保证 HUD、卡片操作栏、右键菜单、载入弹窗四处一致。
3. **一次性收口成本可控**：引入一份纯数据声明的 Agent 清单（`AgentManifest`），让 UI 按能力开关渲染、Runtime 按清单驱动参数与分支，既不超前为 DSH/Kimi 写适配器代码，又能让后续新增 Agent 主要变成“新增一份清单条目 + 对应运行时适配器”，原有 Claude Code 与 Codex 行为零回归风险。

---

## 1. Agent 清单数据结构、包归属与前后端共享机制

### 1.1 包归属：`@matou/domain` 为单一事实源，`@matou/contracts` 派生协议校验

检查依赖关系可知：
- `@matou/domain` 零运行时外部依赖，只描述纯领域模型。
- `@matou/contracts` 依赖 `zod`，被 `@matou/runtime` 和 `@matou/desktop`（Electron Main 与 Renderer）共同依赖。
- `@matou/runtime` 和 `@matou/desktop` 均同时直接依赖 `@matou/domain` 与 `@matou/contracts`。

因此采用如下分层：

1. **清单定义放在 `@matou/domain`**（新增 [packages/domain/src/agent-manifest.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/packages/domain/src/agent-manifest.ts)，经 [packages/domain/src/index.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/packages/domain/src/index.ts) 导出）：
   - 只包含**纯数据结构、常量与无副作用查询函数**（不依赖 Node `fs`、`path`、`child_process`），保证 Browser/Renderer、Electron Main、Runtime 进程均可安全直引。
2. **协议与 Zod Schema 放在 `@matou/contracts`**：
   - [packages/contracts/src/protocol.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/packages/contracts/src/protocol.ts)、[packages/contracts/src/hud.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/packages/contracts/src/hud.ts)、[packages/contracts/src/claude-sessions.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/packages/contracts/src/claude-sessions.ts) 直接从 `@matou/domain` 导入允许的 Agent ID 元组构造 `z.enum(...)` 与联合类型，杜绝多处手工维护枚举清单。
   - 特别保留 [packages/contracts/src/provider-config.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/packages/contracts/src/provider-config.ts) 中的 `ProviderCli = 'claude-code' | 'codex'`（与 `capabilities.providerConfig === true` 对齐），确保“服务供应商”面板与 `apps/runtime/src/provider-config/` 保持不受污染、不加入 Antigravity。
3. **Runtime I/O 适配器留在 `apps/runtime/src/agents/`**：
   - 清单只声明**“是什么、能做什么、用什么策略”**；涉及子进程启动（`agy models`、`agy --conversation`）、文件读写（`conversation_metadata.json`、`~/.gemini/config/plugins/matou/`）与 HTTP Hook 负载归一化的代码留在 Runtime 侧，通过 `manifest.id` 注册并查找适配器实例。

### 1.2 核心数据结构（`AgentManifest`）

```ts
// packages/domain/src/agent-manifest.ts

export const LAUNCHABLE_AGENT_PROFILES = ['claude-code', 'codex', 'antigravity'] as const
export type AgentProfileId = (typeof LAUNCHABLE_AGENT_PROFILES)[number]

/** 卡片运行形态：普通终端、可启动 Agent、团队成员节点 */
export type SessionKind = 'shell' | AgentProfileId | 'agent-team-member'
export type SessionCurrentMode = SessionKind

/** provider_bindings.provider 允许的绑定身份来源 */
export type AgentProviderId = AgentProfileId | 'generic'

export interface AgentManifest {
  /** 内部唯一标识：'claude-code' | 'codex' | 'antigravity' */
  id: AgentProfileId
  /** UI、通知标题、弹窗与错误提示统一展示名称：'Claude Code' | 'Codex' | 'Antigravity' */
  displayName: string
  /** 标签栏、DAG 角标、默认卡片标题等紧凑场景名称：'Claude' | 'Codex' | 'Antigravity' */
  shortName: string

  executable: {
    /** 覆盖可执行文件路径的环境变量名 */
    envVar: 'MATOU_CLAUDE_COMMAND' | 'MATOU_CODEX_COMMAND' | 'MATOU_ANTIGRAVITY_COMMAND'
    /** 默认命令名 */
    defaultCommand: 'claude' | 'codex' | 'agy'
    /** Shell 卡片中输入这些首词/快捷形态时自动识别升级为该 Agent（按需扩展） */
    interactiveLaunchAliases?: readonly string[]
  }

  /** 按权限模式的命令行映射规则（运行时由 LaunchBuilder 消费） */
  launch: {
    resumeFlag?: string                  // claude: '--resume', codex: 'resume', agy: '--conversation'
    forkStrategy: 'cli-flag' | 'subcommand' | 'resume-then-slash-fork' | 'none'
    forkFlag?: string                    // claude: '--fork-session', codex: 'fork'
    modelFlag?: string                   // '--model'
    permissionFlags: Partial<Record<AgentPermissionMode, readonly string[]>>
  }

  /** 事件采集与身份/Transcript 真源声明 */
  events: {
    lifecycleSource: 'http-hook' | 'plugin-command-hook' | 'none'
    statusLineSource: 'settings-http' | 'settings-command' | 'tui-notify' | 'none'
    identitySource: 'hook-or-statusline' | 'statusline-only' | 'none'
    /** 'hook' 表示状态栏 transcript_path 不可信，仅认 hook 路径（agy 即此项） */
    transcriptPathSource: 'hook' | 'hook-or-statusline' | 'none'
    /** 运行中身份变更的二次认领方式：agy Fork 需等 ID 不同于父 ID */
    forkIdentityRule: 'immediate-new-id' | 'differs-from-parent-after-slash-fork'
  }

  /** mt 跨卡片协作说明注入方式 */
  mtInjection: {
    mode: 'plugin-dir' | 'developer-instructions-flag' | 'session-start-system-message' | 'none'
    /** 对应 control-assets/providers/ 下的相对路径 */
    assetRelativePath?: string
  }

  /** 项目级指令文件探测与编辑器弹窗命名（按优先级顺序探测现有文件） */
  instructionFiles: {
    /** 如 Claude 为 ['CLAUDE.md']；Antigravity 为 ['AGENTS.md', 'GEMINI.md']（均无时默认写 AGENTS.md） */
    candidates: readonly string[]
    /** 按钮短文案，如 'ClaudeMd' / 'AGENTS.md' */
    buttonLabel: string
  }

  /** HUD 展示策略 */
  hud: {
    /** 是否把前缀 "Claude " 从 model display_name 剥离（仅 Claude 需要，agy 可能含 Claude 模型故不剥离） */
    stripClaudeModelPrefix: boolean
    /** 未上报额度时是否隐藏占位符 `—`（agy 拿不到即隐藏，Claude 保持原样显示 `—`） */
    hideEmptyQuotaPlaceholder: boolean
    supportsTodos: boolean
    supportsAgentsCount: boolean
    supportsMcpSummary: boolean
    supportsHooksCount: boolean
    supportsQuotaWindows: boolean
  }

  /** UI 入口与后端流水线共用的能力开关 */
  capabilities: {
    /** 卡片是否可 Fork */
    fork: boolean
    /** 是否有「载入已有会话」列表与恢复 */
    sessionLoader: boolean
    /** 是否根据会话元数据/Transcript 自动更新卡片标题并支持「恢复自动标题」 */
    autoTitle: boolean
    /** HUD 上可切换的权限模式列表（空数组表示不支持权限切换） */
    permissionModes: readonly AgentPermissionMode[]
    /** 权限模式切换方式 */
    permissionSwitchMode: 'claude-hybrid' | 'always-respawn' | 'none'
    /** 模型切换能力 */
    modelSwitch: 'claude-inline-command' | 'cli-list-and-respawn' | 'none'
    /** 是否允许写入/读取服务供应商（claude-code / codex 为 true，antigravity 为 false） */
    providerConfig: boolean
    /** 是否在 HUD 右下角显示多账户切换控件 */
    accountSwitching: boolean
    /** 是否启用 Agent Team（Subagent 队友发现） */
    agentTeam: boolean
    /** 是否受 Claude 时区偏好设置影响 */
    claudeTimeZone: boolean
    /** 重启后是否自动按 binding 恢复会话 */
    autoRestore: boolean
    /** Fork 成功所需前置交互门禁 */
    forkRequiresCompletedTurn: boolean
  }
}
```

针对 **`antigravity`** 的核心声明值如下：
- `id: 'antigravity'`, `displayName: 'Antigravity'`, `shortName: 'Antigravity'`
- `executable`: `{ envVar: 'MATOU_ANTIGRAVITY_COMMAND', defaultCommand: 'agy', interactiveLaunchAliases: ['agy', 'agy --dangerously-skip-permissions'] }`（具体支持哪些 shell 命令参数升级见第 3.1 节）
- `launch`: `{ resumeFlag: '--conversation', forkStrategy: 'resume-then-slash-fork', modelFlag: '--model', permissionFlags: { default: [], auto: ['--mode', 'auto'], acceptEdits: ['--mode', 'accept-edits'], plan: ['--mode', 'plan'], bypassPermissions: ['--dangerously-skip-permissions'] } }`
- `events`: `{ lifecycleSource: 'plugin-command-hook', statusLineSource: 'settings-command', identitySource: 'hook-or-statusline', transcriptPathSource: 'hook', forkIdentityRule: 'differs-from-parent-after-slash-fork' }`
- `mtInjection`: `{ mode: 'session-start-system-message', assetRelativePath: 'providers/antigravity-system-message.md' }`
- `instructionFiles`: `{ candidates: ['AGENTS.md', 'GEMINI.md'], buttonLabel: 'AGENTS.md' }`
- `hud`: `{ stripClaudeModelPrefix: false, hideEmptyQuotaPlaceholder: true, supportsTodos: false, supportsAgentsCount: false, supportsMcpSummary: false, supportsHooksCount: false, supportsQuotaWindows: true }`
- `capabilities`: `{ fork: true, sessionLoader: true, autoTitle: true, permissionModes: ['default', 'acceptEdits', 'plan', 'bypassPermissions'], permissionSwitchMode: 'always-respawn', modelSwitch: 'cli-list-and-respawn', providerConfig: false, accountSwitching: false, agentTeam: false, claudeTimeZone: false, autoRestore: true, forkRequiresCompletedTurn: true }`

> 注：`claude-code` 的 `capabilities` 与现有行为 100% 保持一致（`accountSwitching: true`, `providerConfig: true`, `agentTeam: true`, `permissionSwitchMode: 'claude-hybrid'`, `hideEmptyQuotaPlaceholder: false`, `stripClaudeModelPrefix: true` 等）；`codex` 保持本期已有行为（`providerConfig: true`, `fork: false` 等），不借本期改造改变 Codex 用户可见行为。

---

## 2. 需要改动的文件清单与数据库迁移策略

### 2.1 按模块分组的改动清单（逐文件一句说明）

> 注：本表同时标明了严格**不修改**的服务供应商与 Claude 账户隔离区。

#### A. `@matou/domain` 与 `@matou/contracts`（共享清单与契约层）
1. `packages/domain/src/agent-manifest.ts` *(新增)*：定义 `AgentProfileId`、`AgentManifest`、`AGENT_MANIFESTS` 清单常量及按 ID/能力查询工具函数。
2. `packages/domain/src/index.ts`：导出 `agent-manifest` 与新增的多 Agent 会话目录相关类型。
3. [packages/domain/src/model.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/packages/domain/src/model.ts)：将 `SessionKind`、`SessionRun.profile`、`ProviderBinding.provider`、`SemanticAnchor.sourceRef.provider` 从硬编码联合类型改为引用 `AgentProfileId`；`SessionGraphNodeBase.childModeCounts` 扩展为按 profile 统计或保留兼容字段并新增 `byMode`。
4. [packages/domain/src/events.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/packages/domain/src/events.ts)：将 `AgentSemanticEvent.provider` 改为引用 `AgentProviderId`。
5. [packages/contracts/src/protocol.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/packages/contracts/src/protocol.ts)：将 `spawnSchema.profile` 从 `z.enum(['shell', 'claude-code', 'codex'])` 改为引用 `LAUNCHABLE_AGENT_PROFILES`；为 `claude-sessions.*` 增加可选 `provider?: AgentProfileId`（或新增对称的通用会话目录参数并保持旧 RPC 向后兼容）及新增 `agent-models.list` RPC 方法；放宽 `session.set-model` 的模型名校验以支持动态 `agy models` 列表。
6. [packages/contracts/src/hud.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/packages/contracts/src/hud.ts)：为 `SessionHudWire` 增加 `profile?: SessionKind`、`instructionFileLabel?: string`、`effort?: string` 并将 `HudModelStrategy` 放宽为兼容任意已知或动态模型 ID（保留现有 Claude 档位联合类型超集）；按 manifest 控制 `defaultAccountUsage`。
7. [packages/contracts/src/claude-sessions.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/packages/contracts/src/claude-sessions.ts)：在列表/详情/载入契约中支持 `provider?: AgentProfileId`，并允许 `permissionMode` / `model` 对多 Agent 返回可选元数据。

#### B. Runtime — 存储与数据库迁移（`apps/runtime/src/storage/`）
8. [apps/runtime/src/storage/migration-runner.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/runtime/src/storage/migration-runner.ts)：为 `Migration` 接口增加可选 `foreignKeysOff?: boolean`，在启用时于事务外执行 `PRAGMA foreign_keys = OFF; PRAGMA legacy_alter_table = ON;`，迁移后执行 `PRAGMA foreign_key_check` 并恢复外键，确保重建 `sessions` 表时 20+ 外键引用不被破坏。
9. [apps/runtime/src/storage/migrations.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/runtime/src/storage/migrations.ts)：新增 **v35 迁移**，重建 `sessions`、`provider_bindings`、`session_fork_intents` 以解除/扩展 `kind` / `provider` / `source_provider` 的 CHECK 约束并完整保留所有既有列、索引与触发器（详见 §2.2）。

#### C. Runtime — Agent 适配、Hook 服务、目录与会话生命周期（`apps/runtime/src/agents/`、`session/`）
10. `apps/runtime/src/agents/antigravity-plugin-installer.ts` *(新增)*：负责幂等生成并版本校验覆盖 `~/.gemini/config/plugins/matou/`（`plugin.json`、根目录 `hooks.json`、hook 脚本），在缺失环境变量时保证零输出退回。
11. `apps/runtime/src/agents/antigravity-session-catalog.ts` *(新增)*：读取 `~/.gemini/antigravity-cli/cache/conversation_metadata.json`（按 `AppDataDir = 'antigravity-cli'`、非 `Internal`、`WorkspaceURIs` 匹配工作目录筛选）与 `.system_generated/logs/transcript.jsonl`，实现列表、详情、正文搜索与自动标题查询，输出与 `ClaudeSessionCatalog` 对齐的数据结构。
12. `apps/runtime/src/agents/antigravity-models.ts` *(新增)*：异步执行 `agy models`（优先按 `MATOU_ANTIGRAVITY_COMMAND` 解析可执行文件），剥离 ANSI spinner 与回车控制符后解析 `<id>  <Display Name>` 并做短时内存缓存与失败降级。
13. [apps/runtime/src/agents/agent-adapters.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/runtime/src/agents/agent-adapters.ts)：新增 `AntigravityAdapter`，将 Antigravity hook / statusLine 原始 JSON 归一化为 `AgentSemanticEvent` 与统一生命周期内部事件。
14. [apps/runtime/src/session/provider-hook-server.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/runtime/src/session/provider-hook-server.ts)：将 `HookRegistrationRecord.provider` 泛化为 `AgentProfileId`；新增 `registerSession(provider, input)` 入口并保留旧 `registerClaudeSession`/`registerCodexSession` 兼容委托；`#handle` 支持按 `provider` 分发归一化逻辑（解析 agy `camelCase` 字段、禁止用 statusLine 错误 `transcript_path` 覆盖、Pre/PostToolUse 工具计次、Stop `terminationReason`、statusLine `agent_state`/`quota`/`cycle_mode`/`model` 转换，以及取消写死 `provider === 'codex'` 与 `'claude-code'`）。
15. [apps/runtime/src/session/provider-launch-plan.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/runtime/src/session/provider-launch-plan.ts)：改为按 `getAgentManifest(profile)` 生成命令行参数，新增 `antigravity` 分支（`--conversation`、`--model`、`--mode`、`--dangerously-skip-permissions`、`--settings '{"statusLine":...}'`）。
16. [apps/runtime/src/session/pty-session.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/runtime/src/session/pty-session.ts)：将 `profile` 与 `resolveExecutable(profile)` 改为查清单（支持 `MATOU_ANTIGRAVITY_COMMAND ?? 'agy'`）；对缺失命令返回可识别的启动错误类型供前端展示安装指引。
17. [apps/runtime/src/session/session-hud-registry.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/runtime/src/session/session-hud-registry.ts)：允许任意 manifest 中的 Agent 进入 HUD；`inspectProviderConfig` 改为按 manifest 的 `instructionFiles.candidates` 探测 `AGENTS.md` / `GEMINI.md` 或 `CLAUDE.md`；`normalizeModelStrategy` 放宽以接收动态 `agy` 模型名与 ID；`ingestProvider` 支持读取 Antigravity `quota`（5 小时 / 每周按模型组映射 `usageWindows`）、`model`（含 effort 拼接）与 `cycle_mode`。
18. [apps/runtime/src/session/provider-notification-event.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/runtime/src/session/provider-notification-event.ts)：从 `TITLE = 'Claude Code'` 改为按 `manifest.displayName`（如 `Antigravity`）动态生成通知标题与正文，并增加 `authenticating` / `tool_use` / Stop `terminationReason` 对应通知。
19. [apps/runtime/src/session/session-execution-service.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/runtime/src/session/session-execution-service.ts)：将 `profile` 类型改为 `'shell' | AgentProfileId`。
20. [apps/runtime/src/session/session-fork-intent-repository.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/runtime/src/session/session-fork-intent-repository.ts)：在 `AcceptForkIntentInput` 中接收并写入真实 `sourceProvider`，去掉 `'claude-code'` SQL 硬编码；增加查询/标记用于 `antigravity` 的 `/fork` 二阶段完成判断。
21. [apps/runtime/src/session/terminal-work-status-tracker.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/runtime/src/session/terminal-work-status-tracker.ts)：将 `provider` 类型改为 `AgentProfileId`，对于使用 hook/statusLine 完整状态流的 Agent（`antigravity`）禁用 Claude 终端字符流启发式误判，仅保留退出状态和 hook 状态真源。

#### D. Runtime — 领域仓储、画布模式、Fork、恢复、Control、RPC 与主服务
22. [apps/runtime/src/domain/session-repository.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/runtime/src/domain/session-repository.ts)：泛化 `RunRow.profile`、`BindingRow.provider`、`promoteShellToAgent`、`returnAgentToShell`（使用 `manifest.shortName` 生成与回收默认标题）；`observeProviderTitle` 对支持 `autoTitle` 的 Agent 一视同仁。
23. [apps/runtime/src/session-canvas/session-canvas-service.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/runtime/src/session-canvas/session-canvas-service.ts)：`CreateSessionSiblingInput.profile` 扩展为 `'shell' | AgentProfileId`；`defaultTitleForProfile` 改为查 manifest `shortName`；`resolveSiblingFallbackKind` 支持按 source 的真实 `provider` 绑定匹配。
24. [apps/runtime/src/session-canvas/session-graph-repository.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/runtime/src/session-canvas/session-graph-repository.ts)：将 `canFork` 判断从 `row.kind === 'claude-code'` 改为 `getAgentManifest(row.kind)?.capabilities.fork === true`；子计数与模式枚举兼容所有 `AgentProfileId`。
25. [apps/runtime/src/session-canvas/provider-mode-service.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/runtime/src/session-canvas/provider-mode-service.ts)：将 `loadClaudeSession`、`observeHook`、`markClaudeActive`、`markUserExited` 中的 `'claude-code'` / `'Claude'` 改为接收 `provider: AgentProfileId` 并查清单 `shortName` 与能力开关。
26. [apps/runtime/src/session-canvas/fork-workflow-service.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/runtime/src/session-canvas/fork-workflow-service.ts)：`#validForkBinding` 改查 `manifest.capabilities.fork`；`#freezeProviderContext` 在 `manifest.capabilities.providerConfig === false` 时跳过 `providerConfigs.launchSelection()`，仅冻结 `model` / `permissionMode`。
27. [apps/runtime/src/hierarchy/hierarchy-application-service.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/runtime/src/hierarchy/hierarchy-application-service.ts)：将约 1467–1534 行 legacy Fork 中的 `'claude-code'` / `'Claude'` 改为读取 `source.kind` 与对应 manifest。
28. [apps/runtime/src/recovery/runtime-recovery-service.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/runtime/src/recovery/runtime-recovery-service.ts) & [apps/runtime/src/recovery/runtime-session-recovery-scheduler.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/runtime/src/recovery/runtime-session-recovery-scheduler.ts)：将 `profile` 扩展为 `'shell' | AgentProfileId | 'agent-team-member'`。
29. [apps/runtime/src/control/host-action-types.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/runtime/src/control/host-action-types.ts)、[host-control-types.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/runtime/src/control/host-control-types.ts)、[host-topology-projector.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/runtime/src/control/host-topology-projector.ts)、[apps/runtime/src/cli/mt-cli.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/runtime/src/cli/mt-cli.ts)：将 `structure.create.session` 和 `mt create session --profile` 扩展为支持 `antigravity`，更新帮助文案与错误提示。
30. [apps/runtime/src/rpc/runtime-rpc-router.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/runtime/src/rpc/runtime-rpc-router.ts)：`session.create` / `session.set-permission-mode` / `session.set-model` 支持清单内 Agent 与动态模型列表；`claude-sessions.*`（含 `list/detail/search/load`）根据参数 `provider` 分发到 `ClaudeSessionCatalog` 或 `AntigravitySessionCatalog`；`session.instructions-read/write` 按会话 `kind` 对应 manifest 的 `instructionFiles.candidates` 定位（先找已有 `AGENTS.md`/`GEMINI.md`，均无时新建 `AGENTS.md`）；新增 `agent-models.list` RPC（调用 `agy models`）；`runtime-access-policy.ts` 将只读新 RPC 纳入只读策略白名单。
31. [apps/runtime/src/runtime-server.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/runtime/src/runtime-server.ts)：
    - `#spawn` 按清单决定是否调用 `providerConfigs`、是否安装并注册 Antigravity 插件/Hook、跳过 Antigravity 的 `ClaudeSettings` 时区与 `ClaudeFullResumePromptMonitor`。
    - 实装 Antigravity **Fork 编排器**（监听状态栏进入 `idle` 且 ID 等于父 ID → 向子 PTY 发送 `/fork\r` → 等待新 ID 认领并设置超时失败兜底）。
    - `#applyHudRpc` 处理 `session.set-permission-mode`（`always-respawn` 走 `#respawnWithPermission`）与 `session.set-model`（`cli-list-and-respawn` 时写入绑定新模型并重启接回原对话）。
    - `#maybePromoteShellAgent` 改为表驱动解析清单内所有 Agent 的 `interactiveLaunchAliases`（输入 `agy` 即自动升级为 Antigravity 卡片并注入环境与 statusLine 参数）。
    - ENOENT 启动失败时发送携带 Agent 名称与安装命令提示的友好错误。
32. [apps/runtime/src/index.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/runtime/src/index.ts)：`onIdentityRecorded` 对所有支持 `observeHook` 的 Agent 放行（不再仅 `provider === 'claude-code'`）；`forkCoordinator.notify` 按会话真实 `provider` 从清单取通知标题；`forkExecutionDescriptor` 放行所有支持 Fork 的 `SessionKind`（不再仅 `row.kind === 'claude-code'`）；注册并监听 Antigravity 自动标题更新。
33. [apps/runtime/src/compat/legacy-bridge/legacy-importer.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/runtime/src/compat/legacy-bridge/legacy-importer.ts)、[apps/runtime/src/i18n/messages/session.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/runtime/src/i18n/messages/session.ts)、[apps/runtime/src/i18n/messages/sessionCanvas.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/runtime/src/i18n/messages/sessionCanvas.ts)：`legacy-importer` 保留历史导入兼容同时允许识别清单内 `brand`；Runtime i18n 文案从写死 `'Claude Code'` 参数化为按 Agent `displayName` 插值。
34. `apps/runtime/control-assets/providers/antigravity-system-message.md` *(新增)*：复用 `host-control.md` 与 `codex-developer-instructions.md` 中 `mt` 协作正文，面向 Antigravity 作为 SessionStart `injectSteps.systemMessage` 内容源。

#### E. Desktop（`apps/desktop/src/`）
35. [apps/desktop/src/shared/desktop-api.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/desktop/src/shared/desktop-api.ts)：`CreateDetachedTerminalWindowInput.profile` 支持 `'shell' | AgentProfileId | 'agent-team-member'`。
36. [apps/desktop/src/renderer/src/runtime/RuntimeClient.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/desktop/src/renderer/src/runtime/RuntimeClient.ts)：扩展 `TerminalAttachment.profile` 支持 `AgentProfileId`。
37. [apps/desktop/src/renderer/src/terminal/TerminalSurface.tsx](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/desktop/src/renderer/src/terminal/TerminalSurface.tsx) & [terminal-replay-policy.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/desktop/src/renderer/src/terminal/terminal-replay-policy.ts)：扩展 `profile` 类型到 `'shell' | AgentProfileId`，保持全屏 TUI Agent（非 shell）的零序列号权威快照重放策略一致。
38. [apps/desktop/src/renderer/src/hierarchy/hierarchy-types.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/desktop/src/renderer/src/hierarchy/hierarchy-types.ts) & [hierarchy-commands.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/desktop/src/renderer/src/hierarchy/hierarchy-commands.ts)：扩展会话类型与命令参数（`setPermissionMode` 传真实 `provider`，`listClaudeSessions` 等传入可选 `provider`，新增 `listAgentModels` 命令，修复固定写死 `'claude-code'`）。
39. [apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/desktop/src/renderer/src/hierarchy/HierarchyShell.tsx) & [DetachedTerminalApp.tsx](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/desktop/src/renderer/src/hierarchy/DetachedTerminalApp.tsx)：透传会话 `kind/profile`，将 `DetachedTerminalApp` 的 `profile` 白名单与 `provider: 'claude-code'` 硬编码改为查清单；移除 `DetachedTerminalApp` 默认写死 `'opusplan'`。
40. [apps/desktop/src/renderer/src/hierarchy/SceneTabBar.tsx](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/desktop/src/renderer/src/hierarchy/SceneTabBar.tsx)、[TerminalPane.tsx](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/desktop/src/renderer/src/hierarchy/TerminalPane.tsx)、[apps/desktop/src/renderer/src/session-canvas/ChildSessionBadge.tsx](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/desktop/src/renderer/src/session-canvas/ChildSessionBadge.tsx)、[ParentProjection.tsx](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/desktop/src/renderer/src/session-canvas/ParentProjection.tsx)：标签名、Fork 按钮显示、自动标题恢复入口、失败重试/新开按钮、子会话计数提示与父会话投影文案全部改为查 `getAgentManifest(kind)`。
41. [apps/desktop/src/renderer/src/hud/TerminalHud.tsx](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/desktop/src/renderer/src/hud/TerminalHud.tsx)：按清单渲染指令文件按钮（`ClaudeMd` vs `AGENTS.md`）、隐藏 `supportsTodos === false` 的待办、隐藏 `accountSwitching === false` 的 `<ClaudeAccountsControl>`、隐藏 `hideEmptyQuotaPlaceholder === true` 下的空额度 `—`、按 `capabilities.permissionModes` 过滤权限菜单及弹窗文案、按 `capabilities.modelSwitch` 提供/处理模型切换入口（见 §6 待拍板 Q1）。
42. [apps/desktop/src/renderer/src/session-canvas/SessionLoaderDialog.tsx](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/desktop/src/renderer/src/session-canvas/SessionLoaderDialog.tsx)：顶部支持在能力包含 `sessionLoader: true` 的 Agent 间切换（默认显示 `Claude Code` 与 `Antigravity` Tab），所有 `'Claude Code'` / `'Claude'` 对话框文案、空列表提示、消息角色名、默认模型兜底文案改为按所选 Agent 的清单显示。
43. [apps/desktop/src/renderer/src/notifications/agent-event-ingestion.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/desktop/src/renderer/src/notifications/agent-event-ingestion.ts)、[NotificationCenter.tsx](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/desktop/src/renderer/src/notifications/NotificationCenter.tsx)、[osc-notification.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/desktop/src/renderer/src/notifications/osc-notification.ts)：通知缺省标题从写死 `'Claude Code'` 改为从会话 `kind` 的清单 `displayName` 推导。
44. [apps/desktop/src/renderer/src/i18n/messages/dag.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/desktop/src/renderer/src/i18n/messages/dag.ts)、[hierarchyTerminal.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/desktop/src/renderer/src/i18n/messages/hierarchyTerminal.ts)、[hud.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/desktop/src/renderer/src/i18n/messages/hud.ts)、[sessionCanvas.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/desktop/src/renderer/src/i18n/messages/sessionCanvas.ts)：将硬编码的 `'Claude Code'` 文案改造为接收 `agentName: string` 参数的模板函数（`dag.ts` 补齐 `'antigravity': 'Antigravity'`）。

> **明确不改动模块（防串线边界）**：
> - `apps/runtime/src/provider-config/*`（服务供应商存储与模型拉取）
> - `apps/desktop/src/renderer/src/hierarchy/ModelSwitchSettings.tsx`、`ProviderSetupDialog.tsx`、`ClaudeTimeZoneSettings.tsx`（仅保持现有 `Claude Code` / `Codex` 范围）
> - `apps/desktop/src/main/claude-accounts/*`（Claude 账户切换引擎与管理）
> - `apps/desktop/src/main/index.ts:572` 的 `openProviderKeyPage` 仅服务于供应商面板，维持 `'claude-code' | 'codex'` 不变。

### 2.2 数据库是否需要迁移及兼容性方案

经全量检查 [migrations.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/runtime/src/storage/migrations.ts)：

1. **`session_runs.profile` 不需要改结构**：v4 以 `ALTER TABLE session_runs ADD COLUMN profile TEXT NOT NULL DEFAULT 'shell'` 添加，**没有 CHECK 约束**，可直接写入 `'antigravity'`。
2. **三张表存在强制 CHECK 约束，必须执行 v35 迁移**：
   - `sessions.kind`（v1）：`CHECK (kind IN ('shell', 'claude-code', 'codex', 'agent-team-member'))`
   - `provider_bindings.provider`（v21 重建）：`CHECK (provider IN ('claude-code', 'codex', 'generic'))`
   - `session_fork_intents.source_provider`（v11）：`CHECK (source_provider IN ('claude-code'))`
3. **SQLite 重建 `sessions` 表的工程难点与做法**：
   - 现状：[database.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/runtime/src/storage/database.ts#L92) 强制开启了 `PRAGMA foreign_keys = ON`，约 20 张子表存在 `REFERENCES sessions(id)`；同时 `sessions` 上挂载了 1 个索引（`sessions_task_idx`）与 **2 个触发器**（`sessions_default_cwd_after_insert`、`session_environment_binding_after_session_insert`）。
   - 此外 [migration-runner.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/runtime/src/storage/migration-runner.ts#L111) 目前把每条迁移包在 `database.transaction(...)` 中，而 SQLite 规定**事务内执行 `PRAGMA foreign_keys = OFF` 是空操作（不生效）**。如果不开外键保护就 `ALTER TABLE sessions RENAME TO sessions_old` 并 `DROP TABLE sessions_old`，会导致子表 `REFERENCES sessions(id)` 被改写成 `sessions_old` 或触发 `FOREIGN KEY constraint failed`。
   - **v35 安全迁移步骤**：
     1. `Migration` 增加 `foreignKeysOff?: true`，`MigrationRunner` 检测到时在开启事务前先执行 `PRAGMA foreign_keys = OFF; PRAGMA legacy_alter_table = ON;`，事务结束后在 `finally` 中恢复 `PRAGMA legacy_alter_table = OFF; PRAGMA foreign_keys = ON;`。
     2. 在事务内重建 `sessions`（保留全部 v1..v34 列：`id, task_id, execution_context_id, kind, status, title, version, cwd, work_status, created_at, updated_at, last_activity_at, archived_at, title_source, provider_title`）、重创索引 `sessions_task_idx` 与 2 个 `AFTER INSERT` 触发器；同步按 v21 同款步骤重建 `provider_bindings` 与 `session_fork_intents` 及各自索引。
     3. 提交事务前执行 `PRAGMA foreign_key_check`，确保任意一行外键零损坏。
4. **旧数据兼容性**：
   - 不改写任何现有行的 `kind` / `provider` / `title` / `metadata_json`，只扩展表约束允许值（现有行均为 `'shell' | 'claude-code' | 'codex' | 'agent-team-member'`，100% 合法），并沿用既有 `DatabaseBackupService` 迁移前自动备份机制。
   - **约束写法**：推荐将三处约束改为 `CHECK (length(trim(<col>)) > 0)`（应用层由 `@matou/domain` 清单做强校验），或显式加 `'antigravity'`（见 §6 待拍板问题 Q4）。

---

## 3. PRD 第 3 节 14 项能力与文件/机制映射

| # | PRD 能力项 | 落点文件 | 复用的现有机制与实现说明 |
|---|---|---|---|
| 1 | **新建卡片** | `packages/domain/src/agent-manifest.ts`、`packages/contracts/src/protocol.ts`、`apps/runtime/src/control/host-action-types.ts`、`apps/runtime/src/cli/mt-cli.ts`、`apps/runtime/src/session-canvas/session-canvas-service.ts`、`apps/runtime/src/session/provider-launch-plan.ts`、`apps/runtime/src/runtime-server.ts` | 将 `mt create session --profile antigravity`、`structure.create.session`、`session.create` 接到清单；扩展 `#maybePromoteShellAgent` 表驱动支持在 Shell 卡片键入 `agy`（及带参命令）自动升级为 `antigravity` 卡片并注入 hook/statusLine 启动。（注：UI 是否需在新建按钮加显式下拉选择器见 §6 Q2。） |
| 2 | **标签与卡片标识** | `apps/desktop/src/renderer/src/hierarchy/SceneTabBar.tsx`、`apps/desktop/src/renderer/src/i18n/messages/dag.ts`、`apps/desktop/src/renderer/src/session-canvas/ParentProjection.tsx`、`apps/desktop/src/renderer/src/session-canvas/ChildSessionBadge.tsx`、`apps/runtime/src/domain/session-repository.ts`、`apps/runtime/src/session-canvas/session-canvas-service.ts` | 标签、DAG 节点 badge、父投影、子角标统一调 `getAgentManifest(kind)?.shortName ?? 'Shell'`，卡片默认标题写入 `'Antigravity'`，退回 Shell 时由 `returnAgentToShell` 识别并恢复 `'Shell'`。 |
| 3 | **首次进入目录（信任工作区）** | `apps/runtime/src/session/provider-hook-server.ts`、`apps/runtime/src/session/provider-notification-event.ts`、`apps/runtime/src/session/pty-session.ts` | 不自动跳过 `agy` 的信任目录确认框；当 statusLine 报 `agent_state` 等待确认（或未初始化前等待输入）时，复用现有 `needs-input` 状态，用户直接在终端里按键选择完成。 |
| 4 | **工作状态** | `apps/runtime/src/agents/agent-adapters.ts`、`apps/runtime/src/session/provider-hook-server.ts`、`apps/runtime/src/session/provider-work-status.ts`、`apps/runtime/src/session/terminal-work-status-tracker.ts`、`apps/runtime/src/index.ts` | 停用终端 ANSI 文本猜测状态；统一由 `statusLine` 与 `Stop` hook 驱动 `workStatuses.set`：<br>• `agent_state: authenticating / initializing` → `starting`<br>• `working` → `running`<br>• `tool_use` → `needs-input`<br>• `idle` → `idle`<br>• `Stop`: `NO_TOOL_CALL` → 完成转 `idle`；`ERROR` 或带 `error` → `error`；`USER_CANCELED` → `idle`。 |
| 5 | **通知** | `apps/runtime/src/session/provider-notification-event.ts`、`apps/runtime/src/index.ts`、`apps/desktop/src/renderer/src/notifications/agent-event-ingestion.ts`、`NotificationCenter.tsx` | 复用 `AgentNotificationRepository`、`AgentNotificationStore` 的 `cooldownKey` 冷却、聚焦卡片免打扰与声音开关。标题由清单动态注入 `'Antigravity'`；`--settings` 中显式合并禁止/不开启 agy 自带 `notifications`，避免双重通知。 |
| 6 | **HUD** | `apps/runtime/src/session/session-hud-registry.ts`、`apps/runtime/src/rpc/runtime-rpc-router.ts`、`apps/desktop/src/renderer/src/hud/TerminalHud.tsx`、`apps/desktop/src/renderer/src/hud/TerminalHud.tsx`（`InstructionFileDialog`） | 复用 `SessionHudRegistry` 与 `TerminalHud`：<br>• **模型+effort**：statusLine `model.display_name` 直出（不剥 `'Claude '`）。<br>• **上下文**：`context_window.used_percentage`。<br>• **额度**：解析 statusLine `quota` 各模型组 5h/weekly 剩余比例与重置时间，转换为 `usageWindows` 展示；未到前完全隐藏（不显示 `—`）。<br>• **执行模式**：`cycle_mode` 实时同步 `permissionMode`。<br>• **工具活动**：`PreToolUse` 加入 `runningTools` + `toolCall.name/args` 提炼目标，`PostToolUse` 结束并累加 `toolCounts`。<br>• **目录 & Git**：复用已有 `cwdTracker` 与 `SessionGitStateRepository`。<br>• **指令文件**：`inspectProviderConfig` 与 `session.instructions-read/write` 按清单顺序检查 `AGENTS.md`、`GEMINI.md`，存在则按钮展示 `AGENTS.md`（或实际命中的文件名）。<br>• **明确隐藏**：按 `hud.supportsTodos === false` 与 `capabilities.accountSwitching === false` 隐藏待办与 `<ClaudeAccountsControl>`；字段缺失一律隐藏。 |
| 7 | **自动标题** | `apps/runtime/src/agents/antigravity-session-catalog.ts`、`apps/runtime/src/session/provider-hook-server.ts`、`apps/runtime/src/domain/session-repository.ts`、`apps/desktop/src/renderer/src/hierarchy/TerminalPane.tsx` | 复用 `sessions.title_source` (`'default' \| 'auto' \| 'manual'`)、`provider_title`、`observeProviderTitle` 与「恢复自动标题」入口。每当收到 `conversationId` 的 hook（尤其 `PostInvocation` / `Stop`）或 statusLine 防抖触发时，从 `conversation_metadata.json` 读取 `conversations[id].summary.Title`，若非空则触发 `onTitleObserved`（`manual` 时仅刷新 `provider_title` 不覆盖用户标题）。 |
| 8 | **载入已有会话** | `packages/contracts/src/claude-sessions.ts`、`apps/runtime/src/agents/antigravity-session-catalog.ts`、`apps/runtime/src/rpc/runtime-rpc-router.ts`、`apps/runtime/src/session-canvas/provider-mode-service.ts`、`apps/desktop/src/renderer/src/session-canvas/SessionLoaderDialog.tsx` | `SessionLoaderDialog` 增加 Agent 切换标签（`Claude Code` / `Antigravity`）；选中 `Antigravity` 时调用目录 RPC 读取 `conversation_metadata.json`（过滤 `AppDataDir === 'antigravity-cli'`、`Internal !== true`、且 `WorkspaceURIs` 解析本地路径匹配工作目录），展示 `Title`（空则 `Preview` 截断）、`UpdatedAt`、`NumSteps` 步数；预览/正文搜索读取 `hook` 口径对应的 `~/.gemini/antigravity-cli/brain/<id>/.system_generated/logs/transcript.jsonl`；点击载入后写绑定并以 `agy --conversation <id>` 启动。 |
| 9 | **重启恢复** | `apps/runtime/src/recovery/runtime-recovery-service.ts`、`RuntimeSessionRecoveryScheduler`、`apps/runtime/src/session/provider-resume-monitor.ts`、`apps/runtime/src/runtime-server.ts`、`apps/desktop/src/renderer/src/hierarchy/TerminalPane.tsx` | 复用 `RuntimeRecoveryCoordinator` 优先级恢复调度与 `provider_bindings`。重启时读取有效 `providerSessionId`，调用 `agy --conversation <id>` 启动；收到对应 `conversationId` 的 statusLine/hook 即调用 `#settleProviderRecovery` 完成恢复；异常退出/超时复用现有恢复失败横幅（文案改为按 Agent 显示 `Antigravity 恢复失败`）与「重试 / 新开」按钮。 |
| 10 | **Fork** | `apps/runtime/src/session-canvas/fork-workflow-service.ts`、`fork-operation-coordinator.ts`、`session-fork-intent-repository.ts`、`provider-hook-server.ts`、`apps/runtime/src/runtime-server.ts`、`apps/desktop/src/renderer/src/session-canvas/BranchDialog.tsx` | 复用现有 Fork 状态机（含 Worktree 创建、可分叉门槛 `observedNormalStop` 与 `forkAuthority` 租约）：<br>1. `ForkWorkflowService` 对 `providerConfig === false` 跳过供应商冻结；<br>2. 子卡片以 `agy --conversation <父 id>` 启动，HookRegistration 标记 `forkAuthority` 与 `parentProviderSessionId`；<br>3. 子运行期收到 statusLine 首次报告 `agent_state === 'idle'` 且 `conversation_id === parentId` 时，`RuntimeServer` 幂等向子 PTY 写入一次 `/fork\r`（不写入 DB 绑定，只更新内存阶段）；<br>4. 随后 statusLine 或任意 hook 上报 `conversationId !== parentId`（新值非空），`ProviderHookServer` 立即将其视为新子身份并通过 `forkCoordinator.confirmAuthoritativeIdentity` 落盘，Fork 成功；<br>5. 超过超时时间（如 30s）或子进程在换 ID 前退出，复用 `#beginForkFailure` 置为 `failed` 并支持重试/移除。 |
| 11 | **执行模式** | `apps/runtime/src/session/provider-launch-plan.ts`、`apps/runtime/src/runtime-server.ts`、`apps/runtime/src/rpc/runtime-rpc-router.ts`、`apps/desktop/src/renderer/src/hud/TerminalHud.tsx` | 参数映射：`default` → 不传参（即 agy 默认自动模式）；`auto` → `--mode auto`；`acceptEdits` → `--mode accept-edits`；`plan` → `--mode plan`；`bypassPermissions` → `--dangerously-skip-permissions`。HUD 点击切换（具体是否全部重启接回 vs 非 bypass 走 Shift+Tab 见 §6 Q3）走 `#respawnWithPermission`；用户在终端内按 `Shift+Tab` 后，`statusLine` 传入的 `cycle_mode` 经 `SessionHudRegistry.ingestProvider` 实时同步回 HUD。 |
| 12 | **模型** | `apps/runtime/src/agents/antigravity-models.ts`、`apps/runtime/src/rpc/runtime-rpc-router.ts`、`apps/runtime/src/runtime-server.ts`、`apps/desktop/src/renderer/src/hud/TerminalHud.tsx` | 启动参数支持 `--model <id>`；通过新 RPC `agent-models.list` 执行 `agy models` 获取可用模型列表并缓存；`session.set-model` 保存所选模型到 `provider_bindings.metadata_json` 并触发重启接回原对话（不触碰 `ProviderConfigStore`）。HUD 的 UI 入口设计见 §6 Q1。 |
| 13 | **`mt` 跨卡片控制** | `apps/runtime/src/session/pty-session.ts`、`apps/runtime/src/agents/antigravity-plugin-installer.ts`、`apps/runtime/control-assets/providers/antigravity-system-message.md`、`apps/runtime/src/control/host-control-server.ts` | 复用已有 `controlEnvironment` 注入 `MATOU_CONTROL_*` 与 `control-assets/bin` 到子进程 `PATH`；启动 Antigravity 时附加环境变量 `MATOU_ANTIGRAVITY_SYSTEM_MESSAGE_FILE=<controlAssetRoot>/providers/antigravity-system-message.md`，由全局插件的 `SessionStart` hook 返回 `{"injectSteps":[{"systemMessage":{"systemMessage":...}}]}` 注入给模型（不使用独立 skill）。 |
| 14 | **未安装 / 未登录 / 额度用完** | `apps/runtime/src/session/pty-session.ts`、`apps/runtime/src/runtime-server.ts`、`apps/runtime/src/session/provider-notification-event.ts`、`apps/desktop/src/renderer/src/hud/TerminalHud.tsx` | `resolveExecutable('antigravity')` 读取 `MATOU_ANTIGRAVITY_COMMAND ?? 'agy'`。若 `pty.spawn` 抛 `ENOENT`，向卡片展示明确的“未检测到 Antigravity CLI（`agy`）”与安装指引（`npm` / 安装命令提示）。未登录时 statusLine 呈 `authenticating`，卡片状态为 `needs-input` 供用户在终端完成登录。额度用完在 HUD quota 反映为 0% 并结合 Stop `error` 发出出错通知。 |

---

## 4. Hook / 状态栏脚本、插件安装时机与 `ProviderHookServer` 对接

### 4.1 目录结构与插件文件（`~/.gemini/config/plugins/matou/`）

PRD 实测已确定 `agy` 仅发现 `.agents/` 或全局 `~/.gemini/config/` 下的插件，且只支持同步命令型 hook。因此 Matou 在全局目录放置纯被动网关插件：

```text
~/.gemini/config/plugins/matou/
├── .matou-plugin-version       # 记录 Matou 版本与清单内容摘要，用于幂等判定
├── plugin.json                 # { "name": "matou", "version": "...", "description": "Matou terminal integration" }
├── hooks.json                  # 位于插件根目录（agy 约定）
└── hooks/
    ├── matou-hook.mjs          # 由 Matou 运行自带 Node 或系统 node 执行（见下文路径策略）
    └── matou-hook.sh           # 轻量入口，快速放行非 Matou 会话并拉起后台上报
```

#### `hooks.json`（插件根目录）
注册 PRD 全部 6 个事件（统一指向 `${PLUGIN_ROOT}/hooks/matou-hook.sh` 并传入事件名或从 stdin `hookEventName` 读取）：
- `SessionStart`
- `PreToolUse`
- `PostToolUse`
- `PreInvocation`
- `PostInvocation`
- `Stop`

### 4.2 隔离、零延迟与 PreToolUse 零输出策略

1. **Matou 外部运行 `agy` 绝对零影响（首行快速退出）**：
   - `matou-hook.sh` 第 1 行检测：`[ -n "$MATOU_PROVIDER_HOOK_URL" ] && [ -n "$MATOU_PROVIDER_HOOK_TOKEN" ] || exit 0`。
   - 当用户在 iTerm / VS Code / 原生终端直接运行 `agy` 时，这两个变量不存在，脚本在 `< 2ms` 内安静退出（`exit 0`，stdout 0 字节，stderr 0 字节），对用户无任何行为变化或延迟。
2. **PreToolUse 及所有非 SessionStart 事件严格零输出**：
   - 已知实测输出 `{}` 会导致 `agy` 拒绝工具调用。因此除了 `SessionStart`，脚本**绝不向 stdout 打印任何字符（包括空行、`{}`）**，所有子进程 stdout/stderr 均重定向到 `/dev/null`。
   - 所有 HTTP POST 上报均在子 shell / detached 后台异步完成：先一次性把 stdin 读入变量或传入管道，以 `(curl ... >/dev/null 2>&1 </dev/null &)` 或 `node -e`（`spawn(..., { detached: true, stdio: 'ignore' }).unref()`，优先使用 Matou 注入的 `MATOU_CONTROL_NODE_EXECUTABLE` 以免用户本机缺少或版本不一致）立即脱钩返回 `exit 0`。单次工具调用的 hook 耗时稳定控制在数毫秒内，上报失败（如 Runtime 正在退出）绝不影响 `agy` 运行。
3. **`SessionStart` 同步注入 `mt` 系统消息**：
   - 仅当 `hookEventName === 'SessionStart'`（脚本可通过 `grep`/轻量 Node 解析判定，或 hooks.json 给 `SessionStart` 传参 `session-start`！）且 `MATOU_ANTIGRAVITY_SYSTEM_MESSAGE_FILE` 文件存在可读时，同步读取文件内容并向 stdout 输出严格符合 `hooks.md` 规范的双层结构：
     ```json
     {"injectSteps":[{"systemMessage":{"systemMessage":"<文件内容>"}}]}
     ```
   - 同时仍在后台异步向 `MATOU_PROVIDER_HOOK_URL` 上报 `SessionStart` payload，不阻塞 `agy` 启动。

### 4.3 状态栏回调脚本（`--settings` 单次注入 + 用户 `statusLine` 透传）

1. **启动注入**：在 [provider-launch-plan.ts](file:///Users/icesword/Documents/AIProjects/matou-multi-agent/apps/runtime/src/session/provider-launch-plan.ts) 为 Antigravity 注入：
   ```text
   --settings '{"statusLine":{"type":"command","command":"<run-specific-statusline-script>","stack_with_default":true}}'
   ```
   （不写入或修改 `~/.gemini/antigravity-cli/settings.json`，完全会话级临时注入；并确保不启用通知字段。）
2. **保留用户自定义 `statusLine`**：
   - 在每次 `#spawn` 登记 `registerSession('antigravity', ...)` 时，Runtime **只读**检查 `~/.gemini/antigravity-cli/settings.json` 是否存在自定义 `statusLine.command`。
   - 生成的 `<token>.statusline.sh` 先读取完整 stdin：
     - 后台异步将该 JSON POST 至 `ProviderHookServer`（`/hooks/<token>?kind=statusline`）。
     - 若启动时未检测到用户自定义 `statusLine`：脚本**零输出**直接 `exit 0`（靠 `stack_with_default: true` 保留 `agy` 自带默认状态栏）。
     - 若用户配置了原 `statusLine.command`：脚本将该 stdin 同步喂给用户原命令并原样透传其 stdout 与退出码，既拿到 Matou 状态/额度/模型数据，又完整保留用户原状态栏显示。

### 4.4 `ProviderHookServer` 归一化对接

复用现有 run 级 HTTP 服务器 `http://127.0.0.1:<port>/hooks/<token>`（Bearer `<token>` 鉴权、退休窗口 `retire(graceMs)` 防止退出的旧进程晚到事件污染新会话）：

1. 注册扩展：`HookRegistrationRecord` 保存 `provider: AgentProfileId`、新增可选 `parentProviderSessionId?: string`（供 Fork 区分）。
2. 字段与路径分诊（`ProviderHookServer.#handle`）：
   - 先校验 token 与 run 未过期。
   - **区分来源**：通过请求路径或 payload 结构区分 `statusline` 与 `hook`：
     - Antigravity **Hook**（camelCase）：识别 `hookEventName`（`SessionStart` / `PreToolUse` / `PostToolUse` / `PreInvocation` / `PostInvocation` / `Stop`），提取 `conversationId`、`transcriptPath`、`modelName`、`toolCall`（`{ id, name, args }`）、`terminationReason`、`error`。**此处采集的 `transcriptPath` 作为真实对话文件路径并持久化到绑定 metadata**。
     - Antigravity **statusLine**（snake_case）：提取 `conversation_id`、`agent_state`、`model`（`id` / `display_name` / `effort`）、`context_window.used_percentage`、`quota`、`cycle_mode`。**明确丢弃 statusLine 自带的 `transcript_path`**（已在 PRD 实测确认其路径错误）。
3. **身份确认与 Fork 衔接**：
   - 普通会话（无 `parentProviderSessionId`）：hook 或 statusLine 首次携带非空 `conversationId` 即确认会话身份并触发 `onIdentityRecorded`。
   - Fork 子卡片（带 `parentProviderSessionId`）：
     - 若收到的 `conversationId === parentProviderSessionId`，**不调用**身份确认；而是把 `agent_state` 转交给 `#onAntigravityForkProbe(sessionId, runId, agentState, conversationId)`，当发现首次进入 `'idle'` 时触发自动输入 `/fork\r`。
     - 一旦收到 `conversationId !== parentProviderSessionId` 且非空，才触发 `onIdentityRecorded`（携带 `forkAuthority`），完成子会话身份认领与持久化。

### 4.5 插件写入时机与幂等策略

- **写入时机**：在 Runtime 准备首次启动 Antigravity 会话时（即 `runtime-server.ts` 注册 `'antigravity'` Hook 之前，或独立启动预热步骤中通过 `AntigravityPluginInstaller.ensureInstalled()` 调用），确保不开启任何 `antigravity` 卡片时不无故触碰磁盘。
- **幂等与并发安全策略**：
  1. 内部维护预期文件内容映射（`plugin.json`、`hooks.json`、`hooks/matou-hook.sh`、`hooks/matou-hook.mjs`）及 `EXPECTED_PLUGIN_VERSION`（随 Matou 构建产物同步升级）。
  2. 先读取 `~/.gemini/config/plugins/matou/.matou-plugin-version` 及目标文件存在性；若版本标记匹配且关键文件均存在且内容哈希一致，**零写入直接返回**。
  3. 仅当缺失、版本变化或内容不一致时，采用“同目录临时文件 + `chmod 0o755`（脚本）+ `rename` 原子替换”覆盖更新，最后更新 `.matou-plugin-version`。多张 Antigravity 卡片同时启动时通过进程内单飞锁 `Promise` 串行化一次，避免并发写入竞态。

---

## 5. 后续实施关卡拆分、测试与交付物

按关卡渐进落地，每关门禁清晰且互不破坏现有基线：

### 第 2 关：多 Agent 底座 + DB v35 迁移 + 清单驱动化重构
- **范围**：
  - 实现 `@matou/domain` `AgentManifest` 清单与 `@matou/contracts` 契约泛化。
  - 实现 `MigrationRunner` `foreignKeysOff` 与 `migrations.ts` v35 表重建及回滚保护测试。
  - 将 Runtime（Repository / Canvas / Graph / Recovery / Control / `mt` / RPC）与 Desktop（Renderer / Detached / Notifications / i18n / HUD）中所有写死的 `'claude-code' | 'codex'` 与 `'Claude Code'` 文案切换为查清单与按能力门控（此时清单尚未开放或仅测试态启用 `'antigravity'` 占位验证，不破坏供应商与账户隔离）。
- **验证测试**：
  - 新增 `agent-manifest.test.ts` 与 `migrations-v35.test.ts`（覆盖老库 v34 数据升级、外键与触发器完整性校验）。
  - 全量既有 `Claude Code` 与 `Codex` 单测 / 集成测试必须全部通过；运行 `pnpm typecheck` 与 `pnpm check:identifiers`。
- **交付物**：纯多 Agent 清单底座架构、零回退的绿色测试套件。

### 第 3 关：Antigravity 核心闭环（启动 / 恢复 / 插件 Hook / statusLine / 状态 / 通知 / HUD）
- **范围**：
  - 实现 `AntigravityPluginInstaller`（幂等安装 + 外部零输出测试）、`ProviderHookServer` camelCase + statusLine 解析、`AntigravityAdapter`。
  - 打通 `resolvePtyCommand`（`MATOU_ANTIGRAVITY_COMMAND`）、`RuntimeServer` 启动与 ENOENT / 未登录 `needs-input` / Stop 状态映射 / 按 Agent 通知 / HUD（模型+effort、上下文、5h/weekly 额度、工具计次与运行中工具、`AGENTS.md`/`GEMINI.md` 编辑、隐藏待办/Claude账户/空占位）与重建恢复。
- **验证测试**：
  - 单元测试：覆盖 hook 脚本在无 Matou 环境变量时的零输出快退、PreToolUse 零 stdout 校验、`statusLine` 用户原命令透传、camelCase / snake_case 解析、错路径剔除。
  - 集成测试：使用 Mock `agy` CLI 进程跑通 Launch → 信任工作区/未登录 → Working/ToolUse/Stop 通知 → HUD 更新 → 重启恢复 / 恢复失败重试。
  - 现有 Claude/Codex 回归测试全绿、`pnpm check:identifiers` 通过。
- **交付物**：Antigravity 基础可用闭环（对应 PRD 第 1–6、9、14 项及基础通知与 HUD）。

### 第 4 关：Antigravity 进阶闭环（Fork `/fork` 编排 / 载入历史会话 / 自动标题 / 执行模式 & 模型切换 / `mt`）
- **范围**：
  - 实现 Antigravity Fork 二阶段状态机（`--conversation <parent>` + statusLine `idle` 触发 `/fork\r` + 身份变更认领 + 超时失败兜底）。
  - 实现 `AntigravitySessionCatalog` 接入 `SessionLoaderDialog`、自动标题监听 `conversation_metadata.json`（含手动标题保护与恢复自动标题）。
  - 实现 `AntigravityModels` 缓存解析、执行模式与模型重启接回、以及 `SessionStart` 注入 `antigravity-system-message.md` 闭合 `mt` 跨卡片控制。
- **验证测试**：
  - 集成测试：模拟父子卡片同时开、`/fork` 成功与超时回退、`SessionLoaderDialog` Antigravity 列表过滤与预览搜索、`Title` 自动追踪与手改保护、模式/模型重启接回、`mt send/read` 在 Antigravity 卡片间通信。
  - 全量端到端回归测试 + `pnpm check:identifiers`。
- **交付物**：PRD 第 1–14 项代码全部闭合。

### 第 5 关：真实打包 App 实测、补测项核验与「交互对照矩阵」验收
- **范围**：
  - 打包真实 macOS App，使用本机真实 `agy` 验证 PRD 第 4 节标记待实测项：
    1. 补测命令执行、文件编辑、浏览器调用等工具审批时 `agent_state === 'tool_use'` 与自动放行保持 `working` 的精确边界；
    2. 实测父卡片与子卡片并发打开同一对话时执行 `/fork` 是否写坏父对话或错乱状态；
    3. 测量 hook 延迟（工具调用无感知）与事件→HUD/状态同步延迟（≤ 1s）；
    4. 按 AGENTS.md 与 PRD 第 6 节逐项对比同场景 Claude Code 卡片，产出带截图与差异结论的「交互对照矩阵」。
- **交付物**：打包 App 实测报告与完整 14 项交互对照矩阵、全量回归 + `pnpm check:identifiers` 通过证明。

---

## 6. 风险、边界条件与待拍板问题

### 6.1 失败场景与边界应对

| 场景 | 风险点 | 本方案应对措施 |
|---|---|---|
| **`agy` 未安装** | `MATOU_ANTIGRAVITY_COMMAND` 或 `agy` 不在 PATH，`node-pty` 启动抛 `ENOENT` | 先走现有 `resolveProviderCommandEnvironment` 探测登录 shell `PATH`；仍不存在时捕获错误，不回退 silent shell，在卡片顶部与恢复区域以 `'Antigravity'` 名义给出清晰安装指引（保留 Shell 重试或关闭能力）。 |
| **`agy` 未登录 / 首次信任文件夹** | 启动卡在登录引导或目录信任交互页，无 `conversation_id` | 识别 `agent_state: 'authenticating'` / `initializing` 映射为 `needs-input` / `starting` 并提示用户在终端内直接完成交互；不越权自动确认信任或替用户登录。 |
| **额度用完** | statusLine `quota` 剩余 0% 或会话报 Stop `ERROR` | 额度归零在 HUD 上高亮展示剩余 `0%` 与重置时间；Stop 带 `error` 时卡片进入 `error` 状态并按通知规则触发 `Antigravity` 出错通知。 |
| **`agy` 自动升级字段缺失** | `agy` 实测期间已从 1.5.7 升到 1.5.8，未来部分 statusLine/hook 字段可能重命名或消失 | 全部采用严格防御性可选字段解析：缺失 `quota` / `context_window` / `effort` / `cycle_mode` 一律保持最近有效值或按清单 `hideEmpty` 隐藏，绝不将 `undefined` 转为 `0` 或让异常中断状态机。 |
| **Hook 上报失败** | Runtime Hook Server 繁忙、重启中或端口不可达 | Hook 脚本与后台发送**永远 `exit 0`、零异常输出、零阻塞主进程**；仅记录内部 debug 失败（或丢弃），绝不允许 hook 故障卡死或拒绝 `agy` 工具调用（尤其 `PreToolUse` 硬保证 0 字节输出）。 |
| **用户在 Matou 外运行 `agy`** | 全局插件 `~/.gemini/config/plugins/matou/` 对本机所有 `agy` 生效 | 脚本首行校验 `MATOU_PROVIDER_HOOK_URL` 与 `MATOU_PROVIDER_HOOK_TOKEN`，外部终端缺失环境变量则直接 `exit 0` 零输出；statusLine 仅通过每次启动参数 `--settings` 临时注入，绝不污染用户全局 `~/.gemini/antigravity-cli/settings.json`。 |
| **父子卡片并发打开同一对话执行 `/fork`** | 子卡片以 `--conversation <父 id>` 挂载期间，父卡片可能正在工作，同一 `conversationId` 并发写入或触发全局/同名 hook 误认领 | ① 子卡片在运行期间靠 **run 级 URL/token** 完全隔离 HTTP 通道，父卡片与子卡片事件互不串流；② 子卡片绑定到 `parentProviderSessionId` 时**抑制**对父 `provider_bindings` 状态的改写且要求必须等子 statusLine 进入 `idle` 再发送 `/fork\r`，仅在 ID 变为新值后才建子绑定；③ 第 5 关仍列为重点实测：验证 `agy` 底层 CLI 在两进程共持同一 `--conversation` 直至 `/fork` 完成瞬间是否文件锁冲突或污染父对话记录（如发现底层锁冲突，可退一步先短暂检查父卡片是否空闲再发 `/fork`）。 |

### 6.2 需要用户拍板的问题

以下 4 个点涉及**产品交互口径**或与当前代码现状的细节差异，请审定后我们再进第 2 关动手编码：

1. **Q1（PRD #12 卡片内切换模型入口 —— 代码现状缺失 UI 触发器）**
   - **发现的差异**：后端已存在 `session.set-model` RPC 与 `commands.setModel`，但当前 `TerminalHud.tsx` 仅把当前模型渲染为不可点击的纯文本 `<span>`（全仓代码确认没有任何 UI 按钮调用 `commands.setModel`）；同时 PRD 明确“不进服务供应商面板”。而 PRD #12 要求“切换模型与 Claude 卡片同一入口，候选来自 `agy models`，切换后重启并接回对话”。
   - **可选方案**：
     - **方案 A（推荐）**：把 `TerminalHud` 上的 `.status-model` 升级为**支持 `capabilities.modelSwitch !== 'none'` 的 Agent 可点击弹出模型菜单**——对 **`antigravity`** 显示来自 `agy models` 的模型菜单，点击后确认/直接重启并接回对话；而 **`claude-code`** 暂保持不可点击纯文本（或同步启用现有的 3 个档位 `opusplan` / `claude-opus-4-6` / `claude-sonnet-4-6`），确保不未经批准改动 Claude 既有 UI。
     - **方案 B**：也同步为 `claude-code` 开放 `.status-model` 下拉菜单切换既有 3 档位（与 `session.set-model` 契约完全统一，真正“同一入口”），但会微调 Claude 卡片的 HUD 交互可见行为。
   - **请拍板**：`TerminalHud` 模型菜单是只对 `antigravity` 可点击（方案 A），还是同步开放给 `claude-code` 的 3 档模型切换（方案 B）？

2. **Q2（PRD #1 新建卡片入口 —— UI 当前全默认建 Shell）**
   - **发现的现状**：目前桌面端新建事项/画布/同级卡片（`+`）一律直接创建 `Shell` 卡片；CLI/控制面（`mt create session --profile`、`structure.create.session`、`session.create`）与 `SessionLoaderDialog` 则区分 agent 类型。
   - **可选方案**：
     - **方案 A（最小改动，推荐先确认）**：保留现有 UI 默认建 Shell 的主流程，把 `agy`（及别名命令）接入 Shell 终端命令自动升级（在 Shell 敲 `agy` 即刻升级为 Antigravity 卡片），并同步为 `mt create session --profile`、host control、`SessionLoaderDialog` 加入 `antigravity`（满足“凡是能选 Claude Code / Codex 的入口都加这一项”且不增加额外点击负担）。
     - **方案 B**：除上述外，同时在 UI 新建卡片按钮旁增加可选 Agent 下拉菜单（`Shell / Claude Code / Codex / Antigravity`），让用户不敲命令也能鼠标直接新建 `Antigravity`（会新增一个 Claude/Codex 目前没有的 UI 选择器）。
   - **请拍板**：采用方案 A（Shell 输入 `agy` 自动升级 + CLI/RPC/LoadDialog 全打通），还是追加方案 B 的 UI 新建下拉菜单？

3. **Q3（PRD #11 执行模式切换方式 —— `default` 标签与“是否总是重启”）**
   - **发现的差异**：Claude 卡片在 `default / acceptEdits / plan` 间切换时**不重启进程**（向 PTY 发 `\x1b[Z` Shift+Tab 切），仅进出 `bypassPermissions` 时弹窗确认并重启；且菜单里过滤掉了 `'auto'` 只有 4 项。而 PRD #11 写明“Matou 的权限模式切换对应 agy 的四种模式：自动（agy 默认）、自动接受编辑、计划、跳过所有审批。切换方式与 Claude 相同（重启并接回对话）；用户在 agy 里按 Shift+Tab 切换后，HUD 同步显示”。
   - **请拍板两点细节**：
     1. Antigravity 菜单的默认第一项显示文案是沿用 `Default` 还是标为 **`Auto (Default)` / `自动（默认）`**（内部对应 `default` → 不传参，等同 agy 默认自动）？
     2. 切换 `Auto / Accept Edits / Plan` 三档时，是否按 PRD 字面**一律重启接回**（最稳妥、无视 Shift+Tab 光标与顺序变化），还是像 Claude 一样三档发 `Shift+Tab`、仅进出 `Bypass Permissions` 才弹窗确认并重启？

4. **Q4（数据库 v35 CHECK 约束策略）**
   - **可选方案**：
     - **方案 A（推荐）**：在 v35 重建 `sessions` / `provider_bindings` / `session_fork_intents` 时把 `CHECK` 放宽为非空字符串 `CHECK (length(trim(<col>)) > 0)`，由 `@matou/domain` 清单与 RPC Zod schema 负责严格白名单校验——未来接入 Codex 补齐、DSH、Kimi 时**无需再次重建被 20 张表引用的 `sessions`**。
     - **方案 B**：保持数据库严格 `IN ('shell', 'claude-code', 'codex', 'antigravity', ...)`，后续每加一个 Agent 再加一版迁移重建。
   - **请拍板**：倾向方案 A（放宽非空 + 领域层强校验）还是方案 B（每期加 DB 迁移）？
