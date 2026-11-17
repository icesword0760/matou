# Task2 交付：请求路由与新进程集成

本批在已审 Task1 基础上接入 runtime / PTY / gateway；不提交、不安装、不发布，不改 UI 或 egress monitor 内部，不增加依赖。实际 API 和真实账户登录均未调用；HTTP 测试上游只在 loopback。

## 结果与用户行为

新进程准备阶段读取一次 preferences，以 ProviderConfigStore.captureClaudeRouting 单次存储读取捕获供应商目录与 API 连接信息。三档保持 `matou-role/opus|sonnet|haiku`；普通消息及 count_tokens 使用各自地址、模型和鉴权。同 origin 不同 key/path 也清除源认证、cookie。失效目标与非法虚拟档位报错，不改投其它目标。

保存 patch 不 activate。已运行进程和 attach 保留原 PID/快照，停止后恢复使用最新分配并保留 resume identity。自定义压缩仅优先于消息；计数跟实际请求档位。follow 压缩沿实际档位，不走旧默认目标。

目标 output cap 按每次请求限幅；effort 按目标该档设置并校验已知能力，默认/follow 清除继承的源 effort/thinking，未知能力不伪造 adaptive 支持。已知 context 窗口取共同最小值，并钳制512K请求；全未知返回null、保留 unknownContextRoles、清除源窗口声明，使用 CLI 原生默认；低于100K仍拒绝并标明档位和供应商。

订阅精确模型在冷启动发布 raw ID 时只补该账号 prefix 并等待注册，不重映射共享 aliases。租约在准备失败、close晚于await、进程退出、PTY构建失败时释放；关闭重复调用幂等。

官方 OAuth 稳定身份来自既有 `.claude.json.oauthAccount.accountUuid`（缺时使用既有emailAddress）及organizationUuid，读取凭据前后均检查身份。沿用既有官方凭据读取，允许同账号 token 刷新；不使用 token 哈希或伪造常量账号ID。测试全部使用注入/隔离假凭据，未读取真实钥匙串。缺身份配置明确失败，不要求用户重新登录。

## 默认语义的已裁决修正

监督者答复 #1 已明确：Task1 把总 provider.model 当三个默认档位的回填是错误的。未分配、没有该档映射的 API 角色必须用 native catalog 的该档 resolvedModel，仍发往原 API 供应商；总 model 仅保留启动主模型语义。显式 mapping/assignment 优先；没有 API 目录时能力 unknown，有明确目录且不支持原生档位时提示配置，不fallback。

已修正 Task1 parser 并补入正式计划说明。相关测试覆盖总custom无mapping、部分mapping、全显式分配、启动custom保持自定义、Fable/Haiku继承及不冒用官方能力。原 Task1 的字面default测试改为“显式assignment的字面模型”，不再把它误当无映射档位的默认行为。

## 实际文件清单（相对本worktree）

本批源码/测试共18个文件；Task1其它既有未提交改动原样保留。

| 文件 | 本批修改 |
| --- | --- |
| apps/runtime/src/runtime-server.ts | 新进程一次捕获preferences并准备targets；快照贯穿恢复模型/context/routing/digest；关闭失败释放 |
| apps/runtime/src/runtime-server.test.ts | 真runtime+PTY+本地HTTP：保存/attach保持PID和旧key，停止后resume捕获新目标和原provider-session identity |
| apps/runtime/src/session/pty-session.ts | 新角色env/menu/settings/Fable隐藏兼容route；跳过旧订阅投影；清除源预算与强制子代理model；退出/失败close |
| apps/runtime/src/session/claude-model-routing.ts | 使用冻结目标；custom/follow、精确鉴权、输出/effort、非档位custom源供应商语义；失败不回落 |
| apps/runtime/src/session/claude-request-gateway.ts | kind区分message/count；count保留地址/鉴权/模型；支持omit effort；发送前family callback |
| apps/runtime/src/session/claude-request-gateway.test.ts | 改正旧count只改model断言；同origin不同path/key HTTP录制 |
| apps/runtime/src/session/agent-model-settings.ts | launchWindow可接收本次捕获preferences，避免重新读库 |
| apps/runtime/src/session/claude-role-assignments.ts | 修正默认档位解析，导出defaultClaudeRoleModel；捕获供应商显示名用于明确错误 |
| apps/runtime/src/session/claude-role-assignments.test.ts | 对齐已裁决的显式default模型语义，保留Task1其它检查 |
| apps/runtime/src/model-router/subscription-accounts.ts | 冷raw模型prefix采用及就绪等待，严格校验不放宽 |
| apps/runtime/src/model-router/subscription-model-target.test.ts | 未activate过账号的prefix准备不写共享alias |
| apps/runtime/src/provider-config/provider-config-store.ts | runtime私有captureClaudeRouting，单次读取捕获API key与目录，不经RPC返回 |
| apps/runtime/src/session/claude-official-account.ts（新） | 复用原官方凭据读取，加稳定账号身份捕获和刷新检查 |
| apps/runtime/src/session/claude-official-account.test.ts（新） | 同账号刷新、换账号拒绝、无身份不伪造 |
| apps/runtime/src/session/claude-role-launch.ts（新） | 连接准备、租约托管、source连接、默认主模型身份、context、与activate共用digest |
| apps/runtime/src/session/claude-role-launch.test.ts（新） | 冻结、unknown、100K限制、失败/晚close、digest、订阅默认Opus、默认API语义 |
| apps/runtime/src/session/claude-role-routing.test.ts（新） | 真HTTP录制三档/计数/压缩/effort/非法role/Fable/保护callback |
| apps/runtime/src/session/claude-role-pty.test.ts（新） | 真PTY假CLI录制角色env、实际HTTP消息/计数、租约退出/构建失败 |

另更新正式计划的默认语义一条，新增本交付记录。未修改主仓库、原型、生产账户配置、依赖锁、UI或egress内部。

## 接口与Task3接入点

- `ProviderConfigStore.captureClaudeRouting()`：runtime-only `{providers,apiConnections}`，包含私有连接，绝不接到renderer RPC。
- `prepareClaudeRoleLaunch(...)`：返回 snapshot/contextWindow/unknownContextRoles/digest/sourceConnection/sourceEnvironment/resolveMainModel/close；无分配直接undefined，原路径保持。
- `claudeRoleLaunchDigest(...)`：新spawn与显式activate用同一配置摘要，忽略显示名称/额度，不为比较摘要启动router或读取OAuth token。
- `ClaudeModelRoutingOptions.roleSnapshot/sourceConnection/releaseTargets/isClosed`：本次运行目标及生命周期；`assertTargetAllowed(family?)` 作为Task3挂点。
- `GatewayOverride.family` 和 `GatewayOptions.assertTargetAllowed`：计数也透传实际官方family，HTTP发出前调用；保护拒绝时fixture上游请求数0。
- Fable literal overrides 用隐藏 `matou-role/opus-fallback` 路由回Opus，保持原代码中“Fable隐藏路由不污染可见Opus身份”的处理；用户可选三档仍只有原三档。
- **尚未接入多出口保护**：Task3需注册snapshot.officialFamilies并提供上述callback；本批不宣称混合官方目标已受完整监控保护。

## 红绿与实际请求证据

所有命令完整输出在 `/Users/icesword/.claude/agent-foreman/runs/matou-routing-recon-01/logs/`。

| 记录 | 实测结果 |
| --- | --- |
| task2-red-gateway-prefix.log | 实施前3失败：count目标/鉴权丢失、同origin count走source、冷prefix误拒 |
| task2-red-preparation.log / task2-red-preparation-assertions.log | 新模块尚不存在时先记录失败；加最小未实现接口后6项断言红灯，再实现 |
| task2-red-role-routing.log | 9项角色HTTP断言红灯 |
| task2-red-pty.log / task2-red-runtime.log | PTY角色环境与runtime主模型身份红灯 |
| task2-red-digest.log / task2-red-subscription-main.log | 摘要一致性及默认订阅Opus修复前红灯 |
| task2-red-late-close.log | 关闭晚于准备完成未阻止PTY创建的红灯；修复后租约释放通过 |
| task2-red-native-defaults.log | 已裁决默认语义修复前红灯 |
| task2-red-fable-hidden.log | Fable隐藏兼容路由修复前2项失败 |
| task2-runtime-final.log | **17文件通过、1文件跳过：190通过、17既有集成跳过** |
| task2-lifecycle-final.log | **runtime-server相关41通过、131筛选排除** |
| task2-build-packages.log / task2-types-final.log / task2-identifiers.log | 基础包构建、runtime类型、命名检查通过；git diff --check通过 |

合计 **231项Vitest通过**，不重复累加中间轮次；17项既有router账号采纳集成未启用，131项runtime-server用例是筛选排除。未跑全仓测试/全App构建/真实UI。

真实进程与请求证据：

1. `claude-role-routing.test.ts` 启动多个HTTP服务录下目标URL、body.model、独立鉴权和cookie清理；消息与count均录制，输出/effort只改消息。既有gateway流式、取消、HTTP错误和redirect测试一并通过。
2. `claude-role-pty.test.ts` 真实node-pty启动隔离假CLI，由其角色环境发出消息和count。`runtime-server.test.ts` 的Task2场景另验证旧PID/旧key保持、下一进程新key与 `--resume native-role-fixture`。仅记录fixture白名单数据，不dump继承环境或真实凭据。
3. `.cache/task2-evidence/native-selectors.json`：安装的**真实Claude Code 2.1.325**，隔离HOME/config、假API key、仅loopback上游。Opus/Sonnet/Haiku/Fable全部实际发出对应 `matou-role/opus|sonnet|haiku|opus`。日志 `task2-native-selectors.log`。
4. `.cache/task2-evidence/unmapped/native-selectors.json`：移除角色env后，真实CLI Haiku发出 `claude-haiku-5-5`，支持本批默认语义修正。日志 `task2-native-unmapped.log`。
5. 原生CLI子代理额外探针两次**未观察到子代理启动**：`task2-native-subagent.log`、`task2-native-subagent-retry.log`，隔离CLI结果spawned=0。这是额外fixture未闭合，不算通过；已停止继续扩展该探针。已有假CLI角色/Haiku选择和路由测试通过，但不替代原生Task工具的端到端子代理执行证明。
6. 本worktree无固定8.0.69 patched router二进制；未下载/构建、未跑真实订阅协议转换。17跳过不能声称已验证ChatGPT/GPT协议端到端。真实CLI fixture里的usage/cost由假响应计算，不是外部计费。

复跑主命令：

```sh
TMPDIR="$PWD/.cache/task1-tmp" pnpm --filter @matou/runtime exec vitest run --testTimeout=15000 \
 src/session/agent-model-settings.test.ts src/session/claude-role-assignments.test.ts \
 src/session/claude-role-launch.test.ts src/session/claude-official-account.test.ts \
 src/session/claude-role-routing.test.ts src/session/claude-role-pty.test.ts \
 src/session/claude-model-routing.test.ts src/session/claude-request-gateway.test.ts \
 src/model-router/subscription-model-target.test.ts src/model-router/subscription-accounts.test.ts \
 src/model-router/subscription-platform.test.ts src/model-router/subscription-spawn.test.ts \
 src/model-router/subscription-rpc.test.ts src/model-router/claude-launch.test.ts \
 src/provider-config/provider-config-store.test.ts src/provider-config/provider-models.test.ts \
 src/provider-config/subscription-provider-store.test.ts src/session/native-model-catalog.test.ts
TMPDIR="$PWD/.cache/task1-tmp" pnpm --filter @matou/runtime exec vitest run --testTimeout=15000 \
 src/runtime-server.test.ts -t 'provider|compaction|model|egress|Task2 role launch'
pnpm build:packages
pnpm --filter @matou/runtime typecheck
pnpm check:identifiers
git diff --check
```

## 中间失败说明

保留了全部阶段日志。两次runtime fixture调试失败来自测试误用MockPort.messages、RPC payload被旧领域辅助函数包装；修正为sent与直接payload后通过。最初PTY失败用例误把node-pty异步退出当构造reject，并导致清理与journal关闭竞争；改为等待whenClosed、另测关闭时构造拒绝后通过。早期类型检查的可选model/测试字段错误已修复。以上没有隐藏成首轮通过；最终通过结果见最终日志。

## 我自己做的决定

1. 用一个局部runtime私有存储快照方法捕获API目录和key，避免多个异步读取拼接配置；不新增存储或依赖。
2. 官方身份采用已有accountUuid/email+organization配置，前后检查身份，仅对同账号刷新放行；不主动发刷新/身份探测请求。
3. 新路径跳过旧subscriptionLaunch，统一在最终PTY边界写角色env/settings；保留源供应商连接供非档位自定义模型使用，启动custom不伪装成档位。
4. 冷订阅只在精确credential发布了raw model时补prefix，不为“通过”放宽目录条件，也不修改共享role alias。
5. Fable沿用原实现的独立隐藏fallback路由；可见角色ID保持计划规定的三档。
6. 默认API回填修正遵照监督者答复#1，不作为未经确认的产品选择；实测和修改记录如上。
7. 使用本地fixture为主要验证渠道；原生子代理额外探针连续未闭合后停止，明确留验收缺口，不继续钻探或冒称通过。未派实现代理。

## 未闭合与交回边界

runtime/PTY/gateway接入代码已交回，停在Task2供审查。原生Task子代理端到端探针、固定router订阅转换、真实账号/付费请求尚未验证；多出口监控保护明确留给Task3，UI留后续。不要把这批代码/fixture通过当作整个功能产品验收通过。
