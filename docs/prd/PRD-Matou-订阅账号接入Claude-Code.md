# PRD：用订阅账号驱动 Claude Code（ChatGPT / Antigravity）

> 依附于《PRD-Matou-全局供应商切换》。本 PRD 只新增「订阅账号」这一类供应商，其余切换规则沿用原 PRD。

## 1. 产品结论

用户可以在「设置 · 服务供应商 · Claude Code」里，用自己的 **ChatGPT 账号** 或 **Antigravity 账号** 登录，一键把 Matou 中的 Claude Code 切换为由 GPT 或 Gemini / Antigravity 内含模型驱动。无需 API Key、无需安装任何额外工具、无需手动启动任何服务。

实现方式：Matou 内置一个本地「模型路由」（基于 MIT 许可的开源转发服务，固定版本随 Matou 发布），负责协议转换与账号登录；Matou 侧的账号卡片、额度显示、Claude Code 接管参数、异常反馈等关键逻辑，逐项借鉴 CC Switch（MIT）的现有实现（见第 7 节）。

## 2. 用户场景

- 有 ChatGPT Plus/Pro 订阅、没有 Anthropic 订阅的用户，想继续用 Claude Code 的工作方式。
- 有 Google AI Pro/Ultra 订阅的用户，想通过 Antigravity 账号在 Claude Code 里使用其中包含的 Claude / Gemini 模型。
- Anthropic 账号额度用完或受限时，临时切到另一个订阅继续工作，会话不丢。

## 3. 交互规格

### 3.1 入口

- 「设置 · 服务供应商 · Claude Code」点「新增供应商」，选择列表顶部新增分组 **「订阅账号」**：`ChatGPT 账号`、`Antigravity 账号`。原有 API Key 预设保持不变，排在其后。
- 订阅账号需要 **macOS 15 或更高版本**。在 macOS 15 以下，分组内两个选项置灰、不可选，下方显示「需要 macOS 15 或更高版本」/ "Requires macOS 15 or later"。系统支持由 runtime 读取 OS 版本并下发，不依赖渲染层 UA。其它功能不受影响。
- 已有订阅卡片在旧系统上仍显示账号及已保存模型，但切换不可用；runtime 同样直接返回上述文案，不尝试启动路由。启动对账与设置页额度读取也不在旧系统上启动路由。
- Codex 页签不新增此分组（Codex 官方已原生支持 ChatGPT 登录）。

### 3.2 登录

1. 选择 `ChatGPT 账号` → 弹窗显示「在浏览器中登录」按钮与一句说明（"使用你的 ChatGPT 订阅额度，不需要 API Key"）。
2. 选择 `Antigravity 账号` → **首次**先显示风险确认：
   - 标题：使用 Antigravity 账号有封号风险
   - 正文：Google 的 Antigravity 条款不允许第三方工具使用该账号。被判定违规时，Antigravity 与 Gemini CLI 账号可能被停用，申诉机会有限。
   - 勾选「我了解风险，仍要继续」后「继续」才可用；确认结果持久保存，之后不再弹出。
3. 点「在浏览器中登录」→ 系统浏览器打开授权页；弹窗变为「等待浏览器完成登录…」，提供「重新打开浏览器」（同一次登录重开授权页）与「取消」。
4. 登录成功 → 弹窗自动进入模型设置（3.4），供应商名称默认「ChatGPT」/「Antigravity」（同类只有一张；邮箱显示在卡片第二行），用户点「保存」或「保存并切换」。
5. 登录失败 / 超时（5 分钟）/ 取消 → 弹窗内显示原因与「重试」，不产生任何卡片。
6. 同类账号只保留一个：已有 ChatGPT 账号卡片时，再次添加等同于「重新登录」，替换原账号（弹窗内说明）。不提供多账号轮换。

### 3.3 卡片

- 订阅账号卡片与现有卡片同一列表、同一样式；第二行显示 **「账号邮箱 · 套餐」**（如 `a@b.com · Plus`）；模型列显示 Opus 档（订阅会话的默认模型）的真实名称与「默认模型」。
- 卡片显示额度摘要：ChatGPT 显示 5 小时与每周用量百分比；Antigravity 显示各模型组剩余额度。额度在打开设置页时读取一次，不轮询。读取失败只隐藏额度，不影响切换。
- 状态标记：
  - `需要重新登录`：账号续期失败；卡片主按钮变为「重新登录」。
  - `账号不可用`：Google / OpenAI 拒绝该账号（如地区不支持、违规停用）；显示服务方给出的原因，并提供文字按钮「切回 Anthropic 官方」，点击直接切换。
  - `模型路由未运行`：路由意外退出且自动重启失败。
- 编辑：可改名称与模型映射；删除规则沿用原 PRD（使用中的不可删）。删除时同时退出该账号登录。

### 3.4 模型设置

- 复用现有「模型映射」（Opus / Sonnet / Haiku 三档），候选列表从该账号实际可用模型读取。
- 默认映射（候选里存在才填入，否则取列表首个）：
  - ChatGPT：Opus → 旗舰编码模型；Sonnet → 均衡模型；Haiku → 轻量模型。
  - Antigravity：Opus → Claude Opus（thinking）；Sonnet → Claude Sonnet；Haiku → Gemini Flash。
- Claude Code 内 `/model` 菜单显示真实模型名与来源（如「Opus · GPT-6 Astra」，说明「ChatGPT 账号 · gpt-6-astra · 400K」），不显示误导性的 "Custom model"。

### 3.5 切换生效

- 沿用原 PRD：切换后 Matou 中运行的 Claude Code 会话自动重启并恢复会话身份，结果 Toast 与「查看会话」不变。
- 模型路由对用户不可见：选中订阅账号卡片时由 Matou 在后台启动；没有任何会话或设置页需要它时自动停止。用户感知只有「切换 → 会话恢复」。
- 路由启动失败时 **不切换**，保持原供应商，提示「模型路由未能启动」+ 原因 +「重试」。

## 4. 异常与恢复

| 情况 | 用户看到 | 结果 |
|---|---|---|
| 浏览器登录未完成 / 超时 | 弹窗内「登录未完成」+ 重试 | 不新增卡片 |
| 本机登录回调端口被占用（如 Codex CLI 正在登录） | 「另一个程序正在占用登录端口，请先完成或关闭它」 | 不新增卡片 |
| 路由启动失败 | 切换失败提示 + 原因 + 重试 | 保持原供应商 |
| 路由运行中意外退出 | Matou 自动重启路由一次；仍失败则 Toast 提示，卡片标「模型路由未运行」 | 会话保留，下一次请求恢复后可继续 |
| 账号续期失败 | 卡片「需要重新登录」 | 重新登录后同一卡片恢复，无需重新配置映射 |
| 账号被拒（403 等） | 卡片「账号不可用」+ 服务方原因 + 建议切回官方 | 不自动切换 |
| 额度用尽 | Claude Code 内显示上游的额度提示；卡片额度显示 100% | 不自动切换 |

## 5. 安全与数据

- 路由只监听 `127.0.0.1`；客户端访问令牌与管理密钥由 Matou 随机生成，文件权限仅当前用户读写。
- 账号凭据存放在 Matou 自己的数据目录下，与用户自行安装的同类工具互不影响。
- 关闭路由的远程管理、网页控制台下载、自动更新与用量统计；路由版本随 Matou 发布固定。
- 渲染层只拿到「账号邮箱 / 套餐 / 状态 / 额度摘要」，不接触任何令牌。
- 安装包附带路由的 MIT 许可声明。

## 6. 不做（本期）

多账号轮换与负载均衡、额度保护自动暂停、用量统计、Codex 页签订阅账号、Gemini API Key、自动故障切换、路由自动升级。

## 7. CC Switch 借鉴对照（关键代码）

基线：`farion1231/cc-switch` v4.0.31（MIT）。Matou 实现时逐项对照下列源文件的行为，测试覆盖其中已修过的问题；源码文件头注明来源提交与许可。

| Matou 能力 | CC Switch 参照源 | 借鉴要点 |
|---|---|---|
| Claude Code 接管参数 | `src-tauri/src/live/project/claude.rs` | 三档模型变量、`_MODEL_NAME` / `_MODEL_DESCRIPTION` 显示名、5.5 小版本客户端 id 换算、`CLAUDE_CODE_MAX_CONTEXT_TOKENS` 按模型窗口写入、网关模式 `CLAUDE_CODE_AUTO_MODE_SERVER=0`、子代理模型规则 |
| ChatGPT 套餐与额度 | `src-tauri/src/services/subscription.rs`（`wham/usage`） | `plan_type` → 套餐名映射、5 小时 / 每周窗口解析 |
| ChatGPT 可用模型 | `src-tauri/src/services/codex_oauth_models.rs` | 模型列表解析与回退 |
| Antigravity 额度与不可用原因 | `src-tauri/src/services/agy_account/quota.rs` | 额度分组解析、Ultra 档位名、锁区/不可用原因提取 |
| Antigravity 登录与账号识别 | `src-tauri/src/services/agy_account/oauth.rs` | 账号邮箱与项目识别、免费账号 403 处理 |
| 异常文案 | `src-tauri/src/proxy/error_mapper.rs`、`codex_quota_protection.rs` | 上游错误 → 用户可读原因与恢复动作 |
| 协议转换回归用例 | `src-tauri/src/proxy/providers/transform_codex_anthropic.rs`、`streaming_codex_anthropic.rs` 等 | CC Switch 已修过的问题（重复工具参数键、只有 OpenAI 认的字段、思考块、无 `call_id` 的工具结果）作为路由的验收用例 |

## 8. reference product 交互对照矩阵

| 场景 | reference product 基线 | Matou 结果 | 运行证据 | 差异结论 |
|---|---|---|---|---|
| ChatGPT 新增与登录 | 对照范围无同类能力 | 用户本人已完成登录、保存并切换；本轮读回卡片激活及三档映射；不重复登录 | [真实验收记录](../acceptance/subscription-accounts-stage8-real.md)，`session-before.json` | 新增能力；本轮验证保存结果，登录过程由用户完成 |
| ChatGPT 卡片与额度 | 对照范围无同类能力；沿用既有供应商卡片样式 | 使用中；[email redacted] · Pro 500；每周 0%；GPT 6.0 Astra / 默认模型 | 同记录及 `ui-phase-a.md`；仅真实文字观察，无保存截图 | 通过（该套餐无 5 小时窗口）；每周窗口和真实显示名 GPT 6.0 Astra 经用户裁决接受 |
| ChatGPT 会话、问答与 Read | 沿用原 CLI 会话和工具反馈 | 新建会话一次合并问答/Read 成功，返回安全标记；实际响应模型 gpt-6-astra | `final-transcript-metadata.json`、`ui-phase-a.md` | 新建订阅会话通过；用户此前切入订阅时对既有会话的恢复不是本轮观察 |
| ChatGPT `/model` 菜单 | 沿用原生 CLI 模型菜单交互 | 只有 Default 加 Opus / Sonnet / Haiku 三档，显示真实模型名与 ID | `ui-phase-a.md`、`claude-launch-sanitized.json` | 四项范围通过；本轮文字观察经用户接受，正式包视觉待用户验收；Default 的 [1m] 已进入本轮返工并隔离复测，见[返工记录](../acceptance/subscription-accounts-switch-rework.md) |
| ChatGPT WebSearch / WebFetch | 原生 CLI 工具调用、结果与引用反馈 | 搜索一次返回两条结果及引用；抓取 example.com 一次为 200、577 字节并总结 | 同记录的 WebSearch / WebFetch 专节、`ui-phase-b.md` | 通过；WebSearch 与现有 WebFetch 摘要模型归因证据经用户裁决接受；逐请求未公开的技术边界如实保留 |
| ChatGPT 路由代理 | 对照范围无同类内置路由 | 配置代理并实测引擎到该代理双向流量；成功响应 gpt-6-astra | `proxy-proof-phase-a.json`、`proxy-proof-web-tools.json` | 实际转发通过；未保存认证头或请求体 |
| ChatGPT 异常与恢复 | 沿用原 PRD 异常反馈模式 | 只强杀开发版引擎，约 319ms 自动重启；卡片正常；原会话下一问 3 秒恢复 | `engine-restart.json`、`ui-phase-c.md` | 单次重启通过；瞬态未捕获，其它失败弹窗/重启耗尽不由本轮覆盖 |
| ChatGPT 切回 Anthropic 官方 | 沿用原 PRD 的供应商切换与会话恢复 | 官方激活，Toast 一会话已更新，原身份/历史保留；隔离配置显示未登录，官方零次提问 | `session-after-official-switch.json`、`official-launch-sanitized.json`、`ui-phase-c.md` | 地址和认证变量已切回；模型残留本轮已修复，本地假上游双向首请求通过；用户已复核真实账号切换，两次均在 1 秒内，官方参数/地址/激活项正确，见[返工记录](../acceptance/subscription-accounts-switch-rework.md) |
| 旧macOS订阅入口与迁移卡片 | 用户新裁决：需要macOS15，旧系统灰显且runtime拦截 | 14.x支持标记false；两个入口置灰、指定中英文文案；卡片保留且切换禁用；API/官方仍可用 | [版本限制验收](../acceptance/subscription-accounts-macos-and-commit.md)，自动测试及中文Electron截图 | 注入系统版本通过，未冒称真实旧系统设备验收 |
| 打包签名与路由进程 | 本PRD随包能力 | arm64签名包、RunAsNode启用、管理200、只监听环回、退出无残留 | [第九关记录](../acceptance/subscription-accounts-stage9.md) | 用户已复核通过；不公证、不发布 |
| Antigravity 同范围能力 | 对照范围无同类订阅能力；其余沿用既有模式 | 用户决定不做真实账号测试；已有合成凭据隔离结果保留 | 用户第九关放行指示 | 真实账号测试经用户豁免，不冒称实际账号通过 |

2026-11-13 本次 ChatGPT 结果已回填。证据文件均位于 `output/model-router-stage8-real/`，详细结论见上述验收记录；截图按用户允许以文字观察兜底。随后用户已完成真实账号切换补充复核并结束第八关 ChatGPT；Antigravity 真实账号测试经用户豁免。第九关打包签名已由用户复核通过；随后旧macOS限制与本地提交见版本限制验收记录。尚不宣称整个 PRD 全部签收。

## 9. 验收标准

1. 全新安装的 Matou，不装任何其他工具：ChatGPT 账号登录 → 保存并切换 → 已运行的 Claude Code 会话自动恢复 → 发一句话得到 GPT 回复 → 执行一次无害工具调用（读文件）成功 → `/model` 菜单显示真实模型名。
2. Antigravity 账号同上，且首次添加必须经过风险确认。
3. 从订阅账号切回 Anthropic 官方，会话恢复后走官方；路由在无人使用后停止（进程不存在）。
4. 第 4 节每一行异常都能复现并看到对应反馈；路由被强制结束后能自动恢复。
5. 切换耗时与切换 API Key 供应商相比，用户可感知的等待不超过 1 秒的额外开销；空闲时路由不产生持续 CPU 占用。
6. 渲染层与日志中不出现任何令牌；凭据文件权限为 600。
7. `pnpm check:identifiers`、类型检查、单元 / 组件 / E2E 测试通过；安装包内含路由与许可声明且已签名。
