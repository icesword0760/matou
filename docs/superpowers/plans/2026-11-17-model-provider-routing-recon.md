# 正式模型分配接入调研与干净基线

日期：2026-11-17；基线 HEAD：`02bdc536fa685d8e794a81fcad575804bb6f3caf`。
本关只读源码、独立安装依赖、运行既有定点测试、写此调研；没有产品代码修改、提交、UI 生成、真实模型调用或账号操作。既有未跟踪规格文件原样保留。本文是供主负责人定实现方案的接入证据，不是正式实现计划。

## 结论与调用链

既定方案可复用，但不能只增加设置字段。最小闭环覆盖下面六处：设置局部保存、启动快照与档位身份、订阅精确模型准备、能力约束、真实目标出口保护、普通及计数请求转发。最大风险是“表面三档已经配置，实际仍受原供应商映射/能力/出口约束”，以及延迟读取可变设置破坏运行中快照。

实际入口：

```text
ModelSwitchSettings（供应商/账号列表，已有订阅 settings-read）
  + AgentModelSettings（目前仅上下文和压缩）
  → agent-model-settings.get / set
  → RuntimeRpcRouter → AgentModelSettings → SQLite preferences[agent.modelPreferences]

terminal.spawn / SessionExecutionService.startOrResume / ensureSessionRunning
  → RuntimeServer.#spawnSerialized 或 #spawn → #spawnAccountReady
  → provider settings binding + ProviderConfigStore.launchSelection
  → modelSettings.preferences + provider snapshot + launchWindow + egressPlan
  → PtySession.create
    → SubscriptionAccounts.prepareLaunch（默认供应商为订阅时）
    → buildClaudeLaunch → 每次进程独立环境和 settings
    → claudeModelRouting → startClaudeRequestGateway → pty.spawn
Claude 档位 / 子代理请求 → loopback gateway → 精确 API endpoint 或本地订阅 router → 实际供应商
```

定位：`apps/runtime/src/runtime-server.ts:503-508,571-590,1934-1944,2159-2190,2426,2460-2476`；`apps/runtime/src/session/pty-session.ts:213-247,339-385`；`apps/runtime/src/rpc/runtime-rpc-router.ts:189-190`。所有下述路径均相对于本 worktree；行号是本次基线源码位置。

## 1. 设置字段、专用 patch 与失效值来源

**现状证据**：`packages/contracts/src/agent-model-settings.ts:1-25` 无分配字段。`apps/runtime/src/session/agent-model-settings.ts:6-7,41-64,90-95` 在现有 preferences 行读写，validate 重建对象会丢新字段，set 整份覆盖。UI `AgentModelSettings.tsx:108-120` 保存压缩前先 get，再携带 contextWindows 全量 set，仍有 get/set 竞态；`ModelSwitchSettings.tsx:215` 的上下文保存也是另一个写入者，仅新增分配专用 RPC 仍需让旧 set 保留不属于它的分配字段。

**建议类型/签名（供正式方案定名）**：

```ts
type ClaudeRole = 'opus' | 'sonnet' | 'haiku'
type ClaudeRoleAssignments = Partial<Record<ClaudeRole, { providerId: string; model: string }>>
// 缺项 = 沿用启动当时的默认供应商；{} = 恢复全部默认。
// AgentModelPreferences.claudeRoleAssignments?: ClaudeRoleAssignments
// RPC: agent-model-settings.patch-assignments
patchAssignments(input: { assignments: ClaudeRoleAssignments }): Promise<AgentModelSettingsSnapshot>
```

不新增表、依赖或通用 patch 框架。校验输入后，在全部异步校验结束后重新读取最新 preferences，同步 merge/write；不能跨 await 持有旧 preferences。旧 set 的职责保持上下文/压缩并从数据库保留 assignments；必要时把已有上下文/压缩写入口也做按字段合并，防止互相覆盖。协议白名单需补 `packages/contracts/src/protocol.ts:229-230`，RPC 接入上述 router，沿用既有访问策略，不能绕过只读保护。

**UI 取值**：模型目录已有 `AgentModelSettings.snapshot → availableModels`（`agent-model-settings.ts:13-25,45-53`）：官方内建用 native catalog；订阅用 subscription.modelMetadata；API 用 modelMetadata 并补 model/modelMapping 中存量 ID。这份“补入的 ID”证明旧选择存在，不证明目录仍可用。账号实时状态应复用 `ModelSwitchSettings.tsx:99-113` 已读取的 `SubscriptionSettingsSnapshot.accounts[providerId]` 和 provider-config 变更订阅，避免新开额度轮询。`SubscriptionAccounts.#settings:576-625` 区分 needs-sign-in、router-not-running，复用缓存；明确刷新走 `#models:650-664` 并更新持久化目录。模型来源为账号目录与 router definitions 的交集：`#readModels:666-673`，`availableModels:743-758`；非对话模型过滤已有来源可复用，不能只用前端较窄的过滤规则。

AgentModelProvider 当前缺 account status/默认 role 模型/实际默认供应商 ID。建议 snapshot 投影这些非敏感字段，或把父组件已有 provider snapshot/accounts 传入分配区；不要前端猜“列表第一个=默认”。失效 providerId/model 字符串保留于 preferences，候选列表缺项时插入不可选的旧值和错误；运行状态 needs-sign-in/account-unavailable/router-not-running 要与“模型已删除”区分。订阅 settings 的缓存目录未必最新，只有显式刷新后的缺项能更可靠地说明当前目录无该模型；不能把读失败 models=[] 直接当作全被删除。

**必要测试**：`session/agent-model-settings.test.ts` 增加旧设置缺字段、非法 provider/model、空对象恢复、失效选择读回不删除、分配与压缩/上下文交错保存、异步校验期间写入；`runtime-server.test.ts` 或 RPC suite 增加方法白名单/只读拒绝；`AgentModelSettings.test.tsx` 覆盖账号失效、目录删除、草稿保留和 patch payload。这里建议测试，未新增测试代码。

## 2. 启动/恢复/重启：快照边界和档位身份

**现状证据**：已有活进程的同 profile attach 直接返回复用（`runtime-server.ts:1980-2020`），不会走新 PTY。真正恢复和重启进入统一 spawn；Claude 每次读取当前全局供应商，Codex 可以读 binding 的供应商（`:2160-2169`）。绑定模型经 `restoredProviderModel`（`claude-model-routing.ts:110-118`）恢复。当前 preferences 至少在恢复模型选择、launchWindow 和传入 routing 时分别读取（`:2177,2426,2461`），且中间有 await，因此不能称为一次一致快照。

**最小建议**：在一次真实新进程 launch 的准备边界捕获一次 preferences，以及本次用到的 provider views、API launch env/精确模型 metadata；把解析结果作为 runtime 内存参数传给 PTY/routing。建议局部类型：`ResolvedClaudeRoutingSnapshot { preferences; roles: Record<ClaudeRole, ResolvedTarget>; compactionTarget? }`；`ResolvedTarget` 含 providerId、model、目标类型、该模型 metadata 和 runtime 私有连接准备信息，不发往 renderer，不持久化凭据。`launchWindow` 可接收本次 preferences/已解析目标，避免自行再次读库。

“新会话”建议按规格的“新启动进程”解释：新卡片、停止后恢复、崩溃后重建、权限变更导致的重启、fork 新进程都捕获最新分配；仅重新打开/附加活进程保持旧快照。不新增跨重启持久化路由快照。如产品要旧聊天永久固定历史配置，那是额外存储与行为取舍，当前没有采用。此边界有既有生命周期支持（`pty-session.ts:224-228` 的注释明确统一最终入口）。

**禁止误接全局激活**：`provider-config.activate` 在 `runtime-server.ts:1305-1345` 预检并调用 `#restartProviderSessions`；`:4179-4268` 比较 digest、写 binding、处置旧进程再 spawn。分配 patch 不调用 activate、不写 activation token、不排 restart。`#providerLaunchDigest:4127-4160` 当前含 compaction/context/供应商能力，没有 assignments；需按正式方案以同一 launch snapshot 表达相关分配，不能只把整个 preferences 塞进 digest 触发无意义变更。既有 activate 是用户主动动作，保留其原行为。

**另一个快照漏洞**：`claude-model-routing.ts:27-48` 的 custom compaction 在第一次请求才从 store 读 provider/env；仅冻结 preferences 不够。API endpoint/key/model 应在启动准备捕获，官方 OAuth 的正常 token 刷新与订阅凭据存活检查仍可发生，不能把过期 token 永久冻结。供应商删掉/账号失效时显式失败，不改投其他供应商。

**档位身份**：`pty-session.ts:339-349` 非订阅只为已配置 modelMapping 的档位生成 `matou-api/<role>`；同模型可属于不同档位，无法从真实模型名逆推。分配启用时为三个档位都保持稳定虚拟身份，包括默认 provider 没有 modelMapping 时。兼容已有订阅 selector、`[1M]`、Fable→Opus（`model-router/claude-launch.ts:78-84,135-146`）；不要把目标供应商的 Sonnet 映射误当成本次源 Sonnet 的目标模型。default 主模型需要明确映射到原默认档位，不能猜字符串；优先保留既有默认启动语义。

**必要测试**：`runtime-server.test.ts` 测保存前后的 PID/已启动 routing 不变、新建与恢复捕获最新、attach 保持原快照、并发保存不混两版、全局 activate 保留既有重启语义；`claude-model-routing.test.ts`、`subscription-spawn.test.ts` 测三个档位同名模型也路由不同、Fable、`/model`、子代理 selector、custom 优先/follow 跟实际档位、启动后改 endpoint 不影响旧进程。

## 3. SubscriptionAccounts：精确目录模型，不改写共享档位映射

**现状证据**：`subscription-accounts.ts:137-201` 的 prepareLaunch 查持久化 row、精确 credentialRef、有效状态，acquire lease，注册 aliases，最后固定 `mainRole:'opus'` 并构建三个角色。aliases 只来自 row.modelMapping（`:170-186,769-784`）。`ProviderConfigStore.launchSelection:266-271` 也要求原 Opus metadata，不能直接拿它准备一个只指定任意目录模型的目标。`manager.ts:62-63` 的 alias 状态按供应商覆盖，management `setAliases:77-83` 是供应商级 PUT；为每个新 session 临时重映射 opus/sonnet 会污染仍运行的会话。

**重要核清**：固定 router 本来支持目录原始模型 ID，而非只能走三档 aliases。上游固定 commit 的 `sdk/cliproxy/service_models.go:323-334` 先应用 aliases 再 prefix；`:1277-1298` 未映射模型保留，fork=true 时映射模型也保留原 ID；`:705-747` 注册 `<prefix>/<model>`。本地 aliasesFor 设置 fork=true，故无须增建或改写三档 aliases 来支持任意合法目录模型。目录必须经过账号可用目录及注册 whitelist 双重检查，不能把任意文本都送入 router。

**最小建议签名**：在 SubscriptionAccounts 内共用原有 credential/lease preflight，增加精确目标准备入口，例如 `prepareModelTarget(providerConfigId: string, model: string, scope?: string): Promise<{ lease: RouterLease; baseUrl: string; headers: Record<string,string>; model: string; metadata: SubscriptionModel }>`；这里 model 是明确选中的目录 ID，返回 `<prefix>/<raw model without client [1M] marker>`，不接受 role→原 mapping 的再次解析。先核对 `client.models(credential.name)` 真正发布该 ID；缺失报 models-not-ready，不静默使用原 Opus。旧 prepareLaunch 留给旧全供应商启动路径，保持兼容，避免一处修改造成订阅菜单行为改变。scope 可按会话复用同账号 lease，退出/准备失败必须释放；异步准备晚于 close 完成时立即释放。PTY 已有 `:386-390,787-797` 的释放挂点可复用。

现有范围每种订阅只保存一个账号（`provider-config-store.ts:189-191`）；不扩展多账号存储，不新建账号选择系统。`model-router/config.ts:73-78` 已 force-model-prefix、request-retry=0/max-retry-credentials=1，保留无供应商回落策略。prefix 目前按订阅供应商，精确账号保证依赖现有“每种一个账号”约束；不可宣称已支持同供应商多账号隔离。

**必要测试**：`subscription-accounts.test.ts` 与固定 router fixture：选择原 mapping 未包含的模型、Sonnet 指向 GPT 且不走 GPT 供应商的 Sonnet mapping、两活会话不同模型互不改 aliases、缺失 whitelist/失效 credential 不 fallback、共享 lease 退出释放、prepare/close 竞态。`claude-model-routing.test.ts` 需断言跨供应商没有携带源鉴权。

## 4. 多目标 context/output/effort 来源与限制

- 原始类型：`packages/contracts/src/provider-config.ts:87-95` 的 metadata 提供 contextWindow/maxContextWindow/maxOutputTokens/supportedEfforts；API 探测解析在 `provider-models.ts:16-33,76-115`；官方 Claude 本地 CLI catalog 在 `session/native-model-catalog.ts:22-60`。订阅能力来自账号目录与 router definitions 合并，Codex 以 router definition 为 capability contract（`subscription-accounts.ts:743-758`），不是 model ID 名字推测。
- `AgentModelSettings.launchWindow:66-87` 当前只看单 provider/default + 原 role mapping，subscription 默认 512k 偏好来自 `initialContextMode:99-100`。`contextTokens:8-10` 固定 512k 是用户请求预算，不是模型已证明支持 512k。
- `buildClaudeLaunch:152-171` 已说明客户端 `/model` 后不能动态缩小窗口，已知三档取最小，最高 1M；低于100k的订阅启动被拒绝。output 只按 mainRole 设置并 cap128000，跨目标后不足以约束其他档位。`GatewayOverride.maxOutputTokens` 和 gateway rewrite（`claude-request-gateway.ts:286-303`）已有逐请求限幅能力。
- effort 当前来自源 provider effortMapping/reasoningEffort（`claude-model-routing.ts:70-78`）；不能无校验向任意 GPT/DeepSeek 透传同一 effort 或继承不兼容 thinking。

**建议**：用已解析的三档实际 target metadata 算会话共同窗口，尊重各目标 contextWindows 偏好但不得把源供应商的大窗口当目标能力。已知能力取共同安全值；任何目标能力未知，保留 unknown，不能把默认512k称为安全上限（具体 CLI 未知窗口行为须 fixture 验证，未自定新数值）。每次消息按实际目标 maxOutputTokens 限幅，不只按启动主档位。请求 effort 依据明确档位设置意图并对实际模型 supportedEfforts 校验；follow 不强塞源 provider thinking，custom compaction effort 优先。不新增 LLM 探测请求。

**必要测试**：`agent-model-settings.test.ts`/`claude-launch.test.ts` 验证混合 200k/272k/1M、已知+未知、512k超真实能力、默认Haiku仍计入最小；`claude-model-routing.test.ts`/`claude-request-gateway.test.ts` 验证逐目标 output cap、未知不造能力、支持/不支持 effort、count 不注入 generation 参数。

## 5. egress 必须看真实官方目标集合

**现状证据**：`egress/egress-monitor.ts:77-100` 由启动 env/订阅种类产生单 family：Anthropic、OpenAI、ChatGPT subscription；Antigravity subscription 仅 hideTimeZone，沿用这一已有策略。`:106,149-150` 一个 sessionId 只容纳一个 family；`packages/contracts/src/egress.ts:54` wire 同样是单值。`runtime-server.ts:2184-2190,2719-2720` 只注册默认 provider。因此“默认DeepSeek，Opus官方GPT/Claude”会绕过对应保护，custom compaction 跨官方目标也有同类既有风险，与本需求直接相关。

**最小建议**：由启动解析后的各实际 role target + custom compaction target 得到官方 family 去重集合（不能从改成 loopback 后的 URL 推断）。建议将 monitor 内部 session→family 改为集合，注册支持多个 family；任一个未豁免的目标异常则冻结，只有所有目标正常/已豁免才能解冻。不能简单重复调用现有 registerSession（会覆盖），也不能用伪造 sessionId 代理注册（freeze/thaw 需要真实 PID/session）。需要同步审查 family membership helpers `:515-543`、恢复/override路径 `:286-311,492-505` 和 wire 消费者，防止 A 恢复时提前解冻仍受 B 限制的会话。这是需求要求的必要接口调整建议，本关未动核心代码。

同时复用官方任一目标的 hideTimeZone 规则；多出口 country 不能凭默认第三方国家取值，按已有时区隐藏约定保守处理并在正式方案确定。网关实际 HTTP 发出前也应检查目标保护状态，尤其预注册冻结与 gateway 转发之间的竞态；仅暂停 PTY 不保证已经接收的 HTTP 请求停止。可增加局部 `assertTargetAllowed(target)` 回调挂在 resolveOverride/forward 边界，不造新监控器，不发额外公网探测。

**必要测试**：`egress-monitor.test.ts` 的多family注册、任一冻结/全部恢复、单family override不放行另一个、移除会话清理；`runtime-server.test.ts` 默认第三方+官方角色+官方压缩；gateway fixture 断言被保护时上游0请求。

## 6. gateway/count_tokens 与固定订阅网关边界

`claude-request-gateway.ts:116-138` 两种 POST 都调用 resolveOverride，但`:127`把 count override 裁成仅 model，丢目标URL/鉴权；现有 `claude-request-gateway.test.ts:260` 明确断言这个旧行为。对新需求必须改测试，不能把现有绿色视为支持多目标计数。建议 resolve 输入增加 `kind: 'message' | 'count_tokens'`（保留 compaction 判定），count 使用相同 role target 的 baseUrl/headers/model，裁掉 effort/thinking/maxOutputTokens 等 generation 改写，不把带 compaction marker 的 count 当 custom compaction。

鉴权清理已有 `gateway:151-166`：有 override.headers 或跨 origin 时剔除 cookie/认证头；每个精确目标始终传明确 headers（必要时空对象），即使两个账号在同 origin 不同路径也不能继承源密钥。官方 OAuth 复用 `claude-model-routing.ts:80-96` 的 runtime 凭据获取，测试必须 mock；本关没有访问真实钥匙串/登录。loopback不走网络代理、拒绝重定向、流式取消和错误传播沿用。

固定依赖在 `tooling/model-router/version.json:2-4`，commit `f7ae8feeb3710319f524730849fbe32fde1751e4`。仅下载该 commit 的有关源码到本 worktree `.cache/routing-recon-source/` 作证据，没有启动真实账号 router：

- 上游 `internal/runtime/executor/codex_executor_tokens.go:19-67` 翻译 Codex payload 后本地 tokenizer 计数并转回 token-count response；`:70-86` 只专门匹配 GPT5/4.x/3.x，其他（包括本次 GPT6 名称）用 Cl100kBase。**支持计数接口不等于目标模型精确官方计数**。不在此关擅自升级 router/分词器。
- 上游 `internal/runtime/executor/antigravity_executor_tokens.go:22-45,91-108` 验签/翻译后调用 `/v1internal:countTokens`，使用该账号 Bearer token；可能涉及远程鉴权刷新、延迟与配额，不能作纯离线测试。现有 transport patch 保留连接身份；本关没有调用该官方接口。
- raw 目录注册、fork/prefix 证据见第3节下载的 `service_models.go`。源码语义核对不等于已跑 patched binary；`protocol-regression.test.ts:21-25` 需要显式 MATOU_MODEL_ROUTER_BINARY，否则跳过。后续必须在隔离 fixture router 验证普通/计数确实命中指定 target，不能只 mock resolve。

上游复核链接：[Codex计数源码](https://github.com/router-for-me/CLIProxyAPI/blob/f7ae8feeb3710319f524730849fbe32fde1751e4/internal/runtime/executor/codex_executor_tokens.go)、[Antigravity计数源码](https://github.com/router-for-me/CLIProxyAPI/blob/f7ae8feeb3710319f524730849fbe32fde1751e4/internal/runtime/executor/antigravity_executor_tokens.go)、[模型注册源码](https://github.com/router-for-me/CLIProxyAPI/blob/f7ae8feeb3710319f524730849fbe32fde1751e4/sdk/cliproxy/service_models.go)。

## 基线：命令、结果与验证界限

独立 node_modules 为本 worktree 真实目录，不软链主仓库。`pnpm install --frozen-lockfile` 通过，467 packages 全部复用包缓存，pnpm10.17.1，锁文件未变；postinstall仅修复本 worktree node-pty spawn-helper权限。原有警告：electron-winstaller build script被pnpm策略忽略；本关不做Windows打包，未更改允许脚本策略。

日志目录：`/Users/icesword/.claude/agent-foreman/runs/matou-routing-recon-01/logs/`。

| 命令 | 结果 | 完整日志 |
| --- | --- | --- |
| `pnpm install --frozen-lockfile` | 通过；锁文件未更新 | install.log |
| `pnpm build:packages` | domain/ui/contracts 构建通过；仅测试前置产物 | build-packages.log |
| 下方 runtime 定点命令 | 11文件通过/1文件跳过；137通过/17跳过 | runtime-targeted.log |
| `pnpm --filter @matou/desktop exec vitest run src/renderer/src/hierarchy/AgentModelSettings.test.tsx` | 1文件15通过 | settings-ui.log |
| `pnpm --filter @matou/runtime exec vitest run --testTimeout=15000 src/runtime-server.test.ts -t 'provider\|compaction\|model\|egress'`（实际 shell 参数见下） | 1文件；40通过，131不匹配筛选跳过 | runtime-lifecycle.log |
| `pnpm check:identifiers` | 通过（交付文档写入后复查） | identifiers.log |

实际 runtime 命令：

```sh
pnpm --filter @matou/runtime exec vitest run --testTimeout=15000 \
  src/session/agent-model-settings.test.ts \
  src/session/claude-model-routing.test.ts \
  src/session/claude-request-gateway.test.ts \
  src/session/native-model-catalog.test.ts \
  src/provider-config/provider-models.test.ts \
  src/provider-config/provider-config-store.test.ts \
  src/provider-config/subscription-provider-store.test.ts \
  src/model-router/claude-launch.test.ts \
  src/model-router/subscription-accounts.test.ts \
  src/model-router/subscription-spawn.test.ts \
  src/model-router/subscription-rpc.test.ts \
  src/egress/egress-monitor.test.ts
pnpm --filter @matou/runtime exec vitest run --testTimeout=15000 \
  src/runtime-server.test.ts -t 'provider|compaction|model|egress'
```

合计 **192通过、17项集成前置跳过、131项生命周期筛选排除；无测试失败**。17跳过来自 `subscription-accounts.test.ts:184` 的 `MATOU_TEST_MODEL_ROUTER=1` 门槛；未启用真实 pinned engine 的账号采纳模拟集成。与普通基线失败不同，不能记为通过。未跑全量测试/整App构建/真实UI/付费请求/真实账号登录；未运行 protocol-regression binary 集成。SQLite实验提示是环境提示，无故障日志。命令查询个别无匹配退出码不等于测试失败。

## 我自己做的决定

1. 不委派子代理；本关用户限定其余源码/测试由执行者完成，也无UI操作需求。
2. 选择本地定点验证为主，build:packages只作workspace package导出前置；补跑runtime生命周期的相关测试，未跑全量App构建。
3. 不启用 MATOU_TEST_MODEL_ROUTER，不使用真实账号或下载/构建router二进制；对固定commit下载少量原始源码，明示源码证据与二进制实测区别。
4. 最小方案建议采用“新进程”快照边界、同一preferences行专用patch、精确订阅raw model和已有gateway；不新增存储、不改共享角色alias、不扩大多账号范围。均为待正式方案采纳的建议，本关未实施。

## 仍需正式方案/后续fixture闭合

- GPT6通用分词计数精度没有实测；这是既有固定router限制，不应宣称精确，也不在此关升级依赖。
- 多官方family冻结/解冻需要必要接口调整；沿用既有冻结语义，不能省为“只保护默认供应商”。
- 未知context能力时CLI的有效窗口表现，以及default/Fable实际selector，需以隔离固定CLI fixture验证，未臆定额外能力。
- 真实账号删除/重新登录可能使原会话凭据失效；快照保证不主动换目标，不保证撤销的凭据继续可用。
- 没有发现必须改动已确认UI或自动供应商回落的产品冲突；未越过调研关卡。
