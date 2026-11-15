# Antigravity 请求转换对照（2026-11-15）

## 基线与判定范围

- sub2api：只读 `/tmp/router-research/repos/sub2api`，提交 `530c5e3a4f9c73ca2b0881191d0ef8e0d82c5484`。以下 `S` 均指 `backend/internal/pkg/antigravity/request_transformer.go`。
- Matou：CLIProxyAPI **8.0.69**，固定提交 `f7ae8feeb3710319f524730849fbe32fde1751e4`，加本关开始时补丁 `6f9c285b0935111b4963e7b06fe3a727a93f4e79fb75ef5c6e027a1eb0864b3e`。
- `T`：已打补丁的 `internal/translator/antigravity/claude/antigravity_claude_request.go`（本地 `.cache/model-router/build/f7ae8feeb371-6f9c285b0935-arm64/`）。`E`：`internal/runtime/executor/antigravity_executor_request.go`。`A`：`internal/thinking/provider/antigravity/apply.go`。`R`：`internal/runtime/executor/antigravity_reasoning_replay.go`。E/A/R 未被现有补丁修改，行号也适用于 pinned 原始源码。
- pinned 原始源码在 `.cache/model-router/source/f7ae8feeb3710319f524730849fbe32fde1751e4/`；T 的 system 处理已由上一关替换，后续 T 行号与原版有偏移。保留上述基线，避免用新补丁行号冒充原始行号。
- 判定跟到最终 HTTP 请求，而不止比较翻译函数。只有已有失败证据或能证明工具调用失败，且满足行为/布局边界，才列为真缺口。

## 对照矩阵

| 项目 | sub2api（函数、行号） | pinned + 补丁（文件、行号） | 差异及结论 |
|---|---|---|---|
| 1. systemInstruction 结构 | S `buildSystemInstruction` 343–419：字符串/数组转 `role:user,parts`；默认加长 identity、隔离说明和结束标记 | T 325–346、884–894：逐 text part 清洗，仍用 `role:user,parts`，保留已有 interleaved hint | **已覆盖，策略不同**。结构等价；不引入 sub2api 的长身份/结束标记，会改变缓存前缀，且前关明确禁止。 |
| 2. max_tokens / budget_tokens | S `ensureMaxTokensGreaterThanBudget` 63–80、`buildGenerationConfig` 699–750：当 max≤budget，将 max 提到 budget+1000，然后按模型封顶 | T 925–959 只映射；真正校正在 `antigravity_executor_execute.go:95–98` → `helps/model_capabilities.go:69–96` → A `normalizeClaudeBudgetDoc:162–195`：budget≥max 时降到 max−1，低于模型最小 budget 则移除 thinking；E 70–77 最终按模型输出上限封顶 | **已覆盖，策略不同，已裁决保留**。不能据翻译函数单点认定漏修。sub2api 增大总输出/成本，当前实现保留调用方 max。动态 −1 不用正预算规则。12个执行链边界场景通过；最终封顶顺序的合成反例单列待审计。 |
| 3a. tool_choice | S 155–164 固定 `VALIDATED`；没有读取 tool_choice 的转换分支 | T 863–923 映射 auto/none/any/指定工具并清洗 allowedFunctionNames；但 E 84–89 会把普通 Claude 请求的 mode 改成 `VALIDATED`（结构化输出路径例外），Gemini 保留转换 mode | **已覆盖，策略不同**。Matou 的 none 会删除 tools，指定工具保留允许名单；Claude 强制模式的端到端语义不能用翻译器单测宣称正确。本关不照搬 sub2api 的固定模式，亦不擅自改现有行为。 |
| 3b. 多轮/并行 functionCall 归集 | S `buildContents:428–475` 每条消息转 parts；`buildParts:548–598` 保存 id→name 并写 functionCall/Response | T 351–410、570–631、764–802、895–901：归集同轮调用，thinking/text/call 排序，对齐结果，Claude 分离结果回合后合并相邻 user | **已覆盖**，Matou 比参考多了并行结果顺序、相邻消息处理，不替换已有布局。现有 Go 测试见后文。 |
| 3c. 工具结果文字/错误/空值 | S `parseToolResultContent:607–648` 将数组拼文本；空结果补“成功/失败”句子，`buildParts:585–588` 一律写 response.result | T 650–727：保留字符串/对象/数组语义，is_error 写 response.error；缺失 content 写空字符串 | **已覆盖，策略不同**。不把空输出臆测成成功，不改成扁平文本，不引入新增可见文字。 |
| 3d. EnableMCPXML | S `hasMCPTools:275–282`、`buildSystemInstruction:402–405`，mcp__ 工具默认注入 XML 提示词（266–273） | T 813–855 转原生 functionDeclarations；570–631 转 functionCall；650–727 转 functionResponse；没有 XML 提示词注入 | **不适用**。是让模型改用 XML 文本的提示词 workaround，非工具结果协议；引入会改变系统前缀和工具调用行为，无现有原生 MCP 失败证据。 |
| 4. 稳定 session ID / reasoning replay | S `generateStableSessionID:25–41`：第一个 user 的首个文本 part 做 SHA256→前8字节→正63位→负号十进制，无文本则随机；176–179 优先原样 metadata.user_id | E `generateStableSessionIDFromContents:803–825` 算法相同；`geminiToAntigravityDoc:748–759` 显式 request.sessionId > derived > 内容哈希；R 123–188、242–285 将客户端会话、调用者范围用于 replay 隔离 | **已覆盖，部分等价**。哈希兜底完全等价；显式会话优先级/replay 隔离不同，不能原样覆盖。原始 user_id 不等于我们的 derived session。web_search/image_gen 有独立既定路径。 |
| 5a. dummy thought / 历史 assistant | S `buildContents:447–465` 仅对 Gemini 的最后 assistant、thinking 开启且缺少 thought 时插入 Thinking... dummy；历史不加；Claude 不允许 dummy | T 416–551、570–572：Claude 不插 dummy；Gemini 通过签名 carrier 绑定正确 part；184–218 在缺少工具签名时允许 Gemini sentinel。`internal/signature/gemini_sanitize.go:47–69,159+` 统一签名清洗 | **已覆盖，策略不同**。有兼容签名和原生 replay 路径，不增加可见 Thinking... 文本；Claude dummy 继续拒绝。 |
| 5b. 缺失/不兼容 thinking signature | S 523–535：Claude 降为普通文本；131–136 禁用 thinking；真实 signature 直接透传 | T 30–91、151–218、443–452：按 provider 校验、缓存恢复，无有效 Claude 签名则丢弃 thinking；R 164–188 隔离原生 replay | **已覆盖，策略不同**。不把内部 thinking 转成普通 assistant 文本，也不削弱现有 provider 校验。是否禁用本轮新 thinking 还受后续 ApplyRequestThinking 控制，不能只看 T 的 enableThoughtTranslate。 |
| 6. 计费行/身份清洗不回归 | S `NeutralizeSystemIdentity:298–340`：只清开头 attribution/身份，兼容 Codex opener；`filterOpenCodePrompt:285–295` 可能裁掉默认系统内容 | T 333、341 调 `system_prompt_sanitizer.go`（上一关）：每 part 计费行、三句精确身份、8句/1500字符窗口、代码豁免；Codex 沿用独立窄规则 | **已覆盖，策略不同**。保留前关确认的规则及原生请求位置，不引入参考项目的额外系统裁剪/Codex opener 规则。复跑前关测试及 payload 对照。 |
| 7a. 默认输出上限 / adaptive / temperature | S 678–687、699–764：默认64000、Claude封顶64000、其它65000，部分 Opus adaptive固定24576；reasoning 不带 sampling 参数 | T 933–959：adaptive effort/high 与调用方参数；A 50–84 按 registry 能力选 level/budget；E 70–89 对 Claude按registry封顶，非Claude移除 max | **已覆盖，策略不同**。不把参考项目的固定模型/预算策略引入现有路由。无 Matou 上游失败证据，调整会改变延迟、输出和费用。 |
| 7b. 原生搜索 / 内置与客户端工具混用 | S 93–105、874–905：只有搜索时切 flash；有函数工具时删除内置搜索/code_execution | T 319–320、823–825；`claude/web_search.go:28–114` 按能力/工具选择走现有搜索请求；混合函数场景不发送 googleSearch | **已覆盖，策略不同**。已避免混用，不改模型路由或搜索提示词。参考项目还支持 code_execution、新工具形态，不等于 Matou 的普通原生工具缺口。 |
| 7c. JSON Schema 清理与工具名 | S `buildTools:836–870` 用 CleanJSONSchema，支持嵌套 custom spec，默认空 schema | T 323、828–850 清洗工具名/去重/schema；E `sanitizeAntigravityRequestSchemasForModelDoc:167–175`、245–308 对 schema 子树清洗，Claude 参数 union 专门处理；不清洗工具历史 args | **已覆盖**标准 Claude input_schema。嵌套 custom 格式为参考项目的扩展输入，无本项目入口证据，不扩展。 |
| 7d. 工具结果中的图片/文档 | S `extractToolResultInlineParts:651–673`：提取 image/document base64，按目标模型支持放 response.parts 或旁路 part（590–597） | T 665–719：图片放 functionResponse.parts；非图片项保留 JSON；顶层 document 支持见746–751、973–989 | **已覆盖图片，文档行为不同待评估**。工具结果嵌套 document 未转 inlineData；未发现 Matou 实际输入/失败证据，且改旁路布局会触发先问边界。暂不实施。 |
| 7e. 空消息 / 缺少工具 input | S 494–497、510–513 跳空/特殊占位文本；548–569 即使 input 缺失仍构造调用 | T 554–558 跳空 text block；803–808 字符串分支保留空对象，Claude 最终仍有 `parts:[{}]`；576–604 缺 input 不生成调用，null归一为空对象 | **不适用/证据不足；空字符串待审计**。本地捕获已确认空 part 未清理；没有真实上游失败证据。缺 input 则是缺少 Claude 必填字段，不能按参考项目宽容行为认定有效请求缺口。 |

## 已确认的裁决

已覆盖但行为不同的项：1、2、3a/3c、4、5a/5b、6、7a/7b/7d。实施前已通过监督问答提交全部差异，答复选择 A：保留现有策略，只补验证中符合门槛的真实缺口；没有则只提交必要的执行链回归测试及证据。补充澄清 Claude 最终 VALIDATED、最终封顶顺序和空 part 后，第二次答复仍选择 A，明确这三项进入审计，本关不改。

本关**没有确认满足实施门槛的运行时修复项**，不等于所有请求都已证明不会失败。保留问题清单，避免把合成夹具或假上游响应当成真实服务方验收。

第2项的协议约束可参考 [Anthropic extended thinking 文档](https://platform.claude.com/docs/en/build-with-claude/extended-thinking)。该文档对 interleaved thinking 有例外；因此这只能证明需要检查协议与模型能力，不能证明 Matou 当前请求已经被 Antigravity 400 拒绝。参考项目注释也不能代替本项目真实上游失败。

## 对方更激进、我们刻意不做

1. **提高总输出预算**：S 将 max 提高到 budget+1000；我们保留调用方 max，复用已有 budget 归一化，不增加最大输出和潜在费用。
2. **新增/裁剪系统前缀**：S 默认长 identity、隔离段、结束标记、MCP XML，还会裁默认 system；我们保持上一关确认的逐 part 清洗，不改变系统前缀和工具调用方式。
3. **把历史 thought 变成普通文本、插入 Thinking...**：我们保留严格 provider 签名校验和 replay，不新增普通 assistant 文本。S 直接透传非空签名；不照搬更宽松的校验。
4. **空工具结果补“成功”句子、结果数组扁平化**：我们保持空输出、结构化结果与 error 字段，不编造结果。
5. **自动切换模型、固定 adaptive 预算和默认输出量**：不把 S 的搜索 fallback、固定预算或 sampling 策略搬进现有模型路由。

稳定 session 的哈希兜底已经等价；保留当前显式/派生会话优先级及 replay 隔离，属于已裁决的替代实现，并非漏实现。Claude 最终 mode 覆盖也属于既有策略，**不能描述成最终 HTTP 忠实保留 tool_choice**。

## 待审计（本关不修，均没有真实上游失败证据）

机器可读记录：[audit-probes.json](antigravity-sub2api-gap-2026-11-15/audit-probes.json)。原始本地探针日志：`/Users/icesword/.claude/agent-foreman/runs/Matou-sub2api对照-08/logs/go-audit-probe.log`。探针代码留在工作区 `.cache/sub2api-gap-router/internal/runtime/executor/antigravity_sub2api_audit_test.go`，未进入补丁；探针的 PASS 只表示成功捕获，不代表上游接受这些请求。

| 审计项 | 本地最终 HTTP 证据 | 潜在影响 / 复核边界 |
|---|---|---|
| Claude tool_choice 覆盖 | E 84–86。auto → VALIDATED；any → VALIDATED；指定工具 → VALIDATED+allowedFunctionNames；none → VALIDATED、tools 已删除。Gemini 保持 AUTO/NONE/ANY | Claude 强制调用/禁止调用的最终语义需复核；存在上游校验或行为风险，但没有真实工具失败证据。不能只看 T 907–923 判定正确，也不新增固定 VALIDATED 的逻辑。 |
| 输出封顶发生在 budget 归一化之后 | **合成模型能力夹具**：MaxCompletionTokens=16384，Thinking.Min=1024、Max=32768；输入 max_tokens=32768 / budget_tokens=24576；最终 `{"thinkingConfig":{"thinkingBudget":24576},"maxOutputTokens":16384}`。A 167–170 校正后，E 70–77 再封顶 | 代码级不一致：budget 可能超过最终 max。未验证真实动态模型会暴露这种能力组合，亦无真实 400。审计需结合目标模型、interleaved 例外和服务方实际规则，不能将合成结果称为线上故障。 |
| 空字符串 user 消息 | T 803–808；输入 `{"role":"user","content":""}`，最终 `[{"role":"user","parts":[{}]}]` | 空 data part 可能遭上游校验；没有真实上游失败证据，本关不改变消息布局。不能声称后续清理器已覆盖 Claude 空 contents part。 |

另有两个只有静态差异、尚未复现失败的输入兼容候选：嵌套 tool_result document 的媒体落点（7d）、嵌套 custom schema 扩展（7c）。均不扩大为本关功能。

## 本关改动与执行链验证

- 只向 `tooling/model-router/patches/antigravity-client-identity.patch` **追加一个 Go 测试文件** `internal/runtime/executor/antigravity_translation_boundary_test.go`，现有补丁字节前缀完全保留。
- 同步 `tooling/model-router/version.json` 的 patch.sha256：`ca7bc5a4f6a9fe77243138ecbc57ac36f4601da83a9bf5f820537990be1f9a9e`。
- 新旧实际构建树逐文件比对：除上述测试文件外，所有文件一致（排除构建输出二进制）。未修改运行时 Go 代码、Matou UI/IPC/账户、请求布局或缓存前缀。
- 没有复制 sub2api 源码、正则或测试；测试独立编写，没有新增 LGPL 派生代码，因此 `THIRD_PARTY_NOTICES.md` 保持不变。

两个新增 Go 测试，共 **14 个子场景**，均调用真实 `Execute` / `ExecuteStream`，经过输入校验、转换、thinking 处理、请求 schema/signature 处理、最终 HTTP 构造；仅传输被内存 RoundTripper 替换，使用假 token，不连外部网络。

| 测试名 | 场景 / 结果 |
|---|---|
| `TestAntigravityTranslationBoundaryBudget` | 模型能力夹具 Min=1024、Thinking.Max=32768、输出上限64000；6组输入×stream/non-stream共12场景。8192/4096→8192/4096；4096/4096→4096/4095；4096/8192→4096/4095；1024/8192→1024/移除thinking；4096/−1→4096/−1；128000/8192→64000/8192。每组都先断言翻译器原样映射，再检查最终请求，证明现有校正生效，不是本关新增修复。 |
| `TestAntigravityTranslationBoundaryParallelMCP` | stream/non-stream共2场景。两个同名原生 MCP 调用、逆序结果按id对齐；args中的title/format保留；空结果仍为空，失败结果在response.error；计费part丢弃、身份清洗、行内代码豁免；system仍为单part，没有MCP XML或结束标记。 |

[最终 MCP 请求样例](antigravity-sub2api-gap-2026-11-15/mcp-final-request.json) 是内存传输实际捕获的完整请求体。此轮没有运行时修复，因而不提供伪造的“修复前失败/修复后成功”；预算表是**同一现有代码**从翻译阶段到最终发送阶段的变化。

现有测试也随最终构建副本所在完整 Go 包复跑，包括：

- `TestAntigravityBuildRequestUsesDerivedSessionIDAndPreservesExplicit`；`TestAntigravityReasoningReplayScopeUsesStableSessionWithoutSessionId`；`TestAntigravityReasoningReplayScopeUsesClaudeMetadataSession`。
- `TestConvertClaudeRequestToAntigravity_ReorderParallelFunctionCalls`；`TestConvertClaudeRequestToAntigravity_AlignsPermutedParallelToolResultsWithMixedText`；`TestConvertClaudeRequestToAntigravity_ParallelToolResultsAcrossMessagesPreservesToolResultAdjacency`。
- `TestConvertClaudeRequestToAntigravity_ThinkingBlockWithoutSignature`；`TestConvertClaudeRequestToAntigravity_ToolUse_DoesNotReuseThinkingSignature`；各 provider 签名校验/恢复测试。
- 上一关 `TestAntigravitySanitizer*`、`TestSubscriptionPrompt*`、`TestNeutralizeClaudeCodeSystemIdentity*`。

## 统一自检

- `pnpm router:build`：通过，使用 pinned Go 1.27.4；补丁正确应用，生成 pinned 8.0.69 arm64 二进制。
- Go 六个完整包：`internal/runtime/executor`（61.059s）、`internal/translator/antigravity/claude`、`internal/translator/codex/claude`、`internal/translator`、`internal/util`、`internal/signature` 均通过；沿用仓库已有条件 skip。
- `pnpm typecheck`：通过。
- `pnpm build`：通过。
- `CI=1 pnpm test`：首轮在工作区 `.cache/t` 下两项 Unix socket 夹具 ENOENT；缩为 `.t` 后 Antigravity 夹具通过，mt-cli socket 路径仍106字节而失败。原文为 `Error: ENOENT: no such file or directory, chmod .../control/runtime.sock`（`HostControlServer.start:222`）。监督明确答复 A，允许仅测试使用系统默认临时目录；最终 `env -u TMPDIR CI=1 pnpm test` **完整通过，退出码0**。沿用仓库既有 CI 重试/性能预算，不改产品/测试源码、门槛或全局配置。
- 最终全量共 2879 项 Vitest 测试通过、37 项按现有配置跳过，另有6项 Node 工程门禁通过。分组：domain 12、contracts 71、desktop 1102（跳过1）、desktop performance 3、runtime常规1531（跳过36）、runtime-server 151、journal-range-reader 9。`MATOU_MODEL_ROUTER_BINARY` 未设置，既有 pinned 路由器协议集成测试按条件跳过；本关实际转换与最终 HTTP 边界由上述 Go 执行链测试覆盖，不把跳过项算作通过。
- `pnpm check:identifiers` / `git diff --check`：通过。
- patch SHA 与 version.json、本地缓存和 package-resources 两份 source.json 一致；两份二进制与各自记录的 SHA 一致。最终二进制 SHA256：`5c332e015c3f9dbc1b41cc64e0e8eae393826e057e006d28ee91019465866f70`。
- 完整日志位于 `/Users/icesword/.claude/agent-foreman/runs/Matou-sub2api对照-08/logs/`：`go-boundary-initial.log`、`go-audit-probe.log`、`router-build.log`、`go-final-packages.log`、`typecheck.log`、`build.log`、`test-ci.log`（首次环境失败）、`socket-environment-retest.log`（最短工作区路径仍超长）、`test-ci-final.log`（获准系统临时目录后的完整重跑）。

## 我自己做的决定

1. 沿真实执行链比较，而不只看翻译器字段；这样识别出已有 budget 校正，避免重复修改。
2. 两个回归测试复用真实执行器，使用内存传输和显式模型能力夹具；既覆盖流/非流，又不消耗账号或请求真实上游。审计探针不作为成功验收测试进入补丁。
3. 保留原补丁完整字节，只追加自主编写测试；不新增依赖或 LGPL 派生代码。
4. 沿用上一关 CI=1 验收口径；先在工作区尝试短临时目录及 commonjs fixture 边界。macOS socket 路径仍超长后，先取得明确的测试临时目录范围例外再复跑，不修改无关产品代码。

所有保留策略及三项待审计的决定均已交监督裁决，不是我擅自决定略过修复。

## 尚未验证

没有真实 Antigravity 账号的线上请求、上游 400/429 成功率或缓存命中验证。没有 UI 改动，本关未做真实桌面验收。三项待审计保持未解决，不能因本地假上游返回200就宣称它们正确。不推送、不发布、不安装产物。
