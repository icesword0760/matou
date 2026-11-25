# 底部状态栏改造设计（2026-11-25）

## 意图

底部状态栏现在把导航信息、会话健康、过程统计三类内容平铺在同一条 38px 的线上，共 13 项。窗口一窄，现有实现按宽度阈值静默隐藏，用户不知道少了什么。改造后底栏只回答两个问题：**我在哪、能不能继续干**；统计类信息收进一个可点开的入口。规则对 Claude Code、Codex、Antigravity 三种会话统一生效。

## 目标

- 常驻区只保留导航与健康状态；工具、hooks、MCP、待办、MCP 错误收进 `⋯`。
- 子代理（运行中）和订阅额度有数据就常驻，不进 `⋯`；两者没有数据时整项消失，不留占位。
- 订阅额度的 5h 与 Weekly 两个窗口永远同时出现；额外窗口只进额度浮层。
- 缓存倒计时仅剩 ≤10 分钟或已过期时上栏。
- 出口正常（Ok、无 warning）时压缩成国旗 + 绿点；异常、未知保持完整文案。
- 窄窗口先让模型名与会话计数，不让子代理和额度。

## 非目标

- 不改 cwd / git / 运行环境 / 账号入口的产品逻辑；账号入口仍仅 Claude Code 显示。
- 不改卡片、分屏、窗口布局与现有悬浮面板样式语言。
- 不做发布、不改版本号、不新增依赖。

## 现状与原因（file:line）

- 常驻项与统计项混排：`apps/native/desktop/src/workspace.rs:17400-17450` 把 `codex_hud::statistics` 的 mcps / tools / agents / hooks / quota / todos / mcp-errors 全部内联。
- 额度只在两种模式渲染：`apps/native/desktop/src/footer_controls.rs:383-444`，`claude-code` 与 `antigravity` 有分支，**Codex 直接返回 None**。但运行时已从 Codex 的 `rate_limits` 解析出 `usageWindows`（`apps/runtime/src/session/session-hud-registry.ts:834-870`）。
- Claude 无用量时渲染占位 `—`：`footer_controls.rs:414-416`（`ClaudeQuotaSlot { usage: None }`）。
- 缓存只要有 `contextGuard.cache` 就显示：`workspace.rs:20324-20383`。
- 出口文案固定为“地区 · 出口正常”：`egress_panel.rs:676-695`。
- 统计项的宽度阈值（290/330/380）散落在 `workspace.rs:17420-17445` 与测量状态 `workspace.rs:17520-17545`。

## 交互规格

常驻顺序（从左到右）：
`面包屑｜权限｜模型｜上下文环｜子代理（有则）｜额度（有则）｜cwd（有空间时）｜git｜运行环境｜出口｜账号（仅 Claude）｜⋯（有统计时）`

1. **子代理**：沿用现有 `agents` 槽位与颜色（含 ⚠ 状态），位置移到额度之前；计数为 0 时不渲染。
2. **额度**：三种模式共用一个槽位。
   - Claude：订阅账户用量（默认账户）或 `usageWindows`；无数据 → 整项不渲染，`—` 占位删除。
   - Codex / Antigravity：读取 `usageWindows`；无数据 → 整项不渲染。
   - 底栏最多显示两个窗口，优先取标签为 `5h` 与 `Weekly` 的两项（大小写不敏感），不足两项则按原顺序补首个可用窗口。两者同时存在时同进同出，任何宽度都不拆开；只有单窗时显示那一个真实窗口，不隐藏、不凑对。
   - 点击额度打开现有 `FooterMenu::Quota` 浮层，列出 `allUsageWindows`（回退 `usageWindows`）；Claude 若 HUD 里没有窗口数据，回退 `claude_quota_slots`。
3. **`⋯` 会话统计**：新增 `FooterMenu::Stats`。面板标题“会话统计 / Session stats”，每项显示名称与计数，下面最多列 6 行明细，超出显示“+N”；面板最大宽 335、最大高 360，可滚动。agents 不进该面板。没有任何可收纳统计时，`⋯` 不渲染。
4. **上下文**：底栏只保留环和百分比；`used / window` token 数字移入环的悬浮提示（`Context: 26% · 52k / 200k`），不再内联。
5. **缓存倒计时**：仅当 `timing` 为 `precise`/`estimate` 且 `remainingSec <= 600`，或已过期（`remainingSec == 0`）时显示；`timing` 未知或缺失时不显示。
6. **出口**：`tone == Ok` 时标签压缩为国旗（无两位国家码时用地区名），保留绿点与完整 tooltip / accessible label；Warn、Danger、Unknown 保持现状。
7. **窄屏**：应用最小窗口 720pt（`main.rs:374`），底栏约 460pt；560pt 不可达，不作验收。让位顺序：模型（`MODEL_HIDE_BELOW = 210`）→ 会话计数（状态内容宽 < 340）→ 账号文字 → “缓存”二字 → cwd 省略号。上下文沿用 `CONTEXT_HIDE_BELOW = 250`；子代理、额度、Git、环境、出口图标、`⋯` 不参与宽度隐藏。不换行、不增高底栏。
8. 视觉沿用现有 token：`FooterPalette` 深色底 `#151515`、正文 `#6e7681`、模型 `#79c0ff`、权限橙 `#ff6b35`、额度蓝 `#58a6ff`。

## 实现落点

- `workspace.rs`：`terminal_hud` 内重排 primary；移除统计内联与 290/330/380 阈值；新增 `⋯` 触发（`footer-stats`，加入 `control_focus` 与 `footer_bounds_probe`）；会话计数阈值；上下文环 tooltip。
- `footer_controls.rs`：`FooterMenu` 增加 `Stats`；`footer_popover` 增加面板分支；`footer_quota` 统一三种模式并支持空返回；Claude 额度也接上 `footer_menu_trigger`；`claude_quota_slots` 无数据时返回空。
- `codex_hud.rs`：提供额度窗口的底栏子集筛选；提供统计面板渲染（Detail 字段私有，面板渲染放本模块）；`context_ring` 接受可选的 token 明细。
- `egress_panel.rs` / `workspace_wiring.rs`：出口紧凑标签（Ok 时取国旗）。
- 测试与 probe：更新 `apps/native/desktop/examples/workspace_wiring_probe.rs`、`session_levels_probe.rs` 等对 footer 内容与 `footer-quota-menu` 的断言；新增覆盖：Codex 额度渲染、Claude 无用量不渲染、缓存 >600s 不显示、agents 常驻、`⋯` 面板内容、`⋯` 无统计时不渲染。

## 验收

- `cargo test -p matou-native-desktop` 全部通过（含新增测试）。
- `pnpm check:identifiers` 通过。
- 三种会话的实际底栏与 mockup `/tmp/matou-footer-mockup.html` 的“改造后”一致；宽度 970 与最小 720 下均无换行、无静默消失。
- 空数据路径：无额度、无子代理、无统计、无缓存数据时，对应项不占位。
