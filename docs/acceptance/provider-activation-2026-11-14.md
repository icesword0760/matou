# 供应商切换超时修复 — 2026-11-14

用户截图及正在运行的应用均出现：ChatGPT 已标记“使用中”，但页面仍显示“切换失败：Matou 后台没有及时响应，请重试”。真实界面也显示会话已采用订阅模型；未重新切换用户的供应商。

## 原因与修改

- 默认供应商持久化后，RPC 仍串行等待所有符合条件的 Claude 会话重连；客户端普通请求预算为 10 秒，而订阅管理单次检查允许 15 秒。不能用请求超时推断默认配置未修改。
- activate 使用独立的 120 秒上限，普通读取仍为 10 秒。会话最多 4 个并发更新，保持返回顺序、逐会话锁、故障隔离与回滚逻辑。
- 非结构化请求失败后读取最新快照。确认默认配置已切换时展示信息提示，明确现有会话更新结果尚未确认，不伪造全部成功，不提供无效重试。明确的订阅预检错误仍保留原恢复动作。

## 自动验证

- RuntimeClient：42 项通过，包含超过普通请求时限的成功响应、最终超时和显式超时覆盖。
- ModelSwitchSettings：7 项通过，包含保存后响应失败、保存前失败、状态读取失败和结构化错误。
- SubscriptionSettings：44 项通过。
- Runtime provider activation 定点测试：10 项通过。5 个真实 PTY stub 会话验证最大 4 并发、槽位复用与稳定结果顺序，以及既有故障隔离场景。
- desktop/runtime 类型检查、完整构建、标识检查通过。首次类型检查遇到旧的 workspace 声明产物，重建依赖后已通过。
- 完整回归套件未执行。

## 实际界面验证

使用独立测试数据启动真实 Electron UI 和运行时，仅对最终 RPC 回执注入故障：

| 场景 | 结果 | 证据 |
| --- | --- | --- |
| 保存成功，回执延迟 11 秒 | 正常显示默认已切换，无失败提示 | evidence/provider-activation-2026-11-14/slow.png |
| 保存成功，回执丢失 | 显示“切换结果待确认”，说明默认已切换、会话结果未确认，无无效重试 | evidence/provider-activation-2026-11-14/lost.png |

两项 E2E 通过；已检查截图。测试未调用真实订阅模型，也未中断用户会话。本次为已有故障修复，未进行 reference product 全范围 PRD 对照或用户验收。

## 本机安装

- macOS Developer ID 签名成功，`codesign --verify --deep --strict` 通过；未公证、未发布。
- 打包应用以独立数据目录启动，供应商列表真实可见，冒烟检查通过（packaged.png）。首次冒烟仅因严格文字定位未匹配“使用中”合并标签失败，改用真实按钮可访问名称后通过。
- 修复版已安装到 `/Applications/码头.app`，安装后签名校验通过。
- 原应用保留在 `/Users/icesword/.matou/backups/provider-activation-20261114/running-original.app`；安装前副本也保留在同目录 `码头.app`。
- 没有关闭用户当前应用或终端会话。当前进程仍使用旧代码，重启码头后修复才生效。未在用户真实订阅账号上重新执行全局切换。
- 本次源码修改未提交、未推送。

## 后续真实会话压缩调查

2026-11-14 本机重启后用户切回 ChatGPT，出现长时间 Compacting 及完成后 HUD 仍 100%。本段替代“272K 是硬上限导致压缩无法完成”的初步推测。

- 真实 transcript 的 compact_boundary：开始约 12:41:53，完成 12:47:03（Asia/Shanghai）；durationMs=309699，preTokens=839260，postTokens=18007，trigger=manual。原会话已经压缩成功，CUA 观察到 Compacted 和恢复读取项目文件。
- 当时观察到一个约16MB、898条消息的在途压缩请求；完成后临时请求文件已删除。详细请求日志及调试日志未启用，不能追溯拆分上传、上游排队、推理、输出和客户端重试的耗时。路由器配置 request-retry=0；这不证明 Claude 客户端完全没有重试。
- 使用既有本地管理接口只读查询当前账号的 Codex models 接口（HTTP 200）：gpt-6-astra 返回 context_window=272000、max_context_window=872000、supports_experimental_context=true。272K 是接口默认值，不应宣称为该模型/通道的硬上限；此元数据也不等于已实测872K边界。
- OpenAI API 模型文档标注 Astra 1,050,000 tokens：https://developers.openai.com/api/docs/models/gpt-6-astra 。与上述订阅客户端元数据区分。
- Matou 的 parseChatGptModels 当前读取 context_window，启动配置使用该值配置 autoCompactWindow。未擅自提高窗口，未改变用户模型或推理强度。
- 用户明确拒绝用 compact_boundary.postTokens 估算 HUD 的兜底。相关源码和测试已全部撤回，未将估算数字写入实际 HUD。上一版将“未读取压缩记录”直接归为根因、建议采用 postTokens 的结论撤回。
- Claude Code 官方状态栏文档明确：current_usage 在 /compact 后为 null，直到下一次 API 请求重新填入；本机 2.1.325 的 XZe/ITt/C4r 逻辑与此一致。文档：https://code.claude.com/docs/en/statusline#context-window-fields 。
- 已在当前源码执行一次临时定点复现：向 SessionHudRegistry 依次输入 100%、官方格式的 explicit null、21% 新用量，得到 100% → 100% → 21%。1 项诊断测试通过，临时测试文件已删除。这证明 Matou 将“明确未知”当作“无需更新”，会保留过期值；不证明本次现场已经收到了 null。
- 真实 UI 执行只读 /context：显示 Messages 37.2K、Free space 182K（66.9%）、Autocompact buffer 33K、Auto-compact window 272K；同时 HUD 仍为 100%。这是 CLI 当前上下文统计，不是新的上游 usage，也不能用 postTokens=18K 直接代表恢复附件后的完整请求。
- 现场压缩后没有新的模型 assistant usage。观察期间 /context 未触发 statusline 脚本；未捕获压缩后原始上报，所以“上报 null 后码头保留旧值”和“没有新上报”在该现场仍未完全区分。不能据此声称 GPT 用量转换有错。
- 为诊断只对该会话的临时 statusline 脚本和启动 settings 做了带备份的短暂观察配置；结束时逐字节核对恢复。未新增模型请求，未改变模型、推理强度或窗口，未重启会话。压缩/HUD 产品源码本轮没有保留修改。


## 13:17 真实状态栏上报取证：空值送达，HUD 保留旧值

- 观察对象仍为上述压缩后的同一 Claude Code 进程与会话。仅对该会话 statusline 转发脚本加了临时观察，记录 model/context_window/session_id 及实际 HTTP 返回码；未保存密钥、完整请求或对话内容。
- 在当前空闲会话执行 `/model matou-chatgpt/matou-opus`，选择与原会话相同的精确模型 ID，触发真实 Claude Code 状态栏刷新，没有发送新的模型生成请求。
- 2026-11-14 13:17:50（Asia/Shanghai）捕获真实 payload：current_usage=null，used_percentage=null，remaining_percentage=null，context_window_size=272000，total_input_tokens=0，total_output_tokens=0。转发到原会话原 hook URL 的结果为 HTTP 200、curl exit 0。证据：evidence/provider-activation-2026-11-14/context-status-live.json。
- 上报后 CUA 真实 UI 仍为原模型、原 xhigh，HUD 显示 100%。恢复会话启动路径明确启用了 acceptStatuslineIdentity；本次 payload 的 session_id 与已恢复会话相符。
- 代码链路：provider-hook-server → provider-hud-payload-handler → SessionHudRegistry.ingestProvider。session-hud-registry.ts:349–352 只在 percentage 为数字时覆盖旧值，null 经 numeric 转换后不更新；并未把明确未知作为旧值失效处理。结合上一节定点测试，当前持续显示100%的原因已定位为：上报中的未知用量被忽略，HUD保留压缩前旧值。它不是路由器报告了100%，也不是当前上下文仍然满。
- 此次观察不能回放12:47压缩结束瞬间的首次上报，但已经排除“当前无法上报所以一直100%”作为持续不更新的解释：一次真实空值已送达后仍错误保留100%。
- 临时 statusline 脚本已逐字节恢复。此次同模型 `/model` 操作意外同时保存了全局默认；已向用户说明，用户确认原默认为 opus 后只将 settings.json.model 恢复为 opus，effortLevel=xhigh 保持；恢复前配置备份保存在 ~/.matou/backups/context-status-20261114/settings-before-default-restoration.json。
- 没有添加 postTokens 兜底，没有修改 HUD 产品代码或安装应用。后续正确修复应尊重真实 null：清除过期百分比、明确等待真实用量；不能用0%、7%或估算值宣称成功。


## 13:48 空值修复与 GPT 512K 配置

- 本轮按用户要求由主代理直接完成。收到 provider 明确 null 时删除旧 contextPercent；没有该字段的普通 hook 继续保留已有真实值；数字 0 合法。界面显示“等待真实用量 / Awaiting usage”，真实用量到达后恢复数字。没有使用 compact_boundary.postTokens 或本地估算填充 HUD。
- ChatGPT 订阅在 Claude Code 的全部映射档位统一采用 512000 tokens；直接 API GPT 路由也应用同一进程级窗口。Codex 统一传入 model_context_window=512000、model_auto_compact_token_limit=460800、scope=total。新建、恢复和 fork 共用启动入口，普通 Shell 输入 codex 也进入该入口。保留模型、推理档位及权限选择，未修改全局 Claude/Codex 配置。
- 原生 Claude Code 2.1.325 独立配置目录下 get_context_usage 返回 maxTokens=rawMaxTokens=512000、autocompactSource=settings、apiUsage=null。/context 返回的原生 autoCompactThreshold 为479000；配置中的90% override与该展示字段不同，不声称两个 CLI 的原生缓冲/显示阈值完全相同。原生 Codex config/read 返回512000窗口和460800压缩阈值，既有模型与推理档位保持。证据：evidence/provider-activation-2026-11-14/context-native.json。此验证不调用模型，也不代表实测每个模型的上游硬上限。
- 自动验证：HUD registry 24项、TerminalHud 44项；Claude launch与PTY subscription/API启动37项、provider launch plan14项通过；Codex运行时有关启动/恢复/fork/切换的6项通过，含新增Shell输入codex的定点测试。旧的完全相等参数断言已随新增512K参数更新。runtime/desktop类型检查、完整构建、命名检查通过。
- 真实 Electron 界面与真实运行时、注入的provider测量：100% → explicit null → 等待真实用量 → 0% → 21% 全链路通过，等待态没有百分比圆环；截图context-waiting.png、context-measured.png已检查。它证明界面与上报链路，不等于用户真实账号的产品验收。E2E首次因既有副屏要求未启动，改用测试支持的primary选项；第二次定位器错误，按已有终端元素修正后通过。没有改动产品显示布局。
- Codex严格配置探针发现用户已有disable_response_storage旧字段不被当前CLI识别；改用CLI正常启动模式完成只读验证，未修改用户配置。


## 13:54 本轮修复包安装

- 使用当前源码构建并完成Developer ID签名，打包后真实Electron验证SQLite、PTY、重放、截断恢复与数据库前向兼容：1项通过（16.1秒）。codesign --verify --deep --strict在打包后、安装前及安装后均通过。未公证、未发布。
- 已安装 /Applications/码头.app；上一版完整保留于 /Users/icesword/.matou/backups/context-512k-20261114/previous.app。
- 未终止用户当前应用进程和会话；正在运行的进程仍是旧版。重启应用后HUD修复生效，恢复或新建的托管GPT会话采用512K。没有修改用户全局默认模型或effort。
- 本轮没有提交或推送源码。没有执行全仓全量回归，也未进行用户验收。


## 13:56 单独复现长时间压缩：单请求成功，长摘要持续生成

- 输入：从用户原会话的2026-11-14手动压缩前历史复制出独立文件，保留前一次压缩边界后的有效历史。以独立CLAUDE_CONFIG_DIR、独立工作目录、--resume副本、--fork-session启动原生Claude Code 2.1.325；禁用工具与MCP以防继续执行原会话工作。没有改写或重启原会话。使用原有ChatGPT账号/路由、相同matou-opus → gpt-6-astra、effort=xhigh，配置新的512K窗口。
- 压缩前只读原生控制查询确认maxTokens=512000，当前本地统计838670；压缩边界记录pre_tokens=839260，与原事件相同。512K是客户端的后续压缩预算，不会在启动时自动裁掉已有长历史。
- 通过独立回环转发器记录请求大小、消息数、状态码、流事件计数、字符数、usage和时间，不保留HTTP认证头、请求正文或摘要正文。
- 实际发出1次POST /v1/messages?beta=true，请求16349139字节、896条消息。转发到本机路由器47ms；收到HTTP200及首字节10975/10976ms；流结束345111ms，CLI记录压缩duration_ms=345177ms，总观察345.524秒（5分45秒）。没有HTTP错误、SSE错误或第二次客户端请求；路由器配置request-retry=0。
- 流开始message_start的input_tokens=414683是初始字段，不能当作最终账本。结束message_delta报告input_tokens=503055、output_tokens=13786，其中thinking_tokens=1034。摘要文本27053字符。对外进度中曾将414683称为实际输入量，结束后已明确更正为最终503055。
- 压缩成功的post_tokens=12767只作诊断记录，没有写入HUD，不用它估算百分比。
- 结论：本次成功复现数分钟等待；主体时间在单次成功请求的长摘要响应期间，不支持“超过272K硬上限后连续盲重试”解释这次复现。开始约11秒后已有响应，之后约334秒仍在持续返回模型响应；客户端边界外的排队、prefill、思考、网络耗时无法逐项分离。没有对照降低effort，因此不声称xhigh是唯一原因，也未更改用户推理档位。
- 原12:41事件缺少请求日志，本次不能回溯证明它每一阶段或重试数完全相同；能证明相同历史、相同模型及effort，在512K下可以单次成功同时仍需5分45秒。提高窗口本身不会直接缩短一次大历史摘要；降低摘要长度涉及保留哪些上下文，未擅自改变。
- 证据：evidence/provider-activation-2026-11-14/compact-reproduction.json。此次是真实模型推理请求；上述界面E2E仍属于注入provider测量，二者不混同。独立进程、转发器已结束，原会话保持原样。


## 压缩专用模型可行性核对

- 当前原生Claude Code 2.1.325没有找到公开的压缩专用模型配置；正常摘要调用使用cacheSafeParams.model或mainLoopModel，并有原生失败降级分支。官方model-config/env-vars未列出COMPACT_MODEL配置；PreCompact公开输出仅能提供摘要指令或阻止压缩，不能指定模型。
- 原生请求构造Qqn可通过CLAUDE_CODE_GATEWAY_HINT_HEADERS=1发送x-claude-code-compaction: manual/auto。仅对隔离子进程启用该标志，使用本地模拟Anthropic接口验证：普通请求无标记，/compact请求带manual，压缩后普通请求恢复无标记；三次请求携带的会话model均为matou-chatgpt/matou-opus，effort均为xhigh。
- 本地选择器可只对带标记请求选择独立摘要模型/effort；本次返回合成摘要，没有请求真正的快模型，routedModel/routedEffort只是选择器模拟结果，不代表真实上游调用。证据：compact-routing-feasibility.json。
- 这证明当前版本可可靠区分压缩请求，并验证后续主请求模型和effort不变；尚未实现码头产品配置或生产路由，尚未验证候选模型对503K级实际输入的支持、耗时和摘要质量。候选策略替换原有摘要调用，不新增一次LLM调用；不同模型可能改变缓存命中与计费，不能在实测前承诺节省或具体秒数。该原生标记不是文档化稳定接口，未来CLI更新需保留回归检查。


## 独立压缩模型、供应商推理档位与分供应商上下文 — 本轮最终范围

用户明确纠正：上下文长度必须归属于供应商，不能全局统一。最终配置位于设置的 Claude Code 页，选择“上下文供应商”后，分别保存默认 / 512K / 最大可扩展。不同供应商草稿、保存值、启动值彼此隔离。ChatGPT/Codex 初始延续此前512K，官方Claude及其他供应商初始默认；可分别修改。普通默认模式保留CLI原生配置，订阅映射默认沿用已知最小窗口；512K显式请求512000，最大档只采用可靠目录数据，未知不会猜成1M。CLI会话有多个映射模型时共用已知最小窗口。页面显示模型目录数值，并说明该共享窗口边界。

压缩默认跟随会话。独立配置允许选择其他已配置的Claude Code供应商，包括ChatGPT、Antigravity及兼容Messages API；模型和推理强度独立。只改写带原生manual/auto标记的压缩请求，替换原来一次摘要调用，不新增LLM调用；不会承诺速度或费用下降。供应商编辑的Opus/Sonnet/Haiku每档可选推理强度，Codex单模型也可设置。显式供应商档位优先于原请求，follow保留原请求。普通API重复模型采用独立内部角色标识，计数请求只反映射模型，恢复保持角色。

模型元数据来自本供应商目录或对应原生CLI。订阅刷新会缓存该账号的窗口、最大扩展及effort字段，不借另一个Codex账号的缓存证明订阅上限。Claude原生探测使用独立临时配置、本地拒绝连接端点、无用户生成请求；模型切换失败时不沿用旧模型数值。压缩跨供应商剥离来源鉴权，loopback路由绕过外部代理，官方凭据仅发往严格匹配的官方origin。API endpoint与key从同一配置快照获取。出错保留真实上游错误，不静默换另一模型。

### 自动及实际运行证据

- Runtime相关20个文件218项通过；34项环境门控测试跳过。补充启用固定路由器二进制后，协议回归7项通过（本地合成上游）。Runtime启动/恢复/全局切换10项通过，其余140项未在该定点运行执行。两个独立数字策略、未知目录、角色恢复、凭据隔离等均有定点覆盖。
- UI AgentModelSettings最终11项通过；供应商及订阅表单此前定点共58项通过；desktop/runtime类型检查、构建、命名检查通过。未执行全仓全量回归。
- 真实Electron UI + runtime + PTY端到端通过：设置两供应商不同长度、切换保留草稿、保存重启恢复、主请求medium → 摘要low → 主请求medium，target只见其自己的fixture key。另原时区保存/小窗口滚动/重启恢复E2E通过。证据model-settings-routing.json、model-settings-context.png、model-settings-configured.png、model-provider-efforts.png。
- 原生Claude CLI + 生产网关 + 两个本地合成SSE服务验证：Sonnet high → /compact摘要模型low → Sonnet high，出现compact_boundary，三轮均成功。证据native-compact-gateway.json。另使用合成OAuth凭据的原生CLI通过网关与--settings注入验证，OAuth Bearer、beta、流式响应保持，证据official-oauth-gateway.json。
- 初次E2E失败是测试将已展开的更多设置收起，以及标签命中section/包含选项的问题；按真实可访问名称及控件角色修正后通过。初次runtime回归暴露默认启动误触发目录探测的副作用，已改为普通默认直接继承CLI；类型检查暴露可选字段写undefined，已修复。未将这些失败算作通过。
- 没有执行真实Antigravity模型摘要，也未用用户长历史验证新摘要模型的质量、容量或速度；本轮模型调用均为本地合成响应。未修改用户当前主模型、推理强度或中断会话。

### reference product 交互对照矩阵（本次增量）

| 场景 | 基线 | Matou结果/证据 | 结论 |
| --- | --- | --- | --- |
| 编辑供应商的三个角色及保存入口 | 用户本轮现有Matou截图，保留三角色和原保存流程 | model-provider-efforts.png及表单测试 | 新增effort字段，原流程保留 |
| 分供应商上下文与独立压缩配置 | 用户本轮明确新增规格；非现有reference PRD复刻项 | model-settings-context.png、配置与重启E2E | 已实现并验证独立保存 |
| 原生压缩与压缩后恢复 | 当前原生Claude CLI实际运行 | native-compact-gateway.json | 原生流程保持，仅摘要请求分流 |
| reference product全范围界面/交互对照 | 本轮未重新运行参考产品 | 无完整对照证据 | 不宣称原PRD对照矩阵全部闭合或已获用户验收 |


### 本轮安装结果

- 已以当前源码产物打包并完成 Developer ID 签名；打包后、安装前暂存副本及安装后 `codesign --verify --deep --strict` 均通过。未公证、未发布。
- 打包应用使用独立数据目录完成真实 Electron SQLite、PTY、历史重放、截断恢复与数据库兼容冒烟，1项通过（16.5秒）。安装后仅检查签名，没有再次运行重复的全套冒烟。
- 首次打包失败原因为从仓库根目录调用 builder 导致相对 entitlements 路径未找到；改在 desktop 目录运行后成功，无需修改产品代码。
- 新版已安装到 `/Applications/码头.app`；上一版完整保留于 `/Users/icesword/.matou/backups/model-settings-20261114/previous.app`。
- 没有终止用户当前运行的应用和会话；当前进程仍为旧版，重启码头后出现新设置。保存配置后，新建、恢复或重启会话采用新策略。
- 本轮源码未提交、未推送；未进行真实反重力长历史摘要性能/质量测试，也不宣称用户验收完成。
