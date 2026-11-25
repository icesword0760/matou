# 码头 macOS 系统通知设计

2026-11-25 · 已与用户确认方案一与全部细节

## 要做什么

码头里所有 agent（Claude Code、Codex、Antigravity、DSH）产生的通知，在码头不在前台时，同时发一条 macOS 通知中心通知：

- 通知来源显示「码头」，点通知把码头带到前台并跳到对应卡片。
- 应用设置新增「通知」页，「系统通知」默认开启，状态持久化。通知中心底部沿用「通知声音」。
- 人离开电脑回来后，系统通知中心里留着最近的通知，点一条就能回到对应卡片。

## 为什么

现在只有 App 内通知（右上角铃铛 + 通知中心面板）。码头在后台时用户完全看不到 agent 在等确认、已经做完或已经空闲。用户明确要覆盖两个场景：

1. 人在别的 App 里、码头在后台，需要被叫回来。
2. 人离开电脑，回来想知道刚才发生了什么。

## 不做什么（本轮）

- Windows 系统通知（继续用 App 内通知，等 macOS 验证后再补）。
- 聚合摘要、按事件类型分级（只提醒需要操作的）——后续可加，不做在第一版。
- 不在前台之外再判断「当前卡片是否聚焦」：码头在前台时一律只走 App 内通知。
- 不新增通知事件源，不改变现有去重、冷却、同会话只留最新一条的规则。

## 靠什么成立

码头里所有 agent 的通知已经走同一根管道：

1. 各 provider 的 hook 事件进入运行时，生成 `agentNotifications`（`apps/native/runtime/src/provider_hook.rs`，Claude Code / Codex / Antigravity / DSH 共用）。
2. 桌面端 `Workspace::update_notifications`（`apps/native/desktop/src/workspace.rs`）统一接收：按 eventId 去重、按 owner+cooldownKey 冷却、同一会话替换旧条目、正在看的会话直接标已读、其余进通知中心。
3. 点通知中心条目 → `open_notification` → `Command::FocusSession`，已有完整定位链路。

系统通知只要挂在第 2 步的出口，就自动对所有 agent 生效；点击只需要复用第 3 步。

## 已定下的决定

- 通知中心底部保持原样，只保留现有「通知声音」开关，不加任何东西。
- 系统通知开关放在应用设置面板新增的「通知」页（导航在「通用」分组内、语言与时区之后、诊断之前）：一行「系统通知」+ 说明文字「码头不在前台时，在 macOS 通知中心显示所有 AI 会话的提醒。点击通知会回到对应卡片。」开关沿用设置页现有样式（44×26，开启用主题强调色）。
- 默认开启。首次启动（第一次 Workspace 创建）且开关为开时，向 macOS 申请通知权限。
- 被系统拒绝时：开关不假装开着——系统通知不发送，开关下方显示一行提示「macOS 已关闭通知，去系统设置开启」，点击打开系统通知设置页。
- 非 macOS 构建不显示「通知」设置页；macOS 但无 bundle id 时页面仍显示，开关置灰并提示「仅在打包后的码头中可用」。
- 只在码头不在前台时发系统通知（`NSApp.isActive == false`）。前台时保持现有 App 内通知 + 提示音。
- 同一会话的系统通知使用同一 identifier，后来的替换前一条；没有会话 ID 时用 `event:<eventId>`。
- 点击系统通知：激活码头 + 复用 `open_notification` 跳转；目标已不存在时只激活码头，不报错。
- 不重复响：本批通知产生了系统通知时，跳过 `NSBeep`；系统通知的声音跟随「通知声音」开关。
- 非 macOS 构建、或未从 .app bundle 启动（无 bundle id）时：不显示「系统通知」开关，不调用系统通知 API，不崩溃。
- 界面文案用现有通知中心的中文风格；不引入外部品牌词。

## 技术方案

- 新增 `apps/native/desktop/src/system_notifications.rs`：
  - macOS 用 `objc 0.2` + `block2`（两者都已在依赖树中）调用 `UNUserNotificationCenter`。
  - `install()` 在 `Application::run` 之前调用，注册 `UNUserNotificationCenterDelegate`（`willPresent` 返回 banner/list/sound，`didReceive` 取 identifier 入队并激活 App）。
  - 授权状态缓存在原子变量里；`request_authorization`、`refresh_authorization` 异步更新。
  - `post(SystemRequest)` 组装 `UNMutableNotificationContent` 与 `UNNotificationRequest`。
  - 点击队列 `Mutex<Vec<String>>` 由 Workspace 每 250ms 轮询取出。
  - 非 macOS 提供同名空实现，避免调用点到处 `cfg`。
- `Notifications` 增加 `system_enabled` / `system_path`，持久化方式与 `notification-sound` 相同（`<data_root>/notification-system`，写 `1`/`0`，缺失视为开启）。
- `update_notifications`：`push` 改为返回接收回执（条目 id + 是否未读），未读且开关开、App 不在前台、已授权时发系统通知；本批发过系统通知就不 `NSBeep`。
- Workspace 启动时拉起 250ms 轮询，把点击的 identifier 翻译成 eventId / sessionId 后调用现有跳转。
- 调试验收：debug 构建支持 `MATOU_SYSTEM_NOTIFICATION_PROBE=1`，启动 3 秒后发一条固定内容的系统通知，用于验证弹窗、点击激活与开关/授权提示，不参与 release。

## 验收

- 单元测试覆盖：开关默认值、持久化、系统通知决策矩阵（未读 × 开关 × 前台 × 授权）、identifier 选择。
- `cargo test --manifest-path apps/native/Cargo.toml -p matou-native-desktop --offline --locked` 通过。
- `pnpm check:identifiers` 通过。
- 真机：debug .app 启动，打开码头后切到别的 App，`MATOU_SYSTEM_NOTIFICATION_PROBE=1` 的通知出现，来源「码头」；点击回到码头；关闭开关后不再出现；系统设置里拒绝权限后出现提示文案。用真实 agent 会话再走一遍。
