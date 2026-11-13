# 第八关：ChatGPT 真实账号验收

2026-11-13，开发版 PID **6180**，Claude Code **2.1.323**，固定路由 **8.0.69**。本轮真实验收执行完成，**不等于整个 PRD 已全部通过**。后续裁决接受每周额度、真实显示名、WebSearch、WebFetch 摘要归因和本轮文字观察；模型残留、1M 显示和切换耗时进入返工，详见[模型切换返工记录](subscription-accounts-switch-rework.md)。

最初的真实验收轮只改验收文档和证据，没有修改产品代码、重启开发版、提交、推送或发布；本轮返工的代码修改与隔离复测另见返工记录。正式版 PID **97797** 未操作，收尾仍在运行。当前开发版已切到 Anthropic 官方，保留原会话，停在未登录提示，没有向官方发问或代用户登录。

## 用户最终复核（第八关 ChatGPT 结束）

2026-11-13 用户反馈：真实账号两次切换的主观等待均在 1 秒以内；用户另行核对切回官方后的原生进程，参数为 `--model default --resume <原会话>`，`ANTHROPIC_BASE_URL=https://api.anthropic.com`，激活项为 `anthropic-official`。本段依据用户复核，执行者没有追加真实请求。第八关 ChatGPT 结束。Antigravity 按用户决定不做真实账号测试，不再列为待执行账号验收；已有合成凭据隔离结果不冒称真实账号通过。后续进入第九关打包、签名与安装包验收，不发布。

## 结论与用户可见结果

| 场景 | 本轮实际结果 | 结论 |
| --- | --- | --- |
| 用户本人完成登录、保存并切换 | 本轮开始前已由用户完成；存储读回 ChatGPT 已激活，三档映射与交代一致 | 已确认保存结果；登录动作不是本轮目视验收 |
| 设置页卡片 | 使用中；第二行 `[email redacted] · Pro 500`；额度为「每周 0%」；模型列「GPT 6.0 Astra / 默认模型」 | 通过（该套餐无 5 小时窗口）：用户已独立实测 wham/usage 仅有 primary_window=604800 秒、secondary_window=null；真实显示名 GPT 6.0 Astra 已接受 |
| 一句问答 + Read | 合并为一个用户回合；界面显示 Read 1 file，返回 `MATOU_CHATGPT_STAGE8_SAFE_READ`，用时约 10 秒 | 通过；响应记录实际模型是 `gpt-6-astra` |
| `/model` | 仅 Default、Opus · GPT 6.0 Astra、Sonnet · GPT 6.1 Sol、Haiku · GPT 6.0 Luna；三档说明均为真实 ID、272K | 通过四项范围与真实名称；Default 的当前值包含 `[1m]`，三档说明为 272K；未把此字面值当作实测 1M 能力 |
| WebSearch | 明确执行一次搜索，显示 Did 1 search in 13s，整个回合约 35 秒；返回两条新闻结果与引用 | 搜索、结果和引用返回通过；引用是当日新闻目录，标题事实未逐条核实 |
| WebFetch | 明确执行 Fetch(example.com)，577 字节、200 OK，返回中文总结；整个回合约 19 秒 | 抓取和总结通过；代理证据与 Haiku 辅助调用证据见下文 |
| 路由代理 | 配置为 `http://127.0.0.1:17890`，且成功问答期间内核记录引擎到该代理双向收发 | 通过，超出「只看到配置」的验证 |
| 引擎被强杀 | 只杀归属开发版的引擎 38254；supervisor 37814 保留；新引擎 51802 约 318.8ms 就绪，同端口、HTTP 200、凭据 active | 通过一次自动重启；没有测试耗尽重启次数或其它故障 |
| 重启后下一问 | 原会话回复「重启后恢复成功。」，约 3 秒，实际响应模型仍是 `gpt-6-astra`；卡片保持使用中 | 通过；瞬时中间状态未目视捕获 |
| 切回官方与会话恢复 | Toast「已切换为 Anthropic 官方；1 个 Claude Code 会话已更新」；原会话与历史保留，新进程启动；随后显示 `Not logged in · Run /login` | 切换和身份恢复通过；官方认证与推理未验证，也未按要求发问 |
| 官方模式的模型显示 | 底栏仍显示 `matou-chatgpt/matou-opus (1M context)` | 未闭合：不能用切换 Toast 代替模型重置验收；未发官方请求，无法判断实际请求影响 |

## 问答与工具的真实证据

用户侧发给模型的回合共 **4 次**：

1. 一句自我介绍和一次 Read，合并为一条输入。
2. 一次 WebSearch。
3. 一次 WebFetch。
4. 引擎重启后的一句恢复提问。

没有额外搜索、抓取重试、压力请求、Antigravity 请求或官方提问。原生 CLI 自动使用两次 ToolSearch 发现延迟加载的 WebSearch / WebFetch；它们属于对应回合的产品行为。主会话日志记录了 **9 条主循环 API 请求**，内部搜索与摘要辅助请求不全在此日志内，因此不把 4 个用户回合或 9 条主循环记录冒称为全部上游请求数。

自我介绍实际为：

> 我是 Claude Code，一个可以帮助你阅读文件、编写代码和完成开发任务的 AI 助手。

这受到 Claude Code 的系统角色影响，不作为供应商身份依据。实际后端身份由以下证据共同确认：

- 原生进程 40425 的 `ANTHROPIC_BASE_URL` 指向本开发版路由 `127.0.0.1:61588`。
- 请求模型为 `matou-chatgpt/matou-opus`，路由别名映射到 `gpt-6-astra`，且强制账号前缀开启。
- 本轮真实 assistant 响应记录中的 `message.model` 是 **gpt-6-astra**，包括 Read 前后及重启后的回复。
- Read 工具结果与最终回答都包含正确的文件标记，界面显示 1 Tools。

证据：`final-transcript-metadata.json`、`transcript-metadata-phase-a.json`、`claude-launch-sanitized.json`、`ui-phase-a.md`。

## WebSearch 结论

真实查询为 `"November 13, 2026" news headlines`。界面回传两条结果：布鲁克林致命坍塌事故，以及巴基斯坦吉泽尔致命客运面包车事故；引用分别落在 Washington Post 的当日 sitemap 和 Dawn 的当日新闻目录。**这里只记录 CLI 返回结果，不把这些标题当作已逐条核实的新闻事实。** 没有另行访问新闻页面或追加模型请求。

固定路由源码 `internal/translator/codex/claude/codex_claude_request.go:548` 将 Claude 的服务端搜索工具转换为 Codex `web_search`；响应转换器处理 `web_search_call`。本轮实际执行的是原生 WebSearch，结果与引用返回原 Claude Code 会话，插件路由关闭，模型流量走同一 Codex 引擎。源码与真实链路结果一致；本轮没有抓取、保存含认证头的请求体，所以转换类型的依据是固定源码与真实工具结果，而不是一份裸请求抓包。

证据：`ui-phase-b.md`、`router-websearch-source.json`、`api-request-metadata.json`、`proxy-proof-web-tools.json`。

## WebFetch 结论与模型归因边界

实际工具结果：HTTP **200 OK**，**577 bytes**，工具记录 durationMs **3590**；工具摘要为「该域名供文档示例使用，并非服务，不建议用于测试或监控。」主模型最终用一句中文转述该摘要。整个用户回合约 19 秒。

代理证据：

- 原生 Claude 的 HTTP_PROXY、HTTPS_PROXY、ALL_PROXY 都指向同一个用户代理；继承的 NO_PROXY 不排除 example.com 或 api.anthropic.com。
- WebFetch 期间，内核被动计数记录原生 Claude PID 40425 到 `127.0.0.1:17890` 的双向收发，3892 字节入、1886 字节出；同期路由的模型请求也有到该代理的流量。
- 固定客户端的实际抓取路径为 Axios `get`，没有在这次调用中覆盖代理选项，且原生工具返回 200。配置、抓取代码与网络行为共同支持抓取经过用户代理；没有解密 TLS 或持久化 CONNECT / HTTP 请求头来记录目标主机。

Haiku 摘要证据：

- 实际启动环境：`ANTHROPIC_DEFAULT_HAIKU_MODEL=matou-chatgpt/matou-haiku`，别名映射到 **gpt-6-luna**，旧 SMALL_FAST 覆盖已清除。
- 当前固定客户端内，WebFetch 的内容摘要函数使用 `querySource: web_fetch_apply` 调用小模型 helper；helper 从 Haiku 默认映射解析模型。这是对本机同一固定客户端代码的检查。
- 本轮 `/cost` 的真实累计计数包含 `matou-chatgpt/matou-haiku`：**165 input / 24 output / 0 cache read**；说明真实辅助调用使用过 Haiku，而不是仅设置了变量。

**上述三项支持 WebFetch 内部摘要使用 Haiku→GPT-6 Luna，但 `/cost` 是整段会话累计值，内部辅助调用未在主对话日志中逐请求公开，不能将它当作独立标注该次摘要的抓包证明。逐请求独立标注的技术边界仍如实保留；用户裁决现有证据已足够，本项通过。** 面向用户的最终转述来自主模型 GPT-6 Astra，这与工具内部的小模型摘要不是同一回合。

本次没有出现域名预检失败，**没有添加或修改 skipWebFetchPreflight**。无需为本次成功样本跳过域名预检；今后遇到该错误时按交代先报告，再决定。

证据：`webfetch-turn-sanitized.json`、`ui-phase-b.md`、`ui-phase-c.md`、`proxy-proof-web-tools.json`、`proxy-bypass-check.json`、`claude-webfetch-source-excerpts.json`、`claude-helper-source-excerpts.json`、`claude-small-model-env-source.json`。

## 异常恢复与切回官方

只在核对以下归属后发送一次 SIGKILL：6180 开发版 → 37814 supervisor → 38254 引擎；二进制路径、真实数据目录、配置路径和父进程均匹配本任务。没有结束 supervisor、runtime、Claude 或正式版。

新引擎 51802 在原端口 61588 恢复，HTTP 200，保存凭据仍为 active；开发版 6180、runtime 6189、原生 Claude 40425 和正式版 97797 都存活。原生会话和 provider session ID 保持不变，下一问成功。

切回官方后：

- provider-config 中激活项为 Anthropic 官方，ChatGPT 卡片与三档映射保留。
- 原 Matou session ID `92c03f03-bc08-44d1-81f5-f07e0be860c5`、原生 provider session ID `7d2f4336-f2a5-4aef-aaa8-ce7d2a5580b9` 不变；运行进程由 40425 变为 62946。
- 新进程的 base URL 是 `https://api.anthropic.com`；三个订阅默认模型变量及订阅认证变量都不在环境中；使用同一 `--resume` 身份，没有显式 `--model`。
- 路由和 supervisor 在不再被需要后停止，未留下已重启的引擎。
- 官方隔离配置未登录，按要求不提问。底栏仍保留订阅别名，属于待闭合显示/恢复风险，不能据此断言仍在向 GPT 转发。

阶段 A 的一次菜单操作选中了原本已使用的 Opus，客户端把它保存为新会话默认。事后只读检查 `settings.json` 中是普通 `model: opus`，不是含前缀别名；因此**不能把官方底栏别名残留直接归因于这次菜单误选**。本轮不更改该设置、不追加官方请求，也不扩大为产品修复任务。

证据：`engine-restart.json`、`session-after-engine-restart.json`、`session-after-official-switch.json`、`official-launch-sanitized.json`、`final-state.json`、`ui-phase-c.md`。

## 截图、脱敏与验证边界

Terra 通过 CUA 真实观察了设置页、模型菜单、工具结果、重启后卡片和官方未登录状态。本轮可在 CUA 查看截图，但其受支持接口没有安全裁切、打码并保存所需的能力；没有保存未经邮箱打码的设置截图，也没有用生成图片代替证据。依用户允许的兜底，全部留下文字观察。**本轮交付 0 张 PNG；用户裁决本轮接受文字观察，最终视觉验收留到正式安装包。**

证据只落在本 worktree 的 `output/model-router-stage8-real/`。认证头和请求体没有被抓取或写出；账号邮箱统一 `[email redacted]`。保存凭据文件实际权限 **600**，credentials 目录 **700**，provider-config **600**。没有复制真实凭据或 OAuth 授权地址。

本轮未改产品代码，未重复已有测试/构建；运行的是监督者已启动的开发版。命名门禁、空白检查和新证据隐私扫描另行记录。早期相关自动测试结果不替代本次真实行为，也不把历史全量失败改写为全绿。

收尾检查通过：`pnpm check:identifiers`、`git diff --check`；隐私扫描覆盖 42 份新证据及相关文档，实际账号邮箱、当前路由密钥、令牌模式与授权 URL 均无命中。证据为 `identifiers.log`、`privacy-scan.json`。收尾再次核对开发版 6180、正式版 97797 和官方恢复进程 62946 仍在，原 supervisor 与引擎已自然退出。

## 我自己做的决定

1. 将自我介绍与 Read 合并成一个用户回合；WebSearch、WebFetch、恢复提问各一次，官方零次提问。
2. 只做被动内核网络计数与白名单元数据取证，不启用请求正文日志、不改代理或用量统计配置。
3. 用本机 `/cost` 增加辅助模型证据，不为证明 Haiku 额外调用模型；累计值仍按累计值报告。
4. 来自 Terra：不能安全保存裁切打码截图时用文字；只信任本次隔离 workspace；输入法残留先清空核对再提交；官方未登录时不继续登录或发问。菜单误选当前 Opus 的副作用已明确记录。
5. 发现 5 小时额度缺失、显示名差异及官方别名残留时只记录，本轮不擅自修复或改变产品取舍。

## 后续裁决与未闭合项

按用户本轮裁决：每周额度通过（该套餐无 5 小时窗口）；GPT 6.0 Astra 真实显示名接受；WebFetch 摘要模型归因接受；WebSearch 通过；本轮文字观察接受。以下保留返工前的现场状态，修复与隔离复测结果另见[模型切换返工记录](subscription-accounts-switch-rework.md)。

- 最终视觉验收由用户在正式安装包上完成；本轮文字观察已接受。
- 返工前官方恢复后的模型别名显示订阅值；本轮修复及本地假上游首请求验证已通过，用户旧开发版未重启，用户真实账号复核已通过，见上方最终复核。
- 原现场首次原生启动约 25 秒且旧画面滞留；本轮已拆分测时并优化，隔离基准和高并发负载边界见返工记录，用户真实开发版两次切换均在 1 秒内，已通过本关。
- Antigravity、其它异常、打包签名、Intel 原生与全量回归不在本轮新增验证内。
