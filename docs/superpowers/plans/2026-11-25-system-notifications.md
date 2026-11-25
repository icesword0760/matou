# 码头 macOS 系统通知实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 码头所有 agent 的通知在码头不在前台时进入 macOS 通知中心，点击回到对应卡片，并提供「系统通知」开关。

**Architecture:** 通知仍由现有 `agentNotifications` 管道汇总；桌面端 `update_notifications` 在原有去重/冷却之后调用新增的 `system_notifications` 模块发一条系统通知。点击事件由 `UNUserNotificationCenterDelegate` 入队，Workspace 定时取出并复用 `open_notification` 跳转。偏好与现有「通知声音」同构，存 `<data_root>/notification-system`。

**Tech Stack:** Rust 2024、GPUI 0.2.2、objc 0.2.7、block2 0.6、`UNUserNotificationCenter`。

**Spec:** docs/superpowers/specs/2026-11-25-system-notifications-design.md

## Global Constraints

- 仅 macOS 启用系统通知。Windows 隐藏通知设置页。macOS 非 bundle 启动保留页面，将开关置灰并提示仅在打包后的码头中可用，跳过系统通知 API。
- 不新增通知事件源，不改动现有去重、冷却、同会话替换、聚焦会话标已读的规则。
- 只在 `NSApp.isActive == false` 时发系统通知。
- 本批产生系统通知时不播放 `NSBeep`；系统通知声音跟随 `notification-sound` 开关。
- 界面文案保持现有通知中心中文风格；源码与注释使用 Matou/中性命名，不出现参考产品品牌。
- 不提交、不发布、不改版本号；工作区改动与测试结果交回。
- 新增依赖只允许 `block2 = "0.6"`（已在 Cargo.lock 中，作为 gpui 传递依赖存在）。

## Review Focus

- 未授权/被拒绝时不能发通知，也不能显示为已开启；提示必须可点进系统设置。
- 非 .app bundle 启动（`cargo run`）时所有系统通知路径必须静默跳过。
- 点击系统通知时目标卡片已关闭/场景已切换：只激活码头，不 panic、不跳到错误会话。
- 同一会话连续通知只保留一条系统通知；不同会话不能互相覆盖。
- 系统通知发送失败或权限未决时，App 内通知与声音行为必须保持与改动前一致。

---

### Task 1: `system_notifications` 模块

**Files:**
- Create: `apps/native/desktop/src/system_notifications.rs`
- Modify: `apps/native/desktop/src/lib.rs`（导出模块，非 macOS 也不隐藏模块）
- Modify: `apps/native/desktop/Cargo.toml`（macOS 依赖加 `block2 = "=0.6.2"`）
- Test: 模块内 `#[cfg(test)]`

**Interfaces:**
- Produces（两种平台同名，非 macOS 为空实现）：
  - `pub enum Authorization { NotDetermined, Denied, Authorized }`
  - `pub struct SystemRequest { pub identifier: String, pub title: String, pub subtitle: String, pub body: String, pub thread_id: Option<String>, pub event_id: String, pub play_sound: bool }`
  - `pub fn install()` — 幂等；`main()` 在 `Application::run` 前调用
  - `pub fn is_available() -> bool` — 有 bundle id 且 macOS
  - `pub fn authorization() -> Authorization`
  - `pub fn request_authorization_once()` — 仅未决时请求
  - `pub fn refresh_authorization()`
  - `pub fn app_is_active() -> bool`
  - `pub fn post(request: SystemRequest)`
  - `pub fn take_pending_activations() -> Vec<String>`
  - `pub fn open_notification_settings()`
  - `pub fn notification_identifier(session_id: Option<&str>, event_id: &str) -> String` — 纯函数，`session_id.unwrap_or("event:<event_id>")`

- [x] **Step 1: 写失败测试**

```rust
#[test]
fn identifier_prefers_session() {
    assert_eq!(notification_identifier(Some("s1"), "e1"), "s1");
    assert_eq!(notification_identifier(None, "e1"), "event:e1");
}
```

- [x] **Step 2: 运行测试确认失败**

Run: `cargo test --manifest-path apps/native/Cargo.toml -p matou-native-desktop --offline --locked notification_identifier`
Expected: 编译失败（函数不存在）。

- [x] **Step 3: 实现模块**

macOS 实现要点：
- `install()`：`ClassDecl::new("MatouNotificationDelegate", class!(NSObject))` 注册两个方法（若类已存在则直接复用），实例化后 `setDelegate:` 到 `[UNUserNotificationCenter currentNotificationCenter]`，实例存 `static OnceLock<usize>`（`retain` 后泄漏保活，delegate 是弱引用）。
  - `userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:` 签名 `v@:@@@?`：取 `response.notification.request.identifier` → UTF8 → push 全局 `Mutex<Vec<String>>`，`NSApp.activateIgnoringOtherApps_(YES)`，调用 completion handler。
  - `userNotificationCenter:willPresentNotification:withCompletionHandler:` 签名同：completion handler 传 `Banner|List|Sound`（`1|2|4`）。
  - block 用 `block2::RcBlock` 创建，传入时 `&*block as *const _ as *mut c_void`；block 必须保持存活到调用返回。
- `request_authorization_once()`：`requestAuthorizationWithOptions_completionHandler:`，options = `Alert|Sound`（`4|2`），完成回调写 `AtomicU8` 授权状态。
- `refresh_authorization()`：`getNotificationSettingsWithCompletionHandler:`，读 `settings.authorizationStatus`（0 未决、1 拒绝、2 授权、3 provisional 视为授权）。
- `post()`：`UNMutableNotificationContent` 设 title/subtitle/body/`userInfo`（eventId、sessionId 如可用）/`threadIdentifier`；`play_sound` 时 `setSound:` 为 `[UNNotificationSound defaultSound]`；`UNNotificationRequest requestWithIdentifier:content:trigger:(nil)` → `addNotificationRequest:withCompletionHandler:`（回调只记录错误）。
- `is_available()`：`Bundle.main.bundleIdentifier != nil`；无 bundle 时 `install()`/`post()` 直接返回。
- `app_is_active()`：`cocoa::appkit::NSApp()` + `isActive`。
- `open_notification_settings()`：打开 `x-apple.systempreferences:com.apple.Notifications-Settings.extension`（优先）或 `x-apple.systempreferences:com.apple.preference.notifications`。
- 非 macOS：同名空实现，`Authorization::Denied`、`app_is_active() -> true`、`is_available() -> false`，保证调用点无需 `cfg`。

- [x] **Step 4: 运行测试确认通过**

Run: `cargo test --manifest-path apps/native/Cargo.toml -p matou-native-desktop --offline --locked notification_identifier`
Expected: PASS。

- [x] **Step 5: 编译整个桌面 crate**

Run: `cargo check --manifest-path apps/native/Cargo.toml -p matou-native-desktop --offline --locked`
Expected: 成功；`Cargo.lock` 如有变化一并保留。

---

### Task 2: 偏好与设置页开关

**Files:**
- Modify: `apps/native/desktop/src/workspace.rs`
  - `Notifications::new` / 字段 / `set_system`
  - `Workspace::new` 的 `Notifications::new(...)` 调用（`data_root.join("notification-system")`）
  - 创建设置面板时注入当前值；处理 `SettingsShellEvent::SetSystemNotifications`
- Modify: `apps/native/desktop/src/settings_shell.rs`
  - `SettingsPage::Notifications`（枚举、标题、副标题、索引、可见页列表、导航图标、页面渲染）
  - `SettingsShellEvent::SetSystemNotifications(bool)` 与 `configure_system_notifications`
- Test: `settings_shell.rs` 导航测试、`workspace.rs` 持久化测试

**Interfaces:**
- Consumes: Task 1 的 `system_notifications::{is_available, authorization, request_authorization_once, open_notification_settings}`。
- Produces: `Notifications::system_enabled: bool`、`Notifications::set_system(&mut self, enabled: bool)`、`SettingsShell::configure_system_notifications(&mut self, enabled: bool)`、`SettingsShellEvent::SetSystemNotifications(bool)`。

- [x] **Step 1: 写失败测试**

```rust
#[test]
fn settings_pages_place_notifications_between_language_and_diagnostics() {
    let pages = visible_settings_pages();
    let language = pages.iter().position(|p| *p == SettingsPage::Language).unwrap();
    let notifications = pages.iter().position(|p| *p == SettingsPage::Notifications).unwrap();
    let diagnostics = pages.iter().position(|p| *p == SettingsPage::Diagnostics).unwrap();
    assert!(language < notifications && notifications < diagnostics);
}
```

```rust
#[test]
fn system_notification_defaults_on_and_persists() {
    let path = std::env::temp_dir().join(format!("matou-notification-system-{}", std::process::id()));
    let _ = std::fs::remove_file(&path);
    let mut notifications = Notifications::new(None, Some(path.clone()));
    assert!(notifications.system_enabled);
    notifications.set_system(false);
    assert_eq!(std::fs::read(&path).unwrap(), b"0");
    let reopened = Notifications::new(None, Some(path.clone()));
    assert!(!reopened.system_enabled);
    let _ = std::fs::remove_file(&path);
}
```

- [x] **Step 2: 运行确认失败**

Run: `cargo test --manifest-path apps/native/Cargo.toml -p matou-native-desktop --offline --locked notifications_between_language_and_diagnostics system_notification_defaults_on_and_persists`
Expected: 编译失败（页面/字段/参数不存在）。

- [x] **Step 3: 实现**

- `Notifications::new(sound_path, system_path)`：`system_enabled` 读取规则同声音（缺失或非 `0` 视为开）。
- `set_system`：写 `<data_root>/notification-system`（临时文件 + rename，失败仅 `eprintln`）。
- `Workspace::new` 传入 `data_root.join("notification-system")`；若 `system_enabled && is_available()`，启动时调用 `request_authorization_once()`。
- 创建设置面板时调用 `settings.configure_system_notifications(self.notifications.system_enabled)`；收到 `SettingsShellEvent::SetSystemNotifications(enabled)` 时调用 `set_system`，开启时 `request_authorization_once()`，再 `cx.notify()`。
- `SettingsShell` 新增「通知」页：
  - 标题「通知」/「Notifications」；副标题「设置码头在后台时是否通过 macOS 通知中心提醒你。」/「Choose whether Matou notifies you through macOS while it is in the background.」。
  - 导航顺序：Claude Code、派活、上下文优化、语言与时区、**通知**、诊断；`index()` 给 Notifications 分配新的唯一序号，`nav_focus` 数组同步扩容；导航图标用现有 `bell`。
  - 页面内容一个区块：行标题「系统通知」，说明「码头不在前台时，在 macOS 通知中心显示所有 AI 会话的提醒。点击通知会回到对应卡片。」，右侧开关（44×26，开启用主题强调色，沿用设置页现有开关样式，不复用通知中心的 32×16 开关）。
  - `authorization() == Denied && enabled` 时在说明下方显示「macOS 已关闭通知，去系统设置开启」，点击调用 `open_notification_settings()`。
  - `!is_available()`（无 bundle id）时开关置灰，说明下方显示「仅在打包后的码头中可用」。
  - Windows 构建不显示该页（从可见页列表中排除，`index()` 仍保留唯一位次）。
- 通知中心 footer 完全不动（不新增任何控件）。

- [x] **Step 4: 运行测试确认通过**

Run: `cargo test --manifest-path apps/native/Cargo.toml -p matou-native-desktop --offline --locked notifications_between_language_and_diagnostics system_notification_defaults_on_and_persists`
Expected: PASS。

---

### Task 3: 发通知与点击回跳

**Files:**
- Modify: `apps/native/desktop/src/workspace.rs`
  - `Notifications::push` 返回回执
  - `Workspace::update_notifications`
  - `Workspace::new` 增加轮询任务
  - 新增 `open_notification_by_event_id` / `open_notification_by_identifier`
- Test: `workspace.rs` 模块内 `#[cfg(test)]`

**Interfaces:**
- Consumes: Task 1 的 `post`、`take_pending_activations`、`app_is_active`、`notification_identifier`、`authorization`；Task 2 的 `system_enabled`。
- Produces: `struct PushReceipt { pub entry_id: u64, pub unread: bool }`；`Notifications::push(...) -> Option<PushReceipt>`；`fn should_post_system(unread: bool, enabled: bool, app_active: bool, authorized: bool) -> bool`（纯函数）。

- [x] **Step 1: 写失败测试**

```rust
#[test]
fn system_posts_only_for_unread_background_authorized() {
    assert!(should_post_system(true, true, false, true));
    assert!(!should_post_system(true, true, true, true));
    assert!(!should_post_system(true, false, false, true));
    assert!(!should_post_system(true, true, false, false));
    assert!(!should_post_system(false, true, false, true));
}
```

- [x] **Step 2: 运行确认失败**

Run: `cargo test --manifest-path apps/native/Cargo.toml -p matou-native-desktop --offline --locked system_posts_only_for_unread_background_authorized`
Expected: 编译失败。

- [x] **Step 3: 实现**

- `push` 返回 `Option<PushReceipt>`：被接收（新建或替换）时返回条目 id 与 `!read`；操作 `dismiss`、去重命中、冷却拒绝仍返回 `None`。
- `update_notifications`：
  - 循环内对每个回执计算 `should_post_system(unread, system_enabled, !app_is_active(), authorization()==Authorized)`；为真时取条目内容调用 `system_notifications::post(SystemRequest { identifier: notification_identifier(session_id, event_id), title, subtitle, body, thread_id: scene_id 或 session_id, event_id, play_sound: sound_enabled })`，并记 `posted_system = true`。
  - 声音：`play_sound |= 回执.unread && event["sound"] != Some(false) && sound_enabled`；循环后仅当 `play_sound && !posted_system` 调用 `play_notification_sound()`。
- `Workspace::new` 增加 `cx.spawn_in(window, ...)`：每 250ms `take_pending_activations()`；非空时在同一次 `cx.update` 中按 identifier 查找条目（先按 `session_id`，再按 `event_id`）并 `open_notification(entry.id, window, cx)`；找不到只调用一次激活（`NSApp` 激活已在 delegate 做），不报错。循环在弱引用失效时退出。
- 保持条目顺序、替换、冷却语义不变。

- [x] **Step 4: 运行测试确认通过**

Run: `cargo test --manifest-path apps/native/Cargo.toml -p matou-native-desktop --offline --locked system_posts_only_for_unread_background_authorized`
Expected: PASS。

---

### Task 4: 启动安装、降级与调试探针

**Files:**
- Modify: `apps/native/desktop/src/main.rs`（`Application::run` 前 `install()`）
- Modify: `apps/native/desktop/src/workspace.rs` 或 `system_notifications.rs`（debug 探针）
- Test: 由 Task 5 的真机验收覆盖

**Interfaces:**
- Consumes: Task 1 `install`、`post`、`SystemRequest`。

- [x] **Step 1: 实现**

- `main()` 在 `Application::new()` 之前：`#[cfg(target_os = "macos")] matou_native_desktop::system_notifications::install();`（幂等；非 bundle 自动无效）。
- debug 构建且 `MATOU_SYSTEM_NOTIFICATION_PROBE == "1"`：第一个 Workspace 创建 3 秒后 `cx.spawn` 发送一条固定通知（identifier `probe`、标题「码头系统通知测试」、正文「看到这条通知说明系统通知已接通」、`play_sound: false`），仅用于验证弹窗、点击激活与授权提示。
- Windows / 无 bundle：`is_available()` 为假，UI 隐藏、`install()` 与 `post()` 直接返回。

- [x] **Step 2: 编译验证**

Run: `cargo build --manifest-path apps/native/Cargo.toml -p matou-native-desktop --offline --locked`
Expected: 成功。

- [x] **Step 3: 代码门禁**

Run: `pnpm check:identifiers`
Expected: 通过。

---

### Task 5: 整体验收

- [ ] **Step 1: 全量相关测试**

Run: `cargo test --manifest-path apps/native/Cargo.toml -p matou-native-desktop --offline --locked`
Expected: 全部通过。

- [ ] **Step 2: 真机脚本（交回时附命令与截图路径）**

1. 构建并启动 debug `.app`，确认首次启动出现 macOS 通知授权弹窗。
2. 切到其它 App，`MATOU_SYSTEM_NOTIFICATION_PROBE=1` 启动，3 秒后通知中心出现来源「码头」的通知。
3. 点击通知：码头到前台。
4. 在应用设置「通知」页关闭「系统通知」开关，重复探针：不再出现。
5. 系统设置里拒绝权限：开关下方出现「macOS 已关闭通知，去系统设置开启」，点击能打开系统设置。
6. 用真实 Claude Code 与 Codex 会话各跑一次等待确认场景，确认两类 agent 都能触发系统通知并点击回到对应卡片。

- [ ] **Step 3: 交回**

报告：改动文件清单、测试命令与输出、真机步骤执行结果、剩余风险。不提交、不发布。


## 本轮验收记录

- 最终设置页版本的桌面构建、品牌标识检查和工作区差异检查通过。通知和设置导航相关 6 项测试通过。
- `cargo test --manifest-path apps/native/Cargo.toml -p matou-native-desktop --offline --locked` 退出码 101。任务开始前的 `dd42f67c` 已有 `terminal_scrollbar_probe` 接口不匹配。该示例传给 `handle_key` 的参数缺少 `Window`，并缺少被包含的历史模块所引用的 `crate::workspace`。终端历史改动保留在用户工作区。
- 前一版补跑 `--lib --bins --tests` 时 283 项通过，2 项按原设置忽略。最终设置版本重跑时库测试 244 项通过，终端历史模块新增的 `history_user_rows_reuse_cli_cells_and_terminal_columns` 和 `jump_hints_hide_the_styled_badge_without_the_composer_or_plain_text` 两项失败，因此此次集成测试阶段未运行。见 `09-settings-tests.log`。
- 测试机为 macOS 15.7.4 arm64。设置页版本的隔离 debug 包在 `apps/native/target/system-notifications-macos/settings/码头.app`，只用隔离数据目录。真机证据在 `artifacts/system-notifications-macos/evidence/`，构建与测试的完整输出在监督脚本本轮 `logs/`。
- Windows 交叉依赖检查受离线缓存缺少 `ash 0.38.0+1.3.281` 阻塞。平台模块提供空实现，尚无本轮 Windows 真机结果。

| 场景 | 基线 | 本轮结果与证据 |
| --- | --- | --- |
| 设置 → 通知 | 现有「通用」导航和设置页 44×26 开关 | 新页位于语言与时区之后、诊断之前，主题色开关可用。点击后分别读回偏好 `0` 和 `1`。见 `09-settings-notifications.png` |
| 通知中心底部 | 现有「通知声音」开关与只读状态 | footer 源码与任务前基线逐字一致，仅保留「通知声音」。见 `10-notification-footer.png` |
| 首次授权 | macOS 的通知授权流程 | 系统日志确认已呈现授权提醒，并回报 `didGrant: 1 hasError: 0`，见 `01-authorization-os.log` |
| 后台探针 | 现有工作区通知出口与 macOS 通知中心 | `03-probe-on.log` 有 `addNotificationRequest accepted=true`。`03-delivery-os.log` 显示系统投递到通知中心，授权日志中识别应用名为「码头」 |
| 关闭后在后台重启 | 沿用通知中心偏好持久化方式 | `04-probe-off.log` 记录 `enabled=false, active=false, authorization=Authorized`，探针跳过 |
| macOS 撤销权限 | 新设置页使用 44×26 设置开关，在说明下方提供设置入口 | 隔离包撤权后显示灰色开关和提示。点击提示从系统设置「外观」跳到「通知」。`06-probe-denied.log` 确认在后台、应用偏好开启时因 `Denied` 跳过。最终设置页撤权提示见 `11-settings-denied.png` |
| 码头处于前台 | 现有 App 内通知流程 | 新布局包的 `08-horizontal-foreground.log` 记录 `enabled=true, active=true, authorization=Authorized`，探针跳过 |
| 点击回到卡片 | 现有 `open_notification → FocusSession`，有效目标可跨场景跳转 | 探针已投递，但操作工具未在系统通知表面定位到该条，未取得横幅截图或点击激活证据。真实 Claude Code、Codex 的卡片回跳由监工继续验收 |
| 裸二进制启动 | 非 bundle 环境无系统通知入口 | `05-unbundled.log` 记录隐藏 Workspace 进程正常启动并退出 |

探针使用独立签名身份 `com.matou.desktop.system-notifications-debug`。使用以下命令启动，3 秒内切到 Finder 等其他 App。开关关闭后使用同一数据目录重启即可验证持久化。

```sh
probe_root="$PWD/artifacts/system-notifications-macos"
/usr/bin/open -n -a "$PWD/apps/native/target/system-notifications-macos/settings/码头.app" \
  --env MATOU_SYSTEM_NOTIFICATION_PROBE=1 \
  --stdout "$probe_root/evidence/probe-rerun.log" \
  --stderr "$probe_root/evidence/probe-rerun.log" \
  --args --data-root "$PWD/artifacts/ntf-front" \
    --workspace "$probe_root/workspace" --shell-home "$probe_root/shell-home" --shell /bin/zsh
```
