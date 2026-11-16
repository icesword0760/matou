# 第一轮审计修复（2026-11-15）

范围：D1、D4、D6a；D3 实测调查后在现有拨号配置内修复。所有路由运行时代码和 Go 测试都位于既有 `tooling/model-router/patches/antigravity-client-identity.patch`，`version.json` 同步补丁 SHA256。未修改 Matou 界面、请求布局、缓存前缀字段或工具/schema 语义；不涉及线上发布。

## D1：计费行后面的项目指令不再丢失

同一组输入分别以字符串 system 和数组 text part 进入真实 Codex 转换入口。

```text
输入：
x-anthropic-billing-header: sample
Keep project rules.
Read CLAUDE.md.

修复前：整个 system part 消失，input 直接从 user 消息开始。
修复后：developer 的原位置保留 input_text：
Keep project rules.
Read CLAUDE.md.
```

只移除每个 part 开头连续的计费行；正文中后续出现的同名行保留，LF、CRLF、CR 均覆盖。只剩空白时不生成空 developer 消息；未出现计费前缀的非空正文逐字保留。未改通用 `StripClaudeCodeAttributionSystem`，避免扩散到其它提供方。

- 新增 `TestCodexBillingPrefixPreservesBodyAndLayout`：字符串/数组、三种换行；完整输出与“直接传入保留正文”的转换结果逐字相等，验证 part 顺序、developer 角色、空 instructions、user、tool/schema 均保持原布局。数组同时有第二个 text part 和 cache_control 输入。
- 新增 `TestStripLeadingClaudeCodeBillingLines`：连续计费行、空余正文、后续同名行、代码内计费文本。
- 既有 `TestSubscriptionPromptBillingOnlySystem`、`TestSubscriptionPromptTranslations`：计费空块丢弃、工具与用户内容不改、输入不变、相同输入输出稳定。
- 可读实测：[`before/prompts.json`](before/prompts.json) 与 [`after/prompts.json`](after/prompts.json) 中 `finding=D1` 的两种 shape。已亲自读回完整保留正文。

## D4：身份句只在正文替换，代码照原样保留

```text
正文：You are Claude Code, Anthropic's official CLI for Claude.
行内：`You are Claude Code, Anthropic's official CLI for Claude.`
```

修复前 Codex 把正文、行内和 fenced code 中的身份句全部替换。修复后正文变成 `You are a coding assistant.`，代码里的完整身份句逐字保留。

现有 Antigravity 的 backtick mask 与 prose 映射原样移到 `internal/util/claude_system_identity.go`，Antigravity 与 Codex 共同调用 `SystemTextCodeMask` / `MapSystemTextProse`。未增加第二套保护器。仍仅匹配原先三句完整、区分大小写的 Claude Code 身份句；没有扩大开场识别范围。未闭合 fence 保护到末尾，未闭合 inline 保护到行尾，沿用既有行为。

- 新增 `TestCodexSystemIdentityProtectsCode`：三句身份 × 单/双 backtick、fenced、未闭合 fence/inline × 字符串/数组；代码外句子替换，代码内不变。
- 既有 `TestNeutralizeClaudeCodeSystemIdentity` / `TestNeutralizeClaudeCodeSystemIdentityPreservesOtherText` 及 Antigravity 代码保护用例继续通过。
- 可读实测：前后 `prompts.json` 中 `finding=D4`；两个路由中的代码保持相同输入文本。

## D6a：中文品牌可以命中自然中文句子

| 输入 | 修复前 | 修复后 |
|---|---|---|
| 由腾讯开发 | 由腾讯开发 | 由Google开发 |
| 小米手机、华为技术与字节跳动 | 原样保留 | Google手机、Google技术与Google |
| 行内代码 `腾讯` | 原样保留 | 原样保留 |
| fenced code 中的“由百度开发” | 原样保留 | 原样保留 |
| OpenAIClient | 原样保留 | 原样保留 |
| 第九句或超出1500字符的中文品牌 | 原样保留 | 原样保留 |

仅既有中文名单采用子串匹配；英文仍为整词、大小写不敏感。窗口仍是每个 system part 独立的前8句/1500个 Unicode 字符，两者先到为止，代码占用窗口但不替换；跨窗口边缘的品牌不替换。窗口仍在计费删除和完整身份替换之后计算。Codex 不套用 Antigravity 的品牌替换。

- 新增 `TestAntigravitySanitizerChineseSubstrings`：七个中文品牌嵌入自然句、inline/fence、未闭合 fence、第8/9句、1500字符内/跨界/外、代码消耗窗口、英文边界。
- 新增 `TestAntigravityChineseWindowPerSystemPart`：第一个 part 窗口外保留，第二个 part 重新计窗并命中，user 内容不变。
- 更新既有 `TestAntigravitySanitizerBrandsAndWholeWords`：中文连写从不命中断言中移出，英文边界断言保留。
- 可读实测：前后 `prompts.json` 中 `finding=D6a`；已亲自读回中文命中与代码保留结果。

## D3：本地 HTTP CONNECT 内目标站 ClientHello 的真实抓取

结论：**确认绕过，已在现有传输配置内修复**，没有新增依赖、接口或传输层结构。

方法：真实 `codextls.RoundTripper` 发起 HTTPS 请求；本地 loopback HTTP 代理接收 `CONNECT audit-target.test:443`，回复200后读取隧道内首个 TLS record，保留完整十六进制和解析出的 cipher / extension 顺序。全部流量终止在本机，不用真实账号或访问目标网站。直连对照使用 loopback IP，因此省略 SNI；代理目标为域名，因此包含 SNI(0)，此差异是预期的。

| 路径 | cipher 数量 | 扩展编号（按线序） | 结论 |
|---|---:|---|---|
| 修复前直连 | 22 | 10,11,13,5,18,23 | 自定义原生 TLS1.2 指纹生效 |
| 修复前 HTTP CONNECT 后 | 13 | 0,11,65281,23,18,5,10,13,50,43,51 | Go 默认 TLS，出现 supported_versions(43)、key_share(51) |
| 修复后直连 | 22 | 10,11,13,5,18,23 | 直连指纹保持 |
| 修复后 HTTP CONNECT 后 | 22 | 0,10,11,13,5,18,23 | 恢复自定义指纹，无 ALPN(16)、43、51 |

修复后 cipher 完整序列：`00ff,c02c,c02b,c024,c023,c00a,c009,c008,c030,c02f,c028,c027,c014,c013,c012,009d,009c,003d,003c,0035,002f,000a`，与直连相同。

原因：既有 `proxyutil.BuildDialer` 已负责 CONNECT，但 `http.Transport.Proxy` 又同时设置为 HTTP 代理。后者使 Go 的代理分支接管目标 TLS，不走 `DialTLSContext`。修复删除重复的 Proxy hook，继续使用原有 CONNECT 拨号器；补齐 `DialContext`，使普通 HTTP 也通过同一代理拨号路径。保持凭据池、空闲配置和 uTLS 握手超时；复用既有代理认证与取消处理。只阅读工作区内 pinned 上游实现的行为，没有复制参考仓库代码。

证据：[`before/tls.json`](before/tls.json)、[`after/tls.json`](after/tls.json) 包含原始 `record_hex`，可独立重解码。[`capture.go`](capture.go) 是可重跑的采集程序：复制到应用补丁后的 pinned Go module 的 `cmd/audit-capture/main.go`，在该 module 执行 `go run ./cmd/audit-capture <输出目录>`。前后使用同一份程序。已亲自读回 cipher、扩展顺序和原始 record 长度：代理路径1515字节变为209字节，直连均为183字节；随机数等会随每次握手变化，不要求原始字节全等。

新增 Go 测试：

- `TestCodexClientHelloThroughHTTPConnect`：真实匿名/Basic认证代理，比较完整 cipher 列表与扩展线序。
- 更新 `TestNewCodexAuthWithProxyURL_OverrideProxyTakesPrecedence`：改用两个 loopback 代理验证实际 CONNECT 到达账号覆盖代理，全局代理不被调用；不再依赖已删除的 `Transport.Proxy` 内部字段。
- `TestCodexHTTPConnectFailureDoesNotBypassProxy`：代理407时必须失败，不得直连绕过。
- `TestCodexHTTPConnectCancellation`：挂起 CONNECT 能被请求取消。
- `TestCodexHTTPConnectPlainHTTPAndPooling`：普通 HTTP 经 CONNECT、两次请求只用一个池化隧道。

抓取在 ClientHello 后主动断开，客户端 EOF 是预期行为，**不代表完成 TLS 握手、证书验证或线上推理成功**。本关没有真实服务方账号在线验证，也没有抓取 HTTPS 代理/SOCKS 两种链路；它们继续使用既有 proxyutil 拨号器，本次未改其实现。

## 已知限制（按裁决不修）

| 项 | 保留行为与理由 |
|---|---|
| D2（高） | Claude 最终 tool_choice 固定 VALIDATED，会覆盖 any。按已裁决的 ag-proxy / sub2api 同类策略保留，改回 ANY 可能触发上游400；本关不重新裁决或扩大工具语义。 |
| D5（中） | 只处理三句 Claude Code 完整身份，不增加 Codex/Cursor/OpenCode 等开场识别。限制匹配范围，避免误改项目正文。 |
| D6b（中） | 保留每个 system part 前8句/1500字符窗口；窗口外品牌原样保留，不做全量替换。 |
| D7（中） | 保持现有文本与 functionCall 排序、归集和回合布局；本关仅清洗 system 文本，不改变排序策略。 |

## 验证记录

- `pnpm router:build`：最终补丁通过，pinned Go 1.27.4，router 8.0.69；最终 `patch.sha256=77567c1712bd7e55bc34ef8554269df5d2527e15907410a27625df86fae370dc`。package-resources 的 `source.json` 已读回同一哈希。
- `pnpm typecheck`：通过。
- `env -u TMPDIR CI=1 pnpm test`：完整退出0，2879项 Vitest 通过、37项按既有配置跳过，另有6项 Node 工程门禁通过。沿用前两关测试临时目录例外，只有测试使用系统默认临时目录；非测试构建/采集使用工作区 `.cache/tmp`。未改重试、超时或性能门槛，本轮没有时间门槛失败。
- 37项跳过包括需要显式开关的 pinned model-router/protocol-regression 集成测试（`MATOU_MODEL_ROUTER_BINARY` 未设置），没有把它们计为通过。本关请求转换和网络行为由 Go 真实入口及 loopback 代理覆盖。
- Go 九个完整包最终通过：`internal/util`、`internal/translator`、`internal/translator/antigravity/claude`、`internal/translator/codex/claude`、`internal/codextls`、`internal/runtime/executor`（68.146s）、`internal/runtime/executor/helps`、`internal/api/handlers/management`、`internal/auth/codex`。executor 继续沿用既有条件 skip。
- 测试副本与最终构建副本的9个受影响 Go 文件逐字一致。与旧补丁构建树比较，只有这9个 Go 文件变化；`go.mod`、`go.sum` 不变，没有新增依赖。未重复执行已经通过且源码未变化的 Go 包。
- 初次 TLS 测试编译失败原文：`internal/codextls/codextls_test.go:9:2: "net" imported and not used`；已先报告，再删除未使用导入，整包通过。扩大回归时旧 auth 测试直接调用删除后的 `Transport.Proxy` 发生空指针；已改为真实代理优先级断言并整包复跑通过。两份失败原始日志保留。
- `pnpm build`：通过，runtime 与 Electron main/preload/renderer 构建完成。
- 提交前 `pnpm check:identifiers`、`git diff --check`：通过。

原始日志目录：`/Users/icesword/.claude/agent-foreman/runs/Matou-审计修复-09/logs/`。主要文件：`capture-before.log`、`capture-after.log`、`go-targeted.log`（首轮编译失败及4包成功）、`go-tls-targeted.log`（TLS完整包成功）、`go-integration-packages.log`（3包成功及旧auth测试失败）、`go-auth-final.log`、`router-build-final-patch.log`、`typecheck.log`、`test-ci.log`、`build.log`。

## 本轮自行决定

1. D1只处理每个 part 开头连续计费行，后续同名行不删；维持原 developer/parts 和缓存字段布局，不修改其它提供方的通用归因清洗。
2. D4原样提取现有 backtick 保护逻辑，两路共用，避免引入新 Markdown 解析策略。
3. D3用现有 CONNECT 拨号器收敛代理入口，并为普通 HTTP 配置同一拨号器；不新增依赖或改变传输层结构。旧 auth 测试改验实际代理选择，保留原产品要求。
4. 验收沿用既有 CI=1 与仅测试的系统临时目录口径，未改任何时间门槛或跳过失败项。
5. 使用任务开始时的 `feat/login-browser-integration` 分支提交指定信息，不切换分支，不推送、不发布。

## 未验证

未操作桌面界面（本关无UI变更）；未使用真实账号完成线上TLS握手或推理；HTTPS代理及SOCKS链路未单独抓包。上述内容不以构建/单元测试通过代替。
