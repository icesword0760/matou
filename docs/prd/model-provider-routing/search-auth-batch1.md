# 网页搜索专用目标与登录失效反馈：第一批后端交付

本关只修改契约和后端，停在审查点；没有界面改动、提交、合并、发布或安装。所有请求均为本地假上游，没有使用真实账号、钥匙串或付费模型调用。

## 用户结果

- 已配置搜索目标时，Claude Code 的独立 `web_search_*` 子请求和对应计数请求发送到该目标，携带当前请求的完整对话内容，改用目标模型和鉴权。
- 未配置、无工具、空工具、客户端工具、混合工具及 `web_fetch` 保持原路径。带手动/自动压缩标记时，压缩优先；计数沿用已有压缩语义。
- 搜索目标不可用、账号撤销或出口保护失败时明确失败，不回落其他供应商。
- 订阅/OAuth 401/403 返回“档位或网页搜索 · 供应商 登录已过期，请到 设置 → Claude Code 重新登录后重试。”，保留状态码，支持英文。明确重登错误也改写，其他错误保持原状态和原始内容；API Key 错误不改写。
- 运行中的托管订阅账号标记 `needs-sign-in` 并通知设置页，保存重新校验实时状态。新目标准备不能把失效状态自动抹成 ready，重新登录走已有流程。
- 搜索目标使用启动快照。保存不重启现有会话；新进程捕获新值。搜索模型的输出上限生效，8K 搜索模型不会降低会话窗口，也不触发 100K 门槛。

## 接口与文件

| 接口/位置 | 变更 |
| --- | --- |
| `packages/contracts/src/agent-model-settings.ts` | `ClaudeModelTarget`、`ClaudeAssignmentsPatch`；`AgentModelPreferences.claudeWebSearch?` |
| `AgentModelSettings.patchAssignments(input: ClaudeAssignmentsPatch)` | `{ assignments, webSearch?: target \| null }`；未传字段保留旧值，显式 null/undefined 清除；与分配一起校验并原子写同一 preferences 行 |
| `AgentModelSettings.set()` | 旧写入方不能覆盖 assignments 或 webSearch；异步校验后读最新值保留 |
| `apps/runtime/src/rpc/runtime-rpc-router.ts`、`index.ts` | 同一 RPC 入口；注入 `SubscriptionAccounts.snapshot()`，用于实时账号保存校验；原有只读门禁保留 |
| `ResolvedClaudeRoleSnapshot.webSearchTarget?` | 冻结连接、模型、metadata、账号身份和 guard；加入 `officialFamilies` |
| `claude-role-launch.ts` | 搜索目标预备与租约生命周期；仅搜索配置也启用快照；启动 digest 包含搜索配置 |
| `runtime-server.ts` | 激活 digest 和隐藏时区策略纳入搜索目标；没有增加保存时重启 |
| `GatewayOptions.resolveOverride({ compaction, webSearch?, model, kind })` | 网关解析 tools 后提供独立搜索判定；消息与计数统一选择目标 |
| `GatewayOverride.authentication?` | 运行时内部归属信息：`providerId`、`providerName`、可选 `accountIdentity`、`label`；不进入上游请求或客户端响应 |
| `GatewayOptions.onAuthenticationFailure(target)` | 注入回调；网关不直接依赖账号服务内部状态 |
| `SubscriptionAccounts.markNeedsSignIn(providerConfigId, accountIdentity?)` | 核对账号身份、更新既有状态、去重通知；不改凭据、不撤销已有进程；重登后的迟到响应不污染新身份 |
| `pty-session.ts` | 网关回调连接到账号服务，复用原有进程与关闭流程 |

其余修改为上述文件的现有测试和新增 `claude-web-search.test.ts`、`subscription-auth-failure.test.ts`。没有新增存储表、依赖、模型探测或公网请求。

## 证据与校验

证据目录：`evidence/search-auth-batch1/`。JSON 中所有 token、账号和对话均为合成测试数据。

| 场景 | 证据 | 实测 |
| --- | --- | --- |
| Haiku 独立搜索与计数 | `evidence.json` | 两次实际 HTTP 均进入 search 上游，最终 model=`search-model`；正文返回 `target: search` 与 `web_search_tool_result`；仅消息输出限幅到 512 |
| 旧快照未配置搜索 | `unconfigured.json` | 实际 HTTP 命中 Haiku，model=`gemini`；搜索上游请求数 0，返回 `target: haiku` |
| 搜索账号 401/403 | `auth-401.json`、`auth-403.json` | 实际 HTTP 命中搜索上游；状态码不变，返回安全重登文案；真实账号服务 snapshot=`needs-sign-in` |
| PTY 接线 | `pty-auth.json` | 真实本地 PTY 内的合成 CLI 发起搜索；命中 `matou-chatgpt/gpt`；终端收到中文 401 重登文案，账号服务状态同步失效 |
| 保存后快照 | `runtime-server-snapshot.log` | 原档位及仅搜索两种模式：保存后 PID/目标保留，下一进程读取新目标 |
| 红测试 | `red-baseline.log` | 新测试对 Git HEAD 原实现回放：20 失败、46 通过；证明缺口后恢复实现文件。不是第一次命令失败时“vitest 未安装”的环境日志 |
| 运行时定点回归 | `green-runtime-final.log` | 11 文件、159 测试通过 |
| 运行时主集成回归 | `runtime-server-final.log` | 176 测试通过 |
| contracts | `contracts-tests.log` | 9 文件、105 测试通过 |
| 必需工程检查 | `build-packages.log`、`runtime-typecheck.log`、`contracts-typecheck.log`、`identifiers.log`、`diff-check.log` | 全部通过 |

首轮主集成回归暴露了订阅/OAuth 接入现有网关后的旧固定参数/上游 URL 断言：已补充网关设置校验，保留恢复/分叉/权限参数和出口保护断言，定点 9 项通过。随后一轮出现原有时区测试超时，单独复验通过；最终整套结果以上表日志为准，保留日志如实说明。

## 我自己做的决定

1. RPC 使用字段是否存在区分旧客户端与清除：完全不传 `webSearch` 保留；显式 null 清除，运行时直接调用的显式 undefined 同样清除。这落实了交代中的旧客户端兼容要求。
2. 只有搜索配置时也启用既有角色快照，不另建路由/连接存储；搜索目标不加入窗口预算循环。激活摘要和隐藏时区策略沿用同一快照。
3. 没有档位分配的订阅/OAuth 新进程也接入现有网关，只增加鉴权归属观察，保持原目标、模型和鉴权；否则这些会话仍只能显示笼统错误。
4. 明确重登识别限定在失败响应中的结构化错误码/登录失效文案；最多检查 256KB，gzip/deflate/br 解压同样限额，成功 SSE 直接透传。超限或无法解码的普通错误保留原字节，避免扩大内存与响应范围。
5. 账号状态留在既有 `SubscriptionAccounts` 内存状态中，不写凭据。身份校验、去重通知、设置缓存覆盖和新目标准备共同防止失效状态被旧数据覆盖；重新登录复用已有清理流程。
6. 原生官方 OAuth 没有托管订阅记录：仍返回明确重登文案并调用回调，但不凭空创建订阅卡片/状态记录；托管订阅完整标记 `needs-sign-in`。
7. worktree 起初无依赖，按既有锁文件离线安装；没有添加依赖。测试临时目录和证据落在本 worktree 的缓存目录。

## 尚未验证与下一关

- 真实界面、行内“重新登录”按钮、中英文界面、真实账号重新登录和完整产品端到端留给第二批；本批未进行 UI 验收。
- 没有运行真实付费模型、真实公网搜索或现有外部路由二进制账号集成套件；采用本地 HTTP + 实际账号状态服务 + 实际 PTY 验证本关行为。
- 原生 OAuth 的账号状态边界见决定 6；没有新增持久化或凭据写入。
