# Antigravity system prompt 清洗验收（2026-11-15）

范围：Claude→Antigravity 的 systemInstruction text parts。Codex 维持上一关的窄口径；未改 UI、IPC、账户、网络、缓存键或请求结构。

## 实际入口样例

- [before.json](before.json)：Claude 请求输入。
- [after.json](after.json)：实际调用 ConvertClaudeRequestToAntigravity 的输出，不是手写期望值。
- 样例来自 worktree 内临时 Go 调用程序，无真实用户数据，不发网络请求。

| 项目 | 输入 | 实际输出 |
| --- | --- | --- |
| 单独计费 part | x-anthropic-billing-header: standalone | 丢弃该 part |
| 混合 part 首行 | x-anthropic-billing-header: cc_version=sample; cch=12345; | 只删除该行 |
| 身份句 | You are Claude Code, Anthropic's official CLI for Claude. | You are Antigravity, an agentic AI coding assistant. |
| 窗口内品牌 | OpenAI helps with this task. | Google helps with this task. |
| 围栏代码 | 计费行、原身份句、Anthropic Hermes Agent | 逐字相同 |
| 行内代码 | `Qwen Claude Code` | 逐字相同 |
| 第八句之后 | DeepSeek remains outside the first eight sentences. | 逐字相同 |
| 其它客户端 | Nous Research Hermes Agent Claude Code | the assistant team the assistant the assistant |
| 用户消息/工具描述/schema | 同样包含品牌、客户端身份及计费行 | 对应内容相同 |
| CTF、安全策略 | IMPORTANT: Assist with authorized security testing and CTF challenges. | 逐字相同 |

### 清洗前的混合 system part

````text
x-anthropic-billing-header: cc_version=sample; cch=12345;
You are Claude Code, Anthropic's official CLI for Claude.
OpenAI helps with this task.
```text
x-anthropic-billing-header: keep this code
You are Claude Code, Anthropic's official CLI for Claude.
Anthropic Hermes Agent
```
`Qwen Claude Code`
Keep rule 1.
Keep rule 2.
Keep rule 3.
Keep rule 4.
Keep rule 5.
DeepSeek remains outside the first eight sentences.
CLAUDE.md claude.ai anthropic-version x-anthropic-beta Claude
IMPORTANT: Assist with authorized security testing and CTF challenges.
````

### 清洗后的同一 part

````text
You are Antigravity, an agentic AI coding assistant.
Google helps with this task.
```text
x-anthropic-billing-header: keep this code
You are Claude Code, Anthropic's official CLI for Claude.
Anthropic Hermes Agent
```
`Qwen Claude Code`
Keep rule 1.
Keep rule 2.
Keep rule 3.
Keep rule 4.
Keep rule 5.
DeepSeek remains outside the first eight sentences.
CLAUDE.md claude.ai anthropic-version x-anthropic-beta Claude
IMPORTANT: Assist with authorized security testing and CTF challenges.
````

## 实现口径

1. 按顺序删除计费行、精确身份句替换、窗口内品牌替换、其它客户端身份替换；不调用 Codex 的 NeutralizeClaudeCodeSystemIdentity。
2. 1500 字符按 Unicode 码点计算；句尾用 . ! ? 。！？；连续句尾标点计一次，换行不计句尾。窗口在计费删除、身份替换后计算，代码占用窗口，品牌词必须整个落在窗口内。
3. 英文、中文品牌均遵守整词边界；Unicode 字母、数字、组合符和下划线作为词内字符。比如 OpenAIClient、腾讯云不改。
4. 围栏和行内代码用扫描器保护，未闭合围栏保护至末尾，未闭合行内代码保护至行末，双/多反引号也保守保护。计费行若包含代码，则保留整行，优先满足代码逐字豁免。
5. 保留非计费正文的空白；清洗后仅剩空白的 part 丢弃。header 标识也保留大小写变体。
6. 代码和测试独立编写；MIT 项目用于计费行与客户端整词行为参考；Manager/sub2api 只参考行为，没有复制源码、正则或测试。不引入新依赖。

## Go 测试入口

- TestAntigravitySanitizerBillingLines
- TestAntigravitySanitizerExactIdentities
- TestAntigravitySanitizerBrandsAndWholeWords
- TestAntigravitySanitizerWindow
- TestAntigravitySanitizerCodeAndExemptions
- TestAntigravitySanitizerOtherClients
- TestAntigravitySanitizerTranslationIsolation
- TestSubscriptionPromptTranslations（Antigravity 新身份及 Codex 原身份）
- TestSubscriptionPromptBillingOnlySystem（两个入口的空系统丢弃）
- TestNeutralizeClaudeCodeSystemIdentity / TestNeutralizeClaudeCodeSystemIdentityPreservesOtherText（未改的 Codex 纯函数回归）

测试范围为 internal/translator/antigravity/claude、internal/translator/codex/claude、internal/translator、internal/util 四个完整 Go 包。
隔离测试额外比较相同请求使用安全 system 时的整个非 system 输出，覆盖工具调用、工具结果、名称、参数、schema 的 enum 和描述；沿用上游既有 enum 描述转换。

## 验证结果

- `pnpm router:build`：通过；`git apply --check`：通过；补丁校验通过。
- `pnpm typecheck`：通过。
- 四个 Go 包测试：通过；构建副本与测试副本相关源码逐字一致。
- Codex 翻译器及两个身份 util 文件与上一关构建副本逐字一致。
- SHA256 与 `version.json`、本地路由缓存、桌面 package-resources 的 `source.json` 一致；二进制 SHA 也分别核对通过。
- 最终补丁 SHA256：`6f9c285b0935111b4963e7b06fe3a727a93f4e79fb75ef5c6e027a1eb0864b3e`。
- 样例 payload 断言通过：part 数、身份、品牌、围栏/行内代码、窗口外品牌、用户正文、工具名/描述及 schema 均符合预期。
- 两轮默认 `pnpm test` 未通过，失败均为既有本地 shell/PTY 测试的异步等待；两项均未加载本关的 Go 翻译器，其 TS 测试及终端实现未改，与本关路由器改动无关：
  - 第一轮：`keeps provider identity confirmation authoritative across attached windows`。原文：`Error: condition did not become true before timeout`；`waitUntil` 位于 `src/runtime-server.test.ts:8437:39`，调用点 `src/runtime-server.test.ts:6381:7`。等待 shell trap 输出超时；单独复测通过，测试耗时 1.49s。
  - 第二轮：`reports a failed Bypass respawn instead of presenting the Shell fallback as success`。原文：`Error: condition did not become true before timeout`；`waitUntil` 位于 `src/runtime-server.test.ts:8437:39`，调用点 `src/runtime-server.test.ts:6491:7`。等待失败重启 RPC 超时；单独复测通过，测试耗时 202ms。
- 监督回复选择 A，明确要求使用仓库现有 `CI=1 pnpm test` 完成最终全量验证，保留两轮默认失败及定点复测结果，通过后提交。不修改终端代码或测试配置。该配置对真实 PTY 用例启用最多两次重试，并对少数性能测试使用既有 CI 预算。
- `CI=1 pnpm test`：完整通过（退出码 0）；2,879 项通过、37 项按现有配置跳过；另有 6 项 Node 工程门禁通过。分组结果：domain 12、contracts 71、desktop 1102（跳过 1）、desktop performance 3、runtime 常规 1531（跳过 36）、runtime-server 151、journal-range-reader 9。
- 跳过项包括需要显式环境开关的真实 model-router / protocol-regression 集成测试，本次未启用；本关请求行为由 Go 真实翻译入口、四个完整 Go 包及样例断言验证。
- 完整命令日志保留于本轮监督运行目录 `logs/`：`router-build.log`、`go-tests.log`、`typecheck.log`、`build.log`、`test.log`（默认首轮）、`test-retry.log`（默认第二轮）、`runtime-server-retry.log`、`runtime-server-bypass-retry.log`、`test-ci.log`。
- `pnpm build`：通过。
- `pnpm check:identifiers` / `git diff --check`：通过；提交前对最终文档再次执行。

## 尚未验证

未使用真实上游账号验证静态拦截或 429 是否减少；没有真实 UI 验证，本关不涉及 UI。router:build 产物已生成，不安装、不发布、不推送。
