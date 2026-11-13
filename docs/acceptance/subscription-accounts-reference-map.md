# 订阅账号纯函数：参考函数与验收对照

本次范围为启动配置、只读解析、异常映射三个纯函数模块，以及固定二进制的本机协议回归。未接 manager、进程管理、OAuth、存储、RPC 或 UI；未使用真实账号、增加依赖、提交或发布。

参考基线：CC Switch v4.0.31，提交 `3c76d3bda670297dd8c27d7d9be9b18a4c9143f5`，MIT。下表源路径均相对其 `src-tauri/src/`。完整许可见 [subscription-accounts-reference-LICENSE](../../packages/contracts/third-party/subscription-accounts-reference-LICENSE)。所有新增 TS 文件头均有源路径、提交、MIT 和可解析的许可相对路径。

## 函数对照

测试名为 Vitest 的实际测试标题（参数化标题的 `%s` / `%j` 在执行时展开）。

| CC Switch 函数 / 源文件 | Matou 函数 | 测试名 |
| --- | --- | --- |
| `live/project/claude.rs:proxy_projection/proxy_model_fields` | `claude-launch.ts:buildClaudeLaunch` | builds isolated aliases, real names/descriptions and secret-free temporary settings |
| `live/project/claude.rs:terminal_model_picker/TierModel::picker_row`；`proxy/model_mapper.rs:map_model` 的 Fable→Opus 语义 | `claude-launch.ts:buildClaudeLaunch` | builds isolated aliases, real names/descriptions and secret-free temporary settings；真实菜单、档位及字面 Fable 回落见方案 §16 和 `output/model-router-stage4-rework/model-menu.json` |
| `live/project/claude.rs:route_roles` | `claude-launch.ts:resolveRoleModels` | uses role-specific fallback order and reports missing models |
| `live/project/claude.rs:point_release_client_id` | `claude-launch.ts:pointReleaseClientId` | normalizes the exact Opus 5.5 boundary: %s; does not misidentify another version/family: %s |
| `live/project/claude.rs:client_model_for_subagent/client_alias_for_upstream` | `claude-launch.ts:clientModelForSubagent/toClientModelId` | normalizes Sonnet and explicit subagents, retaining the 1M marker |
| `live/project/claude.rs:proxy_model_fields`；第四关返工裁决的三档最小窗口 | `claude-launch.ts:buildClaudeLaunch/formatContextWindow` | takes all three upstream windows even when the main role is %s; rejects an under-100K %s tier even when it is not selected; does not guess a safe minimum when one tier has no window metadata; never raises a small upstream window to the client minimum, and caps larger windows conservatively; does not invent unknown context or output limits |
| `live/project/claude.rs:proxy_projection`；批准方案 §5.3 的最终 env 清理约束 | `claude-launch.ts:inheritedEnvToClear/buildClaudeLaunch` | clears old credentials, cloud selectors, force/subagent, output/compact and all inherited default variables; builds isolated aliases, real names/descriptions and secret-free temporary settings |
| `services/subscription.rs:codex_plan_label/title_case_words` | `account-readers.ts:parseChatGptPlan` | uses reference plan labels without guessing a known subscription: %s |
| `services/subscription.rs:query_codex_usage/window_seconds_to_tier_name/unix_ts_to_iso` | `account-readers.ts:parseChatGptUsage/readChatGptUsage` | identifies swapped primary/secondary windows by seconds and suppresses a zero-usage reset; keeps fractional nonzero usage reset and unknown windows distinct |
| `services/codex_oauth_models.rs:parse_models/push_model_entry` | `account-readers.ts:parseChatGptModels/readChatGptModels` | sorts and deduplicates supported model response shapes: %j; keeps advertised metadata and drops secrets and invalid token limits |
| `services/agy_account/quota.rs:parse_project/code_assist_plan/with_ultra_variant` | `account-readers.ts:parseAntigravityAccount/readAntigravityAccount` | prefers paid tier and distinguishes Ultra variants: %s; matches reference free/current-tier fallback and project shapes |
| `services/agy_account/quota.rs:parse_ineligible_reason` | `account-readers.ts:parseAntigravityEligibility` | detects region-locked paid accounts but honors free-tier and enterprise eligibility |
| `services/agy_account/quota.rs:parse`；`services/subscription_agy.rs:collect_tiers/tier_name/sort_order` | `account-readers.ts:parseAntigravityQuota/readAntigravityQuota` | parses the reference SAMPLE in model-group/window order; skips missing/unknown buckets and clamps numeric ratios like collect_tiers |
| `services/agy_account/quota.rs:fetch_with/error_message` 的 HTTP 失败区分；批准方案 §3.3 的 api-call 外层形态 | `account-readers.ts:unwrapApiCallResponse`，四个 `read*` 入口 | never treats inner 403 as success: %s; handles non-JSON, malformed envelopes, multibyte oversized and cyclic inputs without throwing |
| `services/agy_account/oauth.rs:fetch_identity/identity`；批准方案的凭据白名单 | `account-readers.ts:parseCredentialSummary` | copies only identity/plan/status/renewal fields and sanitizes refresh errors; whitelists the fixed v8 structured refresh_error without nested secrets; extracts only the two non-secret v8 id_token summary claims |
| `proxy/error_mapper.rs:map_proxy_error_to_status/get_error_message`；PRD §4 的状态与原文优先 | `errors.ts:mapRouterFailure/mapAccountStatus/sanitizeProviderReason` | maps startup, runtime exit, callback conflict and incomplete sign-in to exact bilingual copy; does not infer account rejection from a lone quota/model/inference 403 or an unrefreshed 401; attaches sanitized provider reason and official recovery for confirmed account ineligibility; marks exhausted quota 100% while preserving the card status copy |
| `proxy/codex_quota_protection.rs:Snapshot::from_quota` | `errors.ts:quotaSnapshotFromQuota` | reads supported quota windows, rejects invalid usage and ignores zero-usage placeholder resets |
| `proxy/codex_quota_protection.rs:Snapshot::from_headers` | `errors.ts:quotaSnapshotFromHeaders` | uses header window durations rather than primary/secondary order, including active-limit prefix; prefers absolute reset and does not invent windows from invalid header values |
| `proxy/providers/codex_responses_arguments.rs:dedupe_duplicate_argument_keys`；`proxy/forwarder.rs` 的官方 OpenAI 例外 | `protocol-regression.test.ts:request('/v1/responses', ...)` | preserves duplicate raw argument keys for the official OpenAI exception |
| `proxy/providers/codex.rs:strip_codex_openai_only_request_fields`；`proxy/forwarder.rs:should_strip_codex_openai_only_fields` 的供应商范围 | `protocol-regression.test.ts:request('/v1/responses', ...)` | normalizes Codex transport fields while retaining official-only metadata |
| `proxy/providers/transform_codex_responses_xai_sanitize.rs:rewrite_xai_call_id_less_tool_outputs` 的语义保留场景 | `protocol-regression.test.ts:request('/v1/responses', ...)` | preserves orphan tool output as user text without inventing a call_id |
| `proxy/providers/transform_responses.rs/streaming_responses.rs`；`transform_codex_anthropic.rs/streaming_codex_anthropic.rs` 的反向参考场景 | `protocol-regression.test.ts:responseEvents` + 固定引擎 `/v1/messages` | replays paired tools/thinking and keeps progress + final text, stream=%s; maps streaming termination %s to %s |

源码：[claude-launch.ts](../../apps/runtime/src/model-router/claude-launch.ts)、[account-readers.ts](../../apps/runtime/src/model-router/account-readers.ts)、[errors.ts](../../apps/runtime/src/model-router/errors.ts)。单元测试分别为同目录同名 `.test.ts`；集成测试为 [protocol-regression.test.ts](../../apps/runtime/src/model-router/protocol-regression.test.ts)。

## 调用约定与自行决定

1. `buildClaudeLaunch` 返回 `{ok,value}` 或结构化失败。调用方先按 `clearEnv` 删除继承值，再注入 `env`；须传入 `inheritedEnvNames: Object.keys(inheritedEnv)` 才能包含未来未知名字的全部 `ANTHROPIC_DEFAULT_*`。`settings` 只合并到会话临时文件，路由 key 仅在进程 env。`oauth['model-alias']` 由 manager 合并配置；`mainModel` 给调用方选择主模型，不额外写 `ANTHROPIC_MODEL` 或强制子代理变量。
2. 第四关返工后始终计算 Opus、Sonnet、Haiku 三档的最小窗口，不再根据 mainRole 排除 Haiku；任一档小于 100K 即拒绝，任一档未知则不猜测。客户端 2.1.323 的设置说明明确 `autoCompactWindow` 为 100000..1000000：小于 100K 返回 `unsupported-context-window`，绝不抬高；大窗口保守压到 1M；参与计算的任一窗口未知则不写两个窗口字段，返回 `unknown: ['context-window', ...]`。输出未知不写；已知按主档位、最多 128000。客户端不热更新阈值，因此统一在启动时取三档最小值；这个策略已实测覆盖 Haiku 切换，不再依赖热更新。
3. `readChatGptUsage/readChatGptModels/readAntigravityAccount/readAntigravityQuota` 只接受 `{status_code,header,body:string}`，先检查内层状态；`parse*` 为对已解包 JSON 的独立纯解析器。按参考 `quota.rs:MAX_BODY` 限为 256 KiB（UTF-8）；空、非法、循环和未知结构返回失败。未来 ChatGPT 套餐仅按参考规则原词首字母大写，绝不解释成 Plus/Pro；`unknown` 无套餐名。没有自动静态模型目录回落。
4. `parseCredentialSummary` 仅投影白名单。固定 v8 的 `id_token` 若是已解析的对象，仅取 `chatgpt_account_id/plan_type`；字符串 JWT 不解码、不透传。结构化 `refresh_error` 仅取 `code/message/http_status`，消息脱敏。固定路由 `auth_files.go/types.go` 的 `retry_scheduled` 是可重试状态，不等于必须重登录；终态 `sign_in_required/expired_sign_in_required`、明确凭据拒绝才输出重登录。单次额度 403 不改变账号状态。
5. 协议场景同时对照参考实现的适用范围和固定路由源码。官方重复参数保留字节；Codex translator 将 `include` 规范化为 `reasoning.encrypted_content`、移除 `stream_options`，保留官方专用 metadata/access_programs；孤立结果转为用户文本保留内容。后两项初始断言过度假设透传，实跑后查证固定源码再修正；没有更换二进制或新增转换器。未将 xAI/第三方网关清洗范围扩大到订阅产品。

## 自动与实际验证

日志在本 worktree 的 `docs/acceptance/logs/`（日志为本机交付文件，仓库默认忽略 `.log`）。未执行真实 UI 或真实账号验收。

- 安装：`pnpm install --frozen-lockfile`，成功；工作区独立 node_modules，无主仓软链、无新增依赖。
- 工作区依赖构建：`pnpm build:packages`，成功，仅准备本 worktree 类型产物。
- 新模块与固定二进制回归：最终数量和日志见下方最终记录。
- 无二进制：明确输出 `SKIPPED: MATOU_MODEL_ROUTER_BINARY is unset; pinned CLIProxyAPI 8.0.69 integration was NOT run.`，7 项跳过；[日志](logs/subscription-protocol-skip.log)。跳过不算协议验收成功。
- runtime 全量首轮默认并发：118 文件通过、5 文件失败、1 文件跳过；1251 项通过、7 项失败、1 项跳过。失败为既有 journal/fork/storage 超时或性能门槛，详见 [首轮日志](logs/subscription-runtime-test.log)。
- 限制并发并将系统临时目录放入深层 worktree 后：119 文件通过、4 文件失败、1 文件跳过。失败新增原因为 macOS Unix socket 路径长度和临时脚本继承根目录的 ESM 类型；详见 [第二轮日志](logs/subscription-runtime-test-bounded.log)。这些是本轮测试临时目录选择引入的环境问题，不作为产品失败结论；未修改无关产品代码或既有测试。全量 runtime 未全绿，后两组串联测试被前组失败阻断，不冒称已执行通过。

复跑新模块的命令（在 worktree 根目录）：

```bash
MATOU_MODEL_ROUTER_BINARY=/absolute/path/to/pinned/cli-proxy-api \
  pnpm --filter @matou/runtime exec vitest run src/model-router --reporter=verbose
pnpm --filter @matou/runtime typecheck
pnpm check:identifiers
```

协议测试启动时核对日志中的 `8.0.69 / f7ae8fee`；路径已提供但不存在、版本不匹配或未就绪时失败，不跳过。二进制使用本机 API-key 假凭据，覆盖与 E8 同样的 Codex executor/translator；没有 OAuth 或真实推理。只清理自身进程、服务器与 worktree 内测试目录。

## 尚未验证

真实账号、套餐/模型权限与真实服务方响应、真实工具执行、Claude Code `/model` 显示、运行中更小 Haiku 的压缩窗口热更新、manager 对接及产品 UI/打包均不属于本关。纯函数源码没有进程、网络或存储 API，也没有新增运行时 LLM 请求、延迟或模型费用。

## 最终记录（2026-11-13）

| 项目 | 结果与日志 |
| --- | --- |
| 新增测试 | 4 文件、93 项全部通过：86 项纯函数 + 7 项固定二进制协议回归；[完整逐项日志](logs/subscription-tests-final.log) |
| 协议实跑 | 固定 8.0.69 / f7ae8fee；非流式/流式进度与思考、工具配对、重复键、专有字段、孤立结果、end_turn/tool_use/max_tokens 均通过 |
| 缺少二进制 | 7 项明确跳过，stderr 有完整原因；[日志](logs/subscription-protocol-skip.log) |
| runtime 类型检查 | `pnpm --filter @matou/runtime typecheck` 退出 0；[日志](logs/subscription-typecheck.log) |
| 命名卫生 | `pnpm check:identifiers` 退出 0；[日志](logs/subscription-identifiers.log) |
| 来源与许可 | 7 个新增 TS 文件头的许可相对路径均实际解析存在；MIT 文件与固定参考源码 LICENSE 字节一致 |
| 纯函数边界 | 三个生产模块无 Node、fs、进程、网络或存储 API；输入样本不变性已测；未新增 npm 依赖 |
| 全量 runtime | 两次均未全绿，详见上文；不算通过。第二轮本任务生成的夹具移到 `output/subscription-runtime-tests-bounded/` 保留，避免成为新增源码 |
| 产品行为 / UI | 仅本机协议已实际验证；真实账号、客户端/UI 与集成行为未验证 |
| Git | 仅新增本关 7 个 TS 文件、本文及 MIT 许可；未提交、推送、改版本或更改锁文件 |

第四关返工的当前证据见 [方案第 16 节](subscription-accounts-plan.md)。早期的“仅纯函数、未接启动”描述仅指原关卡。当前增加 `modelPicker`、Fable 隐藏 Opus 回落，以及 `errors.ts:mapSubscriptionLaunchFailure` 的受控双语原因；原始引擎诊断仅在脱敏日志中。

## 当前补充：真实模型列表修复

历史小响应256KiB限制不再描述模型目录：当前模型解析4MiB，额度/账号资格仍256KiB；模型正文与管理包装分别有界。经 run-9 明确授权，账号模型读取失败时在 SubscriptionAccounts 使用路由 model-definitions 兜底，纯解析器不包含回落目录。Codex 显示名/窗口/输出以该路由定义为准。详见 [当前记录](subscription-accounts-models-rework.md)。
